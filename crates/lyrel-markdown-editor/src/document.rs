//! 共享文档是唯一可编辑权威；presentation 只持有不可变 revision 快照。
use lyrel_text::{EncodeError, LineEnding, TextFormat};
use gpui_kit::{Context, EntityId, EventEmitter, WeakEntity};
use std::{ops::Range, sync::Arc};

#[derive(Clone)]
pub struct SourceSnapshot {
    text: Arc<String>,
    endings: Arc<Vec<LineEnding>>,
    format: TextFormat,
}
impl SourceSnapshot {
    pub fn text(&self) -> &str { &self.text }
    pub(crate) fn text_handle(&self) -> Arc<String> { self.text.clone() }
    pub fn shared_text(&self) -> Arc<String> { self.text.clone() }
    pub fn format(&self) -> TextFormat { self.format }
    pub fn encode(&self) -> Result<Vec<u8>, EncodeError> {
        let mut represented = String::with_capacity(self.text.len() + self.endings.len());
        let mut endings = self.endings.iter();
        for part in self.text.split_inclusive('\n') {
            if let Some(line) = part.strip_suffix('\n') {
                represented.push_str(line);
                if endings.next() == Some(&LineEnding::CrLf) { represented.push('\r'); }
                represented.push('\n');
            } else { represented.push_str(part); }
        }
        lyrel_text::encode(&represented, &TextFormat { line_ending: LineEnding::Lf, ..self.format })
    }
    pub fn representation_fingerprint(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        for ending in self.endings.iter() { matches!(ending, LineEnding::CrLf).hash(&mut hash); }
        hash.finish()
    }
}

/// 所有范围使用前一个 edit 已应用后的 LF / UTF-8 坐标。
#[derive(Clone, Debug, PartialEq)]
pub struct SourceEdit {
    range: Range<usize>,
    removed: Arc<str>,
    inserted: Arc<str>,
}
impl SourceEdit {
    pub fn range(&self) -> Range<usize> { self.range.clone() }
    pub fn inserted(&self) -> &str { &self.inserted }
    pub fn removed(&self) -> &str { &self.removed }
    pub fn map_offset(&self, at: usize) -> usize {
        if at <= self.range.start { at }
        else if at >= self.range.end { at - self.range.len() + self.inserted.len() }
        else { self.range.start + self.inserted.len() }
    }
    fn inverse(&self) -> Self {
        Self { range: self.range.start..self.range.start + self.inserted.len(), removed: self.inserted.clone(), inserted: self.removed.clone() }
    }
}

#[derive(Clone)]
pub struct DocumentEvent {
    revision: u64,
    origin: Option<EntityId>,
    edits: Arc<[SourceEdit]>,
    snapshot: Arc<SourceSnapshot>,
}
impl DocumentEvent {
    pub fn revision(&self) -> u64 { self.revision }
    pub fn origin(&self) -> Option<EntityId> { self.origin }
    pub fn edits(&self) -> &[SourceEdit] { &self.edits }
    pub fn snapshot(&self) -> &Arc<SourceSnapshot> { &self.snapshot }
}

struct HistoryEntry {
    before: Arc<SourceSnapshot>,
    after: Arc<SourceSnapshot>,
    before_caret: usize,
    after_caret: usize,
    selection: Option<(Range<usize>, Range<usize>)>,
    edits: Vec<SourceEdit>,
    origin: EntityId,
}
struct Composition {
    owner: WeakEntity<crate::EditorState>,
    before: Arc<SourceSnapshot>,
    before_caret: usize,
    range: Range<usize>,
    edits: Vec<SourceEdit>,
}

