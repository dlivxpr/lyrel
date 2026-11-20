//! 图片 I/O、权限和异步生命周期只归工作台；editor 仅消费 bitmap 和发出意图。
use super::super::*;
use super::links::{self, Target};
use gpui_kit::component::{WindowExt as _, button::Button};
use gpui_kit::{AnyWindowHandle, App, ClipboardEntry, EntityId, Global, RenderImage, WeakEntity};
use lyrel_core::host::Host;
use lyrel_document::CancelToken;
use lyrel_document::image::MAX_PIXELS;
use lyrel_markdown_editor::{
    Document, DocumentEvent, EditorEvent as NativeEvent, EditorState as MarkdownEditor,
};
use std::{cell::RefCell, io, rc::Rc, time::Duration};

pub const MAX_BYTES: u64 = 32 * 1024 * 1024;
const CACHE_BYTES: u64 = 256 * 1024 * 1024;

/// Resolve the effective proxy for one URL, in the desktop's precedence order.
///
/// The production implementation (manual `config.json` proxy, then the
/// platform system proxy, then the environment) lives in
/// `lyrel_core::daemon::install::proxy::resolve` and is injected by the
/// desktop crate through [`ImageProxyResolver`]; this crate compiles without
/// the installer's `remote-install` feature, so it cannot resolve proxies
/// itself. Harnesses install their own resolver to route fetches through
/// loopback fixtures.
pub type ProxyResolveFn = Arc<dyn Fn(&str, Option<&str>) -> Option<ureq::Proxy> + Send + Sync>;

/// The proxy resolver the desktop shell installs for remote Markdown image
/// previews.
///
/// `ImageHost::load` clones the installed resolver on the UI thread and hands
/// it to the worker running [`fetch`], which applies the resolver's `Option`
/// verbatim: `Some(proxy)` routes the request through it, `None` explicitly
/// disables any proxy. When no resolver is installed, the fetch keeps ureq's
/// own default configuration (environment variables only).
pub struct ImageProxyResolver {
    resolve: ProxyResolveFn,
}

impl ImageProxyResolver {
    pub fn new(resolve: ProxyResolveFn) -> Self {
        Self { resolve }
    }

    pub fn install(cx: &mut App, resolver: Self) {
        cx.set_global(resolver);
    }

    /// The installed resolver's function, if the shell (or a harness)
    /// installed one.
    pub fn get(cx: &App) -> Option<ProxyResolveFn> {
        cx.try_global::<Self>()
            .map(|resolver| resolver.resolve.clone())
    }
}

impl Global for ImageProxyResolver {}

type Signature = (HostId, PathBuf);
type Images = Rc<RefCell<HashMap<String, Bitmap>>>;

/// Cache and preview share this lease, so eviction cannot retire a live preview's tile.
#[derive(Clone)]
pub struct Bitmap {
    image: Arc<RenderImage>,
    _lease: Entity<()>,
}
impl Bitmap {
    pub fn new(image: Arc<RenderImage>, cx: &mut App) -> Self {
        let release = image.clone();
        let lease = cx.new(|cx| {
            cx.on_release(move |_, cx| cx.drop_image(release, None))
                .detach();
        });
        Self {
            image,
            _lease: lease,
        }
    }
    /// The decoded frames, as the UI texture cache draws them.
    pub fn image(&self) -> Arc<RenderImage> {
        self.image.clone()
    }
}
#[derive(Clone)]
enum State {
    Loading,
    Ready,
    Failed(String),
}
#[derive(Clone)]
struct Capture {
    host: SharedHost,
    path: PathBuf,
    revision: u64,
    selection: std::ops::Range<usize>,
}

pub struct ImageHost {
    panel: WeakEntity<EditorPanelState>,
    id: BufferId,
    window: AnyWindowHandle,
    document: Entity<Document>,
    signature: Option<Signature>,
    generation: u64,
    sources: HashSet<String>,
    scanned_revision: Option<u64>,
    images: Images,
    states: HashMap<String, State>,
    transfers: HashMap<String, (u64, CancelToken)>,
    progress: HashMap<String, (u64, u64)>,
    views: HashMap<EntityId, (WeakEntity<MarkdownEditor>, Subscription)>,
    hover: Option<(EntityId, SharedString, gpui_kit::Bounds<gpui_kit::Pixels>)>,
    notice: Option<SharedString>,
    refresh_scheduled: bool,
    importing: bool,
    _document: Subscription,
}

impl ImageHost {
    pub fn new(
        panel: WeakEntity<EditorPanelState>,
        id: BufferId,
        editor: &Entity<MarkdownEditor>,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let document = editor.read(cx).document().clone();
        let subscription =
            cx.subscribe(&document, |this, _, _: &DocumentEvent, cx| this.refresh(cx));
        cx.on_release(|this, _| {
            this.cancel_transfers();
            this.images.borrow_mut().clear();
        })
        .detach();
        Self {
            panel,
            id,
            window: window.window_handle(),
            document,
            signature: None,
            generation: 0,
            sources: HashSet::new(),
            scanned_revision: None,
            images: Rc::new(RefCell::new(HashMap::new())),
            states: HashMap::new(),
            transfers: HashMap::new(),
            progress: HashMap::new(),
            views: HashMap::new(),
            hover: None,
            refresh_scheduled: false,
            importing: false,
            notice: None,
            _document: subscription,
        }
    }

    /// Every presentation installs its own event subscription and shares only Host bitmap data.
    pub fn attach(owner: &Entity<Self>, editor: &Entity<MarkdownEditor>, cx: &mut App) {
        owner.update(cx, |this, cx| {
            if !this.views.contains_key(&editor.entity_id()) {
                let images = this.images.clone();
                editor.update(cx, |editor, _| {
                    editor.set_block_image_provider(move |src| {
                        images.borrow().get(src).map(|bitmap| bitmap.image.clone())
                    });
                    editor.set_host_link_navigation(!editor.is_source_only());
                });
                let subscription = cx.subscribe(editor, |this, source, event: &NativeEvent, cx| {
                    this.event(source, event, cx)
                });
                this.views
                    .insert(editor.entity_id(), (editor.downgrade(), subscription));
            }
            // Render may observe a save-as/rename without a source edit. Queue a bounded,
            // idempotent identity refresh, outside the borrowed workbench render/update.
            if !this.refresh_scheduled {
                this.refresh_scheduled = true;
                cx.spawn(async move |this, cx| {
                    let _ = this.update(cx, |this, cx| {
                        this.refresh_scheduled = false;
                        this.refresh(cx);
                    });
                })
                .detach();
            }
        });
    }

