//! Host-scoped session discovery, cached rows, and late-result fencing.

use std::path::PathBuf;

use gpui_kit::{App, Context, Entity, EventEmitter as _, Window};
use crate::core::agent_history::PastSession;
use crate::core::cli_agent::CLIAgent;
use crate::core::config::Config;
use crate::terminal::view::TerminalView;
use crate::ui::host_ops::{HostId, HostOps, SharedHost};
use crate::ui::i18n::{L10nKey, t, t_fmt};
use super::command::{Avatar, CommandKind, Item};
use super::{SearchEvent, SearchView};

pub(crate) struct SessionHistory {
    host: HostId,
    here: Option<PathBuf>,
    default_cwd: Option<PathBuf>,
    home: Option<PathBuf>,
}

impl SessionHistory {
    pub(crate) fn new(host: HostId, here: Option<PathBuf>, default_cwd: Option<PathBuf>, home: Option<PathBuf>) -> Self {
        Self { host, here, default_cwd, home }
    }

    pub(crate) fn cached_items(&self, cx: &App) -> (Vec<Item>, usize) {
        self.items(last_sessions(self.host), cx)
    }

    pub(super) fn items(&self, found: Vec<PastSession>, cx: &App) -> (Vec<Item>, usize) {
        let now = crate::core::config::unix_now();
        let hidden = &cx.global::<Config>().hidden_agent_sessions;
        let here = &self.here;
        let home = &self.home;
        let (mut mine, mut rest): (Vec<_>, Vec<_>) = found
            .into_iter()
            .filter(|s| !hidden.contains(&crate::core::agent_history::session_key(s.agent, &s.id)))
            .partition(|s| here.is_some() && s.cwd.as_ref() == here.as_ref());
        let here_section: Option<gpui_kit::SharedString> = here.as_ref().map(|dir| {
            t_fmt(
                L10nKey::SearchSectionSessionsHere,
                &[("dir", &crate::ui::home::display_path(dir, home.as_deref()))],
            )
            .into()
        });
        let recent: gpui_kit::SharedString = t(L10nKey::SearchSectionSessionsRecent).into();
        let row = |s: crate::core::agent_history::PastSession, section: gpui_kit::SharedString| {
            let place = s
                .cwd
                .as_deref()
                .map(|p| crate::ui::home::display_path(p, home.as_deref()));
            let mut subtitle = match (&place, &s.branch) {
                (Some(place), Some(branch)) => Some(format!("{place} · {branch}")),
                (Some(place), None) => Some(place.clone()),
                (None, Some(branch)) => Some(branch.clone()),
                (None, None) => None,
            };
            if let Some(scope) = &s.scope
                && let Some(context) = &scope.opencode
            {
                let connection = context
                    .server
                    .as_ref()
                    .map(|s| s.urls.join(", "))
                    .unwrap_or_else(|| context.connection_argv.join(" "));
                let storage = scope
                    .environment
                    .get("OPENCODE_DB")
                    .or_else(|| scope.environment.get("XDG_DATA_HOME"))
                    .or_else(|| scope.environment.get("HOME"))
                    .or_else(|| scope.environment.get("USERPROFILE"));
                let source = storage.map_or(connection.clone(), |storage| {
                    format!("{connection} · {storage}")
                });
                subtitle =
                    Some(subtitle.map_or(source.clone(), |place| format!("{place} · {source}")));
            }
            let mut item = Item::new(
                s.title,
                CommandKind::ResumeSession {
                    agent: s.agent,
                    session_id: s.id.clone(),
                    cwd: s.cwd,
                    host: self.host,
                    scope: s.scope,
                },
            )
            .with_note(crate::ui::home::relative_time(now, s.updated))
            .with_avatar(Avatar {
                agent: Some(s.agent),
                ..Avatar::default()
            })
            .with_alias(s.id)
            .with_alias(s.agent.display_name())
            .in_section(section);
            if let Some(subtitle) = subtitle {
                item = item.with_subtitle(subtitle);
            }
            item
        };
        let count = mine.len();
        let mut out: Vec<Item> = match here_section {
            Some(section) => mine.drain(..).map(|s| row(s, section.clone())).collect(),
            None => Vec::new(),
        };
        out.extend(rest.drain(..).map(|s| row(s, recent.clone())));
        (out, count)
    }

    pub(crate) fn query(self, host: SharedHost, panes: Vec<Entity<TerminalView>>, agents: Vec<CLIAgent>) -> SessionQuery {
        SessionQuery { history: self, host, panes, agents }
    }
}

pub(crate) struct SessionQuery {
    history: SessionHistory,
    host: SharedHost,
    panes: Vec<Entity<TerminalView>>,
    agents: Vec<CLIAgent>,
}

