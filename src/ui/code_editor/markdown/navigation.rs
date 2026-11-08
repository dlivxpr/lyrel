use super::*;

impl SourcePresentation {
    pub(super) fn geometry_is_current(&self, cx: &App) -> bool {
        self.geometry.as_ref().is_some_and(|(revision, _)| {
            *revision == self.document.read(cx).revision()
                && self.viewport.offset == self.scroll.offset()
                && self.viewport.bounds == Some(self.scroll.bounds())
        })
    }

    pub(super) fn selectable_leaf(&self, leaf: &super::super::projection::Leaf, cx: &App) -> bool {
        !self.live
            || (!self.folds.is_hidden(leaf.range.start)
                && super::super::mdx::view::visible_field(
                    &super::super::mdx::view::FieldContext::new(
                        &self.projection,
                        leaf,
                        self.document.read(cx).source(),
                        &self.mdx_state,
                    ),
                ))
    }

    pub(super) fn logical_line_edge(&self, at: usize, end: bool, cx: &App) -> usize {
        let document = self.document.read(cx);
        let row = document.source().offset_to_point(at).row;
        let raw = if end {
            document.source().line_end_offset(row)
        } else {
            document.source().line_start_offset(row)
        };
        if !self.live || self.projection.revision != document.revision() {
            return raw;
        }
        let Some(leaf) = self.projection.leaf_at(at) else {
            return raw;
        };
        let visible = leaf.source_offset(leaf.display_offset(raw));
        // Home is logical, including on RTL/wrapped lines. A second Home
        // exposes the gutter prefix, matching explicit source access.
        if !end && at == visible && raw < visible {
            raw
        } else {
            visible
        }
    }

    pub(super) fn visual_horizontal_target(
        &self,
        at: usize,
        backward: bool,
        cx: &App,
    ) -> Option<usize> {
        if !self.geometry_is_current(cx) {
            return None;
        }
        let leaf = self.projection.leaf_at(at)?;
        if leaf.deferred {
            return None;
        }
        let geometry = self.geometry.as_ref()?.1.get(&leaf.id)?;
        if geometry.caret_height() <= Pixels::ZERO {
            return None;
        }
        let display = leaf.display_offset(at);
        let caret = geometry.position_for_index(display);
        // Probe the current shaped row, then bisect its first caret transition.
        // The number of geometry queries depends on coordinate precision, not
        // the paragraph's grapheme count; atoms and bidi keep the same mapper.
        let direction = if backward { -1. } else { 1. };
        let y = caret.y + geometry.line_height() * 0.25;
        let bounds = geometry.bounds();
        let limit = if backward {
            bounds.left()
        } else {
            bounds.right()
        };
        let distance = (limit - caret.x).abs();
        let tolerance = geometry.caret_height() / 1024.;
        let candidate = |distance| {
            let ix = hit_test_grapheme(
                &leaf.text,
                geometry,
                point(caret.x + distance * direction, y),
            );
            let position = geometry.position_for_index(ix);
            let dx = (position.x - caret.x) * direction;
            (ix != display
                && dx > Pixels::ZERO
                && (position.y - caret.y).abs() < geometry.line_height() * 0.25)
                .then_some(ix)
        };
        let mut low = Pixels::ZERO;
        let mut high = (geometry.caret_height() * 0.25).min(distance);
        let mut next = candidate(high);
        while next.is_none() && high < distance {
            low = high;
            high = (high * 2.).min(distance);
            next = candidate(high);
        }
        if let Some(mut ix) = next {
            for _ in 0..24 {
                if high - low <= tolerance {
                    break;
                }
                let middle = (low + high) / 2.;
                if let Some(nearest) = candidate(middle) {
                    high = middle;
                    ix = nearest;
                } else {
                    low = middle;
                }
            }
            return Some(leaf.source_offset(ix));
        }
        // At a visual edge, ask the same row's endpoints for its source-order
        // direction rather than visiting every glyph to infer RTL/wrap exit.
        let left = hit_test_grapheme(&leaf.text, geometry, point(bounds.left(), y));
        let right = hit_test_grapheme(&leaf.text, geometry, point(bounds.right(), y));
        (left > right).then(|| self.visible_step(at, !backward, cx))
    }

    pub(super) fn visual_selection_edge(&self, backward: bool, cx: &App) -> usize {
        let selection = self.state.selection;
        if let (Some(a), Some(b)) = (
            self.caret_bounds(selection.anchor, cx),
            self.caret_bounds(selection.head, cx),
        ) {
            if (a.top() - b.top()).abs() < a.size.height.min(b.size.height) * 0.5 {
                return if (a.left() < b.left()) == backward {
                    selection.anchor
                } else {
                    selection.head
                };
            }
        }
        if backward {
            selection.range().start
        } else {
            selection.range().end
        }
    }

