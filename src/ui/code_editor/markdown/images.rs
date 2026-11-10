//! 图片 I/O、权限和异步生命周期只归工作台；editor 仅消费 bitmap 和发出意图。
use super::super::*;
use super::links::{self, Target};
use gpui_kit::{AnyWindowHandle, App, ClipboardEntry, EntityId, RenderImage, WeakEntity};
use gpui_kit::component::{button::{Button, ButtonVariants as _}, ActiveTheme as _, Sizable as _, WindowExt as _};
use lyrel_core::host::Host;
use lyrel_markdown_editor::{Document, DocumentEvent, EditorEvent as NativeEvent, EditorState as MarkdownEditor};
use std::{cell::RefCell, io, rc::Rc, time::Duration};

const MAX_BYTES: u64 = 32 * 1024 * 1024;
const MAX_PIXELS: u64 = 16 * 1024 * 1024;
const CACHE_BYTES: u64 = 256 * 1024 * 1024;

type Signature = (HostId, PathBuf);
type Images = Rc<RefCell<HashMap<String, Arc<RenderImage>>>>;
#[derive(Clone)]
enum State { Loading, Blocked, Ready, Failed(String) }
#[derive(Clone)]
struct Capture {
    host: SharedHost,
    path: PathBuf,
    revision: u64,
    selection: std::ops::Range<usize>,
}

pub(crate) struct ImageHost {
    app: WeakEntity<LyrelApp>,
    id: BufferId,
    window: AnyWindowHandle,
    document: Entity<Document>,
    signature: Option<Signature>,
    generation: u64,
    sources: HashSet<String>,
    scanned_revision: Option<u64>,
    images: Images,
    states: HashMap<String, State>,
    allowed: HashSet<String>,
    views: HashMap<EntityId, (WeakEntity<MarkdownEditor>, Subscription)>,
    hover: Option<SharedString>,
    notice: Option<SharedString>,
    refresh_scheduled: bool,
    importing: bool,
    _document: Subscription,
}

impl ImageHost {
    pub(crate) fn new(app: WeakEntity<LyrelApp>, id: BufferId, editor: &Entity<MarkdownEditor>, window: &Window, cx: &mut Context<Self>) -> Self {
        let document = editor.read(cx).document().clone();
        let subscription = cx.subscribe(&document, |this, _, _: &DocumentEvent, cx| this.refresh(cx));
        Self { app, id, window: window.window_handle(), document, signature: None, generation: 0,
            sources: HashSet::new(), scanned_revision: None, images: Rc::new(RefCell::new(HashMap::new())), states: HashMap::new(),
            allowed: HashSet::new(), views: HashMap::new(), hover: None, refresh_scheduled: false,
            importing: false, notice: None, _document: subscription }
    }

    /// Every presentation installs its own event subscription and shares only Host bitmap data.
    pub(crate) fn attach(owner: &Entity<Self>, editor: &Entity<MarkdownEditor>, cx: &mut App) {
        owner.update(cx, |this, cx| {
            if !this.views.contains_key(&editor.entity_id()) {
                let images = this.images.clone();
                editor.update(cx, |editor, _| {
                    editor.set_block_image_provider(move |src| images.borrow().get(src).cloned());
                    editor.set_host_link_navigation(!editor.is_source_only());
                });
                let subscription = cx.subscribe(editor, |this, source, event: &NativeEvent, cx| this.event(source, event, cx));
                this.views.insert(editor.entity_id(), (editor.downgrade(), subscription));
            }
            // Render may observe a save-as/rename without a source edit. Queue a bounded,
            // idempotent identity refresh, outside the borrowed workbench render/update.
            if !this.refresh_scheduled {
                this.refresh_scheduled = true;
                cx.spawn(async move |this, cx| {
                    let _ = this.update(cx, |this, cx| { this.refresh_scheduled = false; this.refresh(cx); });
                }).detach();
            }
        });
    }

