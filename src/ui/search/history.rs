//! Search Everywhere's Sessions tab: the host-scoped session discovery
//! rendered as searchable rows.
//!
//! The discovery itself — building each provider's scan requests and merging
//! what the owning Host found — is shared with the right panel's Session
//! History tool and Sessions settings in [`crate::ui::session_history`], so
//! every consumer lists the same sessions and the same per-source errors.
//! This module only shapes the result into search rows.

use std::path::PathBuf;

use super::command::{Avatar, CommandKind, Item};
use super::{SearchEvent, SearchView};
use crate::core::agent_history::PastSession;
use crate::core::cli_agent::CLIAgent;
use gpui_kit::{App, Context, Entity, Window};
use lyrel_terminal::view::TerminalView;
use lyrel_ui_base::config::Config;
use lyrel_ui_base::host_ops::{HostId, HostOps, SharedHost};
use lyrel_ui_base::i18n::{L10nKey, t, t_fmt};

pub(crate) struct SessionHistory {
    host: HostId,
    here: Option<PathBuf>,
    default_cwd: Option<PathBuf>,
    home: Option<PathBuf>,
}

impl SessionHistory {
    pub(crate) fn new(
        host: HostId,
        here: Option<PathBuf>,
        default_cwd: Option<PathBuf>,
        home: Option<PathBuf>,
    ) -> Self {
        Self {
            host,
            here,
            default_cwd,
            home,
        }
    }

    pub(crate) fn cached_items(&self, cx: &App) -> (Vec<Item>, usize) {
        self.items(crate::ui::session_history::cached_sessions(self.host), cx)
    }

    pub(super) fn items(&self, found: Vec<PastSession>, cx: &App) -> (Vec<Item>, usize) {
        let now = lyrel_core::core::config::unix_now();
        let hidden = &cx.global::<Config>().hidden_agent_sessions;
        let here = &self.here;
        let home = &self.home;
        // `here` is a directory on this query's host; its records are judged
        // by that host's path semantics, not by the client's platform.
        let host_separator = lyrel_ui_base::host_registry::HostRegistry::lookup(cx, self.host)
            .map_or(std::path::MAIN_SEPARATOR, |host| host.separator());
        let (mut mine, mut rest): (Vec<_>, Vec<_>) = found
            .into_iter()
            .filter(|s| !hidden.contains(&crate::core::agent_history::session_key(s.agent, &s.id)))
            .partition(|s| {
                here.as_deref().is_some_and(|here| {
                    s.cwd.as_deref().is_some_and(|cwd| {
                        crate::core::agent_history::directory_matches_with(cwd, here, host_separator)
                    })
                })
            });
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

    pub(crate) fn query(
        self,
        host: SharedHost,
        panes: Vec<Entity<TerminalView>>,
        agents: Vec<CLIAgent>,
    ) -> SessionQuery {
        SessionQuery {
            history: self,
            host,
            panes,
            agents,
        }
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
        // A normal default/configured scope remains discoverable without an
        // open TUI; the directory it validates against is the pane's spawn
        // directory, then this host's home.
        let default_cwd = history
            .default_cwd
            .clone()
            .or_else(|| history.home.clone())
            .unwrap_or_default();
        let scan = crate::ui::session_history::plan_scan(
            host_id,
            &self.panes,
            &self.agents,
            &default_cwd,
            &cx.global::<Config>().agent_launch.clone(),
            cx,
        );
        HostOps::run_in(
            self.host,
            window,
            cx,
            move |host| crate::ui::session_history::collect_sessions(host, scan),
            move |view, listing, window, cx| {
                if !view.accepts_session_query(generation) {
                    return;
                }
                for error in listing.errors {
                    cx.emit(SearchEvent::SessionError(error));
                }
                crate::ui::session_history::remember_sessions(host_id, listing.sessions.clone());
                let (sessions, here) = history.items(listing.sessions, cx);
                view.set_sessions(sessions, here, window, cx);
            },
        );
    }
}
