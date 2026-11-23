//! The macOS browsing surface: one native `WKWebView` inside the GPUI
//! window, driven directly through `objc2-web-kit` (#392).
//!
//! The direct engine is deliberate. `wry` 0.57's macOS host cannot give the
//! browser the two things this ticket needs at once: its new-window callback
//! only receives a URL, not the `WKNavigationAction` that says whether a user
//! clicked the link (`WKNavigationTypeLinkActivated`), and its child
//! `WebContext` does not select a `WKWebsiteDataStore`. Owning the view lets
//! this host do both:
//!
//! - Site data is one identified `WKWebsiteDataStore` per Host, created with
//!   `dataStoreForIdentifier:` from the same stable key the Windows profile
//!   directory uses. Two Hosts never share cookies or localStorage, and a
//!   Host's data survives a restart.
//! - The navigation delegate is the route guard: a Host-bound page may only
//!   load the loopback origin the panel verified, and a loopback request for
//!   any other origin is cancelled and reported as `RouteBlocked` before it
//!   can reach a coincident desktop service.
//! - A user-initiated `target=_blank` (LinkActivated) becomes a sibling tab
//!   event; every script-opened or unclassified popup is denied.
//!
//! There is no page → workbench bridge, no custom protocol, no message
//! handler. Device permissions, downloads, script dialogs, file panels and
//! dragged files are denied. The only ways in are the surface commands; the
//! only ways out are the events.

use super::super::route::BrowserSiteData;
use super::{BrowserCommand, BrowserEvent, BrowserParentWindow, BrowserSurfaceError, RouteGuard};
use gpui_kit::{Bounds, Pixels};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{NSResponder, NSView};
use objc2_foundation::{
    NSArray, NSDictionary, NSError, NSKeyValueChangeKey, NSKeyValueObservingOptions,
    NSObjectNSKeyValueObserverRegistration, NSPoint, NSRect, NSSize, NSString, NSURL, NSURLRequest,
    NSUUID, ns_string,
};
use objc2_web_kit::{
    WKFrameInfo, WKMediaCaptureType, WKNavigation, WKNavigationAction, WKNavigationActionPolicy,
    WKNavigationDelegate, WKNavigationResponse, WKNavigationResponsePolicy, WKNavigationType,
    WKOpenPanelParameters, WKPermissionDecision, WKSecurityOrigin, WKUIDelegate, WKWebView,
    WKWebViewConfiguration, WKWebsiteDataStore, WKWindowFeatures,
};
use smol::channel::Sender;
use std::cell::RefCell;
use std::ffi::c_void;
use std::ptr::null_mut;
use std::rc::Rc;

struct Shared {
    sender: Sender<BrowserEvent>,
    guard: RouteGuard,
    closed: bool,
}

pub struct Host {
    shared: Rc<RefCell<Shared>>,
    webview: Retained<WKWebView>,
    parent: Retained<NSView>,
    // Kept alive for as long as the view is: the view holds them weakly.
    _navigation: Retained<BrowserNavigationDelegate>,
    _ui: Retained<BrowserUiDelegate>,
    _title: Retained<BrowserTitleObserver>,
}

