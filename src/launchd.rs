//! macOS launchd service for `daemon install`, `up`, `down`, `remove`, and `status`.
//!
//! Same shape as the Homebrew formula. A root LaunchDaemon runs an operator
//! script. The script writes `/etc/resolver/pai-sho`, then execs the daemon
//! with `--tun utun` and `--socket-owner` set to the console user. The key
//! is `/usr/local/var/pai-sho/op.key`. `remove` leaves that key in place.

use crate::protocol::{Request, Response};
use anyhow::{bail, Context, Result};
use std::ffi::CString;
use std::fs;
use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

const LABEL: &str = "pai-sho";
const PLIST: &str = "/Library/LaunchDaemons/pai-sho.plist";
const BIN: &str = "/usr/local/libexec/pai-sho";
const OPERATOR: &str = "/usr/local/libexec/pai-sho-operator";
const KEY: &str = "/usr/local/var/pai-sho/op.key";
const LOG: &str = "/usr/local/var/log/pai-sho-operator.log";
const RESOLVER: &str = "/etc/resolver/pai-sho";
const SOCKET: &str = "/tmp/pai-sho.sock";
const RESOLVER_ADDR: &str = "10.99.0.53";

pub fn install() -> Result<()> {
    prepare()?;
    ensure_dirs()?;
    install_binary()?;
    write_root_file(Path::new(OPERATOR), &operator_script(), 0o755)?;
    write_root_file(Path::new(PLIST), &plist(), 0o644)?;
    // Re-read the plist so a second install picks up a new binary.
    start(true)?;
    println!("{}", install_message(wait_for_daemon().as_ref()));
    Ok(())
}

pub fn up() -> Result<()> {
    prepare()?;
    if !Path::new(PLIST).exists() {
        bail!("pai-sho is not installed. Run `pai-sho daemon install`");
    }
    let restarting = service_loaded();
    start(false)?;
    if restarting {
        println!("pai-sho restarted.");
    } else {
        println!("pai-sho is running.");
    }
    Ok(())
}

pub fn down() -> Result<()> {
    prepare()?;
    if !Path::new(PLIST).exists() && !service_loaded() {
        bail!("pai-sho is not installed. Run `pai-sho daemon install`");
    }
    let domain = domain();
    if service_loaded() {
        launchctl(&["bootout", &domain])?;
    }
    // A plist in /Library/LaunchDaemons loads again at boot. disable keeps
    // `down` down across a reboot. `up` clears it.
    launchctl(&["disable", &domain])?;
    println!("pai-sho is stopped. It stays stopped until `pai-sho daemon up`.");
    Ok(())
}

pub fn remove() -> Result<()> {
    prepare()?;
    let domain = domain();
    if service_loaded() {
        launchctl(&["bootout", &domain])?;
    }
    // `down` disables the label. Clear that before deleting the plist, or a
    // later install stays disabled.
    let _ = launchctl(&["enable", &domain]);
    for path in [PLIST, OPERATOR, BIN, RESOLVER, LOG] {
        remove_file(Path::new(path))?;
    }
    if Path::new(KEY).exists() {
        println!("Service removed. Kept the key at {KEY}");
    } else {
        println!("Service removed.");
    }
    Ok(())
}

pub fn status() -> Result<()> {
    require_macos()?;
    println!("{}", serde_json::to_string_pretty(&report(&gather()?))?);
    Ok(())
}

fn prepare() -> Result<()> {
    require_macos()?;
    ensure_root()
}

fn require_macos() -> Result<()> {
    if cfg!(target_os = "macos") {
        Ok(())
    } else {
        bail!("daemon install, up, down, remove, and status run on macOS");
    }
}

/// Re-exec under sudo when the caller is not root. One try: if sudo returns
/// and we are still unprivileged, stop rather than prompting again.
fn ensure_root() -> Result<()> {
    if is_root() {
        return Ok(());
    }
    if std::env::var_os("PAI_SHO_ELEVATING").is_some() {
        bail!("this command needs root");
    }
    let exe = std::env::current_exe().context("finding this pai-sho")?;
    let status = Command::new("/usr/bin/sudo")
        .arg("--preserve-env=PAI_SHO_ELEVATING")
        .arg("--")
        .arg(&exe)
        .args(std::env::args_os().skip(1))
        .env("PAI_SHO_ELEVATING", "1")
        .status()
        .context("running sudo")?;
    std::process::exit(status.code().unwrap_or(1));
}

