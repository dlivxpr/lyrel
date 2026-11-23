//! The Windows browsing surface: one WebView2 child window inside the GPUI
//! window, created directly through `webview2-com` (wry is deliberately not
//! used here — the same policy reasons as the diagram surface, and this host
//! needs WebView2's `IsUserInitiated` to answer new-window requests). The
//! durable profile lives in the panel-supplied per-Host partition directory;
//! nothing is in-private, so a development origin keeps its site data between
//! launches. Every route the policy denies is cancelled before it leaves the
//! process, and every answer the page gets is the one this file writes.

use super::super::route::BrowserSiteData;
use super::{BrowserCommand, BrowserEvent, BrowserParentWindow, BrowserSurfaceError, RouteGuard};
use gpui_kit::{Bounds, Pixels};
use smol::channel::Sender;
use std::cell::RefCell;
use std::rc::Rc;
use std::result::Result;
use webview2_com::Microsoft::Web::WebView2::Win32::*;
use webview2_com::*;
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, SW_HIDE, SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SWP_NOZORDER,
    SetWindowPos, ShowWindow, WINDOW_EX_STYLE, WS_CHILD, WS_CLIPCHILDREN, WS_CLIPSIBLINGS,
    WS_VISIBLE,
};
use windows::core::{HSTRING, Interface, PCWSTR, PWSTR};

struct Shared {
    host: HWND,
    env: Option<ICoreWebView2Environment>,
    controller: Option<ICoreWebView2Controller>,
    webview: Option<ICoreWebView2>,
    initial_url: String,
    visible: bool,
    rect: RECT,
    closed: bool,
    /// The loopback rule the panel set for this page (#392).
    guard: RouteGuard,
    sender: Sender<BrowserEvent>,
}

pub struct Host {
    shared: Rc<RefCell<Shared>>,
    host: HWND,
}

