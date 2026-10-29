//! Source-authoritative Markdown editing. Views retain only interaction state.
use std::ops::Range;
use gpui_kit::base::{input::{Rope, RopeExt as _}, UndoHistory};
use unicode_segmentation::UnicodeSegmentation as _;
use crate::ui::editor_text::TextFormat;

mod source;
pub(super) use source::SourcePresentation;
#[cfg(test)] mod tests;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Selection { anchor: usize, head: usize }
impl Selection {
    pub(crate) fn new(anchor: usize, head: usize) -> Self { Self { anchor, head } }
    pub(crate) fn caret(at: usize) -> Self { Self::new(at, at) }
    pub(crate) fn range(self) -> Range<usize> { self.anchor.min(self.head)..self.anchor.max(self.head) }
    pub(crate) fn head(self) -> usize { self.head }
    pub(crate) fn is_reversed(self) -> bool { self.head < self.anchor }
}

pub(crate) struct PresentationState {
    id: u64,
    selection: Selection,
    composition: Option<Composition>,
}
impl Default for Selection { fn default() -> Self { Self::caret(0) } }
impl PresentationState {
    pub(crate) fn new(selection: Selection) -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self { id: NEXT.fetch_add(1, Ordering::Relaxed), selection, composition: None }
    }
    pub(crate) fn selection(&self) -> Selection { self.selection }
    pub(crate) fn marked_range(&self) -> Option<Range<usize>> { self.composition.as_ref().map(|c| c.range.clone()) }
    pub(crate) fn rebase(&mut self, transaction: &Transaction) {
        if transaction.origin != self.id { self.selection = transaction.map_selection(self.selection); }
    }
}
impl Default for PresentationState { fn default() -> Self { Self::new(Selection::default()) } }

