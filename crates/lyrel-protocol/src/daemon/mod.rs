//! The wire half of the session daemon: the control and pane dialects with
//! their framing, the synchronous client drivers that speak them, the local
//! transports they ride, and the spawn/singleton probes a thin client needs
//! to find or start its daemon. Everything here is GUI-free and free of the
//! daemon's runtime (PTY, SSH, installer): those stay in `lyrel-core`, which
//! re-exports these modules so existing `lyrel_core::daemon::*` paths keep
//! working.

pub mod control;
pub mod control_endpoint;
pub mod install;
pub mod pidfile;
pub mod protocol;
pub mod router;
pub mod singleton;
pub mod spawn;
pub mod transport;

/// Windows-only like the updater flow that holds it: the macOS updater swaps
/// the bundle by rename and never stops the daemon, so it has no window in
/// which a fresh daemon could relock anything.
#[cfg(windows)]
pub mod update_guard;

#[cfg(windows)]
pub mod winproc;

/// Names the shell a pane was spawned with, so a later pane in the same
/// session reuses it without re-detecting.
pub const DETECTED_SHELL_ENV: &str = "LYREL_DETECTED_SHELL";

/// Identifies the pane inherited by a process started inside its shell.
pub const LYREL_PANE_ENV: &str = "LYREL_PANE";
