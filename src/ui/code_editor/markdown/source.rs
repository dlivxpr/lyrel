use super::projection::{Kind, Projection};
use super::*;
use gpui_kit::TestSupportExt as _;
use gpui_kit::base::text::editor_host::{
    BlockNode, CodeBlock, FlowPosition, InlineNode, LeafGeometry, Paragraph, RenderContext,
    collect_leaf_geometry,
};
use gpui_kit::component::{ActiveTheme as _, input as actions};
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::collections::HashMap;

struct CachedBlock {
    version: u64,
    seen: u64,
    node: BlockNode,
}
/// A native editing presentation. It never owns a second text buffer or history.
pub(crate) struct SourcePresentation {
    document: Entity<MarkdownDocument>,
    state: PresentationState,
    focus: FocusHandle,
    scroll: ScrollHandle,
    geometry: Option<(u64, HashMap<u64, LeafGeometry>)>,
    table_geometry: HashMap<u64, Bounds<Pixels>>,
    dragging: bool,
    reveal_caret: bool,
    wrap: bool,
    live: bool,
    projection: Projection,
    blocks: HashMap<u64, CachedBlock>,
    tables: HashMap<u64, BlockNode>,
    rendered_revision: Option<u64>,
    render_generation: u64,
    parse_task: Option<Task<()>>,
    extra_selections: Vec<Selection>,
    search_matches: Vec<Range<usize>>,
    indent: crate::ui::editor_text::Indent,
    viewport_height: Pixels,
    reveal_at: Option<usize>,
    selection_notification: bool,
    _observe: Subscription,
}
impl SourcePresentation {
    pub(crate) fn new(
        document: Entity<MarkdownDocument>,
        wrap: bool,
        cx: &mut Context<Self>,
    ) -> Self {
        let observe = cx.subscribe(&document, |this, _, event: &DocumentEvent, cx| {
            if let DocumentEvent::Edited(transaction) = event {
                this.state.rebase(transaction);
                if transaction.origin != this.state.id {
                    for selection in &mut this.extra_selections {
                        *selection = transaction.map_selection(*selection);
                    }
                }
                let doc = this.document.read(cx);
                if transaction.revision() == doc.revision() {
                    this.state.selection = Selection::new(
                        doc.boundary(this.state.selection.anchor, false),
                        doc.boundary(this.state.selection.head, false),
                    );
                }
                if this.projection.revision != transaction.revision()
                    && transaction.revision() == doc.revision()
                    && this
                        .projection
                        .revision
                        .wrapping_add(transaction.edits.len() as u64)
                        == transaction.revision()
                {
                    this.projection.rebase(doc.source(), transaction, this.live);
                    this.rendered_revision = None;
                }
                this.sync_inline_reveal();
                this.schedule_parse(cx);
                this.geometry = None;
                cx.notify();
            }
        });
        let projection =
            Projection::source(document.read(cx).source(), document.read(cx).revision());
        Self {
            document,
            state: PresentationState::default(),
            focus: cx.focus_handle().tab_stop(true),
            scroll: ScrollHandle::new(),
            geometry: None,
            table_geometry: HashMap::new(),
            dragging: false,
            reveal_caret: false,
            wrap,
            live: false,
            projection,
            blocks: HashMap::new(),
            tables: HashMap::new(),
            rendered_revision: None,
            render_generation: 0,
            parse_task: None,
            extra_selections: Vec::new(),
            search_matches: Vec::new(),
            indent: crate::ui::editor_text::Indent {
                hard_tabs: false,
                size: 4,
            },
            viewport_height: Pixels::ZERO,
            reveal_at: None,
            selection_notification: false,
            _observe: observe,
        }
    }
    pub(crate) fn document(&self) -> &Entity<MarkdownDocument> {
        &self.document
    }
    pub(crate) fn indent(&self) -> crate::ui::editor_text::Indent {
        self.indent
    }
    pub(crate) fn set_indent(
        &mut self,
        indent: crate::ui::editor_text::Indent,
        cx: &mut Context<Self>,
    ) {
        self.indent = indent;
        cx.notify();
    }
    pub(crate) fn is_live(&self) -> bool {
        self.live
    }
    pub(crate) fn toggle_mode(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.live = !self.live;
        self.geometry = None;
        self.rendered_revision = None;
        self.parse_task = None;
        if self.live {
            self.schedule_parse(cx);
        } else {
            self.projection.install(Projection::source(
                self.document.read(cx).source(),
                self.document.read(cx).revision(),
            ));
        }
        self.reveal_caret = true;
        self.focus.focus(window, cx);
        window.notify_text_selection_changed(&self.focus);
        cx.notify();
    }
    fn schedule_parse(&mut self, cx: &mut Context<Self>) {
        if !self.live && self.projection.revision == self.document.read(cx).revision() {
            return;
        }
        let live = self.live;
        let document = self.document.clone();
        let identity = document.entity_id();
        let revision = document.read(cx).revision();
        let source = document.read(cx).source().clone();
        self.parse_task = Some(cx.spawn(async move |view, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(45))
                .await;
            let projection = cx
                .background_executor()
                .spawn(async move { Projection::parse(&source.to_string(), revision, live) })
                .await;
            let _ = view.update(cx, |view, cx| {
                if view.live == live
                    && view.document.entity_id() == identity
                    && view.document.read(cx).revision() == revision
                {
                    view.projection.install(projection);
                    view.sync_inline_reveal();
                    view.state.selection = view.map_live_caret(view.state.selection, cx);
                    view.selection_notification = true;
                    view.rendered_revision = None;
                    view.geometry = None;
                    cx.notify();
                }
            });
        }));
    }
    // Display-boundary insertion has right affinity after a hidden opening
    // delimiter and left affinity before a hidden closing delimiter.
    fn map_live_caret(&self, selection: Selection, cx: &App) -> Selection {
        if self.live
            && !self.is_composing()
            && selection.range().is_empty()
            && self.projection.revision == self.document.read(cx).revision()
        {
            if let Some(leaf) = self.projection.leaf_at(selection.head) {
                if leaf.inline_position(selection.head) {
                    return selection;
                }
                return Selection::caret(leaf.source_offset(leaf.display_offset(selection.head)));
            }
        }
        selection
    }
    fn sync_inline_reveal(&mut self) {
        let marked = self.state.marked_range();
        for leaf in &mut self.projection.leaves {
            if leaf.contextual_reveal(
                self.state.selection,
                &self.extra_selections,
                marked.as_ref(),
            ) {
                if let Some(cached) = self.blocks.get_mut(&leaf.id) {
                    cached.version = u64::MAX;
                }
                self.geometry = None;
                self.rendered_revision = None;
            }
        }
    }
    pub(crate) fn finish_active_composition(
        document: &Entity<MarkdownDocument>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let owner = document
            .read(cx)
            .composition_owner
            .as_ref()
            .and_then(WeakEntity::upgrade);
        if let Some(owner) = owner {
            owner.update(cx, |view, cx| {
                view.command(Command::FinishComposition, window, cx)
            });
        }
    }
    pub(crate) fn add_selection(&mut self, range: Range<usize>, cx: &mut Context<Self>) {
        let doc = self.document.read(cx);
        let range = doc.safe_range(range);
        let selection = Selection::new(range.start, range.end);
        if selection != self.state.selection && !self.extra_selections.contains(&selection) {
            self.extra_selections.push(selection);
        }
        self.reveal_caret = true;
        self.reveal_at = Some(selection.head);
        self.selection_notification = true;
        self.sync_inline_reveal();
        cx.notify();
    }
    pub(crate) fn set_search_matches(
        &mut self,
        matches: Vec<Range<usize>>,
        cx: &mut Context<Self>,
    ) {
        self.search_matches = matches;
        cx.notify();
    }
    pub(crate) fn select_range(&mut self, range: Range<usize>, cx: &mut Context<Self>) {
        self.extra_selections.clear();
        self.reveal_caret = true;
        self.reveal_at = None;
        self.selection_notification = true;
        if self.is_composing() {
            for cached in self.blocks.values_mut() {
                cached.version = u64::MAX;
            }
            self.rendered_revision = None;
        }
        self.document.update(cx, |document, cx| {
            document.command(
                &mut self.state,
                Command::Select(Selection::new(range.start, range.end)),
            );
            cx.emit(DocumentEvent::PresentationChanged);
        });
        self.state.selection = self.map_live_caret(self.state.selection, cx);
        self.sync_inline_reveal();
        cx.notify();
    }
    pub(crate) fn selection(&self) -> Selection {
        self.state.selection()
    }
    pub(crate) fn occurrence_range(&self) -> Range<usize> {
        self.extra_selections
            .last()
            .copied()
            .unwrap_or(self.state.selection)
            .range()
    }
    pub(crate) fn is_composing(&self) -> bool {
        self.state.composition.is_some()
    }
    pub(crate) fn is_soft_wrap(&self) -> bool {
        self.wrap
    }
    pub(crate) fn set_soft_wrap(&mut self, wrap: bool, cx: &mut Context<Self>) {
        self.wrap = wrap;
        self.geometry = None;
        cx.notify();
    }
    pub(crate) fn scroll_offset(&self) -> Point<Pixels> {
        self.scroll.offset()
    }
    pub(crate) fn set_scroll_offset(&mut self, offset: Point<Pixels>, cx: &mut Context<Self>) {
        self.scroll.set_offset(offset);
        self.geometry = None;
        self.reveal_caret = false;
        cx.notify();
    }
    pub(crate) fn command(
        &mut self,
        command: Command,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.selection = self.map_live_caret(self.state.selection, cx);
        let command = match command {
            Command::Select(selection) => Command::Select(self.map_live_caret(selection, cx)),
            command => command,
        };
        let cell_input = |range: &Range<usize>, text: String, selected| {
            if self.live && self.projection.leaf_at(range.start).is_some_and(|leaf| {
                matches!(leaf.kind, Kind::TableCell(_)) && range.end <= leaf.range.end
            }) {
                super::tables::input(self.document.read(cx).source(), range, text, selected)
            } else {
                (text, selected)
            }
        };
        let command = match command {
            Command::Insert(text) => Command::Insert(cell_input(&self.state.selection.range(), text, None).0),
            Command::Commit(text) => {
                let range = self.state.marked_range().unwrap_or_else(|| self.state.selection.range());
                Command::Commit(cell_input(&range, text, None).0)
            }
            Command::Compose { range, text, selected } => {
                let target = range.clone().or_else(|| self.state.marked_range()).unwrap_or_else(|| self.state.selection.range());
                let (text, selected) = cell_input(&target, text, selected);
                Command::Compose { range, text, selected }
            }
            Command::Replace { range, text, kind } if matches!(kind, EditKind::Typing | EditKind::Paste) => {
                let (text, _) = cell_input(&range, text, None);
                Command::Replace { range, text, kind }
            }
            command => command,
        };
        let marked_before = self.state.marked_range();
        let mut after_extra = None;
        let command = if !self.extra_selections.is_empty() {
            let replacement = match &command {
                Command::Insert(text) | Command::Commit(text) if !self.is_composing() => {
                    Some((text.clone(), EditKind::Typing))
                }
                Command::Replace { text, kind, range }
                    if *range == self.state.selection.range() =>
                {
                    Some((text.clone(), *kind))
                }
                Command::Backspace | Command::Delete => Some((String::new(), EditKind::Command)),
                _ => None,
            };
            if let Some((text, kind)) = replacement {
                let doc = self.document.read(cx);
                let mut edits: Vec<_> = std::iter::once(self.state.selection)
                    .chain(self.extra_selections.iter().copied())
                    .map(|selection| {
                        let mut range = selection.range();
                        if range.is_empty() {
                            if matches!(command, Command::Backspace) {
                                range.start = doc.previous(range.start);
                            }
                            if matches!(command, Command::Delete) {
                                range.end = doc.next(range.end);
                            }
                        }
                        (range, text.clone())
                    })
                    .collect();
                edits.sort_by_key(|(range, _)| range.start);
                let mut delta: isize = 0;
                let mut resulting = Vec::new();
                for (range, text) in &edits {
                    resulting.push(Selection::caret(
                        (range.start as isize + delta) as usize + text.len(),
                    ));
                    delta += text.len() as isize - range.len() as isize;
                }
                after_extra = Some(resulting.into_iter().skip(1).collect());
                Command::BatchReplace { edits, kind }
            } else {
                if matches!(
                    command,
                    Command::Select(_) | Command::Undo | Command::Redo | Command::Compose { .. }
                ) {
                    self.extra_selections.clear();
                }
                command
            }
        } else {
            command
        };
        let owner = self
            .document
            .read(cx)
            .composition_owner
            .as_ref()
            .and_then(WeakEntity::upgrade);
        if let Some(owner) = owner.filter(|owner| owner.entity_id() != cx.entity().entity_id()) {
            owner.update(cx, |view, cx| {
                view.command(Command::FinishComposition, window, cx)
            });
        }
        let own_view = cx.entity().downgrade();
        let changed = self.document.update(cx, |document, cx| {
            let revision = document.revision();
            let transaction = document.command(&mut self.state, command);
            document.composition_owner = self.state.composition.as_ref().map(|_| own_view);
            let changed = document.revision() != revision;
            if changed {
                if self.projection.revision == revision {
                    self.projection
                        .rebase(document.source(), &transaction, self.live);
                }
                self.rendered_revision = None;
                cx.emit(DocumentEvent::Edited(transaction));
                cx.notify();
            } else {
                cx.emit(DocumentEvent::PresentationChanged);
            }
            changed
        });
        if let Some(selections) = after_extra {
            self.extra_selections = selections;
        }
        if marked_before != self.state.marked_range() {
            let after = self.state.marked_range();
            for leaf in &self.projection.leaves {
                if marked_before
                    .iter()
                    .chain(after.iter())
                    .any(|range| range.start <= leaf.range.end && range.end >= leaf.range.start)
                {
                    if let Some(cached) = self.blocks.get_mut(&leaf.id) {
                        cached.version = u64::MAX;
                    }
                }
            }
            self.rendered_revision = None;
        }
        self.sync_inline_reveal();
        if changed {
            self.geometry = None;
            self.schedule_parse(cx);
            window.notify_text_input_changed(&self.focus);
        } else {
            window.notify_text_selection_changed(&self.focus);
        }
        self.reveal_caret = true;
        self.reveal_at = None;
        cx.notify();
    }
    fn caret_bounds(&self, byte: usize, cx: &App) -> Option<Bounds<Pixels>> {
        let (revision, geometry) = self.geometry.as_ref()?;
        if *revision != self.document.read(cx).revision() {
            return None;
        }
        let mapped = self.projection.leaf_at(byte)?;
        let leaf = geometry.get(&mapped.id)?;
        Some(Bounds::new(
            leaf.position_for_index(mapped.display_offset(byte)),
            size(px(1.), leaf.caret_height()),
        ))
    }
    pub(crate) fn range_to_bounds(&self, range: &Range<usize>, cx: &App) -> Option<Bounds<Pixels>> {
        self.caret_bounds(range.end, cx)
    }
    fn point_offset(&self, point: Point<Pixels>, cx: &App) -> Option<usize> {
        let (revision, geometry) = self.geometry.as_ref()?;
        let doc = self.document.read(cx);
        if *revision != doc.revision() {
            return None;
        }
        let (id, leaf) = geometry.iter().min_by(|(a_id, a), (b_id, b)| {
            let distance = |id: &u64, leaf: &LeafGeometry| {
                let bounds = self.table_geometry.get(id).copied().unwrap_or_else(|| leaf.bounds());
                let dy = if point.y < bounds.top() {
                    bounds.top() - point.y
                } else if point.y > bounds.bottom() {
                    point.y - bounds.bottom()
                } else {
                    px(0.)
                };
                let dx = if point.x < bounds.left() {
                    bounds.left() - point.x
                } else if point.x > bounds.right() {
                    point.x - bounds.right()
                } else {
                    px(0.)
                };
                f32::from(dx).powi(2) + f32::from(dy).powi(2)
            };
            distance(a_id, a)
                .partial_cmp(&distance(b_id, b))
                .unwrap_or(std::cmp::Ordering::Equal)
        })?;
        let mapped = self
            .projection
            .leaves
            .iter()
            .find(|mapped| mapped.id == *id)?;
        Some(doc.boundary(mapped.source_offset(leaf.index_for_position(point)), false))
    }
    fn select_point(
        &mut self,
        point: Point<Pixels>,
        extend: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(at) = self.point_offset(point, cx) else {
            return;
        };
        let anchor = if extend {
            self.state.selection.anchor
        } else {
            at
        };
        self.command(Command::Select(Selection::new(anchor, at)), window, cx);
    }
    fn move_cursor(
        &mut self,
        horizontal: i32,
        vertical: i32,
        extend: bool,
        edge: Option<bool>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let doc = self.document.read(cx);
        let current = doc.boundary(self.state.selection.head, false);
        let at = if let Some(end) = edge {
            self.visual_line_edge(current, end, cx).unwrap_or_else(|| {
                let row = doc.source().offset_to_point(current).row;
                if end {
                    doc.source().line_end_offset(row)
                } else {
                    doc.source().line_start_offset(row)
                }
            })
        } else if vertical != 0 {
            self.vertical_target(current, vertical as f32, cx)
                .unwrap_or(current)
        } else if !extend && !self.state.selection.range().is_empty() {
            if horizontal < 0 {
                self.state.selection.range().start
            } else {
                self.state.selection.range().end
            }
        } else if self.live {
            self.visible_step(current, horizontal < 0)
        } else if horizontal < 0 {
            doc.previous(current)
        } else {
            doc.next(current)
        };
        let at = doc.boundary(at, false);
        let anchor = if extend {
            self.state.selection.anchor
        } else {
            at
        };
        self.command(Command::Select(Selection::new(anchor, at)), window, cx);
    }
    fn vertical_target(&self, at: usize, lines: f32, cx: &App) -> Option<usize> {
        let (_, geometry) = self.geometry.as_ref()?;
        let mapped = self.projection.leaf_at(at)?;
        let leaf = geometry.get(&mapped.id)?;
        let caret = leaf.position_for_index(mapped.display_offset(at));
        self.point_offset(
            point(caret.x, caret.y + leaf.line_height() * (lines + 0.25)),
            cx,
        )
    }
    fn visual_line_edge(&self, at: usize, end: bool, cx: &App) -> Option<usize> {
        let (_, geometry) = self.geometry.as_ref()?;
        let mapped = self.projection.leaf_at(at)?;
        let leaf = geometry.get(&mapped.id)?;
        let caret = leaf.position_for_index(mapped.display_offset(at));
        let x = if end {
            leaf.bounds().right()
        } else {
            leaf.bounds().left()
        };
        self.point_offset(point(x, caret.y + leaf.line_height() * 0.25), cx)
    }
    fn delete_line(&mut self, end: bool, window: &mut Window, cx: &mut Context<Self>) {
        let at = self.state.selection.head;
        let range = if self.state.selection.range().is_empty() {
            let target = self.visual_line_edge(at, end, cx).unwrap_or(at);
            target.min(at)..target.max(at)
        } else {
            self.state.selection.range()
        };
        self.command(
            Command::Replace {
                range,
                text: String::new(),
                kind: EditKind::Command,
            },
            window,
            cx,
        );
    }
    fn move_page(&mut self, direction: f32, window: &mut Window, cx: &mut Context<Self>) {
        let at = self.state.selection.head;
        if let Some(caret) = self.caret_bounds(at, cx) {
            if let Some(target) = self.point_offset(
                point(caret.left(), caret.top() + self.viewport_height * direction),
                cx,
            ) {
                self.command(Command::Select(Selection::caret(target)), window, cx);
            }
        }
    }
    fn navigate_table(&mut self, backward: bool, exit: bool, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !self.live || self.projection.revision != self.document.read(cx).revision() {
            return false;
        }
        let Some(target) = super::tables::navigate(&self.projection.leaves, self.state.selection.head, backward, exit) else {
            return false;
        };
        self.command(Command::FinishComposition, window, cx);
        if let Some(at) = target {
            self.command(Command::Select(Selection::caret(at)), window, cx);
        } else if backward {
            window.focus_prev(cx);
        } else {
            window.focus_next(cx);
        }
        true
    }
    fn indent_selection(&mut self, outdent: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.navigate_table(outdent, false, window, cx) {
            return;
        }
        let selected = self.state.selection.range();
        let text = if self.indent.hard_tabs {
            "\t".into()
        } else {
            " ".repeat(self.indent.size)
        };
        if !outdent && selected.is_empty() {
            self.command(Command::Insert(text), window, cx);
            return;
        }
        let doc = self.document.read(cx);
        let start_row = doc.source().offset_to_point(selected.start).row;
        let end_row = doc.source().offset_to_point(selected.end).row;
        let mut edits = Vec::new();
        for row in start_row..=end_row {
            let start = doc.source().line_start_offset(row);
            if row > start_row && start == selected.end {
                break;
            }
            if outdent {
                let end = doc.source().line_end_offset(row);
                let line = doc.source().slice(start..end).to_string();
                let remove = if line.starts_with('\t') {
                    1
                } else {
                    line.bytes()
                        .take(self.indent.size)
                        .take_while(|byte| *byte == b' ')
                        .count()
                };
                if remove > 0 {
                    edits.push((start..start + remove, String::new()));
                }
            } else {
                edits.push((start..start, text.clone()));
            }
        }
        if !edits.is_empty() {
            self.command(
                Command::BatchReplace {
                    edits,
                    kind: EditKind::Command,
                },
                window,
                cx,
            );
        }
    }
    fn word_command(
        &mut self,
        backward: bool,
        extend: bool,
        delete: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let doc = self.document.read(cx);
        let at = self.state.selection.head;
        let row = doc.source().offset_to_point(at).row;
        let start = doc.source().line_start_offset(row);
        let end = doc.source().line_end_offset(row);
        let line = doc.source().slice(start..end).to_string();
        let target = if backward {
            line.unicode_word_indices()
                .rev()
                .find(|(ix, _)| start + ix < at)
                .map_or_else(|| doc.previous(start), |(ix, _)| start + ix)
        } else {
            line.unicode_word_indices()
                .find(|(ix, word)| start + ix + word.len() > at)
                .map_or_else(|| doc.next(end), |(ix, word)| start + ix + word.len())
        };
        if delete {
            self.command(
                Command::Replace {
                    range: target.min(at)..target.max(at),
                    text: String::new(),
                    kind: EditKind::Command,
                },
                window,
                cx,
            );
        } else {
            let anchor = if extend {
                self.state.selection.anchor
            } else {
                target
            };
            self.command(Command::Select(Selection::new(anchor, target)), window, cx);
        }
    }
    fn add_vertical_cursor(&mut self, direction: i32, _: &mut Window, cx: &mut Context<Self>) {
        let at = self
            .extra_selections
            .last()
            .unwrap_or(&self.state.selection)
            .head;
        if let Some(offset) = self.vertical_target(at, direction as f32, cx) {
            self.add_selection(offset..offset, cx);
        }
    }
    fn visible_step(&self, at: usize, backward: bool) -> usize {
        let Some(leaf) = self.projection.leaf_at(at) else {
            return at;
        };
        let display = leaf.display_offset(at);
        if backward {
            if let Some((ix, _)) = leaf
                .text
                .grapheme_indices(true)
                .rev()
                .find(|(ix, _)| *ix < display)
            {
                return leaf.source_offset(ix);
            }
            self.projection
                .leaves
                .iter()
                .rev()
                .find(|other| other.range.end < leaf.range.start)
                .map_or(leaf.content_start(), |other| other.content_end())
        } else {
            if let Some((ix, _)) = leaf
                .text
                .grapheme_indices(true)
                .find(|(ix, _)| *ix > display)
            {
                return leaf.source_offset(ix);
            }
            if display < leaf.text.len() {
                return leaf.content_end();
            }
            self.projection
                .leaves
                .iter()
                .find(|other| other.range.start > leaf.range.end)
                .map_or(leaf.content_end(), |other| other.content_start())
        }
    }
    fn enter(&mut self, hard: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.navigate_table(hard, true, window, cx) {
            return;
        }
        let mut range = self.state.selection.range();
        let region = self
            .live
            .then(|| self.projection.leaf_at(range.start))
            .flatten()
            .filter(|leaf| !matches!(leaf.kind, Kind::Prose | Kind::Heading(_) | Kind::Rule));
        let command = if let Some(leaf) = region {
            let (prefix, text) =
                super::regions::line_break(leaf.kind, &leaf.text, leaf.display_offset(range.start));
            if range.is_empty() {
                range.start -= prefix;
            }
            Command::Replace {
                range,
                text,
                kind: EditKind::Command,
            }
        } else if self.live {
            Command::ParagraphBreak { hard }
        } else {
            Command::Insert("\n".into())
        };
        self.command(command, window, cx);
    }
    fn delete_prose(&mut self, backward: bool, window: &mut Window, cx: &mut Context<Self>) {
        if !self.live
            || !self.state.selection.range().is_empty()
            || !self.extra_selections.is_empty()
        {
            self.command(
                if backward {
                    Command::Backspace
                } else {
                    Command::Delete
                },
                window,
                cx,
            );
            return;
        }
        let at = self.state.selection.head;
        let Some(leaf) = self.projection.leaf_at(at) else {
            return;
        };
        if matches!(leaf.kind, Kind::TableCell(_))
            && ((backward && at <= leaf.content_start()) || (!backward && at >= leaf.content_end()))
        {
            self.navigate_table(backward, false, window, cx);
            return;
        }
        if super::regions::label(leaf.kind).is_some() {
            self.command(
                if backward {
                    Command::Backspace
                } else {
                    Command::Delete
                },
                window,
                cx,
            );
            return;
        }
        let other = if backward && at <= leaf.content_start() {
            self.projection
                .leaves
                .iter()
                .rev()
                .find(|other| {
                    other.range.end < leaf.range.start
                        && (!other.text.is_empty() || other.kind == Kind::Rule)
                })
                .map(|other| {
                    if super::regions::label(other.kind).is_some() {
                        self.document.read(cx).previous(at)..at
                    } else {
                        other.content_end()..leaf.content_start()
                    }
                })
        } else if !backward && at >= leaf.content_end() {
            self.projection
                .leaves
                .iter()
                .find(|other| {
                    other.range.start > leaf.range.end
                        && (!other.text.is_empty() || other.kind == Kind::Rule)
                })
                .map(|other| {
                    if super::regions::label(other.kind).is_some() {
                        at..self.document.read(cx).next(at)
                    } else {
                        leaf.content_end()..other.content_start()
                    }
                })
        } else {
            let next = self.visible_step(at, backward);
            Some(next.min(at)..next.max(at))
        };
        if let Some(range) = other.filter(|range| !range.is_empty()) {
            self.command(
                Command::Replace {
                    range,
                    text: String::new(),
                    kind: EditKind::Command,
                },
                window,
                cx,
            );
        }
    }
    fn copy(&mut self, cut: bool, window: &mut Window, cx: &mut Context<Self>) {
        let mut ranges: Vec<_> = std::iter::once(self.state.selection)
            .chain(self.extra_selections.iter().copied())
            .map(Selection::range)
            .filter(|range| !range.is_empty())
            .collect();
        if ranges.is_empty() {
            return;
        }
        ranges.sort_by_key(|range| range.start);
        let doc = self.document.read(cx);
        let text = ranges
            .iter()
            .map(|range| doc.source().slice(range.clone()).to_string())
            .collect::<Vec<_>>()
            .join("\n");
        let range = self.state.selection.range();
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        if cut {
            self.command(
                Command::Replace {
                    range,
                    text: String::new(),
                    kind: EditKind::Command,
                },
                window,
                cx,
            );
        }
    }
    fn paste_clipboard(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            // New clipboard line breaks follow the current codec's LF insertion
            // convention; this does not rewrite any existing document region.
            let text = if text.contains("\r\n") {
                text.replace("\r\n", "\n")
            } else {
                text
            };
            self.command(
                Command::Replace {
                    range: self.state.selection.range(),
                    text,
                    kind: EditKind::Paste,
                },
                window,
                cx,
            );
        }
    }
}
impl Focusable for SourcePresentation {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
impl EntityInputHandler for SourcePresentation {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        adjusted: &mut Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<String> {
        let doc = self.document.read(cx);
        let range = doc.utf16_range(range);
        *adjusted = Some(doc.utf16_offset(range.start)..doc.utf16_offset(range.end));
        Some(doc.source().slice(range).to_string())
    }
    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let doc = self.document.read(cx);
        let selected = self.state.selection;
        let range = selected.range();
        Some(UTF16Selection {
            range: doc.utf16_offset(range.start)..doc.utf16_offset(range.end),
            reversed: selected.is_reversed(),
        })
    }
    fn marked_text_range(&self, _: &mut Window, cx: &mut Context<Self>) -> Option<Range<usize>> {
        let doc = self.document.read(cx);
        let range = self.state.marked_range()?;
        Some(doc.utf16_offset(range.start)..doc.utf16_offset(range.end))
    }
    fn unmark_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.command(Command::FinishComposition, window, cx);
    }
    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(range) = range {
            let range = self.document.read(cx).utf16_range(range);
            if self.state.marked_range().as_ref() != Some(&range) {
                self.command(Command::FinishComposition, window, cx);
                self.command(
                    Command::Replace {
                        range,
                        text: text.into(),
                        kind: EditKind::Typing,
                    },
                    window,
                    cx,
                );
                return;
            }
        }
        self.command(Command::Commit(text.into()), window, cx);
    }
    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        selected: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range.map(|range| self.document.read(cx).utf16_range(range));
        self.command(
            Command::Compose {
                range,
                text: text.into(),
                selected,
            },
            window,
            cx,
        );
    }
    fn bounds_for_range(
        &mut self,
        range: Range<usize>,
        _: Bounds<Pixels>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let range = self.document.read(cx).utf16_range(range);
        self.caret_bounds(range.start, cx)
    }
    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<usize> {
        Some(
            self.document
                .read(cx)
                .utf16_offset(self.point_offset(point, cx)?),
        )
    }
    fn set_selected_text_range(
        &mut self,
        range: Range<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = self.document.read(cx).utf16_range(range);
        self.command(
            Command::Select(Selection::new(range.start, range.end)),
            window,
            cx,
        );
    }
    fn text_length_utf16(&mut self, _: &mut Window, cx: &mut Context<Self>) -> Option<usize> {
        let doc = self.document.read(cx);
        Some(doc.utf16_offset(doc.source().len()))
    }
}
impl Render for SourcePresentation {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let revision = self.projection.revision;
        if self.rendered_revision != Some(revision) {
            self.render_generation += 1;
            for leaf in &self.projection.leaves {
                let cached = self.blocks.entry(leaf.id).or_insert_with(|| CachedBlock {
                    version: u64::MAX,
                    seen: 0,
                    node: BlockNode::Unknown,
                });
                if cached.version != leaf.version {
                    if self.live && matches!(leaf.kind, Kind::Code(_)) {
                        cached.node = BlockNode::CodeBlock(
                            CodeBlock::from_code(leaf.text.clone(), None::<SharedString>)
                                .with_leaf_key(leaf.id),
                        );
                        cached.version = leaf.version;
                        cached.seen = self.render_generation;
                        continue;
                    }
                    let mut paragraph = Paragraph::default().with_leaf_key(leaf.id);
                    let marked = self.state.marked_range();
                    let marks = leaf.marks(marked.as_ref());
                    paragraph.push(InlineNode::new(leaf.text.clone()).marks(marks));
                    cached.node = match leaf.kind {
                        Kind::Heading(level) if self.live => BlockNode::Heading {
                            level,
                            children: paragraph,
                            span: None,
                        },
                        _ => BlockNode::Paragraph(paragraph),
                    };
                    cached.version = leaf.version;
                }
                cached.seen = self.render_generation;
            }
            self.blocks
                .retain(|_, cached| cached.seen == self.render_generation);
            self.tables.clear();
            let mut ix = 0;
            while ix < self.projection.leaves.len() {
                let leaf = &self.projection.leaves[ix];
                if let Kind::TableCell(cell) = leaf.kind {
                    let end = ix + self.projection.leaves[ix..].iter().take_while(|leaf| {
                        matches!(leaf.kind, Kind::TableCell(other) if other.table() == cell.table())
                    }).count();
                    self.tables.insert(leaf.id, super::tables::block(
                        self.projection.leaves[ix..end].iter().filter_map(|leaf| {
                            let Kind::TableCell(cell) = leaf.kind else { return None; };
                            Some((cell, &self.blocks[&leaf.id].node))
                        }),
                    ));
                    ix = end;
                } else {
                    ix += 1;
                }
            }
            self.rendered_revision = Some(revision);
        }
        let context = RenderContext::from_app(cx).with_body_font_size(window.rem_size());
        let references: Vec<_> = self
            .projection
            .leaves
            .iter()
            .map(|leaf| &self.blocks[&leaf.id].node)
            .collect();
        let block = div()
            .w_full()
            .children(self.projection.leaves.iter().enumerate().filter_map(|(ix, leaf)| {
                if matches!(leaf.kind, Kind::TableCell(_)) && !self.tables.contains_key(&leaf.id) {
                    return None;
                }
                let node = self.tables.get(&leaf.id).unwrap_or(&self.blocks[&leaf.id].node);
                let rendered = node.render_with_id(
                    ("markdown-leaf", leaf.id),
                    FlowPosition::at(&references, ix, ix + 1 == references.len())
                        .first(true)
                        .last(true),
                    &context,
                    window,
                    cx,
                );
                Some(div()
                    .id(("markdown-block", leaf.id))
                    .w_full()
                    .when(
                        self.live
                            && super::regions::label(leaf.kind).is_some()
                            && !leaf.text.is_empty(),
                        |block| {
                            block
                                .font_family(cx.theme().mono_font_family.clone())
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(crate::ui::i18n::t(
                                            super::regions::label(leaf.kind)
                                                .expect("source region label"),
                                        )),
                                )
                        },
                    )
                    .when(self.live && leaf.kind == Kind::Rule, |block| {
                        block.child(BlockNode::HorizontalRule { span: None }.render_with_id(
                            ("markdown-rule", leaf.id),
                            FlowPosition::at(&references, ix, true),
                            &context,
                            window,
                            cx,
                        ))
                    })
                    .child(rendered))
            }));
        let key_context =
            KeyContext::parse("Input mode=editor").expect("static editor key context");
        let root =
            div()
                .id("markdown-source")
                .test_support()
                .size_full()
                .min_h_0()
                .key_context(key_context)
                .track_focus(&self.focus)
                .cursor_text()
                .bg(cx.theme().background)
                .text_color(cx.theme().foreground)
                .when(!self.live, |root| {
                    root.font_family(cx.theme().mono_font_family.clone())
                })
                .when(!self.wrap, |root| root.whitespace_nowrap())
                .role(Role::TextInput)
                .aria_label(if self.live {
                    "Markdown live editor"
                } else {
                    "Markdown source"
                })
                .when(window.is_a11y_active() || cfg!(test), |root| {
                    root.aria_value(self.document.read(cx).source().to_string())
                })
                .on_a11y_action(AccessibleAction::Focus, {
                    let view = cx.entity();
                    move |_, window, cx| {
                        view.read(cx).focus.clone().focus(window, cx);
                    }
                })
                .on_a11y_action(AccessibleAction::SetValue, {
                    let view = cx.entity();
                    move |data, window, cx| {
                        if let Some(accesskit::ActionData::Value(text)) = data {
                            view.update(cx, |view, cx| {
                                let end = view.document.read(cx).source().len();
                                view.command(
                                    Command::Replace {
                                        range: 0..end,
                                        text: text.to_string(),
                                        kind: EditKind::Command,
                                    },
                                    window,
                                    cx,
                                );
                            });
                        }
                    }
                })
                .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                    if event.keystroke.key == "escape" && this.state.composition.is_some() {
                        this.command(Command::CancelComposition, window, cx);
                        cx.stop_propagation();
                    }
                }))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, event: &MouseDownEvent, window, cx| {
                        this.focus.focus(window, cx);
                        this.dragging = true;
                        this.select_point(event.position, event.modifiers.shift, window, cx);
                        cx.stop_propagation();
                    }),
                )
                .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, window, cx| {
                    if this.dragging {
                        this.select_point(event.position, true, window, cx);
                    }
                }))
                .on_mouse_up(
                    MouseButton::Left,
                    cx.listener(|this, _, _, _| this.dragging = false),
                )
                .on_mouse_up_out(
                    MouseButton::Left,
                    cx.listener(|this, _, _, _| this.dragging = false),
                )
                .on_action(
                    cx.listener(|this, _: &actions::Copy, window, cx| this.copy(false, window, cx)),
                )
                .on_action(
                    cx.listener(|this, _: &actions::Cut, window, cx| this.copy(true, window, cx)),
                )
                .on_action(cx.listener(|this, _: &actions::Paste, window, cx| {
                    this.paste_clipboard(window, cx)
                }))
                .on_action(cx.listener(|this, _: &actions::Undo, window, cx| {
                    this.command(Command::Undo, window, cx)
                }))
                .on_action(cx.listener(|this, _: &actions::Redo, window, cx| {
                    this.command(Command::Redo, window, cx)
                }))
                .on_action(cx.listener(|this, _: &actions::Backspace, window, cx| {
                    this.delete_prose(true, window, cx)
                }))
                .on_action(cx.listener(|this, _: &actions::Delete, window, cx| {
                    this.delete_prose(false, window, cx)
                }))
                .on_action(cx.listener(|this, action: &actions::Enter, window, cx| {
                    this.enter(action.shift, window, cx)
                }))
                .on_action(cx.listener(|this, _: &actions::Indent, window, cx| {
                    this.indent_selection(false, window, cx)
                }))
                .on_action(cx.listener(|this, _: &actions::IndentInline, window, cx| {
                    this.indent_selection(false, window, cx)
                }))
                .on_action(cx.listener(|this, _: &actions::Outdent, window, cx| {
                    this.indent_selection(true, window, cx)
                }))
                .on_action(cx.listener(|this, _: &actions::OutdentInline, window, cx| {
                    this.indent_selection(true, window, cx)
                }))
                .on_action(
                    cx.listener(|this, _: &actions::DeleteToBeginningOfLine, window, cx| {
                        this.delete_line(false, window, cx)
                    }),
                )
                .on_action(
                    cx.listener(|this, _: &actions::DeleteToEndOfLine, window, cx| {
                        this.delete_line(true, window, cx)
                    }),
                )
                .on_action(cx.listener(|this, _: &actions::MovePageUp, window, cx| {
                    this.move_page(-1., window, cx)
                }))
                .on_action(cx.listener(|this, _: &actions::MovePageDown, window, cx| {
                    this.move_page(1., window, cx)
                }))
                .on_action(
                    cx.listener(|this, _: &actions::MoveToPreviousWord, window, cx| {
                        this.word_command(true, false, false, window, cx)
                    }),
                )
                .on_action(
                    cx.listener(|this, _: &actions::MoveToNextWord, window, cx| {
                        this.word_command(false, false, false, window, cx)
                    }),
                )
                .on_action(cx.listener(
                    |this, _: &actions::SelectToPreviousWordStart, window, cx| {
                        this.word_command(true, true, false, window, cx)
                    },
                ))
                .on_action(
                    cx.listener(|this, _: &actions::SelectToNextWordEnd, window, cx| {
                        this.word_command(false, true, false, window, cx)
                    }),
                )
                .on_action(cx.listener(
                    |this, _: &actions::DeleteToPreviousWordStart, window, cx| {
                        this.word_command(true, false, true, window, cx)
                    },
                ))
                .on_action(
                    cx.listener(|this, _: &actions::DeleteToNextWordEnd, window, cx| {
                        this.word_command(false, false, true, window, cx)
                    }),
                )
                .on_action(cx.listener(
                    |this, _: &gpui_kit::base::input::AddCursorAbove, window, cx| {
                        this.add_vertical_cursor(-1, window, cx)
                    },
                ))
                .on_action(cx.listener(
                    |this, _: &gpui_kit::base::input::AddCursorBelow, window, cx| {
                        this.add_vertical_cursor(1, window, cx)
                    },
                ))
                .on_action(cx.listener(|this, _: &actions::SelectAll, window, cx| {
                    let end = this.document.read(cx).source().len();
                    this.command(Command::Select(Selection::new(0, end)), window, cx);
                }))
                .on_action(cx.listener(|this, _: &actions::MoveLeft, window, cx| {
                    this.move_cursor(-1, 0, false, None, window, cx)
                }))
                .on_action(cx.listener(|this, _: &actions::MoveRight, window, cx| {
                    this.move_cursor(1, 0, false, None, window, cx)
                }))
                .on_action(cx.listener(|this, _: &actions::MoveUp, window, cx| {
                    this.move_cursor(0, -1, false, None, window, cx)
                }))
                .on_action(cx.listener(|this, _: &actions::MoveDown, window, cx| {
                    this.move_cursor(0, 1, false, None, window, cx)
                }))
                .on_action(cx.listener(
                    |this, _: &gpui_kit::base::actions::SelectLeft, window, cx| {
                        this.move_cursor(-1, 0, true, None, window, cx)
                    },
                ))
                .on_action(cx.listener(
                    |this, _: &gpui_kit::base::actions::SelectRight, window, cx| {
                        this.move_cursor(1, 0, true, None, window, cx)
                    },
                ))
                .on_action(cx.listener(
                    |this, _: &gpui_kit::base::actions::SelectUp, window, cx| {
                        this.move_cursor(0, -1, true, None, window, cx)
                    },
                ))
                .on_action(cx.listener(
                    |this, _: &gpui_kit::base::actions::SelectDown, window, cx| {
                        this.move_cursor(0, 1, true, None, window, cx)
                    },
                ))
                .on_action(
                    cx.listener(|this, _: &actions::MoveToStartOfLine, window, cx| {
                        this.move_cursor(0, 0, false, Some(false), window, cx)
                    }),
                )
                .on_action(
                    cx.listener(|this, _: &actions::MoveToEndOfLine, window, cx| {
                        this.move_cursor(0, 0, false, Some(true), window, cx)
                    }),
                )
                .on_action(cx.listener(|this, _: &actions::MoveHome, window, cx| {
                    this.move_cursor(0, 0, false, Some(false), window, cx)
                }))
                .on_action(cx.listener(|this, _: &actions::MoveEnd, window, cx| {
                    this.move_cursor(0, 0, false, Some(true), window, cx)
                }))
                .on_action(
                    cx.listener(|this, _: &actions::SelectToStartOfLine, window, cx| {
                        this.move_cursor(0, 0, true, Some(false), window, cx)
                    }),
                )
                .on_action(
                    cx.listener(|this, _: &actions::SelectToEndOfLine, window, cx| {
                        this.move_cursor(0, 0, true, Some(true), window, cx)
                    }),
                )
                .on_action(cx.listener(|this, _: &actions::MoveToStart, window, cx| {
                    this.command(Command::Select(Selection::caret(0)), window, cx)
                }))
                .on_action(cx.listener(|this, _: &actions::MoveToEnd, window, cx| {
                    let end = this.document.read(cx).source().len();
                    this.command(Command::Select(Selection::caret(end)), window, cx);
                }))
                .on_action(cx.listener(|this, _: &actions::SelectToStart, window, cx| {
                    this.command(
                        Command::Select(Selection::new(this.state.selection.anchor, 0)),
                        window,
                        cx,
                    )
                }))
                .on_action(cx.listener(|this, _: &actions::SelectToEnd, window, cx| {
                    let end = this.document.read(cx).source().len();
                    this.command(
                        Command::Select(Selection::new(this.state.selection.anchor, end)),
                        window,
                        cx,
                    );
                }))
                .on_action(cx.listener(|this, _: &actions::Escape, window, cx| {
                    if this.state.composition.is_some() {
                        this.command(Command::CancelComposition, window, cx);
                    } else if this.navigate_table(false, true, window, cx) {
                        // Escape leaves the table without changing its source.
                    } else {
                        cx.propagate();
                    }
                }))
                .child(crate::ui::scrollbar::with_vertical_scrollbar(
                    "markdown-source-scrollbar",
                    div()
                        .id("markdown-source-scroll")
                        .size_full()
                        .overflow_y_scroll()
                        .overflow_x_scroll()
                        .track_scroll(&self.scroll)
                        .child(
                            div()
                                .p_3()
                                .capture_any_mouse_down(cx.listener(
                                    |this, event: &MouseDownEvent, window, cx| {
                                        if event.button != MouseButton::Left {
                                            return;
                                        }
                                        this.focus.focus(window, cx);
                                        this.dragging = true;
                                        this.select_point(
                                            event.position,
                                            event.modifiers.shift,
                                            window,
                                            cx,
                                        );
                                        cx.stop_propagation();
                                    },
                                ))
                                .child(block),
                        ),
                    &self.scroll,
                ));
        SourceElement {
            child: root.into_any_element(),
            view: cx.entity(),
            revision,
            color: cx.theme().selection,
            caret: cx.theme().caret,
        }
    }
}
struct SourceElement {
    child: AnyElement,
    view: Entity<SourcePresentation>,
    revision: u64,
    color: Hsla,
    caret: Hsla,
}
impl IntoElement for SourceElement {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
impl Element for SourceElement {
    type RequestLayoutState = ();
    type PrepaintState = ();
    fn id(&self) -> Option<ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }
    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        (self.child.request_layout(window, cx), ())
    }
    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let decorations = {
            let view = self.view.read(cx);
            view.projection
                .leaves
                .iter()
                .map(|leaf| {
                    let mut ranges = Vec::new();
                    for range in &view.search_matches {
                        let start = range.start.max(leaf.range.start);
                        let end = range.end.min(leaf.range.end);
                        let mut color = self.color;
                        color.a *= 0.45;
                        if start < end {
                            ranges.push((
                                leaf.display_offset(start)..leaf.display_offset(end),
                                color,
                            ));
                        }
                    }
                    for selection in std::iter::once(view.state.selection)
                        .chain(view.extra_selections.iter().copied())
                    {
                        let selection = selection.range();
                        let start = selection.start.max(leaf.range.start);
                        let end = selection.end.min(leaf.range.end);
                        if start < end {
                            ranges.push((
                                leaf.display_offset(start)..leaf.display_offset(end),
                                self.color,
                            ));
                        }
                    }
                    (leaf.id, ranges)
                })
                .collect()
        };
        let (_, geometry) = collect_leaf_geometry(decorations, || self.child.prepaint(window, cx));
        self.view.update(cx, |view, cx| {
            let previous_caret = view.caret_bounds(view.state.selection.head, cx);
            view.table_geometry = super::tables::geometry(&view.projection.leaves, &geometry);
            view.geometry = Some((self.revision, geometry));
            view.viewport_height = bounds.size.height;
            if view.focus.is_focused(window)
                && (view.selection_notification
                    || previous_caret != view.caret_bounds(view.state.selection.head, cx))
            {
                view.selection_notification = false;
                window.notify_text_selection_changed(&view.focus);
            }
            if view.reveal_caret && view.focus.is_focused(window) {
                view.reveal_caret = false;
                let at = view.reveal_at.take().unwrap_or(view.state.selection.head);
                if let Some(caret) = view.caret_bounds(at, cx) {
                    let inset = window.rem_size() * 0.75;
                    let delta = if caret.top() < bounds.top() + inset {
                        bounds.top() + inset - caret.top()
                    } else if caret.bottom() > bounds.bottom() - inset {
                        bounds.bottom() - inset - caret.bottom()
                    } else {
                        px(0.)
                    };
                    let delta_x = if caret.left() < bounds.left() + inset {
                        bounds.left() + inset - caret.left()
                    } else if caret.right() > bounds.right() - inset {
                        bounds.right() - inset - caret.right()
                    } else {
                        px(0.)
                    };
                    if delta != px(0.) || delta_x != px(0.) {
                        let offset = view.scroll.offset();
                        view.scroll.set_offset(point(
                            (offset.x + delta_x).min(px(0.)),
                            (offset.y + delta).min(px(0.)),
                        ));
                        view.geometry = None;
                        window.refresh();
                    }
                }
            }
        });
    }
    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child.paint(window, cx);
        let (focus, carets) = {
            let view = self.view.read(cx);
            let carets: Vec<_> = if view.focus.is_focused(window) {
                std::iter::once(view.state.selection)
                    .chain(view.extra_selections.iter().copied())
                    .filter(|selection| selection.range().is_empty())
                    .filter_map(|selection| view.caret_bounds(selection.head, cx))
                    .collect()
            } else {
                Vec::new()
            };
            (view.focus.clone(), carets)
        };
        window.handle_input(
            &focus,
            ElementInputHandler::new(bounds, self.view.clone()),
            cx,
        );
        for caret in carets {
            if caret.top() >= bounds.top()
                && caret.bottom() <= bounds.bottom()
                && caret.left() >= bounds.left()
                && caret.right() <= bounds.right()
            {
                window.paint_quad(fill(caret, self.caret));
            }
        }
    }
}
