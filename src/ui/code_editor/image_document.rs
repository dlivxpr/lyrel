//! 只读静态图片文档。
//!
//! 一张图片是一个工作台文档，不是一个浮层预览：它按 Host 读取，有自己的
//! 标签页、状态行、fit/actual-size/zoom/pan 和可见的错误状态。解码、字节与
//! 像素预算、纹理租约都复用 `markdown::images`，这里只负责文档语义与呈现。
//!
//! 没有 provider 框架：五种静态格式是这条消费者的全部格式，GIF 动画与多页
//! TIFF 仍走外部打开，不在这里假装能逐帧或逐页显示。

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, h_flex, v_flex,
};
use gpui_kit::prelude::*;
use gpui_kit::{
    App, Context, Entity, FocusHandle, Focusable, Pixels, Point, Render, ScrollHandle,
    SharedString, Size, Subscription, TestSupportExt as _, Window, div, px, rems,
};

use super::markdown::images::{Bitmap, StaticImageFormat};
use crate::ui::i18n::{L10nKey, t, t_fmt};

gpui_kit::actions!(
    image,
    [ImageZoomIn, ImageZoomOut, ImageFit, ImageActualSize]
);

/// 仅在图片文档持有键盘时成立，所以终端的键和文本动作都不会被这里截走。
pub(crate) const KEY_CONTEXT: &str = "ImageDocument";

/// 缩放下限。再小就看不出内容，只留一个点。
const ZOOM_MIN: f32 = 0.02;
const ZOOM_MAX: f32 = 32.;
const ZOOM_STEP: f32 = 1.25;
/// 一次方向键推动内容多少像素。
const PAN_STEP: f32 = 48.;

/// 一次 Host 读取的结果，已在 UI 线程完成纹理租约；失败是给人看的字。
pub(super) type ImageOutcome = Result<(Bitmap, StaticImageFormat), SharedString>;

/// 图片文档此刻能画的东西。
///
/// 失败不是空文档：字节到了、名字也认了，但内容不是那张图，工作台仍然要为
/// 这份文件留一个说清楚的位置，下次外部变化再重新读。
#[derive(Clone)]
pub(crate) enum ImageContent {
    Ready(Bitmap),
    Failed(SharedString),
}

impl ImageContent {
    pub(crate) fn ready(&self) -> Option<&Bitmap> {
        match self {
            Self::Ready(bitmap) => Some(bitmap),
            Self::Failed(_) => None,
        }
    }

    pub(crate) fn message(&self) -> Option<&SharedString> {
        match self {
            Self::Ready(_) => None,
            Self::Failed(message) => Some(message),
        }
    }
}

/// 一份图片文件的内容：解码结果、格式与重新读取的状态。
pub(crate) struct ImageDocument {
    /// 名字声明或内容探针认出的格式；解码失败时，它是唯一还能说的格式。
    claimed: Option<StaticImageFormat>,
    format: Option<StaticImageFormat>,
    content: ImageContent,
    reloading: bool,
}

impl ImageDocument {
    pub(super) fn new(claimed: Option<StaticImageFormat>, outcome: ImageOutcome) -> Self {
        let mut document = Self {
            claimed,
            format: claimed,
            content: ImageContent::Failed(SharedString::default()),
            reloading: false,
        };
        document.set(outcome);
        document
    }

    fn set(&mut self, outcome: ImageOutcome) {
        match outcome {
            Ok((bitmap, format)) => {
                self.format = Some(format);
                self.content = ImageContent::Ready(bitmap);
            }
            Err(message) => {
                self.format = self.claimed;
                self.content = ImageContent::Failed(message);
            }
        }
        self.reloading = false;
    }

    /// 外部变化后重新读到的内容。旧图不在失败时继续冒充当前文件。
    pub(super) fn replace(&mut self, outcome: ImageOutcome, cx: &mut Context<Self>) {
        self.set(outcome);
        cx.notify();
    }

    /// 读取开始：状态行说正在读，不把上一次的图当成这一刻的文件。
    pub(super) fn begin_reload(&mut self, cx: &mut Context<Self>) {
        if !self.reloading {
            self.reloading = true;
            cx.notify();
        }
    }

