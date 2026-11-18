//! SVG 文档：一份逐行保真的源码权威，加上与它同步的安全原生预览。
//!
//! 源码不是 Markdown：它用共享的 [`lyrel_markdown_editor::Document`] 保存
//! 每行原始行尾、编码/BOM 与 history，呈现层只是一个 plain-source
//! `EditorState`。预览不是第二份文档：它只按同一 Document 的 revision
//! 生成位图，旧结果、其他文档或更新 revision 的结果都不会被安装。
//!
//! 渲染沿用 `markdown::images` 的 usvg/resvg 安全选项与纹理租约；这里不引入
//! 浏览器文档、脚本运行时、临时文件或第二套净化管线。

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, h_flex, spinner::Spinner,
    v_flex,
};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, App, Context, Entity, FocusHandle, Focusable, Pixels, Point, Render, ScrollHandle,
    SharedString, Size, Subscription, TestSupportExt as _, Window, div, px, rems,
};
use lyrel_markdown_editor::{Document, DocumentEvent, EditorState as SourceEditor};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use super::image_document::{ImageActualSize, ImageFit, ImageZoomIn, ImageZoomOut};
use super::markdown::images::{self, Bitmap};
use crate::ui::i18n::{L10nKey, t, t_fmt};

/// 仅在预览持有键盘时成立，所以源码编辑和终端的键不会被取景命令截走。
pub(crate) const KEY_CONTEXT: &str = "SvgDocument";

/// 输入后短暂合并多次修改，不把每个按键都排成一次光栅化。
const RENDER_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(120);

const ZOOM_MIN: f32 = 0.02;
const ZOOM_MAX: f32 = 32.;
const ZOOM_STEP: f32 = 1.25;
/// 一次方向键推动内容多少像素。
const PAN_STEP: f32 = 48.;

/// 一个分组画的是源码还是预览。分屏时两个分组默认各占一面。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SvgMode {
    Source,
    Preview,
}

impl SvgMode {
    pub(crate) fn opposite(self) -> Self {
        match self {
            Self::Source => Self::Preview,
            Self::Preview => Self::Source,
        }
    }
}

/// 已安装的预览：位图租约与源文固有尺寸（CSS 像素）。
#[derive(Clone)]
pub(crate) struct SvgRendered {
    pub(crate) bitmap: Bitmap,
    pub(crate) width: f32,
    pub(crate) height: f32,
}

/// 预览当前能画的东西。源码始终可编辑，失败只占预览的位置。
#[derive(Clone)]
pub(crate) enum SvgPreview {
    Ready(SvgRendered),
    Failed(SharedString),
}

/// 一份 SVG 文件的内容：共享 Document 与它唯一的预览。
///
/// 预览属于文件，不属于分组：分屏的两个 view 看的是同一 revision 的同一
/// 位图；缩放、滚动和平移才属于各自的 view。
pub(crate) struct SvgDocument {
    document: Entity<Document>,
    preview: Option<SvgPreview>,
    rendering: bool,
    /// 最近一次真正安装的 revision；测试与状态行都读它。
    rendered_revision: Option<u64>,
    generation: u64,
    parse_running: bool,
    cancel: Arc<AtomicBool>,
    _render: Option<gpui_kit::Task<()>>,
    _document: Subscription,
}

impl Drop for SvgDocument {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
    }
}

impl SvgDocument {
    pub(super) fn new(document: Entity<Document>, cx: &mut Context<Self>) -> Self {
        let watch = cx.subscribe(&document, |this, _, _: &DocumentEvent, cx| {
            this.invalidate(cx);
        });
        let mut this = Self {
            document,
            preview: None,
            rendering: false,
            rendered_revision: None,
            generation: 0,
            parse_running: false,
            cancel: Arc::default(),
            _render: None,
            _document: watch,
        };
        this.invalidate(cx);
        this
    }

    pub(crate) fn document(&self) -> &Entity<Document> {
        &self.document
    }

    pub(crate) fn preview(&self) -> Option<&SvgPreview> {
        self.preview.as_ref()
    }

    pub(crate) fn is_rendering(&self) -> bool {
        self.rendering
    }

    #[cfg(test)]
    pub(crate) fn rendered_revision(&self) -> Option<u64> {
        self.rendered_revision
    }

