//! 只读图片文档：静态位图、GIF 动画与多页 TIFF。
//!
//! 一张图片是一个工作台文档，不是一个浮层预览：它按所属 Host 读取，有自己的
//! 标签页、状态行、fit/actual-size/zoom/pan 和可见的错误状态。解码、字节与
//! 像素预算、纹理租约都复用 `markdown::images`，这里只负责文档语义与呈现。
//!
//! 三种内容共用同一份呈现与取景：
//!
//! - 静态位图（PNG/JPEG/WebP/BMP/ICO）由 `decode_static` 一次解码；
//! - GIF 由移植的 `GifPlayback` 逐帧合成，可见时才在后台推进；
//! - 多页 TIFF 只枚举页表，按 presentation 的页请求在后台解码。
//!
//! 页选择与取景属于 presentation；解码结果、动画状态与页缓存属于文档。
//! 一个分屏的翻页不会改写另一个分屏正在看的页。

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, h_flex, v_flex,
};
use gpui_kit::prelude::*;
use gpui_kit::{
    App, Context, Entity, FocusHandle, Focusable, Pixels, Point, Render, ScrollHandle,
    SharedString, Size, Subscription, Task, TestSupportExt as _, Window, div, px, rems,
};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::markdown::images::{self, Bitmap, ImageFormat};
use crate::ui::i18n::{L10nKey, t, t_fmt};
use lyrel_document::image::{GifError, GifPlayback, TiffError, TiffPageError, TiffPages};

gpui_kit::actions!(
    image,
    [
        ImageZoomIn,
        ImageZoomOut,
        ImageFit,
        ImageActualSize,
        ImageNextPage,
        ImagePreviousPage
    ]
);

/// 仅在图片文档持有键盘时成立，所以终端的键和文本动作都不会被这里截走。
pub(crate) const KEY_CONTEXT: &str = "ImageDocument";

/// 缩放下限。再小就看不出内容，只留一个点。
const ZOOM_MIN: f32 = 0.02;
const ZOOM_MAX: f32 = 32.;
const ZOOM_STEP: f32 = 1.25;
/// 一次方向键推动内容多少像素。
const PAN_STEP: f32 = 48.;
/// 没有 presentation 在画时，动画循环只按这个间隔查看是否重新可见；
/// 它不解码帧，也不会把隐藏的标签页算成观众。
const HIDDEN_POLL: Duration = Duration::from_millis(250);
/// 文档保留的已解码 TIFF 页数；每个 presentation 还各自 hold 住正在显示
/// 的那一页的 `Bitmap` 租约，所以这里的淘汰不会收回仍活跃 view 的纹理。
const TIFF_CACHED_PAGES: usize = 2;
/// 一个文档同时进行的页解码任务上限：工作台最多两个分组。
const TIFF_CONCURRENT_DECODES: usize = 2;

/// 一次 Host 读取的结果；解码失败的说明已经是可以显示的一句话。
pub(super) type ImageOutcome = Result<DecodedImage, SharedString>;

/// 已解码、尚未装进文档的内容。字节来自 owning Host，解码在 Host 线程
/// 或后台线程完成；像素仍是没有 GPUI 纹理的 RGBA，纹理租约在 UI 线程建立。
pub(super) enum DecodedImage {
    Static {
        rgba: image::RgbaImage,
        format: ImageFormat,
    },
    Gif {
        player: GifPlayback,
        first: image::RgbaImage,
        delay: Duration,
    },
    Tiff {
        pages: TiffPages,
    },
}

/// Decodes bytes read from the owning Host into document content.
///
/// Runs on the Host's worker thread, before any document exists: no GPUI
/// context, texture or timer is touched here. GIF metadata and its first
/// composited frame, the TIFF page table and a static bitmap are all produced
/// with the format's own decoder.
pub(super) fn decode_loaded(bytes: Vec<u8>, format: ImageFormat) -> ImageOutcome {
    let bytes: Arc<[u8]> = Arc::from(bytes);
    match format {
        ImageFormat::Gif => {
            let mut player = GifPlayback::new(bytes).map_err(|error| gif_error_message(&error))?;
            let (rgba, delay) = player
                .next_frame()
                .map_err(|error| gif_error_message(&error))?
                .ok_or_else(|| SharedString::from(t(L10nKey::EditorImageAnimationEmpty)))?;
            Ok(DecodedImage::Gif {
                player,
                first: rgba,
                delay,
            })
        }
        ImageFormat::Tiff => TiffPages::open(bytes)
            .map(|pages| DecodedImage::Tiff { pages })
            .map_err(|error| tiff_error_message(&error)),
        _ => {
            let (rgba, format) = images::decode_static(&bytes)
                .map_err(|error| SharedString::from(error.to_string()))?;
            // The decoder names the bytes it actually recognised; the name
            // claimed by the opener only labels a failure.
            Ok(DecodedImage::Static { rgba, format })
        }
    }
}

