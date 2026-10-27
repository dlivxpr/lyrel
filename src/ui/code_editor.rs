use crate::ui::notification_history::NotificationWindowExt as _;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui_kit::TestSupportExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{
    Editor, EditorState, Input, InputEvent, InputState, Position, RopeExt as _, TabSize,
};
use gpui_kit::component::menu::{ContextMenuExt as _, PopupMenu, PopupMenuItem};
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, Context, Entity, EntityInputHandler as _, Focusable as _, MouseButton, PromptLevel,
    SharedString, Subscription, Window, div, px, rems,
};
use lyrel_core::core::machine::TabId;

use crate::ui::app::LyrelApp;
use crate::ui::document_column::DocumentChrome;
use crate::ui::editor_session::{self, TabEditor};
use crate::ui::editor_text::{self, EditorConfig, Indent, LineEnding, TextFormat};
use crate::ui::host_ops::{HostId, HostOps, MTime, SharedHost, WatchSub};
use crate::ui::i18n::{L10nKey, t, t_fmt};
use crate::ui::window_chrome::{TITLE_BAR_BORDER, TitleStripRow as _};

mod buffers;
pub(crate) mod commands;
use buffers::Loaded;
mod gutter;
mod nav;
pub(crate) mod outline;
mod problems;
mod split;
mod strip;

pub(crate) use nav::KEY_CONTEXT as NAV_KEY_CONTEXT;

/// An open buffer is named by its input's entity id: unique, stable for the
/// buffer's life, and already what every async landing looks it up by.
pub(crate) type BufferId = gpui_kit::EntityId;

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
pub(crate) enum DiskConflict {
    /// Something else wrote the file while the buffer had edits of its own.
    /// Carries the modification time that was seen, so "Keep mine" can accept
    /// exactly that version as the one a save may replace.
    Changed(Option<MTime>),
    /// The file is gone. The buffer is all that is left of it.
    Deleted,
}

/// One open file, shared by every tab that shows it.
///
/// Buffers used to belong to a tab, which meant the same file open in two tabs
/// was two independent copies: edits in one were invisible to the other, and
/// saving either silently threw the other's away. Now a tab only lists which
/// buffers it shows, and the text lives here once.
pub(crate) struct OpenFile {
    pub(crate) path: PathBuf,
    /// Set for a file that has never been saved; `path` is empty until then.
    untitled: Option<u32>,
    /// The machine `path` lives on, held rather than looked up. Saves,
    /// reloads and duplicate detection all key on its id — an SFTP file and a
    /// local file can share the string `/etc/hosts` without being the same
    /// file — and saving goes straight through this handle, so a buffer stays
    /// saveable however the window's own machine has changed underneath it.
    pub(crate) host: SharedHost,
    pub(crate) input: Entity<EditorState>,
    pub(crate) dirty: bool,
    saved: Fingerprint,
    /// How the bytes on disk are encoded, so a save writes them back the same
    /// way — a CRLF file stays CRLF, a GB18030 file stays GB18030.
    format: TextFormat,
    saved_format: TextFormat,
    config: EditorConfig,
    indent: Indent,
    disk_mtime: Option<MTime>,
    saving: bool,
    save_pending: bool,
    save_then_close: bool,
    reload_seq: u64,
    pub(crate) conflict: Option<DiskConflict>,
    pub(crate) preview: bool,
    pub(crate) wrap: bool,
    /// The rendered-Markdown pane's own scroll. Per file, so switching away
    /// and back lands where you were reading — and so the pane can carry the
    /// scrollbar every other scrolling surface in lyrel has. The editor itself
    /// gets one from `Input`.
    pub(crate) preview_scroll: gpui_kit::ScrollHandle,
    /// Git change markers — see `gutter`.
    gutter: gutter::BufferGutter,
    popups: Entity<crate::ui::lsp::popups::EditorPopups>,
    _sub: Subscription,
    _observe: Subscription,
}

impl OpenFile {
    fn id(&self) -> BufferId {
        self.input.entity_id()
    }

    fn label(&self) -> SharedString {
        if let Some(n) = self.untitled {
            return t_fmt(L10nKey::EditorUntitled, &[("n", &n.to_string())]).into();
        }
        self.path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| self.path.display().to_string())
            .into()
    }

    fn is_at(&self, host: HostId, path: &Path) -> bool {
        self.untitled.is_none() && self.host.id() == host && self.path == path
    }

    fn language(&self) -> &'static str {
        if self.untitled.is_some() {
            return "text";
        }
        language_for_path(&self.path)
    }

    fn local(&self) -> Option<LocalFile> {
        (self.untitled.is_none() && self.host.id().is_local()).then(|| LocalFile {
            id: self.id(),
            input: self.input.clone(),
            popups: self.popups.clone(),
            path: self.path.clone(),
            language: self.language(),
            indent: self.indent,
        })
    }
}

/// A saved file on this machine, as the language servers see it.
pub(crate) struct LocalFile {
    pub(crate) id: BufferId,
    pub(crate) input: Entity<EditorState>,
    pub(crate) popups: Entity<crate::ui::lsp::popups::EditorPopups>,
    pub(crate) path: PathBuf,
    pub(crate) language: &'static str,
    pub(crate) indent: Indent,
}

/// One tab's view of the editor: which buffers it shows, in the order its
/// strip draws them, and which one is in front.
pub(crate) struct TabCode {
    pub(crate) visible: bool,
    pub(crate) files: Vec<BufferId>,
    pub(crate) active: usize,
    pub(crate) roots: Vec<PathBuf>,
    pub(crate) expanded: std::collections::HashSet<PathBuf>,
    pub(crate) selected: Option<PathBuf>,
    /// The second editor group, when the editor is split — the one without
    /// the focus. `files` and `active` above are always the focused group's
    /// (see `split`).
    pub(crate) split: Option<split::OtherGroup>,
    /// Buffers in the order they were last in front, most recent first —
    /// what decides which files keep a place in the header's strip when
    /// it cannot hold them all (see `strip`).
    pub(crate) recent: Vec<BufferId>,
}

impl TabCode {
    pub(crate) fn new() -> Self {
        Self {
            visible: false,
            files: Vec::new(),
            active: 0,
            roots: Vec::new(),
            expanded: std::collections::HashSet::new(),
            selected: None,
            split: None,
            recent: Vec::new(),
        }
    }

    pub(crate) fn active_id(&self) -> Option<BufferId> {
        self.files.get(self.active).copied()
    }

