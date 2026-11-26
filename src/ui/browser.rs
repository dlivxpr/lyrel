//! The Browser tool's pages (#391): local web pages opened from the `+` menu,
//! Search Everywhere, or a terminal link, and agent-requested previews (#393).
//! The right panel's Browser tool lists the open pages; the selected page
//! paints as a document in the document column, beside the terminal docked or
//! over the workspace filling, under the same header contract the GitHub
//! detail keeps. This module owns the tab model, the live page runtime
//! (native surface lifecycle, exactly the diagram surface's paint-driven
//! visibility pattern), the page chrome (back, forward, reload, address,
//! Open in System Browser), the document shell that hosts the chrome, and
//! the entry points that open a page.
//!
//! Remote loopback forwarding and its reconnect flow live in [`route`] (#392);
//! agent-requested previews and the explicit screenshot-and-feedback share
//! live here as tab state and an opt-in composer (#393).
//!
//! The current detail — `LyrelApp::active_browser_detail` — is the single
//! authority for the native paint anchor, the visibility watchdog and the
//! address submission: switching tools or collapsing the panel never hides
//! a raised page, and only the front page's surface exists on screen.
//!
//! The native surface contract lives in [`surface`]; the navigation policy
//! in [`policy`]. There is no page → workbench bridge — the only things a
//! page can ask for are the events the surface defines, and the only ways in
//! are the commands.

pub(crate) mod html_preview;
pub(crate) mod policy;
pub(crate) mod route;
pub(crate) mod surface;

#[cfg(test)]
#[path = "browser/tests.rs"]
mod tests;

#[cfg(test)]
#[path = "browser/html_preview_tests.rs"]
mod html_preview_tests;

#[cfg(test)]
#[path = "browser/agent_html_preview_tests.rs"]
mod agent_html_preview_tests;

#[cfg(test)]
#[path = "browser/overlay_tests.rs"]
mod overlay_tests;

#[cfg(test)]
#[path = "browser/terminal_html_tests.rs"]
mod terminal_html_tests;

#[cfg(test)]
#[path = "browser/editor_scm_html_tests.rs"]
mod editor_scm_html_tests;

pub(crate) use surface::{BrowserCommand, BrowserEvent, BrowserParentWindow, BrowserSurfaceHost};

use gpui_kit::TestSupportExt as _;
use gpui_kit::base::Disableable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::{
    AnyElement, Bounds, Context, Entity, Focusable as _, Pixels, Subscription, Window, div,
    prelude::*, px, rems,
};
use lyrel_ui_base::i18n::{L10nKey, t, t_fmt};

use crate::ui::app::LyrelApp;
use crate::ui::diff_overlay::{HAIRLINE_W, hairline};
use crate::ui::document_column::DocumentChrome;
use crate::ui::right_panel::{TEXT, WebTab};
use crate::ui::window_chrome::{CONTENT_INSET, TitleStripRow as _};
use lyrel_core::core::session::{PreviewSourceState, WorkspaceId};
use lyrel_terminal::view::{AgentPromptError, TerminalView};
use lyrel_ui_base::host_ops::{HostId, SharedHost};

/// The document header's close tile, the same notch the diff and GitHub
/// headers' close buttons use.
const CLOSE_TILE: f32 = crate::ui::tab_strip::RAIL_TILE;

/// One page's live runtime, keyed by its durable tab id in
/// [`crate::ui::right_panel::RightPanelState::web_pages`]. Everything here is
/// panel state: nothing persists but the tab identity in the tool layout.
pub(crate) struct WebPage {
    surface: Option<BrowserSurfaceHost>,
    events: Option<gpui_kit::Task<()>>,
    pub(crate) status: PageStatus,
    /// The URL the surface is actually on (the forwarded endpoint for a
    /// Host-bound loopback page). The tab's original address outlives it in
    /// the layout.
    pub(crate) current_url: String,
    /// What the address bar shows: the Host's own spelling for a forwarded
    /// page, the surface URL otherwise.
    pub(crate) display_url: String,
    /// The Host-bound route state (#392).
    pub(crate) route: RouteState,
    /// Bumped for every verification so a slow answer from a replaced route
    /// cannot land on the new one.
    route_epoch: u64,
    route_pending: bool,
    pub(crate) live_title: String,
    pub(crate) can_back: bool,
    pub(crate) can_forward: bool,
    bounds: Option<Bounds<Pixels>>,
    /// The window the page painted in. Cover signals are per window, so the
    /// watchdog can evaluate them without borrowing a `Window`.
    window_id: Option<gpui_kit::WindowId>,
    /// The native page held keyboard focus when it was concealed; the surface
    /// takes it back when the last cover lifts.
    restore_focus: bool,
    last_paint: std::time::Instant,
    surface_visible: bool,
    surface_pending: bool,
    surface_attempted: bool,
    /// The cover state the surface was last synced against, so a repaint
    /// only talks to the native child when something changed.
    cover_synced: Option<bool>,
    pending_still_frame: Option<u64>,
    still_frame: Option<std::sync::Arc<gpui_kit::Image>>,
    pub(crate) load_error: Option<String>,
    /// The explicit share of this page back to its agent (#393). Created on
    /// the user's first share action, so a page nobody asked about carries no
    /// input state.
    pub(crate) feedback: Option<PreviewFeedback>,
    /// The live authority for that explicit share (#393): the exact pane
    /// view and Host connection a fresh agent request was bound to. Nothing
    /// here is persisted, so a restored page — or a pane view or Host link
    /// that has since been replaced — has no sendable target.
    preview: Option<PreviewBinding>,
    /// The next capture id this page mints. A stale native answer must not
    /// be mistaken for the share the user just asked for.
    next_capture: u64,
    /// Active Host-backed loopback mount for HTML preview tabs (#420).
    /// Dropping the page releases its mount token and shuts down the
    /// loopback listener when no mounts remain.
    pub(crate) html_mount: Option<html_preview::HtmlPreviewMount>,
}

/// The live pane incarnation one preview may send to (#393).
///
/// The pane number alone is not an identity: a restarted daemon assigns
/// numbers from the beginning again. This binding is recorded only from a
/// fresh agent request and compared against the live view's `EntityId` and
/// the exact [`SharedHost`] connection object on every capture and delivery.
#[derive(Clone)]
struct PreviewBinding {
    view: gpui_kit::EntityId,
    host: SharedHost,
    source: PreviewSourceState,
}

impl PreviewBinding {
    fn same_target(&self, other: &PreviewBinding) -> bool {
        self.view == other.view
            && self.source == other.source
            && std::sync::Arc::ptr_eq(&self.host, &other.host)
    }

    fn holds(&self, view: gpui_kit::EntityId, host: &SharedHost) -> bool {
        self.view == view && std::sync::Arc::ptr_eq(&self.host, host)
    }
}

/// The explicit share composer for one agent preview (#393).
pub(crate) struct PreviewFeedback {
    pub(crate) input: Entity<InputState>,
    pub(crate) open: bool,
    pub(crate) pending: bool,
    pub(crate) error: Option<String>,
    pub(crate) notice: Option<String>,
    /// Set after a successful send; the next render clears the field with
    /// the `Window` this state was created with.
    clear_draft: bool,
    pending_capture: Option<CaptureJob>,
    _subscription: Subscription,
}

struct CaptureJob {
    request: u64,
    feedback: String,
    /// The pane view and Host connection verified when the user asked for
    /// this screenshot. The answer may arrive after a cancel, a newer share,
    /// or a replaced source; delivery compares against this snapshot.
    binding: PreviewBinding,
}

/// Why an explicit share cannot be handed to the remembered pane. Every arm
/// is shown in the composer: silence would read as a delivered share.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PreviewTargetError {
    WorkspaceChanged,
    PaneGone,
    PaneElsewhere,
    HostMismatch,
    HostDisconnected,
    Exited,
    Locked,
    /// No fresh request bound this tab to the live view and Host connection
    /// it claims, or that incarnation has been replaced.
    Unverified,
}

impl PreviewTargetError {
    fn message(self, pane: u64) -> String {
        let pane = format!("%{pane}");
        match self {
            Self::WorkspaceChanged => t(L10nKey::BrowserPreviewWorkspaceChanged).to_string(),
            Self::PaneGone => t_fmt(L10nKey::BrowserPreviewPaneGone, &[("pane", &pane)]),
            Self::PaneElsewhere => t_fmt(L10nKey::BrowserPreviewPaneElsewhere, &[("pane", &pane)]),
            Self::HostMismatch => t(L10nKey::BrowserPreviewHostMismatch).to_string(),
            Self::HostDisconnected => t(L10nKey::BrowserPreviewHostDisconnected).to_string(),
            Self::Exited => t_fmt(L10nKey::BrowserPreviewPaneExited, &[("pane", &pane)]),
            Self::Locked => t_fmt(L10nKey::BrowserPreviewPaneLocked, &[("pane", &pane)]),
            Self::Unverified => t(L10nKey::BrowserPreviewSourceUnverified).to_string(),
        }
    }

    fn from_agent_prompt(error: AgentPromptError) -> Self {
        match error {
            AgentPromptError::Exited => Self::Exited,
            AgentPromptError::HostDisconnected => Self::HostDisconnected,
            AgentPromptError::Locked => Self::Locked,
        }
    }
}

/// The Host-bound route a page has at this moment (#392).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RouteState {
    /// This machine's page, or a remote page that has not navigated to
    /// loopback: addresses load as written.
    Local,
    /// A Host-bound loopback page with no verified forward yet.
    Unverified,
    /// The verified route the surface may load.
    Verified(route::VerifiedRoute),
    /// The forward is gone or could not be built. The last verified route is
    /// kept so a stale forwarded history entry can be translated back to the
    /// Host's spelling before it is verified again.
    Lost {
        message: String,
        last: Option<route::VerifiedForward>,
    },
}

impl RouteState {
    fn last_forward(&self) -> Option<&route::VerifiedForward> {
        match self {
            Self::Verified(route::VerifiedRoute::Forwarded(forward)) => Some(forward),
            Self::Lost { last, .. } => last.as_ref(),
            _ => None,
        }
    }

    fn verified(&self) -> Option<&route::VerifiedRoute> {
        match self {
            Self::Verified(route) => Some(route),
            _ => None,
        }
    }
}

/// What a load is for, so a verified route can be applied to the right
/// consumer without keeping a `Window` alive across the verification.
enum PageLoad {
    /// Create the native surface on its first paint. `parent` is `None` only
    /// in the automated workbench fixture, which starts the recording
    /// surface instead of a real WebView.
    Start {
        parent: Option<BrowserParentWindow>,
        bounds: Bounds<Pixels>,
        scale: f32,
        site: route::BrowserSiteData,
    },
    /// Navigate an existing surface.
    Navigate { url: String },
    /// Hand the URL to the system browser.
    External { url: String },
}

/// What the chrome renders over the page area.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PageStatus {
    /// Never painted yet — the page area has no bounds to start a surface in.
    Idle,
    Starting,
    Ready,
    /// A Host-bound page whose forward could not be verified or was lost.
    /// The overlay explains and offers an explicit reconnect; no loopback
    /// load is attempted until it succeeds (#392).
    RouteUnverified(String),
    Failed(String),
    /// No native host (a platform without one, or an automated test that did
    /// not attach the recording surface).
    Unsupported(String),
}

impl WebPage {
    fn new() -> Self {
        Self {
            surface: None,
            events: None,
            status: PageStatus::Idle,
            current_url: String::new(),
            display_url: String::new(),
            route: RouteState::Local,
            route_epoch: 0,
            route_pending: false,
            live_title: String::new(),
            can_back: false,
            can_forward: false,
            bounds: None,
            window_id: None,
            restore_focus: false,
            last_paint: std::time::Instant::now(),
            surface_visible: false,
            surface_pending: false,
            surface_attempted: false,
            cover_synced: None,
            pending_still_frame: None,
            still_frame: None,
            load_error: None,
            feedback: None,
            preview: None,
            next_capture: 0,
            html_mount: None,
        }
    }

    /// The label the strip shows: the live title once the page reports one,
    /// the current URL on the way there, the original address as the last
    /// resort. Never empty — the tab must read as something.
    pub(crate) fn display_title(&self, tab: &WebTab) -> String {
        if !self.live_title.is_empty() {
            return self.live_title.clone();
        }
        if self.html_mount.is_some() || tab.html_path.is_some() {
            if !tab.title.is_empty() {
                return tab.title.clone();
            }
            if !self.display_url.is_empty() {
                return self.display_url.clone();
            }
            return tab.address.clone();
        }
        if !self.current_url.is_empty() && self.current_url != "about:blank" {
            return self.current_url.clone();
        }
        tab.address.clone()
    }

    /// Applies the visibility the render path computed, covering overlay
    /// included: a covered page keeps its live surface hidden, and the panel
    /// paints the bounded still frame in its place. Returns whether anything
    /// changed. Hiding never touches the page, and focus the page held is
    /// handed back when the last cover lifts.
    fn sync_visibility(&mut self, visible: bool, covered: bool) -> bool {
        let Some(surface) = self.surface.as_mut() else {
            return false;
        };
        let effective = visible && !covered;
        if self.surface_visible == effective && self.cover_synced == Some(covered) {
            return false;
        }
        if self.surface_visible && !effective {
            self.restore_focus = surface.has_focus();
            self.still_frame = None;
            let request = surface::still_frame_request_id();
            self.pending_still_frame = surface
                .command(BrowserCommand::Capture { request })
                .ok()
                .map(|()| request);
        }
        self.surface_visible = effective;
        self.cover_synced = Some(covered);
        surface.set_visible(effective);
        if effective && self.restore_focus {
            self.restore_focus = false;
            surface.focus();
        }
        true
    }

    fn invalidate_still_frame(&mut self) {
        self.pending_still_frame = None;
        self.still_frame = None;
    }

    fn finish_still_frame(&mut self, request: u64, png: Option<Vec<u8>>) {
        if self.pending_still_frame != Some(request) {
            return;
        }
        self.pending_still_frame = None;
        self.still_frame = png.and_then(|bytes| {
            const MAX_BYTES: usize = 8 << 20;
            const MAX_PIXELS: u64 = 8 << 20;
            if bytes.len() > MAX_BYTES {
                return None;
            }
            let (width, height) = image::ImageReader::with_format(
                std::io::Cursor::new(&bytes),
                image::ImageFormat::Png,
            )
            .into_dimensions()
            .ok()?;
            if u64::from(width) * u64::from(height) > MAX_PIXELS {
                return None;
            }
            Some(std::sync::Arc::new(gpui_kit::Image::from_bytes(
                gpui_kit::ImageFormat::Png,
                bytes,
            )))
        });
    }
}