    pub(super) fn visible_word_target(&self, at: usize, backward: bool, cx: &App) -> usize {
        let document = self.document.read(cx);
        if !self.live
            || self.projection.revision != document.revision()
            || self.projection.leaf_at(at).is_none_or(|leaf| leaf.deferred)
        {
            return source_word_target(document, at, backward);
        }
        let leaf = self
            .projection
            .leaf_at(at)
            .expect("current materialized leaf");
        let display = leaf.display_offset(at);
        if let Some(adjacent) = adjacent_grapheme(&leaf.text, display, backward) {
            let source = leaf.visible_range(adjacent.clone());
            if source.len() != adjacent.len() || leaf.text[adjacent].contains('\u{fffc}') {
                return if backward { source.start } else { source.end };
            }
        }
        // Start at the caret in the requested direction. Do not scan the other
        // half of a long logical line just to find its containing row first.
        let target = if backward {
            leaf.text[..display]
                .unicode_word_indices()
                .next_back()
                .map(|(ix, _)| ix)
                .filter(|ix| !leaf.text[*ix..display].contains('\n'))
        } else {
            leaf.text[display..]
                .unicode_word_indices()
                .next()
                .map(|(ix, word)| display + ix + word.len())
                .filter(|end| !leaf.text[display..*end].contains('\n'))
        };
        let atom = leaf
            .word_atom_target(display, backward)
            .filter(|target| !leaf.text[display.min(*target)..display.max(*target)].contains('\n'));
        let target = match (target, atom) {
            (Some(word), Some(atom)) => Some(if backward {
                word.max(atom)
            } else {
                word.min(atom)
            }),
            (word, atom) => word.or(atom),
        };
        target.map_or_else(
            || self.visible_step(at, backward, cx),
            |target| leaf.source_offset(target),
        )
    }

    pub(super) fn visible_delete_range(&self, at: usize, backward: bool, cx: &App) -> Range<usize> {
        let document = self.document.read(cx);
        if self.live && self.projection.revision == document.revision() {
            if let Some(leaf) = self.projection.leaf_at(at).filter(|leaf| !leaf.deferred) {
                let display = leaf.display_offset(at);
                if let Some(range) = adjacent_grapheme(&leaf.text, display, backward) {
                    return leaf.visible_range(range);
                }
                if matches!(leaf.kind, Kind::TableCell(_)) {
                    return at..at;
                }
            }
        }
        if backward {
            document.previous(at)..at
        } else {
            at..document.next(at)
        }
    }

    pub(super) fn deletion_ranges(
        &self,
        range: Range<usize>,
        cx: &App,
    ) -> smallvec::SmallVec<[Range<usize>; 4]> {
        if !self.live
            || self.projection.revision != self.document.read(cx).revision()
            || range.is_empty()
        {
            return smallvec::smallvec![range];
        }
        let mut ranges = smallvec::smallvec![range.clone()];
        // A partially selected table retains its structural pipes, padding and
        // separator row. Selecting the complete table still deletes the object.
        let mut table_end = 0;
        let first = self
            .projection
            .leaves
            .partition_point(|leaf| leaf.range.end < range.start)
            .saturating_sub(1);
        for leaf in self.projection.leaves[first..]
            .iter()
            .take_while(|leaf| leaf.range.start <= range.end)
        {
            let Kind::TableCell(cell) = leaf.kind else {
                continue;
            };
            let table = cell.table();
            if table.start < table_end || table.end <= range.start || table.start >= range.end {
                continue;
            }
            table_end = table.end;
            if range.start <= table.start && table.end <= range.end {
                continue;
            }
            let mut protected = table.start;
            let first_cell = self
                .projection
                .leaves
                .partition_point(|leaf| leaf.range.start < table.start);
            for cell_leaf in self.projection.leaves[first_cell..]
                .iter()
                .take_while(|leaf| leaf.range.start <= table.end)
                .filter(|leaf| matches!(leaf.kind, Kind::TableCell(cell) if cell.table() == table))
            {
                let start = cell_leaf.content_start();
                let end = cell_leaf.content_end();
                subtract(&mut ranges, protected..start);
                protected = end;
            }
            subtract(&mut ranges, protected..table.end);
        }
        let mut repairs = smallvec::SmallVec::<[Range<usize>; 4]>::new();
        for range in &ranges {
            let first = self
                .projection
                .leaves
                .partition_point(|leaf| leaf.range.end < range.start);
            for leaf in self.projection.leaves[first..]
                .iter()
                .take_while(|leaf| leaf.range.start <= range.end)
            {
                repairs.extend(leaf.deletion_delimiters(range));
            }
        }
        ranges.extend(repairs);
        for hidden in &self.folds.hidden {
            subtract(&mut ranges, hidden.start.saturating_sub(1)..hidden.end);
        }
        ranges.retain(|range| range.start < range.end);
        ranges.sort_by_key(|range| range.start);
        let mut merged = smallvec::SmallVec::<[Range<usize>; 4]>::new();
        for range in ranges {
            if let Some(last) = merged.last_mut().filter(|last| range.start <= last.end) {
                last.end = last.end.max(range.end);
            } else {
                merged.push(range);
            }
        }
        merged
    }