impl Host {
    pub fn start(
        parent: BrowserParentWindow,
        bounds: Bounds<Pixels>,
        scale: f32,
        url: &str,
        site: BrowserSiteData,
        guard: RouteGuard,
        events: Sender<BrowserEvent>,
    ) -> Result<Self, BrowserSurfaceError> {
        let rect = physical_rect(bounds, scale)?;
        if rect.right <= rect.left || rect.bottom <= rect.top {
            return Err(BrowserSurfaceError::InvalidBounds);
        }
        let host = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                windows::core::w!("STATIC"),
                windows::core::w!(""),
                WS_CHILD | WS_VISIBLE | WS_CLIPCHILDREN | WS_CLIPSIBLINGS,
                rect.left,
                rect.top,
                rect.right - rect.left,
                rect.bottom - rect.top,
                Some(parent.hwnd),
                None,
                None,
                None,
            )
        }
        .map_err(|error| BrowserSurfaceError::Platform(error.to_string()))?;

        let shared = Rc::new(RefCell::new(Shared {
            host,
            env: None,
            controller: None,
            webview: None,
            initial_url: url.to_string(),
            visible: true,
            rect,
            closed: false,
            guard,
            sender: events,
        }));
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        }
        Self::create_environment(&shared, &site.dir);
        Ok(Self { shared, host })
    }

    fn create_environment(shared: &Rc<RefCell<Shared>>, data_dir: &std::path::Path) {
        if shared.borrow().closed {
            return;
        }
        let options = CoreWebView2EnvironmentOptions::default();
        unsafe {
            options.set_additional_browser_arguments(
                "--disable-background-networking --disable-component-update \
                 --disable-domain-reliability --disable-sync --no-first-run \
                 --no-default-browser-check --metrics-recording-only \
                 --disable-features=OptimizationHints,MediaRouter,Translate"
                    .to_string(),
            );
            options.set_are_browser_extensions_enabled(false);
        }
        let environment_options = ICoreWebView2EnvironmentOptions::from(options);
        let handler_shared = shared.clone();
        let handler =
            CreateCoreWebView2EnvironmentCompletedHandler::create(Box::new(move |result, env| {
                if let Err(error) = result {
                    fail(&handler_shared, error.to_string());
                    return Ok(());
                }
                let Some(env) = env else {
                    fail(&handler_shared, "WebView2 returned no environment");
                    return Ok(());
                };
                {
                    let mut state = handler_shared.borrow_mut();
                    if state.closed {
                        return Ok(());
                    }
                    state.env = Some(env.clone());
                }
                Self::create_controller(&handler_shared, env);
                Ok(())
            }));
        let data_dir = HSTRING::from(data_dir.to_string_lossy().as_ref());
        let created = unsafe {
            CreateCoreWebView2EnvironmentWithOptions(
                PCWSTR::null(),
                &data_dir,
                &environment_options,
                &handler,
            )
        };
        if let Err(error) = created {
            fail(shared, error.to_string());
        }
    }

    fn create_controller(shared: &Rc<RefCell<Shared>>, env: ICoreWebView2Environment) {
        let host = match shared.try_borrow() {
            Ok(state) if !state.closed => state.host,
            _ => return,
        };
        let handler_shared = shared.clone();
        let handler = CreateCoreWebView2ControllerCompletedHandler::create(Box::new(
            move |result, controller| {
                if let Err(error) = result {
                    fail(&handler_shared, error.to_string());
                    return Ok(());
                }
                let Some(controller) = controller else {
                    fail(&handler_shared, "WebView2 returned no controller");
                    return Ok(());
                };
                Self::finish_controller(&handler_shared, controller);
                Ok(())
            },
        ));
        let created = unsafe { env.CreateCoreWebView2Controller(host, &handler) };
        if let Err(error) = created {
            fail(shared, error.to_string());
        }
    }

    fn finish_controller(shared: &Rc<RefCell<Shared>>, controller: ICoreWebView2Controller) {
        let result = unsafe { Self::configure(shared, &controller) };
        if let Err(error) = result {
            fail(shared, error.to_string());
            return;
        }
        let (bounds, visible, initial_url) = {
            let mut state = shared.borrow_mut();
            if state.closed {
                let _ = unsafe { controller.Close() };
                return;
            }
            state.controller = Some(controller.clone());
            (state.rect, state.visible, state.initial_url.clone())
        };
        unsafe {
            // The controller's bounds are relative to its host HWND, which
            // already sits at `bounds` inside the GPUI window; using the
            // window-relative rect here would offset the WebView twice.
            let _ = controller.SetBounds(RECT {
                left: 0,
                top: 0,
                right: bounds.right - bounds.left,
                bottom: bounds.bottom - bounds.top,
            });
            let _ = controller.SetIsVisible(visible);
        }
        let Some(webview) = shared.borrow().webview.clone() else {
            fail(shared, "WebView2 returned no document");
            return;
        };
        if let Err(error) = unsafe { webview.Navigate(&HSTRING::from(initial_url.as_str())) } {
            fail(shared, error.to_string());
        }
    }

    unsafe fn configure(
        shared: &Rc<RefCell<Shared>>,
        controller: &ICoreWebView2Controller,
    ) -> windows::core::Result<()> {
        let webview = unsafe { controller.CoreWebView2()? };
        {
            let mut state = shared.borrow_mut();
            if state.closed {
                return Ok(());
            }
            state.webview = Some(webview.clone());
        }
        let settings = unsafe { webview.Settings()? };
        unsafe {
            // No DevTools, no script dialogs, no status bar. Default context
            // menus stay: they are page chrome, and their "open in new
            // window" requests land in the new-window handler below, which
            // only a user gesture passes.
            settings.SetAreDevToolsEnabled(false)?;
            settings.SetAreDefaultScriptDialogsEnabled(false)?;
            settings.SetAreHostObjectsAllowed(false)?;
            settings.SetIsStatusBarEnabled(false)?;
            settings.SetIsZoomControlEnabled(false)?;
            settings.SetIsScriptEnabled(true)?;
            settings.SetIsWebMessageEnabled(false)?;
        }
        if let Ok(settings4) = settings.cast::<ICoreWebView2Settings4>() {
            let _ = unsafe { settings4.SetIsGeneralAutofillEnabled(false) };
            let _ = unsafe { settings4.SetIsPasswordAutosaveEnabled(false) };
        }
        if let Ok(settings5) = settings.cast::<ICoreWebView2Settings5>() {
            let _ = unsafe { settings5.SetIsPinchZoomEnabled(false) };
        }
        if let Ok(controller4) = controller.cast::<ICoreWebView2Controller4>() {
            unsafe { controller4.SetAllowExternalDrop(false)? };
        }

        let mut token = 0i64;

        // Top-level routes: HTTP, HTTPS, about:blank. Everything else —
        // file:, ftp:, data:, custom schemes — is cancelled before it leaves
        // the process.
        let nav_shared = shared.clone();
        unsafe {
            webview.add_NavigationStarting(
                &NavigationStartingEventHandler::create(Box::new(move |_, args| {
                    let Some(args) = args else {
                        return Ok(());
                    };
                    let uri = match argument_uri(&args) {
                        Ok(uri) => uri,
                        Err(_) => return Ok(()),
                    };
                    if super::super::policy::classify(&uri) == super::super::policy::Route::Deny {
                        let _ = args.SetCancel(true);
                        emit(
                            &nav_shared,
                            BrowserEvent::Blocked {
                                reason: format!("navigation to {uri}"),
                            },
                        );
                        return Ok(());
                    }
                    // The verified-forward guard (#392): a Host-bound page
                    // may only load the loopback origin the panel verified.
                    // A reload, a history entry, a redirect or a page's own
                    // loopback link is cancelled here before it can reach a
                    // coincident desktop service.
                    let allowed = match nav_shared.try_borrow() {
                        Ok(state) => state.guard.allows(&uri),
                        Err(_) => false,
                    };
                    if !allowed {
                        let _ = args.SetCancel(true);
                        emit(&nav_shared, BrowserEvent::RouteBlocked { url: uri });
                    }
                    Ok(())
                })),
                &mut token,
            )?;
        }

        // A frame inside the page navigates on its own: same policy, same
        // deny. Subresource requests (images, XHR) are not filtered — an
        // ordinary page needs them, and their top-level counterpart is the
        // route that matters.
        let frame_shared = shared.clone();
        unsafe {
            webview.add_FrameNavigationStarting(
                &NavigationStartingEventHandler::create(Box::new(move |_, args| {
                    let Some(args) = args else {
                        return Ok(());
                    };
                    let uri = match argument_uri(&args) {
                        Ok(uri) => uri,
                        Err(_) => return Ok(()),
                    };
                    if super::super::policy::classify(&uri) == super::super::policy::Route::Deny {
                        let _ = args.SetCancel(true);
                        emit(
                            &frame_shared,
                            BrowserEvent::Blocked {
                                reason: format!("frame navigation to {uri}"),
                            },
                        );
                    }
                    Ok(())
                })),
                &mut token,
            )?;
        }

        // New windows. Only a user-initiated request becomes a sibling tab;
        // a script-opened popup is cancelled here and never surfaces.
        let new_window_shared = shared.clone();
        unsafe {
            webview.add_NewWindowRequested(
                &NewWindowRequestedEventHandler::create(Box::new(move |_, args| {
                    let Some(args) = args else {
                        return Ok(());
                    };
                    let _ = args.SetHandled(true);
                    let uri = match new_window_uri(&args) {
                        Ok(uri) => uri,
                        Err(_) => return Ok(()),
                    };
                    if super::super::policy::classify(&uri) != super::super::policy::Route::Load {
                        emit(
                            &new_window_shared,
                            BrowserEvent::Blocked {
                                reason: format!("a new window to {uri}"),
                            },
                        );
                        return Ok(());
                    }
                    let allowed = match new_window_shared.try_borrow() {
                        Ok(state) => state.guard.allows(&uri),
                        Err(_) => false,
                    };
                    if !allowed {
                        emit(
                            &new_window_shared,
                            BrowserEvent::RouteBlocked { url: uri },
                        );
                        return Ok(());
                    }
                    let mut flag = windows::core::BOOL(0);
                    let user_initiated = args.IsUserInitiated(&mut flag).is_ok() && flag.as_bool();
                    if user_initiated {
                        emit(&new_window_shared, BrowserEvent::NewWindow { url: uri });
                    } else {
                        emit(
                            &new_window_shared,
                            BrowserEvent::Blocked {
                                reason: "an unsolicited popup".to_string(),
                            },
                        );
                    }
                    Ok(())
                })),
                &mut token,
            )?;
        }

        // No downloads. The preview is not a downloader; a site that asks is
        // refused, and the page stays where it was.
        let download_shared = shared.clone();
        let webview4: ICoreWebView2_4 = webview.cast()?;
        unsafe {
            webview4.add_DownloadStarting(
                &DownloadStartingEventHandler::create(Box::new(move |_, args| {
                    let Some(args) = args else {
                        return Ok(());
                    };
                    let _ = args.SetCancel(true);
                    emit(
                        &download_shared,
                        BrowserEvent::Blocked {
                            reason: "a download".to_string(),
                        },
                    );
                    Ok(())
                })),
                &mut token,
            )?;
        }

        // Device permissions (camera, microphone, geolocation, …) default to
        // deny. There is no grant path in the preview; a site that needs one
        // belongs in the system browser.
        let permission_shared = shared.clone();
        unsafe {
            webview.add_PermissionRequested(
                &PermissionRequestedEventHandler::create(Box::new(move |_, args| {
                    let Some(args) = args else {
                        return Ok(());
                    };
                    let _ = args.SetState(COREWEBVIEW2_PERMISSION_STATE_DENY);
                    emit(
                        &permission_shared,
                        BrowserEvent::Blocked {
                            reason: "a browser permission".to_string(),
                        },
                    );
                    Ok(())
                })),
                &mut token,
            )?;
        }

        // The page asking to close its window is not a request the preview
        // honours: the tab's close control is the strip's.
        let close_shared = shared.clone();
        unsafe {
            webview.add_WindowCloseRequested(
                &WindowCloseRequestedEventHandler::create(Box::new(move |_, _| {
                    emit(
                        &close_shared,
                        BrowserEvent::Blocked {
                            reason: "the page asked to close its window".to_string(),
                        },
                    );
                    Ok(())
                })),
                &mut token,
            )?;
        }

        // URL and title keep the chrome and the tab label honest.
        let source_shared = shared.clone();
        unsafe {
            webview.add_SourceChanged(
                &SourceChangedEventHandler::create(Box::new(move |webview, _| {
                    let Some(webview) = webview else {
                        return Ok(());
                    };
                    let mut source = PWSTR::null();
                    if webview.Source(&mut source).is_err() {
                        return Ok(());
                    }
                    let url = take_pwstr(source);
                    let (can_back, can_forward) = {
                        let mut back = windows::core::BOOL(0);
                        let mut forward = windows::core::BOOL(0);
                        let _ = webview.CanGoBack(&mut back);
                        let _ = webview.CanGoForward(&mut forward);
                        (back.as_bool(), forward.as_bool())
                    };
                    emit(
                        &source_shared,
                        BrowserEvent::Navigated {
                            url,
                            can_back,
                            can_forward,
                        },
                    );
                    Ok(())
                })),
                &mut token,
            )?;
        }
        let title_shared = shared.clone();
        unsafe {
            webview.add_DocumentTitleChanged(
                &DocumentTitleChangedEventHandler::create(Box::new(move |webview, _| {
                    let Some(webview) = webview else {
                        return Ok(());
                    };
                    let mut title = PWSTR::null();
                    if webview.DocumentTitle(&mut title).is_err() {
                        return Ok(());
                    }
                    emit(
                        &title_shared,
                        BrowserEvent::TitleChanged {
                            title: take_pwstr(title),
                        },
                    );
                    Ok(())
                })),
                &mut token,
            )?;
        }

        // Load failures surface where the chrome can say so.
        let load_shared = shared.clone();
        unsafe {
            webview.add_NavigationCompleted(
                &NavigationCompletedEventHandler::create(Box::new(move |webview, args| {
                    let Some(args) = args else {
                        return Ok(());
                    };
                    let mut success = windows::core::BOOL(0);
                    let _ = args.IsSuccess(&mut success);
                    if success.as_bool() {
                        return Ok(());
                    }
                    let mut status = COREWEBVIEW2_WEB_ERROR_STATUS_UNKNOWN;
                    let _ = args.WebErrorStatus(&mut status);
                    let url = webview
                        .and_then(|webview| {
                            let mut source = PWSTR::null();
                            webview.Source(&mut source).ok()?;
                            Some(take_pwstr(source))
                        })
                        .unwrap_or_default();
                    emit(
                        &load_shared,
                        BrowserEvent::LoadFailed {
                            url,
                            message: format!("navigation failed ({status:?})"),
                        },
                    );
                    Ok(())
                })),
                &mut token,
            )?;
        }
        Ok(())
    }

    pub fn set_bounds(&mut self, bounds: Bounds<Pixels>, scale: f32) {
        let Ok(rect) = physical_rect(bounds, scale) else {
            return;
        };
        let (host, controller) = {
            let mut state = self.shared.borrow_mut();
            if state.closed {
                return;
            }
            state.rect = rect;
            (state.host, state.controller.clone())
        };
        unsafe {
            let _ = SetWindowPos(
                host,
                None,
                rect.left,
                rect.top,
                rect.right - rect.left,
                rect.bottom - rect.top,
                SWP_NOACTIVATE | SWP_NOZORDER,
            );
            if let Some(controller) = controller {
                let _ = controller.SetBounds(RECT {
                    left: 0,
                    top: 0,
                    right: rect.right - rect.left,
                    bottom: rect.bottom - rect.top,
                });
            }
        }
    }

    pub fn set_visible(&mut self, visible: bool) {
        let (host, controller) = {
            let mut state = self.shared.borrow_mut();
            if state.closed || state.visible == visible {
                return;
            }
            state.visible = visible;
            (state.host, state.controller.clone())
        };
        unsafe {
            let _ = ShowWindow(host, if visible { SW_SHOWNOACTIVATE } else { SW_HIDE });
            if let Some(controller) = controller {
                let _ = controller.SetIsVisible(visible);
            }
        }
    }

    pub fn focus(&mut self) {
        let controller = self.shared.borrow().controller.clone();
        if let Some(controller) = controller {
            unsafe {
                let _ = controller.MoveFocus(COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC);
            }
        }
    }

    pub fn command(&mut self, command: BrowserCommand) -> Result<(), BrowserSurfaceError> {
        let webview = self.shared.borrow().webview.clone();
        let Some(webview) = webview else {
            return Err(BrowserSurfaceError::Platform(
                "the browser surface is not ready".into(),
            ));
        };
        if self.shared.borrow().closed {
            return Err(BrowserSurfaceError::Platform(
                "the browser surface is closed".into(),
            ));
        }
        unsafe {
            match command {
                BrowserCommand::Back => {
                    let _ = webview.GoBack();
                }
                BrowserCommand::Forward => {
                    let _ = webview.GoForward();
                }
                BrowserCommand::Reload => {
                    let _ = webview.Reload();
                }
                BrowserCommand::Navigate { url } => {
                    webview
                        .Navigate(&HSTRING::from(url.as_str()))
                        .map_err(|error| BrowserSurfaceError::Platform(error.to_string()))?;
                }
                BrowserCommand::SetRouteGuard { guard } => {
                    self.shared.borrow_mut().guard = guard;
                }
            }
        }
        Ok(())
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        let (controller, sender) = {
            let mut state = self.shared.borrow_mut();
            state.closed = true;
            state.webview = None;
            state.env = None;
            (state.controller.take(), state.sender.clone())
        };
        if let Some(controller) = controller {
            unsafe {
                let _ = controller.Close();
            }
        }
        unsafe {
            let _ = DestroyWindow(self.host);
        }
        // The per-Host partition directory is deliberately kept: it is the
        // durable site data this Host's pages accumulated, restored with the
        // tab on the next launch. Contrast the diagram surface, whose temp
        // profile is deleted with the presentation.
        let _ = sender.try_send(BrowserEvent::Closed);
    }
}