/// The one address field the browser chrome renders from. Only one page is
/// visible at a time, so one input serves every tab; selecting a tab resets
/// it to that page's current URL.
pub(crate) struct BrowserChrome {
    pub(crate) input: Entity<InputState>,
    displayed: Option<(String, String)>,
    _subscription: Subscription,
}

impl BrowserChrome {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<LyrelApp>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx));
        let subscription = cx.subscribe_in(&input, window, |app, _input, event, _window, cx| {
            if let InputEvent::PressEnter { .. } = event {
                app.browser_address_submitted(cx);
            }
        });
        Self {
            input,
            displayed: None,
            _subscription: subscription,
        }
    }
}

impl LyrelApp {
    pub(crate) fn focus_browser_address(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let chrome = self
            .browser_chrome
            .get_or_insert_with(|| BrowserChrome::new(window, cx));
        window.focus(&chrome.input.read(cx).focus_handle(cx), cx);
    }

    /// Opens `address` as a web tab on this workspace and reveals its
    /// document, reusing an existing tab when `(host, normalized_url)` already
    /// exists. The entry points — the `+` menu, Search Everywhere, a terminal
    /// link, Info panel ports — all land here; the layout API is what keeps the
    /// Browser tool's list, the mirror and the persisted record in one write.
    /// The surface itself starts lazily on the document's first paint.
    pub(crate) fn open_web_page(
        &mut self,
        address: &str,
        host: Option<lyrel_core::core::session::RemoteRef>,
        cx: &mut Context<Self>,
    ) {
        let mut layout = self.tool_layout();
        if !policy::is_blank(address) {
            let normalized = normalized_web_address(address);
            if let Some(existing) = layout.web_tabs.iter().find(|tab| {
                tab.html_path.is_none()
                    && !policy::is_blank(&tab.address)
                    && tab.host == host
                    && normalized_web_address(&tab.address) == normalized
            }) {
                let existing_id = existing.id.clone();
                layout.show(lyrel_core::core::config::RightPanelTab::Browser);
                self.reveal_browser_detail(existing_id, cx);
                self.store_tool_layout(&layout, cx);
                cx.notify();
                return;
            }
        }
        let bound = host.is_some();
        let tab = WebTab {
            id: self.new_web_tab_id(),
            address: address.to_string(),
            title: String::new(),
            host,
            source: None,
            html_path: None,
        };
        let tab_id = tab.id.clone();
        layout.show_web(tab);
        // Raise the page's document before the layout is stored, so the very
        // first paint already finds the detail front and can start the
        // surface. No focus moves here: the keyboard stays wherever the
        // entry point left it.
        self.reveal_browser_detail(tab_id.clone(), cx);
        let page = self
            .right_panel
            .web_pages
            .entry(tab_id)
            .or_insert_with(WebPage::new);
        page.display_url = address.to_string();
        page.route = if bound {
            RouteState::Unverified
        } else {
            RouteState::Local
        };
        self.store_tool_layout(&layout, cx);
        cx.notify();
    }

    /// Opens a local or remote HTML file in the Side Panel Browser (#420).
    ///
    /// Deduplicates by `(host, html_path)`: if an existing tab is already open
    /// for that file on that Host, focuses and reloads it; otherwise opens a
    /// new Browser tab whose address bar shows the clean Host path while the
    /// native surface loads from the Host-backed loopback mount.
    pub(crate) fn open_html_preview(
        &mut self,
        host_id: HostId,
        path: &std::path::Path,
        cx: &mut Context<Self>,
    ) {
        if !lyrel_core::core::web::is_html_preview_path(path) {
            return;
        }
        let host_ref = if host_id.is_local() {
            None
        } else {
            let found = lyrel_ui_base::session::WorkspaceStore::remote_ref(cx, self.workspace)
                .filter(|remote| remote.host_id() == host_id)
                .or_else(|| {
                    lyrel_ui_base::session::WorkspaceStore::all(cx)
                        .views
                        .iter()
                        .filter(|view| view.open)
                        .filter_map(|view| view.host.as_ref())
                        .find(|remote| remote.host_id() == host_id)
                        .cloned()
                });
            if found.is_none() {
                self.notify_html_preview_error("HTML preview Host is unavailable", cx);
                return;
            }
            found
        };
        let clean_path = lyrel_core::core::path_spelling::spelling_on(host_id, path)
            .to_string_lossy()
            .into_owned();
        let file_name = clean_path
            .rsplit(['/', '\\'])
            .next()
            .filter(|name| !name.is_empty())
            .unwrap_or(&clean_path)
            .to_string();

        let mut layout = self.tool_layout();
        if let Some(existing_id) = layout
            .web_tabs
            .iter()
            .find(|tab| {
                tab.host == host_ref
                    && tab
                        .html_path
                        .as_deref()
                        .is_some_and(|existing| html_paths_match(host_id, existing, &clean_path))
            })
            .map(|tab| tab.id.clone())
        {
            layout.show(lyrel_core::core::config::RightPanelTab::Browser);
            self.reveal_browser_detail(existing_id.clone(), cx);
            self.store_tool_layout(&layout, cx);
            self.browser_reload(&existing_id, cx);
            cx.notify();
            return;
        }

        let tab = WebTab {
            id: self.new_web_tab_id(),
            address: clean_path.clone(),
            title: file_name,
            host: host_ref,
            source: None,
            html_path: Some(clean_path.clone()),
        };
        let tab_id = tab.id.clone();
        layout.show_web(tab);
        self.reveal_browser_detail(tab_id.clone(), cx);
        let page = self
            .right_panel
            .web_pages
            .entry(tab_id)
            .or_insert_with(WebPage::new);
        page.display_url = clean_path;
        page.route = RouteState::Local;
        self.store_tool_layout(&layout, cx);
        cx.notify();
    }

    /// Mounts a local or remote HTML file on the Host-backed loopback server
    /// and opens its URL in the OS default system browser (#420).
    pub(crate) fn open_html_in_system_browser(
        &mut self,
        host_id: HostId,
        path: &std::path::Path,
        cx: &mut Context<Self>,
    ) {
        if !lyrel_core::core::web::is_html_preview_path(path) {
            return;
        }
        let Some(host) = lyrel_ui_base::host_registry::HostRegistry::lookup(cx, host_id) else {
            self.notify_html_preview_error(t(L10nKey::BrowserHtmlHostDisconnected), cx);
            return;
        };
        if !host.is_connected() {
            self.notify_html_preview_error(t(L10nKey::BrowserHtmlHostDisconnected), cx);
            return;
        }
        let service = cx
            .default_global::<html_preview::HtmlPreviewService>()
            .clone();
        let path = path.to_path_buf();
        let mount_host = std::sync::Arc::clone(&host);
        lyrel_ui_base::host_ops::HostOps::run(
            host,
            cx,
            move |_| service.mount_external(mount_host, &path),
            move |app, result, cx| match result {
                Ok(url) => cx.open_url(&url),
                Err(err) => app.notify_html_preview_error(&err.to_string(), cx),
            },
        );
    }

    fn notify_html_preview_error(&self, message: &str, cx: &mut gpui_kit::App) {
        log::warn!("html preview failed: {message}");
        if let Some(handle) = crate::ui::windows::WindowRegistry::window_for(cx, self.workspace) {
            let text = message.to_string();
            let _ = handle.update(cx, |_, window, cx| {
                crate::ui::notification_history::NotificationWindowExt::notify_app(
                    window,
                    crate::ui::notification_history::Notification::error(text),
                    cx,
                );
            });
        }
    }

    /// Opens `address` in the OS system browser. Remote loopback addresses are
    /// verified and forwarded first through `BrowserForwardVerifier`; if
    /// verification fails or the Host connection changes, nothing opens.
    pub(crate) fn open_url_in_system_browser(
        &mut self,
        address: &str,
        host: Option<lyrel_core::core::session::RemoteRef>,
        cx: &mut Context<Self>,
    ) {
        if matches!(
            policy::classify(address),
            policy::Route::Blank | policy::Route::Deny
        ) {
            return;
        }
        let Some(host) = host.filter(|_| route::is_loopback(address)) else {
            cx.open_url(address);
            return;
        };
        let plan = match route::route_plan_for_host(&host, address, cx) {
            Ok(plan) => plan,
            Err(failure) => {
                log::warn!(
                    "failed to resolve system browser route for {address}: {}",
                    failure.message
                );
                return;
            }
        };
        match plan {
            route::RoutePlan::Shared { .. } => {
                cx.open_url(address);
            }
            route::RoutePlan::Forward { route, loopback } => {
                let verifier = cx
                    .default_global::<route::BrowserForwardVerifier>()
                    .0
                    .clone();
                let host_id = host.host_id();
                let connection = crate::ui::desktop_lookups::HostLinks::get(cx, host_id);
                let verify_host = loopback.forward_host().to_string();
                let verify_port = loopback.port;
                let address = address.to_string();
                cx.spawn(async move |this, cx| {
                    let result = cx
                        .background_executor()
                        .spawn(async move { verifier.verify(&route, &verify_host, verify_port) })
                        .await;
                    let _ = this.update(cx, |_app, cx| {
                        let live = crate::ui::desktop_lookups::HostLinks::get(cx, host_id);
                        let same_connection = match (&connection, &live) {
                            (Some(before), Some(now)) => {
                                std::sync::Arc::ptr_eq(before, now) && now.client().is_connected()
                            }
                            (None, None) => true,
                            _ => false,
                        };
                        if !same_connection || route::owning_workspace_for_host(&host, cx).is_err()
                        {
                            log::warn!(
                                "aborted system browser open for {address}: Host connection changed"
                            );
                            return;
                        }
                        match result {
                            Ok(local_port) => {
                                let forward =
                                    route::VerifiedForward::from_loopback(&loopback, local_port);
                                if let Some(target) = forward.forwarded_url(&address)
                                    && !matches!(
                                        policy::classify(&target),
                                        policy::Route::Blank | policy::Route::Deny
                                    )
                                {
                                    cx.open_url(&target);
                                }
                            }
                            Err(message) => {
                                log::warn!(
                                    "failed to forward loopback URL {address} for system browser: {message}"
                                );
                            }
                        }
                    });
                })
                .detach();
            }
        }
    }

    /// Reveals one agent-requested preview in its own workspace (#393, #424).
    ///
    /// Called only for the window that holds `source_workspace`. The Host
    /// binding is read from this window's own `WorkspaceStore` record, never
    /// from the event, so a remote preview inherits the verified route the
    /// window already uses. The page is selected and the panel expands; no
    /// focus call happens here, so the terminal keeps the keyboard and the
    /// window is not activated. A second request from the same pane reuses
    /// that pane's preview tab instead of growing the strip.
    pub(crate) fn reveal_agent_preview(
        &mut self,
        source_workspace: WorkspaceId,
        pane: u64,
        address: &str,
        file: Option<&str>,
        scratch: &str,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        enum Target {
            Url(String),
            Html(String),
        }

        let target = if let Some(raw_html) = file {
            let trimmed = raw_html.trim();
            if trimmed.is_empty() || !lyrel_core::core::web::is_html_preview_str(trimmed) {
                return Err(
                    "an agent HTML preview requires a .html, .htm, or .xhtml file path".to_string(),
                );
            }
            Target::Html(trimmed.to_string())
        } else {
            let address = address.trim();
            if !lyrel_core::core::web::is_preview_address(address) {
                return Err("an agent preview needs an absolute http(s) URL".to_string());
            }
            Target::Url(address.to_string())
        };

        let host = lyrel_ui_base::session::WorkspaceStore::remote_ref(cx, self.workspace);
        let expected = match &host {
            Some(host) if host.workspace == source_workspace => Some(host.clone()),
            Some(_) => {
                return Err("the requested preview belongs to another workspace".to_string());
            }
            None if source_workspace == self.workspace => None,
            None => {
                return Err("the requested preview belongs to another workspace".to_string());
            }
        };
        let host_id = expected
            .as_ref()
            .map(|remote| remote.host_id())
            .unwrap_or(HostId::LOCAL);
        let (tab_address, tab_title, tab_html_path) = match target {
            Target::Url(url) => (url, String::new(), None),
            Target::Html(raw_html) => {
                let clean_path = lyrel_core::core::path_spelling::spelling_on(
                    host_id,
                    std::path::Path::new(&raw_html),
                )
                .to_string_lossy()
                .into_owned();
                let file_name = clean_path
                    .rsplit(['/', '\\'])
                    .next()
                    .filter(|name| !name.is_empty())
                    .unwrap_or(&clean_path)
                    .to_string();
                (clean_path.clone(), file_name, Some(clean_path))
            }
        };
        let source = PreviewSourceState {
            workspace: source_workspace,
            pane,
            scratch: scratch.to_string(),
        };
        let mut layout = self.tool_layout();
        let existing_ix = layout.web_tabs.iter().position(|tab| {
            tab.source
                .as_ref()
                .is_some_and(|source| source.pane == pane && source.workspace == source_workspace)
        });
        if let Some(at) = existing_ix {
            let existing = layout.web_tabs[at].clone();
            let host_changed = existing.host != expected;
            let was_html = existing.html_path.is_some();
            let is_html = tab_html_path.is_some();
            let address_changed =
                existing.address != tab_address || existing.html_path != tab_html_path;
            let tab = WebTab {
                id: existing.id.clone(),
                address: tab_address.clone(),
                title: tab_title,
                host: expected,
                source: Some(source),
                html_path: tab_html_path,
            };
            let binding = self.preview_binding_for(&tab, cx);
            layout.web_tabs[at] = tab.clone();
            // A fresh request always reveals its page's document, even when
            // the user had moved to Info or another tool in the meantime
            // (#393). The row is replaced in place; `show` opens or reuses
            // the Browser tool's single list entry.
            layout.show(lyrel_core::core::config::RightPanelTab::Browser);
            let bound = tab.host.is_some();
            if host_changed {
                // The page runtime was created for the old Host (its site-data
                // partition and route guard). A host rebinding starts a fresh
                // page rather than reusing another Host's surface.
                self.right_panel.web_pages.remove(&tab.id);
            }
            let page = self
                .right_panel
                .web_pages
                .entry(tab.id.clone())
                .or_insert_with(WebPage::new);
            page.preview = binding;
            page.display_url = tab_address.clone();
            let has_surface = page.surface.is_some();
            let had_html_mount = page.html_mount.is_some();
            if is_html {
                page.html_mount = None;
                page.load_error = None;
                page.live_title.clear();
                page.status = PageStatus::Starting;
                page.route = RouteState::Local;
                if !has_surface {
                    page.surface_attempted = false;
                    page.surface_pending = false;
                }
            } else if address_changed || host_changed || was_html || had_html_mount {
                page.html_mount = None;
                page.load_error = None;
                page.live_title.clear();
                page.status = PageStatus::Starting;
                page.surface_attempted = false;
                page.surface_pending = false;
                page.route = if bound {
                    RouteState::Unverified
                } else {
                    RouteState::Local
                };
            }
            // Raise before the store, like a fresh open: the first paint
            // after the reveal finds the document front.
            self.reveal_browser_detail(tab.id.clone(), cx);
            self.store_tool_layout(&layout, cx);
            if is_html {
                if has_surface {
                    self.begin_web_load(&tab.id, PageLoad::Navigate { url: tab_address }, cx);
                }
            } else if address_changed || host_changed || was_html || had_html_mount {
                self.navigate_web_tab(&tab.id, &tab_address, cx);
            }
            cx.notify();
            return Ok(());
        }
        let bound = expected.is_some();
        let is_html = tab_html_path.is_some();
        let tab = WebTab {
            id: self.new_web_tab_id(),
            address: tab_address.clone(),
            title: tab_title,
            host: expected,
            source: Some(source),
            html_path: tab_html_path,
        };
        let binding = self.preview_binding_for(&tab, cx);
        let tab_id = tab.id.clone();
        layout.show_web(tab);
        // Raise before the store, like `open_web_page`: the first paint
        // after the reveal finds the document front. The page's own reveal
        // never moves the keyboard — the terminal keeps it.
        self.reveal_browser_detail(tab_id.clone(), cx);
        let page = self
            .right_panel
            .web_pages
            .entry(tab_id)
            .or_insert_with(WebPage::new);
        page.preview = binding;
        page.display_url = tab_address;
        page.route = if is_html || !bound {
            RouteState::Local
        } else {
            RouteState::Unverified
        };
        self.store_tool_layout(&layout, cx);
        cx.notify();
        Ok(())
    }