    fn invalidate(&mut self, cx: &mut Context<Self>) {
        self.cancel.store(true, Ordering::Release);
        self.generation = self.generation.wrapping_add(1);
        self.rendering = true;
        if !self.parse_running {
            self.schedule(cx);
        }
        cx.notify();
    }

    /// 合并等待中的修订；每份文档最多一个在途解析，完成后只处理最新修订。
    /// debounce 后才取得共享快照，不在每次输入时复制全文。
    fn schedule(&mut self, cx: &mut Context<Self>) {
        let revision = self.document.read(cx).revision();
        let identity = self.document.entity_id();
        let generation = self.generation;
        self._render = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(RENDER_DEBOUNCE).await;
            let snapshot = this
                .update(cx, |this, cx| {
                    let document = this.document.read(cx);
                    if this.generation != generation || document.revision() != revision {
                        return None;
                    }
                    let snapshot = document.snapshot();
                    this.parse_running = true;
                    this.cancel.store(false, Ordering::Release);
                    Some((snapshot, this.cancel.clone()))
                })
                .ok()
                .flatten();
            let Some((snapshot, cancel)) = snapshot else {
                return;
            };
            #[cfg(test)]
            let gate = test_support::take_next_render();
            let result = crate::ui::host_ops::off_thread(move || {
                if cancel.load(Ordering::Acquire) {
                    return None;
                }
                #[cfg(test)]
                if let Some(gate) = gate {
                    gate.block();
                }
                Some(images::decode_svg_source(snapshot.text()))
            })
            .await
            .flatten()
            .unwrap_or_else(|| Err(std::io::Error::other(t(L10nKey::EditorSvgPreviewFailed))));
            let _ = this.update(cx, |this, cx| {
                this.parse_running = false;
                this._render = None;
                this.install(identity, revision, generation, result, cx);
            });
        }));
    }

    fn install(
        &mut self,
        identity: gpui_kit::EntityId,
        revision: u64,
        generation: u64,
        result: std::io::Result<images::SvgRaster>,
        cx: &mut Context<Self>,
    ) {
        if self.document.entity_id() != identity
            || self.document.read(cx).revision() != revision
            || self.generation != generation
        {
            // A late result describes a source the document no longer holds.
            self.schedule(cx);
            return;
        }
        self.rendering = false;
        self.rendered_revision = Some(revision);
        self.preview = Some(match result {
            Ok(raster) => SvgPreview::Ready(SvgRendered {
                bitmap: images::bitmap(raster.rgba, cx),
                width: raster.width,
                height: raster.height,
            }),
            Err(error) => SvgPreview::Failed(SharedString::from(error.to_string())),
        });
        cx.notify();
    }
}

/// 一份 SVG 文件在一个分组里的呈现：模式、源码选择、预览取景与键盘。
pub(crate) struct SvgView {
    document: Entity<SvgDocument>,
    source: Entity<SourceEditor>,
    mode: SvgMode,
    focus: FocusHandle,
    /// 只看预览：源码滚动由 SourceEditor 自己持有。
    scroll: ScrollHandle,
    /// `None` 表示适应窗口；`Some` 是固定比例。
    zoom: Option<f32>,
    viewport: Option<Size<Pixels>>,
    /// 上一帧实际画出的比例，状态行读它，显示的就是眼前的样子。
    drawn: f32,
    _document: Subscription,
    _source: Subscription,
}

impl SvgView {
    pub(super) fn new(
        document: Entity<SvgDocument>,
        mode: SvgMode,
        wrap: bool,
        indent: usize,
        cx: &mut Context<Self>,
    ) -> Self {
        let document_for_source = document.clone();
        let source = cx.new(move |cx| {
            let mut source =
                SourceEditor::for_document(document_for_source.read(cx).document().clone(), cx);
            source.set_plain_source(true, cx);
            source.set_soft_wrap(wrap, cx);
            source.set_tab_indent(indent);
            source
        });
        let watch_document = cx.observe(&document, |_, _, cx| cx.notify());
        let watch_source = cx.observe(&source, |_, _, cx| cx.notify());
        Self {
            document,
            source,
            mode,
            focus: cx.focus_handle(),
            scroll: ScrollHandle::new(),
            zoom: None,
            viewport: None,
            drawn: 1.,
            _document: watch_document,
            _source: watch_source,
        }
    }

    pub(crate) fn document(&self) -> &Entity<SvgDocument> {
        &self.document
    }