fn is_root() -> bool {
    // geteuid has no preconditions.
    unsafe { libc::geteuid() == 0 }
}

fn domain() -> String {
    format!("system/{LABEL}")
}

struct InstallView {
    key: String,
    peers: usize,
}

/// Ask the daemon who it is. The job is up before the socket is listening,
/// so try for a few seconds and then let install finish anyway.
fn wait_for_daemon() -> Option<InstallView> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match fetch_view() {
            Ok(view) => return Some(view),
            Err(_) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(100)),
            Err(_) => return None,
        }
    }
}

fn fetch_view() -> Result<InstallView> {
    let mut stream = UnixStream::connect(SOCKET).context("connecting to the daemon")?;
    stream
        .set_read_timeout(Some(Duration::from_secs(1)))
        .context("setting the read timeout")?;
    let request = serde_json::to_string(&Request::List)?;
    writeln!(stream, "{request}").context("writing the list request")?;
    let mut line = String::new();
    BufReader::new(stream)
        .read_line(&mut line)
        .context("reading the list response")?;
    match serde_json::from_str(&line)? {
        Response::List(info) => Ok(InstallView {
            key: info.me,
            peers: info.peers.len(),
        }),
        Response::Error(e) => bail!("{e}"),
        _ => bail!("unexpected response from the daemon"),
    }
}

fn install_message(view: Option<&InstallView>) -> String {
    let Some(view) = view else {
        return "pai-sho is running. Not answering yet. Run `pai-sho key`.".to_string();
    };

    if view.peers == 0 {
        return format!(
            "\
pai-sho is running.

This machine's key:

  {key}",
            key = view.key
        );
    }

    let peers = if view.peers == 1 {
        "1 peer".to_string()
    } else {
        format!("{} peers", view.peers)
    };
    format!("pai-sho is running. {peers}.")
}

/// Copy this binary into `/usr/local/libexec` and give it to root.
/// The LaunchDaemon execs that copy. A user-writable path would run as root
/// on the next start. `eget` updates `~/bin`; `daemon install` refreshes the copy.
fn install_binary() -> Result<()> {
    let src = std::env::current_exe().context("finding this pai-sho")?;
    let dest = Path::new(BIN);
    if let Some(dir) = dest.parent() {
        fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    let tmp = dest.with_file_name(".pai-sho.installing");
    fs::copy(&src, &tmp)
        .with_context(|| format!("copying {} to {}", src.display(), tmp.display()))?;
    if let Err(e) = place_binary(&tmp, dest) {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(())
}

fn place_binary(tmp: &Path, dest: &Path) -> Result<()> {
    fs::set_permissions(tmp, fs::Permissions::from_mode(0o755))
        .with_context(|| format!("chmod {}", tmp.display()))?;
    chown_root(tmp)?;
    fs::rename(tmp, dest).with_context(|| format!("replacing {}", dest.display()))?;
    Ok(())
}

fn ensure_dirs() -> Result<()> {
    for dir in [
        "/usr/local/libexec",
        "/usr/local/var/log",
        "/usr/local/var/pai-sho",
    ] {
        fs::create_dir_all(dir).with_context(|| format!("creating {dir}"))?;
    }
    Ok(())
}

fn write_root_file(path: &Path, contents: &str, mode: u32) -> Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, contents).with_context(|| format!("writing {}", tmp.display()))?;
    fs::set_permissions(&tmp, fs::Permissions::from_mode(mode))
        .with_context(|| format!("chmod {}", tmp.display()))?;
    chown_root(&tmp)?;
    fs::rename(&tmp, path).with_context(|| format!("replacing {}", path.display()))?;
    Ok(())
}

fn chown_root(path: &Path) -> Result<()> {
    // root:wheel. wheel is gid 0 on macOS.
    let c = CString::new(path.as_os_str().as_bytes()).context("path contains a nul")?;
    let rc = unsafe { libc::chown(c.as_ptr(), 0, 0) };
    if rc != 0 {
        bail!("chown {}: {}", path.display(), io::Error::last_os_error());
    }
    Ok(())
}

fn remove_file(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e).with_context(|| format!("removing {}", path.display())),
    }
}

