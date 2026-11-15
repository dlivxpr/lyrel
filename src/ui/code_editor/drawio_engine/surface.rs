//! The narrow native surface for the fixed diagram engine. Beyond GET it
//! also answers the session's own commit/sync POST — the only write the
//! engine can make, sequence-gated by the live session and budgeted by the
//! document size cap.
//!
//! On Windows this is a WebView2 child window created directly through
//! `webview2-com`; there is no general-purpose browser, address bar, tab strip,
//! download manager or developer-tools path. Every navigation, resource,
//! permission, popup, download and dialog is answered by the allowlist in this
//! file. macOS uses the same contract around a WKWebView through a narrow wry
//! host. The native hosts are non-test platform components: lib tests compile a
//! surface type with no host and report truthfully that no WebView was started.

#[cfg(not(test))]
use gpui_kit::Window;
use gpui_kit::{Bounds, Pixels};
#[cfg(not(test))]
use smol::channel::Sender;
use std::sync::Arc;

use super::bridge::Command;
#[cfg(not(test))]
use super::session::PreviewTheme;

#[derive(Clone, Debug, PartialEq)]
// Undo, Redo, Paste and Clipboard are produced only by the platform hosts,
// so automated builds never construct them.
#[cfg_attr(test, allow(dead_code))]
pub(crate) enum SurfaceEvent {
    Ready,
    State {
        page: usize,
        pages: usize,
        zoom: f32,
    },
    /// The newest document the live engine produced — an edit, an engine
    /// replay of undo/redo, or a save flush. The owning document decides
    /// whether it is the canonical revision.
    Commit {
        bytes: Arc<[u8]>,
    },
    /// The engine asked for a workbench save (its own Ctrl+S).
    Save,
    /// The engine asked for the document's own undo/redo.
    Undo,
    Redo,
    /// The engine asked for the system clipboard's text.
    Paste,
    /// Serialized cells the engine copied or cut.
    Clipboard {
        text: String,
    },
    Blocked {
        reason: String,
    },
    Failed {
        message: String,
    },
    Closed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
// The platform arms are built only by the native hosts; automated builds
// keep the wording but never construct them.
#[cfg_attr(test, allow(dead_code))]
pub(crate) enum SurfaceError {
    /// Produced only by the Windows host's own geometry contract.
    #[cfg(all(not(test), windows))]
    InvalidBounds,
    /// Produced while capturing a parent handle on a desktop platform.
    #[cfg(all(not(test), any(windows, target_os = "macos")))]
    Window,
    Platform(String),
    /// The no-host answer on platforms without a desktop surface.
    #[cfg(not(any(windows, target_os = "macos")))]
    Unsupported(&'static str),
}

impl std::fmt::Display for SurfaceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            #[cfg(all(not(test), windows))]
            Self::InvalidBounds => f.write_str("the diagram pane has no drawable size"),
            #[cfg(all(not(test), any(windows, target_os = "macos")))]
            Self::Window => f.write_str("the window has no native handle for the diagram pane"),
            Self::Platform(message) => write!(f, "the diagram surface failed: {message}"),
            #[cfg(not(any(windows, target_os = "macos")))]
            Self::Unsupported(message) => f.write_str(message),
        }
    }
}

/// The GPUI window's native parent handle, captured while the window is
/// borrowed so surface creation can happen after paint without keeping a
/// `&mut Window` alive. The native hosts are non-test platform components, so
/// the handle type and its capture share that gate.
#[cfg(not(test))]
#[derive(Clone, Copy)]
pub(crate) struct ParentWindow {
    #[cfg(windows)]
    hwnd: windows::Win32::Foundation::HWND,
    #[cfg(target_os = "macos")]
    view: raw_window_handle::AppKitWindowHandle,
}

#[cfg(all(not(test), target_os = "macos"))]
impl raw_window_handle::HasWindowHandle for ParentWindow {
    fn window_handle(
        &self,
    ) -> Result<raw_window_handle::WindowHandle<'_>, raw_window_handle::HandleError> {
        let raw = raw_window_handle::RawWindowHandle::AppKit(self.view);
        // The handle stays valid while the GPUI window that produced it is
        // alive; the surface is dropped with its presentation.
        Ok(unsafe { raw_window_handle::WindowHandle::borrow_raw(raw) })
    }
}

#[cfg(not(test))]
impl ParentWindow {
    pub(crate) fn from_window(window: &Window) -> Result<Self, SurfaceError> {
        #[cfg(windows)]
        {
            let handle = raw_window_handle::HasWindowHandle::window_handle(window)
                .map_err(|_| SurfaceError::Window)?;
            return match handle.as_raw() {
                raw_window_handle::RawWindowHandle::Win32(handle) => Ok(Self {
                    hwnd: windows::Win32::Foundation::HWND(handle.hwnd.get() as *mut _),
                }),
                _ => Err(SurfaceError::Window),
            };
        }
        #[cfg(target_os = "macos")]
        {
            let handle = raw_window_handle::HasWindowHandle::window_handle(window)
                .map_err(|_| SurfaceError::Window)?;
            return match handle.as_raw() {
                raw_window_handle::RawWindowHandle::AppKit(handle) => Ok(Self { view: handle }),
                _ => Err(SurfaceError::Window),
            };
        }
        #[cfg(not(any(windows, target_os = "macos")))]
        {
            let _ = window;
            Err(SurfaceError::Unsupported(
                "the draw.io preview engine has no host on this platform",
            ))
        }
    }
}

