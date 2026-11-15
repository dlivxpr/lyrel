//! Read-only draw.io document: Host-owned bytes, engine-owned rendering.
//!
//! The workbench owns identity, acquisition and lifecycle. The fixed engine
//! receives the exact bytes through an isolated WebView and never learns a
//! path. Page selection, zoom and focus belong to a presentation; the parsed
//! page list and the bytes belong to the document, shared by every split.
//!
//! Editing is deliberately absent in this ticket: there is no save path from
//! the engine, no dirty state and no visual edit control. The bridge contract
//! already accepts validated state messages so the next ticket can add edits
//! without changing the Host owner.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, h_flex, v_flex,
};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, App, Bounds, Context, Entity, FocusHandle, Focusable, Pixels, Render, SharedString,
    Subscription, Task, TestSupportExt as _, Window, div, rems,
};
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::drawio_engine::bridge::Command;
use super::drawio_engine::document::{DrawioError, DrawioSource};
use super::drawio_engine::session::PreviewTheme;
use super::drawio_engine::surface::{ParentWindow, SurfaceError, SurfaceEvent, SurfaceHost};
use crate::ui::i18n::{L10nKey, t, t_fmt};

gpui_kit::actions!(
    drawio,
    [
        DrawioNextPage,
        DrawioPreviousPage,
        DrawioZoomIn,
        DrawioZoomOut,
        DrawioFit,
        DrawioActualSize
    ]
);

/// Only true while this document owns the keyboard, so the terminal keeps its
/// own page keys when focus is elsewhere.
pub(crate) const KEY_CONTEXT: &str = "DrawioDocument";

const ZOOM_STEP: f32 = 1.25;
const VISIBILITY_POLL: Duration = Duration::from_millis(120);
/// A native child window does not inherit GPUI's clip. If the presentation
/// stops being painted (tab switch, close, overlay), hide the WebView after
/// this long instead of leaving it over unrelated content.
const VISIBLE_GRACE: Duration = Duration::from_millis(220);

#[derive(Clone)]
enum DrawioContent {
    Ready {
        bytes: Arc<[u8]>,
        source: DrawioSource,
    },
    Failed(SharedString),
}

/// One draw.io file's authoritative bytes and page list.
pub(crate) struct DrawioDocument {
    content: DrawioContent,
    reloading: bool,
    revision: u64,
}

impl DrawioDocument {
    pub(super) fn new(
        bytes: Arc<[u8]>,
        parsed: Result<DrawioSource, DrawioError>,
        _cx: &mut Context<Self>,
    ) -> Self {
        Self {
            content: match parsed {
                Ok(source) => DrawioContent::Ready {
                    bytes,
                    source,
                },
                Err(error) => DrawioContent::Failed(SharedString::from(error.to_string())),
            },
            reloading: false,
            revision: 0,
        }
    }

    pub(super) fn replace(
        &mut self,
        bytes: Arc<[u8]>,
        parsed: Result<DrawioSource, DrawioError>,
        cx: &mut Context<Self>,
    ) {
        self.revision = self.revision.wrapping_add(1);
        self.content = match parsed {
            Ok(source) => DrawioContent::Ready { bytes, source },
            Err(error) => DrawioContent::Failed(SharedString::from(error.to_string())),
        };
        self.reloading = false;
        cx.notify();
    }

    pub(super) fn begin_reload(&mut self, cx: &mut Context<Self>) {
        if !self.reloading {
            self.reloading = true;
            cx.notify();
        }
    }

    pub(crate) fn revision(&self) -> u64 {
        self.revision
    }

    pub(crate) fn reloading(&self) -> bool {
        self.reloading
    }

    fn bytes(&self) -> Option<Arc<[u8]>> {
        match &self.content {
            DrawioContent::Ready { bytes, .. } => Some(bytes.clone()),
            DrawioContent::Failed(_) => None,
        }
    }

