use lyrel_ui_base::notification_history::NotificationWindowExt as _;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui_kit::TestSupportExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{
    Editor, EditorState, Input, InputEvent, InputState, Position, TabSize,
};
use gpui_kit::component::menu::PopupMenuItem;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, IconName, Sizable as _, h_flex, v_flex,
};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, Context, Entity, EntityId, EntityInputHandler as _, Focusable as _, PromptLevel,
    SharedString, Subscription, Window, div, px, rems,
};
use lyrel_core::core::machine::TabId;

use crate::editor_session::TabEditor;
use lyrel_document::text::{EditorConfig, Indent, LineEnding, TextFormat};
use lyrel_ui_base::host_ops::{HostId, HostOps, MTime, SharedHost, WatchSub};
use lyrel_ui_base::i18n::{L10nKey, t, t_fmt};

pub mod book_document;
pub mod book_view;
pub mod buffers;
pub mod commands;
pub mod csv_document;
pub mod document_input;
pub mod drawio_document;
pub mod drawio_engine;
pub mod image_document;
pub mod json_document;
pub mod markdown;
pub mod markdown_formula;
pub mod markdown_search;
pub mod pdf_document;
pub mod structure_edit;
pub mod structure_rows;
pub mod svg_document;
pub mod text;
pub mod toml_document;
pub mod tree_selection;
pub use tree_selection::TreeSelection;
pub mod xml_document;
pub mod yaml_document;
use buffers::Loaded;
pub use document_input::EditorInput;
use document_input::Presentations;
pub mod gutter;
pub mod nav;
pub mod outline;
pub mod overlay_render;
pub mod problems;
pub mod saving;
pub mod split;
pub use split::GroupId;
pub mod sqlite_document;
pub mod sqlite_grid;
pub mod strip;
pub mod structured_document;
pub mod structured_view;
use csv_document::{CsvDocument, CsvMode, CsvView};
use json_document::{JsonDocument, JsonMode, JsonView};
pub use saving::{AfterUnsaved, BarKind};
use saving::{EditorBar, SaveWaiter};
use svg_document::{SvgDocument, SvgMode, SvgView};
use toml_document::TomlMode;
use xml_document::XmlMode;
use yaml_document::YamlMode;
#[cfg(any(test, feature = "test-support", feature = "native-markdown-tests"))]
pub mod test_host;

pub use sqlite_document::KEY_CONTEXT as SQLITE_KEY_CONTEXT;
pub use sqlite_document::{
    CloseValue as SqliteCloseValue, FocusFilter as SqliteFocusFilter,
    InspectValue as SqliteInspectValue, Refresh as SqliteRefresh,
};

/// CSV／TSV 表格的键盘上下文与编辑动作；只在表格呈现持有键盘时生效。
pub use csv_document::KEY_CONTEXT as CSV_KEY_CONTEXT;
pub use csv_document::{
    DeleteColumn as CsvDeleteColumn, DeleteRow as CsvDeleteRow, EditCell as CsvEditCell,
    InsertColumnLeft as CsvInsertColumnLeft, InsertColumnRight as CsvInsertColumnRight,
    InsertRowAbove as CsvInsertRowAbove, InsertRowBelow as CsvInsertRowBelow,
};

/// 只读图片文档的键盘上下文，与它的取景动作一起供 keymap 使用。
pub use book_view::KEY_CONTEXT as BOOK_KEY_CONTEXT;
pub use book_view::{
    BookAddBookmark, BookCycleSpacing, BookCycleWidth, BookFontLarger, BookFontSmaller,
    BookNextChapter, BookNextPage, BookNextResult, BookPreviousChapter, BookPreviousPage,
    BookPreviousResult, BookSearch, BookToggleBookmarks, BookToggleContents,
};
/// draw.io 预览的键盘上下文：翻页与缩放只在这份图有焦点时生效。
pub use drawio_document::KEY_CONTEXT as DRAWIO_KEY_CONTEXT;
pub use drawio_document::{
    DrawioActualSize, DrawioCopy, DrawioCut, DrawioDelete, DrawioFit, DrawioPaste, DrawioRedo,
    DrawioUndo, DrawioZoomIn, DrawioZoomOut,
};
pub use image_document::KEY_CONTEXT as IMAGE_KEY_CONTEXT;
pub use image_document::{ImageActualSize, ImageFit, ImageZoomIn, ImageZoomOut};
/// JSON 结构树的键盘上下文：撤销／重做只在树持有键盘时走共享 history。
pub use json_document::KEY_CONTEXT as JSON_KEY_CONTEXT;
pub use nav::KEY_CONTEXT as NAV_KEY_CONTEXT;
pub use nav::{BreadcrumbFacts, NavOpen, path_segments};
pub use strip::{PICKER_LIST_MAX_H, PICKER_ROW_H, PICKER_W};
/// The chords that belong to the editor's text field and still have to work
/// while the header strip holds the keyboard.
pub const NAV_OR_STRIP_KEY_CONTEXT: &str = "Input || DocumentStrip";
pub use pdf_document::KEY_CONTEXT as PDF_KEY_CONTEXT;
pub use pdf_document::{PdfActualSize, PdfCopy, PdfFitPage, PdfFitWidth, PdfZoomIn, PdfZoomOut};
pub use svg_document::KEY_CONTEXT as SVG_KEY_CONTEXT;
/// TOML 结构树的键盘上下文。
pub use toml_document::KEY_CONTEXT as TOML_KEY_CONTEXT;
/// XML 结构树的键盘上下文：撤销／重做只在树持有键盘时走共享 history。
pub use xml_document::KEY_CONTEXT as XML_KEY_CONTEXT;
/// YAML 结构树的键盘上下文。
pub use yaml_document::KEY_CONTEXT as YAML_KEY_CONTEXT;

/// Retained document identity, independent of any editing presentation or path.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BufferId(pub(crate) u64);

impl BufferId {
    pub fn new() -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }

    /// Stable numeric identity for the desktop element tree.
    pub fn as_u64(self) -> u64 {
        self.0
    }
}

impl Default for BufferId {
    fn default() -> Self {
        Self::new()
    }
}

/// What a buffer's text looked like the last time it matched the disk.
///
/// Dirtiness is a comparison against this rather than a flag set by the first
/// keystroke, so typing a character and deleting it again — or undoing back to
/// the saved text — leaves the file clean, the way every other editor does.
/// The length is checked first: it is free, and it settles almost every
/// keystroke without hashing the whole file.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Fingerprint {
    len: usize,
    hash: u64,
}

impl Fingerprint {
    fn of_str(text: &str) -> Self {
        Self::of_chunks(text.len(), std::iter::once(text))
    }

    fn of_bytes(bytes: &[u8]) -> Self {
        use std::hash::Hasher as _;
        let mut hasher = std::hash::DefaultHasher::new();
        hasher.write(bytes);
        Self {
            len: bytes.len(),
            hash: hasher.finish(),
        }
    }

    fn of_chunks<'a>(len: usize, chunks: impl Iterator<Item = &'a str>) -> Self {
        use std::hash::Hasher as _;
        // SipHash buffers its input, so the same bytes hash the same however
        // the rope happens to have split them into chunks.
        let mut hasher = std::hash::DefaultHasher::new();
        for chunk in chunks {
            hasher.write(chunk.as_bytes());
        }
        Self {
            len,
            hash: hasher.finish(),
        }
    }
}

/// Why the buffer and the file on disk no longer agree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiskConflict {
    /// Something else wrote the file while the buffer had edits of its own.
    /// Carries the modification time that was seen, so "Keep mine" can accept
    /// exactly that version as the one a save may replace.
    Changed(Option<MTime>),
    /// The file is gone. The buffer is all that is left of it.
    Deleted,
}

/// How long a deferred diagram save waits for the engine to confirm its
/// current model. A test deadline stays short so a stalled flush is proven
/// without sleeping the production three seconds. The same gate covers the
/// crate compiled as a dependency of the root whole-window and native test
/// harnesses (`cfg(test)` is false there).
const DRAWIO_FLUSH_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(
    if cfg!(any(
        test,
        feature = "test-support",
        feature = "native-markdown-tests"
    )) {
        60
    } else {
        3_000
    },
);

/// A save that must not capture bytes older than the engines' live models.
/// It records one ticket per presentation that accepted its own sync command
/// and waits for each of those exact confirmations, with a deadline so a dead
/// surface cannot wedge the whole save path.
#[derive(Clone)]
struct DrawioFlush {
    tickets: Vec<(gpui_kit::WeakEntity<drawio_document::DrawioView>, u64)>,
    /// A canvas already known to hold a refused model when the save started;
    /// no ticket can repair it, so the save fails without writing.
    refused: Option<gpui_kit::SharedString>,
    deadline: std::time::Instant,
    force: bool,
    target: Option<PathBuf>,
}

/// One open file, shared by every tab that shows it.
///
/// Buffers used to belong to a tab, which meant the same file open in two tabs
/// was two independent copies: edits in one were invisible to the other, and
/// saving either silently threw the other's away. Now a tab only lists which
/// buffers it shows, and the text lives here once.
pub struct OpenFile {
    pub id: BufferId,
    pub path: PathBuf,
    /// Set for a file that has never been saved; `path` is empty until then.
    pub untitled: Option<u32>,
    /// The machine `path` lives on, held rather than looked up. Saves,
    /// reloads and duplicate detection all key on its id — an SFTP file and a
    /// local file can share the string `/etc/hosts` without being the same
    /// file — and saving goes straight through this handle, so a buffer stays
    /// saveable however the window's own machine has changed underneath it.
    pub host: SharedHost,
    pub input: EditorInput,
    pub presentations: Presentations,
    pub markdown_resources: Option<Entity<markdown::images::ImageHost>>,
    pub dirty: bool,
    saved: Fingerprint,
    /// Ordinary code retains its codec metadata here. Markdown owns it in its document.
    pub format: Option<TextFormat>,
    pub saved_format: TextFormat,
    pub saved_representation: Option<u64>,
    pub config: EditorConfig,
    pub indent: Indent,
    pub disk_mtime: Option<MTime>,
    pub saving: bool,
    pub save_pending: bool,
    pub save_as_pending: Option<PathBuf>,
    pub reload_seq: u64,
    pub reload_request: u64,
    /// A save deferred until the engine confirms its latest serialization
    /// committed through the document. Absent for every other input.
    drawio_flush: Option<DrawioFlush>,
    /// Set when the deferred save's flush was confirmed and the write is
    /// being re-entered: the engine must not be asked to sync a second time.
    pub drawio_flush_done: bool,
    pub conflict: Option<DiskConflict>,
    pub wrap: bool,
    /// Git change markers — see `gutter`.
    pub gutter: gutter::BufferGutter,
    pub popups: Option<Entity<crate::lsp::popups::EditorPopups>>,
    // Held so the subscriptions outlive the file; constructed only by the
    // crate's own buffer factories (`code_editor::buffers`).
    _sub: Subscription,
    _observe: Subscription,
}

impl OpenFile {
    pub fn id(&self) -> BufferId {
        self.id
    }

    #[cfg(any(test, feature = "test-support", feature = "native-markdown-tests"))]
    pub fn is_waiting_for_drawio_save(&self) -> bool {
        self.drawio_flush.is_some()
    }

    /// The format the next save writes back, for documents that have one.
    /// A read-only picture has no text format and nothing to write.
    pub fn format(&self, cx: &gpui_kit::App) -> Option<TextFormat> {
        self.input
            .source_document(cx)
            .map(|document| document.read(cx).format())
            .or(self.format)
    }

    fn representation_fingerprint(&self, cx: &gpui_kit::App) -> Option<u64> {
        self.input
            .source_document(cx)
            .map(|document| document.read(cx).representation_fingerprint())
    }

    fn set_format(&mut self, format: TextFormat, cx: &mut gpui_kit::App) {
        if let Some(view) = self.input.markdown() {
            view.update(cx, |view, cx| view.set_format(format, cx));
        } else if let Some(view) = self.input.json() {
            view.update(cx, |view, cx| view.set_format(format, cx));
        } else if let Some(view) = self.input.yaml() {
            view.update(cx, |view, cx| view.set_format(format, cx));
        } else if let Some(view) = self.input.toml() {
            view.update(cx, |view, cx| view.set_format(format, cx));
        } else if let Some(view) = self.input.svg() {
            view.update(cx, |view, cx| view.set_format(format, cx));
        } else if let Some(view) = self.input.csv() {
            view.update(cx, |view, cx| view.set_format(format, cx));
        } else if let Some(view) = self.input.xml() {
            view.update(cx, |view, cx| view.set_format(format, cx));
        } else {
            self.format = Some(format);
        }
    }

    pub fn label(&self) -> SharedString {
        if let Some(n) = self.untitled {
            return t_fmt(L10nKey::EditorUntitled, &[("n", &n.to_string())]).into();
        }
        self.path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| self.path.display().to_string())
            .into()
    }

    pub fn is_untitled(&self) -> bool {
        self.untitled.is_some()
    }

    fn is_at(&self, host: HostId, path: &Path) -> bool {
        self.untitled.is_none() && self.host.id() == host && self.path == path
    }

    pub fn language(&self) -> &'static str {
        if self.input.markdown().is_some() {
            return "markdown";
        }
        // Save-as or rename does not rebuild this document; it stays SVG until
        // the buffer is released and reopened by extension.
        if self.input.svg().is_some() {
            return "svg";
        }
        if self.input.json().is_some() {
            return "json";
        }
        // YAML/TOML survive a rename or Save As as the document they already are.
        if self.input.yaml().is_some() {
            return "yaml";
        }
        if self.input.toml().is_some() {
            return "toml";
        }
        if self.input.xml().is_some() {
            return "xml";
        }
        // CSV and TSV keep the route chosen at open; a rename is not a
        // delimiter probe.
        if self.input.csv().is_some() {
            return match csv_delimiter(&self.path) {
                b'\t' => "tsv",
                _ => "csv",
            };
        }
        if self.untitled.is_some() {
            return "text";
        }
        language_for_path(&self.path)
    }

    pub fn local(&self) -> Option<LocalFile> {
        if self.untitled.is_some() || !self.host.id().is_local() {
            return None;
        }
        Some(LocalFile {
            id: self.id(),
            input: self.input.code()?.clone(),
            popups: self.popups.as_ref()?.clone(),
            path: self.path.clone(),
            language: self.language(),
            indent: self.indent,
        })
    }
}

