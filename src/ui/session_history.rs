//! Directory-scoped Session History: the right panel's own tool, the shared
//! discovery behind it, and the visibility identity model every session
//! consumer hides through.
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
//! (#366) builds on the listing type and the [`SessionIdentity`] visibility
//! model both consumers hide through.

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
    /// At least one source answered its listing successfully, even with an
    /// empty result: the membership is known, so zero rows reads as a
    /// genuinely empty directory — not as a host that could not list at
    /// all. Every source failing is the other sentence (Unsupported).
    pub(crate) any_listed: bool,
}

impl Default for SessionListing {
    fn default() -> Self {
        Self {
            sessions: Vec::new(),
            errors: Vec::new(),
            host_reachable: true,
            any_listed: false,
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
        if self.errors.is_empty() || self.any_listed {
            // No errors is a genuine empty; so is a membership some source
            // listed successfully (empty or not) with other sources failing
            // beside — the directory was read, and it has no saved sessions.
            // Only a scan where nothing could be listed at all is
            // Unsupported.
            return SessionHealth::Empty;
        }
        SessionHealth::Unsupported
    }
}

/// The identity behind one Lyrel visibility decision. Session History,
/// Search Everywhere and Sessions settings (#366) all name a session by
/// this, so a hide in one place reads the same in the others.
///
/// `key()` is the spelling `Config::hidden_agent_sessions` has always used —
/// `<agent slug>:<session id>`, host-independent — so preferences written
/// before Sessions settings existed keep matching. Everything else the
/// listing preserved (`cwd`, `scope`) rides along so a future consumer can
/// resolve the exact store the session lives in without rescanning.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SessionIdentity {
    pub(crate) host: HostId,
    pub(crate) agent: CLIAgent,
    pub(crate) id: String,
    pub(crate) cwd: Option<PathBuf>,
    pub(crate) scope: Option<lyrel_core::host::session_operations::SessionScope>,
}

/// Where one session's records live, as exactly as the scan could name it.
/// Deletion (#367) and the settings eligibility read this: a session whose
/// store cannot be confirmed must never read as deletable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SessionStore {
    /// The exact session file the scan located (the native pi/Oh My Pi/Kimi
    /// stores name one).
    File(PathBuf),
    /// The store the scope names without an exact file: the agent profile,
    /// the provider-specific store root, the session directory when known,
    /// and OpenCode's saved connection selector when it alone addresses the
    /// store.
    Named {
        profile: Option<String>,
        root: Option<String>,
        dir: Option<PathBuf>,
        connection: Option<String>,
    },
    /// The listing preserved nothing addressable.
    Unconfirmed,
}

/// Environment keys that locate an agent's store. Terminal and rendering
/// selectors (`TERM_PROGRAM`, `KITTY_WINDOW_ID`, image protocol flags, ...)
/// never do: two panes in different terminals share one store. Kept inside
/// `durable_scope`'s allow-list — everything here is durable.
const STORE_ROOT_ENV: &[&str] = &[
    "PI_CODING_AGENT_DIR",
    "PI_CODING_AGENT_SESSION_DIR",
    "PI_CONFIG_DIR",
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
    "XDG_STATE_HOME",
    "XDG_CACHE_HOME",
    "KIMI_CODE_HOME",
    "OPENCODE_DB",
    "HERMES_HOME",
    "HERMES_RUNTIME_DIR",
    "HOME",
    "USERPROFILE",
];

/// The provider-specific store root this scope carries, most exact first:
/// an explicit Hermes home or OMP config root beats a generic HOME, and the
/// generic fallback only applies to providers whose store lives under it.
fn store_root(
    agent: CLIAgent,
    scope: &lyrel_core::host::session_operations::SessionScope,
) -> Option<String> {
    let env = &scope.environment;
    let pick = |keys: &[&str]| keys.iter().find_map(|key| env.get(*key).cloned());
    match agent {
        CLIAgent::Hermes => pick(&["HERMES_HOME", "HERMES_RUNTIME_DIR", "HOME", "USERPROFILE"]),
        CLIAgent::OhMyPi => pick(&[
            "PI_CODING_AGENT_DIR",
            "PI_CONFIG_DIR",
            "XDG_CONFIG_HOME",
            "XDG_DATA_HOME",
            "HOME",
            "USERPROFILE",
        ]),
        CLIAgent::Pi => pick(&["PI_CODING_AGENT_DIR", "PI_CODING_AGENT_SESSION_DIR", "HOME"]),
        CLIAgent::Kimi => pick(&["KIMI_CODE_HOME", "HOME", "USERPROFILE"]),
        CLIAgent::OpenCode => pick(&["OPENCODE_DB", "XDG_DATA_HOME", "HOME", "USERPROFILE"]),
    }
}

/// The durable selectors that decide store identity — profile, store-root
/// environment, and OpenCode's saved connection — with live bridges dropped.
/// Terminal/rendering environment and the shell program deliberately stay
/// out: they describe a pane, not a store, and comparing them would miss a
/// live writer running in another terminal.
fn store_selectors(
    scope: &lyrel_core::host::session_operations::SessionScope,
) -> (Option<String>, Vec<(String, String)>, Vec<String>) {
    let scope = lyrel_core::host::session_operations::durable_scope(scope);
    let roots = scope
        .environment
        .iter()
        .filter(|(key, _)| STORE_ROOT_ENV.contains(&key.as_str()))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    let connection = scope
        .opencode
        .map(|opencode| opencode.connection_argv)
        .unwrap_or_default();
    (scope.profile, roots, connection)
}

impl SessionIdentity {
    pub(crate) fn of(host: HostId, session: &PastSession) -> Self {
        Self {
            host,
            agent: session.agent,
            id: session.id.clone(),
            cwd: session.cwd.clone(),
            scope: session.scope.clone(),
        }
    }

    /// The scoped spelling every hide writer persists: agent slug, owning
    /// host, the exact durable native store when the scan preserved one, and
    /// the native id. Two same-id sessions on different hosts or stores keep
    /// distinct keys, so a hide never reaches past the row it named.
    pub(crate) fn key(&self) -> String {
        format!(
            "{}@{}|{}:{}",
            self.agent.slug(),
            self.host.0,
            scoped_store_key(self.scope.as_ref()),
            escape_key_component(&self.id),
        )
    }

    /// The pre-settings spelling — `<agent slug>:<session id>`. Read-only
    /// backward compatibility: entries already in `hidden_agent_sessions`
    /// keep matching every same-id row until they are removed or an exact
    /// scope override (`show_key()`) re-shows one of them.
    pub(crate) fn legacy_key(&self) -> String {
        crate::core::agent_history::session_key(self.agent, &self.id)
    }

    /// The exact-scope visibility override: `!` + `key()`. Written only when
    /// a row hidden by the legacy `slug:id` spelling is re-shown — removing
    /// the legacy entry would re-show every same-id row, so this marker
    /// names the one identity that reads as visible again.
    pub(crate) fn show_key(&self) -> String {
        format!("!{}", self.key())
    }

    /// Whether two identities name the same stored session on the same host.
    /// Scopes compare by their durable store selectors (environment keys the
    /// store answers from, profile, OpenCode storage) — never by the live
    /// bridge fields or the exact session file, which the running pane's
    /// readout may not repeat.
    pub(crate) fn same_session(&self, other: &Self) -> bool {
        self.host == other.host
            && self.agent == other.agent
            && self.id == other.id
            && same_store_scope(self.scope.as_ref(), other.scope.as_ref())
    }

    /// The most exact store the scan preserved for this session: the session
    /// file when one was located, otherwise the provider-specific durable
    /// selectors (profile, store root, session directory, OpenCode
    /// connection) the scope names.
    pub(crate) fn store(&self) -> SessionStore {
        let Some(scope) = &self.scope else {
            return SessionStore::Unconfirmed;
        };
        if let Some(file) = &scope.session_file {
            return SessionStore::File(file.clone());
        }
        let root = store_root(self.agent, scope);
        let dir = scope.session_dir.clone();
        let connection = scope
            .opencode
            .as_ref()
            .filter(|_| self.agent == CLIAgent::OpenCode)
            .filter(|opencode| !opencode.connection_argv.is_empty())
            .map(|opencode| opencode.connection_argv.join(" "));
        match (scope.profile.clone(), root, dir, connection) {
            (None, None, None, None) => SessionStore::Unconfirmed,
            (profile, root, dir, connection) => SessionStore::Named {
                profile,
                root,
                dir,
                connection,
            },
        }
    }
}

/// Store identity by durable selectors only — profile, store-root
/// environment, OpenCode's saved connection. The exact file and session
/// directory never decide: the scan's scope points at the file it read
/// while a running pane's readout names the store, and terminal/rendering
/// environment plus the shell program describe a pane, not a store. Two
/// `None`s name the same default store; a missing side against a present
/// side does not.
fn same_store_scope(
    a: Option<&lyrel_core::host::session_operations::SessionScope>,
    b: Option<&lyrel_core::host::session_operations::SessionScope>,
) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => store_selectors(a) == store_selectors(b),
        _ => false,
    }
}

