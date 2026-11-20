//! Windows authenticated loopback control listener.
//!
//! The `control.port` discovery, token-authenticated connect, and removal
//! helpers live wire-side in `lyrel-protocol::daemon::control_endpoint`;
//! this module owns the listener that binds the port, checks the token on
//! every connection, and dispatches.

use std::io;
use std::path::PathBuf;
use std::sync::Arc;

use crate::host::SharedHost;

use super::{Services, serve_with};
use crate::daemon::transport;
use std::net::TcpListener;

pub fn spawn_control_listener_with(host: SharedHost, services: Services) -> io::Result<PathBuf> {
    // See the unix arm: uptime is anchored where the listener opens, so a
    // GUI-hosted control server does not report itself as freshly started.
    super::server_started();
    let path = lyrel_protocol::daemon::control_endpoint::control_endpoint_path()?;
    // A control.port left by a dead daemon is stale: connecting to it
    // would pay the OS's refusal delay for nothing. The pidfile says
    // whether the daemon that wrote the port is still alive — when it is
    // gone the port cannot hold a live control server, and the bind below
    // simply overwrites the stale file.
    if !crate::daemon::spawn::recorded_daemon_is_dead()
        && let Ok(live) =
            transport::connect_endpoint(lyrel_protocol::daemon::control_endpoint::CONTROL_PORT_FILE)
    {
        drop(live);
        return Err(io::Error::new(
            io::ErrorKind::AddrInUse,
            format!(
                "a control server is already listening at {}",
                transport::endpoint_display_named(
                    lyrel_protocol::daemon::control_endpoint::CONTROL_PORT_FILE
                )
            ),
        ));
    }
    let (listener, token) =
        transport::bind_endpoint(lyrel_protocol::daemon::control_endpoint::CONTROL_PORT_FILE)
            .map_err(io::Error::other)?;
    std::thread::Builder::new()
        .name("lyrel-control-listener".into())
        .spawn(move || serve_listener_with(listener, token, host, services))?;
    Ok(path)
}

pub fn spawn_control_listener(host: SharedHost) -> io::Result<PathBuf> {
    spawn_control_listener_with(host, Services::none())
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

pub(super) fn control_endpoint_display() -> String {
    lyrel_protocol::daemon::control_endpoint::control_endpoint_display()
}