/// The source-position effects of one command, in application order. Other
/// presentations use this result to keep their own anchors on the same text.
pub(crate) struct Transaction {
    revision: u64,
    origin: u64,
    edits: smallvec::SmallVec<[SourceChange; 1]>,
}
struct SourceChange { range: Range<usize>, inserted_len: usize }
impl Transaction {
    pub(crate) fn revision(&self) -> u64 { self.revision }
    pub(crate) fn map_selection(&self, mut selection: Selection) -> Selection {
        for edit in &self.edits {
            let map = |offset: usize| {
                if offset <= edit.range.start { offset }
                else if offset >= edit.range.end { offset - edit.range.len() + edit.inserted_len }
                else { edit.range.start + edit.inserted_len }
            };
            selection = Selection::new(map(selection.anchor), map(selection.head));
        }
        selection
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EditKind { Typing, Paste, Command }
pub(crate) enum Command {
    Select(Selection),
    Insert(String),
    Replace { range: Range<usize>, text: String, kind: EditKind },
    Backspace,
    Delete,
    Undo,
    Redo,
    Compose { range: Option<Range<usize>>, text: String, selected: Option<Range<usize>> },
    Commit(String),
    FinishComposition,
    CancelComposition,
}

struct Composition { range: Range<usize>, original: String, selection: Selection }
#[derive(Clone)]
struct Change { start: usize, removed: String, inserted: String, before: Selection, after: Selection }

/// One retained source, monotonically increasing revision and shared reversible history.
/// Representation belongs here, not in a source or live rendering cache.
pub(crate) struct MarkdownDocument {
    source: Rope,
    revision: u64,
    format: TextFormat,
    history: UndoHistory<Change>,
    typing_end: Option<(u64, usize)>,
    // A weak input lease prevents another presentation editing through preedit.
    // The marked range, original text and selection stay in that presentation.
    composition_owner: Option<gpui_kit::WeakEntity<SourcePresentation>>,
    pending_changes: smallvec::SmallVec<[SourceChange; 1]>,
}
impl MarkdownDocument {
    pub(crate) fn new(source: String, format: TextFormat) -> Self {
        Self { source: Rope::from(source), revision: 0, format, history: UndoHistory::new(), typing_end: None, composition_owner: None, pending_changes: smallvec::SmallVec::new() }
    }
    pub(crate) fn source(&self) -> &Rope { &self.source }
    pub(crate) fn revision(&self) -> u64 { self.revision }
    pub(crate) fn format(&self) -> TextFormat { self.format }
    pub(crate) fn set_format(&mut self, format: TextFormat) { self.format = format; }

    // A line-local string supplies complete grapheme context even when the rope
    // splits a combining sequence between chunks. No full-document copy on input.
    pub(crate) fn boundary(&self, at: usize, forward: bool) -> usize {
        let at = at.min(self.source.len());
        let line_ix = self.source.offset_to_point(at).row;
        let start = self.source.line_start_offset(line_ix);
        let end = self.source.line_start_offset(line_ix + 1);
        let line = self.source.slice(start..end).to_string();
        let local = at - start;
        let mut previous = 0;
        for (offset, _) in line.grapheme_indices(true) {
            if offset == local { return at; }
            if offset > local { return start + if forward { offset } else { previous }; }
            previous = offset;
        }
        start + if forward { line.len() } else if local == line.len() { local } else { previous }
    }
    pub(crate) fn previous(&self, at: usize) -> usize {
        if at == 0 { return 0; }
        self.boundary(at - 1, false)
    }
    pub(crate) fn next(&self, at: usize) -> usize {
        if at >= self.source.len() { return self.source.len(); }
        self.boundary(at + 1, true)
    }
    pub(crate) fn safe_range(&self, range: Range<usize>) -> Range<usize> {
        self.boundary(range.start.min(range.end), false)..self.boundary(range.end.max(range.start), true)
    }
    pub(crate) fn utf16_offset(&self, byte: usize) -> usize {
        self.source.byte_to_utf16_idx(byte.min(self.source.len()))
    }
    fn byte_offset(&self, utf16: usize, forward: bool) -> usize {
        let utf16 = utf16.min(self.source.len_utf16());
        let mut byte = self.source.utf16_to_byte_idx(utf16);
        if forward && self.source.byte_to_utf16_idx(byte) < utf16 {
            byte += self.source.char_at(byte).map_or(0, char::len_utf8);
        }
        byte
    }
    pub(crate) fn utf16_range(&self, range: Range<usize>) -> Range<usize> {
        self.byte_offset(range.start, false)..self.byte_offset(range.end, true)
    }
    fn replace(&mut self, range: Range<usize>, text: &str) {
        self.pending_changes.push(SourceChange { range: range.clone(), inserted_len: text.len() });
        self.source.replace(range, text);
        self.revision = self.revision.wrapping_add(1);
    }
    fn record(&mut self, change: Change, kind: EditKind, owner: u64) {
        let grouped = kind == EditKind::Typing && self.typing_end == Some((owner, change.start))
            && change.removed.is_empty() && change.before.range().is_empty();
        if grouped { self.history.start_grouping(); } else { self.history.end_grouping(); }
        self.typing_end = (kind == EditKind::Typing && !change.inserted.contains(['\n', '\r']))
            .then_some((owner, change.start + change.inserted.len()));
        self.history.push(change);
        self.history.end_grouping();
    }
    fn edit(&mut self, state: &mut PresentationState, range: Range<usize>, text: String, kind: EditKind) {
        let range = self.safe_range(range);
        let removed = self.source.slice(range.clone()).to_string();
        let before = state.selection;
        let end = range.start + text.len();
        if removed == text {
            state.selection = Selection::caret(self.boundary(end, true));
            self.typing_end = None;
            return;
        }
        self.replace(range.clone(), &text);
        state.selection = Selection::caret(self.boundary(end, true));
        self.record(Change { start: range.start, removed, inserted: text, before, after: state.selection }, kind, state.id);
    }
    fn finish_composition(&mut self, state: &mut PresentationState, cancel: bool) {
        let Some(composition) = state.composition.take() else { return; };
        self.typing_end = None;
        let inserted = self.source.slice(composition.range.clone()).to_string();
        if cancel {
            if inserted != composition.original { self.replace(composition.range, &composition.original); }
            state.selection = composition.selection;
        } else if inserted != composition.original {
            self.record(Change { start: composition.range.start, removed: composition.original,
                inserted, before: composition.selection, after: state.selection }, EditKind::Command, state.id);
        }
    }
    /// All editing consumers use this command boundary. Native ranges are converted
    /// before dispatch; composition state is owned by the calling presentation.
    pub(crate) fn command(&mut self, state: &mut PresentationState, command: Command) -> Transaction {
        self.apply_command(state, command);
        Transaction { revision: self.revision, origin: state.id, edits: std::mem::take(&mut self.pending_changes) }
    }

    fn apply_command(&mut self, state: &mut PresentationState, command: Command) {
        match command {
            Command::Compose { range, text, selected } => {
                self.typing_end = None;
                if text.is_empty() { self.finish_composition(state, true); return; }
                let range = range.or_else(|| state.marked_range()).unwrap_or_else(|| state.selection.range());
                let range = if state.marked_range().as_ref() == Some(&range) { range } else { self.safe_range(range) };
                // A platform replacement outside the current preedit ends that
                // composition rather than recording unrelated text as its inverse.
                if state.composition.as_ref().is_some_and(|c| c.range != range) {
                    self.finish_composition(state, false);
                }
                if state.composition.is_none() {
                    state.composition = Some(Composition { range: range.clone(), original: self.source.slice(range.clone()).to_string(), selection: state.selection });
                }
                self.replace(range.clone(), &text);
                state.composition.as_mut().unwrap().range = range.start..range.start + text.len();
                let selected = selected.unwrap_or_else(|| { let end = text.encode_utf16().count(); end..end });
                let start16 = self.utf16_offset(range.start);
                let bytes = self.utf16_range(start16 + selected.start..start16 + selected.end);
                state.selection = if bytes.is_empty() { Selection::caret(self.boundary(bytes.end, true)) }
                    else { Selection::new(self.boundary(bytes.start, false), self.boundary(bytes.end, true)) };
            }
            Command::CancelComposition => self.finish_composition(state, true),
            Command::FinishComposition => self.finish_composition(state, false),
            Command::Commit(text) => {
                if let Some(range) = state.marked_range() {
                    self.replace(range.clone(), &text);
                    state.composition.as_mut().unwrap().range = range.start..range.start + text.len();
                    state.selection = Selection::caret(self.boundary(range.start + text.len(), true));
                    self.finish_composition(state, false);
                } else { self.edit(state, state.selection.range(), text, EditKind::Typing); }
            }
            command => {
                self.finish_composition(state, false);
                match command {
                    Command::Select(selection) => {
                        self.typing_end = None;
                        state.selection = Selection::new(self.boundary(selection.anchor, false), self.boundary(selection.head, false));
                    }
                    Command::Insert(text) => self.edit(state, state.selection.range(), text, EditKind::Typing),
                    Command::Replace { range, text, kind } => self.edit(state, range, text, kind),
                    Command::Backspace | Command::Delete => {
                        let mut range = state.selection.range();
                        if range.is_empty() {
                            if matches!(command, Command::Backspace) { range.start = self.previous(range.start); }
                            else { range.end = self.next(range.end); }
                        }
                        self.edit(state, range, String::new(), EditKind::Command);
                    }
                    Command::Undo | Command::Redo => {
                        self.typing_end = None;
                        let undo = matches!(command, Command::Undo);
                        let changes = if undo { self.history.undo() } else { self.history.redo() };
                        if let Some(changes) = changes {
                            for change in changes {
                                let (old, new, selection) = if undo { (&change.inserted, &change.removed, change.before) }
                                    else { (&change.removed, &change.inserted, change.after) };
                                self.replace(change.start..change.start + old.len(), new);
                                state.selection = selection;
                            }
                        }
                    }
                    _ => unreachable!(),
                }
            }
        }
    }
}
impl gpui_kit::EventEmitter<DocumentEvent> for MarkdownDocument {}
pub(super) enum DocumentEvent { Edited(Transaction), PresentationChanged }
