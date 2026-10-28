mod completion_adapter;
mod events;
mod history_adapter;
pub(super) mod input;
mod links;
pub(crate) mod menus;
mod notices;
mod presentation;
mod prompt;

#[cfg(test)]
mod test_support;

use super::remote::PaneEvent;
use crate::ui::notification_history::NotificationWindowExt as _;
use gpui_kit::component::scroll::Scrollbar;
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, h_flex};
use gpui_kit::{
    App, ClipboardItem, Context, ExternalPaths, FocusHandle, Focusable, Font, KeyDownEvent,
    Modifiers, MouseButton, MouseDownEvent, Pixels, ScrollEvent, WeakEntity, Window, actions, div,
    prelude::*, px,
};
use rio_vt::crosswords::Mode;
use rio_vt::crosswords::grid::Dimensions;
#[cfg(test)]
use rio_vt::crosswords::grid::Scroll;
use rio_vt::crosswords::pos::{Column, Direction, Pos};
#[cfg(test)]
use rio_vt::crosswords::pos::{Line, Side};

use super::TermSize;
#[cfg(test)]
use super::completion;
use super::completion::state::TerminalCompletion;
use super::element::{GridSnapshot, RenderCell, TerminalElement};
use super::history_session::TerminalHistory;
use super::link::LinkInteraction;
use super::remote::RemoteTerminal;
use super::scrollbar::{GridScroll, TerminalScrollHandle};
use super::search::SearchState;
#[cfg(test)]
use super::typeahead::RawInput;
use crate::core::actions::{
    CloseActiveTab, CopyLinkPathUnderPointer, DecreaseFontSize, ForkAgentSessionDown,
    ForkAgentSessionLeft, ForkAgentSessionRight, ForkAgentSessionUp, IncreaseFontSize, NewTab,
    OpenLinkUnderPointer, OpenLinkWithDefaultApp, RevealLinkUnderPointer, SaveAgentLaunchArgs,
    SendBackTab, SendTab, SplitDown, SplitRight, ToggleMaximizePane,
};
use crate::core::config::{BellMode, Config};
use crate::daemon::protocol::{RemoteContext, ShellSpec};
use crate::ui::i18n::{L10nKey, t, t_fmt};

mod clipboard;

pub(super) const GRID_PAD_X: f32 = 8.;
pub(super) const GRID_PAD_Y: f32 = 4.;

actions!(
    terminal,
    [
        CopyText,
        CutText,
        PasteText,
        AlternatePaste,
        SelectAll,
        UndoEdit,
        RedoEdit,
        FindInTerminal,
        FindNext,
        FindPrevious,
        ClearScrollback,
        InsertNewline,
        InsertNewlineFallback,
        ToggleComposer
    ]
);

pub struct ChildExited;

impl gpui_kit::EventEmitter<ChildExited> for TerminalView {}

pub struct AuthPromptReady;

impl gpui_kit::EventEmitter<AuthPromptReady> for TerminalView {}

pub struct AgentSessionChanged;

impl gpui_kit::EventEmitter<AgentSessionChanged> for TerminalView {}

/// A coding agent started running in this pane: its foreground went from no
/// agent to this one. What quick launch counts as the agent being used.
pub struct AgentDetected(pub crate::core::cli_agent::CLIAgent);

impl gpui_kit::EventEmitter<AgentDetected> for TerminalView {}

/// A file link the user clicked, on its way to whoever can show it. The
/// terminal resolves the path — it is the only thing that knows the pane's
/// directory and host — and the app opens it, because the editor and the file
/// tree are its to drive.
pub struct OpenFileRequested {
    pub host_id: crate::ui::host_ops::HostId,
    pub path: std::path::PathBuf,
    pub line: Option<u32>,
    pub column: Option<u32>,
    /// A directory link asks "where is this?", not "what does it say?" — the
    /// app answers it in the file tree instead of the editor.
    pub is_dir: bool,
}

impl gpui_kit::EventEmitter<OpenFileRequested> for TerminalView {}

pub struct NativeSshParts {
    terminal: RemoteTerminal,
    pane_id: u64,
    persist: Box<crate::daemon::protocol::NativeSshSpec>,
}

impl NativeSshParts {
    pub(crate) fn new(
        terminal: RemoteTerminal,
        pane_id: u64,
        persist: Box<crate::daemon::protocol::NativeSshSpec>,
    ) -> Self {
        Self {
            terminal,
            pane_id,
            persist,
        }
    }
    pub(crate) fn pane_id(&self) -> u64 {
        self.pane_id
    }
}

/// What a pane is called when nothing running in it has said otherwise.
pub(crate) const DEFAULT_TITLE: &str = "lyrel";

/// What a pane is *saying* about itself, if anything — the reading behind
/// [`TerminalView::stated_title`], split out so it can be pinned without a
/// live pane.
///
/// Anything but the placeholder counts. That is wider than "arrived over OSC
/// 0/2" on purpose: an SSH pane answers to the host it dialled and a workspace
/// pane to its workspace's name, and those are names lyrel gave the pane
/// deliberately (#438) rather than the absence of one. The literal string
/// `lyrel` is the only title that says nothing, because it is the app's own
/// name standing in for a pane that has never introduced itself.
pub(crate) fn stated_title(title: &str) -> Option<&str> {
    match title.trim() {
        "" => None,
        t if t == DEFAULT_TITLE => None,
        t => Some(t),
    }
}

pub struct ShellParts {
    terminal: RemoteTerminal,
    pub(crate) pane_id: u64,
    shell_spec: Option<ShellSpec>,
    pub(crate) workspace: Option<crate::terminal::PaneWorkspace>,
    pub(crate) restored: bool,
    pub(crate) owner: Option<crate::core::session::WorkspaceId>,
}

impl ShellParts {
    pub(crate) fn new(
        terminal: RemoteTerminal,
        pane_id: u64,
        shell_spec: Option<ShellSpec>,
        workspace: Option<crate::terminal::PaneWorkspace>,
        restored: bool,
        owner: Option<crate::core::session::WorkspaceId>,
    ) -> Self {
        Self {
            terminal,
            pane_id,
            shell_spec,
            workspace,
            restored,
            owner,
        }
    }
}

fn cwd_is_on_host(pane_runs_remotely: bool, host_is_local: bool) -> bool {
    match pane_runs_remotely {
        false => host_is_local,
        true => !host_is_local,
    }
}

pub struct TerminalView {
    pub terminal: RemoteTerminal,
    host_id: crate::ui::host_ops::HostId,
    workspace: Option<crate::terminal::PaneWorkspace>,
    pub pane_id: u64,
    shell_spec: Option<ShellSpec>,
    owner_workspace: Option<crate::core::session::WorkspaceId>,
    ssh_spec: Option<Box<crate::daemon::protocol::NativeSshSpec>>,
    remote_paste: super::paste::RemotePaste,
    remote_clipboard_write_in_flight: Option<u64>,
    remote_clipboard_write_generation: u64,
    pub focus_handle: FocusHandle,
    pub(super) lifecycle: super::lifecycle::TerminalLifecycle,
    pub font: Font,
    pub font_bold: Option<Font>,
    pub font_italic: Option<Font>,
    font_features: Option<gpui_kit::FontFeatures>,
    pub font_size: Pixels,
    pub line_height_mul: f32,
    pub cell_width: Pixels,
    pub(super) line_height: Pixels,
    /// What the grid's height leaves under its last whole row: rows are
    /// drawn from the top, so this is empty space above the bottom padding.
    pub(super) grid_slack: Pixels,
    /// The grid the last frame painted, and the snapshot that went with it.
    /// A frame that cannot have the terminal lock repaints this rather than
    /// waiting on the pane's reader — see [`TerminalElement::build_grid`].
    /// Owned per pane rather than kept in one shared scratch buffer, because
    /// what makes it reusable is that it is still the *previous frame of this
    /// pane* when the next one starts.
    pub(super) grid_buf: Vec<RenderCell>,
    pub(super) grid_snap: Option<GridSnapshot>,
    /// Terminal mode and selection as of the last frame that got the lock.
    /// What the *frame* declares — the keymap context it publishes, whether it
    /// draws a selection — is read from here, so drawing never queues behind
    /// the pane's reader for two bits it can be one frame late about.
    /// Everything with a decision to make (a keystroke asking whether a
    /// full-screen program owns the screen) still asks the terminal itself.
    frame_alt_screen: bool,
    frame_has_selection: bool,
    pub(super) selection_scroll: super::selection_scroll::SelectionScroll,
    pub title: String,
    /// A title the pane has been told about but has not adopted yet — see
    /// `set_title_when_settled`. `None` means the tab is showing the newest
    /// title there is.
    pending_title: Option<String>,
    /// What the pane is called before anything running in it says otherwise —
    /// and what it goes back to when the program resets the title or the
    /// session ends. "lyrel" for a local shell; for an SSH pane it is the host
    /// it dialled, so a window full of them is still readable (#438); for a
    /// workspace pane it is the workspace's name, set in `set_workspace`.
    pub(super) default_title: String,
    /// The link supervisor asked this pane's machine for a relink and was
    /// refused — the pane is gone at the far end, and asking again can only
    /// repeat the answer. Cleared when a relink is adopted anyway (the manual
    /// reconnect path), which is the one thing that changes the question.
    relink_abandoned: bool,
    /// A relink for this pane is already on the wire. Both askers set it —
    /// the machine-level reconnect and the pump's own sweep — because the
    /// daemon keeps exactly one subscriber per pane and a second `Attach`
    /// kicks the first. Without this the pump would join a dial still in
    /// flight every 250 ms, and a dial can sit for fifteen seconds waiting
    /// for the far end's verdict.
    relink_inflight: bool,
    last_mouse_cell: Option<(usize, usize)>,
    /// Where the grid sat in the window on the last frame, so an event that
    /// arrives with nothing but a window position — the wheel — can be told
    /// which cell it is over.
    pub(super) cell_geom: Option<super::element::CellGeom>,
    pub(super) links: LinkInteraction,
    /// The scrollback bar's end of the grid: where it thinks the viewport is,
    /// and where it has asked for it to go. See [`super::scrollbar`].
    pub(super) scroll_handle: TerminalScrollHandle,
    pub search: Option<SearchState>,
    pub(super) search_focused: bool,
    /// The message box docked under an agent, once it has been asked for —
    /// see [`super::composer`].
    pub(super) composer: Option<super::composer::Composer>,
    pub(super) composer_focused: bool,
    pub(super) search_case_sensitive: bool,
    pub(super) search_regex: bool,
    pub(super) search_regex_error: bool,
    pub(super) search_last_query: String,
    /// Bumped by every wakeup that reaches an open search bar, so the pending
    /// rescan can tell "the pane went quiet" from "more output landed".
    pub(super) search_scan_epoch: u64,
    /// Whether a rescan is already waiting out the debounce. One task at a
    /// time, however fast the pane is printing.
    pub(super) search_scan_armed: bool,
    pub bell_flash: bool,
    /// Bumped by every bell, so only the timer armed by the latest one clears
    /// the flash: a burst of bells holds one steady flash instead of strobing.
    bell_epoch: u64,
    pub report_mouse: bool,
    last_at_prompt: bool,
    running_since: Option<std::time::Instant>,
    running_title: String,
    running_agent: Option<crate::core::cli_agent::CLIAgent>,
    agent_activity: super::agent_activity::AgentActivity,
    git_status_cwd: Option<std::path::PathBuf>,
    /// Command completion can fall entirely between foreground polls.
    git_prompt_cycle: u64,
    pub(super) input: super::prompt_input::PromptInput,
    history: TerminalHistory,
    completion: TerminalCompletion,
    integration_notice: Option<String>,
    integration_notice_shown: bool,
    created_at: std::time::Instant,
    /// How opaque the pane wants this terminal painted this frame: 1.0 at
    /// rest, [`crate::ui::pane::INACTIVE_DIM`] for an unfocused pane in a
    /// split, [`crate::ui::pane::LIFTED_DIM`] while the pane is being
    /// dragged. The pane leaf computes it and stores it here, because the
    /// dim is applied by blending the terminal's own colours toward the
    /// window background instead of the pane's element opacity — an opacity
    /// style would alpha-multiply every quad and path separately, so a
    /// powerline triangle stacked over a segment quad would show the
    /// already-dimmed segment through its own (1-dim) alpha and land with a
    /// visible seam where it meets the segment. The terminal element resets
    /// the field to 1.0 at the end of every paint, so the value never
    /// outlives the frame it was written for.
    pub(super) dim: f32,
}

/// What it takes to open one of a pane's loopback ports from this machine.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum PortRoute {
    /// The port is already reachable here under its own number.
    Direct,
    /// A local forward has to exist first; the pane's `ForwardRoute` builds it.
    Forward,
    /// Another machine's port, with no way to reach it from here.
    #[default]
    Blocked,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) enum LoopbackPlan {
    Direct,
    NoForwardNeeded,
    ForwardOnPane(u64),
    ForwardOnWorkspace(Box<crate::terminal::PaneWorkspace>),
}

/// What a pane's loopback port takes to open, given how its links would be
/// forwarded and whether the daemon listing it is this machine's.
///
/// Deliberately not "is the pane local": a remote pane's :3000 is perfectly
/// reachable once a forward exists, and building that forward is something
/// this app already knows how to do. Answering only "local or not" is what
/// left the Ports list showing a port it then refused to open.
pub(super) fn port_route_of(plan: &LoopbackPlan, local: bool) -> PortRoute {
    match plan {
        // WSL shares this machine's loopback, so its ports are already here
        // under the same number.
        LoopbackPlan::NoForwardNeeded => PortRoute::Direct,
        LoopbackPlan::ForwardOnPane(_) | LoopbackPlan::ForwardOnWorkspace(_) => PortRoute::Forward,
        // No plan and no forwarding: this machine's own ports open, and
        // another machine's do not.
        LoopbackPlan::Direct if local => PortRoute::Direct,
        LoopbackPlan::Direct => PortRoute::Blocked,
    }
}

pub(super) fn loopback_plan(
    enabled: bool,
    workspace: Option<&crate::terminal::PaneWorkspace>,
    remote_kind: Option<crate::daemon::protocol::RemoteKind>,
    pane_id: u64,
) -> LoopbackPlan {
    if !enabled {
        return LoopbackPlan::Direct;
    }
    if let Some(ws) = workspace {
        if ws.shares_localhost() {
            return LoopbackPlan::NoForwardNeeded;
        }
        if ws.spec.is_none() {
            log::warn!("remote workspace has no connection spec; not forwarding localhost links");
            return LoopbackPlan::Direct;
        }
        return LoopbackPlan::ForwardOnWorkspace(Box::new(ws.clone()));
    }
    match remote_kind {
        Some(crate::daemon::protocol::RemoteKind::NativeSsh) => {
            LoopbackPlan::ForwardOnPane(pane_id)
        }
        _ => LoopbackPlan::Direct,
    }
}

impl TerminalView {
    /// How forward requests about this pane reach the daemon that owns them —
    /// through the workspace when there is one, and by pane id when there is
    /// not. The Ports list and the port watcher build the same thing.
    pub(crate) fn forward_route(&self) -> crate::ui::forwards::route::ForwardRoute {
        crate::ui::forwards::route::ForwardRoute::new(self.pane_id, self.workspace.clone())
    }

    fn loopback_plan(&self, cx: &gpui_kit::App) -> LoopbackPlan {
        loopback_plan(
            cx.global::<Config>().ssh_loopback_forward,
            self.workspace.as_ref(),
            self.terminal.remote_context().map(|r| r.kind),
            self.pane_id,
        )
    }

    /// How a loopback port this pane is serving can be reached from here.
    ///
    /// The Ports list asks this about every listener it found, and it is a
    /// different question from "is this pane local": a remote pane's :3000 is
    /// perfectly reachable once a forward exists, and building that forward is
    /// something this app already knows how to do. Answering only "local or
    /// not" is what left the list showing a port it refused to open.
    pub(crate) fn port_route(&self, cx: &gpui_kit::App) -> PortRoute {
        port_route_of(&self.loopback_plan(cx), self.host_id().is_local())
    }
}

enum CmdKey {
    Consumed,
    Bubble,
    FallThrough,
}

const OPPORTUNISTIC_GIT_GAP: std::time::Duration = std::time::Duration::from_millis(1500);

/// How long a pane with an unknown or negative repository answer waits
/// before asking again. Discovery can miss temporarily unavailable metadata;
/// retry without issuing a request on every poll tick.
const GIT_RETRY_GAP: std::time::Duration = std::time::Duration::from_secs(10);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum GitRefresh {
    Edge,
    Opportunistic,
    /// Nothing happened; the cache just has no answer for this pane's cwd
    /// yet, or the last probe found no repository. Throttled by [`GIT_RETRY_GAP`].
    Retry,
}

/// The longest command line to put in a confirmation. The shell sends up to
/// 512 bytes; a dialog asking whether to end your work should still read as a
/// sentence.
const BUSY_COMMAND_MAX: usize = 60;

/// Undo the escaping the shell integration applies to an OSC 133;C payload so
/// it cannot break OSC framing: `%` and the four control bytes. Anything else
/// is left exactly as the user typed it.
fn unescape_mark_text(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(i) = rest.find('%') {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        let (decoded, width) = match tail.get(..3) {
            Some("%25") => ("%", 3),
            Some("%1B") | Some("%07") | Some("%0D") => ("", 3),
            Some("%0A") => (" ", 3),
            _ => ("%", 1),
        };
        out.push_str(decoded);
        rest = &tail[width..];
    }
    out.push_str(rest);
    out
}

fn clamp_command(cmd: &str) -> String {
    let cmd = cmd.trim();
    match cmd.chars().count() > BUSY_COMMAND_MAX {
        false => cmd.to_string(),
        true => format!(
            "{}…",
            cmd.chars()
                .take(BUSY_COMMAND_MAX)
                .collect::<String>()
                .trim_end()
        ),
    }
}

/// The Windows spelling of a WSL pane's POSIX cwd, for reading rather than
/// writing: the distro's `\\wsl$` share lets local `read_dir` reach the files.
///
/// Everything stays on the share, `/mnt/<drive>` included. Mapping the
/// automount back to the drive letter would list faster, but an absolute
/// word completes against its *cwd's* path prefix (`resolve_dir` keeps the
/// prefix when a rooted word lands on it) — so a drive-spelled cwd would send
/// `ls /etc<Tab>` to `C:\etc` instead of the distro's `/etc`. One prefix,
/// one meaning. A distro name with a path separator cannot name a share.
fn wsl_share_path(distro: &str, posix: &str) -> Option<std::path::PathBuf> {
    if distro.is_empty() || distro.contains(['\\', '/']) {
        return None;
    }
    let rest = posix.strip_prefix('/')?;
    Some(std::path::PathBuf::from(format!(
        r"\\wsl$\{distro}\{}",
        rest.replace('/', "\\")
    )))
}

/// The distro whose `\\wsl$` share holds a pane's filesystem, or `None` when
/// the pane is not a WSL one and Tab belongs to the shell.
///
/// The two kinds of WSL pane are told apart by who reports the distro. A pane
/// that runs `wsl.exe` is tagged by its remote context, and it reaches the
/// distro of whichever machine *hosts* it — so only this machine's panes may
/// take the share; a remote host's same-named distro would list the wrong
/// files. A pane in a WSL workspace is tagged by its workspace target instead,
/// and needs no such check: lyrel reaches those distros by running `wsl.exe`
/// here, so the share is this machine's by construction — even though the
/// pane's host, being the distro's own server, is not `HostId::LOCAL`.
fn wsl_share_distro(
    remote: Option<&crate::daemon::protocol::RemoteContext>,
    workspace: Option<&crate::terminal::PaneWorkspace>,
    host_is_local: bool,
) -> Option<String> {
    match remote {
        Some(remote) => (remote.kind == crate::daemon::protocol::RemoteKind::Wsl && host_is_local)
            .then(|| remote.target.clone()),
        None => match &workspace?.target {
            crate::core::session::RemoteTarget::Wsl { distro } => Some(distro.clone()),
            _ => None,
        },
    }
}

/// The directory the Files panel lists for a pane, spelled so the pane's
/// host can read it — `None` when no spelling can.
///
/// A cwd the host resolves natively is used as-is. Past that, only a pane
/// whose files sit in a WSL distro on this machine has another way in: its
/// POSIX cwd goes through the distro's `\\wsl$` share (#896 — handed to a
/// local `read_dir` raw, `/home/me` is `C:\home\me`, and the panel showed
/// "Could not be read"). A cwd that is already a Windows path came from the
/// Windows side of the pane and is readable as it stands. Anything else — a
/// shell ssh'd onward to a machine lyrel has no link to — has no spelling
/// here, and rooting the tree at it could only list nothing or the wrong
/// machine's files.
fn files_cwd(
    host_cwd: Option<std::path::PathBuf>,
    wsl_distro: Option<&str>,
    cwd: Option<std::path::PathBuf>,
) -> Option<std::path::PathBuf> {
    if host_cwd.is_some() {
        return host_cwd;
    }
    let distro = wsl_distro?;
    let cwd = cwd?;
    let spelled = cwd.to_string_lossy();
    match spelled.starts_with('/') {
        true => wsl_share_path(distro, &spelled),
        false => Some(cwd),
    }
}

fn fallback_chain(family: &str, configured: &[String]) -> Vec<String> {
    let mut chain = configured.to_vec();
    let mut pin = |name: &str| {
        if family != name && !chain.iter().any(|f| f == name) {
            chain.push(name.to_string());
        }
    };
    for name in crate::core::config::platform_last_resort_fallbacks() {
        pin(name);
    }
    pin("Hack");
    chain
}

/// Put `chain` on the regular face and on the bold and italic ones.
///
/// Bold and italic never carry a chain of their own — `alt_font` copies theirs
/// off the regular face when they are built — so a rebuild that skipped them
/// would leave two of the three faces resolving against the old chain.
fn apply_fallback_chain(
    chain: Vec<String>,
    font: &mut Font,
    bold: &mut Option<Font>,
    italic: &mut Option<Font>,
) {
    let fallbacks = Some(gpui_kit::FontFallbacks::from_fonts(chain));
    font.fallbacks = fallbacks.clone();
    if let Some(font) = bold {
        font.fallbacks = fallbacks.clone();
    }
    if let Some(font) = italic {
        font.fallbacks = fallbacks;
    }
}

