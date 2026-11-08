//! Table authoring routes into the owning document; no cell buffer/history.
use super::*;

impl SourcePresentation {
    pub(super) fn insert_table(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.command(Command::FinishComposition, window, cx);
        if !self.extra_selections.is_empty()
            || self.projection.revision != self.document.read(cx).revision()
        {
            return;
        }
        let mut range = self.state.selection.range();
        if let Some(leaf) = self.projection.leaf_at(range.start) {
            match leaf.kind {
                Kind::TableCell(cell) => range = cell.table().end..cell.table().end,
                Kind::Mdx(_) => {
                    if let Some(owner) = self.projection.mdx.components.iter().find(|component| {
                        let owner = component.source_range();
                        owner.start <= range.start && range.end <= owner.end
                    }) {
                        let end = owner.source_range().end;
                        range = end..end;
                    } else {
                        return;
                    }
                }
                Kind::Prose | Kind::Heading(_) => {}
                _ => range = leaf.range.end..leaf.range.end,
            }
        }
        let document = self.document.read(cx);
        let source = document.source();
        let before = source
            .slice(document.boundary(range.start.saturating_sub(2), false)..range.start)
            .to_string();
        let after = source
            .slice(range.end..document.boundary((range.end + 2).min(source.len()), true))
            .to_string();
        let leading = if range.start == 0 {
            0
        } else {
            2 - before
                .chars()
                .rev()
                .take_while(|ch| *ch == '\n')
                .count()
                .min(2)
        };
        let trailing = 2 - after.chars().take_while(|ch| *ch == '\n').count().min(2);
        let text = format!(
            "{}|  |  |\n| --- | --- |\n|  |  |{}",
            "\n".repeat(leading),
            "\n".repeat(trailing)
        );
        let selection = Selection::caret(range.start + leading + 2);
        self.command(
            Command::BatchReplaceSelected {
                edits: vec![(range, text)],
                selection,
            },
            window,
            cx,
        );
        self.focus.focus(window, cx);
    }

    pub(super) fn table_insert_button(&self, cx: &mut Context<Self>) -> impl IntoElement {
        use gpui_kit::component::{
            Disableable as _, Sizable as _,
            button::{Button, ButtonVariants as _},
        };
        gpui_kit::component::h_flex()
            .flex_none()
            .px_3()
            .py_1()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_action(cx.listener(|view, _: &actions::Escape, window, cx| {
                view.focus.focus(window, cx);
                cx.stop_propagation();
            }))
            .child(
                Button::new("markdown-insert-table")
                    .ghost()
                    .small()
                    .label(t(L10nKey::EditorTableInsert))
                    .disabled(!self.extra_selections.is_empty())
                    .on_click(cx.listener(|view, _, window, cx| view.insert_table(window, cx))),
            )
    }

    pub(super) fn table_input(
        &self,
        range: &Range<usize>,
        text: String,
        selected: Option<Range<usize>>,
        cx: &App,
    ) -> (String, Option<Range<usize>>) {
        if self.live
            && self.projection.revision == self.document.read(cx).revision()
            && self.projection.leaf_at(range.start).is_some_and(|leaf| {
                matches!(leaf.kind, Kind::TableCell(_)) && range.end <= leaf.range.end
            })
        {
            super::super::tables::input(self.document.read(cx).source(), range, text, selected)
        } else {
            (text, selected)
        }
    }

    // Replacement across partial cells deletes only editable content, then puts
    // the text in the first selected cell. Whole-table selections remain normal
    // document replacements. Direction and all edits stay in one transaction.
    pub(super) fn table_replacement(
        &self,
        range: Range<usize>,
        text: String,
        cx: &App,
    ) -> Vec<(Range<usize>, String)> {
        let partial = self.live
            && self.projection.revision == self.document.read(cx).revision()
            && self.projection.leaves.iter().any(|leaf| {
                let Kind::TableCell(cell) = leaf.kind else {
                    return false;
                };
                let table = cell.table();
                range.start < table.end
                    && table.start < range.end
                    && !(range.start <= table.start && table.end <= range.end)
            });
        if !partial {
            let (text, _) = self.table_input(&range, text, None, cx);
            return vec![(range, text)];
        }
        let mut ranges = self.deletion_ranges(range.clone(), cx).into_iter();
        let first = ranges.next().unwrap_or_else(|| {
            let at = self
                .projection
                .leaf_at(range.start)
                .map_or(range.start, |leaf| {
                    leaf.source_offset(leaf.display_offset(range.start))
                });
            at..at
        });
        let (text, _) = self.table_input(&first, text, None, cx);
        std::iter::once((first, text))
            .chain(ranges.map(|range| (range, String::new())))
            .collect()
    }