impl SessionQuery {
    pub(super) fn start(self, generation: u64, window: &mut Window, cx: &mut Context<SearchView>) {
        let history = self.history;
        let host_id = history.host;
        use lyrel_core::host::session_operations::{
            SessionOperation, SessionOperationKind, SessionScope,
        };
        let opencode = crate::core::cli_agent::CLIAgent::OpenCode;
        let mut opencode_scopes = Vec::new();
        let hermes = crate::core::cli_agent::CLIAgent::Hermes;
        let mut hermes_scopes = Vec::new();
        let mut hermes_errors = Vec::new();
        for leaf in self.panes {
            let pane = leaf.read(cx);
            if pane.host_id() == host_id
                && pane.agent() == Some(hermes)
                && let Some(state) = pane.agent_session()
                && let Some(cwd) = pane.host_cwd()
                && let Some(scope) = state.readout.session_scope.clone()
                && let Some(launch_argv) =
                    lyrel_core::host::session_operations::launch_context(hermes, &state)
                && !hermes_scopes
                    .iter()
                    .any(|request: &SessionOperation| request.scope == scope)
            {
                hermes_scopes.push(SessionOperation {
                    expected_host: host_id.0,
                    agent: hermes,
                    kind: SessionOperationKind::Resume,
                    source_id: String::new(),
                    cwd,
                    launch_argv,
                    scope,
                });
            }
            if pane.host_id() != host_id || pane.agent() != Some(opencode) {
                continue;
            }
            let Some(state) = pane.agent_session() else {
                continue;
            };
            let Some(cwd) = pane.host_cwd() else {
                continue;
            };
            let Some(mut scope) = state.readout.session_scope.clone() else {
                continue;
            };
            let Some(launch_argv) =
                lyrel_core::host::session_operations::launch_context(opencode, &state)
            else {
                continue;
            };
            if let Some(context) = &mut scope.opencode {
                context.launch_argv = launch_argv.clone();
            }
            if opencode_scopes
                .iter()
                .any(|request: &SessionOperation| request.scope == scope)
            {
                continue;
            }
            opencode_scopes.push(SessionOperation {
                expected_host: host_id.0,
                agent: opencode,
                kind: SessionOperationKind::Resume,
                source_id: String::new(),
                cwd,
                launch_argv,
                scope,
            });
        }
        // A normal default/configured scope remains discoverable without an open TUI.
        let default_launch = crate::core::cli_agent::parse_command_argv(
            &opencode.launch_command(&cx.global::<Config>().agent_launch),
        );
        let default_cwd = history.default_cwd.clone().or_else(|| history.home.clone()).unwrap_or_default();
        if self.agents.contains(&opencode)
            && let Ok(launch_argv) = default_launch
        {
            opencode_scopes.push(SessionOperation {
                expected_host: host_id.0,
                agent: opencode,
                kind: SessionOperationKind::Resume,
                source_id: String::new(),
                cwd: default_cwd.clone(),
                launch_argv,
                scope: SessionScope::default(),
            });
        }
        if self.agents.contains(&hermes) {
            match crate::core::cli_agent::parse_command_argv(
                &hermes.launch_command(&cx.global::<Config>().agent_launch),
            ) {
                Ok(launch_argv) => hermes_scopes.push(SessionOperation {
                    expected_host: host_id.0,
                    agent: hermes,
                    kind: SessionOperationKind::Resume,
                    source_id: String::new(),
                    cwd: default_cwd,
                    launch_argv,
                    scope: SessionScope::default(),
                }),
                Err(message) => hermes_errors.push(
                    lyrel_core::host::session_operations::SessionOperationError {
                        kind: lyrel_core::host::session_operations::SessionErrorKind::Unsupported,
                        message: format!("Hermes history launch context: {message}"),
                        retained: None,
                    },
                ),
            }
        }
        HostOps::run_in(
            self.host,
            window,
            cx,
            move |h| {
                let mut found = h.agent_sessions();
                let mut errors = hermes_errors;
                for request in opencode_scopes {
                    match h.opencode_sessions(&request) {
                        Ok(sessions) => {
                            if let Ok(found) = &mut found {
                                for session in sessions {
                                    let same =
                                        |existing: &crate::core::agent_history::PastSession| {
                                            existing.agent == session.agent
                                                && existing.id == session.id
                                                && existing
                                                    .scope
                                                    .as_ref()
                                                    .and_then(|s| s.opencode.as_ref())
                                                    .map(|s| (&s.server, &s.connection_argv))
                                                    == session
                                                        .scope
                                                        .as_ref()
                                                        .and_then(|s| s.opencode.as_ref())
                                                        .map(|s| (&s.server, &s.connection_argv))
                                        };
                                    if !found.iter().any(same) {
                                        found.push(session);
                                    }
                                }
                            }
                        }
                        Err(error) => errors.push(error),
                    }
                }
                for request in hermes_scopes {
                    match h.hermes_sessions(&request) {
                        Ok(sessions) => {
                            if let Ok(found) = &mut found {
                                for session in sessions {
                                    if !found.iter().any(|existing| {
                                        existing.agent == session.agent
                                            && existing.id == session.id
                                            && existing.scope == session.scope
                                    }) {
                                        found.push(session);
                                    }
                                }
                            }
                        }
                        Err(error) => errors.push(error),
                    }
                }
                if let Ok(found) = &mut found {
                    found.sort_by_key(|s| std::cmp::Reverse(s.updated));
                }
                (found, errors)
            },
            move |view, (found, errors), window, cx| {
                if !view.accepts_session_query(generation) {
                    return;
                }
                for error in errors {
                    cx.emit(SearchEvent::SessionError(error));
                }
                let found = match found {
                    Ok(found) => found,
                    Err(e) => {
                        log::warn!("listing agent sessions: {e}");
                        return;
                    }
                };
                remember_sessions(host_id, found.clone());
                let (sessions, here) = history.items(found, cx);
                view.set_sessions(sessions, here, window, cx);
            },
        );
    }
}

/// What each host's last session scan found: what the Sessions tab shows
/// while the next scan runs, so it does not open empty.
static LAST_SESSIONS: std::sync::LazyLock<
    std::sync::Mutex<
        std::collections::HashMap<
            crate::ui::host_ops::HostId,
            Vec<crate::core::agent_history::PastSession>,
        >,
    >,
> = std::sync::LazyLock::new(Default::default);

fn last_sessions(
    host: crate::ui::host_ops::HostId,
) -> Vec<crate::core::agent_history::PastSession> {
    LAST_SESSIONS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&host)
        .cloned()
        .unwrap_or_default()
}

fn remember_sessions(
    host: crate::ui::host_ops::HostId,
    found: Vec<crate::core::agent_history::PastSession>,
) {
    LAST_SESSIONS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(host, found);
}