impl TerminalView {
    pub fn from_shell_parts(
        mut parts: ShellParts,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        parts.terminal.claim_spawn();
        let mut view = Self::with_terminal(parts.terminal, parts.pane_id, window, cx);
        view.shell_spec = parts.shell_spec;
        view.owner_workspace = parts.owner;
        view.set_workspace(parts.workspace);
        view
    }

    pub fn owner_workspace(&self) -> Option<crate::core::session::WorkspaceId> {
        self.owner_workspace
    }

    pub fn from_native_ssh_parts(
        parts: NativeSshParts,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut view = Self::with_terminal(parts.terminal, parts.pane_id, window, cx);
        if let Some(name) = parts
            .persist
            .display_name
            .as_deref()
            .map(str::trim)
            .filter(|n| !n.is_empty())
        {
            view.default_title = name.to_string();
            view.title = name.to_string();
        }
        view.ssh_spec = Some(parts.persist);
        view
    }

    fn with_terminal(
        terminal: RemoteTerminal,
        pane_id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let config = cx.global::<Config>();
        let font_family = config.font_family.clone();
        let fallbacks = fallback_chain(&font_family, &config.font_fallbacks);
        let font_size = px(config.font_size);
        let line_height_mul = config.line_height;
        let font_features = config
            .font_features
            .as_ref()
            .map(crate::core::config::gpui_font_features);
        let report_mouse = config.mouse_reporting;
        let prompt_editor = config.prompt_editor;
        let agent_activity = super::agent_activity::AgentActivity::new(terminal.reattached());
        let mut font = gpui_kit::font(font_family);
        font.fallbacks = Some(gpui_kit::FontFallbacks::from_fonts(fallbacks.clone()));
        if let Some(features) = &font_features {
            font.features = features.clone();
        }
        let alt_font = |family: &Option<String>| {
            family.as_ref().map(|f| {
                let mut af = gpui_kit::font(f.clone());
                af.fallbacks = Some(gpui_kit::FontFallbacks::from_fonts(fallbacks.clone()));
                if let Some(features) = &font_features {
                    af.features = features.clone();
                }
                af
            })
        };
        let font_bold = alt_font(&config.font_family_bold);
        let font_italic = alt_font(&config.font_family_italic);

        let focus_handle = cx.focus_handle();

        let lifecycle = super::lifecycle::TerminalLifecycle::new(
            &terminal,
            &focus_handle,
            window,
            cx,
            super::lifecycle::LifecycleCallbacks {
                state: |view| &mut view.lifecycle,
                event: Self::handle_event,
                focus: |view, focused, cx| {
                    if focused {
                        view.agent_activity
                            .focused((view.host_id, view.pane_id), cx);
                    } else {
                        view.input.unmark();
                    }
                    view.report_focus_change(focused);
                },
                foreground: |view, window, cx| view.poll_foreground(window, cx),
                detach: |view| view.terminal.detach_link(),
            },
        );

        super::color_scheme::watch(cx);

        window.focus(&focus_handle, cx);

        Self {
            terminal,
            host_id: crate::ui::host_ops::HostId::LOCAL,
            workspace: None,
            pane_id,
            shell_spec: None,
            owner_workspace: None,
            ssh_spec: None,
            remote_paste: super::paste::RemotePaste::default(),
            remote_clipboard_write_in_flight: None,
            remote_clipboard_write_generation: 0,
            focus_handle,
            lifecycle,
            font,
            font_bold,
            font_italic,
            font_features,
            font_size,
            line_height_mul,
            cell_width: px(8.),
            line_height: px(17.),
            grid_slack: px(0.),
            grid_buf: Vec::new(),
            grid_snap: None,
            frame_alt_screen: false,
            frame_has_selection: false,
            selection_scroll: super::selection_scroll::SelectionScroll::default(),
            title: DEFAULT_TITLE.to_string(),
            pending_title: None,
            default_title: DEFAULT_TITLE.to_string(),
            relink_abandoned: false,
            relink_inflight: false,
            last_mouse_cell: None,
            cell_geom: None,
            report_mouse,
            links: LinkInteraction::default(),
            scroll_handle: TerminalScrollHandle::default(),
            search: None,
            dim: 1.,
            search_focused: false,
            composer: None,
            composer_focused: false,
            search_case_sensitive: false,
            search_regex: false,
            search_regex_error: false,
            search_last_query: String::new(),
            search_scan_epoch: 0,
            search_scan_armed: false,
            bell_flash: false,
            bell_epoch: 0,
            last_at_prompt: false,
            running_since: None,
            running_title: String::new(),
            running_agent: None,
            agent_activity,
            git_status_cwd: None,
            git_prompt_cycle: 0,
            input: super::prompt_input::PromptInput::new(prompt_editor),
            history: TerminalHistory::new(),
            completion: TerminalCompletion::default(),
            integration_notice: None,
            integration_notice_shown: false,
            created_at: std::time::Instant::now(),
        }
    }

    pub fn cursor_visible(&self) -> bool {
        self.lifecycle.cursor_visible()
    }

    pub fn show_cursor(&mut self) {
        self.lifecycle.show_cursor();
    }

    pub fn set_grid_size(
        &mut self,
        cols: usize,
        rows: usize,
        cell_width: Pixels,
        line_height: Pixels,
        scale: f32,
        cx: &mut Context<Self>,
    ) {
        // A column change reflows every wrapped line, and a match point is an
        // absolute (line, column) against the width it was scanned at — so
        // the highlights keep washing the *old* positions until something
        // rescans. Output rescans them (`Wakeup` → refresh), but a quiet
        // local pane has no output coming: without this the drift outlasts
        // the resize indefinitely (#586). Rescan with the output path's
        // discipline — it keeps the selection and never scrolls. Rows alone
        // reflow nothing, so a height-only drag stays cheap.
        let cols_changed = cols != self.terminal.size().cols;
        if (cols, rows) != (self.terminal.size().cols, self.terminal.size().rows) {
            self.links.invalidate_geometry();
        }
        self.cell_width = cell_width;
        self.line_height = line_height;
        // Report the cell size to the child in *device* pixels (logical × display
        // scale), so `ws_xpixel`/`ws_ypixel` describe the real framebuffer. A
        // pixel-aware program like terminal-browser renders its frame at that
        // native resolution; painted back into logical-pixel bounds, gpui blits
        // it ~1:1 on the framebuffer instead of upscaling a half-resolution
        // bitmap (which looked soft and magnified on Retina). This is what
        // kitty/ghostty report. `self.cell_width` stays logical — glyph layout
        // and mouse mapping work in logical pixels.
        let scale = if scale.is_finite() && scale > 0. {
            scale
        } else {
            1.
        };
        self.terminal.resize(
            TermSize::new(cols, rows),
            (cell_width.as_f32() * scale).round().max(1.) as u16,
            (line_height.as_f32() * scale).round().max(1.) as u16,
        );
        if cols_changed && self.search.is_some() {
            self.refresh_matches_after_output(cx);
        }
    }

    pub fn cwd(&self) -> Option<std::path::PathBuf> {
        self.terminal.foreground_cwd()
    }

    /// The title this pane is showing, or `None` while it is still answering
    /// to the app's own name — see [`stated_title`]. The label ladder reads
    /// this where the machine tree reads
    /// [`PaneRecord::osc_title`](lyrel_core::core::machine::PaneRecord::osc_title),
    /// which is what lets the tab strip and the switcher name a tab the same
    /// way.
    pub(crate) fn stated_title(&self) -> Option<&str> {
        stated_title(&self.title)
    }

    /// Sets how opaque the pane wants this terminal painted; the pane leaf
    /// calls this every frame while rendering, and the terminal element
    /// blends its colours toward the window background during paint (see
    /// [`Self::dim`] for why that beats an element-opacity style). The
    /// element resets the value to 1.0 at the end of every paint, so a
    /// render site that forgets to set it gets full brightness next frame —
    /// never a stale dim.
    pub(crate) fn set_dim(&mut self, dim: f32) {
        self.dim = dim;
    }

    pub fn remote_context(&self) -> Option<RemoteContext> {
        self.terminal.remote_context()
    }

    pub fn local_cwd(&self) -> Option<std::path::PathBuf> {
        self.paths_are_local().then(|| self.cwd())?
    }

    fn paths_are_local(&self) -> bool {
        self.remote_context().is_none() && self.host_id.is_local()
    }

    /// The directory a `~` in this pane's paths stands for, for anything that
    /// draws one of them shortened.
    ///
    /// The pane's own host answers, never this machine on its behalf (#580).
    /// A shell that has ssh'd somewhere gets no answer at all: the paths it
    /// reports are on a third machine lyrel has no link to, and neither this
    /// laptop's home nor the pane host's describes it — the same reason #568
    /// stopped resolving file links in those panes.
    pub fn display_home(&self, cx: &gpui_kit::App) -> Option<std::path::PathBuf> {
        match self.remote_context().is_some() {
            true => None,
            false => crate::ui::path_display::home_for_host(cx, self.host_id),
        }
    }

    pub fn spawnable_cwd(&self) -> Option<std::path::PathBuf> {
        self.remote_context().is_none().then(|| self.cwd())?
    }

    pub fn host(&self, cx: &gpui_kit::App) -> Option<crate::ui::host_ops::SharedHost> {
        crate::ui::host_registry::HostRegistry::lookup(cx, self.host_id)
    }

    pub fn host_id(&self) -> crate::ui::host_ops::HostId {
        self.host_id
    }

    pub fn workspace(&self) -> Option<&crate::terminal::PaneWorkspace> {
        self.workspace.as_ref()
    }

    pub fn set_workspace(&mut self, workspace: Option<crate::terminal::PaneWorkspace>) {
        self.host_id = workspace
            .as_ref()
            .map_or(crate::ui::host_ops::HostId::LOCAL, |w| w.target.host_id());
        // The pane answers to its workspace's name from here on: untitled tabs
        // show it, and a dead link's "— disconnected" suffix hangs off it
        // instead of the bare app name.
        if let Some(label) = workspace.as_ref().and_then(|w| w.label.clone()) {
            if self.title == self.default_title {
                self.title = label.clone();
            }
            self.default_title = label;
        }
        self.workspace = workspace;
        self.links.invalidate_geometry();
    }

    pub fn pane_route(&self) -> crate::terminal::PaneRoute {
        crate::terminal::PaneRoute::for_workspace(self.workspace.as_ref())
    }

    fn accepts_input(&self, cx: &gpui_kit::App) -> bool {
        // A local pane has no `PaneWorkspace`, but the window that owns it can
        // still have been taken over by a remote client.
        let Some(ws) = self
            .workspace()
            .map(|w| w.workspace)
            .or(self.owner_workspace)
        else {
            return true;
        };
        crate::ui::remote_workspace::workspace_accepts_input(cx, ws)
    }

    pub fn relink_plan(&self) -> (u64, TermSize, u16, u16) {
        (
            self.pane_id,
            self.terminal.size(),
            self.cell_width.as_f32().round() as u16,
            self.line_height.as_f32().round() as u16,
        )
    }