    fn identity(&self, cx: &App) -> Option<(SharedHost, Option<PathBuf>)> {
        let panel = self.panel.upgrade()?;
        let file = panel.read(cx).buffers.get(self.id)?;
        let source = file.input.markdown()?;
        if source.read(cx).document() != &self.document {
            return None;
        }
        Some((
            file.host.clone(),
            file.untitled.is_none().then(|| file.path.clone()),
        ))
    }
    fn capture(&self, editor: &Entity<MarkdownEditor>, cx: &App) -> Option<Capture> {
        let (host, path) = self.identity(cx)?;
        Some(Capture {
            host,
            path: path?,
            revision: self.document.read(cx).revision(),
            selection: editor.read(cx).selected_range(),
        })
    }
    fn current(&self, capture: &Capture, cx: &App) -> bool {
        self.identity(cx).is_some_and(|(host, path)| {
            host.id() == capture.host.id() && path.as_ref() == Some(&capture.path)
        }) && self.document.read(cx).revision() == capture.revision
    }
    fn notify(&mut self, cx: &mut Context<Self>) {
        for (editor, _) in self.views.values() {
            let _ = editor.update(cx, |_, cx| cx.notify());
        }
        let _ = self.panel.update(cx, |_, cx| cx.notify());
        cx.notify();
    }
    /// Stops every in-flight picture transfer and drops its buffers.
    fn cancel_transfers(&mut self) {
        for (_, token) in std::mem::take(&mut self.transfers).into_values() {
            token.cancel();
        }
        self.progress.clear();
    }
    fn message(&mut self, message: impl Into<SharedString>, cx: &mut Context<Self>) {
        let message = message.into();
        self.notice = Some(message.clone());
        let _ = self.window.update(cx, |_, window, cx| {
            window.notify_app(message.to_string(), cx)
        });
        self.notify(cx);
    }
    fn event(
        &mut self,
        editor: Entity<MarkdownEditor>,
        event: &NativeEvent,
        cx: &mut Context<Self>,
    ) {
        match event {
            NativeEvent::OpenLink(target) => {
                let panel = self.panel.clone();
                let id = self.id;
                let _ = self.window.update(cx, |_, window, cx| {
                    let _ = panel.update(cx, |panel, cx| {
                        panel.follow_link(id, &editor, target, window, cx)
                    });
                });
            }
            NativeEvent::HoverLink(target) => {
                let next = target.as_ref().and_then(|(hit, bounds)| match hit {
                    lyrel_markdown_editor::LinkHit::Url(url) => {
                        Some((editor.entity_id(), url.clone().into(), *bounds))
                    }
                    _ => None,
                });
                if next.is_some()
                    || self
                        .hover
                        .as_ref()
                        .is_some_and(|(view, _, _)| *view == editor.entity_id())
                {
                    self.hover = next;
                    self.notify(cx);
                }
            }
            NativeEvent::SelectionChanged => {
                if self.hover.take().is_some() {
                    self.notify(cx);
                }
            }
            NativeEvent::PreviewImage(source) => self.preview(&editor, source, cx),
            NativeEvent::PasteAttachments(item) => {
                let Some(capture) = self.capture(&editor, cx) else {
                    self.message(t(L10nKey::MarkdownImageChooseLocation), cx);
                    return;
                };
                if let Some(image) = item.entries().iter().find_map(|entry| match entry {
                    ClipboardEntry::Image(image) => Some(image),
                    _ => None,
                }) {
                    self.import(editor, capture, Import::Bytes(image.bytes.clone()), cx);
                } else {
                    let paths: Vec<_> = item
                        .entries()
                        .iter()
                        .filter_map(|entry| match entry {
                            ClipboardEntry::ExternalPaths(paths) => Some(paths.paths()),
                            _ => None,
                        })
                        .flatten()
                        .cloned()
                        .collect();
                    self.import(editor, capture, Import::Paths(paths), cx);
                }
            }
            NativeEvent::InsertImages(paths) => self.insert(&editor, paths.clone(), cx),
            _ => {}
        }
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        self.views.retain(|_, (view, _)| view.upgrade().is_some());
        let Some((host, path)) = self.identity(cx) else {
            // The document this preview belonged to is gone; a read still in
            // flight must stop instead of finishing for nobody.
            self.cancel_transfers();
            return;
        };
        let signature = path.as_ref().map(|path| (host.id(), path.clone()));
        let changed_identity = signature != self.signature;
        let revision = self.document.read(cx).revision();
        if !changed_identity
            && self.scanned_revision == Some(revision)
            && self.sources.len() == self.states.len()
        {
            return;
        }
        self.scanned_revision = Some(revision);
        self.hover = None;
        if changed_identity {
            self.cancel_transfers();
            self.signature = signature;
            self.generation = self.generation.wrapping_add(1);
            self.images.borrow_mut().clear();
            self.states.clear();
            for (view, _) in self.views.values() {
                let _ = view.update(cx, |view, cx| view.refresh_resource_layout(cx));
            }
        }
        let source_only = path
            .as_ref()
            .and_then(|path| path.extension())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("mdx"));
        let sources: HashSet<_> = if source_only {
            HashSet::new()
        } else {
            self.document
                .read(cx)
                .snapshot()
                .image_sources()
                .into_iter()
                .collect()
        };
        let changed = sources != self.sources || changed_identity;
        self.sources = sources;
        let stale: Vec<String> = self
            .transfers
            .keys()
            .filter(|source| !self.sources.contains(*source))
            .cloned()
            .collect();
        for source in stale {
            if let Some((_, token)) = self.transfers.remove(&source) {
                token.cancel();
            }
            self.progress.remove(&source);
        }
        self.states
            .retain(|source, _| self.sources.contains(source));
        self.images
            .borrow_mut()
            .retain(|source, _| self.sources.contains(source));
        let pending: Vec<_> = self
            .sources
            .iter()
            .filter(|source| !self.states.contains_key(*source))
            .cloned()
            .collect();
        for source in pending {
            self.load(host.clone(), path.as_deref(), source, cx);
        }
        if changed {
            self.notify(cx);
        }
    }
    fn load(
        &mut self,
        host: SharedHost,
        document: Option<&Path>,
        source: String,
        cx: &mut Context<Self>,
    ) {
        let target = match links::resolve(&*host, document, destination(&source)) {
            Ok(Target::External(url)) if matches!(url.scheme(), "http" | "https") => {
                Resource::External(url)
            }
            Ok(Target::File(path, _)) => Resource::File(path),
            Ok(_) => {
                self.states.insert(
                    source,
                    State::Failed(t(L10nKey::MarkdownLinkUnsupportedScheme).into()),
                );
                return;
            }
            Err(reason) => {
                self.states.insert(source, State::Failed(t(reason).into()));
                return;
            }
        };
        let proxy = cx
            .global::<lyrel_ui_base::config::Config>()
            .http_proxy
            .clone();
        // Cloned here, on the UI thread: the worker cannot touch globals, and
        // the resolver must stay swappable for loopback fixtures.
        let resolve = ImageProxyResolver::get(cx);
        let generation = self.generation;
        let signature = self.signature.clone();
        let cancel = CancelToken::new();
        if let Some((_, superseded)) = self
            .transfers
            .insert(source.clone(), (generation, cancel.clone()))
        {
            superseded.cancel();
        }
        self.progress.remove(&source);
        self.states.insert(source.clone(), State::Loading);
        // A blocking Host read cannot call back into GPUI, so it reports
        // progress through a channel drained by a UI-side task.
        let (progress_tx, progress_rx) = smol::channel::unbounded::<(u64, u64)>();
        let progress_source = source.clone();
        cx.spawn(async move |this, cx| {
            while let Ok((read, total)) = progress_rx.recv().await {
                let live = this
                    .update(cx, |this, cx| {
                        if this
                            .transfers
                            .get(&progress_source)
                            .is_some_and(|(current, _)| *current == generation)
                        {
                            this.progress.insert(progress_source.clone(), (read, total));
                            this.notify(cx);
                        }
                    })
                    .is_ok();
                if !live {
                    break;
                }
            }
        })
        .detach();
        let worker_cancel = cancel.clone();
        HostOps::run(
            host,
            cx,
            move |h| -> io::Result<Arc<RenderImage>> {
                let bytes = match target {
                    Resource::File(path) => {
                        let mut report = |update: lyrel_core::host::read::ReadProgress| {
                            let _ = progress_tx.send_blocking((update.read, update.total));
                        };
                        read_bytes(h, &path, MAX_BYTES, &worker_cancel, &mut report)?.1
                    }
                    Resource::External(url) => fetch(&url, proxy.as_deref(), resolve.as_ref())?,
                };
                decode(&bytes).map(|(rgba, _)| render(rgba))
            },
            move |this, result, cx| {
                // A completion from an older generation must not drop the
                // live request's cancel token or progress: only the owner of
                // the entry may retire it.
                if this
                    .transfers
                    .get(&source)
                    .is_some_and(|(current, _)| *current == generation)
                {
                    this.transfers.remove(&source);
                    this.progress.remove(&source);
                }
                let current = this.identity(cx).and_then(|(h, p)| p.map(|p| (h.id(), p)));
                if this.generation != generation
                    || this.signature != signature
                    || current != signature
                    || !this.sources.contains(&source)
                {
                    return;
                }
                match result {
                    Ok(image) => {
                        let used: u64 = this
                            .images
                            .borrow()
                            .values()
                            .map(|image| {
                                let size = image.image.size(0);
                                size.width.0 as u64 * size.height.0 as u64 * 4
                            })
                            .sum();
                        let size = image.size(0);
                        if used + size.width.0 as u64 * size.height.0 as u64 * 4 > CACHE_BYTES {
                            this.states.insert(
                                source,
                                State::Failed(t(L10nKey::MarkdownImageLimit).into()),
                            );
                        } else {
                            this.images
                                .borrow_mut()
                                .insert(source.clone(), Bitmap::new(image, cx));
                            this.states.insert(source, State::Ready);
                        }
                    }
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => {
                        // The document, identity or source that asked for this
                        // picture is gone; leave nothing claiming to be loading.
                        this.states.remove(&source);
                    }
                    Err(error) => {
                        this.states.insert(source, State::Failed(error.to_string()));
                    }
                }
                for (view, _) in this.views.values() {
                    let _ = view.update(cx, |view, cx| view.refresh_resource_layout(cx));
                }
                this.notify(cx);
            },
        );
    }

    fn insert(
        &mut self,
        editor: &Entity<MarkdownEditor>,
        paths: Vec<PathBuf>,
        cx: &mut Context<Self>,
    ) {
        let Some(capture) = self.capture(editor, cx) else {
            self.message(t(L10nKey::MarkdownImageChooseLocation), cx);
            return;
        };
        if !paths.is_empty() {
            self.import(editor.clone(), capture, Import::Paths(paths), cx);
            return;
        }
        let picker = cx.prompt_for_paths(gpui_kit::PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some(t(L10nKey::MarkdownInsertImage).into()),
        });
        let editor = editor.downgrade();
        cx.spawn(async move |this, cx| match picker.await {
            Ok(Ok(Some(paths))) => {
                let _ = this.update(cx, |this, cx| {
                    if let Some(editor) = editor.upgrade() {
                        this.import(editor, capture, Import::Paths(paths), cx);
                    }
                });
            }
            Ok(Err(error)) => {
                let _ = this.update(cx, |this, cx| this.message(error.to_string(), cx));
            }
            _ => {}
        })
        .detach();
    }

    fn import(
        &mut self,
        editor: Entity<MarkdownEditor>,
        capture: Capture,
        input: Import,
        cx: &mut Context<Self>,
    ) {
        self.notice = None;
        if self.importing {
            self.message(t(L10nKey::MarkdownImageWriting), cx);
            return;
        }
        if !self.current(&capture, cx) || editor.read(cx).selected_range() != capture.selection {
            self.message(t(L10nKey::MarkdownImageChanged), cx);
            return;
        }
        self.importing = true;
        self.notify(cx);
        let path = capture.path.clone();
        let request = capture.clone();
        let editor = editor.downgrade();
        HostOps::run(
            capture.host.clone(),
            cx,
            move |h| store(h, &path, input),
            move |this, result, cx| {
                this.importing = false;
                match result {
                    Ok(sources) => {
                        let Some(editor) = editor.upgrade() else {
                            return;
                        };
                        if !this.current(&request, cx)
                            || editor.read(cx).selected_range() != request.selection
                        {
                            this.message(t(L10nKey::MarkdownImageSelectionChanged), cx);
                        } else {
                            let mut text = String::new();
                            let view = editor.read(cx);
                            if request.selection.start > 0
                                && !view.text()[..request.selection.start].ends_with('\n')
                            {
                                text.push('\n');
                            }
                            for source in sources {
                                text.push_str(&format!("![]({source})\n"));
                            }
                            editor.update(cx, |view, cx| {
                                view.replace_range(request.selection, &text, cx);
                            });
                        }
                    }
                    Err(error) => this.message(
                        format!("{}: {error}", t(L10nKey::MarkdownImageWriteFailed)),
                        cx,
                    ),
                }
                this.notify(cx);
            },
        );
    }

    fn preview(&mut self, editor: &Entity<MarkdownEditor>, source: &str, cx: &mut Context<Self>) {
        let current = self
            .identity(cx)
            .and_then(|(host, path)| path.map(|path| (host.id(), path)));
        if current != self.signature
            || !self
                .document
                .read(cx)
                .snapshot()
                .image_sources()
                .iter()
                .any(|src| src == source)
        {
            self.message(t(L10nKey::MarkdownImageChanged), cx);
            return;
        }
        let image = self.images.borrow().get(source).cloned();
        let Some(image) = image else {
            self.message(t(L10nKey::EditorImageFailed), cx);
            return;
        };
        let focus = editor.read(cx).focus_handle(cx);
        let title = source.to_owned();
        let _ = self.window.update(cx, |_, window, cx| {
            open_preview_dialog(title, image, Some(focus), window, cx);
        });
    }

    #[cfg(any(test, feature = "test-support", feature = "native-markdown-tests"))]
    pub fn is_loaded(&self, source: &str) -> bool {
        matches!(self.states.get(source), Some(State::Ready))
    }
}

