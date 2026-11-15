//! 只读 PDF 文档：页、缩放、密码与既有批注／表单外观。
//!
//! 工作台拥有文档身份、所属 Host、标签页、保存／关闭与焦点；这里只拥有
//! “这份文件解析后能显示什么”和每个编辑组自己的页／缩放取景。字节由
//! owning Host 读取，解析与单页光栅化在后台完成。PDF 原文始终是权威：
//! 无 `/AP` 的表单值只以真实字段几何叠加为只读文本，绝不写回或重新序列化。
//!
//! 本切片不实现大纲导航、选字复制与搜索（#285），也不创建批注或填表。

use std::sync::Arc;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, h_flex, v_flex,
};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, App, Context, Entity, FocusHandle, Focusable, Pixels, Point, Render, ScrollHandle,
    SharedString, Size, Subscription, TestSupportExt as _, Window, div, px, rems,
};
use lyrel_core::host::transfer::CancelToken;

use super::markdown;
use crate::ui::i18n::{L10nKey, t, t_fmt};

gpui_kit::actions!(
    pdf,
    [
        PdfZoomIn,
        PdfZoomOut,
        PdfFitWidth,
        PdfFitPage,
        PdfActualSize,
        PdfNextPage,
        PdfPreviousPage
    ]
);

/// 只有 PDF 文档持有键盘时成立。
pub(crate) const KEY_CONTEXT: &str = "PdfDocument";

const ZOOM_MIN: f32 = 0.5;
const ZOOM_MAX: f32 = 3.;
const ZOOM_STEP: f32 = 1.25;
const PAGE_PAD_X: f32 = 12.;
const MAX_PAGE_WIDTH: f32 = 4096.;

/// 一次 Host 读取的结果；解析在 Host 线程完成，锁定与格式错误是文档状态。
pub(super) enum PdfLoaded {
    Ready(Arc<lyrel_pdf::PdfDocument>),
    Locked(Arc<Vec<u8>>),
    Failed(SharedString),
}

/// 文档此刻的解析状态。密码只在提交时短暂经过，不作为文档状态保存。
#[derive(Clone)]
pub(crate) enum PdfContent {
    Ready(Arc<lyrel_pdf::PdfDocument>),
    Locked { wrong: bool },
    Unlocking,
    Failed(SharedString),
}

impl PdfContent {
    pub(crate) fn ready(&self) -> Option<&Arc<lyrel_pdf::PdfDocument>> {
        match self {
            Self::Ready(document) => Some(document),
            _ => None,
        }
    }

    pub(crate) fn is_locked(&self) -> bool {
        matches!(self, Self::Locked { .. })
    }
}

/// 一份 PDF 文件的内容：不可变源字节与解析结果。密码不留下副本。
pub(crate) struct PdfDocument {
    bytes: Option<Arc<Vec<u8>>>,
    content: PdfContent,
    revision: u64,
    unlock_seq: u64,
    reloading: bool,
}

impl PdfDocument {
    pub(super) fn new(loaded: PdfLoaded) -> Self {
        let mut document = Self {
            bytes: None,
            content: PdfContent::Failed(SharedString::default()),
            revision: 1,
            unlock_seq: 0,
            reloading: false,
        };
        document.set(loaded);
        document
    }

    fn set(&mut self, loaded: PdfLoaded) {
        match loaded {
            PdfLoaded::Ready(document) => {
                self.bytes = None;
                self.content = PdfContent::Ready(document);
            }
            PdfLoaded::Locked(bytes) => {
                self.bytes = Some(bytes);
                self.content = PdfContent::Locked { wrong: false };
            }
            PdfLoaded::Failed(message) => {
                self.bytes = None;
                self.content = PdfContent::Failed(message);
            }
        }
    }

    pub(super) fn replace(&mut self, loaded: PdfLoaded, cx: &mut Context<Self>) {
        self.set(loaded);
        self.reloading = false;
        self.revision = self.revision.wrapping_add(1);
        cx.notify();
    }

    pub(super) fn begin_reload(&mut self, cx: &mut Context<Self>) {
        if !self.reloading {
            self.reloading = true;
            cx.notify();
        }
    }

    pub(crate) fn content(&self) -> &PdfContent {
        &self.content
    }

    pub(crate) fn ready(&self) -> Option<Arc<lyrel_pdf::PdfDocument>> {
        self.content.ready().cloned()
    }