    fn source(&self) -> Option<&DrawioSource> {
        match &self.content {
            DrawioContent::Ready { source, .. } => Some(source),
            DrawioContent::Failed(_) => None,
        }
    }

    pub(crate) fn page_count(&self) -> usize {
        self.source().map_or(0, |source| source.pages.len())
    }

    pub(crate) fn page_name(&self, page: usize) -> Option<&str> {
        self.source()?
            .pages
            .get(page)?
            .name
            .as_deref()
    }

    pub(crate) fn failure(&self) -> Option<&SharedString> {
        match &self.content {
            DrawioContent::Failed(message) => Some(message),
            DrawioContent::Ready { .. } => None,
        }
    }

    pub(crate) fn is_ready(&self) -> bool {
        matches!(self.content, DrawioContent::Ready { .. })
    }
}

#[derive(Clone)]
enum EngineStatus {
    Idle,
    Starting,
    Ready,
    Failed(SharedString),
    Unsupported(SharedString),
}

pub(crate) struct DrawioView {
    document: Entity<DrawioDocument>,
    focus: FocusHandle,
    page: usize,
    zoom: f32,
    engine: EngineStatus,
    blocked: Option<SharedString>,
    surface: Option<SurfaceHost>,
    surface_events: Option<Task<()>>,
    watchdog: Option<Task<()>>,
    surface_pending: bool,
    surface_attempted: bool,
    bounds: Option<Bounds<Pixels>>,
    last_paint: Instant,
    surface_visible: bool,
    revision: u64,
    _document_observed: Subscription,
}

