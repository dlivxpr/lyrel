//! Source-authoritative Markdown editing. Views retain only interaction state.
use crate::ui::editor_text::{self, EncodeError, LineEnding, TextFormat};
use gpui_kit::base::{
    UndoHistory,
    input::{Rope, RopeExt as _},
};
use std::ops::Range;
use unicode_segmentation::UnicodeSegmentation as _;

mod source;
mod projection;
pub(super) use source::SourcePresentation;
#[cfg(test)]
mod tests;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Selection {
    anchor: usize,
    head: usize,
}
impl Selection {
    pub(crate) fn new(anchor: usize, head: usize) -> Self {
        Self { anchor, head }
    }
    pub(crate) fn caret(at: usize) -> Self {
        Self::new(at, at)
    }
    pub(crate) fn range(self) -> Range<usize> {
        self.anchor.min(self.head)..self.anchor.max(self.head)
    }
    pub(crate) fn head(self) -> usize {
        self.head
    }
    pub(crate) fn is_reversed(self) -> bool {
        self.head < self.anchor
    }
}

pub(crate) struct PresentationState {
    id: u64,
    selection: Selection,
    composition: Option<Composition>,
}
impl Default for Selection {
    fn default() -> Self {
        Self::caret(0)
    }
}
impl PresentationState {
    pub(crate) fn new(selection: Selection) -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self {
            id: NEXT.fetch_add(1, Ordering::Relaxed),
            selection,
            composition: None,
        }
    }
    pub(crate) fn selection(&self) -> Selection {
        self.selection
    }
    pub(crate) fn marked_range(&self) -> Option<Range<usize>> {
        self.composition.as_ref().map(|c| c.range.clone())
    }
    pub(crate) fn rebase(&mut self, transaction: &Transaction) {
        if transaction.origin != self.id {
            self.selection = transaction.map_selection(self.selection);
        }
    }
}
impl Default for PresentationState {
    fn default() -> Self {
        Self::new(Selection::default())
    }
}