enum Resource {
    File(PathBuf),
    External(url::Url),
}

fn destination(source: &str) -> &str {
    let source = source.trim();
    if let Some(angle) = source.strip_prefix('<')
        && let Some(end) = angle.find('>')
    {
        return &angle[..end];
    }
    for marker in [" \"", " '"] {
        if let Some((path, title)) = source.split_once(marker)
            && title.ends_with(&marker[1..])
        {
            return path;
        }
    }
    source
}
enum Import {
    Bytes(Vec<u8>),
    Paths(Vec<PathBuf>),
}

// Preserve keys across worker threads; format with the UI locale on completion.
#[derive(Debug)]
enum ImageError {
    Localized(L10nKey),
    Redirect(u16),
    /// The bytes were read as an image format but would not decode.
    Corrupt(String),
    /// SVG source that the native parser refused.
    Svg(String),
}

impl std::fmt::Display for ImageError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Localized(key) => formatter.write_str(t(*key)),
            Self::Redirect(status) => write!(
                formatter,
                "HTTP {status} — {}",
                t(L10nKey::MarkdownImageRedirectBlocked)
            ),
            Self::Corrupt(message) => {
                write!(
                    formatter,
                    "{} ({message})",
                    t(L10nKey::EditorImageUndecodable)
                )
            }
            Self::Svg(message) => {
                write!(formatter, "{} ({message})", t(L10nKey::EditorSvgInvalid))
            }
        }
    }
}

impl std::error::Error for ImageError {}

fn invalid(message: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

/// The image formats a workbench document can open.
///
/// The five static raster formats share [`decode_static`]; GIF animation and
/// multi-page TIFF have their own frame/page decoders in `image_document` and
/// are never shown as a static first frame. Markdown preview still resolves
/// only the static subset through [`decode_static`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageFormat {
    Png,
    Jpeg,
    WebP,
    Bmp,
    Ico,
    Gif,
    Tiff,
}

impl ImageFormat {
    pub fn from_extension(path: &Path) -> Option<Self> {
        let extension = path.extension()?.to_str()?.to_ascii_lowercase();
        Some(match extension.as_str() {
            "png" => Self::Png,
            "jpg" | "jpeg" | "jpe" | "jfif" => Self::Jpeg,
            "webp" => Self::WebP,
            "bmp" | "dib" => Self::Bmp,
            "ico" => Self::Ico,
            "gif" => Self::Gif,
            "tif" | "tiff" => Self::Tiff,
            _ => return None,
        })
    }