    pub fn adopt_relink(
        &mut self,
        stream: crate::daemon::transport::Stream,
        buffered: Vec<u8>,
        route: &crate::terminal::PaneRoute,
        size: TermSize,
        cell_w: u16,
        cell_h: u16,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<()> {
        self.remote_clipboard_write_generation =
            self.remote_clipboard_write_generation.wrapping_add(1);
        self.remote_clipboard_write_in_flight = None;
        self.terminal
            .adopt_relink(stream, buffered, route, size, cell_w, cell_h)?;
        self.relink_abandoned = false;
        self.relink_inflight = false;
        self.title = self.default_title.clone();
        cx.notify();
        Ok(())
    }

    pub fn detach_link(&mut self, cx: &mut Context<Self>) {
        self.terminal.detach_link();
        cx.notify();
    }

    pub fn host_cwd(&self) -> Option<std::path::PathBuf> {
        self.cwd_is_on_host().then(|| self.cwd())?
    }

    fn cwd_is_on_host(&self) -> bool {
        cwd_is_on_host(!self.paths_are_local(), self.host_id.is_local())
    }

    pub fn agent(&self) -> Option<crate::core::cli_agent::CLIAgent> {
        self.terminal.foreground_agent()
    }

    pub fn agent_session(&self) -> Option<crate::core::cli_agent::AgentSessionState> {
        self.terminal.agent_session()
    }

    /// What this pane is in the middle of, when it can say so. `None` means
    /// either nothing is running or the shell never told us — and a terminal
    /// that guessed would raise this question on every single close.
    pub fn busy(&self) -> Option<PaneBusy> {
        use crate::core::cli_agent::AgentStatus;
        // `Done` is the opposite of busy: the turn is over, and the badge
        // saying so is exactly what sends a reader to close the tab. Only a
        // turn still in flight — running, or stopped on a question — is work
        // that closing would cut short.
        if let Some(agent) = self.agent()
            && self
                .agent_session()
                .is_some_and(|s| matches!(s.status, AgentStatus::Working | AgentStatus::Waiting))
        {
            return Some(PaneBusy::Agent(agent.display_name()));
        }
        // Without shell integration `at_prompt` is permanently false, so
        // `running_since` is permanently Some. Only trust it when the shell is
        // actually reporting.
        if !self.terminal.shell_active() {
            return None;
        }
        self.running_since?;
        // Prefer what the shell said it was running. `running_title` is only
        // the window title as it stood when the command began, and a prompt
        // that titles by directory — a very common setup — made this ask
        // "lyrel is still running. Closing ends it." about a folder. The
        // OSC 133;C mark carries the submitted line itself.
        let named = Some(clamp_command(&unescape_mark_text(
            &self.terminal.running_command(),
        )))
        .filter(|t| !t.is_empty());
        Some(PaneBusy::Command(match named {
            Some(cmd) => cmd,
            None => match self.running_title.trim().is_empty() {
                true => self.title.clone(),
                false => self.running_title.clone(),
            },
        }))
    }

    pub fn agent_result_unread(&self) -> bool {
        self.agent_activity.unread()
    }

    pub fn mark_agent_result_unread(&mut self, refocus_incoming: bool, cx: &mut App) {
        self.agent_activity
            .mark_unread((self.host_id, self.pane_id), refocus_incoming, cx);
    }

    pub fn git_status(&self, cx: &App) -> Option<crate::terminal::git_status::GitStatus> {
        let cwd = self.git_status_cwd.as_ref()?;
        cx.try_global::<crate::terminal::git_status::GitStatusCache>()?
            .status_for(self.host_id, cwd)
    }

    pub fn git_status_cwd(&self) -> Option<&std::path::Path> {
        self.git_status_cwd.as_deref()
    }

    /// Retry unknown and negative repo answers. A transient discovery failure
    /// must not leave an idle pane in Ungrouped until it moves or restarts.
    ///
    /// Read-only on purpose: this runs on every poll tick, and the cache is a
    /// global whose every mutable touch redraws the window — while a probe is
    /// in flight or throttled, the answer has to be "no" at no cost.
    fn git_retry_due(&self, cx: &App) -> bool {
        let Some(cwd) = self.git_status_cwd.as_deref() else {
            return false;
        };
        let Some(cache) = cx.try_global::<crate::terminal::git_status::GitStatusCache>() else {
            return false;
        };
        cache.retry_due(self.host_id, cwd, GIT_RETRY_GAP)
    }

    fn recheck_git_root(&self, cx: &mut Context<Self>) {
        if !self.lifecycle.is_displayed() {
            return;
        }
        let Some(cwd) = self.git_status_cwd.as_deref() else {
            return;
        };
        let Some(host) = self.host(cx).filter(|host| host.is_connected()) else {
            return;
        };
        let id = self.host_id;
        let Some(request) = cx
            .global::<crate::terminal::git_status::GitStatusCache>()
            .begin_root_check(id, cwd, GIT_RETRY_GAP)
        else {
            return;
        };
        let cwd = cwd.to_path_buf();
        let probe_cwd = cwd.clone();
        let pane = cx.weak_entity();
        crate::ui::host_ops::HostOps::run_detached(
            host,
            cx,
            move |host| crate::terminal::git_status::discover_root(host, &probe_cwd),
            move |cx, result| {
                let changed = cx
                    .global::<crate::terminal::git_status::GitStatusCache>()
                    .finish_root_check(id, &cwd, request, result);
                if changed {
                    let _ = pane.update(cx, |view, cx| {
                        if view.host_id == id
                            && view.git_status_cwd.as_deref() == Some(&cwd)
                            && view.lifecycle.is_displayed()
                        {
                            view.refresh_git_status(Some(cwd), GitRefresh::Edge, cx);
                        }
                    });
                }
            },
        );
    }

    /// Plant the cwd the git-status poll would have found. For tests that
    /// need a pane to look like it is sitting somewhere known — a real poll
    /// needs a live shell reporting a directory, which a quiet test pane has
    /// no way to do.
    #[cfg(test)]
    pub(crate) fn set_git_status_cwd_for_test(&mut self, cwd: Option<std::path::PathBuf>) {
        if self.git_status_cwd != cwd {
            self.links.invalidate_geometry();
        }
        self.git_status_cwd = cwd;
    }

    /// The directory this pane's *work* is happening in — what every panel
    /// that answers "where am I?" should show.
    ///
    /// [`Self::cwd`] is the kernel's idea: the cwd of the foreground process.
    /// That is right for a shell, and wrong for a coding agent, because an
    /// agent moving into a git worktree does not `chdir` — the `claude`
    /// process stays where it was launched while the session works somewhere
    /// else entirely. The hook stream carries the agent's own cwd for exactly
    /// this reason (`AgentSessionState::cwd`), and the git-status poll already
    /// folds the two together into `git_status_cwd`; this reads that result
    /// back out under a name that doesn't imply it's only about git.
    ///
    /// `git_status_cwd` is only ever set for a pane whose paths belong to its
    /// host, so the fallback here is what decides that: this one takes any
    /// cwd, [`Self::effective_host_cwd`] takes only one the host can resolve.
    pub fn effective_cwd(&self) -> Option<std::path::PathBuf> {
        self.git_status_cwd.clone().or_else(|| self.cwd())
    }

    /// [`Self::effective_cwd`], restricted to paths the pane's host can act
    /// on — for callers that will hand the result to a `Host` call.
    pub fn effective_host_cwd(&self) -> Option<std::path::PathBuf> {
        self.git_status_cwd.clone().or_else(|| self.host_cwd())
    }

    /// The directory the Files panel roots its tree at — see [`files_cwd`].
    ///
    /// Follows a coding agent the way [`Self::effective_cwd`] does. A WSL
    /// pane gets no `git_status_cwd` (its paths are not its host's), so the
    /// agent's own cwd is read here directly before the shell's.
    pub fn files_cwd(&self) -> Option<std::path::PathBuf> {
        let distro = wsl_share_distro(
            self.terminal.remote_context().as_ref(),
            self.workspace.as_ref(),
            self.host_id.is_local(),
        );
        let cwd = self
            .terminal
            .agent_session()
            .and_then(|s| s.cwd)
            .or_else(|| self.cwd());
        files_cwd(self.effective_host_cwd(), distro.as_deref(), cwd)
    }

    pub fn refresh_git_status_now(&mut self, cx: &mut Context<Self>) {
        let cwd = self.git_status_cwd.clone();
        if cwd.is_some() {
            self.refresh_git_status(cwd, GitRefresh::Opportunistic, cx);
        }
    }

    pub fn selection_text(&self) -> Option<String> {
        self.terminal
            .term
            .lock()
            .selection_to_string()
            .filter(|t| !t.trim().is_empty())
    }

    pub fn send_agent_prompt(&self, prompt: &str) {
        self.terminal
            .write(crate::core::agent_prompt::submit_bytes(prompt));
    }

    pub fn run_command_line(&self, cmd: &str) {
        self.terminal.write(format!("{cmd}\r").into_bytes());
    }

    pub fn shell_spec(&self) -> Option<ShellSpec> {
        self.shell_spec.clone()
    }

    /// The shell binary this pane is running, for the path-quoting rules.
    ///
    /// `None` before the pane has resolved one, which [`crate::core::shell_quote::quote_for_shell`]
    /// answers from the platform — PowerShell on Windows, POSIX elsewhere.
    /// The one pane that guess is wrong for is a cmd.exe pane that has not
    /// reported in yet.
    pub(super) fn shell_program(&self) -> Option<String> {
        self.shell_spec.as_ref().map(|s| s.program.clone())
    }

    pub fn ssh_spec(&self) -> Option<Box<crate::daemon::protocol::NativeSshSpec>> {
        self.ssh_spec.clone()
    }

    pub(crate) fn set_restored_ssh_spec(
        &mut self,
        spec: Option<Box<crate::daemon::protocol::NativeSshSpec>>,
    ) {
        self.ssh_spec = spec;
    }

    pub fn ssh_phase(&self) -> Option<crate::daemon::protocol::SshPhase> {
        self.terminal.ssh_phase()
    }

    pub fn ssh_disconnected(&self) -> bool {
        self.ssh_spec.is_some() && self.terminal.exited
    }

    /// Whether this pane is a workspace pane whose link died under it — the
    /// far-end session should still be alive, so the link supervisor keeps
    /// asking for it back. A child that exited is over, not disconnected, and
    /// a pane whose machine already refused the relink is past asking.
    pub fn wants_relink(&self) -> bool {
        self.workspace.is_some()
            && self.terminal.exited
            && !self.terminal.child_exited()
            && !self.relink_abandoned
            && !self.relink_inflight
    }

    /// Claims this pane for one relink attempt. Every path that dials for a
    /// pane calls this first, so the other paths leave it alone until the
    /// attempt reports back through `relink_settled` or `adopt_relink`.
    pub fn mark_relinking(&mut self) {
        self.relink_inflight = true;
    }

    /// Releases the claim `mark_relinking` took, for the attempts that end
    /// without a stream to adopt. A pane freed this way is up for asking
    /// again on the next sweep.
    pub fn relink_settled(&mut self) {
        self.relink_inflight = false;
    }

    /// Records that this pane's machine refused to give the pane back — it is
    /// gone at the far end — so the link supervisor stops asking. The pane
    /// keeps its disconnected face; only the retrying stops.
    pub fn abandon_relink(&mut self) {
        self.relink_abandoned = true;
        self.relink_inflight = false;
    }

    fn has_selection(&self) -> bool {
        self.terminal.term.lock().selection.is_some()
    }

    /// Whether *this frame* draws a selection. The grid half comes from
    /// [`Self::sync_frame_facts`] rather than the terminal, which is what keeps
    /// the draw off the lock; a selection that appears while the reader holds
    /// it is drawn one frame later.
    fn any_selection(&self) -> bool {
        self.frame_has_selection
            || (self.input_active() && self.input.editor().selected_text().is_some())
    }

    fn jump_to_prompt(&mut self) {
        let mut term = self.terminal.term.lock();
        self.selection_scroll.jump_to_prompt(&mut term);
        drop(term);
        self.terminal.viewport_reporter().report();
    }

    pub fn select_all(&mut self, cx: &mut Context<Self>) {
        let mut term = self.terminal.term.lock();
        self.selection_scroll.select_all(&mut term);
        drop(term);
        cx.notify();
    }

    pub fn mouse_mode(&self) -> bool {
        self.report_mouse
            && self
                .terminal
                .term
                .lock()
                .mode()
                .intersects(Mode::MOUSE_MODE)
    }

    fn write_mouse(&self, base: u8, mods: &Modifiers, col: usize, row: usize, pressed: bool) {
        let sgr = self.terminal.term.lock().mode().contains(Mode::SGR_MOUSE);
        if let Some(msg) = encode_mouse(sgr, base, mods, col, row, pressed) {
            self.terminal.write(msg);
        }
    }

    pub fn mouse_press(&mut self, button: MouseButton, col: usize, row: usize, mods: &Modifiers) {
        let base = match button {
            MouseButton::Left => 0,
            MouseButton::Middle => 1,
            MouseButton::Right => 2,
            _ => return,
        };
        self.last_mouse_cell = Some((col, row));
        self.write_mouse(base, mods, col, row, true);
    }

    pub fn mouse_release(&mut self, button: MouseButton, col: usize, row: usize, mods: &Modifiers) {
        let base = match button {
            MouseButton::Left => 0,
            MouseButton::Middle => 1,
            MouseButton::Right => 2,
            _ => return,
        };
        self.write_mouse(base, mods, col, row, false);
    }

    pub fn mouse_drag(&mut self, button: MouseButton, col: usize, row: usize, mods: &Modifiers) {
        if self.last_mouse_cell == Some((col, row)) {
            return;
        }
        let wants = self.report_mouse
            && self
                .terminal
                .term
                .lock()
                .mode()
                .intersects(Mode::MOUSE_DRAG | Mode::MOUSE_MOTION);
        if !wants {
            return;
        }
        self.last_mouse_cell = Some((col, row));
        let base = match button {
            MouseButton::Left => 32,
            MouseButton::Middle => 33,
            MouseButton::Right => 34,
            _ => return,
        };
        self.write_mouse(base, mods, col, row, true);
    }

    pub fn mouse_motion(&mut self, col: usize, row: usize, mods: &Modifiers) {
        if self.last_mouse_cell == Some((col, row)) {
            return;
        }
        if !self.report_mouse
            || !self
                .terminal
                .term
                .lock()
                .mode()
                .contains(Mode::MOUSE_MOTION)
        {
            return;
        }
        self.last_mouse_cell = Some((col, row));
        self.write_mouse(35, mods, col, row, true);
    }

    /// `at` is the cell under the pointer when the wheel turned. A wheel
    /// report has to say where it happened — tmux scrolls the pane under the
    /// pointer, or its window list — and the last click is no stand-in for
    /// that: a program asking for button events only (1002) is never told the
    /// pointer moved away from it (#1131).
    pub fn scroll(
        &mut self,
        lines: i32,
        at: Option<(usize, usize)>,
        mods: &Modifiers,
        cx: &mut Context<Self>,
    ) {
        if lines == 0 {
            return;
        }
        self.selection_scroll.cancel_animation();
        let mut mode = self.terminal.term.lock().mode();
        if !self.report_mouse {
            mode.remove(Mode::MOUSE_MODE);
        }
        match wheel_route(mode, mods.shift, lines > 0) {
            WheelRoute::Report { base } => {
                let (col, row) = at.or(self.last_mouse_cell).unwrap_or((0, 0));
                for _ in 0..lines.unsigned_abs() {
                    self.write_mouse(base, mods, col, row, true);
                }
            }
            WheelRoute::Arrows { seq } => {
                let mut out = Vec::with_capacity(seq.len() * lines.unsigned_abs() as usize);
                for _ in 0..lines.unsigned_abs() {
                    out.extend_from_slice(seq);
                }
                self.terminal.write(out);
            }
            WheelRoute::Scrollback => {
                self.selection_scroll
                    .scroll_lines(&mut self.terminal.term.lock(), lines);
                self.terminal.viewport_reporter().report();
                cx.notify();
            }
        }
    }

    pub fn find_step(&mut self, forward: bool, cx: &mut Context<Self>) {
        let direction = if forward {
            Direction::Right
        } else {
            Direction::Left
        };
        self.step_match(direction, cx);
    }

    /// Plain text of the retained buffer, including history and soft-wrapped
    /// Unicode graphemes. Reading it does not alter selection or scroll offset.
    pub fn scrollback_text(&self) -> String {
        let term = self.terminal.term.lock();
        term.bounds_to_string(
            Pos::new(term.topmost_line(), Column(0)),
            Pos::new(term.bottommost_line(), term.last_column()),
        )
    }

    /// Clear presentation without discarding retained history or restarting
    /// the process. Only a live pane receives Ctrl+L through its own transport.
    pub fn clear_screen(&mut self, cx: &mut Context<Self>) {
        self.selection_scroll.cancel_animation();
        if self.terminal.clear_presentation(false) {
            self.terminal.write(vec![0x0c_u8]);
        }
        cx.notify();
    }

    pub fn clear_scrollback(&mut self, cx: &mut Context<Self>) {
        self.selection_scroll.clear_scrollback();
        if self.terminal.clear_presentation(true) {
            self.terminal.write(vec![0x0c_u8]);
        }
        cx.notify();
    }

    pub fn set_font_family(&mut self, family: String, cx: &mut Context<Self>) {
        let mut font = gpui_kit::font(family);
        if let Some(features) = &self.font_features {
            font.features = features.clone();
        }
        self.font = font;
        // Rebuild rather than carry the chain over: `fallback_chain` skips
        // pinning a last-resort face that the family already is, so the chain
        // that went with the old family can be missing a pin the new one needs.
        self.reread_fallback_chain(cx);
        cx.notify();
    }

    /// Rebuild the fallback chain from the config and put it on all three faces.
    ///
    /// The chain is built once in `with_terminal` and then only ever cloned
    /// around, so a `font_fallbacks` edit reaches new panes and no one else.
    pub fn reread_fallback_chain(&mut self, cx: &mut Context<Self>) {
        let chain = fallback_chain(&self.font.family, &cx.global::<Config>().font_fallbacks);
        apply_fallback_chain(
            chain,
            &mut self.font,
            &mut self.font_bold,
            &mut self.font_italic,
        );
    }

    pub fn set_font_family_bold(&mut self, family: Option<String>, cx: &mut Context<Self>) {
        self.font_bold = self.alt_font(family);
        cx.notify();
    }

    pub fn set_font_family_italic(&mut self, family: Option<String>, cx: &mut Context<Self>) {
        self.font_italic = self.alt_font(family);
        cx.notify();
    }

    pub fn set_font_features(
        &mut self,
        features: Option<gpui_kit::FontFeatures>,
        cx: &mut Context<Self>,
    ) {
        self.font_features = features.clone();
        let apply = |font: &mut Font| {
            font.features = features.clone().unwrap_or_default();
        };
        apply(&mut self.font);
        if let Some(font) = &mut self.font_bold {
            apply(font);
        }
        if let Some(font) = &mut self.font_italic {
            apply(font);
        }
        cx.notify();
    }

    fn alt_font(&self, family: Option<String>) -> Option<Font> {
        family.map(|f| {
            let mut af = gpui_kit::font(f);
            af.fallbacks = self.font.fallbacks.clone();
            if let Some(features) = &self.font_features {
                af.features = features.clone();
            }
            af
        })
    }

    /// Tell the source control cache that a command just ran here.
    ///
    /// The `.git` watch catches anything that writes the repository, and the
    /// file tree catches edits in the directories it is showing. What is left
    /// is the common case neither sees: a command that edits a file somewhere
    /// the tree is not looking. A command boundary is the cheapest honest
    /// signal that that may have happened.
    ///
    /// Only the epoch moves. Scheduling the debounced re-read needs the app
    /// entity, which a pane does not hold — but `refresh_git_status` below
    /// writes `GitStatusCache`, the app observes that global, and the panel's
    /// next render finds the repository stale and asks. One notify, not two.
    fn mark_repo_changed(&self, cwd: Option<&std::path::Path>, cx: &mut Context<Self>) {
        use crate::terminal::git_data::ScmData;
        use crate::terminal::git_status::GitStatusCache;

        let Some(cwd) = cwd else { return };
        let Some(root) = cx
            .try_global::<GitStatusCache>()
            .and_then(|cache| cache.repo_root_for(self.host_id, cwd))
            .map(std::path::Path::to_path_buf)
        else {
            return;
        };
        cx.default_global::<ScmData>().bump(self.host_id, &root);
    }

    fn refresh_git_status(
        &mut self,
        cwd: Option<std::path::PathBuf>,
        trigger: GitRefresh,
        cx: &mut Context<Self>,
    ) {
        use crate::terminal::git_status::GitStatusCache;

        let changed = self.git_status_cwd != cwd;
        self.git_status_cwd = cwd.clone();
        if changed {
            self.links.invalidate_geometry();
        }
        let Some(cwd) = cwd else {
            if changed {
                cx.notify();
            }
            return;
        };
        let id = self.host_id;
        let Some(host) = self.host(cx) else {
            if changed {
                cx.notify();
            }
            return;
        };
        if !host.is_connected() {
            if changed {
                cx.notify();
            }
            return;
        }
        cx.default_global::<GitStatusCache>();
        let claimed = cx.update_global::<GitStatusCache, _>(|cache, _| match trigger {
            GitRefresh::Edge => cache.begin_probe(id, &cwd),
            GitRefresh::Opportunistic => {
                cache.begin_probe_throttled(id, &cwd, OPPORTUNISTIC_GIT_GAP)
            }
            GitRefresh::Retry => cache.begin_probe_throttled(id, &cwd, GIT_RETRY_GAP),
        });
        if !claimed {
            return;
        }
        let probe_cwd = cwd.clone();
        let pane = cx.weak_entity();
        crate::ui::host_ops::HostOps::run_detached(
            host,
            cx,
            move |h| crate::terminal::git_status::probe_repo(h, &probe_cwd),
            move |cx, result| {
                use crate::terminal::git_status::RepoProbe;
                let rerun = cx.update_global::<GitStatusCache, _>(|cache, _| match result {
                    RepoProbe::Repo(snap) => cache.finish_probe(id, &cwd, Some(snap)),
                    RepoProbe::NotARepo => cache.finish_probe(id, &cwd, None),
                    RepoProbe::Failed => cache.fail_probe(id, &cwd),
                });
                if rerun {
                    let _ = pane.update(cx, |view, cx| {
                        if view.git_status_cwd.as_deref() == Some(&cwd) {
                            view.refresh_git_status(Some(cwd), GitRefresh::Edge, cx);
                        }
                    });
                }
            },
        );
    }

    pub fn on_select_start(
        &mut self,
        col: usize,
        row: usize,
        left: bool,
        clicks: usize,
        shift: bool,
        cx: &mut Context<Self>,
    ) {
        let mut term = self.terminal.term.lock();
        self.selection_scroll.start(
            &mut term,
            col,
            row,
            left,
            clicks,
            shift,
            cx.global::<Config>(),
        );
        drop(term);
        cx.notify();
    }

    pub fn on_select_update(&mut self, col: usize, row: usize, left: bool, cx: &mut Context<Self>) {
        if !self.selection_scroll.is_selecting() {
            return;
        }
        let mut term = self.terminal.term.lock();
        self.selection_scroll.update(
            &mut term,
            col,
            row,
            left,
            &cx.global::<Config>().word_separators,
        );
        drop(term);
        cx.notify();
    }

    pub fn select_autoscroll(
        &mut self,
        overshoot: f32,
        col: usize,
        left: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(epoch) = self.selection_scroll.autoscroll(overshoot, col, left) else {
            return;
        };
        self.drag_scroll_tick(epoch, cx);
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(50))
                    .await;
                if !matches!(
                    this.update(cx, |view, cx| view.drag_scroll_tick(epoch, cx)),
                    Ok(true)
                ) {
                    break;
                }
            }
        })
        .detach();
    }

    fn drag_scroll_tick(&mut self, epoch: u64, cx: &mut Context<Self>) -> bool {
        let moved = self.selection_scroll.drag_tick(
            epoch,
            || self.terminal.term.lock(),
            &cx.global::<Config>().word_separators,
        );
        let Some(moved) = moved else {
            return false;
        };
        self.terminal.viewport_reporter().report();
        if moved {
            cx.notify();
        }
        true
    }

    pub fn on_select_end(&mut self, cx: &mut Context<Self>) {
        use super::selection_scroll::SelectEndCopy;
        let editor_gesture = self.input.end_selection();
        let copy = self
            .selection_scroll
            .end(cx.global::<Config>().copy_on_select, editor_gesture);
        match copy {
            SelectEndCopy::None => {}
            SelectEndCopy::Grid => self.copy_selection(cx),
            SelectEndCopy::Editor => {
                if let Some(text) = self.input.editor().selected_text() {
                    cx.write_to_clipboard(ClipboardItem::new_string(text));
                }
            }
        }
    }

    fn on_scroll(&mut self, ev: &ScrollEvent, window: &mut Window, cx: &mut Context<Self>) {
        use super::selection_scroll::WheelMotion;
        let quantized = || {
            !ev.modifiers.shift && {
                let mode = self.terminal.term.lock().mode();
                mode.intersects(Mode::MOUSE_MODE)
                    || mode.contains(Mode::ALT_SCREEN | Mode::ALTERNATE_SCROLL)
            }
        };
        match self.selection_scroll.wheel(
            ev,
            self.line_height.as_f32(),
            cx.global::<Config>(),
            cx.platform_info().os,
            quantized,
        ) {
            WheelMotion::Zoom(steps) => {
                // Share clamping and persistence with keyboard and menu zoom.
                for _ in 0..steps.unsigned_abs() {
                    if steps > 0 {
                        window.dispatch_action(Box::new(IncreaseFontSize), cx);
                    } else {
                        window.dispatch_action(Box::new(DecreaseFontSize), cx);
                    }
                }
            }
            WheelMotion::Quantized(lines) => {
                if lines != 0 {
                    let at = self
                        .cell_geom
                        .map(|geom| geom.pos_to_cell(ev.position))
                        .map(|(col, row, _)| (col, row));
                    self.scroll(lines, at, &ev.modifiers, cx);
                }
            }
            WheelMotion::Animate(delta) => {
                if let Some(epoch) = self.selection_scroll.queue_animation(delta) {
                    schedule_scroll_anim_frame(cx.weak_entity(), epoch, window);
                    cx.notify();
                }
            }
            WheelMotion::Direct(delta) => {
                self.smooth_scroll(delta, cx);
            }
        }
    }

    /// Apply a direct viewport movement and report only an actual visual change.
    fn smooth_scroll(&mut self, delta: f32, cx: &mut Context<Self>) -> bool {
        let mut term = self.terminal.term.lock();
        let moved = self.selection_scroll.smooth_scroll(&mut term, delta);
        drop(term);
        self.terminal.viewport_reporter().report();
        if moved {
            cx.notify();
        }
        moved
    }

    fn scroll_anim_frame(&mut self, epoch: u64, cx: &mut Context<Self>) -> bool {
        let frame = self
            .selection_scroll
            .animation_frame(epoch, || self.terminal.term.lock());
        let Some((moved, running)) = frame else {
            return false;
        };
        self.terminal.viewport_reporter().report();
        if moved {
            cx.notify();
        }
        running
    }

    /// Settle up with the scrollback bar for this frame: move the viewport
    /// where a drag asked for, then tell the bar where the grid ended up.
    ///
    /// Both halves belong here rather than in the handle, so the bar — which
    /// runs from a mouse handler, with no pane to call into — never reaches
    /// into the terminal behind the pane's back.
    fn sync_scrollbar(&mut self) {
        if let Some(target) = self.scroll_handle.take_pending() {
            let mut term = self.terminal.term.lock();
            self.selection_scroll.scroll_to(&mut term, target);
        }
        // Not worth a wait: the scrollbar is a picture of where the grid is,
        // and a frame that cannot have the lock keeps the picture it drew last
        // time rather than parking the whole window to refresh a thumb.
        let Some(term) = self.terminal.term.try_lock() else {
            return;
        };
        let grid = GridScroll {
            history: term.grid.history_size(),
            display_offset: term.grid.display_offset(),
            screen_lines: term.screen_lines(),
            line_height: self.line_height.as_f32(),
        };
        drop(term);
        self.scroll_handle.sync(grid);
        self.terminal.viewport_reporter().report();
    }

    /// Re-read the two things the frame itself declares — the terminal mode its
    /// keymap context is built from, and whether there is a selection to draw.
    ///
    /// One `try_lock` at the top of the frame, not one per reader: every
    /// caller inside `render` would otherwise take the lock separately, and
    /// each of those is another chance to sit behind the pane's reader with the
    /// whole window's frame in hand. Failing to get it leaves the previous
    /// frame's answers in place, which is the same bargain the grid makes in
    /// [`TerminalElement::build_grid`].
    fn sync_frame_facts(&mut self) {
        let Some(term) = self.terminal.term.try_lock() else {
            return;
        };
        self.frame_alt_screen = term.mode().contains(Mode::ALT_SCREEN);
        self.frame_has_selection = term.selection.is_some();
    }

    /// The scrollback bar, laid down the right edge of the grid.
    ///
    /// The track is inset to the rows themselves — [`GRID_PAD_Y`] is padding
    /// the grid never scrolls through, and counting it would leave the thumb
    /// short of the ends by that much.
    fn render_scrollbar(&self) -> impl IntoElement + use<> {
        div()
            .absolute()
            .top(px(GRID_PAD_Y))
            .left_0()
            .right_0()
            .h(self.line_height * self.terminal.size().rows as f32)
            // No `scrollbar_show` override: the bar takes `cx.theme()`'s, which
            // `apply_theme` pins to `Scrolling` for every list in the app. A
            // pane disagreeing with the sidebar about when a scrollbar is worth
            // showing would be the odd one out.
            .child(Scrollbar::vertical(&self.scroll_handle).id("terminal-scrollbar"))
    }
}

/// Why closing a pane would end work that is still going on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PaneBusy {
    Command(String),
    Agent(&'static str),
}

impl Focusable for TerminalView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Drop for TerminalView {
    fn drop(&mut self) {
        self.flush_pending_history();
    }
}

#[derive(Debug, PartialEq)]
enum WheelRoute {
    Report { base: u8 },
    Arrows { seq: &'static [u8] },
    Scrollback,
}

fn wheel_route(mode: Mode, shift: bool, up: bool) -> WheelRoute {
    if !shift && mode.intersects(Mode::MOUSE_MODE) {
        return WheelRoute::Report {
            base: if up { 64 } else { 65 },
        };
    }
    if !shift && mode.contains(Mode::ALT_SCREEN | Mode::ALTERNATE_SCROLL) {
        let seq: &'static [u8] = match (up, mode.contains(Mode::APP_CURSOR)) {
            (true, true) => b"\x1bOA",
            (true, false) => b"\x1b[A",
            (false, true) => b"\x1bOB",
            (false, false) => b"\x1b[B",
        };
        return WheelRoute::Arrows { seq };
    }
    WheelRoute::Scrollback
}

/// Hands a path to whatever the OS has it associated with. Also the fallback
/// for a directory the file tree cannot reach.
///
/// The path is spelled this OS's way first: Explorer takes `c:/a.png` as a
/// request to show Documents.
pub(crate) fn open_file_path(path: &std::path::Path) -> std::io::Result<()> {
    let path = &super::search::spelled_for(super::search::PathStyle::NATIVE, path);
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else if cfg!(windows) {
        "explorer"
    } else {
        "xdg-open"
    };
    std::process::Command::new(opener).arg(path).spawn()?;
    Ok(())
}

/// Shows a path where it lives, rather than opening it.
///
/// `open -R` and `explorer /select,` both select the file inside its folder;
/// no desktop-neutral Linux equivalent exists, so there the folder is opened
/// and the file is left for the eye to find.
pub(crate) fn reveal_file_path(path: &std::path::Path) -> std::io::Result<()> {
    let path = &super::search::spelled_for(super::search::PathStyle::NATIVE, path);
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut c = std::process::Command::new("open");
        c.arg("-R").arg(path);
        c
    };
    // Explorer wants `/select,` bare and the path quoted behind it. `arg`
    // quotes the whole thing the moment the path holds a space, and Explorer
    // answers a quoted switch by opening Documents and reporting success —
    // so the command line is written out by hand.
    #[cfg(windows)]
    let mut command = {
        use std::os::windows::process::CommandExt;
        let mut c = std::process::Command::new("explorer");
        c.raw_arg(format!("/select,\"{}\"", path.display()));
        c
    };
    #[cfg(not(any(target_os = "macos", windows)))]
    let mut command = {
        let mut c = std::process::Command::new("xdg-open");
        c.arg(path.parent().unwrap_or(path));
        c
    };
    command.spawn()?;
    Ok(())
}

fn encode_mouse(
    sgr: bool,
    base: u8,
    mods: &Modifiers,
    col: usize,
    row: usize,
    pressed: bool,
) -> Option<Vec<u8>> {
    let mut mod_bits = 0u8;
    if mods.shift {
        mod_bits += 4;
    }
    if mods.alt {
        mod_bits += 8;
    }
    if mods.control {
        mod_bits += 16;
    }

    if sgr {
        let c = if pressed { 'M' } else { 'm' };
        let msg = format!("\x1b[<{};{};{}{}", base + mod_bits, col + 1, row + 1, c);
        Some(msg.into_bytes())
    } else {
        if col >= 223 || row >= 223 {
            return None;
        }
        let code = if pressed {
            base + mod_bits
        } else {
            3 + mod_bits
        };
        Some(vec![
            0x1b,
            b'[',
            b'M',
            32 + code,
            (32 + 1 + col) as u8,
            (32 + 1 + row) as u8,
        ])
    }
}

