//! One integration-test binary for the ordinary `lyrel-server` consumers.
//!
//! Cargo links one executable per top-level `tests/*.rs` file. Everything that
//! is not an explicitly documented SSH target lives under `suite/` and is
//! mounted here, so the real daemon/PTY/client consumers keep their behavior
//! while linking one binary instead of eighteen. Module paths are the old file
//! stems: substring filters keep working, `--exact` callers must use the
//! prefixed name (for example `client_lib::kitty_graphics::pty_sender`).
//!
//! `native_ssh_graphics`, `native_ssh_lifecycle` and `native_ssh_surface`
//! remain top-level targets because `docs/remote/ssh.mdx` invokes them by
//! `--test` name.

#[path = "suite/agent_hook_report.rs"]
mod agent_hook_report;
#[path = "suite/cli.rs"]
mod cli;
#[path = "suite/client_lib.rs"]
mod client_lib;
#[path = "suite/daemon_reap.rs"]
mod daemon_reap;
#[path = "suite/daemon_stop.rs"]
mod daemon_stop;
#[path = "suite/handoff.rs"]
mod handoff;
#[path = "suite/host_metrics.rs"]
mod host_metrics;
#[path = "suite/machine_tree.rs"]
mod machine_tree;
#[path = "suite/native_ssh_capabilities.rs"]
pub mod native_ssh_capabilities;
#[path = "suite/pane_history.rs"]
mod pane_history;
#[path = "suite/pane_send_input.rs"]
mod pane_send_input;
#[path = "suite/program_status.rs"]
mod program_status;
#[path = "suite/remote_read_session.rs"]
pub mod remote_read_session;
#[path = "suite/remote_router.rs"]
mod remote_router;
#[path = "suite/remote_sqlite.rs"]
pub mod remote_sqlite;
#[path = "suite/routed_pane.rs"]
mod routed_pane;
#[path = "suite/scrollback_restore.rs"]
mod scrollback_restore;
#[path = "suite/stdio_conformance.rs"]
mod stdio_conformance;