/// Creates and owns one native child surface. Windows uses WebView2 directly;
/// macOS uses a WKWebView through wry's registry host. Both expose the same
/// session, bridge, asset and lifecycle contract.
/// The test-only stand-in for a live engine surface: it records the
/// commands production code sends and answers whether the engine would take
/// them, so the save flush, clipboard and page paths run through
/// `SurfaceHost::command` instead of a no-op.
#[cfg(test)]
pub(crate) struct TestSurface {
    pub(crate) commands: std::cell::RefCell<Vec<Command>>,
}

#[cfg(test)]
impl TestSurface {
    pub(crate) fn new() -> Self {
        Self {
            commands: std::cell::RefCell::new(Vec::new()),
        }
    }
}

pub(crate) struct SurfaceHost {
    #[cfg(all(windows, not(test)))]
    inner: windows_host::Host,
    #[cfg(all(target_os = "macos", not(test)))]
    inner: macos_host::Host,
    #[cfg(test)]
    inner: TestSurface,
    #[cfg(not(any(windows, target_os = "macos", test)))]
    _unsupported: std::marker::PhantomData<()>,
}

impl SurfaceHost {
    #[cfg(test)]
    pub(crate) fn test() -> Self {
        Self {
            inner: TestSurface::new(),
        }
    }

    #[cfg(test)]
    pub(crate) fn test_commands(&self) -> Vec<Command> {
        self.inner.commands.borrow().clone()
    }

    #[cfg(not(test))]
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn start(
        parent: ParentWindow,
        bounds: Bounds<Pixels>,
        scale: f32,
        document: Arc<[u8]>,
        theme: PreviewTheme,
        events: Sender<SurfaceEvent>,
    ) -> Result<Self, SurfaceError> {
        #[cfg(windows)]
        {
            return windows_host::Host::start(parent, bounds, scale, document, theme, events)
                .map(|inner| Self { inner });
        }
        #[cfg(target_os = "macos")]
        {
            let _ = scale;
            return macos_host::Host::start(parent, bounds, document, theme, events)
                .map(|inner| Self { inner });
        }
        #[cfg(not(any(windows, target_os = "macos")))]
        {
            let _ = (parent, bounds, scale, document, theme, events);
            Err(SurfaceError::Unsupported(
                "the draw.io preview engine has no host on this platform",
            ))
        }
    }

    pub(crate) fn set_bounds(&mut self, bounds: Bounds<Pixels>, scale: f32) {
        #[cfg(all(windows, not(test)))]
        self.inner.set_bounds(bounds, scale);
        #[cfg(all(target_os = "macos", not(test)))]
        {
            let _ = scale;
            self.inner.set_bounds(bounds);
        }
        #[cfg(any(test, not(any(windows, target_os = "macos"))))]
        {
            let _ = (bounds, scale);
        }
    }

    pub(crate) fn set_visible(&mut self, visible: bool) {
        #[cfg(all(windows, not(test)))]
        self.inner.set_visible(visible);
        #[cfg(all(target_os = "macos", not(test)))]
        self.inner.set_visible(visible);
        #[cfg(any(test, not(any(windows, target_os = "macos"))))]
        {
            let _ = visible;
        }
    }

    pub(crate) fn focus(&mut self) {
        #[cfg(all(windows, not(test)))]
        self.inner.focus();
        #[cfg(all(target_os = "macos", not(test)))]
        self.inner.focus();
    }

    /// Queues one command for the engine. `Ok(true)` means the message was
    /// accepted for delivery — the session is live and not failed; `Ok(false)`
    /// means the surface is up but the engine is not ready to receive it, so
    /// callers waiting on an answer must not wait on this surface.
    pub(crate) fn command(&mut self, command: Command) -> Result<bool, SurfaceError> {
        #[cfg(test)]
        {
            self.inner.commands.borrow_mut().push(command);
            return Ok(true);
        }
        #[cfg(all(windows, not(test)))]
        return self.inner.command(command);
        #[cfg(all(target_os = "macos", not(test)))]
        return self.inner.command(command);
        #[cfg(not(any(windows, target_os = "macos", test)))]
        {
            let _ = command;
            Err(SurfaceError::Unsupported(
                "the draw.io preview engine has no host on this platform",
            ))
        }
    }
}