/// The source-position effects of one command, in application order. Other
/// presentations use this result to keep their own anchors on the same text.
pub(crate) struct Transaction {
    revision: u64,
    origin: u64,
    edits: smallvec::SmallVec<[SourceChange; 1]>,
}
struct SourceChange {
    range: Range<usize>,
    inserted_len: usize,
}
impl Transaction {
    pub(crate) fn revision(&self) -> u64 {
        self.revision
    }
    pub(crate) fn map_selection(&self, mut selection: Selection) -> Selection {
        for edit in &self.edits {
            let map = |offset: usize| {
                if offset <= edit.range.start {
                    offset
                } else if offset >= edit.range.end {
                    offset - edit.range.len() + edit.inserted_len
                } else {
                    edit.range.start + edit.inserted_len
                }
            };
            selection = Selection::new(map(selection.anchor), map(selection.head));
        }
        selection
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EditKind {
    Typing,
    Paste,
    Command,
}
pub(crate) enum Command {
    Reload {
        text: String,
        format: TextFormat,
    },
    Select(Selection),
    Insert(String),
    ParagraphBreak { hard: bool },
    Replace {
        range: Range<usize>,
        text: String,
        kind: EditKind,
    },
    /// Non-overlapping ranges in the original source coordinate system.
    BatchReplace {
        edits: Vec<(Range<usize>, String)>,
        kind: EditKind,
    },
    Backspace,
    Delete,
    Undo,
    Redo,
    Compose {
        range: Option<Range<usize>>,
        text: String,
        selected: Option<Range<usize>>,
    },
    Commit(String),
    FinishComposition,
    CancelComposition,
}

struct Composition {
    range: Range<usize>,
    original: SourceText,
    selection: Selection,
}
#[derive(Clone)]
struct Change {
    edits: smallvec::SmallVec<[ReplacedSource; 1]>,
    before: Selection,
    after: Selection,
}
#[derive(Clone)]
struct ReplacedSource {
    start: usize,
    removed: SourceText,
    inserted: SourceText,
}

/// LF is the editing coordinate system; each LF carries its own disk spelling.
/// Undo and preedit own only the affected representation, never a second source.
#[derive(Clone, PartialEq, Eq)]
struct SourceText {
    text: String,
    endings: Vec<LineEnding>,
}
impl SourceText {
    fn from_input(text: String, new_line: LineEnding) -> Self {
        let endings = text
            .match_indices('\n')
            .map(|(at, _)| {
                if at > 0 && text.as_bytes()[at - 1] == b'\r' {
                    LineEnding::CrLf
                } else {
                    new_line
                }
            })
            .collect();
        let text = if text.contains("\r\n") {
            text.replace("\r\n", "\n")
        } else {
            text
        };
        Self { text, endings }
    }
}

/// One retained source, monotonically increasing revision and shared reversible history.
/// Representation belongs here, not in a source or live rendering cache.
pub(crate) struct MarkdownDocument {
    source: Rope,
    revision: u64,
    format: TextFormat,
    endings: Vec<LineEnding>,
    history: UndoHistory<Change>,
    typing_end: Option<(u64, usize)>,
    // A weak input lease prevents another presentation editing through preedit.
    // The marked range, original text and selection stay in that presentation.
    composition_owner: Option<gpui_kit::WeakEntity<SourcePresentation>>,
    pending_changes: smallvec::SmallVec<[SourceChange; 1]>,
}
impl MarkdownDocument {
    pub(crate) fn new(source: String, format: TextFormat) -> Self {
        let source = SourceText::from_input(source, LineEnding::Lf);
        Self {
            source: Rope::from(source.text),
            endings: source.endings,
            revision: 0,
            format,
            history: UndoHistory::new(),
            typing_end: None,
            composition_owner: None,
            pending_changes: smallvec::SmallVec::new(),
        }
    }
    pub(crate) fn source(&self) -> &Rope {
        &self.source
    }
    pub(crate) fn revision(&self) -> u64 {
        self.revision
    }
    pub(crate) fn format(&self) -> TextFormat {
        self.format
    }
    pub(crate) fn set_format(&mut self, format: TextFormat) {
        if format.line_ending != self.format.line_ending {
            self.endings.fill(format.line_ending);
        }
        self.format = format;
    }
    pub(crate) fn representation_fingerprint(&self) -> u64 {
        use std::hash::{Hash as _, Hasher as _};
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        for ending in &self.endings {
            matches!(ending, LineEnding::CrLf).hash(&mut hash);
        }
        hash.finish()
    }
    pub(crate) fn encode(&self) -> Result<Vec<u8>, EncodeError> {
        let mut text = String::with_capacity(self.source.len() + self.endings.len());
        let mut endings = self.endings.iter();
        for chunk in self.source.chunks() {
            for part in chunk.split_inclusive('\n') {
                if let Some(line) = part.strip_suffix('\n') {
                    text.push_str(line);
                    if endings.next() == Some(&LineEnding::CrLf) {
                        text.push('\r');
                    }
                    text.push('\n');
                } else {
                    text.push_str(part);
                }
            }
        }
        editor_text::encode(
            &text,
            &TextFormat {
                line_ending: LineEnding::Lf,
                ..self.format
            },
        )
    }
    fn source_text(&self, range: Range<usize>) -> SourceText {
        let start = self.source.offset_to_point(range.start).row;
        let end = self.source.offset_to_point(range.end).row;
        SourceText {
            text: self.source.slice(range).to_string(),
            endings: self.endings[start..end].to_vec(),
        }
    }

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
            if offset == local {
                return at;
            }
            if offset > local {
                return start + if forward { offset } else { previous };
            }
            previous = offset;
        }
        start
            + if forward {
                line.len()
            } else if local == line.len() {
                local
            } else {
                previous
            }
    }
    pub(crate) fn previous(&self, at: usize) -> usize {
        if at == 0 {
            return 0;
        }
        self.boundary(at - 1, false)
    }
    pub(crate) fn next(&self, at: usize) -> usize {
        if at >= self.source.len() {
            return self.source.len();
        }
        self.boundary(at + 1, true)
    }
    pub(crate) fn safe_range(&self, range: Range<usize>) -> Range<usize> {
        self.boundary(range.start.min(range.end), false)
            ..self.boundary(range.end.max(range.start), true)
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
    fn replace(&mut self, range: Range<usize>, text: &SourceText) {
        let start = self.source.offset_to_point(range.start).row;
        let end = self.source.offset_to_point(range.end).row;
        self.endings
            .splice(start..end, text.endings.iter().copied());
        self.pending_changes.push(SourceChange {
            range: range.clone(),
            inserted_len: text.text.len(),
        });
        self.source.replace(range, &text.text);
        self.revision = self.revision.wrapping_add(1);
    }
    fn record(&mut self, change: Change, kind: EditKind, owner: u64) {
        let edit = &change.edits[0];
        let grouped = kind == EditKind::Typing
            && change.edits.len() == 1
            && self.typing_end == Some((owner, edit.start))
            && edit.removed.text.is_empty()
            && change.before.range().is_empty();
        if grouped {
            self.history.start_grouping();
        } else {
            self.history.end_grouping();
        }
        self.typing_end = (kind == EditKind::Typing
            && change.edits.len() == 1
            && !edit.inserted.text.contains(['\n', '\r']))
        .then_some((owner, edit.start + edit.inserted.text.len()));
        self.history.push(change);
        self.history.end_grouping();
    }
    fn edit(
        &mut self,
        state: &mut PresentationState,
        range: Range<usize>,
        text: String,
        kind: EditKind,
    ) {
        let range = self.safe_range(range);
        let removed = self.source_text(range.clone());
        let text = SourceText::from_input(text, self.format.line_ending);
        let before = state.selection;
        let end = range.start + text.text.len();
        if removed == text {
            state.selection = Selection::caret(self.boundary(end, true));
            self.typing_end = None;
            return;
        }
        self.replace(range.clone(), &text);
        state.selection = Selection::caret(self.boundary(end, true));
        self.record(
            Change {
                edits: smallvec::smallvec![ReplacedSource {
                    start: range.start,
                    removed,
                    inserted: text
                }],
                before,
                after: state.selection,
            },
            kind,
            state.id,
        );
    }
    fn batch_edit(
        &mut self,
        state: &mut PresentationState,
        mut edits: Vec<(Range<usize>, String)>,
        kind: EditKind,
    ) {
        for (range, _) in &mut edits {
            *range = self.safe_range(range.clone());
        }
        edits.sort_by_key(|(range, _)| std::cmp::Reverse(range.start));
        let before = state.selection;
        let mut changes = smallvec::SmallVec::new();
        for (range, text) in edits {
            let removed = self.source_text(range.clone());
            let inserted = SourceText::from_input(text, self.format.line_ending);
            if removed == inserted {
                continue;
            }
            self.replace(range.clone(), &inserted);
            state.selection =
                Selection::caret(self.boundary(range.start + inserted.text.len(), true));
            changes.push(ReplacedSource {
                start: range.start,
                removed,
                inserted,
            });
        }
        if !changes.is_empty() {
            self.record(
                Change {
                    edits: changes,
                    before,
                    after: state.selection,
                },
                kind,
                state.id,
            );
        }
    }
    fn finish_composition(&mut self, state: &mut PresentationState, cancel: bool) {
        let Some(composition) = state.composition.take() else {
            return;
        };
        self.typing_end = None;
        let inserted = self.source_text(composition.range.clone());
        if cancel {
            if inserted != composition.original {
                self.replace(composition.range, &composition.original);
            }
            state.selection = composition.selection;
        } else if inserted != composition.original {
            self.record(
                Change {
                    edits: smallvec::smallvec![ReplacedSource {
                        start: composition.range.start,
                        removed: composition.original,
                        inserted,
                    }],
                    before: composition.selection,
                    after: state.selection,
                },
                EditKind::Command,
                state.id,
            );
        }
    }
    /// All editing consumers use this command boundary. Native ranges are converted
    /// before dispatch; composition state is owned by the calling presentation.
    pub(crate) fn command(
        &mut self,
        state: &mut PresentationState,
        command: Command,
    ) -> Transaction {
        self.apply_command(state, command);
        Transaction {
            revision: self.revision,
            origin: state.id,
            edits: std::mem::take(&mut self.pending_changes),
        }
    }

    fn apply_command(&mut self, state: &mut PresentationState, command: Command) {
        match command {
            Command::Compose {
                range,
                text,
                selected,
            } => {
                self.typing_end = None;
                if text.is_empty() {
                    self.finish_composition(state, true);
                    return;
                }
                let range = range
                    .or_else(|| state.marked_range())
                    .unwrap_or_else(|| state.selection.range());
                let range = if state.marked_range().as_ref() == Some(&range) {
                    range
                } else {
                    self.safe_range(range)
                };
                // A platform replacement outside the current preedit ends that
                // composition rather than recording unrelated text as its inverse.
                if state.composition.as_ref().is_some_and(|c| c.range != range) {
                    self.finish_composition(state, false);
                }
                if state.composition.is_none() {
                    state.composition = Some(Composition {
                        range: range.clone(),
                        original: self.source_text(range.clone()),
                        selection: state.selection,
                    });
                }
                let text = SourceText::from_input(text, self.format.line_ending);
                self.replace(range.clone(), &text);
                state.composition.as_mut().unwrap().range =
                    range.start..range.start + text.text.len();
                let selected = selected.unwrap_or_else(|| {
                    let end = text.text.encode_utf16().count();
                    end..end
                });
                let start16 = self.utf16_offset(range.start);
                let bytes = self.utf16_range(start16 + selected.start..start16 + selected.end);
                state.selection = if bytes.is_empty() {
                    Selection::caret(self.boundary(bytes.end, true))
                } else {
                    Selection::new(
                        self.boundary(bytes.start, false),
                        self.boundary(bytes.end, true),
                    )
                };
            }
            Command::CancelComposition => self.finish_composition(state, true),
            Command::FinishComposition => self.finish_composition(state, false),
            Command::Commit(text) => {
                if let Some(range) = state.marked_range() {
                    let text = SourceText::from_input(text, self.format.line_ending);
                    self.replace(range.clone(), &text);
                    state.composition.as_mut().unwrap().range =
                        range.start..range.start + text.text.len();
                    state.selection =
                        Selection::caret(self.boundary(range.start + text.text.len(), true));
                    self.finish_composition(state, false);
                } else {
                    self.edit(state, state.selection.range(), text, EditKind::Typing);
                }
            }
            command => {
                self.finish_composition(state, false);
                match command {
                    Command::Reload { text, format } => {
                        let text = SourceText::from_input(text, LineEnding::Lf);
                        self.replace(0..self.source.len(), &text);
                        self.format = format;
                        self.history = UndoHistory::new();
                        self.typing_end = None;
                        state.selection =
                            Selection::caret(self.boundary(state.selection.head, false));
                    }
                    Command::Select(selection) => {
                        self.typing_end = None;
                        state.selection = Selection::new(
                            self.boundary(selection.anchor, false),
                            self.boundary(selection.head, false),
                        );
                    }
                    Command::Insert(text) => {
                        self.edit(state, state.selection.range(), text, EditKind::Typing)
                    }
                    Command::ParagraphBreak { hard } => {
                        self.edit(
                            state,
                            state.selection.range(),
                            if hard { "  \n" } else { "\n\n" }.into(),
                            EditKind::Command,
                        );
                    }
                    Command::Replace { range, text, kind } => self.edit(state, range, text, kind),
                    Command::BatchReplace { edits, kind } => self.batch_edit(state, edits, kind),
                    Command::Backspace | Command::Delete => {
                        let mut range = state.selection.range();
                        if range.is_empty() {
                            if matches!(command, Command::Backspace) {
                                range.start = self.previous(range.start);
                            } else {
                                range.end = self.next(range.end);
                            }
                        }
                        self.edit(state, range, String::new(), EditKind::Command);
                    }
                    Command::Undo | Command::Redo => {
                        self.typing_end = None;
                        let undo = matches!(command, Command::Undo);
                        let changes = if undo {
                            self.history.undo()
                        } else {
                            self.history.redo()
                        };
                        if let Some(changes) = changes {
                            for change in changes {
                                let mut edits = change.edits;
                                if undo {
                                    edits.reverse();
                                }
                                for edit in edits {
                                    let (old, new) = if undo {
                                        (&edit.inserted, &edit.removed)
                                    } else {
                                        (&edit.removed, &edit.inserted)
                                    };
                                    self.replace(edit.start..edit.start + old.text.len(), new);
                                }
                                state.selection = if undo { change.before } else { change.after };
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
pub(super) enum DocumentEvent {
    Edited(Transaction),
    PresentationChanged,
}
