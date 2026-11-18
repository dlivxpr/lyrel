use super::pdf_document::{PdfDocument, PdfLoaded, PdfView};
use super::sqlite_document::{SqliteDocument, SqliteView};
use super::*;
use lyrel_core::core::session::WorkspaceId;
use lyrel_core::host::sqlite::{SqliteCancel, SqliteOpened};
const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024;
/// Pictures are not text: the workbench image budget is `markdown::images`'.
const IMAGE_MAX_BYTES: u64 = markdown::images::MAX_BYTES;
use super::book_document::{self, BookDocument, BookOpen, LoadedBook};
use super::book_view::BookView;
use super::drawio_document::{DrawioDocument, DrawioView};
use lyrel_code_editor::drawio;
use std::sync::Arc;
/// PDFs share the owning Host's bounded versioned read and the same 32 MiB
/// input ceiling; parsing never raises the text or control-frame limits.
const PDF_MAX_BYTES: u64 = markdown::images::MAX_BYTES;
use super::image_document::{ImageDocument, ImageOutcome, ImageView};
use super::toml_document::{TomlDocument, TomlMode, TomlView};
use super::xml_document::{XmlDocument, XmlMode, XmlView};
use super::yaml_document::{YamlDocument, YamlMode, YamlView};
use lyrel_code_editor::json::projection::JsonFormat;
use markdown::images;
use markdown::images::ImageFormat;

const RELOAD_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(200);

/// How far up the tree to look for `.editorconfig` files. Each level is one
/// round trip on a remote host, and a project nested deeper than this without
/// a `root = true` somewhere above it is not one worth stalling an open for.
const EDITORCONFIG_DEPTH: usize = 16;

/// Document identity and I/O lifetime. Tabs only retain presentation relations;
/// replacing an editing presentation does not replace the document.
/// What one deferred draw.io save turned into: a confirmed engine revision
/// ready to write, a refusal that must fail visibly, or a flush that never
/// arrived.
pub(super) enum DrawioFlushReadiness {
    Confirmed {
        force: bool,
        target: Option<PathBuf>,
    },
    Rejected {
        reason: SharedString,
    },
    Expired,
}

/// Every live draw.io presentation for one buffer, primary first. The weak
/// ticket uses this shape too; strong handles here are only for one frame.
fn drawio_views(file: &OpenFile) -> Vec<Entity<DrawioView>> {
    let mut views = Vec::new();
    if let Some(primary) = file.input.drawio() {
        views.push(primary.clone());
    }
    for (_, _, view) in file.presentations.drawios() {
        if !views.contains(&view) {
            views.push(view);
        }
    }
    views
}

/// Whether any canvas of this buffer holds a model the document refused.
fn drawio_has_unconfirmed_changes(file: &OpenFile, cx: &gpui_kit::App) -> bool {
    drawio_views(file)
        .iter()
        .any(|view| view.read(cx).has_unconfirmed_changes())
}

pub(super) struct EditorBuffers {
    files: Vec<OpenFile>,
    next_untitled: u32,
    open_seq: u64,
    context_epoch: u64,
    pending: HashMap<(TabId, HostId, PathBuf), u64>,
    foreground: HashMap<TabId, u64>,
    watch: Option<Arc<WatchSub>>,
    watch_host: Option<SharedHost>,
    watch_generation: u64,
    watch_refresh: bool,
    watch_opening: bool,
    watch_busy: bool,
    watch_dirty: bool,
    watched_dirs: HashSet<PathBuf>,
    watched_files: HashSet<PathBuf>,
    events_tx: smol::channel::Sender<WatchBatch>,
}

struct WatchBatch {
    generation: u64,
    paths: Vec<PathBuf>,
}

/// The workbench supplies only identities and verifies they still exist.
#[derive(Clone, Copy)]
pub(super) struct OpenTarget {
    pub(super) workspace: WorkspaceId,
    pub(super) tab: TabId,
    pub(super) group: Option<split::GroupId>,
    pub(super) window_host: HostId,
    pub(super) origin: Option<(BufferId, u64)>,
    pub(super) origin_epoch: Option<(HostId, u64)>,
}

pub(super) enum OpenResult {
    Opened {
        id: BufferId,
        created: bool,
        front: bool,
    },
    Failed {
        host: HostId,
        error: EditorOpenError,
    },
}

type Access<E> = fn(&mut E) -> &mut EditorBuffers;
type Edited<E> = fn(&mut E, BufferId, &mut Context<E>);
type ValidTarget<E> = fn(&E, OpenTarget, &gpui_kit::App) -> bool;
type Opened<E> = fn(&mut E, OpenTarget, OpenResult, &mut Window, &mut Context<E>);

