use super::projection::{Kind, Leaf, Projection};
use markdown::mdast::{AlignKind, Node};
use std::ops::Range;

mod layout;
pub(super) use layout::Layout;
mod structure;
pub(super) use structure::{column_at, edit};
pub(super) mod metadata;

/// Derived from the same mdast table that supplies the inline cell children.
/// No cell owns text, focus, composition or undo state.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct Cell {
    start: usize,
    end: usize,
    row: usize,
    column: usize,
    align: AlignKind,
}
impl Cell {
    pub fn table(self) -> Range<usize> {
        self.start..self.end
    }
    pub fn is_header(self) -> bool {
        self.row == 0
    }
    pub fn same_row(self, other: Self) -> bool {
        self.start == other.start && self.row == other.row
    }
    pub fn column(self) -> usize {
        self.column
    }
    pub fn shift(&mut self, shift: impl Fn(usize) -> usize) {
        self.start = shift(self.start);
        self.end = shift(self.end);
    }
    pub fn rebase(&mut self, transaction: &super::Transaction) {
        self.start = transaction
            .map_selection(super::Selection::caret(self.start))
            .head();
        for edit in &transaction.edits {
            self.end = if self.end < edit.range.start {
                self.end
            } else if self.end >= edit.range.end {
                self.end - edit.range.len() + edit.inserted_len
            } else {
                edit.range.start + edit.inserted_len
            };
        }
    }
}

pub(super) fn project(
    source: &str,
    node: &Node,
    images: &std::sync::Arc<super::images::Definitions>,
    leaves: &mut Vec<Leaf>,
) -> bool {
    let Node::Table(table) = node else {
        return false;
    };
    let Some(position) = node.position() else {
        return false;
    };
    for (row, row_node) in table.children.iter().enumerate() {
        for (column, cell) in row_node.children().into_iter().flatten().enumerate() {
            if column >= table.align.len() {
                break; // GFM ignores excess body cells; their source stays untouched.
            }
            let Some(cell_position) = cell.position() else {
                continue;
            };
            let range = cell_position.start.offset..cell_position.end.offset;
            let metadata = Cell {
                start: position.start.offset,
                end: position.end.offset,
                row,
                column,
                align: table.align[column],
            };
            let mut leaf = Leaf::new(range.clone(), Kind::TableCell(metadata));
            leaf.image_definitions = images.clone();
            for child in cell.children().into_iter().flatten() {
                Projection::inline(source, child, &mut leaf);
            }
            // An empty cell still has one insertion point, between its padding.
            leaf.empty_insertion(range.start + range.len() / 2);
            leaves.push(leaf);
        }
    }
    true
}

#[derive(Clone, Copy)]
pub(super) enum Navigation {
    Cell { backward: bool },
    Exit { backward: bool },
    Row { backward: bool },
    Boundary { backward: bool },
}
impl Navigation {
    pub fn backward(self) -> bool {
        match self {
            Self::Cell { backward }
            | Self::Row { backward }
            | Self::Exit { backward }
            | Self::Boundary { backward } => backward,
        }
    }
}

/// Tab visits actual cells in source row order (header first), never the
/// delimiter row, and never creates rows. At either edge it selects the nearest
/// document leaf outside the table; a document edge transfers workbench focus.
/// Enter visits the next row in the same column; at the last row it exits.
/// Escape exits immediately. At a cell edge Backspace/Delete move without
/// deleting table syntax. Source mode retains literal source editing.
pub(super) fn navigate(
    leaves: &[Leaf],
    at: usize,
    navigation: Navigation,
) -> Option<Option<usize>> {
    let backward = navigation.backward();
    let exit = matches!(navigation, Navigation::Exit { .. });
    let ix = leaves.iter().position(|leaf| {
        matches!(leaf.kind, Kind::TableCell(_)) && leaf.range.start <= at && at <= leaf.range.end
    })?;
    let Kind::TableCell(cell) = leaves[ix].kind else {
        return None;
    };
    if matches!(navigation, Navigation::Row { .. }) {
        let row = if backward {
            cell.row.checked_sub(1)
        } else {
            Some(cell.row + 1)
        };
        if let Some(target) = leaves.iter().find(|leaf| {
            matches!(leaf.kind, Kind::TableCell(other)
                if other.start == cell.start && Some(other.row) == row && other.column == cell.column)
        }) {
            return Some(Some(target.content_start()));
        }
        return navigate(leaves, at, Navigation::Exit { backward });
    }
    let eligible = |leaf: &&Leaf| {
        !exit || !matches!(leaf.kind, Kind::TableCell(other) if other.start == cell.start)
    };
    let target = if backward {
        leaves[..ix].iter().rev().find(eligible)
    } else {
        leaves[ix + 1..].iter().find(eligible)
    };
    Some(target.map(|leaf| {
        if backward
            && (matches!(navigation, Navigation::Boundary { .. })
                || !matches!(leaf.kind, Kind::TableCell(_)))
        {
            leaf.content_end()
        } else {
            leaf.content_start()
        }
    }))
}

