//! Directory-scoped Session History: the right panel's own tool, and the
//! shared discovery behind it.
//!
//! The discovery is host-scoped: a scan runs on the Host that owns the
//! session stores (through [`HostOps`], never on the UI thread) and every
//! landed result is bound to the host and generation that asked for it, so a
//! stale answer can never repaint a newer question. Each agent provider
//! answers separately — the native pi/Oh My Pi/Kimi stores through
//! `Host::agent_sessions`, OpenCode and Hermes through their scoped requests
//! — so one source failing never empties the others: the listing keeps the
//! per-source errors beside the sessions it did find, and the view says
//! which is which. Search Everywhere's Sessions tab consumes the same
//! plan/collect pair (see [`crate::ui::search`]), and Sessions settings
//! (#366) builds on the listing type.

use std::path::{Path, PathBuf};

use gpui_kit::TestSupportExt as _;
use gpui_kit::base::actions::{Confirm, SelectDown, SelectUp};
use gpui_kit::component::menu::{ContextMenuExt as _, DropdownMenu as _, PopupMenu, PopupMenuItem};
use gpui_kit::component::{ActiveTheme as _, Sizable as _, h_flex, v_flex};
use gpui_kit::{
    AnyElement, App, Context, Entity, Focusable as _, MouseButton, Window, anchored, deferred, div,
    prelude::*, px, rems,
};

use lyrel_core::core::agent_history::PastSession;
use lyrel_core::host::session_operations::{
    SessionErrorKind, SessionOperation, SessionOperationError, SessionOperationKind,
};
use lyrel_terminal::view::TerminalView;
use lyrel_ui_base::config::Config;
use lyrel_ui_base::host_ops::{HostId, HostOps};
use lyrel_ui_base::i18n::{L10nKey, t};

use crate::core::cli_agent::CLIAgent;
use crate::ui::app::LyrelApp;
use crate::ui::right_panel::{META, TEXT, TEXT_INSET};
use crate::ui::window_chrome::CONTENT_INSET;

/// What one host's scan found: every session the providers listed, most
/// recently active first, and the per-source errors that explain what could
/// not be. This is the shared listing handed to Search Everywhere's Sessions
/// tab, this tool, and Sessions settings (#366): identity (`agent`, `id`,
/// `scope`, `cwd`) is preserved verbatim for whoever consumes it next.
///
/// `host_reachable` is the transport's own verdict, kept apart from the
/// per-source errors on purpose: a provider failing (an OpenCode service
/// with no TUI, a store the transport cannot serve) is not a disconnected
/// host, and the view must not say "reconnect" when the machine is fine.
#[derive(Clone, Debug)]
pub(crate) struct SessionListing {
    pub(crate) sessions: Vec<PastSession>,
    pub(crate) errors: Vec<SessionOperationError>,
    pub(crate) host_reachable: bool,
}

impl Default for SessionListing {
    fn default() -> Self {
        Self {
            sessions: Vec::new(),
            errors: Vec::new(),
            host_reachable: true,
        }
    }
}

/// Why a listing has nothing (or not everything) to show. Empty, unavailable
/// and incomplete discovery are different sentences and the view has to say
/// which one it means.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SessionHealth {
    /// Scan finished; this directory genuinely has no saved sessions.
    Empty,
    /// The owning host could not be reached at all.
    Disconnected,
    /// The host answered but cannot list sessions (an old Lyrel server, or a
    /// provider/transport without the capability).
    Unsupported,
    /// Sessions were listed, and some sources failed besides.
    Partial,
    /// Every source answered.
    Ready,
}

impl SessionListing {
    /// How a directory-filtered view of this listing reads, given how many
    /// sessions survived the directory filter.
    pub(crate) fn health(&self, visible: usize) -> SessionHealth {
        if visible > 0 {
            return match self.errors.is_empty() {
                true => SessionHealth::Ready,
                false => SessionHealth::Partial,
            };
        }
        if !self.host_reachable {
            return SessionHealth::Disconnected;
        }
        if self.errors.is_empty() {
            return SessionHealth::Empty;
        }
        SessionHealth::Unsupported
    }
}

/// The per-provider requests one scan runs: OpenCode and Hermes each answer
/// per observed storage scope, so a pane running under a non-default profile
/// or service keeps its history discoverable.
pub(crate) struct SessionScan {
    opencode: Vec<SessionOperation>,
    hermes: Vec<SessionOperation>,
    hermes_errors: Vec<SessionOperationError>,
}