impl EditorBuffers {
    pub(super) fn new<E: 'static>(
        window: &Window,
        cx: &mut Context<E>,
        access: Access<E>,
        edited: Edited<E>,
    ) -> Self {
        let (tx, rx) = smol::channel::unbounded::<WatchBatch>();
        cx.spawn_in(window, async move |view, cx| {
            while let Ok(first) = rx.recv().await {
                cx.background_executor().timer(RELOAD_DEBOUNCE).await;
                let mut batches = vec![first];
                while let Ok(more) = rx.try_recv() {
                    batches.push(more);
                }
                if view
                    .update_in(cx, |view, window, cx| {
                        let owner = access(view);
                        let mut paths = HashSet::new();
                        for batch in batches {
                            if batch.generation == owner.watch_generation {
                                paths.extend(batch.paths);
                            }
                        }
                        for path in paths {
                            if access(view).watched_files.contains(&path) {
                                access(view).external_change(&path, window, cx, access, edited);
                            }
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        Self {
            files: Vec::new(),
            next_untitled: 1,
            open_seq: 0,
            context_epoch: 0,
            pending: HashMap::new(),
            foreground: HashMap::new(),
            watch: None,
            watch_host: None,
            watch_generation: 0,
            watch_refresh: false,
            watch_opening: false,
            watch_busy: false,
            watch_dirty: false,
            watched_dirs: HashSet::new(),
            watched_files: HashSet::new(),
            events_tx: tx,
        }
    }

    pub(super) fn invalidate_context(&mut self) {
        self.context_epoch = self.context_epoch.wrapping_add(1);
        self.pending.clear();
        self.foreground.clear();
        self.watch_generation = self.watch_generation.wrapping_add(1);
        self.watch = None;
        self.watch_host = None;
        self.watch_opening = false;
        self.watch_busy = false;
        self.watch_dirty = false;
        self.watched_files.clear();
        self.watched_dirs.clear();
        self.watch_refresh = true;
        for file in &mut self.files {
            file.reload_seq = file.reload_seq.wrapping_add(1);
            file.reload_request = file.reload_request.wrapping_add(1);
        }
    }

    pub(super) fn take_watch_refresh(&mut self) -> bool {
        std::mem::take(&mut self.watch_refresh)
    }

    pub(super) fn forget_tab(&mut self, tab: TabId) {
        self.foreground.remove(&tab);
    }

    pub(super) fn get(&self, id: BufferId) -> Option<&OpenFile> {
        self.files.iter().find(|file| file.id() == id)
    }

    /// 测试等监视装上再改文件,不然外部变化没有观察者。
    #[cfg(test)]
    pub(super) fn watching(&self) -> bool {
        self.watch.is_some()
    }

    pub(super) fn get_mut(&mut self, id: BufferId) -> Option<&mut OpenFile> {
        self.files.iter_mut().find(|file| file.id() == id)
    }

    pub(super) fn at(&self, host: HostId, path: &Path) -> Option<BufferId> {
        self.files
            .iter()
            .find(|file| file.is_at(host, path))
            .map(OpenFile::id)
    }

    pub(super) fn iter(&self) -> std::slice::Iter<'_, OpenFile> {
        self.files.iter()
    }

    pub(super) fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    pub(super) fn remove(&mut self, id: BufferId) {
        self.files.retain(|file| file.id() != id);
    }

    pub(super) fn next_untitled(&mut self) -> u32 {
        let number = self.next_untitled;
        self.next_untitled += 1;
        number
    }

    pub(super) fn is_dirty(&self, host: HostId, path: &Path) -> bool {
        self.files
            .iter()
            .any(|file| file.dirty && file.is_at(host, path))
    }

    pub(super) fn note_edit(&mut self, id: BufferId, cx: &gpui_kit::App) {
        let Some(file) = self.get_mut(id) else { return };
        // A read-only image has no edits to compare: it is never unsaved, so
        // closing it never asks, and Save has nothing to write.
        if file.input.drawio().is_some() {
            // Engine commits already landed on the document; its content
            // digest carries undo-to-saved semantics, so editing back to the
            // last-written bytes reads clean again. A canvas whose commit was
            // refused still holds newer work: it is unsaved even when the
            // document bytes equal the file.
            let unconfirmed = drawio_has_unconfirmed_changes(file, cx);
            file.dirty = unconfirmed
                || file
                    .input
                    .drawio_document(cx)
                    .is_some_and(|document| document.fingerprint() != file.saved);
            file.reload_seq = file.reload_seq.wrapping_add(1);
            return;
        }
        let Some(text) = file.input.text() else {
            return;
        };
        // Edits retire source applicability, not the read request's identity:
        // a completed owning read must still surface a conflict without overwriting input.
        file.reload_seq = file.reload_seq.wrapping_add(1);
        let input = text.read(cx);
        let text = input.text();
        let len = text.len();
        let same =
            len == file.saved.len && Fingerprint::of_chunks(len, text.chunks()) == file.saved;
        file.dirty = !same
            || file.format(cx) != Some(file.saved_format)
            || file.representation_fingerprint(cx) != file.saved_representation;
    }

    /// Once a frame for each draw.io buffer: engine commits already landed
    /// on the document, so only the dirty comparison needs a sweep here. The
    /// returned views carry pending workbench requests (the engine's own
    /// Ctrl+S); the caller drains them through the same Host save path.
    pub(super) fn drawio_pending(
        &mut self,
        cx: &gpui_kit::App,
    ) -> Vec<(BufferId, Vec<Entity<DrawioView>>)> {
        let mut pending = Vec::new();
        for file in &mut self.files {
            let Some(primary) = file.input.drawio().cloned() else {
                continue;
            };
            let unconfirmed = drawio_has_unconfirmed_changes(file, cx);
            if let Some(document) = file.input.drawio_document(cx) {
                file.dirty = unconfirmed || document.fingerprint() != file.saved;
            }
            let mut views = vec![primary];
            for (_, _, view) in file.presentations.drawios().iter() {
                if !views.iter().any(|existing| existing == view) {
                    views.push(view.clone());
                }
            }
            pending.push((file.id(), views));
        }
        pending
    }

    /// Resolves deferred draw.io saves. A save wrote a sync request to every
    /// live presentation; it writes only once each of those exact requests is
    /// confirmed. A page refusal fails the save at once; a request whose
    /// session was replaced no longer owns newer bytes. The deadline is not
    /// permission to write older bytes, so an unanswered request expires.
    pub(super) fn take_drawio_flush_resolutions(
        &mut self,
        cx: &gpui_kit::App,
    ) -> Vec<(BufferId, DrawioFlushReadiness)> {
        use super::drawio_document::SyncResolution;

        let now = std::time::Instant::now();
        let mut ready = Vec::new();
        for file in &mut self.files {
            let Some(flush) = file.drawio_flush.clone() else {
                continue;
            };
            if let Some(reason) = flush.refused {
                file.drawio_flush = None;
                ready.push((file.id(), DrawioFlushReadiness::Rejected { reason }));
                continue;
            }
            let mut pending = false;
            let mut rejected = None;
            for (view, request) in &flush.tickets {
                let Some(view) = view.upgrade() else {
                    continue;
                };
                match view.read(cx).sync_resolution(*request) {
                    SyncResolution::Pending => pending = true,
                    SyncResolution::Confirmed => {}
                    SyncResolution::Rejected(reason) => rejected = Some(reason),
                    SyncResolution::Retired => {}
                }
            }
            // A confirmation only proves the page accepted *its* POST; the
            // document may still have refused the bytes (parse or size), in
            // which case the canvas holds newer, unsaved work.
            if rejected.is_none()
                && !pending
                && let Some(reason) = flush
                    .tickets
                    .iter()
                    .filter_map(|(view, _)| view.upgrade())
                    .find_map(|view| view.read(cx).unconfirmed_reason())
            {
                rejected = Some(reason);
            }
            if rejected.is_none() && pending && now < flush.deadline {
                continue;
            }
            file.drawio_flush = None;
            if let Some(reason) = rejected {
                ready.push((file.id(), DrawioFlushReadiness::Rejected { reason }));
            } else if !pending {
                // This save already waited for the engines: the re-entered
                // write must not flush again, or it would never reach disk.
                file.drawio_flush_done = true;
                ready.push((
                    file.id(),
                    DrawioFlushReadiness::Confirmed {
                        force: flush.force,
                        target: flush.target,
                    },
                ));
            } else {
                ready.push((file.id(), DrawioFlushReadiness::Expired));
            }
        }
        ready
    }

    /// Loading is host-bound; the target guard runs before allocating a buffer.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn open<E: 'static>(
        &mut self,
        host: SharedHost,
        path: &Path,
        cursor: Option<Position>,
        target: OpenTarget,
        window: &mut Window,
        cx: &mut Context<E>,
        access: Access<E>,
        edited: Edited<E>,
        valid: ValidTarget<E>,
        opened: Opened<E>,
    ) -> Option<BufferId> {
        let key = (target.tab, host.id(), path.to_path_buf());
        self.pending.remove(&key);
        self.open_seq = self.open_seq.wrapping_add(1);
        let seq = self.open_seq;
        self.foreground.insert(target.tab, seq);
        if let Some(id) = self.at(host.id(), path) {
            self.place_cursor(id, cursor, target, window, cx);
            return Some(id);
        }
        self.pending.insert(key.clone(), seq);
        let requested = path.to_path_buf();
        let guard_host = host.clone();
        HostOps::run_in(
            host.clone(),
            window,
            cx,
            move |h| load_file(h, requested),
            move |view, result, window, cx| {
                let current = access(view).pending.get(&key).copied() == Some(seq);
                if !current {
                    close_unclaimed_loaded(&guard_host, &result);
                    return;
                }
                access(view).pending.remove(&key);
                if !valid(view, target, cx) {
                    close_unclaimed_loaded(&guard_host, &result);
                    return;
                }
                let Some(current) = access(view).foreground.get(&target.tab).copied() else {
                    close_unclaimed_loaded(&guard_host, &result);
                    return;
                };
                let front = current == seq;
                let result = match result {
                    Ok(loaded) => {
                        let (id, created) =
                            access(view).install(host, loaded, window, cx, access, edited);
                        if front {
                            access(view).place_cursor(id, cursor, target, window, cx);
                        }
                        OpenResult::Opened { id, created, front }
                    }
                    Err(error) if front => OpenResult::Failed {
                        host: host.id(),
                        error,
                    },
                    Err(_) => return,
                };
                opened(view, target, result, window, cx);
            },
        );
        None
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn restore<E: 'static>(
        &mut self,
        host: SharedHost,
        files: Vec<PathBuf>,
        target: OpenTarget,
        window: &Window,
        cx: &mut Context<E>,
        access: Access<E>,
        valid: ValidTarget<E>,
        landed: impl FnOnce(&mut E, Vec<Option<Loaded>>, &mut Window, &mut Context<E>) + 'static,
    ) {
        let epoch = self.context_epoch;
        let guard_host = host.clone();
        HostOps::run_in(
            host,
            window,
            cx,
            move |host| {
                files
                    .into_iter()
                    .map(|path| (path.clone(), load_file(host, path)))
                    .collect::<Vec<_>>()
            },
            move |view, loaded, window, cx| {
                if access(view).context_epoch != epoch || !valid(view, target, cx) {
                    close_unclaimed_batch(&guard_host, &loaded);
                    return;
                }
                let loaded = loaded
                    .into_iter()
                    .map(|(path, result)| match result {
                        Ok(file) => Some(file),
                        Err(error) => {
                            if !is_markdown_path(&path)
                                && !is_svg_path(&path)
                                && !is_json_path(&path)
                                && !is_yaml_path(&path)
                                && !is_toml_path(&path)
                                && !is_csv_path(&path)
                                && !super::is_xml_path(&path)
                            {
                                return None;
                            }
                            let message = match error {
                                EditorOpenError::Message(message) => message,
                                EditorOpenError::NotText(path) => t_fmt(
                                    L10nKey::EditorBinaryFile,
                                    &[("path", &path.display().to_string())],
                                ),
                            };
                            window.notify_app(message, cx);
                            None
                        }
                    })
                    .collect();
                landed(view, loaded, window, cx);
            },
        );
    }

    fn place_cursor<E: 'static>(
        &mut self,
        id: BufferId,
        cursor: Option<Position>,
        target: OpenTarget,
        window: &mut Window,
        cx: &mut Context<E>,
    ) {
        let Some(position) = cursor else { return };
        let Some(file) = self.get_mut(id) else { return };
        // A picture has no caret to put down: the request is about text.
        if !file.input.is_text() {
            return;
        }
        let input = if let (Some(source), Some(group)) = (file.input.markdown(), target.group) {
            EditorInput::Markdown(
                file.presentations
                    .markdown_view(source, target.tab, group, window, cx),
            )
        } else if let (Some(primary), Some(group)) = (file.input.svg(), target.group) {
            EditorInput::Svg(file.presentations.svg_view(primary, target.tab, group, cx))
        } else if let (Some(primary), Some(group)) = (file.input.json(), target.group) {
            EditorInput::Json(file.presentations.json_view(primary, target.tab, group, cx))
        } else if let (Some(primary), Some(group)) = (file.input.yaml(), target.group) {
            EditorInput::Yaml(file.presentations.yaml_view(primary, target.tab, group, cx))
        } else if let (Some(primary), Some(group)) = (file.input.toml(), target.group) {
            EditorInput::Toml(file.presentations.toml_view(primary, target.tab, group, cx))
        } else if let (Some(primary), Some(group)) = (file.input.csv(), target.group) {
            EditorInput::Csv(
                file.presentations
                    .csv_view(primary, target.tab, group, window, cx),
            )
        } else if let (Some(primary), Some(group)) = (file.input.xml(), target.group) {
            EditorInput::Xml(file.presentations.xml_view(primary, target.tab, group, cx))
        } else {
            file.input.clone()
        };
        place_cursor(input, position, CURSOR_SCROLL_ATTEMPTS, window, cx);
    }

    pub(super) fn install<E: 'static>(
        &mut self,
        host: SharedHost,
        loaded: Loaded,
        window: &mut Window,
        cx: &mut Context<E>,
        access: Access<E>,
        edited: Edited<E>,
    ) -> (BufferId, bool) {
        if let Some(id) = self.at(host.id(), loaded.path()) {
            if let Loaded::Sqlite(sqlite) = loaded
                && let Ok(opened) = sqlite.opened
            {
                host.sqlite_close(opened.view);
            }
            return (id, false);
        }
        match loaded {
            Loaded::Image(image) => (self.create_image(host, image, window, cx), true),
            Loaded::Drawio(drawio) => (self.create_drawio(host, drawio, cx), true),
            Loaded::Sqlite(sqlite) => (self.create_sqlite(host, sqlite, window, cx), true),
            Loaded::Book(book) => (self.create_book(host, book, window, cx), true),
            Loaded::Pdf {
                path,
                mtime,
                content,
            } => (
                self.create_pdf(host, path, mtime, content, window, cx),
                true,
            ),
            Loaded::Text {
                path,
                text,
                format,
                mtime,
                config,
            } => {
                let language = language_for_path(&path);
                let indent = config.indent(lyrel_text::detect_indent(&text, language));
                (
                    self.create(
                        host,
                        path,
                        None,
                        BufferKind::Text,
                        text,
                        format,
                        config,
                        indent,
                        mtime,
                        window,
                        cx,
                        access,
                        edited,
                    ),
                    true,
                )
            }
        }
    }

    /// A read-only image document: the decode already happened off the UI
    /// thread, so this only takes the texture lease and installs the state.
    fn create_image<E: 'static>(
        &mut self,
        host: SharedHost,
        image: LoadedImage,
        _window: &mut Window,
        cx: &mut Context<E>,
    ) -> BufferId {
        let LoadedImage {
            path,
            claimed,
            mtime,
            result,
        } = image;
        let wrap = cx.global::<crate::core::config::Config>().editor_soft_wrap;
        let id = BufferId::new();
        // A budget or format refusal is part of the document: it says so in
        // its own body and status line, and the next external change retries.
        let document = cx.new(|cx| ImageDocument::new(claimed, result, cx));
        let view = cx.new(|cx| ImageView::new(document, cx));
        let sub = cx.observe(&view, |_, _, cx| cx.notify());
        let observe = cx.observe(&view, |_, _, cx| cx.notify());
        self.files.push(OpenFile {
            id,
            path,
            untitled: None,
            host,
            input: EditorInput::Image(view),
            presentations: Presentations::default(),
            markdown_resources: None,
            dirty: false,
            saved: Fingerprint::default(),
            format: None,
            saved_format: TextFormat::default(),
            saved_representation: None,
            config: EditorConfig::default(),
            indent: lyrel_text::detect_indent("", "text"),
            disk_mtime: mtime,
            saving: false,
            save_pending: false,
            save_as_pending: None,
            reload_seq: 0,
            reload_request: 0,
            drawio_flush: None,
            drawio_flush_done: false,
            conflict: None,
            wrap,
            gutter: gutter::BufferGutter::default(),
            popups: None,
            _sub: sub,
            _observe: observe,
        });
        id
    }

    /// A read-only draw.io document. The page list and failure are already
    /// decided off the UI thread; the native surface starts only when the
    /// presentation paints with a real window handle.
    fn create_drawio<E: 'static>(
        &mut self,
        host: SharedHost,
        drawio: LoadedDrawio,
        cx: &mut Context<E>,
    ) -> BufferId {
        let LoadedDrawio {
            path,
            bytes,
            source,
            mtime,
        } = drawio;
        let wrap = cx.global::<crate::core::config::Config>().editor_soft_wrap;
        let id = BufferId::new();
        let saved = Fingerprint::of_bytes(&bytes);
        let document = cx.new(|cx| DrawioDocument::new(bytes, source, cx));
        let view = cx.new(|cx| DrawioView::new(document, cx));
        let sub = cx.observe(&view, |_, _, cx| cx.notify());
        let observe = cx.observe(&view, |_, _, cx| cx.notify());
        self.files.push(OpenFile {
            id,
            path,
            untitled: None,
            host,
            input: EditorInput::Drawio(view),
            presentations: Presentations::default(),
            markdown_resources: None,
            dirty: false,
            saved,
            format: None,
            saved_format: TextFormat::default(),
            saved_representation: None,
            config: EditorConfig::default(),
            indent: lyrel_text::detect_indent("", "text"),
            disk_mtime: mtime,
            saving: false,
            save_pending: false,
            save_as_pending: None,
            reload_seq: 0,
            reload_request: 0,
            drawio_flush: None,
            drawio_flush_done: false,
            conflict: None,
            wrap,
            gutter: gutter::BufferGutter::default(),
            popups: None,
            _sub: sub,
            _observe: observe,
        });
        id
    }

    /// 数据库读取视图在所属 Host 建立，文档拥有并在关闭时释放它。
    fn create_sqlite<E: 'static>(
        &mut self,
        host: SharedHost,
        sqlite: LoadedSqlite,
        window: &mut Window,
        cx: &mut Context<E>,
    ) -> BufferId {
        let LoadedSqlite {
            path,
            mtime,
            opened,
        } = sqlite;
        let id = BufferId::new();
        let document_host = host.clone();
        let document = cx.new(|cx| SqliteDocument::new(document_host, path.clone(), opened, cx));
        let view = cx.new(|cx| SqliteView::new(document.clone(), window, cx));
        document.update(cx, |document, cx| document.start(window, cx));
        let sub = cx.observe(&view, |_, _, cx| cx.notify());
        let observe = cx.observe(&view, |_, _, cx| cx.notify());
        let wrap = cx.global::<crate::core::config::Config>().editor_soft_wrap;
        self.files.push(OpenFile {
            id,
            path,
            untitled: None,
            host,
            input: EditorInput::Sqlite(view),
            presentations: Presentations::default(),
            markdown_resources: None,
            dirty: false,
            saved: Fingerprint::default(),
            format: None,
            saved_format: TextFormat::default(),
            saved_representation: None,
            config: EditorConfig::default(),
            indent: lyrel_text::detect_indent("", "text"),
            disk_mtime: mtime,
            saving: false,
            save_pending: false,
            save_as_pending: None,
            reload_seq: 0,
            reload_request: 0,
            drawio_flush: None,
            drawio_flush_done: false,
            conflict: None,
            wrap,
            gutter: gutter::BufferGutter::default(),
            popups: None,
            _sub: sub,
            _observe: observe,
        });
        id
    }

    /// A book document owns its coherent temporary copy, chapter cache,
    /// search index and reading record; the presentation is only a view.
    fn create_book<E: 'static>(
        &mut self,
        host: SharedHost,
        book: LoadedBook,
        window: &mut Window,
        cx: &mut Context<E>,
    ) -> BufferId {
        let path = book.path.clone();
        let mtime = book.mtime;
        let wrap = cx.global::<crate::core::config::Config>().editor_soft_wrap;
        let id = BufferId::new();
        let document = cx.new(|cx| BookDocument::new(book, cx));
        let view = cx.new(|cx| BookView::new(document, window, cx));
        let sub = cx.observe(&view, |_, _, cx| cx.notify());
        let observe = cx.observe(&view, |_, _, cx| cx.notify());
        self.files.push(OpenFile {
            id,
            path,
            untitled: None,
            host,
            input: EditorInput::Book(view),
            presentations: Presentations::default(),
            markdown_resources: None,
            dirty: false,
            saved: Fingerprint::default(),
            format: None,
            saved_format: TextFormat::default(),
            saved_representation: None,
            config: EditorConfig::default(),
            indent: lyrel_text::detect_indent("", "text"),
            disk_mtime: mtime,
            saving: false,
            save_pending: false,
            save_as_pending: None,
            reload_seq: 0,
            reload_request: 0,
            drawio_flush: None,
            drawio_flush_done: false,
            conflict: None,
            wrap,
            gutter: gutter::BufferGutter::default(),
            popups: None,
            _sub: sub,
            _observe: observe,
        });
        id
    }

    /// A read-only PDF document. Parsing and page rasterization happen off the
    /// UI thread; this only installs the document identity and its first view.
    fn create_pdf<E: 'static>(
        &mut self,
        host: SharedHost,
        path: PathBuf,
        mtime: Option<MTime>,
        content: PdfLoaded,
        window: &mut Window,
        cx: &mut Context<E>,
    ) -> BufferId {
        let wrap = cx.global::<crate::core::config::Config>().editor_soft_wrap;
        let id = BufferId::new();
        let document = cx.new(|_| PdfDocument::new(content));
        let view = cx.new(|cx| PdfView::new(document, window, cx));
        let sub = cx.observe(&view, |_, _, cx| cx.notify());
        let observe = cx.observe(&view, |_, _, cx| cx.notify());
        self.files.push(OpenFile {
            id,
            path,
            untitled: None,
            host,
            input: EditorInput::Pdf(view),
            presentations: Presentations::default(),
            markdown_resources: None,
            dirty: false,
            saved: Fingerprint::default(),
            format: None,
            saved_format: TextFormat::default(),
            saved_representation: None,
            config: EditorConfig::default(),
            indent: lyrel_text::detect_indent("", "text"),
            disk_mtime: mtime,
            saving: false,
            save_pending: false,
            save_as_pending: None,
            reload_seq: 0,
            reload_request: 0,
            drawio_flush: None,
            drawio_flush_done: false,
            conflict: None,
            wrap,
            gutter: gutter::BufferGutter::default(),
            popups: None,
            _sub: sub,
            _observe: observe,
        });
        id
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn create<E: 'static>(
        &mut self,
        host: SharedHost,
        path: PathBuf,
        untitled: Option<u32>,
        kind: BufferKind,
        text: String,
        format: TextFormat,
        config: EditorConfig,
        indent: Indent,
        mtime: Option<MTime>,
        window: &mut Window,
        cx: &mut Context<E>,
        access: Access<E>,
        edited: Edited<E>,
    ) -> BufferId {
        let language = if untitled.is_some() {
            "text"
        } else {
            language_for_path(&path)
        };
        let cfg = cx.global::<crate::core::config::Config>();
        let wrap = cfg.editor_soft_wrap;
        let native_svg = untitled.is_none() && is_svg_path(&path);
        let native_json = untitled.is_none()
            && !native_svg
            && is_json_path(&path)
            && JsonFormat::from_path(&path).is_some();
        let native_yaml = untitled.is_none() && !native_svg && !native_json && is_yaml_path(&path);
        let native_toml = untitled.is_none() && !native_svg && !native_json && is_toml_path(&path);
        let native_csv = untitled.is_none() && is_csv_path(&path);
        let native_xml = untitled.is_none()
            && !native_svg
            && !native_json
            && !native_yaml
            && !native_toml
            && super::is_xml_path(&path);
        let native_markdown = if native_svg
            || native_json
            || native_yaml
            || native_toml
            || native_csv
            || native_xml
        {
            false
        } else if untitled.is_some() {
            matches!(kind, BufferKind::Markdown)
        } else {
            is_markdown_path(&path)
        };
        let id = BufferId::new();
        let (input, popups, sub, observe) = if native_svg {
            let document = cx.new(|_| lyrel_markdown_editor::Document::new(text, format));
            let sub = cx.subscribe_in(
                &document,
                window,
                move |view, _, _: &lyrel_markdown_editor::DocumentEvent, _, cx| {
                    access(view).note_edit(id, cx);
                    edited(view, id, cx);
                },
            );
            let preview = cx.new(|cx| SvgDocument::new(document, cx));
            let presentation =
                cx.new(|cx| SvgView::new(preview, SvgMode::Source, wrap, indent.size as usize, cx));
            let observe = cx.observe(&presentation, |_, _, cx| cx.notify());
            (EditorInput::Svg(presentation), None, sub, observe)
        } else if native_json {
            let json_format =
                JsonFormat::from_path(&path).expect("native JSON path has a JSON-family format");
            let document = cx.new(|_| lyrel_markdown_editor::Document::new(text, format));
            let sub = cx.subscribe_in(
                &document,
                window,
                move |view, _, _: &lyrel_markdown_editor::DocumentEvent, _, cx| {
                    access(view).note_edit(id, cx);
                    edited(view, id, cx);
                },
            );
            let json_document = cx.new(|cx| JsonDocument::new(document, json_format, cx));
            let presentation = cx.new(|cx| {
                JsonView::new(
                    json_document,
                    JsonMode::Structure,
                    wrap,
                    indent.size as usize,
                    cx,
                )
            });
            let observe = cx.observe(&presentation, |_, _, cx| cx.notify());
            (EditorInput::Json(presentation), None, sub, observe)
        } else if native_yaml {
            let document = cx.new(|_| lyrel_markdown_editor::Document::new(text, format));
            let sub = cx.subscribe_in(
                &document,
                window,
                move |view, _, _: &lyrel_markdown_editor::DocumentEvent, _, cx| {
                    access(view).note_edit(id, cx);
                    edited(view, id, cx);
                },
            );
            let yaml_document = cx.new(|cx| YamlDocument::new(document, cx));
            let presentation = cx.new(|cx| {
                YamlView::new(
                    yaml_document,
                    YamlMode::Structure,
                    wrap,
                    indent.size as usize,
                    cx,
                )
            });
            let observe = cx.observe(&presentation, |_, _, cx| cx.notify());
            (EditorInput::Yaml(presentation), None, sub, observe)
        } else if native_toml {
            let document = cx.new(|_| lyrel_markdown_editor::Document::new(text, format));
            let sub = cx.subscribe_in(
                &document,
                window,
                move |view, _, _: &lyrel_markdown_editor::DocumentEvent, _, cx| {
                    access(view).note_edit(id, cx);
                    edited(view, id, cx);
                },
            );
            let toml_document = cx.new(|cx| TomlDocument::new(document, cx));
            let presentation = cx.new(|cx| {
                TomlView::new(
                    toml_document,
                    TomlMode::Structure,
                    wrap,
                    indent.size as usize,
                    cx,
                )
            });
            let observe = cx.observe(&presentation, |_, _, cx| cx.notify());
            (EditorInput::Toml(presentation), None, sub, observe)
        } else if native_csv {
            let delimiter = csv_delimiter(&path);
            let document = cx.new(|_| lyrel_markdown_editor::Document::new(text, format));
            let sub = cx.subscribe_in(
                &document,
                window,
                move |view, _, _: &lyrel_markdown_editor::DocumentEvent, _, cx| {
                    access(view).note_edit(id, cx);
                    edited(view, id, cx);
                },
            );
            let table = cx.new(|cx| CsvDocument::new(document, delimiter, cx));
            let presentation = cx.new(|cx| {
                CsvView::new(
                    table,
                    CsvMode::Table,
                    wrap,
                    indent.size as usize,
                    window,
                    cx,
                )
            });
            let observe = cx.observe(&presentation, |_, _, cx| cx.notify());
            (EditorInput::Csv(presentation), None, sub, observe)
        } else if native_xml {
            let document = cx.new(|_| lyrel_markdown_editor::Document::new(text, format));
            let sub = cx.subscribe_in(
                &document,
                window,
                move |view, _, _: &lyrel_markdown_editor::DocumentEvent, _, cx| {
                    access(view).note_edit(id, cx);
                    edited(view, id, cx);
                },
            );
            let xml_document = cx.new(|cx| XmlDocument::new(document, cx));
            let presentation = cx.new(|cx| {
                XmlView::new(
                    xml_document,
                    XmlMode::Structure,
                    wrap,
                    indent.size as usize,
                    cx,
                )
            });
            let observe = cx.observe(&presentation, |_, _, cx| cx.notify());
            (EditorInput::Xml(presentation), None, sub, observe)
        } else if native_markdown {
            let document = cx.new(|_| lyrel_markdown_editor::Document::new(text, format));
            let sub = cx.subscribe_in(
                &document,
                window,
                move |view, _, _: &lyrel_markdown_editor::DocumentEvent, _, cx| {
                    access(view).note_edit(id, cx);
                    edited(view, id, cx);
                },
            );
            let presentation = cx.new(|cx| {
                let mut view = lyrel_markdown_editor::EditorState::for_document(document, cx);
                markdown::configure(&mut view, cx);
                view.set_soft_wrap(wrap, cx);
                view.set_tab_indent(indent.size as usize);
                if path
                    .extension()
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("mdx"))
                {
                    view.set_source_only(true, cx);
                }
                view
            });
            super::markdown_formula::install(&presentation, window, cx);
            let observe = cx.observe(&presentation, |_, _, cx| cx.notify());
            (EditorInput::Markdown(presentation), None, sub, observe)
        } else {
            let input = cx.new(|cx| {
                let mut state = EditorState::new(window, cx)
                    .language(language)
                    .tab_size(tab_size(indent))
                    .line_number(true)
                    .searchable(true)
                    .replaceable(true)
                    .folding(true)
                    .soft_wrap(wrap)
                    .context_menu(false)
                    .default_value(text);
                // Base's default bar only appears while scrolling. A document
                // keeps its bars visible so a horizontally overflowing line —
                // and the vertical position — stay discoverable at rest.
                state.set_scrollbar_renderer(Some(std::rc::Rc::new(|context, _window, _cx| {
                    gpui_kit::component::scroll::Scrollbar::new(&context)
                        .mode(gpui_kit::component::scroll::ScrollbarMode::Always)
                        .into_any_element()
                })));
                state
            });
            let popups = cx.new(|cx| crate::ui::lsp::popups::EditorPopups::new(&input, window, cx));
            let sub = cx.subscribe_in(&input, window, move |view, _, ev, _, cx| {
                if matches!(ev, InputEvent::Change) {
                    access(view).note_edit(id, cx);
                    edited(view, id, cx);
                }
            });
            let observe = cx.observe(&input, |_, _, cx| cx.notify());
            (EditorInput::Code(input), Some(popups), sub, observe)
        };
        // `create` only builds text inputs; a picture goes through
        // `create_image`, so the text capability is always here.
        let saved = {
            let text = input.text().expect("text input");
            let value = text.read(cx);
            Fingerprint::of_chunks(value.text().len(), value.text().chunks())
        };
        let saved_representation = input
            .source_document(cx)
            .map(|document| document.read(cx).representation_fingerprint());
        self.files.push(OpenFile {
            id,
            path,
            untitled,
            host,
            input,
            presentations: Presentations::default(),
            markdown_resources: None,
            dirty: false,
            saved,
            format: (!native_markdown && !native_svg && !native_toml && !native_csv)
                .then_some(format),
            saved_format: format,
            saved_representation,
            config,
            indent,
            disk_mtime: mtime,
            saving: false,
            save_pending: false,
            save_as_pending: None,
            reload_seq: 0,
            reload_request: 0,
            drawio_flush: None,
            drawio_flush_done: false,
            conflict: None,
            wrap,
            gutter: gutter::BufferGutter::default(),
            popups,
            _sub: sub,
            _observe: observe,
        });
        id
    }

    /// A watch only sees buffers on its own Host. SFTP-only buffers retain
    /// save-time conflict detection rather than borrowing a local watcher.
    pub(super) fn rebuild_watch<E: 'static>(
        &mut self,
        host: Option<SharedHost>,
        cx: &mut Context<E>,
        access: Access<E>,
    ) {
        let changed_host = match (&self.watch_host, &host) {
            (Some(a), Some(b)) => !Arc::ptr_eq(a, b),
            (None, None) => false,
            _ => true,
        };
        if changed_host {
            self.watch_generation = self.watch_generation.wrapping_add(1);
            self.watch = None;
            self.watch_host = host.clone();
            self.watch_opening = false;
            self.watch_busy = false;
            self.watch_dirty = false;
        }
        let files: HashSet<PathBuf> = self
            .files
            .iter()
            .filter(|file| {
                file.untitled.is_none() && Some(file.host.id()) == host.as_ref().map(|h| h.id())
            })
            .map(|file| file.path.clone())
            .collect();
        let dirs = files
            .iter()
            .filter_map(|path| path.parent().map(Path::to_path_buf))
            .collect();
        self.watched_files = files;
        if dirs == self.watched_dirs && !changed_host {
            return;
        }
        self.watched_dirs = dirs;
        self.apply_watch(cx, access);
    }

    fn apply_watch<E: 'static>(&mut self, cx: &mut Context<E>, access: Access<E>) {
        let Some(host) = self.watch_host.clone() else {
            return;
        };
        let generation = self.watch_generation;
        let want: Vec<PathBuf> = self.watched_dirs.iter().cloned().collect();
        if let Some(sub) = self.watch.clone() {
            if self.watch_busy {
                self.watch_dirty = true;
                return;
            }
            self.watch_busy = true;
            HostOps::run(
                host,
                cx,
                move |_| sub.set_dirs(&want),
                move |view, result, cx| {
                    let owner = access(view);
                    if owner.watch_generation != generation {
                        return;
                    }
                    owner.watch_busy = false;
                    if let Err(error) = result {
                        log::warn!("editor: could not update the watched set: {error}");
                    }
                    if std::mem::take(&mut owner.watch_dirty) {
                        owner.apply_watch(cx, access);
                    }
                },
            );
            return;
        }
        if self.watch_opening {
            return;
        }
        self.watch_opening = true;
        let opened_with = self.watched_dirs.clone();
        HostOps::run(
            host,
            cx,
            move |h| h.watch(&want).map(Arc::new),
            move |view, result: std::io::Result<Arc<WatchSub>>, cx| {
                let owner = access(view);
                if owner.watch_generation != generation {
                    return;
                }
                owner.watch_opening = false;
                let sub = match result {
                    Ok(sub) => sub,
                    Err(error) => {
                        log::warn!("editor: external-change watcher unavailable: {error}");
                        return;
                    }
                };
                let events = sub.events().clone();
                let tx = owner.events_tx.clone();
                owner.watch = Some(sub);
                if owner.watched_dirs != opened_with {
                    owner.apply_watch(cx, access);
                }
                cx.spawn(async move |_, _| {
                    while let Ok(paths) = events.recv().await {
                        if tx.send(WatchBatch { generation, paths }).await.is_err() {
                            break;
                        }
                    }
                })
                .detach();
            },
        );
    }

    pub(super) fn external_change<E: 'static>(
        &mut self,
        path: &Path,
        window: &Window,
        cx: &mut Context<E>,
        access: Access<E>,
        edited: Edited<E>,
    ) {
        let Some(host) = self.watch_host.clone() else {
            return;
        };
        let Some(id) = self.at(host.id(), path) else {
            return;
        };
        let Some(seq) = self.get(id).map(|file| file.reload_seq) else {
            return;
        };
        let generation = self.watch_generation;
        let target = path.to_path_buf();
        let expected = target.clone();
        HostOps::run_in(
            host.clone(),
            window,
            cx,
            move |h| match h.stat(&target) {
                Ok(meta) => Observed::Present(meta.mtime),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Observed::Missing,
                Err(_) => Observed::Present(None),
            },
            move |view, observed, window, cx| {
                let owner = access(view);
                if owner.watch_generation != generation {
                    return;
                }
                if owner
                    .get(id)
                    .is_none_or(|file| file.reload_seq != seq || !file.is_at(host.id(), &expected))
                {
                    return;
                }
                if owner.apply_external_change(id, observed, cx) {
                    owner.reload(id, window, cx, access, edited);
                }
            },
        );
    }

    /// Returns whether the clean buffer needs reloading; conflicts never
    /// replace text and therefore preserve every tab's unsaved edits.
    fn apply_external_change<E: 'static>(
        &mut self,
        id: BufferId,
        observed: Observed,
        cx: &mut Context<E>,
    ) -> bool {
        let Some(file) = self.get_mut(id) else {
            return false;
        };
        let pending = file
            .input
            .source_document(cx)
            .is_some_and(|document| document.read(cx).has_composition());
        match classify_external_change(
            file.saving,
            file.dirty || pending,
            file.disk_mtime,
            observed,
        ) {
            ExternalChange::Ignore => {}
            ExternalChange::Conflict => {
                if let Observed::Present(mtime) = observed {
                    file.conflict = Some(DiskConflict::Changed(mtime));
                }
                cx.notify();
            }
            ExternalChange::Deleted => {
                file.conflict = Some(DiskConflict::Deleted);
                file.disk_mtime = None;
                cx.notify();
            }
            ExternalChange::Reload => return true,
        }
        false
    }

    pub(super) fn reload<E: 'static>(
        &mut self,
        id: BufferId,
        window: &Window,
        cx: &mut Context<E>,
        access: Access<E>,
        edited: Edited<E>,
    ) {
        let Some(file) = self.get_mut(id).filter(|file| file.untitled.is_none()) else {
            return;
        };
        let target = file.path.clone();
        let expected = target.clone();
        let host = file.host.clone();
        let host_id = host.id();
        // Markdown and SVG decode through the strict source codec so mixed
        // terminators and declared encodings survive a reload.
        let source = file.input.source_document(cx);
        let source_reload = source.is_some();
        let image = file.input.image().cloned();
        let drawio = file.input.drawio().cloned();
        let drawio_reload = drawio.is_some();
        if let Some(view) = &drawio {
            let document = view.read(cx).document().clone();
            document.update(cx, |document, cx| document.begin_reload(cx));
        }
        let sqlite = file.input.sqlite().cloned();
        let book = file
            .input
            .book()
            .map(|view| view.read(cx).document().clone());
        let pdf = file.input.pdf().cloned();
        let input_lease = source.as_ref().map(|document| document.read(cx).revision());
        let pending_lease = source
            .as_ref()
            .map(|document| document.read(cx).has_composition());
        // The document's own claim survives a rename or a probe-only open;
        // reload must not fall back to the extension and lose the format.
        let image_claimed = image
            .as_ref()
            .and_then(|view| view.read(cx).document().read(cx).claimed());
        file.reload_seq = file.reload_seq.wrapping_add(1);
        let edit_seq = file.reload_seq;
        file.reload_request = file.reload_request.wrapping_add(1);
        let seq = file.reload_request;
        // The status line says a fresh read is under way; the old picture
        // stays until the new one (or its refusal) replaces it.
        if let Some(view) = &image {
            let document = view.read(cx).document().clone();
            document.update(cx, |document, cx| document.begin_reload(cx));
        }
        if let Some(view) = &pdf {
            let document = view.read(cx).document().clone();
            document.update(cx, |document, cx| document.begin_reload(cx));
        }
        let image_reload = image.is_some();
        let sqlite_reload = sqlite.is_some();
        let guard_host = host.clone();
        let pdf_reload = pdf.is_some();
        HostOps::run_in(
            host,
            window,
            cx,
            move |h| {
                if drawio_reload {
                    let meta = h.stat(&target)?;
                    if meta.len > drawio::MAX_DOCUMENT_BYTES as u64 {
                        return Ok(Reloaded::Drawio(LoadedDrawio {
                            path: target,
                            bytes: Arc::from([]),
                            source: Err(drawio::DrawioError::TooLarge),
                            mtime: meta.mtime,
                        }));
                    }
                    let bytes = h.read_file(&target, drawio::MAX_DOCUMENT_BYTES as u64)?;
                    let source = drawio::parse(&bytes);
                    return Ok(Reloaded::Drawio(LoadedDrawio {
                        path: target,
                        bytes: Arc::from(bytes),
                        source,
                        mtime: meta.mtime,
                    }));
                }
                if sqlite_reload {
                    let meta = h.stat(&target)?;
                    return Ok(Reloaded::Sqlite(load_sqlite(h, target, meta)));
                }
                if book.is_some() {
                    let meta = h.stat(&target)?;
                    return book_document::load_claimed(h, target, meta).map(Reloaded::Book);
                }
                if image_reload {
                    let format = image_claimed
                        .or_else(|| ImageFormat::from_extension(&target))
                        .ok_or_else(|| {
                            std::io::Error::new(
                                std::io::ErrorKind::InvalidData,
                                t(L10nKey::MarkdownImageUnsupported),
                            )
                        })?;
                    let meta = h.stat(&target)?;
                    return read_image(h, target, format, meta).map(Reloaded::Image);
                }
                if pdf_reload {
                    let meta = h.stat(&target)?;
                    return read_pdf(h, target, meta)
                        .map(|(content, mtime)| Reloaded::Pdf(content, mtime));
                }
                let bytes = h.read_file(&target, MAX_FILE_BYTES)?;
                let decoded = if source_reload {
                    lyrel_text::decode_source(&bytes)?
                } else {
                    lyrel_text::decode(&bytes)
                }
                .ok_or_else(|| {
                    std::io::Error::new(std::io::ErrorKind::InvalidData, "no longer text")
                })?;
                let mtime = h.stat(&target).ok().and_then(|meta| meta.mtime);
                Ok(Reloaded::Text(decoded, mtime))
            },
            move |view, result: std::io::Result<Reloaded>, window, cx| {
                let Some(file) = access(view).get_mut(id) else {
                    close_unclaimed_sqlite(&guard_host, &result);
                    return;
                };
                if file.reload_request != seq || !file.is_at(host_id, &expected) {
                    close_unclaimed_sqlite(&guard_host, &result);
                    return;
                }
                let reloaded = match result {
                    Ok(reloaded) => reloaded,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        file.conflict = Some(DiskConflict::Deleted);
                        file.disk_mtime = None;
                        cx.notify();
                        return;
                    }
                    Err(error) => {
                        log::warn!("editor: reload of {} failed: {error}", file.path.display());
                        HostOps::notify_err(window, cx, &file.label(), &error);
                        return;
                    }
                };
                let (decoded, mtime) = match reloaded {
                    Reloaded::Text(decoded, mtime) => (decoded, mtime),
                    // A diagram is read-only in this ticket, so the freshest
                    // bytes are the document. The old engine session is retired
                    // by the document revision change.
                    Reloaded::Drawio(drawio) => {
                        file.disk_mtime = drawio.mtime;
                        if file.dirty {
                            // The freshest bytes stay on disk while the engine
                            // edits live in the document; "Keep mine" now has
                            // a real side to keep.
                            file.conflict = Some(DiskConflict::Changed(drawio.mtime));
                            cx.notify();
                            return;
                        }
                        file.conflict = None;
                        if let Some(view) = file.input.drawio().cloned() {
                            let LoadedDrawio { bytes, source, .. } = drawio;
                            file.saved = Fingerprint::of_bytes(&bytes);
                            let document = view.read(cx).document().clone();
                            document.update(cx, |document, cx| document.replace(bytes, source, cx));
                        }
                        cx.notify();
                        return;
                    }
                    // A book cannot be edited, so nothing here can conflict:
                    // the freshest coherent copy is the document. Typography is
                    // kept; a changed fingerprint resets position/bookmarks.
                    Reloaded::Book(book) => {
                        file.disk_mtime = book.mtime;
                        file.conflict = None;
                        let document = file
                            .input
                            .book()
                            .map(|view| view.read(cx).document().clone());
                        if let Some(document) = document {
                            document.update(cx, |document, cx| document.replace(book, cx));
                        }
                        cx.notify();
                        return;
                    }
                    // PDFs are read-only too: a clean document takes the new
                    // bytes, including a new password or format failure.
                    Reloaded::Pdf(content, mtime) => {
                        file.disk_mtime = mtime;
                        file.conflict = None;
                        let document = file
                            .input
                            .pdf()
                            .map(|view| view.read(cx).document().clone());
                        if let Some(document) = document {
                            document.update(cx, |document, cx| document.replace(content, cx));
                        }
                        cx.notify();
                        return;
                    }
                    // A picture cannot be edited, so nothing here can conflict:
                    // the freshest bytes are the document.
                    Reloaded::Image(image) => {
                        file.disk_mtime = image.mtime;
                        file.conflict = None;
                        let document = file
                            .input
                            .image()
                            .map(|view| view.read(cx).document().clone());
                        if let Some(document) = document {
                            document.update(cx, |document, cx| document.replace(image.result, cx));
                        }
                        cx.notify();
                        return;
                    }
                    // A database is read-only here too: the freshest view
                    // replaces the old one, and late pages of the old view
                    // are rejected by its identity.
                    Reloaded::Sqlite(sqlite) => {
                        file.disk_mtime = sqlite.mtime;
                        file.conflict = None;
                        let document = file
                            .input
                            .sqlite()
                            .map(|view| view.read(cx).document().clone());
                        if let Some(document) = document {
                            document.update(cx, |document, cx| {
                                document.replace(sqlite.opened, window, cx)
                            });
                        }
                        cx.notify();
                        return;
                    }
                };
                if file.reload_seq != edit_seq
                    || input_lease.is_some_and(|lease| {
                        source
                            .as_ref()
                            .is_none_or(|document| document.read(cx).revision() != lease)
                    })
                {
                    file.conflict = Some(DiskConflict::Changed(mtime));
                    cx.notify();
                    return;
                }
                if pending_lease.is_some_and(|pending| {
                    pending
                        || source
                            .as_ref()
                            .is_none_or(|document| document.read(cx).has_composition())
                }) {
                    file.conflict = Some(DiskConflict::Changed(mtime));
                    cx.notify();
                    return;
                }
                file.disk_mtime = mtime;
                file.conflict = None;
                file.saved_format = decoded.format;
                let input = file.input.clone();
                if let Some(document) = &source {
                    document.update(cx, |document, cx| {
                        document.reload(decoded.text, decoded.format, cx)
                    });
                } else {
                    file.set_format(decoded.format, cx);
                    replace_buffer_text(&input, &decoded.text, window, cx);
                }
                let file = access(view).get_mut(id).unwrap();
                // This arm only runs for reloaded text; pictures reload their
                // own image state and never reach it.
                if let Some(text) = file.input.text() {
                    let value = text.read(cx);
                    file.saved = Fingerprint::of_chunks(value.text().len(), value.text().chunks());
                }
                file.saved_representation = file.representation_fingerprint(cx);
                access(view).note_edit(id, cx);
                edited(view, id, cx);
            },
        );
    }

    pub(super) fn path_moved(
        &mut self,
        host: HostId,
        from: &Path,
        to: &Path,
        cx: &mut gpui_kit::App,
    ) -> bool {
        let mut moved = false;
        for file in &mut self.files {
            if file.untitled.is_some() || file.host.id() != host {
                continue;
            }
            let Ok(rest) = file.path.strip_prefix(from) else {
                continue;
            };
            file.path = if rest.as_os_str().is_empty() {
                to.to_path_buf()
            } else {
                to.join(rest)
            };
            file.reload_seq = file.reload_seq.wrapping_add(1);
            file.reload_request = file.reload_request.wrapping_add(1);
            file.input
                .set_highlighter(language_for_path(&file.path), cx);
            let source_only = file
                .path
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("mdx"));
            file.presentations
                .set_source_only(source_only, &file.input, cx);
            moved = true;
        }
        moved
    }

    pub(super) fn path_removed(&mut self, host: HostId, path: &Path) {
        for file in &mut self.files {
            if file.untitled.is_none() && file.host.id() == host && file.path.starts_with(path) {
                file.conflict = Some(DiskConflict::Deleted);
                file.disk_mtime = None;
                file.reload_seq = file.reload_seq.wrapping_add(1);
                file.reload_request = file.reload_request.wrapping_add(1);
            }
        }
    }
}
/// A file read off its Host, decoded and ready to become a buffer.
pub(super) enum Loaded {
    Text {
        path: PathBuf,
        text: String,
        format: TextFormat,
        mtime: Option<MTime>,
        config: EditorConfig,
    },
    /// An image: the bytes arrived, but whether they decode is part of the
    /// document, not of the read — a corrupt picture still gets a place.
    Image(LoadedImage),
    /// A draw.io file. Parsing its page list happened on the Host thread;
    /// failure is part of the document, not a reason to fall back to text.
    Drawio(LoadedDrawio),
    /// 读取视图由所属 Host 建立；失败也保留独立文档和可见错误。
    Sqlite(LoadedSqlite),
    /// A coherent book copy and its package answer. A corrupt or protected
    /// EPUB is still a document with a place and a visible reason.
    Book(LoadedBook),
    /// A PDF: parse/password failures are the document's own visible state, so
    /// the file still gets a tab with an explanation and a retry path.
    Pdf {
        path: PathBuf,
        mtime: Option<MTime>,
        content: PdfLoaded,
    },
}