impl DrawioView {
    pub(super) fn new(document: Entity<DrawioDocument>, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        let revision = document.read(cx).revision();
        let observed = cx.observe(&document, |this, _, cx| this.sync(cx));
        let watchdog = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(VISIBILITY_POLL).await;
                let Ok(alive) = this.update(cx, |view, cx| view.poll_visibility(cx)) else {
                    break;
                };
                if !alive {
                    break;
                }
            }
        });
        Self {
            document,
            focus,
            page: 0,
            zoom: 1.,
            engine: EngineStatus::Idle,
            blocked: None,
            surface: None,
            surface_events: None,
            watchdog: Some(watchdog),
            surface_pending: false,
            surface_attempted: false,
            bounds: None,
            last_paint: Instant::now(),
            surface_visible: false,
            revision,
            _document_observed: observed,
        }
    }

    pub(super) fn new_at(
        document: Entity<DrawioDocument>,
        page: usize,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut view = Self::new(document, cx);
        view.page = page;
        view
    }

    pub(crate) fn document(&self) -> &Entity<DrawioDocument> {
        &self.document
    }

    pub(crate) fn handle(&self) -> FocusHandle {
        self.focus.clone()
    }

    pub(crate) fn page(&self) -> usize {
        self.page
    }

    pub(crate) fn page_count(&self, cx: &App) -> usize {
        self.document.read(cx).page_count()
    }

    pub(crate) fn page_label(&self, cx: &App) -> Option<SharedString> {
        let count = self.page_count(cx);
        (count > 1).then(|| {
            t_fmt(
                L10nKey::EditorDrawioPageOf,
                &[
                    ("page", &(self.page + 1).to_string()),
                    ("pages", &count.to_string()),
                ],
            )
            .into()
        })
    }

    fn multipage(&self, cx: &App) -> bool {
        self.page_count(cx) > 1
    }

    pub(crate) fn next_page(&mut self, cx: &mut Context<Self>) {
        self.set_page(self.page.saturating_add(1), cx);
    }

    pub(crate) fn previous_page(&mut self, cx: &mut Context<Self>) {
        self.set_page(self.page.saturating_sub(1), cx);
    }

    pub(crate) fn set_page(&mut self, page: usize, cx: &mut Context<Self>) {
        let count = self.page_count(cx);
        if count == 0 {
            return;
        }
        let page = page.min(count - 1);
        if page == self.page {
            return;
        }
        self.page = page;
        if let Some(surface) = &mut self.surface {
            let _ = surface.command(Command::Page(page));
        }
        cx.notify();
    }

    pub(crate) fn zoom_in(&mut self, cx: &mut Context<Self>) {
        self.send_zoom(Command::ZoomIn, cx);
    }

    pub(crate) fn zoom_out(&mut self, cx: &mut Context<Self>) {
        self.send_zoom(Command::ZoomOut, cx);
    }

    pub(crate) fn fit(&mut self, cx: &mut Context<Self>) {
        self.send_zoom(Command::Fit, cx);
    }

    pub(crate) fn actual_size(&mut self, cx: &mut Context<Self>) {
        self.send_zoom(Command::ActualSize, cx);
    }

    fn send_zoom(&mut self, command: Command, cx: &mut Context<Self>) {
        if let Some(surface) = &mut self.surface {
            let _ = surface.command(command);
        } else if matches!(command, Command::ActualSize) {
            self.zoom = 1.;
            cx.notify();
        }
    }

    pub(crate) fn zoom_label(&self) -> SharedString {
        format!("{:.0}%", self.zoom * 100.).into()
    }

    pub(crate) fn status_text(&self, cx: &App) -> SharedString {
        let document = self.document.read(cx);
        if let Some(message) = document.failure() {
            return message.clone();
        }
        let mut parts = vec![String::from("draw.io")];
        if let Some(label) = self.page_label(cx) {
            parts.push(label.to_string());
        }
        match &self.engine {
            EngineStatus::Idle | EngineStatus::Starting => parts.push(
                t(L10nKey::EditorDrawioLoading).to_string(),
            ),
            EngineStatus::Ready => parts.push(self.zoom_label().to_string()),
            EngineStatus::Failed(message) | EngineStatus::Unsupported(message) => {
                parts.push(message.to_string());
            }
        }
        if document.reloading() {
            parts.push(t(L10nKey::EditorDrawioLoading).to_string());
        }
        parts.join(" · ").into()
    }

    pub(crate) fn blocked_text(&self) -> Option<SharedString> {
        self.blocked.clone()
    }

    fn sync(&mut self, cx: &mut Context<Self>) {
        let revision = self.document.read(cx).revision();
        if revision == self.revision {
            cx.notify();
            return;
        }
        self.revision = revision;
        let count = self.page_count(cx);
        self.page = if count == 0 { 0 } else { self.page.min(count - 1) };
        // A new document is a new session. The old WebView token and URL must
        // not survive a reload.
        self.surface = None;
        self.surface_events = None;
        self.surface_pending = false;
        self.surface_attempted = false;
        self.surface_visible = false;
        self.blocked = None;
        self.engine = EngineStatus::Idle;
        self.zoom = 1.;
        self.bounds = None;
        cx.notify();
    }

    fn poll_visibility(&mut self, cx: &mut Context<Self>) -> bool {
        if let Some(surface) = &mut self.surface {
            let visible = self.last_paint.elapsed() < VISIBLE_GRACE
                && self.bounds.is_some_and(|bounds| {
                    f32::from(bounds.size.width) > 1. && f32::from(bounds.size.height) > 1.
                });
            if visible != self.surface_visible {
                self.surface_visible = visible;
                surface.set_visible(visible);
            }
        }
        let _ = cx;
        true
    }

    fn prepared(&mut self, bounds: Bounds<Pixels>, window: &Window, cx: &mut Context<Self>) {
        self.last_paint = Instant::now();
        let changed = self.bounds.is_none_or(|old| {
            f32::from(old.origin.x - bounds.origin.x).abs() > 0.5
                || f32::from(old.origin.y - bounds.origin.y).abs() > 0.5
                || f32::from(old.size.width - bounds.size.width).abs() > 0.5
                || f32::from(old.size.height - bounds.size.height).abs() > 0.5
        });
        self.bounds = Some(bounds);
        let scale = window.scale_factor();
        if let Some(surface) = &mut self.surface {
            if changed {
                surface.set_bounds(bounds, scale);
            }
            if !self.surface_visible {
                self.surface_visible = true;
                surface.set_visible(true);
            }
            return;
        }
        if self.surface_pending || self.surface_attempted {
            return;
        }
        let parent = match ParentWindow::from_window(window) {
            Ok(parent) => parent,
            Err(error) => {
                self.engine = EngineStatus::Unsupported(SharedString::from(error.to_string()));
                return;
            }
        };
        #[cfg(test)]
        {
            let _ = (parent, cx);
            self.surface_attempted = true;
            self.engine = EngineStatus::Unsupported(SharedString::from(
                "the native diagram surface is not started by automated tests",
            ));
            return;
        }
        #[cfg(not(test))]
        {
            let Some(bytes) = self.document.read(cx).bytes() else {
                return;
            };
            let theme = if cx.theme().is_dark() {
                PreviewTheme::Dark
            } else {
                PreviewTheme::Light
            };
            self.surface_pending = true;
            self.engine = EngineStatus::Starting;
            cx.spawn(async move |this, cx| {
                let _ = this.update(cx, |view, cx| {
                    view.start_surface(parent, bounds, scale, bytes, theme, cx);
                });
            })
            .detach();
        }
    }

    #[cfg(not(test))]
    fn start_surface(
        &mut self,
        parent: ParentWindow,
        bounds: Bounds<Pixels>,
        scale: f32,
        bytes: Arc<[u8]>,
        theme: PreviewTheme,
        cx: &mut Context<Self>,
    ) {
        self.surface_pending = false;
        self.surface_attempted = true;
        let (sender, receiver) = smol::channel::unbounded();
        match SurfaceHost::start(parent, bounds, scale, bytes, theme, sender) {
            Ok(surface) => {
                self.surface = Some(surface);
                self.surface_visible = true;
                self.engine = EngineStatus::Starting;
                self.surface_events = Some(cx.spawn(async move |this, cx| {
                    while let Ok(event) = receiver.recv().await {
                        let Ok(alive) = this.update(cx, |view, cx| {
                            view.on_surface_event(event, cx);
                            true
                        }) else {
                            break;
                        };
                        if !alive {
                            break;
                        }
                    }
                }));
            }
            Err(error) => {
                self.engine = engine_error(error);
            }
        }
        cx.notify();
    }

    #[cfg(test)]
    fn start_surface(
        &mut self,
        _parent: ParentWindow,
        _bounds: Bounds<Pixels>,
        _scale: f32,
        _bytes: Arc<[u8]>,
        _theme: PreviewTheme,
        cx: &mut Context<Self>,
    ) {
        self.surface_pending = false;
        self.surface_attempted = true;
        self.engine = EngineStatus::Unsupported(SharedString::from(
            "the native diagram surface is not started by automated tests",
        ));
        cx.notify();
    }

    fn on_surface_event(&mut self, event: SurfaceEvent, cx: &mut Context<Self>) {
        match event {
            SurfaceEvent::Ready => {
                self.engine = EngineStatus::Ready;
                if self.page > 0
                    && let Some(surface) = &mut self.surface
                {
                    let _ = surface.command(Command::Page(self.page));
                }
            }
            SurfaceEvent::State { page, pages, zoom } => {
                self.engine = EngineStatus::Ready;
                if pages > 0 {
                    self.page = page.min(pages.saturating_sub(1));
                }
                if zoom.is_finite() && zoom > 0. {
                    self.zoom = zoom;
                }
            }
            SurfaceEvent::Blocked { reason } => {
                self.blocked = Some(SharedString::from(reason));
            }
            SurfaceEvent::Failed { message } => {
                self.engine = EngineStatus::Failed(SharedString::from(message));
                self.surface = None;
                self.surface_events = None;
                self.surface_visible = false;
            }
            SurfaceEvent::Closed => {
                self.surface = None;
                self.surface_events = None;
                self.surface_visible = false;
                if matches!(self.engine, EngineStatus::Starting) {
                    self.engine = EngineStatus::Idle;
                }
            }
        }
        cx.notify();
    }

    fn retry(&mut self, cx: &mut Context<Self>) {
        self.surface = None;
        self.surface_events = None;
        self.surface_pending = false;
        self.surface_attempted = false;
        self.surface_visible = false;
        self.blocked = None;
        self.engine = EngineStatus::Idle;
        self.last_paint = Instant::now();
        cx.notify();
    }

    fn controls(&self, cx: &mut Context<Self>, multipage: bool, ready: bool) -> impl IntoElement {
        let count = self.page_count(cx);
        h_flex()
            .flex_none()
            .items_center()
            .gap_1()
            .when(multipage, |row| {
                row.child(
                    Button::new("drawio-page-previous")
                        .ghost()
                        .xsmall()
                        .flex_none()
                        .icon(IconName::ChevronLeft)
                        .accessibility_label(t(L10nKey::EditorDrawioPagePrevious))
                        .tooltip(t(L10nKey::EditorDrawioPagePrevious))
                        .disabled(self.page == 0)
                        .on_click(cx.listener(|this, _, _, cx| this.previous_page(cx))),
                )
                .child(
                    div()
                        .id("drawio-page-facts")
                        .test_support()
                        .flex_none()
                        .text_xs()
                        .child(self.page_label(cx).unwrap_or_default()),
                )
                .child(
                    Button::new("drawio-page-next")
                        .ghost()
                        .xsmall()
                        .flex_none()
                        .icon(IconName::ChevronRight)
                        .accessibility_label(t(L10nKey::EditorDrawioPageNext))
                        .tooltip(t(L10nKey::EditorDrawioPageNext))
                        .disabled(self.page + 1 >= count)
                        .on_click(cx.listener(|this, _, _, cx| this.next_page(cx))),
                )
            })
            .when(ready, |row| {
                row.child(
                    Button::new("drawio-fit")
                        .ghost()
                        .xsmall()
                        .flex_none()
                        .label(t(L10nKey::EditorImageFit))
                        .accessibility_label(t(L10nKey::EditorImageFitAction))
                        .tooltip(t(L10nKey::EditorImageFitAction))
                        .on_click(cx.listener(|this, _, _, cx| this.fit(cx))),
                )
                .child(
                    Button::new("drawio-actual")
                        .ghost()
                        .xsmall()
                        .flex_none()
                        .label(t(L10nKey::EditorImageActualSize))
                        .accessibility_label(t(L10nKey::EditorImageActualSizeAction))
                        .tooltip(t(L10nKey::EditorImageActualSizeAction))
                        .on_click(cx.listener(|this, _, _, cx| this.actual_size(cx))),
                )
            })
    }

    fn failure_panel(
        &self,
        title: SharedString,
        detail: Option<SharedString>,
        retry: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (muted, foreground, warning) = {
            let theme = cx.theme();
            (
                theme.muted_foreground,
                theme.foreground,
                theme.warning,
            )
        };
        v_flex()
            .id("drawio-failed")
            .test_support()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .items_center()
            .justify_center()
            .gap_2()
            .px_4()
            .child(Icon::new(IconName::TriangleAlert).large().text_color(warning))
            .child(div().text_sm().text_color(foreground).child(title))
            .when_some(detail, |panel, detail| {
                panel.child(
                    div()
                        .max_w(rems(34.))
                        .text_xs()
                        .text_color(muted)
                        .child(detail),
                )
            })
            .when(retry, |panel| {
                panel.child(
                    Button::new("drawio-retry")
                        .small()
                        .label(t(L10nKey::EditorDrawioRetry))
                        .on_click(cx.listener(|this, _, _, cx| this.retry(cx))),
                )
            })
            .into_any_element()
    }
}