    /// What the first bytes say, for a file whose name claims nothing.
    ///
    /// BMP is left out on purpose: `BM` is two ordinary capital letters, so a
    /// text file opening with them is not evidence enough to take it away
    /// from the text editor. GIF's `GIF87a`/`GIF89a` and TIFF's `II*\0`/`MM\0*`
    /// (including BigTIFF) are binary structures and cannot be mistaken for
    /// ordinary text.
    pub fn probe(bytes: &[u8]) -> Option<Self> {
        if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
            return Some(Self::Png);
        }
        if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
            return Some(Self::Jpeg);
        }
        if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
            return Some(Self::WebP);
        }
        if bytes.starts_with(&[0x00, 0x00, 0x01, 0x00]) {
            return Some(Self::Ico);
        }
        if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
            return Some(Self::Gif);
        }
        if bytes.starts_with(b"II\x2a\x00")
            || bytes.starts_with(b"MM\x00\x2a")
            || bytes.starts_with(b"II\x2b\x00")
            || bytes.starts_with(b"MM\x00\x2b")
        {
            return Some(Self::Tiff);
        }
        None
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Png => "PNG",
            Self::Jpeg => "JPEG",
            Self::WebP => "WebP",
            Self::Bmp => "BMP",
            Self::Ico => "ICO",
            Self::Gif => "GIF",
            Self::Tiff => "TIFF",
        }
    }
}

pub fn too_large() -> io::Error {
    invalid(ImageError::Localized(L10nKey::MarkdownImageLimit))
}

/// Reads one picture's bytes from its owning Host.
///
/// The versioned segment session is the real path for a preview: the bytes and
/// their version stay one object while they travel. A Host that reports
/// `Unsupported` — an SFTP pane host, or a peer without `host-file-range-v1` —
/// keeps the previous bounded whole-file read for inputs inside the existing
/// picture budget; a larger input still fails visibly instead of being read
/// some other way.
pub fn read_bytes(
    host: &dyn Host,
    path: &std::path::Path,
    budget: u64,
    cancel: &CancelToken,
    mut progress: impl FnMut(lyrel_core::host::read::ReadProgress),
) -> io::Result<(lyrel_core::host::Meta, Vec<u8>)> {
    // The document task owns cancellation; the Host transfer checks the same
    // flag, so superseding or closing the consumer stops the segment read.
    let transfer = lyrel_core::host::transfer::CancelToken::from_flag(cancel.shared_flag());
    match lyrel_core::host::read::read_file_segmented(host, path, budget, &transfer, &mut progress)
    {
        Ok(outcome) => Ok((outcome.meta, outcome.bytes)),
        Err(error) if error.kind() == io::ErrorKind::Unsupported => {
            let meta = host.stat(path)?;
            if meta.len > budget {
                return Err(too_large());
            }
            log::debug!(
                "host cannot open a versioned read session; using its bounded whole-file read for {}",
                path.display()
            );
            let bytes = host.read_file(path, budget)?;
            progress(lyrel_core::host::read::ReadProgress {
                read: bytes.len() as u64,
                total: bytes.len() as u64,
            });
            Ok((meta, bytes))
        }
        Err(error) => Err(error),
    }
}

/// Decodes one of the five static formats under the same byte and pixel
/// budgets as a Markdown preview, and under the same rule that a name never
/// decides how bytes are read: SVG is not among them, so a file called
/// `.png` is never handed to `usvg`.
///
/// GIF animation and multi-page TIFF are documents with their own frame/page
/// decoders; handing their bytes here is refused rather than showing a static
/// first frame.
pub fn decode_static(bytes: &[u8]) -> io::Result<(image::RgbaImage, ImageFormat)> {
    let (rgba, extension) = decode(bytes).map_err(|error| {
        // Budget and unsupported-format refusals are already words; anything
        // else is the decoder saying the bytes are not the picture they claim.
        let described = error
            .get_ref()
            .is_some_and(|inner| inner.downcast_ref::<ImageError>().is_some());
        match described {
            true => error,
            false => invalid(ImageError::Corrupt(error.to_string())),
        }
    })?;
    let format = match extension {
        "png" => ImageFormat::Png,
        "jpg" => ImageFormat::Jpeg,
        "webp" => ImageFormat::WebP,
        "bmp" => ImageFormat::Bmp,
        "ico" => ImageFormat::Ico,
        _ => {
            return Err(invalid(ImageError::Localized(
                L10nKey::MarkdownImageUnsupported,
            )));
        }
    };
    Ok((rgba, format))
}

/// Decodes one EPUB illustration with the same safe bytes/pixels policy as a
/// Markdown image, including SVG with both external-resource resolvers
/// disabled. Animation and multipage semantics remain image-document features;
/// an illustration is a still picture.
pub fn decode_book_illustration(bytes: &[u8]) -> io::Result<image::RgbaImage> {
    decode(bytes).map(|(rgba, _)| rgba)
}

/// Uploads decoded pixels as one UI tile under [`Bitmap`]'s lease.
pub fn bitmap(rgba: image::RgbaImage, cx: &mut App) -> Bitmap {
    Bitmap::new(render(rgba), cx)
}

fn decode(bytes: &[u8]) -> io::Result<(image::RgbaImage, &'static str)> {
    if bytes.len() as u64 > MAX_BYTES {
        return Err(invalid(ImageError::Localized(L10nKey::MarkdownImageLimit)));
    }
    if let Ok(format) = image::guess_format(bytes) {
        let mut reader = image::ImageReader::with_format(std::io::Cursor::new(bytes), format);
        let mut limits = image::Limits::default();
        limits.max_image_width = Some(8192);
        limits.max_image_height = Some(8192);
        limits.max_alloc = Some(MAX_PIXELS * 8);
        reader.limits(limits);
        let rgba = reader
            .decode()
            .map_err(|error| match error {
                // The reader refuses oversized dimensions before allocating;
                // that is the budget speaking, not a corrupt file.
                image::ImageError::Limits(_) => {
                    invalid(ImageError::Localized(L10nKey::MarkdownImageLimit))
                }
                error => invalid(error.to_string()),
            })?
            .into_rgba8();
        if rgba.width() as u64 * rgba.height() as u64 > MAX_PIXELS {
            return Err(invalid(ImageError::Localized(L10nKey::MarkdownImageLimit)));
        }
        let extension = match format {
            image::ImageFormat::Png => "png",
            image::ImageFormat::Jpeg => "jpg",
            image::ImageFormat::Gif => "gif",
            image::ImageFormat::WebP => "webp",
            image::ImageFormat::Bmp => "bmp",
            image::ImageFormat::Tiff => "tiff",
            image::ImageFormat::Ico => "ico",
            _ => {
                return Err(invalid(ImageError::Localized(
                    L10nKey::MarkdownImageUnsupported,
                )));
            }
        };
        return Ok((rgba, extension));
    }
    let (options, blocked) = svg_options(100., 100.);
    let tree = resvg::usvg::Tree::from_data(bytes, &options)
        .map_err(|error| invalid(error.to_string()))?;
    let (rgba, ..) = rasterize_svg(&tree, &blocked, L10nKey::MarkdownImageLimit, None)?;
    Ok((rgba, "svg"))
}

/// One SVG document preview: its pixels and the source's intrinsic size in
/// CSS pixels. `width`/`height` are what fit/actual-size are relative to.
pub struct SvgRaster {
    pub rgba: image::RgbaImage,
    pub width: f32,
    pub height: f32,
}

/// Decodes SVG source for a document preview.
///
/// The root's own dimensions decide the image; a document without usable
/// dimensions gets the browser's 300×150 default viewport, and a `viewBox`
/// supplies a percent-sized root's ratio. Both rules come from Glim's frozen
/// `svg_preview.rs`; the sanitizer, data-image re-encoding, temporary files and
/// browser image document there are deliberately not transplanted. Scripts
/// have no runtime here, and both resource resolvers stay closed, so nested
/// file, network and data images cannot be read.
pub fn decode_svg_source(source: &str) -> io::Result<SvgRaster> {
    let (options, blocked) = svg_options(300., 150.);
    let xml = resvg::usvg::roxmltree::Document::parse_with_options(
        source,
        resvg::usvg::roxmltree::ParsingOptions {
            allow_dtd: true,
            ..Default::default()
        },
    )
    .map_err(|error| invalid(ImageError::Svg(error.to_string())))?;
    let root = xml.root_element();
    let viewport = if root.attribute("viewBox").is_none() {
        // Glim 的独立文档默认视口：不接受 usvg 随后按内容 bbox 收缩尺寸。
        let width = root
            .attribute("width")
            .and_then(|value| svg_length(value, 300.))
            .unwrap_or(300.);
        let height = root
            .attribute("height")
            .and_then(|value| svg_length(value, 150.))
            .unwrap_or(150.);
        Some(
            resvg::usvg::Size::from_wh(width, height)
                .ok_or_else(|| invalid(ImageError::Localized(L10nKey::EditorSvgLimit)))?,
        )
    } else {
        None
    };
    let tree = resvg::usvg::Tree::from_xmltree(&xml, &options)
        .map_err(|error| invalid(ImageError::Svg(error.to_string())))?;
    let (rgba, width, height) = rasterize_svg(&tree, &blocked, L10nKey::EditorSvgLimit, viewport)?;
    Ok(SvgRaster {
        rgba,
        width,
        height,
    })
}