    pub(super) fn deletion_command(&self, range: Range<usize>, cx: &App) -> Command {
        let at = range.start;
        let mut ranges = self.deletion_ranges(range, cx);
        if ranges.is_empty() {
            return Command::Select(Selection::caret(at));
        }
        if ranges.len() == 1 {
            return Command::Replace {
                range: ranges.pop().unwrap(),
                text: String::new(),
                kind: EditKind::Command,
            };
        }
        let removed_before: usize = ranges
            .iter()
            .map(|range| at.min(range.end).saturating_sub(range.start))
            .sum();
        Command::BatchReplaceSelected {
            kind: EditKind::Command,
            edits: ranges
                .into_iter()
                .map(|range| (range, String::new()))
                .collect(),
            selection: Selection::caret(at - removed_before),
        }
    }

    pub(super) fn adjacent_block_atom(
        &self,
        at: usize,
        backward: bool,
        cx: &App,
    ) -> Option<Range<usize>> {
        let document = self.document.read(cx);
        let source = document.source();
        let current = self.projection.leaf_at(at)?;
        if current.block_atom_range().is_none()
            && (backward && at > current.content_start() || !backward && at < current.content_end())
        {
            return None;
        }
        for leaf in &self.projection.leaves {
            let Some(mut range) = leaf.block_atom_range() else {
                continue;
            };
            // Inline objects adjacent to text remain inline. A block atom must
            // own its logical source line before its newline can be removed.
            let start = source.line_start_offset(source.offset_to_point(range.start).row);
            let end = source.line_end_offset(source.offset_to_point(range.end).row);
            if !source
                .slice(start..range.start)
                .chars()
                .all(char::is_whitespace)
                || !source
                    .slice(range.end..end)
                    .chars()
                    .all(char::is_whitespace)
            {
                continue;
            }
            let same = leaf.id == current.id;
            let adjacent = if backward {
                range.end <= at
                    && source.slice(range.end..at).chars().all(char::is_whitespace)
                    && at <= current.content_start()
            } else {
                at <= range.start
                    && source
                        .slice(at..range.start)
                        .chars()
                        .all(char::is_whitespace)
                    && at >= current.content_end()
            };
            if same && !(range.start < at && at < range.end) || adjacent {
                range.start = start;
                range.end = end;
                if source.slice(end..source.len()).chars().next() == Some('\n') {
                    range.end += 1;
                } else if start > 0 && source.slice(start - 1..start).chars().next() == Some('\n') {
                    range.start -= 1;
                }
                return Some(range);
            }
        }
        None
    }

    pub(super) fn vertical_fallback(&self, at: usize, backward: bool, cx: &App) -> usize {
        let document = self.document.read(cx);
        if !self.live {
            let point = document.source().offset_to_point(at);
            let last = document
                .source()
                .offset_to_point(document.source().len())
                .row;
            let row = if backward {
                point.row.saturating_sub(1)
            } else {
                (point.row + 1).min(last)
            };
            return document.boundary(
                (document.source().line_start_offset(row) + point.column as usize)
                    .min(document.source().line_end_offset(row)),
                false,
            );
        }
        let Some(current) = self.projection.leaf_at(at) else {
            return at;
        };
        let mut candidates = self
            .projection
            .leaves
            .iter()
            .filter(|leaf| self.selectable_leaf(leaf, cx));
        if backward {
            candidates
                .rfind(|leaf| leaf.range.end < current.range.start)
                .map_or(0, |leaf| leaf.content_end())
        } else {
            candidates
                .find(|leaf| leaf.range.start > current.range.end)
                .map_or(document.source().len(), |leaf| leaf.content_start())
        }
    }