    /// The source chip on the preview's chrome: which pane asked, and whether
    /// that pane can still receive a share. The closed form is visible even
    /// before the user tries to send.
    pub(crate) fn agent_preview_source_label(
        &self,
        tab: &WebTab,
        cx: &gpui_kit::App,
    ) -> Option<gpui_kit::SharedString> {
        let source = tab.source.as_ref()?;
        let pane = format!("%{}", source.pane);
        let key = match self.preview_target(tab, source, cx) {
            Ok(_) => L10nKey::BrowserPreviewSource,
            Err(PreviewTargetError::Unverified) => L10nKey::BrowserPreviewSourceStale,
            Err(_) => L10nKey::BrowserPreviewSourceMissing,
        };
        Some(t_fmt(key, &[("pane", &pane)]).into())
    }

    /// Resolves the remembered source pane to a live view on the preview's
    /// verified Host. Every route check the share depends on is here: window
    /// workspace, pane ownership, Host route and connection. The pane number
    /// alone is not an identity — the same number can be live on another
    /// Host or workspace — so the lookup resolves the original pane's route
    /// as well as its number. It does not yet claim that this is the same
    /// pane incarnation that asked for the page — [`Self::preview_target`]
    /// compares the live binding for that.
    fn preview_identity(
        &self,
        tab: &WebTab,
        source: &PreviewSourceState,
        cx: &gpui_kit::App,
    ) -> Result<(Entity<TerminalView>, SharedHost), PreviewTargetError> {
        let window_host = lyrel_ui_base::session::WorkspaceStore::remote_ref(cx, self.workspace);
        if window_host != tab.host {
            return Err(PreviewTargetError::WorkspaceChanged);
        }
        let source_workspace_matches = match &tab.host {
            Some(host) => source.workspace == host.workspace,
            None => source.workspace == self.workspace,
        };
        if !source_workspace_matches {
            return Err(PreviewTargetError::WorkspaceChanged);
        }
        let expected = tab
            .host
            .as_ref()
            .map(|host| host.host_id())
            .unwrap_or(HostId::LOCAL);
        // A pane number is only unique inside one Host route: the same
        // number can be live on another Host or workspace, so the original
        // pane is resolved by route as well as number. A same-numbered pane
        // elsewhere is kept as a fallback so the checks below still name the
        // precise mismatch instead of reporting the pane as gone.
        let mut same_number = self
            .tabs
            .iter()
            .flat_map(|tab| tab.pane.terminals())
            .filter(|view| view.read(cx).pane_id == source.pane);
        let mut elsewhere = None;
        let mut original = None;
        for candidate in &mut same_number {
            let terminal = candidate.read(cx);
            let on_the_preview_route = match &tab.host {
                Some(host) => {
                    terminal.host_id() == host.host_id()
                        && terminal.workspace().is_some_and(|workspace| {
                            workspace.workspace == self.workspace && workspace.target == host.target
                        })
                }
                None => terminal.host_id().is_local(),
            };
            if on_the_preview_route {
                original = Some(candidate);
                break;
            }
            elsewhere.get_or_insert(candidate);
        }
        let view = original.or(elsewhere).ok_or(PreviewTargetError::PaneGone)?;
        let terminal = view.read(cx);
        if terminal
            .owner_workspace()
            .is_some_and(|workspace| workspace != self.workspace)
        {
            return Err(PreviewTargetError::PaneElsewhere);
        }
        let on_the_preview_host = match &tab.host {
            Some(host) => {
                terminal.host_id() == host.host_id()
                    && terminal.workspace().is_some_and(|workspace| {
                        workspace.workspace == self.workspace && workspace.target == host.target
                    })
            }
            None => terminal.host_id().is_local(),
        };
        if !on_the_preview_host {
            return Err(if terminal.host_id() != expected {
                PreviewTargetError::HostMismatch
            } else {
                PreviewTargetError::PaneElsewhere
            });
        }
        let host = terminal
            .host(cx)
            .ok_or(PreviewTargetError::HostDisconnected)?;
        if host.id() != expected {
            return Err(PreviewTargetError::HostMismatch);
        }
        if !host.is_connected() {
            return Err(PreviewTargetError::HostDisconnected);
        }
        Ok((view, host))
    }

    /// Records the live authority a fresh agent request just established.
    /// `None` means the pane is not live right now; the tab still shows the
    /// request, but no share is sendable until a later request succeeds.
    fn preview_binding_for(&self, tab: &WebTab, cx: &gpui_kit::App) -> Option<PreviewBinding> {
        let source = tab.source.as_ref()?;
        self.preview_identity(tab, source, cx)
            .ok()
            .map(|(view, host)| PreviewBinding {
                view: view.entity_id(),
                host,
                source: source.clone(),
            })
    }

    /// The authoritative share target: the pane number must resolve to the
    /// same live view and Host connection a fresh agent request bound, and
    /// the pane's stream must be able to take input now.
    fn preview_target(
        &self,
        tab: &WebTab,
        source: &PreviewSourceState,
        cx: &gpui_kit::App,
    ) -> Result<(Entity<TerminalView>, SharedHost), PreviewTargetError> {
        let (view, host) = self.preview_identity(tab, source, cx)?;
        let binding = self
            .right_panel
            .web_pages
            .get(&tab.id)
            .and_then(|page| page.preview.as_ref())
            .ok_or(PreviewTargetError::Unverified)?;
        if &binding.source != source || !binding.holds(view.entity_id(), &host) {
            return Err(PreviewTargetError::Unverified);
        }
        if let Some(error) = view.read(cx).agent_prompt_error(cx) {
            return Err(PreviewTargetError::from_agent_prompt(error));
        }
        Ok((view, host))
    }

    /// Revalidates a capture or a delivery against the exact incarnation
    /// verified when the user asked for the share. A cancelled share, a
    /// newer request, a replaced pane view, or a reconnected Host all fail
    /// here rather than reaching whatever pane now owns the number.
    fn preview_binding_holds(
        &self,
        tab_id: &str,
        expected: &PreviewBinding,
        cx: &gpui_kit::App,
    ) -> Result<(Entity<TerminalView>, SharedHost), PreviewTargetError> {
        let tab = self.web_tab(tab_id).ok_or(PreviewTargetError::Unverified)?;
        let source = tab.source.as_ref().ok_or(PreviewTargetError::Unverified)?;
        if source != &expected.source {
            return Err(PreviewTargetError::Unverified);
        }
        let (view, host) = self.preview_identity(&tab, source, cx)?;
        if !expected.holds(view.entity_id(), &host) {
            return Err(PreviewTargetError::Unverified);
        }
        let page_binding = self
            .right_panel
            .web_pages
            .get(tab_id)
            .and_then(|page| page.preview.as_ref())
            .ok_or(PreviewTargetError::Unverified)?;
        if !expected.same_target(page_binding) {
            return Err(PreviewTargetError::Unverified);
        }
        if let Some(error) = view.read(cx).agent_prompt_error(cx) {
            return Err(PreviewTargetError::from_agent_prompt(error));
        }
        Ok((view, host))
    }