fn mib(bytes: u64) -> u64 {
    bytes.div_ceil(1024 * 1024)
}

/// The decoders live in the GUI-free crate; their user-facing wording is
/// localized here, together with every other editor message.
fn gif_error_message(error: &GifError) -> SharedString {
    match error {
        GifError::Budget { required, limit } => t_fmt(
            L10nKey::EditorImageAnimationLimit,
            &[
                ("required", &mib(*required).to_string()),
                ("limit", &mib(*limit).to_string()),
            ],
        )
        .into(),
        GifError::EmptyCanvas | GifError::NoFrames => t(L10nKey::EditorImageAnimationEmpty).into(),
        GifError::Truncated => t(L10nKey::EditorImageAnimationTruncated).into(),
        GifError::Decode(message) => {
            format!("{} ({message})", t(L10nKey::EditorImageUndecodable)).into()
        }
    }
}

fn tiff_error_message(error: &TiffError) -> SharedString {
    match error {
        TiffError::Pages { limit } => t_fmt(
            L10nKey::EditorImagePagesLimit,
            &[("pages", &limit.to_string())],
        )
        .into(),
        TiffError::Corrupt(message) => {
            format!("{} ({message})", t(L10nKey::EditorImageUndecodable)).into()
        }
    }
}

fn tiff_page_error_message(error: &TiffPageError) -> SharedString {
    match error {
        TiffPageError::Missing => t(L10nKey::EditorImagePageMissing).into(),
        TiffPageError::Dimensions {
            width,
            height,
            limit,
        } => t_fmt(
            L10nKey::EditorImagePageDimensionsLimit,
            &[
                ("width", &width.to_string()),
                ("height", &height.to_string()),
                ("limit", &limit.to_string()),
            ],
        )
        .into(),
        TiffPageError::Budget { required, limit } => t_fmt(
            L10nKey::EditorImagePageBytesLimit,
            &[
                ("required", &mib(*required).to_string()),
                ("limit", &mib(*limit).to_string()),
            ],
        )
        .into(),
        TiffPageError::Unsupported(layout) => {
            format!("{} ({layout})", t(L10nKey::EditorImagePageUnsupported)).into()
        }
        TiffPageError::Corrupt(message) => {
            format!("{} ({message})", t(L10nKey::EditorImageUndecodable)).into()
        }
    }
}

/// 图片文档此刻能画的东西。
///
/// 失败不是空文档：字节到了、名字也认了，但内容不是那张图，工作台仍然要为
/// 这份文件留一个说清楚的位置，下次外部变化再重新读。多页 TIFF 切到尚未
/// 解码的页时是 `Loading`，不是上一页或一张空白图。
#[derive(Clone)]
pub(crate) enum ImageContent {
    Ready(Bitmap),
    Loading,
    Failed(SharedString),
}

impl ImageContent {
    pub(crate) fn ready(&self) -> Option<&Bitmap> {
        match self {
            Self::Ready(bitmap) => Some(bitmap),
            Self::Loading | Self::Failed(_) => None,
        }
    }

    pub(crate) fn message(&self) -> Option<&SharedString> {
        match self {
            Self::Ready(_) | Self::Loading => None,
            Self::Failed(message) => Some(message),
        }
    }

    /// 只有真的换了内容才该重画；动画每帧换一块纹理，切页换一张图。
    fn same(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Ready(left), Self::Ready(right)) => Arc::ptr_eq(&left.image(), &right.image()),
            (Self::Loading, Self::Loading) => true,
            (Self::Failed(left), Self::Failed(right)) => left == right,
            _ => false,
        }
    }
}