    pub(crate) fn source(&self) -> &Entity<SourceEditor> {
        &self.source
    }

    pub(crate) fn mode(&self) -> SvgMode {
        self.mode
    }

    pub(crate) fn focus_handle(&self, cx: &App) -> FocusHandle {
        match self.mode {
            SvgMode::Source => self.source.read(cx).focus_handle(cx),
            SvgMode::Preview => self.focus.clone(),
        }
    }

    pub(crate) fn scroll_offset(&self, cx: &App) -> Point<Pixels> {
        match self.mode {
            SvgMode::Source => self.source.read(cx).scroll_offset(),
            SvgMode::Preview => self.scroll.offset(),
        }
    }

    pub(crate) fn set_scroll_offset(&mut self, offset: Point<Pixels>, cx: &mut Context<Self>) {
        match self.mode {
            SvgMode::Source => self
                .source
                .update(cx, |source, cx| source.set_scroll_offset(offset, cx)),
            SvgMode::Preview => {
                self.scroll.set_offset(offset);
                cx.notify();
            }
        }
    }

    /// The source editor's scroll, independent of which side is showing.
    pub(crate) fn source_scroll_offset(&self, cx: &App) -> Point<Pixels> {
        self.source.read(cx).scroll_offset()
    }

    pub(crate) fn preview_scroll_offset(&self) -> Point<Pixels> {
        self.scroll.offset()
    }

    pub(crate) fn selected_range(&self, cx: &App) -> std::ops::Range<usize> {
        self.source.read(cx).selected_range()
    }

    /// Text operations on this presentation always target the source; using
    /// one while the preview is visible brings the source back instead of
    /// editing an invisible buffer.
    pub(crate) fn select_source_range(
        &mut self,
        range: std::ops::Range<usize>,
        cx: &mut Context<Self>,
    ) {
        self.show_source(cx);
        self.source
            .update(cx, |source, cx| source.select_range(range, cx));
    }

    pub(crate) fn add_source_selection(
        &mut self,
        range: std::ops::Range<usize>,
        cx: &mut Context<Self>,
    ) {
        self.show_source(cx);
        self.source
            .update(cx, |source, cx| source.add_selection(range, cx));
    }

    pub(crate) fn replace_source_range(
        &mut self,
        range: std::ops::Range<usize>,
        text: &str,
        cx: &mut Context<Self>,
    ) {
        self.show_source(cx);
        self.source
            .update(cx, |source, cx| source.replace_range(range, text, cx));
    }

    pub(crate) fn replace_source_ranges(
        &mut self,
        replacements: Vec<(std::ops::Range<usize>, String)>,
        cx: &mut Context<Self>,
    ) {
        self.show_source(cx);
        let source = self.source.clone();
        source.update(cx, |source, cx| source.replace_ranges(replacements, cx));
    }

    pub(crate) fn replace_source_lines(
        &mut self,
        range: std::ops::Range<usize>,
        lines: Vec<std::ops::Range<usize>>,
        leading: bool,
        trailing: bool,
        cx: &mut Context<Self>,
    ) {
        self.show_source(cx);
        let source = self.source.clone();
        source.update(cx, |source, cx| {
            source.replace_lines(range, lines, leading, trailing, cx)
        });
    }

    pub(crate) fn set_source_history_selection(
        &mut self,
        before: std::ops::Range<usize>,
        after: std::ops::Range<usize>,
        cx: &mut Context<Self>,
    ) {
        let source = self.source.clone();
        source.update(cx, |source, cx| {
            source.set_history_selection(before, after, cx)
        });
    }

    /// Copies the origin group's source caret/scroll and preview framing into
    /// a split view without changing which side that new view shows.
    pub(crate) fn seed_presentation(
        &mut self,
        selection: std::ops::Range<usize>,
        source_scroll: Point<Pixels>,
        preview_scroll: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let source = self.source.clone();
        source.update(cx, |source, cx| {
            source.select_range(selection, cx);
            source.set_scroll_offset(source_scroll, cx);
        });
        self.scroll.set_offset(preview_scroll);
    }

    pub(crate) fn finish_composition(&mut self, cx: &mut Context<Self>) {
        self.source
            .update(cx, |source, cx| source.finish_owned_composition(cx));
    }

    pub(crate) fn set_soft_wrap(&mut self, wrap: bool, cx: &mut Context<Self>) {
        self.source
            .update(cx, |source, cx| source.set_soft_wrap(wrap, cx));
    }