#[cfg(all(windows, not(test)))]
mod windows_host {
    use super::{ParentWindow, PreviewTheme, SurfaceError, SurfaceEvent};
    use crate::ui::code_editor::drawio_engine::assets::{self, EngineResponse};
    use crate::ui::code_editor::drawio_engine::bridge::{BridgeEvent, Command, LiveSession};
    use gpui_kit::{Bounds, Pixels};
    use smol::channel::Sender;
    use std::cell::RefCell;
    use std::path::PathBuf;
    use std::rc::Rc;
    use std::result::Result;
    use std::sync::Arc;
    use webview2_com::Microsoft::Web::WebView2::Win32::*;
    use webview2_com::*;
    use windows::Win32::Foundation::{HWND, RECT};
    use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
    use windows::Win32::UI::Shell::SHCreateMemStream;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, SW_HIDE, SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SWP_NOZORDER,
        SetWindowPos, ShowWindow, WINDOW_EX_STYLE, WS_CHILD, WS_CLIPCHILDREN, WS_CLIPSIBLINGS,
        WS_VISIBLE,
    };
    use windows::core::{HSTRING, Interface, PCWSTR, PWSTR};

    struct Shared {
        host: HWND,
        data_dir: PathBuf,
        env: Option<ICoreWebView2Environment>,
        controller: Option<ICoreWebView2Controller>,
        webview: Option<ICoreWebView2>,
        session: Option<LiveSession>,
        theme: PreviewTheme,
        visible: bool,
        rect: RECT,
        closed: bool,
        sender: Sender<SurfaceEvent>,
    }

    pub(super) struct Host {
        shared: Rc<RefCell<Shared>>,
        host: HWND,
        data_dir: PathBuf,
    }

    impl Host {
        pub(super) fn start(
            parent: ParentWindow,
            bounds: Bounds<Pixels>,
            scale: f32,
            document: Arc<[u8]>,
            theme: PreviewTheme,
            events: Sender<SurfaceEvent>,
        ) -> Result<Self, SurfaceError> {
            let rect = physical_rect(bounds, scale)?;
            if rect.right <= rect.left || rect.bottom <= rect.top {
                return Err(SurfaceError::InvalidBounds);
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
            .map_err(|error| SurfaceError::Platform(error.to_string()))?;

            let data_dir = private_data_dir()?;
            let session = LiveSession::start(document)
                .map_err(|error| SurfaceError::Platform(error.to_string()))?;
            let shared = Rc::new(RefCell::new(Shared {
                host,
                data_dir: data_dir.clone(),
                env: None,
                controller: None,
                webview: None,
                session: Some(session),
                theme,
                visible: true,
                rect,
                closed: false,
                sender: events,
            }));
            unsafe {
                let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            }
            Self::create_environment(&shared);
            Ok(Self {
                shared,
                host,
                data_dir,
            })
        }

        fn create_environment(shared: &Rc<RefCell<Shared>>) {
            let (data_dir, closed) = {
                let state = shared.borrow();
                (state.data_dir.clone(), state.closed)
            };
            if closed {
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
            let handler = CreateCoreWebView2EnvironmentCompletedHandler::create(Box::new(
                move |result, env| {
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
                },
            ));
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
            let env_for_handler = env.clone();
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
                    Self::finish_controller(&handler_shared, env_for_handler, controller);
                    Ok(())
                },
            ));
            let created = unsafe {
                if let Ok(env10) = env.cast::<ICoreWebView2Environment10>() {
                    match env10.CreateCoreWebView2ControllerOptions() {
                        Ok(options) => {
                            let _ = options.SetIsInPrivateModeEnabled(true);
                            env10.CreateCoreWebView2ControllerWithOptions(host, &options, &handler)
                        }
                        Err(error) => Err(error),
                    }
                } else {
                    env.CreateCoreWebView2Controller(host, &handler)
                }
            };
            if let Err(error) = created {
                fail(shared, error.to_string());
            }
        }

        fn finish_controller(
            shared: &Rc<RefCell<Shared>>,
            env: ICoreWebView2Environment,
            controller: ICoreWebView2Controller,
        ) {
            let result = unsafe { Self::configure(shared, &env, &controller) };
            if let Err(error) = result {
                fail(shared, error.to_string());
                return;
            }
            let (bounds, visible) = {
                let mut state = shared.borrow_mut();
                if state.closed {
                    let _ = unsafe { controller.Close() };
                    return;
                }
                state.controller = Some(controller.clone());
                (state.rect, state.visible)
            };
            unsafe {
                // The controller's bounds are relative to its host HWND. The
                // host already sits at `bounds` inside the GPUI window; using
                // the window-relative rect here would offset the WebView twice.
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
            // The bind script is injected after navigation completes; see the
            // NavigationCompleted handler installed in `configure`.
            if let Err(error) = unsafe { webview.Navigate(&HSTRING::from(assets::SHELL_URL)) } {
                fail(shared, error.to_string());
            }
        }

        unsafe fn configure(
            shared: &Rc<RefCell<Shared>>,
            env: &ICoreWebView2Environment,
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
                settings.SetAreDevToolsEnabled(false)?;
                settings.SetAreDefaultContextMenusEnabled(false)?;
                settings.SetAreDefaultScriptDialogsEnabled(false)?;
                settings.SetIsWebMessageEnabled(true)?;
                settings.SetAreHostObjectsAllowed(false)?;
                settings.SetIsStatusBarEnabled(false)?;
                settings.SetIsZoomControlEnabled(false)?;
                settings.SetIsScriptEnabled(true)?;
                settings.SetIsBuiltInErrorPageEnabled(false)?;
            }
            if let Ok(settings3) = settings.cast::<ICoreWebView2Settings3>() {
                let _ = unsafe { settings3.SetAreBrowserAcceleratorKeysEnabled(false) };
            }
            if let Ok(settings4) = settings.cast::<ICoreWebView2Settings4>() {
                let _ = unsafe { settings4.SetIsGeneralAutofillEnabled(false) };
                let _ = unsafe { settings4.SetIsPasswordAutosaveEnabled(false) };
            }
            if let Ok(settings5) = settings.cast::<ICoreWebView2Settings5>() {
                let _ = unsafe { settings5.SetIsPinchZoomEnabled(false) };
            }
            if let Ok(settings6) = settings.cast::<ICoreWebView2Settings6>() {
                let _ = unsafe { settings6.SetIsSwipeNavigationEnabled(false) };
            }
            if let Ok(settings8) = settings.cast::<ICoreWebView2Settings8>() {
                let _ = unsafe { settings8.SetIsReputationCheckingRequired(false) };
            }
            if let Ok(controller4) = controller.cast::<ICoreWebView2Controller4>() {
                unsafe { controller4.SetAllowExternalDrop(false)? };
            } else {
                return Err(windows::core::Error::from(
                    windows::Win32::Foundation::E_NOINTERFACE,
                ));
            }
            let webview4: ICoreWebView2_4 = webview.cast()?;
            let webview22: ICoreWebView2_22 = webview.cast()?;
            unsafe {
                webview22.AddWebResourceRequestedFilterWithRequestSourceKinds(
                    windows::core::w!("*"),
                    COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL,
                    COREWEBVIEW2_WEB_RESOURCE_REQUEST_SOURCE_KINDS_ALL,
                )?;
            }

            let mut token = 0i64;
            let resource_shared = shared.clone();
            let resource_env = env.clone();
            unsafe {
                webview.add_WebResourceRequested(
                    &WebResourceRequestedEventHandler::create(Box::new(move |_, args| {
                        let Some(args) = args else {
                            return Ok(());
                        };
                        respond(&resource_shared, &resource_env, &args);
                        Ok(())
                    })),
                    &mut token,
                )?;
            }

            let nav_shared = shared.clone();
            unsafe {
                webview.add_NavigationStarting(
                    &NavigationStartingEventHandler::create(Box::new(move |_, args| {
                        let Some(args) = args else {
                            return Ok(());
                        };
                        let uri = argument_uri(&args)?;
                        if uri != assets::SHELL_URL {
                            let _ = args.SetCancel(true);
                            emit(
                                &nav_shared,
                                SurfaceEvent::Blocked {
                                    reason: format!("navigation to {uri}"),
                                },
                            );
                        }
                        Ok(())
                    })),
                    &mut token,
                )?;
            }
            let frame_shared = shared.clone();
            unsafe {
                webview.add_FrameNavigationStarting(
                    &NavigationStartingEventHandler::create(Box::new(move |_, args| {
                        let Some(args) = args else {
                            return Ok(());
                        };
                        let uri = argument_uri(&args)?;
                        let allowed = uri == assets::ENGINE_URL || uri == "about:blank";
                        if !allowed {
                            let _ = args.SetCancel(true);
                            emit(
                                &frame_shared,
                                SurfaceEvent::Blocked {
                                    reason: format!("frame navigation to {uri}"),
                                },
                            );
                        }
                        Ok(())
                    })),
                    &mut token,
                )?;
            }

            let new_window_shared = shared.clone();
            unsafe {
                webview.add_NewWindowRequested(
                    &NewWindowRequestedEventHandler::create(Box::new(move |_, args| {
                        let Some(args) = args else {
                            return Ok(());
                        };
                        let _ = args.SetHandled(true);
                        emit(
                            &new_window_shared,
                            SurfaceEvent::Blocked {
                                reason: "a new window".to_string(),
                            },
                        );
                        Ok(())
                    })),
                    &mut token,
                )?;
            }
            let download_shared = shared.clone();
            unsafe {
                webview4.add_DownloadStarting(
                    &DownloadStartingEventHandler::create(Box::new(move |_, args| {
                        let Some(args) = args else {
                            return Ok(());
                        };
                        let _ = args.SetCancel(true);
                        emit(
                            &download_shared,
                            SurfaceEvent::Blocked {
                                reason: "a download".to_string(),
                            },
                        );
                        Ok(())
                    })),
                    &mut token,
                )?;
            }
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
                            SurfaceEvent::Blocked {
                                reason: "a browser permission".to_string(),
                            },
                        );
                        Ok(())
                    })),
                    &mut token,
                )?;
            }
            let message_shared = shared.clone();
            unsafe {
                webview.add_WebMessageReceived(
                    &WebMessageReceivedEventHandler::create(Box::new(move |_, args| {
                        let Some(args) = args else {
                            return Ok(());
                        };
                        let mut source = PWSTR::null();
                        if args.Source(&mut source).is_err() {
                            return Ok(());
                        }
                        let source = take_pwstr(source);
                        let mut body = PWSTR::null();
                        let body = match args.TryGetWebMessageAsString(&mut body) {
                            Ok(()) => take_pwstr(body),
                            Err(_) => return Ok(()),
                        };
                        let event = {
                            let mut state = message_shared.borrow_mut();
                            if state.closed {
                                return Ok(());
                            }
                            let Some(session) = state.session.as_mut() else {
                                return Ok(());
                            };
                            session.accept(&source, body.as_bytes())
                        };
                        match event {
                            Ok(event) => emit_bridge(&message_shared, event),
                            Err(_) => {
                                // A stale, foreign or malformed message is
                                // dropped without changing the document.
                            }
                        }
                        Ok(())
                    })),
                    &mut token,
                )?;
            }
            let bind_shared = shared.clone();
            unsafe {
                webview.add_NavigationCompleted(
                    &NavigationCompletedEventHandler::create(Box::new(move |webview, args| {
                        let Some(args) = args else {
                            return Ok(());
                        };
                        let mut success = windows::core::BOOL(0);
                        let _ = args.IsSuccess(&mut success);
                        if !success.as_bool() {
                            fail(&bind_shared, "the diagram host page failed to load");
                            return Ok(());
                        }
                        let Some(webview) = webview else {
                            return Ok(());
                        };
                        let mut source = PWSTR::null();
                        if webview.Source(&mut source).is_err() {
                            return Ok(());
                        }
                        let source = take_pwstr(source);
                        if source != assets::SHELL_URL {
                            return Ok(());
                        }
                        let script = {
                            let state = bind_shared.borrow();
                            state
                                .session
                                .as_ref()
                                .map(|session| session.bind_script(state.theme))
                        };
                        let Some(script) = script else {
                            return Ok(());
                        };
                        let handler =
                            ExecuteScriptCompletedHandler::create(Box::new(|_, _| Ok(())));
                        let _ = webview.ExecuteScript(&HSTRING::from(script), &handler);
                        Ok(())
                    })),
                    &mut token,
                )?;
            }
            Ok(())
        }

        pub(super) fn set_bounds(&mut self, bounds: Bounds<Pixels>, scale: f32) {
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

        pub(super) fn set_visible(&mut self, visible: bool) {
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

        pub(super) fn focus(&mut self) {
            let controller = self.shared.borrow().controller.clone();
            if let Some(controller) = controller {
                unsafe {
                    let _ = controller.MoveFocus(COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC);
                }
            }
        }

        pub(super) fn command(&mut self, command: Command) -> Result<bool, SurfaceError> {
            let message = {
                let mut state = self.shared.borrow_mut();
                if state.closed {
                    return Err(SurfaceError::Platform(
                        "the diagram surface is closed".into(),
                    ));
                }
                state
                    .session
                    .as_mut()
                    .and_then(|session| session.command(&command))
            };
            let Some(message) = message else {
                return Ok(false);
            };
            let webview = self.shared.borrow().webview.clone();
            let Some(webview) = webview else {
                return Err(SurfaceError::Platform(
                    "the diagram surface is not ready".into(),
                ));
            };
            unsafe {
                webview
                    .PostWebMessageAsJson(&HSTRING::from(message))
                    .map_err(|error| SurfaceError::Platform(error.to_string()))?;
            }
            Ok(true)
        }
    }

    impl Drop for Host {
        fn drop(&mut self) {
            let (controller, sender) = {
                let mut state = self.shared.borrow_mut();
                state.closed = true;
                state.session = None;
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
            let _ = std::fs::remove_dir_all(&self.data_dir);
            let _ = sender.try_send(SurfaceEvent::Closed);
        }
    }

    fn physical_rect(bounds: Bounds<Pixels>, scale: f32) -> Result<RECT, SurfaceError> {
        let x = (f32::from(bounds.origin.x) * scale).round() as i32;
        let y = (f32::from(bounds.origin.y) * scale).round() as i32;
        let width = (f32::from(bounds.size.width) * scale).round() as i32;
        let height = (f32::from(bounds.size.height) * scale).round() as i32;
        if width <= 0 || height <= 0 {
            return Err(SurfaceError::InvalidBounds);
        }
        Ok(RECT {
            left: x,
            top: y,
            right: x + width,
            bottom: y + height,
        })
    }

    fn private_data_dir() -> Result<PathBuf, SurfaceError> {
        let mut suffix = [0u8; 16];
        getrandom::fill(&mut suffix).map_err(|error| SurfaceError::Platform(error.to_string()))?;
        let mut name = String::from("lyrel-drawio-");
        for byte in suffix {
            name.push(char::from(b"0123456789abcdef"[(byte >> 4) as usize]));
            name.push(char::from(b"0123456789abcdef"[(byte & 0xf) as usize]));
        }
        let path = std::env::temp_dir().join(name);
        std::fs::create_dir_all(&path)
            .map_err(|error| SurfaceError::Platform(error.to_string()))?;
        Ok(path)
    }

    unsafe fn argument_uri(
        args: &ICoreWebView2NavigationStartingEventArgs,
    ) -> windows::core::Result<String> {
        let mut uri = PWSTR::null();
        unsafe { args.Uri(&mut uri)? };
        Ok(take_pwstr(uri))
    }

    fn emit(shared: &Rc<RefCell<Shared>>, event: SurfaceEvent) {
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
            state.session = None;
        }
        emit(
            shared,
            SurfaceEvent::Failed {
                message: message.into(),
            },
        );
    }

    fn emit_bridge(shared: &Rc<RefCell<Shared>>, event: BridgeEvent) {
        match event {
            BridgeEvent::Ready => emit(shared, SurfaceEvent::Ready),
            BridgeEvent::State { page, pages, zoom } => {
                emit(shared, SurfaceEvent::State { page, pages, zoom })
            }
            BridgeEvent::Save => emit(shared, SurfaceEvent::Save),
            BridgeEvent::Undo => emit(shared, SurfaceEvent::Undo),
            BridgeEvent::Redo => emit(shared, SurfaceEvent::Redo),
            BridgeEvent::Paste => emit(shared, SurfaceEvent::Paste),
            BridgeEvent::Clipboard { text, .. } => emit(shared, SurfaceEvent::Clipboard { text }),
            BridgeEvent::Failed { message } => emit(shared, SurfaceEvent::Failed { message }),
        }
    }

    fn respond(
        shared: &Rc<RefCell<Shared>>,
        env: &ICoreWebView2Environment,
        args: &ICoreWebView2WebResourceRequestedEventArgs,
    ) {
        let Ok(request) = (unsafe { args.Request() }) else {
            return;
        };
        let mut method = PWSTR::null();
        if unsafe { request.Method(&mut method) }.is_err() {
            return;
        }
        let method = take_pwstr(method);
        let mut uri = PWSTR::null();
        if unsafe { request.Uri(&mut uri) }.is_err() {
            return;
        }
        let uri = take_pwstr(uri);
        // Inline images the document already owns are permitted by CSP; the
        // engine never gets a data: navigation because NavigationStarting and
        // frame-src reject those documents.
        if uri.starts_with("data:") || uri.starts_with("blob:") {
            return;
        }
        // A commit carries the diagram in its body; every other request is a
        // plain GET. The body is read with a hard cap so an engine page can
        // never make the host buffer an unbounded upload.
        let mut post_body: Option<Vec<u8>> = None;
        if method == "POST" {
            let Ok(content) = (unsafe { request.Content() }) else {
                return;
            };
            let Some(body) = (unsafe { read_request_body(&content) }) else {
                return;
            };
            post_body = Some(body);
        }
        let (response, commit) = {
            let mut state = match shared.try_borrow_mut() {
                Ok(state) => state,
                Err(_) => return,
            };
            assets::respond(&method, &uri, state.session.as_mut(), post_body.as_deref())
        };
        if let Some(bytes) = commit {
            emit(shared, SurfaceEvent::Commit { bytes });
        }
        if let Ok(native) = native_response(env, &response) {
            let _ = unsafe { args.SetResponse(&native) };
        }
    }

    /// Reads a POST request stream with a hard ceiling: anything larger than
    /// the document budget plus slack is cut off and refused by `respond`.
    unsafe fn read_request_body(stream: &windows::Win32::System::Com::IStream) -> Option<Vec<u8>> {
        const LIMIT: usize = assets::MAX_DOCUMENT_BYTES + 1;
        let mut body = Vec::with_capacity(64 * 1024);
        let mut chunk = [0u8; 16 * 1024];
        loop {
            let mut read = 0u32;
            let result = unsafe {
                stream.Read(
                    chunk.as_mut_ptr().cast(),
                    chunk.len() as u32,
                    Some(&mut read),
                )
            };
            if !result.is_ok() || read == 0 {
                break;
            }
            body.extend_from_slice(&chunk[..read as usize]);
            if body.len() > LIMIT {
                break;
            }
        }
        Some(body)
    }

    fn native_response(
        env: &ICoreWebView2Environment,
        response: &EngineResponse,
    ) -> windows::core::Result<ICoreWebView2WebResourceResponse> {
        let mut headers = format!("Content-Type: {}\n", response.content_type);
        headers.push_str(&format!(
            "Content-Length: {}\n",
            response.body.bytes().len()
        ));
        for (name, value) in &response.headers {
            headers.push_str(name);
            headers.push_str(": ");
            headers.push_str(value);
            headers.push('\n');
        }
        let reason = HSTRING::from(response.reason);
        let headers = HSTRING::from(headers);
        let body = response.body.bytes();
        let stream = if body.is_empty() {
            None
        } else {
            unsafe { SHCreateMemStream(Some(body)) }
        };
        unsafe {
            env.CreateWebResourceResponse(
                stream.as_ref(),
                response.status as i32,
                &reason,
                &headers,
            )
        }
    }
}