fn physical_rect(bounds: Bounds<Pixels>, scale: f32) -> Result<RECT, BrowserSurfaceError> {
    let x = (f32::from(bounds.origin.x) * scale).round() as i32;
    let y = (f32::from(bounds.origin.y) * scale).round() as i32;
    let width = (f32::from(bounds.size.width) * scale).round() as i32;
    let height = (f32::from(bounds.size.height) * scale).round() as i32;
    if width <= 0 || height <= 0 {
        return Err(BrowserSurfaceError::InvalidBounds);
    }
    Ok(RECT {
        left: x,
        top: y,
        right: x + width,
        bottom: y + height,
    })
}

unsafe fn argument_uri(
    args: &ICoreWebView2NavigationStartingEventArgs,
) -> windows::core::Result<String> {
    let mut uri = PWSTR::null();
    unsafe { args.Uri(&mut uri)? };
    Ok(take_pwstr(uri))
}

unsafe fn new_window_uri(
    args: &ICoreWebView2NewWindowRequestedEventArgs,
) -> windows::core::Result<String> {
    let mut uri = PWSTR::null();
    unsafe { args.Uri(&mut uri)? };
    Ok(take_pwstr(uri))
}

fn take_pwstr(ptr: PWSTR) -> String {
    if ptr.is_null() {
        return String::new();
    }
    let mut end = ptr;
    // SAFETY: WebView2 returns null-terminated string buffers owned by the
    // caller only for the length of this call; measuring then copying stays
    // inside the buffer.
    unsafe {
        while *end.0 != 0 {
            end = PWSTR(end.0.add(1));
        }
        let len = end.0.offset_from(ptr.0) as usize;
        let slice = std::slice::from_raw_parts(ptr.0, len);
        String::from_utf16_lossy(slice)
    }
}

fn emit(shared: &Rc<RefCell<Shared>>, event: BrowserEvent) {
    let sender = match shared.try_borrow() {
        Ok(state) => state.sender.clone(),
        Err(_) => return,
    };
    let _ = sender.try_send(event);
}

fn fail(shared: &Rc<RefCell<Shared>>, message: impl Into<String>) {
    {
        let mut state = shared.borrow_mut();
        if state.closed {
            return;
        }
        state.closed = true;
    }
    emit(
        shared,
        BrowserEvent::Failed {
            message: message.into(),
        },
    );
}