    pub(crate) fn set_format(
        &mut self,
        format: lyrel_document::text::TextFormat,
        cx: &mut Context<Self>,
    ) {
        self.source
            .update(cx, |source, cx| source.set_format(format, cx));
    }

    pub(crate) fn show_source(&mut self, cx: &mut Context<Self>) {
        if self.mode != SvgMode::Source {
            self.mode = SvgMode::Source;
            cx.notify();
        }
    }

    /// Flip source and preview. Switching away from source commits an
    /// in-flight composition through the document's ordinary history first.
    pub(crate) fn toggle_mode(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.finish_composition(cx);
        self.mode = self.mode.opposite();
        let focus = self.focus_handle(cx);
        focus.focus(window, cx);
        cx.notify();
    }

    pub(crate) fn zoom_label(&self) -> SharedString {
        format!("{:.0}%", self.drawn * 100.).into()
    }

    pub(crate) fn preview_size(&self, cx: &App) -> Option<(f32, f32)> {
        match self.document.read(cx).preview() {
            Some(SvgPreview::Ready(rendered)) => Some((rendered.width, rendered.height)),
            _ => None,
        }
    }

    pub(crate) fn is_rendering(&self, cx: &App) -> bool {
        self.document.read(cx).is_rendering()
    }

    pub(crate) fn preview_failure(&self, cx: &App) -> Option<SharedString> {
        match self.document.read(cx).preview() {
            Some(SvgPreview::Failed(message)) => Some(message.clone()),
            _ => None,
        }
    }

    pub(crate) fn zoom_in(&mut self, cx: &mut Context<Self>) {
        let base = self.zoom.unwrap_or(self.drawn).max(ZOOM_MIN);
        self.set_zoom(base * ZOOM_STEP, cx);
    }

    pub(crate) fn zoom_out(&mut self, cx: &mut Context<Self>) {
        let base = self.zoom.unwrap_or(self.drawn).max(ZOOM_MIN);
        self.set_zoom(base / ZOOM_STEP, cx);
    }

    pub(crate) fn fit(&mut self, cx: &mut Context<Self>) {
        if self.zoom.take().is_some() {
            cx.notify();
        }
    }

    pub(crate) fn actual_size(&mut self, cx: &mut Context<Self>) {
        self.set_zoom(1., cx);
    }

    fn set_zoom(&mut self, scale: f32, cx: &mut Context<Self>) {
        let scale = scale.clamp(ZOOM_MIN, ZOOM_MAX);
        if self.zoom != Some(scale) {
            self.zoom = Some(scale);
            cx.notify();
        }
    }

    /// 内容按指针推动的方向走：拖动向右走向左的边缘，gpui 把它记成向零靠近。
    pub(crate) fn pan(&mut self, dx: Pixels, dy: Pixels, cx: &mut Context<Self>) {
        let max = self.scroll.max_offset();
        let offset = self.scroll.offset();
        let next = Point {
            x: (offset.x - dx).clamp(-max.x, px(0.)),
            y: (offset.y - dy).clamp(-max.y, px(0.)),
        };
        if next != offset {
            self.scroll.set_offset(next);
            cx.notify();
        }
    }

    /// 画布量出来的大小。适应窗口要它，所以只在真的变了时重画。
    pub(super) fn set_viewport(&mut self, size: Size<Pixels>, cx: &mut Context<Self>) {
        let changed = self.viewport.is_none_or(|old| {
            f32::from(old.width - size.width).abs() > 0.5
                || f32::from(old.height - size.height).abs() > 0.5
        });
        if !changed {
            return;
        }
        self.viewport = Some(size);
        if self.zoom.is_none() {
            cx.notify();
        }
    }

    fn scale_for(&self, image: Size<Pixels>) -> f32 {
        match self.zoom {
            Some(scale) => scale,
            None => {
                let Some(viewport) = self.viewport else {
                    return 1.;
                };
                let width = f32::from(image.width).max(1.);
                let height = f32::from(image.height).max(1.);
                let fit =
                    (f32::from(viewport.width) / width).min(f32::from(viewport.height) / height);
                // 用户缩放下限不适用 fit:窄视图也要能把整张图画进去。
                fit.clamp(f32::EPSILON, 1.)
            }
        }
    }