pub(super) struct LoadedDrawio {
    path: PathBuf,
    bytes: Arc<[u8]>,
    source: Result<drawio::DrawioSource, drawio::DrawioError>,
    mtime: Option<MTime>,
}

pub(super) struct LoadedSqlite {
    path: PathBuf,
    mtime: Option<MTime>,
    opened: std::io::Result<SqliteOpened>,
}

pub(super) struct LoadedImage {
    path: PathBuf,
    /// What the name or magic claimed; the failure label when the bytes do
    /// not decode.
    claimed: Option<ImageFormat>,
    mtime: Option<MTime>,
    result: ImageOutcome,
}

impl Loaded {
    fn path(&self) -> &Path {
        match self {
            Self::Text { path, .. } => path,
            Self::Image(image) => &image.path,
            Self::Drawio(drawio) => &drawio.path,
            Self::Sqlite(sqlite) => &sqlite.path,
            Self::Book(book) => &book.path,
            Self::Pdf { path, .. } => path,
        }
    }
}

/// 一次重新读取在 Host 线程上得到的东西。
enum Reloaded {
    Text(lyrel_text::Decoded, Option<MTime>),
    Image(LoadedImage),
    Drawio(LoadedDrawio),
    Sqlite(LoadedSqlite),
    Book(LoadedBook),
    Pdf(PdfLoaded, Option<MTime>),
}