/// Build the scan requests from what only the UI can read: the scopes open
/// panes observe, and the default scope each offered agent gets. Pure with
/// respect to the Host — everything here was already in the window.
pub(crate) fn plan_scan(
    host_id: HostId,
    panes: &[Entity<TerminalView>],
    agents: &[CLIAgent],
    default_cwd: &Path,
    agent_launch: &std::collections::HashMap<String, String>,
    cx: &App,
) -> SessionScan {
    let opencode = CLIAgent::OpenCode;
    let hermes = CLIAgent::Hermes;
    let mut opencode_scopes = Vec::new();
    let mut hermes_scopes = Vec::new();
    let mut hermes_errors = Vec::new();
    for leaf in panes {
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
    if agents.contains(&opencode)
        && let Ok(launch_argv) =
            crate::core::cli_agent::parse_command_argv(&opencode.launch_command(agent_launch))
    {
        opencode_scopes.push(SessionOperation {
            expected_host: host_id.0,
            agent: opencode,
            kind: SessionOperationKind::Resume,
            source_id: String::new(),
            cwd: default_cwd.to_path_buf(),
            launch_argv,
            scope: Default::default(),
        });
    }
    if agents.contains(&hermes) {
        match crate::core::cli_agent::parse_command_argv(&hermes.launch_command(agent_launch)) {
            Ok(launch_argv) => hermes_scopes.push(SessionOperation {
                expected_host: host_id.0,
                agent: hermes,
                kind: SessionOperationKind::Resume,
                source_id: String::new(),
                cwd: default_cwd.to_path_buf(),
                launch_argv,
                scope: Default::default(),
            }),
            Err(message) => hermes_errors.push(SessionOperationError {
                kind: SessionErrorKind::Unsupported,
                message: format!("Hermes history launch context: {message}"),
                retained: None,
            }),
        }
    }
    SessionScan {
        opencode: opencode_scopes,
        hermes: hermes_scopes,
        hermes_errors,
    }
}

/// Run one scan on its owning Host (blocking — call through `HostOps`).
/// `agent_sessions` covers pi/Oh My Pi/Kimi; the scoped requests cover
/// OpenCode and Hermes. A failing source is recorded as an error and never
/// empties what the other sources found.
pub(crate) fn collect_sessions(
    host: &dyn lyrel_core::host::Host,
    scan: SessionScan,
) -> SessionListing {
    let mut listing = SessionListing {
        sessions: Vec::new(),
        errors: scan.hermes_errors,
        host_reachable: true,
    };
    match host.agent_sessions() {
        Ok(found) => listing.sessions.extend(found),
        Err(error) => {
            // The transport's own verdict is the only "disconnected" signal:
            // a host that is up but cannot look reads as a per-source
            // failure, never as a lost machine.
            if !host.is_connected() {
                listing.host_reachable = false;
            }
            listing.errors.push(SessionOperationError {
                kind: SessionErrorKind::Connection,
                message: format!("native agent history: {error}"),
                retained: None,
            });
        }
    }
    // The native scanner caps its recent-first list; a listing that hit the
    // cap cannot promise an older exact-directory record is absent, so the
    // cap is said out loud and consumers of the shared listing (#366's bulk
    // actions especially) can refuse to treat it as complete.
    if listing.sessions.len() >= lyrel_core::core::agent_history::MAX_SESSIONS {
        listing.errors.push(SessionOperationError {
            kind: SessionErrorKind::Unsupported,
            message: format!(
                "native agent history reached its {}-session cap; older records may be missing",
                lyrel_core::core::agent_history::MAX_SESSIONS
            ),
            retained: None,
        });
    }
    for request in scan.opencode {
        match host.opencode_sessions(&request) {
            Ok(sessions) => {
                for session in sessions {
                    let same = |existing: &PastSession| {
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
                    if !listing.sessions.iter().any(same) {
                        listing.sessions.push(session);
                    }
                }
                // A default service scope names no store: whatever running
                // service answered, its storage binding is the service's own,
                // which the listing cannot confirm. The sessions stay listed
                // — and the source is marked incomplete rather than silent,
                // so a disposable-store scan never passes off an unrelated
                // service's history as the scoped result.
                if !opencode_storage_confirmed(&request.scope) {
                    listing.errors.push(SessionOperationError {
                        kind: SessionErrorKind::Unsupported,
                        message: "OpenCode history storage could not be confirmed for the default service scope"
                            .into(),
                        retained: None,
                    });
                }
            }
            Err(error) => listing.errors.push(error),
        }
    }
    for request in scan.hermes {
        match host.hermes_sessions(&request) {
            Ok(sessions) => {
                for session in sessions {
                    if !listing.sessions.iter().any(|existing| {
                        existing.agent == session.agent
                            && existing.id == session.id
                            && existing.scope == session.scope
                    }) {
                        listing.sessions.push(session);
                    }
                }
            }
            Err(error) => listing.errors.push(error),
        }
    }
    listing
        .sessions
        .sort_by_key(|s| std::cmp::Reverse(s.updated));
    listing
}

/// Whether a scope names the store an OpenCode service answers from. The
/// selectors are the same ones the row subtitle spells out; a scope with
/// none of them reached whatever service the launch resolves to, and the
/// listing cannot say whose history that is.
fn opencode_storage_confirmed(scope: &lyrel_core::host::session_operations::SessionScope) -> bool {
    scope.environment.contains_key("OPENCODE_DB")
        || scope.environment.contains_key("XDG_DATA_HOME")
        || scope.environment.contains_key("HOME")
        || scope.environment.contains_key("USERPROFILE")
}

/// What each host's last session scan found: what every history consumer
/// shows while the next scan runs, so none of them opens empty.
static LAST_SESSIONS: std::sync::LazyLock<
    std::sync::Mutex<std::collections::HashMap<HostId, Vec<PastSession>>>,
> = std::sync::LazyLock::new(Default::default);

pub(crate) fn cached_sessions(host: HostId) -> Vec<PastSession> {
    LAST_SESSIONS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&host)
        .cloned()
        .unwrap_or_default()
}

pub(crate) fn remember_sessions(host: HostId, found: Vec<PastSession>) {
    LAST_SESSIONS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(host, found);
}

/// Which directory the tool lists: following the context pane, or pinned to
/// one checkout of its repository.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SessionDirectory {
    FollowCwd,
    Worktree(PathBuf),
}