    /// 状态行与工具栏共用的取景按钮：适应窗口、实际大小。
    fn framing_controls(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let ready = matches!(self.document.read(cx).preview(), Some(SvgPreview::Ready(_)));
        h_flex()
            .flex_none()
            .items_center()
            .gap_1()
            .child(
                Button::new("svg-fit")
                    .ghost()
                    .xsmall()
                    .flex_none()
                    .disabled(!ready)
                    .label(t(L10nKey::EditorImageFit))
                    .accessibility_label(t(L10nKey::EditorImageFitAction))
                    .tooltip(t(L10nKey::EditorImageFitAction))
                    .on_click(cx.listener(|this, _, _, cx| this.fit(cx))),
            )
            .child(
                Button::new("svg-actual")
                    .ghost()
                    .xsmall()
                    .flex_none()
                    .disabled(!ready)
                    .label(t(L10nKey::EditorImageActualSize))
                    .accessibility_label(t(L10nKey::EditorImageActualSizeAction))
                    .tooltip(t(L10nKey::EditorImageActualSizeAction))
                    .on_click(cx.listener(|this, _, _, cx| this.actual_size(cx))),
            )
    }

    fn toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let entity = cx.entity_id();
        let (ready, rendering, failure) = {
            let document = self.document.read(cx);
            let ready = match document.preview() {
                Some(SvgPreview::Ready(rendered)) => Some((rendered.width, rendered.height)),
                _ => None,
            };
            let failure = match document.preview() {
                Some(SvgPreview::Failed(message)) => Some(message.clone()),
                _ => None,
            };
            (ready, document.is_rendering(), failure)
        };
        let preview_mode = self.mode == SvgMode::Preview;
        let (toggle_label, toggle_tip) = match preview_mode {
            true => (
                t(L10nKey::EditorMarkdownSource),
                t(L10nKey::EditorSvgShowSource),
            ),
            false => (
                t(L10nKey::EditorSvgPreview),
                t(L10nKey::EditorSvgShowPreview),
            ),
        };
        let border = cx.theme().border;
        let muted = cx.theme().muted_foreground;
        let warning = cx.theme().warning;
        let percent = self.zoom_label();

        h_flex()
            .id(("svg-toolbar", entity))
            .flex_none()
            .w_full()
            .items_center()
            .gap_2()
            .px_2()
            .py_1()
            .border_b_1()
            .border_color(border)
            .child(div().flex_none().text_xs().text_color(muted).child("SVG"))
            .when_some(ready, |row, (width, height)| {
                row.child(div().flex_none().text_xs().text_color(muted).child(t_fmt(
                    L10nKey::EditorImagePixelSize,
                    &[
                        ("width", &format!("{width:.0}")),
                        ("height", &format!("{height:.0}")),
                    ],
                )))
            })
            .child(div().flex_1())
            .when(rendering, |row| {
                row.child(
                    h_flex()
                        .id(("svg-preview-loading", entity))
                        .flex_none()
                        .items_center()
                        .gap_1()
                        .text_xs()
                        .text_color(muted)
                        .child(Spinner::new().xsmall())
                        .child(t(L10nKey::EditorSvgRendering)),
                )
            })
            .when_some(failure, |row, message| {
                row.child(
                    div()
                        .id(("svg-preview-failed", entity))
                        .test_support()
                        .flex_none()
                        .text_xs()
                        .text_color(warning)
                        .child(t(L10nKey::EditorSvgPreviewFailed))
                        .tooltip(move |window, cx| {
                            gpui_kit::component::tooltip::Tooltip::new(message.clone())
                                .build(window, cx)
                        }),
                )
            })
            .when(preview_mode && ready.is_some(), |row| {
                row.child(div().flex_none().text_xs().text_color(muted).child(percent))
                    .child(self.framing_controls(cx))
            })
            .child(
                Button::new("svg-mode-toggle")
                    .ghost()
                    .xsmall()
                    .flex_none()
                    .label(toggle_label)
                    .tooltip(toggle_tip)
                    .on_click(cx.listener(|this, _, window, cx| this.toggle_mode(window, cx))),
            )
    }

    fn preview_body(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let rendered = match self.document.read(cx).preview() {
            Some(SvgPreview::Ready(rendered)) => rendered.clone(),
            Some(SvgPreview::Failed(message)) => {
                let message = message.clone();
                return failed_preview(message, cx.entity_id(), cx).into_any_element();
            }
            None => return loading_preview(cx.entity_id(), cx).into_any_element(),
        };
        // SVG 用户单位是 CSS 像素：位图按其固有尺寸绘制，不按位图像素换算。
        let natural = Size {
            width: px(rendered.width),
            height: px(rendered.height),
        };
        let scale = self.scale_for(natural);
        let ring = if self.focus.contains_focused(window, cx) {
            cx.theme().primary
        } else {
            gpui_kit::transparent_black()
        };
        div()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .border_1()
            .border_color(ring)
            .on_children_prepainted({
                let view = cx.entity();
                move |bounds, _, cx| {
                    if let Some(bounds) = bounds.first() {
                        view.update(cx, |view, cx| view.set_viewport(bounds.size, cx));
                    }
                }
            })
            .child(
                div()
                    .id(("svg-preview", cx.entity_id()))
                    .test_support()
                    .size_full()
                    .overflow_scroll()
                    .track_scroll(&self.scroll)
                    .child(
                        // 可滚动的是这个内容盒:它拿到源文的固有尺寸,滚动
                        // 上限才随放大增长;fit 时 min 仍填满画布以便居中。
                        h_flex()
                            .min_w_full()
                            .min_h_full()
                            .w(natural.width * scale)
                            .h(natural.height * scale)
                            .items_center()
                            .justify_center()
                            .child(
                                gpui_kit::img(rendered.bitmap.image())
                                    .id(("svg-preview-pixels", cx.entity_id()))
                                    .test_support()
                                    .flex_none()
                                    .w(natural.width * scale)
                                    .h(natural.height * scale),
                            ),
                    ),
            )
            .into_any_element()
    }
}