/// A saved file on this machine, as the language servers see it.
pub struct LocalFile {
    pub(crate) id: BufferId,
    pub(crate) input: Entity<EditorState>,
    pub(crate) popups: Entity<crate::lsp::popups::EditorPopups>,
    pub(crate) path: PathBuf,
    pub(crate) language: &'static str,
    pub(crate) indent: Indent,
}

impl LocalFile {
    pub fn id(&self) -> BufferId {
        self.id
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn input(&self) -> &Entity<EditorState> {
        &self.input
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn into_input(self) -> Entity<EditorState> {
        self.input
    }
}

/// Where one editor group's header strip has the keyboard, and the document
/// that was in front when it got there. The pair is what lets a group keep its
/// own cell across a focus swap or a group switch, while a change made
/// anywhere else — a quick open, a link, the list — moves the target back to
/// the new document in front instead of leaving the strip on a cell the user
/// did not choose there.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StripNav {
    pub(crate) cell: BufferId,
    pub(crate) active: BufferId,
}

/// One tab's view of the editor: which buffers it shows, in the order its
/// strip draws them, and which one is in front.
pub struct TabCode {
    pub group: split::GroupId,
    pub visible: bool,
    pub files: Vec<BufferId>,
    pub active: usize,
    pub roots: Vec<PathBuf>,
    pub expanded: std::collections::HashSet<PathBuf>,
    pub selected: crate::code_editor::TreeSelection,
    /// The second editor group, when the editor is split — the one without
    /// the focus. `files` and `active` above are always the focused group's
    /// (see `split`).
    pub split: Option<split::OtherGroup>,
    /// Buffers in the order they were last in front, most recent first —
    /// what decides which files keep a place in the header's strip when
    /// it cannot hold them all (see `strip`).
    pub recent: Vec<BufferId>,
    /// Where the keyboard sits in each editor group's header strip: the
    /// document whose cell is that group's roving Tab stop, and the document
    /// that was in front when it was chosen. Keyed by group, so a focus swap
    /// or a group switch leaves the other group's place alone. A document
    /// that is closed, hidden, or no longer in the strip is skipped when the
    /// strip is next drawn (see `strip`).
    pub strip_nav: std::collections::HashMap<split::GroupId, StripNav>,
}

impl TabCode {
    pub fn new() -> Self {
        Self {
            group: split::GroupId::default(),
            visible: false,
            files: Vec::new(),
            active: 0,
            roots: Vec::new(),
            expanded: std::collections::HashSet::new(),
            selected: Default::default(),
            split: None,
            recent: Vec::new(),
            strip_nav: Default::default(),
        }
    }

    /// The identity of the focused editor group: stable across focus swaps
    /// and group switches, so presentations and navigation key on it.
    pub fn group(&self) -> split::GroupId {
        self.group
    }

    pub fn active_id(&self) -> Option<BufferId> {
        self.files.get(self.active).copied()
    }

    /// Adds a buffer just right of the one in front, the way a browser opens
    /// a tab, and brings it forward. One the tab already shows only comes
    /// forward — it keeps its place in the strip.
    pub fn show(&mut self, id: BufferId) {
        if let Some(pos) = self.files.iter().position(|f| *f == id) {
            self.active = pos;
            return;
        }
        let at = if self.files.is_empty() {
            0
        } else {
            (self.active + 1).min(self.files.len())
        };
        self.files.insert(at, id);
        self.active = at;
    }

    /// Lists a buffer at the end of the strip without bringing it forward —
    /// for files arriving in the background: a restore, a merge, a rescue.
    pub fn append(&mut self, id: BufferId) {
        if !self.files.contains(&id) {
            self.files.push(id);
        }
    }

    /// Lists another strip's files after this one's, in their order.
    pub fn adopt(&mut self, ids: &[BufferId]) {
        for id in ids {
            self.append(*id);
        }
    }

    /// Takes a buffer out of the strip. The neighbour that slides into its
    /// place comes forward, so closing tabs one after another walks along the
    /// strip rather than jumping about.
    fn forget(&mut self, id: BufferId) -> bool {
        let Some(pos) = self.files.iter().position(|f| *f == id) else {
            return false;
        };
        self.files.remove(pos);
        if self.active > pos || self.active >= self.files.len() {
            self.active = self.active.saturating_sub(1);
        }
        true
    }
}

impl Default for TabCode {
    fn default() -> Self {
        Self::new()
    }
}

/// Cross-panel intents the editor panel hands to the workbench shell.
///
/// Payloads carry only values — identities, paths, and the captured
/// `AfterUnsaved` intent — never shell entities or callbacks. `LyrelApp`
/// subscribes to these at the window root and performs the shell-side
/// effect (tab placement, terminal focus, LSP and SCM notifications).
pub enum EditorPanelEvent {
    /// The buffer's text changed; the shell runs its LSP edit bookkeeping.
    BufferEdited { id: BufferId },
    /// A save finished writing; the shell notifies LSP and SCM.
    BufferSaved {
        id: BufferId,
        host: HostId,
        directory: Option<PathBuf>,
    },
    /// Save As changed the path; the shell refreshes Host watches and LSP.
    IdentityChanged,
    /// The rename bar was answered; the shell runs the LSP rename.
    RenameRequested {
        id: BufferId,
        offset: usize,
        name: String,
    },
    /// The unsaved-changes prompt was answered; the close continues.
    Continue(AfterUnsaved),
    /// These buffers were discarded from the prompt; the shell forgets them
    /// in every tab and the editor drops them from the registry.
    BuffersDiscarded { ids: Vec<BufferId> },
    /// An asynchronous open landed; the shell validates the tab/group and
    /// places the buffer, or ignores a stale target.
    OpenCompleted {
        workspace: lyrel_core::core::session::WorkspaceId,
        window_host: HostId,
        tab: TabId,
        group: Option<split::GroupId>,
        id: BufferId,
        created: bool,
        front: bool,
    },
    /// A tab's recorded files finished restoring; the shell merges them into
    /// the tab's groups and takes the focus when nothing else did.
    TabRestored {
        workspace: lyrel_core::core::session::WorkspaceId,
        window_host: HostId,
        tab: TabId,
        open: Vec<(PathBuf, BufferId, bool)>,
        snapshot: crate::editor_session::TabEditor,
    },
    /// A Markdown link requested a file in its originating presentation.
    FollowLink {
        buffer: BufferId,
        source: EntityId,
        host: SharedHost,
        path: PathBuf,
        anchor: Option<String>,
        revision: u64,
    },
    /// Type `@path` into the running agent's prompt.
    AttachToAgent { path: PathBuf, suffix: String },
    /// Hand a file the editor cannot show to the OS/desktop.
    OpenExternally { host: HostId, path: PathBuf },
    /// The file watcher must be rebuilt against the current window host.
    RebuildWatcher,
    /// A cancelled save prompt or an answered bar returns the keyboard to the
    /// front document.
    FocusDocument,
    /// A status control requests a command on the foreground document.
    StatusCommand(EditorStatusCommand),
    /// The go-to-line bar was answered; the shell focuses and places the
    /// cursor, exactly as the old bar handler did.
    GoToLineRequested {
        id: BufferId,
        line: u32,
        column: u32,
    },
    /// A language server named one place to go (a single definition or
    /// reference): open it in the editor at the 1-based source position,
    /// through the same shell path a `file.rs:12:5` link takes.
    OpenLocationAt {
        path: PathBuf,
        line: u32,
        column: u32,
    },
    /// A language server named several places (references, or definitions
    /// with more than one answer): list them in the shell search's Locations
    /// tab. `root` is the server's project root, for paths shown relative.
    ShowLocations {
        heading: L10nKey,
        root: PathBuf,
        locations: Vec<crate::lsp::Found>,
    },
    /// The answered `workspace/symbol` query: project-wide symbol hits (with
    /// the front file's own already filtered out) for the shell's Symbols
    /// tab. An empty list clears the tab's project rows. `root` is the
    /// server's project root, for paths shown relative.
    ShowWorkspaceSymbols {
        root: PathBuf,
        hits: Vec<(crate::lsp::Found, crate::lsp::WorkspaceHit)>,
    },
}

#[derive(Clone, Copy)]
pub enum EditorStatusCommand {
    TogglePreview,
    ToggleWrap,
    ToggleLineEnding,
    GoToLine,
}

impl gpui_kit::EventEmitter<EditorPanelEvent> for EditorPanelState {}

pub struct EditorPanelState {
    pub buffers: buffers::EditorBuffers,
    pub bar: Option<EditorBar>,
    pub waiters: Vec<SaveWaiter>,
    pub unsaved_prompt_open: bool,
    pub prompt_epoch: u64,
    /// Tabs whose remembered files have been reopened (or found to have none).
    pub restored: HashSet<TabId>,
    /// What was last written to the session store for each tab, so an
    /// unchanged tab costs a comparison per frame and nothing more.
    pub recorded: HashMap<TabId, TabEditor>,
    /// Back/forward history, outlines and Go to Symbol's preview.
    pub nav: nav::EditorNav,
    /// The Problems list at the foot of the panel — see `problems`.
    pub problems: problems::ProblemsPane,
    /// The header's list of every open file, while it is open.
    pub strip_picker: Option<strip::StripPicker>,
    /// The keyboard, while it is in the shared header strip. The strip is one
    /// Tab stop for the whole focused group (see `strip`); the document it
    /// is on is each group's own remembered cell.
    pub strip_keyboard: gpui_kit::FocusHandle,
    /// The focus of a panel with no file in it. Without one there is nothing
    /// in the panel to hold the focus, so closing the last file handed it to
    /// the terminal and the next ⌘W closed the terminal instead of the panel.
    pub empty_focus: gpui_kit::FocusHandle,
    /// The tab whose panel held the focus when it was last drawn, so the
    /// frame after its last file closes knows whether to take the focus over
    /// — and a tab switched to does not take it from its own terminal.
    pub had_focus: Option<TabId>,
}

impl EditorPanelState {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let buffers = buffers::EditorBuffers::new(window, cx);
        Self {
            buffers,
            bar: None,
            waiters: Vec::new(),
            unsaved_prompt_open: false,
            prompt_epoch: 0,
            restored: HashSet::new(),
            recorded: HashMap::new(),
            nav: nav::EditorNav::new(cx),
            problems: Default::default(),
            strip_picker: None,
            strip_keyboard: cx.focus_handle().tab_stop(true),
            empty_focus: cx.focus_handle(),
            had_focus: None,
        }
    }

    fn buffer_edited(&mut self, id: BufferId, cx: &mut Context<Self>) {
        self.buffer_changed(id, cx);
    }

    /// Re-derives everything an edit of any kind touches inside the panel —
    /// gutter baseline, navigation history — and tells the shell about the
    /// edit so its LSP bookkeeping can run.
    fn buffer_changed(&mut self, id: BufferId, cx: &mut Context<Self>) {
        self.gutter_note_edit(id, cx);
        self.nav_note_edit(id, cx);
        cx.emit(EditorPanelEvent::BufferEdited { id });
        cx.notify();
    }

    /// File-tree dirtiness is Host-aware, like duplicate detection.
    pub fn is_dirty(&self, host: HostId, path: &Path) -> bool {
        self.buffers.is_dirty(host, path)
    }

    /// The buffer registry, for the shell's read-side questions (dirty
    /// scans, labels, gutter roots).
    pub fn buffers(&self) -> &buffers::EditorBuffers {
        &self.buffers
    }

    pub fn take_watch_refresh(&mut self) -> bool {
        self.buffers.take_watch_refresh()
    }

    pub fn strip_keyboard(&self) -> &gpui_kit::FocusHandle {
        &self.strip_keyboard
    }

    pub fn strip_keyboard_focused(&self, window: &Window) -> bool {
        self.strip_keyboard.is_focused(window)
    }

    /// Whether a document-navigation surface (the strip picker or a bar) is
    /// open above the panel — the shell's overlay question.
    pub fn navigation_overlay_open(&self) -> bool {
        self.strip_picker.is_some() || self.bar.is_some()
    }

    /// The focus of a panel with no file in it; the shell parks the keyboard
    /// there when the last file closes.
    pub fn empty_focus(&self) -> &gpui_kit::FocusHandle {
        &self.empty_focus
    }

    /// The tab whose panel held the focus when it was last drawn, so the
    /// frame after its last file closes knows whether to take the focus over.
    pub fn had_focus(&self) -> Option<TabId> {
        self.had_focus
    }

    pub fn set_had_focus(&mut self, tab: Option<TabId>) {
        self.had_focus = tab;
    }

    /// The workspace/host changed: stale pending opens are dropped, every
    /// buffer's reload generation moves, and open prompts retire.
    pub fn invalidate_context(&mut self) {
        self.buffers.invalidate_context();
        self.retire_prompts();
    }

    /// Only saved, local files are visible to language servers.
    pub fn local_files(&self) -> Vec<LocalFile> {
        self.buffers.iter().filter_map(OpenFile::local).collect()
    }

    pub fn local_file(&self, id: BufferId) -> Option<LocalFile> {
        self.buffers.get(id).and_then(OpenFile::local)
    }
}