    /// Opens the explicit share composer on `tab_id`, creating its field on
    /// first use. This is a user action, so it may take the keyboard.
    pub(crate) fn browser_preview_feedback_open(
        &mut self,
        tab_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.web_tab(tab_id) else {
            return;
        };
        if tab.source.is_none() {
            return;
        }
        let placeholder = t(L10nKey::BrowserPreviewFeedbackPlaceholder).to_string();
        let submit_tab = tab_id.to_string();
        let page = self
            .right_panel
            .web_pages
            .entry(tab_id.to_string())
            .or_insert_with(WebPage::new);
        if page.feedback.is_none() {
            let input = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));
            let subscription =
                cx.subscribe_in(&input, window, move |app, _input, event, window, cx| {
                    if matches!(event, InputEvent::PressEnter { .. }) {
                        app.browser_preview_feedback_send(&submit_tab, window, cx);
                    }
                });
            page.feedback = Some(PreviewFeedback {
                input,
                open: true,
                pending: false,
                error: None,
                notice: None,
                clear_draft: false,
                pending_capture: None,
                _subscription: subscription,
            });
        }
        let input = {
            let feedback = page.feedback.as_mut().expect("feedback was just ensured");
            feedback.open = true;
            if !feedback.pending {
                feedback.error = None;
            }
            feedback.input.clone()
        };
        window.focus(&input.read(cx).focus_handle(cx), cx);
        cx.notify();
    }

    pub(crate) fn browser_preview_feedback_cancel(&mut self, tab_id: &str, cx: &mut Context<Self>) {
        if let Some(feedback) = self
            .right_panel
            .web_pages
            .get_mut(tab_id)
            .and_then(|page| page.feedback.as_mut())
        {
            feedback.open = false;
            feedback.pending = false;
            feedback.pending_capture = None;
            feedback.error = None;
            feedback.notice = None;
        }
        cx.notify();
    }

    fn set_preview_feedback_error(
        &mut self,
        tab_id: &str,
        message: String,
        cx: &mut Context<Self>,
    ) {
        if let Some(feedback) = self
            .right_panel
            .web_pages
            .get_mut(tab_id)
            .and_then(|page| page.feedback.as_mut())
        {
            feedback.pending = false;
            feedback.pending_capture = None;
            feedback.error = Some(message);
        }
        cx.notify();
    }

    /// The explicit send: verify the source, then ask the surface for one
    /// screenshot. Nothing is written or delivered until that answer arrives,
    /// and both steps are owned by this one click.
    pub(crate) fn browser_preview_feedback_send(
        &mut self,
        tab_id: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.web_tab(tab_id) else {
            return;
        };
        let Some(source) = tab.source.clone() else {
            self.set_preview_feedback_error(
                tab_id,
                t(L10nKey::BrowserPreviewNoSource).to_string(),
                cx,
            );
            return;
        };
        if let Some(feedback) = self
            .right_panel
            .web_pages
            .get(tab_id)
            .and_then(|page| page.feedback.as_ref())
            && feedback.pending
        {
            return;
        }
        let (view, host) = match self.preview_target(&tab, &source, cx) {
            Ok(target) => target,
            Err(error) => {
                self.set_preview_feedback_error(tab_id, error.message(source.pane), cx);
                return;
            }
        };
        let binding = PreviewBinding {
            view: view.entity_id(),
            host,
            source: source.clone(),
        };
        let feedback_text = self
            .right_panel
            .web_pages
            .get(tab_id)
            .and_then(|page| page.feedback.as_ref())
            .map(|feedback| feedback.input.read(cx).value().to_string())
            .unwrap_or_default();
        let request = {
            let Some(page) = self.right_panel.web_pages.get_mut(tab_id) else {
                return;
            };
            page.next_capture = page.next_capture.wrapping_add(1);
            let request = page.next_capture;
            let Some(feedback) = page.feedback.as_mut() else {
                return;
            };
            feedback.pending = true;
            feedback.error = None;
            feedback.notice = None;
            feedback.pending_capture = Some(CaptureJob {
                request,
                feedback: feedback_text.trim().to_string(),
                binding,
            });
            request
        };
        let command = self
            .right_panel
            .web_pages
            .get_mut(tab_id)
            .and_then(|page| page.surface.as_mut())
            .map(|surface| surface.command(BrowserCommand::Capture { request }))
            .unwrap_or_else(|| {
                Err(surface::BrowserSurfaceError::Platform(
                    "the browser surface is not running".to_string(),
                ))
            });
        if let Err(error) = command {
            self.set_preview_feedback_error(
                tab_id,
                t_fmt(
                    L10nKey::BrowserPreviewCaptureUnavailable,
                    &[("error", &error.to_string())],
                ),
                cx,
            );
            return;
        }
        cx.notify();
    }

    fn preview_capture_failed(
        &mut self,
        tab_id: &str,
        request: u64,
        message: String,
        cx: &mut Context<Self>,
    ) {
        let matches = self
            .right_panel
            .web_pages
            .get_mut(tab_id)
            .and_then(|page| page.feedback.as_mut())
            .filter(|feedback| {
                feedback
                    .pending_capture
                    .as_ref()
                    .is_some_and(|job| job.request == request)
            });
        let Some(feedback) = matches else {
            return;
        };
        feedback.pending = false;
        feedback.pending_capture = None;
        feedback.error = Some(t_fmt(
            L10nKey::BrowserPreviewCaptureFailed,
            &[("error", &message)],
        ));
        cx.notify();
    }

    /// A screenshot arrived for the share the user asked for. The request
    /// stays pending — and keeps its identity — through the Host write; the
    /// bytes are written through the pane's own Host (which may be remote),
    /// then the prompt is delivered only if the same live incarnation still
    /// owns the capture the user asked for.
    fn preview_capture_ready(
        &mut self,
        tab_id: &str,
        request: u64,
        png: Vec<u8>,
        cx: &mut Context<Self>,
    ) {
        let Some((feedback_text, binding)) = self
            .right_panel
            .web_pages
            .get(tab_id)
            .and_then(|page| page.feedback.as_ref())
            .and_then(|feedback| feedback.pending_capture.as_ref())
            .filter(|job| job.request == request)
            .map(|job| (job.feedback.clone(), job.binding.clone()))
        else {
            // A cancelled or superseded share: never consume the newer
            // request that may be pending now.
            return;
        };
        let (_, host) = match self.preview_binding_holds(tab_id, &binding, cx) {
            Ok(target) => target,
            Err(error) => {
                self.set_preview_feedback_error(tab_id, error.message(binding.source.pane), cx);
                return;
            }
        };
        let Some(tab) = self.web_tab(tab_id) else {
            self.set_preview_feedback_error(
                tab_id,
                PreviewTargetError::Unverified.message(binding.source.pane),
                cx,
            );
            return;
        };
        let address = self
            .right_panel
            .web_pages
            .get(tab_id)
            .map(|page| {
                if page.display_url.is_empty() {
                    tab.address.clone()
                } else {
                    page.display_url.clone()
                }
            })
            .unwrap_or_else(|| tab.address.clone());
        let scratch = std::path::PathBuf::from(&binding.source.scratch);
        if !host.is_absolute(&scratch) {
            self.set_preview_feedback_error(
                tab_id,
                t(L10nKey::BrowserPreviewScratchInvalid).to_string(),
                cx,
            );
            return;
        }
        let file_name = format!(
            "preview-{}-{}.png",
            binding.source.pane,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_millis())
                .unwrap_or_default()
        );
        let tab = tab_id.to_string();
        let landing_binding = binding.clone();
        lyrel_ui_base::host_ops::HostOps::run(
            host,
            cx,
            move |host| {
                let file = host.join(&scratch, &file_name);
                host.create_dir(&scratch, true)?;
                host.write_file(&file, &png)?;
                Ok(file)
            },
            move |app, result, cx| {
                app.preview_share_landed(
                    &tab,
                    request,
                    landing_binding,
                    feedback_text,
                    address,
                    result,
                    cx,
                );
            },
        );
        cx.notify();
    }

    #[allow(clippy::too_many_arguments)]
    fn preview_share_landed(
        &mut self,
        tab_id: &str,
        request: u64,
        binding: PreviewBinding,
        feedback: String,
        address: String,
        result: std::io::Result<std::path::PathBuf>,
        cx: &mut Context<Self>,
    ) {
        // Only the share the user is still waiting on may finish. A cancel,
        // a newer share, or a replaced page leaves the newer state alone.
        let still_pending = self
            .right_panel
            .web_pages
            .get(tab_id)
            .and_then(|page| page.feedback.as_ref())
            .and_then(|feedback| feedback.pending_capture.as_ref())
            .is_some_and(|job| job.request == request);
        if !still_pending {
            return;
        }
        let path = match result {
            Ok(path) => path,
            Err(error) => {
                self.set_preview_feedback_error(
                    tab_id,
                    t_fmt(
                        L10nKey::BrowserPreviewWriteFailed,
                        &[("error", &error.to_string())],
                    ),
                    cx,
                );
                return;
            }
        };
        let path_text = path.to_string_lossy().into_owned();
        let prompt = crate::core::agent_prompt::build_preview_feedback_prompt(
            &feedback,
            Some(&path_text),
            &address,
        );
        let target = self.preview_binding_holds(tab_id, &binding, cx);
        let outcome = match (target, prompt) {
            (Ok((view, _)), Some(prompt)) => view
                .read(cx)
                .deliver_agent_prompt(&prompt, cx)
                .map_err(|error| {
                    PreviewTargetError::from_agent_prompt(error).message(binding.source.pane)
                }),
            (Err(error), _) => Err(error.message(binding.source.pane)),
            (Ok(_), None) => Err(t(L10nKey::BrowserPreviewNoSource).to_string()),
        };
        let Some(share) = self
            .right_panel
            .web_pages
            .get_mut(tab_id)
            .and_then(|page| page.feedback.as_mut())
        else {
            return;
        };
        // Re-checked after delivery so a share replaced in between keeps its
        // own pending request and result.
        if !share
            .pending_capture
            .as_ref()
            .is_some_and(|job| job.request == request)
        {
            return;
        }
        share.pending_capture = None;
        share.pending = false;
        match outcome {
            Ok(()) => {
                let sent = t_fmt(
                    L10nKey::BrowserPreviewSent,
                    &[("pane", &format!("%{}", binding.source.pane))],
                );
                share.notice = Some(sent);
                share.error = None;
                share.clear_draft = true;
            }
            Err(reason) => {
                let saved = t_fmt(
                    L10nKey::BrowserPreviewSavedUnsent,
                    &[("path", &path_text), ("reason", &reason)],
                );
                share.error = Some(saved);
                share.notice = None;
            }
        }
        cx.notify();
    }

    /// The explicit system-browser action on the page chrome (#382): hands
    /// the page the surface is actually on to the OS. A Host-bound loopback
    /// page is verified first and opened at its forwarded endpoint; when the
    /// forward cannot be verified nothing opens — never the original address,
    /// which could now be a coincident desktop service (#392).
    pub(crate) fn browser_open_external(&mut self, tab_id: &str, cx: &mut Context<Self>) {
        let Some(tab) = self.web_tab(tab_id) else {
            return;
        };
        if let Some(html_path) = tab.html_path.clone() {
            let page = self.right_panel.web_pages.get(tab_id);
            if page_showing_html_preview(page) {
                let host_id = tab
                    .host
                    .as_ref()
                    .map(|host| host.host_id())
                    .unwrap_or(HostId::LOCAL);
                let target_path = page
                    .and_then(|page| {
                        page.html_mount
                            .as_ref()
                            .and_then(|mount| mount.display_path_for_url(&page.current_url))
                    })
                    .unwrap_or(html_path);
                self.open_html_in_system_browser(host_id, std::path::Path::new(&target_path), cx);
                return;
            }
        }
        let url = self
            .right_panel
            .web_pages
            .get(tab_id)
            .map(|page| {
                if page.display_url.is_empty() {
                    tab.address.clone()
                } else {
                    page.display_url.clone()
                }
            })
            .unwrap_or_else(|| tab.address.clone());
        if matches!(
            policy::classify(&url),
            policy::Route::Blank | policy::Route::Deny
        ) {
            return;
        }
        if tab.host.is_some() && route::is_loopback(&url) {
            self.begin_web_load(tab_id, PageLoad::External { url }, cx);
            return;
        }
        cx.open_url(&url);
    }

    /// Enter submits the address field: complete what was typed, and either
    /// navigate the front page or refuse the route before the surface ever
    /// sees it. The front detail is the authority — the field only ever
    /// serves the document it is painted over.
    pub(crate) fn browser_address_submitted(&mut self, cx: &mut Context<Self>) {
        let Some(tab_id) = self.active_browser_detail().map(str::to_string) else {
            return;
        };
        let typed = self
            .browser_chrome
            .as_ref()
            .map(|chrome| chrome.input.read(cx).value().to_string())
            .unwrap_or_default();
        let Some(url) = policy::complete_typed(&typed) else {
            // Not a route the preview takes: put the field back to what the
            // page is on and leave the page where it was.
            if let Some(chrome) = self.browser_chrome.as_mut() {
                chrome.displayed = None;
            }
            cx.notify();
            return;
        };
        if policy::classify(&url) == policy::Route::Load
            && self
                .web_tab(&tab_id)
                .is_some_and(|tab| policy::is_blank(&tab.address) || tab.html_path.is_some())
        {
            let mut layout = self.tool_layout();
            if let Some(tab) = layout.web_tabs.iter_mut().find(|tab| tab.id == tab_id) {
                let bound = tab.host.is_some();
                let was_html = tab.html_path.take().is_some();
                tab.address.clone_from(&url);
                if was_html && let Some(page) = self.right_panel.web_pages.get_mut(&tab_id) {
                    page.html_mount = None;
                    page.live_title.clear();
                    page.route = if bound {
                        RouteState::Unverified
                    } else {
                        RouteState::Local
                    };
                }
                self.store_tool_layout(&layout, cx);
            }
        }
        self.navigate_web_tab(&tab_id, &url, cx);
    }

    /// Navigates the live surface without changing its saved starting address.
    /// A loopback address on a Host-bound page is verified through the Host
    /// first and loaded at the forwarded endpoint (#392).
    pub(crate) fn navigate_web_tab(&mut self, tab_id: &str, url: &str, cx: &mut Context<Self>) {
        if let Some(page) = self.right_panel.web_pages.get_mut(tab_id) {
            page.load_error = None;
            page.display_url = url.to_string();
        }
        self.begin_web_load(
            tab_id,
            PageLoad::Navigate {
                url: url.to_string(),
            },
            cx,
        );
    }

    pub(crate) fn browser_back(&mut self, tab_id: &str, cx: &mut Context<Self>) {
        self.browser_command(tab_id, BrowserCommand::Back, cx);
    }

    pub(crate) fn browser_forward(&mut self, tab_id: &str, cx: &mut Context<Self>) {
        self.browser_command(tab_id, BrowserCommand::Forward, cx);
    }

    /// Reload re-verifies a Host-bound loopback page before it loads. The
    /// surface's own reload would hit the remembered local port, which may
    /// now be a different service (#392).
    pub(crate) fn browser_reload(&mut self, tab_id: &str, cx: &mut Context<Self>) {
        let Some(tab) = self.web_tab(tab_id) else {
            return;
        };
        if let Some(html_path) = tab.html_path.clone() {
            if self
                .right_panel
                .web_pages
                .get(tab_id)
                .is_some_and(|page| page.surface.is_some())
            {
                self.begin_web_load(tab_id, PageLoad::Navigate { url: html_path }, cx);
                return;
            }
            if let Some(page) = self.right_panel.web_pages.get_mut(tab_id) {
                page.surface_attempted = false;
                page.surface_pending = false;
                page.route_pending = false;
                page.load_error = None;
                page.route = RouteState::Local;
                page.status = PageStatus::Starting;
                page.display_url = html_path;
            }
            cx.notify();
            return;
        }
        let url = self
            .right_panel
            .web_pages
            .get(tab_id)
            .map(|page| {
                if page.display_url.is_empty() {
                    tab.address.clone()
                } else {
                    page.display_url.clone()
                }
            })
            .unwrap_or_else(|| tab.address.clone());
        if tab.host.is_some() && route::is_loopback(&url) {
            self.begin_web_load(tab_id, PageLoad::Navigate { url: url.clone() }, cx);
            return;
        }
        if let Some(page) = self.right_panel.web_pages.get(tab_id)
            && page.surface.is_none()
        {
            // The first load failed before a surface existed: let the next
            // paint try again.
            if let Some(page) = self.right_panel.web_pages.get_mut(tab_id) {
                page.surface_attempted = false;
                page.surface_pending = false;
                page.status = PageStatus::Starting;
            }
            cx.notify();
            return;
        }
        self.browser_command(tab_id, BrowserCommand::Reload, cx);
    }

    /// The explicit recovery action on a lost forward (#392): verify again
    /// and load the original address through the new endpoint. The address
    /// itself never changes, so the tab keeps its identity.
    pub(crate) fn browser_reconnect(&mut self, tab_id: &str, cx: &mut Context<Self>) {
        let Some(tab) = self.web_tab(tab_id) else {
            return;
        };
        if tab.html_path.is_some() {
            if let Some(page) = self.right_panel.web_pages.get_mut(tab_id) {
                page.load_error = None;
            }
            self.browser_reload(tab_id, cx);
            return;
        }
        let url = self
            .right_panel
            .web_pages
            .get(tab_id)
            .map(|page| {
                if page.display_url.is_empty() {
                    tab.address.clone()
                } else {
                    page.display_url.clone()
                }
            })
            .unwrap_or_else(|| tab.address.clone());
        if let Some(page) = self.right_panel.web_pages.get_mut(tab_id) {
            page.load_error = None;
        }
        if tab.host.is_some() && route::is_loopback(&url) {
            if self
                .right_panel
                .web_pages
                .get(tab_id)
                .is_some_and(|page| page.surface.is_none())
            {
                // No surface yet: reset the start attempt and let the next
                // paint run verification from scratch.
                if let Some(page) = self.right_panel.web_pages.get_mut(tab_id) {
                    page.surface_attempted = false;
                    page.surface_pending = false;
                    page.status = PageStatus::Starting;
                    page.route = RouteState::Unverified;
                }
                cx.notify();
                return;
            }
            self.begin_web_load(tab_id, PageLoad::Navigate { url: url.clone() }, cx);
            return;
        }
        self.browser_reload(tab_id, cx);
    }

    /// Purges live HTML preview mounts and marks in-app HTML preview tabs as
    /// disconnected when a remote Host link is removed (#421).
    pub(crate) fn on_html_host_disconnected(&mut self, host_id: HostId, cx: &mut Context<Self>) {
        if host_id.is_local() {
            return;
        }
        let matching: Vec<(String, Option<String>)> = self
            .tool_layout()
            .web_tabs
            .into_iter()
            .filter_map(|tab| {
                if tab.html_path.is_some()
                    && tab.host.as_ref().is_some_and(|h| h.host_id() == host_id)
                {
                    Some((tab.id, tab.html_path))
                } else {
                    None
                }
            })
            .collect();
        if matching.is_empty() {
            return;
        }
        let message = t(L10nKey::BrowserHtmlHostDisconnected).to_string();
        for (tab_id, html_path) in matching {
            let on_html = page_showing_html_preview(self.right_panel.web_pages.get(&tab_id));
            if let Some(page) = self.right_panel.web_pages.get_mut(&tab_id) {
                page.html_mount = None;
                page.route_epoch = page.route_epoch.wrapping_add(1);
                page.route_pending = false;
                if on_html {
                    if page.display_url.is_empty()
                        && let Some(path) = html_path
                    {
                        page.display_url = path;
                    }
                    page.invalidate_still_frame();
                    page.route = RouteState::Lost {
                        message: message.clone(),
                        last: None,
                    };
                    page.status = PageStatus::RouteUnverified(message.clone());
                    page.surface_attempted = true;
                }
            }
            self.set_page_guard(&tab_id, surface::RouteGuard::DenyLoopback);
        }
        cx.notify();
    }

    /// Reloads open HTML preview tabs on `host_id` whose target file matches any
    /// path in `paths` (#421).
    pub(crate) fn reload_html_previews_for_paths(
        &mut self,
        host_id: HostId,
        paths: &[std::path::PathBuf],
        cx: &mut Context<Self>,
    ) {
        if paths.is_empty() {
            return;
        }
        let candidates: Vec<(String, bool, bool, bool)> = self
            .tool_layout()
            .web_tabs
            .into_iter()
            .filter_map(|tab| {
                let html_path = tab.html_path?;
                let tab_host_id = tab
                    .host
                    .as_ref()
                    .map(|h| h.host_id())
                    .unwrap_or(HostId::LOCAL);
                if tab_host_id != host_id {
                    return None;
                }
                let page = self.right_panel.web_pages.get(&tab.id);
                if !page_showing_html_preview(page) {
                    return None;
                }
                let page = page?;
                let current_html = page
                    .html_mount
                    .as_ref()
                    .and_then(|mount| mount.display_path_for_url(&page.current_url));
                let canonical_file = page
                    .html_mount
                    .as_ref()
                    .map(|mount| mount.canonical_file().to_string_lossy().into_owned());
                let matched = paths.iter().any(|changed| {
                    let changed_str = changed.to_string_lossy();
                    html_paths_match(host_id, &html_path, &changed_str)
                        || canonical_file
                            .as_deref()
                            .is_some_and(|c| html_paths_match(host_id, c, &changed_str))
                        || current_html
                            .as_deref()
                            .is_some_and(|cur| html_paths_match(host_id, cur, &changed_str))
                });
                if !matched {
                    return None;
                }
                Some((
                    tab.id,
                    page.surface.is_some(),
                    page.html_mount.is_some(),
                    matches!(page.status, PageStatus::Failed(_)),
                ))
            })
            .collect();
        for (tab_id, has_surface, has_mount, is_failed) in candidates {
            if has_surface && has_mount {
                self.browser_command(&tab_id, BrowserCommand::Reload, cx);
            } else if is_failed {
                self.browser_reload(&tab_id, cx);
            }
        }
    }

    /// The tab entry a web page id names in the Browser tool's list.
    fn web_tab(&self, tab_id: &str) -> Option<WebTab> {
        self.right_panel
            .tool_layouts
            .get(&self.workspace)?
            .web_tabs
            .iter()
            .find(|tab| tab.id == tab_id)
            .cloned()
    }

    /// Verifies the Host route for `load` when it needs one, then delivers
    /// it. A non-loopback load and a page with no Host binding go straight
    /// through.
    fn begin_web_load(&mut self, tab_id: &str, load: PageLoad, cx: &mut Context<Self>) {
        let Some(tab) = self.web_tab(tab_id) else {
            return;
        };
        let url = match &load {
            PageLoad::Start { .. } => self
                .right_panel
                .web_pages
                .get(tab_id)
                .map(|page| {
                    if page.display_url.is_empty() {
                        tab.address.clone()
                    } else {
                        page.display_url.clone()
                    }
                })
                .unwrap_or_else(|| tab.address.clone()),
            PageLoad::Navigate { url } | PageLoad::External { url } => url.clone(),
        };
        let Some(page) = self.right_panel.web_pages.get_mut(tab_id) else {
            return;
        };
        page.route_epoch = page.route_epoch.wrapping_add(1);
        page.route_pending = false;
        let epoch = page.route_epoch;
        if let Some(html_path) = tab.html_path.clone()
            && !matches!(load, PageLoad::External { .. })
        {
            self.begin_html_preview_load(tab_id, &tab, html_path, load, epoch, cx);
            return;
        }
        if tab.host.is_none() || !route::is_loopback(&url) {
            self.deliver_web_load(tab_id, load, None, cx);
            return;
        }
        let plan = match route::route_plan(&tab, &url, cx) {
            Ok(plan) => plan,
            Err(failure) => {
                self.fail_web_route(tab_id, failure.message, cx);
                return;
            }
        };
        match plan {
            route::RoutePlan::Shared { origin } => {
                let verified = route::VerifiedRoute::Shared { origin };
                self.apply_route_and_load(tab_id, verified, load, cx);
            }
            route::RoutePlan::Forward { route, loopback } => {
                // While the route is being verified, no loopback load is
                // safe: the previous endpoint may already be a coincident
                // local service (#392).
                self.set_page_guard(tab_id, surface::RouteGuard::DenyLoopback);
                if let Some(page) = self.right_panel.web_pages.get_mut(tab_id) {
                    page.route_pending = true;
                    page.status = PageStatus::Starting;
                }
                let verifier = cx
                    .default_global::<route::BrowserForwardVerifier>()
                    .0
                    .clone();
                let host_id = tab
                    .host
                    .as_ref()
                    .expect("a forwarded page has a Host")
                    .host_id();
                let connection = crate::ui::desktop_lookups::HostLinks::get(cx, host_id);
                let async_tab_id = tab_id.to_string();
                let verify_host = loopback.forward_host().to_string();
                let verify_port = loopback.port;
                cx.spawn(async move |this, cx| {
                    let result = cx
                        .background_executor()
                        .spawn(async move { verifier.verify(&route, &verify_host, verify_port) })
                        .await;
                    let _ = this.update(cx, |app, cx| {
                        let current = app
                            .right_panel
                            .web_pages
                            .get(async_tab_id.as_str())
                            .is_some_and(|page| {
                                page.route_epoch == epoch
                                    && app.web_tab(&async_tab_id).and_then(|tab| tab.host)
                                        == tab.host
                            });
                        if !current {
                            return; // A newer route or tab owns the answer now.
                        }
                        let live = crate::ui::desktop_lookups::HostLinks::get(cx, host_id);
                        let same_connection = match (&connection, &live) {
                            (Some(before), Some(now)) => {
                                std::sync::Arc::ptr_eq(before, now) && now.client().is_connected()
                            }
                            (None, None) => true,
                            _ => false,
                        };
                        if !same_connection || route::owning_workspace(&tab, cx).is_err() {
                            app.fail_web_route(
                                &async_tab_id,
                                "the browser Host connection changed".into(),
                                cx,
                            );
                            return;
                        }
                        match result {
                            Ok(local_port) => {
                                let forward =
                                    route::VerifiedForward::from_loopback(&loopback, local_port);
                                let verified = route::VerifiedRoute::Forwarded(forward);
                                app.apply_route_and_load(&async_tab_id, verified, load, cx);
                            }
                            Err(message) => {
                                app.fail_web_route(&async_tab_id, message, cx);
                            }
                        }
                    });
                })
                .detach();
            }
        }
    }

    /// Mounts `html_path` on `tab`'s Host via [`html_preview::HtmlPreviewService`]
    /// on the background executor, preserving `route_epoch` against concurrent
    /// reloads or navigations, and starts or navigates the surface to the
    /// loopback mount URL without leaking it into `display_url`.
    fn begin_html_preview_load(
        &mut self,
        tab_id: &str,
        tab: &WebTab,
        html_path: String,
        load: PageLoad,
        epoch: u64,
        cx: &mut Context<Self>,
    ) {
        if let Some(page) = self.right_panel.web_pages.get_mut(tab_id) {
            page.display_url = html_path.clone();
        }
        let host_id = tab
            .host
            .as_ref()
            .map(|host| host.host_id())
            .unwrap_or(HostId::LOCAL);
        let Some(host) = lyrel_ui_base::host_registry::HostRegistry::lookup(cx, host_id) else {
            if let Some(page) = self.right_panel.web_pages.get_mut(tab_id) {
                page.html_mount = None;
            }
            self.fail_web_route(
                tab_id,
                t(L10nKey::BrowserHtmlHostDisconnected).to_string(),
                cx,
            );
            return;
        };
        if !host.is_connected() {
            if let Some(page) = self.right_panel.web_pages.get_mut(tab_id) {
                page.html_mount = None;
            }
            self.fail_web_route(
                tab_id,
                t(L10nKey::BrowserHtmlHostDisconnected).to_string(),
                cx,
            );
            return;
        }
        self.set_page_guard(tab_id, surface::RouteGuard::DenyLoopback);
        if let Some(page) = self.right_panel.web_pages.get_mut(tab_id) {
            page.route_pending = true;
            page.status = PageStatus::Starting;
        }
        let service = cx
            .default_global::<html_preview::HtmlPreviewService>()
            .clone();
        let async_tab_id = tab_id.to_string();
        let expected_host = tab.host.clone();
        let mount_path = std::path::PathBuf::from(&html_path);
        let before_host = std::sync::Arc::clone(&host);
        let mount_host = std::sync::Arc::clone(&host);
        lyrel_ui_base::host_ops::HostOps::run(
            host,
            cx,
            move |_| service.mount_in_app(mount_host, &mount_path),
            move |app, result, cx| {
                let current = app
                    .right_panel
                    .web_pages
                    .get(async_tab_id.as_str())
                    .is_some_and(|page| page.route_epoch == epoch)
                    && app.web_tab(&async_tab_id).is_some_and(|current_tab| {
                        current_tab.host == expected_host
                            && current_tab.html_path.as_deref() == Some(html_path.as_str())
                    });
                if !current {
                    return;
                }
                let live_ok = lyrel_ui_base::host_registry::HostRegistry::lookup(cx, host_id)
                    .is_some_and(|now| {
                        std::sync::Arc::ptr_eq(&before_host, &now) && now.is_connected()
                    });
                if !live_ok {
                    if let Some(page) = app.right_panel.web_pages.get_mut(&async_tab_id) {
                        page.html_mount = None;
                    }
                    app.fail_web_route(
                        &async_tab_id,
                        t(L10nKey::BrowserHtmlHostDisconnected).to_string(),
                        cx,
                    );
                    return;
                }
                match result {
                    Ok(mount) => {
                        let mount_url = mount.url().to_string();
                        let mount_origin = mount.origin().to_string();
                        if let Some(page) = app.right_panel.web_pages.get_mut(&async_tab_id) {
                            page.route_pending = false;
                            page.route = RouteState::Local;
                            page.load_error = None;
                            page.html_mount = Some(mount);
                            page.display_url = html_path;
                        }
                        match load {
                            PageLoad::Start {
                                parent,
                                bounds,
                                scale,
                                site,
                            } => {
                                app.start_web_surface(
                                    &async_tab_id,
                                    parent,
                                    bounds,
                                    scale,
                                    site,
                                    mount_url,
                                    surface::RouteGuard::Origin(mount_origin),
                                    cx,
                                );
                            }
                            PageLoad::Navigate { .. } => {
                                if let Some(page) = app.right_panel.web_pages.get_mut(&async_tab_id)
                                {
                                    page.invalidate_still_frame();
                                    if let Some(surface) = page.surface.as_mut() {
                                        let guard = surface::RouteGuard::Origin(mount_origin);
                                        if surface
                                            .command(BrowserCommand::SetRouteGuard { guard })
                                            .is_ok()
                                            && surface
                                                .command(BrowserCommand::Navigate {
                                                    url: mount_url.clone(),
                                                })
                                                .is_ok()
                                        {
                                            page.current_url = mount_url;
                                            page.status = PageStatus::Starting;
                                        } else {
                                            page.status = PageStatus::Failed(
                                                "the browser surface could not navigate"
                                                    .to_string(),
                                            );
                                        }
                                    } else {
                                        page.current_url = mount_url;
                                        page.status = PageStatus::Starting;
                                    }
                                }
                                cx.notify();
                            }
                            PageLoad::External { .. } => {}
                        }
                    }
                    Err(err) => {
                        app.set_page_guard(&async_tab_id, surface::RouteGuard::DenyLoopback);
                        if let Some(page) = app.right_panel.web_pages.get_mut(&async_tab_id) {
                            page.route_pending = false;
                            page.html_mount = None;
                            page.surface_attempted = true;
                            page.display_url = html_path;
                            let message = if err.kind() == std::io::ErrorKind::NotFound {
                                t(L10nKey::BrowserHtmlMissing).to_string()
                            } else {
                                err.to_string()
                            };
                            page.status = PageStatus::Failed(message);
                        }
                        cx.notify();
                    }
                }
            },
        );
    }

    /// Stores a verified route, sends it to the surface as the approved
    /// origin, then performs the load it was verified for.
    fn apply_route_and_load(
        &mut self,
        tab_id: &str,
        verified: route::VerifiedRoute,
        load: PageLoad,
        cx: &mut Context<Self>,
    ) {
        if let Some(page) = self.right_panel.web_pages.get_mut(tab_id) {
            page.route = RouteState::Verified(verified.clone());
            page.route_pending = false;
        }
        self.deliver_web_load(tab_id, load, Some(verified), cx);
    }

    /// Sends the surface the current loopback rule. A Host-bound page with
    /// no verified route gets [`surface::RouteGuard::DenyLoopback`], so even a
    /// page-initiated reload cannot reach the old endpoint (#392).
    fn set_page_guard(&mut self, tab_id: &str, guard: surface::RouteGuard) {
        if let Some(page) = self.right_panel.web_pages.get_mut(tab_id)
            && let Some(surface) = page.surface.as_mut()
        {
            let _ = surface.command(BrowserCommand::SetRouteGuard { guard });
        }
    }

    fn fail_web_route(&mut self, tab_id: &str, message: String, cx: &mut Context<Self>) {
        self.set_page_guard(tab_id, surface::RouteGuard::DenyLoopback);
        if let Some(page) = self.right_panel.web_pages.get_mut(tab_id) {
            page.route = RouteState::Lost {
                message: message.clone(),
                last: page.route.last_forward().cloned(),
            };
            page.status = PageStatus::RouteUnverified(message);
            page.route_pending = false;
            page.surface_attempted = true;
        }
        cx.notify();
    }

    /// Performs one load with the route (if any) it was verified against.
    fn deliver_web_load(
        &mut self,
        tab_id: &str,
        load: PageLoad,
        verified: Option<route::VerifiedRoute>,
        cx: &mut Context<Self>,
    ) {
        let guard = match &verified {
            Some(verified) => surface::RouteGuard::Origin(verified.approved_origin().to_string()),
            None if self.web_tab(tab_id).is_some_and(|tab| tab.host.is_some()) => {
                surface::RouteGuard::DenyLoopback
            }
            None => surface::RouteGuard::Open,
        };
        match load {
            PageLoad::External { url } => {
                let target = if let Some(route) = &verified {
                    let Some(target) = route.forwarded_url(&url) else {
                        return;
                    };
                    target
                } else {
                    url
                };
                if matches!(
                    policy::classify(&target),
                    policy::Route::Blank | policy::Route::Deny
                ) {
                    return;
                }
                cx.open_url(&target);
            }
            PageLoad::Navigate { url } => {
                let target = if let Some(route) = &verified {
                    let Some(target) = route.forwarded_url(&url) else {
                        self.fail_web_route(
                            tab_id,
                            "the address changed during forwarding".into(),
                            cx,
                        );
                        return;
                    };
                    target
                } else {
                    url.clone()
                };
                if let Some(page) = self.right_panel.web_pages.get_mut(tab_id) {
                    page.invalidate_still_frame();
                    page.load_error = None;
                    page.display_url = url;
                    if let Some(surface) = page.surface.as_mut() {
                        if surface
                            .command(BrowserCommand::SetRouteGuard { guard })
                            .is_ok()
                            && surface
                                .command(BrowserCommand::Navigate {
                                    url: target.clone(),
                                })
                                .is_ok()
                        {
                            page.current_url = target;
                            page.status = PageStatus::Starting;
                        } else {
                            page.status = PageStatus::Failed(
                                "the browser surface could not navigate".to_string(),
                            );
                        }
                    } else {
                        page.current_url = target;
                        page.status = PageStatus::Starting;
                    }
                }
                cx.notify();
            }
            PageLoad::Start {
                parent,
                bounds,
                scale,
                site,
            } => {
                let original = self
                    .right_panel
                    .web_pages
                    .get(tab_id)
                    .map(|page| page.display_url.clone())
                    .filter(|url| !url.is_empty())
                    .or_else(|| self.web_tab(tab_id).map(|tab| tab.address.clone()))
                    .unwrap_or_default();
                let target = if let Some(route) = &verified {
                    let Some(target) = route.forwarded_url(&original) else {
                        self.fail_web_route(
                            tab_id,
                            "the address changed during forwarding".into(),
                            cx,
                        );
                        return;
                    };
                    target
                } else {
                    initial_navigate_url(&original)
                };
                self.start_web_surface(tab_id, parent, bounds, scale, site, target, guard, cx);
            }
        }
    }

    /// Creates the surface at a verified start URL (or, in automated workbench
    /// tests, the recording surface that observes the same commands).
    #[allow(clippy::too_many_arguments)]
    fn start_web_surface(
        &mut self,
        tab_id: &str,
        parent: Option<BrowserParentWindow>,
        bounds: Bounds<Pixels>,
        scale: f32,
        site: route::BrowserSiteData,
        start_url: String,
        guard: surface::RouteGuard,
        cx: &mut Context<Self>,
    ) {
        let tab_id = tab_id.to_string();
        if let Some(page) = self.right_panel.web_pages.get_mut(&tab_id) {
            page.surface_pending = true;
            page.status = PageStatus::Starting;
            page.current_url = start_url.clone();
        }
        #[cfg(any(test, feature = "test-support"))]
        if test_support::is_automated(cx) {
            if let Some(page) = self.right_panel.web_pages.get_mut(&tab_id) {
                let (sender, receiver) = smol::channel::unbounded();
                page.surface = Some(BrowserSurfaceHost::test(sender));
                page.surface_attempted = true;
                page.surface_pending = false;
                page.events = Some(browser_event_pump(tab_id.clone(), receiver, cx));
                if let Some(surface) = page.surface.as_mut() {
                    let _ = surface.command(BrowserCommand::SetRouteGuard { guard });
                    let _ = surface.command(BrowserCommand::Navigate { url: start_url });
                }
            }
            cx.notify();
            return;
        }
        let Some(parent) = parent else {
            if let Some(page) = self.right_panel.web_pages.get_mut(&tab_id) {
                page.surface_attempted = true;
                page.surface_pending = false;
                page.status =
                    PageStatus::Unsupported(t(L10nKey::BrowserSurfaceAutomated).to_string());
            }
            cx.notify();
            return;
        };
        cx.spawn(async move |this, cx| {
            let _ = this.update(cx, |view, cx| {
                let Some(page) = view.right_panel.web_pages.get_mut(&tab_id) else {
                    return;
                };
                page.surface_pending = false;
                let (sender, receiver) = smol::channel::unbounded();
                // The WebView2 host creates its profile directory on start,
                // and the WKWebView host names its data store instead of a
                // path: the panel never touches the install-local profile.
                match BrowserSurfaceHost::start(
                    parent, bounds, scale, &start_url, site, guard, sender,
                ) {
                    Ok(surface) => {
                        page.surface = Some(surface);
                        page.surface_attempted = true;
                        page.events = Some(browser_event_pump(tab_id.clone(), receiver, cx));
                    }
                    Err(error) => {
                        page.surface_attempted = true;
                        page.status = PageStatus::Failed(error.to_string());
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn browser_command(&mut self, tab_id: &str, command: BrowserCommand, cx: &mut Context<Self>) {
        if let Some(page) = self.right_panel.web_pages.get_mut(tab_id) {
            if page.route_pending {
                page.route_epoch = page.route_epoch.wrapping_add(1);
                page.route_pending = false;
            }
            page.invalidate_still_frame();
            if let Some(surface) = page.surface.as_mut()
                && let Err(error) = surface.command(command)
            {
                page.status = PageStatus::Failed(error.to_string());
            }
        }
        cx.notify();
    }

    /// The recording-surface seam for automated window tests: attaches the
    /// test host to the page so command and visibility paths run for real
    /// without starting a WebView.
    #[cfg(test)]
    pub(crate) fn attach_test_browser_surface(&mut self, tab_id: &str, cx: &mut Context<Self>) {
        let tab_id = tab_id.to_string();
        let page = self
            .right_panel
            .web_pages
            .entry(tab_id.clone())
            .or_insert_with(WebPage::new);
        let (sender, receiver) = smol::channel::unbounded();
        page.surface = Some(BrowserSurfaceHost::test(sender));
        page.surface_attempted = true;
        page.events = Some(browser_event_pump(tab_id.clone(), receiver, cx));
        page.status = PageStatus::Starting;
        cx.notify();
    }

    #[cfg(test)]
    pub(crate) fn browser_surface_commands(&self, tab_id: &str) -> Vec<BrowserCommand> {
        self.right_panel
            .web_pages
            .get(tab_id)
            .and_then(|page| page.surface.as_ref())
            .map(|surface| surface.test_commands())
            .unwrap_or_default()
    }

    /// Feeds one native event into the page it belongs to. Arrives on the
    /// event pump task, so there is no `Window`: actions that need one
    /// (opening a sibling tab is window-free; only keyboard-exit needs one)
    /// must not require it.
    pub(crate) fn on_browser_event(
        &mut self,
        tab_id: &str,
        event: BrowserEvent,
        cx: &mut Context<Self>,
    ) {
        match event {
            BrowserEvent::Ready => {
                if let Some(page) = self.right_panel.web_pages.get_mut(tab_id)
                    && matches!(page.status, PageStatus::Starting)
                {
                    page.status = PageStatus::Ready;
                }
            }
            BrowserEvent::Navigated {
                url,
                can_back,
                can_forward,
            } => {
                let html_fallback = self.web_tab(tab_id).and_then(|tab| tab.html_path);
                if let Some(page) = self.right_panel.web_pages.get_mut(tab_id) {
                    page.invalidate_still_frame();
                    page.display_url = if let Some(mount) = page.html_mount.as_ref()
                        && url.starts_with(mount.origin())
                    {
                        mount
                            .display_path_for_url(&url)
                            .or(html_fallback)
                            .unwrap_or_else(|| {
                                mount.canonical_file().to_string_lossy().into_owned()
                            })
                    } else {
                        page.route
                            .verified()
                            .and_then(|verified| verified.original_url(&url))
                            .unwrap_or_else(|| url.clone())
                    };
                    page.current_url = url;
                    page.can_back = can_back;
                    page.can_forward = can_forward;
                    page.load_error = None;
                    if !matches!(
                        page.status,
                        PageStatus::Failed(_)
                            | PageStatus::Unsupported(_)
                            | PageStatus::RouteUnverified(_)
                    ) {
                        page.status = PageStatus::Ready;
                    }
                }
            }
            BrowserEvent::Captured { request, png } => {
                if surface::is_still_frame_request(request) {
                    if let Some(page) = self.right_panel.web_pages.get_mut(tab_id) {
                        page.finish_still_frame(request, Some(png));
                    }
                } else {
                    self.preview_capture_ready(tab_id, request, png, cx);
                }
            }
            BrowserEvent::CaptureFailed { request, message } => {
                if surface::is_still_frame_request(request) {
                    if let Some(page) = self.right_panel.web_pages.get_mut(tab_id) {
                        page.finish_still_frame(request, None);
                    }
                } else {
                    self.preview_capture_failed(tab_id, request, message, cx);
                }
            }
            BrowserEvent::TitleChanged { title } => {
                if let Some(page) = self.right_panel.web_pages.get_mut(tab_id) {
                    page.live_title = title.clone();
                }
                // The strip label reads the live title; the in-memory layout
                // follows it (persisted record picks it up on the next real
                // layout write, as a restore-time hint only).
                self.retitle_web_tab(tab_id, title);
            }
            BrowserEvent::NewWindow { url } => {
                // A user-initiated new window becomes a sibling tab on the
                // same Host; the surface itself never opens a window. The
                // sibling keeps the Host's spelling of the address, so its
                // own first load verifies the route (#392).
                let page = self.right_panel.web_pages.get(tab_id);
                if let Some(mount) = page.and_then(|page| page.html_mount.as_ref())
                    && url.starts_with(mount.origin())
                {
                    if let Some(sibling_path) = mount.display_path_for_url(&url) {
                        let host_id = mount.host_id();
                        self.open_html_preview(host_id, std::path::Path::new(&sibling_path), cx);
                    }
                    return;
                }
                let url = page
                    .and_then(|page| {
                        page.route
                            .last_forward()
                            .and_then(|forward| forward.original_url(&url))
                    })
                    .unwrap_or(url);
                if let Some(tab) = self.web_tab(tab_id)
                    && policy::classify(&url) == policy::Route::Load
                {
                    self.open_web_page(&url, tab.host.clone(), cx);
                }
            }
            BrowserEvent::RouteBlocked { url } => {
                // The surface refused a loopback load the guard did not
                // approve. Translate a stale forwarded history entry back to
                // the Host's spelling, then verify it and load the new
                // endpoint. No load happens if verification fails.
                let Some(tab) = self.web_tab(tab_id) else {
                    return;
                };
                let original = self
                    .right_panel
                    .web_pages
                    .get(tab_id)
                    .map(|page| route::original_loopback_url(&url, page.route.last_forward()))
                    .unwrap_or(url);
                if tab.host.is_some() && route::is_loopback(&original) {
                    if let Some(page) = self.right_panel.web_pages.get_mut(tab_id) {
                        page.display_url = original.clone();
                    }
                    self.begin_web_load(tab_id, PageLoad::Navigate { url: original }, cx);
                }
            }
            BrowserEvent::LoadFailed { url, message } => {
                let html_fallback = self.web_tab(tab_id).and_then(|tab| tab.html_path);
                let host_bound = self.web_tab(tab_id).is_some_and(|tab| tab.host.is_some());
                if let Some(page) = self.right_panel.web_pages.get_mut(tab_id) {
                    page.display_url = if let Some(mount) = page.html_mount.as_ref()
                        && url.starts_with(mount.origin())
                    {
                        mount
                            .display_path_for_url(&url)
                            .or(html_fallback)
                            .unwrap_or_else(|| {
                                mount.canonical_file().to_string_lossy().into_owned()
                            })
                    } else {
                        page.route
                            .verified()
                            .and_then(|verified| verified.original_url(&url))
                            .unwrap_or_else(|| url.clone())
                    };
                    page.current_url = url;
                    page.load_error = Some(message.clone());
                    if host_bound && page.route.verified().is_some() {
                        // The forward may be gone. The page is covered by the
                        // reconnect overlay; nothing reloads the old local
                        // port.
                        page.route = RouteState::Lost {
                            message: message.clone(),
                            last: page.route.last_forward().cloned(),
                        };
                        page.status = PageStatus::RouteUnverified(message);
                    } else if matches!(page.status, PageStatus::Starting | PageStatus::Ready) {
                        page.status = PageStatus::Ready;
                    }
                }
                if host_bound {
                    self.set_page_guard(tab_id, surface::RouteGuard::DenyLoopback);
                }
            }
            BrowserEvent::Blocked { reason } => {
                log::info!("browser page blocked {reason}");
            }
            BrowserEvent::Failed { message } => {
                if let Some(page) = self.right_panel.web_pages.get_mut(tab_id) {
                    page.status = PageStatus::Failed(message);
                }
            }
            BrowserEvent::Closed => {
                self.right_panel.web_pages.remove(tab_id);
            }
        }
        cx.notify();
    }

    /// Every cover that gives a native child surface way to workbench UI:
    /// the coordinator's explicit sources, the Park deck's own fan/drop
    /// state, the inbox popover, a deferred popup (dropdown, select,
    /// combobox), a focused `PopupMenu` (context menus), and a dialog or
    /// sheet registered through `lyrel_ui_base::native_surface`.
    fn native_surface_cover_registered(
        &self,
        window: gpui_kit::WindowId,
        cx: &gpui_kit::App,
    ) -> bool {
        self.surface_coordinator.is_covered()
            || self
                .panel_slide(crate::ui::panel_slide::PanelBoundary::Right)
                .is_some()
            || self.parked_deck.read(cx).covers_native_surface(cx)
            || self.notification_bell.read(cx).is_open()
            || gpui_kit::base::GlobalState::is_in_deferred_context(cx)
            || lyrel_ui_base::native_surface::is_covered(window, cx)
    }

    fn native_surface_covered(&self, window: &Window, cx: &gpui_kit::App) -> bool {
        self.native_surface_cover_registered(window.window_handle().window_id(), cx)
            || crate::ui::surface_coordinator::popup_menu_open(window, cx)
    }

    /// The paint hook's target: records bounds, starts the surface on first
    /// paint, and syncs visibility (covering overlays included) while painted.
    /// Only the front detail paints its anchor — the document column renders
    /// no other browser shell, so a buried page never reaches this hook.
    pub(crate) fn browser_page_prepared(
        &mut self,
        tab_id: &str,
        bounds: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.active_browser_detail() != Some(tab_id) {
            return;
        }
        let scale = window.scale_factor();
        let window_id = window.window_handle().window_id();
        let covered = self.native_surface_covered(window, cx);
        let tab_id_owned = tab_id.to_string();
        let fallback_url = self
            .web_tab(&tab_id_owned)
            .map(|tab| tab.html_path.unwrap_or(tab.address))
            .unwrap_or_default();
        let page = self
            .right_panel
            .web_pages
            .entry(tab_id_owned.clone())
            .or_insert_with(WebPage::new);
        if page.display_url.is_empty() && !fallback_url.is_empty() {
            page.display_url = fallback_url;
        }
        page.last_paint = std::time::Instant::now();
        page.window_id = Some(window_id);
        let changed = page.bounds.is_none_or(|old| {
            f32::from(old.origin.x - bounds.origin.x).abs() > 0.5
                || f32::from(old.origin.y - bounds.origin.y).abs() > 0.5
                || f32::from(old.size.width - bounds.size.width).abs() > 0.5
                || f32::from(old.size.height - bounds.size.height).abs() > 0.5
        });
        page.bounds = Some(bounds);
        // A transition can briefly leave no viewport to mount into. Let the
        // next layout pass start the surface once it has usable bounds.
        if bounds.size.width <= px(1.) || bounds.size.height <= px(1.) {
            page.sync_visibility(false, covered);
            return;
        }
        if let Some(surface) = page.surface.as_mut() {
            if changed {
                surface.set_bounds(bounds, scale);
            }
            page.sync_visibility(true, covered);
            return;
        }
        if page.surface_pending || page.surface_attempted || page.route_pending {
            return;
        }
        let Some(tab) = self.web_tab(tab_id) else {
            return;
        };
        let Some(site) = route::browser_site_data(&tab) else {
            if let Some(page) = self.right_panel.web_pages.get_mut(tab_id) {
                page.surface_attempted = true;
                page.status =
                    PageStatus::Unsupported(t(L10nKey::BrowserSurfaceNoDataDir).to_string());
            }
            cx.notify();
            return;
        };
        #[cfg(any(test, feature = "test-support"))]
        let automated = test_support::is_automated(cx);
        #[cfg(not(any(test, feature = "test-support")))]
        let automated = false;
        let parent = if automated {
            None
        } else {
            match BrowserParentWindow::from_window(window) {
                Ok(parent) => Some(parent),
                Err(error) => {
                    if let Some(page) = self.right_panel.web_pages.get_mut(tab_id) {
                        page.surface_attempted = true;
                        page.status = PageStatus::Unsupported(error.to_string());
                    }
                    return;
                }
            }
        };
        let tab_id = tab_id_owned;
        self.begin_web_load(
            &tab_id,
            PageLoad::Start {
                parent,
                bounds,
                scale,
                site,
            },
            cx,
        );
    }

    /// Syncs retained native pages after selection, document or cover
    /// changes. A recently painted page can come back after a cover lifts,
    /// retaining its scroll position, form draft, and focus. The front
    /// detail is the only page that may stay visible — a page whose document
    /// is dismissed, and every surface when no detail is front, hides here
    /// no matter what the panel does; switching tools or collapsing the
    /// panel never reaches this path as a reason to hide.
    pub(crate) fn sync_browser_surfaces(&mut self, cx: &mut Context<Self>) {
        const VISIBLE_GRACE: std::time::Duration = std::time::Duration::from_millis(1500);
        if self.right_panel.web_pages.is_empty() {
            return;
        }
        // The front detail is the only page that may stay visible. Borrowed
        // straight from the tab: this runs on selection, document and cover
        // changes, and the answer is a compare, not a copy.
        let active_web = self
            .tabs
            .get(self.tabs.active_index())
            .and_then(|tab| tab.browser_detail());
        let now = std::time::Instant::now();
        // Covers are window-scoped (a dialog in another window must not hide
        // this page); resolve each painted window once per pass.
        let mut covered_by_window: std::collections::HashMap<gpui_kit::WindowId, bool> =
            std::collections::HashMap::new();
        for page in self.right_panel.web_pages.values() {
            if let Some(window) = page.window_id {
                covered_by_window.entry(window).or_insert_with(|| {
                    self.native_surface_cover_registered(window, cx)
                        || crate::ui::surface_coordinator::popup_menu_open_for_window(window, cx)
                });
            }
        }
        let mut notify = false;
        for (tab_id, page) in self.right_panel.web_pages.iter_mut() {
            let selected = active_web == Some(tab_id.as_str());
            let painted = selected
                && now.duration_since(page.last_paint) < VISIBLE_GRACE
                && page.bounds.is_some_and(|bounds| {
                    f32::from(bounds.size.width) > 1. && f32::from(bounds.size.height) > 1.
                });
            let covered = page
                .window_id
                .and_then(|window| covered_by_window.get(&window).copied())
                .unwrap_or(false);
            if page.sync_visibility(painted, covered) {
                notify = true;
            }
            // Refresh the anchor while the page is visible.
            if painted && !covered {
                cx.notify();
            }
        }
        if notify {
            cx.notify();
        }
    }

    /// Hides every native page the front detail does not name — including
    /// when no page is front at all, so a dismissed document or a tab
    /// teardown leaves no orphan HWND behind. The window composition runs
    /// this at its top each frame; the steady state costs one id comparison
    /// per page and never notifies.
    pub(crate) fn hide_inactive_browser_surfaces(&mut self, cx: &mut Context<Self>) {
        if self.right_panel.web_pages.is_empty() {
            return;
        }
        // Borrowed from the tab, not copied: this runs at the top of every
        // window composition, and the steady state must stay one compare
        // per page.
        let active = self
            .tabs
            .get(self.tabs.active_index())
            .and_then(|tab| tab.browser_detail());
        let mut changed = false;
        for (tab_id, page) in self.right_panel.web_pages.iter_mut() {
            if active == Some(tab_id.as_str()) || !page.surface_visible {
                continue;
            }
            let covered = page.cover_synced.unwrap_or(false);
            changed |= page.sync_visibility(false, covered);
        }
        if changed {
            cx.notify();
        }
    }

    /// Docked pages leave the lower-right toast stack on the terminal side.
    /// Filled pages reserve space at the edge of their viewport while toasts exist.
    pub(crate) fn browser_toast_clearance(&self, window: &Window, cx: &gpui_kit::App) -> Pixels {
        if self.active_browser_detail().is_none() {
            return px(8.);
        }
        match self.document_dock_px(window, cx) {
            Some(width) => px(width + self.right_panel_final_px(window, cx) + 8.),
            None => px(8.),
        }
    }

    /// Applies [`Self::browser_toast_clearance`] to the shared notification
    /// settings. Called from the window's render before the notification
    /// layer paints, so this window's toasts use this window's panel width.
    pub(crate) fn apply_toast_clearance(&self, window: &Window, cx: &mut gpui_kit::App) {
        let right = self.browser_toast_clearance(window, cx);
        let theme = gpui_kit::component::Theme::global_mut(cx);
        theme.notification.margins.right = right;
    }

    /// Projects the selected page address without overwriting an in-progress edit.
    fn sync_browser_address_input(
        &mut self,
        tab_id: &str,
        url: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(chrome) = self.browser_chrome.as_mut() else {
            return;
        };
        if chrome
            .displayed
            .as_ref()
            .is_some_and(|(id, address)| id == tab_id && address == url)
        {
            return;
        }
        chrome.displayed = Some((tab_id.to_owned(), url.to_owned()));
        chrome.input.update(cx, |input, cx| {
            if input.value() != url {
                input.set_value(url, window, cx);
            }
        });
    }

    fn new_web_tab_id(&mut self) -> String {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        // A time component keeps ids unique across restarts of this process;
        // the counter separates ids minted in the same tick.
        format!(
            "web-{:x}-{:x}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
            NEXT.fetch_add(1, Ordering::Relaxed),
        )
    }
}

/// The event pump every surface (native or test) feeds: one task per page,
/// draining the channel until the page (or the app) goes away.
fn browser_event_pump(
    tab_id: String,
    receiver: smol::channel::Receiver<BrowserEvent>,
    cx: &mut Context<LyrelApp>,
) -> gpui_kit::Task<()> {
    cx.spawn(async move |this, cx| {
        while let Ok(event) = receiver.recv().await {
            let tab_id = tab_id.clone();
            let Ok(alive) = this.update(cx, |view, cx| {
                view.on_browser_event(&tab_id, event, cx);
                true
            }) else {
                break;
            };
            if !alive {
                break;
            }
        }
    })
}

/// The URL a tab's surface starts on: the original address for loadable
/// routes, the blank page for everything else (a denied scheme must not
/// reach the surface at all — the host would refuse it anyway).
pub(crate) fn initial_navigate_url(address: &str) -> String {
    match policy::classify(address) {
        policy::Route::Load => address.to_string(),
        policy::Route::Blank | policy::Route::Deny => "about:blank".to_string(),
    }
}

/// Whether a tab's page is currently showing its HTML preview (either mounted
/// at the preview origin or not yet navigated away to an external http(s) URL).
fn page_showing_html_preview(page: Option<&WebPage>) -> bool {
    page.is_none_or(|page| {
        if let Some(mount) = page.html_mount.as_ref() {
            page.current_url.is_empty() || page.current_url.starts_with(mount.origin())
        } else {
            policy::classify(&page.display_url) != policy::Route::Load
        }
    })
}

/// Compares two HTML preview paths for tab deduplication on `host_id`.
fn html_paths_match(host_id: HostId, a: &str, b: &str) -> bool {
    if host_id.is_local() && cfg!(windows) {
        let norm = |s: &str| {
            let spelled =
                lyrel_core::core::path_spelling::spelling_on(host_id, std::path::Path::new(s));
            let replaced = spelled.to_string_lossy().replace('/', "\\");
            let trimmed = replaced.trim_end_matches('\\');
            trimmed.to_lowercase()
        };
        norm(a) == norm(b)
    } else {
        a == b
    }
}

/// The Host label the chrome shows: the bound Host's route label, or the
/// local-machine wording for a page that lives here.
pub(crate) fn browser_host_label(tab: &WebTab) -> gpui_kit::SharedString {
    match &tab.host {
        Some(host) => {
            let name = host
                .via
                .as_ref()
                .map(|via| via.label())
                .unwrap_or_else(|| host.target.to_string());
            t_fmt(L10nKey::BrowserHostRemote, &[("host", name.as_str())]).into()
        }
        None => t(L10nKey::BrowserHostLocal).into(),
    }
}

fn normalized_web_address(address: &str) -> String {
    url::Url::parse(address)
        .map(|url| url.as_str().to_string())
        .unwrap_or_else(|_| address.to_string())
}

impl LyrelApp {
    /// The front browser page as a document: the shared document header —
    /// the Host name, a dot and the page title on the left, the Dock / Fill
    /// switch and the standard close tile on the right — over the page
    /// chrome (toolbar, the agent source row, the explicit-share composer)
    /// and the native paint anchor. Renders only while the Browser detail
    /// is the front document, in both chromes: the native surface shows
    /// exactly one page, and a buried browser shell would paint a stale
    /// frame over the document that actually leads.
    pub(crate) fn render_browser_document(
        &mut self,
        chrome: DocumentChrome,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        use gpui_kit::component::button::ButtonCustomVariant;
        let tab_id = self.active_browser_detail()?.to_string();
        let tab = self.web_tab(&tab_id)?;
        let toast_lane = if matches!(chrome, DocumentChrome::Fill)
            && !gpui_kit::component::WindowExt::notifications(window, cx).is_empty()
        {
            cx.theme().notification.width + px(16.)
        } else {
            px(0.)
        };
        let theme = cx.theme();
        let (muted, border, foreground) = (theme.muted_foreground, theme.border, theme.foreground);
        let (can_back, can_forward, status, display_url, load_error, title) = self
            .right_panel
            .web_pages
            .get(&tab.id)
            .map(|page| {
                (
                    page.can_back,
                    page.can_forward,
                    page.status.clone(),
                    page.display_url.clone(),
                    page.load_error.clone(),
                    page.display_title(&tab),
                )
            })
            .unwrap_or_else(|| {
                (
                    false,
                    false,
                    PageStatus::Idle,
                    String::new(),
                    None,
                    tab.address.clone(),
                )
            });
        // A Host-bound page keeps the Host's spelling in the address bar: the
        // forwarded endpoint is an implementation detail (#392).
        let address = if display_url.is_empty() {
            tab.html_path.clone().unwrap_or_else(|| tab.address.clone())
        } else {
            display_url
        };
        // A successful share asks the next render to clear the field; this is
        // the first moment a `Window` is available to do it.
        if let Some(feedback) = self
            .right_panel
            .web_pages
            .get_mut(&tab.id)
            .and_then(|page| page.feedback.as_mut())
            && feedback.clear_draft
        {
            feedback.clear_draft = false;
            let input = feedback.input.clone();
            input.update(cx, |state, cx| state.set_value("", window, cx));
        }
        let source_label = self.agent_preview_source_label(&tab, cx);
        let source_tip = tab.source.as_ref().map(|source| {
            t_fmt(
                L10nKey::BrowserPreviewSourceTip,
                &[("pane", &format!("%{}", source.pane))],
            )
        });
        let feedback_view = self
            .right_panel
            .web_pages
            .get(&tab.id)
            .and_then(|page| page.feedback.as_ref())
            .filter(|feedback| feedback.open)
            .map(|feedback| {
                (
                    feedback.input.clone(),
                    feedback.pending,
                    feedback.error.clone(),
                    feedback.notice.clone(),
                )
            });
        let covered = self.native_surface_covered(window, cx);

        if self.browser_chrome.is_none() {
            self.browser_chrome = Some(BrowserChrome::new(window, cx));
        }
        self.sync_browser_address_input(&tab.id, &address, window, cx);
        let address_input = self
            .browser_chrome
            .as_ref()
            .expect("chrome just ensured")
            .input
            .clone();

        let icon_button =
            |id: &'static str,
             icon: IconName,
             tip: L10nKey,
             enabled: bool,
             action: fn(&mut LyrelApp, &str, &mut Context<LyrelApp>)| {
                let tab_id = tab.id.clone();
                Button::new(id)
                    .icon(Icon::new(icon))
                    .custom(
                        ButtonCustomVariant::new(cx)
                            .color(cx.theme().transparent)
                            .foreground(if enabled { foreground } else { muted })
                            .hover(cx.theme().foreground.opacity(0.06)),
                    )
                    .with_size(px(13. / crate::ui::window_chrome::BUTTON_ICON_SCALE))
                    .w(px(24.))
                    .h(px(24.))
                    .disabled(!enabled)
                    .tooltip(t(tip))
                    .accessibility_label(t(tip))
                    .on_click(cx.listener(move |this, _, _window, cx| {
                        action(this, &tab_id, cx);
                    }))
            };

        let share_button = tab.source.as_ref().map(|_| {
            let tab_id = tab.id.clone();
            Button::new("browser-preview-share")
                .label(t(L10nKey::BrowserPreviewShare))
                .small()
                .flex_shrink_0()
                .tooltip(t(L10nKey::BrowserPreviewShareTip))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.browser_preview_feedback_open(&tab_id, window, cx);
                }))
        });

        let toolbar = h_flex()
            .id("browser-chrome")
            .test_support()
            .flex_none()
            .w_full()
            .items_center()
            .gap_1()
            .px_1()
            .py_1()
            .child(icon_button(
                "browser-back",
                IconName::ArrowLeft,
                L10nKey::BrowserBack,
                can_back,
                LyrelApp::browser_back,
            ))
            .child(icon_button(
                "browser-forward",
                IconName::ArrowRight,
                L10nKey::BrowserForward,
                can_forward,
                LyrelApp::browser_forward,
            ))
            .child(icon_button(
                "browser-reload",
                IconName::RotateCw,
                L10nKey::BrowserReload,
                true,
                LyrelApp::browser_reload,
            ))
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .h(px(24.))
                    .px_2()
                    .rounded(px(6.))
                    .bg(cx.theme().muted)
                    .items_center()
                    .child(
                        div().flex_1().min_w_0().child(
                            Input::new(&address_input)
                                .appearance(false)
                                .xsmall()
                                .cleanable(true),
                        ),
                    ),
            )
            .child(icon_button(
                "browser-external",
                IconName::ExternalLink,
                L10nKey::BrowserOpenExternal,
                true,
                |this, tab_id, cx| this.browser_open_external(tab_id, cx),
            ));

        // The agent source row keeps the pane chip and the share affordance,
        // and only an agent preview has one: an ordinary page shows no row at
        // all. The Host chip itself moved into the document header.
        let source_row = tab.source.as_ref().map(|_| {
            h_flex()
                .id("browser-preview-source-row")
                .test_support()
                .flex_none()
                .min_w_0()
                .w_full()
                .gap_1()
                .px_1()
                .pb_1()
                .border_b_1()
                .border_color(border)
                .children(source_label.map(|label| {
                    div()
                        .id("browser-preview-source")
                        .test_support()
                        .flex_shrink_0()
                        .px_2()
                        .h(px(20.))
                        .flex()
                        .items_center()
                        .rounded(px(5.))
                        .bg(cx.theme().muted)
                        .text_xs()
                        .text_color(muted)
                        .when_some(source_tip, |chip, tip| {
                            chip.tooltip(move |window, cx| {
                                gpui_kit::component::tooltip::Tooltip::new(tip.clone())
                                    .build(window, cx)
                            })
                        })
                        .child(label)
                }))
                .children(share_button)
        });

        let tab_id_bounds = tab.id.clone();
        let viewport = div()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .relative()
            .on_children_prepainted({
                let app = cx.entity().downgrade();
                move |bounds, window, cx| {
                    if let Some(bounds) = bounds.first().copied() {
                        let app = app.clone();
                        let tab_id = tab_id_bounds.clone();
                        window.defer(cx, move |window, cx| {
                            let _ = app.update(cx, |app, cx| {
                                app.browser_page_prepared(&tab_id, bounds, window, cx);
                            });
                        });
                    }
                }
            })
            // Inset the measured child against the resolved viewport. Percentage
            // height can resolve to zero inside the surrounding flex column.
            .child(div().absolute().inset_0())
            .id("browser-surface-anchor")
            .test_support();

        let feedback_row = feedback_view.map(|(feedback_input, pending, error, notice)| {
            let tab_id = tab.id.clone();
            let cancel_id = tab.id.clone();
            let pane = tab
                .source
                .as_ref()
                .map(|source| format!("%{}", source.pane))
                .unwrap_or_default();
            let send_label: gpui_kit::SharedString = if pending {
                t(L10nKey::BrowserPreviewSending).to_string().into()
            } else {
                t_fmt(L10nKey::BrowserPreviewSend, &[("pane", &pane)]).into()
            };
            v_flex()
                .id("browser-preview-feedback")
                .test_support()
                .flex_none()
                .w_full()
                .gap_1()
                .px_2()
                .py_1()
                .border_b_1()
                .border_color(border)
                .child(
                    h_flex()
                        .w_full()
                        .items_center()
                        .gap_1()
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .h(px(26.))
                                .px_2()
                                .rounded(px(6.))
                                .bg(cx.theme().muted)
                                .flex()
                                .items_center()
                                .child(
                                    div().flex_1().min_w_0().child(
                                        Input::new(&feedback_input)
                                            .appearance(false)
                                            .xsmall()
                                            .cleanable(false),
                                    ),
                                ),
                        )
                        .child(
                            Button::new("browser-preview-send")
                                .label(send_label)
                                .small()
                                .disabled(pending)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.browser_preview_feedback_send(&tab_id, window, cx);
                                })),
                        )
                        .child(
                            Button::new("browser-preview-cancel")
                                .label(t(L10nKey::BrowserPreviewCancel))
                                .small()
                                .on_click(cx.listener(move |this, _, _window, cx| {
                                    this.browser_preview_feedback_cancel(&cancel_id, cx);
                                })),
                        ),
                )
                .when_some(error, |col, error| {
                    col.child(
                        div()
                            .id("browser-preview-feedback-error")
                            .test_support()
                            .max_w_full()
                            .text_xs()
                            .text_color(cx.theme().danger)
                            .child(error),
                    )
                })
                .when_some(notice, |col, notice| {
                    col.child(
                        div()
                            .id("browser-preview-feedback-notice")
                            .test_support()
                            .text_xs()
                            .text_color(muted)
                            .child(notice),
                    )
                })
        });

        let overlay = if covered
            && load_error.is_none()
            && !matches!(status, PageStatus::Failed(_) | PageStatus::Unsupported(_))
        {
            let frame = self
                .right_panel
                .web_pages
                .get(&tab.id)
                .and_then(|page| page.still_frame.clone());
            Some(browser_still_frame(&tab.id, &title, &address, frame, cx))
        } else {
            browser_overlay(
                &tab.id,
                tab.html_path.is_some(),
                &status,
                &address,
                load_error.as_deref(),
                cx,
            )
        };

        let host_label = browser_host_label(&tab);
        let header = h_flex()
            .id("browser-document-header")
            .test_support()
            .flex_shrink_0()
            .min_w_0()
            // The same strip geometry the other document headers keep, so
            // the layout switch lands on the same line in all three.
            .title_strip_row(HAIRLINE_W, hairline(cx))
            .pl(px(CONTENT_INSET))
            .pr(px(crate::ui::window_chrome::tile_trailing_inset()
                + (crate::ui::window_chrome::TILE_SIZE - CLOSE_TILE) / 2.))
            .gap_2()
            .items_center()
            .children(self.render_document_terminal_button(chrome, window, cx))
            // The title names the document the way a GitHub detail's does:
            // the Host the page lives on, a dot, then the page title. The
            // Host name is capped so a long remote alias cannot push the
            // title, the Dock / Fill switch or Close out at a narrow window.
            .child(
                h_flex()
                    .id("browser-document-title")
                    .test_support()
                    .flex_1()
                    .min_w_0()
                    .items_center()
                    .gap(px(6.))
                    .child(
                        div()
                            .id("browser-host")
                            .test_support()
                            .flex_none()
                            .min_w_0()
                            .max_w(px(160.))
                            .truncate()
                            .text_size(rems(TEXT))
                            .text_color(muted)
                            .tooltip({
                                let label = host_label.clone();
                                move |window, cx| {
                                    gpui_kit::component::tooltip::Tooltip::new(label.clone())
                                        .build(window, cx)
                                }
                            })
                            .child(host_label),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_size(rems(TEXT))
                            .text_color(muted)
                            .child("·"),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(rems(TEXT))
                            .font_weight(gpui_kit::FontWeight::MEDIUM)
                            .text_color(foreground)
                            .child(title),
                    ),
            )
            .child(
                div()
                    .occlude()
                    .flex_shrink_0()
                    .child(self.document_layout_switch(chrome, cx)),
            )
            .child(
                div().occlude().flex_shrink_0().child(
                    crate::ui::window_chrome::chrome_tile_sized(
                        Button::new("browser-detail-close").icon(Icon::new(IconName::Close)),
                        CLOSE_TILE,
                        crate::ui::tab_strip::RAIL_TILE_GLYPH,
                        false,
                        cx,
                    )
                    .rounded(px(crate::ui::tab_strip::RAIL_TILE_RADIUS))
                    .tooltip(t(L10nKey::Close))
                    .accessibility_label(t(L10nKey::Close))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.close_browser_detail(window, cx);
                    })),
                ),
            );

        let body = v_flex()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .child(toolbar)
            .children(source_row)
            .children(feedback_row)
            .child(viewport.mr(toast_lane).children(overlay));

        let shell = v_flex()
            .id("browser-detail-document")
            .test_support()
            .text_color(cx.theme().foreground)
            .child(header)
            .child(body);
        // The fill chrome covers the workspace, so it stops window
        // translucency and the theme backdrop the way the diff and GitHub
        // shells do. Docked, the document column has already painted the
        // surface.
        let shell = match chrome {
            DocumentChrome::Fill => shell
                .absolute()
                .inset_0()
                .occlude()
                .bg(
                    match cx.try_global::<lyrel_ui_base::presets::ActiveBackground>() {
                        Some(background) => crate::ui::theme::window_background_opaque(background),
                        None => cx.theme().background.alpha(1.0).into(),
                    },
                )
                .children(crate::ui::app::overlay_surface_layers(cx)),
            DocumentChrome::Dock => shell.size_full().min_w_0(),
        };
        Some(shell.into_any_element())
    }
}