    pub(super) fn pointer_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.drag_pointer = Some(event.position);
        self.drag_origin = None;
        if !event.modifiers.shift && event.click_count >= 2 {
            if let Some(at) = self.point_offset(event.position, cx) {
                let range = self.pointer_unit(at, event.click_count, cx);
                let selection = Selection::new(range.start, range.end);
                self.command(Command::Select(selection), window, cx);
                self.drag_origin = Some((selection, event.click_count));
            }
        } else {
            self.select_point(event.position, event.modifiers.shift, window, cx);
        }
        self.reveal_caret = false;
    }

    fn pointer_unit(&self, at: usize, clicks: usize, cx: &App) -> Range<usize> {
        let document = self.document.read(cx);
        if clicks >= 3 {
            let row = document.source().offset_to_point(at).row;
            let start = document.source().line_start_offset(row);
            let end = document.source().line_end_offset(row);
            return start..document.next(end);
        }
        if let Some(leaf) = self.projection.leaf_at(at).filter(|leaf| !leaf.deferred) {
            let display = leaf.display_offset(at);
            if let Some((ix, word)) = leaf
                .text
                .split_word_bound_indices()
                .find(|(ix, word)| *ix <= display && display < ix + word.len())
            {
                return leaf.visible_range(ix..ix + word.len());
            }
        }
        at..document.next(at)
    }

    pub(super) fn drag_to(
        &mut self,
        point: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.drag_pointer = Some(point);
        if let Some((origin, clicks)) = self.drag_origin {
            if let Some(at) = self.point_offset(point, cx) {
                let target = self.pointer_unit(at, clicks, cx);
                let selection = if at < origin.anchor {
                    Selection::new(origin.head, target.start)
                } else {
                    Selection::new(origin.anchor, target.end)
                };
                self.command(Command::Select(selection), window, cx);
            }
        } else {
            self.select_point(point, true, window, cx);
        }
        self.reveal_caret = false;
        self.schedule_drag_frame(window, cx);
    }

    pub(super) fn cancel_drag(&mut self) {
        self.dragging = false;
        self.drag_pointer = None;
        self.drag_origin = None;
        // A queued frame still owns drag_frame_pending until it is delivered.
        // Clearing it here could enqueue a second loop for a new gesture.
    }

    pub(super) fn schedule_drag_frame(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.drag_frame_pending || (!self.dragging && self.reorder.is_none()) {
            return;
        }
        let Some(pointer) = self.drag_pointer else {
            return;
        };
        let bounds = self.scroll.bounds();
        let margin = window.rem_size();
        if pointer.y >= bounds.top() + margin
            && pointer.y <= bounds.bottom() - margin
            && pointer.x >= bounds.left()
            && pointer.x <= bounds.right()
        {
            return;
        }
        self.drag_frame_pending = true;
        let view = cx.entity().downgrade();
        // on_next_frame creates frame demand without reading current_view().
        // This path runs from input dispatch and frame callbacks, not drawing;
        // the callback notifies this retained entity when scrolling changes.
        window.on_next_frame(move |window, cx| {
            let Some(view) = view.upgrade() else { return };
            view.update(cx, |view, cx| {
                view.drag_frame_pending = false;
                if (!view.dragging && view.reorder.is_none()) || !view.focus.is_focused(window) {
                    view.cancel_drag();
                    view.cancel_reorder(cx);
                    return;
                }
                let Some(pointer) = view.drag_pointer else {
                    return;
                };
                if view.geometry_is_current(cx) {
                    // Hit testing consumes the geometry snapshot collected in
                    // SourceElement::prepaint, never event-phase measurement.
                    // This callback alone decides whether another frame is
                    // needed; reaching the scroll limit must stop scheduling.
                    view.drag_frame_pending = true;
                    if view.reorder.is_some() {
                        view.move_reorder(pointer, window, cx);
                    } else {
                        view.drag_to(pointer, window, cx);
                    }
                    view.drag_frame_pending = false;
                }
                let bounds = view.scroll.bounds();
                let margin = window.rem_size();
                let speed = |at: Pixels, low: Pixels, high: Pixels| {
                    if at < low {
                        (low - at).min(margin * 4.)
                    } else if at > high {
                        -(at - high).min(margin * 4.)
                    } else {
                        Pixels::ZERO
                    }
                };
                let offset = view.scroll.offset();
                let maximum = view.scroll.max_offset();
                let next = point(
                    (offset.x + speed(pointer.x, bounds.left(), bounds.right()))
                        .clamp(-maximum.x, Pixels::ZERO),
                    (offset.y + speed(pointer.y, bounds.top() + margin, bounds.bottom() - margin))
                        .clamp(-maximum.y, Pixels::ZERO),
                );
                if next != offset {
                    view.scroll.set_offset(next);
                    view.viewport.anchor = None;
                    view.geometry = None;
                    cx.notify();
                    view.schedule_drag_frame(window, cx);
                }
            });
        });
    }
}

