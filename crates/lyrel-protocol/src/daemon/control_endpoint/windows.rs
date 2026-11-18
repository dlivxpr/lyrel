//! Windows authenticated loopback control endpoint: the `control.port` file
//! records the TCP port and the token that authenticates a client, exactly
//! the way the pane endpoint's `daemon.port` does (see
//! [`crate::daemon::transport`]).

use std::io;
use std::path::PathBuf;

use crate::daemon::{spawn, transport};

pub const CONTROL_PORT_FILE: &str = "control.port";

/// Where this config dir's control endpoint is recorded.
pub fn control_endpoint_path() -> io::Result<PathBuf> {
    transport::port_path_named(CONTROL_PORT_FILE)
        .ok_or_else(|| io::Error::other("no config directory to record the control endpoint in"))
}

/// See the unix arm: a bind that failed only means "serve panes only" when
/// something else is actually answering there.
///
/// Gated on the pidfile the same way the core listener's bind gates its own
/// probe, and for the same reason: a TCP connect that completes proves only
/// that *something* accepted on the recorded port, and a port a dead daemon
/// wrote can have been recycled by a stranger since. When the daemon that
/// wrote it is gone, nothing behind that port is ours.
pub fn control_endpoint_answers() -> bool {
    !spawn::recorded_daemon_is_dead() && transport::connect_endpoint(CONTROL_PORT_FILE).is_ok()
}

/// Connect to the control server recorded for this config dir, presenting
/// the endpoint's token.
pub fn connect_control() -> io::Result<std::net::TcpStream> {
    transport::connect_endpoint(CONTROL_PORT_FILE)
}

/// Remove the `control.port` file. Callers own the liveness check: the
/// shutdown path removes its own file unconditionally, while the reap path
/// has already established that the recorded daemon is dead.
pub fn remove_control_endpoint() {
    transport::remove_endpoint(CONTROL_PORT_FILE);
}

/// Where the control endpoint is recorded, for logs and error messages.
/// Empty when no config dir names one.
pub fn control_endpoint_display() -> String {
    control_endpoint_path()
        .map(|p| p.display().to_string())
        .unwrap_or_default()
}
