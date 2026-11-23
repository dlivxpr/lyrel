//! The macOS browsing surface: one WKWebView inside the GPUI window through
//! wry's child host — the same engine dependency the diagram surface uses,
//! without its custom protocol, IPC handler or any page → workbench channel.
//! The durable profile lives in the panel-supplied per-Host partition
//! directory through one `WebContext` per presentation.
//!
//! One platform gap, stated plainly: WebView2 answers `IsUserInitiated` on a
//! new-window request, and the Windows host uses it to open a sibling tab;
//! wry's macOS new-window callback does not surface the gesture, so this
//! host denies every popup request there. A `target=_blank` click on macOS
//! is refused rather than guessed at — unsolicited popups cannot escape, and
//! the user-initiated sibling-tab route is the Windows behavior until the
//! platform hands us the same signal.

use super::{BrowserCommand, BrowserEvent, BrowserParentWindow, BrowserSurfaceError};
use gpui_kit::{Bounds, Pixels};
use smol::channel::Sender;
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, Mutex, MutexGuard};
use wry::dpi::{LogicalPosition, LogicalSize, Position, Size};
use wry::{
    NewWindowResponse, PageLoadEvent, PermissionKind, PermissionResponse, Rect, WebContext,
    WebView, WebViewBuilder,
};

struct Shared {
    sender: Sender<BrowserEvent>,
    closed: bool,
}

pub struct Host {
    shared: Arc<Mutex<Shared>>,
    webview: Rc<RefCell<Option<WebView>>>,
    // The context owns the WKWebViewWebsiteDataStore rooted at the per-Host
    // partition directory; dropping the presentation drops the view, the
    // directory itself is the durable site data and stays.
    _context: WebContext,
}

impl Host {
    pub fn start(
        parent: BrowserParentWindow,
        bounds: Bounds<Pixels>,
        url: &str,
        data_dir: PathBuf,
        events: Sender<BrowserEvent>,
    ) -> Result<Self, BrowserSurfaceError> {
        let shared = Arc::new(Mutex::new(Shared {
            sender: events.clone(),
            closed: false,
        }));
        let slot: Rc<RefCell<Option<WebView>>> = Rc::new(RefCell::new(None));
        let mut context = WebContext::new(Some(data_dir));

        let nav_shared = shared.clone();
        let navigation = move |url: String| {
            let allowed = super::super::policy::classify(&url) != super::super::policy::Route::Deny;
            if !allowed {
                emit(
                    &nav_shared,
                    BrowserEvent::Blocked {
                        reason: format!("navigation to {url}"),
                    },
                );
            }
            allowed
        };
        // wry cannot attest a user gesture on this request; deny rather than
        // let an unsolicited popup through. See the module doc.
        let popup_shared = shared.clone();
        let new_window = move |url: String, _features: wry::NewWindowFeatures| {
            emit(
                &popup_shared,
                BrowserEvent::Blocked {
                    reason: format!("a new window to {url}"),
                },
            );
            NewWindowResponse::Deny
        };
        let download_sender = events.clone();
        let download = move |_url: String, _path: &mut PathBuf| {
            let _ = download_sender.try_send(BrowserEvent::Blocked {
                reason: "a download".to_string(),
            });
            false
        };
        // WKWebView denies media-capture requests without a delegate grant;
        // the permission handler closes every other ask the same way.
        let permission_sender = events.clone();
        let permission = move |_kind: PermissionKind| {
            let _ = permission_sender.try_send(BrowserEvent::Blocked {
                reason: "a browser permission".to_string(),
            });
            PermissionResponse::Deny
        };
        // Files dragged onto the page stay outside the app flow.
        let drag_sender = events.clone();
        let drag = move |_event: wry::DragDropEvent| {
            let _ = drag_sender.try_send(BrowserEvent::Blocked {
                reason: "a dragged file".to_string(),
            });
            true
        };
        let title_sender = events.clone();
        let title_changed = move |title: String| {
            let _ = title_sender.try_send(BrowserEvent::TitleChanged { title });
        };
        // Every finished page load reports the URL and the history state the
        // chrome renders from; this is also the Ready signal.
        let load_sender = events.clone();
        let load_slot = slot.clone();
        let load = move |event: PageLoadEvent, _url: String| {
            if !matches!(event, PageLoadEvent::Finished) {
                return;
            }
            let Some(webview) = load_slot.borrow().as_ref() else {
                return;
            };
            let url = webview.url().unwrap_or_default();
            let can_back = webview.can_go_back().unwrap_or(false);
            let can_forward = webview.can_go_forward().unwrap_or(false);
            let _ = load_sender.try_send(BrowserEvent::Navigated {
                url,
                can_back,
                can_forward,
            });
            let _ = load_sender.try_send(BrowserEvent::Ready);
        };

        let webview = WebViewBuilder::new_with_web_context(&mut context)
            .with_url(url)
            .with_bounds(rect(bounds))
            .with_visible(true)
            .with_accept_first_mouse(true)
            // Not incognito: the partition directory is this Host's durable
            // site data, restored with the tab on the next launch.
            .with_navigation_handler(navigation)
            .with_new_window_req_handler(new_window)
            .with_download_started_handler(download)
            .with_permission_handler(permission)
            .with_drag_drop_handler(drag)
            .with_document_title_changed_handler(title_changed)
            .with_on_page_load_handler(load)
            .build_as_child(&parent)
            .map_err(|error| BrowserSurfaceError::Platform(error.to_string()))?;
        *slot.borrow_mut() = Some(webview);
        Ok(Self {
            shared,
            webview: slot,
            _context: context,
        })
    }