/// `reload` bootstraps a freshly written plist. Otherwise a loaded service is
/// kickstarted, which re-runs the operator script and chowns the socket to
/// the console user logged in now.
fn start(reload: bool) -> Result<()> {
    let domain = domain();
    // enable first. bootstrap refuses a label that `down` disabled.
    launchctl(&["enable", &domain])?;
    let loaded = service_loaded();
    if loaded && !reload {
        launchctl(&["kickstart", "-k", &domain])?;
        return Ok(());
    }
    if loaded {
        launchctl(&["bootout", &domain])?;
        wait_unloaded();
    }
    launchctl(&["bootstrap", "system", PLIST])?;
    Ok(())
}

/// `bootout` returns before the old daemon has gone. A `bootstrap` while it's
/// still going fails with "5: Input/output error" and leaves nothing running,
/// so wait, up to 10 seconds, for launchd to drop it.
fn wait_unloaded() {
    for _ in 0..50 {
        if !service_loaded() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
}

fn service_loaded() -> bool {
    Command::new("launchctl")
        .args(["print", &domain()])
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false)
}

struct ServiceStatus {
    installed: bool,
    loaded: bool,
    disabled: bool,
    launchd_state: Option<String>,
    pid: Option<String>,
    socket_owner: Option<String>,
    key_present: bool,
    resolver_present: bool,
}

fn gather() -> Result<ServiceStatus> {
    let installed = Path::new(PLIST).exists();
    let disabled_text = launchctl_stdout(&["print-disabled", "system"])?;
    let disabled = label_disabled(&disabled_text, LABEL);
    let printed = launchctl_output(&["print", &domain()])?;
    let loaded = if printed.status.success() {
        true
    } else {
        let err = String::from_utf8_lossy(&printed.stderr);
        if err.contains("Could not find service") {
            false
        } else {
            bail!("launchctl print {} failed: {}", domain(), err.trim());
        }
    };
    let text = String::from_utf8_lossy(&printed.stdout);
    Ok(ServiceStatus {
        installed,
        loaded,
        disabled,
        launchd_state: loaded
            .then(|| field(&text, "state"))
            .flatten()
            .map(str::to_string),
        pid: loaded
            .then(|| field(&text, "pid"))
            .flatten()
            .map(str::to_string),
        socket_owner: socket_owner(Path::new(SOCKET)),
        key_present: Path::new(KEY).exists(),
        resolver_present: Path::new(RESOLVER).exists(),
    })
}

#[derive(serde::Serialize)]
struct StatusReport {
    service: String,
    state: String,
    pid: Option<u32>,
    disabled: bool,
    installed: bool,
    loaded: bool,
    socket: Option<String>,
    socket_owner: Option<String>,
    key: Option<String>,
    resolver: Option<String>,
    log: String,
}

fn report(status: &ServiceStatus) -> StatusReport {
    StatusReport {
        service: domain(),
        state: state_word(status).to_string(),
        pid: status.pid.as_deref().and_then(|pid| pid.parse().ok()),
        disabled: status.disabled,
        installed: status.installed,
        loaded: status.loaded,
        socket: status.socket_owner.as_ref().map(|_| SOCKET.to_string()),
        socket_owner: status.socket_owner.clone(),
        key: status.key_present.then(|| KEY.to_string()),
        resolver: status.resolver_present.then(|| RESOLVER.to_string()),
        log: LOG.to_string(),
    }
}

fn state_word(status: &ServiceStatus) -> &str {
    if status.loaded {
        return status.launchd_state.as_deref().unwrap_or("loaded");
    }
    if status.installed {
        "stopped"
    } else {
        "not installed"
    }
}

/// `"label" => disabled` in `launchctl print-disabled system`.
fn label_disabled(text: &str, label: &str) -> bool {
    let needle = format!("\"{label}\" =>");
    text.lines().any(|line| {
        let Some(rest) = line.trim().strip_prefix(&needle) else {
            return false;
        };
        rest.split_whitespace().next() == Some("disabled")
    })
}

fn field<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    let prefix = format!("{key} = ");
    text.lines().find_map(|line| {
        let value = line.trim().strip_prefix(&prefix)?;
        let value = value.trim();
        (!value.is_empty()).then_some(value)
    })
}