impl Host {
    pub fn start(
        parent: BrowserParentWindow,
        bounds: Bounds<Pixels>,
        url: &str,
        site: BrowserSiteData,
        guard: RouteGuard,
        events: Sender<BrowserEvent>,
    ) -> Result<Self, BrowserSurfaceError> {
        let mtm = MainThreadMarker::new().ok_or_else(|| {
            BrowserSurfaceError::Platform("the browser must start on the main thread".into())
        })?;
        // SAFETY: the handle stays valid while the GPUI window that produced
        // it is alive, and the surface is dropped with its presentation.
        let parent_view =
            unsafe { Retained::retain(parent.view.ns_view.as_ptr().cast::<NSView>()) }.ok_or_else(
                || {
                    BrowserSurfaceError::Platform(
                        "the window has no view to anchor the page".into(),
                    )
                },
            )?;

        let configuration = unsafe { WKWebViewConfiguration::new(mtm) };
        let identifier = NSUUID::from_bytes(super::super::route::site_data_identifier(&site.key));
        let data_store = unsafe { WKWebsiteDataStore::dataStoreForIdentifier(&identifier, mtm) };
        unsafe { configuration.setWebsiteDataStore(&data_store) };

        let webview = unsafe {
            WKWebView::initWithFrame_configuration(
                WKWebView::alloc(mtm),
                frame(&parent_view, bounds),
                &configuration,
            )
        };

        let shared = Rc::new(RefCell::new(Shared {
            sender: events,
            guard,
            closed: false,
        }));
        let navigation = BrowserNavigationDelegate::new(shared.clone(), mtm);
        let ui = BrowserUiDelegate::new(shared.clone(), mtm);
        unsafe {
            webview.setNavigationDelegate(Some(ProtocolObject::from_ref(&*navigation)));
            webview.setUIDelegate(Some(ProtocolObject::from_ref(&*ui)));
            parent_view.addSubview(&webview);
            webview.setHidden(false);
        }
        let title = BrowserTitleObserver::new(webview.clone(), shared.clone(), mtm);
        load(&webview, url)?;
        Ok(Self {
            shared,
            webview,
            parent: parent_view,
            _navigation: navigation,
            _ui: ui,
            _title: title,
        })
    }

    pub fn set_bounds(&mut self, bounds: Bounds<Pixels>) {
        self.webview.setFrame(frame(&self.parent, bounds));
    }

    pub fn set_visible(&mut self, visible: bool) {
        self.webview.setHidden(!visible);
    }

    pub fn focus(&mut self) {
        if let Some(window) = self.webview.window() {
            let responder: &NSResponder = &self.webview;
            let _ = window.makeFirstResponder(Some(responder));
        }
    }

    pub fn has_focus(&self) -> bool {
        self.webview
            .window()
            .and_then(|window| window.firstResponder())
            .and_then(|responder| responder.downcast::<NSView>().ok())
            .is_some_and(|view| view.isDescendantOf(&self.webview))
    }

    pub fn command(&mut self, command: BrowserCommand) -> Result<(), BrowserSurfaceError> {
        if self.shared.borrow().closed {
            return Err(BrowserSurfaceError::Platform(
                "the browser surface is closed".into(),
            ));
        }
        match command {
            BrowserCommand::Back => {
                unsafe { self.webview.goBack() };
            }
            BrowserCommand::Forward => {
                unsafe { self.webview.goForward() };
            }
            BrowserCommand::Reload => {
                unsafe { self.webview.reload() };
            }
            BrowserCommand::Navigate { url } => load(&self.webview, &url)?,
            BrowserCommand::SetRouteGuard { guard } => {
                self.shared.borrow_mut().guard = guard;
            }
        }
        Ok(())
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        let sender = {
            let mut shared = self.shared.borrow_mut();
            shared.closed = true;
            shared.sender.clone()
        };
        unsafe {
            self.webview.setNavigationDelegate(None);
            self.webview.setUIDelegate(None);
            self.webview.removeFromSuperview();
        }
        let _ = sender.try_send(BrowserEvent::Closed);
    }
}

/// The child view's frame in the parent's coordinate space. AppKit's default
/// view coordinates are bottom-left based; GPUI's are top-left based, so a
/// non-flipped parent needs the y flipped (the same conversion wry's child
/// host performs).
fn frame(parent: &NSView, bounds: Bounds<Pixels>) -> NSRect {
    let x = f64::from(f32::from(bounds.origin.x));
    let y = f64::from(f32::from(bounds.origin.y));
    let width = f64::from(f32::from(bounds.size.width));
    let height = f64::from(f32::from(bounds.size.height));
    let origin_y = if parent.isFlipped() {
        y
    } else {
        parent.frame().size.height - y - height
    };
    NSRect::new(NSPoint::new(x, origin_y), NSSize::new(width, height))
}

fn load(webview: &WKWebView, url: &str) -> Result<(), BrowserSurfaceError> {
    let Some(ns_url) = NSURL::URLWithString(&NSString::from_str(url)) else {
        return Err(BrowserSurfaceError::Platform(format!(
            "the browser cannot load {url}"
        )));
    };
    unsafe { webview.loadRequest(&NSURLRequest::requestWithURL(&ns_url)) };
    Ok(())
}