/// The loading or failure state shown over the page area.
fn browser_overlay(
    tab_id: &str,
    is_html_preview: bool,
    status: &PageStatus,
    address: &str,
    load_error: Option<&str>,
    cx: &mut Context<LyrelApp>,
) -> Option<AnyElement> {
    let muted = cx.theme().muted_foreground;
    let message = match status {
        PageStatus::Ready => load_error.map(str::to_owned),
        PageStatus::Idle | PageStatus::Starting => Some(t(L10nKey::BrowserStarting).to_string()),
        PageStatus::RouteUnverified(message) => Some(message.clone()),
        PageStatus::Failed(message) | PageStatus::Unsupported(message) => Some(message.clone()),
    }?;
    let route_unverified = matches!(status, PageStatus::RouteUnverified(_));
    let tab_id = tab_id.to_string();
    Some(
        v_flex()
            .id(format!("browser-overlay-{tab_id}"))
            .test_support()
            .absolute()
            .inset_0()
            .items_center()
            .justify_center()
            .gap_1()
            .bg(cx.theme().background)
            .child(
                div()
                    .id("browser-overlay-message")
                    .test_support()
                    .max_w_80()
                    .text_sm()
                    .text_color(muted)
                    .child(message),
            )
            .when(
                load_error.is_some() || route_unverified || matches!(status, PageStatus::Failed(_)),
                |col| {
                    let col = col.child(
                        div()
                            .id("browser-overlay-address")
                            .test_support()
                            .text_xs()
                            .text_color(muted)
                            .child(address.to_string()),
                    );
                    let col = if route_unverified {
                        let reconnect_id = tab_id.clone();
                        col.child(
                            Button::new(format!("browser-overlay-reconnect-{tab_id}"))
                                .label(t(L10nKey::BrowserReconnect))
                                .small()
                                .on_click(cx.listener(move |this, _, _window, cx| {
                                    this.browser_reconnect(&reconnect_id, cx);
                                })),
                        )
                    } else if is_html_preview {
                        let retry_id = tab_id.clone();
                        col.child(
                            Button::new(format!("browser-overlay-retry-{tab_id}"))
                                .label(t(L10nKey::BrowserRetry))
                                .small()
                                .on_click(cx.listener(move |this, _, _window, cx| {
                                    this.browser_reconnect(&retry_id, cx);
                                })),
                        )
                    } else {
                        col
                    };
                    col.when(!is_html_preview, |col| {
                        let open_id = tab_id.clone();
                        col.child(
                            Button::new(format!("browser-overlay-open-{tab_id}"))
                                .label(t(L10nKey::BrowserOpenExternal))
                                .small()
                                .on_click(cx.listener(move |this, _, _window, cx| {
                                    this.browser_open_external(&open_id, cx);
                                })),
                        )
                    })
                },
            )
            .into_any_element(),
    )
}