    fn identity(&self, cx: &App) -> Option<(SharedHost, Option<PathBuf>)> {
        let app = self.app.upgrade()?;
        let file = app.read(cx).editor.buffers.get(self.id)?;
        let source = file.input.markdown()?;
        if source.read(cx).document() != &self.document { return None; }
        Some((file.host.clone(), file.untitled.is_none().then(|| file.path.clone())))
    }
    fn capture(&self, editor: &Entity<MarkdownEditor>, cx: &App) -> Option<Capture> {
        let (host, path) = self.identity(cx)?;
        Some(Capture { host, path: path?, revision: self.document.read(cx).revision(), selection: editor.read(cx).selected_range() })
    }
    fn current(&self, capture: &Capture, cx: &App) -> bool {
        self.identity(cx).is_some_and(|(host, path)| host.id() == capture.host.id() && path.as_ref() == Some(&capture.path))
            && self.document.read(cx).revision() == capture.revision
    }
    fn notify(&self, cx: &mut Context<Self>) {
        for (editor, _) in self.views.values() {
            let _ = editor.update(cx, |_, cx| cx.notify());
        }
        let _ = self.app.update(cx, |_, cx| cx.notify());
        cx.notify();
    }
    fn message(&mut self, message: impl Into<SharedString>, cx: &mut Context<Self>) {
        let message = message.into();
        self.notice = Some(message.clone());
        let _ = self.window.update(cx, |_, window, cx| window.notify_app(message.to_string(), cx));
        self.notify(cx);
    }
    fn event(&mut self, editor: Entity<MarkdownEditor>, event: &NativeEvent, cx: &mut Context<Self>) {
        match event {
            NativeEvent::OpenLink(target) => {
                let app = self.app.clone();
                let id = self.id;
                let _ = self.window.update(cx, |_, window, cx| {
                    let _ = app.update(cx, |app, cx| links::follow(app, id, &editor, target, window, cx));
                });
            }
            NativeEvent::HoverLink(target) => {
                self.hover = target.as_ref().and_then(|(hit, _)| match hit { lyrel_markdown_editor::LinkHit::Url(url) => Some(url.clone().into()), _ => None });
                self.notify(cx);
            }
            NativeEvent::PreviewImage(source) => self.preview(&editor, source, cx),
            NativeEvent::PasteAttachments(item) => {
                let Some(capture) = self.capture(&editor, cx) else { self.message(t(L10nKey::MarkdownImageChooseLocation), cx); return; };
                if let Some(image) = item.entries().iter().find_map(|entry| match entry { ClipboardEntry::Image(image) => Some(image), _ => None }) {
                    self.import(editor, capture, Import::Bytes(image.bytes.clone()), cx);
                } else {
                    let paths: Vec<_> = item.entries().iter().filter_map(|entry| match entry { ClipboardEntry::ExternalPaths(paths) => Some(paths.paths()), _ => None }).flatten().cloned().collect();
                    self.import(editor, capture, Import::Paths(paths), cx);
                }
            }
            NativeEvent::InsertImages(paths) => self.insert(&editor, paths.clone(), cx),
            _ => {}
        }
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        self.views.retain(|_, (view, _)| view.upgrade().is_some());
        let Some((host, path)) = self.identity(cx) else { return; };
        let signature = path.as_ref().map(|path| (host.id(), path.clone()));
        let changed_identity = signature != self.signature;
        let revision = self.document.read(cx).revision();
        if !changed_identity && self.scanned_revision == Some(revision) && self.sources.len() == self.states.len() { return; }
        self.scanned_revision = Some(revision);
        if changed_identity {
            self.signature = signature;
            self.generation = self.generation.wrapping_add(1);
            self.images.borrow_mut().clear();
            self.states.clear();
            self.allowed.clear();
            for (view, _) in self.views.values() {
                let _ = view.update(cx, |view, cx| view.refresh_resource_layout(cx));
            }
        }
        let sources: HashSet<_> = self.views.values().find_map(|(view, _)| view.upgrade()).filter(|view| !view.read(cx).is_source_only()).map(|view| view.read(cx).image_sources().into_iter().collect()).unwrap_or_default();
        let changed = sources != self.sources || changed_identity;
        self.sources = sources;
        self.states.retain(|source, _| self.sources.contains(source));
        self.images.borrow_mut().retain(|source, _| self.sources.contains(source));
        self.allowed.retain(|source| self.sources.contains(source));
        let pending: Vec<_> = self.sources.iter().filter(|source| !self.states.contains_key(*source)).cloned().collect();
        for source in pending { self.load(host.clone(), path.as_deref(), source, cx); }
        if changed { self.notify(cx); }
    }
    fn load(&mut self, host: SharedHost, document: Option<&Path>, source: String, cx: &mut Context<Self>) {
        let target = match links::resolve(&*host, document, destination(&source)) {
            Ok(Target::External(url)) if matches!(url.scheme(), "http" | "https") => {
                if !self.allowed.contains(&source) { self.states.insert(source, State::Blocked); return; }
                Resource::External(url)
            }
            Ok(Target::File(path, _)) => Resource::File(path),
            Ok(_) => { self.states.insert(source, State::Failed(t(L10nKey::MarkdownLinkUnsupportedScheme).into())); return; }
            Err(reason) => { self.states.insert(source, State::Failed(t(reason).into())); return; }
        };
        let proxy = cx.global::<crate::core::config::Config>().http_proxy.clone();
        let generation = self.generation;
        let signature = self.signature.clone();
        self.states.insert(source.clone(), State::Loading);
        HostOps::run(host, cx, move |h| -> io::Result<Arc<RenderImage>> {
            let bytes = match target { Resource::File(path) => h.read_file(&path, MAX_BYTES)?, Resource::External(url) => fetch(&url, proxy.as_deref())? };
            decode(&bytes).map(|(rgba, _)| render(rgba))
        }, move |this, result, cx| {
            let current = this.identity(cx).and_then(|(h,p)| p.map(|p| (h.id(),p)));
            if this.generation != generation || this.signature != signature || current != signature || !this.sources.contains(&source) { return; }
            match result {
                Ok(image) => {
                    let used: u64 = this.images.borrow().values().map(|image| { let size = image.size(0); size.width.0 as u64 * size.height.0 as u64 * 4 }).sum();
                    let size = image.size(0);
                    if used + size.width.0 as u64 * size.height.0 as u64 * 4 > CACHE_BYTES {
                        this.states.insert(source, State::Failed(t(L10nKey::MarkdownImageLimit).into()));
                    } else {
                        this.images.borrow_mut().insert(source.clone(), image);
                        this.states.insert(source, State::Ready);
                    }
                }
                Err(error) => { this.states.insert(source, State::Failed(error.to_string())); }
            }
            for (view, _) in this.views.values() {
                let _ = view.update(cx, |view, cx| view.refresh_resource_layout(cx));
            }
            this.notify(cx);
        });
    }

