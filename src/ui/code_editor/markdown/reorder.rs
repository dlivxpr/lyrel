//! Gutter reordering moves existing source fragments through the shared history.
//! There is no block model or cross-presentation drag payload.
use super::*;
use gpui_kit::component::{
    Sizable as _,
    button::{Button, ButtonVariants as _},
};
use markdown::mdast::Node;

#[cfg(test)]
#[path = "reorder/tests.rs"]
mod tests;

actions!(
    markdown_reorder,
    [
        PreviousDestination,
        NextDestination,
        FirstDestination,
        LastDestination,
        CommitReorder,
        CancelReorder
    ]
);
struct Bindings;
impl Global for Bindings {}
pub(super) fn init(cx: &mut App) {
    if cx.has_global::<Bindings>() {
        return;
    }
    cx.set_global(Bindings);
    cx.bind_keys([
        KeyBinding::new("up", PreviousDestination, Some("MarkdownReorder")),
        KeyBinding::new("down", NextDestination, Some("MarkdownReorder")),
        KeyBinding::new("home", FirstDestination, Some("MarkdownReorder")),
        KeyBinding::new("end", LastDestination, Some("MarkdownReorder")),
        KeyBinding::new("enter", CommitReorder, Some("MarkdownReorder")),
        KeyBinding::new("escape", CancelReorder, Some("MarkdownReorder")),
    ]);
}

struct Row {
    range: Range<usize>,
    indent: usize,
    quoted: bool,
    list: bool,
    blank: bool,
}
struct Inventory {
    rows: Vec<Row>,
    opaque: Vec<Range<usize>>,
    length: usize,
}
impl Inventory {
    fn new(source: &str, root: &Node, properties: Vec<Range<usize>>) -> Self {
        let mut rows = Vec::new();
        let mut start = 0;
        for line in source.split_inclusive('\n') {
            let text = line.trim_end_matches('\n');
            let content = text.trim_start_matches([' ', '\t']);
            let indent = text.len() - content.len();
            let digits = content.bytes().take_while(u8::is_ascii_digit).count();
            let list = ["- ", "+ ", "* "]
                .iter()
                .any(|prefix| content.starts_with(prefix))
                || digits > 0
                    && matches!(content.as_bytes().get(digits), Some(b'.' | b')'))
                    && content.as_bytes().get(digits + 1) == Some(&b' ');
            rows.push(Row {
                range: start..start + line.len(),
                indent,
                quoted: content.starts_with('>'),
                list,
                blank: content.is_empty(),
            });
            start += line.len();
        }
        let mut inventory = Self {
            rows,
            opaque: properties,
            length: source.len(),
        };
        inventory.collect(source, root);
        inventory
            .opaque
            .sort_by_key(|range| (range.start, std::cmp::Reverse(range.end)));
        let mut merged: Vec<Range<usize>> = Vec::new();
        for range in inventory.opaque.drain(..) {
            if let Some(previous) = merged
                .last_mut()
                .filter(|previous| range.start < previous.end)
            {
                previous.end = previous.end.max(range.end);
            } else {
                merged.push(range);
            }
        }
        inventory.opaque = merged;
        inventory
    }
    fn collect(&mut self, source: &str, node: &Node) {
        if matches!(
            node,
            Node::Code(_)
                | Node::Math(_)
                | Node::Table(_)
                | Node::Html(_)
                | Node::Yaml(_)
                | Node::Toml(_)
                | Node::MdxFlowExpression(_)
                | Node::MdxJsxFlowElement(_)
                | Node::MdxjsEsm(_)
        ) {
            if let Some(position) = node.position() {
                let mut start = super::super::blocks::line_start(source, position.start.offset);
                let end = super::super::blocks::line_end(source, position.end.offset);
                let end = end + usize::from(end < source.len());
                if matches!(node, Node::Table(_) | Node::Math(_)) && start > 0 {
                    let previous = super::super::blocks::line_start(source, start - 1);
                    let line = source[previous..start].trim();
                    if line.starts_with("<!-- table:") && matches!(node, Node::Table(_))
                        || line.starts_with("<!-- math:") && matches!(node, Node::Math(_))
                    {
                        start = previous;
                    }
                }
                self.opaque.push(start..end);
            }
            return;
        }
        if let Some(children) = node.children() {
            for child in children {
                self.collect(source, child);
            }
        }
    }
    fn row(&self, at: usize) -> Option<usize> {
        let ix = self
            .rows
            .partition_point(|row| row.range.start <= at)
            .checked_sub(1)?;
        self.rows
            .get(ix)
            .filter(|row| at < row.range.end)
            .map(|_| ix)
    }
    fn scope(&self, at: usize) -> Option<Range<usize>> {
        let ix = self.row(at)?;
        if let Some(range) = self.opaque.iter().find(|range| range.contains(&at)) {
            return Some(range.clone());
        }
        let row = &self.rows[ix];
        if row.quoted {
            let mut first = ix;
            let mut last = ix + 1;
            while first > 0 && self.rows[first - 1].quoted {
                first -= 1;
            }
            while last < self.rows.len() && self.rows[last].quoted {
                last += 1;
            }
            return Some(self.rows[first].range.start..self.rows[last - 1].range.end);
        }
        let mut last = ix + 1;
        if row.list {
            while last < self.rows.len()
                && !self.rows[last].blank
                && self.rows[last].indent > row.indent
            {
                if let Some(opaque) = self
                    .opaque
                    .iter()
                    .find(|range| range.contains(&self.rows[last].range.start))
                {
                    last = self
                        .rows
                        .partition_point(|row| row.range.start < opaque.end);
                } else {
                    last += 1;
                }
            }
        }
        Some(row.range.start..self.rows[last - 1].range.end)
    }
    fn snap(&self, mut at: usize, folds: &[Range<usize>]) -> usize {
        for range in self.opaque.iter().chain(folds) {
            if range.start < at && at < range.end {
                at = if at - range.start <= range.end - at {
                    range.start
                } else {
                    range.end
                };
            }
        }
        at
    }
}