fn failed_preview(
    message: SharedString,
    entity: gpui_kit::EntityId,
    cx: &mut Context<SvgView>,
) -> impl IntoElement {
    let (foreground, muted, warning) = {
        let theme = cx.theme();
        (theme.foreground, theme.muted_foreground, theme.warning)
    };
    v_flex()
        .id(("svg-preview-error", entity))
        .test_support()
        .flex_1()
        .min_w_0()
        .min_h_0()
        .items_center()
        .justify_center()
        .gap_2()
        .px_4()
        .child(
            Icon::new(IconName::TriangleAlert)
                .large()
                .text_color(warning),
        )
        .child(
            div()
                .text_sm()
                .text_color(foreground)
                .child(t(L10nKey::EditorSvgPreviewFailed)),
        )
        .child(
            div()
                .max_w(rems(30.))
                .text_xs()
                .text_color(muted)
                .child(message),
        )
}

fn loading_preview(entity: gpui_kit::EntityId, cx: &mut Context<SvgView>) -> impl IntoElement {
    let muted = cx.theme().muted_foreground;
    v_flex()
        .id(("svg-preview-loading-body", entity))
        .flex_1()
        .min_w_0()
        .min_h_0()
        .items_center()
        .justify_center()
        .gap_2()
        .text_xs()
        .text_color(muted)
        .child(Spinner::new().small())
        .child(t(L10nKey::EditorSvgRendering))
}

/// 源码面的滚动/视口测量与 Markdown 的 source 呈现相同：编辑器自己持有
/// 滚动，外层只告诉它画布有多大。
fn source_body(source: &Entity<SourceEditor>) -> AnyElement {
    let source = source.clone();
    let measure = source.clone();
    div()
        .on_children_prepainted(move |bounds, _, cx| {
            if let Some(bounds) = bounds.first() {
                measure.update(cx, |source, cx| source.set_scroll_viewport(*bounds, cx));
            }
        })
        .flex_1()
        .min_w_0()
        .min_h_0()
        .child(v_flex().size_full().min_h_0().child(source))
        .into_any_element()
}

impl Focusable for SvgView {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        match self.mode {
            SvgMode::Source => self.source.read(cx).focus_handle(cx),
            SvgMode::Preview => self.focus.clone(),
        }
    }
}

