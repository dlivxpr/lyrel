//! Domain and helper modules.
//!
//! The pure configuration/model/agent/git/worktree/terminal-helper modules
//! live in `lyrel-protocol` and are re-exported here unchanged, so that
//! `lyrel_core::core::<module>` keeps its existing paths for the GUI, the
//! daemon, the CLI and the server. The modules listed as `pub mod` below keep
//! effectful or authoritative implementations in this crate (the `MachineStore`
//! persistence, the agent-history scanner, the agent-hook runner, the tokio
//! process helper, and the XTGETTCAP owner-side tests that exercise the shared
//! protocol framing filter).

pub mod agent_history;
pub mod agent_prompt;
pub mod agent_hooks;
pub use lyrel_protocol::core::cli_agent;
pub mod clipboard;
pub use lyrel_protocol::core::codename;
pub mod crash;
pub use lyrel_protocol::core::config;
pub use lyrel_protocol::core::git;
pub use lyrel_protocol::core::gitignore;
pub mod glyph_protocol;
pub mod graphics_state;
pub use lyrel_protocol::core::group_key;
mod iterm2_graphics;
pub use lyrel_protocol::core::keychain;
pub mod kitty_graphics;
pub mod logfile;
pub mod machine;
pub use lyrel_protocol::core::osc;
pub use lyrel_protocol::core::pane_colors;
pub use lyrel_protocol::core::pane_sidecar;
pub use lyrel_protocol::core::paste;
pub use lyrel_protocol::core::path_spelling;
pub mod proc;
pub mod program_status;
pub use lyrel_protocol::core::session;
pub use lyrel_protocol::core::shell_quote;
pub use lyrel_protocol::core::shells;
mod sixel_graphics;
pub use lyrel_protocol::core::ssh_profile;
pub use lyrel_protocol::core::tab_view;
pub use lyrel_protocol::core::term_modes;
pub mod terminal_resize;
pub mod threads;
pub use lyrel_protocol::core::window_state;
pub use lyrel_protocol::core::worktree;
pub mod xtgettcap;