    fn insert(&mut self, editor: &Entity<MarkdownEditor>, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        let Some(capture) = self.capture(editor, cx) else { self.message(t(L10nKey::MarkdownImageChooseLocation), cx); return; };
        if !paths.is_empty() { self.import(editor.clone(), capture, Import::Paths(paths), cx); return; }
        let picker = cx.prompt_for_paths(gpui_kit::PathPromptOptions { files: true, directories: false, multiple: true, prompt: Some(t(L10nKey::MarkdownInsertImage).into()) });
        let editor = editor.downgrade();
        cx.spawn(async move |this, cx| {
            match picker.await {
                Ok(Ok(Some(paths))) => {
                    let _ = this.update(cx, |this, cx| {
                        if let Some(editor) = editor.upgrade() { this.import(editor, capture, Import::Paths(paths), cx); }
                    });
                }
                Ok(Err(error)) => { let _ = this.update(cx, |this, cx| this.message(error.to_string(), cx)); }
                _ => {}
            }
        }).detach();
    }

    fn import(&mut self, editor: Entity<MarkdownEditor>, capture: Capture, input: Import, cx: &mut Context<Self>) {
        self.notice = None;
        if self.importing { self.message(t(L10nKey::MarkdownImageWriting), cx); return; }
        if !self.current(&capture, cx) || editor.read(cx).selected_range() != capture.selection {
            self.message(t(L10nKey::MarkdownImageChanged), cx); return;
        }
        self.importing = true;
        self.notify(cx);
        let path = capture.path.clone();
        let request = capture.clone();
        let editor = editor.downgrade();
        HostOps::run(capture.host.clone(), cx, move |h| store(h, &path, input), move |this, result, cx| {
            this.importing = false;
            match result {
                Ok(sources) => {
                    let Some(editor) = editor.upgrade() else { return; };
                    if !this.current(&request, cx) || editor.read(cx).selected_range() != request.selection {
                        this.message(t(L10nKey::MarkdownImageSelectionChanged), cx);
                    } else {
                        let mut text = String::new();
                        let view = editor.read(cx);
                        if request.selection.start > 0 && !view.text()[..request.selection.start].ends_with('\n') { text.push('\n'); }
                        for source in sources { text.push_str(&format!("![]({source})\n")); }
                        editor.update(cx, |view, cx| {
                            view.replace_range(request.selection, &text, cx);
                        });
                    }
                }
                Err(error) => this.message(format!("{}: {error}", t(L10nKey::MarkdownImageWriteFailed)), cx),
            }
            this.notify(cx);
        });
    }