/// 一份图片文件的内容：解码结果、格式与重新读取的状态。
pub(crate) struct ImageDocument {
    /// 名字声明或内容探针认出的格式；解码失败时，它是唯一还能说的格式。
    claimed: Option<ImageFormat>,
    format: Option<ImageFormat>,
    media: Media,
    reloading: bool,
    /// 重新读取就加一；迟到的 GIF 帧或 TIFF 页用它作废。
    revision: u64,
}

enum Media {
    Static(ImageContent),
    Gif(GifRun),
    Tiff(TiffRun),
}

/// GIF 文档级播放状态：帧内容跨 presentation 共享，取景各自独立。
struct GifRun {
    frame: ImageContent,
    player: Arc<Mutex<GifPlayback>>,
    next_delay: Duration,
    /// 上一帧里有 presentation 被画过；动画循环消费它。
    visible: bool,
    generation: u64,
    finished: bool,
    task: Option<Task<()>>,
}

/// 多页 TIFF：页表、按页缓存与在途页请求。
struct TiffRun {
    pages: TiffPages,
    cache: HashMap<usize, ImageContent>,
    /// 最近使用的页，用于把缓存收缩到 [`TIFF_CACHED_PAGES`]。
    order: VecDeque<usize>,
    pending: HashSet<usize>,
    generation: u64,
}

impl ImageDocument {
    pub(super) fn new(
        claimed: Option<ImageFormat>,
        outcome: ImageOutcome,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut document = Self {
            claimed,
            format: claimed,
            media: Media::Static(ImageContent::Loading),
            reloading: false,
            revision: 0,
        };
        document.set(outcome, cx);
        document
    }

    fn set(&mut self, outcome: ImageOutcome, cx: &mut Context<Self>) {
        match outcome {
            Ok(DecodedImage::Static { rgba, format }) => {
                self.format = Some(format);
                self.media = Media::Static(ImageContent::Ready(images::bitmap(rgba, cx)));
            }
            Ok(DecodedImage::Gif {
                player,
                first,
                delay,
            }) => {
                self.format = Some(ImageFormat::Gif);
                let generation = self.revision;
                self.media = Media::Gif(GifRun {
                    frame: ImageContent::Ready(images::bitmap(first, cx)),
                    player: Arc::new(Mutex::new(player)),
                    next_delay: delay,
                    visible: false,
                    generation,
                    finished: false,
                    task: None,
                });
                self.start_animation(cx);
            }
            Ok(DecodedImage::Tiff { pages }) => {
                self.format = Some(ImageFormat::Tiff);
                let generation = self.revision;
                self.media = Media::Tiff(TiffRun {
                    pages,
                    cache: HashMap::new(),
                    order: VecDeque::new(),
                    pending: HashSet::new(),
                    generation,
                });
            }
            Err(message) => {
                self.format = self.claimed;
                self.media = Media::Static(ImageContent::Failed(message));
            }
        }
        self.reloading = false;
    }

    /// 外部变化后重新读到的内容。旧图不在失败时继续冒充当前文件。
    pub(super) fn replace(&mut self, outcome: ImageOutcome, cx: &mut Context<Self>) {
        self.revision = self.revision.wrapping_add(1);
        self.set(outcome, cx);
        cx.notify();
    }

    /// 读取开始：状态行说正在读，不把上一次的图当成这一刻的文件。
    pub(super) fn begin_reload(&mut self, cx: &mut Context<Self>) {
        if !self.reloading {
            self.reloading = true;
            cx.notify();
        }
    }

    pub(crate) fn claimed(&self) -> Option<ImageFormat> {
        self.claimed
    }

    pub(crate) fn format(&self) -> Option<ImageFormat> {
        self.format
    }

    pub(crate) fn reloading(&self) -> bool {
        self.reloading
    }

    /// Bumped when a fresh read replaces the document's content.
    pub(crate) fn revision(&self) -> u64 {
        self.revision
    }

    /// 某个 presentation 该显示的页；静态与 GIF 忽略页号。
    fn content_for_page(&self, page: usize) -> Option<&ImageContent> {
        match &self.media {
            Media::Static(content) => Some(content),
            Media::Gif(run) => Some(&run.frame),
            Media::Tiff(run) => run.cache.get(&page),
        }
    }

    pub(crate) fn page_count(&self) -> Option<usize> {
        match &self.media {
            Media::Tiff(run) => Some(run.pages.len()),
            Media::Static(_) | Media::Gif(_) => None,
        }
    }