#[derive(Clone, Copy)]
pub enum BufferKind {
    Text,
    Markdown,
}

pub fn is_markdown_path(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            ["md", "markdown", "mdx"]
                .iter()
                .any(|candidate| ext.eq_ignore_ascii_case(candidate))
        })
}

/// The conventional SQLite database extensions. The owning Host opens the
/// database read-only; failed opens keep a visible document and explanation.
pub fn is_sqlite_path(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            ["sqlite", "sqlite3", "db", "db3"]
                .iter()
                .any(|candidate| ext.eq_ignore_ascii_case(candidate))
        })
}

/// SVG source has its own document route: the editor with a native preview,
/// never a browser document. The extension is the only routing key here, so a
/// corrupt file still opens where its source can be corrected.
pub fn is_svg_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("svg"))
}

/// JSON-family source has its own document route: one plain-source Document
/// plus a lossless structure/record view. The extension decides the route, so
/// a temporarily invalid file still opens where it can be repaired.
pub fn is_json_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            ["json", "jsonc", "jsonl", "ndjson"]
                .iter()
                .any(|candidate| extension.eq_ignore_ascii_case(candidate))
        })
}

/// YAML source has its own document route: one plain-source Document plus a
/// lossless structure tree that keeps anchors, aliases, tags and documents.
pub fn is_yaml_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            extension.eq_ignore_ascii_case("yaml") || extension.eq_ignore_ascii_case("yml")
        })
}

/// TOML documents have one plain-source Document and a lossless structure
/// view. The extension decides the route, so a temporarily invalid file still
/// opens where it can be repaired.
pub fn is_toml_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("toml"))
}

/// XML source has its own document route: one plain-source Document plus an
/// ordered, namespace-aware structure tree. The extension decides the route,
/// so a temporarily invalid file still opens where it can be repaired.
pub fn is_xml_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("xml"))
}

/// CSV/TSV records have one source authority and a derived table. The
/// extension chooses the delimiter and the route; content never turns an
/// ordinary text file into a table.
pub fn is_csv_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| matches!(extension.to_ascii_lowercase().as_str(), "csv" | "tsv"))
}

pub fn csv_delimiter(path: &Path) -> u8 {
    match path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("tsv"))
    {
        true => b'\t',
        false => b',',
    }
}

/// draw.io documents have their own read-only engine route. `.drawio` and
/// `.dio` are unambiguous; a bare `.xml` needs the bounded content probe in
/// `buffers::load_file`, so it is not claimed here.
pub fn is_drawio_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            extension.eq_ignore_ascii_case("drawio") || extension.eq_ignore_ascii_case("dio")
        })
}

pub fn language_for_path(path: &Path) -> &'static str {
    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
        let lowered = name.to_ascii_lowercase();
        match lowered.as_str() {
            "makefile" | "gnumakefile" => return "make",
            "cmakelists.txt" => return "cmake",
            _ => {}
        }
        if lowered.starts_with('.') && (lowered.contains("shrc") || lowered.ends_with("profile")) {
            return "bash";
        }
    }
    let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
        return "text";
    };
    match ext.to_ascii_lowercase().as_str() {
        "rs" => "rust",
        "go" => "go",
        "py" | "pyi" => "python",
        "js" | "mjs" | "cjs" | "jsx" => "javascript",
        "ts" | "mts" | "cts" => "typescript",
        "tsx" => "tsx",
        "json" => "json",
        "jsonc" => "jsonc",
        "jsonl" | "ndjson" => "jsonl",
        "xml" => "xml",
        "toml" => "toml",
        "yaml" | "yml" => "yaml",
        "csv" => "csv",
        "tsv" => "tsv",
        "html" | "htm" => "html",
        "svg" => "svg",
        "css" => "css",
        "md" | "markdown" | "mdx" => "markdown",
        "sh" | "bash" | "zsh" => "bash",
        "c" | "h" => "c",
        "cpp" | "cc" | "cxx" | "hpp" | "hh" => "cpp",
        "java" => "java",
        "kt" | "kts" => "kotlin",
        "lua" => "lua",
        "rb" => "ruby",
        "php" => "php",
        "sql" => "sql",
        "swift" => "swift",
        "scala" => "scala",
        "zig" => "zig",
        "proto" => "proto",
        "diff" | "patch" => "diff",
        "ex" | "exs" => "elixir",
        "erb" => "erb",
        "ejs" => "ejs",
        "svelte" => "svelte",
        "astro" => "astro",
        "graphql" | "gql" => "graphql",
        "cs" => "csharp",
        "cmake" | "mk" => "cmake",
        _ => "text",
    }
}

/// What the status bar calls a language id from [`language_for_path`].
/// `None` for plain text, which the bar leaves unsaid.
pub fn language_label(id: &str) -> Option<&'static str> {
    Some(match id {
        "rust" => "Rust",
        "go" => "Go",
        "python" => "Python",
        "javascript" => "JavaScript",
        "typescript" => "TypeScript",
        "tsx" => "TypeScript JSX",
        "json" => "JSON",
        "jsonc" => "JSONC",
        "jsonl" => "JSONL",
        "xml" => "XML",
        "toml" => "TOML",
        "yaml" => "YAML",
        "csv" => "CSV",
        "tsv" => "TSV",
        "html" => "HTML",
        "svg" => "SVG",
        "css" => "CSS",
        "markdown" => "Markdown",
        "bash" => "Shell",
        "c" => "C",
        "cpp" => "C++",
        "java" => "Java",
        "kotlin" => "Kotlin",
        "lua" => "Lua",
        "ruby" => "Ruby",
        "php" => "PHP",
        "sql" => "SQL",
        "swift" => "Swift",
        "scala" => "Scala",
        "zig" => "Zig",
        "proto" => "Protocol Buffers",
        "diff" => "Diff",
        "elixir" => "Elixir",
        "erb" => "ERB",
        "ejs" => "EJS",
        "svelte" => "Svelte",
        "astro" => "Astro",
        "graphql" => "GraphQL",
        "csharp" => "C#",
        "cmake" => "CMake",
        "make" => "Makefile",
        _ => return None,
    })
}

/// How many frames a jump-to-line may wait for the editor to be laid out.
///
/// `InputState::scroll_to` gives up when the buffer has never been painted,
/// and that is exactly the state a file that just opened is in: the cursor
/// lands on the right line and the view stays at the top of the file, which
/// is the one thing a `:120` link exists to avoid. Three frames is the same
/// bounded-retry shape `prefill::select_all_when_drawn` uses against the same
/// class of problem.
pub const CURSOR_SCROLL_ATTEMPTS: u8 = 3;

/// Puts the cursor at `position`, re-trying on later frames until the scroll
/// that follows it can actually be computed.
///
/// Applied immediately as well as on the retry: the cursor itself lands with
/// no layout, so the position is right even for a pane that never paints.
pub fn place_cursor(
    input: EditorInput,
    position: Position,
    left: u8,
    window: &mut Window,
    cx: &mut gpui_kit::App,
) {
    let Some(text) = input.text() else {
        // A picture has no caret to put down: the request is about text.
        return;
    };
    text.set_cursor_position(position, window, cx);
    // Native source presentations retain a view-local reveal request until
    // layout paints. Replaying their selection on later frames would overwrite
    // newer navigation.
    if input.text().is_some_and(|text| text.is_source()) {
        return;
    }
    // A target near the top of the file scrolls nowhere and is already done;
    // so is one that has landed. Either way this stops.
    if left <= 1 || input.read(cx).scroll_offset().y != px(0.) {
        return;
    }
    window.on_next_frame(move |window, cx| {
        place_cursor(input, position, left - 1, window, cx);
    });
    // Registering a callback does not by itself ask for a frame, and an
    // overlay that has finished drawing has no other reason to produce one.
    window.refresh();
}

/// Replaces a buffer's text with `new` as one ordinary edit.
///
/// `InputState::set_value` would be simpler, and it clears the undo history —
/// so a file reloaded because an agent or a `git checkout` touched it could no
/// longer be undone past that moment. Replacing only the span that differs is
/// an edit like any other: it goes on the undo stack, and the cursor and
/// scroll stay where they were for everything outside it.
fn replace_buffer_text(
    input: &EditorInput,
    new: &str,
    window: &mut Window,
    cx: &mut gpui_kit::App,
) {
    let Some(text) = input.text() else {
        // Nothing to write into: a picture holds bytes, not editable text.
        return;
    };
    let state = text.read(cx);
    let old = state.text().to_string();
    let Some((start, old_end, new_end)) = differing_span(&old, new) else {
        return;
    };
    let cursor = state.cursor_position();
    text.replace(start..old_end, &new[start..new_end], window, cx);
    text.set_cursor_position(cursor, window, cx);
}

/// The byte span where `old` and `new` differ: `(start, end in old, end in
/// new)`, on character boundaries in both. `None` when they are equal.
fn differing_span(old: &str, new: &str) -> Option<(usize, usize, usize)> {
    if old == new {
        return None;
    }
    let mut start = old
        .bytes()
        .zip(new.bytes())
        .take_while(|(a, b)| a == b)
        .count();
    while !old.is_char_boundary(start) || !new.is_char_boundary(start) {
        start -= 1;
    }
    let max_suffix = old.len().min(new.len()) - start;
    let mut suffix = old
        .bytes()
        .rev()
        .zip(new.bytes().rev())
        .take(max_suffix)
        .take_while(|(a, b)| a == b)
        .count();
    while !old.is_char_boundary(old.len() - suffix) || !new.is_char_boundary(new.len() - suffix) {
        suffix -= 1;
    }
    Some((start, old.len() - suffix, new.len() - suffix))
}

/// Parses what the go-to-line bar was given: `120`, `120:4` or `:120`.
/// One-based, as every compiler and the status bar count.
fn parse_line_target(text: &str) -> Option<(u32, u32)> {
    let text = text.trim().trim_start_matches(':');
    let mut parts = text.splitn(2, [':', ',']);
    let line: u32 = parts.next()?.trim().parse().ok()?;
    let column = match parts.next() {
        Some(c) if !c.trim().is_empty() => c.trim().parse().ok()?,
        _ => 1,
    };
    (line > 0).then_some((line, column.max(1)))
}

/// Why the built-in editor could not take a file.
enum EditorOpenError {
    /// Not text, so the editor was never the right place for it.
    NotText(PathBuf),
    /// Something already worded for the user.
    Message(String),
}

fn tab_size(indent: Indent) -> TabSize {
    TabSize {
        tab_size: indent.size.max(1),
        hard_tabs: indent.hard_tabs,
    }
}

/// Whether handing this path to the desktop would run it rather than show it.
///
/// The execute bit is what `open` reads to decide between displaying a file
/// and launching it; Windows has no such bit, so there the extension is the
/// only thing that says so.
pub fn is_program(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        return std::fs::metadata(path)
            .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0);
    }
    #[cfg(not(unix))]
    {
        let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
            return false;
        };
        matches!(
            ext.to_ascii_lowercase().as_str(),
            "exe"
                | "com"
                | "bat"
                | "cmd"
                | "scr"
                | "pif"
                | "msi"
                | "ps1"
                | "vbs"
                | "js"
                | "jse"
                | "wsf"
                | "wsh"
                | "cpl"
                | "msc"
                | "hta"
                | "reg"
                | "lnk"
        )
    }
}

/// A file a browser renders rather than showing its source. SVG is not one:
/// it opens as a source document with its own native preview, so a corrupt or
/// remote SVG never falls through to the system browser or its network access.
pub fn opens_in_browser(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| matches!(e.to_ascii_lowercase().as_str(), "html" | "htm" | "xhtml"))
}

/// Opens a local file in the default web browser. On macOS that is asked
/// for by name: what `.html` is associated with is as often an editor. The
/// other desktops go by the association.
pub fn open_in_browser(path: &Path) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    if let Some(bundle) = default_browser_bundle_id() {
        std::process::Command::new("open")
            .arg("-b")
            .arg(bundle)
            .arg(path)
            .spawn()?;
        return Ok(());
    }
    lyrel_ui_base::desktop_file::open_file_path(path)
}

/// The app that handles `https:` links — the default browser.
#[cfg(target_os = "macos")]
fn default_browser_bundle_id() -> Option<String> {
    use core_foundation::base::TCFType as _;
    use core_foundation::string::{CFString, CFStringRef};

    #[link(name = "CoreServices", kind = "framework")]
    unsafe extern "C" {
        fn LSCopyDefaultHandlerForURLScheme(scheme: CFStringRef) -> CFStringRef;
    }
    let scheme = CFString::new("https");
    let handler = unsafe { LSCopyDefaultHandlerForURLScheme(scheme.as_concrete_TypeRef()) };
    // A Copy function: the string is ours to release, which the wrapper does.
    (!handler.is_null()).then(|| unsafe { CFString::wrap_under_create_rule(handler) }.to_string())
}

/// The lines a selection covers, 1-based and inclusive. One that ends at the
/// very start of a line — a whole-line selection — does not take that line.
fn selected_lines(
    text: &(impl Clone + Into<text::TextSnapshot>),
    range: std::ops::Range<usize>,
) -> Option<(usize, usize)> {
    let text = text.clone().into();
    if range.is_empty() {
        return None;
    }
    let start = text.offset_to_point(range.start);
    let end = text.offset_to_point(range.end);
    let last = match end.column == 0 && end.row > start.row {
        true => end.row - 1,
        false => end.row,
    };
    Some((start.row + 1, last + 1))
}

impl EditorPanelState {
    /// The buffer in front of the given tab's editor.
    pub fn active_file(&self, code: &TabCode) -> Option<&OpenFile> {
        self.buffers.get(code.active_id()?)
    }

    pub fn markdown_active(&self, code: &TabCode) -> bool {
        self.active_file(code)
            .is_some_and(|file| file.input.markdown().is_some())
    }