    fn preview(&mut self, editor: &Entity<MarkdownEditor>, source: &str, cx: &mut Context<Self>) {
        let Some(image) = self.images.borrow().get(source).cloned() else {
            self.message(t(L10nKey::EditorImageFailed), cx); return;
        };
        let focus = editor.read(cx).focus_handle(cx);
        let preview = cx.new(|_| ImagePreview { image, zoom: 1., scroll: gpui_kit::ScrollHandle::new() });
        let title = source.to_owned();
        let _ = self.window.update(cx, |_, window, cx| {
            window.open_dialog(cx, move |dialog, window, _| dialog.title(title.clone()).w(window.rem_size() * 50.)
                .child(preview.clone()).on_close({ let focus = focus.clone(); move |_, window, cx| focus.focus(window, cx) }));
        });
    }

    #[cfg(any(test, feature = "native-markdown-tests"))]
    pub(super) fn is_loaded(&self, source: &str) -> bool { matches!(self.states.get(source), Some(State::Ready)) }
}

enum Resource { File(PathBuf), External(url::Url) }

fn destination(source: &str) -> &str {
    let source = source.trim();
    if let Some(angle) = source.strip_prefix('<') && let Some(end) = angle.find('>') { return &angle[..end]; }
    for marker in [" \"", " '"] {
        if let Some((path, title)) = source.split_once(marker) && title.ends_with(&marker[1..]) { return path; }
    }
    source
}
enum Import { Bytes(Vec<u8>), Paths(Vec<PathBuf>) }

fn invalid(message: impl Into<String>) -> io::Error { io::Error::new(io::ErrorKind::InvalidData, message.into()) }
fn decode(bytes: &[u8]) -> io::Result<(image::RgbaImage, &'static str)> {
    if bytes.len() as u64 > MAX_BYTES { return Err(invalid(t(L10nKey::MarkdownImageLimit))); }
    if let Ok(format) = image::guess_format(bytes) {
        let mut reader = image::ImageReader::with_format(std::io::Cursor::new(bytes), format);
        let mut limits = image::Limits::default();
        limits.max_image_width = Some(8192);
        limits.max_image_height = Some(8192);
        limits.max_alloc = Some(MAX_PIXELS * 8);
        reader.limits(limits);
        let rgba = reader.decode().map_err(|error| invalid(error.to_string()))?.into_rgba8();
        if rgba.width() as u64 * rgba.height() as u64 > MAX_PIXELS { return Err(invalid(t(L10nKey::MarkdownImageLimit))); }
        let extension = match format { image::ImageFormat::Png => "png", image::ImageFormat::Jpeg => "jpg", image::ImageFormat::Gif => "gif", image::ImageFormat::WebP => "webp", image::ImageFormat::Bmp => "bmp", image::ImageFormat::Tiff => "tiff", image::ImageFormat::Ico => "ico", _ => return Err(invalid(t(L10nKey::MarkdownImageUnsupported))) };
        return Ok((rgba, extension));
    }
    // usvg has no JavaScript runtime. Disable *both* resource resolvers: its default
    // string resolver reads local files, including for an SVG read on a remote Host.
    let blocked = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let strings = blocked.clone();
    let data = blocked.clone();
    static FONTS: std::sync::LazyLock<Arc<resvg::usvg::fontdb::Database>> = std::sync::LazyLock::new(|| {
        let mut database = resvg::usvg::fontdb::Database::new();
        database.load_system_fonts();
        Arc::new(database)
    });
    let fontdb = FONTS.clone();
    let options = resvg::usvg::Options { image_href_resolver: resvg::usvg::ImageHrefResolver {
        resolve_string: Box::new(move |_, _| { strings.store(true, std::sync::atomic::Ordering::Relaxed); None }),
        resolve_data: Box::new(move |_, _, _| { data.store(true, std::sync::atomic::Ordering::Relaxed); None }),
    }, fontdb, ..Default::default() };
    let tree = resvg::usvg::Tree::from_data(bytes, &options).map_err(|error| invalid(error.to_string()))?;
    if blocked.load(std::sync::atomic::Ordering::Relaxed) { return Err(invalid(t(L10nKey::MarkdownImageExternalReference))); }
    let size = tree.size().to_int_size();
    if size.width() > 8192 || size.height() > 8192 || size.width() as u64 * size.height() as u64 > MAX_PIXELS { return Err(invalid(t(L10nKey::MarkdownImageLimit))); }
    let mut pixmap = resvg::tiny_skia::Pixmap::new(size.width(), size.height()).ok_or_else(|| invalid(t(L10nKey::MarkdownImageLimit)))?;
    resvg::render(&tree, resvg::tiny_skia::Transform::identity(), &mut pixmap.as_mut());
    let mut rgba = pixmap.take();
    // tiny-skia stores premultiplied RGBA; GPUI consumes straight BGRA.
    for pixel in rgba.chunks_exact_mut(4) {
        if pixel[3] > 0 { for channel in 0..3 { pixel[channel] = ((pixel[channel] as u32 * 255 + pixel[3] as u32 / 2) / pixel[3] as u32).min(255) as u8; } }
    }
    Ok((image::RgbaImage::from_raw(size.width(), size.height(), rgba).ok_or_else(|| invalid(t(L10nKey::MarkdownImageUnsupported)))?, "svg"))
}
fn render(mut rgba: image::RgbaImage) -> Arc<RenderImage> {
    for pixel in rgba.pixels_mut() { pixel.0.swap(0, 2); }
    Arc::new(RenderImage::new(smallvec::SmallVec::from_elem(image::Frame::new(rgba), 1)))
}