/// The durable-store component of a scoped key: the session directory, the
/// exact file when the scan located one, and the same provider selectors
/// that decide store identity (profile, store-root environment, OpenCode
/// connection). Terminal and rendering environment stays out so the key is
/// stable across scans from different terminals. `none` when the listing
/// retained no scope — the key still distinguishes hosts and ids.
fn scoped_store_key(scope: Option<&lyrel_core::host::session_operations::SessionScope>) -> String {
    let Some(scope) = scope else {
        return "none".into();
    };
    let (profile, roots, connection) = store_selectors(scope);
    let mut parts: Vec<String> = Vec::new();
    if let Some(profile) = &profile {
        parts.push(format!("p={}", escape_key_component(profile)));
    }
    if let Some(dir) = &scope.session_dir {
        parts.push(format!(
            "d={}",
            escape_key_component(&dir.to_string_lossy())
        ));
    }
    if let Some(file) = &scope.session_file {
        parts.push(format!(
            "f={}",
            escape_key_component(&file.to_string_lossy())
        ));
    }
    for (key, value) in &roots {
        parts.push(format!(
            "e={}={}",
            escape_key_component(key),
            escape_key_component(value)
        ));
    }
    if !connection.is_empty() {
        let argv = connection
            .iter()
            .map(|arg| escape_key_component(arg))
            .collect::<Vec<_>>()
            .join(";");
        parts.push(format!("o={argv}"));
    }
    match parts.is_empty() {
        true => "none".into(),
        false => parts.join(","),
    }
}