impl Render for SvgView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // 预览的比例只有画出来才算数；状态行与工具栏读的是这一帧。
        let natural = match self.document.read(cx).preview() {
            Some(SvgPreview::Ready(rendered)) => Some(Size {
                width: px(rendered.width),
                height: px(rendered.height),
            }),
            _ => None,
        };
        self.drawn = natural.map_or(1., |natural| self.scale_for(natural));
        let mode = self.mode;
        let focus = self.focus.clone();
        let item = v_flex()
            .id(("svg-document", cx.entity_id()))
            .size_full()
            .min_w_0()
            .min_h_0()
            .child(self.toolbar(cx));
        let body = match mode {
            SvgMode::Source => source_body(&self.source),
            SvgMode::Preview => self.preview_body(window, cx),
        };
        // The menu's framing commands dispatch through this view in either
        // mode; the keyboard context itself exists only over the preview, so
        // source editing keeps every key for the text.
        let item = item
            .child(body)
            .on_action(cx.listener(|this, _: &ImageZoomIn, _, cx| this.zoom_in(cx)))
            .on_action(cx.listener(|this, _: &ImageZoomOut, _, cx| this.zoom_out(cx)))
            .on_action(cx.listener(|this, _: &ImageFit, _, cx| this.fit(cx)))
            .on_action(cx.listener(|this, _: &ImageActualSize, _, cx| this.actual_size(cx)));
        match mode {
            SvgMode::Preview => item
                .key_context(KEY_CONTEXT)
                .track_focus(&focus)
                .on_key_down(
                    cx.listener(|this, event: &gpui_kit::KeyDownEvent, _window, cx| {
                        let (dx, dy) = match event.keystroke.key.as_str() {
                            "left" => (px(-PAN_STEP), px(0.)),
                            "right" => (px(PAN_STEP), px(0.)),
                            "up" => (px(0.), px(-PAN_STEP)),
                            "down" => (px(0.), px(PAN_STEP)),
                            "pageup" => (px(0.), px(-PAN_STEP * 8.)),
                            "pagedown" => (px(0.), px(PAN_STEP * 8.)),
                            _ => return,
                        };
                        this.pan(dx, dy, cx);
                        cx.stop_propagation();
                    }),
                )
                .on_mouse_down(
                    gpui_kit::MouseButton::Left,
                    cx.listener(|this, _, window, cx| {
                        let focus = this.focus.clone();
                        focus.focus(window, cx);
                    }),
                )
                .into_any_element(),
            SvgMode::Source => item.into_any_element(),
        }
    }
}

/// 右侧菜单：先给源码/预览切换，再是预览取景命令。
pub(super) fn menu_entries(
    menu: gpui_kit::component::menu::PopupMenu,
) -> gpui_kit::component::menu::PopupMenu {
    menu.separator()
        .menu(
            t(L10nKey::EditorSvgShowPreview),
            Box::new(crate::core::actions::ToggleDocumentPreview),
        )
        .menu(t(L10nKey::EditorImageZoomIn), Box::new(ImageZoomIn))
        .menu(t(L10nKey::EditorImageZoomOut), Box::new(ImageZoomOut))
        .menu(t(L10nKey::EditorImageFit), Box::new(ImageFit))
        .menu(t(L10nKey::EditorImageActualSize), Box::new(ImageActualSize))
}

/// 预览的确定性单元测试门闩，不编译进产品或独立 native 消费者。
#[cfg(test)]
pub(crate) mod test_support {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    use std::{cell::RefCell, sync::mpsc, time::Duration};

    pub(super) struct RenderGate {
        receiver: mpsc::Receiver<()>,
        entered: Arc<AtomicBool>,
    }

    thread_local! {
        static GATE: RefCell<Option<RenderGate>> = const { RefCell::new(None) };
    }

    /// Pauses the next scheduled preview render before it parses. The returned
    /// sender releases it; the flag reports that the blocking worker has
    /// arrived.
    pub(crate) fn pause_next_render() -> (mpsc::Sender<()>, Arc<AtomicBool>) {
        let (tx, rx) = mpsc::channel();
        let entered = Arc::new(AtomicBool::new(false));
        GATE.with_borrow_mut(|gate| {
            *gate = Some(RenderGate {
                receiver: rx,
                entered: entered.clone(),
            })
        });
        (tx, entered)
    }

    pub(super) fn take_next_render() -> Option<RenderGate> {
        GATE.with_borrow_mut(Option::take)
    }

    impl RenderGate {
        pub(super) fn block(self) {
            self.entered.store(true, Ordering::SeqCst);
            let _ = self.receiver.recv_timeout(Duration::from_secs(20));
        }
    }
}

#[cfg(test)]
#[path = "svg_document/tests.rs"]
mod tests;