impl Default for SessionDirectory {
    fn default() -> Self {
        SessionDirectory::FollowCwd
    }
}

/// The identity a started scan belongs to: its host and the directory the
/// request was scoped to. Results landing for any other generation are
/// stale and retired.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SessionBound {
    pub(crate) host: HostId,
    /// The directory sessions will be matched against. `None` — no context
    /// pane, or none with a directory — is not scannable and renders its
    /// own empty state rather than a listing.
    pub(crate) dir: Option<PathBuf>,
}

impl Default for SessionBound {
    fn default() -> Self {
        Self {
            host: HostId::LOCAL,
            dir: None,
        }
    }
}

/// The Session History tool's view state.
#[derive(Default)]
pub(crate) struct SessionHistoryPanel {
    follow: SessionDirectory,
    bound: SessionBound,
    listing: SessionListing,
    generation: u64,
    scanning: bool,
    landed: bool,
    /// Worktree choices for the selector, loaded for the directory whose
    /// repository they belong to.
    worktrees: Vec<lyrel_core::core::worktree::Listed>,
    worktrees_for: Option<PathBuf>,
    loading_worktrees: bool,
    /// The keyboard's row, when the list holds it.
    selected: Option<usize>,
    /// The row whose action menu is open (keyboard or pointer).
    actions_row: Option<usize>,
    actions_menu: Option<Entity<PopupMenu>>,
}

impl SessionHistoryPanel {
    /// The directory the listing should match right now: the pinned
    /// worktree, or the context pane's live directory.
    fn wanted_dir(&self, pane_dir: Option<PathBuf>) -> Option<PathBuf> {
        match &self.follow {
            SessionDirectory::Worktree(path) => Some(path.clone()),
            SessionDirectory::FollowCwd => pane_dir,
        }
    }

    /// Drop whatever the open action menu was showing.
    fn close_actions(&mut self, cx: &mut Context<LyrelApp>) {
        self.actions_row = None;
        self.actions_menu = None;
        cx.notify();
    }
}

/// Height of the directory selector row and of one session row; the list's
/// fixed pitch, used to scroll the keyboard's selection into view.
const SCOPE_ROW_H: f32 = 30.;
const ROW_H: f32 = 28.;
/// The agent disc, the same size Search Everywhere's session rows use.
const ROW_AVATAR: f32 = 18.;

impl LyrelApp {
    /// Keep the Session History listing bound to the context pane's host and
    /// directory. Driven from the window's render like the port watch: the
    /// context pane's directory has no event of its own, so the bound is
    /// recomputed each frame and a scan starts only when it moved.
    pub(crate) fn sync_session_history(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.right_panel_tab != lyrel_core::core::config::RightPanelTab::Sessions
            || !self.right_panel_visible
        {
            return;
        }
        let pane_dir = self
            .context_pane(window, cx)
            .and_then(|pane| pane.read(cx).host_cwd());
        let wanted = SessionBound {
            host: self.spawn_host(cx),
            dir: self.session_history.wanted_dir(pane_dir),
        };
        if self.session_history.scanning || self.session_history.bound == wanted {
            return;
        }
        // No directory (no pane yet): nothing to match, and scanning a
        // made-up home would list sessions the tool cannot scope.
        let Some(dir) = wanted.dir.clone() else {
            self.session_history.bound = wanted;
            self.session_history.landed = true;
            self.session_history.listing = SessionListing::default();
            return;
        };
        let Some(host) = lyrel_ui_base::host_registry::HostRegistry::lookup(cx, wanted.host) else {
            // The host is gone from under the workspace: say disconnected
            // instead of waiting on a scan that has nobody to run on.
            self.session_history.bound = wanted;
            self.session_history.landed = true;
            self.session_history.listing = SessionListing {
                sessions: Vec::new(),
                errors: vec![SessionOperationError {
                    kind: SessionErrorKind::Connection,
                    message: "session history host is disconnected".into(),
                    retained: None,
                }],
                host_reachable: false,
            };
            return;
        };
        // Claim the bound before the trip: a stale landing must not be able
        // to re-bind a bound that has already moved on.
        self.session_history.bound = wanted;
        self.session_history.scanning = true;
        self.session_history.generation += 1;
        let generation = self.session_history.generation;
        let panes: Vec<Entity<TerminalView>> = self
            .tabs
            .iter()
            .flat_map(|tab| tab.pane.terminals())
            .collect();
        let agents = self.offered_agents(cx);
        let scan = plan_scan(
            self.session_history.bound.host,
            &panes,
            &agents,
            &dir,
            &cx.global::<Config>().agent_launch.clone(),
            cx,
        );
        let host_id = self.session_history.bound.host;
        HostOps::run_in(
            host,
            window,
            cx,
            move |host| collect_sessions(host, scan),
            move |app, listing, _window, cx| {
                app.session_history_landed(generation, host_id, listing, cx);
            },
        );
    }

    /// A scan landed: keep it only if its generation is still the live one,
    /// cache what the host said, and let the next render re-check the bound —
    /// the pane may have moved while the scan was in flight.
    fn session_history_landed(
        &mut self,
        generation: u64,
        host: HostId,
        listing: SessionListing,
        cx: &mut Context<Self>,
    ) {
        if self.session_history.generation != generation {
            return;
        }
        remember_sessions(host, listing.sessions.clone());
        self.session_history.scanning = false;
        self.session_history.landed = true;
        self.session_history.listing = listing;
        self.session_history.selected = None;
        cx.notify();
    }