    pub(super) fn protect_table_replacement(&self, command: Command, cx: &App) -> Command {
        if !self.extra_selections.is_empty() || self.is_composing() {
            return command;
        }
        let range = match &command {
            Command::Insert(_) | Command::Commit(_) => self.state.selection.range(),
            Command::Replace { range, kind, .. }
                if matches!(kind, EditKind::Typing | EditKind::Paste) =>
            {
                range.clone()
            }
            _ => return command,
        };
        if !self.live
            || self.projection.revision != self.document.read(cx).revision()
            || self.projection.leaf_at(range.start).is_some_and(|leaf| {
                range.end <= leaf.range.end
                    && (!matches!(leaf.kind, Kind::TableCell(_))
                        || (leaf.content_start() <= range.start && range.end <= leaf.content_end()))
            })
            || !self.projection.leaves.iter().any(|leaf| {
                let Kind::TableCell(cell) = leaf.kind else {
                    return false;
                };
                let table = cell.table();
                range.start < table.end
                    && table.start < range.end
                    && !(range.start <= table.start && table.end <= range.end)
            })
        {
            return command;
        }
        let (range, text, kind) = match command {
            Command::Insert(text) | Command::Commit(text) => {
                (self.state.selection.range(), text, EditKind::Typing)
            }
            Command::Replace { range, text, kind }
                if matches!(kind, EditKind::Typing | EditKind::Paste) =>
            {
                (range, text, kind)
            }
            other => return other,
        };
        // Empty edits retain the shared deletion/format-repair route.
        if text.is_empty() {
            return Command::Replace { range, text, kind };
        }
        let edits = self.table_replacement(range.clone(), text, cx);
        if edits.len() == 1 && edits[0].0 == range {
            let (range, text) = edits.into_iter().next().unwrap();
            return Command::Replace { range, text, kind };
        }
        let selection = Selection::caret(edits[0].0.start + edits[0].1.len());
        Command::BatchReplaceSelected { edits, selection }
    }

    pub(super) fn table_arrow_target(
        &self,
        at: usize,
        horizontal: i32,
        vertical: i32,
        edge: Option<bool>,
        cx: &App,
    ) -> Option<usize> {
        if !self.live || self.projection.revision != self.document.read(cx).revision() {
            return None;
        }
        let leaf = self.projection.leaf_at(at)?;
        if !matches!(leaf.kind, Kind::TableCell(_)) {
            return None;
        }
        if let Some(end) = edge {
            return Some(if end {
                leaf.content_end()
            } else {
                leaf.content_start()
            });
        }
        if vertical != 0 {
            return super::super::tables::vertical(&self.projection.leaves, at, vertical < 0);
        }
        let backward = horizontal < 0;
        if (backward && at <= leaf.content_start()) || (!backward && at >= leaf.content_end()) {
            return super::super::tables::navigate(
                &self.projection.leaves,
                at,
                super::super::tables::Navigation::Boundary { backward },
            )
            .flatten();
        }
        None
    }

    pub(super) fn enter_table(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !self.live || self.projection.revision != self.document.read(cx).revision() {
            return false;
        }
        let at = self.state.selection.head();
        let Some(target) = super::super::tables::navigate(
            &self.projection.leaves,
            at,
            super::super::tables::Navigation::Row { backward: false },
        ) else {
            return false;
        };
        self.command(Command::FinishComposition, window, cx);
        if let Some(target) = target {
            self.command(Command::Select(Selection::caret(target)), window, cx);
        } else {
            // A table-only document needs a real editable paragraph below it,
            // not focus transfer to unrelated workbench chrome.
            let document = self.document.read(cx);
            let end = document.source().len();
            let newline = if end > 0 && document.source().slice(end - 1..end).to_string() == "\n" {
                "\n"
            } else {
                "\n\n"
            };
            self.command(
                Command::BatchReplaceSelected {
                    edits: vec![(end..end, newline.into())],
                    selection: Selection::caret(end + newline.len()),
                },
                window,
                cx,
            );
        }
        true
    }
}
