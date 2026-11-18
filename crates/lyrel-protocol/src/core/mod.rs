//! GUI-free domain and helper modules shared by the daemon, the desktop, the
//! CLI and `lyrel-insights`.
//!
//! Everything here is pure value/parse/helper code: configuration and SSH
//! credential-reference naming, the machine-tree/session value models, the
//! terminal OSC/paste/term-mode/XTGETTCAP client helpers, and the
//! cli-agent/agent-hooks/worktree/git helper families. Effectful authorities
//! (the `MachineStore`, terminal image/Glyph state, SSH/PTY engines, the
//! `Host` implementations) live in `lyrel-core`, which re-exports these
//! modules under the same `core::` paths.

pub mod agent_history;
pub mod agent_hooks;
pub mod cli_agent;
pub mod codename;
pub mod config;
pub mod git;
pub mod group_key;
pub mod keychain;
pub mod machine;
pub mod osc;
pub mod pane_colors;
pub mod pane_sidecar;
pub mod paste;
pub mod path_spelling;
pub mod proc;
pub mod program_status;
pub mod session;
pub mod shells;
pub mod shell_quote;
pub mod ssh_profile;
pub mod tab_view;
pub mod term_modes;
pub mod window_state;
pub mod worktree;
pub mod xtgettcap;