    /// The rows the tool would draw right now: the landed listing filtered
    /// by the current directory and the user's hidden sessions, recent
    /// first. Shared by the render and the keyboard handlers so selection
    /// always names the row it names on screen.
    fn session_rows(&self, window: &Window, cx: &App) -> Vec<PastSession> {
        let Some(dir) = self.session_history.wanted_dir(
            self.context_pane(window, cx)
                .and_then(|pane| pane.read(cx).host_cwd()),
        ) else {
            return Vec::new();
        };
        let hidden = &cx.global::<Config>().hidden_agent_sessions;
        let mut rows: Vec<PastSession> = self
            .session_history
            .listing
            .sessions
            .iter()
            .filter(|s| !hidden.contains(&crate::core::agent_history::session_key(s.agent, &s.id)))
            .filter(|s| s.cwd.as_deref() == Some(dir.as_path()))
            .cloned()
            .collect();
        rows.sort_by_key(|s| std::cmp::Reverse(s.updated));
        rows
    }

    pub(crate) fn render_panel_sessions(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.sync_session_history(window, cx);
        let title = self.panel_title(t(L10nKey::PanelSessionsTitle), None, None, window, cx);
        let dir = self.session_history.wanted_dir(
            self.context_pane(window, cx)
                .and_then(|pane| pane.read(cx).host_cwd()),
        );
        let Some(dir) = dir else {
            let body = self.panel_empty(
                t(L10nKey::PanelNoWorkingDirectory),
                Some(t(L10nKey::PanelNoWorkingDirectoryHint)),
                cx,
            );
            return self.panel_scroll(body, title);
        };
        let rows = self.session_rows(window, cx);
        let home = crate::ui::desktop_lookups::home_for_host(cx, self.session_history.bound.host);
        let health = self.session_history.listing.health(rows.len());
        let selector = self.session_history_selector(window, &dir, home.as_deref(), cx);
        let body: AnyElement = match health {
            SessionHealth::Disconnected => v_flex()
                .child(selector)
                .child(self.panel_empty(
                    t(L10nKey::PanelSessionsDisconnected),
                    Some(t(L10nKey::PanelSessionsDisconnectedHint)),
                    cx,
                ))
                .into_any_element(),
            SessionHealth::Unsupported => v_flex()
                .child(selector)
                .child(self.panel_empty(
                    t(L10nKey::PanelSessionsUnsupported),
                    Some(t(L10nKey::PanelSessionsUnsupportedHint)),
                    cx,
                ))
                .into_any_element(),
            SessionHealth::Empty if !self.session_history.landed => v_flex()
                .child(selector)
                .child(self.panel_empty(t(L10nKey::PanelLoading), None, cx))
                .into_any_element(),
            SessionHealth::Empty => v_flex()
                .child(selector)
                .child(self.panel_empty(
                    t(L10nKey::PanelSessionsEmpty),
                    Some(t(L10nKey::PanelSessionsEmptyHint)),
                    cx,
                ))
                .into_any_element(),
            SessionHealth::Ready | SessionHealth::Partial => {
                // Clamp a selection left over from a taller list.
                if self
                    .session_history
                    .selected
                    .is_some_and(|s| s >= rows.len())
                {
                    self.session_history.selected = rows.len().checked_sub(1);
                }
                let selected = self.session_history.selected;
                let list_focus = self.session_history_focus.clone();
                let mut list = v_flex()
                    .id("panel-sessions-list")
                    .test_support()
                    .track_focus(&list_focus)
                    .key_context("List")
                    .on_action(cx.listener(|this, _: &SelectUp, window, cx| {
                        this.move_history_selection(false, window, cx);
                    }))
                    .on_action(cx.listener(|this, _: &SelectDown, window, cx| {
                        this.move_history_selection(true, window, cx);
                    }))
                    .on_action(cx.listener(|this, _: &Confirm, window, cx| {
                        this.confirm_history_selection(window, cx);
                    }))
                    .on_key_down(
                        cx.listener(|this, ev: &gpui_kit::KeyDownEvent, window, cx| {
                            if crate::ui::search::is_edit_gesture(&ev.keystroke) {
                                this.open_history_actions(window, cx);
                                cx.stop_propagation();
                            }
                        }),
                    );
                for (ix, session) in rows.iter().enumerate() {
                    list =
                        list.child(self.session_history_row(ix, session, selected == Some(ix), cx));
                }
                let mut body = v_flex().child(selector).child(list);
                if health == SessionHealth::Partial {
                    body = body.child(
                        div()
                            .px(px(TEXT_INSET))
                            .py(px(4.))
                            .text_size(rems(META))
                            .text_color(cx.theme().muted_foreground)
                            .child(t(L10nKey::PanelSessionsPartial)),
                    );
                }
                body.into_any_element()
            }
        };
        self.panel_scroll(body, title)
    }