/// Reads, sizes, decodes and configures a file, on the host's own thread.
fn load_file(
    h: &dyn lyrel_core::host::Host,
    requested: PathBuf,
) -> Result<Loaded, EditorOpenError> {
    let path = h.canonicalize(&requested).unwrap_or(requested);
    let claimed = ImageFormat::from_extension(&path);
    let claimed_pdf = is_pdf_path(&path);
    let meta = h.stat(&path).map_err(|e| {
        EditorOpenError::Message(t_fmt(
            L10nKey::EditorCantOpen,
            &[("path", &path.display().to_string()), ("e", &e.to_string())],
        ))
    })?;
    // A file that names itself an EPUB, or an archive whose container names
    // one, is a book before it is a picture or a text decode. A generic ZIP
    // that carries no EPUB container keeps the workbench's existing handling.
    if let Some(loaded) = load_book_candidate(h, &path, meta)? {
        return Ok(loaded);
    }
    if claimed_pdf {
        return load_pdf(h, path, meta);
    }
    if is_drawio_path(&path) {
        return load_drawio(h, path, meta);
    }
    if let Some(format) = claimed {
        return load_image(h, path, format, meta);
    }
    // 数据库不能被当作文本读进内存：整库下载已经被数据库浏览本身排除。
    // 名称是唯一不读文件内容的判据；真正的识别与错误由只读打开给出。
    if is_sqlite_path(&path) {
        return Ok(Loaded::Sqlite(load_sqlite(h, path, meta)));
    }
    if meta.len > MAX_FILE_BYTES {
        return Err(EditorOpenError::Message(t_fmt(
            L10nKey::EditorFileTooLarge,
            &[
                ("path", &path.display().to_string()),
                ("size", &(meta.len / (1024 * 1024)).to_string()),
            ],
        )));
    }
    let bytes = h.read_file(&path, MAX_FILE_BYTES).map_err(|e| {
        EditorOpenError::Message(t_fmt(
            L10nKey::EditorCantRead,
            &[("path", &path.display().to_string()), ("e", &e.to_string())],
        ))
    })?;
    // A draw.io document saved with a generic name still has its own root.
    // This is a bounded probe, and it wins over the text and image fallbacks:
    // rendering it as source would hide the diagram the file declares.
    if drawio::looks_like_drawio(&bytes) {
        return Ok(loaded_drawio(path, meta.mtime, bytes));
    }
    // A name that claims nothing can still hold a picture. Only magic the
    // formats actually start with counts here, and only after text decoding
    // has had its chance: a file the editor can show as text keeps that.
    if let Some(format) = ImageFormat::probe(&bytes) {
        return load_image(h, path, format, meta);
    }
    if is_pdf_bytes(&bytes) {
        return Ok(Loaded::Pdf {
            path,
            mtime: meta.mtime,
            content: pdf_content(bytes),
        });
    }
    let Some(decoded) = decode_file(&path, &bytes).map_err(|error| {
        EditorOpenError::Message(t_fmt(
            L10nKey::EditorCantRead,
            &[
                ("path", &path.display().to_string()),
                ("e", &error.to_string()),
            ],
        ))
    })?
    else {
        // A ZIP renamed away from `.epub` is still identified by its container,
        // not by its extension. A plain archive has no container and keeps the
        // existing external-open behaviour.
        if bytes.starts_with(b"PK\x03\x04") || bytes.starts_with(b"PK\x05\x06") {
            if let Ok(BookOpen::Book(book)) = book_document::load(h, path.clone(), meta) {
                return Ok(Loaded::Book(book));
            }
        }
        return Err(EditorOpenError::NotText(path));
    };
    let config = read_editorconfig(h, &path);
    Ok(Loaded::Text {
        path,
        text: decoded.text,
        format: decoded.format,
        mtime: meta.mtime,
        config,
    })
}