    pub(crate) fn revision(&self) -> u64 {
        self.revision
    }

    pub(crate) fn reloading(&self) -> bool {
        self.reloading
    }

    /// 尝试用用户输入的密码重新解析。密码只在这次调用的任务里存在，解析
    /// 结束即释放；失败回到锁定状态并给出可见提示，而不是无限等待。
    pub(super) fn unlock(&mut self, password: String, cx: &mut Context<Self>) {
        if !self.content.is_locked() {
            return;
        }
        let Some(bytes) = self.bytes.clone() else {
            return;
        };
        self.unlock_seq = self.unlock_seq.wrapping_add(1);
        let seq = self.unlock_seq;
        let revision = self.revision;
        self.content = PdfContent::Unlocking;
        cx.notify();
        #[cfg(test)]
        let gate = tests::take_work(tests::Work::Unlock);
        cx.spawn(async move |this, cx| {
            let parsed = cx
                .background_executor()
                .spawn(async move {
                    #[cfg(test)]
                    if let Some(gate) = gate.as_ref() {
                        gate.block().await;
                    }
                    let result = lyrel_pdf::PdfDocument::load(bytes, &password);
                    #[cfg(test)]
                    if let Some(gate) = gate {
                        gate.finish();
                    }
                    result
                })
                .await;
            let _ = this.update(cx, |document, cx| {
                if document.unlock_seq != seq || document.revision != revision {
                    return;
                }
                match parsed {
                    Ok(parsed) => {
                        document.bytes = None;
                        document.content = PdfContent::Ready(Arc::new(parsed));
                        document.revision = document.revision.wrapping_add(1);
                    }
                    Err(lyrel_pdf::LoadError::Locked) => {
                        document.content = PdfContent::Locked { wrong: true };
                    }
                    Err(lyrel_pdf::LoadError::Other(message)) => {
                        document.bytes = None;
                        document.content = PdfContent::Failed(SharedString::from(message));
                        document.revision = document.revision.wrapping_add(1);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Fit {
    Width,
    Page,
}

#[derive(Default)]
struct PageSlot {
    image: Option<markdown::images::Bitmap>,
    /// Render generation of `image`; differs from the view's generation when a
    /// re-render is in flight and the old bitmap is still shown scaled.
    rendered: u64,
    /// Generation whose attempt was started, so a failed page does not respawn
    /// every frame. Reset when the page leaves the window.
    attempted: u64,
    /// `(attempt serial, generation)` while a background render is in flight.
    loading: Option<(u64, u64)>,
    failed: bool,
}

/// 一份 PDF 在一个分组里的取景：页、缩放和位图窗口。
pub(crate) struct PdfView {
    document: Entity<PdfDocument>,
    focus: FocusHandle,
    scroll: ScrollHandle,
    zoom: f32,
    fit: Option<Fit>,
    dims: Vec<(f32, f32)>,
    slots: Vec<PageSlot>,
    revision: u64,
    current: usize,
    viewport: Option<Size<Pixels>>,
    generation: u64,
    next_attempt: u64,
    cancel: CancelToken,
    password: Entity<InputState>,
    _password_sub: Subscription,
    _observe: Subscription,
}

impl PdfView {
    pub(super) fn new(
        document: Entity<PdfDocument>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let observe = cx.observe(&document, |_, _, cx| cx.notify());
        let password = cx.new(|cx| {
            InputState::new(window, cx)
                .masked(true)
                .placeholder(t(L10nKey::EditorPdfPasswordPlaceholder))
        });
        let password_sub = cx.subscribe_in(
            &password,
            window,
            |view, _, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    view.submit_password(window, cx);
                }
            },
        );
        cx.on_release(|view, _| view.cancel.cancel()).detach();
        Self {
            document,
            focus: cx.focus_handle(),
            scroll: ScrollHandle::new(),
            zoom: 1.,
            fit: Some(Fit::Width),
            dims: Vec::new(),
            slots: Vec::new(),
            revision: 0,
            current: 0,
            viewport: None,
            generation: 1,
            next_attempt: 1,
            cancel: CancelToken::default(),
            password,
            _password_sub: password_sub,
            _observe: observe,
        }
    }

    pub(crate) fn document(&self) -> &Entity<PdfDocument> {
        &self.document
    }

    pub(crate) fn handle(&self) -> FocusHandle {
        self.focus.clone()
    }

    pub(crate) fn scroll(&self) -> &ScrollHandle {
        &self.scroll
    }

    #[cfg(test)]
    pub(crate) fn password_input(&self) -> Entity<InputState> {
        self.password.clone()
    }

    #[cfg(test)]
    pub(crate) fn page_number(&self) -> usize {
        self.current + 1
    }

    #[cfg(test)]
    pub(crate) fn page_count(&self) -> usize {
        self.dims.len()
    }

    #[cfg(test)]
    pub(crate) fn loaded_pages(&self) -> Vec<usize> {
        self.slots
            .iter()
            .enumerate()
            .filter_map(|(index, slot)| slot.image.is_some().then_some(index))
            .collect()
    }

    #[cfg(test)]
    pub(crate) fn image(&self, index: usize) -> Option<Arc<gpui_kit::RenderImage>> {
        self.slots
            .get(index)
            .and_then(|slot| slot.image.as_ref())
            .map(markdown::images::Bitmap::image)
    }

    /// Whether the document has pages to frame; false while locked or failed.
    pub(crate) fn has_pages(&self) -> bool {
        !self.dims.is_empty()
    }

    pub(crate) fn zoom_label(&self) -> SharedString {
        format!("{:.0}%", self.page_width() / lyrel_pdf::PAGE_WIDTH * 100.).into()
    }

    pub(crate) fn status_text(&self, cx: &App) -> SharedString {
        let document = self.document.read(cx);
        match document.content() {
            PdfContent::Ready(_) => {
                let page = self.current.min(self.dims.len().saturating_sub(1)) + 1;
                let mut parts = Vec::with_capacity(2);
                parts.push(t_fmt(
                    L10nKey::EditorPdfPageOf,
                    &[
                        ("page", &page.to_string()),
                        ("total", &self.dims.len().to_string()),
                    ],
                ));
                if document.reloading() {
                    parts.push(t(L10nKey::EditorImageLoading).to_string());
                }
                parts.join(" · ").into()
            }
            PdfContent::Locked { wrong: true } => format!(
                "{} · {}",
                t(L10nKey::EditorPdfPasswordTitle),
                t(L10nKey::EditorPdfWrongPassword)
            )
            .into(),
            PdfContent::Locked { wrong: false } => t(L10nKey::EditorPdfPasswordTitle).into(),
            PdfContent::Unlocking => t(L10nKey::EditorPdfUnlocking).into(),
            PdfContent::Failed(message) => {
                format!("{} · {message}", t(L10nKey::EditorPdfFailed)).into()
            }
        }
    }

    fn page_width(&self) -> f32 {
        let viewport = self
            .viewport
            .map(|size| f32::from(size.width))
            .unwrap_or(lyrel_pdf::PAGE_WIDTH);
        let available_width = (viewport - 2. * PAGE_PAD_X).max(32.);
        let width = match self.fit {
            Some(Fit::Width) => available_width,
            Some(Fit::Page) => {
                let (page_width, page_height) = self
                    .dims
                    .get(self.current.min(self.dims.len().saturating_sub(1)))
                    .copied()
                    .unwrap_or((612., 792.));
                let viewport_height = self
                    .viewport
                    .map(|size| f32::from(size.height))
                    .unwrap_or(900.);
                let available_height = (viewport_height - 2. * lyrel_pdf::PAGE_PAD_Y).max(32.);
                available_width.min(available_height * page_width / page_height.max(1.))
            }
            None => lyrel_pdf::PAGE_WIDTH * self.zoom,
        };
        width.clamp(32., MAX_PAGE_WIDTH)
    }

    fn bump_generation(&mut self, cx: &mut Context<Self>) {
        self.generation = self.generation.wrapping_add(1);
        self.cancel.cancel();
        self.cancel = CancelToken::default();
        cx.notify();
    }

    fn set_viewport(&mut self, size: Size<Pixels>, cx: &mut Context<Self>) {
        let changed = self.viewport.is_none_or(|old| {
            f32::from(old.width - size.width).abs() > 0.5
                || f32::from(old.height - size.height).abs() > 0.5
        });
        if !changed {
            return;
        }
        self.viewport = Some(size);
        if self.fit.is_some() {
            self.bump_generation(cx);
        } else {
            cx.notify();
        }
    }

    pub(crate) fn zoom_in(&mut self, cx: &mut Context<Self>) {
        let current = self.page_width() / lyrel_pdf::PAGE_WIDTH;
        self.fit = None;
        self.zoom = (current * ZOOM_STEP).clamp(ZOOM_MIN, ZOOM_MAX);
        self.bump_generation(cx);
    }

    pub(crate) fn zoom_out(&mut self, cx: &mut Context<Self>) {
        let current = self.page_width() / lyrel_pdf::PAGE_WIDTH;
        self.fit = None;
        self.zoom = (current / ZOOM_STEP).clamp(ZOOM_MIN, ZOOM_MAX);
        self.bump_generation(cx);
    }

    pub(crate) fn fit_width(&mut self, cx: &mut Context<Self>) {
        if self.fit != Some(Fit::Width) {
            self.fit = Some(Fit::Width);
            self.bump_generation(cx);
        }
    }

    pub(crate) fn fit_page(&mut self, cx: &mut Context<Self>) {
        if self.fit != Some(Fit::Page) {
            self.fit = Some(Fit::Page);
            self.bump_generation(cx);
        }
    }

    pub(crate) fn actual_size(&mut self, cx: &mut Context<Self>) {
        self.fit = None;
        self.zoom = 1.;
        self.bump_generation(cx);
    }

    fn go_to_page(&mut self, index: usize, cx: &mut Context<Self>) {
        if self.dims.is_empty() {
            return;
        }
        let index = index.min(self.dims.len() - 1);
        let page_width = self.page_width();
        let top = lyrel_pdf::page_top_y(&self.dims, page_width, index);
        let viewport_height = self
            .viewport
            .map(|size| f32::from(size.height))
            .unwrap_or(900.);
        let content = lyrel_pdf::content_height(&self.dims, page_width);
        let max_scroll = (content - viewport_height).max(0.);
        let y = (-top).clamp(-max_scroll, 0.);
        self.scroll.set_offset(Point {
            x: px(0.),
            y: px(y),
        });
        self.current = index;
        cx.notify();
    }

    pub(crate) fn next_page(&mut self, cx: &mut Context<Self>) {
        if self.slots.is_empty() {
            return;
        }
        self.go_to_page((self.current + 1).min(self.dims.len() - 1), cx);
    }

    pub(crate) fn previous_page(&mut self, cx: &mut Context<Self>) {
        if self.slots.is_empty() {
            return;
        }
        self.go_to_page(self.current.saturating_sub(1), cx);
    }

    fn submit_password(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.document.read(cx).content().is_locked() {
            return;
        }
        let password = self.password.read(cx).value().to_string();
        // The input owns only this pending entry; clear it before parsing starts.
        self.password
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.document
            .update(cx, |document, cx| document.unlock(password, cx));
    }

    fn sync_document(&mut self, cx: &mut Context<Self>) -> Option<Arc<lyrel_pdf::PdfDocument>> {
        let (revision, ready) = {
            let document = self.document.read(cx);
            (document.revision(), document.ready())
        };
        if revision != self.revision {
            self.revision = revision;
            self.dims = ready
                .as_ref()
                .map(|document| document.page_dims().to_vec())
                .unwrap_or_default();
            self.slots = (0..self.dims.len()).map(|_| PageSlot::default()).collect();
            self.current = self.current.min(self.dims.len().saturating_sub(1));
            self.bump_generation(cx);
        }
        ready
    }

    /// Keep only pages near the viewport rasterized and spawn missing or stale
    /// ones on the background executor. Every result is checked against the
    /// document revision, generation and current window before it can install
    /// a texture or move focus.
    fn ensure_window(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(document) = self.sync_document(cx) else {
            return;
        };
        if self.dims.is_empty() {
            return;
        }
        let page_width = self.page_width();
        let viewport_height = f32::from(self.scroll.bounds().size.height);
        let scroll_y = f32::from(-self.scroll.offset().y);
        let (start, end) =
            lyrel_pdf::keep_window(&self.dims, page_width, scroll_y, viewport_height);
        self.current = lyrel_pdf::current_page(&self.dims, page_width, scroll_y)
            .min(self.dims.len().saturating_sub(1));

        for (index, slot) in self.slots.iter_mut().enumerate() {
            if index < start || index > end {
                // Dropping the bitmap retires its texture through the existing
                // `Bitmap` lease; a re-entering page gets a fresh attempt.
                slot.image = None;
                slot.rendered = 0;
                slot.attempted = 0;
                slot.loading = None;
                slot.failed = false;
            }
        }

        let generation = self.generation;
        let mut to_render = Vec::new();
        let mut attempt = self.next_attempt;
        for (index, slot) in self.slots.iter_mut().enumerate() {
            if index < start || index > end {
                continue;
            }
            let stale = slot.image.is_none() || slot.rendered != generation;
            let busy = slot
                .loading
                .is_some_and(|(_, loading_generation)| loading_generation == generation);
            if !busy && stale && slot.attempted != generation {
                slot.loading = Some((attempt, generation));
                slot.attempted = generation;
                slot.failed = false;
                to_render.push((index, attempt));
                attempt = attempt.wrapping_add(1);
            }
        }
        self.next_attempt = attempt;
        if to_render.is_empty() {
            return;
        }
        let center = (start + end) / 2;
        to_render.sort_by_key(|&(index, _)| (index as i64 - center as i64).unsigned_abs());

        let scale_factor = window.scale_factor();
        let document_id = self.document.entity_id();
        let revision = self.revision;
        for (index, attempt) in to_render {
            let scale = lyrel_pdf::render_scale(page_width, scale_factor, self.dims[index].0);
            let document = document.clone();
            let cancel = self.cancel.clone();
            #[cfg(test)]
            let gate = if index == 0 {
                tests::take_work(tests::Work::FirstPage)
            } else {
                None
            };
            cx.spawn(async move |view, cx| {
                let rendered = cx
                    .background_executor()
                    .spawn(async move {
                        #[cfg(test)]
                        if let Some(gate) = gate.as_ref() {
                            gate.block().await;
                        }
                        let result = document.render_page(index, scale, &|| cancel.is_cancelled());
                        #[cfg(test)]
                        if let Some(gate) = gate {
                            gate.finish();
                        }
                        result
                    })
                    .await;
                let _ = view.update(cx, |view, cx| {
                    view.land_page(
                        index,
                        attempt,
                        document_id,
                        revision,
                        generation,
                        rendered,
                        cx,
                    );
                });
            })
            .detach();
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn land_page(
        &mut self,
        index: usize,
        attempt: u64,
        document_id: gpui_kit::EntityId,
        revision: u64,
        generation: u64,
        rendered: Result<lyrel_pdf::RenderedPage, lyrel_pdf::RenderError>,
        cx: &mut Context<Self>,
    ) {
        // A newer attempt for the same page owns the slot now; this result is
        // retired even if it carries the same generation.
        let owns_slot = self
            .slots
            .get(index)
            .is_some_and(|slot| slot.loading == Some((attempt, generation)));
        if !owns_slot {
            return;
        }
        if let Some(slot) = self.slots.get_mut(index) {
            slot.loading = None;
        }
        if self.document.entity_id() != document_id
            || self.document.read(cx).revision() != revision
            || self.generation != generation
        {
            return;
        }
        let page_width = self.page_width();
        let viewport_height = f32::from(self.scroll.bounds().size.height);
        let scroll_y = f32::from(-self.scroll.offset().y);
        let (start, end) =
            lyrel_pdf::keep_window(&self.dims, page_width, scroll_y, viewport_height);
        if index < start || index > end {
            if let Some(slot) = self.slots.get_mut(index) {
                slot.image = None;
                slot.attempted = 0;
            }
            return;
        }
        match rendered {
            Ok(page) => {
                let rgba = image::RgbaImage::from_raw(page.width, page.height, page.rgba);
                let Some(slot) = self.slots.get_mut(index) else {
                    return;
                };
                match rgba {
                    Some(rgba) => {
                        slot.image = Some(markdown::images::bitmap(rgba, cx));
                        slot.rendered = generation;
                        slot.failed = false;
                    }
                    None => slot.failed = true,
                }
            }
            Err(lyrel_pdf::RenderError::Cancelled) => {
                if let Some(slot) = self.slots.get_mut(index) {
                    slot.attempted = 0;
                }
            }
            Err(_) => {
                if let Some(slot) = self.slots.get_mut(index) {
                    slot.failed = true;
                }
            }
        }
        cx.notify();
    }

    fn toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let ready = self.document.read(cx).content().ready().is_some();
        let page = (self.current + 1).min(self.dims.len().max(1));
        let total = self.dims.len();
        h_flex()
            .flex_none()
            .w_full()
            .items_center()
            .gap_2()
            .px_2()
            .py_1()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                div()
                    .flex_none()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child("PDF"),
            )
            .when(ready, |row| {
                row.child(
                    Button::new("pdf-previous")
                        .ghost()
                        .xsmall()
                        .flex_none()
                        .disabled(page <= 1)
                        .label("‹")
                        .accessibility_label(t(L10nKey::EditorPdfPreviousPage))
                        .tooltip(t(L10nKey::EditorPdfPreviousPage))
                        .on_click(cx.listener(|this, _, _, cx| this.previous_page(cx))),
                )
                .child(
                    div()
                        .id("pdf-page-identity")
                        .test_support()
                        .flex_none()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(t_fmt(
                            L10nKey::EditorPdfPageOf,
                            &[("page", &page.to_string()), ("total", &total.to_string())],
                        )),
                )
                .child(
                    Button::new("pdf-next")
                        .ghost()
                        .xsmall()
                        .flex_none()
                        .disabled(page >= total)
                        .label("›")
                        .accessibility_label(t(L10nKey::EditorPdfNextPage))
                        .tooltip(t(L10nKey::EditorPdfNextPage))
                        .on_click(cx.listener(|this, _, _, cx| this.next_page(cx))),
                )
                .child(div().flex_1())
                .child(
                    Button::new("pdf-fit-width")
                        .ghost()
                        .xsmall()
                        .flex_none()
                        .label(t(L10nKey::EditorPdfFitWidth))
                        .accessibility_label(t(L10nKey::EditorPdfFitWidth))
                        .tooltip(t(L10nKey::EditorPdfFitWidth))
                        .on_click(cx.listener(|this, _, _, cx| this.fit_width(cx))),
                )
                .child(
                    Button::new("pdf-fit-page")
                        .ghost()
                        .xsmall()
                        .flex_none()
                        .label(t(L10nKey::EditorPdfFitPage))
                        .accessibility_label(t(L10nKey::EditorPdfFitPage))
                        .tooltip(t(L10nKey::EditorPdfFitPage))
                        .on_click(cx.listener(|this, _, _, cx| this.fit_page(cx))),
                )
                .child(
                    Button::new("pdf-zoom-out")
                        .ghost()
                        .xsmall()
                        .flex_none()
                        .label("−")
                        .accessibility_label(t(L10nKey::EditorImageZoomOut))
                        .tooltip(t(L10nKey::EditorImageZoomOut))
                        .on_click(cx.listener(|this, _, _, cx| this.zoom_out(cx))),
                )
                .child(
                    div()
                        .id("pdf-zoom")
                        .test_support()
                        .flex_none()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(self.zoom_label()),
                )
                .child(
                    Button::new("pdf-zoom-in")
                        .ghost()
                        .xsmall()
                        .flex_none()
                        .label("+")
                        .accessibility_label(t(L10nKey::EditorImageZoomIn))
                        .tooltip(t(L10nKey::EditorImageZoomIn))
                        .on_click(cx.listener(|this, _, _, cx| this.zoom_in(cx))),
                )
            })
    }

    fn render_ready(
        &self,
        document: Arc<lyrel_pdf::PdfDocument>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let page_width = self.page_width();
        let border = cx.theme().border;
        let muted = cx.theme().muted_foreground;
        let foreground = cx.theme().foreground;
        let content_height = lyrel_pdf::content_height(&self.dims, page_width);
        let (start, end) = lyrel_pdf::keep_window(
            &self.dims,
            page_width,
            f32::from(-self.scroll.offset().y),
            f32::from(self.scroll.bounds().size.height),
        );
        let mut column = div()
            .relative()
            .w_full()
            .min_w(px(page_width + PAGE_PAD_X * 2.))
            .h(px(content_height));
        for index in start..=end {
            let Some(dim) = self.dims.get(index) else {
                break;
            };
            let height = lyrel_pdf::display_height(*dim, page_width);
            let slot = &self.slots[index];
            let image = slot.image.as_ref().map(markdown::images::Bitmap::image);
            let mut page = div()
                .id(SharedString::from(format!("pdf-page-{index}")))
                .test_support()
                .absolute()
                .top(px(lyrel_pdf::page_top_y(&self.dims, page_width, index)))
                .left(gpui_kit::relative(0.5))
                .ml(px(-page_width / 2.))
                .w(px(page_width))
                .h(px(height))
                .overflow_hidden()
                .bg(gpui_kit::white())
                .border_1()
                .border_color(border);
            match image {
                Some(image) => {
                    page = page.child(gpui_kit::img(image).size_full());
                }
                None => {
                    page = page.child(
                        v_flex()
                            .size_full()
                            .items_center()
                            .justify_center()
                            .gap_1()
                            .child(div().text_xs().text_color(muted).child(t_fmt(
                                L10nKey::EditorPdfPageOf,
                                &[
                                    ("page", &(index + 1).to_string()),
                                    ("total", &self.dims.len().to_string()),
                                ],
                            )))
                            .when(slot.failed, |column| {
                                column.child(
                                    div()
                                        .text_xs()
                                        .text_color(foreground)
                                        .child(t(L10nKey::EditorPdfRenderFailed)),
                                )
                            })
                            .when(!slot.failed, |column| {
                                column.child(
                                    div()
                                        .text_xs()
                                        .text_color(muted)
                                        .child(t(L10nKey::EditorImageLoading)),
                                )
                            }),
                    );
                }
            }
            for field in document
                .fields()
                .iter()
                .filter(|field| field.page == index && field.needs_text_overlay())
            {
                page = page.child(field_overlay(field, page_width, *dim));
            }
            column = column.child(page);
        }
        div()
            .on_children_prepainted({
                let view = cx.entity().downgrade();
                let scroll = self.scroll.clone();
                move |_, _, cx| {
                    let size = scroll.bounds().size;
                    let _ = view.update(cx, |view, cx| view.set_viewport(size, cx));
                }
            })
            .id("pdf-scroll")
            .size_full()
            .min_h_0()
            .overflow_scroll()
            .track_scroll(&self.scroll)
            .child(column)
            .into_any_element()
    }

    fn render_password(&self, wrong: bool, cx: &mut Context<Self>) -> AnyElement {
        let title = if wrong {
            t(L10nKey::EditorPdfWrongPassword)
        } else {
            t(L10nKey::EditorPdfPasswordTitle)
        };
        v_flex()
            .id("pdf-password")
            .test_support()
            .size_full()
            .items_center()
            .justify_center()
            .gap_3()
            .px_4()
            .child(
                Icon::new(IconName::TriangleAlert)
                    .large()
                    .text_color(cx.theme().warning),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().foreground)
                    .child(title),
            )
            .child(div().w(rems(20.)).child(Input::new(&self.password).small()))
            .child(
                Button::new("pdf-unlock")
                    .small()
                    .label(t(L10nKey::EditorPdfUnlock))
                    .accessibility_label(t(L10nKey::EditorPdfUnlock))
                    .on_click(cx.listener(|this, _, window, cx| this.submit_password(window, cx))),
            )
            .into_any_element()
    }
}

/// A value with no `/AP` is drawn as read-only text at its decrypted field
/// rectangle. Coordinates are normalized page fractions; font size is in PDF
/// points and scales with the current page width.
fn field_overlay(
    field: &lyrel_pdf::FormField,
    page_width: f32,
    page_dim: (f32, f32),
) -> AnyElement {
    let scale = page_width / page_dim.0.max(1.);
    let rect = field.rect;
    let width = (rect.w * page_width).max(1.);
    let height = (rect.h * lyrel_pdf::display_height(page_dim, page_width)).max(1.);
    let font_size = (field.font_size.unwrap_or(11.) * scale).clamp(1., height);
    let line_height = font_size * 1.2;
    let lines: Vec<&str> = match field.multiline {
        true => field
            .value
            .split(['\r', '\n'])
            .filter(|line| !line.is_empty() || field.value.contains('\n'))
            .collect(),
        false => vec![field.value.as_str()],
    };
    let mut text = v_flex().w_full();
    if !field.multiline {
        text = text.justify_center();
    }
    for line in lines {
        let row = match field.alignment {
            lyrel_pdf::FieldAlignment::Left => h_flex().justify_start(),
            lyrel_pdf::FieldAlignment::Center => h_flex().justify_center(),
            lyrel_pdf::FieldAlignment::Right => h_flex().justify_end(),
        };
        text = text.child(
            row.w_full().child(
                div()
                    .text_size(px(font_size))
                    .line_height(px(line_height))
                    .text_color(gpui_kit::black().opacity(0.88))
                    .child(line.to_string()),
            ),
        );
    }
    div()
        .id(SharedString::from(format!("pdf-field-{}", field.name)))
        .test_support()
        .aria_label(field.value.clone())
        .absolute()
        .left(px(rect.x * page_width))
        .top(px(rect.y * lyrel_pdf::display_height(page_dim, page_width)))
        .w(px(width))
        .h(px(height))
        .px(px((scale * 2.).min(6.)))
        .py(px(scale.min(4.)))
        .overflow_hidden()
        .child(text)
        .into_any_element()
}

impl Focusable for PdfView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for PdfView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.ensure_window(window, cx);
        let content = self.document.read(cx).content().clone();
        let focused = self.focus.contains_focused(window, cx);
        let ring = if focused {
            cx.theme().primary
        } else {
            gpui_kit::transparent_black()
        };
        let body = match content {
            PdfContent::Ready(document) => self.render_ready(document, cx),
            PdfContent::Locked { wrong } => self.render_password(wrong, cx),
            PdfContent::Unlocking => v_flex()
                .id("pdf-unlocking")
                .test_support()
                .size_full()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(t(L10nKey::EditorPdfUnlocking)),
                )
                .into_any_element(),
            PdfContent::Failed(message) => v_flex()
                .id("pdf-failed")
                .test_support()
                .size_full()
                .items_center()
                .justify_center()
                .gap_2()
                .px_4()
                .child(
                    Icon::new(IconName::TriangleAlert)
                        .large()
                        .text_color(cx.theme().warning),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().foreground)
                        .child(t(L10nKey::EditorPdfFailed)),
                )
                .child(
                    div()
                        .max_w(rems(30.))
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(message),
                )
                .into_any_element(),
        };
        v_flex()
            .id(("pdf-document", cx.entity_id()))
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &PdfZoomIn, _, cx| this.zoom_in(cx)))
            .on_action(cx.listener(|this, _: &PdfZoomOut, _, cx| this.zoom_out(cx)))
            .on_action(cx.listener(|this, _: &PdfFitWidth, _, cx| this.fit_width(cx)))
            .on_action(cx.listener(|this, _: &PdfFitPage, _, cx| this.fit_page(cx)))
            .on_action(cx.listener(|this, _: &PdfActualSize, _, cx| this.actual_size(cx)))
            .on_action(cx.listener(|this, _: &PdfNextPage, _, cx| this.next_page(cx)))
            .on_action(cx.listener(|this, _: &PdfPreviousPage, _, cx| this.previous_page(cx)))
            .on_key_down(
                cx.listener(|this, event: &gpui_kit::KeyDownEvent, _window, cx| {
                    match event.keystroke.key.as_str() {
                        "left" | "pageup" => this.previous_page(cx),
                        "right" | "pagedown" => this.next_page(cx),
                        _ => return,
                    }
                    cx.stop_propagation();
                }),
            )
            .on_mouse_down(
                gpui_kit::MouseButton::Left,
                cx.listener(|this, _, window, cx| this.focus.focus(window, cx)),
            )
            .size_full()
            .min_w_0()
            .min_h_0()
            .border_1()
            .border_color(ring)
            .child(self.toolbar(cx))
            .child(body)
    }
}

/// 右侧菜单里的取景命令，与快捷键、工具栏按钮是同一组动作。
pub(super) fn menu_entries(
    menu: gpui_kit::component::menu::PopupMenu,
) -> gpui_kit::component::menu::PopupMenu {
    menu.separator()
        .menu(t(L10nKey::EditorPdfPreviousPage), Box::new(PdfPreviousPage))
        .menu(t(L10nKey::EditorPdfNextPage), Box::new(PdfNextPage))
        .separator()
        .menu(t(L10nKey::EditorPdfFitWidth), Box::new(PdfFitWidth))
        .menu(t(L10nKey::EditorPdfFitPage), Box::new(PdfFitPage))
        .menu(t(L10nKey::EditorImageActualSize), Box::new(PdfActualSize))
        .menu(t(L10nKey::EditorImageZoomIn), Box::new(PdfZoomIn))
        .menu(t(L10nKey::EditorImageZoomOut), Box::new(PdfZoomOut))
}

#[cfg(test)]
#[path = "pdf_document/tests.rs"]
mod tests;
