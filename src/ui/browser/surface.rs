//! The Side Panel browser's native surface: one WebView2 child window on
//! Windows (through `webview2-com`) or one native WKWebView on macOS
//! (through `objc2-web-kit`), exposed to the panel as
//! [`BrowserSurfaceHost`]. This is a browsing surface, not the draw.io
//! document engine: there is no privileged bridge, no fixed origin, no custom
//! protocol, no page → workbench message channel at all. The only ways out
//! are the events below, and the only ways in are the commands below — plus
//! geometry and visibility, which the panel drives from its paint, the same
//! lifecycle the diagram surface uses.
//!
//! Policy (#382): top-level navigation is limited to HTTP, HTTPS and the
//! built-in blank page by every host before any request is made; device
//! permissions are denied by default; downloads and unsolicited popups never
//! leave the surface; a new-window request becomes an event only when the
//! platform can show it was user-initiated (WebView2's `IsUserInitiated`,
//! WKWebView's `WKNavigationTypeLinkActivated`). A Host-bound page is also
//! held to [`RouteGuard`]: only the loopback origin the panel verified may
//! load, so a reload, history entry or redirect cannot reach a coincident
//! desktop service (#392).

use gpui_kit::{Bounds, Pixels};
use smol::channel::Sender;

/// One native event the browsing surface reports to the panel.
#[derive(Clone, Debug, PartialEq)]
// The platform arms are built only by the native hosts; automated builds
// keep the wording but never construct them.
#[cfg_attr(test, allow(dead_code))]
pub enum BrowserEvent {
    /// The surface exists and its first page committed. The panel may start
    /// sending commands; anything queued before this is dropped by the host.
    Ready,
    /// The top-level page changed — a load, a redirect, back or forward.
    /// Carries the history state the chrome needs for its buttons.
    Navigated {
        url: String,
        can_back: bool,
        can_forward: bool,
    },
    /// The page's document title changed.
    TitleChanged {
        title: String,
    },
    /// The user asked for a new window (a `target=_blank`-style open with a
    /// user gesture behind it). The panel answers by opening a sibling tab
    /// bound to the same Host; the surface itself never creates a window.
    NewWindow {
        url: String,
    },
    /// A navigation failed after it was allowed.
    LoadFailed {
        url: String,
        message: String,
    },
    /// A Host-bound page tried to load a loopback origin the panel has not
    /// verified (a reload, a history entry, a redirect, an absolute link to
    /// another port). The navigation was cancelled before it could reach a
    /// coincident desktop service; the panel re-verifies the route and then
    /// loads the forwarded URL itself (#392).
    RouteBlocked {
        url: String,
    },
    /// Something the policy refused: a denied scheme, an unsolicited popup,
    /// a download, a permission. Diagnostic — the page stays where it was.
    Blocked {
        reason: String,
    },
    /// The surface itself is unusable (creation failure, crash). The panel
    /// shows the message and offers the explicit external-open action.
    Failed {
        message: String,
    },
    Closed,
}

/// One command the panel sends the surface.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BrowserCommand {
    Back,
    Forward,
    Reload,
    /// Navigate the top level. `url` is `about:blank` or an http(s) URL the
    /// policy has already cleared.
    Navigate {
        url: String,
    },
    /// Sets the loopback rule for the surface. A Host-bound page starts at
    /// [`RouteGuard::DenyLoopback`] until the panel verifies its forward, then
    /// moves to [`RouteGuard::Origin`]. Any loopback origin the guard does not
    /// approve is cancelled and reported as [`BrowserEvent::RouteBlocked`]
    /// until the panel verifies it and sets a new origin (#392).
    SetRouteGuard {
        guard: RouteGuard,
    },
}

/// Which loopback origins a surface may load.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RouteGuard {
    /// A page with no Host binding: loopback is this machine's own.
    Open,
    /// A Host-bound page whose forward is not verified: no loopback load is
    /// safe, because the original address could now be a coincident local
    /// service.
    DenyLoopback,
    /// The one verified origin serving the Host-bound page.
    Origin(String),
}