    /// 一次 presentation 的画面出现了：GIF 可以继续推进。
    fn note_visible(&mut self) {
        if let Media::Gif(run) = &mut self.media {
            run.visible = true;
        }
    }

    /// 请求某一页。已有缓存或在途请求时不重复解码。
    fn request_page(&mut self, page: usize, cx: &mut Context<Self>) {
        let Media::Tiff(run) = &mut self.media else {
            return;
        };
        if page >= run.pages.len()
            || run.cache.contains_key(&page)
            || run.pending.contains(&page)
            || run.pending.len() >= TIFF_CONCURRENT_DECODES
        {
            return;
        }
        run.pending.insert(page);
        let pages = run.pages.clone();
        let generation = run.generation;
        cx.spawn(async move |this, cx| {
            let decoded = cx.background_spawn(async move { pages.decode(page) }).await;
            let _ = this.update(cx, |document, cx| {
                document.page_install(generation, page, decoded, cx);
            });
        })
        .detach();
    }

    fn page_install(
        &mut self,
        generation: u64,
        page: usize,
        decoded: Result<image::RgbaImage, TiffPageError>,
        cx: &mut Context<Self>,
    ) {
        let Media::Tiff(run) = &mut self.media else {
            return;
        };
        // 重新读取或换文档后，迟到的页结果不得装进新文档。
        if run.generation != generation {
            return;
        }
        run.pending.remove(&page);
        if page >= run.pages.len() {
            return;
        }
        let content = match decoded {
            Ok(image) => ImageContent::Ready(images::bitmap(image, cx)),
            Err(error) => ImageContent::Failed(tiff_page_error_message(&error)),
        };
        run.cache.insert(page, content);
        run.order.push_back(page);
        while run.cache.len() > TIFF_CACHED_PAGES {
            let Some(oldest) = run.order.pop_front() else {
                break;
            };
            if oldest != page {
                run.cache.remove(&oldest);
            }
        }
        cx.notify();
    }

