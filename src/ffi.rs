//! C entry for an app that links `libpai_sho.a` and runs the process in the app.
//! One process per app. The socket is still opened so other tools can talk to
//! it. The iOS app does not have to use the socket. No tun is opened.

use std::ffi::{c_char, c_int, CStr};
use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;

static RUNNING: AtomicBool = AtomicBool::new(false);

fn c_str(ptr: *const c_char) -> Option<String> {
    if ptr.is_null() {
        return None;
    }
    Some(
        unsafe { CStr::from_ptr(ptr) }
            .to_string_lossy()
            .into_owned(),
    )
}

/// Start the process on a background thread. Returns 0 when the thread is
/// spawned, -1 when `host` or `socket_path` is missing or `host` is not an IP,
/// -2 when a process is already running. `key_path` may be null, in which case
/// the process uses its usual key path.
#[no_mangle]
pub extern "C" fn pai_sho_start(
    host: *const c_char,
    socket_path: *const c_char,
    key_path: *const c_char,
) -> c_int {
    let Some(host) = c_str(host) else {
        return -1;
    };
    let Some(socket_path) = c_str(socket_path) else {
        return -1;
    };
    let Ok(host) = host.parse::<IpAddr>() else {
        return -1;
    };
    let key_path = c_str(key_path).map(PathBuf::from);
    if RUNNING.swap(true, Ordering::SeqCst) {
        return -2;
    }
    let spawned = thread::Builder::new()
        .name("pai-sho".into())
        .spawn(move || {
            let runtime = match tokio::runtime::Runtime::new() {
                Ok(runtime) => runtime,
                Err(e) => {
                    tracing::error!("pai_sho_start runtime: {e}");
                    RUNNING.store(false, Ordering::SeqCst);
                    return;
                }
            };
            let result = runtime.block_on(crate::daemon::run(
                host,
                PathBuf::from(&socket_path).as_path(),
                Vec::new(),
                Vec::new(),
                key_path,
                None,
                None,
                None,
                None,
                None,
            ));
            if let Err(e) = result {
                tracing::error!("pai_sho_start: {e}");
            }
            RUNNING.store(false, Ordering::SeqCst);
        });
    match spawned {
        Ok(_) => 0,
        Err(e) => {
            tracing::error!("pai_sho_start thread: {e}");
            RUNNING.store(false, Ordering::SeqCst);
            -1
        }
    }
}

/// Ask the process started by `pai_sho_start` to stop.
#[no_mangle]
pub extern "C" fn pai_sho_stop() {
    crate::daemon::request_shutdown();
}
