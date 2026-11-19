//! GUI-side `core` facade: local adapters below, and the pure protocol/core
//! modules re-exported by name. No glob — `config`, `session` and `keychain`
//! are the local modules and nothing here shadows them.

pub use lyrel_core::core::{
    agent_history, agent_hooks, cli_agent, crash, git, group_key, logfile, machine, osc, proc,
    shell_quote, shells, ssh_profile, threads, worktree,
};

pub mod actions;
pub mod agent_prompt;
#[cfg(target_os = "windows")]
pub mod aumid;
pub mod cli_install;
pub mod config;
pub mod default_terminal;
pub mod explorer_context_menu;
pub mod keychain;
pub mod rate_meter;
pub mod session;
pub mod ssh_config;
pub mod update;
pub mod window_state;