fn current_url(webview: &WKWebView) -> String {
    unsafe {
        webview
            .URL()
            .and_then(|url| url.absoluteString())
            .map(|url| url.to_string())
            .unwrap_or_default()
    }
}

fn current_title(webview: &WKWebView) -> String {
    unsafe {
        webview
            .title()
            .map(|title| title.to_string())
            .unwrap_or_default()
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

struct NavigationIvars {
    shared: Rc<RefCell<Shared>>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = NavigationIvars]
    struct BrowserNavigationDelegate;

    unsafe impl NSObjectProtocol for BrowserNavigationDelegate {}

    unsafe impl WKNavigationDelegate for BrowserNavigationDelegate {
        #[unsafe(method(webView:decidePolicyForNavigationAction:decisionHandler:))]
        fn decide_policy(
            &self,
            _webview: &WKWebView,
            action: &WKNavigationAction,
            handler: &block2::DynBlock<dyn Fn(WKNavigationActionPolicy)>,
        ) {
            let request = unsafe { action.request() };
            let Some(url) =
                (unsafe { request.URL() }).and_then(|url| unsafe { url.absoluteString() })
            else {
                handler.call((WKNavigationActionPolicy::Cancel,));
                return;
            };
            let url = url.to_string();
            if super::super::policy::classify(&url) == super::super::policy::Route::Deny {
                emit(
                    &self.ivars().shared,
                    BrowserEvent::Blocked {
                        reason: format!("navigation to {url}"),
                    },
                );
                handler.call((WKNavigationActionPolicy::Cancel,));
                return;
            }
            let allowed = self
                .ivars()
                .shared
                .try_borrow()
                .is_ok_and(|state| state.guard.allows(&url));
            if !allowed {
                emit(&self.ivars().shared, BrowserEvent::RouteBlocked { url });
                handler.call((WKNavigationActionPolicy::Cancel,));
                return;
            }
            // A `download` attribute is a download, not a page.
            let should_download = unsafe { action.shouldPerformDownload() };
            handler.call((if should_download {
                WKNavigationActionPolicy::Cancel
            } else {
                WKNavigationActionPolicy::Allow
            },));
        }

        #[unsafe(method(webView:decidePolicyForNavigationResponse:decisionHandler:))]
        fn decide_response(
            &self,
            _webview: &WKWebView,
            response: &WKNavigationResponse,
            handler: &block2::DynBlock<dyn Fn(WKNavigationResponsePolicy)>,
        ) {
            let can_show = unsafe { response.canShowMIMEType() };
            if !can_show {
                emit(
                    &self.ivars().shared,
                    BrowserEvent::Blocked {
                        reason: "a download".to_string(),
                    },
                );
                handler.call((WKNavigationResponsePolicy::Cancel,));
                return;
            }
            handler.call((WKNavigationResponsePolicy::Allow,));
        }

        #[unsafe(method(webView:didCommitNavigation:))]
        fn did_commit(&self, webview: &WKWebView, _navigation: Option<&WKNavigation>) {
            emit_navigated(&self.ivars().shared, webview);
            emit(
                &self.ivars().shared,
                BrowserEvent::TitleChanged {
                    title: current_title(webview),
                },
            );
        }

        #[unsafe(method(webView:didFinishNavigation:))]
        fn did_finish(&self, webview: &WKWebView, _navigation: Option<&WKNavigation>) {
            emit_navigated(&self.ivars().shared, webview);
            emit(
                &self.ivars().shared,
                BrowserEvent::TitleChanged {
                    title: current_title(webview),
                },
            );
            emit(&self.ivars().shared, BrowserEvent::Ready);
        }

        #[unsafe(method(webView:didFailNavigation:withError:))]
        fn did_fail(
            &self,
            webview: &WKWebView,
            _navigation: Option<&WKNavigation>,
            error: &NSError,
        ) {
            emit_failed(&self.ivars().shared, webview, error);
        }

        #[unsafe(method(webView:didFailProvisionalNavigation:withError:))]
        fn did_fail_provisional(
            &self,
            webview: &WKWebView,
            _navigation: Option<&WKNavigation>,
            error: &NSError,
        ) {
            emit_failed(&self.ivars().shared, webview, error);
        }

        #[unsafe(method(webViewWebContentProcessDidTerminate:))]
        fn web_content_process_did_terminate(&self, _webview: &WKWebView) {
            fail(&self.ivars().shared, "the page's content process stopped");
        }
    }
);

impl BrowserNavigationDelegate {
    fn new(shared: Rc<RefCell<Shared>>, mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(NavigationIvars { shared });
        unsafe { msg_send![super(this), init] }
    }
}

fn emit_navigated(shared: &Rc<RefCell<Shared>>, webview: &WKWebView) {
    let url = current_url(webview);
    let can_back = unsafe { webview.canGoBack() };
    let can_forward = unsafe { webview.canGoForward() };
    emit(
        shared,
        BrowserEvent::Navigated {
            url,
            can_back,
            can_forward,
        },
    );
}

fn emit_failed(shared: &Rc<RefCell<Shared>>, webview: &WKWebView, error: &NSError) {
    let message = unsafe { error.localizedDescription() }.to_string();
    emit(
        shared,
        BrowserEvent::LoadFailed {
            url: current_url(webview),
            message,
        },
    );
}

struct UiIvars {
    shared: Rc<RefCell<Shared>>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = UiIvars]
    struct BrowserUiDelegate;

    unsafe impl NSObjectProtocol for BrowserUiDelegate {}

    unsafe impl WKUIDelegate for BrowserUiDelegate {
        #[unsafe(method_id(webView:createWebViewWithConfiguration:forNavigationAction:windowFeatures:))]
        unsafe fn create_web_view(
            &self,
            _webview: &WKWebView,
            _configuration: &WKWebViewConfiguration,
            action: &WKNavigationAction,
            _window_features: &WKWindowFeatures,
        ) -> Option<Retained<WKWebView>> {
            let request = unsafe { action.request() };
            let url = unsafe { request.URL() }
                .and_then(|url| unsafe { url.absoluteString() })
                .map(|url| url.to_string())
                .unwrap_or_default();
            if super::super::policy::classify(&url) != super::super::policy::Route::Load {
                emit(
                    &self.ivars().shared,
                    BrowserEvent::Blocked {
                        reason: format!("a new window to {url}"),
                    },
                );
                return None;
            }
            let allowed = self
                .ivars()
                .shared
                .try_borrow()
                .is_ok_and(|state| state.guard.allows(&url));
            if !allowed {
                emit(&self.ivars().shared, BrowserEvent::RouteBlocked { url });
                return None;
            }
            // Only a real link click becomes a sibling tab. A script-opened
            // window, a form target or an unclassified request is refused.
            if unsafe { action.navigationType() } == WKNavigationType::LinkActivated {
                emit(&self.ivars().shared, BrowserEvent::NewWindow { url });
            } else {
                emit(
                    &self.ivars().shared,
                    BrowserEvent::Blocked {
                        reason: "an unsolicited popup".to_string(),
                    },
                );
            }
            None
        }

        #[unsafe(method(webViewDidClose:))]
        fn did_close(&self, _webview: &WKWebView) {
            emit(
                &self.ivars().shared,
                BrowserEvent::Blocked {
                    reason: "the page asked to close its window".to_string(),
                },
            );
        }

        #[unsafe(method(webView:runJavaScriptAlertPanelWithMessage:initiatedByFrame:completionHandler:))]
        fn alert(
            &self,
            _webview: &WKWebView,
            _message: &NSString,
            _frame: &WKFrameInfo,
            handler: &block2::DynBlock<dyn Fn()>,
        ) {
            emit(
                &self.ivars().shared,
                BrowserEvent::Blocked {
                    reason: "a script dialog".to_string(),
                },
            );
            handler.call(());
        }

        #[unsafe(method(webView:runJavaScriptConfirmPanelWithMessage:initiatedByFrame:completionHandler:))]
        fn confirm(
            &self,
            _webview: &WKWebView,
            _message: &NSString,
            _frame: &WKFrameInfo,
            handler: &block2::DynBlock<dyn Fn(bool)>,
        ) {
            emit(
                &self.ivars().shared,
                BrowserEvent::Blocked {
                    reason: "a script dialog".to_string(),
                },
            );
            handler.call((false,));
        }

        #[unsafe(method(webView:runJavaScriptTextInputPanelWithPrompt:defaultText:initiatedByFrame:completionHandler:))]
        fn prompt(
            &self,
            _webview: &WKWebView,
            _prompt: &NSString,
            _default_text: Option<&NSString>,
            _frame: &WKFrameInfo,
            handler: &block2::DynBlock<dyn Fn(*mut NSString)>,
        ) {
            emit(
                &self.ivars().shared,
                BrowserEvent::Blocked {
                    reason: "a script dialog".to_string(),
                },
            );
            handler.call((null_mut(),));
        }

        #[unsafe(method(webView:runOpenPanelWithParameters:initiatedByFrame:completionHandler:))]
        fn open_panel(
            &self,
            _webview: &WKWebView,
            _parameters: &WKOpenPanelParameters,
            _frame: &WKFrameInfo,
            handler: &block2::DynBlock<dyn Fn(*mut NSArray<NSURL>)>,
        ) {
            emit(
                &self.ivars().shared,
                BrowserEvent::Blocked {
                    reason: "a file upload panel".to_string(),
                },
            );
            handler.call((null_mut(),));
        }

        #[unsafe(method(webView:requestMediaCapturePermissionForOrigin:initiatedByFrame:type:decisionHandler:))]
        fn media_capture(
            &self,
            _webview: &WKWebView,
            _origin: &WKSecurityOrigin,
            _frame: &WKFrameInfo,
            _capture_type: WKMediaCaptureType,
            handler: &block2::DynBlock<dyn Fn(WKPermissionDecision)>,
        ) {
            emit(
                &self.ivars().shared,
                BrowserEvent::Blocked {
                    reason: "a browser permission".to_string(),
                },
            );
            handler.call((WKPermissionDecision::Deny,));
        }
    }
);