// Glim MIT absolute_length 规则的本地适配，来源见 svg_document/UPSTREAM.toml。
fn svg_length(value: &str, default: f32) -> Option<f32> {
    let value = value.trim();
    let (number, factor) = [
        ("px", 1.),
        ("pt", 96. / 72.),
        ("pc", 16.),
        ("in", 96.),
        ("cm", 96. / 2.54),
        ("mm", 96. / 25.4),
        ("q", 96. / 101.6),
        ("%", default / 100.),
    ]
    .into_iter()
    .find_map(|(suffix, factor)| value.strip_suffix(suffix).map(|number| (number, factor)))
    .unwrap_or((value, 1.));
    let pixels = number.trim().parse::<f32>().ok()? * factor;
    (pixels.is_finite() && pixels > 0.).then_some(pixels)
}

/// Builds parser options that cannot reach the filesystem or the network.
/// The default viewport differs by caller only in its size; SVG user units are
/// CSS pixels in both cases.
fn svg_options(
    default_width: f32,
    default_height: f32,
) -> (
    resvg::usvg::Options<'static>,
    Arc<std::sync::atomic::AtomicBool>,
) {
    // usvg has no JavaScript runtime. Disable *both* resource resolvers: its default
    // string resolver reads local files, including for an SVG read on a remote Host.
    let blocked = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let strings = blocked.clone();
    let data = blocked.clone();
    static FONTS: std::sync::LazyLock<Arc<resvg::usvg::fontdb::Database>> =
        std::sync::LazyLock::new(|| {
            let mut database = resvg::usvg::fontdb::Database::new();
            database.load_system_fonts();
            Arc::new(database)
        });
    let fontdb = FONTS.clone();
    let options = resvg::usvg::Options {
        image_href_resolver: resvg::usvg::ImageHrefResolver {
            resolve_string: Box::new(move |_, _| {
                strings.store(true, std::sync::atomic::Ordering::Relaxed);
                None
            }),
            resolve_data: Box::new(move |_, _, _| {
                data.store(true, std::sync::atomic::Ordering::Relaxed);
                None
            }),
        },
        fontdb,
        default_size: resvg::usvg::Size::from_wh(default_width, default_height)
            .expect("non-empty SVG default viewport"),
        ..Default::default()
    };
    (options, blocked)
}

/// Rasterizes a parsed tree inside the shared preview budget and returns the
/// pixels with the tree's logical size.
fn rasterize_svg(
    tree: &resvg::usvg::Tree,
    blocked: &std::sync::atomic::AtomicBool,
    budget: L10nKey,
    viewport: Option<resvg::usvg::Size>,
) -> io::Result<(image::RgbaImage, f32, f32)> {
    if blocked.load(std::sync::atomic::Ordering::Relaxed) {
        return Err(invalid(ImageError::Localized(
            L10nKey::MarkdownImageExternalReference,
        )));
    }
    let logical = viewport.unwrap_or_else(|| tree.size());
    if !(logical.width().is_finite()
        && logical.height().is_finite()
        && logical.width() > 0.
        && logical.height() > 0.)
    {
        return Err(invalid(ImageError::Localized(budget)));
    }
    let size = logical.to_int_size();
    if size.width() == 0
        || size.height() == 0
        || size.width() > 8192
        || size.height() > 8192
        || size.width() as u64 * size.height() as u64 > MAX_PIXELS
    {
        return Err(invalid(ImageError::Localized(budget)));
    }
    let mut pixmap = resvg::tiny_skia::Pixmap::new(size.width(), size.height())
        .ok_or_else(|| invalid(ImageError::Localized(budget)))?;
    resvg::render(
        tree,
        resvg::tiny_skia::Transform::identity(),
        &mut pixmap.as_mut(),
    );
    let mut rgba = pixmap.take();
    // tiny-skia stores premultiplied RGBA; GPUI consumes straight BGRA.
    for pixel in rgba.as_chunks_mut::<4>().0 {
        if pixel[3] > 0 {
            for channel in 0..3 {
                pixel[channel] = ((pixel[channel] as u32 * 255 + pixel[3] as u32 / 2)
                    / pixel[3] as u32)
                    .min(255) as u8;
            }
        }
    }
    let rgba = image::RgbaImage::from_raw(size.width(), size.height(), rgba)
        .ok_or_else(|| invalid(ImageError::Localized(budget)))?;
    Ok((rgba, logical.width(), logical.height()))
}
fn render(mut rgba: image::RgbaImage) -> Arc<RenderImage> {
    for pixel in rgba.pixels_mut() {
        pixel.0.swap(0, 2);
    }
    Arc::new(RenderImage::new(smallvec::SmallVec::from_elem(
        image::Frame::new(rgba),
        1,
    )))
}

fn fetch(
    url: &url::Url,
    proxy: Option<&str>,
    resolve: Option<&ProxyResolveFn>,
) -> io::Result<Vec<u8>> {
    fetch_with_limit(url, proxy, resolve, MAX_BYTES)
}

/// Fetches one remote picture with an explicit byte budget.
///
/// `resolve` is the desktop proxy resolver cloned by [`ImageHost::load`]. The
/// resolver already applies the desktop precedence (manual `config.json`
/// proxy, then the platform system proxy, then the environment), so its
/// `Option` is applied verbatim — including an explicit `None`, which matters
/// because ureq's default agent configuration otherwise reads the proxy
/// environment variables on its own. Without a resolver the builder is left
/// untouched and ureq's default applies.
fn fetch_with_limit(
    url: &url::Url,
    proxy: Option<&str>,
    resolve: Option<&ProxyResolveFn>,
    max_bytes: u64,
) -> io::Result<Vec<u8>> {
    let mut builder = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(30)))
        .timeout_connect(Some(Duration::from_secs(10)))
        .max_redirects(0)
        .http_status_as_error(false);
    if let Some(resolve) = resolve {
        builder = builder.proxy(resolve(url.as_str(), proxy));
    }
    let agent: ureq::Agent = builder.build().into();
    let mut response = agent
        .get(url.as_str())
        .call()
        .map_err(|error| io::Error::other(error.to_string()))?;
    if !(200..300).contains(&response.status().as_u16()) {
        return Err(io::Error::other(ImageError::Redirect(
            response.status().as_u16(),
        )));
    }
    response
        .body_mut()
        .with_config()
        .limit(max_bytes)
        .read_to_vec()
        .map_err(|error| io::Error::other(error.to_string()))
}

