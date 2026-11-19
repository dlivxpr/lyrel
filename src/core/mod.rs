//! GUI-side core adapters and explicitly named shared core modules.

pub use lyrel_core::core::{
    agent_history, agent_hooks, agent_prompt, cli_agent, crash, git, group_key, logfile, machine,
    osc, proc, shell_quote, shells, ssh_profile, worktree,
};

pub mod cli_install;
pub mod default_terminal;
pub mod explorer_context_menu;
pub mod keychain;
pub mod update;
pub mod window_state;