/// The selected page's last captured frame. A page that has not produced a
/// frame yet keeps its own title and address visible while covered.
fn browser_still_frame(
    tab_id: &str,
    title: &str,
    address: &str,
    frame: Option<std::sync::Arc<gpui_kit::Image>>,
    cx: &mut Context<LyrelApp>,
) -> AnyElement {
    if let Some(frame) = frame {
        return div()
            .id(format!("browser-still-frame-{tab_id}"))
            .test_support()
            .absolute()
            .inset_0()
            .overflow_hidden()
            .bg(cx.theme().background)
            .child(
                div()
                    .id("browser-still-frame-image")
                    .test_support()
                    .size_full()
                    .child(
                        gpui_kit::img(frame)
                            .size_full()
                            .object_fit(gpui_kit::ObjectFit::Contain),
                    ),
            )
            .into_any_element();
    }
    let muted = cx.theme().muted_foreground;
    let foreground = cx.theme().foreground;
    let border = cx.theme().border;
    v_flex()
        .id(format!("browser-still-frame-{tab_id}"))
        .test_support()
        .absolute()
        .inset_0()
        .items_center()
        .justify_center()
        .p_4()
        .bg(cx.theme().background)
        .child(
            v_flex()
                .id("browser-still-frame-card")
                .test_support()
                .w_full()
                .max_w_80()
                .min_w_0()
                .gap_1()
                .p_3()
                .rounded_lg()
                .border_1()
                .border_color(border)
                .bg(cx.theme().muted.opacity(0.35))
                .child(
                    h_flex()
                        .min_w_0()
                        .items_center()
                        .gap_2()
                        .child(Icon::new(IconName::Globe).size_4().text_color(muted))
                        .child(
                            div()
                                .id("browser-still-frame-title")
                                .test_support()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_sm()
                                .text_color(foreground)
                                .child(title.to_string()),
                        ),
                )
                .child(
                    div()
                        .id("browser-still-frame-address")
                        .test_support()
                        .min_w_0()
                        .truncate()
                        .text_xs()
                        .text_color(muted)
                        .child(address.to_string()),
                )
                .child(
                    div()
                        .id("browser-still-frame-hint")
                        .test_support()
                        .text_xs()
                        .text_color(muted)
                        .child(t(L10nKey::BrowserConcealedHint)),
                ),
        )
        .into_any_element()
}

/// The automated-test seam, scoped per `App` exactly like the diagram
/// surface's: marked by the whole-workbench harness, read by
/// `browser_page_prepared` to refuse a real WebView.
#[cfg(any(test, feature = "test-support"))]
pub(crate) mod test_support {
    use gpui_kit::App;

    pub(crate) struct AutomatedBrowserSurface;

    impl gpui_kit::Global for AutomatedBrowserSurface {}

    pub(crate) fn mark_automated(cx: &mut App) {
        cx.set_global(AutomatedBrowserSurface);
    }

    pub(crate) fn is_automated(cx: &App) -> bool {
        cx.try_global::<AutomatedBrowserSurface>().is_some()
    }
}
