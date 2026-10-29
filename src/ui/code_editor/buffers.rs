use super::*;
use lyrel_core::core::session::WorkspaceId;
const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024;

const RELOAD_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(200);

/// How far up the tree to look for `.editorconfig` files. Each level is one
/// round trip on a remote host, and a project nested deeper than this without
/// a `root = true` somewhere above it is not one worth stalling an open for.
const EDITORCONFIG_DEPTH: usize = 16;

/// Document identity and I/O lifetime. Tabs only retain presentation relations;
/// replacing an editing presentation does not replace the document.
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
        // Every edit also retires reads/stat results prepared for older text.
        file.reload_seq = file.reload_seq.wrapping_add(1);
        let input = file.input.read(cx);
        let text = input.text();
        let len = text.len();
        let same =
            len == file.saved.len && Fingerprint::of_chunks(len, text.chunks()) == file.saved;
        file.dirty = !same
            || file.format(cx) != file.saved_format
            || file.representation_fingerprint(cx) != file.saved_representation;
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
        let preserve = cx.try_global::<MarkdownWorkbench>().is_some();
        HostOps::run_in(
            host.clone(),
            window,
            cx,
            move |h| load_file(h, requested, preserve),
            move |view, result, window, cx| {
                let current = access(view).pending.get(&key).copied() == Some(seq);
                if !current {
                    return;
                }
                access(view).pending.remove(&key);
                if !valid(view, target, cx) {
                    return;
                }
                let Some(current) = access(view).foreground.get(&target.tab).copied() else {
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
        let preserve = cx.try_global::<MarkdownWorkbench>().is_some();
        HostOps::run_in(
            host,
            window,
            cx,
            move |host| {
                files
                    .into_iter()
                    .map(|path| (path.clone(), load_file(host, path, preserve)))
                    .collect::<Vec<_>>()
            },
            move |view, loaded, window, cx| {
                if access(view).context_epoch != epoch || !valid(view, target, cx) {
                    return;
                }
                let loaded = loaded
                    .into_iter()
                    .map(|(path, result)| match result {
                        Ok(file) => Some(file),
                        Err(error) => {
                            if !preserve || language_for_path(&path) != "markdown" {
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

    fn place_cursor(
        &mut self,
        id: BufferId,
        cursor: Option<Position>,
        target: OpenTarget,
        window: &mut Window,
        cx: &mut gpui_kit::App,
    ) {
        let Some(position) = cursor else { return };
        let Some(file) = self.get_mut(id) else { return };
        file.preview = false;
        let input = if let (Some(source), Some(group)) = (file.input.markdown(), target.group) {
            EditorInput::Markdown(file.presentations.view(source, target.tab, group, cx))
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
        if let Some(id) = self.at(host.id(), &loaded.path) {
            return (id, false);
        }
        let language = language_for_path(&loaded.path);
        let indent = loaded
            .config
            .indent(editor_text::detect_indent(&loaded.text, language));
        (
            self.create(
                host,
                loaded.path,
                None,
                loaded.text,
                loaded.format,
                loaded.config,
                indent,
                loaded.mtime,
                window,
                cx,
                access,
                edited,
            ),
            true,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn create<E: 'static>(
        &mut self,
        host: SharedHost,
        path: PathBuf,
        untitled: Option<u32>,
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
        let native_markdown = cx.try_global::<MarkdownWorkbench>().is_some()
            && (untitled.is_some() || matches!(
                path.extension()
                    .and_then(|ext| ext.to_str())
                    .map(str::to_ascii_lowercase)
                    .as_deref(),
                Some("md" | "markdown" | "mdx")
            ));
        let preview = !native_markdown && cfg.editor_markdown_preview && language == "markdown";
        let id = BufferId::new();
        let (input, popups, sub, observe) = if native_markdown {
            let document = cx.new(|_| markdown::MarkdownDocument::new(text, format));
            document.update(cx, |document, cx| {
                document.set_resource_context(host.clone(), untitled.is_none().then(|| path.clone()), cx);
            });
            let sub = cx.subscribe_in(
                &document,
                window,
                move |view, _, event: &markdown::DocumentEvent, _, cx| match event {
                    markdown::DocumentEvent::Edited(_) => {
                        access(view).note_edit(id, cx);
                        edited(view, id, cx);
                    }
                    markdown::DocumentEvent::PresentationChanged
                    | markdown::DocumentEvent::ResourceContextChanged
                    | markdown::DocumentEvent::ImagesChanged => cx.notify(),
                    markdown::DocumentEvent::FollowLink { .. }
                    | markdown::DocumentEvent::RequestAssetLocation { .. } => {},
                },
            );
            let presentation = cx.new(|cx| markdown::SourcePresentation::new(document, wrap, cx));
            presentation.update(cx, |view, cx| view.set_indent(indent, cx));
            let observe = cx.observe(&presentation, |_, _, cx| cx.notify());
            (EditorInput::Markdown(presentation), None, sub, observe)
        } else {
            let input = cx.new(|cx| {
                EditorState::new(window, cx)
                    .language(language)
                    .tab_size(tab_size(indent))
                    .line_number(true)
                    .searchable(true)
                    .replaceable(true)
                    .folding(true)
                    .soft_wrap(wrap)
                    .context_menu(false)
                    .default_value(text)
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
        let value = input.read(cx);
        let saved = Fingerprint::of_chunks(value.text().len(), value.text().chunks());
        let saved_representation = input.markdown().map(|view| {
            view.read(cx)
                .document()
                .read(cx)
                .representation_fingerprint()
        });
        self.files.push(OpenFile {
            id,
            path,
            untitled,
            host,
            input,
            presentations: MarkdownPresentations::default(),
            dirty: false,
            saved,
            format: (!native_markdown).then_some(format),
            saved_format: format,
            saved_representation,
            config,
            indent,
            disk_mtime: mtime,
            saving: false,
            save_pending: false,
            reload_seq: 0,
            conflict: None,
            preview,
            wrap,
            preview_scroll: gpui_kit::ScrollHandle::new(),
            gutter: gutter::BufferGutter::default(),
            popups,
            _assets_sub: None,
            asset_location: None,
            _sub: sub,
            _observe: observe,
            _links_sub: None,
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
        match classify_external_change(file.saving, file.dirty, file.disk_mtime, observed) {
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
        file.reload_seq = file.reload_seq.wrapping_add(1);
        let seq = file.reload_seq;
        let preserve = file.input.markdown().is_some();
        HostOps::run_in(
            host,
            window,
            cx,
            move |h| {
                let bytes = h.read_file(&target, MAX_FILE_BYTES)?;
                let decoded = decode_file(&target, &bytes, preserve)?.ok_or_else(|| {
                    std::io::Error::new(std::io::ErrorKind::InvalidData, "no longer text")
                })?;
                let mtime = h.stat(&target).ok().and_then(|meta| meta.mtime);
                Ok((decoded, mtime))
            },
            move |view,
                  result: std::io::Result<(editor_text::Decoded, Option<MTime>)>,
                  window,
                  cx| {
                let Some(file) = access(view).get_mut(id) else {
                    return;
                };
                if file.reload_seq != seq || !file.is_at(host_id, &expected) {
                    return;
                }
                let (decoded, mtime) = match result {
                    Ok(loaded) => loaded,
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
                file.disk_mtime = mtime;
                file.conflict = None;
                file.saved_format = decoded.format;
                let input = file.input.clone();
                if let Some(source) = input.markdown() {
                    source.update(cx, |source, cx| {
                        source.command(
                            markdown::Command::Reload {
                                text: decoded.text,
                                format: decoded.format,
                            },
                            window,
                            cx,
                        )
                    });
                } else {
                    file.set_format(decoded.format, cx);
                    replace_buffer_text(&input, &decoded.text, window, cx);
                }
                let file = access(view).get_mut(id).unwrap();
                let value = file.input.read(cx);
                file.saved = Fingerprint::of_chunks(value.text().len(), value.text().chunks());
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
            if let Some(view) = file.input.markdown() {
                view.read(cx).document().clone().update(cx, |document, cx| {
                    document.set_resource_context(file.host.clone(), Some(file.path.clone()), cx);
                });
            }
            file.reload_seq = file.reload_seq.wrapping_add(1);
            file.input
                .set_highlighter(language_for_path(&file.path), cx);
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
            }
        }
    }
}
/// A file read off its host, decoded, with everything needed to edit it.
pub(super) struct Loaded {
    path: PathBuf,
    text: String,
    format: TextFormat,
    mtime: Option<MTime>,
    config: EditorConfig,
}

/// Reads, sizes, decodes and configures a file, on the host's own thread.
fn load_file(
    h: &dyn lyrel_core::host::Host,
    requested: PathBuf,
    preserve: bool,
) -> Result<Loaded, EditorOpenError> {
    let path = h.canonicalize(&requested).unwrap_or(requested);
    let meta = h.stat(&path).map_err(|e| {
        EditorOpenError::Message(t_fmt(
            L10nKey::EditorCantOpen,
            &[("path", &path.display().to_string()), ("e", &e.to_string())],
        ))
    })?;
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
    let Some(decoded) = decode_file(&path, &bytes, preserve).map_err(|error| {
        EditorOpenError::Message(t_fmt(
            L10nKey::EditorCantRead,
            &[
                ("path", &path.display().to_string()),
                ("e", &error.to_string()),
            ],
        ))
    })?
    else {
        return Err(EditorOpenError::NotText(path));
    };
    let config = read_editorconfig(h, &path);
    Ok(Loaded {
        path,
        text: decoded.text,
        format: decoded.format,
        mtime: meta.mtime,
        config,
    })
}

fn decode_file(
    path: &Path,
    bytes: &[u8],
    preserve: bool,
) -> std::io::Result<Option<editor_text::Decoded>> {
    let markdown = matches!(
        path.extension()
            .and_then(|ext| ext.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("md" | "markdown" | "mdx")
    );
    if preserve && markdown {
        editor_text::decode_source(bytes)
    } else {
        Ok(editor_text::decode(bytes))
    }
}

/// Collects the `.editorconfig` files that apply to `path`, nearest first,
/// stopping at the first one that declares itself the root.
fn read_editorconfig(h: &dyn lyrel_core::host::Host, path: &Path) -> EditorConfig {
    let mut configs = Vec::new();
    for dir in path.ancestors().skip(1).take(EDITORCONFIG_DEPTH) {
        let file = h.join(dir, editor_text::EDITORCONFIG);
        let Ok(bytes) = h.read_file(&file, 64 * 1024) else {
            continue;
        };
        let contents = String::from_utf8_lossy(&bytes).into_owned();
        let root = editor_text::is_root(&contents);
        configs.push((dir.to_path_buf(), contents));
        if root {
            break;
        }
    }
    editor_text::editorconfig_for(path, &configs)
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