    /// The header's one row: the scope label, the directory it means, and
    /// the dropdown that re-points the listing at another checkout of the
    /// same repository or back at the context pane.
    fn session_history_selector(
        &mut self,
        window: &mut Window,
        dir: &Path,
        home: Option<&Path>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        use gpui_kit::component::button::ButtonVariants as _;
        let muted = cx.theme().muted_foreground;
        let fg = cx.theme().foreground;
        self.load_session_worktrees(window, dir, cx);
        let follow = matches!(self.session_history.follow, SessionDirectory::FollowCwd);
        let pinned = match &self.session_history.follow {
            SessionDirectory::Worktree(path) => Some(path.clone()),
            SessionDirectory::FollowCwd => None,
        };
        let label = match follow {
            true => t(L10nKey::PanelSessionsFollowCwd),
            false => t(L10nKey::PanelSessionsWorktrees),
        };
        let shown: gpui_kit::SharedString = crate::ui::home::display_path(dir, home).into();
        let full_path: gpui_kit::SharedString =
            lyrel_ui_base::path_display::readable_path(&dir.to_string_lossy())
                .into_owned()
                .into();
        let worktrees: Vec<(PathBuf, gpui_kit::SharedString)> = self
            .session_history
            .worktrees
            .iter()
            .map(|wt| {
                (
                    wt.path.clone(),
                    crate::ui::home::display_path(&wt.path, home).into(),
                )
            })
            .collect();
        let menu_follow = t(L10nKey::PanelSessionsFollowCwd);
        let menu_section = t(L10nKey::PanelSessionsWorktrees);
        let app = cx.entity().downgrade();
        h_flex()
            .id("panel-sessions-scope")
            .test_support()
            .flex_none()
            .items_center()
            .gap(px(6.))
            .h(px(SCOPE_ROW_H))
            .pl(px(CONTENT_INSET))
            .pr(px(CONTENT_INSET))
            .child(
                div()
                    .flex_none()
                    .text_size(rems(META))
                    .text_color(muted)
                    .child(label),
            )
            .child(
                div().flex_1().min_w_0().child(
                    gpui_kit::component::button::Button::new("panel-sessions-directory")
                        .ghost()
                        .small()
                        .w_full()
                        .dropdown_caret(true)
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis_start()
                                .text_size(rems(TEXT))
                                .text_color(fg)
                                .child(shown.clone()),
                        )
                        .accessibility_label(full_path.clone())
                        .tooltip(full_path)
                        .dropdown_menu_with_anchor(
                            gpui_kit::Anchor::BottomLeft,
                            move |menu, window, _cx| {
                                let follow_app = app.clone();
                                let mut menu = menu.min_w(window.rem_size() * 12.5).item(
                                    PopupMenuItem::new(menu_follow).checked(follow).on_click(
                                        move |_, _, cx| {
                                            let _ = follow_app.update(cx, |this, cx| {
                                                this.pick_session_directory(
                                                    SessionDirectory::FollowCwd,
                                                    cx,
                                                );
                                            });
                                        },
                                    ),
                                );
                                if !worktrees.is_empty() {
                                    menu =
                                        menu.separator().item(PopupMenuItem::label(menu_section));
                                    for (path, display) in &worktrees {
                                        let path = path.clone();
                                        let checked = pinned.as_deref() == Some(path.as_path());
                                        menu = menu.item(
                                            PopupMenuItem::new(display.clone())
                                                .checked(checked)
                                                .on_click({
                                                    let app = app.clone();
                                                    move |_, _window, cx| {
                                                        let _ = app.update(cx, |this, cx| {
                                                            this.pick_session_directory(
                                                                SessionDirectory::Worktree(
                                                                    path.clone(),
                                                                ),
                                                                cx,
                                                            );
                                                        });
                                                    }
                                                }),
                                        );
                                    }
                                }
                                menu
                            },
                        ),
                ),
            )
            .into_any_element()
    }

    /// One compact row: the agent's disc, the session's title, and its
    /// relative time. The path stays in the selector — it is not repeated
    /// per row. Selected or hovered, the row says what the keyboard can do.
    fn session_history_row(
        &mut self,
        ix: usize,
        session: &PastSession,
        selected: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let sf = cx.global::<lyrel_ui_base::presets::Surfaces>().sidebar;
        let fg = cx.theme().foreground;
        let muted = cx.theme().muted_foreground;
        let now = lyrel_core::core::config::unix_now();
        let id = gpui_kit::SharedString::from(format!("panel-sessions-row-{ix}"));
        let focus = self.session_history_focus.clone();
        let resume = {
            let session = session.clone();
            let host = self.session_history.bound.host;
            cx.listener(move |this, _, window, cx| {
                this.activate_history_session(&session, host, window, cx);
            })
        };
        let menu_session = session.clone();
        let menu_host = self.session_history.bound.host;
        let app = cx.entity().downgrade();
        let actions_menu = self
            .session_history
            .actions_row
            .filter(|row| *row == ix)
            .and_then(|_| self.session_history.actions_menu.clone());

        let mut row = h_flex()
            .id(id.clone())
            .test_support()
            .group(id.clone())
            .w_full()
            .h(px(ROW_H))
            .px(px(TEXT_INSET))
            .items_center()
            .gap(px(8.))
            .cursor_pointer()
            .when(selected, |r| r.bg(gpui_kit::rgb(sf.selected)))
            .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                cx.stop_propagation();
                focus.focus(window, cx);
            })
            .on_click(resume)
            .context_menu(move |menu, _window, _cx| {
                LyrelApp::history_session_menu(menu, &menu_session, menu_host, &app)
            })
            .child(crate::ui::tab_strip::avatar(
                ("panel-sessions-avatar", ix),
                crate::ui::search::Avatar {
                    agent: Some(session.agent),
                    ..Default::default()
                },
                ROW_AVATAR,
                cx,
            ))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(rems(TEXT))
                    .text_color(fg)
                    .child(session.title.clone()),
            )
            .child(
                div()
                    .flex_none()
                    .text_size(rems(META))
                    .text_color(muted)
                    .child(crate::ui::home::relative_time(now, session.updated)),
            );
        if selected {
            row = row.child(
                h_flex()
                    .flex_none()
                    .items_center()
                    .gap(px(6.))
                    .text_size(rems(META))
                    .text_color(muted)
                    .child(crate::ui::dialog::chord(["↵"], cx))
                    .child(t(L10nKey::SessionActionResume))
                    .child(crate::ui::dialog::chord(
                        crate::ui::keymap::key_tokens("secondary-e"),
                        cx,
                    ))
                    .child(t(L10nKey::SessionActionsHint)),
            );
        }
        if let Some(menu) = actions_menu {
            row = row.relative().child(
                deferred(
                    anchored()
                        .anchor(gpui_kit::Anchor::BottomRight)
                        .snap_to_window_with_margin(px(8.))
                        .child(menu),
                )
                .with_priority(gpui_kit::base::POPUP_PRIORITY),
            );
        }
        row.into_any_element()
    }

    /// Re-point the listing: a checkout of the same repository, or back at
    /// the context pane's live directory. Only the listing moves — no pane
    /// changes — and the next render's sync rescans for the new bound.
    fn pick_session_directory(&mut self, follow: SessionDirectory, cx: &mut Context<Self>) {
        self.session_history.follow = follow;
        self.session_history.selected = None;
        self.session_history.close_actions(cx);
        cx.notify();
    }

    /// Activate a history row: hand the session's full identity to the
    /// shared resume path. Where the session opens — an already-open pane,
    /// a reused idle Shell, or a new pane to the right — is the restorer's
    /// decision (#365); this tool only ever submits the identity.
    fn activate_history_session(
        &mut self,
        session: &PastSession,
        host: HostId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.resume_session(
            crate::ui::agent_launch::ResumeSessionRequest {
                agent: session.agent,
                session_id: session.id.clone(),
                cwd: session.cwd.clone(),
                host_id: host,
                scope: session.scope.clone(),
                fork: false,
            },
            window,
            cx,
        );
    }

    /// The action menu one session row can do: resume, fork where the agent
    /// supports it, copy the native ID, hide from Lyrel's lists.
    fn history_session_menu(
        mut menu: PopupMenu,
        session: &PastSession,
        host: HostId,
        app: &gpui_kit::WeakEntity<LyrelApp>,
    ) -> PopupMenu {
        let resume_session = session.clone();
        menu = menu.item(
            PopupMenuItem::new(t(L10nKey::SessionActionResume)).on_click({
                let app = app.clone();
                move |_, window, cx| {
                    let session = resume_session.clone();
                    let _ = app.update(cx, |this, cx| {
                        this.activate_history_session(&session, host, window, cx);
                    });
                }
            }),
        );
        if session.agent.fork_label().is_some() {
            let fork_session = session.clone();
            menu = menu.item(PopupMenuItem::new(t(L10nKey::CmdForkSession)).on_click({
                let app = app.clone();
                move |_, window, cx| {
                    let session = fork_session.clone();
                    let _ = app.update(cx, |this, cx| {
                        this.resume_session(
                            crate::ui::agent_launch::ResumeSessionRequest {
                                agent: session.agent,
                                session_id: session.id.clone(),
                                cwd: session.cwd.clone(),
                                host_id: host,
                                scope: session.scope.clone(),
                                fork: true,
                            },
                            window,
                            cx,
                        );
                    });
                }
            }));
        }
        let copy_id = session.id.clone();
        menu = menu.item(PopupMenuItem::new(t(L10nKey::CmdCopySessionId)).on_click(
            move |_, _, cx| {
                cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(copy_id.clone()));
            },
        ));
        let hide_agent = session.agent;
        let hide_id = session.id.clone();
        menu.item(PopupMenuItem::new(t(L10nKey::SessionActionHide)).on_click({
            let app = app.clone();
            move |_, _window, cx| {
                let _ = app.update(cx, |this, cx| {
                    this.update_config(cx, |cfg| {
                        cfg.hidden_agent_sessions
                            .insert(crate::core::agent_history::session_key(
                                hide_agent, &hide_id,
                            ));
                    });
                });
            }
        }))
    }

    /// The row the keyboard is on gets the action menu: the same content
    /// the row's context menu shows, opened from the keyboard with the same
    /// gesture Search Everywhere uses.
    fn open_history_actions(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ix) = self.session_history.selected else {
            return;
        };
        let rows = self.session_rows(window, cx);
        let Some(session) = rows.get(ix).cloned() else {
            return;
        };
        let host = self.session_history.bound.host;
        let app = cx.entity().downgrade();
        let menu = PopupMenu::build(window, cx, move |menu, _window, _cx| {
            LyrelApp::history_session_menu(menu, &session, host, &app)
        });
        let focus = menu.read(cx).focus_handle(cx).clone();
        window.focus(&focus, cx);
        self.session_history.actions_row = Some(ix);
        self.session_history.actions_menu = Some(menu);
        // A dismissed menu takes its row with it. The subscription outlives
        // the menu on purpose: it only clears state that is already gone.
        cx.subscribe(
            self.session_history.actions_menu.as_ref().unwrap(),
            move |this, _, _: &gpui_kit::DismissEvent, cx| {
                this.session_history.actions_row = None;
                this.session_history.actions_menu = None;
                cx.notify();
            },
        )
        .detach();
        cx.notify();
    }

    fn move_history_selection(&mut self, forward: bool, window: &Window, cx: &mut Context<Self>) {
        let rows = self.session_rows(window, cx);
        if rows.is_empty() {
            return;
        }
        let next = match self.session_history.selected {
            None => match forward {
                true => 0,
                false => rows.len() - 1,
            },
            Some(ix) => match forward {
                true => (ix + 1).min(rows.len() - 1),
                false => ix.saturating_sub(1),
            },
        };
        if self.session_history.selected != Some(next) {
            self.session_history.selected = Some(next);
            // Keep the keyboard's row in view: rows share one fixed pitch
            // under the selector, so the offset is arithmetic.
            self.right_panel.scroll.set_offset(gpui_kit::point(
                px(0.),
                px(SCOPE_ROW_H + next as f32 * ROW_H),
            ));
            cx.notify();
        }
    }

    fn confirm_history_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rows = self.session_rows(window, cx);
        let Some(ix) = self.session_history.selected else {
            return;
        };
        let Some(session) = rows.get(ix).cloned() else {
            return;
        };
        let host = self.session_history.bound.host;
        self.activate_history_session(&session, host, window, cx);
    }

    /// Load the other checkouts of the repository `dir` is in, for the
    /// selector's menu. A directory that is not in a repository offers no
    /// worktrees — the menu shows only "Follow Current Directory".
    fn load_session_worktrees(&mut self, window: &Window, dir: &Path, cx: &mut Context<Self>) {
        if self.session_history.loading_worktrees
            || self.session_history.worktrees_for.as_deref() == Some(dir)
        {
            return;
        }
        let Some(host) =
            lyrel_ui_base::host_registry::HostRegistry::lookup(cx, self.session_history.bound.host)
        else {
            return;
        };
        self.session_history.loading_worktrees = true;
        self.session_history.worktrees_for = Some(dir.to_path_buf());
        let dir = dir.to_path_buf();
        let claimed = dir.clone();
        HostOps::run_in(
            host,
            window,
            cx,
            move |host| lyrel_core::core::worktree::list(host, &dir),
            move |app, listed, _window, cx| {
                app.session_history.loading_worktrees = false;
                // A newer directory may have claimed the slot meanwhile; the
                // worktrees belong to the repository the claim names.
                if app.session_history.worktrees_for.as_deref() == Some(&claimed) {
                    app.session_history.worktrees = listed.unwrap_or_default();
                    cx.notify();
                }
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lyrel_core::host::Host as _;

    fn error(kind: SessionErrorKind) -> SessionOperationError {
        SessionOperationError {
            kind,
            message: "test".into(),
            retained: None,
        }
    }

    /// Empty, unavailable and incomplete discovery are distinct states the
    /// view must tell apart; the classifier is the one place that decides.
    /// A provider's `Connection` error is a per-source failure — only the
    /// transport's own verdict reads as Disconnected.
    #[test]
    fn health_distinguishes_empty_disconnected_unsupported_and_partial() {
        let ready = SessionListing {
            sessions: vec![PastSession {
                agent: CLIAgent::Pi,
                id: "a".into(),
                cwd: None,
                title: String::new(),
                branch: None,
                updated: 0,
                scope: None,
            }],
            errors: vec![],
            host_reachable: true,
        };
        assert_eq!(ready.health(1), SessionHealth::Ready);
        assert_eq!(
            ready.health(0),
            SessionHealth::Empty,
            "a filter can empty a ready listing"
        );

        let partial = SessionListing {
            sessions: ready.sessions.clone(),
            errors: vec![error(SessionErrorKind::Unsupported)],
            host_reachable: true,
        };
        assert_eq!(partial.health(1), SessionHealth::Partial);

        // A provider failing with the host up (e.g. an OpenCode service with
        // no TUI) is not a disconnected host.
        let provider_connection = SessionListing {
            sessions: vec![],
            errors: vec![error(SessionErrorKind::Connection)],
            host_reachable: true,
        };
        assert_eq!(provider_connection.health(0), SessionHealth::Unsupported);

        let disconnected = SessionListing {
            sessions: vec![],
            errors: vec![error(SessionErrorKind::Connection)],
            host_reachable: false,
        };
        assert_eq!(disconnected.health(0), SessionHealth::Disconnected);

        let unsupported = SessionListing {
            sessions: vec![],
            errors: vec![error(SessionErrorKind::Unsupported)],
            host_reachable: true,
        };
        assert_eq!(unsupported.health(0), SessionHealth::Unsupported);

        assert_eq!(
            SessionListing::default().health(0),
            SessionHealth::Empty,
            "no scan and no errors is a genuine empty, not an error"
        );
    }

    /// A scope that names no store cannot confirm whose history a reached
    /// OpenCode service listed; the default scope is exactly that, while an
    /// observed or configured scope names its store.
    #[test]
    fn opencode_storage_confirmation_tracks_scope_selectors() {
        use lyrel_core::host::session_operations::SessionScope;
        assert!(
            !opencode_storage_confirmed(&SessionScope::default()),
            "the default service scope names no store"
        );
        let mut scoped = SessionScope::default();
        scoped
            .environment
            .insert("XDG_DATA_HOME".into(), "/tmp/isolated".into());
        assert!(
            opencode_storage_confirmed(&scoped),
            "a scope carrying store selectors confirms its storage"
        );
    }

    /// The tool's config key round-trips. A build without the variant does
    /// not know `"sessions"`; `Config`'s lenient tab field folds the unknown
    /// value to Info instead of failing the whole config — the same
    /// rollback cost Search had.
    #[test]
    fn sessions_config_key_round_trips() {
        let json =
            serde_json::to_string(&lyrel_core::core::config::RightPanelTab::Sessions).unwrap();
        assert_eq!(json, "\"sessions\"");
        let back: lyrel_core::core::config::RightPanelTab = serde_json::from_str(&json).unwrap();
        assert_eq!(back, lyrel_core::core::config::RightPanelTab::Sessions);
    }

    /// The catalog registers the tool once, with ids unique among the tools.
    #[test]
    fn sessions_tool_is_in_the_catalog_with_unique_ids() {
        use lyrel_core::core::config::RightPanelTab;
        let entries: Vec<_> = crate::ui::right_panel::TOOL_HOST_TOOLS
            .into_iter()
            .filter(|(tab, _, _)| *tab == RightPanelTab::Sessions)
            .collect();
        assert_eq!(entries.len(), 1, "the tool is registered exactly once");
        let mut ids: Vec<_> = [
            RightPanelTab::Info,
            RightPanelTab::Files,
            RightPanelTab::Scm,
            RightPanelTab::GitHub,
            RightPanelTab::Sessions,
        ]
        .into_iter()
        .map(crate::ui::right_panel::tool_tab_id)
        .collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), 5, "every tool's tab id is distinct");
    }

    /// Managed-remote scope fencing through the real transport: the remote
    /// Host's own scan lists what lives on the far machine, binds the
    /// sessions to the remote identity, and reports the sources it could not
    /// list (e.g. an unavailable provider launcher) as errors while keeping
    /// the successful records. Seeding runs against the fixture's disposable remote
    /// home through the production SFTP Host.
    #[gpui_kit::test]
    #[ignore = "requires isolated pinned native SSH/SFTP, same-commit server, and explicit LYREL_TEST_SSH_* endpoint; run alone"]
    fn native_ssh_session_listing_is_host_scoped_and_partial_aware(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        let binary = std::path::PathBuf::from(
            std::env::var_os("LYREL_TEST_SSH_LOCAL_SERVER").expect("same-commit server"),
        );
        let fixture = crate::native_ssh_graphics_fixture::NativeSshFixture::start(&binary);
        lyrel_core::core::config::set_config_dir(fixture.local_config_dir());
        let host = fixture.connect_control();
        assert!(host.is_connected());
        let remote_home = std::path::PathBuf::from(fixture.remote_home());
        // One disposable pi session on the far machine, in the layout the
        // real scanner reads. It names the remote cwd it ran in.
        let record = concat!(
            "{\"type\":\"session\",\"id\":\"remote-pi\",\"cwd\":\"/tmp/remote-project\"}\n",
            "{\"type\":\"message\",\"message\":{\"role\":\"user\",\"content\":\"scoped work\"}}\n"
        );
        let seeded = host.join(&remote_home, ".pi/agent/sessions/r/remote-pi.jsonl");
        host.create_dir(seeded.parent().unwrap(), true).unwrap();
        host.write_file(&seeded, record.as_bytes()).unwrap();

        let agent_launch = std::collections::HashMap::from([(
            CLIAgent::Hermes.slug().to_owned(),
            format!("{}/unavailable-hermes-launcher", fixture.remote_home()),
        )]);
        // The remote Pi store answers even if a separate Hermes launcher
        // cannot run. Keep the failure independent of the remote host's PATH.
        let scan = cx.update(|cx| {
            plan_scan(
                host.id(),
                &[],
                &CLIAgent::ALL,
                &remote_home,
                &agent_launch,
                cx,
            )
        });
        let listing = collect_sessions(&*host, scan);
        let found = listing
            .sessions
            .iter()
            .find(|s| s.id == "remote-pi")
            .expect("the remote store's session is listed through the real transport");
        assert_eq!(found.agent, CLIAgent::Pi);
        assert_eq!(
            found.cwd.as_deref(),
            Some(std::path::Path::new("/tmp/remote-project"))
        );
        assert!(
            !listing.errors.is_empty(),
            "a source the remote cannot list is reported, not swallowed"
        );
        assert_eq!(
            listing.health(1),
            SessionHealth::Partial,
            "sessions plus per-source errors read as a partial result"
        );

        // A dropped link reads as disconnected, never as an empty store.
        let mut fixture = fixture;
        fixture.restart_router();
        let scan = cx.update(|cx| {
            plan_scan(
                host.id(),
                &[],
                &CLIAgent::ALL,
                &remote_home,
                &agent_launch,
                cx,
            )
        });
        let listing = collect_sessions(&*host, scan);
        assert_eq!(listing.health(0), SessionHealth::Disconnected);
    }
}