fn fetch(url: &url::Url, proxy: Option<&str>) -> io::Result<Vec<u8>> {
    let mut builder = ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(30))).timeout_connect(Some(Duration::from_secs(10))).max_redirects(0).http_status_as_error(false);
    if let Some(proxy) = lyrel_core::daemon::install::proxy::resolve(url.as_str(), proxy) { builder = builder.proxy(Some(proxy)); }
    let agent: ureq::Agent = builder.build().into();
    let mut response = agent.get(url.as_str()).call().map_err(|error| io::Error::other(error.to_string()))?;
    if !(200..300).contains(&response.status().as_u16()) { return Err(io::Error::other(format!("HTTP {} — {}", response.status(), t(L10nKey::MarkdownImageRedirectBlocked)))); }
    response.body_mut().with_config().limit(MAX_BYTES).read_to_vec().map_err(|error| io::Error::other(error.to_string()))
}

fn store(host: &dyn Host, document: &Path, input: Import) -> io::Result<Vec<String>> {
    // Input OS picker/drop paths are explicitly local; destination paths are *only*
    // interpreted by the captured owning Host. Never reverse these two roles.
    let inputs = match input {
        Import::Bytes(bytes) => vec![bytes],
        Import::Paths(paths) => {
            let local = lyrel_core::host::local::LocalHost::new();
            paths.into_iter().map(|path| local.read_file(&path, MAX_BYTES)).collect::<io::Result<Vec<_>>>()?
        }
    };
    let prepared = inputs.into_iter().map(|bytes| { let (_, extension) = decode(&bytes)?; Ok((bytes, extension)) }).collect::<io::Result<Vec<_>>>()?;
    let parent = links::parent(host, document)?;
    let dir = host.join(&parent, "assets");
    host.create_dir(&dir, true)?;
    // An existing assets symlink cannot send attachments outside the document directory.
    let actual_parent = host.canonicalize(&parent)?;
    let actual_dir = host.canonicalize(&dir)?;
    if links::parent(host, &actual_dir)? != actual_parent { return Err(io::Error::new(io::ErrorKind::PermissionDenied, t(L10nKey::MarkdownImageUnsafeDirectory))); }
    let mut created = Vec::new();
    let result = (|| {
        for (bytes, extension) in prepared {
            let name = format!("image-{}.{}", uuid::Uuid::new_v4(), extension);
            let path = host.join(&dir, &name);
            host.create_file_new(&path)?;
            created.push(path.clone());
            host.write_file(&path, &bytes)?;
        }
        Ok(created.iter().map(|path| format!("assets/{}", path.file_name().unwrap().to_string_lossy())).collect())
    })();
    if result.is_err() { for path in created { let _ = host.remove(&path, false); } }
    result
}