fn socket_owner(path: &Path) -> Option<String> {
    use std::os::unix::fs::MetadataExt;
    let uid = fs::metadata(path).ok()?.uid();
    // getpwuid has no preconditions. A null result means the uid has no name.
    let pw = unsafe { libc::getpwuid(uid) };
    if pw.is_null() {
        return Some(uid.to_string());
    }
    let name = unsafe { std::ffi::CStr::from_ptr((*pw).pw_name) };
    Some(name.to_string_lossy().into_owned())
}

fn launchctl_output(args: &[&str]) -> Result<std::process::Output> {
    Command::new("launchctl")
        .args(args)
        .output()
        .with_context(|| format!("running launchctl {}", args.join(" ")))
}

fn launchctl_stdout(args: &[&str]) -> Result<String> {
    let out = launchctl_output(args)?;
    if out.status.success() {
        return Ok(String::from_utf8_lossy(&out.stdout).into_owned());
    }
    let err = String::from_utf8_lossy(&out.stderr);
    let err = err.trim();
    if err.is_empty() {
        bail!("launchctl {} failed: exit {}", args.join(" "), out.status);
    }
    bail!("launchctl {} failed: {err}", args.join(" "));
}

fn launchctl(args: &[&str]) -> Result<()> {
    let out = Command::new("launchctl")
        .args(args)
        .output()
        .with_context(|| format!("running launchctl {}", args.join(" ")))?;
    if out.status.success() {
        return Ok(());
    }
    let mut msg = String::from_utf8_lossy(&out.stderr).trim().to_string();
    if msg.is_empty() {
        msg = String::from_utf8_lossy(&out.stdout).trim().to_string();
    }
    if msg.is_empty() {
        msg = format!("exit {}", out.status);
    }
    bail!("launchctl {} failed: {msg}", args.join(" "));
}

fn operator_script() -> String {
    format!(
        r#"#!/bin/bash
# Rewritten by `pai-sho daemon install`.
set -eu
mkdir -p /etc/resolver
printf 'nameserver {addr}\n' > {resolver}
KEY="{key}"
mkdir -p "$(dirname "$KEY")"
u=$(stat -f%Su /dev/console) || exit 1
if [ -z "$u" ]; then
  echo "pai-sho: no user on /dev/console" >&2
  exit 1
fi
exec {bin} --socket {socket} daemon \
  --key "$KEY" --tun utun --socket-owner "$u"
"#,
        addr = RESOLVER_ADDR,
        resolver = RESOLVER,
        key = KEY,
        bin = BIN,
        socket = SOCKET,
    )
}

fn plist() -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{label}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{operator}</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>StandardOutPath</key>
    <string>{log}</string>
    <key>StandardErrorPath</key>
    <string>{log}</string>
    <key>EnvironmentVariables</key>
    <dict>
        <key>PATH</key>
        <string>/usr/bin:/bin:/usr/sbin:/sbin</string>
    </dict>