/// Arrow movement preserves the visible grapheme column inside a cell. The
/// separator has no leaf and therefore cannot become a vertical destination.
pub(super) fn vertical(leaves: &[Leaf], at: usize, backward: bool) -> Option<usize> {
    use unicode_segmentation::UnicodeSegmentation as _;
    let current = leaves.iter().find(|leaf| {
        matches!(leaf.kind, Kind::TableCell(_)) && leaf.range.start <= at && at <= leaf.range.end
    })?;
    let Kind::TableCell(cell) = current.kind else {
        return None;
    };
    let row = if backward {
        cell.row.checked_sub(1)?
    } else {
        cell.row + 1
    };
    let target = leaves.iter().find(|leaf| {
        matches!(leaf.kind, Kind::TableCell(other)
            if other.start == cell.start && other.row == row && other.column == cell.column)
    })?;
    let column = current.text[..current.display_offset(at)]
        .graphemes(true)
        .count();
    let offset = target
        .text
        .grapheme_indices(true)
        .nth(column)
        .map_or(target.text.len(), |(offset, _)| offset);
    Some(target.source_offset(offset))
}

/// A typed pipe is cell content, not a new column. Keep already escaped input
/// unchanged and account for syntax bytes in platform composition selections.
pub(super) fn input(
    source: &super::Rope,
    range: &Range<usize>,
    text: String,
    selected: Option<Range<usize>>,
) -> (String, Option<Range<usize>>) {
    if !text.contains(['|', '\n', '\r']) {
        return (text, selected);
    }
    use gpui_kit::base::input::RopeExt as _;
    let line = source.offset_to_point(range.start).row;
    let before = source
        .slice(source.line_start_offset(line)..range.start)
        .to_string();
    let mut escaped = before
        .bytes()
        .rev()
        .take_while(|byte| *byte == b'\\')
        .count()
        % 2
        != 0;
    let mut output = String::with_capacity(text.len());
    let mut selected = selected;
    let original_selection = selected.clone();
    let mut offset = 0;
    let mut output_offset = 0;
    let text = text.trim_end_matches(['\r', '\n']);
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        // Native selections use UTF-16; preserve their affinity before any
        // inserted escape and collapse CRLF as one visible space.
        if let (Some(selection), Some(original)) = (&mut selected, &original_selection) {
            if original.start >= offset {
                selection.start = output_offset;
            }
            if original.end >= offset {
                selection.end = output_offset;
            }
        }
        offset += ch.len_utf16();
        if ch == '\r' && chars.peek() == Some(&'\n') {
            chars.next();
            offset += 1;
        }
        let ch = if matches!(ch, '\r' | '\n') { ' ' } else { ch };
        if ch == '|' && !escaped {
            output.push('\\');
            output_offset += 1;
        }
        output.push(ch);
        output_offset += ch.len_utf16();
        escaped = ch == '\\' && !escaped;
    }
    if let (Some(selection), Some(original)) = (&mut selected, &original_selection) {
        if original.start >= offset {
            selection.start = output_offset;
        }
        if original.end >= offset {
            selection.end = output_offset;
        }
    }
    (output, selected)
}

gpui_kit::actions!(
    markdown_table,
    [
        InsertTable,
        InsertRowBefore,
        InsertRowAfter,
        DuplicateRow,
        DeleteTable,
        DeleteRow,
        InsertColumnBefore,
        InsertColumnAfter,
        DeleteColumn,
        AlignDefault,
        AlignLeft,
        AlignCenter,
        AlignRight,
    ]
);

#[derive(Clone, Copy)]
pub(crate) enum Operation {
    InsertRowBefore,
    InsertRowAfter,
    DuplicateRow,
    DeleteTable,
    DeleteRow,
    InsertColumnBefore,
    InsertColumnAfter,
    DeleteColumn,
    AlignDefault,
    AlignLeft,
    AlignCenter,
    AlignRight,
}


pub(crate) fn bindings(cx: &gpui_kit::App) -> Vec<gpui_kit::KeyBinding> {
    let bind = |keys: &str, action: Box<dyn gpui_kit::Action>| {
        let predicate = gpui_kit::KeyBindingContextPredicate::parse("MarkdownTable")
            .expect("static table key context")
            .into();
        gpui_kit::KeyBinding::load(keys, action, Some(predicate), false, None, cx)
            .expect("static table shortcut")
    };
    vec![
        bind("secondary-alt-shift-up", Box::new(InsertRowBefore)),
        bind("secondary-alt-shift-down", Box::new(InsertRowAfter)),
        bind("secondary-alt-shift-d", Box::new(DuplicateRow)),
        bind("secondary-alt-shift-backspace", Box::new(DeleteRow)),
        bind("secondary-alt-shift-left", Box::new(InsertColumnBefore)),
        bind("secondary-alt-shift-right", Box::new(InsertColumnAfter)),
        bind("secondary-alt-shift-delete", Box::new(DeleteColumn)),
        bind("secondary-alt-shift-0", Box::new(AlignDefault)),
        bind("secondary-alt-shift-1", Box::new(AlignLeft)),
        bind("secondary-alt-shift-2", Box::new(AlignCenter)),
        bind("secondary-alt-shift-3", Box::new(AlignRight)),
    ]
}