fn engine_error(error: SurfaceError) -> EngineStatus {
    match error {
        SurfaceError::Unsupported(message) => {
            EngineStatus::Unsupported(SharedString::from(message))
        }
        other => EngineStatus::Failed(SharedString::from(other.to_string())),
    }
}

impl Focusable for DrawioView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for DrawioView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.last_paint = Instant::now();
        let (border, muted, foreground, warning, primary) = {
            let theme = cx.theme();
            (
                theme.border,
                theme.muted_foreground,
                theme.foreground,
                theme.warning,
                theme.primary,
            )
        };
        let focused = self.focus.contains_focused(window, cx);
        let ring = if focused { primary } else { gpui_kit::transparent_black() };
        let multipage = self.multipage(cx);
        let page_label = self.page_label(cx);
        let ready = matches!(self.engine, EngineStatus::Ready);
        let document_failure = self.document.read(cx).failure().cloned();
        let reloading = self.document.read(cx).reloading();
        let engine = self.engine.clone();
        let blocked = self.blocked.clone();
        let zoom = self.zoom_label();
        let surface_started = self.surface.is_some();

        let toolbar = h_flex()
            .flex_none()
            .w_full()
            .items_center()
            .gap_2()
            .px_2()
            .py_1()
            .border_b_1()
            .border_color(border)
            .child(
                div()
                    .id("drawio-format")
                    .test_support()
                    .flex_none()
                    .text_xs()
                    .text_color(muted)
                    .child("draw.io"),
            )
            .when_some(page_label, |row, label| {
                row.child(
                    div()
                        .id("drawio-page-label")
                        .test_support()
                        .flex_none()
                        .text_xs()
                        .text_color(muted)
                        .child(label),
                )
            })
            .child(div().flex_1())
            .when(reloading, |row| {
                row.child(
                    div()
                        .flex_none()
                        .text_xs()
                        .text_color(muted)
                        .child(t(L10nKey::EditorDrawioLoading)),
                )
            })
            .when(ready, |row| {
                row.child(
                    div()
                        .id("drawio-zoom")
                        .test_support()
                        .flex_none()
                        .text_xs()
                        .text_color(muted)
                        .child(zoom),
                )
            })
            .child(self.controls(cx, multipage, ready));

        let body = if let Some(message) = document_failure {
            self.failure_panel(
                t(L10nKey::EditorDrawioFailed).into(),
                Some(message),
                false,
                cx,
            )
        } else {
            match &engine {
                EngineStatus::Unsupported(message) => self.failure_panel(
                    t(L10nKey::EditorDrawioUnavailable).into(),
                    Some(message.clone()),
                    false,
                    cx,
                ),
                EngineStatus::Failed(message) => self.failure_panel(
                    t(L10nKey::EditorDrawioFailed).into(),
                    Some(message.clone()),
                    true,
                    cx,
                ),
                EngineStatus::Idle | EngineStatus::Starting | EngineStatus::Ready => {
                    let canvas = div()
                        .flex_1()
                        .min_w_0()
                        .min_h_0()
                        .relative()
                        .border_1()
                        .border_color(ring)
                        .on_children_prepainted({
                            let view = cx.entity();
                            move |bounds, window, cx| {
                                if let Some(bounds) = bounds.first() {
                                    let bounds = *bounds;
                                    view.update(cx, |view, cx| {
                                        view.prepared(bounds, window, cx)
                                    });
                                }
                            }
                        })
                        .test_support();
                    let overlay = if !matches!(engine, EngineStatus::Ready) && !surface_started {
                        Some(
                            v_flex()
                                .absolute()
                                .inset_0()
                                .items_center()
                                .justify_center()
                                .child(
                                    div()
                                        .id("drawio-loading")
                                        .test_support()
                                        .text_sm()
                                        .text_color(muted)
                                        .child(t(L10nKey::EditorDrawioLoading)),
                                )
                                .into_any_element(),
                        )
                    } else {
                        None
                    };
                    let blocked_note = blocked.map(|reason| {
                        v_flex()
                            .absolute()
                            .left_2()
                            .right_2()
                            .bottom_2()
                            .px_2()
                            .py_1()
                            .border_1()
                            .border_color(warning)
                            .child(
                                div()
                                    .id("drawio-blocked")
                                    .test_support()
                                    .text_xs()
                                    .text_color(foreground)
                                    .child(t_fmt(
                                        L10nKey::EditorDrawioBlocked,
                                        &[("what", reason.as_ref())],
                                    )),
                            )
                            .into_any_element()
                    });
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .min_h_0()
                        .child(canvas)
                        .when_some(overlay, |col, overlay| col.child(overlay))
                        .when_some(blocked_note, |col, note| col.child(note))
                        .into_any_element()
                }
            }
        };

        v_flex()
            .id(("drawio-document", cx.entity_id()))
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &DrawioNextPage, _, cx| this.next_page(cx)))
            .on_action(cx.listener(|this, _: &DrawioPreviousPage, _, cx| this.previous_page(cx)))
            .on_action(cx.listener(|this, _: &DrawioZoomIn, _, cx| this.zoom_in(cx)))
            .on_action(cx.listener(|this, _: &DrawioZoomOut, _, cx| this.zoom_out(cx)))
            .on_action(cx.listener(|this, _: &DrawioFit, _, cx| this.fit(cx)))
            .on_action(cx.listener(|this, _: &DrawioActualSize, _, cx| this.actual_size(cx)))
            .on_key_down(
                cx.listener(|this, event: &gpui_kit::KeyDownEvent, _, cx| {
                    let key = event.keystroke.key.as_str();
                    if event.keystroke.modifiers.secondary(cx)
                        && matches!(key, "pageup" | "pagedown")
                    {
                        return;
                    }
                    if this.multipage(cx) {
                        match key {
                            "pageup" => {
                                this.previous_page(cx);
                                cx.stop_propagation();
                            }
                            "pagedown" => {
                                this.next_page(cx);
                                cx.stop_propagation();
                            }
                            "home" => {
                                this.set_page(0, cx);
                                cx.stop_propagation();
                            }
                            "end" => {
                                let last = this.page_count(cx).saturating_sub(1);
                                this.set_page(last, cx);
                                cx.stop_propagation();
                            }
                            _ => {}
                        }
                    }
                }),
            )
            .on_mouse_down(
                gpui_kit::MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    this.focus.focus(window, cx);
                    if let Some(surface) = &mut this.surface {
                        surface.focus();
                    }
                }),
            )
            .size_full()
            .min_w_0()
            .min_h_0()
            .child(toolbar)
            .child(body)
    }
}

/// Right-click menu: the same page and zoom actions as the toolbar and keys.
pub(super) fn menu_entries(
    menu: gpui_kit::component::menu::PopupMenu,
    multipage: bool,
) -> gpui_kit::component::menu::PopupMenu {
    let menu = menu
        .separator()
        .menu(t(L10nKey::EditorImageZoomIn), Box::new(DrawioZoomIn))
        .menu(t(L10nKey::EditorImageZoomOut), Box::new(DrawioZoomOut))
        .menu(t(L10nKey::EditorImageFit), Box::new(DrawioFit))
        .menu(t(L10nKey::EditorImageActualSize), Box::new(DrawioActualSize));
    if multipage {
        menu.separator()
            .menu(t(L10nKey::EditorDrawioPagePrevious), Box::new(DrawioPreviousPage))
            .menu(t(L10nKey::EditorDrawioPageNext), Box::new(DrawioNextPage))
    } else {
        menu
    }
}

#[cfg(test)]
#[path = "drawio_document/tests.rs"]
mod tests;