    pub(crate) fn format(&self) -> Option<StaticImageFormat> {
        self.format
    }

    pub(crate) fn content(&self) -> &ImageContent {
        &self.content
    }

    pub(crate) fn reloading(&self) -> bool {
        self.reloading
    }
}

/// 一份图片文件在一个分组里的呈现：缩放、平移与它的键盘。
///
/// 缩放和平移属于呈现，不属于文件：同一张图在分屏两边可以有各自的取景，
/// 与 Markdown 的滚动位置一样。
pub(crate) struct ImageView {
    document: Entity<ImageDocument>,
    focus: FocusHandle,
    scroll: ScrollHandle,
    /// `None` 表示适应窗口；`Some` 是固定比例。
    zoom: Option<f32>,
    viewport: Option<Size<Pixels>>,
    /// 上一帧实际画出的比例，状态行读它，显示的就是眼前的样子。
    drawn: f32,
    _observe: Subscription,
}

impl ImageView {
    pub(super) fn new(document: Entity<ImageDocument>, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe(&document, |_, _, cx| cx.notify());
        Self {
            document,
            focus: cx.focus_handle(),
            scroll: ScrollHandle::new(),
            zoom: None,
            viewport: None,
            drawn: 1.,
            _observe: observe,
        }
    }

    pub(crate) fn document(&self) -> &Entity<ImageDocument> {
        &self.document
    }

    pub(crate) fn handle(&self) -> FocusHandle {
        self.focus.clone()
    }

    /// 画布的滚动状态，用于跨分组拷贝取景和外部设置滚动位置。
    pub(crate) fn scroll(&self) -> &ScrollHandle {
        &self.scroll
    }

    /// 上一帧画出的比例，和状态行显示的一致。
    #[cfg(test)]
    pub(crate) fn scale(&self) -> f32 {
        self.drawn
    }