#[cfg(all(target_os = "macos", not(test)))]
mod macos_host {
    use super::{ParentWindow, PreviewTheme, SurfaceError, SurfaceEvent};
    use crate::ui::code_editor::drawio_engine::assets::{self, EngineResponse};
    use crate::ui::code_editor::drawio_engine::bridge::{BridgeEvent, Command, LiveSession};
    use gpui_kit::{Bounds, Pixels};
    use parking_lot::Mutex;
    use smol::channel::Sender;
    use std::borrow::Cow;
    use std::cell::RefCell;
    use std::path::PathBuf;
    use std::rc::Rc;
    use std::sync::Arc;
    use wry::dpi::{LogicalPosition, LogicalSize, Position, Size};
    use wry::http::Response as HttpResponse;
    use wry::{
        NewWindowResponse, PageLoadEvent, PermissionKind, PermissionResponse, Rect, WebView,
        WebViewBuilder,
    };

    struct Shared {
        session: LiveSession,
        sender: Sender<SurfaceEvent>,
        closed: bool,
    }

    pub(super) struct Host {
        // Wry's protocol callback is Send + Sync; native WebView access stays
        // in the main-thread-only slot below, outside this session lock.
        shared: Arc<Mutex<Shared>>,
        webview: Rc<RefCell<Option<WebView>>>,
    }