impl RouteGuard {
    /// Whether this rule permits loading `address`. Every non-loopback URL
    /// is allowed (an ordinary external page); loopback is decided by the
    /// rule. The surface hosts call this from their navigation handlers, so a
    /// reload, a history entry, a redirect or a page's own loopback link can
    /// never bypass the panel's verification.
    pub fn allows(&self, address: &str) -> bool {
        match self {
            Self::Open => true,
            Self::DenyLoopback => super::policy::loopback_origin(address).is_none(),
            Self::Origin(origin) => {
                super::policy::loopback_origin(address).is_none_or(|candidate| candidate == *origin)
            }
        }
    }
}

/// Why a surface operation could not run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BrowserSurfaceError {
    /// Produced by a host's own geometry contract.
    #[cfg(any(windows, target_os = "macos"))]
    InvalidBounds,
    /// Produced while capturing a parent handle on a desktop platform.
    #[cfg(any(windows, target_os = "macos"))]
    Window,
    Platform(String),
    /// The no-host answer on platforms without a desktop surface.
    #[cfg(not(any(windows, target_os = "macos")))]
    Unsupported(&'static str),
}

impl std::fmt::Display for BrowserSurfaceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            #[cfg(any(windows, target_os = "macos"))]
            Self::InvalidBounds => f.write_str("the browser page has no drawable size"),
            #[cfg(any(windows, target_os = "macos"))]
            Self::Window => f.write_str("the window has no native handle for the browser page"),
            Self::Platform(message) => write!(f, "the browser surface failed: {message}"),
            #[cfg(not(any(windows, target_os = "macos")))]
            Self::Unsupported(message) => f.write_str(message),
        }
    }
}

/// The GPUI window's native parent handle, captured while the window is
/// borrowed so surface creation can happen after paint without keeping a
/// `&mut Window` alive. Mirrors the diagram surface's capture: the handle
/// stays valid while the GPUI window that produced it is alive.
#[derive(Clone, Copy)]
pub struct BrowserParentWindow {
    #[cfg(windows)]
    hwnd: windows::Win32::Foundation::HWND,
    #[cfg(target_os = "macos")]
    view: raw_window_handle::AppKitWindowHandle,
}

#[cfg(target_os = "macos")]
impl raw_window_handle::HasWindowHandle for BrowserParentWindow {
    fn window_handle(
        &self,
    ) -> Result<raw_window_handle::WindowHandle<'_>, raw_window_handle::HandleError> {
        let raw = raw_window_handle::RawWindowHandle::AppKit(self.view);
        // SAFETY: the handle stays valid while the GPUI window that produced
        // it is alive; the surface is dropped with its presentation.
        Ok(unsafe { raw_window_handle::WindowHandle::borrow_raw(raw) })
    }
}

impl BrowserParentWindow {
    pub fn from_window(window: &gpui_kit::Window) -> Result<Self, BrowserSurfaceError> {
        #[cfg(windows)]
        {
            let handle = raw_window_handle::HasWindowHandle::window_handle(window)
                .map_err(|_| BrowserSurfaceError::Window)?;
            match handle.as_raw() {
                raw_window_handle::RawWindowHandle::Win32(handle) => Ok(Self {
                    hwnd: windows::Win32::Foundation::HWND(handle.hwnd.get() as *mut _),
                }),
                _ => Err(BrowserSurfaceError::Window),
            }
        }
        #[cfg(target_os = "macos")]
        {
            let handle = raw_window_handle::HasWindowHandle::window_handle(window)
                .map_err(|_| BrowserSurfaceError::Window)?;
            return match handle.as_raw() {
                raw_window_handle::RawWindowHandle::AppKit(handle) => Ok(Self { view: handle }),
                _ => Err(BrowserSurfaceError::Window),
            };
        }
        #[cfg(not(any(windows, target_os = "macos")))]
        {
            let _ = window;
            Err(BrowserSurfaceError::Unsupported(
                "the Side Panel browser has no host on this platform",
            ))
        }
    }
}

