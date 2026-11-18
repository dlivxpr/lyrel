//! Control-endpoint discovery and client-side connection helpers.
//!
//! The control endpoint is the daemon's machine-tree socket (`control.sock`
//! on Unix, an authenticated loopback `control.port` file on Windows). Its
//! address rule, its liveness probe, and the connect/remove helpers a client
//! needs live here, next to the wire protocols that ride them. The listener
//! that serves the endpoint — binding, accepting, dispatching — stays in
//! `lyrel-core` (`lyrel_core::host::server`), which re-exports these helpers
//! so existing paths keep working.

#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

/// Overrides the control socket path, exactly the way the pane endpoint's
/// address can be pinned. Empty values are ignored.
#[cfg(unix)]
pub const CONTROL_SOCK_ENV: &str = "LYREL_CONTROL_SOCK";

#[cfg(unix)]
pub use unix::{
    connect_control, control_endpoint_answers, control_endpoint_display, control_socket_path,
    remove_control_endpoint, socket_path_in,
};

#[cfg(windows)]
pub use windows::{
    CONTROL_PORT_FILE, connect_control, control_endpoint_answers, control_endpoint_display,
    control_endpoint_path, remove_control_endpoint,
};