    impl Host {
        pub(super) fn start(
            parent: ParentWindow,
            bounds: Bounds<Pixels>,
            document: Arc<[u8]>,
            theme: PreviewTheme,
            events: Sender<SurfaceEvent>,
        ) -> Result<Self, SurfaceError> {
            let session = LiveSession::start_for(document, assets::MAC_SHELL_URL)
                .map_err(|error| SurfaceError::Platform(error.to_string()))?;
            let shared = Arc::new(Mutex::new(Shared {
                session,
                sender: events.clone(),
                closed: false,
            }));
            let slot: Rc<RefCell<Option<WebView>>> = Rc::new(RefCell::new(None));

            let protocol_shared = shared.clone();
            let protocol = move |_id: &str, request: wry::http::Request<Vec<u8>>| {
                let method = request.method().as_str().to_string();
                let uri = request.uri().to_string();
                let body = request.body().clone();
                let (response, commit) = {
                    let mut state = protocol_shared.lock();
                    if state.closed {
                        return HttpResponse::new(Cow::Owned(Vec::new()));
                    }
                    assets::respond_from(
                        &method,
                        &uri,
                        assets::MAC_ORIGIN,
                        Some(&mut state.session),
                        Some(body.as_slice()),
                    )
                };
                if let Some(bytes) = commit {
                    let sender = protocol_shared.lock().sender.clone();
                    let _ = sender.try_send(SurfaceEvent::Commit { bytes });
                }
                http_response(response)
            };

            let ipc_shared = shared.clone();
            let ipc = move |request: wry::http::Request<String>| {
                let source = request.uri().to_string();
                let body = request.body().as_bytes().to_vec();
                let event = {
                    let mut state = ipc_shared.lock();
                    if state.closed {
                        return;
                    }
                    state.session.accept(&source, &body)
                };
                match event {
                    Ok(event) => emit_bridge(&ipc_shared, event),
                    Err(_) => {
                        // Stale, foreign or malformed messages are dropped.
                    }
                }
            };

            let nav_sender = events.clone();
            let navigation = move |url: String| {
                let allowed = url == assets::MAC_SHELL_URL
                    || url == assets::MAC_ENGINE_URL
                    || url == "about:blank";
                if !allowed {
                    let _ = nav_sender.try_send(SurfaceEvent::Blocked {
                        reason: format!("navigation to {url}"),
                    });
                }
                allowed
            };
            let popup_sender = events.clone();
            let new_window = move |_url: String, _features: wry::NewWindowFeatures| {
                let _ = popup_sender.try_send(SurfaceEvent::Blocked {
                    reason: "a new window".to_string(),
                });
                NewWindowResponse::Deny
            };
            let download_sender = events.clone();
            let mut download = move |_url: String, _path: &mut PathBuf| {
                let _ = download_sender.try_send(SurfaceEvent::Blocked {
                    reason: "a download".to_string(),
                });
                false
            };
            let permission_sender = events.clone();
            let permission = move |_kind: PermissionKind| {
                let _ = permission_sender.try_send(SurfaceEvent::Blocked {
                    reason: "a browser permission".to_string(),
                });
                PermissionResponse::Deny
            };
            let drag_sender = events.clone();
            let drag = move |_event: wry::DragDropEvent| {
                let _ = drag_sender.try_send(SurfaceEvent::Blocked {
                    reason: "a dragged file".to_string(),
                });
                true
            };
            let load_shared = shared.clone();
            let load_slot = slot.clone();
            let load = move |event: PageLoadEvent, url: String| {
                if !matches!(event, PageLoadEvent::Finished) || url != assets::MAC_SHELL_URL {
                    return;
                }
                let script = {
                    let state = load_shared.lock();
                    state.session.bind_script(theme)
                };
                if let Some(webview) = load_slot.borrow().as_ref() {
                    let _ = webview.evaluate_script(&script);
                }
            };

            let builder = WebViewBuilder::new()
                .with_url(assets::MAC_SHELL_URL)
                .with_bounds(rect(bounds))
                .with_visible(true)
                .with_accept_first_mouse(true)
                .with_incognito(true)
                .with_custom_protocol(assets::MAC_SCHEME.to_string(), protocol)
                .with_ipc_handler(ipc)
                .with_navigation_handler(navigation)
                .with_new_window_req_handler(new_window)
                .with_download_started_handler(download)
                .with_permission_handler(permission)
                .with_drag_drop_handler(drag)
                .with_on_page_load_handler(load);
            let webview = builder
                .build_as_child(&parent)
                .map_err(|error| SurfaceError::Platform(error.to_string()))?;
            *slot.borrow_mut() = Some(webview);
            Ok(Self {
                shared,
                webview: slot,
            })
        }