/// A reloaded read view that never reached a live document is closed here,
/// not left for the Host's idle reaper: a tab that closed mid-read should not
/// keep a transaction open.
fn close_unclaimed_sqlite(host: &SharedHost, result: &std::io::Result<Reloaded>) {
    if let Ok(Reloaded::Sqlite(sqlite)) = result
        && let Ok(opened) = &sqlite.opened
    {
        host.sqlite_close(opened.view);
    }
}

/// The same release for a first open whose target was closed or replaced
/// before it landed.
fn close_unclaimed_loaded(host: &SharedHost, result: &Result<Loaded, EditorOpenError>) {
    if let Ok(Loaded::Sqlite(sqlite)) = result
        && let Ok(opened) = &sqlite.opened
    {
        host.sqlite_close(opened.view);
    }
}

/// And for a restore batch that reached no live tab.
fn close_unclaimed_batch(host: &SharedHost, loaded: &[(PathBuf, Result<Loaded, EditorOpenError>)]) {
    for (_, result) in loaded {
        close_unclaimed_loaded(host, result);
    }
}

/// Opens a read-only view on the owning Host, still on its thread. The view
/// is not read or copied here: only the bounded metadata and page requests
/// that follow cross the transport. An open failure is carried into the
/// document so the reader can see the reason and retry.
fn load_sqlite(
    h: &dyn lyrel_core::host::Host,
    path: PathBuf,
    meta: lyrel_core::host::Meta,
) -> LoadedSqlite {
    let cancel = SqliteCancel::new();
    let opened = h.sqlite_open(&path, &cancel);
    LoadedSqlite {
        path,
        mtime: meta.mtime,
        opened,
    }
}