pub(super) struct Gesture {
    inventory: Inventory,
    source: Range<usize>,
    folds: Vec<Range<usize>>,
    token: (EntityId, u64, u64, u64, Option<EntityId>),
    resources: Option<crate::ui::code_editor::markdown::resources::ResourceContext>,
    target: Option<usize>,
    pointer: Option<Point<Pixels>>,
    indicator: Option<Pixels>,
    _blur: Subscription,
}

impl SourcePresentation {
    pub(super) fn reorder_grips(
        &self,
        leaf: &super::super::projection::Leaf,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        if !self.live
            || self.is_composing()
            || self.projection.revision != self.document.read(cx).revision()
            || matches!(
                leaf.kind,
                Kind::PendingSource | Kind::Mdx(_) | Kind::Metadata
            )
        {
            return Vec::new();
        }
        let rope = self.document.read(cx).source().clone();
        let first = rope.offset_to_point(leaf.range.start).row;
        let last = rope
            .offset_to_point(leaf.range.end.saturating_sub(1).max(leaf.range.start))
            .row;
        let single = matches!(
            leaf.kind,
            Kind::Code(_) | Kind::Mermaid(_) | Kind::Math | Kind::Html | Kind::TableCell(_)
        );
        let mut grips = Vec::new();
        for row in first..=if single { first } else { last } {
            let at = rope.line_start_offset(row);
            if self.folds.is_hidden(at) {
                continue;
            }
            let y = if row == first {
                Pixels::ZERO
            } else {
                let Some(bounds) = self.caret_bounds(at, cx) else {
                    continue;
                };
                let Some((_, geometry)) = &self.geometry else {
                    continue;
                };
                let Some(first) = geometry.get(&leaf.id) else {
                    continue;
                };
                bounds.top() - first.bounds().top()
            };
            let id: ElementId = if row == first {
                ("markdown-reorder-grip", leaf.id).into()
            } else {
                (SharedString::from(format!("markdown-reorder-grip-row-{}", leaf.id)), row - first).into()
            };
            grips.push(
                Button::new(id)
                    .small()
                    .ghost()
                    .label("⠿")
                    .tooltip(t(L10nKey::MarkdownMoveContent))
                    .accessibility_label(t(L10nKey::MarkdownMoveContent))
                    .absolute()
                    .left(rems(-1.75))
                    .top(y)
                    .w_6()
                    .h_6()
                    .cursor_grab()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |view, event: &MouseDownEvent, window, cx| {
                            view.begin_reorder(at, Some(event.position), window, cx);
                            window.prevent_default();
                            cx.stop_propagation();
                        }),
                    )
                    .on_click(cx.listener(move |view, event, window, cx| {
                        if !matches!(event, ClickEvent::Mouse(_)) {
                            view.begin_reorder(at, None, window, cx);
                        }
                    }))
                    .into_any_element(),
            );
        }
        grips
    }
    fn begin_reorder(
        &mut self,
        at: usize,
        pointer: Option<Point<Pixels>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.reorder = None;
        if !self.live
            || self.is_composing()
            || self.projection.revision != self.document.read(cx).revision()
        {
            return;
        }
        let document = self.document.read(cx);
        let source = document.source().to_string();
        let Ok(root) = super::super::parser::parse(&source, document.grammar(), true) else {
            return;
        };
        let inventory = Inventory::new(&source, &root, self.projection.properties.reorder_ranges());
        let Some(mut range) = inventory.scope(at) else {
            return;
        };
        let folds: Vec<_> = self
            .projection
            .block_index
            .folds
            .iter()
            .filter(|fold| {
                self.folds
                    .hidden
                    .iter()
                    .any(|hidden| hidden.start <= fold.body.start && fold.body.end <= hidden.end)
            })
            .map(|fold| {
                let end = super::super::blocks::line_end(&source, fold.body.end.saturating_sub(1));
                fold.header.start..end + usize::from(end < source.len())
            })
            .collect();
        for fold in &folds {
            if fold.start == range.start {
                range.end = range.end.max(fold.end);
            }
        }
        self.dragging = false;
        self.drag_origin = None;
        self.drag_pointer = None;
        self.pointer_link = None;
        self.reveal_caret = false;
        self.focus.focus(window, cx);
        let blur = cx.on_blur(&self.focus, window, |view, _, cx| {
            view.cancel_reorder(cx);
        });
        self.reorder = Some(Gesture {
            inventory,
            source: range.clone(),
            folds,
            token: self.input_token(cx),
            resources: self.document.read(cx).resources().cloned(),
            target: Some(range.start),
            pointer,
            indicator: None,
            _blur: blur,
        });
        cx.notify();
    }
    fn reorder_is_current(&self, window: &Window, cx: &App) -> bool {
        self.reorder.as_ref().is_some_and(|gesture| {
            self.live
                && self.focus.is_focused(window)
                && gesture.token == self.input_token(cx)
                && match &gesture.resources {
                    Some(resources) => resources.is_current(self.document.read(cx).resources()),
                    None => self.document.read(cx).resources().is_none(),
                }
        })
    }
    pub(super) fn cancel_reorder(&mut self, cx: &mut Context<Self>) -> bool {
        if self.reorder.take().is_none() {
            return false;
        }
        self.drag_pointer = None;
        cx.notify();
        true
    }
    fn destination_y(&self, at: usize, cx: &App) -> Option<Pixels> {
        let gesture = self.reorder.as_ref()?;
        let eof = at == gesture.inventory.length;
        let at = if eof {
            gesture
                .folds
                .iter()
                .find(|fold| fold.end == at)
                .map_or(at, |fold| fold.start)
        } else {
            at
        };
        let leaf = self.projection.leaf_at(at)?;
        let bounds = self
            .viewport
            .frame
            .iter()
            .find(|(id, _, _)| *id == leaf.id)
            .map(|(_, _, bounds)| bounds);
        if eof {
            return bounds.map(|bounds| bounds.bottom());
        }
        if at == leaf.range.start {
            return bounds.map(|bounds| bounds.top());
        }
        if at >= leaf.range.end {
            return bounds.map(|bounds| bounds.bottom());
        }
        self.caret_bounds(at, cx).map(|bounds| bounds.top())
    }
    pub(super) fn move_reorder(
        &mut self,
        pointer: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.reorder.is_none() {
            return false;
        }
        if !self.reorder_is_current(window, cx) {
            self.cancel_reorder(cx);
            return true;
        }
        self.reorder.as_mut().unwrap().pointer = Some(pointer);
        self.drag_pointer = Some(pointer);
        if !self.geometry_is_current(cx) {
            let gesture = self.reorder.as_mut().unwrap();
            let changed = gesture.target.is_some() || gesture.indicator.is_some();
            gesture.target = None;
            gesture.indicator = None;
            if changed {
                cx.notify();
            }
            self.schedule_drag_frame(window, cx);
            return true;
        }
        let viewport = self.scroll.bounds();
        let mut target = None;
        if viewport.contains(&pointer) {
            let gesture = self.reorder.as_ref().unwrap();
            let mut nearest = None;
            // Restrict row measurement to the committed visible leaves. A long
            // document does not become a full-document hit-test on each move.
            for leaf in &self.projection.leaves[self.viewport.visible.clone()] {
                let Some((_, _, block_bounds)) =
                    self.viewport.frame.iter().find(|(id, _, _)| *id == leaf.id)
                else {
                    continue;
                };
                let range = &leaf.range;
                let first = gesture
                    .inventory
                    .rows
                    .partition_point(|row| row.range.end <= range.start);
                for row in gesture.inventory.rows[first..]
                    .iter()
                    .take_while(|row| row.range.start < range.end)
                {
                    if self.folds.is_hidden(row.range.start) {
                        continue;
                    }
                    let Some(bounds) = self.caret_bounds(row.range.start, cx) else {
                        continue;
                    };
                    let distance = (pointer.y - bounds.center().y).abs();
                    let after = gesture
                        .folds
                        .iter()
                        .find(|fold| fold.start == row.range.start)
                        .map_or(row.range.end, |fold| fold.end);
                    let at = if pointer.y < bounds.center().y {
                        row.range.start
                    } else {
                        after
                    };
                    if nearest.as_ref().is_none_or(|(best, _)| distance < *best) {
                        nearest = Some((distance, at));
                    }
                }
                // Rendered opaque leaves may have no caret for their fence.
                let distance = (pointer.y - block_bounds.center().y).abs();
                let last = gesture.inventory.row(range.end.saturating_sub(1));
                if let (Some(first), Some(last)) = (
                    gesture.inventory.rows.get(first),
                    last.and_then(|ix| gesture.inventory.rows.get(ix)),
                ) {
                    let after = gesture
                        .folds
                        .iter()
                        .find(|fold| fold.start == first.range.start)
                        .map_or(last.range.end, |fold| fold.end);
                    let at = if pointer.y < block_bounds.center().y {
                        first.range.start
                    } else {
                        after
                    };
                    if nearest.as_ref().is_none_or(|(best, _)| distance < *best) {
                        nearest = Some((distance, at));
                    }
                }
            }
            target = nearest
                .map(|(_, at)| gesture.inventory.snap(at, &gesture.folds))
                .filter(|at| *at < gesture.source.start || *at > gesture.source.end);
        }
        let indicator = target.and_then(|at| self.destination_y(at, cx));
        let gesture = self.reorder.as_mut().unwrap();
        let changed = gesture.target != target || gesture.indicator != indicator;
        gesture.target = target;
        gesture.indicator = indicator;
        if changed {
            cx.notify();
        }
        self.schedule_drag_frame(window, cx);
        true
    }
    pub(super) fn finish_reorder(
        &mut self,
        pointer: Option<Point<Pixels>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.reorder.is_none() {
            return false;
        }
        if let Some(pointer) = pointer {
            self.move_reorder(pointer, window, cx);
        }
        let current = self.reorder_is_current(window, cx);
        let Some(gesture) = self.reorder.take() else {
            return true;
        };
        self.drag_pointer = None;
        if !current {
            cx.notify();
            return true;
        }
        let Some(target) = gesture
            .target
            .filter(|at| *at < gesture.source.start || *at > gesture.source.end)
        else {
            cx.notify();
            return true;
        };
        let moved = gesture.source;
        let rope = self.document.read(cx).source();
        let terminated =
            |end: usize| end > 0 && rope.slice(end - 1..end).chars().next() == Some('\n');
        let (range, lines, caret, trailing_newline) = if target < moved.start {
            (
                target..moved.end,
                vec![moved.clone(), target..moved.start],
                target,
                terminated(moved.start),
            )
        } else {
            // An unterminated EOF row becomes an internal row: add only its
            // necessary separator, never strip a moved fragment's terminator.
            let separator = usize::from(!terminated(target));
            (
                moved.start..target,
                vec![moved.end..target, moved.clone()],
                target - moved.len() + separator,
                terminated(moved.end),
            )
        };
        self.extra_selections.clear();
        self.command(
            Command::ReplaceLines {
                range,
                lines,
                leading_newline: false,
                trailing_newline,
                selection: Selection::caret(caret),
            },
            window,
            cx,
        );
        self.focus.focus(window, cx);
        cx.notify();
        true
    }
    fn step_reorder(
        &mut self,
        direction: i8,
        edge: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.reorder_is_current(window, cx) {
            self.cancel_reorder(cx);
            return;
        }
        let gesture = self.reorder.as_ref().unwrap();
        let current = gesture.target.unwrap_or(gesture.source.start);
        let mut candidates = gesture
            .inventory
            .rows
            .iter()
            .map(|row| row.range.start)
            .chain(std::iter::once(gesture.inventory.length))
            .map(|at| gesture.inventory.snap(at, &gesture.folds))
            .filter(|at| *at < gesture.source.start || *at > gesture.source.end);
        let target = if edge {
            if direction < 0 {
                candidates.next()
            } else {
                candidates.last()
            }
        } else if direction < 0 {
            candidates.filter(|at| *at < current).last()
        } else {
            candidates.find(|at| *at > current)
        };
        if let Some(target) = target {
            let indicator = self.destination_y(target, cx);
            let gesture = self.reorder.as_mut().unwrap();
            gesture.target = Some(target);
            gesture.indicator = indicator;
            self.reveal_at = Some(target);
            self.reveal_caret = true;
            cx.notify();
        }
    }
    pub(super) fn refresh_reorder_geometry(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(gesture) = &self.reorder else {
            return;
        };
        if let Some(pointer) = gesture.pointer {
            self.move_reorder(pointer, window, cx);
        } else if let Some(target) = gesture.target {
            let indicator = if target < gesture.source.start || target > gesture.source.end {
                self.destination_y(target, cx)
            } else {
                None
            };
            if self.reorder.as_ref().unwrap().indicator != indicator {
                self.reorder.as_mut().unwrap().indicator = indicator;
                cx.notify();
            }
        }
    }
    pub(super) fn reorder_indicator(&self, window: &Window, cx: &App) -> Option<AnyElement> {
        if !self.reorder_is_current(window, cx) || !self.geometry_is_current(cx) {
            return None;
        }
        let y = self.reorder.as_ref()?.indicator?;
        Some(
            div()
                .id("markdown-reorder-indicator")
                .test_support()
                .role(Role::Status)
                .aria_label(t(L10nKey::MarkdownMoveDestination))
                .absolute()
                .left_0()
                .right_0()
                .top(y - self.scroll.bounds().top() - self.scroll.offset().y)
                .h(rems(0.125))
                .bg(cx.theme().drag_border)
                .into_any_element(),
        )
    }
    pub(super) fn reorder_status(&self, cx: &App) -> Option<AnyElement> {
        self.reorder.as_ref()?;
        Some(
            div()
                .id("markdown-reorder-status")
                .test_support()
                .role(Role::Status)
                .absolute()
                .bottom_0()
                .left_0()
                .right_0()
                .p_2()
                .text_sm()
                .bg(cx.theme().background)
                .text_color(cx.theme().muted_foreground)
                .child(t(L10nKey::MarkdownMoveHelp))
                .into_any_element(),
        )
    }
    pub(super) fn reorder_actions<E: InteractiveElement>(
        &self,
        root: E,
        cx: &mut Context<Self>,
    ) -> E {
        root.on_action(cx.listener(|view, _: &PreviousDestination, window, cx| {
            view.step_reorder(-1, false, window, cx)
        }))
        .on_action(cx.listener(|view, _: &NextDestination, window, cx| {
            view.step_reorder(1, false, window, cx)
        }))
        .on_action(cx.listener(|view, _: &FirstDestination, window, cx| {
            view.step_reorder(-1, true, window, cx)
        }))
        .on_action(cx.listener(|view, _: &LastDestination, window, cx| {
            view.step_reorder(1, true, window, cx)
        }))
        .on_action(cx.listener(|view, _: &CommitReorder, window, cx| {
            view.finish_reorder(None, window, cx);
        }))
        .on_action(cx.listener(|view, _: &CancelReorder, _, cx| {
            view.cancel_reorder(cx);
        }))
    }
}
