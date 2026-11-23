//! The Side Panel browser (#391): local web pages as first-class tabs in the
//! right panel's strip, beside the single-instance tools. This module owns
//! the tab model, the live page runtime (native surface lifecycle, exactly
//! the diagram surface's paint-driven visibility pattern), the page chrome
//! (back, forward, reload, address, Host, Open in System Browser), and the
//! entry points that open a page: the `+` menu, Search Everywhere, and a
//! local terminal link's URL.
//!
//! What is deliberately not here: remote loopback forwarding and its
//! reconnect flow (#392), and agent-requested previews with screenshot and
//! feedback (#393). Both build on this module's tab model.
//!
//! The native surface contract lives in [`surface`]; the navigation policy
//! in [`policy`]. There is no page → workbench bridge — the only things a
//! page can ask for are the events the surface defines, and the only ways in
//! are the commands.

pub(crate) mod policy;
pub(crate) mod surface;

#[cfg(test)]
#[path = "browser/tests.rs"]
mod tests;

#[cfg(test)]
#[path = "browser/overlay_tests.rs"]
mod overlay_tests;

pub(crate) use surface::{BrowserCommand, BrowserEvent, BrowserParentWindow, BrowserSurfaceHost};

use gpui_kit::TestSupportExt as _;
use gpui_kit::base::Disableable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::{
    AnyElement, Bounds, Context, Entity, Pixels, Subscription, Window, div, prelude::*, px,
};
use lyrel_ui_base::i18n::{L10nKey, t, t_fmt};

use crate::ui::app::LyrelApp;
use crate::ui::right_panel::{PanelTab, ToolLayout, WebTab};

/// One page's live runtime, keyed by its durable tab id in
/// [`crate::ui::right_panel::RightPanelState::web_pages`]. Everything here is
/// panel state: nothing persists but the tab identity in the tool layout.
pub(crate) struct WebPage {
    surface: Option<BrowserSurfaceHost>,
    events: Option<gpui_kit::Task<()>>,
    pub(crate) status: PageStatus,
    /// The URL the page is actually on — what the address bar shows and what
    /// the surface last reported. The tab's original address outlives it in
    /// the layout.
    pub(crate) current_url: String,
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
    pub(crate) load_error: Option<String>,
}