    /// Adds a buffer just right of the one in front, the way a browser opens
    /// a tab, and brings it forward. One the tab already shows only comes
    /// forward — it keeps its place in the strip.
    fn show(&mut self, id: BufferId) {
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
    fn append(&mut self, id: BufferId) {
        if !self.files.contains(&id) {
            self.files.push(id);
        }
    }

    /// Lists another strip's files after this one's, in their order.
    pub(crate) fn adopt(&mut self, ids: &[BufferId]) {
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

/// What a question about unsaved files was standing in the way of.
#[derive(Clone, Debug)]
pub(crate) enum AfterUnsaved {
    CloseTab(TabId),
    /// Take these files out of that tab's strip — Close Others and Close to
    /// the Right.
    CloseFiles(TabId, Vec<BufferId>),
    ClosePane(gpui_kit::EntityId),
    CloseWindow,
    Quit,
}

/// Saves that something is waiting on: once every buffer in `ids` has landed
/// clean, `then` runs. Any one of them failing drops the wait, and with it
/// the close it would have carried out.
struct SaveWaiter {
    ids: Vec<BufferId>,
    then: AfterUnsaved,
}

enum BarKind {
    GoToLine,
    SaveAs {
        id: BufferId,
        then_close: bool,
    },
    /// A new name for the symbol at `offset` (`ui::lsp`).
    Rename {
        id: BufferId,
        offset: usize,
    },
}

/// The one-line prompt that sits above the text: go to line, or name a file
/// to save to on a machine the native save panel cannot browse.
struct EditorBar {
    kind: BarKind,
    input: Entity<InputState>,
    _sub: Subscription,
}

pub(crate) struct EditorPanelState {
    buffers: buffers::EditorBuffers,
    bar: Option<EditorBar>,
    waiters: Vec<SaveWaiter>,
    unsaved_prompt_open: bool,
    /// Tabs whose remembered files have been reopened (or found to have none).
    restored: HashSet<TabId>,
    /// What was last written to the session store for each tab, so an
    /// unchanged tab costs a comparison per frame and nothing more.
    recorded: HashMap<TabId, TabEditor>,
    /// Back/forward history, outlines and Go to Symbol's preview.
    nav: nav::EditorNav,
    /// The Problems list at the foot of the panel — see `problems`.
    problems: problems::ProblemsPane,
    /// The header's list of every open file, while it is open.
    strip_picker: Option<strip::StripPicker>,
    /// The focus of a panel with no file in it. Without one there is nothing
    /// in the panel to hold the focus, so closing the last file handed it to
    /// the terminal and the next ⌘W closed the terminal instead of the panel.
    empty_focus: gpui_kit::FocusHandle,
    /// The tab whose panel held the focus when it was last drawn, so the
    /// frame after its last file closes knows whether to take the focus over
    /// — and a tab switched to does not take it from its own terminal.
    had_focus: Option<TabId>,
}

impl EditorPanelState {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<LyrelApp>) -> Self {
        let buffers = buffers::EditorBuffers::new(
            window,
            cx,
            |app| &mut app.editor.buffers,
            Self::buffer_edited,
        );
        Self {
            buffers,
            bar: None,
            waiters: Vec::new(),
            unsaved_prompt_open: false,
            restored: HashSet::new(),
            recorded: HashMap::new(),
            nav: nav::EditorNav::new(cx),
            problems: Default::default(),
            strip_picker: None,
            empty_focus: cx.focus_handle(),
            had_focus: None,
        }
    }

    fn buffer_edited(app: &mut LyrelApp, id: BufferId, cx: &mut Context<LyrelApp>) {
        app.editor_buffer_changed(id, cx);
    }

    /// File-tree dirtiness is Host-aware, like duplicate detection.
    pub(crate) fn is_dirty(&self, host: HostId, path: &Path) -> bool {
        self.buffers.is_dirty(host, path)
    }

    /// Only saved, local files are visible to language servers.
    pub(crate) fn local_files(&self) -> Vec<LocalFile> {
        self.buffers.iter().filter_map(OpenFile::local).collect()
    }

    pub(crate) fn local_file(&self, id: BufferId) -> Option<LocalFile> {
        self.buffers.get(id).and_then(OpenFile::local)
    }
}

pub(crate) fn language_for_path(path: &Path) -> &'static str {
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
        "json" | "jsonc" => "json",
        "toml" => "toml",
        "yaml" | "yml" => "yaml",
        "html" | "htm" => "html",
        "css" => "css",
        "md" | "markdown" => "markdown",
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
fn language_label(id: &str) -> Option<&'static str> {
    Some(match id {
        "rust" => "Rust",
        "go" => "Go",
        "python" => "Python",
        "javascript" => "JavaScript",
        "typescript" => "TypeScript",
        "tsx" => "TypeScript JSX",
        "json" => "JSON",
        "toml" => "TOML",
        "yaml" => "YAML",
        "html" => "HTML",
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
const CURSOR_SCROLL_ATTEMPTS: u8 = 3;

/// Puts the cursor at `position`, re-trying on later frames until the scroll
/// that follows it can actually be computed.
///
/// Applied immediately as well as on the retry: the cursor itself lands with
/// no layout, so the position is right even for a pane that never paints.
fn place_cursor(
    input: Entity<EditorState>,
    position: Position,
    left: u8,
    window: &mut Window,
    cx: &mut gpui_kit::App,
) {
    input.update(cx, |state, cx| {
        state.set_cursor_position(position, window, cx);
    });
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
    input: &Entity<EditorState>,
    new: &str,
    window: &mut Window,
    cx: &mut gpui_kit::App,
) {
    let old = input.read(cx).text().to_string();
    let Some((start, old_end, new_end)) = differing_span(&old, new) else {
        return;
    };
    let start16 = old[..start].encode_utf16().count();
    let end16 = start16 + old[start..old_end].encode_utf16().count();
    let replacement = &new[start..new_end];
    input.update(cx, |state, cx| {
        let cursor = state.cursor_position();
        state.replace_text_in_range(Some(start16..end16), replacement, window, cx);
        state.set_cursor_position(cursor, window, cx);
    });
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
fn is_program(path: &Path) -> bool {
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

/// A file a browser renders rather than showing its source.
fn opens_in_browser(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
        matches!(
            e.to_ascii_lowercase().as_str(),
            "html" | "htm" | "xhtml" | "svg"
        )
    })
}

/// Opens a local file in the default web browser. On macOS that is asked
/// for by name: what `.html` is associated with is as often an editor. The
/// other desktops go by the association.
fn open_in_browser(path: &Path) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    if let Some(bundle) = default_browser_bundle_id() {
        std::process::Command::new("open")
            .arg("-b")
            .arg(bundle)
            .arg(path)
            .spawn()?;
        return Ok(());
    }
    crate::terminal::view::open_file_path(path)
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
    text: &gpui_kit::component::input::Rope,
    range: std::ops::Range<usize>,
) -> Option<(usize, usize)> {
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

/// How a save attempt ended, as reported back from the host's thread.
enum SaveOutcome {
    Saved(Option<MTime>),
    /// The file changed on disk since the buffer last matched it; nothing
    /// was written.
    Conflict(Option<MTime>),
    Failed(std::io::Error),
}

/// Writes `bytes` to `target`, first making sure nothing else wrote it since
/// the buffer last matched it. `expect` is `None` to write regardless — a
/// Save As, an Overwrite the user chose, or a file being recreated.
///
/// This is also the only external-change detection a host without a watcher
/// gets: an SFTP buffer never hears about someone else's edit until now.
fn write_checked(
    h: &dyn lyrel_core::host::Host,
    target: &Path,
    bytes: &[u8],
    expect: Option<Option<MTime>>,
) -> SaveOutcome {
    if let Some(expected) = expect {
        match h.stat(target) {
            Ok(meta) if meta.mtime != expected => return SaveOutcome::Conflict(meta.mtime),
            Ok(_) => {}
            // Gone since it was opened: saving puts it back, which is what a
            // save of a deleted file means.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return SaveOutcome::Failed(e),
        }
    }
    match h.write_file(target, bytes) {
        Ok(meta) => SaveOutcome::Saved(meta.mtime),
        Err(e) => SaveOutcome::Failed(e),
    }
}

impl LyrelApp {
    pub(crate) fn tab_code(&self) -> Option<&TabCode> {
        self.tabs.get(self.tabs.active_index())?.code.as_deref()
    }

    pub(crate) fn tab_code_mut(&mut self) -> Option<&mut TabCode> {
        self.tabs.active_mut()?.code.as_deref_mut()
    }

    pub(crate) fn tab_code_mut_or_init(&mut self) -> Option<&mut TabCode> {
        let tab = self.tabs.active_mut()?;
        Some(tab.code.get_or_insert_with(|| Box::new(TabCode::new())))
    }

    pub(crate) fn code_panel_visible(&self) -> bool {
        self.tab_code().is_some_and(|c| c.visible)
    }

    /// The buffer in front of the active tab's editor.
    fn active_buffer(&self) -> Option<&OpenFile> {
        self.editor.buffers.get(self.tab_code()?.active_id()?)
    }

    /// Where the file in front of the active tab's editor lives, if it has
    /// been saved anywhere yet.
    pub(crate) fn editor_active_location(&self) -> Option<(HostId, &Path)> {
        let f = self.active_buffer()?;
        f.untitled
            .is_none()
            .then(|| (f.host.id(), f.path.as_path()))
    }

    fn tab_index_of(&self, tab: TabId) -> Option<usize> {
        self.tabs.iter().position(|t| t.id() == tab)
    }

    /// How many places show this buffer: every tab showing it, twice for a
    /// tab whose split shows it in both groups.
    fn buffer_refs(&self, id: BufferId) -> usize {
        self.tabs
            .iter()
            .filter_map(|t| t.code.as_deref())
            .map(|c| c.views_of(id))
            .sum()
    }

    /// How many tabs show this buffer, in either group.
    fn buffer_tabs(&self, id: BufferId) -> usize {
        self.tabs
            .iter()
            .filter_map(|t| t.code.as_deref())
            .filter(|c| c.shows(id))
            .count()
    }

    /// Unsaved buffers that only tab `tab_ix` shows — the ones closing it
    /// would lose. A buffer another tab also shows survives the close.
    pub(crate) fn editor_unsaved_in_tab(&self, tab_ix: usize) -> Vec<BufferId> {
        let Some(code) = self.tabs.get(tab_ix).and_then(|t| t.code.as_deref()) else {
            return Vec::new();
        };
        code.all_files()
            .into_iter()
            .filter(|id| {
                self.editor.buffers.get(*id).is_some_and(|b| b.dirty) && self.buffer_tabs(*id) == 1
            })
            .collect()
    }

    /// Every unsaved buffer in the window.
    pub(crate) fn editor_unsaved(&self) -> Vec<BufferId> {
        self.editor
            .buffers
            .iter()
            .filter(|b| b.dirty)
            .map(OpenFile::id)
            .collect()
    }

    pub(crate) fn editor_context_changed(&mut self) {
        self.editor.buffers.invalidate_context();
    }

    fn editor_rebuild_watcher(&mut self, cx: &mut Context<Self>) {
        let host = self.active_host(cx);
        self.editor
            .buffers
            .rebuild_watch(host, cx, |app| &mut app.editor.buffers);
        self.lsp_sync_buffers(cx);
    }

    /// Shows what a file link in the grid pointed at.
    ///
    /// A file opens in the editor, on the line the link named; the tree
    /// follows along so "what does it say?" and "where does it live?" are
    /// answered by the same click. A directory has no contents to show, so it
    /// is only ever the tree.
    pub(crate) fn open_linked_file(
        &mut self,
        path: &Path,
        line: Option<u32>,
        column: Option<u32>,
        is_dir: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if is_dir {
            self.file_tree_show(path, window, cx);
            return;
        }
        self.open_file_in_editor_at(path, line, column, window, cx);
        self.file_tree_reveal_path(path, cx);
    }

    /// [`Self::open_file_in_editor`], landing the cursor on a line the caller
    /// already knows — what a `src/main.rs:120:3` in the grid was pointing at.
    pub(crate) fn open_file_in_editor_at(
        &mut self,
        path: &Path,
        line: Option<u32>,
        column: Option<u32>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(host) = self.active_host(cx) else {
            return;
        };
        let cursor = line.map(|line| Position {
            line: line.saturating_sub(1),
            character: column.unwrap_or(1).saturating_sub(1),
        });
        self.editor_open_on_host_at(host, path, cursor, window, cx);
    }

    pub(crate) fn open_file_in_editor(
        &mut self,
        path: &Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(host) = self.active_host(cx) else {
            return;
        };
        self.editor_open_on_host(host, path, window, cx);
    }

    /// [`Self::open_file_in_editor`] against an explicit host — the SFTP
    /// browser's files live on a host that is never the active one.
    pub(crate) fn editor_open_on_host(
        &mut self,
        host: SharedHost,
        path: &Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.editor_open_on_host_at(host, path, None, window, cx);
    }

    fn editor_open_on_host_at(
        &mut self,
        host: SharedHost,
        path: &Path,
        cursor: Option<Position>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.tabs.get(self.tabs.active_index()).map(|tab| tab.id()) else {
            return;
        };
        self.raise_code_overlay();
        let target = buffers::OpenTarget {
            workspace: self.workspace,
            tab,
            window_host: self.spawn_host(cx),
        };
        let reused = self.editor.buffers.open(
            host,
            path,
            cursor,
            target,
            window,
            cx,
            |app| &mut app.editor.buffers,
            EditorPanelState::buffer_edited,
            Self::editor_open_target_valid,
            Self::editor_opened,
        );
        if let Some(id) = reused {
            self.editor_show_in_tab(tab, id, true, window, cx);
        }
    }

    fn editor_open_target_valid(&self, target: buffers::OpenTarget, cx: &gpui_kit::App) -> bool {
        self.workspace == target.workspace
            && self.spawn_host(cx) == target.window_host
            && self.tab_index_of(target.tab).is_some()
    }

    fn editor_opened(
        &mut self,
        target: buffers::OpenTarget,
        result: buffers::OpenResult,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match result {
            buffers::OpenResult::Opened { id, created, front } => {
                if created {
                    self.editor_buffer_created(id, window, cx);
                }
                self.editor_show_in_tab(target.tab, id, front, window, cx);
                self.editor_rebuild_watcher(cx);
            }
            buffers::OpenResult::Failed {
                host,
                error: EditorOpenError::NotText(path),
            } => {
                self.open_outside_the_editor(host, &path, window, cx);
            }
            buffers::OpenResult::Failed {
                error: EditorOpenError::Message(message),
                ..
            } => {
                window.notify_app(message, cx);
            }
        }
    }

    /// What to do with a file the built-in editor cannot show.
    ///
    /// A click on a PNG or a `.zip` meant "open this", not "tell me it is not
    /// text", and on this machine the desktop knows how. A file on another
    /// machine has nobody here to hand it to, so that one gets the words.
    ///
    /// A program does not: `open` on a Mach-O binary runs it, and a build's
    /// output is full of paths to programs. Clicking a word in a terminal
    /// must not be a way to execute one, so those keep the words too.
    fn open_outside_the_editor(
        &mut self,
        host_id: HostId,
        path: &Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !host_id.is_local() || !self.can_spawn_locally(cx) || is_program(path) {
            window.notify_app(
                t_fmt(
                    L10nKey::EditorBinaryFile,
                    &[("path", &path.display().to_string())],
                ),
                cx,
            );
            return;
        }
        Self::open_with(path, crate::terminal::view::open_file_path, window, cx);
    }

    /// Runs one of the desktop openers on a local file, and says so when it
    /// fails to spawn — as any opener can (#542).
    fn open_with(
        path: &Path,
        opener: fn(&Path) -> std::io::Result<()>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Err(e) = opener(path) {
            log::warn!("failed to open {}: {e}", path.display());
            window.notify_app(
                crate::ui::host_ops::failure(
                    t_fmt(
                        L10nKey::LinkFileOpenFailed,
                        &[
                            ("path", &path.display().to_string()),
                            ("error", &e.to_string()),
                        ],
                    ),
                    &e,
                ),
                cx,
            );
        }
    }

    /// Puts a buffer in a tab's strip and, when `front`, in front of its
    /// editor with the panel open and focused.
    fn editor_show_in_tab(
        &mut self,
        tab: TabId,
        id: BufferId,
        front: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab_ix) = self.tab_index_of(tab) else {
            return;
        };
        let Some(t) = self.tabs.get_mut(tab_ix) else {
            return;
        };
        let code = t.code.get_or_insert_with(|| Box::new(TabCode::new()));
        if !front {
            // Listed, not brought forward: the one in front stays in front.
            code.append(id);
            cx.notify();
            return;
        }
        code.show(id);
        code.visible = true;
        if tab_ix == self.tabs.active_index() {
            self.editor.bar = None;
            self.focus_editor(window, cx);
        }
        cx.notify();
    }

    /// Makes a buffer out of a loaded file, or finds the one already open for
    /// it, and shows it in `tab`.
    fn editor_install(
        &mut self,
        host: SharedHost,
        loaded: Loaded,
        tab: TabId,
        front: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> BufferId {
        let (id, created) = self.editor.buffers.install(
            host,
            loaded,
            window,
            cx,
            |app| &mut app.editor.buffers,
            EditorPanelState::buffer_edited,
        );
        if created {
            self.editor_buffer_created(id, window, cx);
        }
        self.editor_show_in_tab(tab, id, front, window, cx);
        self.editor_rebuild_watcher(cx);
        id
    }

    #[allow(clippy::too_many_arguments)]
    fn editor_new_buffer(
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
        cx: &mut Context<Self>,
    ) -> BufferId {
        let id = self.editor.buffers.create(
            host,
            path,
            untitled,
            text,
            format,
            config,
            indent,
            mtime,
            window,
            cx,
            |app| &mut app.editor.buffers,
            EditorPanelState::buffer_edited,
        );
        self.editor_buffer_created(id, window, cx);
        id
    }

    fn editor_buffer_created(&mut self, id: BufferId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(file) = self.editor.buffers.get(id) else {
            return;
        };
        let input = file.input.clone();
        self.watch_document_status_focus(&input.read(cx).focus_handle(cx), window, cx);
        self.editor
            .nav
            .track_buffer(id, input.read(cx).text().clone());
    }

    /// Re-derives whether a buffer differs from what is on disk, after an
    /// edit of any kind — typing, undo, a reload, a save rule.
    fn editor_note_edit(&mut self, id: BufferId, cx: &mut Context<Self>) {
        self.editor.buffers.note_edit(id, cx);
        self.editor_buffer_changed(id, cx);
    }

    fn editor_buffer_changed(&mut self, id: BufferId, cx: &mut Context<Self>) {
        self.lsp_buffer_edited(id, cx);
        self.editor_gutter_note_edit(id, cx);
        self.editor_nav_note_edit(id, cx);
        cx.notify();
    }

    pub(crate) fn editor_active_local_file(&self) -> Option<LocalFile> {
        self.active_buffer().and_then(OpenFile::local)
    }

    /// Asks for a new name for the symbol at `offset`, in the bar above the
    /// text; `ui::lsp` does the renaming once it is answered.
    pub(crate) fn editor_open_rename_bar(
        &mut self,
        id: BufferId,
        offset: usize,
        current: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.editor_open_bar(BarKind::Rename { id, offset }, current, window, cx);
    }

    /// A new, empty, never-saved buffer, on the window's own machine.
    pub(crate) fn editor_new_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(host) = self.active_host(cx) else {
            return;
        };
        let Some(tab) = self.tabs.get(self.tabs.active_index()).map(|t| t.id()) else {
            return;
        };
        self.raise_code_overlay();
        let n = self.editor.buffers.next_untitled();
        let indent = editor_text::detect_indent("", "text");
        let id = self.editor_new_buffer(
            host,
            PathBuf::new(),
            Some(n),
            String::new(),
            TextFormat::default(),
            EditorConfig::default(),
            indent,
            None,
            window,
            cx,
        );
        self.editor_show_in_tab(tab, id, true, window, cx);
    }

    /// A scrollback export is an unsaved document, never a filesystem path on
    /// the pane's host. Saving uses the ordinary Save As and close safeguards.
    pub(crate) fn editor_open_scrollback(
        &mut self,
        text: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.tabs.get(self.tabs.active_index()).map(|t| t.id()) else {
            return;
        };
        let host: SharedHost = lyrel_core::host::local::LocalHost::new();
        let n = self.editor.buffers.next_untitled();
        let indent = editor_text::detect_indent(&text, "text");
        let dirty = !text.is_empty();
        let id = self.editor_new_buffer(
            host,
            PathBuf::new(),
            Some(n),
            text,
            TextFormat::default(),
            EditorConfig::default(),
            indent,
            None,
            window,
            cx,
        );
        if let Some(file) = self.editor.buffers.get_mut(id) {
            file.saved = Fingerprint::of_str("");
            file.dirty = dirty;
        }
        self.raise_code_overlay();
        self.editor_show_in_tab(tab, id, true, window, cx);
    }

    pub(crate) fn toggle_code_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.active_mut() else {
            return;
        };
        let buried = tab.reveal_code();
        if buried {
            self.focus_editor(window, cx);
            cx.notify();
            return;
        }
        let Some(tab) = self.tabs.active_mut() else {
            return;
        };
        let code = tab.code.get_or_insert_with(|| Box::new(TabCode::new()));
        if code.visible {
            code.visible = false;
            self.editor.had_focus = None;
            self.editor.bar = None;
            self.file_tree.editing = None;
            self.focus_active(window, cx);
            cx.notify();
            return;
        }
        code.visible = true;
        self.file_tree_refresh_roots(window, cx);
        if self.active_buffer().is_some() {
            self.focus_editor(window, cx);
        } else {
            // With no file to show, the panel says "Open a file from the file
            // tree" and hands the tree the focus — but nothing was putting the
            // tree on screen, so ⌘⇧E on a fresh tab opened an empty editor
            // pointing at a panel the reader could not see or reach from
            // there. Reveal it, then focus it.
            if !self.file_tree_on_screen(cx) {
                self.set_right_panel_tab(crate::core::config::RightPanelTab::Files, cx);
            }
            self.file_tree.focus_handle.focus(window, cx);
        }
        cx.notify();
    }

    fn raise_code_overlay(&mut self) {
        if let Some(tab) = self.tabs.active_mut() {
            tab.reveal_code();
        }
    }

    fn focus_editor(&self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(f) = self.active_buffer() {
            f.input.update(cx, |input, cx| input.focus(window, cx));
        }
    }

    /// Settings borrows focus, not the terminal's place in the workbench.
    /// Keep explicit terminal commands on `focus_active`; only an overlay's
    /// return path restores the foreground document or the last docked shell.
    pub(crate) fn focus_previous_content(&self, window: &mut Window, cx: &mut Context<Self>) {
        match self.status_document_context(window, cx) {
            Some(crate::ui::tab::OverlayTop::Code) => {
                if self.active_buffer().is_some_and(|file| !file.preview) {
                    self.focus_editor(window, cx);
                } else {
                    self.editor.empty_focus.focus(window, cx);
                }
            }
            Some(crate::ui::tab::OverlayTop::Diff) => {
                if let Some(diff) = self
                    .tabs
                    .get(self.tabs.active_index())
                    .and_then(|tab| tab.diff_overlay.as_ref())
                {
                    window.focus(&diff.focus_handle, cx);
                }
            }
            None => self.focus_active(window, cx),
        }
    }

    /// Brings the file at `pos` in the active tab's strip to the front.
    pub(crate) fn editor_activate(
        &mut self,
        pos: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(code) = self.tab_code_mut() else {
            return;
        };
        if pos >= code.files.len() {
            return;
        }
        code.active = pos;
        self.editor.bar = None;
        self.focus_editor(window, cx);
        cx.notify();
    }

    /// The status bar's Preview / Edit button, and `ToggleDocumentPreview`.
    /// Acts on the active tab's open file, and only when that file is
    /// Markdown and the code panel is on screen — anything else has no
    /// rendered form to switch to. The new state is remembered for the next
    /// Markdown file opened.
    pub(crate) fn toggle_document_preview(&mut self, cx: &mut Context<Self>) {
        if !self.code_panel_visible() {
            return;
        }
        let Some(id) = self.tab_code().and_then(TabCode::active_id) else {
            return;
        };
        let Some(f) = self.editor.buffers.get_mut(id) else {
            return;
        };
        if f.language() != "markdown" {
            return;
        }
        f.preview = !f.preview;
        let preview = f.preview;
        self.update_config(cx, |cfg| cfg.editor_markdown_preview = preview);
        cx.notify();
    }

    /// The status bar's Wrap button, and `ToggleDocumentWrap`. Same reach as
    /// [`Self::toggle_document_preview`], for any file; the new state becomes
    /// what the next file opens with.
    pub(crate) fn toggle_document_wrap(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.code_panel_visible() {
            return;
        }
        let Some(id) = self.tab_code().and_then(TabCode::active_id) else {
            return;
        };
        let Some(f) = self.editor.buffers.get_mut(id) else {
            return;
        };
        f.wrap = !f.wrap;
        let wrap = f.wrap;
        f.input.clone().update(cx, |st, cx| {
            st.set_soft_wrap(wrap, window, cx);
        });
        self.update_config(cx, |cfg| cfg.editor_soft_wrap = wrap);
        cx.notify();
    }

    /// The status bar's line-ending button: flips the file between LF and
    /// CRLF. The buffer itself always holds `\n`; this only changes what a
    /// save writes, so it marks the file unsaved without touching its text.
    fn toggle_line_ending(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.tab_code().and_then(TabCode::active_id) else {
            return;
        };
        let Some(f) = self.editor.buffers.get_mut(id) else {
            return;
        };
        f.format.line_ending = match f.format.line_ending {
            LineEnding::Lf => LineEnding::CrLf,
            LineEnding::CrLf => LineEnding::Lf,
        };
        self.editor_note_edit(id, cx);
    }

    /// Run one of gpui-component's editing actions on the active buffer, as
    /// if it had been pressed there. Returns false with no buffer to run it
    /// on. The palette closes before its row runs, so this focuses the
    /// buffer first rather than relying on where the focus happens to be.
    pub(crate) fn editor_dispatch(
        &mut self,
        action: &dyn gpui_kit::Action,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.code_panel_visible() {
            return false;
        }
        let Some(input) = self.active_buffer().map(|f| f.input.clone()) else {
            return false;
        };
        let handle = input.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
        handle.dispatch_action(action, window, cx);
        true
    }

    /// The window's listeners for the editor's text commands, when the user
    /// has bound one: each runs on the editor only while it has the focus.
    pub(crate) fn with_editor_text_commands(
        root: gpui_kit::Stateful<gpui_kit::Div>,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Stateful<gpui_kit::Div> {
        let root = commands::install(root, cx);
        use crate::core::actions::{
            EditorJoinLines, EditorRemoveSurroundingBrackets, EditorTransformLowercase,
            EditorTransformTitleCase, EditorTransformUppercase, EditorTrimTrailingWhitespace,
        };
        root.on_action(
            cx.listener(|this, _: &EditorTransformUppercase, window, cx| {
                if !this.editor_has_focus(window, cx) {
                    cx.propagate();
                    return;
                }
                this.editor_dispatch(
                    &crate::ui::code_editor::commands::TransformToUppercase,
                    window,
                    cx,
                );
            }),
        )
        .on_action(
            cx.listener(|this, _: &EditorTransformLowercase, window, cx| {
                if !this.editor_has_focus(window, cx) {
                    cx.propagate();
                    return;
                }
                this.editor_dispatch(
                    &crate::ui::code_editor::commands::TransformToLowercase,
                    window,
                    cx,
                );
            }),
        )
        .on_action(
            cx.listener(|this, _: &EditorTransformTitleCase, window, cx| {
                if !this.editor_has_focus(window, cx) {
                    cx.propagate();
                    return;
                }
                this.editor_dispatch(
                    &crate::ui::code_editor::commands::TransformToTitleCase,
                    window,
                    cx,
                );
            }),
        )
        .on_action(
            cx.listener(|this, _: &EditorTrimTrailingWhitespace, window, cx| {
                if !this.editor_has_focus(window, cx) {
                    cx.propagate();
                    return;
                }
                this.editor_dispatch(
                    &crate::ui::code_editor::commands::TrimTrailingWhitespace,
                    window,
                    cx,
                );
            }),
        )
        .on_action(cx.listener(|this, _: &EditorJoinLines, window, cx| {
            if !this.editor_has_focus(window, cx) {
                cx.propagate();
                return;
            }
            this.editor_dispatch(&crate::ui::code_editor::commands::JoinLines, window, cx);
        }))
        .on_action(
            cx.listener(|this, _: &EditorRemoveSurroundingBrackets, window, cx| {
                if !this.editor_has_focus(window, cx) {
                    cx.propagate();
                    return;
                }
                this.editor_dispatch(
                    &crate::ui::code_editor::commands::RemoveSurroundingBrackets,
                    window,
                    cx,
                );
            }),
        )
    }

    /// Whether an editor exists to run [`Self::editor_dispatch`] on.
    pub(crate) fn editor_can_dispatch(&self) -> bool {
        self.code_panel_visible() && self.active_buffer().is_some()
    }

    pub(crate) fn editor_has_focus(&self, window: &Window, cx: &Context<Self>) -> bool {
        self.code_panel_visible()
            && self.active_buffer().is_some_and(|f| {
                f.input
                    .read(cx)
                    .focus_handle(cx)
                    .contains_focused(window, cx)
            })
    }

    /// Whether anything in the editor panel — the text, its search bar, the
    /// go-to-line prompt — has the focus. Wider than
    /// [`Self::editor_has_focus`], for keys that belong to the panel as a
    /// whole: ⌘S from the find box should still save.
    pub(crate) fn editor_panel_has_focus(&self, window: &Window, cx: &Context<Self>) -> bool {
        self.editor_has_focus(window, cx)
            || (self.code_panel_visible() && self.editor.empty_focus.is_focused(window))
            || self.editor.bar.as_ref().is_some_and(|b| {
                b.input
                    .read(cx)
                    .focus_handle(cx)
                    .contains_focused(window, cx)
            })
    }

    pub(crate) fn editor_save_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.tab_code().and_then(TabCode::active_id) else {
            return;
        };
        self.editor_save_file(id, false, false, window, cx);
    }

    pub(crate) fn editor_save_as_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.tab_code().and_then(TabCode::active_id) else {
            return;
        };
        self.editor_save_as(id, false, window, cx);
    }

    /// Applies the file's `.editorconfig` save rules — trailing whitespace,
    /// final newline — to the buffer itself, before it is written, so what is
    /// on screen is what is on disk.
    fn apply_save_rules(&mut self, id: BufferId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(f) = self.editor.buffers.get(id) else {
            return;
        };
        let text = f.input.read(cx).text().to_string();
        if let Some(fixed) = editor_text::apply_save_rules(&text, &f.config) {
            let input = f.input.clone();
            replace_buffer_text(&input, &fixed, window, cx);
        }
    }

    fn editor_save_file(
        &mut self,
        id: BufferId,
        then_close: bool,
        force: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(f) = self.editor.buffers.get_mut(id) else {
            return;
        };
        if f.untitled.is_some() {
            self.editor_save_as(id, then_close, window, cx);
            return;
        }
        f.save_then_close |= then_close;
        if f.saving {
            f.save_pending = true;
            return;
        }
        self.apply_save_rules(id, window, cx);
        let Some(f) = self.editor.buffers.get_mut(id) else {
            return;
        };
        let text = f.input.read(cx).text().to_string();
        // Pasted text can carry its own `\r\n`; the buffer's line ending is
        // the file's, and encoding adds it back uniformly.
        let text = if text.contains("\r\n") {
            text.replace("\r\n", "\n")
        } else {
            text
        };
        let bytes = match editor_text::encode(&text, &f.format) {
            Ok(bytes) => bytes,
            Err(e) => {
                self.editor_offer_utf8(id, then_close, e.unmappable, window, cx);
                return;
            }
        };
        let Some(f) = self.editor.buffers.get_mut(id) else {
            return;
        };
        f.saving = true;
        let expect = match (force, f.conflict) {
            (true, _) | (_, Some(DiskConflict::Deleted)) => None,
            _ => Some(f.disk_mtime),
        };
        let written = Fingerprint::of_str(&text);
        let format = f.format;
        let host = f.host.clone();
        let target = f.path.clone();
        let host_id = host.id();
        let saved_in = target.parent().map(Path::to_path_buf);
        HostOps::run_in(
            host,
            window,
            cx,
            move |h| write_checked(h, &target, &bytes, expect),
            move |app, outcome: SaveOutcome, window, cx| {
                let Some(f) = app.editor.buffers.get_mut(id) else {
                    return;
                };
                f.saving = false;
                let pending = std::mem::take(&mut f.save_pending);
                match outcome {
                    SaveOutcome::Saved(mtime) => {
                        f.disk_mtime = mtime;
                        f.saved = written;
                        f.saved_format = format;
                        f.conflict = None;
                        app.editor_note_edit(id, cx);
                        app.lsp_buffer_saved(id, cx);
                        app.editor_gutter_refetch(id);
                        // A save is a working-tree edit the `.git` watch cannot
                        // see, and the file tree only sees it while it happens
                        // to be showing that directory.
                        if let Some(dir) = &saved_in {
                            app.scm_invalidate_cwd(host_id, dir, cx);
                        }
                        if pending {
                            app.editor_save_file(id, false, false, window, cx);
                            return;
                        }
                        let close =
                            app.editor.buffers.get_mut(id).is_some_and(|f| {
                                std::mem::take(&mut f.save_then_close) && !f.dirty
                            });
                        if close {
                            app.editor_drop_buffer(id, cx);
                        }
                        app.editor_saves_landed(window, cx);
                    }
                    SaveOutcome::Conflict(observed) => {
                        f.save_then_close = false;
                        f.conflict = Some(DiskConflict::Changed(observed));
                        app.editor_saves_failed(id);
                        app.editor_ask_overwrite(id, window, cx);
                    }
                    SaveOutcome::Failed(e) => {
                        f.save_then_close = false;
                        // "Save failed" did not say which file, and with more
                        // than one editor tab open that is the first thing you
                        // need to know.
                        let context = t_fmt(L10nKey::EditorSaveFailed, &[("name", &f.label())]);
                        HostOps::notify_err(window, cx, &context, &e);
                        app.editor_saves_failed(id);
                    }
                }
                cx.notify();
            },
        );
        cx.notify();
    }

    /// Asked when a save finds the file changed underneath it.
    fn editor_ask_overwrite(&mut self, id: BufferId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(f) = self.editor.buffers.get(id) else {
            return;
        };
        let name = f.label();
        let answer = window.prompt(
            PromptLevel::Warning,
            &t_fmt(L10nKey::EditorSaveConflictTitle, &[("name", &name)]),
            Some(t(L10nKey::EditorSaveConflictBody)),
            &crate::ui::confirm_answers(t(L10nKey::EditorOverwrite), t(L10nKey::Cancel)),
            cx,
        );
        cx.spawn_in(window, async move |app, cx| {
            if !matches!(answer.await, Ok(0)) {
                return;
            }
            let _ = app.update_in(cx, |app, window, cx| {
                app.editor_save_file(id, false, true, window, cx);
            });
        })
        .detach();
    }

    /// Asked when the text holds a character the file's encoding cannot.
    fn editor_offer_utf8(
        &mut self,
        id: BufferId,
        then_close: bool,
        unmappable: char,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(f) = self.editor.buffers.get_mut(id) else {
            return;
        };
        f.save_then_close = false;
        let name = f.label();
        let encoding = f.format.encoding_label();
        self.editor_saves_failed(id);
        let answer = window.prompt(
            PromptLevel::Warning,
            &t_fmt(
                L10nKey::EditorEncodeFailedTitle,
                &[("name", &name), ("encoding", encoding)],
            ),
            Some(&t_fmt(
                L10nKey::EditorEncodeFailedBody,
                &[("ch", &unmappable.to_string()), ("encoding", encoding)],
            )),
            &crate::ui::confirm_answers(t(L10nKey::EditorSaveAsUtf8), t(L10nKey::Cancel)),
            cx,
        );
        cx.spawn_in(window, async move |app, cx| {
            if !matches!(answer.await, Ok(0)) {
                return;
            }
            let _ = app.update_in(cx, |app, window, cx| {
                if let Some(f) = app.editor.buffers.get_mut(id) {
                    f.format = TextFormat {
                        line_ending: f.format.line_ending,
                        ..TextFormat::default()
                    };
                }
                app.editor_save_file(id, then_close, false, window, cx);
            });
        })
        .detach();
    }

    /// Save As: the native panel for a file on this machine, the path bar for
    /// one on a machine the panel cannot browse.
    fn editor_save_as(
        &mut self,
        id: BufferId,
        then_close: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(f) = self.editor.buffers.get(id) else {
            return;
        };
        let suggested = if f.untitled.is_some() {
            PathBuf::from(format!("{}.txt", f.label()))
        } else {
            PathBuf::from(f.label().to_string())
        };
        let dir = match f.path.parent() {
            Some(dir) if f.untitled.is_none() => dir.to_path_buf(),
            _ => self
                .tab_code()
                .and_then(|c| c.roots.first().cloned())
                .unwrap_or_default(),
        };
        if !f.host.id().is_local() {
            let start = if dir.as_os_str().is_empty() {
                suggested
            } else {
                f.host.join(&dir, &suggested.to_string_lossy())
            };
            self.editor_open_bar(
                BarKind::SaveAs { id, then_close },
                start.display().to_string(),
                window,
                cx,
            );
            return;
        }
        let dir = if dir.as_os_str().is_empty() {
            dirs_home()
        } else {
            dir
        };
        let rx = cx.prompt_for_new_path(&dir, suggested.to_str());
        cx.spawn_in(window, async move |app, cx| {
            let Ok(Ok(Some(path))) = rx.await else {
                // Cancelled: a close that was waiting for this save must not
                // go ahead on some later, unrelated one.
                let _ = app.update(cx, |app, _cx| app.editor_saves_failed(id));
                return;
            };
            let _ = app.update_in(cx, |app, window, cx| {
                app.editor_save_to(id, path, then_close, window, cx);
            });
        })
        .detach();
    }

    /// Writes a buffer to a new path and points it there from then on.
    fn editor_save_to(
        &mut self,
        id: BufferId,
        path: PathBuf,
        then_close: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(f) = self.editor.buffers.get(id) else {
            return;
        };
        let host = f.host.clone();
        if let Some(other) = self.editor.buffers.at(host.id(), &path)
            && other != id
        {
            window.notify_app(
                t_fmt(
                    L10nKey::EditorAlreadyOpen,
                    &[("path", &path.display().to_string())],
                ),
                cx,
            );
            return;
        }
        if let Some(f) = self.editor.buffers.get_mut(id) {
            f.path = path.clone();
            f.untitled = None;
            f.conflict = None;
            // A new file keeps the format the buffer had; its own
            // `.editorconfig` is read the next time it opens.
            let language = language_for_path(&path);
            f.input
                .update(cx, |st, cx| st.set_highlighter(language, cx));
        }
        self.editor_rebuild_watcher(cx);
        // Written as a save to a file already known to be the one intended:
        // the native panel has asked about replacing an existing file, and
        // the path bar asks before it gets here.
        self.editor_save_file(id, then_close, true, window, cx);
    }

    fn editor_open_bar(
        &mut self,
        kind: BarKind,
        initial: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let placeholder = match kind {
            BarKind::GoToLine => {
                let total = self
                    .active_buffer()
                    .map(|f| f.input.read(cx).text().lines_len())
                    .unwrap_or(1);
                t_fmt(
                    L10nKey::EditorGoToLinePlaceholder,
                    &[("total", &total.to_string())],
                )
            }
            BarKind::SaveAs { .. } => t(L10nKey::EditorSaveAsPlaceholder).to_string(),
            BarKind::Rename { .. } => t_fmt(L10nKey::LspRenamePlaceholder, &[("name", &initial)]),
        };
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(placeholder)
                .default_value(initial)
        });
        let sub = cx.subscribe_in(
            &input,
            window,
            |this: &mut LyrelApp, _input, ev, window, cx| {
                match ev {
                    InputEvent::PressEnter { .. } => this.editor_submit_bar(window, cx),
                    // Clicking away from a go-to-line box is how you say never
                    // mind. A Save As is left up: it may be half-typed.
                    InputEvent::Blur => {
                        if matches!(
                            this.editor.bar.as_ref().map(|b| &b.kind),
                            Some(BarKind::GoToLine | BarKind::Rename { .. })
                        ) {
                            this.editor.bar = None;
                            cx.notify();
                        }
                    }
                    _ => {}
                }
            },
        );
        input.update(cx, |st, cx| st.focus(window, cx));
        self.editor.bar = Some(EditorBar {
            kind,
            input,
            _sub: sub,
        });
        cx.notify();
    }

    pub(crate) fn editor_go_to_line(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.code_panel_visible() || self.active_buffer().is_none() {
            return;
        }
        if let Some(f) = self.tab_code().and_then(TabCode::active_id)
            && let Some(f) = self.editor.buffers.get_mut(f)
        {
            // There is no line to land on in the rendered view.
            f.preview = false;
        }
        self.editor_open_bar(BarKind::GoToLine, String::new(), window, cx);
    }

    fn editor_close_bar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(BarKind::SaveAs { id, .. }) = self.editor.bar.as_ref().map(|b| &b.kind) {
            let id = *id;
            self.editor_saves_failed(id);
        }
        self.editor.bar = None;
        self.focus_editor(window, cx);
        cx.notify();
    }

    fn editor_submit_bar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(bar) = self.editor.bar.as_ref() else {
            return;
        };
        let text = bar.input.read(cx).value().to_string();
        match bar.kind {
            BarKind::GoToLine => {
                let Some((line, column)) = parse_line_target(&text) else {
                    return;
                };
                self.editor_close_bar(window, cx);
                let Some(f) = self.active_buffer() else {
                    return;
                };
                let input = f.input.clone();
                let lines = input.read(cx).text().lines_len().max(1) as u32;
                let position = Position {
                    line: line.min(lines) - 1,
                    character: column - 1,
                };
                place_cursor(input, position, CURSOR_SCROLL_ATTEMPTS, window, cx);
            }
            BarKind::Rename { id, offset } => {
                let name = text.trim().to_string();
                if name.is_empty() {
                    return;
                }
                self.editor_close_bar(window, cx);
                self.lsp_rename(id, offset, name, window, cx);
            }
            BarKind::SaveAs { id, then_close } => {
                let path = PathBuf::from(text.trim());
                if path.as_os_str().is_empty() {
                    return;
                }
                let Some(host) = self.editor.buffers.get(id).map(|f| f.host.clone()) else {
                    return;
                };
                if !host.is_absolute(&path) {
                    return;
                }
                self.editor.bar = None;
                cx.notify();
                let check = path.clone();
                HostOps::run_in(
                    host,
                    window,
                    cx,
                    move |h| h.exists(&check),
                    move |app, exists: bool, window, cx| {
                        if !exists {
                            app.editor_save_to(id, path, then_close, window, cx);
                            return;
                        }
                        let answer = window.prompt(
                            PromptLevel::Warning,
                            &t_fmt(
                                L10nKey::EditorReplaceExisting,
                                &[("path", &path.display().to_string())],
                            ),
                            None,
                            &crate::ui::confirm_answers(
                                t(L10nKey::EditorReplace),
                                t(L10nKey::Cancel),
                            ),
                            cx,
                        );
                        cx.spawn_in(window, async move |app, cx| {
                            if !matches!(answer.await, Ok(0)) {
                                return;
                            }
                            let _ = app.update_in(cx, |app, window, cx| {
                                app.editor_save_to(id, path, then_close, window, cx);
                            });
                        })
                        .detach();
                    },
                );
            }
        }
    }

    /// Closes the file at `pos` in the active tab's strip. Asks first only
    /// when this is the last tab showing a buffer with unsaved changes.
    pub(crate) fn editor_close_file(
        &mut self,
        pos: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let tab_ix = self.tabs.active_index();
        let Some(id) = self.tab_code().and_then(|c| c.files.get(pos).copied()) else {
            return;
        };
        let Some(f) = self.editor.buffers.get(id) else {
            return;
        };
        if !f.dirty || self.buffer_refs(id) > 1 {
            self.editor_remove_from_tab(tab_ix, id, cx);
            return;
        }
        let name = f.label();
        let answer = window.prompt(
            PromptLevel::Warning,
            &t_fmt(L10nKey::EditorUnsavedChanges, &[("name", &name)]),
            None,
            // Cancel sits between Save and Discard on purpose. The platform
            // renders the first button as the default and lays the rest out
            // beside it, so Discard was landing directly next to the key that
            // Return presses. Apple separates them for exactly this reason.
            // Three answers, so the shared helper does not fit: Save keeps
            // index 0 (rightmost, Return), Cancel takes Escape, and Discard
            // sits on the far left where nothing lands by reflex.
            &[
                gpui_kit::PromptButton::ok(t(L10nKey::Save)),
                gpui_kit::PromptButton::cancel(t(L10nKey::Cancel)),
                gpui_kit::PromptButton::ok(t(L10nKey::EditorDiscard)),
            ],
            cx,
        );
        cx.spawn_in(window, async move |app, cx| {
            let Ok(choice) = answer.await else { return };
            let _ = app.update_in(cx, |app, window, cx| match choice {
                0 => app.editor_save_file(id, true, false, window, cx),
                2 => app.editor_drop_buffer(id, cx),
                _ => {}
            });
        })
        .detach();
    }

    /// Closes the file with this buffer in the front tab's strip, wherever
    /// it has moved to since a menu was built for it.
    fn editor_close_buffer(&mut self, id: BufferId, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(pos) = self
            .tab_code()
            .and_then(|c| c.files.iter().position(|f| *f == id))
        {
            self.editor_close_file(pos, window, cx);
        }
    }

    /// Closes several files in the front tab's strip, asking once about the
    /// ones whose edits would be lost with them — not those another tab still
    /// shows.
    fn editor_close_buffers(
        &mut self,
        ids: Vec<BufferId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let tab_ix = self.tabs.active_index();
        let Some(tab) = self.tabs.get(tab_ix).map(|t| t.id()) else {
            return;
        };
        let unsaved: Vec<BufferId> = ids
            .iter()
            .copied()
            .filter(|id| {
                self.editor.buffers.get(*id).is_some_and(|f| f.dirty) && self.buffer_refs(*id) == 1
            })
            .collect();
        if self.editor_guard_unsaved(
            unsaved,
            AfterUnsaved::CloseFiles(tab, ids.clone()),
            window,
            cx,
        ) {
            return;
        }
        for id in ids {
            self.editor_remove_from_tab(tab_ix, id, cx);
        }
    }

    /// Types `@path` into the running agent's prompt, with the selected
    /// lines after it when there are any.
    fn editor_attach_to_agent(&mut self, id: BufferId, cx: &mut Context<Self>) {
        let Some(f) = self.editor.buffers.get(id).filter(|f| f.untitled.is_none()) else {
            return;
        };
        let state = f.input.read(cx);
        let suffix = match selected_lines(state.text(), state.selected_range()) {
            Some((a, b)) if a == b => format!("#L{a}"),
            Some((a, b)) => format!("#L{a}-{b}"),
            None => String::new(),
        };
        let path = f.path.clone();
        self.attach_path_to_agent(&path, &suffix, cx);
    }

    /// Right-click in the text. Edits dispatch to the editor itself, so each
    /// row shows the chord that does the same thing.
    fn editor_body_menu(
        menu: PopupMenu,
        app: &gpui_kit::WeakEntity<Self>,
        id: BufferId,
        cx: &gpui_kit::App,
    ) -> PopupMenu {
        use gpui_kit::component::input as edit;
        let Some(this) = app.upgrade() else {
            return menu;
        };
        let this = this.read(cx);
        let Some(f) = this.editor.buffers.get(id) else {
            return menu;
        };
        let state = f.input.read(cx);
        let selected = !state.selected_range().is_empty();
        let mut menu = menu.min_w(px(220.)).action_context(state.focus_handle(cx));
        if f.untitled.is_none() {
            menu = menu
                .item(
                    PopupMenuItem::new(t(L10nKey::FileTreeContextAttachAgent)).on_click({
                        let app = app.clone();
                        move |_, _window, cx| {
                            let _ = app.update(cx, |this, cx| this.editor_attach_to_agent(id, cx));
                        }
                    }),
                )
                .separator();
        }
        let menu = menu
            .menu(t(L10nKey::AppMenuUndo), Box::new(edit::Undo))
            .menu(t(L10nKey::AppMenuRedo), Box::new(edit::Redo))
            .separator()
            .menu_with_disabled(t(L10nKey::AppMenuCut), Box::new(edit::Cut), !selected)
            .menu_with_disabled(t(L10nKey::AppMenuCopy), Box::new(edit::Copy), !selected)
            .menu_with_disabled(
                t(L10nKey::AppMenuPaste),
                Box::new(edit::Paste),
                cx.read_from_clipboard().is_none(),
            )
            .menu(t(L10nKey::AppMenuSelectAll), Box::new(edit::SelectAll))
            .separator()
            // The line commands act on every line a cursor or selection
            // touches. Their chords live on gpui-component's `CodeEditor` key
            // context and are re-added in `keymap::fixed_bindings`, so they
            // beat the app's own keys while the editor has focus.
            .menu(
                t(L10nKey::EditorToggleComment),
                Box::new(commands::ToggleLineComment),
            )
            .menu(t(L10nKey::EditorMoveLineUp), Box::new(commands::MoveLineUp))
            .menu(
                t(L10nKey::EditorMoveLineDown),
                Box::new(commands::MoveLineDown),
            )
            .menu(
                t(L10nKey::EditorDuplicateLine),
                Box::new(commands::CopyLineDown),
            )
            .menu(t(L10nKey::EditorDeleteLine), Box::new(commands::DeleteLine))
            .separator()
            .menu(t(L10nKey::AppMenuFind), Box::new(edit::Search))
            .menu(
                t(L10nKey::EditorGoToLineAction),
                Box::new(crate::core::actions::EditorGoToLine),
            )
            .menu_with_disabled(
                t(L10nKey::EditorGitRevertChange),
                Box::new(crate::core::actions::EditorRevertChange),
                !this.editor_gutter_can_revert(id, cx),
            )
            .menu(
                t(L10nKey::EditorGoToMatchingBracket),
                Box::new(commands::MoveToMatchingBracket),
            );
        let menu = this.lsp_menu_items(menu, id, cx);
        this.editor_file_menu_items(menu, id, app, cx)
    }

    /// Right-click on a file in the header's strip.
    fn editor_tab_menu(
        menu: PopupMenu,
        app: &gpui_kit::WeakEntity<Self>,
        pos: usize,
        cx: &gpui_kit::App,
    ) -> PopupMenu {
        let Some(this) = app.upgrade() else {
            return menu;
        };
        let this = this.read(cx);
        let Some(files) = this.tab_code().map(|c| c.files.clone()) else {
            return menu;
        };
        let Some(&id) = files.get(pos) else {
            return menu;
        };
        let others: Vec<BufferId> = files.iter().copied().filter(|f| *f != id).collect();
        let right = files[pos + 1..].to_vec();
        let close = |label: L10nKey, ids: Vec<BufferId>| {
            let app = app.clone();
            PopupMenuItem::new(t(label))
                .disabled(ids.is_empty())
                .on_click(move |_, window, cx| {
                    let ids = ids.clone();
                    let _ = app.update(cx, |this, cx| this.editor_close_buffers(ids, window, cx));
                })
        };
        let menu = menu
            .min_w(px(220.))
            .item(
                PopupMenuItem::new(t(L10nKey::TabContextCloseTab)).on_click({
                    let app = app.clone();
                    move |_, window, cx| {
                        let _ = app.update(cx, |this, cx| this.editor_close_buffer(id, window, cx));
                    }
                }),
            )
            .item(close(L10nKey::AppMenuCloseOtherTabs, others))
            .item(close(L10nKey::AppMenuCloseTabsRight, right));
        this.editor_file_menu_items(menu, id, app, cx)
    }

    /// What both menus offer for the file as a whole: open it outside lyrel,
    /// show it in its folder, copy where it is. Nothing for a file that has
    /// never been saved, since it is not anywhere yet.
    fn editor_file_menu_items(
        &self,
        menu: PopupMenu,
        id: BufferId,
        app: &gpui_kit::WeakEntity<Self>,
        cx: &gpui_kit::App,
    ) -> PopupMenu {
        let Some(f) = self.editor.buffers.get(id).filter(|f| f.untitled.is_none()) else {
            return menu;
        };
        let path = f.path.clone();
        // Only this machine's desktop can open or show a file, and only a
        // file that is on this machine — the same rule as the file tree.
        let local = f.host.id().is_local();
        let mut menu = menu.separator();
        if local && self.can_spawn_locally(cx) && !is_program(&path) {
            let browser = opens_in_browser(&path);
            let label = match browser {
                true => t(L10nKey::PanelOpenInBrowser),
                false => t(L10nKey::AppMenuOpenLinkWithDefaultApp),
            };
            let opener: fn(&Path) -> std::io::Result<()> = match browser {
                true => open_in_browser,
                false => crate::terminal::view::open_file_path,
            };
            menu = menu.item(PopupMenuItem::new(label).on_click({
                let app = app.clone();
                let path = path.clone();
                move |_, window, cx| {
                    let _ = app.update(cx, |_, cx| Self::open_with(&path, opener, window, cx));
                }
            }));
        }
        if local {
            menu = menu.item(
                PopupMenuItem::new(crate::ui::right_panel::reveal_label()).on_click({
                    let path = path.clone();
                    move |_, _window, cx| {
                        cx.reveal_path(&crate::ui::path_display::native_separators(&path));
                    }
                }),
            );
        }
        let copy = |label: L10nKey, text: String| {
            PopupMenuItem::new(t(label)).on_click(move |_, _window, cx| {
                cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(text.clone()));
            })
        };
        // A remote host's paths are already spelled its own way; only one on
        // this machine is re-spelled with its separators.
        let full = match local {
            true => crate::ui::path_display::native_separators(&path)
                .display()
                .to_string(),
            false => path.display().to_string(),
        };
        menu = menu.item(copy(L10nKey::FileTreeContextCopyPath, full));
        if let Some(rel) = self.path_under_tree_root(&path) {
            let rel = match local {
                true => crate::ui::path_display::native_separators(&rel)
                    .display()
                    .to_string(),
                false => rel.display().to_string(),
            };
            menu = menu.item(copy(L10nKey::EditorCopyRelativePath, rel));
        }
        menu
    }

    pub(crate) fn editor_close_active_if_focused(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let empty = self
            .tab_code()
            .is_some_and(|c| c.visible && c.files.is_empty());
        // ⌘⇧E on an empty panel hands the focus to the file tree, so from
        // there ⌘W is the empty panel's too.
        let tree = empty && self.file_tree.focus_handle.contains_focused(window, cx);
        if !(tree || self.editor_panel_has_focus(window, cx)) {
            return false;
        }
        let Some(code) = self.tab_code_mut() else {
            return false;
        };
        if code.files.is_empty() {
            code.visible = false;
            self.editor.had_focus = None;
            self.editor.bar = None;
            self.file_tree.editing = None;
            self.focus_active(window, cx);
            cx.notify();
            return true;
        }
        let active = code.active;
        self.editor_close_file(active, window, cx);
        true
    }

    /// Takes a buffer out of one tab's strip, and drops it once no tab shows
    /// it any more.
    fn editor_remove_from_tab(&mut self, tab_ix: usize, id: BufferId, cx: &mut Context<Self>) {
        if let Some(code) = self
            .tabs
            .get_mut(tab_ix)
            .and_then(|t| t.code.as_deref_mut())
        {
            code.close_in_focused(id);
        }
        if self.buffer_refs(id) == 0 {
            self.editor_drop_buffer(id, cx);
        }
        cx.notify();
    }

    /// Forgets a buffer everywhere, edits and all.
    fn editor_drop_buffer(&mut self, id: BufferId, cx: &mut Context<Self>) {
        // Whatever was waiting on this buffer to be saved is not going to
        // see that happen now.
        self.editor_saves_failed(id);
        for code in self.tabs.iter_mut().filter_map(|t| t.code.as_deref_mut()) {
            code.forget_everywhere(id);
        }
        self.editor.buffers.remove(id);
        self.editor.nav.forget_buffer(id);
        if matches!(
            self.editor.bar.as_ref().map(|b| &b.kind),
            Some(BarKind::SaveAs { id: bar_id, .. }) if *bar_id == id
        ) {
            self.editor.bar = None;
        }
        self.editor_rebuild_watcher(cx);
        cx.notify();
    }

    // ---- Unsaved changes standing in the way of a close ----

    /// Asks about unsaved buffers before `then` goes ahead. Returns `true`
    /// when there was something to ask about: the caller stops there, and the
    /// answer carries the close on — after saving, after discarding, or not
    /// at all.
    pub(crate) fn editor_guard_unsaved(
        &mut self,
        ids: Vec<BufferId>,
        then: AfterUnsaved,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if ids.is_empty() {
            return false;
        }
        if self.editor.unsaved_prompt_open {
            return true;
        }
        self.editor.unsaved_prompt_open = true;
        let names: Vec<SharedString> = ids
            .iter()
            .filter_map(|id| self.editor.buffers.get(*id).map(OpenFile::label))
            .collect();
        let title = match names.as_slice() {
            [one] => t_fmt(L10nKey::EditorUnsavedChanges, &[("name", one)]),
            _ => t_fmt(
                L10nKey::EditorUnsavedChangesMany,
                &[("count", &names.len().to_string())],
            ),
        };
        let mut body: Vec<String> = names.iter().take(8).map(|n| n.to_string()).collect();
        if names.len() > 8 {
            body.push("…".into());
        }
        let body = (names.len() > 1).then(|| body.join("\n"));
        let answer = window.prompt(
            PromptLevel::Warning,
            &title,
            body.as_deref(),
            // The same arrangement as closing a single file, and for the same
            // reason: Discard as far from Return as the dialog allows.
            &[
                gpui_kit::PromptButton::ok(if names.len() > 1 {
                    t(L10nKey::EditorSaveAll)
                } else {
                    t(L10nKey::Save)
                }),
                gpui_kit::PromptButton::cancel(t(L10nKey::Cancel)),
                gpui_kit::PromptButton::ok(t(L10nKey::EditorDiscard)),
            ],
            cx,
        );
        // Showing it is part of the question: the file being asked about may
        // be behind the terminal, in a tab that is not the one in front.
        if let Some(tab_ix) = ids.first().and_then(|id| {
            self.tabs
                .iter()
                .position(|t| t.code.as_deref().is_some_and(|c| c.shows(*id)))
        }) && tab_ix == self.tabs.active_index()
            && let Some(code) = self.tab_code_mut()
            && let Some(pos) = {
                if !code.files.contains(&ids[0]) {
                    code.swap_focus();
                }
                code.files.iter().position(|f| *f == ids[0])
            }
        {
            code.active = pos;
            code.visible = true;
        }
        cx.spawn_in(window, async move |app, cx| {
            let choice = answer.await;
            let _ = app.update_in(cx, |app, window, cx| {
                app.editor.unsaved_prompt_open = false;
                match choice {
                    Ok(0) => {
                        app.editor.waiters.push(SaveWaiter {
                            ids: ids.clone(),
                            then,
                        });
                        for id in ids {
                            app.editor_save_file(id, false, false, window, cx);
                        }
                    }
                    Ok(2) => {
                        for id in ids {
                            app.editor_drop_buffer(id, cx);
                        }
                        app.editor_continue(then, window, cx);
                    }
                    _ => cx.notify(),
                }
            });
        })
        .detach();
        true
    }

    /// Runs whatever close was waiting on saves that have now all landed.
    fn editor_saves_landed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (ready, waiting): (Vec<SaveWaiter>, Vec<SaveWaiter>) =
            std::mem::take(&mut self.editor.waiters)
                .into_iter()
                .partition(|w| {
                    w.ids.iter().all(|id| {
                        self.editor
                            .buffers
                            .get(*id)
                            .is_none_or(|f| !f.dirty && !f.saving && f.untitled.is_none())
                    })
                });
        self.editor.waiters = waiting;
        for w in ready {
            self.editor_continue(w.then, window, cx);
        }
    }

    /// A save that was part of a close did not happen, so neither does the
    /// close: whatever it would have thrown away is still unsaved.
    fn editor_saves_failed(&mut self, id: BufferId) {
        self.editor.waiters.retain(|w| !w.ids.contains(&id));
    }

    fn editor_continue(&mut self, then: AfterUnsaved, window: &mut Window, cx: &mut Context<Self>) {
        match then {
            AfterUnsaved::CloseTab(tab) => {
                if let Some(ix) = self.tab_index_of(tab) {
                    self.close_tab(ix, window, cx);
                }
            }
            AfterUnsaved::CloseFiles(tab, ids) => {
                if let Some(ix) = self.tab_index_of(tab) {
                    for id in ids {
                        self.editor_remove_from_tab(ix, id, cx);
                    }
                }
            }
            AfterUnsaved::ClosePane(pane) => self.close_pane_after_unsaved(pane, window, cx),
            AfterUnsaved::CloseWindow => self.close_window_after_unsaved(window, cx),
            AfterUnsaved::Quit => self.quit_after_unsaved(window, cx),
        }
    }

    // ---- The file tree moving files out from under their buffers ----

    /// A file or folder was renamed through lyrel: buffers at or under the old
    /// path follow it. Without this the buffer kept the old name, and the next
    /// save wrote a second copy there.
    pub(crate) fn editor_path_moved(
        &mut self,
        host: HostId,
        from: &Path,
        to: &Path,
        cx: &mut Context<Self>,
    ) {
        let moved = self.editor.buffers.path_moved(host, from, to, cx);
        if moved {
            self.editor_rebuild_watcher(cx);
            cx.notify();
        }
    }

    /// A file or folder was deleted through lyrel: its buffers say so rather
    /// than waiting for the watcher, which a remote host may not have.
    pub(crate) fn editor_path_removed(
        &mut self,
        host: HostId,
        path: &Path,
        cx: &mut Context<Self>,
    ) {
        self.editor.buffers.path_removed(host, path);
        cx.notify();
    }

    // ---- Changes made by something else ----

    pub(crate) fn editor_reload_from_disk(
        &mut self,
        id: BufferId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.editor.buffers.reload(
            id,
            window,
            cx,
            |app| &mut app.editor.buffers,
            EditorPanelState::buffer_edited,
        );
    }

    // ---- Keeping the registry, the tabs and the session store in step ----

    /// Once per frame: buffers no tab shows any more are dropped — or, if
    /// they hold unsaved work, handed to the tab in front rather than lost —
    /// the active tab gets back the files it had open last time, and any
    /// tab whose files changed is recorded for next time.
    ///
    /// Done here rather than at each place a tab can go — a close, a shell
    /// that exits, a tab dragged into another, a workspace switch, a server
    /// restart — because this sees all of them, including ones added later.
    pub(crate) fn editor_sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.editor.buffers.take_watch_refresh() {
            self.editor_rebuild_watcher(cx);
        }
        // A tab that left and came back under the same id — a server restart
        // rebuilds every tab that way — gets its files back like a relaunch.
        if self.editor.restored.len() > self.tabs.len() {
            let live: HashSet<TabId> = self.tabs.iter().map(|t| t.id()).collect();
            self.editor.restored.retain(|id| live.contains(id));
        }
        self.editor_adopt_orphans(window, cx);
        self.editor_restore_active(window, cx);
        self.editor_record_sessions(cx);
    }

    fn editor_adopt_orphans(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // No tabs at all is the gap in the middle of a server restart or a
        // workspace switch, not a verdict on the buffers: wait for the tabs.
        if self.editor.buffers.is_empty() || self.tabs.is_empty() {
            return;
        }
        let shown: HashSet<BufferId> = self
            .tabs
            .iter()
            .filter_map(|t| t.code.as_deref())
            .flat_map(TabCode::all_files)
            .collect();
        let orphans: Vec<(BufferId, bool)> = self
            .editor
            .buffers
            .iter()
            .filter(|b| !shown.contains(&b.id()))
            .map(|b| (b.id(), b.dirty))
            .collect();
        if orphans.is_empty() {
            return;
        }
        let mut changed = false;
        for (id, dirty) in orphans {
            if !dirty {
                self.editor.buffers.remove(id);
                self.editor.nav.forget_buffer(id);
                changed = true;
                continue;
            }
            // Nowhere to put it (every tab gone mid-restart): keep it, and it
            // is adopted on a later frame.
            let Some(tab) = self.tabs.get(self.tabs.active_index()).map(|t| t.id()) else {
                continue;
            };
            let name = self
                .editor
                .buffers
                .get(id)
                .map(OpenFile::label)
                .unwrap_or_default();
            self.editor_show_in_tab(tab, id, false, window, cx);
            window.notify_app(t_fmt(L10nKey::EditorOrphanAdopted, &[("name", &name)]), cx);
            changed = true;
        }
        if changed {
            self.editor_rebuild_watcher(cx);
            cx.notify();
        }
    }

    fn editor_restore_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.get(self.tabs.active_index()) else {
            return;
        };
        let tab_id = tab.id();
        if !self.editor.restored.insert(tab_id) {
            return;
        }
        if tab.code.as_deref().is_some_and(|c| !c.files.is_empty()) {
            return;
        }
        let Some(state) = editor_session::get(cx, tab_id) else {
            return;
        };
        // The left group can be empty while the right is not: it held only
        // files that are not recorded (untitled, remote).
        if state.files.is_empty() && state.split.as_ref().is_none_or(|s| s.files.is_empty()) {
            return;
        }
        let Some(host) = self.active_host(cx) else {
            return;
        };
        // What was recorded is what is being put back; recording it again
        // before the files have loaded would write down an empty tab.
        self.editor.recorded.insert(tab_id, state.clone());
        // Both groups' files in one trip, the left group's first.
        let files: Vec<PathBuf> = state
            .files
            .iter()
            .chain(state.split.iter().flat_map(|s| s.files.iter()))
            .cloned()
            .collect();
        let requested = files.clone();
        let target = buffers::OpenTarget {
            workspace: self.workspace,
            tab: tab_id,
            window_host: self.spawn_host(cx),
        };
        self.editor.buffers.restore(
            host.clone(),
            files,
            target,
            window,
            cx,
            |app| &mut app.editor.buffers,
            Self::editor_open_target_valid,
            move |app, loaded: Vec<Option<Loaded>>, window, cx| {
                let mut opened: Vec<(PathBuf, BufferId)> = Vec::new();
                for (path, l) in requested.into_iter().zip(loaded) {
                    if let Some(l) = l {
                        let id = app.editor_install(host.clone(), l, tab_id, false, window, cx);
                        opened.push((path, id));
                    }
                }
                let group = |paths: &[PathBuf], active: usize| -> (Vec<BufferId>, usize) {
                    let front = paths.get(active);
                    let mut ids: Vec<BufferId> = Vec::new();
                    let mut at = 0;
                    for path in paths {
                        let Some(&(_, id)) = opened.iter().find(|(p, _)| p == path) else {
                            continue;
                        };
                        if ids.contains(&id) {
                            continue;
                        }
                        if Some(path) == front {
                            at = ids.len();
                        }
                        ids.push(id);
                    }
                    (ids, at)
                };
                let (left, left_active) = group(&state.files, state.active);
                let right = state
                    .split
                    .as_ref()
                    .map(|s| (group(&s.files, s.active), s.focused));
                let Some(tab_ix) = app.tab_index_of(tab_id) else {
                    return;
                };
                let Some(code) = app.tabs[tab_ix].code.as_deref_mut() else {
                    return;
                };
                let restored: Vec<BufferId> = opened.iter().map(|(_, id)| *id).collect();
                // Merged into what the tab has now, not written over it: a
                // file opened, or a split made, while these were loading stays.
                let untouched = code.restore_groups(
                    &restored,
                    (left, left_active),
                    right.map(|((files, active), focused)| (files, active, focused)),
                );
                let visible = state.visible && !code.files.is_empty();
                code.visible |= visible;
                // Only a restore nobody has touched takes the keyboard; one the
                // reader has moved on from leaves it where they put it.
                if untouched && tab_ix == app.tabs.active_index() && visible {
                    app.focus_editor(window, cx);
                }
                cx.notify();
            },
        );
    }

    fn editor_record_sessions(&mut self, cx: &mut Context<Self>) {
        let spawn_host = self.spawn_host(cx);
        let mut changed: Vec<(TabId, TabEditor)> = Vec::new();
        for tab in &self.tabs {
            let Some(code) = tab.code.as_deref() else {
                continue;
            };
            let tab_id = tab.id();
            // A tab still waiting for its files to come back has nothing to
            // say about them yet.
            if !self.editor.restored.contains(&tab_id) {
                continue;
            }
            // Only files on the window's own machine: an SFTP buffer's host
            // is a connection that will not exist next launch.
            let group = |ids: &[BufferId], front: usize| -> (Vec<PathBuf>, usize) {
                let mut files = Vec::new();
                let mut active = 0;
                for (pos, id) in ids.iter().enumerate() {
                    let Some(f) = self.editor.buffers.get(*id) else {
                        continue;
                    };
                    if f.untitled.is_some() || f.host.id() != spawn_host {
                        continue;
                    }
                    if pos == front {
                        active = files.len();
                    }
                    files.push(f.path.clone());
                }
                (files, active)
            };
            let sides = code.sides();
            let (files, active) = group(&sides[0].files, sides[0].active);
            let split = sides.get(1).and_then(|right| {
                let (files, active) = group(&right.files, right.active);
                (!files.is_empty()).then_some(editor_session::SplitEditor {
                    files,
                    active,
                    focused: right.focused,
                })
            });
            let state = TabEditor {
                files,
                active,
                visible: code.visible,
                split,
            };
            if self.editor.recorded.get(&tab_id) != Some(&state) {
                changed.push((tab_id, state));
            }
        }
        for (tab_id, state) in changed {
            self.editor.recorded.insert(tab_id, state.clone());
            editor_session::put(cx, tab_id, state);
        }
    }

    /// A tab is being closed: the files only it showed go with it. Unsaved
    /// ones were asked about before the close got here — except when a shell
    /// exiting took the tab, which cannot ask; those stay, and are handed to
    /// the tab in front.
    pub(crate) fn editor_close_tab_files(&mut self, tab_ix: usize, cx: &mut Context<Self>) {
        let Some(ids) = self
            .tabs
            .get(tab_ix)
            .and_then(|t| t.code.as_deref())
            .map(TabCode::all_files)
        else {
            return;
        };
        for id in ids {
            if self.buffer_tabs(id) == 1 && self.editor.buffers.get(id).is_some_and(|b| !b.dirty) {
                self.editor_drop_buffer(id, cx);
            }
        }
    }

    /// A tab was closed for good: forget what it had open.
    pub(crate) fn editor_forget_tab(&mut self, tab: TabId, cx: &mut Context<Self>) {
        self.editor.recorded.remove(&tab);
        self.editor.buffers.forget_tab(tab);
        self.editor.nav.forget_tab(tab);
        editor_session::remove(cx, tab);
    }
}