/// Advance the in-flight scroll animation once per presented frame.
///
/// Registered by the wheel adapter: gpui runs the callback
/// immediately before the next frame is drawn, and it re-registers itself as
/// long as the animation lives. Stepping at frame time instead of on a
/// free-running timer keeps the decay steps locked to the monitor's vblank
/// cadence — a 16 ms timer drifting against a 16.67 ms vblank makes some
/// presented frames show twice the movement of their neighbours and others
/// none, which reads as stutter. Frame-aligned stepping shows every frame
/// exactly the ground it covered. The weak handle and the epoch guard make the
/// chain die quietly when the pane is closed or the animation is cancelled.
fn schedule_scroll_anim_frame(view: WeakEntity<TerminalView>, epoch: u64, window: &mut Window) {
    window.on_next_frame(move |window, cx| {
        let Some(view) = view.upgrade() else {
            return;
        };
        if view.update(cx, |view, cx| view.scroll_anim_frame(epoch, cx)) {
            schedule_scroll_anim_frame(view.downgrade(), epoch, window);
        }
    });
}

#[cfg(test)]
mod tests {

    #[test]
    fn a_busy_command_name_undoes_the_shell_escaping() {
        use super::unescape_mark_text;
        // The integration escapes % and the four bytes that would break OSC
        // framing; everything else is the line as typed.
        assert_eq!(unescape_mark_text("printf '100%25'"), "printf '100%'");
        assert_eq!(unescape_mark_text("a%1Bb%07c%0Dd"), "abcd");
        assert_eq!(unescape_mark_text("one%0Atwo"), "one two");
        // A bare % the shell somehow left alone must survive rather than eat
        // the next two characters.
        assert_eq!(unescape_mark_text("50% off"), "50% off");
        assert_eq!(unescape_mark_text("cargo build"), "cargo build");
    }

    #[test]
    fn a_busy_command_name_stays_short_enough_to_read() {
        use super::{BUSY_COMMAND_MAX, clamp_command};
        assert_eq!(clamp_command("  sleep 300  "), "sleep 300");
        let long = "cargo ".repeat(40);
        let out = clamp_command(&long);
        assert!(out.ends_with('…'), "{out:?}");
        assert!(out.chars().count() <= BUSY_COMMAND_MAX + 1, "{out:?}");
        // No dangling space before the ellipsis.
        assert!(!out.contains(" …"), "{out:?}");
    }
    use super::{LoopbackPlan, PortRoute, WheelRoute, cwd_is_on_host, loopback_plan};
    use super::{encode_mouse, fallback_chain, wheel_route};
    use super::{files_cwd, wsl_share_distro, wsl_share_path};
    use gpui_kit::Modifiers;
    use rio_vt::crosswords::Mode;
    use std::path::PathBuf;

    use crate::core::session::{RemoteTarget, WorkspaceId};
    use crate::daemon::protocol::RemoteKind;
    use crate::terminal::PaneWorkspace;

    fn ws(target: RemoteTarget, with_spec: bool) -> PaneWorkspace {
        PaneWorkspace {
            workspace: WorkspaceId::new(),
            target,
            spec: with_spec.then(|| {
                Box::new(
                    serde_json::from_str(
                        r#"{"host":"dev.box","port":22,"user":"me","auth_mode":"auto"}"#,
                    )
                    .unwrap(),
                )
            }),
            label: None,
            resize_echo: false,
        }
    }

    #[test]
    fn local_pane_opens_localhost_directly() {
        assert_eq!(loopback_plan(true, None, None, 1), LoopbackPlan::Direct);
    }

    #[test]
    fn ssh_pane_forwards_on_the_pane() {
        assert_eq!(
            loopback_plan(true, None, Some(RemoteKind::NativeSsh), 7),
            LoopbackPlan::ForwardOnPane(7)
        );
        assert_eq!(
            loopback_plan(true, None, Some(RemoteKind::Wsl), 7),
            LoopbackPlan::Direct
        );
    }

    #[test]
    fn remote_workspace_pane_forwards_on_the_workspace() {
        let w = ws(RemoteTarget::direct("me", "dev.box", 22), true);
        assert_eq!(
            loopback_plan(true, Some(&w), None, 7),
            LoopbackPlan::ForwardOnWorkspace(Box::new(w.clone())),
            "no RemoteContext, but still forwarded"
        );
        assert_eq!(
            loopback_plan(true, Some(&w), Some(RemoteKind::NativeSsh), 7),
            LoopbackPlan::ForwardOnWorkspace(Box::new(w))
        );
    }

    /// What the Ports list asks about every listener it found. The middle
    /// case is the one that matters: a remote pane's port is not unreachable,
    /// it is one forward away.
    #[test]
    fn a_remote_port_is_a_forward_away_rather_than_out_of_reach() {
        use super::port_route_of;
        assert_eq!(
            port_route_of(&LoopbackPlan::ForwardOnPane(7), false),
            PortRoute::Forward
        );
        assert_eq!(
            port_route_of(&LoopbackPlan::NoForwardNeeded, false),
            PortRoute::Direct,
            "WSL serves onto this machine's own loopback"
        );
        assert_eq!(
            port_route_of(&LoopbackPlan::Direct, true),
            PortRoute::Direct,
            "this machine's ports open with no help"
        );
        assert_eq!(
            port_route_of(&LoopbackPlan::Direct, false),
            PortRoute::Blocked,
            "another machine's port with forwarding turned off is not ours to \
             open — opening it here would reach some unrelated local service"
        );
    }

    #[test]
    fn wsl_workspace_needs_no_forward() {
        let w = ws(
            RemoteTarget::Wsl {
                distro: "Ubuntu".into(),
            },
            false,
        );
        assert_eq!(
            loopback_plan(true, Some(&w), None, 7),
            LoopbackPlan::NoForwardNeeded
        );
    }

    #[test]
    fn workspace_without_a_spec_does_not_forward() {
        let w = ws(RemoteTarget::direct("me", "dev.box", 22), false);
        assert_eq!(loopback_plan(true, Some(&w), None, 7), LoopbackPlan::Direct);
    }

    fn wsl_context(distro: &str) -> crate::daemon::protocol::RemoteContext {
        crate::daemon::protocol::RemoteContext {
            kind: RemoteKind::Wsl,
            argv: Vec::new(),
            target: distro.to_string(),
        }
    }

    #[test]
    fn a_wsl_exe_pane_on_this_machine_completes_over_its_own_share() {
        assert_eq!(
            wsl_share_distro(Some(&wsl_context("Ubuntu-24.04")), None, true),
            Some("Ubuntu-24.04".to_string())
        );
    }

    /// The same pane on a remote machine reaches that machine's distro, which
    /// no share here can list.
    #[test]
    fn a_remote_hosts_wsl_exe_pane_leaves_tab_to_the_shell() {
        assert_eq!(
            wsl_share_distro(Some(&wsl_context("Ubuntu-24.04")), None, false),
            None
        );
    }

    /// A WSL workspace's panes are served by the daemon inside the distro, so
    /// their host is never `LOCAL` — and the distro is still on this machine.
    #[test]
    fn a_wsl_workspace_pane_completes_over_the_share_its_target_names() {
        let w = ws(
            RemoteTarget::Wsl {
                distro: "Ubuntu-24.04".into(),
            },
            false,
        );
        assert_eq!(
            wsl_share_distro(None, Some(&w), false),
            Some("Ubuntu-24.04".to_string())
        );
    }

    #[test]
    fn panes_with_no_distro_of_their_own_have_no_share() {
        let ssh = ws(RemoteTarget::direct("me", "dev.box", 22), true);
        assert_eq!(wsl_share_distro(None, Some(&ssh), false), None);
        assert_eq!(wsl_share_distro(None, None, true), None);
    }