        pub(super) fn set_bounds(&mut self, bounds: Bounds<Pixels>) {
            if let Some(webview) = self.webview.borrow().as_ref() {
                let _ = webview.set_bounds(rect(bounds));
            }
        }

        pub(super) fn set_visible(&mut self, visible: bool) {
            if let Some(webview) = self.webview.borrow().as_ref() {
                let _ = webview.set_visible(visible);
            }
        }

        pub(super) fn focus(&mut self) {
            if let Some(webview) = self.webview.borrow().as_ref() {
                let _ = webview.focus();
            }
        }

        pub(super) fn command(&mut self, command: Command) -> Result<bool, SurfaceError> {
            let message = {
                let mut state = self.shared.lock();
                if state.closed {
                    return Err(SurfaceError::Platform(
                        "the diagram surface is closed".into(),
                    ));
                }
                state.session.command(&command)
            };
            let Some(message) = message else {
                return Ok(false);
            };
            let webview = self.webview.borrow();
            let Some(webview) = webview.as_ref() else {
                return Err(SurfaceError::Platform(
                    "the diagram surface is not ready".into(),
                ));
            };
            webview
                .evaluate_script(&format!("window.__lyrel.nativeMessage({message})"))
                .map_err(|error| SurfaceError::Platform(error.to_string()))?;
            Ok(true)
        }
    }

