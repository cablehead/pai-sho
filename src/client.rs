//! CLI client - sends commands to daemon over Unix socket (AF_UNIX on Windows too).

use crate::protocol::{Request, Response, VERSION};
#[cfg(windows)]
use crate::uds::UnixStream;
use crate::Command;
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
#[cfg(unix)]
use tokio::net::UnixStream;

/// The socket of the daemon to talk to, when `--socket` doesn't say: your own
/// daemon's, next to its key, else the macOS service's. When neither exists
/// yet, your own, so the error names where it looked first.
pub fn find_socket() -> PathBuf {
    let own = crate::daemon::default_socket_path();
    if own.exists() {
        return own;
    }
    #[cfg(target_os = "macos")]
    {
        let service = std::path::Path::new(crate::launchd::SOCKET);
        if service.exists() {
            return service.to_path_buf();
        }
    }
    own
}

pub async fn send_command(socket_path: &Path, command: Command) -> Result<()> {
    let qr = matches!(command, Command::Invite { qr: true, .. });
    let request = match command {
        Command::Invite {
            key, name, expose, ..
        } => Request::Invite { key, name, expose },
        Command::Accept { handle, name } => Request::Accept { handle, name },
        Command::Forget { peer } => Request::Forget { peer },
        Command::Rename { peer, name } => Request::Rename { peer, name },
        Command::Expose { port, to, all } => Request::Expose { port, to, all },
        Command::Unexpose { port, to } => Request::Unexpose { port, to },
        Command::List => Request::List,
        Command::Key => Request::Key,
        Command::Project { peer, ip, name } => Request::Project {
            peer,
            ip: ip.map(|ip| ip.to_string()),
            name,
        },
        Command::Unproject { peer } => Request::Unproject { peer },
        Command::Daemon { .. } => unreachable!("daemon handled separately"),
    };

    let stream = UnixStream::connect(socket_path)
        .await
        .context("failed to connect to daemon - is it running?")?;

    let (reader, mut writer) = tokio::io::split(stream);
    let mut reader = BufReader::new(reader);

    // Send request
    let request_json = serde_json::to_string(&request)?;
    writer.write_all(request_json.as_bytes()).await?;
    writer.write_all(b"\n").await?;

    // Read response
    let mut line = String::new();
    reader.read_line(&mut line).await?;
    let response: Response = serde_json::from_str(&line)?;

    // Print response
    match response {
        Response::Ok => println!("OK"),
        Response::Key(key) => println!("{}", key),
        Response::Invite(invite) => {
            if qr {
                print!("{}", crate::qr::render(&invite)?);
            }
            println!("{}", invite);
        }
        Response::List(mut info) => {
            info.cli = VERSION.to_string();
            println!("{}", serde_json::to_string_pretty(&info)?);
        }
        Response::Error(e) => {
            eprintln!("Error: {}", e);
            std::process::exit(1);
        }
    }

    Ok(())
}