/// The test-only stand-in for a live browsing surface: it records the
/// commands production code sends and answers events the way a native host
/// would, so the panel's whole command path runs through
/// [`BrowserSurfaceHost::command`] instead of a no-op.
#[cfg(any(test, feature = "test-support"))]
pub struct BrowserTestSurface {
    pub commands: std::cell::RefCell<Vec<BrowserCommand>>,
    _events: Sender<BrowserEvent>,
    /// The last visibility the panel asked for, so a consumer can prove the
    /// overlay path hid the page and brought it back.
    pub visible: std::cell::Cell<Option<bool>>,
    /// Whether the recording page currently holds native focus.
    pub focused: std::cell::Cell<bool>,
    /// How many times the panel handed keyboard focus back to the page.
    pub focus_requests: std::cell::Cell<usize>,
}

/// Creates and owns one child browsing surface. Windows uses WebView2
/// directly; macOS owns a native WKWebView. The platform hosts compile in
/// every feature combination; `Test` is added for automated builds, and which
/// variant one presentation holds is a runtime decision made by the panel
/// view.
pub struct BrowserSurfaceHost {
    inner: Inner,
}

enum Inner {
    #[cfg(windows)]
    Windows(windows_host::Host),
    #[cfg(target_os = "macos")]
    Macos(macos_host::Host),
    #[cfg(any(test, feature = "test-support"))]
    Test(std::rc::Rc<BrowserTestSurface>),
    #[cfg(not(any(windows, target_os = "macos", test, feature = "test-support")))]
    Unsupported,
}