/// Recognises a book before generic text and ZIP handling.
///
/// `.epub` is a claim even when the container is broken; a `.zip` becomes a
/// book only when its package really opens, so ordinary archives retain their
/// external-open behaviour.
fn load_book_candidate(
    h: &dyn lyrel_core::host::Host,
    path: &Path,
    meta: lyrel_core::host::Meta,
) -> Result<Option<Loaded>, EditorOpenError> {
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase);
    let claimed = extension.as_deref() == Some("epub");
    if !claimed && extension.as_deref() != Some("zip") {
        return Ok(None);
    }
    if claimed {
        return book_document::load_claimed(h, path.to_path_buf(), meta)
            .map(|book| Some(Loaded::Book(book)))
            .map_err(|error| book_read_error(path, error));
    }
    match book_document::load(h, path.to_path_buf(), meta) {
        Ok(BookOpen::Book(book)) if book.is_ready() => Ok(Some(Loaded::Book(book))),
        Ok(BookOpen::Book(_)) | Ok(BookOpen::NotEpub(_)) => Ok(None),
        Err(error) => Err(book_read_error(path, error)),
    }
}

fn book_read_error(path: &Path, error: std::io::Error) -> EditorOpenError {
    let key = match error.kind() {
        std::io::ErrorKind::FileTooLarge => L10nKey::EditorFileTooLarge,
        _ => L10nKey::EditorCantRead,
    };
    EditorOpenError::Message(t_fmt(
        key,
        &[
            ("path", &path.display().to_string()),
            ("e", &error.to_string()),
            ("size", &error.to_string()),
        ],
    ))
}

