//! Pointer activation is deliberately separate from the text-selection algorithm.
use super::*;

/// Local source permission belongs to one presentation and one revision.
pub(super) struct Destination {
    revision: u64,
    range: Range<usize>,
    return_to: Selection,
}

pub(super) struct Gesture {
    revision: u64,
    source: Range<usize>,
    target: String,
    down: Point<Pixels>,
    at: SourceCaret,
    resources: Option<super::super::resources::ResourceContext>,
}
impl Gesture {
    fn moved(&self, position: Point<Pixels>, window: &Window) -> bool {
        let slop = window.rem_size() * 0.25;
        (position.x - self.down.x).abs() > slop || (position.y - self.down.y).abs() > slop
    }
}
fn modifier(modifiers: Modifiers) -> bool {
    if cfg!(target_os = "macos") {
        modifiers.platform && !modifiers.control && !modifiers.alt && !modifiers.shift
    } else {
        modifiers.control && !modifiers.platform && !modifiers.alt && !modifiers.shift
    }
}

impl SourcePresentation {
    pub(super) fn link_destination_range(&mut self) -> Option<Range<usize>> {
        let current = self.link_destination.as_ref().is_some_and(|source| {
            let selected = self.state.selection.range();
            self.live
                && self.extra_selections.is_empty()
                && self.projection.revision == source.revision
                && source.range.start <= selected.start
                && selected.end <= source.range.end
        });
        if !current {
            self.link_destination = None;
        }
        self.link_destination
            .as_ref()
            .map(|source| source.range.clone())
    }

    pub(super) fn rebase_link_destination(
        destination: &mut Option<Destination>,
        owner: u64,
        transaction: &Transaction,
    ) {
        let Some(source) = destination.as_mut() else {
            return;
        };
        if transaction.revision() <= source.revision {
            return; // This command or a newer one already applied the transaction.
        }
        if transaction.origin != owner
            || source.revision.wrapping_add(transaction.edits.len() as u64)
                != transaction.revision()
        {
            *destination = None;
            return;
        }
        for edit in &transaction.edits {
            if edit.range.start < source.range.start || edit.range.end > source.range.end {
                *destination = None;
                return;
            }
            source.range.end = source.range.end - edit.range.len() + edit.inserted_len;
        }
        source.return_to = transaction.map_selection(source.return_to);
        source.revision = transaction.revision();
    }

    pub(super) fn exit_link_destination(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(source) = self.link_destination.take() else {
            return false;
        };
        self.command(Command::Select(source.return_to), window, cx);
        self.focus.focus(window, cx);
        true
    }

    pub(super) fn edit_link_destination(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.projection.revision != self.document.read(cx).revision() {
            return;
        }
        let Some(range) = self
            .projection
            .links
            .at(self.state.selection.head())
            .map(|link| link.destination.clone())
        else {
            return;
        };
        let return_to = self
            .link_destination
            .as_ref()
            .map_or(self.state.selection, |source| source.return_to);
        self.link_destination = Some(Destination {
            revision: self.projection.revision,
            range: range.clone(),
            return_to,
        });
        self.command(
            Command::Select(Selection::new(range.start, range.end)),
            window,
            cx,
        );
        self.focus.focus(window, cx);
    }