fn store(host: &dyn Host, document: &Path, input: Import) -> io::Result<Vec<String>> {
    // Input OS picker/drop paths are explicitly local; destination paths are *only*
    // interpreted by the captured owning Host. Never reverse these two roles.
    let inputs = match input {
        Import::Bytes(bytes) => vec![bytes],
        Import::Paths(paths) => {
            let local = lyrel_core::host::local::LocalHost::new();
            paths
                .into_iter()
                .map(|path| local.read_file(&path, MAX_BYTES))
                .collect::<io::Result<Vec<_>>>()?
        }
    };
    let prepared = inputs
        .into_iter()
        .map(|bytes| {
            let (_, extension) = decode(&bytes)?;
            Ok((bytes, extension))
        })
        .collect::<io::Result<Vec<_>>>()?;
    if prepared.is_empty() {
        return Err(invalid(ImageError::Localized(
            L10nKey::MarkdownImageUnsupported,
        )));
    }
    let parent = links::parent(host, document)?;
    let dir = host.join(&parent, "assets");
    host.create_dir(&dir, true)?;
    // An existing assets symlink cannot send attachments outside the document directory.
    let actual_parent = host.canonicalize(&parent)?;
    let actual_dir = host.canonicalize(&dir)?;
    if links::parent(host, &actual_dir)? != actual_parent {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            ImageError::Localized(L10nKey::MarkdownImageUnsafeDirectory),
        ));
    }
    let mut created = Vec::new();
    let result = (|| {
        for (bytes, extension) in prepared {
            let name = format!("image-{}.{}", uuid::Uuid::new_v4(), extension);
            let path = host.join(&dir, &name);
            host.create_file_new(&path)?;
            created.push(path.clone());
            host.write_file(&path, &bytes)?;
        }
        Ok(created
            .iter()
            .map(|path| format!("assets/{}", path.file_name().unwrap().to_string_lossy()))
            .collect())
    })();
    if result.is_err() {
        for path in created {
            let _ = host.remove(&path, false);
        }
    }
    result
}

pub fn controls(
    owner: &Entity<ImageHost>,
    editor: &Entity<MarkdownEditor>,
    cx: &App,
) -> AnyElement {
    let host = owner.read(cx);
    let mut surface = v_flex()
        .id(("markdown-resource-status", editor.entity_id()))
        .gap_1()
        .min_w_0()
        .max_h(rems(8.))
        .overflow_y_scroll();
    if host.importing {
        surface = surface.child(
            div()
                .id("markdown-image-writing")
                .test_support()
                .role(gpui_kit::Role::Status)
                .aria_label(t(L10nKey::MarkdownImageWriting))
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(t(L10nKey::MarkdownImageWriting)),
        );
    }
    surface = surface.children(host.notice.clone().map(|notice| {
        div()
            .id("markdown-resource-notice")
            .test_support()
            .role(gpui_kit::Role::Status)
            .aria_label(notice.clone())
            .text_sm()
            .text_color(cx.theme().muted_foreground)
            .child(notice)
    }));
    let mut rows: Vec<_> = host
        .states
        .iter()
        .filter(|(_, state)| !matches!(state, State::Ready))
        .collect();
    rows.sort_by_key(|(source, _)| *source);
    for (source, state) in rows {
        let message = match state {
            State::Loading => {
                let percent = host
                    .progress
                    .get(source)
                    .filter(|(_, total)| *total > 0)
                    .map(|(read, total)| read.saturating_mul(100) / total);
                match percent {
                    Some(percent) => {
                        format!("{} {percent}%", t(L10nKey::EditorImageLoading))
                    }
                    None => t(L10nKey::EditorImageLoading).to_owned(),
                }
            }
            State::Failed(reason) => format!("{}: {reason}", t(L10nKey::EditorImageFailed)),
            State::Ready => unreachable!(),
        };
        let retry = owner.downgrade();
        let target = source.clone();
        surface = surface.child(
            gpui_kit::component::h_flex()
                .gap_2()
                .min_w_0()
                .child(
                    div()
                        .id(SharedString::from(format!("image-status-{source}")))
                        .test_support()
                        .role(gpui_kit::Role::Status)
                        .aria_label(format!("{source} — {message}"))
                        .flex_1()
                        .min_w_0()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(format!("{source} — {message}")),
                )
                .when(!matches!(state, State::Loading), |row| {
                    row.child(
                        Button::new(SharedString::from(format!("image-retry-{source}")))
                            .ghost()
                            .small()
                            .label(t(L10nKey::EditorLoadImages))
                            .on_click(move |_, _, cx| {
                                let _ = retry.update(cx, |host, cx| {
                                    host.states.remove(&target);
                                    host.refresh(cx);
                                    host.notify(cx);
                                });
                            }),
                    )
                }),
        );
    }
    div()
        .relative()
        .min_w_0()
        .child(surface)
        .children(
            host.hover
                .as_ref()
                .filter(|(view, _, _)| *view == editor.entity_id())
                .map(|(_, target, bounds)| {
                    div().absolute().child(
                        gpui_kit::deferred(
                            gpui_kit::anchored()
                                .position(gpui_kit::point(bounds.left(), bounds.bottom()))
                                .snap_to_window_with_margin(cx.theme().font_size * 0.5)
                                .child(
                                    div()
                                        .id("markdown-link-hover")
                                        .test_support()
                                        .role(gpui_kit::Role::Status)
                                        .aria_label(target.clone())
                                        .p_1()
                                        .rounded(cx.theme().radius)
                                        .bg(cx.theme().popover)
                                        .text_sm()
                                        .text_color(cx.theme().popover_foreground)
                                        .child(target.clone()),
                                ),
                        )
                        .with_priority(1),
                    )
                }),
        )
        .into_any_element()
}

fn open_preview_dialog(
    title: String,
    image: Bitmap,
    focus: Option<gpui_kit::FocusHandle>,
    window: &mut Window,
    cx: &mut App,
) -> Entity<ImagePreview> {
    let preview = cx.new(|_| ImagePreview {
        image,
        zoom: 1.,
        scroll: gpui_kit::ScrollHandle::new(),
        drag: None,
    });
    let dialog_preview = preview.clone();
    window.open_dialog(cx, move |dialog, window, cx| {
        let dialog_w = dialog_preview.read(cx).dialog_width(window);
        dialog
            .title(title.clone())
            .centered()
            .w(dialog_w)
            .child(dialog_preview.clone())
            .on_close({
                let focus = focus.clone();
                move |_, window, cx| {
                    if let Some(focus) = &focus {
                        focus.focus(window, cx);
                    }
                }
            })
    });
    preview
}

struct PreviewMetrics {
    scale: f32,
    content_w: gpui_kit::Pixels,
    content_h: gpui_kit::Pixels,
    box_w: gpui_kit::Pixels,
    box_h: gpui_kit::Pixels,
    dialog_w: gpui_kit::Pixels,
}

struct ImagePreview {
    image: Bitmap,
    zoom: f32,
    scroll: gpui_kit::ScrollHandle,
    drag: Option<(
        gpui_kit::Point<gpui_kit::Pixels>,
        gpui_kit::Point<gpui_kit::Pixels>,
    )>,
}

impl ImagePreview {
    fn metrics(&self, window: &Window) -> PreviewMetrics {
        let size = self.image.image.size(0);
        let img_w = (size.width.0 as f32).max(1.);
        let img_h = (size.height.0 as f32).max(1.);
        let rem = window.rem_size();
        let viewport = window.viewport_size();
        let dialog_pad_w = rem * 2.125;
        let max_w = (viewport.width - rem * 4. - dialog_pad_w).max(rem * 12.);
        let max_h = (viewport.height - rem * 12.).max(rem * 4.);
        let fit = (f32::from(max_w) / img_w)
            .min(f32::from(max_h) / img_h)
            .min(1.);
        let scale = fit * self.zoom;
        let content_w = px(img_w * scale);
        let content_h = px(img_h * scale);
        let box_w = content_w.min(max_w);
        let box_h = content_h.min(max_h);
        let min_dialog_w = (rem * 18.).min(viewport.width - rem * 2.);
        let dialog_w = (box_w + dialog_pad_w)
            .max(min_dialog_w)
            .min(viewport.width - rem * 2.);
        PreviewMetrics {
            scale,
            content_w,
            content_h,
            box_w,
            box_h,
            dialog_w,
        }
    }

    fn dialog_width(&self, window: &Window) -> gpui_kit::Pixels {
        self.metrics(window).dialog_w
    }
}