/// Reads a `.drawio`/`.dio` file under the diagram budget. An oversized or
/// malformed file still becomes a visible document, not a silent text fallback.
fn load_drawio(
    h: &dyn lyrel_core::host::Host,
    path: PathBuf,
    meta: lyrel_core::host::Meta,
) -> Result<Loaded, EditorOpenError> {
    let requested = path.clone();
    if meta.len > drawio::MAX_DOCUMENT_BYTES as u64 {
        return Ok(Loaded::Drawio(LoadedDrawio {
            path,
            bytes: Arc::from([]),
            source: Err(drawio::DrawioError::TooLarge),
            mtime: meta.mtime,
        }));
    }
    let bytes = h
        .read_file(&path, drawio::MAX_DOCUMENT_BYTES as u64)
        .map_err(|error| {
            EditorOpenError::Message(t_fmt(
                L10nKey::EditorCantRead,
                &[
                    ("path", &requested.display().to_string()),
                    ("e", &error.to_string()),
                ],
            ))
        })?;
    Ok(loaded_drawio(path, meta.mtime, bytes))
}

fn loaded_drawio(path: PathBuf, mtime: Option<MTime>, bytes: Vec<u8>) -> Loaded {
    let source = drawio::parse(&bytes);
    Loaded::Drawio(LoadedDrawio {
        path,
        bytes: Arc::from(bytes),
        source,
        mtime,
    })
}