/// Percent-escape every byte outside the unreserved set, so two adjacent key
/// components can never blur into each other whatever a native id, a profile
/// name, or a remote path happens to contain.
fn escape_key_component(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for byte in raw.bytes() {
        match byte {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(char::from(byte))
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// Whether Lyrel's lists forget this session: the one predicate Session
/// History, Search Everywhere and Sessions settings all filter through.
/// Hidden when the scoped key is present, or when the legacy `slug:id`
/// spelling is present without this identity's exact-scope override.
pub(crate) fn is_session_hidden(cfg: &Config, identity: &SessionIdentity) -> bool {
    let hidden = &cfg.hidden_agent_sessions;
    hidden.contains(&identity.key())
        || (hidden.contains(&identity.legacy_key()) && !hidden.contains(&identity.show_key()))
}

/// Persist one hide: the scoped key names the exact row; a stale override
/// marker for the same identity is dropped so the row stays hidden.
pub(crate) fn hide_session(cfg: &mut Config, identity: &SessionIdentity) {
    cfg.hidden_agent_sessions.insert(identity.key());
    cfg.hidden_agent_sessions.remove(&identity.show_key());
}

/// Persist one redisplay. A scoped hide is simply removed; a legacy
/// `slug:id` hide covers every same-id row on every host, so re-showing only
/// this identity writes its exact-scope override instead of dropping the
/// legacy entry and exposing its siblings.
pub(crate) fn show_session(cfg: &mut Config, identity: &SessionIdentity) {
    cfg.hidden_agent_sessions.remove(&identity.key());
    if cfg.hidden_agent_sessions.contains(&identity.legacy_key()) {
        cfg.hidden_agent_sessions.insert(identity.show_key());
    }
}

/// The per-provider requests one scan runs: OpenCode and Hermes each answer
/// per observed storage scope, so a pane running under a non-default profile
/// or service keeps its history discoverable.
///
/// `directory` is the optional query the whole scan is scoped to: the owning
/// Host applies it to the file-scanned stores before their recent cap, and
/// the merged listing is filtered to it here. `None` lists everything
/// (Search Everywhere's Sessions tab); the Session History tool sets it to
/// the directory it is bound to. Consumers that cannot paginate exactly
/// against it (the OpenCode service window) stay explicit about their cap.
pub(crate) struct SessionScan {
    directory: Option<PathBuf>,
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
        directory: None,
        opencode: opencode_scopes,
        hermes: hermes_scopes,
        hermes_errors,
    }
}

/// Run one scan on its owning Host (blocking — call through `HostOps`).
/// `agent_sessions` covers pi/Oh My Pi/Kimi; the scoped requests cover
/// OpenCode and Hermes. A failing source is recorded as an error and never
/// empties what the other sources found. With `scan.directory`, the owning
/// Host filters the file-scanned stores before their recent cap and the
/// merged listing is filtered to the directory here; sources that cannot
/// paginate exactly against a directory (the OpenCode service's recency
/// window) keep their limit warning so a capped answer never reads complete.
///
/// This is the whole discovery in one blocking step — what Search
/// Everywhere's Sessions tab and Sessions settings (#366) run. The Session
/// History tool runs the very same pieces as two landings
/// ([`collect_native_sessions`] first, [`collect_service_sessions`] when the
/// slow sources answered, [`merge_listing`] for both) so the fast native
/// rows paint before the services do.
pub(crate) fn collect_sessions(
    host: &dyn lyrel_core::host::Host,
    scan: SessionScan,
) -> SessionListing {
    let directory = scan.directory.clone();
    let mut listing = collect_native_sessions(host, directory.as_deref());
    let services = collect_service_sessions(host, scan);
    merge_listing(host, directory.as_deref(), &mut listing, services);
    listing
}

/// The native-file portion of a scan: the pi/Oh My Pi/Kimi stores the
/// owning Host reads off its own disk. No service round-trips happen here —
/// on an isolated machine this is the part that answers in milliseconds —
/// so the Session History tool lands it on its own and keeps the slow
/// service portion pending beside it instead of waiting for it.
pub(crate) fn collect_native_sessions(
    host: &dyn lyrel_core::host::Host,
    directory: Option<&Path>,
) -> SessionListing {
    let mut listing = SessionListing::default();
    match host.agent_sessions(directory) {
        Ok(found) => {
            listing.any_listed = true;
            listing.sessions.extend(found);
        }
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
    listing
}

/// The service portion of a scan: OpenCode and Hermes answer their scoped
/// requests here, each separately, so one absent or slow service is
/// recorded beside the others and never blocks — or empties — the native
/// portion. The returned listing holds only what the services answered;
/// folding it into the native portion is [`merge_listing`]'s job.
pub(crate) fn collect_service_sessions(
    host: &dyn lyrel_core::host::Host,
    scan: SessionScan,
) -> SessionListing {
    let directory = scan.directory.clone();
    let mut listing = SessionListing {
        sessions: Vec::new(),
        errors: scan.hermes_errors,
        host_reachable: true,
        any_listed: false,
    };
    for request in &scan.opencode {
        match host.opencode_sessions(request) {
            Ok(sessions) => {
                listing.any_listed = true;
                // The service pages a fixed recency window; a scoped listing
                // cannot tell whether the directory's older records fell
                // past it, so a full window is said out loud instead of
                // reading as a complete answer.
                let window_full = directory.is_some()
                    && sessions.len() >= lyrel_core::core::agent_history::MAX_SESSIONS;
                for session in sessions {
                    if !listing
                        .sessions
                        .iter()
                        .any(|existing| same_listed_session(existing, &session))
                    {
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
                if window_full {
                    listing.errors.push(SessionOperationError {
                        kind: SessionErrorKind::Unsupported,
                        message: format!(
                            "OpenCode history reached its {}-session window; older sessions for this directory may be missing",
                            lyrel_core::core::agent_history::MAX_SESSIONS
                        ),
                        retained: None,
                    });
                }
            }
            Err(error) => listing.errors.push(error),
        }
    }
    for request in &scan.hermes {
        match host.hermes_sessions(request) {
            Ok(sessions) => {
                listing.any_listed = true;
                for session in sessions {
                    if !listing
                        .sessions
                        .iter()
                        .any(|existing| same_listed_session(existing, &session))
                    {
                        listing.sessions.push(session);
                    }
                }
            }
            Err(error) => listing.errors.push(error),
        }
    }
    listing
}

/// Fold a later portion into the listing the earlier portions built: a row
/// the listing already holds (by [`same_listed_session`]) is not repeated,
/// the portion's errors join the listing's, and the combined answer is then
/// filtered to the scan's directory with the owning Host's path semantics —
/// the same match the Host's own scanner used — and ordered most-recent-
/// first. Pure data work over one cheap `Host` read, so a landing callback
/// may run it on the UI thread; [`collect_sessions`] runs the same merge on
/// the Host's own thread.
pub(crate) fn merge_listing(
    host: &dyn lyrel_core::host::Host,
    directory: Option<&Path>,
    listing: &mut SessionListing,
    extra: SessionListing,
) {
    for session in extra.sessions {
        if !listing
            .sessions
            .iter()
            .any(|existing| same_listed_session(existing, &session))
        {
            listing.sessions.push(session);
        }
    }
    listing.errors.extend(extra.errors);
    listing.host_reachable &= extra.host_reachable;
    listing.any_listed |= extra.any_listed;
    // Service providers answer a recency window, not a directory query: the
    // merged listing is filtered here, with the owning Host's path semantics
    // (the scanner used the same match on its own machine), so every
    // consumer of the shared listing sees exactly the queried directory —
    // and a Windows client never merges a Linux host's /tmp/Repo with
    // /tmp/repo.
    if let Some(dir) = directory {
        listing.sessions.retain(|s| {
            s.cwd.as_deref().is_some_and(|cwd| {
                lyrel_core::core::agent_history::directory_matches_with(cwd, dir, host.separator())
            })
        });
    }
    listing
        .sessions
        .sort_by_key(|s| std::cmp::Reverse(s.updated));
}

/// Whether `incoming` is the row `existing` already lists: the identity the
/// portions dedup on. Same agent and id, plus the service-scoped selectors a
/// service row carries — an OpenCode row repeats on its server and
/// connection argv, a Hermes row on its whole scope — while the native
/// agents hold one row per id, so the agent and id are the whole identity
/// there.
fn same_listed_session(existing: &PastSession, incoming: &PastSession) -> bool {
    if existing.agent != incoming.agent || existing.id != incoming.id {
        return false;
    }
    match incoming.agent {
        CLIAgent::OpenCode => {
            existing
                .scope
                .as_ref()
                .and_then(|scope| scope.opencode.as_ref())
                .map(|scope| (&scope.server, &scope.connection_argv))
                == incoming
                    .scope
                    .as_ref()
                    .and_then(|scope| scope.opencode.as_ref())
                    .map(|scope| (&scope.server, &scope.connection_argv))
        }
        CLIAgent::Hermes => existing.scope == incoming.scope,
        _ => true,
    }
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

/// What each host's last *unscoped* session scan found: the placeholder
/// Search Everywhere's Sessions tab shows while its own query runs. Only
/// Search writes it — the Session History tool's scans are directory-scoped,
/// and caching those here would drop every other directory from the
/// placeholder.
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

/// The identity a started scan belongs to: its host, the actual connection,
/// and the directory the request was scoped to. Results landing for any
/// other generation are stale and retired; a bound that no longer equals the
/// wanted one is rescanned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SessionBound {
    pub(crate) host: HostId,
    /// The host connection the bound was claimed against — the registry's
    /// host object identity. A relink replaces that object, so a reconnect
    /// with an identical directory is a different bound and rescans; `0`
    /// means no live connection was found for the host.
    pub(crate) connection: usize,
    /// The repository the listed directory belongs to: recorded when a
    /// worktree is pinned, so the pin keeps its origin repository instead of
    /// silently re-resolving under whatever repository the path names now.
    /// `None` — the followed pane directory — is its own scope.
    pub(crate) repo: Option<PathBuf>,
    /// The directory sessions will be matched against. `None` — no context
    /// pane, or none with a directory — is not scannable and renders its
    /// own empty state rather than a listing.
    pub(crate) dir: Option<PathBuf>,
}

impl Default for SessionBound {
    fn default() -> Self {
        Self {
            host: HostId::LOCAL,
            connection: 0,
            repo: None,
            dir: None,
        }
    }
}

/// Where a pinned worktree's scope came from. The pin keeps this origin
/// until the user re-picks: a workspace relink must not rebind the listing
/// to the new host, and a path that now names another repository is a
/// different scope, not the same pin.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PinOrigin {
    host: HostId,
    connection: usize,
    repo: Option<PathBuf>,
}

/// The Session History tool's view state.
#[derive(Default)]
pub(crate) struct SessionHistoryPanel {
    follow: SessionDirectory,
    /// The origin host/connection/repository a pinned worktree was pinned
    /// on. `None` while following the context pane.
    pin_origin: Option<PinOrigin>,
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
    /// Keep the Session History listing bound to its scope: the context
    /// pane's host and directory, or the origin host and repository a
    /// worktree was pinned on. Driven from the window's render like the port
    /// watch: the context pane's directory has no event of its own, so the
    /// bound is recomputed each frame and a scan starts only when it moved.
    /// A scan already in flight never blocks the rebind — the new bound
    /// claims the panel and the old trip's landing retires itself on its
    /// generation and connection fences — so a slow service source cannot
    /// hold the old rows up or the new ones back.
    pub(crate) fn sync_session_history(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.right_panel_tab != lyrel_core::core::config::RightPanelTab::Sessions
            || !self.right_panel_visible
        {
            return;
        }
        let pane = self.context_pane(window, cx);
        let pane_dir = pane.as_ref().and_then(|pane| pane.read(cx).host_cwd());
        // The directory's owning host: the context pane's for the followed
        // directory, the pin's origin for a pinned worktree — never the
        // workspace's current host on its own, which a relink would silently
        // swap under a pinned listing.
        let (dir, host) = match &self.session_history.follow {
            SessionDirectory::Worktree(path) => (
                Some(path.clone()),
                self.session_history
                    .pin_origin
                    .as_ref()
                    .map_or_else(|| self.spawn_host(cx), |origin| origin.host),
            ),
            SessionDirectory::FollowCwd => (
                pane_dir,
                pane.as_ref()
                    .map_or_else(|| self.spawn_host(cx), |pane| pane.read(cx).host_id()),
            ),
        };
        let connection = lyrel_ui_base::host_registry::HostRegistry::lookup(cx, host)
            .map_or(0, |host| {
                std::sync::Arc::as_ptr(&host) as *const () as usize
            });
        let repo = match &self.session_history.follow {
            SessionDirectory::Worktree(_) => self
                .session_history
                .pin_origin
                .as_ref()
                .and_then(|origin| origin.repo.clone()),
            SessionDirectory::FollowCwd => None,
        };
        let wanted = SessionBound {
            host,
            connection,
            repo,
            dir,
        };
        if self.session_history.bound == wanted {
            return;
        }
        // The bound moved: the listing on screen belongs to the old scope,
        // so it comes down now — only the current generation may paint —
        // and the fast native landing puts the new scope's rows up.
        self.session_history.listing = SessionListing::default();
        self.session_history.landed = false;
        self.session_history.selected = None;
        self.session_history.close_actions(cx);
        // No directory (no pane yet): nothing to match, and scanning a
        // made-up home would list sessions the tool cannot scope.
        let Some(dir) = wanted.dir.clone() else {
            self.session_history.bound = wanted;
            self.session_history.scanning = false;
            self.session_history.landed = true;
            return;
        };
        let Some(host) = lyrel_ui_base::host_registry::HostRegistry::lookup(cx, wanted.host) else {
            // The host is gone from under the workspace: say disconnected
            // instead of waiting on a scan that has nobody to run on.
            self.session_history.bound = wanted;
            self.session_history.scanning = false;
            self.session_history.landed = true;
            self.session_history.listing = SessionListing {
                sessions: Vec::new(),
                errors: vec![SessionOperationError {
                    kind: SessionErrorKind::Connection,
                    message: "session history host is disconnected".into(),
                    retained: None,
                }],
                host_reachable: false,
                any_listed: false,
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
        let mut scan = plan_scan(
            self.session_history.bound.host,
            &panes,
            &agents,
            &dir,
            &cx.global::<Config>().agent_launch.clone(),
            cx,
        );
        // The tool lists one directory: the owning Host filters its
        // file-scanned stores to it before the recent cap, and service
        // sources are filtered to it when the listing merges.
        scan.directory = Some(dir);
        let host_id = self.session_history.bound.host;
        let connection = self.session_history.bound.connection;
        // The native portion answers in milliseconds; it lands on its own,
        // before the service portion is even asked. That trip starts from
        // the native landing, not here.
        let directory = scan.directory.clone();
        HostOps::run_in(
            host,
            window,
            cx,
            move |host| collect_native_sessions(host, directory.as_deref()),
            move |app, native, window, cx| {
                app.session_history_native_landed(
                    generation, host_id, connection, scan, native, window, cx,
                );
            },
        );
    }

    /// The native portion landed: it paints at once — the panel stays
    /// `scanning`, and the footer says the service portion is still out —
    /// and it starts that portion's trip from here. A landing whose
    /// generation is no longer the live one, or whose connection is not the
    /// object the registry holds anymore, retires without touching the
    /// panel: its answers belong to a scope the bound has already left.
    fn session_history_native_landed(
        &mut self,
        generation: u64,
        host_id: HostId,
        connection: usize,
        scan: SessionScan,
        native: SessionListing,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.session_history.generation != generation {
            return;
        }
        let Some(host) = lyrel_ui_base::host_registry::HostRegistry::lookup(cx, host_id) else {
            return;
        };
        if std::sync::Arc::as_ptr(&host) as *const () as usize != connection {
            self.session_history.scanning = false;
            return;
        }
        let old_rows = self.session_rows(window, cx);
        let directory = scan.directory.clone();
        let host_reachable = native.host_reachable;
        let mut listing = native;
        // One portion, one merge: the shared listing's directory filter and
        // ordering, with nothing else to fold in yet.
        merge_listing(
            &*host,
            directory.as_deref(),
            &mut listing,
            SessionListing::default(),
        );
        self.apply_history_landing(old_rows, listing, window, cx);
        // A host the native portion already reads as unreachable cannot
        // answer the services either (their own first check is the same
        // connection), so the pending state would be a wait for a known
        // answer: fail closed on the native portion's verdict instead.
        if !host_reachable {
            self.session_history.scanning = false;
            cx.notify();
            return;
        }
        HostOps::run_in(
            host,
            window,
            cx,
            move |host| collect_service_sessions(host, scan),
            move |app, services, window, cx| {
                app.session_history_services_landed(
                    generation, host_id, connection, directory, services, window, cx,
                );
            },
        );
    }

    /// The service portion landed: fold it into the listing the panel
    /// already shows and close the scan. Every source's final errors and
    /// limit warnings are on the listing now; the footer reads them. The
    /// same generation and connection fences as the native landing apply —
    /// a superseded service answer never repaints the current scope.
    fn session_history_services_landed(
        &mut self,
        generation: u64,
        host_id: HostId,
        connection: usize,
        directory: Option<PathBuf>,
        services: SessionListing,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.session_history.generation != generation {
            return;
        }
        let Some(host) = lyrel_ui_base::host_registry::HostRegistry::lookup(cx, host_id) else {
            return;
        };
        if std::sync::Arc::as_ptr(&host) as *const () as usize != connection {
            self.session_history.scanning = false;
            return;
        }
        self.session_history.scanning = false;
        let old_rows = self.session_rows(window, cx);
        let mut listing = std::mem::take(&mut self.session_history.listing);
        merge_listing(&*host, directory.as_deref(), &mut listing, services);
        self.apply_history_landing(old_rows, listing, window, cx);
    }

    /// Put one landed portion's listing up. The keyboard's row survives the
    /// relist only while the same session is still a row (a later portion
    /// reordering the list must not move the selection onto another
    /// session): it is re-pointed at the session's new index, and it is
    /// dropped when that session is gone. An open action menu is dropped
    /// whenever the row its index names changed — a menu that stayed would
    /// act on whatever row took the index.
    fn apply_history_landing(
        &mut self,
        old_rows: Vec<PastSession>,
        listing: SessionListing,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        self.session_history.listing = listing;
        self.session_history.landed = true;
        let new_rows = self.session_rows(window, cx);
        let same_row = |a: &PastSession, b: &PastSession| a.agent == b.agent && a.id == b.id;
        self.session_history.selected = self
            .session_history
            .selected
            .and_then(|ix| old_rows.get(ix))
            .and_then(|old| new_rows.iter().position(|new| same_row(old, new)));
        let menu_stale = match self.session_history.actions_row {
            Some(ix) => match (old_rows.get(ix), new_rows.get(ix)) {
                (Some(old), Some(new)) => !same_row(old, new),
                _ => true,
            },
            None => false,
        };
        if menu_stale {
            self.session_history.close_actions(cx);
        }
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
        let cfg = cx.global::<Config>();
        let host = self.session_history.bound.host;
        // The rows judge the owning Host's records by the owning Host's
        // path semantics (a remote Linux host stays case-sensitive even
        // under a Windows client), not by this machine's.
        let host_separator =
            lyrel_ui_base::host_registry::HostRegistry::lookup(cx, self.session_history.bound.host)
                .map_or(std::path::MAIN_SEPARATOR, |host| host.separator());
        let mut rows: Vec<PastSession> = self
            .session_history
            .listing
            .sessions
            .iter()
            .filter(|s| !is_session_hidden(cfg, &SessionIdentity::of(host, s)))
            .filter(|s| {
                s.cwd.as_deref().is_some_and(|cwd| {
                    crate::core::agent_history::directory_matches_with(cwd, &dir, host_separator)
                })
            })
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
            // Loading covers the first trip, every later scope change, and
            // however long the service portion is still out: with a scan in
            // flight, zero rows — Empty or Unsupported — is the trip's
            // in-flight state, never the listing's verdict. An early native
            // error with the services still pending is pending, not "cannot
            // list"; only the finished listing may say Unsupported.
            SessionHealth::Empty | SessionHealth::Unsupported
                if !self.session_history.landed || self.session_history.scanning =>
            {
                v_flex()
                    .child(selector)
                    .child(self.panel_empty(t(L10nKey::PanelLoading), None, cx))
                    .into_any_element()
            }
            SessionHealth::Unsupported => v_flex()
                .child(selector)
                .child(self.panel_empty(
                    t(L10nKey::PanelSessionsUnsupported),
                    Some(t(L10nKey::PanelSessionsUnsupportedHint)),
                    cx,
                ))
                .into_any_element(),
            SessionHealth::Empty => {
                // A directory some source listed successfully (its
                // membership is known) with other sources failing beside
                // still says which ones could not be listed.
                let mut body = v_flex().child(selector).child(self.panel_empty(
                    t(L10nKey::PanelSessionsEmpty),
                    Some(t(L10nKey::PanelSessionsEmptyHint)),
                    cx,
                ));
                if !self.session_history.listing.errors.is_empty() {
                    body = body.child(
                        div()
                            .id("panel-sessions-partial")
                            .test_support()
                            .px(px(TEXT_INSET))
                            .py(px(4.))
                            .text_size(rems(META))
                            .text_color(cx.theme().muted_foreground)
                            .child(t(L10nKey::PanelSessionsPartial)),
                    );
                }
                body.into_any_element()
            }
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
                if self.session_history.scanning {
                    // The native rows are up; the service portion is still
                    // out. Say so — this is not the listing's whole answer.
                    body = body.child(
                        div()
                            .id("panel-sessions-pending")
                            .test_support()
                            .px(px(TEXT_INSET))
                            .py(px(4.))
                            .text_size(rems(META))
                            .text_color(cx.theme().muted_foreground)
                            .child(t(L10nKey::PanelSessionsPending)),
                    );
                } else if health == SessionHealth::Partial {
                    body = body.child(
                        div()
                            .id("panel-sessions-partial")
                            .test_support()
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
    /// Pinning records where the pin was made — the host connection and the
    /// repository the worktree list belonged to — so the listing keeps that
    /// origin until the user re-picks instead of following the workspace's
    /// current host after a relink.
    fn pick_session_directory(&mut self, follow: SessionDirectory, cx: &mut Context<Self>) {
        self.session_history.pin_origin = match &follow {
            SessionDirectory::Worktree(_) => Some(PinOrigin {
                host: self.session_history.bound.host,
                connection: self.session_history.bound.connection,
                // The worktrees list belongs to the directory it was loaded
                // for; a stale list (another directory claimed the slot
                // meanwhile) says nothing about this pin's repository.
                repo: (self.session_history.worktrees_for.as_deref()
                    == self.session_history.bound.dir.as_deref())
                .then(|| {
                    self.session_history
                        .worktrees
                        .iter()
                        .find(|worktree| worktree.main)
                        .map(|worktree| worktree.path.clone())
                })
                .flatten(),
            }),
            SessionDirectory::FollowCwd => None,
        };
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
        let hide_host = host;
        let hide_scope = session.scope.clone();
        let hide_cwd = session.cwd.clone();
        menu.item(PopupMenuItem::new(t(L10nKey::SessionActionHide)).on_click({
            let app = app.clone();
            move |_, _window, cx| {
                let _ = app.update(cx, |this, cx| {
                    this.update_config(cx, |cfg| {
                        let identity = SessionIdentity {
                            host: hide_host,
                            agent: hide_agent,
                            id: hide_id.clone(),
                            cwd: hide_cwd.clone(),
                            scope: hide_scope.clone(),
                        };
                        hide_session(cfg, &identity);
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
    use gpui_kit::test::TestWindowExt as _;
    use lyrel_core::daemon::protocol::DaemonMsg;
    use lyrel_core::host::Host;
    use lyrel_ui_base::host_ops::{Entry, Meta, Output, SearchHit, SharedHost, WatchSub};
    use std::io::Write as _;
    use std::sync::{Arc, Mutex};

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
            any_listed: true,
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
            any_listed: true,
        };
        assert_eq!(partial.health(1), SessionHealth::Partial);

        // A provider failing with the host up (e.g. an OpenCode service with
        // no TUI) is not a disconnected host.
        let provider_connection = SessionListing {
            sessions: vec![],
            errors: vec![error(SessionErrorKind::Connection)],
            host_reachable: true,
            any_listed: false,
        };
        assert_eq!(provider_connection.health(0), SessionHealth::Unsupported);

        let disconnected = SessionListing {
            sessions: vec![],
            errors: vec![error(SessionErrorKind::Connection)],
            host_reachable: false,
            any_listed: false,
        };
        assert_eq!(disconnected.health(0), SessionHealth::Disconnected);

        let unsupported = SessionListing {
            sessions: vec![],
            errors: vec![error(SessionErrorKind::Unsupported)],
            host_reachable: true,
            any_listed: false,
        };
        assert_eq!(unsupported.health(0), SessionHealth::Unsupported);

        // A directory some source listed successfully (empty membership,
        // known) with other sources failing beside is an empty directory —
        // not a host that cannot list at all.
        let listed_empty_with_errors = SessionListing {
            sessions: vec![],
            errors: vec![error(SessionErrorKind::Unsupported)],
            host_reachable: true,
            any_listed: true,
        };
        assert_eq!(listed_empty_with_errors.health(0), SessionHealth::Empty);

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

    fn identity(agent: CLIAgent, id: &str, host: HostId, store: Option<&str>) -> SessionIdentity {
        let scope = store.map(|root| {
            let mut scope = lyrel_core::host::session_operations::SessionScope::default();
            scope.session_dir = Some(PathBuf::from(root));
            scope
                .environment
                .insert("PI_CODING_AGENT_DIR".into(), root.into());
            scope
        });
        SessionIdentity {
            host,
            agent,
            id: id.into(),
            cwd: None,
            scope,
        }
    }

    /// One native id can name sessions on two hosts and in two stores. The
    /// scoped key carries host + exact durable store + id, so hiding one row
    /// never collapses its same-id siblings.
    #[test]
    fn scoped_key_distinguishes_same_id_across_hosts_and_stores() {
        let local = identity(CLIAgent::Pi, "same", HostId::LOCAL, Some("/store/a"));
        let other_host = identity(
            CLIAgent::Pi,
            "same",
            HostId::from_connection_key("ssh-direct:me@box:22"),
            Some("/store/a"),
        );
        let other_store = identity(CLIAgent::Pi, "same", HostId::LOCAL, Some("/store/b"));
        let no_store = identity(CLIAgent::Pi, "same", HostId::LOCAL, None);

        let keys = [
            local.key(),
            other_host.key(),
            other_store.key(),
            no_store.key(),
        ];
        let mut unique = keys.to_vec();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), 4, "every scope keeps its own key: {keys:?}");
        assert!(keys.iter().all(|key| key.contains(":same")));
        // The legacy spelling is read-only compatibility, never written.
        assert_eq!(local.legacy_key(), "pi:same");

        let mut cfg = Config::default();
        hide_session(&mut cfg, &local);
        assert!(is_session_hidden(&cfg, &local));
        assert!(
            !is_session_hidden(&cfg, &other_host),
            "a same-id row on another host stays visible"
        );
        assert!(
            !is_session_hidden(&cfg, &other_store),
            "a same-id row in another store stays visible"
        );
        assert!(
            !is_session_hidden(&cfg, &no_store),
            "a same-id row with no confirmed store stays visible"
        );
    }

    /// The persisted key keeps the durable store selectors but never live
    /// bridges or credential-carrying environment: a running OpenCode
    /// service readout collapses to its connection selector.
    #[test]
    fn scoped_key_never_persists_live_bridge_or_credentials() {
        let mut scope = lyrel_core::host::session_operations::SessionScope::default();
        scope.opencode = Some(
            lyrel_core::host::session_operations::opencode::OpenCodeScope {
                connection_argv: vec!["opencode".into(), "--config".into(), "/srv/oc.json".into()],
                launch_argv: vec!["--port".into(), "4096".into()],
                server: Some(
                    lyrel_core::host::session_operations::opencode::OpenCodeServer {
                        pid: 42,
                        urls: vec!["http://127.0.0.1:4096".into()],
                    },
                ),
                bridge: None,
                session: None,
            },
        );
        scope
            .environment
            .insert("OPENCODE_DB".into(), "/srv/oc.db".into());
        scope
            .environment
            .insert("OPENAI_API_KEY".into(), "secret-token".into());
        let identity = SessionIdentity {
            host: HostId::LOCAL,
            agent: CLIAgent::OpenCode,
            id: "svc".into(),
            cwd: None,
            scope: Some(scope),
        };
        let key = identity.key();
        assert!(key.contains("OPENCODE_DB"), "durable selectors stay: {key}");
        assert!(
            key.contains("srv%2Foc.json"),
            "the connection selector stays: {key}"
        );
        assert!(
            !key.contains("secret-token") && !key.contains("4096"),
            "no credential or live token is persisted: {key}"
        );
    }

    /// A legacy `slug:id` entry — written before Sessions settings existed —
    /// is the backward-compatible default: every same-id row on every host
    /// stays hidden until the entry is removed.
    #[test]
    fn legacy_hide_is_the_backward_compatible_default() {
        let local = identity(CLIAgent::Pi, "same", HostId::LOCAL, Some("/store/a"));
        let other_host = identity(
            CLIAgent::Pi,
            "same",
            HostId::from_connection_key("ssh-direct:me@box:22"),
            Some("/store/a"),
        );

        let mut cfg = Config::default();
        cfg.hidden_agent_sessions.insert(local.legacy_key());
        assert!(is_session_hidden(&cfg, &local));
        assert!(
            is_session_hidden(&cfg, &other_host),
            "one legacy hide keeps covering same-id rows on other hosts"
        );
    }

    /// Re-showing one legacy-hidden row writes the exact-scope override
    /// instead of deleting the legacy key: the named row comes back, its
    /// same-id siblings stay hidden, and re-hiding it clears the override.
    #[test]
    fn exact_scope_override_redisplays_one_legacy_hidden_row() {
        let local = identity(CLIAgent::Pi, "same", HostId::LOCAL, Some("/store/a"));
        let sibling = identity(CLIAgent::Pi, "same", HostId::LOCAL, Some("/store/b"));

        let mut cfg = Config::default();
        cfg.hidden_agent_sessions.insert(local.legacy_key());

        show_session(&mut cfg, &local);
        assert!(
            cfg.hidden_agent_sessions.contains(&local.legacy_key()),
            "the legacy entry survives so its siblings stay hidden"
        );
        assert!(
            cfg.hidden_agent_sessions.contains(&local.show_key()),
            "the exact-scope override is persisted: {}",
            local.show_key()
        );
        assert!(
            !is_session_hidden(&cfg, &local),
            "the re-shown row reads visible again"
        );
        assert!(
            is_session_hidden(&cfg, &sibling),
            "the same-id sibling stays hidden"
        );

        hide_session(&mut cfg, &local);
        assert!(
            !cfg.hidden_agent_sessions.contains(&local.show_key()),
            "re-hiding drops the stale override"
        );
        assert!(is_session_hidden(&cfg, &local));
        assert!(is_session_hidden(&cfg, &sibling));
    }

    /// Hiding and re-showing a row that only carries a scoped key is a plain
    /// insert/remove round trip — no legacy entry, no override marker.
    #[test]
    fn hide_show_round_trip_through_the_scoped_key() {
        let identity = identity(CLIAgent::OhMyPi, "row", HostId::LOCAL, None);
        let mut cfg = Config::default();
        hide_session(&mut cfg, &identity);
        assert!(cfg.hidden_agent_sessions.contains(&identity.key()));
        assert!(
            !cfg.hidden_agent_sessions.contains(&identity.legacy_key()),
            "new hides never write the legacy spelling"
        );
        show_session(&mut cfg, &identity);
        assert!(cfg.hidden_agent_sessions.is_empty());
        assert!(!is_session_hidden(&cfg, &identity));
    }

    /// Terminal and rendering environment describes a pane, not a store: it
    /// never enters the persisted key, so a hide survives rescans from a
    /// different terminal, and never defeats the live-writer overlap check.
    #[test]
    fn terminal_environment_neither_keys_nor_splits_store_identity() {
        let mut scan_scope = lyrel_core::host::session_operations::SessionScope::default();
        scan_scope.session_dir = Some(PathBuf::from("/store/pi"));
        scan_scope
            .environment
            .insert("PI_CODING_AGENT_DIR".into(), "/store/pi".into());
        scan_scope
            .environment
            .insert("TERM_PROGRAM".into(), "WezTerm".into());
        scan_scope.shell_program = Some("wezterm".into());

        let mut rescan_scope = scan_scope.clone();
        rescan_scope
            .environment
            .insert("TERM_PROGRAM".into(), "kitty".into());
        rescan_scope
            .environment
            .insert("KITTY_WINDOW_ID".into(), "7".into());
        rescan_scope.shell_program = Some("bash".into());

        let scanned = SessionIdentity {
            host: HostId::LOCAL,
            agent: CLIAgent::Pi,
            id: "row".into(),
            cwd: None,
            scope: Some(scan_scope.clone()),
        };
        let rescanned = SessionIdentity {
            host: HostId::LOCAL,
            agent: CLIAgent::Pi,
            id: "row".into(),
            cwd: None,
            scope: Some(rescan_scope.clone()),
        };
        assert_eq!(
            scanned.key(),
            rescanned.key(),
            "the persisted key is stable across terminal environment"
        );

        // A running pane's readout names the store without the exact
        // directory: it must still read as the same session.
        let mut readout_scope = rescan_scope;
        readout_scope.session_dir = None;
        let live = SessionIdentity {
            host: HostId::LOCAL,
            agent: CLIAgent::Pi,
            id: "row".into(),
            cwd: None,
            scope: Some(readout_scope),
        };
        assert!(
            scanned.same_session(&live),
            "a live writer in another terminal still reads as the same session"
        );

        let mut other_store = scan_scope;
        other_store
            .environment
            .insert("PI_CODING_AGENT_DIR".into(), "/other/store".into());
        let elsewhere = SessionIdentity {
            host: HostId::LOCAL,
            agent: CLIAgent::Pi,
            id: "row".into(),
            cwd: None,
            scope: Some(other_store),
        };
        assert_ne!(scanned.key(), elsewhere.key());
        assert!(
            !scanned.same_session(&elsewhere),
            "a genuinely different store is still distinguished"
        );
    }

    /// The confirmed store names the provider's own selectors: an explicit
    /// Hermes home, an OMP config root, the session directory, and OpenCode's
    /// saved connection when it alone addresses the store.
    #[test]
    fn store_names_provider_specific_durable_selectors() {
        let mut hermes_scope = lyrel_core::host::session_operations::SessionScope::default();
        hermes_scope
            .environment
            .insert("HERMES_HOME".into(), "/srv/hermes".into());
        let hermes = SessionIdentity {
            host: HostId::LOCAL,
            agent: CLIAgent::Hermes,
            id: "h".into(),
            cwd: None,
            scope: Some(hermes_scope),
        };
        assert_eq!(
            hermes.store(),
            SessionStore::Named {
                profile: None,
                root: Some("/srv/hermes".into()),
                dir: None,
                connection: None,
            },
            "an explicit Hermes home is the confirmed store"
        );

        let mut omp_scope = lyrel_core::host::session_operations::SessionScope::default();
        omp_scope
            .environment
            .insert("PI_CONFIG_DIR".into(), "/srv/omp/config".into());
        omp_scope.session_dir = Some(PathBuf::from("/srv/omp/sessions"));
        let omp = SessionIdentity {
            host: HostId::LOCAL,
            agent: CLIAgent::OhMyPi,
            id: "o".into(),
            cwd: None,
            scope: Some(omp_scope),
        };
        assert_eq!(
            omp.store(),
            SessionStore::Named {
                profile: None,
                root: Some("/srv/omp/config".into()),
                dir: Some(PathBuf::from("/srv/omp/sessions")),
                connection: None,
            },
            "the OMP config root and session directory ride along"
        );

        let oc_scope = lyrel_core::host::session_operations::SessionScope {
            opencode: Some(
                lyrel_core::host::session_operations::opencode::OpenCodeScope {
                    connection_argv: vec!["opencode".into(), "--as".into(), "svc".into()],
                    ..Default::default()
                },
            ),
            ..Default::default()
        };
        let opencode = SessionIdentity {
            host: HostId::LOCAL,
            agent: CLIAgent::OpenCode,
            id: "svc".into(),
            cwd: None,
            scope: Some(oc_scope),
        };
        assert_eq!(
            opencode.store(),
            SessionStore::Named {
                profile: None,
                root: None,
                dir: None,
                connection: Some("opencode --as svc".into()),
            },
            "a connection-only OpenCode scope is still a confirmed store"
        );
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
        let case_variant = host.join(&remote_home, ".pi/agent/sessions/r/remote-pi-case.jsonl");
        host.write_file(
            &case_variant,
            b"{\"type\":\"session\",\"id\":\"remote-pi-case\",\"cwd\":\"/tmp/Remote-Project\"}\n{\"type\":\"message\",\"message\":{\"role\":\"user\",\"content\":\"case decoy\"}}\n",
        )
        .unwrap();

        let agent_launch = std::collections::HashMap::from([(
            CLIAgent::Hermes.slug().to_owned(),
            format!("{}/unavailable-hermes-launcher", fixture.remote_home()),
        )]);
        // The remote Pi store answers even if a separate Hermes launcher
        // cannot run. Keep the failure independent of the remote host's PATH.
        let scan = cx.update(|cx| {
            let mut scan = plan_scan(
                host.id(),
                &[],
                &CLIAgent::ALL,
                &remote_home,
                &agent_launch,
                cx,
            );
            scan.directory = Some(PathBuf::from("/tmp/remote-project"));
            scan
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
            listing.sessions.iter().all(|s| s.id != "remote-pi-case"),
            "the Linux owning Host distinguishes directories that differ in case"
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

    /// The scope fence names the actual host connection and the pinned
    /// repository, not just the workspace's host id: a relink with an
    /// identical directory is a different bound (rescan), and a pinned
    /// worktree keeps its origin instead of re-binding to whatever host the
    /// workspace currently names.
    #[test]
    fn session_bound_fences_on_connection_and_pin_origin() {
        let base = SessionBound {
            host: HostId::LOCAL,
            connection: 0,
            repo: None,
            dir: Some(PathBuf::from("/repo/main")),
        };
        // A reconnect replaces the connection object: same host id, same
        // directory — still a different bound, so the listing rescans.
        assert_ne!(
            base,
            SessionBound {
                connection: 1,
                ..base.clone()
            },
            "an identical directory on a new connection must rescan"
        );
        // A pinned worktree carries the repository it was pinned on.
        assert_ne!(
            base,
            SessionBound {
                repo: Some(PathBuf::from("/repo/main")),
                ..base.clone()
            },
            "the same path under another repository is another scope"
        );
        // No directory remains its own state: nothing to scan against.
        assert_ne!(
            base,
            SessionBound {
                dir: None,
                ..base.clone()
            }
        );
        assert_eq!(
            base,
            SessionBound {
                dir: Some(PathBuf::from("/repo/main")),
                ..SessionBound::default()
            }
        );
    }

    /// The extracted portions are the blocking scan, just split: native
    /// rows first, service rows folded in with the same identity dedup, the
    /// same directory filter under the owning Host's path semantics, the
    /// same most-recent-first order, and every source's errors kept beside
    /// the sessions it found.
    #[test]
    fn split_portions_merge_to_what_the_blocking_collect_sessions_returns() {
        let (host, _fixture) = fixture_host(SessionFixture::merged());
        let native = collect_native_sessions(&*host, Some(Path::new("/project")));
        assert_eq!(
            ids(&native.sessions),
            ["pi-native", "omp-native"],
            "the native portion answers for the queried directory"
        );
        let services = collect_service_sessions(&*host, service_scan(host.id()));
        assert_eq!(
            ids(&services.sessions),
            ["oc-service", "oc-elsewhere"],
            "the service portion folds the identity it repeated but keeps its other rows"
        );
        assert!(
            services.errors.iter().any(|e| e
                .message
                .contains("OpenCode history storage could not be confirmed")),
            "the unconfirmed default service scope is said out loud"
        );
        assert!(
            services
                .errors
                .iter()
                .any(|e| e.message.contains("fixture hermes is unavailable")),
            "a failing service is recorded beside the ones that answered"
        );
        let mut listing = native;
        merge_listing(&*host, Some(Path::new("/project")), &mut listing, services);
        assert_eq!(
            ids(&listing.sessions),
            ["oc-service", "omp-native", "pi-native"]
        );
        assert!(
            listing
                .sessions
                .iter()
                .all(|s| s.cwd.as_deref() != Some(Path::new("/elsewhere"))),
            "the merge filters every source to the queried directory"
        );
        assert!(listing.host_reachable, "both portions reached their host");

        // The blocking entry point Search Everywhere and Sessions settings
        // consume is the same two portions through the same merge.
        let whole = collect_sessions(&*host, service_scan(host.id()));
        assert_eq!(ids(&whole.sessions), ids(&listing.sessions));
        assert_eq!(
            whole
                .errors
                .iter()
                .map(|e| e.message.as_str())
                .collect::<Vec<_>>(),
            listing
                .errors
                .iter()
                .map(|e| e.message.as_str())
                .collect::<Vec<_>>(),
        );
    }

    /// End to end through the production window: the native rows paint
    /// while a service source is held pending, a scope change drops the old
    /// rows immediately and retires the superseded service landing on its
    /// generation fence, and the keyboard's selection follows its session
    /// across the reorder the late portion brings.
    #[gpui_kit::test]
    fn native_rows_paint_while_service_pending_and_stale_landings_retire(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        use lyrel_core::core::config::RightPanelTab;
        use lyrel_ui_base::route::PaneWorkspace;
        use lyrel_ui_base::session::{RemoteTarget, WorkspaceId};

        let (app, mut vcx, mut streams) = crate::ui::app::test_window::harness_with_tabs(cx, 1);
        let target = RemoteTarget::Alias {
            alias: "session-history-fixture".into(),
        };
        let fixture = Arc::new(Mutex::new(SessionFixture::production_like()));
        vcx.update(|_, cx| {
            lyrel_ui_base::host_registry::HostRegistry::insert(
                cx,
                fixture_host_with(target.host_id(), &fixture),
            );
        });
        app.update_in(&mut vcx, |app, window, cx| {
            let leaf = app.tabs[0]
                .detail_pane(window, cx)
                .expect("the harness pane");
            leaf.update(cx, |view, _| {
                view.set_workspace(Some(PaneWorkspace {
                    workspace: WorkspaceId::new(),
                    target: target.clone(),
                    spec: None,
                    label: Some("session-history-fixture".into()),
                    resize_echo: false,
                }));
            });
        });
        // Offer OpenCode and Hermes on any PATH: the scan must plan their
        // service requests whether or not the real CLIs are installed.
        vcx.update(|_, cx| {
            let mut cfg = cx.global::<Config>().clone();
            let command = format!("\"{}\"", std::env::current_exe().unwrap().display());
            cfg.agent_launch
                .insert(CLIAgent::OpenCode.slug().into(), command.clone());
            cfg.agent_launch
                .insert(CLIAgent::Hermes.slug().into(), command);
            cx.set_global(cfg);
        });
        let mut daemon = streams.remove(0);
        send_cwd(&mut daemon, "/project");
        for _ in 0..200 {
            vcx.run_until_parked();
            if pane_dir(&app, &mut vcx).as_deref() == Some(Path::new("/project")) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(
            pane_dir(&app, &mut vcx).as_deref(),
            Some(Path::new("/project")),
            "the pane reports its directory before the panel scans"
        );
        app.update_in(&mut vcx, |app, window, cx| {
            app.set_right_panel_tab(RightPanelTab::Sessions, window, cx);
        });

        // First paint: the native portion landed while the service source
        // sits parked behind the closed gate.
        settle(&mut vcx);
        assert!(
            app.update_in(&mut vcx, |app, _window, _cx| app.session_history.scanning),
            "the service portion is still pending"
        );
        assert!(
            app.update_in(&mut vcx, |app, _window, _cx| app.session_history.landed),
            "the native portion painted"
        );
        assert_eq!(
            ids(&app.update_in(&mut vcx, |app, _window, _cx| {
                app.session_history.listing.sessions.clone()
            })),
            ["omp-native", "pi-native"],
            "the known native rows are listed before any service answers"
        );
        vcx.update(|window, cx| {
            window.render_frame(cx);
            assert!(
                observed(window, "panel-sessions-row-0"),
                "the first native row is on screen, not a loading placeholder"
            );
            assert!(
                observed(window, "panel-sessions-row-1"),
                "the second native row is on screen"
            );
            assert!(
                observed(window, "panel-sessions-pending"),
                "the pending service source is said on screen"
            );
        });

        // The user moves the keyboard onto a native row; then the directory
        // changes while the service source is still parked. The old rows
        // come down at once — only the current generation may paint.
        app.update_in(&mut vcx, |app, _window, _cx| {
            app.session_history.selected = Some(1);
        });
        send_cwd(&mut daemon, "/other");
        for _ in 0..200 {
            vcx.run_until_parked();
            if pane_dir(&app, &mut vcx).as_deref() == Some(Path::new("/other")) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        vcx.update(|window, cx| window.render_frame(cx));
        assert!(
            app.update_in(&mut vcx, |app, _window, _cx| {
                app.session_history.listing.sessions.is_empty()
            }),
            "the scope change takes the old generation's rows down before anything lands"
        );
        settle(&mut vcx);
        assert!(
            app.update_in(&mut vcx, |app, _window, _cx| app.session_history.scanning),
            "the current scope's service portion is still pending behind the gate"
        );
        assert_eq!(
            ids(&app.update_in(&mut vcx, |app, _window, _cx| {
                app.session_history.listing.sessions.clone()
            })),
            ["kimi-native"],
            "only the current scope's rows are up: {}",
            phase_b_dump(&app, &mut vcx, &fixture)
        );
        vcx.update(|window, cx| {
            window.render_frame(cx);
            assert!(
                observed(window, "panel-sessions-row-0"),
                "the current scope's row is on screen"
            );
            assert!(
                observed(window, "panel-sessions-pending"),
                "the new scope still says its service source is pending on screen"
            );
        });
        app.update_in(&mut vcx, |app, _window, _cx| {
            app.session_history.selected = Some(0);
        });

        // Both service answers land: the second scan's rows merge, and the
        // first scan's marker error arrives late enough that its landing
        // must retire on the generation fence — the marker may not paint.
        fixture_wait_for_calls(&fixture, 2);
        settle(&mut vcx);
        let (scanning, selected, errors) = app.update_in(&mut vcx, |app, _window, _cx| {
            (
                app.session_history.scanning,
                app.session_history.selected,
                app.session_history.listing.errors.clone(),
            )
        });
        let sessions = app.update_in(&mut vcx, |app, _window, _cx| {
            app.session_history.listing.sessions.clone()
        });
        assert!(!scanning, "the live scan closed");
        assert_eq!(ids(&sessions), ["oc-service", "kimi-native"]);
        assert_eq!(
            selected,
            Some(1),
            "the selection followed its session across the reorder"
        );
        assert_eq!(sessions[1].id, "kimi-native");
        assert!(
            errors
                .iter()
                .all(|e| e.message != "stale-generation marker"),
            "the superseded service landing retired instead of painting: {errors:?}"
        );
        assert!(
            errors.iter().any(|e| e
                .message
                .contains("OpenCode history storage could not be confirmed")),
            "the live landing kept the final source limits and errors: {errors:?}"
        );
        assert!(
            errors
                .iter()
                .any(|e| e.message.contains("fixture hermes is unavailable")),
            "the failing service source is reported on the final listing: {errors:?}"
        );
        vcx.update(|window, cx| {
            window.render_frame(cx);
            assert!(
                !observed(window, "panel-sessions-pending"),
                "the finished listing no longer reads as pending"
            );
            assert!(
                observed(window, "panel-sessions-partial"),
                "the finished listing reads its incomplete sources on screen"
            );
        });
    }

    fn ids(sessions: &[PastSession]) -> Vec<&str> {
        sessions.iter().map(|session| session.id.as_str()).collect()
    }

    /// A scan the way `plan_scan` builds it for one offered service agent:
    /// the default scope the pane would fall back to.
    fn service_scan(host: HostId) -> SessionScan {
        let service_request = |agent| SessionOperation {
            expected_host: host.0,
            agent,
            kind: SessionOperationKind::Resume,
            source_id: String::new(),
            cwd: PathBuf::from("/project"),
            launch_argv: vec!["fixture".into()],
            scope: Default::default(),
        };
        SessionScan {
            directory: Some(PathBuf::from("/project")),
            opencode: vec![service_request(CLIAgent::OpenCode)],
            hermes: vec![service_request(CLIAgent::Hermes)],
            hermes_errors: Vec::new(),
        }
    }

    /// The directory the context pane currently reports, if any.
    fn pane_dir(
        app: &gpui_kit::Entity<LyrelApp>,
        vcx: &mut gpui_kit::VisualTestContext,
    ) -> Option<PathBuf> {
        app.update_in(vcx, |app, window, cx| {
            app.context_pane(window, cx)
                .and_then(|pane| pane.read(cx).host_cwd())
        })
    }

    fn phase_b_dump(
        app: &gpui_kit::Entity<LyrelApp>,
        vcx: &mut gpui_kit::VisualTestContext,
        fixture: &Arc<Mutex<SessionFixture>>,
    ) -> String {
        let (scanning, landed, generation, bound_dir) = app.update_in(vcx, |app, _window, _cx| {
            (
                app.session_history.scanning,
                app.session_history.landed,
                app.session_history.generation,
                app.session_history.bound.dir.clone(),
            )
        });
        let fixture = fixture.lock().unwrap();
        format!(
            "scanning={scanning} landed={landed} generation={generation} \
             bound_dir={bound_dir:?} native_calls={} service_calls={}",
            fixture.native_calls, fixture.service_calls
        )
    }

    fn settle(vcx: &mut gpui_kit::VisualTestContext) {
        vcx.run_until_parked();
        vcx.update(|window, cx| window.render_frame(cx));
        vcx.run_until_parked();
        vcx.update(|window, cx| window.render_frame(cx));
    }

    /// Whether an observed element id painted visible in the window's last
    /// frame. Text queries cannot see views GPUI Fast retains, so on-screen
    /// proof goes through the `test_support` element observations.
    fn observed(window: &gpui_kit::Window, id: &'static str) -> bool {
        window.find_all(id).iter().any(|element| element.visible())
    }

    fn send_cwd(daemon: &mut lyrel_core::daemon::transport::Stream, dir: &str) {
        DaemonMsg::Cwd(PathBuf::from(dir)).encode(daemon).unwrap();
        daemon.flush().unwrap();
    }

    /// The fixture host and its fixture: the service source answers slowly.
    fn fixture_host(
        fixture: SessionFixture,
    ) -> (Arc<GatedSessionHost>, Arc<Mutex<SessionFixture>>) {
        let fixture = Arc::new(Mutex::new(fixture));
        let host = fixture_host_with(HostId::from_connection_key("session-fixture"), &fixture);
        (host, fixture)
    }

    fn fixture_host_with(
        id: HostId,
        fixture: &Arc<Mutex<SessionFixture>>,
    ) -> Arc<GatedSessionHost> {
        Arc::new(GatedSessionHost {
            id,
            inner: lyrel_core::host::local::shared(),
            fixture: fixture.clone(),
        })
    }

    /// The slow-source fixture: the native stores answer immediately, every
    /// OpenCode service call sleeps before answering (the production Windows
    /// stall, minus the two minutes) — the first call with the marker error,
    /// the later calls with the service rows — and Hermes fails at once like
    /// an uninstalled provider.
    struct SessionFixture {
        native: Vec<PastSession>,
        opencode: Vec<PastSession>,
        stale_marker: Option<String>,
        /// How long a service answer takes. Generous windows: the
        /// assertions between a scan and its service landing run in
        /// milliseconds.
        delay: std::time::Duration,
        service_calls: usize,
        completed_calls: usize,
        native_calls: usize,
    }

    impl SessionFixture {
        const DELAY: std::time::Duration = std::time::Duration::from_secs(2);

        fn production_like() -> Self {
            Self {
                native: vec![
                    session_row("pi-native", CLIAgent::Pi, "/project", 100),
                    session_row("omp-native", CLIAgent::OhMyPi, "/project", 200),
                    session_row("kimi-native", CLIAgent::Kimi, "/other", 150),
                ],
                opencode: vec![
                    session_row("oc-service", CLIAgent::OpenCode, "/other", 250),
                    session_row("oc-service", CLIAgent::OpenCode, "/other", 250),
                ],
                stale_marker: Some("stale-generation marker".into()),
                delay: Self::DELAY,
                service_calls: 0,
                completed_calls: 0,
                native_calls: 0,
            }
        }

        /// Both portions hold rows in the queried directory; the service
        /// portion repeats one identity and holds one row outside it.
        fn merged() -> Self {
            Self {
                native: vec![
                    session_row("pi-native", CLIAgent::Pi, "/project", 100),
                    session_row("omp-native", CLIAgent::OhMyPi, "/project", 200),
                ],
                opencode: vec![
                    session_row("oc-service", CLIAgent::OpenCode, "/project", 250),
                    session_row("oc-service", CLIAgent::OpenCode, "/project", 250),
                    session_row("oc-elsewhere", CLIAgent::OpenCode, "/elsewhere", 300),
                ],
                stale_marker: None,
                delay: std::time::Duration::ZERO,
                service_calls: 0,
                completed_calls: 0,
                native_calls: 0,
            }
        }
    }

    /// Wait until enough fixture service calls answered, so the superseded
    /// landing's marker had its chance to paint (a fenced one never does).
    /// The lock is taken per poll: a panic here must not poison the mutex
    /// the parked service threads are about to take.
    fn fixture_wait_for_calls(fixture: &Mutex<SessionFixture>, want: usize) {
        for _ in 0..600 {
            if fixture.lock().unwrap().completed_calls >= want {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        panic!("the fixture service calls never completed");
    }

    fn session_row(id: &str, agent: CLIAgent, cwd: &str, updated: u64) -> PastSession {
        PastSession {
            agent,
            id: id.into(),
            cwd: Some(PathBuf::from(cwd)),
            title: format!("{id} title"),
            branch: None,
            updated,
            scope: None,
        }
    }

    struct GatedSessionHost {
        id: HostId,
        inner: SharedHost,
        fixture: Arc<Mutex<SessionFixture>>,
    }

    impl Host for GatedSessionHost {
        fn id(&self) -> HostId {
            self.id
        }
        fn separator(&self) -> char {
            self.inner.separator()
        }
        fn is_absolute(&self, p: &Path) -> bool {
            self.inner.is_absolute(p)
        }
        fn read_dir(&self, dir: &Path, root: Option<&Path>) -> std::io::Result<Vec<Entry>> {
            self.inner.read_dir(dir, root)
        }
        fn stat(&self, p: &Path) -> std::io::Result<Meta> {
            self.inner.stat(p)
        }
        fn read_file(&self, p: &Path, max_bytes: u64) -> std::io::Result<Vec<u8>> {
            self.inner.read_file(p, max_bytes)
        }
        fn canonicalize(&self, p: &Path) -> std::io::Result<PathBuf> {
            self.inner.canonicalize(p)
        }
        fn search(
            &self,
            roots: &[PathBuf],
            query: &str,
            limit: usize,
            max_dirs: usize,
            show_hidden: bool,
        ) -> std::io::Result<Vec<SearchHit>> {
            self.inner
                .search(roots, query, limit, max_dirs, show_hidden)
        }
        fn search_content(
            &self,
            roots: &[PathBuf],
            query: &lyrel_core::host::ContentQuery,
            limits: &lyrel_core::host::ContentLimits,
        ) -> std::io::Result<lyrel_core::host::ContentResults> {
            self.inner.search_content(roots, query, limits)
        }
        fn write_file(&self, p: &Path, bytes: &[u8]) -> std::io::Result<Meta> {
            self.inner.write_file(p, bytes)
        }
        fn create_file_new(&self, p: &Path) -> std::io::Result<()> {
            self.inner.create_file_new(p)
        }
        fn create_dir(&self, p: &Path, recursive: bool) -> std::io::Result<()> {
            self.inner.create_dir(p, recursive)
        }
        fn rename(&self, from: &Path, to: &Path) -> std::io::Result<()> {
            self.inner.rename(from, to)
        }
        fn remove(&self, p: &Path, recursive: bool) -> std::io::Result<()> {
            self.inner.remove(p, recursive)
        }
        fn repo_root(&self, p: &Path) -> std::io::Result<Option<PathBuf>> {
            self.inner.repo_root(p)
        }
        fn git(&self, cwd: &Path, args: &[&str]) -> std::io::Result<Output> {
            self.inner.git(cwd, args)
        }
        fn shells(&self) -> std::io::Result<lyrel_core::host::ShellInventory> {
            self.inner.shells()
        }
        fn watch(&self, dirs: &[PathBuf]) -> std::io::Result<WatchSub> {
            self.inner.watch(dirs)
        }
        fn agent_sessions(&self, directory: Option<&Path>) -> std::io::Result<Vec<PastSession>> {
            let mut fixture = self.fixture.lock().unwrap();
            fixture.native_calls += 1;
            Ok(fixture
                .native
                .iter()
                .filter(|session| {
                    directory.is_none_or(|dir| {
                        session.cwd.as_deref().is_some_and(|cwd| {
                            lyrel_core::core::agent_history::directory_matches_with(
                                cwd,
                                dir,
                                self.separator(),
                            )
                        })
                    })
                })
                .cloned()
                .collect())
        }
        fn opencode_sessions(
            &self,
            _request: &SessionOperation,
        ) -> Result<Vec<PastSession>, SessionOperationError> {
            let (first, rows, marker, delay) = {
                let mut fixture = self.fixture.lock().unwrap();
                let first = fixture.service_calls == 0;
                fixture.service_calls += 1;
                (
                    first,
                    fixture.opencode.clone(),
                    fixture.stale_marker.clone(),
                    fixture.delay,
                )
            };
            // The production stall: the service answer waits, and the scope
            // may move on underneath the wait.
            std::thread::sleep(delay);
            let outcome = match (first, marker) {
                (true, Some(marker)) => Err(SessionOperationError {
                    // The first call belongs to the first scan: answer with
                    // the marker, so a landing that escaped the generation
                    // fence would show it on the panel.
                    kind: SessionErrorKind::Connection,
                    message: marker.into(),
                    retained: None,
                }),
                _ => Ok(rows),
            };
            let mut fixture = self.fixture.lock().unwrap();
            fixture.completed_calls += 1;
            outcome
        }
        fn hermes_sessions(
            &self,
            request: &SessionOperation,
        ) -> Result<Vec<PastSession>, SessionOperationError> {
            let _ = request;
            Err(SessionOperationError {
                kind: SessionErrorKind::Unsupported,
                message: "fixture hermes is unavailable".into(),
                retained: None,
            })
        }
    }
}