pub(crate) fn controls(owner: &Entity<ImageHost>, editor: &Entity<MarkdownEditor>, cx: &App) -> AnyElement {
    let host = owner.read(cx);
    let source = editor.downgrade();
    let insert = owner.downgrade();
    let mut surface = v_flex().id(("markdown-resource-status", editor.entity_id())).gap_1().min_w_0().max_h(rems(8.)).overflow_y_scroll().child(
        gpui_kit::component::h_flex().gap_2().child(Button::new(("markdown-insert-image", editor.entity_id())).ghost().small()
            .label(t(if host.importing { L10nKey::MarkdownImageWriting } else { L10nKey::MarkdownInsertImage })).on_click(move |_, _, cx| {
                if let Some(editor) = source.upgrade() { let _ = insert.update(cx, |host, cx| host.insert(&editor, Vec::new(), cx)); }
            })).children(host.hover.clone().map(|target| div().id("markdown-link-hover").test_support().role(gpui_kit::Role::Status).aria_label(target.clone()).text_sm().text_color(cx.theme().muted_foreground).child(target)))
    );
    surface = surface.children(host.notice.clone().map(|notice| div().id("markdown-resource-notice").test_support().role(gpui_kit::Role::Status).aria_label(notice.clone()).text_sm().text_color(cx.theme().muted_foreground).child(notice)));
    let mut rows: Vec<_> = host.states.iter().filter(|(_, state)| !matches!(state, State::Ready)).collect();
    rows.sort_by_key(|(source, _)| *source);
    for (source, state) in rows {
        let message = match state { State::Loading => t(L10nKey::EditorImageLoading).to_owned(), State::Blocked => t(L10nKey::MarkdownImageNetworkPolicy).to_owned(), State::Failed(reason) => format!("{}: {reason}", t(L10nKey::EditorImageFailed)), State::Ready => unreachable!() };
        let retry = owner.downgrade();
        let target = source.clone();
        surface = surface.child(gpui_kit::component::h_flex().gap_2().min_w_0().child(div().id(SharedString::from(format!("image-status-{source}"))).test_support().role(gpui_kit::Role::Status).aria_label(format!("{source} — {message}")).flex_1().min_w_0().text_sm().text_color(cx.theme().muted_foreground).child(format!("{source} — {message}")))
            .when(!matches!(state, State::Loading), |row| row.child(Button::new(SharedString::from(format!("image-retry-{source}"))).ghost().small().label(t(L10nKey::EditorLoadImages)).on_click(move |_, _, cx| {
                let _ = retry.update(cx, |host, cx| { host.allowed.insert(target.clone()); host.states.remove(&target); host.refresh(cx); host.notify(cx); });
            })));
    }
    surface.into_any_element()
}

struct ImagePreview { image: Arc<RenderImage>, zoom: f32, scroll: gpui_kit::ScrollHandle }
impl gpui_kit::Render for ImagePreview {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let size = self.image.size(0);
        let max_w = (window.viewport_size().width - window.rem_size() * 6.).max(window.rem_size());
        let max_h = (window.viewport_size().height - window.rem_size() * 14.).max(window.rem_size() * 4.);
        let fit = (f32::from(max_w) / size.width.0 as f32).min(f32::from(max_h) / size.height.0 as f32).min(1.);
        let scale = fit * self.zoom;
        v_flex().gap_2().child(gpui_kit::component::h_flex().gap_2()
            .child(Button::new("image-zoom-out").ghost().small().label("−").accessibility_label("Zoom image out").on_click(cx.listener(|this, _, _, cx| { this.zoom = (this.zoom / 1.25).max(0.1); cx.notify(); })))
            .child(Button::new("image-zoom-fit").ghost().small().label(t(L10nKey::MarkdownImageFit)).on_click(cx.listener(|this, _, _, cx| { this.zoom = 1.; this.scroll.set_offset(gpui_kit::point(px(0.), px(0.))); cx.notify(); })))
            .child(Button::new("image-zoom-in").ghost().small().label("+").accessibility_label("Zoom image in").on_click(cx.listener(|this, _, _, cx| { this.zoom = (this.zoom * 1.25).min(16.); cx.notify(); })))
            .child(format!("{:.0}%", scale * 100.)))
            .child(div().id("markdown-image-preview").h(max_h).max_w(max_w).overflow_scroll().track_scroll(&self.scroll)
                .child(div().id("markdown-image-preview-content").test_support().w(px(size.width.0 as f32 * scale)).h(px(size.height.0 as f32 * scale)).flex_none().child(gpui_kit::img(self.image.clone()).size_full()))
            )
    }
}
