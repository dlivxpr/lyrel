//! 只读 PDF 文档：页、缩放、大纲、选字复制、搜索、密码与既有批注／表单外观。
//!
//! 工作台拥有文档身份、所属 Host、标签页、保存／关闭与焦点；这里只拥有
//! “这份文件解析后能显示什么”和每个编辑组自己的页／缩放／选区／搜索取景。
//! 字节由 owning Host 读取，解析、页文本提取、全文搜索与单页光栅化都在
//! 后台完成，并有独立的并发与结果预算。PDF 原文始终是权威：无 `/AP` 的
//! 表单值只以真实字段几何叠加为只读文本，绝不写回或重新序列化。
//!
//! 本切片不创建批注、不填表、不写 PDF，也不做 OCR：扫描页没有文本层时
//! 明确声明不可搜索／选字，而不是把空的搜索结果当成词不存在。

use std::collections::{HashMap, VecDeque};
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
        PdfPreviousPage,
        PdfCopy
    ]
);

/// 只有 PDF 文档持有键盘时成立。
pub(crate) const KEY_CONTEXT: &str = "PdfDocument";

const ZOOM_MIN: f32 = 0.5;
const ZOOM_MAX: f32 = 3.;
const ZOOM_STEP: f32 = 1.25;
const PAGE_PAD_X: f32 = 12.;
const MAX_PAGE_WIDTH: f32 = 4096.;

/// 每呈现缓存的页文本层上限；超出时丢弃最早提取、且没有活跃选区的页。
const TEXT_CACHE_PAGES: usize = 16;
/// 同时在后台提取的页文本任务数；搜索扫掠再占一个后台任务。
const TEXT_JOBS: usize = 2;
/// 全文搜索的结果预算；命中更多时明确标记截断，而不是假装查完。
const MAX_MATCHES: usize = 1000;
/// 搜索扫掠带回 UI 的最大文本层页数，防止整篇提取结果常驻内存。
const MAX_SWEPT_TEXT_PAGES: usize = TEXT_CACHE_PAGES;

/// 一页文本层的呈现内缓存状态。
enum TextSlot {
    /// 已排队或在后台提取中。
    Loading,
    Ready(Arc<lyrel_pdf::PageText>),
    /// 页面存在但没有文本层（纯扫描页）。
    Empty,
    /// 解释过程失败；搜索明确报告该页无法读取。
    Failed,
}

/// 一次全文搜索命中：所在页与每行一个归一化高亮矩形。
struct SearchMatch {
    page: usize,
    rects: Vec<lyrel_pdf::NormRect>,
}

/// 后台搜索扫掠的结果：命中、预算信息与要并入缓存的文本层。
struct SweepResult {
    matches: Vec<SearchMatch>,
    texts: Vec<(usize, Arc<lyrel_pdf::PageText>)>,
    text_pages: usize,
    failed_pages: usize,
    truncated: bool,
    cancelled: bool,
}

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

