//! Windows authenticated loopback endpoint discovery and listener ownership.
use std::io;
use std::path::PathBuf;
use std::sync::Arc;

use crate::host::SharedHost;

use super::{Services, serve_with};
use crate::daemon::transport;
use std::net::TcpListener;

pub const CONTROL_PORT_FILE: &str = "control.port";

pub fn control_endpoint_path() -> io::Result<PathBuf> {
    transport::port_path_named(CONTROL_PORT_FILE).ok_or_else(|| {
        io::Error::other("no config directory to record the control endpoint in")
    })
}

pub fn spawn_control_listener_with(
    host: SharedHost,
    services: Services,
) -> io::Result<PathBuf> {
    // See the unix arm: uptime is anchored where the listener opens, so a
    // GUI-hosted control server does not report itself as freshly started.
    super::server_started();
    let path = control_endpoint_path()?;
    // A control.port left by a dead daemon is stale: connecting to it
    // would pay the OS's refusal delay for nothing. The pidfile says
    // whether the daemon that wrote the port is still alive — when it is
    // gone the port cannot hold a live control server, and the bind below
    // simply overwrites the stale file.
    if !crate::daemon::spawn::recorded_daemon_is_dead() {
        if let Ok(live) = transport::connect_endpoint(CONTROL_PORT_FILE) {
            drop(live);
            return Err(io::Error::new(
                io::ErrorKind::AddrInUse,
                format!(
                    "a control server is already listening at {}",
                    transport::endpoint_display_named(CONTROL_PORT_FILE)
                ),
            ));
        }
    }
    let (listener, token) =
        transport::bind_endpoint(CONTROL_PORT_FILE).map_err(io::Error::other)?;
    std::thread::Builder::new()
        .name("lyrel-control-listener".into())
        .spawn(move || serve_listener_with(listener, token, host, services))?;
    Ok(path)
}

pub fn spawn_control_listener(host: SharedHost) -> io::Result<PathBuf> {
    spawn_control_listener_with(host, Services::none())
}

/// See the unix arm: a bind that failed only means "serve panes only" when
/// something else is actually answering there.
///
/// Gated on the pidfile the same way [`spawn_control_listener_with`] gates
/// its own probe, and for the same reason: a TCP connect that completes
/// proves only that *something* accepted on the recorded port, and a port a
/// dead daemon wrote can have been recycled by a stranger since. When the
/// daemon that wrote it is gone, nothing behind that port is ours.
pub fn control_endpoint_answers() -> bool {
    !crate::daemon::spawn::recorded_daemon_is_dead()
        && transport::connect_endpoint(CONTROL_PORT_FILE).is_ok()
}

pub fn serve_listener_with(
    listener: TcpListener,
    token: transport::Token,
    host: SharedHost,
    services: Services,
) {
    for stream in listener.incoming() {
        match stream {
            Ok(mut stream) => {
                transport::tune(&stream);
                let host = Arc::clone(&host);
                let services = services.clone();
                let spawned = std::thread::Builder::new()
                    .name("lyrel-control-conn".into())
                    .spawn(move || {
                        if let Err(e) = transport::check_endpoint_token(&mut stream, &token) {
                            log::warn!("control connection rejected: {e}");
                            return;
                        }
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

pub fn connect_control() -> io::Result<std::net::TcpStream> {
    transport::connect_endpoint(CONTROL_PORT_FILE)
}

pub fn remove_control_endpoint() {
    transport::remove_endpoint(CONTROL_PORT_FILE);
}

pub(super) fn control_endpoint_display() -> String {
    control_endpoint_path()
        .map(|p| p.display().to_string())
        .unwrap_or_default()
}