impl BrowserUiDelegate {
    fn new(shared: Rc<RefCell<Shared>>, mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(UiIvars { shared });
        unsafe { msg_send![super(this), init] }
    }
}

struct TitleIvars {
    webview: Retained<WKWebView>,
    shared: Rc<RefCell<Shared>>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = TitleIvars]
    struct BrowserTitleObserver;

    impl BrowserTitleObserver {
        #[unsafe(method(observeValueForKeyPath:ofObject:change:context:))]
        fn observe(
            &self,
            key_path: Option<&NSString>,
            _object: Option<&AnyObject>,
            _change: Option<&NSDictionary<NSKeyValueChangeKey, AnyObject>>,
            _context: *mut c_void,
        ) {
            if key_path.is_some_and(|path| unsafe { path.isEqualToString(ns_string!("title")) }) {
                let title = current_title(&self.ivars().webview);
                emit(
                    &self.ivars().shared,
                    BrowserEvent::TitleChanged { title },
                );
            }
        }
    }

    unsafe impl NSObjectProtocol for BrowserTitleObserver {}
);

impl BrowserTitleObserver {
    fn new(
        webview: Retained<WKWebView>,
        shared: Rc<RefCell<Shared>>,
        mtm: MainThreadMarker,
    ) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(TitleIvars { webview, shared });
        let observer: Retained<Self> = unsafe { msg_send![super(this), init] };
        unsafe {
            observer
                .ivars()
                .webview
                .addObserver_forKeyPath_options_context(
                    &observer,
                    ns_string!("title"),
                    NSKeyValueObservingOptions::New,
                    null_mut(),
                );
        }
        observer
    }
}

impl Drop for BrowserTitleObserver {
    fn drop(&mut self) {
        unsafe {
            self.ivars()
                .webview
                .removeObserver_forKeyPath(self, ns_string!("title"));
        }
    }
}