impl gpui_kit::Render for ImagePreview {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let metrics = self.metrics(window);
        let cursor = if self.drag.is_some() {
            gpui_kit::CursorStyle::ClosedHand
        } else {
            gpui_kit::CursorStyle::OpenHand
        };
        v_flex()
            .gap_2()
            .child(
                gpui_kit::component::h_flex()
                    .gap_2()
                    .child(
                        Button::new("image-zoom-out")
                            .ghost()
                            .small()
                            .label("−")
                            .accessibility_label("Zoom image out")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.zoom = (this.zoom / 1.25).max(0.1);
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("image-zoom-fit")
                            .ghost()
                            .small()
                            .label(t(L10nKey::MarkdownImageFit))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.zoom = 1.;
                                this.scroll.set_offset(gpui_kit::point(px(0.), px(0.)));
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("image-zoom-in")
                            .ghost()
                            .small()
                            .label("+")
                            .accessibility_label("Zoom image in")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.zoom = (this.zoom * 1.25).min(16.);
                                cx.notify();
                            })),
                    )
                    .child(format!("{:.0}%", metrics.scale * 100.)),
            )
            .child(
                div()
                    .id("markdown-image-preview")
                    .test_support()
                    .mx_auto()
                    .w(metrics.box_w)
                    .h(metrics.box_h)
                    .overflow_scroll()
                    .track_scroll(&self.scroll)
                    .cursor(cursor)
                    .on_scroll(cx.listener(|this, ev: &gpui_kit::ScrollEvent, window, cx| {
                        let dy = f32::from(ev.delta.pixel_delta(window.line_height()).y);
                        if dy.abs() <= 0.01 {
                            return;
                        }
                        let factor = 1.15_f32.powf((dy / 20.).clamp(-4., 4.));
                        this.zoom = (this.zoom * factor).clamp(0.1, 16.);
                        let max = this.scroll.max_offset();
                        let off = this.scroll.offset();
                        this.scroll.set_offset(gpui_kit::point(
                            off.x.clamp(-max.x, px(0.)),
                            off.y.clamp(-max.y, px(0.)),
                        ));
                        cx.stop_propagation();
                        cx.notify();
                    }))
                    .on_mouse_down(
                        gpui_kit::MouseButton::Left,
                        cx.listener(|this, ev: &gpui_kit::MouseDownEvent, _, cx| {
                            this.drag = Some((ev.position, this.scroll.offset()));
                            cx.stop_propagation();
                            cx.notify();
                        }),
                    )
                    .on_mouse_move(cx.listener(|this, ev: &gpui_kit::MouseMoveEvent, _, cx| {
                        let Some((start_pos, start_offset)) = this.drag else {
                            return;
                        };
                        let delta = ev.position - start_pos;
                        let max = this.scroll.max_offset();
                        let next = gpui_kit::point(
                            (start_offset.x + delta.x).clamp(-max.x, px(0.)),
                            (start_offset.y + delta.y).clamp(-max.y, px(0.)),
                        );
                        this.scroll.set_offset(next);
                        cx.stop_propagation();
                        cx.notify();
                    }))
                    .on_mouse_up(
                        gpui_kit::MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            if this.drag.take().is_some() {
                                cx.notify();
                            }
                        }),
                    )
                    .on_mouse_up_out(
                        gpui_kit::MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            if this.drag.take().is_some() {
                                cx.notify();
                            }
                        }),
                    )
                    .child(
                        div()
                            .id("markdown-image-preview-content")
                            .test_support()
                            .w(metrics.content_w)
                            .h(metrics.content_h)
                            .flex_none()
                            .child(gpui_kit::img(self.image.image.clone()).size_full()),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::{SocketAddr, TcpListener, TcpStream};
    use std::sync::Mutex;
    use std::thread;

    /// One fixture's answer to a single HTTP request.
    struct Reply {
        status: u16,
        headers: Vec<(&'static str, String)>,
        body: Vec<u8>,
    }

    fn ok(body: &[u8]) -> Reply {
        Reply {
            status: 200,
            headers: Vec::new(),
            body: body.to_vec(),
        }
    }

    /// Serves `respond` on loopback for up to `cap` connections, recording
    /// every request line it receives. The caller asserts on the recording
    /// after its fetch returned: both are synchronous, so a recorded line
    /// proves a request happened and a missing one proves it did not.
    fn serve(
        respond: impl Fn(&str) -> Reply + Send + 'static,
        cap: usize,
    ) -> (SocketAddr, Arc<Mutex<Vec<String>>>) {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let addr = listener.local_addr().unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen_thread = seen.clone();
        thread::spawn(move || {
            for stream in listener.incoming().take(cap) {
                let Ok(mut stream) = stream else { continue };
                if let Some(reply) = read_request(&mut stream).map(|line| {
                    seen_thread.lock().unwrap().push(line.clone());
                    respond(&line)
                }) {
                    let _ = write_reply(&mut stream, reply);
                }
            }
        });
        (addr, seen)
    }

    /// Reads one request's head and returns its request line.
    fn read_request(stream: &mut TcpStream) -> Option<String> {
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .ok()?;
        let mut head = Vec::new();
        let mut chunk = [0u8; 1024];
        loop {
            match stream.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => {
                    head.extend_from_slice(&chunk[..n]);
                    if head.ends_with(b"\r\n\r\n") {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        head.split(|byte| *byte == b'\r')
            .next()
            .map(|line| String::from_utf8_lossy(line).into_owned())
            .filter(|line| !line.is_empty())
    }

    fn write_reply(stream: &mut TcpStream, reply: Reply) -> std::io::Result<()> {
        let reason = match reply.status {
            200 => "OK",
            302 => "Found",
            _ => "Reply",
        };
        let mut head = format!("HTTP/1.1 {} {}\r\n", reply.status, reason);
        for (name, value) in &reply.headers {
            head.push_str(&format!("{name}: {value}\r\n"));
        }
        head.push_str(&format!(
            "Content-Length: {}\r\nConnection: close\r\n\r\n",
            reply.body.len()
        ));
        stream.write_all(head.as_bytes())?;
        stream.write_all(&reply.body)
    }

    /// A resolver that always routes through `proxy`, like the production
    /// shell's installed resolver does for a configured proxy.
    fn via(proxy: SocketAddr) -> ProxyResolveFn {
        Arc::new(move |_url, _manual| Some(ureq::Proxy::new(&format!("http://{proxy}")).unwrap()))
    }

    /// A resolver that resolves to no proxy at all.
    fn none() -> ProxyResolveFn {
        Arc::new(|_url: &str, _manual: Option<&str>| -> Option<ureq::Proxy> { None })
    }

    /// A port nothing listens on (the listener is dropped immediately), so a
    /// fetch can name an origin that must never be contacted directly.
    fn free_addr() -> SocketAddr {
        let probe = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let addr = probe.local_addr().unwrap();
        drop(probe);
        addr
    }

    #[test]
    fn fetch_routes_through_the_injected_proxy_resolver() {
        // ureq 3 tunnels HTTP requests through an HTTP proxy via CONNECT.
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let proxy_addr = listener.local_addr().unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen_thread = seen.clone();
        thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept()
                && let Some(connect_line) = read_request(&mut stream)
            {
                seen_thread.lock().unwrap().push(connect_line);
                let _ = stream.write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n");
                let _ = stream.flush();
                if let Some(get_line) = read_request(&mut stream) {
                    seen_thread.lock().unwrap().push(get_line);
                    let _ = write_reply(&mut stream, ok(b"via-proxy"));
                    let _ = stream.flush();
                }
            }
        });
        let origin = free_addr();
        let resolve = via(proxy_addr);
        let url = url::Url::parse(&format!("http://{origin}/image.png")).unwrap();

        let bytes = fetch_with_limit(&url, None, Some(&resolve), MAX_BYTES).unwrap();

        assert_eq!(bytes, b"via-proxy");
        // The proxy received the CONNECT tunnel for the unreachable origin and
        // the tunneled GET, proving the injected resolver routed the fetch.
        assert_eq!(
            *seen.lock().unwrap(),
            vec![
                format!("CONNECT {origin} HTTP/1.1"),
                "GET /image.png HTTP/1.1".to_string(),
            ]
        );
    }

    #[test]
    fn fetch_applies_resolver_none_as_no_proxy() {
        let (addr, seen) = serve(|_| ok(b"direct"), 2);
        let resolve = none();
        let url = url::Url::parse(&format!("http://{addr}/image.png")).unwrap();

        let bytes = fetch_with_limit(&url, None, Some(&resolve), MAX_BYTES).unwrap();

        assert_eq!(bytes, b"direct");
        // Origin form proves the request went straight to the server: had the
        // builder kept ureq's environment default, the ambient CI proxy would
        // have taken over.
        assert_eq!(
            *seen.lock().unwrap(),
            vec!["GET /image.png HTTP/1.1".to_string()]
        );
    }

    #[test]
    fn fetch_refuses_redirects_without_contacting_the_target() {
        let (target_addr, target_seen) = serve(|_| ok(b"forbidden"), 1);
        let (addr, seen) = serve(
            move |_| Reply {
                status: 302,
                headers: vec![("Location", format!("http://{target_addr}/elsewhere"))],
                body: Vec::new(),
            },
            2,
        );
        let resolve = none();
        let url = url::Url::parse(&format!("http://{addr}/image.png")).unwrap();

        let error = fetch_with_limit(&url, None, Some(&resolve), MAX_BYTES).unwrap_err();

        let inner = error.into_inner().expect("a boxed image error");
        assert!(
            matches!(
                *inner.downcast::<ImageError>().expect("an ImageError"),
                ImageError::Redirect(302)
            ),
            "redirect refusal must report the 302"
        );
        assert_eq!(
            *seen.lock().unwrap(),
            vec!["GET /image.png HTTP/1.1".to_string()],
            "exactly the original request may reach the fixture"
        );
        assert!(
            target_seen.lock().unwrap().is_empty(),
            "redirect target must never be contacted"
        );
    }

    #[test]
    fn fetch_enforces_the_response_size_limit() {
        let body = vec![7u8; 1024];
        let (addr, _seen) = serve(
            move |_| Reply {
                status: 200,
                headers: Vec::new(),
                body: body.clone(),
            },
            2,
        );
        let resolve = none();
        let url = url::Url::parse(&format!("http://{addr}/image.png")).unwrap();

        let error = fetch_with_limit(&url, None, Some(&resolve), 512).unwrap_err();

        let is_redirect = error
            .into_inner()
            .and_then(|inner| inner.downcast::<ImageError>().ok())
            .is_some_and(|image_error| matches!(*image_error, ImageError::Redirect(_)));
        assert!(
            !is_redirect,
            "a size violation must not surface as a redirect refusal"
        );
    }

    struct EmptyRoot;
    impl gpui_kit::Render for EmptyRoot {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full()
        }
    }

    fn test_bitmap(width: u32, height: u32, cx: &mut App) -> Bitmap {
        let pixels = vec![255u8; (width * height * 4) as usize];
        let image = image::RgbaImage::from_raw(width, height, pixels).unwrap();
        let render = Arc::new(gpui_kit::RenderImage::new(vec![image::Frame::new(image)]));
        Bitmap::new(render, cx)
    }

    #[gpui_kit::test]
    fn image_preview_adapts_size_and_supports_wheel_zoom_and_drag_pan(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        use gpui_kit::component::Root;
        use gpui_kit::{
            AppContext as _, Bounds, MouseButton, Point, ScrollDelta, ScrollEvent, Size,
            WindowBounds, WindowOptions, test::TestEventExt as _, test::TestWindowExt as _,
        };
        let window = cx.update(|cx| {
            gpui_kit::component::init(cx);
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                        Point::default(),
                        Size::new(px(1200.), px(800.)),
                    ))),
                    ..Default::default()
                },
                |window, cx| {
                    let root = cx.new(|_| EmptyRoot);
                    cx.new(|cx| Root::new(root, window, cx))
                },
            )
            .unwrap()
        });
        let mut vcx = gpui_kit::VisualTestContext::from_window(window.into(), cx);
        let preview = vcx.update(|window, cx| {
            let small = test_bitmap(160, 120, cx);
            let preview = open_preview_dialog("small.png".into(), small, None, window, cx);
            window.render_frame(cx);
            preview
        });
        vcx.run_until_parked();
        vcx.update(|window, cx| {
            window.render_frame(cx);
            let box_bounds = window.find("markdown-image-preview").bounds();
            let dialog_bounds = window.find("surface").bounds();
            assert!(
                f32::from(box_bounds.size.height) < 200.,
                "small 160x120 image preview height should adapt, got {:?}",
                box_bounds.size
            );
            assert!(
                f32::from(dialog_bounds.size.width) < 500.,
                "small 160x120 dialog width should adapt, got {:?}",
                dialog_bounds.size
            );
            window.close_dialog(cx);
            window.render_frame(cx);
        });
        let _ = preview;

        let preview = vcx.update(|window, cx| {
            let wide = test_bitmap(1000, 300, cx);
            let preview = open_preview_dialog("wide.png".into(), wide, None, window, cx);
            window.render_frame(cx);
            preview
        });
        vcx.run_until_parked();
        let (preview_center, initial_size) = vcx.update(|window, cx| {
            window.render_frame(cx);
            let box_bounds = window.find("markdown-image-preview").bounds();
            let content_size = window.find("markdown-image-preview-content").bounds().size;
            assert!(
                f32::from(box_bounds.size.width) >= 980.,
                "wide 1000x300 preview should expand past 50rem to fit image, got {:?}",
                box_bounds.size
            );
            assert!(
                f32::from(box_bounds.size.height) < 400.,
                "wide 1000x300 preview height should adapt to ~300px, got {:?}",
                box_bounds.size
            );
            (box_bounds.center(), content_size)
        });

        vcx.simulate_mouse_move(preview_center, None, Default::default());
        vcx.simulate_event(ScrollEvent {
            position: preview_center,
            delta: ScrollDelta::Lines(gpui_kit::point(0., 2.)),
            ..Default::default()
        });
        vcx.run_until_parked();
        let zoomed_size = vcx.update(|window, cx| {
            window.render_frame(cx);
            window.find("markdown-image-preview-content").bounds().size
        });
        assert!(
            zoomed_size.width > initial_size.width && zoomed_size.height > initial_size.height,
            "scroll wheel up must zoom in: before={initial_size:?}, after={zoomed_size:?}"
        );

        vcx.simulate_event(ScrollEvent {
            position: preview_center,
            delta: ScrollDelta::Lines(gpui_kit::point(0., -1.)),
            ..Default::default()
        });
        vcx.run_until_parked();
        let zoomed_out_size = vcx.update(|window, cx| {
            window.render_frame(cx);
            window.find("markdown-image-preview-content").bounds().size
        });
        assert!(
            zoomed_out_size.width < zoomed_size.width
                && zoomed_out_size.height < zoomed_size.height,
            "scroll wheel down must zoom out: zoomed={zoomed_size:?}, out={zoomed_out_size:?}"
        );

        vcx.simulate_event(ScrollEvent {
            position: preview_center,
            delta: ScrollDelta::Lines(gpui_kit::point(0., 3.)),
            ..Default::default()
        });
        vcx.run_until_parked();
        vcx.update(|window, cx| window.render_frame(cx));

        let before_pan = preview.read_with(&vcx, |p, _| p.scroll.offset());
        vcx.update(|window, cx| {
            window.pointer_move(preview_center, None, cx);
            window.pointer_down(preview_center, MouseButton::Left, cx);
            window.pointer_move(
                preview_center - gpui_kit::point(px(60.), px(40.)),
                Some(MouseButton::Left),
                cx,
            );
            window.pointer_up(
                preview_center - gpui_kit::point(px(60.), px(40.)),
                MouseButton::Left,
                cx,
            );
            window.render_frame(cx);
        });
        let after_pan = preview.read_with(&vcx, |p, _| p.scroll.offset());
        assert_ne!(
            after_pan, before_pan,
            "dragging zoomed image preview must pan scroll offset"
        );
    }
}
