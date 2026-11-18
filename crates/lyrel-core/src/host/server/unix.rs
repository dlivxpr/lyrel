//! Unix control listener: private binding and per-connection dispatch.
//!
//! The endpoint's address and liveness live wire-side in
//! `lyrel-protocol::daemon::control_endpoint`; this module owns the listener
//! that binds and serves it.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::host::SharedHost;

use super::{Services, serve_with};
use std::os::unix::fs::PermissionsExt as _;
use std::os::unix::net::{UnixListener, UnixStream};

pub fn bind_control_socket(path: &Path) -> io::Result<UnixListener> {
    let parent = path.parent().unwrap_or(Path::new("."));
    if !parent.exists() {
        std::fs::create_dir_all(parent)?;
        let _ = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700));
    }

    if path.exists() {
        match UnixStream::connect(path) {
            Ok(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::AddrInUse,
                    format!(
                        "a control server is already listening on {}",
                        path.display()
                    ),
                ));
            }
            Err(_) => {
                let _ = std::fs::remove_file(path);
            }
        }
    }

    let listener = bind_private(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(listener)
}

pub(super) fn bind_private(path: &Path) -> io::Result<UnixListener> {
    static UMASK: Mutex<()> = Mutex::new(());
    let _held = UMASK.lock().unwrap_or_else(|e| e.into_inner());

    let previous = unsafe { libc::umask(0o077) };
    let bound = UnixListener::bind(path);
    unsafe { libc::umask(previous) };
    bound
}

pub fn serve_listener(listener: UnixListener, host: SharedHost) {
    serve_listener_with(listener, host, Services::none())
}

pub fn serve_listener_with(listener: UnixListener, host: SharedHost, services: Services) {
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let host = Arc::clone(&host);
                let services = services.clone();
                let spawned = std::thread::Builder::new()
                    .name("lyrel-control-conn".into())
                    .spawn(move || {
                        if let Err(e) = serve_with(stream, host, services) {
                            log::warn!("control connection failed: {e}");
                        }
                    });
                if let Err(e) = spawned {
                    log::warn!("could not start a control connection thread: {e}");
                }
            }
            Err(e) => log::warn!("control accept failed: {e}"),
        }
    }
}

pub fn spawn_control_listener(host: SharedHost) -> io::Result<PathBuf> {
    spawn_control_listener_with(host, Services::none())
}

pub fn spawn_control_listener_with(host: SharedHost, services: Services) -> io::Result<PathBuf> {
    // Anchor uptime here, not at the first Status request. The GUI can host
    // the control listener too, and there `run_with` never runs — so
    // without this the OnceLock is first set by whoever asks, and every
    // answer reports a server that just started.
    super::server_started();
    let path = lyrel_protocol::daemon::control_endpoint::control_socket_path()?;
    let listener = bind_control_socket(&path)?;
    std::thread::Builder::new()
        .name("lyrel-control-listener".into())
        .spawn(move || serve_listener_with(listener, host, services))?;
    Ok(path)
}

pub(super) fn control_endpoint_display() -> String {
    lyrel_protocol::daemon::control_endpoint::control_endpoint_display()
}

#[cfg(test)]
use lyrel_protocol::daemon::control_endpoint::{CONTROL_SOCK_ENV, socket_path_in};

#[cfg(test)]
mod tests;