/// 一份 PDF 在一个分组里的取景：页、缩放、位图窗口、选区与搜索。
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
    last_scroll_y: f32,
    viewport: Option<Size<Pixels>>,
    generation: u64,
    next_attempt: u64,
    cancel: CancelToken,
    password: Entity<InputState>,
    // 大纲面板：展开状态与键盘游标都是每个呈现自己的。
    toc_open: bool,
    toc_index: usize,
    // 页文本层：有界缓存、队列与并发计数；提取在 UI 线程之外。
    page_text: HashMap<usize, TextSlot>,
    text_queue: VecDeque<usize>,
    text_jobs: usize,
    // 拖拽选区：进行中的 (页, 起点, 当前点)，完成后只留 (页, 起点, 终点)。
    sel_drag: Option<(usize, lyrel_pdf::NormPoint, lyrel_pdf::NormPoint)>,
    text_sel: Option<(usize, lyrel_pdf::NormPoint, lyrel_pdf::NormPoint)>,
    // 文档内查找：输入框、结果、当前命中与扫掠身份。
    find_open: bool,
    find_input: Entity<InputState>,
    matches: Vec<SearchMatch>,
    current_match: Option<usize>,
    search_seq: u64,
    search_running: bool,
    search_text_pages: usize,
    search_failed_pages: usize,
    search_truncated: bool,
    text_cancel: CancelToken,
    _find_sub: Subscription,
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
        let find_input =
            cx.new(|cx| InputState::new(window, cx).placeholder(t(L10nKey::SearchFind)));
        let find_sub = cx.subscribe_in(
            &find_input,
            window,
            |view, _, event: &InputEvent, _, cx| match event {
                InputEvent::Change => view.query_changed(cx),
                InputEvent::PressEnter { shift, .. } => {
                    if *shift {
                        view.previous_match(cx);
                    } else {
                        view.next_match(cx);
                    }
                }
                _ => {}
            },
        );
        cx.on_release(|view, _| {
            view.cancel.cancel();
            view.text_cancel.cancel();
        })
        .detach();
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
            last_scroll_y: f32::NAN,
            viewport: None,
            generation: 1,
            next_attempt: 1,
            cancel: CancelToken::default(),
            password,
            toc_open: false,
            toc_index: 0,
            page_text: HashMap::new(),
            text_queue: VecDeque::new(),
            text_jobs: 0,
            sel_drag: None,
            text_sel: None,
            find_open: false,
            find_input,
            matches: Vec::new(),
            current_match: None,
            search_seq: 0,
            search_running: false,
            search_text_pages: 0,
            search_failed_pages: 0,
            search_truncated: false,
            text_cancel: CancelToken::default(),
            _find_sub: find_sub,
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

    /// 是否有选中文字可复制。
    pub(crate) fn has_selection(&self) -> bool {
        self.text_sel.is_some()
    }

    #[cfg(test)]
    pub(crate) fn scroll_offset(&self) -> Point<Pixels> {
        self.scroll.offset()
    }

    #[cfg(test)]
    pub(crate) fn match_count(&self) -> usize {
        self.matches.len()
    }

    #[cfg(test)]
    pub(crate) fn current_match_index(&self) -> Option<usize> {
        self.current_match
    }

    #[cfg(test)]
    pub(crate) fn text_page(&self, index: usize) -> Option<Arc<lyrel_pdf::PageText>> {
        self.page_text.get(&index).and_then(|slot| match slot {
            TextSlot::Ready(text) => Some(text.clone()),
            _ => None,
        })
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
                if self.search_text_pages == 0
                    && !self.search_running
                    && self.search_seq > 0
                    && !self.dims.is_empty()
                    // 只有真的按非空查询扫完整篇，才有资格下这个结论；空查询
                    // 从没扫过任何页，不能因此把有文本的 PDF 说成无文本层。
                    && !self.find_input.read(cx).value().trim().is_empty()
                {
                    // 已完成的全文扫掠证明整篇都没有文本层；空命中不是
                    // “词不存在”的证据，状态行如实标注。
                    parts.push(t(L10nKey::EditorPdfNoTextLayer).to_string());
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
        // 缩放／适应改变页面像素后，命中的归一化矩形仍指向同一段文字；
        // 重新走一次跳转把当前命中所在的页与位置放回视野，而不是停在旧
        // 页宽算出的滚动位置。
        if self.find_open
            && let Some(index) = self.current_match
        {
            self.goto_match(index, cx);
        }
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
        self.last_scroll_y = f32::from(-self.scroll.offset().y);
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

    /// 跳到一个大纲／链接目的地：带坐标时把目标点放到视口顶部留边，
    /// 否则整页对齐；x 坐标只在页面比视口宽时水平居中该点。
    fn go_to_destination(&mut self, destination: &lyrel_pdf::Destination, cx: &mut Context<Self>) {
        if self.dims.is_empty() {
            return;
        }
        let page = destination.page.min(self.dims.len() - 1);
        let page_width = self.page_width();
        let display = lyrel_pdf::display_height(self.dims[page], page_width);
        let viewport = self.scroll.bounds().size;
        let viewport_h = f32::from(viewport.height).max(1.);
        let viewport_w = f32::from(viewport.width).max(1.);
        let content = lyrel_pdf::content_height(&self.dims, page_width);
        let max_scroll = (content - viewport_h).max(0.);
        let point_y = lyrel_pdf::page_top_y(&self.dims, page_width, page)
            + destination.y.unwrap_or(0.) * display;
        // 无 y 的目的地整页对齐；有 y 的留一点上缘，让跳转点在视野里。
        let y = if destination.y.is_some() {
            point_y - 48.
        } else {
            point_y
        };
        let offset_y = (-y).clamp(-max_scroll, 0.);
        let column_w = (page_width + 2. * PAGE_PAD_X).max(viewport_w);
        let offset_x = match destination.x {
            Some(fx) if column_w > viewport_w => {
                let point_x = column_w / 2. - page_width / 2. + fx * page_width;
                (-(point_x - viewport_w / 2.)).clamp(-(column_w - viewport_w), 0.)
            }
            _ => 0.,
        };
        self.scroll.set_offset(Point {
            x: px(offset_x),
            y: px(offset_y),
        });
        self.current = page;
        // 显式钉住的页是 source of truth:把 last_scroll_y 预填为 set_offset 后
        // ensure_window 会看到的值,避免下一帧用旧 scroll 反推覆盖 current。
        self.last_scroll_y = f32::from(-self.scroll.offset().y);
        cx.notify();
    }

    fn toggle_toc(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.toc_open = !self.toc_open;
        if self.toc_open {
            // 打开后键盘上下方向键直接在大纲里移动，焦点留在文档呈现上。
            self.focus.focus(window, cx);
            let items = self
                .document
                .read(cx)
                .ready()
                .map(|document| document.outline().len())
                .unwrap_or(0);
            self.toc_index = self.toc_index.min(items.saturating_sub(1));
        }
        cx.notify();
    }

    /// 大纲键盘游标移动一格；`activate_outline` 真正跳转。
    fn toc_step(&mut self, forward: bool, cx: &mut Context<Self>) {
        let items = self
            .document
            .read(cx)
            .ready()
            .map(|document| document.outline().len())
            .unwrap_or(0);
        if items == 0 {
            return;
        }
        self.toc_index = if forward {
            (self.toc_index + 1).min(items - 1)
        } else {
            self.toc_index.saturating_sub(1)
        };
        cx.notify();
    }

    /// 激活键盘游标所指的大纲条目；没有目的地的条目不可跳转。
    fn activate_outline(&mut self, cx: &mut Context<Self>) {
        let Some(document) = self.document.read(cx).ready() else {
            return;
        };
        let Some(destination) = document
            .outline()
            .get(self.toc_index)
            .and_then(|item| item.destination)
        else {
            return;
        };
        self.go_to_destination(&destination, cx);
    }

    /// 打开文档内查找并把焦点交给查询输入；再次触发只聚焦，不重置查询。
    fn open_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.document.read(cx).ready().is_none() {
            return;
        }
        self.find_open = true;
        self.find_input.update(cx, |input, cx| {
            input.focus(window, cx);
            input.select_all(window, cx);
        });
        self.start_sweep(cx);
        cx.notify();
    }

    /// 关闭查找条，清掉高亮与结果并退休还在路上的扫掠。
    fn close_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.find_open {
            return;
        }
        self.find_open = false;
        self.matches.clear();
        self.current_match = None;
        self.search_seq = self.search_seq.wrapping_add(1);
        self.search_running = false;
        self.text_cancel.cancel();
        self.text_cancel = CancelToken::default();
        self.focus.focus(window, cx);
        cx.notify();
    }

    /// 查询文字变了：退休旧扫掠，为当前输入重新启动一次有界全文搜索。
    fn query_changed(&mut self, cx: &mut Context<Self>) {
        if !self.find_open {
            return;
        }
        self.start_sweep(cx);
    }

    /// 一次全文扫掠：在后台逐页提取文本并收集命中，单个任务、结果与
    /// 带回的文本层都有上限。seq／revision 变化后结果一律不落地。
    fn start_sweep(&mut self, cx: &mut Context<Self>) {
        self.search_seq = self.search_seq.wrapping_add(1);
        let seq = self.search_seq;
        self.text_cancel.cancel();
        self.text_cancel = CancelToken::default();
        self.matches.clear();
        self.current_match = None;
        self.search_running = false;
        self.search_text_pages = 0;
        self.search_failed_pages = 0;
        self.search_truncated = false;
        let Some(document) = self.document.read(cx).ready() else {
            return;
        };
        let query = self.find_input.read(cx).value().to_string();
        if query.trim().is_empty() {
            cx.notify();
            return;
        }
        self.search_running = true;
        let pages = document.page_count();
        let cancelled = self.text_cancel.clone();
        let document_id = self.document.entity_id();
        let revision = self.document.read(cx).revision();
        #[cfg(test)]
        let gate = tests::take_work(tests::Work::Search);
        cx.spawn(async move |view, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    #[cfg(test)]
                    if let Some(gate) = gate.as_ref() {
                        gate.block().await;
                    }
                    let mut matches = Vec::new();
                    let mut texts = Vec::new();
                    let mut text_pages = 0usize;
                    let mut failed_pages = 0usize;
                    let mut truncated = false;
                    for page in 0..pages {
                        if cancelled.is_cancelled() {
                            #[cfg(test)]
                            if let Some(gate) = gate {
                                gate.finish();
                            }
                            return SweepResult {
                                matches,
                                texts,
                                text_pages,
                                failed_pages,
                                truncated,
                                cancelled: true,
                            };
                        }
                        match document.text(page) {
                            Some(text) if text.is_empty() => {}
                            Some(text) => {
                                text_pages += 1;
                                for rects in text.find_matches(&query) {
                                    if rects.is_empty() {
                                        continue;
                                    }
                                    if matches.len() >= MAX_MATCHES {
                                        truncated = true;
                                        break;
                                    }
                                    matches.push(SearchMatch { page, rects });
                                }
                                if !text.is_empty() && texts.len() < MAX_SWEPT_TEXT_PAGES {
                                    texts.push((page, Arc::new(text)));
                                }
                            }
                            None => failed_pages += 1,
                        }
                        if truncated {
                            break;
                        }
                    }
                    #[cfg(test)]
                    if let Some(gate) = gate {
                        gate.finish();
                    }
                    SweepResult {
                        matches,
                        texts,
                        text_pages,
                        failed_pages,
                        truncated,
                        cancelled: false,
                    }
                })
                .await;
            let _ = view.update(cx, |view, cx| {
                view.land_sweep(seq, document_id, revision, result, cx);
            });
        })
        .detach();
        cx.notify();
    }

    /// 扫掠结果落地：只对仍然是当前查询／文档／修订的结果更新状态。
    fn land_sweep(
        &mut self,
        seq: u64,
        document_id: gpui_kit::EntityId,
        revision: u64,
        result: SweepResult,
        cx: &mut Context<Self>,
    ) {
        if self.document.entity_id() != document_id
            || self.document.read(cx).revision() != revision
            || self.search_seq != seq
            || !self.find_open
        {
            return;
        }
        self.search_running = false;
        if result.cancelled {
            return;
        }
        self.search_text_pages = result.text_pages;
        self.search_failed_pages = result.failed_pages;
        self.search_truncated = result.truncated;
        self.matches = result.matches;
        for (page, text) in result.texts {
            self.install_page_text(page, TextSlot::Ready(text));
        }
        self.current_match = self.match_from_viewport();
        if let Some(index) = self.current_match {
            self.goto_match(index, cx);
        }
        cx.notify();
    }

    /// 第一个位于当前视口顶或之下的命中，让搜索从在读的位置开始。
    fn match_from_viewport(&self) -> Option<usize> {
        if self.matches.is_empty() {
            return None;
        }
        let page_width = self.page_width();
        let scroll_y = f32::from(-self.scroll.offset().y).max(0.);
        let index = self.matches.iter().position(|m| {
            let rect_y = m.rects.first().map(|rect| rect.y).unwrap_or(0.);
            lyrel_pdf::page_top_y(&self.dims, page_width, m.page)
                + rect_y * lyrel_pdf::display_height(self.dims[m.page], page_width)
                >= scroll_y
        });
        Some(index.unwrap_or(0))
    }

    pub(crate) fn next_match(&mut self, cx: &mut Context<Self>) {
        if self.matches.is_empty() {
            return;
        }
        let count = self.matches.len();
        let index = self
            .current_match
            .map_or(0, |current| (current + 1) % count);
        self.current_match = Some(index);
        self.goto_match(index, cx);
    }

    pub(crate) fn previous_match(&mut self, cx: &mut Context<Self>) {
        if self.matches.is_empty() {
            return;
        }
        let count = self.matches.len();
        let index = self
            .current_match
            .map_or(0, |current| (current + count - 1) % count);
        self.current_match = Some(index);
        self.goto_match(index, cx);
    }

    /// 命中已经在视野里就不滚动；否则滚到命中略低于视口顶的位置。
    fn goto_match(&mut self, index: usize, cx: &mut Context<Self>) {
        if self.dims.is_empty() {
            return;
        }
        let Some(m) = self.matches.get(index) else {
            return;
        };
        let page = m.page.min(self.dims.len() - 1);
        let rect_y = m.rects.first().map(|rect| rect.y).unwrap_or(0.);
        let rect_h = m.rects.first().map(|rect| rect.h).unwrap_or(0.);
        let page_width = self.page_width();
        let display = lyrel_pdf::display_height(self.dims[page], page_width);
        let top = lyrel_pdf::page_top_y(&self.dims, page_width, page) + rect_y * display;
        let bottom = top + rect_h * display;
        let scroll_y = f32::from(-self.scroll.offset().y).max(0.);
        let viewport_h = f32::from(self.scroll.bounds().size.height).max(1.);
        if top < scroll_y + 8. || bottom > scroll_y + viewport_h - 8. {
            let y = (top - 80.).max(0.);
            self.scroll.set_offset(Point {
                x: px(0.),
                y: px(-y),
            });
        }
        // 命中所在的页就是当前页:status 与后续 page 步进都以它为准,而不是
        // 让命中略高出页顶的 80px 留边把视口反推回上一页。
        self.current = page;
        self.last_scroll_y = f32::from(-self.scroll.offset().y);
        cx.notify();
    }

    /// 把一页文本层并入有界缓存：容量满时丢掉最旧、且不在选区里的页。
    fn install_page_text(&mut self, page: usize, slot: TextSlot) {
        self.page_text.insert(page, slot);
        while self.page_text.len() > TEXT_CACHE_PAGES {
            let selected = self
                .sel_drag
                .map(|(page, _, _)| page)
                .or(self.text_sel.map(|(page, _, _)| page));
            let Some(&oldest) = self.page_text.keys().find(|&&key| Some(key) != selected) else {
                break;
            };
            self.page_text.remove(&oldest);
        }
    }

    /// 需要某页文本层时在后台提取：并发受 TEXT_JOBS 限制，多出的页排队。
    fn ensure_page_text(&mut self, page: usize, cx: &mut Context<Self>) {
        if self.page_text.contains_key(&page) {
            return;
        }
        if self.document.read(cx).ready().is_none() {
            return;
        }
        self.page_text.insert(page, TextSlot::Loading);
        self.text_queue.push_back(page);
        self.pump_text(cx);
    }

    /// 把排队的页交给后台解释，直到达到并发预算。
    fn pump_text(&mut self, cx: &mut Context<Self>) {
        while self.text_jobs < TEXT_JOBS {
            let Some(page) = self.text_queue.pop_front() else {
                break;
            };
            let Some(document) = self.document.read(cx).ready() else {
                self.page_text.remove(&page);
                continue;
            };
            if !matches!(self.page_text.get(&page), Some(TextSlot::Loading)) {
                continue;
            }
            self.text_jobs += 1;
            let document_id = self.document.entity_id();
            let revision = self.document.read(cx).revision();
            cx.spawn(async move |view, cx| {
                let extracted = cx
                    .background_executor()
                    .spawn(async move { document.text(page) })
                    .await;
                let _ = view.update(cx, |view, cx| {
                    view.land_text(page, document_id, revision, extracted, cx);
                });
            })
            .detach();
        }
    }

    /// 页文本落地：只有仍占着 Loading 位、同一文档与修订的结果生效；
    /// 其它视为迟到结果丢弃，然后继续补满并发。
    fn land_text(
        &mut self,
        page: usize,
        document_id: gpui_kit::EntityId,
        revision: u64,
        extracted: Option<lyrel_pdf::PageText>,
        cx: &mut Context<Self>,
    ) {
        self.text_jobs = self.text_jobs.saturating_sub(1);
        let current = self.document.entity_id().eq(&document_id)
            && self.document.read(cx).revision() == revision;
        if current && matches!(self.page_text.get(&page), Some(TextSlot::Loading)) {
            match extracted {
                Some(text) if text.is_empty() => {
                    self.page_text.insert(page, TextSlot::Empty);
                }
                Some(text) => {
                    self.install_page_text(page, TextSlot::Ready(Arc::new(text)));
                }
                None => {
                    self.page_text.insert(page, TextSlot::Failed);
                }
            }
        }
        self.pump_text(cx);
        cx.notify();
    }

    /// 窗口坐标点到 (页, 归一化页内坐标)。页面在滚动列里的实际几何由
    /// scroll bounds、偏移和纯布局函数推出，不依赖该页是否已光栅化。
    fn point_to_page(&self, position: Point<Pixels>) -> Option<(usize, lyrel_pdf::NormPoint)> {
        if self.dims.is_empty() {
            return None;
        }
        let bounds = self.scroll.bounds();
        let offset = self.scroll.offset();
        let page_width = self.page_width();
        let column_w = (page_width + 2. * PAGE_PAD_X).max(f32::from(bounds.size.width));
        let column_x =
            f32::from(bounds.origin.x) + f32::from(offset.x) + column_w / 2. - page_width / 2.;
        let column_y = f32::from(bounds.origin.y) + f32::from(offset.y);
        let probe = f32::from(position.y) - column_y;
        for index in 0..self.dims.len() {
            let top = lyrel_pdf::page_top_y(&self.dims, page_width, index);
            let height = lyrel_pdf::display_height(self.dims[index], page_width);
            if probe >= top && probe < top + height {
                return Some((
                    index,
                    lyrel_pdf::NormPoint {
                        x: ((f32::from(position.x) - column_x) / page_width).clamp(0., 1.),
                        y: ((probe - top) / height.max(1.)).clamp(0., 1.),
                    },
                ));
            }
        }
        None
    }

    /// 复制当前选区文字到剪贴板；页文本层必须已在缓存中。
    fn copy_selection(&mut self, cx: &mut Context<Self>) {
        let Some((page, from, to)) = self.text_sel else {
            return;
        };
        let Some(TextSlot::Ready(text)) = self.page_text.get(&page) else {
            return;
        };
        if let Some(content) = text.selected_text(from, to) {
            cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(content));
        }
    }

    /// 一次鼠标拖动：起点在页内才记录起点，拖动/释放只更新同页终点。
    fn begin_drag(
        &mut self,
        event: &gpui_kit::MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.focus.focus(window, cx);
        self.text_sel = None;
        if !self.scroll.bounds().contains(&event.position) {
            cx.notify();
            return;
        }
        if let Some((page, point)) = self.point_to_page(event.position) {
            self.ensure_page_text(page, cx);
            self.sel_drag = Some((page, point, point));
        }
        cx.notify();
    }

    fn update_drag(&mut self, event: &gpui_kit::MouseMoveEvent, cx: &mut Context<Self>) {
        let Some((page, start, _)) = self.sel_drag else {
            return;
        };
        if let Some((current_page, point)) = self.point_to_page(event.position)
            && current_page == page
        {
            self.sel_drag = Some((page, start, point));
            cx.notify();
        }
    }

    /// 释放拖动：超过阈值成为持久选区，否则当普通点击处理。释放点本身
    /// 就是终点——快速拖动可能没有落在同一页的最后一次 move 事件。
    fn end_drag(&mut self, position: Option<Point<Pixels>>, cx: &mut Context<Self>) {
        const MIN_DRAG: f32 = 0.005;
        let Some((page, from, mut to)) = self.sel_drag.take() else {
            return;
        };
        if let Some((released_page, point)) = position.and_then(|p| self.point_to_page(p))
            && released_page == page
        {
            to = point;
        }
        if (from.x - to.x).abs() >= MIN_DRAG || (from.y - to.y).abs() >= MIN_DRAG {
            self.text_sel = Some((page, from, to));
        }
        cx.notify();
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
            self.last_scroll_y = f32::NAN;
            // 旧文档的文本层、选区与命中全部退休；查找条若开着，对新内容
            // 重新扫一遍而不是带着旧命中。
            self.page_text.clear();
            self.text_queue.clear();
            self.sel_drag = None;
            self.text_sel = None;
            self.matches.clear();
            self.current_match = None;
            self.toc_index = 0;
            self.bump_generation(cx);
            if self.find_open {
                self.start_sweep(cx);
            }
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
        // current 只在视口真正滚到别处时才从 scroll_y 反推,避免 go_to_destination
        // 显式钉住的页在 scroll 被 clamp(如视口比内容大)时被旧 offset 覆盖。
        if (scroll_y - self.last_scroll_y).abs() > 0.5 {
            self.last_scroll_y = scroll_y;
            self.current = lyrel_pdf::current_page(&self.dims, page_width, scroll_y)
                .min(self.dims.len().saturating_sub(1));
        }

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
                let has_outline = self
                    .document
                    .read(cx)
                    .ready()
                    .is_some_and(|document| !document.outline().is_empty());
                let mut row = row.child(
                    Button::new("pdf-find")
                        .ghost()
                        .xsmall()
                        .flex_none()
                        .label(t(L10nKey::AppMenuFind))
                        .accessibility_label(t(L10nKey::AppMenuFind))
                        .tooltip(t(L10nKey::AppMenuFind))
                        .on_click(cx.listener(|this, _, window, cx| this.open_find(window, cx))),
                );
                if has_outline {
                    row = row.child(
                        Button::new("pdf-toc-toggle")
                            .ghost()
                            .xsmall()
                            .flex_none()
                            .label(t(L10nKey::EditorPdfOutline))
                            .accessibility_label(t(L10nKey::EditorPdfOutline))
                            .tooltip(t(L10nKey::EditorPdfOutline))
                            .on_click(
                                cx.listener(|this, _, window, cx| this.toggle_toc(window, cx)),
                            ),
                    );
                }
                row
            })
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
            // 链接注释：透明点击目标，内部目的地走文档几何，外部 http(s)
            // URI 交给系统打开；与上游一样只允许这两种目标。
            if let Some(links) = document.links().get(index) {
                for (link_index, link) in links.iter().enumerate() {
                    let target = link.target.clone();
                    page = page.child(
                        div()
                            .id(SharedString::from(format!("pdf-link-{index}-{link_index}")))
                            .test_support()
                            .absolute()
                            .left(px(link.x * page_width))
                            .top(px(link.y * height))
                            .w(px(link.w * page_width))
                            .h(px(link.h * height))
                            .rounded(px(2.))
                            .cursor_pointer()
                            .hover(|link| link.bg(cx.theme().primary.opacity(0.12)))
                            .on_click(cx.listener(move |this, _, _window, cx| match &target {
                                lyrel_pdf::LinkTarget::Page(destination) => {
                                    this.go_to_destination(destination, cx)
                                }
                                lyrel_pdf::LinkTarget::Uri(uri) => cx.open_url(uri),
                            })),
                    );
                }
            }
            // 搜索命中高亮：每个命中一行一个归一化矩形；当前命中描边强调。
            if self.find_open {
                let mut match_boxes = Vec::new();
                for (match_index, m) in self.matches.iter().enumerate() {
                    if m.page != index {
                        continue;
                    }
                    let current = self.current_match == Some(match_index);
                    for rect in &m.rects {
                        let mut box_el = div()
                            .absolute()
                            .left(px(rect.x * page_width))
                            .top(px(rect.y * height))
                            .w(px(rect.w * page_width))
                            .h(px(rect.h * height))
                            .rounded(px(1.))
                            .bg(cx.theme().warning.opacity(if current { 0.5 } else { 0.3 }));
                        if current {
                            box_el = box_el.border_1().border_color(cx.theme().warning);
                        }
                        match_boxes.push(box_el);
                    }
                }
                if !match_boxes.is_empty() {
                    page = page.child(
                        div()
                            .id(SharedString::from(format!("pdf-match-{index}")))
                            .test_support()
                            .absolute()
                            .inset_0()
                            .children(match_boxes),
                    );
                }
            }
            // 选区高亮：拖拽中的实时反馈与完成后的持久选区一样，按归一化
            // 矩形绘制，缩放/重排只是重新映射同样的几何。
            if let Some((sel_page, from, to)) = self.sel_drag.or(self.text_sel)
                && sel_page == index
            {
                let rects = self
                    .page_text
                    .get(&index)
                    .and_then(|slot| match slot {
                        TextSlot::Ready(text) => Some(text.clone()),
                        _ => None,
                    })
                    .and_then(|text| text.select(from, to))
                    .map(|selection| selection.rects)
                    .unwrap_or_default();
                if !rects.is_empty() {
                    page = page.child(
                        div()
                            .id(SharedString::from(format!("pdf-selection-{index}")))
                            .test_support()
                            .absolute()
                            .inset_0()
                            .children(rects.iter().map(|rect| {
                                div()
                                    .absolute()
                                    .left(px(rect.x * page_width))
                                    .top(px(rect.y * height))
                                    .w(px(rect.w * page_width))
                                    .h(px(rect.h * height))
                                    .rounded(px(1.))
                                    .bg(cx.theme().selection.opacity(0.4))
                            })),
                    );
                }
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
            .flex_1()
            .min_w_0()
            // h_flex 交叉轴默认居中:不给高度时滚动区会取内容高并居中出视口
            // (bounds 跑到屏幕上方负坐标)。显式 h_full 让滚动视口就是容器高度,
            // offset/max_offset 与可见性才真实。
            .h_full()
            .overflow_scroll()
            .on_mouse_down(
                gpui_kit::MouseButton::Left,
                cx.listener(|this, event: &gpui_kit::MouseDownEvent, window, cx| {
                    this.begin_drag(event, window, cx)
                }),
            )
            .on_mouse_move(
                cx.listener(|this, event: &gpui_kit::MouseMoveEvent, _, cx| {
                    this.update_drag(event, cx)
                }),
            )
            .on_mouse_up(
                gpui_kit::MouseButton::Left,
                cx.listener(|this, event: &gpui_kit::MouseUpEvent, _, cx| {
                    this.end_drag(Some(event.position), cx)
                }),
            )
            .track_scroll(&self.scroll)
            .child(column)
            .into_any_element()
    }

    /// 查找条：输入、命中计数与上下一步；Enter / Shift+Enter 走同一条
    /// 步进逻辑，Escape 关闭并还焦点给文档。
    fn render_find_bar(&self, cx: &mut Context<Self>) -> AnyElement {
        let query = self.find_input.read(cx).value();
        let mut parts = Vec::new();
        if query.trim().is_empty() {
            parts.push("0 / 0".to_string());
        } else if self.search_running {
            parts.push(t(L10nKey::EditorPdfSearching).to_string());
        } else if self.matches.is_empty() {
            if self.search_text_pages == 0 {
                // 所有页都可读但没有任何文本层：如实说不可搜索，不回答
                // "无结果"。一页都读不出来时只报页级失败（见下方计数），
                // 同样不下"无匹配"的结论。
                if self.search_failed_pages == 0 {
                    parts.push(t(L10nKey::EditorPdfNoTextLayer).to_string());
                }
            } else {
                parts.push(t(L10nKey::EditorPdfNoMatches).to_string());
            }
        } else {
            let mut count = t_fmt(
                L10nKey::EditorPdfMatchOf,
                &[
                    (
                        "current",
                        &(self.current_match.map_or(0, |i| i + 1)).to_string(),
                    ),
                    ("total", &self.matches.len().to_string()),
                ],
            );
            if self.search_truncated {
                count = format!("{count}+");
            }
            parts.push(count);
        }
        if self.search_failed_pages > 0 && !self.search_running {
            parts.push(t_fmt(
                L10nKey::EditorPdfPagesSkipped,
                &[("count", &self.search_failed_pages.to_string())],
            ));
        }
        let count = parts.join(" · ");
        h_flex()
            .id("pdf-find-bar")
            .test_support()
            .key_context("PdfFindBar")
            .flex_none()
            .w_full()
            .items_center()
            .gap_2()
            .px_2()
            .py_1()
            .border_b_1()
            .border_color(cx.theme().border)
            // Enter/Shift-Enter 走 InputState 的 PressEnter 事件（订阅处统一
            // 步进）；Escape 动作在单行输入里会向上传播，直接在这里关掉。
            .on_action(
                cx.listener(|this, _: &gpui_kit::component::input::Escape, window, cx| {
                    this.close_find(window, cx)
                }),
            )
            .child(
                div()
                    .w(rems(16.))
                    .child(Input::new(&self.find_input).small()),
            )
            .child(
                div()
                    .id("pdf-find-count")
                    .test_support()
                    .aria_label(count.clone())
                    .flex_none()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(count),
            )
            .child(div().flex_1())
            .child(
                Button::new("pdf-find-previous")
                    .ghost()
                    .xsmall()
                    .flex_none()
                    .disabled(self.matches.is_empty())
                    .label("‹")
                    .accessibility_label(t(L10nKey::AppMenuFindPrevious))
                    .tooltip(t(L10nKey::AppMenuFindPrevious))
                    .on_click(cx.listener(|this, _, _, cx| this.previous_match(cx))),
            )
            .child(
                Button::new("pdf-find-next")
                    .ghost()
                    .xsmall()
                    .flex_none()
                    .disabled(self.matches.is_empty())
                    .label("›")
                    .accessibility_label(t(L10nKey::AppMenuFindNext))
                    .tooltip(t(L10nKey::AppMenuFindNext))
                    .on_click(cx.listener(|this, _, _, cx| this.next_match(cx))),
            )
            .child(
                Button::new("pdf-find-close")
                    .ghost()
                    .xsmall()
                    .flex_none()
                    .label("✕")
                    .accessibility_label(t(L10nKey::Close))
                    .tooltip(t(L10nKey::Close))
                    .on_click(cx.listener(|this, _, window, cx| this.close_find(window, cx))),
            )
            .into_any_element()
    }

    /// 大纲面板：缩进的书签列表，点击或键盘 Enter 跳到各自目的地。
    /// 没有目的地的条目灰显且不可点，不假装能导航。
    fn render_toc(&self, document: &lyrel_pdf::PdfDocument, cx: &mut Context<Self>) -> AnyElement {
        let mut column = div()
            .id("pdf-toc")
            .test_support()
            .flex_none()
            .w(rems(17.))
            .h_full()
            .overflow_y_scroll()
            .border_r_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().sidebar)
            .py_1();
        for (index, item) in document.outline().iter().enumerate() {
            let destination = item.destination;
            let active = self.toc_index == index;
            let title = item.title.clone();
            let mut row = div()
                .id(SharedString::from(format!("pdf-toc-{index}")))
                .test_support()
                .aria_label(title.clone())
                .pl(px(10. + item.level as f32 * 14.))
                .pr_2()
                .py_1()
                .text_xs()
                .text_color(if destination.is_some() {
                    cx.theme().foreground
                } else {
                    cx.theme().muted_foreground
                })
                .rounded_md()
                .child(title);
            if active {
                row = row.bg(cx.theme().accent);
            }
            if destination.is_some() {
                row = row
                    .cursor_pointer()
                    .hover(|row| row.bg(cx.theme().accent))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.toc_index = index;
                        this.focus.focus(window, cx);
                        if let Some(destination) = destination {
                            this.go_to_destination(&destination, cx);
                        }
                    }));
            }
            column = column.child(row);
        }
        column.into_any_element()
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
        let ready_document = content.ready().cloned();
        let body = match content {
            PdfContent::Ready(document) => {
                let pane = v_flex().size_full().min_h_0().child(
                    h_flex()
                        .flex_1()
                        .min_h_0()
                        .when_some(
                            (self.toc_open
                                && ready_document
                                    .as_ref()
                                    .is_some_and(|document| !document.outline().is_empty()))
                            .then_some(document.clone()),
                            |row, document| row.child(self.render_toc(&document, cx)),
                        )
                        .child(self.render_ready(document, cx)),
                );
                pane.into_any_element()
            }
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
        // 查找条渲染在文档按键上下文之外：输入框里的数字/连字符必须成为
        // 查询文字，而不是被 PDF 的缩放快捷键吞掉；同时点击输入框不会被
        // 文档的 on_mouse_down 抢走焦点。
        let find_bar =
            (self.find_open && ready_document.is_some()).then(|| self.render_find_bar(cx));
        v_flex()
            .id(("pdf-document", cx.entity_id()))
            .size_full()
            .min_w_0()
            .min_h_0()
            .border_1()
            .border_color(ring)
            .child(self.toolbar(cx))
            .when_some(find_bar, |this, bar| this.child(bar))
            .child(
                v_flex()
                    .key_context(KEY_CONTEXT)
                    .track_focus(&self.focus)
                    .on_action(cx.listener(|this, _: &PdfZoomIn, _, cx| this.zoom_in(cx)))
                    .on_action(cx.listener(|this, _: &PdfZoomOut, _, cx| this.zoom_out(cx)))
                    .on_action(cx.listener(|this, _: &PdfFitWidth, _, cx| this.fit_width(cx)))
                    .on_action(cx.listener(|this, _: &PdfFitPage, _, cx| this.fit_page(cx)))
                    .on_action(cx.listener(|this, _: &PdfActualSize, _, cx| this.actual_size(cx)))
                    .on_action(cx.listener(|this, _: &PdfNextPage, _, cx| this.next_page(cx)))
                    .on_action(
                        cx.listener(|this, _: &PdfPreviousPage, _, cx| this.previous_page(cx)),
                    )
                    .on_action(cx.listener(|this, _: &PdfCopy, _, cx| this.copy_selection(cx)))
                    .on_action(cx.listener(
                        |this, _: &gpui_kit::component::input::Search, window, cx| {
                            this.open_find(window, cx)
                        },
                    ))
                    .on_action(
                        cx.listener(|this, _: &crate::terminal::view::FindNext, _, cx| {
                            this.next_match(cx)
                        }),
                    )
                    .on_action(cx.listener(
                        |this, _: &crate::terminal::view::FindPrevious, _, cx| {
                            this.previous_match(cx)
                        },
                    ))
                    .on_key_down(
                        cx.listener(|this, event: &gpui_kit::KeyDownEvent, window, cx| {
                            // 只有文档视图本身持有焦点时才吃翻页/大纲/退出键；查找输入
                            // 框内这些键（包括 Enter/Escape）走输入框自己的语义。
                            if !this.focus.is_focused(window) {
                                return;
                            }
                            match event.keystroke.key.as_str() {
                                "left" | "pageup" => this.previous_page(cx),
                                "right" | "pagedown" => this.next_page(cx),
                                "up" if this.toc_open => this.toc_step(false, cx),
                                "down" if this.toc_open => this.toc_step(true, cx),
                                "enter" if this.toc_open => this.activate_outline(cx),
                                "escape" if this.toc_open && !this.find_open => {
                                    this.toc_open = false
                                }
                                "escape" => this.close_find(window, cx),
                                _ => return,
                            }
                            cx.stop_propagation();
                        }),
                    )
                    .on_mouse_down(
                        gpui_kit::MouseButton::Left,
                        cx.listener(|this, _, window, cx| this.focus.focus(window, cx)),
                    )
                    .on_mouse_up_out(
                        gpui_kit::MouseButton::Left,
                        cx.listener(|this, event: &gpui_kit::MouseUpEvent, _, cx| {
                            this.end_drag(Some(event.position), cx)
                        }),
                    )
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .child(body),
            )
    }
}

/// 右侧菜单里的取景命令，与快捷键、工具栏按钮是同一组动作。复制项只在
/// 真有选中文字时可用，灰色项仍占住位置，菜单形状保持稳定。
pub(super) fn menu_entries(
    menu: gpui_kit::component::menu::PopupMenu,
    has_selection: bool,
) -> gpui_kit::component::menu::PopupMenu {
    menu.separator()
        .menu(t(L10nKey::EditorPdfPreviousPage), Box::new(PdfPreviousPage))
        .menu(t(L10nKey::EditorPdfNextPage), Box::new(PdfNextPage))
        .menu(
            t(L10nKey::AppMenuFind),
            Box::new(gpui_kit::component::input::Search),
        )
        .menu_with_disabled(t(L10nKey::AppMenuCopy), Box::new(PdfCopy), !has_selection)
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