/// Reads an image under the image budget and decodes it, still on the Host's
/// thread. Only an unreadable file is an open failure; everything the decoder
/// says becomes the document's own visible state.
fn load_image(
    h: &dyn lyrel_core::host::Host,
    path: PathBuf,
    format: ImageFormat,
    meta: lyrel_core::host::Meta,
) -> Result<Loaded, EditorOpenError> {
    let requested = path.clone();
    read_image(h, path, format, meta)
        .map(Loaded::Image)
        .map_err(|error| {
            EditorOpenError::Message(t_fmt(
                L10nKey::EditorCantRead,
                &[
                    ("path", &requested.display().to_string()),
                    ("e", &error.to_string()),
                ],
            ))
        })
}

fn is_pdf_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("pdf"))
}

fn is_pdf_bytes(bytes: &[u8]) -> bool {
    bytes.starts_with(b"%PDF-")
}

/// Read a PDF under the same bounded, versioned Host session as a picture.
/// Parse/decrypt failures are part of the document, so only the read itself
/// can fail the open.
fn load_pdf(
    h: &dyn lyrel_core::host::Host,
    path: PathBuf,
    meta: lyrel_core::host::Meta,
) -> Result<Loaded, EditorOpenError> {
    let requested = path.clone();
    let shown = path.clone();
    read_pdf(h, path, meta)
        .map(|(content, mtime)| Loaded::Pdf {
            path: requested,
            mtime,
            content,
        })
        .map_err(|error| {
            EditorOpenError::Message(t_fmt(
                L10nKey::EditorCantRead,
                &[
                    ("path", &shown.display().to_string()),
                    ("e", &error.to_string()),
                ],
            ))
        })
}

fn read_pdf(
    h: &dyn lyrel_core::host::Host,
    path: PathBuf,
    meta: lyrel_core::host::Meta,
) -> std::io::Result<(PdfLoaded, Option<MTime>)> {
    let mut mtime = meta.mtime;
    if meta.len > PDF_MAX_BYTES {
        return Ok((
            PdfLoaded::Failed(t(L10nKey::EditorPdfTooLarge).to_string().into()),
            mtime,
        ));
    }
    let (bound, bytes) = images::read_bytes(
        h,
        &path,
        PDF_MAX_BYTES,
        &lyrel_core::host::transfer::CancelToken::default(),
        |_| {},
    )?;
    mtime = bound.mtime;
    Ok((pdf_content(bytes), mtime))
}

fn pdf_content(bytes: Vec<u8>) -> PdfLoaded {
    let bytes = Arc::new(bytes);
    match lyrel_pdf::PdfDocument::load(bytes.clone(), "") {
        Ok(document) => PdfLoaded::Ready(Arc::new(document)),
        Err(lyrel_pdf::LoadError::Locked) => PdfLoaded::Locked(bytes),
        Err(lyrel_pdf::LoadError::Other(message)) => PdfLoaded::Failed(message.into()),
    }
}

/// 图片的读取只有一种失败：Host 读不出来。字节读到了但不能解码，是文档
/// 自己的状态，由调用者装进缓冲区。
fn read_image(
    h: &dyn lyrel_core::host::Host,
    path: PathBuf,
    format: ImageFormat,
    meta: lyrel_core::host::Meta,
) -> std::io::Result<LoadedImage> {
    let mut mtime = meta.mtime;
    let result = if meta.len > IMAGE_MAX_BYTES {
        // Not worth reading to say they are too many.
        Err(SharedString::from(images::too_large().to_string()))
    } else {
        let (bound, bytes) = images::read_bytes(
            h,
            &path,
            IMAGE_MAX_BYTES,
            &lyrel_core::host::transfer::CancelToken::default(),
            |_| {},
        )?;
        // The picture and the mtime recorded for later external-change
        // checks come from the version that was actually opened.
        mtime = bound.mtime;
        image_document::decode_loaded(bytes, format)
    };
    Ok(LoadedImage {
        path,
        claimed: Some(format),
        mtime,
        result,
    })
}

fn decode_file(path: &Path, bytes: &[u8]) -> std::io::Result<Option<lyrel_text::Decoded>> {
    if is_markdown_path(path)
        || is_svg_path(path)
        || is_json_path(path)
        || is_yaml_path(path)
        || is_toml_path(path)
        || is_csv_path(path)
        || is_xml_path(path)
    {
        lyrel_text::decode_source(bytes)
    } else {
        Ok(lyrel_text::decode(bytes))
    }
}

/// Collects the `.editorconfig` files that apply to `path`, nearest first,
/// stopping at the first one that declares itself the root.
fn read_editorconfig(h: &dyn lyrel_core::host::Host, path: &Path) -> EditorConfig {
    let mut configs = Vec::new();
    for dir in path.ancestors().skip(1).take(EDITORCONFIG_DEPTH) {
        let file = h.join(dir, lyrel_text::EDITORCONFIG);
        let Ok(bytes) = h.read_file(&file, 64 * 1024) else {
            continue;
        };
        let contents = String::from_utf8_lossy(&bytes).into_owned();
        let root = lyrel_text::is_root(&contents);
        configs.push((dir.to_path_buf(), contents));
        if root {
            break;
        }
    }
    lyrel_text::editorconfig_for(path, &configs)
}
/// What a watcher saw at a path: the file's modification time, or that it
/// is not there any more.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Observed {
    Present(Option<MTime>),
    Missing,
}

#[derive(Debug, PartialEq, Eq)]
enum ExternalChange {
    Ignore,
    Conflict,
    Deleted,
    Reload,
}

fn classify_external_change(
    saving: bool,
    dirty: bool,
    disk_mtime: Option<MTime>,
    observed: Observed,
) -> ExternalChange {
    if saving {
        return ExternalChange::Ignore;
    }
    let observed = match observed {
        Observed::Missing => return ExternalChange::Deleted,
        Observed::Present(m) => m,
    };
    if observed.is_some() && observed == disk_mtime {
        return ExternalChange::Ignore;
    }
    if dirty {
        ExternalChange::Conflict
    } else {
        ExternalChange::Reload
    }
}

#[cfg(test)]
mod tests;