    pub(in crate::ui::code_editor::markdown::source) fn link_label_regions<'a>(
        &'a self,
        link: &'a super::super::links::Link,
        cx: &'a App,
    ) -> impl Iterator<Item = Bounds<Pixels>> + 'a {
        use unicode_segmentation::UnicodeSegmentation as _;
        let geometries = self
            .geometry
            .as_ref()
            .filter(|_| self.geometry_is_current(cx))
            .map(|(_, geometries)| geometries);
        // Leaves are source-ordered. Only the label's leaves participate; a
        // pointer over a short link must not walk the rest of the document.
        let first = self
            .projection
            .leaves
            .partition_point(|leaf| leaf.range.end <= link.label.start);
        let mut segments = self.projection.leaves[first..]
            .iter()
            .take_while(move |leaf| leaf.range.start < link.label.end)
            .filter_map(move |leaf| {
                let geometry = geometries?.get(&leaf.id)?;
                let start = leaf.display_offset(link.label.start.max(leaf.range.start));
                let end = leaf.display_offset(link.label.end.min(leaf.range.end));
                Some(leaf.text[start..end].grapheme_indices(true).filter_map(
                    move |(offset, grapheme)| {
                        navigation::grapheme_line_box(geometry, start + offset, grapheme)
                    },
                ))
            })
            .flatten();
        let mut pending: Option<Bounds<Pixels>> = None;
        std::iter::from_fn(move || {
            let mut region = pending.take().or_else(|| segments.next())?;
            for bounds in segments.by_ref() {
                if region.top() == bounds.top()
                    && bounds.left() <= region.right()
                    && region.left() <= bounds.right()
                {
                    region = region.union(&bounds);
                } else {
                    pending = Some(bounds);
                    return Some(region);
                }
            }
            Some(region)
        })
    }
    pub(in crate::ui::code_editor::markdown::source) fn pointer_link_at(
        &self,
        position: Point<Pixels>,
        cx: &App,
    ) -> Option<(&super::super::links::Link, SourceCaret, Bounds<Pixels>)> {
        if self
            .viewport
            .bounds
            .is_none_or(|bounds| !bounds.contains(&position))
        {
            return None;
        }
        if !self.live
            || self.state.composition.is_some()
            || self.projection.revision != self.document.read(cx).revision()
        {
            return None;
        }
        let at = self.point_caret_for(position, false, cx)?;
        let link = self.projection.links.at(at.byte)?;
        if at.byte < link.label.start || at.byte > link.label.end {
            return None;
        }
        let region = self
            .link_label_regions(link, cx)
            .find(|bounds| bounds.contains(&position))?;
        Some((link, at, region))
    }

    pub(super) fn begin_pointer_link(&mut self, event: &MouseDownEvent, cx: &App) -> bool {
        self.pointer_link = None;
        if event.button != MouseButton::Left || event.click_count != 1 || !modifier(event.modifiers)
        {
            return false;
        }
        if let Some((link, at, _)) = self.pointer_link_at(event.position, cx) {
            self.pointer_link = Some(Gesture {
                revision: self.projection.revision,
                source: link.source.clone(),
                target: link.target.clone(),
                down: event.position,
                at,
                resources: self.document.read(cx).resources().cloned(),
            });
        }
        self.pointer_link.is_some()
    }

    pub(super) fn move_pointer_link(
        &mut self,
        event: &MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(gesture) = self.pointer_link.as_ref() else {
            return;
        };
        if event.pressed_button != Some(MouseButton::Left) {
            self.pointer_link = None;
            return;
        }
        if !gesture.moved(event.position, window) {
            return;
        }
        let gesture = self.pointer_link.take().unwrap();
        // A modifier drag is still a text selection, not a delayed navigation.
        // Seed the existing selection engine from the captured source offset;
        // revealing link syntax must not shift the start to a different byte.
        self.select_caret(gesture.at, false, window, cx);
        self.dragging = true;
        self.drag_origin = None;
        self.drag_pointer = Some(event.position);
    }

    pub(super) fn finish_pointer_link(
        &mut self,
        position: Point<Pixels>,
        modifiers: Modifiers,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let Some(gesture) = self.pointer_link.take() else {
            return;
        };
        let document = self.document.read(cx);
        let same_owner = match (&gesture.resources, document.resources()) {
            (Some(owner), current) => owner.is_current(current),
            (None, None) => true,
            _ => false,
        };
        if modifier(modifiers)
            && !gesture.moved(position, window)
            && same_owner
            && document.revision() == gesture.revision
            && self
                .pointer_link_at(position, cx)
                .is_some_and(|(link, _, _)| {
                    link.source == gesture.source && link.target == gesture.target
                })
        {
            self.follow_link(gesture.target, cx);
        }
    }

    pub(crate) fn link_menu(
        &self,
        menu: gpui_kit::component::menu::PopupMenu,
        cx: &App,
    ) -> gpui_kit::component::menu::PopupMenu {
        if self.projection.revision != self.document.read(cx).revision() {
            return menu;
        }
        let Some(link) = self.projection.links.at(self.state.selection.head()) else {
            return menu;
        };
        let label = if !link.source.contains(&link.destination.start) {
            L10nKey::MarkdownEditReferenceDestination
        } else {
            L10nKey::MarkdownEditLinkDestination
        };
        menu.separator()
            .menu(t(label), Box::new(EditLinkDestination))
            .menu(t(L10nKey::MarkdownFollowLink), Box::new(FollowLink))
    }
}