</dict>
</plist>
"#,
        label = LABEL,
        operator = OPERATOR,
        log = LOG,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operator_script_matches_the_service_settings() {
        let script = operator_script();
        for line in [
            "#!/bin/bash",
            "set -eu",
            "mkdir -p /etc/resolver",
            "printf 'nameserver 10.99.0.53\\n' > /etc/resolver/pai-sho",
            "KEY=\"/usr/local/var/pai-sho/op.key\"",
            "u=$(stat -f%Su /dev/console) || exit 1",
            "exec /usr/local/libexec/pai-sho --socket /tmp/pai-sho.sock daemon \\",
            "  --key \"$KEY\" --tun utun --socket-owner \"$u\"",
        ] {
            assert!(
                script.lines().any(|got| got == line),
                "missing {line:?} in:\n{script}"
            );
        }
    }

    #[test]
    fn plist_is_a_keepalive_launch_daemon() {
        let text = plist();
        for line in [
            "    <string>pai-sho</string>",
            "        <string>/usr/local/libexec/pai-sho-operator</string>",
            "    <key>RunAtLoad</key>",
            "    <true/>",
            "    <key>KeepAlive</key>",
            "    <string>/usr/local/var/log/pai-sho-operator.log</string>",
            "        <string>/usr/bin:/bin:/usr/sbin:/sbin</string>",
        ] {
            assert!(
                text.lines().any(|got| got == line),
                "missing {line:?} in:\n{text}"
            );
        }
    }

    #[test]
    fn status_reads_launchd_text() {
        let disabled = "\
disabled services = {
\t\"com.apple.ftpd\" => disabled
\t\"pai-sho\" => disabled
\t\"pai-sho-extra\" => enabled
}
";
        assert!(label_disabled(disabled, "pai-sho"));
        assert!(!label_disabled(disabled, "pai-sho-extra"));
        assert!(!label_disabled(disabled, "com.apple.ftpd.missing"));

        let printed = "\
system/pai-sho = {
\tstate = running
\tpid = 439
\tpath = /Library/LaunchDaemons/pai-sho.plist
}
";
        assert_eq!(field(printed, "state"), Some("running"));
        assert_eq!(field(printed, "pid"), Some("439"));
        assert_eq!(field(printed, "program"), None);
    }

    #[test]
    fn status_json_for_each_state() {
        let running = serde_json::to_value(report(&ServiceStatus {
            installed: true,
            loaded: true,
            disabled: false,
            launchd_state: Some("running".to_string()),
            pid: Some("439".to_string()),
            socket_owner: Some("ndyg".to_string()),
            key_present: true,
            resolver_present: true,
        }))
        .unwrap();
        assert_eq!(
            running,
            serde_json::json!({
                "service": "system/pai-sho",
                "state": "running",
                "pid": 439,
                "disabled": false,
                "installed": true,
                "loaded": true,
                "socket": "/tmp/pai-sho.sock",
                "socket_owner": "ndyg",
                "key": "/usr/local/var/pai-sho/op.key",
                "resolver": "/etc/resolver/pai-sho",
                "log": "/usr/local/var/log/pai-sho-operator.log"
            })
        );

        let stopped = serde_json::to_value(report(&ServiceStatus {
            installed: true,
            loaded: false,
            disabled: true,
            launchd_state: None,
            pid: None,
            socket_owner: None,
            key_present: true,
            resolver_present: false,
        }))
        .unwrap();
        assert_eq!(stopped["state"], "stopped");
        assert_eq!(stopped["disabled"], true);
        assert_eq!(stopped["loaded"], false);
        assert!(stopped["socket"].is_null());
        assert!(stopped["socket_owner"].is_null());
        assert!(stopped["resolver"].is_null());
        assert_eq!(stopped["key"], "/usr/local/var/pai-sho/op.key");

        let missing = serde_json::to_value(report(&ServiceStatus {
            installed: false,
            loaded: false,
            disabled: false,
            launchd_state: None,
            pid: None,
            socket_owner: None,
            key_present: false,
            resolver_present: false,
        }))
        .unwrap();
        assert_eq!(missing["state"], "not installed");
        assert_eq!(missing["installed"], false);
        assert!(missing["pid"].is_null());
        assert!(missing["key"].is_null());

        let spawning = serde_json::to_value(report(&ServiceStatus {
            installed: true,
            loaded: true,
            disabled: false,
            launchd_state: Some("spawn scheduled".to_string()),
            pid: Some("nope".to_string()),
            socket_owner: None,
            key_present: false,
            resolver_present: false,
        }))
        .unwrap();
        assert_eq!(spawning["state"], "spawn scheduled");
        assert!(spawning["pid"].is_null());
    }

    #[test]
    fn install_message_introduces_a_new_machine() {
        let text = install_message(Some(&InstallView {
            key: "abc".to_string(),
            peers: 0,
        }));
        assert_eq!(
            text,
            "\
pai-sho is running.

This machine's key:

  abc"
        );
    }

    #[test]
    fn install_message_skips_the_intro_once_peers_exist() {
        assert_eq!(
            install_message(Some(&InstallView {
                key: "abc".to_string(),
                peers: 1,
            })),
            "pai-sho is running. 1 peer."
        );
        assert_eq!(
            install_message(Some(&InstallView {
                key: "abc".to_string(),
                peers: 2,
            })),
            "pai-sho is running. 2 peers."
        );
    }

    #[test]
    fn install_message_when_the_socket_is_late() {
        assert_eq!(
            install_message(None),
            "pai-sho is running. Not answering yet. Run `pai-sho key`."
        );
    }

    #[test]
    fn other_oses_are_refused_before_sudo() {
        if cfg!(target_os = "macos") {
            assert!(require_macos().is_ok());
            return;
        }
        let err = install().unwrap_err().to_string();
        assert!(err.contains("macOS"), "{err}");
    }
}