    pub fn image_active(&self, code: &TabCode) -> bool {
        self.active_file(code)
            .is_some_and(|file| file.input.image().is_some())
    }

    pub fn sqlite_active(&self, code: &TabCode) -> bool {
        self.active_file(code)
            .is_some_and(|file| file.input.sqlite().is_some())
    }

    pub fn pdf_active(&self, code: &TabCode) -> bool {
        self.active_file(code)
            .is_some_and(|file| file.input.pdf().is_some())
    }

    pub fn svg_active(&self, code: &TabCode) -> bool {
        self.active_file(code)
            .is_some_and(|file| file.input.svg().is_some())
    }

    pub fn json_active(&self, code: &TabCode) -> bool {
        self.active_file(code)
            .is_some_and(|file| file.input.json().is_some())
    }

    pub fn yaml_active(&self, code: &TabCode) -> bool {
        self.active_file(code)
            .is_some_and(|file| file.input.yaml().is_some())
    }

    pub fn toml_active(&self, code: &TabCode) -> bool {
        self.active_file(code)
            .is_some_and(|file| file.input.toml().is_some())
    }

    pub fn csv_active(&self, code: &TabCode) -> bool {
        self.active_file(code)
            .is_some_and(|file| file.input.csv().is_some())
    }

    pub fn xml_active(&self, code: &TabCode) -> bool {
        self.active_file(code)
            .is_some_and(|file| file.input.xml().is_some())
    }

    pub fn drawio_active(&self, code: &TabCode) -> bool {
        self.active_file(code)
            .is_some_and(|file| file.input.drawio().is_some())
    }

    /// The input of the buffer in front of `code`, preferring the
    /// presentation that already exists for this tab and group.
    pub fn active_editor_input(&self, tab: TabId, code: &TabCode) -> Option<EditorInput> {
        self.editor_input_for(code.active_id()?, tab, code.group)
    }

    /// The live input for a buffer in one tab's group: the group's own
    /// presentation when it exists, the shared input otherwise.
    pub fn editor_input_for(
        &self,
        id: BufferId,
        tab: TabId,
        group: split::GroupId,
    ) -> Option<EditorInput> {
        let file = self.buffers.get(id)?;
        Some(
            file.presentations
                .markdown_existing(tab, group)
                .map(EditorInput::Markdown)
                .or_else(|| {
                    file.presentations
                        .svg_existing(tab, group)
                        .map(EditorInput::Svg)
                })
                .or_else(|| {
                    file.presentations
                        .json_existing(tab, group)
                        .map(EditorInput::Json)
                })
                .or_else(|| {
                    file.presentations
                        .yaml_existing(tab, group)
                        .map(EditorInput::Yaml)
                })
                .or_else(|| {
                    file.presentations
                        .toml_existing(tab, group)
                        .map(EditorInput::Toml)
                })
                .or_else(|| {
                    file.presentations
                        .csv_existing(tab, group)
                        .map(EditorInput::Csv)
                })
                .or_else(|| {
                    file.presentations
                        .xml_existing(tab, group)
                        .map(EditorInput::Xml)
                })
                .or_else(|| {
                    file.presentations
                        .image_existing(tab, group)
                        .map(EditorInput::Image)
                })
                .or_else(|| {
                    file.presentations
                        .sqlite_existing(tab, group)
                        .map(EditorInput::Sqlite)
                })
                .or_else(|| {
                    file.presentations
                        .book_existing(tab, group)
                        .map(EditorInput::Book)
                })
                .or_else(|| {
                    file.presentations
                        .pdf_existing(tab, group)
                        .map(EditorInput::Pdf)
                })
                .or_else(|| {
                    file.presentations
                        .drawio_existing(tab, group)
                        .map(EditorInput::Drawio)
                })
                .unwrap_or_else(|| file.input.clone()),
        )
    }

    pub fn active_local_file(&self, code: &TabCode) -> Option<LocalFile> {
        self.active_file(code).and_then(OpenFile::local)
    }

    /// The host/path the front buffer of `code` lives at, for the status bar.
    pub fn active_location(&self, code: &TabCode) -> Option<(HostId, &Path)> {
        let file = self.active_file(code)?;
        Some((file.host.id(), file.path.as_path()))
    }

    // ---- Opening ----