/// 文档身份、源文、逐行行尾、编码策略、history 与 revision 共同存活。
/// 所有修改发出同一种有序事件；读取和 presentation 切换不产生修改。
pub struct Document {
    current: Arc<SourceSnapshot>,
    revision: u64,
    undo: Vec<HistoryEntry>,
    redo: Vec<HistoryEntry>,
    composition: Option<Composition>,
}
impl EventEmitter<DocumentEvent> for Document {}
impl Document {
    pub fn new(text: String, format: TextFormat) -> Self {
        let (text, endings) = normalize_input(&text, LineEnding::Lf);
        Self { current: Arc::new(SourceSnapshot { text: Arc::new(text), endings: Arc::new(endings), format }), revision: 0, undo: Vec::new(), redo: Vec::new(), composition: None }
    }
    pub fn source(&self) -> &str { self.current.text() }
    pub fn snapshot(&self) -> Arc<SourceSnapshot> { self.current.clone() }
    pub fn revision(&self) -> u64 { self.revision }
    pub fn format(&self) -> TextFormat { self.current.format }
    pub fn encode(&self) -> Result<Vec<u8>, EncodeError> { self.current.encode() }
    pub fn representation_fingerprint(&self) -> u64 { self.current.representation_fingerprint() }
    pub fn has_composition(&self) -> bool { self.composition.is_some() }
    pub(crate) fn composition_owner(&self) -> Option<WeakEntity<crate::EditorState>> { self.composition.as_ref().map(|c| c.owner.clone()) }
    pub(crate) fn finish_orphaned_composition(&mut self, cx: &mut Context<Self>) -> Arc<SourceSnapshot> {
        if let Some(composition) = self.composition.as_ref().filter(|composition| composition.owner.upgrade().is_none()) {
            let origin = composition.owner.entity_id();
            let caret = composition.range.end;
            self.finish_composition(origin, false, caret, cx);
            self.publish(Some(origin), Vec::new(), cx);
        }
        self.current.clone()
    }
    pub(crate) fn set_format(&mut self, format: TextFormat, origin: EntityId, caret: usize, cx: &mut Context<Self>) {
        if self.current.format == format { return; }
        let before = self.current.clone();
        let mut after = (*before).clone();
        if after.format.line_ending != format.line_ending { after.endings = Arc::new(vec![format.line_ending; after.endings.len()]); }
        after.format = format;
        self.current = Arc::new(after);
        self.undo.push(HistoryEntry { before, after: self.current.clone(), before_caret: caret, after_caret: caret, selection: None, edits: Vec::new(), origin });
        self.redo.clear();
        self.publish(Some(origin), Vec::new(), cx);
    }
    pub fn reload(&mut self, text: String, format: TextFormat, cx: &mut Context<Self>) {
        let old_len = self.source().len();
        let (text, endings) = normalize_input(&text, LineEnding::Lf);
        let edit = SourceEdit { range: 0..old_len, removed: self.source().into(), inserted: text.as_str().into() };
        self.current = Arc::new(SourceSnapshot { text: Arc::new(text), endings: Arc::new(endings), format });
        self.undo.clear(); self.redo.clear(); self.composition = None;
        self.publish(None, vec![edit], cx);
    }
    pub(crate) fn replace(&mut self, range: Range<usize>, text: &str, origin: EntityId, before_caret: usize, coalesce: bool, cx: &mut Context<Self>) -> Arc<SourceSnapshot> {
        assert!(self.composition.is_none(), "composition must be finalized before another command");
        let before = self.current.clone();
        let edit = self.splice(range, text);
        if let Some(edit) = edit {
            let after_caret = edit.range.start + edit.inserted.len();
            if coalesce && self.undo.last().is_some_and(|entry| entry.origin == origin && Arc::ptr_eq(&entry.after, &before)) {
                let entry = self.undo.last_mut().unwrap();
                entry.after = self.current.clone(); entry.after_caret = after_caret; entry.edits.push(edit.clone());
            } else {
                self.undo.push(HistoryEntry { before, after: self.current.clone(), before_caret, after_caret, selection: None, edits: vec![edit.clone()], origin });
            }
            self.redo.clear();
            self.publish(Some(origin), vec![edit], cx);
        }
        self.current.clone()
    }
    pub(crate) fn replace_ranges(&mut self, mut replacements: Vec<(Range<usize>, String)>, origin: EntityId, caret: usize, cx: &mut Context<Self>) -> Arc<SourceSnapshot> {
        assert!(self.composition.is_none());
        replacements.sort_by(|a, b| b.0.start.cmp(&a.0.start));
        let mut end = self.source().len();
        for (range, _) in &replacements {
            assert!(range.start <= range.end && range.end <= end && self.source().is_char_boundary(range.start) && self.source().is_char_boundary(range.end));
            end = range.start;
        }
        let before = self.current.clone();
        let edits: Vec<_> = replacements.into_iter().filter_map(|(range, text)| self.splice(range, &text)).collect();
        if !edits.is_empty() {
            let after_caret = edits.iter().fold(caret, |at, edit| edit.map_offset(at));
            self.undo.push(HistoryEntry { before, after: self.current.clone(), before_caret: caret, after_caret, selection: None, edits: edits.clone(), origin });
            self.redo.clear();
            self.publish(Some(origin), edits, cx);
        }
        self.current.clone()
    }
    pub(crate) fn undo(&mut self, origin: EntityId, cx: &mut Context<Self>) -> Option<(Arc<SourceSnapshot>, Range<usize>)> {
        let entry = self.undo.pop()?;
        let edits = entry.edits.iter().rev().map(SourceEdit::inverse).collect();
        self.current = entry.before.clone();
        let caret = entry.selection.as_ref().map_or(entry.before_caret..entry.before_caret, |(before, _)| before.clone());
        self.redo.push(entry);
        self.publish(Some(origin), edits, cx);
        Some((self.current.clone(), caret))
    }
    pub(crate) fn redo(&mut self, origin: EntityId, cx: &mut Context<Self>) -> Option<(Arc<SourceSnapshot>, Range<usize>)> {
        let entry = self.redo.pop()?;
        let edits = entry.edits.clone();
        self.current = entry.after.clone();
        let caret = entry.selection.as_ref().map_or(entry.after_caret..entry.after_caret, |(_, after)| after.clone());
        self.undo.push(entry);
        self.publish(Some(origin), edits, cx);
        Some((self.current.clone(), caret))
    }
    pub(crate) fn compose(&mut self, range: Range<usize>, text: &str, owner: WeakEntity<crate::EditorState>, caret: usize, cx: &mut Context<Self>) -> Arc<SourceSnapshot> {
        let origin = owner.entity_id();
        if self.composition.is_none() {
            self.composition = Some(Composition { owner, before: self.current.clone(), before_caret: caret, range: range.clone(), edits: Vec::new() });
        }
        assert_eq!(self.composition.as_ref().unwrap().owner.entity_id(), origin);
        if text.is_empty() {
            self.finish_composition(origin, true, caret, cx);
        } else if let Some(edit) = self.splice(range.clone(), text) {
            let composition = self.composition.as_mut().unwrap();
            composition.range = range.start..range.start + edit.inserted.len();
            composition.edits.push(edit.clone());
            self.publish(Some(origin), vec![edit], cx);
        }
        self.current.clone()
    }
    pub(crate) fn finish_composition(&mut self, origin: EntityId, cancel: bool, caret: usize, cx: &mut Context<Self>) -> Arc<SourceSnapshot> {
        let Some(composition) = self.composition.take() else { return self.current.clone(); };
        assert_eq!(composition.owner.entity_id(), origin);
        if cancel {
            self.current = composition.before;
            let edits = composition.edits.iter().rev().map(SourceEdit::inverse).collect();
            self.publish(Some(origin), edits, cx);
        } else if !composition.edits.is_empty() {
            self.undo.push(HistoryEntry { before: composition.before, after: self.current.clone(), before_caret: composition.before_caret, after_caret: caret, selection: None, edits: composition.edits, origin });
            self.redo.clear();
        }
        self.current.clone()
    }
    pub(crate) fn replace_lines(&mut self, range: Range<usize>, lines: Vec<Range<usize>>, leading: bool, trailing: bool, origin: EntityId, caret: usize, cx: &mut Context<Self>) -> Arc<SourceSnapshot> {
        assert!(self.composition.is_none());
        let mut inserted = String::new(); let mut endings = Vec::new();
        if leading { inserted.push('\n'); endings.push(self.current.format.line_ending); }
        let line_count = lines.len();
        for (index, line) in lines.into_iter().enumerate() {
            let row = self.source()[..line.start].bytes().filter(|&byte| byte == b'\n').count();
            let text = &self.source()[line];
            let count = text.bytes().filter(|&byte| byte == b'\n').count();
            inserted.push_str(text); endings.extend_from_slice(&self.current.endings[row..row + count]);
            if index + 1 < line_count && !inserted.ends_with('\n') { inserted.push('\n'); endings.push(self.current.format.line_ending); }
        }
        if trailing && !inserted.ends_with('\n') { inserted.push('\n'); endings.push(self.current.format.line_ending); }
        if !trailing && inserted.ends_with('\n') { inserted.pop(); endings.pop(); }
        let before = self.current.clone();
        if let Some(edit) = self.splice_exact(range, inserted, endings) {
            let after_caret = edit.map_offset(caret);
            self.undo.push(HistoryEntry { before, after: self.current.clone(), before_caret: caret, after_caret, selection: None, edits: vec![edit.clone()], origin });
            self.redo.clear(); self.publish(Some(origin), vec![edit], cx);
        }
        self.current.clone()
    }
    pub(crate) fn set_history_selection(&mut self, origin: EntityId, before: Range<usize>, after: Range<usize>, coalesced: bool) {
        if let Some(entry) = self.undo.last_mut().filter(|entry| entry.origin == origin && Arc::ptr_eq(&entry.after, &self.current)) {
            let before = if coalesced { entry.selection.as_ref().map_or(entry.before_caret..entry.before_caret, |(before, _)| before.clone()) } else { before };
            entry.selection = Some((before, after));
        }
    }
    fn splice(&mut self, range: Range<usize>, text: &str) -> Option<SourceEdit> {
        let (inserted, endings) = normalize_input(text, self.current.format.line_ending);
        self.splice_exact(range, inserted, endings)
    }
    fn splice_exact(&mut self, range: Range<usize>, inserted: String, endings: Vec<LineEnding>) -> Option<SourceEdit> {
        assert!(range.start <= range.end && self.source().is_char_boundary(range.start) && self.source().is_char_boundary(range.end));
        let start_row = self.source()[..range.start].bytes().filter(|&b| b == b'\n').count();
        let removed_lines = self.source()[range.clone()].bytes().filter(|&b| b == b'\n').count();
        if self.source()[range.clone()] == inserted && self.current.endings[start_row..start_row + removed_lines] == endings { return None; }
        let removed = Arc::from(&self.source()[range.clone()]);
        let current = Arc::make_mut(&mut self.current);
        Arc::make_mut(&mut current.text).replace_range(range.clone(), &inserted);
        Arc::make_mut(&mut current.endings).splice(start_row..start_row + removed_lines, endings);
        Some(SourceEdit { range, removed, inserted: inserted.into() })
    }
    fn publish(&mut self, origin: Option<EntityId>, edits: Vec<SourceEdit>, cx: &mut Context<Self>) {
        self.revision = self.revision.checked_add(1).expect("document revision overflow");
        cx.emit(DocumentEvent { revision: self.revision, origin, edits: edits.into(), snapshot: self.current.clone() });
        cx.notify();
    }
}
fn normalize_input(text: &str, new_line: LineEnding) -> (String, Vec<LineEnding>) {
    let endings = text.match_indices('\n').map(|(at, _)| if at > 0 && text.as_bytes()[at - 1] == b'\r' { LineEnding::CrLf } else { new_line }).collect();
    (if text.contains("\r\n") { text.replace("\r\n", "\n") } else { text.to_owned() }, endings)
}