impl BrowserSurfaceHost {
    #[cfg(any(test, feature = "test-support"))]
    pub fn test(events: Sender<BrowserEvent>) -> Self {
        Self {
            inner: Inner::Test(std::rc::Rc::new(BrowserTestSurface {
                commands: std::cell::RefCell::new(Vec::new()),
                _events: events,
                visible: std::cell::Cell::new(None),
                focused: std::cell::Cell::new(false),
                focus_requests: std::cell::Cell::new(0),
            })),
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn test_commands(&self) -> Vec<BrowserCommand> {
        match &self.inner {
            Inner::Test(surface) => surface.commands.borrow().clone(),
            #[allow(unreachable_patterns)]
            _ => Vec::new(),
        }
    }

    /// The last visibility the panel asked the recording surface for.
    #[cfg(any(test, feature = "test-support"))]
    pub fn test_visibility(&self) -> Option<bool> {
        match &self.inner {
            Inner::Test(surface) => surface.visible.get(),
            #[allow(unreachable_patterns)]
            _ => None,
        }
    }

    /// How many times the panel handed native focus back to the page.
    #[cfg(any(test, feature = "test-support"))]
    pub fn test_focus_requests(&self) -> usize {
        match &self.inner {
            Inner::Test(surface) => surface.focus_requests.get(),
            #[allow(unreachable_patterns)]
            _ => 0,
        }
    }

    /// Pretends the user clicked into the page, so a cover can record that the
    /// native surface held focus.
    #[cfg(any(test, feature = "test-support"))]
    pub fn test_take_focus(&self) {
        if let Inner::Test(surface) = &self.inner {
            surface.focused.set(true);
        }
    }

    /// Starts one surface at `bounds` (window coordinates, physical scaling
    /// applied by the Windows host) showing `url`. `site` is the named
    /// per-Host site-data partition the panel prepared. `guard` is the
    /// loopback rule for the page; when it is [`RouteGuard::Origin`] the panel
    /// has already verified that origin and `url` is on it. `events` receives every
    /// [`BrowserEvent`] until the host is dropped.
    pub fn start(
        parent: BrowserParentWindow,
        bounds: Bounds<Pixels>,
        scale: f32,
        url: &str,
        site: super::route::BrowserSiteData,
        guard: RouteGuard,
        events: Sender<BrowserEvent>,
    ) -> Result<Self, BrowserSurfaceError> {
        #[cfg(windows)]
        {
            windows_host::Host::start(parent, bounds, scale, url, site, guard, events).map(|host| {
                Self {
                    inner: Inner::Windows(host),
                }
            })
        }
        #[cfg(target_os = "macos")]
        {
            let _ = scale;
            return macos_host::Host::start(parent, bounds, url, site, guard, events).map(|host| {
                Self {
                    inner: Inner::Macos(host),
                }
            });
        }
        #[cfg(not(any(windows, target_os = "macos")))]
        {
            let _ = (parent, bounds, scale, url, site, guard, events);
            Err(BrowserSurfaceError::Unsupported(
                "the Side Panel browser has no host on this platform",
            ))
        }
    }

    pub fn set_bounds(&mut self, bounds: Bounds<Pixels>, scale: f32) {
        match &mut self.inner {
            #[cfg(windows)]
            Inner::Windows(host) => host.set_bounds(bounds, scale),
            #[cfg(target_os = "macos")]
            Inner::Macos(host) => {
                let _ = scale;
                host.set_bounds(bounds);
            }
            #[cfg(any(test, feature = "test-support"))]
            Inner::Test(_) => {
                let _ = (bounds, scale);
            }
            #[cfg(not(any(windows, target_os = "macos", test, feature = "test-support")))]
            Inner::Unsupported => {
                let _ = (bounds, scale);
            }
        }
    }

    /// Shows or hides the native child. The panel calls this from its paint
    /// lifecycle and from the surface coordinator: hidden while concealed by
    /// a covering overlay or out of the painted tree, visible otherwise.
    /// Hiding never destroys the page.
    pub fn set_visible(&mut self, visible: bool) {
        match &mut self.inner {
            #[cfg(windows)]
            Inner::Windows(host) => host.set_visible(visible),
            #[cfg(target_os = "macos")]
            Inner::Macos(host) => host.set_visible(visible),
            #[cfg(any(test, feature = "test-support"))]
            Inner::Test(surface) => {
                surface.visible.set(Some(visible));
                if !visible {
                    surface.focused.set(false);
                }
            }
            #[cfg(not(any(windows, target_os = "macos", test, feature = "test-support")))]
            Inner::Unsupported => {
                let _ = visible;
            }
        }
    }

    /// Whether the native page currently holds keyboard focus. The panel
    /// records this before concealing the surface and hands it back when the
    /// last cover lifts.
    pub fn has_focus(&self) -> bool {
        match &self.inner {
            #[cfg(windows)]
            Inner::Windows(host) => host.has_focus(),
            #[cfg(target_os = "macos")]
            Inner::Macos(host) => host.has_focus(),
            #[cfg(any(test, feature = "test-support"))]
            Inner::Test(surface) => surface.focused.get(),
            #[cfg(not(any(windows, target_os = "macos", test, feature = "test-support")))]
            Inner::Unsupported => false,
        }
    }

    /// Hands keyboard focus to the native page. Page editing shortcuts stay
    /// with the page while the surface holds focus; workbench chords keep
    /// application priority through the window's keystroke interceptors.
    pub fn focus(&mut self) {
        match &mut self.inner {
            #[cfg(windows)]
            Inner::Windows(host) => host.focus(),
            #[cfg(target_os = "macos")]
            Inner::Macos(host) => host.focus(),
            #[cfg(any(test, feature = "test-support"))]
            Inner::Test(surface) => {
                surface.focused.set(true);
                surface.focus_requests.set(surface.focus_requests.get() + 1);
            }
            #[cfg(not(any(windows, target_os = "macos", test, feature = "test-support")))]
            Inner::Unsupported => {}
        }
    }

    pub fn command(&mut self, command: BrowserCommand) -> Result<(), BrowserSurfaceError> {
        match &mut self.inner {
            #[cfg(any(test, feature = "test-support"))]
            Inner::Test(surface) => {
                surface.commands.borrow_mut().push(command);
                Ok(())
            }
            #[cfg(windows)]
            Inner::Windows(host) => host.command(command),
            #[cfg(target_os = "macos")]
            Inner::Macos(host) => host.command(command),
            #[cfg(not(any(windows, target_os = "macos", test, feature = "test-support")))]
            Inner::Unsupported => {
                let _ = command;
                Err(BrowserSurfaceError::Unsupported(
                    "the Side Panel browser has no host on this platform",
                ))
            }
        }
    }
}

// Dropping the host destroys the native child with it (each platform Host's
// own Drop tears the WebView down) and closes the page: a closed tab's live
// page is destroyed, never revived on restore.

#[cfg(windows)]
mod windows_host;

#[cfg(target_os = "macos")]
mod macos_host;