fn dirs_home() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

impl LyrelApp {
    pub(crate) fn render_code_overlay(
        &mut self,
        chrome: DocumentChrome,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if !self.code_panel_visible() {
            return None;
        }
        // The last file closed with the focus in it: keep the focus in the
        // panel, or gpui reports it lost and it falls back to the terminal.
        let tab = self.tabs.get(self.tabs.active_index()).map(|t| t.id());
        if tab.is_some() && self.editor.had_focus == tab && self.active_buffer().is_none() {
            self.editor.empty_focus.focus(window, cx);
        }
        self.editor.had_focus = tab.filter(|_| self.editor_panel_has_focus(window, cx));
        self.editor_split_follow_focus(window, cx);
        self.editor_gutter_sync(cx);
        self.editor_nav_tick(cx);
        if let Some(code) = self.tab_code_mut() {
            code.note_front();
        }
        let breadcrumbs = self.render_editor_breadcrumbs(window, cx);
        let body = self.render_editor_body(
            self.tab_code().and_then(TabCode::active_id),
            self.tab_code().map_or(0, TabCode::focused_slot),
            cx,
        );
        let conflict_banner = self
            .active_buffer()
            .and_then(|f| f.conflict.map(|c| (f.id(), c)))
            .map(|(id, c)| self.render_editor_conflict_banner(id, c, cx));
        let bar = self.render_editor_bar(cx);

        let header = self.render_editor_header(chrome, window, cx);
        let editor_col = v_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .child(header)
            .when_some(conflict_banner, |this, b| this.child(b))
            .children(bar)
            .child(self.render_editor_groups(breadcrumbs, body, window, cx))
            .children(self.render_editor_problems(window, cx));

        // The panel's own paint is the same either way; only the box is not.
        // Filling the workspace means stopping the window's translucency and
        // repainting the theme image the root's copy now sits under; docking
        // means sitting in the same plane as the right panel, which the column
        // wrapper has already painted.
        let shell = v_flex().id("code-panel");
        let shell = match chrome {
            DocumentChrome::Fill => shell
                .absolute()
                .inset_0()
                .occlude()
                // Opaque on purpose: this overlay covers the whole workspace
                // (everything but the detail panel) and an open file must never
                // let the window translucency / backdrop material show through
                // it. The preset's gradient fill is preserved, just with
                // alpha 1 — the same paint the settings overlay uses. The
                // theme background image is repainted on top of it, since the
                // root's copy now sits below this fill.
                .bg(crate::ui::theme::overlay_background(cx))
                .children(crate::ui::app::overlay_surface_layers(cx)),
            DocumentChrome::Dock => shell.size_full().min_w_0(),
        };
        let shell = self.editor_nav_actions(shell, cx);
        let shell = self.editor_split_command_sync(shell, cx);
        Some(
            shell
                .track_focus(&self.editor.empty_focus)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, window, cx| {
                        if !this.editor_panel_has_focus(window, cx) {
                            this.editor.empty_focus.focus(window, cx);
                        }
                        if let Some(tab) = this.tabs.get(this.tabs.active_index()) {
                            this.status_document_focus = Some((tab.id(), true));
                        }
                        cx.notify();
                    }),
                )
                .on_key_down(
                    cx.listener(|this, ev: &gpui_kit::KeyDownEvent, window, cx| {
                        if ev.keystroke.key != "escape" {
                            return;
                        }
                        if this.editor_gutter_close_peek(cx) {
                            cx.stop_propagation();
                            return;
                        }
                        // Escape in the go-to-line or Save As box dismisses the
                        // box, not the whole editor.
                        if this.editor.bar.is_some() {
                            this.editor_close_bar(window, cx);
                            cx.stop_propagation();
                            return;
                        }
                        this.toggle_code_panel(window, cx);
                    }),
                )
                .child(h_flex().flex_1().min_h_0().w_full().child(editor_col))
                .into_any_element(),
        )
    }

    /// One group's text: the file `id`, rendered or as source, or the empty
    /// panel. `slot` tells the two groups' elements apart.
    pub(crate) fn render_editor_body(
        &self,
        id: Option<BufferId>,
        slot: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match id.and_then(|id| self.editor.buffers.get(id)) {
            None => self.render_editor_empty(cx).into_any_element(),
            Some(f) if f.preview => {
                let markdown = f.input.read(cx).text().to_string();
                let scroll = f.preview_scroll.clone();
                // The bar's wrapper takes its height from `flex_1`, so it needs
                // a column with a definite height to grow inside — hand it one
                // rather than dropping it straight into the overlay, or the
                // pane sizes to its content and there is nothing left to
                // scroll.
                v_flex()
                    .size_full()
                    .child(crate::ui::scrollbar::with_vertical_scrollbar(
                        ("editor-md-preview-scrollbar", slot),
                        div()
                            .id(("editor-md-preview", slot))
                            .size_full()
                            .overflow_y_scroll()
                            .track_scroll(&scroll)
                            .px_4()
                            .py_3()
                            .child(
                                gpui_kit::component::text::TextView::markdown(
                                    ("editor-md-preview-body", slot),
                                    markdown,
                                )
                                .style(crate::ui::theme::markdown_style(cx)),
                            ),
                        &scroll,
                    ))
                    .into_any_element()
            }
            Some(f) => {
                let input = f.input.clone();
                let id = f.id();
                let app = cx.entity().downgrade();
                div()
                    .id(("editor-body", slot))
                    .size_full()
                    .map(|root| crate::ui::lsp::popups::EditorPopups::decorate(root, &f.popups))
                    .child(
                        Editor::new(&input)
                            .appearance(false)
                            .font_family(cx.theme().mono_font_family.clone())
                            .text_size(cx.theme().mono_font_size)
                            .size_full(),
                    )
                    .children(self.render_editor_gutter(id, cx))
                    .children(self.render_editor_gutter_peek(id, cx))
                    .child(f.popups.clone())
                    .context_menu(move |menu, _window, cx| {
                        Self::editor_body_menu(menu, &app, id, cx)
                    })
                    .into_any_element()
            }
        }
    }

    fn render_editor_header(
        &self,
        chrome: DocumentChrome,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let row = h_flex().id("editor-header");
        // v6 chrome: no rule under the bar — the header, the breadcrumbs and
        // the text are one plane — and only as many file cells as the column
        // has room for, the rest behind a `+N` list (see `strip`).
        let (tile, glyph) = (
            crate::ui::tab_strip::RAIL_TILE,
            crate::ui::tab_strip::RAIL_TILE_GLYPH,
        );
        let cap = self.editor_strip_cap(chrome, cx);
        let (shown, hidden) = self
            .tab_code()
            .map(|c| c.strip_split(cap))
            .unwrap_or_default();
        let active_id = self.tab_code().and_then(TabCode::active_id);
        let files: Vec<(usize, SharedString, String, bool, bool)> = shown
            .into_iter()
            .filter_map(|(pos, id)| {
                let f = self.editor.buffers.get(id)?;
                let tip = if f.untitled.is_some() {
                    f.label().to_string()
                } else {
                    f.path.display().to_string()
                };
                Some((pos, f.label(), tip, Some(id) == active_id, f.dirty))
            })
            .collect();
        let strip = if files.is_empty() {
            div()
                .min_w_0()
                .px(px(8.))
                .text_ellipsis()
                .text_size(gpui_kit::rems(strip::CELL_TEXT))
                .text_color(cx.theme().muted_foreground)
                .child(SharedString::from(t(L10nKey::EditorNoFileOpen)))
                .into_any_element()
        } else {
            h_flex()
                .id("editor-file-tabs")
                .min_w_0()
                .gap(px(2.))
                .overflow_hidden()
                .children(files.into_iter().map(|(pos, name, tip, active, dirty)| {
                    self.render_file_tab(pos, name, tip, active, dirty, cx)
                }))
                .into_any_element()
        };
        let back = self.render_document_terminal_button(chrome, window, cx);
        row.flex_none()
            // The border is spent on the box, not on the paint: this header
            // shows no rule, but its tiles sit in the band the traffic lights
            // hang over and have to keep the title bar's line.
            .title_strip_row(px(TITLE_BAR_BORDER), cx.theme().transparent)
            .gap(px(2.))
            // The whole reserve, not the reserve less the first control's own
            // inset. The filled editor leads with the back pill, whose 8pt of
            // inner padding is *inside* its hover box — carving it out of here
            // starts that box 8pt early and consumes part of the gap reserved
            // after the traffic lights, even when the lead follows their inset.
            // Docked, the row leads with the file tabs instead, and the same
            // subtraction put their first
            // glyph at 4pt rather than the `CONTENT_INSET` this bar is built on.
            .pl(px(crate::ui::window_chrome::CONTENT_INSET))
            // The glyph, not the tile, lands on `CONTENT_INSET`, the column
            // the file name starts on at the other end of the bar.
            .pr(px(
                crate::ui::window_chrome::CONTENT_INSET - (tile - glyph) / 2.
            ))
            .children(back)
            .child(strip)
            .children(self.render_strip_overflow(&hidden, cap, cx))
            .child(
                div().occlude().flex_none().child(
                    crate::ui::window_chrome::chrome_tile_sized(
                        Button::new("editor-new-file").icon(Icon::new(IconName::Plus)),
                        tile,
                        glyph,
                        false,
                        cx,
                    )
                    .rounded(px(crate::ui::tab_strip::RAIL_TILE_RADIUS))
                    .tooltip(t(L10nKey::EditorNewFile))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.editor_new_file(window, cx);
                    })),
                ),
            )
            // Whatever is left of the bar stays a place to drag the window by.
            .child(div().flex_1().h_full())
            .child(self.document_layout_switch(chrome, cx))
            .child(
                div().occlude().flex_none().ml(px(4.)).child(
                    crate::ui::window_chrome::chrome_tile_sized(
                        Button::new("editor-panel-close").icon(Icon::new(IconName::Close)),
                        tile,
                        glyph,
                        false,
                        cx,
                    )
                    .rounded(px(crate::ui::tab_strip::RAIL_TILE_RADIUS))
                    .tooltip(t(L10nKey::EditorBackToTerminal))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.toggle_code_panel(window, cx);
                    })),
                ),
            )
    }

    /// One file in the header's strip: its name, and a slot that shows the
    /// unsaved dot at rest and the close button under the pointer — the dot
    /// says there is something to lose before the × offers to lose it.
    fn render_file_tab(
        &self,
        pos: usize,
        name: SharedString,
        tip: String,
        active: bool,
        dirty: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let group: SharedString = format!("editor-file-tab-{pos}").into();
        let slot = 16.;
        let muted = cx.theme().muted_foreground;
        let close = div()
            .id(("editor-file-tab-close", pos))
            .role(gpui_kit::Role::Button)
            .aria_label(format!("{} {name}", t(L10nKey::Close)))
            .flex_none()
            .size(px(slot))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(4.))
            .hover(|s| s.bg(cx.theme().muted))
            .child(Icon::new(IconName::Close).xsmall().text_color(muted))
            .on_click(cx.listener(move |this, _, window, cx| {
                cx.stop_propagation();
                this.editor_close_file(pos, window, cx);
            }));
        // Unsaved is a state, not an alarm: the dot is in caption ink, the
        // same as the `+N` button's, not the warning hue.
        let slot_el = div()
            .flex_none()
            .size(px(slot))
            .flex()
            .items_center()
            .justify_center()
            .when(dirty, |d| {
                d.child(
                    div()
                        .group_hover(group.clone(), |s| s.invisible())
                        .size(px(6.))
                        .rounded_full()
                        .bg(muted),
                )
            });
        div()
            .id(("editor-file-tab", pos))
            .role(gpui_kit::Role::Tab)
            .aria_label(match dirty {
                true => format!("{name}, {}", t(L10nKey::SettingsUnsaved)),
                false => name.to_string(),
            })
            .aria_selected(active)
            .group(group.clone())
            .occlude()
            .flex_none()
            .min_w_0()
            .max_w(px(strip::CELL_MAX_W))
            .h(px(strip::CELL_H))
            .flex()
            .items_center()
            .gap(px(6.))
            .pl(px(10.))
            .pr(px(6.))
            .rounded(px(strip::CELL_RADIUS))
            .text_size(gpui_kit::rems(strip::CELL_TEXT))
            .map(|d| match active {
                true => d
                    .font_weight(gpui_kit::FontWeight::MEDIUM)
                    .text_color(cx.theme().foreground)
                    .bg(cx.theme().sidebar_accent),
                false => d.text_color(muted).hover(|s| {
                    s.bg(gpui_kit::rgb(
                        cx.global::<crate::ui::presets::Surfaces>().sidebar.hover,
                    ))
                }),
            })
            .child(
                div()
                    .min_w_0()
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .text_ellipsis()
                    .child(name),
            )
            .child(
                div()
                    .relative()
                    .flex_none()
                    .size(px(slot))
                    .child(div().absolute().inset_0().child(slot_el))
                    .child(
                        div()
                            .absolute()
                            .inset_0()
                            .when(!active, |d| d.invisible())
                            .when(dirty, |d| d.invisible())
                            .group_hover(group, |s| s.visible())
                            .child(close),
                    ),
            )
            .tooltip(move |window, cx| {
                gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx)
            })
            .on_click(cx.listener(move |this, _, window, cx| {
                this.editor_activate(pos, window, cx);
            }))
            .on_mouse_down(
                MouseButton::Middle,
                cx.listener(move |this, _, window, cx| {
                    this.editor_close_file(pos, window, cx);
                }),
            )
            .context_menu({
                let app = cx.entity().downgrade();
                move |menu, _window, cx| Self::editor_tab_menu(menu, &app, pos, cx)
            })
            .into_any_element()
    }

    fn render_editor_bar(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let bar = self.editor.bar.as_ref()?;
        let label = match bar.kind {
            BarKind::GoToLine => t(L10nKey::EditorGoToLine),
            BarKind::SaveAs { .. } => t(L10nKey::EditorSaveAs),
            BarKind::Rename { .. } => t(L10nKey::LspRenameSymbol),
        };
        Some(
            h_flex()
                .flex_none()
                .w_full()
                .items_center()
                .gap_2()
                .px(px(crate::ui::window_chrome::CONTENT_INSET))
                .py_1()
                .border_b_1()
                .border_color(cx.theme().sidebar_border)
                .text_sm()
                .child(
                    div()
                        .flex_none()
                        .text_color(cx.theme().muted_foreground)
                        .child(label),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(Input::new(&bar.input).small()),
                )
                .into_any_element(),
        )
    }

    pub(crate) fn render_document_status(&self, cx: &mut Context<Self>) -> AnyElement {
        let active = self.active_buffer();
        let cursor: Option<SharedString> = active.map(|f| {
            let input = f.input.read(cx);
            let pos = input.cursor_position();
            let ln_col = t_fmt(
                L10nKey::EditorLnCol,
                &[
                    ("line", &(pos.line + 1).to_string()),
                    ("column", &(pos.character + 1).to_string()),
                ],
            );
            ln_col.into()
        });
        let wrap: Option<bool> = active.map(|f| f.wrap);
        let is_markdown = active.is_some_and(|f| f.language() == "markdown");
        let preview = active.is_some_and(|f| f.preview);
        let indent: Option<SharedString> = active.map(|f| {
            let key = if f.indent.hard_tabs {
                L10nKey::EditorIndentTabs
            } else {
                L10nKey::EditorIndentSpaces
            };
            t_fmt(key, &[("n", &f.indent.size.to_string())]).into()
        });
        let line_ending: Option<&'static str> = active.map(|f| f.format.line_ending.label());
        let encoding: Option<SharedString> = active.map(|f| f.format.encoding_label().into());
        let language: Option<&'static str> = active.and_then(|f| language_label(f.language()));
        let lsp = self.render_lsp_status(cx).map(|(element, _)| element);

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
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.toggle_editor_problems(cx);
                        this.focus_editor(window, cx);
                    })),
            )
            .when(is_markdown, |this| {
                let label = if preview {
                    t(L10nKey::EditorEdit)
                } else {
                    t(L10nKey::EditorPreview)
                };
                this.child(
                    item("status-md-preview", label.into()).on_click(cx.listener(
                        |this, _, window, cx| {
                            this.toggle_document_preview(cx);
                            this.focus_editor(window, cx);
                        },
                    )),
                )
            })
            .when_some(wrap, |this, wrap| {
                let label = if wrap {
                    t(L10nKey::EditorWrapOn)
                } else {
                    t(L10nKey::EditorWrapOff)
                };
                this.child(item("status-wrap", label.into()).on_click(cx.listener(
                    |this, _, window, cx| {
                        this.toggle_document_wrap(window, cx);
                        this.focus_editor(window, cx);
                    },
                )))
            })
            // Tabular figures, so the position does not jitter sideways as
            // the caret walks from line 9 to line 10. A click asks for a line
            // to go to.
            .when_some(cursor, |this, t| {
                this.child(
                    item("status-cursor", t)
                        .font_features(crate::ui::theme::tabular_figures())
                        .on_click(
                            cx.listener(|this, _, window, cx| this.editor_go_to_line(window, cx)),
                        ),
                )
            })
            .when_some(indent, |this, t| this.child(div().flex_none().child(t)))
            .when_some(encoding, |this, t| this.child(div().flex_none().child(t)))
            .when_some(line_ending, |this, eol| {
                this.child(
                    item("status-eol", eol.into())
                        .tooltip(t(L10nKey::EditorToggleLineEnding))
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.toggle_line_ending(cx);
                            this.focus_editor(window, cx);
                        })),
                )
            })
            .when_some(language, |this, l| this.child(div().flex_none().child(l)))
            .into_any_element()
    }

    fn render_editor_empty(&self, cx: &Context<Self>) -> gpui_kit::Div {
        v_flex()
            .on_mouse_down(
                gpui_kit::MouseButton::Left,
                cx.listener(|this, _, window, cx| this.editor.empty_focus.focus(window, cx)),
            )
            .size_full()
            .items_center()
            .justify_center()
            .gap_2()
            .child(
                Icon::new(IconName::File)
                    .large()
                    .text_color(cx.theme().muted_foreground),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(crate::ui::i18n::t(
                        crate::ui::i18n::L10nKey::OpenFileFromTree,
                    )),
            )
    }

    fn render_editor_conflict_banner(
        &self,
        id: BufferId,
        conflict: DiskConflict,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        use crate::ui::dialog::{self, Tone};
        let theme = cx.theme();
        let rungs = cx.global::<crate::ui::presets::Surfaces>().window;
        // The floating notices' grammar, laid flat: a neutral strip over a
        // hairline, and the state carried by one amber dot. A strip tinted
        // amber end to end, with a white outlined button from the component
        // library beside a bare-text one, was three visual languages in one
        // 32px row.
        //
        // The filled answer is always the safe one: Keep mine / Save hold on
        // to the unsaved edits, Reload / Close throw them away, so those are
        // offered, not pressed on the reader.
        let row = h_flex()
            .flex_none()
            .w_full()
            .items_center()
            .gap(px(8.))
            .pl(px(dialog::INSET))
            .pr(px(6.))
            .h(px(dialog::FOOTER_H))
            .border_b_1()
            .border_color(theme.border)
            .text_size(rems(crate::ui::right_panel::TAB_TEXT))
            .child(
                div()
                    .flex_none()
                    .size(px(6.))
                    .rounded_full()
                    .bg(theme.warning),
            );
        let message = |key| div().flex_1().min_w_0().truncate().child(t(key));
        match conflict {
            DiskConflict::Changed(observed) => row
                .child(message(L10nKey::FileChangedOnDisk))
                .child(dialog::button(
                    "editor-conflict-reload",
                    t(L10nKey::Reload),
                    Tone::Secondary,
                    true,
                    rungs,
                    cx,
                    cx.listener(move |this, _, window, cx| {
                        this.editor_reload_from_disk(id, window, cx);
                    }),
                ))
                .child(dialog::button(
                    "editor-conflict-keep",
                    t(L10nKey::KeepMine),
                    Tone::Primary,
                    true,
                    rungs,
                    cx,
                    cx.listener(move |this, _, _w, cx| {
                        if let Some(f) = this.editor.buffers.get_mut(id) {
                            // The version seen on disk is now the one a
                            // save is allowed to replace.
                            f.disk_mtime = observed;
                            f.conflict = None;
                            cx.notify();
                        }
                    }),
                ))
                .into_any_element(),
            DiskConflict::Deleted => row
                .child(message(L10nKey::EditorFileDeletedOnDisk))
                .child(dialog::button(
                    "editor-deleted-close",
                    t(L10nKey::Close),
                    Tone::Secondary,
                    true,
                    rungs,
                    cx,
                    cx.listener(move |this, _, _w, cx| {
                        this.editor_drop_buffer(id, cx);
                    }),
                ))
                .child(dialog::button(
                    "editor-deleted-save",
                    t(L10nKey::Save),
                    Tone::Primary,
                    true,
                    rungs,
                    cx,
                    cx.listener(move |this, _, window, cx| {
                        this.editor_save_file(id, false, true, window, cx);
                    }),
                ))
                .into_any_element(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A temp dir as the editor will see paths under it: through the local
    /// host's `canonicalize`, the call `load_file` makes. That resolves
    /// macOS's `/var` → `/private/var` and Windows' 8.3 short names
    /// (`RUNNER~1`), and never answers in Windows' `\\?\` form.
    pub(crate) fn test_real_dir(dir: &Path) -> PathBuf {
        lyrel_core::host::local::LocalHost::new()
            .canonicalize(dir)
            .unwrap()
    }

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
        let ids: Vec<BufferId> = (1..=4u64).map(gpui_kit::EntityId::from).collect();
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
        let ids: Vec<BufferId> = (1..=4u64).map(gpui_kit::EntityId::from).collect();
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

#[cfg(test)]
mod close_gpui_tests {
    use gpui_kit::test::TestWindowExt as _;
    use gpui_kit::{Entity, TestAppContext, VisualTestContext};

    use super::nav::gpui_tests::open_rs;
    use super::*;
    use crate::core::actions::CloseActiveTab;
    use crate::ui::app::test_window::harness_with_tabs;

    fn panel(app: &Entity<LyrelApp>, vcx: &mut VisualTestContext) -> (bool, usize) {
        app.read_with(vcx, |app, _| {
            let code = app.tab_code().expect("the tab is still there");
            (code.visible, code.files.len())
        })
    }

    /// Runs what is pending and paints a frame: the panel notes whether it
    /// holds the focus as it is drawn.
    fn settle(vcx: &mut VisualTestContext) {
        vcx.run_until_parked();
        vcx.update(|window, cx| window.render_frame(cx));
        vcx.run_until_parked();
    }

    fn terminal_focused(app: &Entity<LyrelApp>, vcx: &mut VisualTestContext) -> bool {
        app.update_in(vcx, |app, window, cx| {
            app.tabs[app.tabs.active_index()]
                .pane
                .leaves()
                .iter()
                .any(|l| l.contains_focused(window, cx))
        })
    }

    /// ⌘W, ⌘W on an editor with one file closes the file and then the empty
    /// editor. The second one used to close the terminal: closing the last
    /// file left nothing in the panel to hold the focus, so it fell back to
    /// the terminal pane.
    #[gpui_kit::test]
    fn closing_the_last_file_keeps_close_on_the_editor(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        open_rs(&app, &mut vcx, "/close/a.rs");
        settle(&mut vcx);
        assert_eq!(panel(&app, &mut vcx), (true, 1));

        vcx.dispatch_action(CloseActiveTab);
        settle(&mut vcx);
        assert_eq!(panel(&app, &mut vcx), (true, 0), "the file closes first");
        assert!(!terminal_focused(&app, &mut vcx));

        vcx.dispatch_action(CloseActiveTab);
        settle(&mut vcx);
        assert_eq!(panel(&app, &mut vcx), (false, 0), "then the empty editor");
        assert!(
            terminal_focused(&app, &mut vcx),
            "and the terminal has it back"
        );

        let tab = app.read_with(&vcx, |app, _| app.tabs[0].id());
        vcx.dispatch_action(CloseActiveTab);
        settle(&mut vcx);
        assert!(
            app.read_with(&vcx, |app, _| app.tabs.iter().all(|t| t.id() != tab)),
            "only now is ⌘W the terminal's"
        );
    }

    /// A tab switched to keeps its focus in its terminal, even with an empty
    /// editor showing and the focus in the editor of the tab left behind.
    #[gpui_kit::test]
    fn switching_to_an_empty_editor_leaves_the_focus_alone(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 2);
        crate::ui::i18n::set_locale("en");
        app.update_in(&mut vcx, |app, _, _| {
            app.tabs.select(1);
            app.tab_code_mut_or_init().unwrap().visible = true;
            app.tabs.select(0);
        });
        open_rs(&app, &mut vcx, "/close/a.rs");
        settle(&mut vcx);
        app.update_in(&mut vcx, |app, window, cx| {
            app.tabs.select(1);
            app.focus_active(window, cx);
        });
        settle(&mut vcx);
        assert_eq!(panel(&app, &mut vcx), (true, 0));
        assert!(terminal_focused(&app, &mut vcx));
    }

    /// With the focus in the terminal beside an empty editor, ⌘W is still
    /// the terminal's.
    #[gpui_kit::test]
    fn close_from_the_terminal_is_still_the_terminals(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        app.update_in(&mut vcx, |app, window, cx| {
            app.tab_code_mut_or_init().unwrap().visible = true;
            app.focus_active(window, cx);
        });
        vcx.run_until_parked();
        assert!(terminal_focused(&app, &mut vcx));
        assert!(!app.update_in(&mut vcx, |app, window, cx| {
            app.editor_close_active_if_focused(window, cx)
        }));
        assert_eq!(panel(&app, &mut vcx), (true, 0));
    }

    #[gpui_kit::test]
    fn document_dock_fill_and_close_keep_the_same_editor_and_terminal(cx: &mut TestAppContext) {
        use crate::core::actions::{
            ActivateTab1, ActivateTab2, EditorNewFile, ToggleCodePanel, ToggleDocumentFill,
        };
        use gpui_kit::size;

        let (app, mut vcx, _streams) = harness_with_tabs(cx, 2);
        vcx.simulate_resize(size(px(1600.), px(900.)));
        for collapsed in [false, true] {
            app.update_in(&mut vcx, |app, window, cx| {
                app.sidebar_collapsed = collapsed;
                app.right_panel_visible = true;
                app.focus_active(window, cx);
                cx.notify();
            });
            settle(&mut vcx);
            let pane = app.read_with(&vcx, |app, _| {
                app.tabs[0].pane.first_leaf().unwrap().entity_id()
            });
            vcx.dispatch_action(EditorNewFile);
            settle(&mut vcx);
            let editor = app.read_with(&vcx, |app, _| app.active_buffer().unwrap().input.clone());
            assert!(app.update_in(&mut vcx, |app, window, cx| {
                app.document_dock_px(window, cx).is_some()
            }));
            for docked in [false, true] {
                vcx.dispatch_action(ToggleDocumentFill);
                settle(&mut vcx);
                assert_eq!(
                    app.update_in(&mut vcx, |app, window, cx| {
                        app.document_dock_px(window, cx).is_some()
                    }),
                    docked,
                );
                app.read_with(&vcx, |app, _| {
                    assert_eq!(
                        app.active_buffer().unwrap().input.entity_id(),
                        editor.entity_id()
                    );
                    assert_eq!(app.tabs[0].pane.first_leaf().unwrap().entity_id(), pane);
                });
                vcx.update(|window, _| {
                    assert!(window.find("editor-panel-close").visible());
                    assert_eq!(window.find_all("editor-panel-close").len(), 1);
                });
            }
            vcx.update(|window, cx| window.click("editor-panel-close", cx));
            settle(&mut vcx);
            assert!(!app.read_with(&vcx, |app, _| app.code_panel_visible()));
            assert!(terminal_focused(&app, &mut vcx));
            vcx.dispatch_action(ToggleCodePanel);
            settle(&mut vcx);
            assert_eq!(
                app.read_with(&vcx, |app, _| app
                    .active_buffer()
                    .unwrap()
                    .input
                    .entity_id()),
                editor.entity_id()
            );
            vcx.dispatch_action(CloseActiveTab);
            settle(&mut vcx);
            assert!(app.read_with(&vcx, |app, _| app.code_panel_visible()));
            vcx.dispatch_action(CloseActiveTab);
            settle(&mut vcx);
            assert!(!app.read_with(&vcx, |app, _| app.code_panel_visible()));
            assert!(terminal_focused(&app, &mut vcx));
            assert_eq!(app.read_with(&vcx, |app, _| app.tabs.len()), 2);
            vcx.dispatch_action(ActivateTab2);
            settle(&mut vcx);
            assert_eq!(app.read_with(&vcx, |app, _| app.tabs.active_index()), 1);
            vcx.dispatch_action(ActivateTab1);
            settle(&mut vcx);
        }
    }

    #[gpui_kit::test]
    fn global_status_navigates_and_saves_the_displayed_document(cx: &mut TestAppContext) {
        use crate::core::actions::{EditorNewFile, EditorSaveAs, ToggleDocumentFill};
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        vcx.simulate_resize(gpui_kit::size(px(1600.), px(900.)));
        vcx.dispatch_action(EditorNewFile);
        settle(&mut vcx);
        vcx.update(|window, cx| window.input("one\ntwo\nthree", cx));
        settle(&mut vcx);
        vcx.update(|window, cx| window.click("status-cursor", cx));
        settle(&mut vcx);
        vcx.update(|window, cx| {
            window.press("secondary-a", cx);
            window.input("2:2", cx);
            window.press("enter", cx);
        });
        settle(&mut vcx);
        app.read_with(&vcx, |app, cx| {
            let pos = app
                .active_buffer()
                .unwrap()
                .input
                .read(cx)
                .cursor_position();
            assert_eq!((pos.line, pos.character), (1, 1));
        });
        vcx.update(|window, cx| window.click("status-cursor", cx));
        settle(&mut vcx);
        vcx.simulate_keystrokes("escape");
        settle(&mut vcx);
        assert!(app.update_in(&mut vcx, |app, window, cx| {
            app.code_panel_visible() && app.editor_has_focus(window, cx)
        }));
        for _ in 0..2 {
            vcx.dispatch_action(ToggleDocumentFill);
            settle(&mut vcx);
            vcx.update(|window, cx| {
                let cursor = window.find("status-cursor").bounds();
                let global = window.find("workbench-status").bounds();
                assert!(cursor.top() >= global.top() && cursor.bottom() <= global.bottom());
                assert_eq!(window.find_all("status-cursor").len(), 1);
                window.click("status-wrap", cx);
            });
            settle(&mut vcx);
            assert!(app.update_in(&mut vcx, |app, window, cx| app.editor_has_focus(window, cx)));
        }
        // The line-ending control changes the real save output, not just its label.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("status.txt");
        vcx.update(|window, cx| window.click("status-eol", cx));
        settle(&mut vcx);
        vcx.dispatch_action(EditorSaveAs);
        settle(&mut vcx);
        cx.simulate_new_path_selection(|_| Some(path.clone()));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !path.exists() || app.read_with(&vcx, |app, _| app.active_buffer().unwrap().saving) {
            assert!(
                std::time::Instant::now() < deadline,
                "the document save did not finish"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
            settle(&mut vcx);
        }
        assert_eq!(std::fs::read(&path).unwrap(), b"one\r\ntwo\r\nthree");
    }

    #[gpui_kit::test]
    fn docked_status_follows_terminal_focus_and_recovers_document_actions(cx: &mut TestAppContext) {
        use crate::core::actions::EditorNewFile;
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        vcx.simulate_resize(gpui_kit::size(px(1600.), px(900.)));
        vcx.dispatch_action(EditorNewFile);
        settle(&mut vcx);
        app.update_in(&mut vcx, |app, window, cx| {
            let handle = app.tabs[0].pane.first_leaf().unwrap().focus_handle(cx);
            window.focus(&handle, cx);
        });
        settle(&mut vcx);
        vcx.update(|window, _| assert!(window.try_find("status-cursor").is_none()));
        app.update_in(&mut vcx, |app, window, cx| app.focus_editor(window, cx));
        settle(&mut vcx);
        vcx.update(|window, cx| {
            assert!(window.find("status-cursor").visible());
            window.click("status-problems", cx);
        });
        settle(&mut vcx);
        vcx.update(|window, _| assert!(window.find("editor-problems").visible()));
        vcx.update(|window, cx| window.click("editor-panel-close", cx));
        settle(&mut vcx);
        assert!(terminal_focused(&app, &mut vcx));
        vcx.update(|window, _| assert!(window.try_find("status-cursor").is_none()));
    }

    #[gpui_kit::test]
    fn diff_status_uses_its_host_path_not_the_covered_document(cx: &mut TestAppContext) {
        use crate::core::actions::{EditorNewFile, ToggleCodePanel};
        use crate::terminal::git_diff::{DiffSnapshot, DiffSource};
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        vcx.simulate_resize(gpui_kit::size(px(1600.), px(900.)));
        vcx.dispatch_action(EditorNewFile);
        settle(&mut vcx);
        let remote = HostId::from_connection_key("status-remote-host");
        app.update_in(&mut vcx, |app, window, cx| {
            app.open_supplied_diff(
                remote,
                Arc::new(DiffSnapshot {
                    root: PathBuf::from("/srv/项目 with spaces/repository"),
                    source: DiffSource::Patch {
                        id: "status-context".into(),
                        label: None,
                    },
                    ..Default::default()
                }),
                None,
                window,
                cx,
            );
        });
        settle(&mut vcx);
        vcx.update(|window, cx| {
            assert!(window.try_find("status-cursor").is_none());
            window.click("status-path", cx);
            assert_eq!(
                cx.read_from_clipboard().unwrap().text().as_deref(),
                Some("/srv/项目 with spaces/repository")
            );
        });
        // Raising the still-open editor restores its own position controls.
        vcx.dispatch_action(ToggleCodePanel);
        settle(&mut vcx);
        vcx.update(|window, _| assert!(window.find("status-cursor").visible()));
    }

    #[gpui_kit::test]
    fn settings_escape_restores_the_current_document_or_docked_terminal(cx: &mut TestAppContext) {
        use crate::core::actions::{EditorNewFile, ToggleDocumentFill};
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        vcx.simulate_resize(gpui_kit::size(px(1600.), px(900.)));
        vcx.dispatch_action(EditorNewFile);
        settle(&mut vcx);
        // Both dock and fill must return typing to the document, not its shell.
        for text in ["docked", "filled"] {
            if text == "filled" {
                vcx.dispatch_action(ToggleDocumentFill);
                settle(&mut vcx);
            }
            vcx.update(|window, cx| window.click("status-settings", cx));
            settle(&mut vcx);
            assert!(app.read_with(&vcx, |app, _| app.settings.is_some()));
            vcx.simulate_keystrokes("escape");
            settle(&mut vcx);
            assert!(app.update_in(&mut vcx, |app, window, cx| app.editor_has_focus(window, cx)));
            vcx.update(|window, cx| window.input(text, cx));
            settle(&mut vcx);
        }
        assert_eq!(
            app.read_with(&vcx, |app, cx| app
                .active_buffer()
                .unwrap()
                .input
                .read(cx)
                .text()
                .to_string()),
            "dockedfilled"
        );
        vcx.dispatch_action(ToggleDocumentFill);
        app.update_in(&mut vcx, |app, window, cx| {
            let handle = app.tabs[0].pane.first_leaf().unwrap().focus_handle(cx);
            window.focus(&handle, cx);
        });
        settle(&mut vcx);
        vcx.update(|window, cx| window.click("status-settings", cx));
        settle(&mut vcx);
        vcx.simulate_keystrokes("escape");
        settle(&mut vcx);
        assert!(terminal_focused(&app, &mut vcx));
    }
}

#[cfg(test)]
mod pane_scrollback_tests {
    use gpui_kit::TestAppContext;

    #[gpui_kit::test]
    fn scrollback_action_opens_a_saveable_unicode_draft_without_changing_the_pane(
        cx: &mut TestAppContext,
    ) {
        let (app, mut vcx, _streams) = crate::ui::app::test_window::harness_with_tabs(cx, 1);
        let pane = app.update_in(&mut vcx, |app, window, cx| {
            let pane = app.tabs[0].pane.terminals()[0].clone();
            let terminal = pane.read(cx).terminal.term.clone();
            let output = format!(
                "\x1b[31m历史 e\u{301} 🎉\x1b[0m\r\nsecond line\r\n{}",
                "retained history\r\n".repeat(40),
            );
            rio_vt::performer::handler::Processor::default()
                .advance(&mut *terminal.lock(), output.as_bytes());
            let focus = pane.read(cx).focus_handle.clone();
            window.focus(&focus, cx);
            pane
        });
        vcx.dispatch_action(crate::core::actions::OpenScrollbackInEditor);
        vcx.run_until_parked();
        app.update_in(&mut vcx, |app, window, cx| {
            let file = app
                .active_buffer()
                .expect("scrollback opens a real editor buffer");
            let text = file.input.read(cx).text().to_string();
            assert!(
                text.starts_with("历史 e\u{301} 🎉\nsecond line"),
                "{text:?}"
            );
            assert!(!text.contains('\x1b'));
            assert!(file.untitled.is_some());
            assert!(
                file.dirty,
                "closing the draft must offer Save As instead of losing it"
            );
            assert!(app.editor_has_focus(window, cx));
            assert_eq!(app.tabs[0].pane.terminals()[0], pane);
        });
    }
}