    #[test]
    fn a_wsl_cwd_gets_a_windows_spelling_the_completion_engine_can_list() {
        let share = |posix: &str| wsl_share_path("Ubuntu-24.04", posix);

        // A distro-native path goes through the share.
        assert_eq!(
            share("/home/me/repo"),
            Some(PathBuf::from(r"\\wsl$\Ubuntu-24.04\home\me\repo"))
        );
        assert_eq!(share("/"), Some(PathBuf::from(r"\\wsl$\Ubuntu-24.04\")));

        // The automount stays on the share too: a drive-spelled cwd would
        // send an absolute word (`ls /etc<Tab>`) to `C:\etc` instead of the
        // distro's /etc, because a rooted word completes against its cwd's
        // path prefix.
        assert_eq!(
            share("/mnt/c/Users/me"),
            Some(PathBuf::from(r"\\wsl$\Ubuntu-24.04\mnt\c\Users\me"))
        );

        // No absolute POSIX path, no translation — and a distro name that
        // could break out of the share is refused outright.
        assert_eq!(share("relative/path"), None);
        assert_eq!(wsl_share_path("", "/home/me"), None);
        assert_eq!(wsl_share_path(r"evil\distro", "/home/me"), None);
        assert_eq!(wsl_share_path("evil/distro", "/home/me"), None);
    }

    /// #896: the Files panel handed a WSL pane's `/home/me` to the local
    /// `read_dir`, which on Windows is `C:\home\me`. The share is the only
    /// spelling this machine can list — and drops into the tree land there too.
    #[test]
    fn a_wsl_panes_files_root_goes_through_its_distros_share() {
        assert_eq!(
            files_cwd(None, Some("Ubuntu"), Some(PathBuf::from("/home/me/repo"))),
            Some(PathBuf::from(r"\\wsl$\Ubuntu\home\me\repo"))
        );
        assert_eq!(
            files_cwd(None, Some("Ubuntu"), Some(PathBuf::from("/mnt/c/Users/me"))),
            Some(PathBuf::from(r"\\wsl$\Ubuntu\mnt\c\Users\me"))
        );
    }

    /// A cwd the host resolves itself — a local shell, a workspace pane on its
    /// own server, a WSL workspace's distro daemon — is never rewritten.
    #[test]
    fn a_cwd_on_the_panes_own_host_roots_the_tree_as_is() {
        let native = Some(PathBuf::from("/home/me/repo"));
        assert_eq!(files_cwd(native.clone(), None, None), native);
        assert_eq!(
            files_cwd(
                native.clone(),
                Some("Ubuntu"),
                Some(PathBuf::from("/elsewhere"))
            ),
            native
        );
    }

    /// wsl.exe's own cwd on the Windows side is already a local path.
    #[test]
    fn a_wsl_pane_with_a_windows_cwd_keeps_it() {
        assert_eq!(
            files_cwd(None, Some("Ubuntu"), Some(PathBuf::from(r"C:\Users\me"))),
            Some(PathBuf::from(r"C:\Users\me"))
        );
    }

    /// No host spelling and no local distro — a shell ssh'd onward, or a WSL
    /// pane whose distro could not be named — roots nothing rather than a
    /// path the tree could only fail to read.
    #[test]
    fn a_cwd_no_host_can_read_roots_nothing() {
        assert_eq!(files_cwd(None, None, Some(PathBuf::from("/home/me"))), None);
        assert_eq!(
            files_cwd(None, Some(""), Some(PathBuf::from("/home/me"))),
            None
        );
        assert_eq!(files_cwd(None, Some("Ubuntu"), None), None);
    }

    #[test]
    fn the_off_switch_disables_every_route() {
        let w = ws(RemoteTarget::direct("me", "dev.box", 22), true);
        assert_eq!(
            loopback_plan(false, Some(&w), None, 7),
            LoopbackPlan::Direct
        );
        assert_eq!(
            loopback_plan(false, None, Some(RemoteKind::NativeSsh), 7),
            LoopbackPlan::Direct
        );
    }

    #[test]
    fn fallback_chain_pins_bundled_hack_last() {
        let configured = vec!["Menlo".to_string(), "Apple Color Emoji".to_string()];

        let chain = fallback_chain("JetBrains Mono", &configured);
        assert_eq!(chain[..2], ["Menlo", "Apple Color Emoji"]);
        assert_eq!(chain.last().unwrap(), "Hack");

        let chain = fallback_chain("Hack", &configured);
        assert_eq!(chain[..2], ["Menlo", "Apple Color Emoji"]);
        assert!(!chain.iter().any(|f| f == "Hack"));

        let with_hack = vec!["Hack".to_string(), "Menlo".to_string()];
        let chain = fallback_chain("SF Mono", &with_hack);
        assert_eq!(chain[..2], ["Hack", "Menlo"]);

        assert_eq!(
            fallback_chain("Hack Nerd Font", &[]).last().unwrap(),
            "Hack",
            "a Hack-prefixed family name must not suppress the bundled anchor"
        );
    }

    #[test]
    fn apply_fallback_chain_reaches_every_face_a_view_has() {
        // Bold and italic are the ones at risk: they hold a copy taken off the
        // regular face when `alt_font` built them, so a rebuild that wrote only
        // the regular face would strand them on the chain it replaced.
        let mut font = gpui_kit::font("Hack");
        let mut bold = Some(gpui_kit::font("Hack Bold"));
        let mut italic = Some(gpui_kit::font("Hack Italic"));

        super::apply_fallback_chain(vec!["Menlo".to_string()], &mut font, &mut bold, &mut italic);

        for face in [&font, bold.as_ref().unwrap(), italic.as_ref().unwrap()] {
            assert_eq!(face.fallbacks.as_ref().unwrap().fallback_list(), ["Menlo"]);
        }

        // Neither is configured by default, and a view carries `None` for one
        // it was never given.
        let (mut none_bold, mut none_italic) = (None, None);
        super::apply_fallback_chain(
            vec!["Menlo".to_string()],
            &mut font,
            &mut none_bold,
            &mut none_italic,
        );
        assert_eq!(font.fallbacks.unwrap().fallback_list(), ["Menlo"]);
    }

    #[test]
    fn fallback_chain_appends_platform_stock_faces() {
        let stock = crate::core::config::platform_last_resort_fallbacks();
        assert!(!stock.is_empty(), "every platform needs a CJK last resort");

        let legacy = vec![
            "Menlo".to_string(),
            "Hasklug Nerd Font Mono".to_string(),
            "Maple Mono NF CN".to_string(),
            "Apple Color Emoji".to_string(),
        ];
        let chain = fallback_chain("Hack", &legacy);
        for name in stock {
            assert!(
                chain.iter().any(|f| f == name),
                "{name} missing from repaired chain {chain:?}"
            );
        }

        assert_eq!(chain[..legacy.len()], legacy[..]);

        let explicit = vec![stock[0].to_string()];
        let chain = fallback_chain("Hack", &explicit);
        assert_eq!(
            chain.iter().filter(|f| *f == stock[0]).count(),
            1,
            "stock face duplicated in {chain:?}"
        );

        assert!(!fallback_chain(stock[0], &[]).iter().any(|f| f == stock[0]));
    }

    #[test]
    fn wheel_routes_by_negotiated_mode_with_reporting_first() {
        let mouse = Mode::MOUSE_REPORT_CLICK;
        assert_eq!(
            wheel_route(mouse, false, true),
            WheelRoute::Report { base: 64 }
        );
        assert_eq!(
            wheel_route(mouse, false, false),
            WheelRoute::Report { base: 65 }
        );

        let alt = Mode::ALT_SCREEN | Mode::ALTERNATE_SCROLL;
        assert_eq!(
            wheel_route(alt, false, true),
            WheelRoute::Arrows { seq: b"\x1b[A" }
        );
        assert_eq!(
            wheel_route(alt, false, false),
            WheelRoute::Arrows { seq: b"\x1b[B" }
        );
        assert_eq!(
            wheel_route(alt | Mode::APP_CURSOR, false, true),
            WheelRoute::Arrows { seq: b"\x1bOA" }
        );
        assert_eq!(
            wheel_route(alt | Mode::APP_CURSOR, false, false),
            WheelRoute::Arrows { seq: b"\x1bOB" }
        );

        assert_eq!(
            wheel_route(mouse | alt, false, true),
            WheelRoute::Report { base: 64 }
        );

        assert_eq!(
            wheel_route(Mode::empty(), false, true),
            WheelRoute::Scrollback
        );
    }

    #[test]
    fn wheel_ignores_alternate_scroll_outside_the_alt_screen() {
        assert_eq!(
            wheel_route(Mode::ALTERNATE_SCROLL, false, true),
            WheelRoute::Scrollback
        );
    }

    #[test]
    fn shift_wheel_always_scrolls_the_local_scrollback() {
        let everything =
            Mode::MOUSE_MOTION | Mode::ALT_SCREEN | Mode::ALTERNATE_SCROLL | Mode::APP_CURSOR;
        assert_eq!(wheel_route(everything, true, true), WheelRoute::Scrollback);
        assert_eq!(wheel_route(everything, true, false), WheelRoute::Scrollback);
    }

    #[test]
    fn sgr_mouse_reports_one_based_decimal_with_modifier_bits() {
        let plain = Modifiers::default();
        assert_eq!(
            encode_mouse(true, 0, &plain, 4, 8, true).unwrap(),
            b"\x1b[<0;5;9M".to_vec()
        );
        assert_eq!(
            encode_mouse(true, 2, &plain, 4, 8, false).unwrap(),
            b"\x1b[<2;5;9m".to_vec()
        );
        let all = Modifiers {
            shift: true,
            alt: true,
            control: true,
            ..Modifiers::default()
        };
        assert_eq!(
            encode_mouse(true, 0, &all, 0, 0, true).unwrap(),
            b"\x1b[<28;1;1M".to_vec()
        );
        assert_eq!(
            encode_mouse(true, 64, &plain, 10, 3, true).unwrap(),
            b"\x1b[<64;11;4M".to_vec()
        );
        assert_eq!(
            encode_mouse(true, 35, &plain, 1, 1, true).unwrap(),
            b"\x1b[<35;2;2M".to_vec()
        );
    }

    #[test]
    fn sgr_mouse_has_no_coordinate_cap() {
        let plain = Modifiers::default();
        assert_eq!(
            encode_mouse(true, 0, &plain, 500, 300, true).unwrap(),
            b"\x1b[<0;501;301M".to_vec()
        );
    }

    #[test]
    fn x10_mouse_packs_bytes_and_drops_button_on_release() {
        let plain = Modifiers::default();
        assert_eq!(
            encode_mouse(false, 0, &plain, 4, 8, true).unwrap(),
            vec![0x1b, b'[', b'M', 32, 32 + 1 + 4, 32 + 1 + 8]
        );
        assert_eq!(
            encode_mouse(false, 2, &plain, 4, 8, false).unwrap(),
            vec![0x1b, b'[', b'M', 32 + 3, 32 + 1 + 4, 32 + 1 + 8]
        );
        let ctrl = Modifiers {
            control: true,
            ..Modifiers::default()
        };
        assert_eq!(
            encode_mouse(false, 1, &ctrl, 0, 0, true).unwrap(),
            vec![0x1b, b'[', b'M', 32 + 1 + 16, 33, 33]
        );
    }

    #[test]
    fn x10_mouse_drops_out_of_range_coordinates_whole() {
        let plain = Modifiers::default();
        assert!(encode_mouse(false, 0, &plain, 223, 0, true).is_none());
        assert!(encode_mouse(false, 0, &plain, 0, 223, true).is_none());
        let last = encode_mouse(false, 0, &plain, 222, 222, true).unwrap();
        assert_eq!(&last[4..], &[255, 255]);
    }

    #[test]
    fn only_a_matching_host_may_answer_for_a_panes_paths() {
        assert!(cwd_is_on_host(false, true));
        assert!(cwd_is_on_host(true, false));

        assert!(!cwd_is_on_host(true, true));
        assert!(!cwd_is_on_host(false, false));
    }

    #[test]
    fn a_panes_host_is_its_workspaces_machine() {
        use crate::core::session::{RemoteTarget, WorkspaceId};
        use crate::ui::host_ops::HostId;

        let target = RemoteTarget::Alias {
            alias: "build-box".into(),
        };
        let ws = PaneWorkspace {
            workspace: WorkspaceId::new(),
            target: target.clone(),
            spec: None,
            label: None,
            resize_echo: false,
        };

        let remote = ws.target.host_id();
        assert_eq!(remote, target.host_id(), "the workspace's own machine");
        assert!(!remote.is_local(), "a remote workspace is not this machine");
        assert_eq!(
            HostId::from_connection_key("ssh-alias:build-box"),
            remote,
            "the id the connection was opened under, or the registry lookup misses"
        );

        let sibling = PaneWorkspace {
            workspace: WorkspaceId::new(),
            target,
            spec: None,
            label: None,
            resize_echo: false,
        };
        assert_eq!(sibling.target.host_id(), remote);
    }
}

/// A connected pair of [`crate::daemon::transport::Stream`]s, one for each end
/// of a pane's link to its daemon.
///
/// The client half is what a pane really reads and writes; the daemon half is
/// the test's, to speak protocol into.
///
/// This is the one thing a pane harness needs that Unix and Windows spell
/// differently — `socketpair` there, a loopback connect here — and every gpui
/// test in this crate is portable once it goes through this instead of naming
/// `UnixStream` itself.
#[cfg(test)]
pub(crate) fn test_stream_pair() -> (
    crate::daemon::transport::Stream,
    crate::daemon::transport::Stream,
) {
    #[cfg(unix)]
    {
        std::os::unix::net::UnixStream::pair().unwrap()
    }
    #[cfg(windows)]
    {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let client_side = std::net::TcpStream::connect(addr).unwrap();
        let (daemon_side, _) = listener.accept().unwrap();
        (client_side, daemon_side)
    }
}

#[cfg(test)]
pub(crate) fn quiet_test_pane(
    pane_id: u64,
    window: &mut Window,
    cx: &mut gpui_kit::App,
) -> (
    gpui_kit::Entity<TerminalView>,
    crate::daemon::transport::Stream,
) {
    let (client_side, daemon_side) = test_stream_pair();
    let terminal = RemoteTerminal::from_stream(client_side, TermSize::new(80, 24))
        .expect("quiet test terminal");
    let view = cx.new(|cx| TerminalView::with_terminal(terminal, pane_id, window, cx));
    (view, daemon_side)
}

#[cfg(test)]
pub(crate) fn quiet_test_shell_parts(
    pane_id: u64,
    owner: Option<crate::core::session::WorkspaceId>,
) -> (ShellParts, crate::daemon::transport::Stream) {
    let (client_side, daemon_side) = test_stream_pair();
    let terminal = RemoteTerminal::from_stream(client_side, TermSize::new(80, 24))
        .expect("quiet test shell parts");
    (
        ShellParts {
            terminal,
            pane_id,
            shell_spec: None,
            workspace: None,
            restored: false,
            owner,
        },
        daemon_side,
    )
}

/// The same pane, but reattached rather than spawned — what restoring last
/// session's tabs builds, and the only shape in which the daemon replays state
/// the pane already had.
#[cfg(test)]
pub(crate) fn quiet_reattached_test_pane(
    pane_id: u64,
    window: &mut Window,
    cx: &mut gpui_kit::App,
) -> (
    gpui_kit::Entity<TerminalView>,
    crate::daemon::transport::Stream,
) {
    let (client_side, daemon_side) = test_stream_pair();
    let terminal = RemoteTerminal::from_stream_reattached(client_side, TermSize::new(80, 24))
        .expect("quiet reattached test terminal");
    let view = cx.new(|cx| TerminalView::with_terminal(terminal, pane_id, window, cx));
    (view, daemon_side)
}

/// A quiet pane that was dialled by hand, with no saved host behind it.
///
/// Ungated on purpose: the transport this hands back is already
/// platform-neutral, and gating it left every test that wanted an SSH pane
/// silently skipped on Windows.
#[cfg(test)]
pub(crate) fn quiet_test_ssh_pane(
    pane_id: u64,
    window: &mut Window,
    cx: &mut gpui_kit::App,
) -> (
    gpui_kit::Entity<TerminalView>,
    crate::daemon::transport::Stream,
) {
    quiet_test_ssh_pane_of(pane_id, None, window, cx)
}

/// The same, for a pane opened from a saved host — `profile_id` is what tells
/// the two apart everywhere the connection is offered back to the user.
#[cfg(test)]
pub(crate) fn quiet_test_ssh_pane_of(
    pane_id: u64,
    profile_id: Option<uuid::Uuid>,
    window: &mut Window,
    cx: &mut gpui_kit::App,
) -> (
    gpui_kit::Entity<TerminalView>,
    crate::daemon::transport::Stream,
) {
    let mut spec: crate::daemon::protocol::NativeSshSpec =
        serde_json::from_str(r#"{"host":"build-box","port":22,"user":"me","auth_mode":"auto"}"#)
            .expect("a minimal NativeSshSpec decodes");
    spec.profile_id = profile_id.map(|id| id.to_string());
    quiet_test_ssh_pane_with(pane_id, spec, window, cx)
}

/// The same again, over a spec the caller shaped — for everything a live
/// connection carries beyond its address.
#[cfg(test)]
pub(crate) fn quiet_test_ssh_pane_with(
    pane_id: u64,
    spec: crate::daemon::protocol::NativeSshSpec,
    window: &mut Window,
    cx: &mut gpui_kit::App,
) -> (
    gpui_kit::Entity<TerminalView>,
    crate::daemon::transport::Stream,
) {
    let (view, stream) = quiet_test_pane(pane_id, window, cx);
    view.update(cx, |view, _| {
        view.ssh_spec = Some(Box::new(spec));
    });
    (view, stream)
}

#[cfg(test)]
#[path = "agent_activity_tests.rs"]
mod agent_activity_tests;

#[cfg(test)]
mod gpui_tests {
    use super::test_support::*;
    use super::*;
    use crate::core::config::MouseZoomModifier;
    use rio_vt::selection::{Selection, SelectionType};

    mod selection_scroll_consumer_tests {
        include!("selection_scroll_consumer_tests.rs");
    }
    use crate::daemon::protocol::{ClientMsg, DaemonMsg};
    use crate::daemon::transport::Stream;
    use crate::terminal::lifecycle::declare_displayed;
    use crate::terminal::remote::PtySource;
    use gpui_kit::{Bounds, Entity, TestAppContext, point};

    #[gpui_kit::test]
    fn an_agent_that_has_finished_its_turn_is_not_busy(cx: &mut TestAppContext) {
        use crate::core::cli_agent::{AgentSessionState, AgentStatus, CLIAgent};

        crate::core::config::pin_test_config_dir();
        let (window, mut daemon) = harness(cx);
        DaemonMsg::Agent(Some(CLIAgent::OhMyPi))
            .encode(&mut daemon)
            .unwrap();

        let report = |status: AgentStatus, daemon: &mut Stream| {
            DaemonMsg::AgentStatus(Some(AgentSessionState {
                status,
                message: None,
                session_id: Some("sid-abc".into()),
                launch_argv: Some(vec!["omp".into()]),
                rich: true,
                cwd: None,
                activity: 0,
                turns: 0,
                pending_requests: Vec::new(),
                request_resume_status: Default::default(),
                pending_stop: false,
                readout: Default::default(),
            }))
            .encode(daemon)
            .unwrap();
        };
        let settled = |want: Option<PaneBusy>, cx: &mut TestAppContext| {
            for _ in 0..200 {
                if window.update(cx, |view, _, _| view.busy()).unwrap() == want {
                    return true;
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            false
        };

        report(AgentStatus::Working, &mut daemon);
        assert!(
            settled(Some(PaneBusy::Agent("Oh My Pi")), cx),
            "a turn in flight is work closing would cut short"
        );

        // The green Done badge is what sends a reader to close the tab. Asking
        // "Oh My Pi is still working" there is both false and in the way.
        report(AgentStatus::Done, &mut daemon);
        assert!(
            settled(None, cx),
            "a finished turn must not hold the tab open"
        );
    }

    /// An agent that moves into a git worktree does not `chdir` — the process
    /// stays put and only its hook stream says where the work went. Every
    /// panel that answers "where am I?" reads `effective_cwd`, so this is the
    /// one place that has to prefer the agent's answer over the kernel's.
    #[gpui_kit::test]
    fn a_pane_follows_its_agent_into_a_worktree(cx: &mut TestAppContext) {
        use crate::core::cli_agent::{AgentSessionState, AgentStatus};
        use std::io::Write as _;
        use std::path::PathBuf;

        let launched_in = PathBuf::from("/repo");
        let working_in = PathBuf::from("/repo/.omp/worktrees/wt");

        let (window, mut daemon) = harness(cx);
        DaemonMsg::Cwd(launched_in.clone())
            .encode(&mut daemon)
            .unwrap();
        DaemonMsg::AgentStatus(Some(AgentSessionState {
            status: AgentStatus::Working,
            message: None,
            session_id: Some("sid-wt".into()),
            launch_argv: Some(vec!["omp".into()]),
            rich: true,
            cwd: Some(working_in.clone()),
            activity: 0,
            turns: 0,
            pending_requests: Vec::new(),
            request_resume_status: Default::default(),
            pending_stop: false,
            readout: Default::default(),
        }))
        .encode(&mut daemon)
        .unwrap();
        daemon.flush().unwrap();
        for _ in 0..200 {
            let seen = window
                .update(cx, |view, _, _| {
                    view.cwd().is_some() && view.agent_session().is_some()
                })
                .unwrap();
            if seen {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }

        window
            .update(cx, |view, window, cx| {
                assert_eq!(
                    view.cwd(),
                    Some(launched_in.clone()),
                    "the process really is still in the launch directory"
                );
                view.poll_foreground(window, cx);
                assert_eq!(
                    view.effective_cwd(),
                    Some(working_in.clone()),
                    "the file tree, the cwd row and the SCM panel all root here"
                );
                assert_eq!(view.effective_host_cwd(), Some(working_in.clone()));
            })
            .unwrap();

        // Turn over: the agent is gone, and with it any claim about where the
        // work is. Falling back to a stale worktree would be worse than the
        // bug this fixes.
        DaemonMsg::AgentStatus(None).encode(&mut daemon).unwrap();
        daemon.flush().unwrap();
        for _ in 0..200 {
            let gone = window
                .update(cx, |view, _, _| view.agent_session().is_none())
                .unwrap();
            if gone {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        window
            .update(cx, |view, window, cx| {
                view.poll_foreground(window, cx);
                assert_eq!(view.effective_cwd(), Some(launched_in.clone()));
            })
            .unwrap();
    }

    fn graphics_placement(
        key: u64,
        resource: u64,
        row: i64,
        col: u32,
    ) -> lyrel_core::core::kitty_graphics::Placement {
        let mut placement = lyrel_core::core::kitty_graphics::Placement::default();
        placement.key = key;
        placement.resource = resource;
        placement.image_id = 7;
        placement.placement_id = key as u32;
        placement.cols = 4;
        placement.rows = 2;
        placement.x = 1;
        placement.w = 1;
        placement.h = 1;
        placement.anchor_row = row;
        placement.anchor_col = col;
        placement.span_cols = 4;
        placement.span_rows = 2;
        placement.clip_bottom = 2;
        placement
    }

    fn send_graphics(
        stream: &mut Stream,
        update: lyrel_core::core::kitty_graphics::GraphicsUpdate,
    ) {
        assert!(
            !matches!(
                &update,
                lyrel_core::core::kitty_graphics::GraphicsUpdate::Resource { .. }
                    | lyrel_core::core::kitty_graphics::GraphicsUpdate::Place(_)
                    | lyrel_core::core::kitty_graphics::GraphicsUpdate::Evicted { .. }
            ),
            "owner fixture must explicitly reserve before transmitting"
        );
        DaemonMsg::Graphics(update.encode_frame().into())
            .encode(stream)
            .unwrap();
    }

    fn send_admitted_graphics(
        images: &crate::terminal::images::ImageStore,
        stream: &mut Stream,
        update: lyrel_core::core::kitty_graphics::GraphicsUpdate,
    ) {
        let frame = update.encode_frame();
        assert!(
            images.admit_test_resource(&update, frame.len()),
            "fixture resource must fit the production offer budget"
        );
        DaemonMsg::Graphics(frame.into()).encode(stream).unwrap();
    }

    include!("graphics_consumer_tests.rs");
    include!("graphics_resize_consumer_tests.rs");
    include!("graphics_layout_consumer_tests.rs");
    include!("graphics_recovery_consumer_tests.rs");
    include!("native_ssh_consumer_tests.rs");
    include!("lifecycle_consumer_tests.rs");

    #[gpui_kit::test]
    fn graphics_consumer_streamed_png_and_compression_render_after_rejected_transfer(
        cx: &mut TestAppContext,
    ) {
        use lyrel_core::core::kitty_graphics::{
            GraphicsEffect, GraphicsSniffer, GraphicsState, Segment, Sniffed,
        };
        let (window, view, mut daemon) = rooted_harness(cx);
        let images = view.read_with(cx, |view, _| view.terminal.images());
        let mut state = GraphicsState::new(crate::daemon::protocol::WinSize {
            cols: 80,
            rows: 24,
            cell_w: 10,
            cell_h: 20,
        });
        let mut sniffer = GraphicsSniffer::new();
        // Use the production parser and admission path for source bytes. The
        // existing GPUI seam receives real resource frames, never fake pixels.
        let mut deliver = |bytes: &[u8]| {
            for byte in bytes.chunks(1) {
                let segments = match sniffer.sniff(byte) {
                    Sniffed::Plain(output) => vec![Segment::Output(output.to_vec())],
                    Sniffed::Segments(segments) => segments,
                };
                for segment in segments {
                    let effects = match segment {
                        Segment::Output(output) => {
                            let effects = state.output(&output);
                            DaemonMsg::Output(output).encode(&mut daemon).unwrap();
                            effects
                        }
                        Segment::Command { control, image } => state.apply(control, image),
                        Segment::Reply(_) => panic!("q=2 must suppress malformed replies"),
                    };
                    for effect in effects {
                        match effect {
                            GraphicsEffect::Update(update) => {
                                send_admitted_graphics(&images, &mut daemon, update)
                            }
                            GraphicsEffect::Frame(frame) => send_admitted_graphics(
                                &images,
                                &mut daemon,
                                lyrel_core::core::kitty_graphics::GraphicsUpdate::decode_frame_owned(
                                    frame.into_vec(),
                                )
                                .expect("daemon encoded a valid graphics update"),
                            ),
                            GraphicsEffect::Output(output) => {
                                DaemonMsg::Output(output).encode(&mut daemon).unwrap();
                            }
                            GraphicsEffect::Reply(_) => panic!("q=2 must suppress replies"),
                        }
                    }
                }
            }
        };
        let png = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGP4z8DQAAAEgQGALFXOsAAAAABJRU5ErkJggg==";
        let compressed_png = "eJzrDPBz5+WS4mJgYOD19HAJYmBgYARhDjYGBgZ50SOdYAkXx5CKOck/zh+4wMDA0sjYoBN6bgMDAwODp6ufyzqnhCYAzeMQYw==";
        for (id, format, compression, payload) in [
            (810, 100, "", png),
            (811, 100, ",o=z", compressed_png),
            (812, 32, ",o=z", "eJz7z8DQAAAEgAGA"),
        ] {
            deliver(format!("\x1b[3;4H\x1b_Ga=T,i={id},f={format},s=1,v=1,C=1,q=2{compression},m=1;{}\x1b\\\x1b_Gm=0;{}\x1b\\", &payload[..4], &payload[4..]).as_bytes());
        }
        // Invalid retransmissions cannot remove any accepted placement or move
        // the cursor. The final marker observes the real reader's text order.
        deliver(b"\x1b[7;9H\x1b_Ga=T,i=810,f=100,q=2;iVBORw0KGgoAAAANSUhEUgAAAAEAAAAB\x1b\\\x1b_Ga=T,i=811,s=4294967295,v=4294967295,q=2;AAAA\x1b\\safe");
        for _ in 0..200 {
            if view.read_with(cx, |view, _| {
                view.terminal.images().snapshot().len() == 3
                    && view.terminal.term.lock().grid.cursor.pos.col.0 == 12
            }) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        window.update(cx, |_, window, _| window.refresh()).unwrap();
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(400));
        cx.run_until_parked();
        view.read_with(cx, |view, _| {
            let images = view.terminal.images().snapshot();
            assert_eq!(images.len(), 3);
            for image in images {
                assert_eq!(
                    image.data.as_ref().unwrap().as_bytes(0).unwrap(),
                    &[0, 0, 255, 128]
                );
                assert_eq!((image.anchor_row, image.anchor_col), (2, 3));
                assert!(
                    image.painted.load(std::sync::atomic::Ordering::Acquire),
                    "validated PNG/raw pixels reach the terminal paint path"
                );
            }
            let term = view.terminal.term.lock();
            assert_eq!(
                (term.grid.cursor.pos.row.0, term.grid.cursor.pos.col.0),
                (6, 12)
            );
        });
    }

    #[gpui_kit::test]
    fn graphics_consumer_region_slice_preserves_texture_and_scales_with_device_cell_geometry(
        cx: &mut TestAppContext,
    ) {
        use lyrel_core::core::kitty_graphics::{GraphicsUpdate, Image, WireFormat};
        use std::sync::Arc;
        let (window, mut daemon) = harness_on(cx, PtySource::Raw);
        let images = window
            .update(cx, |view, _, _| view.terminal.images())
            .unwrap();
        send_admitted_graphics(
            &images,
            &mut daemon,
            GraphicsUpdate::Resource {
                key: 801,
                image: Arc::new(Image {
                    width: 1,
                    height: 2,
                    format: WireFormat::Rgba,
                    compressed: false,
                    data: vec![255, 0, 0, 255, 0, 0, 255, 80],
                }),
            },
        );
        let mut placement = graphics_placement(802, 801, 2, 3);
        placement.x = 0;
        placement.w = 1;
        placement.h = 2;
        placement.cols = 2;
        placement.rows = 4;
        placement.span_rows = 4;
        placement.clip_top = 1;
        placement.clip_bottom = 3;
        send_admitted_graphics(&images, &mut daemon, GraphicsUpdate::Place(placement));
        for _ in 0..200 {
            if window
                .update(cx, |view, _, _| {
                    view.terminal.images().snapshot().len() == 1
                })
                .unwrap()
            {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        window
            .update(cx, |view, _, _| {
                let images = view.terminal.images().snapshot();
                let image = &images[0];
                assert_eq!(
                    image.data.as_ref().unwrap().as_bytes(0).unwrap(),
                    &[0, 0, 255, 255, 255, 0, 0, 80]
                );
                let (crop, full) =
                    image.paint_bounds(point(px(0.), px(0.)), px(10.), px(20.), 0, 0, 1.);
                assert_eq!(
                    crop,
                    Bounds::new(point(px(30.), px(60.)), gpui_kit::size(px(20.), px(20.)))
                );
                assert_eq!(
                    full,
                    Bounds::new(point(px(30.), px(40.)), gpui_kit::size(px(20.), px(40.)))
                );
                let (crop, full) =
                    image.paint_bounds(point(px(0.), px(0.)), px(15.), px(30.), 0, 0, 1.5);
                assert_eq!(
                    crop,
                    Bounds::new(point(px(45.), px(90.)), gpui_kit::size(px(30.), px(30.)))
                );
                assert_eq!(
                    full,
                    Bounds::new(point(px(45.), px(60.)), gpui_kit::size(px(30.), px(60.)))
                );
            })
            .unwrap();
        send_graphics(
            &mut daemon,
            GraphicsUpdate::Remove {
                placements: vec![802],
                resources: vec![],
            },
        );
        for _ in 0..200 {
            if window
                .update(cx, |view, _, _| {
                    view.terminal.images().snapshot().is_empty()
                })
                .unwrap()
            {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        window
            .update(cx, |view, window, _| {
                window.refresh();
                assert!(
                    view.terminal.images().snapshot().is_empty(),
                    "permanently clipped/deleted slices cannot return on paint"
                );
            })
            .unwrap();
    }

    #[gpui_kit::test]
    fn graphics_consumer_uses_session_history_origin_and_screen_not_local_cursor(
        cx: &mut TestAppContext,
    ) {
        use lyrel_core::core::kitty_graphics::{GraphicsUpdate, Image, WireFormat};
        use std::sync::Arc;

        let (window, mut daemon) = harness_on(cx, PtySource::Raw);
        let images = window
            .update(cx, |view, _, _| view.terminal.images())
            .unwrap();
        send_admitted_graphics(
            &images,
            &mut daemon,
            GraphicsUpdate::Resource {
                key: 901,
                image: Arc::new(Image {
                    width: 1,
                    height: 1,
                    format: WireFormat::Rgba,
                    compressed: false,
                    data: vec![200, 40, 10, 80],
                }),
            },
        );
        let mut placement = graphics_placement(902, 901, 0, 0);
        placement.anchor_row = 10_005;
        placement.anchor_col = 3;
        placement.x = 0;
        send_admitted_graphics(&images, &mut daemon, GraphicsUpdate::Place(placement));
        send_graphics(
            &mut daemon,
            GraphicsUpdate::Screen {
                screen: 0,
                history_origin: 10_003,
                epoch: 0,
            },
        );
        // A decoded resource can exist before its following Screen frame.
        // The later text marker fences the production reader's ordered stream.
        DaemonMsg::Output(b"\x1b[20;60HMAIN-READY\x1b[20;60H".to_vec())
            .encode(&mut daemon)
            .unwrap();
        for _ in 0..200 {
            if window
                .update(cx, |view, _, _| {
                    view.scrollback_text().contains("MAIN-READY")
                        && view.terminal.images().snapshot().len() == 1
                })
                .unwrap()
            {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        window
            .update(cx, |view, _, _| {
                assert!(view.scrollback_text().contains("MAIN-READY"));
                let images = view.terminal.images().snapshot();
                assert_eq!(images.len(), 1);
                let (bounds, _) =
                    images[0].paint_bounds(point(px(0.), px(0.)), px(10.), px(20.), 0, 0, 1.);
                assert_eq!(bounds.origin, point(px(30.), px(40.)));
                let (history, _) =
                    images[0].paint_bounds(point(px(0.), px(0.)), px(10.), px(20.), 0, 3, 1.);
                assert_eq!(history.origin.y, px(100.));
            })
            .unwrap();
        send_graphics(
            &mut daemon,
            GraphicsUpdate::Screen {
                screen: 1,
                history_origin: 0,
                epoch: 1,
            },
        );
        DaemonMsg::Output(b"ALT-READY".to_vec())
            .encode(&mut daemon)
            .unwrap();
        for _ in 0..200 {
            if window
                .update(cx, |view, _, _| {
                    view.scrollback_text().contains("ALT-READY")
                        && view.terminal.images().snapshot().is_empty()
                })
                .unwrap()
            {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        window
            .update(cx, |view, _, _| {
                assert!(view.scrollback_text().contains("ALT-READY"));
                assert!(view.terminal.images().snapshot().is_empty())
            })
            .unwrap();
        send_graphics(
            &mut daemon,
            GraphicsUpdate::Screen {
                screen: 0,
                history_origin: 10_003,
                epoch: 2,
            },
        );
        DaemonMsg::Output(b"\x1b[20;60HMAIN-RESTORED\x1b[20;60H".to_vec())
            .encode(&mut daemon)
            .unwrap();
        for _ in 0..200 {
            if window
                .update(cx, |view, _, _| {
                    view.scrollback_text().contains("MAIN-RESTORED")
                        && view.terminal.images().snapshot().len() == 1
                })
                .unwrap()
            {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        window
            .update(cx, |view, _, _| {
                assert!(view.scrollback_text().contains("MAIN-RESTORED"));
                assert_eq!(view.terminal.images().snapshot().len(), 1)
            })
            .unwrap();
    }

    #[gpui_kit::test]
    fn graphics_consumer_upload_places_shared_cropped_bgra_at_session_coordinates(
        cx: &mut TestAppContext,
    ) {
        use lyrel_core::core::kitty_graphics::{GraphicsUpdate, Image, WireFormat};
        use std::sync::Arc;

        let (window, mut daemon) = harness_on(cx, PtySource::Raw);
        let (other, mut other_daemon) = harness_on(cx, PtySource::Raw);
        let images = window
            .update(cx, |view, _, _| view.terminal.images())
            .unwrap();
        let other_images = other
            .update(cx, |view, _, _| view.terminal.images())
            .unwrap();
        send_admitted_graphics(
            &images,
            &mut daemon,
            GraphicsUpdate::Resource {
                key: 11,
                image: Arc::new(Image {
                    width: 2,
                    height: 1,
                    format: WireFormat::Rgba,
                    compressed: false,
                    data: vec![255, 0, 0, 255, 10, 20, 30, 128],
                }),
            },
        );
        DaemonMsg::Output(b"\x1b[3;6Huploaded".to_vec())
            .encode(&mut daemon)
            .unwrap();
        for _ in 0..200 {
            if window
                .update(cx, |view, _, _| {
                    view.terminal.term.lock().grid.cursor.pos.col.0 == 13
                })
                .unwrap()
            {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        window
            .update(cx, |view, _, _| {
                assert!(
                    view.terminal.images().snapshot().is_empty(),
                    "upload is not a display command"
                );
            })
            .unwrap();

        send_admitted_graphics(
            &images,
            &mut daemon,
            GraphicsUpdate::Place(graphics_placement(21, 11, 2, 13)),
        );
        DaemonMsg::Output(b"\x1b[5;9H".to_vec())
            .encode(&mut daemon)
            .unwrap();
        send_admitted_graphics(
            &images,
            &mut daemon,
            GraphicsUpdate::Place(graphics_placement(22, 11, 4, 8)),
        );
        DaemonMsg::Output(b"\x1b[9;2H".to_vec())
            .encode(&mut daemon)
            .unwrap();
        for _ in 0..200 {
            let ready = window
                .update(cx, |view, _, _| {
                    view.terminal.images().snapshot().len() == 2
                })
                .unwrap();
            if ready {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        window
            .update(cx, |view, _, _| {
                let images = view.terminal.images().snapshot();
                assert_eq!(images.len(), 2);
                let first = images
                    .iter()
                    .find(|image| image.placement.key == 21)
                    .unwrap();
                let second = images
                    .iter()
                    .find(|image| image.placement.key == 22)
                    .unwrap();
                assert_eq!((first.anchor_row, first.anchor_col), (2, 13));
                assert_eq!((second.anchor_row, second.anchor_col), (4, 8));
                assert_eq!(
                    first.data.as_ref().unwrap().as_bytes(0).unwrap(),
                    &[0, 0, 255, 255, 30, 20, 10, 128]
                );
                assert!(Arc::ptr_eq(
                    first.data.as_ref().unwrap(),
                    second.data.as_ref().unwrap()
                ));
                assert!(Arc::ptr_eq(&first.painted, &second.painted));
                let (crop, full) =
                    first.paint_bounds(point(px(0.), px(0.)), px(10.), px(20.), 0, 0, 1.);
                assert_eq!(
                    crop,
                    Bounds::new(point(px(130.), px(40.)), gpui_kit::size(px(40.), px(40.)))
                );
                assert_eq!(
                    full,
                    Bounds::new(point(px(90.), px(40.)), gpui_kit::size(px(80.), px(40.)))
                );
                let mut natural = first.clone();
                natural.placement.x = 0;
                natural.placement.w = 2;
                natural.placement.cols = 0;
                natural.placement.rows = 0;
                let (natural_bounds, _) =
                    natural.paint_bounds(point(px(0.), px(0.)), px(10.), px(20.), 0, 0, 2.);
                assert_eq!(natural_bounds.size, gpui_kit::size(px(1.), px(0.5)));
                natural.placement.cols = 3;
                natural.placement.offset_x = 4;
                let (width_only, _) =
                    natural.paint_bounds(point(px(0.), px(0.)), px(10.), px(20.), 0, 0, 2.);
                assert_eq!(width_only.size, gpui_kit::size(px(28.), px(14.)));
                assert_eq!(width_only.origin.x, px(132.));
                natural.placement.cols = 0;
                natural.placement.rows = 3;
                natural.placement.clip_bottom = 3;
                natural.placement.offset_x = 0;
                natural.placement.offset_y = 4;
                let (height_only, _) =
                    natural.paint_bounds(point(px(0.), px(0.)), px(10.), px(20.), 0, 0, 2.);
                assert_eq!(height_only.size, gpui_kit::size(px(116.), px(58.)));
                natural.placement.cols = 2;
                let (contained, _) =
                    natural.paint_bounds(point(px(0.), px(0.)), px(10.), px(20.), 0, 0, 2.);
                assert_eq!(
                    contained.size,
                    gpui_kit::size(px(20.), px(10.)),
                    "both spans contain rather than stretch the crop"
                );
            })
            .unwrap();
        other
            .update(cx, |view, _, _| {
                assert!(view.terminal.images().snapshot().is_empty())
            })
            .unwrap();
        send_admitted_graphics(
            &other_images,
            &mut other_daemon,
            GraphicsUpdate::Resource {
                key: 11,
                image: Arc::new(Image {
                    width: 2,
                    height: 1,
                    format: WireFormat::Rgb,
                    compressed: false,
                    data: vec![40, 50, 60, 70, 80, 90],
                }),
            },
        );
        send_admitted_graphics(
            &other_images,
            &mut other_daemon,
            GraphicsUpdate::Place(graphics_placement(21, 11, 0, 0)),
        );
        for _ in 0..200 {
            if other
                .update(cx, |view, _, _| {
                    !view.terminal.images().snapshot().is_empty()
                })
                .unwrap()
            {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }

        let mut replacement = graphics_placement(21, 11, 8, 1);
        replacement.x = 0;
        replacement.cols = 2;
        send_admitted_graphics(&images, &mut daemon, GraphicsUpdate::Place(replacement));
        send_graphics(
            &mut daemon,
            GraphicsUpdate::Remove {
                placements: vec![22],
                resources: vec![],
            },
        );
        DaemonMsg::Output(b"done".to_vec())
            .encode(&mut daemon)
            .unwrap();
        for _ in 0..200 {
            let ready = window
                .update(cx, |view, _, _| {
                    let images = view.terminal.images().snapshot();
                    images.len() == 1 && images[0].placement.cols == 2
                })
                .unwrap();
            if ready {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        window
            .update(cx, |view, _, _| {
                let images = view.terminal.images().snapshot();
                assert_eq!(images.len(), 1);
                assert_eq!(
                    (
                        images[0].placement.key,
                        images[0].placement.x,
                        images[0].placement.cols
                    ),
                    (21, 0, 2)
                );
                assert_eq!((images[0].anchor_row, images[0].anchor_col), (8, 1));
                assert_eq!(
                    images[0].data.as_ref().unwrap().as_bytes(0).unwrap(),
                    &[0, 0, 255, 255, 30, 20, 10, 128]
                );
            })
            .unwrap();
        other
            .update(cx, |view, _, _| {
                let images = view.terminal.images().snapshot();
                assert_eq!(images.len(), 1);
                assert_eq!(
                    images[0].data.as_ref().unwrap().as_bytes(0).unwrap(),
                    &[60, 50, 40, 255, 90, 80, 70, 255]
                );
                assert_eq!(
                    images[0].placement.cols, 4,
                    "the other pane's same-key update must not leak here"
                );
            })
            .unwrap();
    }

    #[gpui_kit::test]
    fn graphics_consumer_paint_uses_visible_crop_and_skips_resources_outside_pane(
        cx: &mut TestAppContext,
    ) {
        use lyrel_core::core::kitty_graphics::{GraphicsUpdate, Image, WireFormat};
        use std::sync::{Arc, atomic::Ordering};

        let (window, view, mut daemon) = rooted_harness(cx);
        let images = view.read_with(cx, |view, _| view.terminal.images());
        window.update(cx, |_, window, _| window.refresh()).unwrap();
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(400));
        cx.run_until_parked();
        for key in [61, 62] {
            send_admitted_graphics(
                &images,
                &mut daemon,
                GraphicsUpdate::Resource {
                    key,
                    image: Arc::new(Image {
                        width: 2,
                        height: 1,
                        format: WireFormat::Rgba,
                        compressed: false,
                        data: vec![255, 0, 0, 255, 0, 200, 0, 80],
                    }),
                },
            );
        }
        send_admitted_graphics(
            &images,
            &mut daemon,
            GraphicsUpdate::Place(graphics_placement(71, 61, 0, 0)),
        );
        let mut output = b"\r\n".repeat(200);
        output.extend_from_slice(b"\x1b[H");
        DaemonMsg::Output(output).encode(&mut daemon).unwrap();
        send_graphics(
            &mut daemon,
            GraphicsUpdate::Screen {
                screen: 0,
                history_origin: 200,
                epoch: 0,
            },
        );
        send_admitted_graphics(
            &images,
            &mut daemon,
            GraphicsUpdate::Place(graphics_placement(72, 62, 200, 0)),
        );
        // Hold the UI pump until both decodes finish, so the first resource
        // cannot have been submitted to the atlas before scrolling offscreen.
        for _ in 0..200 {
            if view.read_with(cx, |view, _| view.terminal.images().snapshot().len() == 2) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        window.update(cx, |_, window, _| window.refresh()).unwrap();
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(400));
        cx.run_until_parked();
        view.read_with(cx, |view, _| {
            let images = view.terminal.images().snapshot();
            let outside = images
                .iter()
                .find(|image| image.placement.key == 71)
                .unwrap();
            let visible = images
                .iter()
                .find(|image| image.placement.key == 72)
                .unwrap();
            assert!(
                !outside.painted.load(Ordering::Acquire),
                "fully clipped resources must not allocate atlas tiles"
            );
            assert!(
                visible.painted.load(Ordering::Acquire),
                "real TerminalElement paint must claim the visible resource"
            );
            assert_eq!(
                visible.data.as_ref().unwrap().as_bytes(0).unwrap(),
                &[0, 0, 255, 255, 0, 200, 0, 80]
            );
            assert_eq!(
                (
                    visible.placement.x,
                    visible.placement.w,
                    visible.placement.h
                ),
                (1, 1, 1)
            );
        });
    }

    #[gpui_kit::test]
    fn clearing_the_scrollback_drops_the_selection_anchored_in_it(cx: &mut TestAppContext) {
        let (window, mut daemon) = harness(cx);

        // Overflow the 24-row viewport so there is a scrollback to purge.
        let mut out = Vec::new();
        for i in 0..60 {
            out.extend_from_slice(format!("line {i}\r\n").as_bytes());
        }
        DaemonMsg::Output(out).encode(&mut daemon).unwrap();
        for _ in 0..200 {
            let filled = window
                .update(cx, |view, _, _| {
                    view.terminal.term.lock().grid.history_size() > 0
                })
                .unwrap();
            if filled {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }

        window
            .update(cx, |view, _, cx| {
                let history = view.terminal.term.lock().grid.history_size();
                assert!(history > 0, "the test needs a scrollback to clear");

                // A selection reaching into history must not survive its purge.
                view.terminal.term.lock().selection = Some(Selection::new(
                    SelectionType::Simple,
                    Pos::new(Line(-1), Column(0)),
                    Side::Left,
                ));

                view.clear_scrollback(cx);
            })
            .unwrap();
        daemon
            .set_read_timeout(Some(std::time::Duration::from_secs(2)))
            .unwrap();
        loop {
            if let crate::daemon::protocol::ClientMsg::ClearPresentation { history } =
                crate::daemon::protocol::ClientMsg::read(&mut daemon).unwrap()
            {
                assert!(history);
                break;
            }
        }
        // The real daemon applies this in its output lock and emits this same
        // text sequence before graphics removals. The consumer must wait for it.
        DaemonMsg::Output(b"\x1b[2J\x1b[3J".to_vec())
            .encode(&mut daemon)
            .unwrap();
        for _ in 0..200 {
            if window
                .update(cx, |view, _, _| {
                    view.terminal.term.lock().selection.is_none()
                })
                .unwrap()
            {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        window
            .update(cx, |view, _, _| {
                assert!(
                    view.terminal.term.lock().selection.is_none(),
                    "purged history cannot retain its selection"
                );
                assert_eq!(view.terminal.term.lock().grid.history_size(), 0);
            })
            .unwrap();
    }

    #[gpui_kit::test]
    fn a_theme_change_reaches_only_the_panes_that_asked(cx: &mut TestAppContext) {
        let set_background = |cx: &mut TestAppContext, bg: gpui_kit::Hsla| {
            cx.update(|cx| gpui_kit::component::Theme::global_mut(cx).background = bg)
        };
        let (_asked_window, mut asked) = harness(cx);
        let (_plain_window, mut plain) = harness(cx);
        set_background(cx, gpui_kit::white());

        DaemonMsg::Output(b"\x1b[?2031h\x1b[?996n".to_vec())
            .encode(&mut asked)
            .unwrap();
        assert_eq!(
            pumped_input(cx, &mut asked, 20).as_deref(),
            Some("\x1b[?997;2n"),
            "996 answers the current scheme"
        );

        set_background(cx, gpui_kit::black());
        assert_eq!(
            pumped_input(cx, &mut asked, 20).as_deref(),
            Some("\x1b[?997;1n")
        );
        DaemonMsg::Output(b"\x1b]11;?\x07".to_vec())
            .encode(&mut asked)
            .unwrap();
        assert!(
            pumped_input(cx, &mut asked, 20)
                .is_some_and(|reply| reply.contains("11;rgb:0000/0000/0000")),
            "OSC 11 answers with the background the flip left"
        );

        set_background(cx, gpui_kit::white());
        assert_eq!(
            pumped_input(cx, &mut asked, 20).as_deref(),
            Some("\x1b[?997;2n")
        );
        assert_eq!(
            pumped_input(cx, &mut plain, 2),
            None,
            "a pane that never set 2031 is not written to"
        );

        DaemonMsg::Output(b"\x1b[?2031l".to_vec())
            .encode(&mut asked)
            .unwrap();
        assert_eq!(pumped_input(cx, &mut asked, 1), None);
        set_background(cx, gpui_kit::black());
        assert_eq!(pumped_input(cx, &mut asked, 2), None, "2031 switched off");
    }

    #[gpui_kit::test]
    fn a_reattach_that_replays_2031_reports_the_scheme_once(cx: &mut TestAppContext) {
        let (_window, mut daemon) = harness(cx);
        cx.update(|cx| gpui_kit::component::Theme::global_mut(cx).background = gpui_kit::black());
        // What a replay sends: the modes ahead of the ring, then each ring
        // segment behind its size, then the shell's state. Nothing live
        // follows: an idle pane still hears once.
        let size = crate::daemon::protocol::WinSize {
            cols: 80,
            rows: 24,
            cell_w: 8,
            cell_h: 16,
        };
        let mut replay = Vec::new();
        for frame in [
            DaemonMsg::Snapshot(b"\x1b[?2031h".to_vec()),
            DaemonMsg::Size(size),
            DaemonMsg::Snapshot(b"\x1b[?996n".to_vec()),
            DaemonMsg::Snapshot(b"$ ".to_vec()),
            DaemonMsg::Prompt {
                active: true,
                at_prompt: false,
                last_exit: None,
            },
        ] {
            frame.encode(&mut replay).unwrap();
        }
        std::io::Write::write_all(&mut daemon, &replay).unwrap();
        assert_eq!(
            pumped_input(cx, &mut daemon, 20).as_deref(),
            Some("\x1b[?997;1n"),
            "the theme may have flipped while detached"
        );
        assert_eq!(
            pumped_input(cx, &mut daemon, 2),
            None,
            "one report for the whole replay, and the replayed 996 is not answered"
        );
    }

    /// A program that switched 2031 on and died without `?2031l` leaves a
    /// replay whose last word is still `?2031h`. The shell that owns the pane
    /// now never asked, so neither a reattach nor a theme flip may type a
    /// report into its command line.
    #[gpui_kit::test]
    fn a_dead_programs_2031_never_reaches_the_shell(cx: &mut TestAppContext) {
        let set_background = |cx: &mut TestAppContext, bg: gpui_kit::Hsla| {
            cx.update(|cx| gpui_kit::component::Theme::global_mut(cx).background = bg)
        };
        let dead = b"\x1b]133;C;claude\x07\x1b[?2031hclaude output\r\n";
        let prompt = b"\x1b]133;D;137\x07\x1b]133;A\x07$ \x1b]133;B\x07";
        let restored = crate::daemon::pane::restore_preamble(Some("this shell is new"));

        for tail in [&prompt[..], &restored[..]] {
            let (_window, mut daemon) = harness(cx);
            set_background(cx, gpui_kit::white());
            let mut replay = Vec::new();
            for frame in [
                DaemonMsg::Snapshot(dead.to_vec()),
                DaemonMsg::Snapshot(tail.to_vec()),
            ] {
                frame.encode(&mut replay).unwrap();
            }
            std::io::Write::write_all(&mut daemon, &replay).unwrap();
            assert_eq!(
                pumped_input(cx, &mut daemon, 2),
                None,
                "a reattach reports nothing"
            );
            set_background(cx, gpui_kit::black());
            assert_eq!(pumped_input(cx, &mut daemon, 2), None, "nor does a flip");
        }

        // A ring takes many reads. One that ends after the segment holding
        // `?2031h` and before the one holding the prompt is not the end of
        // the replay.
        let (_window, mut daemon) = harness(cx);
        set_background(cx, gpui_kit::white());
        let size = crate::daemon::protocol::WinSize {
            cols: 80,
            rows: 24,
            cell_w: 8,
            cell_h: 16,
        };
        let mut head = Vec::new();
        DaemonMsg::Size(size).encode(&mut head).unwrap();
        DaemonMsg::Snapshot(dead.to_vec())
            .encode(&mut head)
            .unwrap();
        std::io::Write::write_all(&mut daemon, &head).unwrap();
        assert_eq!(
            pumped_input(cx, &mut daemon, 2),
            None,
            "half a replay reports nothing"
        );
        let mut rest = Vec::new();
        DaemonMsg::Size(size).encode(&mut rest).unwrap();
        DaemonMsg::Snapshot(prompt.to_vec())
            .encode(&mut rest)
            .unwrap();
        DaemonMsg::Prompt {
            active: true,
            at_prompt: true,
            last_exit: Some(137),
        }
        .encode(&mut rest)
        .unwrap();
        std::io::Write::write_all(&mut daemon, &rest).unwrap();
        assert_eq!(pumped_input(cx, &mut daemon, 2), None, "nor does the rest");

        // Live, the same: the program's exit and the next prompt end it.
        let (_window, mut live) = harness(cx);
        set_background(cx, gpui_kit::white());
        DaemonMsg::Output(dead.to_vec()).encode(&mut live).unwrap();
        assert_eq!(pumped_input(cx, &mut live, 1), None);
        set_background(cx, gpui_kit::black());
        assert_eq!(
            pumped_input(cx, &mut live, 20).as_deref(),
            Some("\x1b[?997;1n")
        );
        DaemonMsg::Output(prompt.to_vec())
            .encode(&mut live)
            .unwrap();
        assert_eq!(pumped_input(cx, &mut live, 1), None);
        set_background(cx, gpui_kit::white());
        assert_eq!(
            pumped_input(cx, &mut live, 2),
            None,
            "a shell at its prompt hears nothing"
        );
        DaemonMsg::Output(b"\x1b[?996n".to_vec())
            .encode(&mut live)
            .unwrap();
        assert_eq!(
            pumped_input(cx, &mut live, 20).as_deref(),
            Some("\x1b[?997;2n"),
            "a live 996 is still answered"
        );
    }

    /// Shaped after what macOS actually delivers, measured on a wheel mouse and
    /// a trackpad: both arrive as pixels, and only the trackpad ever reports a
    /// phase. One wheel detent is ~103px, roughly five lines at a 21px line
    /// height; a trackpad event is a fraction of that but can reach ~3 lines
    /// when flicked, which is why phase and not size decides.
    fn wheel(view: &TerminalView, lines: f32, phase: gpui_kit::TouchPhase) -> ScrollEvent {
        ScrollEvent {
            delta: gpui_kit::ScrollDelta::Pixels(point(
                px(0.),
                px(lines * view.line_height.as_f32()),
            )),
            touch_phase: phase,
            ..Default::default()
        }
    }

    fn notch(view: &TerminalView, lines: f32) -> ScrollEvent {
        wheel(view, lines, gpui_kit::TouchPhase::Moved)
    }

    #[gpui_kit::test]
    fn a_disconnected_remote_pane_keeps_the_line_instead_of_handing_it_to_nowhere(
        cx: &mut TestAppContext,
    ) {
        crate::core::config::pin_test_config_dir();
        let (window, mut daemon) = harness(cx);
        cx.update(|cx| crate::ui::keymap::init(cx));
        DaemonMsg::Prompt {
            active: true,
            at_prompt: true,
            last_exit: None,
        }
        .encode(&mut daemon)
        .unwrap();
        wait_for_input_active(&window, cx);

        window
            .update(cx, |view, window, cx| {
                window.activate_window();
                view.focus_handle.focus(window, cx);
                view.input.editor_mut().set("zzqqx");
                bind_to_a_disconnected_remote_workspace(view, cx);
            })
            .unwrap();

        let mut vcx = gpui_kit::VisualTestContext::from_window(window.into(), cx);
        vcx.simulate_keystrokes("tab");

        window
            .update(cx, |view, _, cx| {
                assert_eq!(
                    view.input.editor().text(),
                    "zzqqx",
                    "a Tab dispatched through SendTab must not empty the line"
                );
                assert!(
                    !view.input.has_handoff(),
                    "nothing was handed off, so the editor keeps the prompt"
                );

                view.submit_command(cx);
                assert_eq!(
                    view.input.editor().text(),
                    "zzqqx",
                    "submit_command guards the link too, even though on_key_down already does"
                );
            })
            .unwrap();
        assert_eq!(
            next_input_until_timeout(&mut daemon),
            None,
            "not one byte reached the wire"
        );
    }

    #[gpui_kit::test]
    fn a_tab_on_a_detached_remote_pane_never_asks_for_a_listing(cx: &mut TestAppContext) {
        use std::io::Write as _;
        crate::core::config::pin_test_config_dir();
        let (window, mut daemon) = harness(cx);
        DaemonMsg::Prompt {
            active: true,
            at_prompt: true,
            last_exit: None,
        }
        .encode(&mut daemon)
        .unwrap();
        DaemonMsg::Cwd(std::path::PathBuf::from("/home/me/proj"))
            .encode(&mut daemon)
            .unwrap();
        daemon.flush().unwrap();
        wait_for_input_active(&window, cx);
        for _ in 0..200 {
            if window
                .update(cx, |view, _, _| view.cwd().is_some())
                .unwrap()
            {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }

        window
            .update(cx, |view, _, cx| {
                view.input.editor_mut().set("ls /home/me/");
                bind_to_a_disconnected_remote_workspace(view, cx);
                assert!(
                    view.remote_ssh_cwd().is_some(),
                    "the pane has to look remote enough to want a listing at all"
                );

                view.tab_pressed(true, cx);
                assert!(
                    view.completion.notice_text().is_none(),
                    "a Tab must not send an SFTP listing down a link that is not attached"
                );
                assert_eq!(
                    view.input.editor().text(),
                    "ls /home/me/",
                    "and the line stays where it was"
                );
            })
            .unwrap();
    }

    #[gpui_kit::test]
    fn a_disconnected_remote_pane_swallows_every_kind_of_typing(cx: &mut TestAppContext) {
        let (window, mut daemon) = harness(cx);
        window
            .update(cx, |view, window, cx| {
                bind_to_a_disconnected_remote_workspace(view, cx);

                type_char(view, "x", window, cx);
                view.commit_text("y", cx);
                view.paste("pasted".into(), cx);
                view.send_to_pty(b"raw", cx);
                view.dump_hold(0, cx);
            })
            .unwrap();
        assert_eq!(
            next_input_until_timeout(&mut daemon),
            None,
            "a read-only window must not put one byte of typing on the wire"
        );
    }

    #[gpui_kit::test]
    fn a_disconnected_remote_pane_still_selects_and_copies(cx: &mut TestAppContext) {
        let (window, mut daemon) = harness(cx);
        DaemonMsg::Output(b"secrets".to_vec())
            .encode(&mut daemon)
            .unwrap();
        for _ in 0..400 {
            cx.run_until_parked();
            let ready = window
                .update(cx, |view, _, _| {
                    let term = view.terminal.term.clone();
                    let term = term.lock();
                    term.grid[Line(0)][Column(0)].c() == 's'
                })
                .unwrap();
            if ready {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }

        let chord = |key: &str| KeyDownEvent {
            keystroke: gpui_kit::Keystroke {
                modifiers: Modifiers {
                    platform: true,
                    ..Modifiers::default()
                },
                key: key.into(),
                key_char: None,
            },
            is_held: false,
            prefer_character_input: false,
        };
        window
            .update(cx, |view, window, cx| {
                bind_to_a_disconnected_remote_workspace(view, cx);
                view.terminal.exited = true;
                view.on_key_down(&chord("a"), window, cx);
                assert!(
                    view.terminal.term.lock().selection.is_some(),
                    "⌘A must still select on a read-only window"
                );
                view.on_key_down(&chord("c"), window, cx);
            })
            .unwrap();
        let copied = cx.update(|cx| cx.read_from_clipboard().and_then(|item| item.text()));
        assert!(
            copied.is_some_and(|t| t.contains("secrets")),
            "⌘C must still copy on a read-only window"
        );
    }

    #[gpui_kit::test]
    fn a_disconnected_remote_pane_still_answers_terminal_queries(cx: &mut TestAppContext) {
        let (window, mut daemon) = harness(cx);
        window
            .update(cx, |view, _, cx| {
                bind_to_a_disconnected_remote_workspace(view, cx);
                view.handle_event(PaneEvent::PtyWrite("\x1b[?62;c".into()), cx);
            })
            .unwrap();
        assert_eq!(
            next_input(&mut daemon),
            b"\x1b[?62;c".to_vec(),
            "a query reply is the emulator's answer, not the user's typing"
        );
    }

    /// What the link supervisor's relink sweep keys off: a workspace pane
    /// whose link died asks to come back, until its machine refuses.
    #[gpui_kit::test]
    fn only_a_dead_workspace_pane_wants_a_relink(cx: &mut TestAppContext) {
        let (window, _daemon) = harness(cx);
        window
            .update(cx, |view, _, cx| {
                bind_to_a_disconnected_remote_workspace(view, cx);
                assert!(!view.wants_relink(), "a live pane asks for nothing");
                view.handle_event(PaneEvent::Exit, cx);
                assert!(view.wants_relink());
                view.abandon_relink();
                assert!(!view.wants_relink(), "a refusal is final");
            })
            .unwrap();
    }

    /// The daemon keeps one subscriber per pane, so a second `Attach` for a
    /// pane already being dialled for kicks the first off. A pane claimed for
    /// an attempt therefore asks for nothing until that attempt reports back —
    /// otherwise the pump, which sweeps every 250 ms, would join a dial that
    /// can sit fifteen seconds waiting for the far end's verdict.
    #[gpui_kit::test]
    fn a_pane_already_being_dialled_for_asks_for_nothing(cx: &mut TestAppContext) {
        let (window, _daemon) = harness(cx);
        window
            .update(cx, |view, _, cx| {
                bind_to_a_disconnected_remote_workspace(view, cx);
                view.handle_event(PaneEvent::Exit, cx);
                assert!(view.wants_relink());
                view.mark_relinking();
                assert!(!view.wants_relink(), "the attempt on the wire owns it");
                view.relink_settled();
                assert!(
                    view.wants_relink(),
                    "an attempt that came back wrong frees it"
                );
            })
            .unwrap();
    }

    #[gpui_kit::test]
    fn an_exited_local_pane_still_swallows_every_key(cx: &mut TestAppContext) {
        let (window, _daemon) = harness(cx);
        window
            .update(cx, |view, window, cx| {
                view.terminal.exited = true;
                let cmd_a = KeyDownEvent {
                    keystroke: gpui_kit::Keystroke {
                        modifiers: Modifiers {
                            platform: true,
                            ..Modifiers::default()
                        },
                        key: "a".into(),
                        key_char: None,
                    },
                    is_held: false,
                    prefer_character_input: false,
                };
                view.on_key_down(&cmd_a, window, cx);
                assert!(
                    view.terminal.term.lock().selection.is_none(),
                    "an exited local pane is finished; its keyboard is unchanged"
                );
            })
            .unwrap();
    }

    #[gpui_kit::test]
    fn a_local_pane_types_exactly_as_it_always_did(cx: &mut TestAppContext) {
        let (window, mut daemon) = harness(cx);
        window
            .update(cx, |view, window, cx| {
                bind_to_a_disconnected_remote_workspace(view, cx);
                view.set_workspace(None);
                assert!(view.accepts_input(cx));
                type_char(view, "z", window, cx);
            })
            .unwrap();
        assert_eq!(next_input(&mut daemon), b"z".to_vec());
    }

    #[gpui_kit::test]
    fn a_relink_moves_the_pane_onto_the_new_socket_and_resets_the_mirror(cx: &mut TestAppContext) {
        let (window, mut old_daemon) = harness(cx);
        let read_row = |cx: &mut TestAppContext, len: usize| -> String {
            window
                .update(cx, |view, _, _| {
                    let term = view.terminal.term.clone();
                    let term = term.lock();
                    let grid = &term.grid;
                    (0..len).map(|c| grid[Line(0)][Column(c)].c()).collect()
                })
                .unwrap()
        };

        DaemonMsg::Output(b"before".to_vec())
            .encode(&mut old_daemon)
            .unwrap();
        let mut seen = String::new();
        for _ in 0..400 {
            cx.run_until_parked();
            seen = read_row(cx, 6);
            if seen == "before" {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(seen, "before", "the pre-drop screen is what we relink over");

        let (new_client, mut new_daemon) = super::test_stream_pair();
        window
            .update(cx, |view, _, cx| {
                view.adopt_relink(
                    new_client,
                    Vec::new(),
                    &crate::terminal::PaneRoute::Local,
                    TermSize::new(100, 30),
                    8,
                    17,
                    cx,
                )
                .expect("the swap itself cannot fail");
                assert_eq!(
                    view.title, "lyrel",
                    "a relinked pane is not \"process exited\""
                );
            })
            .unwrap();
        assert_ne!(
            read_row(cx, 6),
            "before",
            "the mirror must be reset before the daemon replays onto it"
        );

        let resize = loop {
            match ClientMsg::read(&mut new_daemon).expect("the new socket is live") {
                ClientMsg::Resize(win) => break win,
                _ => continue,
            }
        };
        assert_eq!((resize.cols, resize.rows), (100, 30));

        window
            .update(cx, |view, _, cx| view.send_to_pty(b"after", cx))
            .unwrap();
        assert_eq!(next_input(&mut new_daemon), b"after".to_vec());

        let mut leftovers: Vec<Vec<u8>> = Vec::new();
        loop {
            match ClientMsg::read(&mut old_daemon) {
                Ok(ClientMsg::Input(bytes)) => leftovers.push(bytes),
                Ok(_) => continue,
                Err(_) => break,
            }
        }
        assert!(
            leftovers.is_empty(),
            "the retired socket must never see another byte: {leftovers:?}"
        );
    }

    #[gpui_kit::test]
    fn buffer_search_honors_case_and_regex_toggles(cx: &mut TestAppContext) {
        let (window, mut daemon) = harness(cx);

        DaemonMsg::Output(b"Hello World\r\nhello world\r\nWORLD wide\r\n".to_vec())
            .encode(&mut daemon)
            .unwrap();

        for _ in 0..200 {
            let ready = window
                .update(cx, |v, _, _| {
                    let term = v.terminal.term.lock();
                    let grid = &term.grid;
                    (0..grid.screen_lines() as i32)
                        .any(|l| (0..grid.columns()).any(|c| grid[Line(l)][Column(c)].c() == 'W'))
                })
                .unwrap();
            if ready {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }

        window
            .update(cx, |view, window, cx| {
                fn set_query(
                    view: &mut TerminalView,
                    q: &str,
                    window: &mut Window,
                    cx: &mut Context<TerminalView>,
                ) {
                    let input = view.search.as_ref().unwrap().input.clone();
                    input.update(cx, |s, cx| s.set_value(q, window, cx));
                    view.recompute_matches(cx);
                }

                view.open_search(window, cx);
                assert!(view.search.is_some(), "Cmd+F opens the bar");

                set_query(view, "world", window, cx);
                assert_eq!(view.search.as_ref().unwrap().matches.len(), 3);
                assert!(!view.search_regex_error);

                view.search_case_sensitive = true;
                view.recompute_matches(cx);
                assert_eq!(view.search.as_ref().unwrap().matches.len(), 1);
                view.search_case_sensitive = false;

                set_query(view, "wor.d", window, cx);
                assert_eq!(view.search.as_ref().unwrap().matches.len(), 0);
                view.search_regex = true;
                view.recompute_matches(cx);
                assert_eq!(view.search.as_ref().unwrap().matches.len(), 3);

                view.search_regex = true;
                set_query(view, "(", window, cx);
                assert!(view.search_regex_error);
                assert_eq!(view.search.as_ref().unwrap().matches.len(), 0);
                view.search_regex = false;
                view.recompute_matches(cx);
                assert!(!view.search_regex_error);

                view.close_search(window, cx);
                assert_eq!(view.search_last_query, "(");
                assert!(view.search.is_none());
            })
            .unwrap();
    }

    /// A match point is an absolute (line, column) against the width it was
    /// scanned at, so a column change reflows the text out from under every
    /// highlight. Output rescans them, but a quiet pane has none coming —
    /// the resize itself has to rescan (#586).
    #[gpui_kit::test]
    fn a_column_resize_rescans_the_open_searchs_highlights(cx: &mut TestAppContext) {
        // Rooted: the open bar's input reaches for `Root` when a frame draws
        // (see `rooted_harness`).
        let (window, view, mut daemon) = rooted_harness(cx);

        // 76 columns of text: one row at 80 wide, wrapped onto two at 40.
        let mut line = vec![b'a'; 70];
        line.extend_from_slice(b"needle\r\n");
        DaemonMsg::Output(line).encode(&mut daemon).unwrap();

        for _ in 0..200 {
            let ready = cx.update(|cx| {
                let v = view.read(cx);
                let term = v.terminal.term.lock();
                let grid = &term.grid;
                (0..grid.screen_lines() as i32)
                    .any(|l| (0..grid.columns()).any(|c| grid[Line(l)][Column(c)].c() == 'n'))
            });
            if ready {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }

        window
            .update(cx, |_, window, cx| {
                view.update(cx, |v, cx| {
                    v.open_search(window, cx);
                    let input = v.search.as_ref().unwrap().input.clone();
                    input.update(cx, |s, cx| s.set_value("needle", window, cx));
                    v.recompute_matches(cx);
                    let before = *v.search.as_ref().unwrap().matches[0].start();
                    assert_eq!(
                        (before.row.0, before.col.0),
                        (0, 70),
                        "one unwrapped row at 80 columns"
                    );

                    // No output, no debounce: the resize alone has to move the
                    // highlight to where the reflow put the text. Where exactly
                    // the wrapped half lands (next row, or the first half pushed
                    // into scrollback) is the grid's business — what matters is
                    // that the highlight sits on the needle, not its old row.
                    v.set_grid_size(40, 24, px(8.), px(17.), 1., cx);
                    let after = *v.search.as_ref().unwrap().matches[0].start();
                    assert_ne!(
                        (after.row.0, after.col.0),
                        (0, 70),
                        "column 70 does not even exist at 40 wide — a stale point"
                    );
                    let term = v.terminal.term.lock();
                    let cell = term.grid[after.row][after.col].c();
                    drop(term);
                    assert_eq!(cell, 'n', "the highlight follows the reflowed text");

                    // A rows-only change reflows nothing; the scan must not move.
                    v.set_grid_size(40, 12, px(8.), px(17.), 1., cx);
                    let still = *v.search.as_ref().unwrap().matches[0].start();
                    assert_eq!((still.row.0, still.col.0), (after.row.0, after.col.0));
                });
            })
            .unwrap();
    }

    /// The composer end to end on a live pane: laid over the agent's input
    /// once the input is on screen, a message leaves as one paste and then its
    /// own Enter, the box steps aside — keyboard and all — while the agent
    /// shows something else where its input goes, comes back when the input
    /// does, and is gone when the agent quits.
    #[gpui_kit::test]
    fn the_composer_stands_in_for_the_agents_input_and_steps_aside_for_its_prompts(
        cx: &mut TestAppContext,
    ) {
        use crate::core::cli_agent::{AgentSessionState, AgentStatus, CLIAgent};

        let (window, view, mut daemon) = rooted_harness(cx);
        let draw = |cx: &mut TestAppContext| {
            window.update(cx, |_, window, _| window.refresh()).unwrap();
            cx.executor()
                .advance_clock(std::time::Duration::from_millis(400));
            cx.run_until_parked();
        };
        let wait_for = |cx: &mut TestAppContext, want: &dyn Fn(&TerminalView) -> bool| {
            for _ in 0..200 {
                if cx.update(|cx| want(view.read(cx))) {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            panic!("the pane never got there");
        };
        let screen_has = |v: &TerminalView, needle: char| {
            let term = v.terminal.term.lock();
            let grid = &term.grid;
            (0..grid.screen_lines() as i32)
                .any(|l| (0..grid.columns()).any(|c| grid[Line(l)][Column(c)].c() == needle))
        };
        // Let the window lay the grid out first: the agent draws for the size
        // it is given, as Pi does after every resize.
        draw(cx);
        let (cols, lines) = cx.update(|cx| {
            let term = view.read(cx).terminal.term.lock();
            (term.columns(), term.screen_lines())
        });
        let rule = "─".repeat(cols);
        // Pi's input, as it draws it: the prompt between two rules
        // at the bottom of the screen, its status row under them.
        let input = format!(
            "\x1b[2J\x1b[H⏺ Ready.\x1b[{};1H{rule}\x1b[{};1H> \x1b[{};1H{rule}\x1b[{};1H  ? for shortcuts",
            lines - 3,
            lines - 2,
            lines - 1,
            lines,
        );

        window
            .update(cx, |_, window, cx| {
                view.update(cx, |v, cx| v.toggle_composer(window, cx));
            })
            .unwrap();
        assert!(
            cx.update(|cx| view.read(cx).composer.is_none()),
            "a shell has nobody to compose for"
        );

        DaemonMsg::Agent(Some(CLIAgent::Pi))
            .encode(&mut daemon)
            .unwrap();
        DaemonMsg::Output(format!("\x1b[?2004h{input}").into_bytes())
            .encode(&mut daemon)
            .unwrap();
        wait_for(cx, &|v| {
            v.agent() == Some(CLIAgent::Pi)
                && screen_has(v, '>')
                && v.terminal
                    .term
                    .lock()
                    .mode()
                    .contains(Mode::BRACKETED_PASTE)
        });
        let rows = cx.update(|cx| view.read(cx).terminal.size().rows);

        window
            .update(cx, |_, window, cx| {
                view.update(cx, |v, cx| v.toggle_composer(window, cx));
            })
            .unwrap();
        draw(cx);
        window
            .update(cx, |_, window, cx| {
                view.update(cx, |v, cx| {
                    assert!(
                        v.composer_shown(),
                        "the input is on screen, so the box is over it"
                    );
                    let input = v.composer.as_ref().unwrap().input.clone();
                    assert!(
                        input.read(cx).focus_handle(cx).is_focused(window),
                        "opening the box puts the caret in it"
                    );
                    input.update(cx, |s, cx| s.set_value("one\ntwo", window, cx));
                });
            })
            .unwrap();
        assert_eq!(
            cx.update(|cx| view.read(cx).terminal.size().rows),
            rows,
            "laid over the input, the box leaves the grid its rows"
        );

        window
            .update(cx, |_, window, cx| {
                view.update(cx, |v, cx| v.submit_composer(window, cx));
            })
            .unwrap();
        draw(cx);
        assert_eq!(next_input(&mut daemon), b"\x1b[200~one\ntwo\x1b[201~");
        assert_eq!(
            next_input(&mut daemon),
            b"\r",
            "Enter goes as its own write"
        );
        assert_eq!(
            cx.update(|cx| view
                .read(cx)
                .composer
                .as_ref()
                .unwrap()
                .input
                .read(cx)
                .value()),
            "",
            "a sent message leaves the box"
        );

        // A click on the grid takes the keyboard out of the box, and it
        // stays out: the box does not pull it back on the next frame.
        window
            .update(cx, |_, window, cx| {
                view.update(cx, |v, cx| window.focus(&v.focus_handle, cx));
            })
            .unwrap();
        draw(cx);
        draw(cx);
        window
            .update(cx, |_, window, cx| {
                view.update(cx, |v, _| {
                    assert!(
                        v.focus_handle.is_focused(window),
                        "the keyboard stays where the click put it"
                    );
                    assert!(!v.composer_focused);
                });
            })
            .unwrap();

        // A paste at the grid is for the input under the box: it goes into
        // the box, and the keyboard with it — never unseen into the agent.
        view.update(cx, |v, cx| v.paste("pasted\nhere".into(), cx));
        draw(cx);
        window
            .update(cx, |_, window, cx| {
                view.update(cx, |v, cx| {
                    let input = v.composer.as_ref().unwrap().input.clone();
                    assert_eq!(input.read(cx).value(), "pasted\nhere");
                    assert!(input.read(cx).focus_handle(cx).is_focused(window));
                    input.update(cx, |s, cx| s.set_value("", window, cx));
                });
            })
            .unwrap();

        // A picker takes the input's place. Past the grace period the box
        // steps aside and the keyboard goes to the TUI.
        DaemonMsg::Output(
            b"\x1b[2J\x1b[H Select model\r\n \xe2\x9d\xaf 1. Opus\r\n   2. Sonnet".to_vec(),
        )
        .encode(&mut daemon)
        .unwrap();
        wait_for(cx, &|v| !screen_has(v, '─'));
        draw(cx);
        std::thread::sleep(super::super::composer::STEP_ASIDE_AFTER);
        draw(cx);
        window
            .update(cx, |_, window, cx| {
                view.update(cx, |v, _| {
                    assert!(!v.composer_shown(), "the picker is not covered");
                    assert!(
                        v.focus_handle.is_focused(window),
                        "the picker has the keyboard"
                    );
                });
            })
            .unwrap();

        // The input comes back, and so do the box and its caret.
        DaemonMsg::Output(input.clone().into_bytes())
            .encode(&mut daemon)
            .unwrap();
        wait_for(cx, &|v| screen_has(v, '─'));
        draw(cx);
        window
            .update(cx, |_, window, cx| {
                view.update(cx, |v, cx| {
                    assert!(v.composer_shown());
                    let input = v.composer.as_ref().unwrap().input.clone();
                    assert!(input.read(cx).focus_handle(cx).is_focused(window));
                });
            })
            .unwrap();

        // A question the hooks report steps it aside at once, and nothing in
        // the box is sent as the answer.
        DaemonMsg::AgentStatus(Some(AgentSessionState {
            status: AgentStatus::Waiting,
            rich: true,
            ..Default::default()
        }))
        .encode(&mut daemon)
        .unwrap();
        wait_for(cx, &|v| {
            v.agent_session()
                .is_some_and(|s| s.status == AgentStatus::Waiting)
        });
        window
            .update(cx, |_, window, cx| {
                view.update(cx, |v, cx| {
                    let input = v.composer.as_ref().unwrap().input.clone();
                    input.update(cx, |s, cx| s.set_value("yes", window, cx));
                    v.submit_composer(window, cx);
                });
            })
            .unwrap();
        draw(cx);
        assert_eq!(
            next_input_until_timeout(&mut daemon),
            None,
            "a question in the TUI is not answered from the box"
        );
        assert!(cx.update(|cx| !view.read(cx).composer_shown()));
        assert_eq!(
            cx.update(|cx| view
                .read(cx)
                .composer
                .as_ref()
                .unwrap()
                .input
                .read(cx)
                .value()),
            "yes",
            "the held message stays in the box"
        );

        DaemonMsg::Agent(None).encode(&mut daemon).unwrap();
        wait_for(cx, &|v| v.agent().is_none());
        draw(cx);
        window
            .update(cx, |_, window, cx| {
                view.update(cx, |v, _| {
                    assert!(!v.composer_shown(), "no agent, no box");
                    assert!(
                        v.focus_handle.is_focused(window),
                        "the keyboard goes back to the terminal"
                    );
                });
            })
            .unwrap();
    }

    /// The selection that seeds the search query is the thing being searched
    /// for — opening the bar must not erase it, and closing the bar must not
    /// either (#584). Only *changing* the query retires it.
    #[gpui_kit::test]
    fn the_search_bar_opens_and_closes_around_a_grid_selection(cx: &mut TestAppContext) {
        let (window, view, mut daemon) = rooted_harness(cx);

        DaemonMsg::Output(b"some needle in the haystack\r\n".to_vec())
            .encode(&mut daemon)
            .unwrap();
        for _ in 0..200 {
            let ready = cx.update(|cx| {
                let v = view.read(cx);
                let term = v.terminal.term.lock();
                let grid = &term.grid;
                (0..grid.screen_lines() as i32)
                    .any(|l| (0..grid.columns()).any(|c| grid[Line(l)][Column(c)].c() == 'n'))
            });
            if ready {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }

        window
            .update(cx, |_, window, cx| {
                view.update(cx, |v, cx| {
                    // Select "needle" (row 0, columns 5..=10 — the end side
                    // includes its cell) — the seed the bar picks up.
                    let mut sel = Selection::new(
                        SelectionType::Simple,
                        Pos::new(Line(0), Column(5)),
                        Side::Left,
                    );
                    sel.update(Pos::new(Line(0), Column(10)), Side::Right);
                    v.terminal.term.lock().selection = Some(sel);

                    v.open_search(window, cx);
                    assert_eq!(
                        v.search.as_ref().unwrap().input.read(cx).value(),
                        "needle",
                        "the bar opens on the selection as its query"
                    );
                    assert!(
                        v.terminal.term.lock().selection.is_some(),
                        "opening the bar must not eat the selection that seeded it"
                    );

                    v.close_search(window, cx);
                    assert!(
                        v.terminal.term.lock().selection.is_some(),
                        "closing the bar must not eat it either"
                    );

                    // But a query the user *changed* retires the old selection:
                    // it no longer names what the search is about.
                    v.open_search(window, cx);
                    let input = v.search.as_ref().unwrap().input.clone();
                    input.update(cx, |s, cx| s.set_value("haystack", window, cx));
                    v.recompute_matches(cx);
                    assert!(
                        v.terminal.term.lock().selection.is_none(),
                        "a changed query retires the stale selection"
                    );
                });
            })
            .unwrap();
    }

    #[gpui_kit::test]
    fn output_under_an_open_search_bar_is_searched_too(cx: &mut TestAppContext) {
        let (window, view, mut daemon) = rooted_harness(cx);

        fn wait_for(cx: &mut TestAppContext, view: &Entity<TerminalView>, needle: char) {
            for _ in 0..200 {
                let seen = cx.update(|cx| {
                    let v = view.read(cx);
                    let term = v.terminal.term.lock();
                    let grid = &term.grid;
                    (0..grid.screen_lines() as i32).any(|l| {
                        (0..grid.columns()).any(|c| grid[Line(l)][Column(c)].c() == needle)
                    })
                });
                if seen {
                    return;
                }
                cx.run_until_parked();
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            panic!("the pane never printed {needle:?}");
        }

        DaemonMsg::Output(b"world 1\r\n".to_vec())
            .encode(&mut daemon)
            .unwrap();
        wait_for(cx, &view, '1');

        window
            .update(cx, |_, window, cx| {
                view.update(cx, |v, cx| {
                    v.open_search(window, cx);
                    let input = v.search.as_ref().unwrap().input.clone();
                    input.update(cx, |s, cx| s.set_value("world", window, cx));
                    v.recompute_matches(cx);
                    assert_eq!(v.search.as_ref().unwrap().matches.len(), 1);
                });
            })
            .unwrap();

        // A second line arrives while the bar is up. Until it is rescanned the
        // count still says 1, and the one highlight it does draw has slid onto
        // whatever line took the old one's place.
        DaemonMsg::Output(b"world 2\r\n".to_vec())
            .encode(&mut daemon)
            .unwrap();
        wait_for(cx, &view, '2');
        for _ in 0..8 {
            cx.executor()
                .advance_clock(super::super::search::SCAN_DEBOUNCE * 2);
            cx.run_until_parked();
        }

        cx.update(|cx| {
            let v = view.read(cx);
            let search = v.search.as_ref().expect("the bar is still open");
            assert_eq!(
                search.matches.len(),
                2,
                "the line printed under the open bar has to be counted too"
            );
            assert_eq!(
                search.current_index,
                Some(0),
                "and the match the user was standing on is still the one they are on"
            );
            assert!(
                !v.search_scan_armed,
                "the debounce has to settle, not rescan forever"
            );
        });
    }

    /// Print lines into the grid and post the wakeup the reader thread would
    /// have posted behind them.
    ///
    /// Going through the parser directly rather than the socket keeps the two
    /// tests below off both real time and the clock: the pane has printed by
    /// the time this returns, and no timer has had a chance to fire.
    fn print(cx: &mut TestAppContext, view: &Entity<TerminalView>, lines: Vec<String>) {
        cx.update(|cx| {
            view.update(cx, |v, cx| {
                {
                    use rio_vt::performer::handler::Handler as _;
                    let mut term = v.terminal.term.lock();
                    for line in lines {
                        for c in line.chars() {
                            term.input(c);
                        }
                        term.carriage_return();
                        term.newline();
                    }
                }
                v.handle_event(PaneEvent::Wakeup, cx);
            })
        });
    }

    /// Filler that scrolls the grid without ever matching the query.
    fn filler(lines: usize) -> Vec<String> {
        (0..lines).map(|i| format!("filler {i}")).collect()
    }

    /// What the grid holds where the current match says its word starts, so a
    /// test can ask the question the eye asks: is the highlight still on it?
    fn word_at_the_match(
        cx: &mut TestAppContext,
        view: &Entity<TerminalView>,
        len: usize,
    ) -> String {
        cx.update(|cx| {
            let v = view.read(cx);
            let start = *v
                .search
                .as_ref()
                .and_then(|s| s.current())
                .expect("a match to stand on")
                .start();
            let term = v.terminal.term.lock();
            let grid = &term.grid;
            (0..len)
                .map(|i| grid[start.row][Column(start.col.0 + i)].c())
                .collect()
        })
    }

    /// Print the searched-for word and bury it in the scrollback, then open the
    /// bar on it. Returns with exactly one match, sitting in history.
    fn search_a_buried_word(
        window: &gpui_kit::WindowHandle<gpui_kit::component::Root>,
        cx: &mut TestAppContext,
        view: &Entity<TerminalView>,
    ) {
        let rows = cx.update(|cx| view.read(cx).terminal.term.lock().grid.screen_lines());
        print(cx, view, vec!["world 1".to_string()]);
        print(cx, view, filler(rows + 8));
        cx.update(|cx| {
            assert!(
                view.read(cx).terminal.term.lock().grid.history_size() > 0,
                "the word has to be in the scrollback for the grid to scroll under it"
            );
        });

        window
            .update(cx, |_, window, cx| {
                view.update(cx, |v, cx| {
                    v.open_search(window, cx);
                    let input = v.search.as_ref().unwrap().input.clone();
                    input.update(cx, |s, cx| s.set_value("world", window, cx));
                    v.recompute_matches(cx);
                    assert_eq!(v.search.as_ref().unwrap().matches.len(), 1);
                });
            })
            .unwrap();
    }

    /// Drawing must never queue for the grid lock.
    ///
    /// One UI thread paints every pane in every window, and the thread holding
    /// this lock is the pane's own reader part-way through feeding a batch of
    /// output into the emulator. A draw that waited for it would wire one
    /// pane's write speed to the frame rate of the whole window — the read-side
    /// twin of #709, which was this same thread parked in `write(2)`.
    ///
    /// No second thread and no timing: `try_lock` fails against a lock this
    /// thread already holds, so "the reader has it" is reproduced exactly, with
    /// nothing to race. The cost of that trade is what a regression looks like
    /// — put `lock()` back and this test hangs on the re-entry rather than
    /// failing, which reads as a CI timeout on exactly this name.
    #[gpui_kit::test]
    fn a_frame_that_cannot_have_the_grid_leaves_the_previous_one_alone(cx: &mut TestAppContext) {
        use super::super::element::PaintColors;

        let (_window, view, _daemon) = rooted_harness(cx);
        let element = TerminalElement::new(view.clone());
        let mut buf = Vec::new();
        let build = |cx: &mut TestAppContext, buf: &mut Vec<RenderCell>, must_block: bool| {
            cx.update(|cx| {
                let colors = PaintColors::resolve(cx.theme(), cx);
                element.build_grid(
                    &colors,
                    buf,
                    24,
                    80,
                    false,
                    cx,
                    1.,
                    gpui_kit::Rgba::default(),
                    must_block,
                )
            })
        };

        assert!(
            build(cx, &mut buf, true).is_some(),
            "the first frame has no previous grid to stand in for it, so it waits and builds"
        );
        assert_eq!(buf.len(), 24 * 80);

        // Shortened so the next call cannot touch the buffer without saying so:
        // building would resize it back to a full grid.
        buf.truncate(3);
        let term = cx.update(|cx| view.read(cx).terminal.term.clone());
        let held = term.lock();
        let refused = build(cx, &mut buf, false);
        drop(held);

        assert!(
            refused.is_none(),
            "a frame that cannot have the lock says so instead of waiting for it"
        );
        assert_eq!(
            buf.len(),
            3,
            "the previous frame's cells have to survive for that frame to be painted again"
        );

        assert!(
            build(cx, &mut buf, false).is_some(),
            "with the lock free, a frame builds without being told to wait"
        );
        assert_eq!(buf.len(), 24 * 80);
    }

    #[gpui_kit::test]
    fn a_highlight_follows_its_text_as_output_scrolls_under_it(cx: &mut TestAppContext) {
        let (window, view, _daemon) = rooted_harness(cx);
        search_a_buried_word(&window, cx, &view);
        assert_eq!(
            word_at_the_match(cx, &view, 5),
            "world",
            "the scan has to name its own word"
        );

        // Output arrives and the clock never moves, so nothing rescans: the
        // stored point is all the highlight has to go on. Every line printed
        // here slid the grid up under it, and it has to have come along.
        print(cx, &view, filler(7));
        cx.update(|cx| {
            assert!(
                view.read(cx).search_scan_armed,
                "the rescan is still waiting for the pane to go quiet"
            );
        });
        assert_eq!(
            word_at_the_match(cx, &view, 5),
            "world",
            "a match point has to name the row its text scrolled to, \
             not the row it was read from"
        );
    }

    #[gpui_kit::test]
    fn a_pane_that_never_pauses_is_rescanned_anyway(cx: &mut TestAppContext) {
        let (window, view, _daemon) = rooted_harness(cx);
        search_a_buried_word(&window, cx, &view);

        // A pane mid-flood: something new lands every half debounce window, so
        // the pause the rescan is waiting for never comes.
        for i in 0..(super::super::search::SCAN_LAG_LIMIT * 2 + 2) {
            print(cx, &view, vec![format!("world {}", i + 2)]);
            cx.executor()
                .advance_clock(super::super::search::SCAN_DEBOUNCE / 2);
            cx.run_until_parked();
        }

        cx.update(|cx| {
            let search = view
                .read(cx)
                .search
                .as_ref()
                .expect("the bar is still open");
            assert!(
                search.matches.len() > 1,
                "printing without a pause must not hold the scan off forever, \
                 got {} matches",
                search.matches.len()
            );
        });
    }

    #[gpui_kit::test]
    fn hidden_cursor_at_prompt_anchors_the_editor_at_the_real_cell_not_top_left(
        cx: &mut TestAppContext,
    ) {
        use rio_vt::ansi::CursorShape;

        let (window, mut daemon) = harness(cx);

        DaemonMsg::Prompt {
            active: true,
            at_prompt: true,
            last_exit: Some(0),
        }
        .encode(&mut daemon)
        .unwrap();
        DaemonMsg::Output(b"\x1b[4;11H\x1b[?25l".to_vec())
            .encode(&mut daemon)
            .unwrap();

        let mut state = (false, false, None);
        for _ in 0..400 {
            cx.run_until_parked();
            state = window
                .update(cx, |view, _, _| {
                    let hidden = matches!(
                        view.terminal.term.lock().cursor().content,
                        CursorShape::Hidden
                    );
                    (view.input_active(), hidden, view.cursor_cell())
                })
                .unwrap();
            if state == (true, true, Some((3, 10))) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }

        let (active, hidden, cell) = state;
        assert!(
            active,
            "shell at its prompt must make the inline editor active"
        );
        assert!(
            hidden,
            "the TUI's `?25l` must leave the cursor shape Hidden"
        );
        assert_eq!(
            cell,
            Some((3, 10)),
            "a Hidden shape must not collapse the editor anchor to the top-left corner"
        );
    }

    /// #844: a TUI that resets DECTCEM and draws its own reverse-video caret
    /// gets no terminal caret painted over it — focused, unfocused, and after
    /// a re-attach replays its screen — and `?25h` brings the caret back.
    ///
    /// The stream is the reporter's shape: an alternate screen and 69 `?25l`
    /// interleaved with 75 `?25h`, the last one a hide. What is checked is the
    /// snapshot a real paint left behind, through the same `painted_cursor`
    /// the element paints from.
    #[gpui_kit::test]
    fn dectcem_reset_paints_no_terminal_caret_over_the_tuis_own(cx: &mut TestAppContext) {
        use crate::core::config::CursorStyle;

        // An Ink-style frame: our own caret as one reverse-video cell at
        // row 5 column 13, and the real cursor parked on it.
        const FRAME: &[u8] = b"\x1b[5;1H\x1b[2K> type here \x1b[7m \x1b[27m\x1b[5;13H";
        let mut stream = b"\x1b[?1049h".to_vec();
        stream.extend(std::iter::repeat_n(&b"\x1b[?25h"[..], 7).flatten());
        for _ in 0..68 {
            stream.extend_from_slice(b"\x1b[?25l");
            stream.extend_from_slice(FRAME);
            stream.extend_from_slice(b"\x1b[?25h");
        }
        stream.extend_from_slice(b"\x1b[?25l");
        stream.extend_from_slice(FRAME);
        assert_eq!(stream.windows(6).filter(|w| w == b"\x1b[?25l").count(), 69);
        assert_eq!(stream.windows(6).filter(|w| w == b"\x1b[?25h").count(), 75);

        type Painted = Option<(usize, usize, CursorStyle)>;
        // Paints frames until the one the pane settles on matches `want`, and
        // returns the last painted caret either way.
        let paint_until = |window: &gpui_kit::WindowHandle<TerminalView>,
                           cx: &mut TestAppContext,
                           focused: bool,
                           want: &dyn Fn(Painted) -> bool| {
            let mut painted = None;
            for _ in 0..400 {
                window
                    .update(cx, |view, window, cx| {
                        if focused {
                            window.activate_window();
                            view.focus_handle.focus(window, cx);
                        } else {
                            window.blur(cx);
                        }
                        cx.notify();
                    })
                    .unwrap();
                let mut vcx = gpui_kit::VisualTestContext::from_window((*window).into(), cx);
                vcx.update(|window, _| window.refresh());
                vcx.run_until_parked();
                let (text, snap) = window
                    .update(cx, |view, window, _| {
                        assert_eq!(view.focus_handle.is_focused(window), focused);
                        use rio_vt::crosswords::grid::Dimensions as _;
                        use rio_vt::crosswords::pos::{Column, Line};
                        let term = view.terminal.term.lock();
                        let row = &term.grid[Line(4)];
                        let text = (0..term.grid.columns())
                            .map(|col| row[Column(col)].c())
                            .collect::<String>();
                        (text, view.grid_snap.as_ref().map(|s| s.painted_cursor()))
                    })
                    .unwrap();
                if text.starts_with("> type here")
                    && let Some(p) = snap
                {
                    painted = p;
                    if want(p) {
                        break;
                    }
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            painted
        };

        let (window, mut daemon) = harness(cx);
        DaemonMsg::Output(stream.clone())
            .encode(&mut daemon)
            .unwrap();
        let focused = paint_until(&window, cx, true, &|p| p.is_none());
        assert_eq!(
            focused, None,
            "a focused pane painted its caret over a TUI that reset DECTCEM"
        );
        let unfocused = paint_until(&window, cx, false, &|p| p.is_none());
        assert_eq!(
            unfocused, None,
            "an unfocused pane painted its hollow caret over a TUI that reset DECTCEM"
        );

        DaemonMsg::Output(b"\x1b[?25h".to_vec())
            .encode(&mut daemon)
            .unwrap();
        let shown = paint_until(&window, cx, true, &|p| p.is_some());
        assert_eq!(
            shown.map(|(row, col, _)| (row, col)),
            Some((4, 12)),
            "`?25h` must bring the caret back where the program parked it"
        );

        // Switching back to the tab: a brand new view replays the screen the
        // daemon kept, then the prompt state, which says a program is running.
        let (window, mut daemon) = harness(cx);
        DaemonMsg::Snapshot(stream).encode(&mut daemon).unwrap();
        DaemonMsg::Prompt {
            active: true,
            at_prompt: false,
            last_exit: None,
        }
        .encode(&mut daemon)
        .unwrap();
        let replayed = paint_until(&window, cx, true, &|p| p.is_none());
        assert_eq!(
            replayed, None,
            "a re-attached pane painted its caret over a TUI that reset DECTCEM"
        );
    }

    #[gpui_kit::test]
    fn ssh_drop_mid_tui_recovers_at_the_next_prompt(cx: &mut TestAppContext) {
        use rio_vt::ansi::CursorShape;

        let (window, mut daemon) = harness(cx);

        DaemonMsg::Output(b"\x1b[?1049h\x1b[?25l".to_vec())
            .encode(&mut daemon)
            .unwrap();
        DaemonMsg::Prompt {
            active: true,
            at_prompt: true,
            last_exit: Some(255),
        }
        .encode(&mut daemon)
        .unwrap();

        let mut state = (false, true, true);
        for _ in 0..400 {
            cx.run_until_parked();
            state = window
                .update(cx, |view, _, _| {
                    let hidden = matches!(
                        view.terminal.term.lock().cursor().content,
                        CursorShape::Hidden
                    );
                    (view.at_shell_prompt(), view.on_alt_screen(), hidden)
                })
                .unwrap();
            if state == (true, false, false) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }

        let (at_prompt, on_alt, hidden) = state;
        assert!(at_prompt, "the host shell is back at its prompt");
        assert!(
            !on_alt,
            "the prompt report must pull the grid off the stranded alt screen"
        );
        assert!(
            !hidden,
            "the prompt report must re-show the DECTCEM-hidden cursor"
        );

        window
            .update(cx, |view, _, _| {
                assert!(
                    view.input_active(),
                    "off the alt screen and at the prompt, the editor is live"
                );
            })
            .unwrap();
    }

    #[gpui_kit::test]
    fn a_remote_workspace_pane_reports_its_cwd_as_remote(cx: &mut TestAppContext) {
        use std::io::Write as _;
        let (window, mut daemon) = harness(cx);
        DaemonMsg::Cwd(std::path::PathBuf::from("/home/me/proj"))
            .encode(&mut daemon)
            .unwrap();
        daemon.flush().unwrap();
        for _ in 0..200 {
            let seen = window
                .update(cx, |view, _, _| view.cwd().is_some())
                .unwrap();
            if seen {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }

        window
            .update(cx, |view, _, cx| {
                assert_eq!(
                    view.local_cwd(),
                    Some(std::path::PathBuf::from("/home/me/proj"))
                );
                assert_eq!(view.remote_ssh_cwd(), None);

                bind_to_a_disconnected_remote_workspace(view, cx);

                assert!(
                    view.remote_context().is_none(),
                    "the far daemon reports a plain local pane — if this ever \
                     stops holding, the binding below is no longer the only signal"
                );
                assert_eq!(
                    view.local_cwd(),
                    None,
                    "a routed pane's cwd is not a path on this machine"
                );
                assert_eq!(
                    view.remote_ssh_cwd(),
                    Some("/home/me/proj".to_string()),
                    "Tab must ask the workspace's connection about it"
                );
            })
            .unwrap();
    }

    /// #958: `prompt_cursor_style` shapes the shell's own caret at a prompt,
    /// leaves the running program's cursor alone, and steps aside for a vi-mode
    /// prompt, whose insert/normal shapes are the shell's to set.
    #[gpui_kit::test]
    fn prompt_cursor_style_shapes_only_the_prompt(cx: &mut TestAppContext) {
        use crate::core::config::{CursorStyle, PromptCursorStyle};

        let (window, mut daemon) = harness(cx);
        let shape = |cx: &mut TestAppContext| {
            cx.update(|cx| {
                let view = window.entity(cx).unwrap();
                TerminalElement::new(view).prompt_cursor_shape(cx)
            })
        };
        let wait_for = |cx: &mut TestAppContext, want: &dyn Fn(&TerminalView) -> bool| {
            for _ in 0..200 {
                cx.run_until_parked();
                if window.update(cx, |view, _, _| want(view)).unwrap() {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            panic!("the pane never reached the expected prompt state");
        };
        cx.update(|cx| cx.global_mut::<Config>().prompt_cursor_style = PromptCursorStyle::Bar);

        assert_eq!(
            shape(cx),
            None,
            "no prompt reported yet: a program owns the cursor"
        );

        DaemonMsg::Prompt {
            active: true,
            at_prompt: true,
            last_exit: None,
        }
        .encode(&mut daemon)
        .unwrap();
        wait_for(cx, &|view| view.terminal.at_prompt());
        assert_eq!(shape(cx), Some(CursorStyle::Bar));

        cx.update(|cx| cx.global_mut::<Config>().prompt_cursor_style = PromptCursorStyle::Follow);
        assert_eq!(shape(cx), None, "follow leaves the prompt on cursor_style");
        cx.update(|cx| cx.global_mut::<Config>().prompt_cursor_style = PromptCursorStyle::Bar);

        DaemonMsg::Output(b"\x1b]133;V;1\x07\x1b]133;B\x07".to_vec())
            .encode(&mut daemon)
            .unwrap();
        wait_for(cx, &|view| view.terminal.shell_vi_mode());
        assert_eq!(
            shape(cx),
            None,
            "a vi-mode prompt keeps the shell's own shapes"
        );

        DaemonMsg::Prompt {
            active: true,
            at_prompt: false,
            last_exit: None,
        }
        .encode(&mut daemon)
        .unwrap();
        wait_for(cx, &|view| !view.terminal.at_prompt());
        assert_eq!(shape(cx), None, "a running command gets cursor_style back");
    }
}

/// The window between the shell reporting a prompt and its line editor
/// actually reading, which is where a fast typist's line goes missing (#433).
///
/// These drive a real `TerminalView` over a pane link on every platform, so
/// they are not gated to unix the way `gpui_tests` is.
#[cfg(test)]
#[path = "prompt_input/handover_tests.rs"]
mod prompt_handover_tests;