    impl Drop for Host {
        fn drop(&mut self) {
            let sender = {
                let mut shared = self.shared.lock();
                shared.closed = true;
                shared.sender.clone()
            };
            let _ = self.webview.borrow_mut().take();
            let _ = sender.try_send(SurfaceEvent::Closed);
        }
    }

    fn rect(bounds: Bounds<Pixels>) -> Rect {
        Rect {
            position: Position::Logical(LogicalPosition::new(
                f64::from(f32::from(bounds.origin.x)),
                f64::from(f32::from(bounds.origin.y)),
            )),
            size: Size::Logical(LogicalSize::new(
                f64::from(f32::from(bounds.size.width)),
                f64::from(f32::from(bounds.size.height)),
            )),
        }
    }

    fn http_response(response: EngineResponse) -> HttpResponse<Cow<'static, [u8]>> {
        let mut builder = HttpResponse::builder().status(response.status);
        builder = builder.header("Content-Type", response.content_type);
        builder = builder.header("Content-Length", response.body.bytes().len().to_string());
        for (name, value) in &response.headers {
            builder = builder.header(*name, value.as_str());
        }
        match builder.body(Cow::Owned(response.body.bytes().to_vec())) {
            Ok(response) => response,
            Err(_) => HttpResponse::new(Cow::Owned(Vec::new())),
        }
    }

    fn emit_bridge(shared: &Arc<Mutex<Shared>>, event: BridgeEvent) {
        let sender = shared.lock().sender.clone();
        let event = match event {
            BridgeEvent::Ready => SurfaceEvent::Ready,
            BridgeEvent::State { page, pages, zoom } => SurfaceEvent::State { page, pages, zoom },
            BridgeEvent::Save => SurfaceEvent::Save,
            BridgeEvent::Undo => SurfaceEvent::Undo,
            BridgeEvent::Redo => SurfaceEvent::Redo,
            BridgeEvent::Paste => SurfaceEvent::Paste,
            BridgeEvent::Clipboard { text, .. } => SurfaceEvent::Clipboard { text },
            BridgeEvent::Failed { message } => SurfaceEvent::Failed { message },
        };
        let _ = sender.try_send(event);
    }
}