    /// Starts opening `path` on `host`, returning the id immediately when the
    /// buffer was already open (the shell shows it); an asynchronous load
    /// lands through `opened` and the `OpenCompleted` event instead.
    /// `workspace`/`tab`/`group`/`window_host` are shell-resolved values; the
    /// editor verifies its own epochs before anything lands.
    #[allow(clippy::too_many_arguments)]
    pub fn open_on_host_at(
        &mut self,
        host: SharedHost,
        path: &Path,
        cursor: Option<Position>,
        origin: Option<(BufferId, u64)>,
        workspace: lyrel_core::core::session::WorkspaceId,
        tab: TabId,
        group: Option<split::GroupId>,
        window_host: HostId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<BufferId> {
        let target = buffers::OpenTarget {
            workspace,
            tab,
            group,
            window_host,
            origin,
            origin_epoch: origin.and_then(|(id, _)| self.resource_epoch(id)),
            context_epoch: self.buffers.context_epoch(),
        };
        self.buffers.open(
            host,
            path,
            cursor,
            target,
            window,
            cx,
            |editor| &mut editor.buffers,
            Self::buffer_edited,
            Self::open_target_valid,
            Self::opened,
        )
    }

    fn resource_epoch(&self, id: BufferId) -> Option<(HostId, u64)> {
        let file = self.buffers.get(id)?;
        Some((file.host.id(), file.reload_seq))
    }

    /// Whether the original Markdown presentation still belongs to this group.
    pub fn markdown_presentation_in(
        &self,
        id: BufferId,
        tab: TabId,
        group: GroupId,
        source: EntityId,
    ) -> bool {
        self.buffers
            .get(id)
            .and_then(|file| file.presentations.markdown_existing(tab, group))
            .is_some_and(|view| view.entity_id() == source)
    }

    /// Whether an asynchronous open may still land. Only editor-verifiable
    /// predicates live here — the context epoch captured when the open
    /// started, and the origin document's identity. Whether the tab and group
    /// still exist is the shell's check, made again in the `OpenCompleted`
    /// handler; a buffer that lands nowhere is cleaned up by orphan adoption.
    fn open_target_valid(&self, target: buffers::OpenTarget, cx: &gpui_kit::App) -> bool {
        if target.context_epoch != self.buffers.context_epoch() {
            return false;
        }
        target.origin.is_none_or(|(id, revision)| {
            self.buffers
                .get(id)
                .and_then(|file| file.input.markdown())
                .is_some_and(|view| {
                    let document = view.read(cx).document().read(cx);
                    document.revision() == revision
                        && self.resource_epoch(id) == target.origin_epoch
                })
        })
    }

    /// The completion of an asynchronous open, running on the editor entity.
    /// Placement into the tab is the shell's answer to `OpenCompleted`.
    fn opened(
        &mut self,
        target: buffers::OpenTarget,
        result: buffers::OpenResult,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match result {
            buffers::OpenResult::Opened { id, created, front } => {
                if created {
                    self.buffer_created(id, window, cx);
                }
                cx.emit(EditorPanelEvent::OpenCompleted {
                    workspace: target.workspace,
                    window_host: target.window_host,
                    tab: target.tab,
                    group: target.group,
                    id,
                    created,
                    front,
                });
            }
            buffers::OpenResult::Failed {
                host,
                error: EditorOpenError::NotText(path),
            } => {
                cx.emit(EditorPanelEvent::OpenExternally { host, path });
            }
            buffers::OpenResult::Failed {
                error: EditorOpenError::Message(message),
                ..
            } => {
                window.notify_app(message, cx);
            }
        }
    }

    /// Registers everything a newly created buffer needs inside the panel:
    /// navigation history and, for Markdown, its image resources. The shell
    /// separately watches the document focus for its status line.
    fn buffer_created(&mut self, id: BufferId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(file) = self.buffers.get(id) else {
            return;
        };
        let input = file.input.clone();
        // 文本历史只属于有文本的文档；图片没有可导航的编辑位置。
        if let Some(text) = input.text() {
            self.nav.track_buffer(id, text.read(cx).text().clone());
        }
        if let Some(source) = input.markdown() {
            let owner = cx.entity().downgrade();
            let resources =
                cx.new(|cx| markdown::images::ImageHost::new(owner, id, source, window, cx));
            markdown::images::ImageHost::attach(&resources, source, cx);
            self.buffers.get_mut(id).unwrap().markdown_resources = Some(resources);
        }
    }

    /// Makes a buffer out of a loaded file, or finds the one already open for
    /// it. Returns the buffer id and whether it was created; the caller — the
    /// restore completion, or the shell — decides where it shows.
    fn install(
        &mut self,
        host: SharedHost,
        loaded: Loaded,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (BufferId, bool) {
        let (id, created) = self.buffers.install(
            host,
            loaded,
            window,
            cx,
            |editor| &mut editor.buffers,
            Self::buffer_edited,
        );
        if created {
            self.buffer_created(id, window, cx);
        }
        (id, created)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new_buffer(
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
        cx: &mut Context<Self>,
    ) -> BufferId {
        let id = self.buffers.create(
            host,
            path,
            untitled,
            kind,
            text,
            format,
            config,
            indent,
            mtime,
            window,
            cx,
            |editor| &mut editor.buffers,
            Self::buffer_edited,
        );
        self.buffer_created(id, window, cx);
        id
    }

    pub fn new_untitled(
        &mut self,
        host: SharedHost,
        kind: BufferKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> BufferId {
        let number = self.buffers.next_untitled();
        self.new_buffer(
            host,
            PathBuf::new(),
            Some(number),
            kind,
            String::new(),
            TextFormat::default(),
            EditorConfig::default(),
            lyrel_document::text::detect_indent("", "text"),
            None,
            window,
            cx,
        )
    }

    /// A scrollback export is an unsaved document, never a filesystem path on
    /// the pane's host. Saving uses the ordinary Save As and close safeguards.
    pub fn open_scrollback(
        &mut self,
        text: String,
        host: SharedHost,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> BufferId {
        let n = self.buffers.next_untitled();
        let indent = lyrel_document::text::detect_indent(&text, "text");
        let dirty = !text.is_empty();
        let id = self.new_buffer(
            host,
            PathBuf::new(),
            Some(n),
            BufferKind::Text,
            text,
            TextFormat::default(),
            EditorConfig::default(),
            indent,
            None,
            window,
            cx,
        );
        if let Some(file) = self.buffers.get_mut(id) {
            file.saved = Fingerprint::of_str("");
            file.dirty = dirty;
        }
        id
    }

    // ---- Focus ----

    /// Puts the keyboard on the document surface of `code`'s front buffer.
    pub fn focus_editor(
        &mut self,
        tab: TabId,
        code: &TabCode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(file) = self.active_file(code) else {
            return;
        };
        let group = code.group;
        if let Some(input) = file.input.markdown() {
            let view = file
                .presentations
                .markdown_view(input, tab, group, window, cx);
            EditorInput::Markdown(view).focus(window, cx);
        } else if let Some(primary) = file.input.svg() {
            let view = file.presentations.svg_view(primary, tab, group, cx);
            EditorInput::Svg(view).focus(window, cx);
        } else if let Some(primary) = file.input.json() {
            let view = file.presentations.json_view(primary, tab, group, cx);
            EditorInput::Json(view).focus(window, cx);
        } else if let Some(primary) = file.input.yaml() {
            let view = file.presentations.yaml_view(primary, tab, group, cx);
            EditorInput::Yaml(view).focus(window, cx);
        } else if let Some(primary) = file.input.toml() {
            let view = file.presentations.toml_view(primary, tab, group, cx);
            EditorInput::Toml(view).focus(window, cx);
        } else if let Some(primary) = file.input.csv() {
            let view = file.presentations.csv_view(primary, tab, group, window, cx);
            EditorInput::Csv(view).focus(window, cx);
        } else if let Some(primary) = file.input.xml() {
            let view = file.presentations.xml_view(primary, tab, group, cx);
            EditorInput::Xml(view).focus(window, cx);
        } else if let Some(primary) = file.input.image() {
            let view = file.presentations.image_view(primary, tab, group, cx);
            EditorInput::Image(view).focus(window, cx);
        } else if let Some(primary) = file.input.sqlite() {
            let view = file
                .presentations
                .sqlite_view(primary, tab, group, window, cx);
            EditorInput::Sqlite(view).focus(window, cx);
        } else if let Some(primary) = file.input.book() {
            let view = file
                .presentations
                .book_view(primary, tab, group, window, cx);
            EditorInput::Book(view).focus(window, cx);
        } else if let Some(primary) = file.input.pdf() {
            let view = file.presentations.pdf_view(primary, tab, group, window, cx);
            EditorInput::Pdf(view).focus(window, cx);
        } else if let Some(primary) = file.input.drawio() {
            let view = file.presentations.drawio_view(primary, tab, group, cx);
            EditorInput::Drawio(view).focus(window, cx);
        } else {
            file.input.focus(window, cx);
        }
    }

    /// Brings the file at `pos` in `code`'s strip to the front. The shell has
    /// already bounds-checked `pos` against `code.files`.
    pub fn activate(
        &mut self,
        pos: usize,
        tab: TabId,
        code: &mut TabCode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if pos >= code.files.len() {
            return;
        }
        code.active = pos;
        self.dismiss_bar();
        self.focus_editor(tab, code, window, cx);
        cx.notify();
    }

    /// Switch the active native Markdown presentation between live editing and source.
    /// `ToggleDocumentPreview` remains the action ID for existing custom keybindings.
    pub fn toggle_document_preview(
        &mut self,
        tab: TabId,
        code: &TabCode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(view) = self
            .active_editor_input(tab, code)
            .and_then(|input| input.markdown().cloned())
        {
            view.update(cx, |view, cx| view.toggle_mode(cx));
            cx.notify();
        } else if let Some(view) = self
            .active_editor_input(tab, code)
            .and_then(|input| input.svg().cloned())
        {
            view.update(cx, |view, cx| view.toggle_mode(window, cx));
            cx.notify();
        } else if let Some(view) = self
            .active_editor_input(tab, code)
            .and_then(|input| input.json().cloned())
        {
            view.update(cx, |view, cx| view.toggle_mode(window, cx));
            cx.notify();
        } else if let Some(view) = self
            .active_editor_input(tab, code)
            .and_then(|input| input.yaml().cloned())
        {
            view.update(cx, |view, cx| view.toggle_mode(window, cx));
            cx.notify();
        } else if let Some(view) = self
            .active_editor_input(tab, code)
            .and_then(|input| input.toml().cloned())
        {
            view.update(cx, |view, cx| view.toggle_mode(window, cx));
            cx.notify();
        } else if let Some(view) = self
            .active_editor_input(tab, code)
            .and_then(|input| input.csv().cloned())
        {
            view.update(cx, |view, cx| view.toggle_mode(window, cx));
            cx.notify();
        } else if let Some(view) = self
            .active_editor_input(tab, code)
            .and_then(|input| input.xml().cloned())
        {
            view.update(cx, |view, cx| view.toggle_mode(window, cx));
            cx.notify();
        }
    }

    /// The status bar's Wrap button, and `ToggleDocumentWrap`. Same reach as
    /// `toggle_document_preview`, for any file; the new state becomes what the
    /// next file opens with. The shell writes the returned state to config.
    pub fn toggle_document_wrap(
        &mut self,
        code: &TabCode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<bool> {
        let id = code.active_id()?;
        let f = self.buffers.get_mut(id)?;
        // 只读图片没有要折行的文本：命令对它不生效，也不改下一个文本
        // 文件的开箱设置。
        let text = f.input.text()?;
        f.wrap = !f.wrap;
        let wrap = f.wrap;
        text.set_soft_wrap(wrap, window, cx);
        f.presentations.set_soft_wrap(wrap, &f.input, cx);
        Some(wrap)
    }

    /// The status bar's line-ending button: flips the file between LF and
    /// CRLF. The buffer itself always holds `\n`; this only changes what a
    /// save writes, so it marks the file unsaved without touching its text.
    pub fn toggle_line_ending(&mut self, code: &TabCode, cx: &mut Context<Self>) {
        let Some(id) = code.active_id() else {
            return;
        };
        let Some(f) = self.buffers.get_mut(id) else {
            return;
        };
        // 只读图片没有保存格式可以改：这个按钮对它不该出现，也不该生效。
        let Some(mut format) = f.format(cx) else {
            return;
        };
        format.line_ending = match format.line_ending {
            LineEnding::Lf => LineEnding::CrLf,
            LineEnding::CrLf => LineEnding::Lf,
        };
        f.set_format(format, cx);
        self.note_edit(id, cx);
    }

    /// Re-derives whether a buffer differs from what is on disk, after an
    /// edit of any kind — typing, undo, a reload, a save rule.
    pub fn note_edit(&mut self, id: BufferId, cx: &mut Context<Self>) {
        self.buffers.note_edit(id, cx);
        self.buffer_changed(id, cx);
    }

    /// Finishes an IME composition before a close decision: the shared
    /// document when another tab shows the buffer too, the live input when
    /// this tab is its only showplace.
    pub fn finish_close_composition(
        &mut self,
        id: BufferId,
        shared: bool,
        tab: TabId,
        group: split::GroupId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(file) = self.buffers.get(id) else {
            return;
        };
        if shared {
            if let Some(view) = file.presentations.markdown_existing(tab, group) {
                view.update(cx, |view, cx| view.finish_owned_composition(cx));
            } else if let Some(view) = file.presentations.svg_existing(tab, group) {
                view.update(cx, |view, cx| view.finish_composition(cx));
            } else if let Some(view) = file.presentations.json_existing(tab, group) {
                view.update(cx, |view, cx| view.finish_composition(cx));
            } else if let Some(view) = file.presentations.csv_existing(tab, group) {
                view.update(cx, |view, cx| view.finish_composition(cx));
            } else if let Some(view) = file.presentations.xml_existing(tab, group) {
                view.update(cx, |view, cx| view.finish_composition(cx));
            }
        } else {
            file.input.finish_composition(window, cx);
        }
    }

    /// Types `@path` into the running agent's prompt, with the selected
    /// lines after it when there are any. `tab`/`group` name the editor
    /// group the request came from.
    pub fn attach_to_agent(
        &mut self,
        id: BufferId,
        tab: TabId,
        group: split::GroupId,
        cx: &mut Context<Self>,
    ) {
        let Some(f) = self.buffers.get(id).filter(|f| f.untitled.is_none()) else {
            return;
        };
        let input = self
            .editor_input_for(id, tab, group)
            .expect("retained document");
        let suffix = match input.text() {
            Some(text) => {
                let state = text.read(cx);
                match selected_lines(&state.text(), state.selected_range()) {
                    Some((a, b)) if a == b => format!("#L{a}"),
                    Some((a, b)) => format!("#L{a}-{b}"),
                    None => String::new(),
                }
            }
            // A picture has no lines to point at; the path itself still
            // names it for the agent.
            None => String::new(),
        };
        let path = f.path.clone();
        cx.emit(EditorPanelEvent::AttachToAgent { path, suffix });
    }

    // ---- The file tree moving files out from under their buffers ----

    /// A file or folder was renamed through lyrel: buffers at or under the old
    /// path follow it. Without this the buffer kept the old name, and the next
    /// save wrote a second copy there.
    pub fn path_moved(&mut self, host: HostId, from: &Path, to: &Path, cx: &mut Context<Self>) {
        let moved = self.buffers.path_moved(host, from, to, cx);
        if moved {
            cx.emit(EditorPanelEvent::RebuildWatcher);
            cx.notify();
        }
    }

    /// A file or folder was deleted through lyrel: its buffers say so rather
    /// than waiting for the watcher, which a remote host may not have.
    pub fn path_removed(&mut self, host: HostId, path: &Path, cx: &mut Context<Self>) {
        self.buffers.path_removed(host, path);
        cx.notify();
    }

    // ---- Changes made by something else ----

    pub fn reload_from_disk(&mut self, id: BufferId, window: &mut Window, cx: &mut Context<Self>) {
        self.buffers.reload(
            id,
            window,
            cx,
            |editor| &mut editor.buffers,
            Self::buffer_edited,
        );
    }

    // ---- Buffer lifetime ----

    /// Drops the buffer from the panel: the saved-buffer registry, the
    /// navigation history, and the buffer itself. The shell has already
    /// removed it from every tab that showed it.
    pub fn drop_buffer(&mut self, id: BufferId, cx: &mut Context<Self>) {
        self.forget_saved_buffer(id);
        self.buffers.remove(id);
        self.nav.forget_buffer(id);
        cx.emit(EditorPanelEvent::RebuildWatcher);
        cx.notify();
    }

    /// The tab's restore has started: what was recorded is what is being put
    /// back, and recording it again before the files load would write down an
    /// empty tab.
    pub fn note_restoring(&mut self, tab: TabId, snapshot: &crate::editor_session::TabEditor) {
        self.recorded.insert(tab, snapshot.clone());
    }

    // ---- Once-per-frame bookkeeping ----

    /// Editor-side half of the frame sync: restore-set pruning against the
    /// live tab ids the shell passes in, and deferred diagram-save
    /// resolutions. `root` is the shell-resolved save root. Presentation
    /// retention and session recording need the tab set and stay with the
    /// shell.
    pub fn sync(
        &mut self,
        live: &std::collections::HashSet<TabId>,
        root: &Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // A tab that left and came back under the same id — a server restart
        // rebuilds every tab that way — gets its files back like a relaunch.
        if self.restored.len() > live.len() {
            self.restored.retain(|id| live.contains(id));
        }
        for (id, readiness) in self.buffers.take_drawio_flush_resolutions(cx) {
            match readiness {
                crate::code_editor::buffers::DrawioFlushReadiness::Confirmed { force, target } => {
                    // A confirmed flush takes the same Host write path as a
                    // synchronous save: conflicts, prompts and identity
                    // checks still apply to the bytes that are current now.
                    self.save_at(id, force, target, root, window, cx);
                }
                crate::code_editor::buffers::DrawioFlushReadiness::Rejected { reason } => {
                    // A page or HTTP refusal is request-scoped: the canvas
                    // keeps its model, the document keeps the last accepted
                    // bytes, and the save fails where the user can see it.
                    self.drawio_flush_failed(id, Some(reason), window, cx);
                }
                crate::code_editor::buffers::DrawioFlushReadiness::Expired => {
                    // The engine never confirmed: writing the bytes we have
                    // would silently drop the model on its canvas. Fail the
                    // save the way any other failed write does — the dirty
                    // document stays, and a pending close is cancelled.
                    self.drawio_flush_failed(id, None, window, cx);
                }
            }
        }
    }

    /// A deferred diagram save whose engine refused or never confirmed its
    /// model is a failed save, not a licence to write older bytes: the same
    /// toast a failed write raises, the document stays dirty, and any close
    /// that was waiting on this save is cancelled instead of dropping the
    /// work. A refused request keeps its own reason visible.
    fn drawio_flush_failed(
        &mut self,
        id: BufferId,
        reason: Option<gpui_kit::SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(label) = self.buffers.get(id).map(|file| file.label().to_string()) else {
            return;
        };
        let context = t_fmt(L10nKey::EditorSaveFailed, &[("name", &label)]);
        let (kind, message) = match reason {
            Some(reason) => (std::io::ErrorKind::Other, reason.to_string()),
            None => (
                std::io::ErrorKind::TimedOut,
                "the diagram engine did not confirm its current model".to_string(),
            ),
        };
        lyrel_ui_base::host_ops::HostOps::notify_err(
            window,
            cx,
            &context,
            &std::io::Error::new(kind, message),
        );
        self.saves_failed(id);
        cx.notify();
    }

    /// The draw.io engine's own save requests, picked up once per frame; the
    /// shell feeds each back through the ordinary Host save path with the
    /// shell-resolved save root.
    pub fn take_drawio_save_requests(&mut self, cx: &mut Context<Self>) -> Vec<BufferId> {
        let mut requested = Vec::new();
        for (id, views) in self.buffers.drawio_pending(cx) {
            for view in views {
                let view_requested = view.update(cx, |view, _| view.take_save_request());
                if view_requested {
                    requested.push(id);
                }
            }
        }
        requested
    }

    /// Drops presentations whose tab or group no longer shows the buffer.
    /// `shown` is the shell's membership predicate, supplied per frame.
    pub fn retain_presentations(
        &mut self,
        shown: &impl Fn(BufferId, TabId, split::GroupId) -> bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        for file in self.buffers.iter() {
            let id = file.id();
            file.presentations
                .retain(|tab, group| shown(id, tab, group), window, cx);
        }
    }

    /// Rebuilds the file watcher against the shell-resolved window host.
    pub fn rebuild_watch(&mut self, host: Option<SharedHost>, cx: &mut Context<Self>) {
        self.buffers.rebuild_watch(host, cx);
    }

    /// Whether this tab has not had its recorded files restored yet. A pure
    /// query: `restore_tab` performs the marking insert.
    pub fn needs_restore(&self, tab: TabId) -> bool {
        !self.restored.contains(&tab)
    }

    pub fn is_restored(&self, tab: &TabId) -> bool {
        self.restored.contains(tab)
    }

    pub fn is_recorded(&self, tab: TabId, state: &crate::editor_session::TabEditor) -> bool {
        self.recorded.get(&tab) == Some(state)
    }

    pub fn record_session(&mut self, tab: TabId, state: crate::editor_session::TabEditor) {
        self.recorded.insert(tab, state);
    }

    /// A tab was closed for good: forget what it had open.
    pub fn forget_tab(&mut self, tab: TabId) {
        self.recorded.remove(&tab);
        self.buffers.forget_tab(tab);
        self.nav.forget_tab(tab);
    }

    /// Restores the tab's recorded files when the shell asks. `host`,
    /// `workspace` and `window_host` are shell-resolved values; the shell has
    /// also verified the tab exists. The completion emits `TabRestored` for
    /// the shell to merge and focus.
    #[allow(clippy::too_many_arguments)]
    pub fn restore_tab(
        &mut self,
        host: SharedHost,
        tab: TabId,
        has_files: bool,
        workspace: lyrel_core::core::session::WorkspaceId,
        window_host: HostId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.restored.insert(tab) {
            return;
        }
        if has_files {
            return;
        }
        let Some(state) = crate::editor_session::get(cx, tab) else {
            return;
        };
        // The left group can be empty while the right is not: it held only
        // files that are not recorded (untitled, remote).
        if state.files.is_empty() && state.split.as_ref().is_none_or(|s| s.files.is_empty()) {
            return;
        }
        self.note_restoring(tab, &state);
        // Both groups' files in one trip, the left group's first.
        let files: Vec<PathBuf> = state
            .files
            .iter()
            .chain(state.split.iter().flat_map(|s| s.files.iter()))
            .cloned()
            .collect();
        let requested = files.clone();
        let target = buffers::OpenTarget {
            workspace,
            tab,
            group: None,
            window_host,
            origin: None,
            origin_epoch: None,
            context_epoch: self.buffers.context_epoch(),
        };
        let install_host = host.clone();
        self.buffers.restore(
            host,
            files,
            target,
            window,
            cx,
            |editor| &mut editor.buffers,
            Self::open_target_valid,
            move |editor, loaded: Vec<Option<Loaded>>, window, cx| {
                let mut opened: Vec<(PathBuf, BufferId, bool)> = Vec::new();
                for (path, l) in requested.into_iter().zip(loaded) {
                    if let Some(l) = l {
                        let (id, created) = editor.install(install_host.clone(), l, window, cx);
                        opened.push((path, id, created));
                    }
                }
                cx.emit(EditorPanelEvent::TabRestored {
                    workspace: target.workspace,
                    window_host: target.window_host,
                    tab,
                    open: opened,
                    snapshot: state,
                });
            },
        );
    }
}

impl EditorPanelState {
    /// One group's text: the file `id`, rendered or as source, or a blank
    /// pane. `slot` tells the two groups' elements apart. `tab` and `code`
    /// are the shell-resolved active tab and its editor state. The shell
    /// attaches the source documents' context menus to the returned element.
    pub fn render_body(
        &mut self,
        id: BufferId,
        slot: usize,
        tab: TabId,
        code: &TabCode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Stateful<gpui_kit::Div> {
        match self.buffers.get(id) {
            None => v_flex()
                .id(("editor-body", slot))
                .min_w_0()
                .min_h_0()
                .size_full(),
            Some(f) if f.input.book().is_some() => {
                let primary = f.input.book().expect("book document presentation");
                let group = if code.focused_slot() == slot {
                    code.group
                } else {
                    code.split.as_ref().expect("second editor group").id
                };
                let view = f.presentations.book_view(primary, tab, group, window, cx);
                v_flex()
                    .id(("book-presentation", view.entity_id()))
                    .min_w_0()
                    .min_h_0()
                    .size_full()
                    .child(view)
            }
            Some(f) if f.input.image().is_some() => {
                let primary = f.input.image().expect("image document presentation");
                let group = if code.focused_slot() == slot {
                    code.group
                } else {
                    code.split.as_ref().expect("second editor group").id
                };
                let view = f.presentations.image_view(primary, tab, group, cx);
                v_flex()
                    .id(("image-presentation", view.entity_id()))
                    .min_w_0()
                    .min_h_0()
                    .size_full()
                    .child(view)
            }
            Some(f) if f.input.drawio().is_some() => {
                let primary = f.input.drawio().expect("draw.io document presentation");
                let group = if code.focused_slot() == slot {
                    code.group
                } else {
                    code.split.as_ref().expect("second editor group").id
                };
                let view = f.presentations.drawio_view(primary, tab, group, cx);
                v_flex()
                    .id(("drawio-presentation", view.entity_id()))
                    .min_w_0()
                    .min_h_0()
                    .size_full()
                    .child(view)
            }
            Some(f) if f.input.svg().is_some() => {
                let primary = f.input.svg().expect("SVG document presentation");
                let group = if code.focused_slot() == slot {
                    code.group
                } else {
                    code.split.as_ref().expect("second editor group").id
                };
                let view = f.presentations.svg_view(primary, tab, group, cx);
                v_flex()
                    .id(("svg-presentation", view.entity_id()))
                    .min_w_0()
                    .min_h_0()
                    .size_full()
                    .font_family(cx.theme().mono_font_family.clone())
                    .text_size(rems(1.))
                    .text_color(cx.theme().foreground)
                    .child(view)
            }
            Some(f) if f.input.json().is_some() => {
                let primary = f.input.json().expect("JSON document presentation");
                let group = if code.focused_slot() == slot {
                    code.group
                } else {
                    code.split.as_ref().expect("second editor group").id
                };
                let view = f.presentations.json_view(primary, tab, group, cx);
                v_flex()
                    .id(("json-presentation", view.entity_id()))
                    .min_w_0()
                    .min_h_0()
                    .size_full()
                    .font_family(cx.theme().mono_font_family.clone())
                    .text_size(rems(1.))
                    .text_color(cx.theme().foreground)
                    .child(view)
            }
            Some(f) if f.input.yaml().is_some() => {
                let primary = f.input.yaml().expect("YAML document presentation");
                let group = if code.focused_slot() == slot {
                    code.group
                } else {
                    code.split.as_ref().expect("second editor group").id
                };
                let view = f.presentations.yaml_view(primary, tab, group, cx);
                v_flex()
                    .id(("yaml-presentation", view.entity_id()))
                    .min_w_0()
                    .min_h_0()
                    .size_full()
                    .font_family(cx.theme().mono_font_family.clone())
                    .text_size(rems(1.))
                    .text_color(cx.theme().foreground)
                    .child(view)
            }
            Some(f) if f.input.toml().is_some() => {
                let primary = f.input.toml().expect("TOML document presentation");
                let group = if code.focused_slot() == slot {
                    code.group
                } else {
                    code.split.as_ref().expect("second editor group").id
                };
                let view = f.presentations.toml_view(primary, tab, group, cx);
                v_flex()
                    .id(("toml-presentation", view.entity_id()))
                    .min_w_0()
                    .min_h_0()
                    .size_full()
                    .font_family(cx.theme().mono_font_family.clone())
                    .text_size(rems(1.))
                    .text_color(cx.theme().foreground)
                    .child(view)
            }
            Some(f) if f.input.xml().is_some() => {
                let primary = f.input.xml().expect("XML document presentation");
                let group = if code.focused_slot() == slot {
                    code.group
                } else {
                    code.split.as_ref().expect("second editor group").id
                };
                let view = f.presentations.xml_view(primary, tab, group, cx);
                v_flex()
                    .id(("xml-presentation", view.entity_id()))
                    .min_w_0()
                    .min_h_0()
                    .size_full()
                    .font_family(cx.theme().mono_font_family.clone())
                    .text_size(rems(1.))
                    .text_color(cx.theme().foreground)
                    .child(view)
            }
            Some(f) if f.input.csv().is_some() => {
                let primary = f.input.csv().expect("CSV document presentation");
                let group = if code.focused_slot() == slot {
                    code.group
                } else {
                    code.split.as_ref().expect("second editor group").id
                };
                let view = f.presentations.csv_view(primary, tab, group, window, cx);
                v_flex()
                    .id(("csv-presentation", view.entity_id()))
                    .min_w_0()
                    .min_h_0()
                    .size_full()
                    .font_family(cx.theme().mono_font_family.clone())
                    .text_size(rems(1.))
                    .text_color(cx.theme().foreground)
                    .child(view)
            }
            Some(f) if f.input.pdf().is_some() => {
                let primary = f.input.pdf().expect("pdf document presentation");
                let group = if code.focused_slot() == slot {
                    code.group
                } else {
                    code.split.as_ref().expect("second editor group").id
                };
                let view = f.presentations.pdf_view(primary, tab, group, window, cx);
                v_flex()
                    .id(("pdf-presentation", view.entity_id()))
                    .min_w_0()
                    .min_h_0()
                    .size_full()
                    .child(view)
            }
            Some(f) if f.input.sqlite().is_some() => {
                let primary = f.input.sqlite().expect("sqlite document presentation");
                let group = if code.focused_slot() == slot {
                    code.group
                } else {
                    code.split.as_ref().expect("second editor group").id
                };
                let view = f.presentations.sqlite_view(primary, tab, group, window, cx);
                v_flex()
                    .id(("sqlite-presentation", view.entity_id()))
                    .min_w_0()
                    .min_h_0()
                    .size_full()
                    .child(view)
            }
            Some(f) if f.input.markdown().is_some() => {
                let input = f.input.markdown().unwrap();
                let group = if code.focused_slot() == slot {
                    code.group
                } else {
                    code.split.as_ref().expect("second editor group").id
                };
                let view = f.presentations.markdown_view(input, tab, group, window, cx);
                let scrollbar_id = SharedString::from(format!(
                    "markdown-document-scrollbar-{}",
                    view.entity_id().as_u64()
                ));
                if let Some(resources) = &f.markdown_resources {
                    markdown::images::ImageHost::attach(resources, &view, cx);
                }
                v_flex()
                    .id(("markdown-presentation", view.entity_id()))
                    .min_w_0()
                    .size_full()
                    .font_family(if view.read(cx).is_live() {
                        SharedString::from("Arial")
                    } else {
                        cx.theme().mono_font_family.clone()
                    })
                    .text_size(rems(1.))
                    .text_color(cx.theme().foreground)
                    .when(!view.read(cx).is_source_only(), |body| {
                        body.children(
                            f.markdown_resources
                                .as_ref()
                                .map(|resources| markdown::images::controls(resources, &view, cx)),
                        )
                    })
                    .child(markdown::table_code::render_controls(&view))
                    .children(f.presentations.search(&view))
                    .child(
                        div()
                            .on_children_prepainted({
                                let view = view.clone();
                                move |bounds, _, cx| {
                                    if let Some(bounds) = bounds.first() {
                                        view.update(cx, |view, cx| {
                                            view.set_scroll_viewport(*bounds, cx)
                                        });
                                    }
                                }
                            })
                            .id(scrollbar_id)
                            .test_support()
                            .relative()
                            .flex_1()
                            .min_w_0()
                            .min_h_0()
                            // Native document inset also clears the existing dock resize band.
                            // Keep it inside the scroll owner rather than adding shell padding.
                            .child(v_flex().size_full().min_h_0().child(view)),
                    )
            }
            Some(f) => {
                let input = f.input.code().expect("ordinary editor variant").clone();
                let id = f.id();
                div()
                    .id(("editor-body", slot))
                    .size_full()
                    .map(|root| {
                        crate::lsp::popups::EditorPopups::decorate(
                            root,
                            f.popups.as_ref().expect("code editor popups"),
                        )
                    })
                    .child(
                        Editor::new(&input)
                            .appearance(false)
                            .font_family(cx.theme().mono_font_family.clone())
                            .text_size(cx.theme().mono_font_size)
                            .size_full(),
                    )
                    .children(self.render_gutter(id, cx))
                    .children(self.render_gutter_peek(id, tab, code, cx))
                    .children(f.popups.clone())
            }
        }
    }

    /// The document status line: cursor, language, encoding, wrap, the LSP
    /// state and the document-specific controls. `lsp` is the shell-rendered
    /// LSP status element, passed in as a value.
    pub fn render_status(
        &self,
        tab: TabId,
        code: &TabCode,
        lsp: Option<AnyElement>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let active = self.active_file(code);
        let text = active.is_some_and(|file| file.input.is_text());
        let image = self
            .active_editor_input(tab, code)
            .and_then(|input| input.image().cloned());
        let image_status: Option<SharedString> =
            image.as_ref().map(|view| view.read(cx).status_text(cx));
        let image_zoom: Option<SharedString> =
            image.as_ref().map(|view| view.read(cx).zoom_label());
        let sqlite_status: Option<SharedString> = self
            .active_editor_input(tab, code)
            .and_then(|input| input.sqlite().cloned())
            .map(|view| view.read(cx).status_text());
        let pdf = self
            .active_editor_input(tab, code)
            .and_then(|input| input.pdf().cloned());
        let pdf_status: Option<SharedString> =
            pdf.as_ref().map(|view| view.read(cx).status_text(cx));
        let pdf_zoom: Option<SharedString> = pdf
            .as_ref()
            .filter(|view| view.read(cx).has_pages())
            .map(|view| view.read(cx).zoom_label());
        let image_page: Option<SharedString> =
            image.as_ref().and_then(|view| view.read(cx).page_label(cx));
        let drawio = self
            .active_editor_input(tab, code)
            .and_then(|input| input.drawio().cloned());
        let drawio_status: Option<SharedString> =
            drawio.as_ref().map(|view| view.read(cx).status_text(cx));
        let drawio_page: Option<SharedString> = drawio
            .as_ref()
            .and_then(|view| view.read(cx).page_label(cx));
        let drawio_zoom: Option<SharedString> =
            drawio.as_ref().map(|view| view.read(cx).zoom_label());
        let book_status: Option<SharedString> = self
            .active_editor_input(tab, code)
            .and_then(|input| input.book().cloned())
            .map(|view| view.read(cx).status_text(cx));
        let book_chapter: Option<SharedString> = self
            .active_editor_input(tab, code)
            .and_then(|input| input.book().cloned())
            .map(|view| view.read(cx).chapter_label(cx));
        let book_progress: Option<SharedString> = self
            .active_editor_input(tab, code)
            .and_then(|input| input.book().cloned())
            .map(|view| view.read(cx).progress_label(cx));
        let csv_view = self
            .active_editor_input(tab, code)
            .and_then(|input| input.csv().cloned());
        let csv_table_mode = csv_view
            .as_ref()
            .is_some_and(|view| view.read(cx).mode() == CsvMode::Table);
        let cursor: Option<SharedString> = (!csv_table_mode)
            .then(|| {
                self.active_editor_input(tab, code)
                    .and_then(|input| input.text().map(|text| text.read(cx).cursor_position()))
            })
            .flatten()
            .map(|pos| {
                t_fmt(
                    L10nKey::EditorLnCol,
                    &[
                        ("line", &(pos.line + 1).to_string()),
                        ("column", &(pos.character + 1).to_string()),
                    ],
                )
                .into()
            });
        let wrap: Option<bool> = active.filter(|_| text).map(|f| f.wrap);
        let native_live = self.active_editor_input(tab, code).and_then(|input| {
            input
                .markdown()
                .map(|view| (view.read(cx).is_live(), view.read(cx).is_source_only()))
        });
        let svg_state = self
            .active_editor_input(tab, code)
            .and_then(|input| input.svg().cloned())
            .map(|view| {
                let view = view.read(cx);
                (
                    view.mode(),
                    view.preview_size(cx),
                    view.zoom_label(),
                    view.is_rendering(cx),
                    view.preview_failure(cx),
                )
            });
        let json_state = self
            .active_editor_input(tab, code)
            .and_then(|input| input.json().cloned())
            .map(|view| {
                let view = view.read(cx);
                (
                    view.mode(),
                    view.is_parsing(cx),
                    view.fatal_error(cx),
                    view.selected_location(cx),
                    view.node_count(cx),
                    view.record_count(cx),
                    view.invalid_records(cx),
                )
            });
        let xml_state = self
            .active_editor_input(tab, code)
            .and_then(|input| input.xml().cloned())
            .map(|view| {
                let view = view.read(cx);
                (
                    view.mode(),
                    view.is_parsing(cx),
                    view.fatal_error(cx),
                    view.selected_location(cx),
                    view.element_count(cx),
                    view.attribute_count(cx),
                )
            });
        let yaml_state = self
            .active_editor_input(tab, code)
            .and_then(|input| input.yaml().cloned())
            .map(|view| {
                let view = view.read(cx);
                (
                    view.mode(),
                    view.is_parsing(cx),
                    view.fatal_error(cx),
                    view.selected_location(cx),
                    view.node_count(cx),
                    view.document_count(cx),
                )
            });
        let toml_state = self
            .active_editor_input(tab, code)
            .and_then(|input| input.toml().cloned())
            .map(|view| {
                let view = view.read(cx);
                (
                    view.mode(),
                    view.is_parsing(cx),
                    view.fatal_error(cx),
                    view.selected_location(cx),
                    view.node_count(cx),
                )
            });
        let csv_state = csv_view.as_ref().map(|view| {
            let view = view.read(cx);
            (
                view.mode(),
                view.facts(cx),
                view.selection(),
                view.visible_count(),
                view.is_parsing(cx),
                view.parse_error(cx),
                view.filter().to_owned(),
                view.sort(),
            )
        });
        let indent: Option<SharedString> = active.filter(|_| text).map(|f| {
            let key = if f.indent.hard_tabs {
                L10nKey::EditorIndentTabs
            } else {
                L10nKey::EditorIndentSpaces
            };
            t_fmt(key, &[("n", &f.indent.size.to_string())]).into()
        });
        let line_ending: Option<&'static str> = active
            .filter(|_| text)
            .and_then(|f| f.format(cx))
            .map(|format| format.line_ending.label());
        let encoding: Option<SharedString> = active
            .filter(|_| text)
            .and_then(|f| f.format(cx))
            .map(|format| format.encoding_label().into());
        let language: Option<&'static str> = active.and_then(|f| language_label(f.language()));

        let item = |id: &'static str, label: SharedString| {
            Button::new(id).ghost().xsmall().flex_none().label(label)
        };
        h_flex()
            .id("document-status")
            .test_support()
            .flex_1()
            .min_w_0()
            .gap_1()
            .overflow_x_scroll()
            .whitespace_nowrap()
            .child(
                Button::new("status-problems")
                    .ghost()
                    .xsmall()
                    .flex_none()
                    .accessibility_label(t(L10nKey::EditorProblemsToggle))
                    .tooltip(t(L10nKey::EditorProblemsToggle))
                    .child(lsp.unwrap_or_else(|| {
                        div()
                            .child(t(L10nKey::EditorProblemsTitle))
                            .into_any_element()
                    }))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.toggle_problems(cx);
                        cx.emit(EditorPanelEvent::FocusDocument);
                    })),
            )
            .when_some(native_live, |this, (live, source_only)| {
                let label = t(if live || source_only {
                    L10nKey::EditorMarkdownSource
                } else {
                    L10nKey::EditorMarkdownLive
                });
                this.child(
                    item("status-md-preview", label.into())
                        .disabled(source_only)
                        .on_click(status_command(EditorStatusCommand::TogglePreview, cx)),
                )
            })
            .when_some(
                csv_state,
                |this, (mode, facts, selection, visible, parsing, error, filter, sort)| {
                    let (label, tip) = match mode {
                        CsvMode::Table => {
                            (t(L10nKey::EditorCsvTable), t(L10nKey::EditorCsvShowSource))
                        }
                        CsvMode::Source => (
                            t(L10nKey::EditorMarkdownSource),
                            t(L10nKey::EditorCsvShowTable),
                        ),
                    };
                    let mut this = this.child(
                        item("status-csv-mode", label.into())
                            .tooltip(tip)
                            .on_click(status_command(EditorStatusCommand::TogglePreview, cx)),
                    );
                    if mode == CsvMode::Table {
                        if let Some((records, columns)) = facts {
                            this = this.child(
                                div()
                                    .id("status-csv-facts")
                                    .test_support()
                                    .flex_none()
                                    .child(t_fmt(
                                        L10nKey::EditorCsvFacts,
                                        &[
                                            ("records", &records.to_string()),
                                            ("columns", &columns.to_string()),
                                        ],
                                    )),
                            );
                            if let Some(cell) = selection {
                                this = this.child(
                                    div()
                                        .id("status-csv-cell")
                                        .test_support()
                                        .flex_none()
                                        .child(t_fmt(
                                            L10nKey::EditorCsvCell,
                                            &[
                                                (
                                                    "record",
                                                    &(cell.record().index() + 1).to_string(),
                                                ),
                                                ("column", &(cell.column() + 1).to_string()),
                                            ],
                                        )),
                                );
                            }
                            if !filter.is_empty() {
                                this = this.child(
                                    div()
                                        .id("status-csv-filter")
                                        .test_support()
                                        .flex_none()
                                        .child(t_fmt(
                                            L10nKey::EditorCsvFilterStatus,
                                            &[
                                                ("filter", &filter),
                                                ("visible", &visible.to_string()),
                                                ("records", &records.to_string()),
                                            ],
                                        )),
                                );
                            }
                            if let Some(sort) = sort {
                                this = this.child(
                                    div()
                                        .id("status-csv-sort")
                                        .test_support()
                                        .flex_none()
                                        .child(csv_document::sort_label(sort)),
                                );
                            }
                        }
                        if parsing {
                            this = this.child(
                                div()
                                    .id("status-csv-parsing")
                                    .test_support()
                                    .flex_none()
                                    .child(t(L10nKey::EditorCsvParsing)),
                            );
                        }
                        if error.is_some() {
                            this = this.child(
                                div()
                                    .id("status-csv-error")
                                    .test_support()
                                    .flex_none()
                                    .text_color(cx.theme().warning)
                                    .child(t(L10nKey::EditorCsvTableUnavailable)),
                            );
                        }
                    }
                    this
                },
            )
            .when_some(svg_state, |this, (mode, size, zoom, rendering, failure)| {
                let (label, tip) = match mode {
                    SvgMode::Source => (
                        t(L10nKey::EditorMarkdownSource),
                        t(L10nKey::EditorSvgShowPreview),
                    ),
                    SvgMode::Preview => (
                        t(L10nKey::EditorSvgPreview),
                        t(L10nKey::EditorSvgShowSource),
                    ),
                };
                this.child(
                    item("status-svg-mode", label.into())
                        .tooltip(tip)
                        .on_click(status_command(EditorStatusCommand::TogglePreview, cx)),
                )
                .when_some(size, |this, (width, height)| {
                    this.child(
                        div()
                            .id("status-svg-size")
                            .test_support()
                            .flex_none()
                            .child(t_fmt(
                                L10nKey::EditorImagePixelSize,
                                &[
                                    ("width", &format!("{width:.0}")),
                                    ("height", &format!("{height:.0}")),
                                ],
                            )),
                    )
                })
                .when(rendering, |this| {
                    this.child(
                        div()
                            .id("status-svg-rendering")
                            .test_support()
                            .flex_none()
                            .child(t(L10nKey::EditorSvgRendering)),
                    )
                })
                .when(failure.is_some(), |this| {
                    this.child(
                        div()
                            .id("status-svg-preview-failed")
                            .test_support()
                            .flex_none()
                            .text_color(cx.theme().warning)
                            .child(t(L10nKey::EditorSvgPreviewFailed)),
                    )
                })
                .when(mode == SvgMode::Preview, |this| {
                    this.child(
                        div()
                            .id("status-svg-zoom")
                            .test_support()
                            .flex_none()
                            .child(zoom),
                    )
                })
            })
            .when_some(
                xml_state,
                |this, (mode, parsing, fatal, location, elements, attributes)| {
                    let (label, tip) = match mode {
                        XmlMode::Source => (
                            t(L10nKey::EditorXmlSource),
                            t(L10nKey::EditorXmlShowStructure),
                        ),
                        XmlMode::Structure => (
                            t(L10nKey::EditorXmlStructure),
                            t(L10nKey::EditorXmlShowSource),
                        ),
                    };
                    let mut this = this.child(
                        item("status-xml-mode", label.into())
                            .tooltip(tip)
                            .on_click(status_command(EditorStatusCommand::TogglePreview, cx)),
                    );
                    if let (Some(elements), Some(attributes)) = (elements, attributes) {
                        this = this.child(
                            div()
                                .id("status-xml-count")
                                .test_support()
                                .flex_none()
                                .child(t_fmt(
                                    L10nKey::EditorXmlFacts,
                                    &[
                                        ("elements", &elements.to_string()),
                                        ("attributes", &attributes.to_string()),
                                    ],
                                )),
                        );
                    }
                    if let Some(location) = location {
                        this = this.child(
                            div()
                                .id("status-xml-location")
                                .test_support()
                                .flex_none()
                                .child(t_fmt(
                                    L10nKey::EditorLnCol,
                                    &[
                                        ("line", &location.line.to_string()),
                                        ("column", &location.column.to_string()),
                                    ],
                                )),
                        );
                    }
                    if parsing {
                        this = this.child(
                            div()
                                .id("status-xml-parsing")
                                .test_support()
                                .flex_none()
                                .child(t(L10nKey::EditorXmlParsing)),
                        );
                    }
                    if let Some((title, _)) = fatal {
                        this = this.child(
                            div()
                                .id("status-xml-error")
                                .test_support()
                                .flex_none()
                                .text_color(cx.theme().warning)
                                .child(title),
                        );
                    }
                    this
                },
            )
            .when_some(
                json_state,
                |this, (mode, parsing, fatal, location, nodes, records, invalid)| {
                    let (label, tip) = match mode {
                        JsonMode::Source => (
                            t(L10nKey::EditorJsonSource),
                            t(L10nKey::EditorJsonShowStructure),
                        ),
                        JsonMode::Structure => (
                            t(L10nKey::EditorJsonStructure),
                            t(L10nKey::EditorJsonShowSource),
                        ),
                    };
                    let mut this = this.child(
                        item("status-json-mode", label.into())
                            .tooltip(tip)
                            .on_click(status_command(EditorStatusCommand::TogglePreview, cx)),
                    );
                    if let Some(count) = records {
                        this = this.child(
                            div()
                                .id("status-json-count")
                                .test_support()
                                .flex_none()
                                .child(t_fmt(
                                    L10nKey::EditorJsonRecords,
                                    &[("count", &count.to_string())],
                                )),
                        );
                    } else if let Some(count) = nodes {
                        this = this.child(
                            div()
                                .id("status-json-count")
                                .test_support()
                                .flex_none()
                                .child(t_fmt(
                                    L10nKey::EditorJsonNodes,
                                    &[("count", &count.to_string())],
                                )),
                        );
                    }
                    if let Some(location) = location {
                        this = this.child(
                            div()
                                .id("status-json-location")
                                .test_support()
                                .flex_none()
                                .child(t_fmt(
                                    L10nKey::EditorLnCol,
                                    &[
                                        ("line", &location.line.to_string()),
                                        ("column", &location.column.to_string()),
                                    ],
                                )),
                        );
                    }
                    if parsing {
                        this = this.child(
                            div()
                                .id("status-json-parsing")
                                .test_support()
                                .flex_none()
                                .child(t(L10nKey::EditorJsonParsing)),
                        );
                    }
                    if let Some((title, _)) = fatal {
                        this = this.child(
                            div()
                                .id("status-json-error")
                                .test_support()
                                .flex_none()
                                .text_color(cx.theme().warning)
                                .child(title),
                        );
                    } else if invalid > 0 {
                        this = this.child(
                            div()
                                .id("status-json-invalid")
                                .test_support()
                                .flex_none()
                                .text_color(cx.theme().warning)
                                .child(t_fmt(
                                    L10nKey::EditorJsonInvalidRecords,
                                    &[("count", &invalid.to_string())],
                                )),
                        );
                    }
                    this
                },
            )
            .when_some(
                toml_state,
                |this, (mode, parsing, fatal, location, nodes)| {
                    let (label, tip) = match mode {
                        TomlMode::Source => (
                            t(L10nKey::EditorTomlSource),
                            t(L10nKey::EditorTomlShowStructure),
                        ),
                        TomlMode::Structure => (
                            t(L10nKey::EditorTomlStructure),
                            t(L10nKey::EditorTomlShowSource),
                        ),
                    };
                    let mut this = this.child(
                        item("status-toml-mode", label.into())
                            .tooltip(tip)
                            .on_click(status_command(EditorStatusCommand::TogglePreview, cx)),
                    );
                    if let Some(count) = nodes {
                        this = this.child(
                            div()
                                .id("status-toml-count")
                                .test_support()
                                .flex_none()
                                .child(t_fmt(
                                    L10nKey::EditorTomlNodes,
                                    &[("count", &count.to_string())],
                                )),
                        );
                    }
                    if let Some(location) = location {
                        this = this.child(
                            div()
                                .id("status-toml-location")
                                .test_support()
                                .flex_none()
                                .child(t_fmt(
                                    L10nKey::EditorLnCol,
                                    &[
                                        ("line", &location.line.to_string()),
                                        ("column", &location.column.to_string()),
                                    ],
                                )),
                        );
                    }
                    if parsing {
                        this = this.child(
                            div()
                                .id("status-toml-parsing")
                                .test_support()
                                .flex_none()
                                .child(t(L10nKey::EditorTomlParsing)),
                        );
                    }
                    if let Some((title, _)) = fatal {
                        this = this.child(
                            div()
                                .id("status-toml-error")
                                .test_support()
                                .flex_none()
                                .text_color(cx.theme().warning)
                                .child(title),
                        );
                    }
                    this
                },
            )
            .when_some(
                yaml_state,
                |this, (mode, parsing, fatal, location, nodes, documents)| {
                    let (label, tip) = match mode {
                        YamlMode::Source => (
                            t(L10nKey::EditorYamlSource),
                            t(L10nKey::EditorYamlShowStructure),
                        ),
                        YamlMode::Structure => (
                            t(L10nKey::EditorYamlStructure),
                            t(L10nKey::EditorYamlShowSource),
                        ),
                    };
                    let mut this = this.child(
                        item("status-yaml-mode", label.into())
                            .tooltip(tip)
                            .on_click(status_command(EditorStatusCommand::TogglePreview, cx)),
                    );
                    if let Some(count) = documents {
                        this = this.child(
                            div()
                                .id("status-yaml-documents")
                                .test_support()
                                .flex_none()
                                .child(t_fmt(
                                    L10nKey::EditorYamlDocuments,
                                    &[("count", &count.to_string())],
                                )),
                        );
                    }
                    if let Some(count) = nodes {
                        this = this.child(
                            div()
                                .id("status-yaml-count")
                                .test_support()
                                .flex_none()
                                .child(t_fmt(
                                    L10nKey::EditorYamlNodes,
                                    &[("count", &count.to_string())],
                                )),
                        );
                    }
                    if let Some(location) = location {
                        this = this.child(
                            div()
                                .id("status-yaml-location")
                                .test_support()
                                .flex_none()
                                .child(t_fmt(
                                    L10nKey::EditorLnCol,
                                    &[
                                        ("line", &location.line.to_string()),
                                        ("column", &location.column.to_string()),
                                    ],
                                )),
                        );
                    }
                    if parsing {
                        this = this.child(
                            div()
                                .id("status-yaml-parsing")
                                .test_support()
                                .flex_none()
                                .child(t(L10nKey::EditorYamlParsing)),
                        );
                    }
                    if let Some((title, _)) = fatal {
                        this = this.child(
                            div()
                                .id("status-yaml-error")
                                .test_support()
                                .flex_none()
                                .text_color(cx.theme().warning)
                                .child(title),
                        );
                    }
                    this
                },
            )
            .when_some(wrap, |this, wrap| {
                let label = if wrap {
                    t(L10nKey::EditorWrapOn)
                } else {
                    t(L10nKey::EditorWrapOff)
                };
                this.child(
                    item("status-wrap", label.into())
                        .on_click(status_command(EditorStatusCommand::ToggleWrap, cx)),
                )
            })
            // Tabular figures, so the position does not jitter sideways as
            // the caret walks from line 9 to line 10. A click asks for a line
            // to go to.
            .when_some(cursor, |this, t| {
                this.child(
                    item("status-cursor", t)
                        .font_features(lyrel_ui_base::display::tabular_figures())
                        .on_click(status_command(EditorStatusCommand::GoToLine, cx)),
                )
            })
            .when_some(indent, |this, t| this.child(div().flex_none().child(t)))
            .when_some(encoding, |this, t| this.child(div().flex_none().child(t)))
            .when_some(line_ending, |this, eol| {
                this.child(
                    item("status-eol", eol.into())
                        .tooltip(t(L10nKey::EditorToggleLineEnding))
                        .on_click(status_command(EditorStatusCommand::ToggleLineEnding, cx)),
                )
            })
            // A picture states what it is and what it cannot be: no caret,
            // no wrap, no line ending, and nothing to save.
            .when_some(image_status, |this, status| {
                this.child(
                    div()
                        .id("status-image-facts")
                        .test_support()
                        .aria_label(status.clone())
                        .flex_none()
                        .child(status),
                )
            })
            .when_some(image_page, |this, page| {
                this.child(
                    div()
                        .id("status-image-page")
                        .test_support()
                        .aria_label(page.clone())
                        .flex_none()
                        .child(page),
                )
            })
            .when(image_zoom.is_some(), |this| {
                this.child(
                    div()
                        .id("status-image-readonly")
                        .test_support()
                        .flex_none()
                        .child(t(L10nKey::EditorImageReadOnly)),
                )
            })
            .when_some(image_zoom, |this, zoom| {
                this.child(
                    div()
                        .id("status-image-zoom")
                        .test_support()
                        .flex_none()
                        .child(zoom),
                )
            })
            .when_some(book_chapter, |this, chapter| {
                this.child(
                    div()
                        .id("status-book-chapter")
                        .test_support()
                        .aria_label(chapter.clone())
                        .flex_none()
                        .child(chapter),
                )
            })
            .when_some(book_progress, |this, progress| {
                this.child(
                    div()
                        .id("status-book-progress")
                        .test_support()
                        .flex_none()
                        .child(progress),
                )
            })
            .when_some(book_status, |this, status| {
                this.child(
                    div()
                        .id("status-book-facts")
                        .test_support()
                        .aria_label(status.clone())
                        .flex_none()
                        .child(status),
                )
            })
            // A draw.io document has no caret, wrap or encoding; its status
            // names the engine, pages and zoom instead.
            .when_some(drawio_status, |this, status| {
                this.child(
                    div()
                        .id("status-drawio-facts")
                        .test_support()
                        .aria_label(status.clone())
                        .flex_none()
                        .child(status),
                )
            })
            .when_some(drawio_page, |this, page| {
                this.child(
                    div()
                        .id("status-drawio-page")
                        .test_support()
                        .aria_label(page.clone())
                        .flex_none()
                        .child(page),
                )
            })
            .when(drawio.is_some(), |this| {
                this.child(
                    div()
                        .id("status-drawio-editable")
                        .test_support()
                        .flex_none()
                        .child(t(L10nKey::EditorDrawioEditable)),
                )
            })
            .when_some(drawio_zoom, |this, zoom| {
                this.child(
                    div()
                        .id("status-drawio-zoom")
                        .test_support()
                        .flex_none()
                        .child(zoom),
                )
            })
            // A PDF states its page identity, zoom and read-only nature; it has
            // no caret, wrap, line ending or editable text format.
            .when_some(pdf_status, |this, status| {
                this.child(
                    div()
                        .id("status-pdf-facts")
                        .test_support()
                        .flex_none()
                        .child(status),
                )
            })
            // A database states its actual read view, page and journal mode.
            .when_some(sqlite_status, |this, status| {
                this.child(
                    div()
                        .id("status-sqlite-facts")
                        .test_support()
                        .flex_none()
                        .child(status),
                )
            })
            .when(pdf_zoom.is_some(), |this| {
                this.child(
                    div()
                        .id("status-pdf-readonly")
                        .test_support()
                        .flex_none()
                        .child(t(L10nKey::EditorImageReadOnly)),
                )
            })
            .when_some(pdf_zoom, |this, zoom| {
                this.child(
                    div()
                        .id("status-pdf-zoom")
                        .test_support()
                        .flex_none()
                        .child(zoom),
                )
            })
            .when_some(language, |this, l| this.child(div().flex_none().child(l)))
            .into_any_element()
    }
}

fn status_command(
    command: EditorStatusCommand,
    cx: &Context<EditorPanelState>,
) -> impl Fn(&gpui_kit::ClickEvent, &mut Window, &mut gpui_kit::App) + use<> {
    let editor = cx.entity().downgrade();
    move |_, _, cx| {
        let _ = editor.update(cx, |_, cx| {
            cx.emit(EditorPanelEvent::StatusCommand(command))
        });
    }
}

fn dirs_home() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

#[cfg(test)]
mod tests {
    use super::*;
    /// Handing a file the editor cannot read to the desktop is how a click
    /// opens a PNG. It must not be how a click runs a build's output.
    #[cfg(unix)]
    #[test]
    fn a_file_the_desktop_would_run_is_not_handed_to_it() {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join(format!("lyrel-program-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create dir");
        let image = dir.join("shot.png");
        let program = dir.join("built");
        std::fs::write(&image, b"\x89PNG\0\0").expect("write image");
        std::fs::write(&program, b"\x7fELF\0\0").expect("write program");
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755))
            .expect("mark executable");

        assert!(!is_program(&image), "a picture is only ever shown");
        assert!(is_program(&program), "a binary would be launched");
        assert!(
            !is_program(&dir),
            "a directory is not a program, whatever its mode says"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn language_map_covers_common_extensions() {
        for (path, lang) in [
            ("a/b/main.rs", "rust"),
            ("x.tsx", "tsx"),
            ("x.jsx", "javascript"),
            ("x.yml", "yaml"),
            ("Makefile", "make"),
            ("CMakeLists.txt", "cmake"),
            (".zshrc", "bash"),
            ("notes.md", "markdown"),
            ("notes.mdx", "markdown"),
            ("notes.MARKDOWN", "markdown"),
            ("query.SQL", "sql"),
            ("unknown.xyz", "text"),
            ("no_ext", "text"),
        ] {
            assert_eq!(language_for_path(Path::new(path)), lang, "path {path}");
        }
    }

    #[test]
    fn the_fingerprint_does_not_depend_on_how_the_text_is_chunked() {
        let text = "fn main() {\n    println!(\"中文\");\n}\n";
        let whole = Fingerprint::of_str(text);
        let (a, b) = text.split_at(13);
        assert_eq!(
            Fingerprint::of_chunks(text.len(), [a, b].into_iter()),
            whole
        );
        assert_ne!(Fingerprint::of_str("fn main() {}\n"), whole);
    }

    #[test]
    fn only_the_span_that_changed_is_replaced() {
        assert_eq!(differing_span("abc", "abc"), None);
        assert_eq!(
            differing_span("hello world", "hello there world"),
            Some((6, 6, 12))
        );
        assert_eq!(differing_span("abc", "abXc"), Some((2, 2, 3)));
        assert_eq!(differing_span("abc", ""), Some((0, 3, 0)));
        assert_eq!(differing_span("", "new"), Some((0, 0, 3)));
        // "aaa" → "aa": the overlap of prefix and suffix must not double-count.
        assert_eq!(differing_span("aaa", "aa"), Some((2, 3, 2)));
    }

    #[test]
    fn a_changed_span_never_splits_a_character() {
        // 中 and 丰 share their first two bytes in UTF-8.
        let (s, oe, ne) = differing_span("x中y", "x丰y").unwrap();
        let (old, new) = ("x中y", "x丰y");
        assert!(old.is_char_boundary(s) && new.is_char_boundary(s));
        assert_eq!(&old[s..oe], "中");
        assert_eq!(&new[s..ne], "丰");
    }

    #[test]
    fn an_agent_is_told_the_lines_a_selection_covers() {
        let text = gpui_kit::component::input::Rope::from("one\ntwo\nthree\n");
        assert_eq!(selected_lines(&text, 0..0), None);
        assert_eq!(selected_lines(&text, 1..2), Some((1, 1)));
        assert_eq!(selected_lines(&text, 2..9), Some((1, 3)));
        // Whole lines picked by dragging to the start of the next one.
        assert_eq!(selected_lines(&text, 4..14), Some((2, 3)));
        assert_eq!(selected_lines(&text, 0..4), Some((1, 1)));
    }

    #[test]
    fn go_to_line_reads_the_forms_people_type() {
        assert_eq!(parse_line_target("120"), Some((120, 1)));
        assert_eq!(parse_line_target(" 120:4 "), Some((120, 4)));
        assert_eq!(parse_line_target(":7"), Some((7, 1)));
        assert_eq!(parse_line_target("7,3"), Some((7, 3)));
        assert_eq!(parse_line_target("7:"), Some((7, 1)));
        assert_eq!(parse_line_target("0"), None);
        assert_eq!(parse_line_target("abc"), None);
        assert_eq!(parse_line_target(""), None);
    }

    #[test]
    fn a_strip_opens_new_files_beside_the_current_one_and_closes_toward_the_right() {
        let ids: Vec<BufferId> = (0..4).map(|_| BufferId::new()).collect();
        let mut code = TabCode::new();
        code.show(ids[0]);
        code.show(ids[1]);
        assert_eq!(code.files, vec![ids[0], ids[1]]);
        code.active = 0;
        code.show(ids[2]);
        assert_eq!(
            code.files,
            vec![ids[0], ids[2], ids[1]],
            "opened right of the active one"
        );
        assert_eq!(code.active, 1);

        // Showing one already open only brings it forward.
        code.show(ids[1]);
        assert_eq!(code.files.len(), 3);
        assert_eq!(code.active, 2);

        // Closing the active file brings its right-hand neighbour forward,
        // or the left one at the end of the strip.
        code.active = 1;
        assert!(code.forget(ids[2]));
        assert_eq!(code.files, vec![ids[0], ids[1]]);
        assert_eq!(code.active, 1);
        assert!(code.forget(ids[1]));
        assert_eq!(code.active, 0);
        // Closing one left of the active file keeps the same file in front.
        code.show(ids[3]);
        assert_eq!(code.files, vec![ids[0], ids[3]]);
        assert!(code.forget(ids[0]));
        assert_eq!(code.active_id(), Some(ids[3]));
        assert!(!code.forget(ids[0]));
    }

    #[test]
    fn files_arriving_in_the_background_keep_their_order_and_the_front_file() {
        let ids: Vec<BufferId> = (0..4).map(|_| BufferId::new()).collect();
        let mut code = TabCode::new();
        code.show(ids[0]);
        // A restore lists files in the order they were recorded; showing each
        // beside the active one would have reversed them.
        code.adopt(&ids[1..]);
        assert_eq!(code.files, ids);
        assert_eq!(code.active_id(), Some(ids[0]));
        code.adopt(&ids[2..3]);
        assert_eq!(
            code.files.len(),
            4,
            "a file already listed is not listed twice"
        );
    }
}