    pub(crate) fn zoom_label(&self) -> SharedString {
        format!("{:.0}%", self.drawn * 100.).into()
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

    /// 状态行的一段：格式、像素大小，读取中就说正在读。
    pub(crate) fn status_text(&self, cx: &App) -> SharedString {
        let document = self.document.read(cx);
        match document.content() {
            ImageContent::Ready(bitmap) => {
                let size = bitmap.image().size(0);
                let mut parts = Vec::with_capacity(3);
                if let Some(format) = document.format() {
                    parts.push(format.label().to_string());
                }
                parts.push(t_fmt(
                    L10nKey::EditorImagePixelSize,
                    &[
                        ("width", &size.width.0.to_string()),
                        ("height", &size.height.0.to_string()),
                    ],
                ));
                if document.reloading() {
                    parts.push(t(L10nKey::EditorImageLoading).to_string());
                }
                parts.join(" · ").into()
            }
            ImageContent::Failed(message) => {
                format!("{} · {message}", t(L10nKey::EditorImageFailed)).into()
            }
        }
    }

    /// 状态行与工具栏共用的按钮：适应窗口、实际大小。
    fn controls(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let ready = self.document.read(cx).content().ready().is_some();
        h_flex()
            .flex_none()
            .items_center()
            .gap_1()
            .child(
                Button::new("image-fit")
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
                Button::new("image-actual")
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
}

impl Focusable for ImageView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for ImageView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Colors are copied out before the element tree borrows the context
        // again for a listener; `theme` itself holds a borrow of it.
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
        let (content, format, reloading) = {
            let document = self.document.read(cx);
            (
                document.content().clone(),
                document.format(),
                document.reloading(),
            )
        };
        let image = content.ready().map(|bitmap| bitmap.image());
        // 位图像素是文件事实:工具栏标原始像素,布局才换算成逻辑点。
        let pixels = image
            .as_ref()
            .map(|image| image.size(0))
            .map(|size| (size.width.0, size.height.0));
        let scale_factor = window.scale_factor();
        let natural = image.as_ref().map(|image| {
            let size = image.size(0);
            Size {
                width: px(size.width.0 as f32 / scale_factor),
                height: px(size.height.0 as f32 / scale_factor),
            }
        });
        let scale = natural.map_or(1., |natural| self.scale_for(natural));
        self.drawn = scale;
        let percent = format!("{:.0}%", scale * 100.);
        let ring = if focused {
            primary
        } else {
            gpui_kit::transparent_black()
        };

        let toolbar = h_flex()
            .flex_none()
            .w_full()
            .items_center()
            .gap_2()
            .px_2()
            .py_1()
            .border_b_1()
            .border_color(border)
            .when_some(format, |row, format| {
                row.child(
                    div()
                        .flex_none()
                        .text_xs()
                        .text_color(muted)
                        .child(format.label()),
                )
            })
            .when_some(pixels, |row, (width, height)| {
                row.child(div().flex_none().text_xs().text_color(muted).child(t_fmt(
                    L10nKey::EditorImagePixelSize,
                    &[
                        ("width", &width.to_string()),
                        ("height", &height.to_string()),
                    ],
                )))
            })
            .child(div().flex_1())
            .when(reloading, |row| {
                row.child(
                    div()
                        .flex_none()
                        .text_xs()
                        .text_color(muted)
                        .child(t(L10nKey::EditorImageLoading)),
                )
            })
            .when(image.is_some(), |row| {
                row.child(div().flex_none().text_xs().text_color(muted).child(percent))
                    .child(self.controls(cx))
            });

        let body = match (image, natural) {
            (Some(image), Some(natural)) => div()
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
                        .id(("image-document-canvas", cx.entity_id()))
                        .size_full()
                        .overflow_scroll()
                        .track_scroll(&self.scroll)
                        .child(
                            // 可滚动的是这个内容盒:它拿到图片的真实尺寸,滚动
                            // 上限才随放大增长;fit 时 min 仍填满画布以便居中。
                            h_flex()
                                .min_w_full()
                                .min_h_full()
                                .w(natural.width * scale)
                                .h(natural.height * scale)
                                .items_center()
                                .justify_center()
                                .child(gpui_kit::img(image).size_full()),
                        ),
                )
                .into_any_element(),
            _ => {
                let message = content.message().cloned().unwrap_or_default();
                v_flex()
                    .id("status-image-failed")
                    .test_support()
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .items_center()
                    .justify_center()
                    .gap_2()
                    .px_4()
                    .border_1()
                    .border_color(ring)
                    .child(
                        Icon::new(IconName::TriangleAlert)
                            .large()
                            .text_color(warning),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(foreground)
                            .child(t(L10nKey::EditorImageFailed)),
                    )
                    .child(
                        div()
                            .max_w(rems(30.))
                            .text_xs()
                            .text_color(muted)
                            .child(message),
                    )
                    .into_any_element()
            }
        };

        v_flex()
            .id(("image-document", cx.entity_id()))
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &ImageZoomIn, _, cx| this.zoom_in(cx)))
            .on_action(cx.listener(|this, _: &ImageZoomOut, _, cx| this.zoom_out(cx)))
            .on_action(cx.listener(|this, _: &ImageFit, _, cx| this.fit(cx)))
            .on_action(cx.listener(|this, _: &ImageActualSize, _, cx| this.actual_size(cx)))
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
                cx.listener(|this, _, window, cx| this.focus.focus(window, cx)),
            )
            .size_full()
            .min_w_0()
            .min_h_0()
            .child(toolbar)
            .child(body)
    }
}

/// 右侧菜单里的取景命令，和快捷键、工具栏按钮是同一组动作。
pub(super) fn menu_entries(
    menu: gpui_kit::component::menu::PopupMenu,
) -> gpui_kit::component::menu::PopupMenu {
    menu.separator()
        .menu(t(L10nKey::EditorImageZoomIn), Box::new(ImageZoomIn))
        .menu(t(L10nKey::EditorImageZoomOut), Box::new(ImageZoomOut))
        .menu(t(L10nKey::EditorImageFit), Box::new(ImageFit))
        .menu(t(L10nKey::EditorImageActualSize), Box::new(ImageActualSize))
}

#[cfg(test)]
#[path = "image_document/tests.rs"]
mod tests;
