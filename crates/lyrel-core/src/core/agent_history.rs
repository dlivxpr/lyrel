//! Past coding-agent sessions on this computer.
//!
//! The value model, parsers and the filesystem scanner live in
//! `lyrel-protocol` and are re-exported here unchanged. This crate adds the
//! one enrichment that needs the daemon's `MachineStore`: the live pane
//! agent scopes, without which resuming a session that is still running in a
//! non-default scope would not list it.

pub use lyrel_protocol::core::agent_history::*;

/// [`Roots::local`] plus the session scopes of the panes this daemon
/// currently runs — the answer the full host's `agent_sessions` uses, and
/// what the GUI's resume list scans.
pub fn local_roots() -> std::io::Result<Roots> {
    let mut roots = Roots::local()?;
    if let Some(store) = crate::core::machine::observed_store() {
        for pane in store.machine().panes {
            if let Some(agent) = pane.agent
                && let Some(scope) = agent
                    .session_scope
                    .filter(|scope| scope.session_dir.is_some())
            {
                let scopes = match agent.agent {
                    crate::core::cli_agent::CLIAgent::OhMyPi => &mut roots.omp_scopes,
                    crate::core::cli_agent::CLIAgent::Pi => &mut roots.pi_scopes,
                    _ => continue,
                };
                if !scopes.contains(&scope) {
                    scopes.push(scope);
                }
            }
        }
    }
    Ok(roots)
}