    pub fn set_bounds(&mut self, bounds: Bounds<Pixels>) {
        if let Some(webview) = self.webview.borrow().as_ref() {
            let _ = webview.set_bounds(rect(bounds));
        }
    }

    pub fn set_visible(&mut self, visible: bool) {
        if let Some(webview) = self.webview.borrow().as_ref() {
            let _ = webview.set_visible(visible);
        }
    }

    pub fn focus(&mut self) {
        if let Some(webview) = self.webview.borrow().as_ref() {
            let _ = webview.focus();
        }
    }

    /// WKWebView keeps its first responder across `setHidden:`/`setVisible:`
    /// on macOS, so the panel has nothing to hand back; the platform focuses
    /// the page itself when it is shown again.
    pub fn has_focus(&self) -> bool {
        false
    }

    pub fn command(&mut self, command: BrowserCommand) -> Result<(), BrowserSurfaceError> {
        if lock(&self.shared).closed {
            return Err(BrowserSurfaceError::Platform(
                "the browser surface is closed".into(),
            ));
        }
        let webview = self.webview.borrow();
        let Some(webview) = webview.as_ref() else {
            return Err(BrowserSurfaceError::Platform(
                "the browser surface is not ready".into(),
            ));
        };
        match command {
            BrowserCommand::Back => webview
                .go_back()
                .map_err(|error| BrowserSurfaceError::Platform(error.to_string())),
            BrowserCommand::Forward => webview
                .go_forward()
                .map_err(|error| BrowserSurfaceError::Platform(error.to_string())),
            BrowserCommand::Reload => webview
                .reload()
                .map_err(|error| BrowserSurfaceError::Platform(error.to_string())),
            BrowserCommand::Navigate { url } => webview
                .load_url(&url)
                .map_err(|error| BrowserSurfaceError::Platform(error.to_string())),
        }
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        let sender = {
            let mut shared = lock(&self.shared);
            shared.closed = true;
            shared.sender.clone()
        };
        let _ = self.webview.borrow_mut().take();
        let _ = sender.try_send(BrowserEvent::Closed);
    }
}

fn lock(shared: &Mutex<Shared>) -> MutexGuard<'_, Shared> {
    // The lock is never held across an await and never re-entered, so a
    // poisoned lock means a panicked peer — recover the data rather than
    // cascade the panic into the UI thread.
    shared
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
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

fn emit(shared: &Arc<Mutex<Shared>>, event: BrowserEvent) {
    let sender = lock(shared).sender.clone();
    let _ = sender.try_send(event);
}