    fn start_animation(&mut self, cx: &mut Context<Self>) {
        let Media::Gif(run) = &mut self.media else {
            return;
        };
        if run.task.is_some() || run.finished {
            return;
        }
        let player = Arc::clone(&run.player);
        let generation = run.generation;
        let task = cx.spawn(async move |this, cx| {
            loop {
                let Some((delay, finished)) = this
                    .update(cx, |document, _| document.gif_wait(generation))
                    .ok()
                else {
                    break;
                };
                if finished {
                    break;
                }
                cx.background_executor().timer(delay).await;
                let Some(visible) = this
                    .update(cx, |document, _| document.gif_take_visible(generation))
                    .ok()
                else {
                    break;
                };
                if !visible {
                    // 隐藏的标签页不推进帧；只等下一次可见检查。
                    cx.background_executor().timer(HIDDEN_POLL).await;
                    continue;
                }
                let player = Arc::clone(&player);
                let decoded = cx
                    .background_spawn(async move {
                        let mut player = player
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                        player.next_frame()
                    })
                    .await;
                if this
                    .update(cx, |document, cx| {
                        document.gif_install(generation, decoded, cx)
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        run.task = Some(task);
    }

    fn gif_wait(&self, generation: u64) -> (Duration, bool) {
        match &self.media {
            Media::Gif(run) if run.generation == generation => (run.next_delay, run.finished),
            _ => (Duration::ZERO, true),
        }
    }

    fn gif_take_visible(&mut self, generation: u64) -> bool {
        match &mut self.media {
            Media::Gif(run) if run.generation == generation && !run.finished => {
                std::mem::take(&mut run.visible)
            }
            _ => false,
        }
    }

    fn gif_install(
        &mut self,
        generation: u64,
        decoded: Result<Option<(image::RgbaImage, Duration)>, GifError>,
        cx: &mut Context<Self>,
    ) {
        let Media::Gif(run) = &mut self.media else {
            return;
        };
        if run.generation != generation || run.finished {
            return;
        }
        match decoded {
            Ok(Some((image, delay))) => {
                run.frame = ImageContent::Ready(images::bitmap(image, cx));
                run.next_delay = delay;
            }
            // 有限循环播完：保持最后一帧，不再请求解码。
            Ok(None) => run.finished = true,
            Err(error) => {
                run.finished = true;
                run.frame = ImageContent::Failed(gif_error_message(&error));
            }
        }
        cx.notify();
    }
}

/// 一份图片文件在一个分组里的呈现：缩放、平移、页与它的键盘。
///
/// 缩放、平移和页属于呈现，不属于文件：同一张图在分屏两边可以有各自的取景
/// 与页，与 Markdown 的滚动位置一样。
pub(crate) struct ImageView {
    document: Entity<ImageDocument>,
    focus: FocusHandle,
    scroll: ScrollHandle,
    /// `None` 表示适应窗口；`Some` 是固定比例。
    zoom: Option<f32>,
    viewport: Option<Size<Pixels>>,
    /// 上一帧实际画出的比例，状态行读它，显示的就是眼前的样子。
    drawn: f32,
    /// 多页 TIFF 的当前页；其他格式恒为 0。
    page: usize,
    /// 这个 presentation 正在画的页/帧；页解码迟到时不会被替换。
    shown: ImageContent,
    /// 与 `shown` 对应的文档读取代次；重新读取后旧图不能继续冒充文件。
    revision: u64,
    _observe: Subscription,
}

impl ImageView {
    pub(super) fn new(document: Entity<ImageDocument>, cx: &mut Context<Self>) -> Self {
        Self::new_at(document, 0, cx)
    }

    pub(super) fn new_at(
        document: Entity<ImageDocument>,
        page: usize,
        cx: &mut Context<Self>,
    ) -> Self {
        let (page, shown, multipage, revision) = {
            let document = document.read(cx);
            let count = document.page_count();
            let page = count.map_or(0, |count| page.min(count.saturating_sub(1)));
            (
                page,
                document
                    .content_for_page(page)
                    .cloned()
                    .unwrap_or(ImageContent::Loading),
                count.is_some(),
                document.revision(),
            )
        };
        if multipage {
            document.update(cx, |document, cx| document.request_page(page, cx));
        }
        let observe = cx.observe(&document, |view, _, cx| view.sync(cx));
        Self {
            document,
            focus: cx.focus_handle(),
            scroll: ScrollHandle::new(),
            zoom: None,
            viewport: None,
            drawn: 1.,
            page,
            shown,
            revision,
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

    /// 这个 presentation 此刻正在画的画面。
    #[cfg(test)]
    pub(crate) fn content(&self) -> &ImageContent {
        &self.shown
    }

    pub(crate) fn page(&self) -> usize {
        self.page
    }

    pub(crate) fn page_count(&self, cx: &App) -> Option<usize> {
        self.document.read(cx).page_count()
    }

    /// “第 2 页，共 5 页”；单页文档没有页身份可报。
    pub(crate) fn page_label(&self, cx: &App) -> Option<SharedString> {
        self.page_count(cx).filter(|count| *count > 1).map(|count| {
            t_fmt(
                L10nKey::EditorImagePageOf,
                &[
                    ("page", &(self.page + 1).to_string()),
                    ("pages", &count.to_string()),
                ],
            )
            .into()
        })
    }

    fn multipage(&self, cx: &App) -> bool {
        self.page_count(cx).is_some_and(|count| count > 1)
    }

    pub(crate) fn next_page(&mut self, cx: &mut Context<Self>) {
        self.step_page(1, cx);
    }

    pub(crate) fn previous_page(&mut self, cx: &mut Context<Self>) {
        self.step_page(-1, cx);
    }

    fn step_page(&mut self, step: isize, cx: &mut Context<Self>) {
        let Some(count) = self.page_count(cx) else {
            return;
        };
        let page = if step < 0 {
            self.page.saturating_sub(step.unsigned_abs())
        } else {
            self.page.saturating_add(step as usize)
        }
        .min(count.saturating_sub(1));
        self.set_page(page, cx);
    }

    pub(crate) fn set_page(&mut self, page: usize, cx: &mut Context<Self>) {
        let Some(count) = self.page_count(cx) else {
            return;
        };
        let page = page.min(count.saturating_sub(1));
        if page == self.page {
            return;
        }
        self.page = page;
        // 新页的尺寸可能不同：不沿用上一页的平移偏移，缩放模式保留。
        // 已缓存的页直接显示，否则明确进入 Loading 并请求解码。
        self.shown = self
            .document
            .read(cx)
            .content_for_page(page)
            .cloned()
            .unwrap_or(ImageContent::Loading);
        self.scroll.set_offset(Point::default());
        if matches!(self.shown, ImageContent::Loading) {
            self.document
                .update(cx, |document, cx| document.request_page(page, cx));
        }
        cx.notify();
    }

    fn sync(&mut self, cx: &mut Context<Self>) {
        let (count, revision, page, content) = {
            let document = self.document.read(cx);
            let count = document.page_count();
            let page = count.map_or(self.page, |count| self.page.min(count.saturating_sub(1)));
            (
                count,
                document.revision(),
                page,
                document.content_for_page(page).cloned(),
            )
        };
        let mut changed = false;
        if page != self.page {
            self.page = page;
            self.scroll.set_offset(Point::default());
            changed = true;
        }
        if revision != self.revision {
            // A fresh read owns the document now; whatever was on screen
            // belonged to the old bytes.
            self.revision = revision;
            let next = content.clone().unwrap_or(ImageContent::Loading);
            if !self.shown.same(&next) {
                self.shown = next;
                changed = true;
            }
        } else if let Some(next) = content {
            if !self.shown.same(&next) {
                self.shown = next;
                changed = true;
            }
        }
        // A missing cache entry on the same page is the document's eviction,
        // not disappearance: this view still owns its bitmap lease, and the
        // next page change or reload will ask for bytes again.
        if count.is_some() && matches!(self.shown, ImageContent::Loading) {
            self.document
                .update(cx, |document, cx| document.request_page(self.page, cx));
        }
        if changed {
            cx.notify();
        }
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

    /// 状态行的一段：格式、像素大小，读取/翻页中就说正在读。
    pub(crate) fn status_text(&self, cx: &App) -> SharedString {
        let document = self.document.read(cx);
        let mut parts = Vec::with_capacity(4);
        match &self.shown {
            ImageContent::Ready(bitmap) => {
                if let Some(format) = document.format() {
                    parts.push(format.label().to_string());
                }
                let size = bitmap.image().size(0);
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
            ImageContent::Loading => {
                if let Some(format) = document.format() {
                    parts.push(format.label().to_string());
                }
                parts.push(t(L10nKey::EditorImagePageLoading).to_string());
                parts.join(" · ").into()
            }
            ImageContent::Failed(message) => {
                format!("{} · {message}", t(L10nKey::EditorImageFailed)).into()
            }
        }
    }

    /// 状态行与工具栏共用的按钮：翻页、适应窗口、实际大小。
    fn controls(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let ready = self.shown.ready().is_some();
        let multipage = self.multipage(cx);
        let count = self.page_count(cx).unwrap_or(0);
        h_flex()
            .flex_none()
            .items_center()
            .gap_1()
            .when(multipage, |row| {
                row.child(
                    Button::new("image-page-previous")
                        .ghost()
                        .xsmall()
                        .flex_none()
                        .icon(IconName::ChevronLeft)
                        .accessibility_label(t(L10nKey::EditorImagePagePrevious))
                        .tooltip(t(L10nKey::EditorImagePagePrevious))
                        .disabled(self.page == 0)
                        .on_click(cx.listener(|this, _, _, cx| this.previous_page(cx))),
                )
                .child(
                    div()
                        .id("image-page-facts")
                        .test_support()
                        .flex_none()
                        .text_xs()
                        .child(self.page_label(cx).unwrap_or_default()),
                )
                .child(
                    Button::new("image-page-next")
                        .ghost()
                        .xsmall()
                        .flex_none()
                        .icon(IconName::ChevronRight)
                        .accessibility_label(t(L10nKey::EditorImagePageNext))
                        .tooltip(t(L10nKey::EditorImagePageNext))
                        .disabled(self.page + 1 >= count)
                        .on_click(cx.listener(|this, _, _, cx| this.next_page(cx))),
                )
            })
            .when(ready, |row| {
                row.child(
                    Button::new("image-fit")
                        .ghost()
                        .xsmall()
                        .flex_none()
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
                        .label(t(L10nKey::EditorImageActualSize))
                        .accessibility_label(t(L10nKey::EditorImageActualSizeAction))
                        .tooltip(t(L10nKey::EditorImageActualSizeAction))
                        .on_click(cx.listener(|this, _, _, cx| this.actual_size(cx))),
                )
            })
    }
}

impl Focusable for ImageView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for ImageView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // 这次画面算作一次可见；GIF 由此知道有观众，隐藏后不再推进帧。
        self.document
            .update(cx, |document, _| document.note_visible());
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
        let format = self.document.read(cx).format();
        let reloading = self.document.read(cx).reloading();
        let multipage = self.multipage(cx);
        let page_label = self.page_label(cx);
        let image = self.shown.ready().map(|bitmap| bitmap.image());
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
            .when_some(page_label, |row, label| {
                row.child(
                    div()
                        .id("image-page-label")
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
                        .child(t(L10nKey::EditorImageLoading)),
                )
            })
            .when(matches!(self.shown, ImageContent::Loading), |row| {
                row.child(
                    div()
                        .flex_none()
                        .text_xs()
                        .text_color(muted)
                        .child(t(L10nKey::EditorImagePageLoading)),
                )
            })
            .when(image.is_some() || multipage, |row| {
                row.when(image.is_some(), |row| {
                    row.child(div().flex_none().text_xs().text_color(muted).child(percent))
                })
                .child(self.controls(cx))
            });

        let body = if let (Some(image), Some(natural)) = (image, natural) {
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
                                .child(
                                    gpui_kit::img(image)
                                        .id(("image-document-pixels", cx.entity_id()))
                                        .test_support()
                                        .flex_none()
                                        .w(natural.width * scale)
                                        .h(natural.height * scale),
                                ),
                        ),
                )
                .into_any_element()
        } else if let Some(message) = self.shown.message().cloned() {
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
        } else {
            v_flex()
                .id("status-image-loading")
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
                    div()
                        .text_sm()
                        .text_color(muted)
                        .child(t(L10nKey::EditorImagePageLoading)),
                )
                .into_any_element()
        };

        v_flex()
            .id(("image-document", cx.entity_id()))
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &ImageZoomIn, _, cx| this.zoom_in(cx)))
            .on_action(cx.listener(|this, _: &ImageZoomOut, _, cx| this.zoom_out(cx)))
            .on_action(cx.listener(|this, _: &ImageFit, _, cx| this.fit(cx)))
            .on_action(cx.listener(|this, _: &ImageActualSize, _, cx| this.actual_size(cx)))
            .on_action(cx.listener(|this, _: &ImageNextPage, _, cx| this.next_page(cx)))
            .on_action(cx.listener(|this, _: &ImagePreviousPage, _, cx| this.previous_page(cx)))
            .on_key_down(
                cx.listener(|this, event: &gpui_kit::KeyDownEvent, _window, cx| {
                    let key = event.keystroke.key.as_str();
                    // Ctrl/Cmd+PageUp/PageDown already belong to tab navigation in
                    // the application keymap; leave those chords alone.
                    if event.keystroke.modifiers.secondary(cx)
                        && matches!(key, "pageup" | "pagedown")
                    {
                        return;
                    }
                    if this.multipage(cx)
                        && let Some(count) = this.page_count(cx)
                    {
                        match key {
                            "pageup" => {
                                this.previous_page(cx);
                                cx.stop_propagation();
                                return;
                            }
                            "pagedown" => {
                                this.next_page(cx);
                                cx.stop_propagation();
                                return;
                            }
                            "home" => {
                                this.set_page(0, cx);
                                cx.stop_propagation();
                                return;
                            }
                            "end" => {
                                this.set_page(count.saturating_sub(1), cx);
                                cx.stop_propagation();
                                return;
                            }
                            _ => {}
                        }
                    }
                    let (dx, dy) = match key {
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

/// 右侧菜单里的取景与翻页命令，和快捷键、工具栏按钮是同一组动作。
pub(crate) fn menu_entries(
    menu: gpui_kit::component::menu::PopupMenu,
    multipage: bool,
) -> gpui_kit::component::menu::PopupMenu {
    let menu = menu
        .separator()
        .menu(t(L10nKey::EditorImageZoomIn), Box::new(ImageZoomIn))
        .menu(t(L10nKey::EditorImageZoomOut), Box::new(ImageZoomOut))
        .menu(t(L10nKey::EditorImageFit), Box::new(ImageFit))
        .menu(t(L10nKey::EditorImageActualSize), Box::new(ImageActualSize));
    if multipage {
        menu.separator()
            .menu(
                t(L10nKey::EditorImagePagePrevious),
                Box::new(ImagePreviousPage),
            )
            .menu(t(L10nKey::EditorImagePageNext), Box::new(ImageNextPage))
    } else {
        menu
    }
}

#[cfg(test)]
#[path = "image_document/tests.rs"]
mod tests;
