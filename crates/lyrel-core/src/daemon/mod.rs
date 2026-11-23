#[cfg(windows)]
pub(crate) mod conpty;
pub mod duplex;
/// Upgrading the daemon in place, keeping the ptys and the shells on them.
///
/// Unix only, and not for want of trying: `execve` is what makes it possible,
/// and Windows has neither that nor a way to hand a ConPTY to another process.
/// There the daemon still stops and starts, and `scrollback` softens it.
#[cfg(unix)]
pub mod handoff;
pub mod history;
pub mod install;
mod kimi;
pub mod pane;
pub(crate) mod procctl;
pub mod procinfo;
pub mod procstat;
#[cfg_attr(not(any(target_os = "macos", target_os = "linux")), allow(dead_code))]
pub(crate) mod remote;
pub mod remote_link;
/// Disclaiming the launcher's privacy attribution at startup (#909).
#[cfg(target_os = "macos")]
pub mod responsibility;
pub mod router;
pub mod scrollback;
pub mod server;
pub mod ssh;

pub(crate) mod shell_integration;

/// The Windows environment refresh new panes get (#333). Only the registry
/// reader and the spawn wiring are Windows-only; the merge itself is a pure
/// function, so `cfg(test)` keeps the module compiling everywhere and its
/// semantics tested on every platform's CI runner rather than only on Windows.
#[cfg(any(windows, test))]
pub(crate) mod windows_env;
#[cfg(windows)]
pub(crate) mod winproc;

// The wire protocols, client drivers, transports, endpoint discovery, and the
// spawn/singleton probes live in `lyrel-protocol`; these re-exports keep every
// existing `lyrel_core::daemon::*` path working for the GUI, the CLI, and the
// server tests.
#[cfg(not(windows))]
pub(crate) use lyrel_protocol::daemon::DETECTED_SHELL_ENV;
#[cfg(windows)]
pub use lyrel_protocol::daemon::update_guard;
pub use lyrel_protocol::daemon::{control, pidfile, protocol, singleton, spawn, transport};