/// What the chrome renders over the page area.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PageStatus {
    /// Never painted yet — the page area has no bounds to start a surface in.
    Idle,
    Starting,
    Ready,
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
            load_error: None,
        }
    }

    /// The label the strip shows: the live title once the page reports one,
    /// the current URL on the way there, the original address as the last
    /// resort. Never empty — the tab must read as something.
    pub(crate) fn display_title(&self, tab: &WebTab) -> String {
        if !self.live_title.is_empty() {
            return self.live_title.clone();
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

    /// Whether the page should paint the bounded still frame instead of its
    /// live native surface.
    pub(crate) fn still_frame(&self) -> bool {
        !self.surface_visible && self.surface.is_some()
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
    /// Opens `address` as a new web tab on this workspace and selects it.
    /// The entry points — the `+` menu, Search Everywhere, a terminal link —
    /// all land here; the layout API is what keeps the strip, the mirror and
    /// the persisted record in one write. The surface itself starts lazily
    /// on the tab's first paint.
    pub(crate) fn open_web_page(
        &mut self,
        address: &str,
        host: Option<lyrel_core::core::session::RemoteRef>,
        cx: &mut Context<Self>,
    ) {
        let tab = WebTab {
            id: self.new_web_tab_id(),
            address: address.to_string(),
            title: String::new(),
            host,
        };
        let tab_id = tab.id.clone();
        let mut layout = self.tool_layout();
        layout.show_web(tab);
        self.right_panel
            .web_pages
            .entry(tab_id)
            .or_insert_with(WebPage::new);
        self.store_tool_layout(&layout, cx);
        cx.notify();
    }

    /// The explicit system-browser action on the page chrome (#382): hands
    /// the page the surface is actually on to the OS. Remote loopback pages
    /// go through the verified forwarded endpoint — that route is #392's;
    /// until it lands, a remote-bound tab's external open uses the original
    /// address and never a same-numbered local port.
    pub(crate) fn browser_open_external(&mut self, tab_id: &str, cx: &mut Context<Self>) {
        let url = {
            let page_url = self
                .right_panel
                .web_pages
                .get(tab_id)
                .map(|page| page.current_url.clone())
                .unwrap_or_default();
            if page_url.is_empty() {
                let layout = self.tool_layout();
                layout
                    .order
                    .iter()
                    .find_map(|entry| match entry {
                        PanelTab::Web(tab) if tab.id == tab_id => Some(tab.address.clone()),
                        _ => None,
                    })
                    .unwrap_or_default()
            } else {
                page_url
            }
        };
        if matches!(
            policy::classify(&url),
            policy::Route::Blank | policy::Route::Deny
        ) {
            return;
        }
        cx.open_url(&url);
    }

    /// Enter submits the address field: complete what was typed, and either
    /// navigate the selected page or refuse the route before the surface
    /// ever sees it.
    pub(crate) fn browser_address_submitted(&mut self, cx: &mut Context<Self>) {
        let layout = self.tool_layout();
        let Some(tab_id) = layout_active_web(&layout) else {
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
        self.navigate_web_tab(&tab_id, &url, cx);
    }

    /// Navigates the page's surface and records what the page is on. The
    /// layout's original address does not move — it is the tab's identity.
    pub(crate) fn navigate_web_tab(&mut self, tab_id: &str, url: &str, cx: &mut Context<Self>) {
        let command = BrowserCommand::Navigate {
            url: url.to_string(),
        };
        if let Some(page) = self.right_panel.web_pages.get_mut(tab_id) {
            page.load_error = None;
            match page.surface.as_mut() {
                Some(surface) => match surface.command(command) {
                    Ok(()) => page.status = PageStatus::Starting,
                    Err(error) => page.status = PageStatus::Failed(error.to_string()),
                },
                None => {
                    // No surface yet (the tab has not painted): this is the
                    // URL `start` will navigate to.
                    page.current_url = url.to_string();
                }
            }
        }
        cx.notify();
    }

    pub(crate) fn browser_back(&mut self, tab_id: &str, cx: &mut Context<Self>) {
        self.browser_command(tab_id, BrowserCommand::Back, cx);
    }

    pub(crate) fn browser_forward(&mut self, tab_id: &str, cx: &mut Context<Self>) {
        self.browser_command(tab_id, BrowserCommand::Forward, cx);
    }

    pub(crate) fn browser_reload(&mut self, tab_id: &str, cx: &mut Context<Self>) {
        self.browser_command(tab_id, BrowserCommand::Reload, cx);
    }

    fn browser_command(&mut self, tab_id: &str, command: BrowserCommand, cx: &mut Context<Self>) {
        if let Some(page) = self.right_panel.web_pages.get_mut(tab_id)
            && let Some(surface) = page.surface.as_mut()
            && let Err(error) = surface.command(command)
        {
            page.status = PageStatus::Failed(error.to_string());
        }
        cx.notify();
    }

    /// The recording-surface seam for automated window tests: attaches the
    /// test host to the page so command and visibility paths run for real
    /// without starting a WebView.
    #[cfg(any(test, feature = "test-support"))]
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

    #[cfg(any(test, feature = "test-support"))]
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
                if let Some(page) = self.right_panel.web_pages.get_mut(tab_id) {
                    page.current_url = url;
                    page.can_back = can_back;
                    page.can_forward = can_forward;
                    page.load_error = None;
                    if !matches!(
                        page.status,
                        PageStatus::Failed(_) | PageStatus::Unsupported(_)
                    ) {
                        page.status = PageStatus::Ready;
                    }
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
                // same Host; the surface itself never opens a window.
                let layout = self.tool_layout();
                if let Some(tab) = layout.order.iter().find_map(|entry| match entry {
                    PanelTab::Web(tab) if tab.id == tab_id => Some(tab),
                    _ => None,
                }) && policy::classify(&url) == policy::Route::Load
                {
                    self.open_web_page(&url, tab.host.clone(), cx);
                }
            }
            BrowserEvent::LoadFailed { url, message } => {
                if let Some(page) = self.right_panel.web_pages.get_mut(tab_id) {
                    page.current_url = url;
                    page.load_error = Some(message);
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
    pub(crate) fn native_surface_covered(
        &self,
        window: gpui_kit::WindowId,
        cx: &mut gpui_kit::App,
    ) -> bool {
        self.surface_coordinator.is_covered()
            || self
                .panel_slide(crate::ui::panel_slide::PanelBoundary::Right)
                .is_some()
            || self.parked_deck.read(cx).covers_native_surface(cx)
            || self.notification_bell.read(cx).is_open()
            || gpui_kit::base::GlobalState::is_in_deferred_context(cx)
            || crate::ui::surface_coordinator::popup_menu_open(window, cx)
            || lyrel_ui_base::native_surface::is_covered(window, cx)
    }

    /// The paint hook's target: records bounds, starts the surface on first
    /// paint, and syncs visibility (covering overlays included) while painted.
    pub(crate) fn browser_page_prepared(
        &mut self,
        tab_id: &str,
        bounds: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let scale = window.scale_factor();
        let window_id = window.window_handle().window_id();
        let covered = self.native_surface_covered(window_id, cx);
        let tab_id_owned = tab_id.to_string();
        let page = self
            .right_panel
            .web_pages
            .entry(tab_id_owned.clone())
            .or_insert_with(WebPage::new);
        page.last_paint = std::time::Instant::now();
        page.window_id = Some(window_id);
        let changed = page.bounds.is_none_or(|old| {
            f32::from(old.origin.x - bounds.origin.x).abs() > 0.5
                || f32::from(old.origin.y - bounds.origin.y).abs() > 0.5
                || f32::from(old.size.width - bounds.size.width).abs() > 0.5
                || f32::from(old.size.height - bounds.size.height).abs() > 0.5
        });
        page.bounds = Some(bounds);
        if let Some(surface) = page.surface.as_mut() {
            if changed {
                surface.set_bounds(bounds, scale);
            }
            page.sync_visibility(true, covered);
            return;
        }
        if page.surface_pending || page.surface_attempted {
            return;
        }
        // Automated window tests scope this per-App: report the surface as
        // not started instead of creating a real WebView. Production and the
        // native harness never set the marker, so they take the real path —
        // the same seam the diagram surface has.
        #[cfg(any(test, feature = "test-support"))]
        if test_support::is_automated(cx) {
            page.surface_attempted = true;
            page.status = PageStatus::Unsupported(t(L10nKey::BrowserSurfaceAutomated).to_string());
            cx.notify();
            return;
        }
        let tab = {
            let layout = self.tool_layout();
            layout.order.iter().find_map(|entry| match entry {
                PanelTab::Web(tab) if tab.id == tab_id => Some(tab.clone()),
                _ => None,
            })
        };
        let Some(tab) = tab else {
            return;
        };
        let Some(page) = self.right_panel.web_pages.get_mut(tab_id) else {
            return;
        };
        let parent = match BrowserParentWindow::from_window(window) {
            Ok(parent) => parent,
            Err(error) => {
                page.status = PageStatus::Unsupported(error.to_string());
                return;
            }
        };
        let partition = browser_partition_dir(&tab);
        let start_url = if page.current_url.is_empty() {
            initial_navigate_url(&tab.address)
        } else {
            page.current_url.clone()
        };
        page.surface_pending = true;
        page.status = PageStatus::Starting;
        let tab_id = tab_id_owned;
        cx.spawn(async move |this, cx| {
            // The partition directory is prepared off the render path.
            let dir_ready = partition
                .as_ref()
                .map(|dir| std::fs::create_dir_all(dir).map(|_| dir.clone()));
            let _ = this.update(cx, |view, cx| {
                let Some(page) = view.right_panel.web_pages.get_mut(&tab_id) else {
                    return; // The tab went away between scheduling and now.
                };
                page.surface_pending = false;
                match dir_ready {
                    Some(Ok(dir)) => {
                        let (sender, receiver) = smol::channel::unbounded();
                        match BrowserSurfaceHost::start(
                            parent, bounds, scale, &start_url, dir, sender,
                        ) {
                            Ok(surface) => {
                                page.surface = Some(surface);
                                page.surface_attempted = true;
                                page.events =
                                    Some(browser_event_pump(tab_id.clone(), receiver, cx));
                            }
                            Err(error) => {
                                page.surface_attempted = true;
                                page.status = PageStatus::Failed(error.to_string());
                            }
                        }
                    }
                    Some(Err(error)) => {
                        page.surface_attempted = true;
                        page.status = PageStatus::Failed(error.to_string());
                    }
                    None => {
                        page.surface_attempted = true;
                        page.status = PageStatus::Unsupported(
                            t(L10nKey::BrowserSurfaceNoDataDir).to_string(),
                        );
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Watchdog for pages whose paint stopped — tab switch, panel collapse,
    /// workspace switch, or a covering overlay concealing the area. The
    /// native child hides once the grace expires; the page itself is never
    /// touched, so scroll position, an unsubmitted form and focus return all
    /// survive. Cover state is re-read on every tick, not only when a paint
    /// happens, so a menu or dialog in another crate still reaches the page.
    pub(crate) fn sync_browser_surfaces(&mut self, cx: &mut Context<Self>) {
        const VISIBLE_GRACE: std::time::Duration = std::time::Duration::from_millis(1500);
        let layout = self.tool_layout();
        let active_web = layout_active_web(&layout);
        let panel_open = layout.open;
        let now = std::time::Instant::now();
        // Covers are window-scoped (a dialog in another window must not hide
        // this page); resolve each painted window once per pass.
        let mut covered_by_window: std::collections::HashMap<gpui_kit::WindowId, bool> =
            std::collections::HashMap::new();
        for page in self.right_panel.web_pages.values() {
            if let Some(window) = page.window_id {
                covered_by_window
                    .entry(window)
                    .or_insert_with(|| self.native_surface_covered(window, cx));
            }
        }
        let mut notify = false;
        for (tab_id, page) in self.right_panel.web_pages.iter_mut() {
            let selected = active_web.as_deref() == Some(tab_id.as_str());
            let painted = selected
                && panel_open
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
            // A live page keeps repainting so the watchdog can tell "still
            // shown" from "concealed": while visible, stay in the paint set.
            if painted && !covered {
                cx.notify();
            }
        }
        if notify {
            cx.notify();
        }
    }

    /// The global toast stack's right inset while a web page is visible: the
    /// content area's lower-right edge, so ordinary toasts clear the WebView
    /// instead of covering it. No cover is raised here — a toast never hides
    /// the page.
    pub(crate) fn browser_toast_clearance(&self, window: &Window, cx: &gpui_kit::App) -> Pixels {
        let layout = self.tool_layout();
        if layout.open && layout_active_web(&layout).is_some() {
            px(self.right_panel_px(window, cx) + 8.)
        } else {
            px(8.)
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

/// The page the layout has selected, if it is a web tab.
pub(crate) fn layout_active_web(layout: &ToolLayout) -> Option<String> {
    match &layout.active {
        PanelTab::Web(tab) => Some(tab.id.clone()),
        PanelTab::Tool(_) => None,
    }
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

/// The durable per-Host site-data partition for `tab`'s surface: one
/// directory per bound Host under the app config's `browser/` folder, the
/// local machine's pages in `browser/local`. `None` only when the config dir
/// itself is unavailable — the surface then reports Unsupported rather than
/// borrowing a shared profile.
pub(crate) fn browser_partition_dir(tab: &WebTab) -> Option<std::path::PathBuf> {
    let root = lyrel_core::core::config::config_dir_path()?;
    let key = tab
        .host
        .as_ref()
        .map(|host| format!("host-{}", host.host_id().0))
        .unwrap_or_else(|| "local".to_string());
    Some(root.join("browser").join(key))
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

/// Renders one web tab's page: the chrome row over the native surface's
/// paint anchor. `tab` is the layout entry the selection named.
pub(crate) fn render_browser_page(
    app: &mut LyrelApp,
    tab: &WebTab,
    window: &mut Window,
    cx: &mut Context<LyrelApp>,
) -> AnyElement {
    use gpui_kit::component::button::ButtonCustomVariant;
    let theme = cx.theme();
    let (muted, border, foreground) = (theme.muted_foreground, theme.border, theme.foreground);
    let (can_back, can_forward, status, current_url, load_error, title) = app
        .right_panel
        .web_pages
        .get(&tab.id)
        .map(|page| {
            (
                page.can_back,
                page.can_forward,
                page.status.clone(),
                page.current_url.clone(),
                page.load_error.clone(),
                page.display_title(tab),
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
    let address = if current_url.is_empty() {
        initial_navigate_url(&tab.address)
    } else {
        current_url.clone()
    };
    let covered = app.native_surface_covered(window.window_handle().window_id(), cx);

    if app.browser_chrome.is_none() {
        app.browser_chrome = Some(BrowserChrome::new(window, cx));
    }
    app.sync_browser_address_input(&tab.id, &address, window, cx);
    let chrome = app.browser_chrome.as_ref().expect("chrome just ensured");
    let input = chrome.input.clone();
    let tab_id = tab.id.clone();

    let icon_button =
        |id: &'static str,
         icon: IconName,
         tip: L10nKey,
         enabled: bool,
         action: fn(&mut LyrelApp, &str, &mut Context<LyrelApp>)| {
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
                .on_click(cx.listener(move |this, _, _window, cx| {
                    action(this, &tab_id, cx);
                }))
        };

    let toolbar = h_flex()
        .id("browser-chrome")
        .test_support()
        .flex_none()
        .w_full()
        .items_center()
        .gap_1()
        .px_1()
        .py_1()
        .border_b_1()
        .border_color(border)
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
                        Input::new(&input)
                            .appearance(false)
                            .xsmall()
                            .cleanable(true),
                    ),
                ),
        )
        .child(
            div()
                .id("browser-host")
                .test_support()
                .flex_none()
                .px_2()
                .h(px(20.))
                .flex()
                .items_center()
                .rounded(px(5.))
                .bg(cx.theme().muted)
                .text_xs()
                .text_color(muted)
                .child(browser_host_label(tab)),
        )
        .child(icon_button(
            "browser-external",
            IconName::ExternalLink,
            L10nKey::BrowserOpenExternal,
            true,
            |this, tab_id, cx| this.browser_open_external(tab_id, cx),
        ));

    let tab_id_bounds = tab.id.clone();
    let anchor = div()
        .flex_1()
        .min_w_0()
        .min_h_0()
        .relative()
        .on_children_prepainted({
            let app = cx.entity();
            move |bounds, window, cx| {
                if let Some(bounds) = bounds.first() {
                    let bounds = *bounds;
                    app.update(cx, |app, cx| {
                        app.browser_page_prepared(&tab_id_bounds, bounds, window, cx);
                    });
                }
            }
        })
        // `on_children_prepainted` is delivered for child bounds; the empty
        // child gives the native surface a real paint event to anchor to.
        .child(div().size_full())
        .id("browser-surface-anchor")
        .test_support();

    v_flex()
        .flex_1()
        .min_w_0()
        .min_h_0()
        .child(toolbar)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .min_h_0()
                .relative()
                .child(anchor)
                .children(browser_overlay(
                    &tab.id,
                    &status,
                    &address,
                    load_error.as_deref(),
                    covered,
                    &title,
                    cx,
                )),
        )
        .into_any_element()
}

/// The frame over the page area while the surface is not Ready: a bounded
/// static frame, so concealment never shows a hole and a failed page says
/// why in place. While a workbench overlay covers the page, the same bounded
/// frame names the page it stands in for — the surface itself is hidden, not
/// destroyed, so the draft, scroll position and focus come back with it.
fn browser_overlay(
    tab_id: &str,
    status: &PageStatus,
    address: &str,
    load_error: Option<&str>,
    covered: bool,
    title: &str,
    cx: &mut Context<LyrelApp>,
) -> Option<AnyElement> {
    if covered
        && load_error.is_none()
        && !matches!(status, PageStatus::Failed(_) | PageStatus::Unsupported(_))
    {
        return Some(browser_still_frame(tab_id, title, address, cx));
    }
    let muted = cx.theme().muted_foreground;
    let message = match status {
        PageStatus::Ready => load_error.map(str::to_owned),
        PageStatus::Idle | PageStatus::Starting => Some(t(L10nKey::BrowserStarting).to_string()),
        PageStatus::Failed(message) | PageStatus::Unsupported(message) => Some(message.clone()),
    }?;
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
                load_error.is_some() || matches!(status, PageStatus::Failed(_)),
                |col| {
                    col.child(
                        div()
                            .id("browser-overlay-address")
                            .test_support()
                            .text_xs()
                            .text_color(muted)
                            .child(address.to_string()),
                    )
                    .child(
                        Button::new(format!("browser-overlay-open-{tab_id}"))
                            .label(t(L10nKey::BrowserOpenExternal))
                            .small()
                            .on_click(cx.listener(move |this, _, _window, cx| {
                                this.browser_open_external(&tab_id, cx);
                            })),
                    )
                },
            )
            .into_any_element(),
    )
}

/// The bounded still frame painted in place of a concealed native page. It
/// carries the covered tab's own title and address, so a tab switch during a
/// transition shows the incoming page rather than a stale one.
fn browser_still_frame(
    tab_id: &str,
    title: &str,
    address: &str,
    cx: &mut Context<LyrelApp>,
) -> AnyElement {
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