fn subtract(ranges: &mut smallvec::SmallVec<[Range<usize>; 4]>, protected: Range<usize>) {
    if protected.is_empty() {
        return;
    }
    let mut ix = 0;
    while ix < ranges.len() {
        let range = ranges[ix].clone();
        if range.end <= protected.start || protected.end <= range.start {
            ix += 1;
            continue;
        }
        ranges.remove(ix);
        if range.start < protected.start {
            ranges.insert(ix, range.start..protected.start);
            ix += 1;
        }
        if protected.end < range.end {
            ranges.insert(ix, protected.end..range.end);
            ix += 1;
        }
    }
}

pub(super) fn hit_test_grapheme(text: &str, geometry: &LeafGeometry, at: Point<Pixels>) -> usize {
    let index = floor_grapheme(text, geometry.index_for_position(at));
    // Whole-leaf hit testing can round past the next glyph at a grapheme edge.
    // Check adjacent carets in the same shaped layout without walking the leaf.
    std::iter::once(index)
        .chain(adjacent_grapheme(text, index, true).map(|range| range.start))
        .chain(adjacent_grapheme(text, index, false).map(|range| range.end))
        .map(|index| {
            let caret = geometry.position_for_index(index);
            let dy = if at.y < caret.y {
                caret.y - at.y
            } else {
                (at.y - caret.y - geometry.line_height()).max(Pixels::ZERO)
            };
            (index, (dy, (at.x - caret.x).abs()))
        })
        .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
        .map_or(index, |(index, _)| index)
}

fn floor_grapheme(text: &str, at: usize) -> usize {
    let at = text.floor_char_boundary(at.min(text.len()));
    let mut cursor = unicode_segmentation::GraphemeCursor::new(at, text.len(), true);
    if cursor
        .is_boundary(text, 0)
        .expect("complete leaf supplies grapheme context")
    {
        at
    } else {
        cursor
            .prev_boundary(text, 0)
            .expect("complete leaf supplies grapheme context")
            .unwrap_or(0)
    }
}

pub(super) fn adjacent_grapheme(text: &str, at: usize, backward: bool) -> Option<Range<usize>> {
    let at = floor_grapheme(text, at);
    let mut cursor = unicode_segmentation::GraphemeCursor::new(at, text.len(), true);
    if backward {
        cursor
            .prev_boundary(text, 0)
            .expect("complete leaf supplies grapheme context")
            .map(|start| start..at)
    } else {
        cursor
            .next_boundary(text, 0)
            .expect("complete leaf supplies grapheme context")
            .map(|end| at..end)
    }
}

fn source_word_target(document: &MarkdownDocument, at: usize, backward: bool) -> usize {
    let source = document.source();
    let row = source.offset_to_point(at).row;
    let row_start = source.line_start_offset(row);
    let row_end = source.line_end_offset(row);
    // ASCII space/tab are definite UAX word breaks. Keep complete intervening
    // context, including apostrophes and arbitrary Extend/Format sequences;
    // never infer words from alphanumeric classes or truncate at a byte budget.
    let separator = |ch: char| matches!(ch, ' ' | '\t');
    let mut start = at;
    let mut seen = false;
    for ch in source.chars_at(at).reversed() {
        if start <= row_start {
            break;
        }
        if separator(ch) && (seen || !backward) {
            start -= ch.len_utf8();
            break;
        }
        start -= ch.len_utf8();
        seen |= !separator(ch);
    }
    let mut end = at;
    let mut seen = false;
    for ch in source.chars_at(at) {
        if end >= row_end {
            break;
        }
        if separator(ch) && (seen || backward) {
            end += ch.len_utf8();
            break;
        }
        end += ch.len_utf8();
        seen |= !separator(ch);
    }
    let slice = source.slice(start..end);
    let owned;
    let text = if let Some(text) = slice.as_str() {
        text
    } else {
        // A context crossing Rope chunks must remain contiguous for the public
        // Unicode iterator. A pathological unbroken token can still require the
        // legacy full context; correctness takes precedence over truncation.
        owned = slice.to_string();
        &owned
    };
    if backward {
        text.unicode_word_indices()
            .rev()
            .find(|(ix, _)| start + ix < at)
            .map_or_else(|| document.previous(row_start), |(ix, _)| start + ix)
    } else {
        text.unicode_word_indices()
            .find(|(ix, word)| start + ix + word.len() > at)
            .map_or_else(
                || document.next(row_end),
                |(ix, word)| start + ix + word.len(),
            )
    }
}
