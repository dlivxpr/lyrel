use super::*;

impl EditorState {
    pub fn document(&self) -> &Entity<Document> { &self.document }
    pub fn occurrence_range(&self) -> Range<usize> { self.extra_selections.last().unwrap_or(&self.selected_range).clone() }
    pub fn add_selection(&mut self, range: Range<usize>, cx: &mut Context<Self>) {
        self.finish_composition(cx);
        let range = self.content.floor_char_boundary(range.start.min(self.content.len()))..self.content.floor_char_boundary(range.end.min(self.content.len()));
        if range != self.selected_range && !self.extra_selections.contains(&range) { self.extra_selections.push(range); }
        self.last_edit = EditKind::Other; cx.notify();
    }
    pub(super) fn replace_occurrences(&mut self, text: &str, cx: &mut Context<Self>) {
        let primary = self.selected_range.clone();
        let mut selections = self.extra_selections.clone(); selections.push(primary.clone());
        selections.sort_by_key(|range| range.start);
        selections.dedup();
        let mut ranges: Vec<Range<usize>> = Vec::with_capacity(selections.len());
        for range in selections {
            if let Some(previous) = ranges.last_mut().filter(|previous| range.start < previous.end) {
                previous.end = previous.end.max(range.end);
            } else { ranges.push(range); }
        }
        let selections = ranges;
        let mut shift: isize = 0;
        let len = text.len() - text.matches("\r\n").count();
        let mut after_primary = 0..0; let mut after_extra = Vec::with_capacity(self.extra_selections.len());
        for range in &selections {
            let at = range.start.checked_add_signed(shift).expect("ordered occurrence offset") + len;
            if range.start <= primary.start && primary.end <= range.end { after_primary = at..at; } else { after_extra.push(at..at); }
            shift += len as isize - range.len() as isize;
        }
        self.replace_ranges(selections.into_iter().map(|range| (range, text.to_owned())).collect(), cx);
        self.selected_range = after_primary.clone(); self.extra_selections = after_extra;
        self.set_history_selection(primary, after_primary, cx);
        self.last_edit = EditKind::Other; cx.emit(EditorEvent::Changed); cx.notify();
    }
    /// Enable only after the host subscribes to OpenLink and HoverLink.
    pub fn set_host_link_navigation(&mut self, enabled: bool) { self.host_links = enabled; }
    /// Enable only after a real custom-color picker is installed by the host.
    pub fn set_host_custom_colors(&mut self, enabled: bool) { self.host_custom_colors = enabled; }
    pub fn is_source_only(&self) -> bool { self.source_only }
    pub fn tab_indent(&self) -> usize { self.tab_indent }
    pub fn set_source_only(&mut self, source_only: bool, cx: &mut Context<Self>) {
        self.finish_composition(cx);
        self.source_only = source_only;
        if source_only { self.markdown_style = None; }
        self.invalidate_geometry(); cx.notify();
    }
    pub fn set_history_selection(&mut self, before: Range<usize>, after: Range<usize>, cx: &mut Context<Self>) {
        let origin = cx.entity_id();
        self.document.update(cx, |document, _| document.set_history_selection(origin, before, after, false));
    }
    pub(super) fn indent_selected_lines(&mut self, outdent: bool, cx: &mut Context<Self>) -> bool {
        if self.selected_range.is_empty() || !self.content[self.selected_range.clone()].contains('\n') { return false; }
        let start = self.content[..self.selected_range.start].rfind('\n').map_or(0, |at| at + 1);
        let mut at = start;
        let mut replacements = Vec::new();
        for line in self.content[start..].split_inclusive('\n') {
            if at >= self.selected_range.end { break; }
            if outdent {
                let count = if line.starts_with('\t') { 1 } else { line.bytes().take(self.tab_indent).take_while(|byte| *byte == b' ').count() };
                if count > 0 { replacements.push((at..at + count, String::new())); }
            } else { replacements.push((at..at, " ".repeat(self.tab_indent))); }
            at += line.len();
        }
        self.replace_ranges(replacements, cx);
        true
    }
    pub fn replace_ranges(&mut self, replacements: Vec<(Range<usize>, String)>, cx: &mut Context<Self>) {
        self.finish_composition(cx);
        let before = self.selected_range.clone();
        let mut after = before.clone();
        let mut ordered: Vec<_> = replacements.iter().collect();
        ordered.sort_by_key(|(range, _)| std::cmp::Reverse(range.start));
        for (range, text) in ordered {
            let len = text.len() - text.matches("\r\n").count();
            let map = |at: usize| if at < range.start { at } else if at >= range.end { at - range.len() + len } else { range.start + len };
            after = map(after.start)..map(after.end);
        }
        let document = self.document.clone(); let origin = cx.entity_id(); let caret = self.cursor_offset();
        let snapshot = document.update(cx, |document, cx| document.replace_ranges(replacements, origin, caret, cx));
        self.diagnostics.clear();
        self.content = snapshot.text_handle(); self.content_gen = document.read(cx).revision();
        self.selected_range = after.clone();
        self.set_history_selection(before, after, cx);
        self.last_edit = EditKind::Other; self.invalidate_geometry(); cx.notify();
    }
    pub fn replace_lines(&mut self, range: Range<usize>, lines: Vec<Range<usize>>, leading: bool, trailing: bool, cx: &mut Context<Self>) {
        self.finish_composition(cx);
        let document = self.document.clone(); let origin = cx.entity_id(); let caret = self.cursor_offset();
        let snapshot = document.update(cx, |document, cx| document.replace_lines(range, lines, leading, trailing, origin, caret, cx));
        self.diagnostics.clear();
        self.content = snapshot.text_handle(); self.content_gen = document.read(cx).revision();
        self.last_edit = EditKind::Other; self.invalidate_geometry(); cx.notify();
    }
    pub fn selected_range(&self) -> Range<usize> { self.selected_range.clone() }
    pub fn selection_reversed(&self) -> bool { self.selection_reversed }
    pub fn scroll_offset(&self) -> Point<Pixels> { self.scroll.offset() }
    pub fn set_scroll_offset(&mut self, offset: Point<Pixels>, cx: &mut Context<Self>) { self.scroll.set_offset(offset); cx.notify(); }
    pub fn is_live(&self) -> bool { self.markdown_style.is_some() }
    pub fn is_soft_wrap(&self) -> bool { self.soft_wrap }
    pub fn set_soft_wrap(&mut self, wrap: bool, cx: &mut Context<Self>) { self.soft_wrap = wrap; self.invalidate_geometry(); cx.notify(); }
    pub fn set_live_style(&mut self, style: SyntaxStyle, cx: &mut Context<Self>) {
        self.live_style = Some(style.clone());
        if self.is_live() { self.markdown_style = Some(style); }
        self.invalidate_geometry(); cx.notify();
    }
    pub fn toggle_mode(&mut self, cx: &mut Context<Self>) {
        if self.source_only { return; }
        self.finish_composition(cx);
        if self.markdown_style.is_some() { self.markdown_style = None; }
        else { self.markdown_style = self.live_style.clone(); }
        self.invalidate_geometry(); cx.notify();
    }
    pub fn select_range(&mut self, range: Range<usize>, cx: &mut Context<Self>) {
        self.finish_composition(cx);
        self.extra_selections.clear();
        let start = self.content.floor_char_boundary(range.start.min(self.content.len()));
        let end = self.content.floor_char_boundary(range.end.min(self.content.len()));
        self.selected_range = start.min(end)..start.max(end);
        self.selection_reversed = start > end;
        self.last_edit = EditKind::Other;
        cx.emit(EditorEvent::SelectionChanged); cx.notify();
    }
    pub fn line_column(&self, at: usize) -> (usize, usize) {
        let at = self.content.floor_char_boundary(at.min(self.content.len()));
        let line = self.content[..at].bytes().filter(|&b| b == b'\n').count();
        let start = self.content[..at].rfind('\n').map_or(0, |at| at + 1);
        (line, self.content[start..at].chars().count())
    }
    pub fn offset_for_line_column(&self, line: usize, column: usize) -> usize {
        let start = if line == 0 { 0 } else { self.content.match_indices('\n').nth(line - 1).map_or(self.content.len(), |(at, _)| at + 1) };
        let text = self.content[start..].split('\n').next().unwrap_or("");
        start + text.char_indices().nth(column).map_or(text.len(), |(at, _)| at)
    }
    pub fn range_to_bounds(&self, range: &Range<usize>) -> Option<Bounds<Pixels>> {
        let bounds = self.last_bounds?;
        let (row, col) = self.row_col(range.start.min(self.content.len()));
        let lh = self.line_h(row);
        let p = line_pos(self.wrapped.get(row)?, self.bidi_map(row), self.display_col(row, col), lh)?;
        Some(Bounds::new(point(bounds.left() + p.x + self.row_origin_x(row), bounds.top() + self.line_tops.get(row).copied()? + p.y), size(px(1.), lh)))
    }
    pub fn set_format(&mut self, format: lyrel_text::TextFormat, cx: &mut Context<Self>) {
        self.finish_composition(cx);
        let document = self.document.clone(); let origin = cx.entity_id(); let caret = self.cursor_offset();
        document.update(cx, |document, cx| document.set_format(format, origin, caret, cx));
    }
    pub fn finish_composition(&mut self, cx: &mut Context<Self>) {
        let document = self.document.clone();
        let owner = document.read(cx).composition_owner();
        let Some(owner) = owner else { self.marked_range = None; return; };
        if owner.entity_id() != cx.entity_id() {
            if let Some(owner) = owner.upgrade() {
                owner.update(cx, |owner, cx| owner.finish_composition(cx));
            } else {
                self.content = document.update(cx, |document, cx| document.finish_orphaned_composition(cx)).text_handle();
                self.content_gen = document.read(cx).revision();
                self.invalidate_geometry();
            }
            return;
        }
        let origin = cx.entity_id(); let caret = self.cursor_offset();
        self.content = document.update(cx, |document, cx| document.finish_composition(origin, false, caret, cx)).text_handle();
        self.marked_range = None; self.last_edit = EditKind::Other;
    }
    pub(super) fn claim_composition(&mut self, cx: &mut Context<Self>) {
        let owner = self.document.read(cx).composition_owner();
        if owner.is_some_and(|owner| owner.entity_id() != cx.entity_id()) {
            self.finish_composition(cx);
        }
    }
    pub(super) fn replace_source(&mut self, range: Range<usize>, text: &str, cx: &mut Context<Self>) {
        self.claim_composition(cx);
        let document = self.document.clone();
        let origin = cx.entity_id(); let caret = self.cursor_offset();
        let before_selection = self.selected_range.clone();
        let after_caret = range.start + text.len() - text.matches("\r\n").count();
        let coalesced = self.pending_coalesce;
        let composing = self.marked_range.is_some() && document.read(cx).has_composition();
        let snapshot = if self.marked_range.is_some() && document.read(cx).has_composition() {
            let owner = cx.weak_entity();
            document.update(cx, |document, cx| {
                document.compose(range.clone(), text, owner, caret, cx);
                document.finish_composition(origin, false, range.start + text.len(), cx)
            })
        } else {
            let coalesce = self.pending_coalesce;
            document.update(cx, |document, cx| document.replace(range, text, origin, caret, coalesce, cx))
        };
        if !composing { document.update(cx, |document, _| document.set_history_selection(origin, before_selection, after_caret..after_caret, coalesced)); }
        self.content = snapshot.text_handle(); self.content_gen = document.read(cx).revision();
        self.pending_coalesce = false; self.invalidate_geometry();
    }
    pub(super) fn invalidate_geometry(&mut self) {
        self.revealed_caret = None;
        self.last_bounds = None;
        self.wrapped.clear(); self.line_tops.clear(); self.table_rows.clear(); self.offset_maps.clear(); self.rtl_rows.clear();
        *self.shape_memo.borrow_mut() = None;
        *self.scan_cache.borrow_mut() = None;
        self.utf16_anchor.set((self.content_gen, 0, 0));
    }
}
