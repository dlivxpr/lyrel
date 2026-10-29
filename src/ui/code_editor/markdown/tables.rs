use super::projection::{Kind, Leaf, Projection};
use gpui_kit::base::text::editor_host::{BlockNode, ColumnAlign, Paragraph, Table};
use markdown::mdast::{AlignKind, Node};
use std::ops::Range;

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
    pub fn shift(&mut self, shift: impl Fn(usize) -> usize) {
        self.start = shift(self.start);
        self.end = shift(self.end);
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

pub(super) fn block<'a>(cells: impl Iterator<Item = (Cell, &'a BlockNode)>) -> BlockNode {
    let mut rows: Vec<Vec<Paragraph>> = Vec::new();
    let mut aligns = Vec::new();
    for (cell, node) in cells {
        let BlockNode::Paragraph(paragraph) = node else {
            continue;
        };
        while rows.len() <= cell.row {
            rows.push(Vec::new());
        }
        if cell.row == 0 {
            aligns.push(match cell.align {
                AlignKind::Left => ColumnAlign::Left,
                AlignKind::Right => ColumnAlign::Right,
                AlignKind::Center => ColumnAlign::Center,
                AlignKind::None => ColumnAlign::None,
            });
        }
        rows[cell.row].push(paragraph.clone());
    }
    BlockNode::Table(Table::new(rows, aligns))
}

#[derive(Clone, Copy)]
pub(super) enum Navigation {
    Cell { backward: bool },
    Exit { backward: bool },
    Boundary { backward: bool },
}
impl Navigation {
    pub fn backward(self) -> bool {
        match self {
            Self::Cell { backward } | Self::Exit { backward } | Self::Boundary { backward } => {
                backward
            }
        }
    }
}

/// Tab visits actual cells in source row order (header first), never the
/// delimiter row, and never creates rows. At either edge it selects the nearest
/// document leaf outside the table; a document edge transfers workbench focus.
/// Escape/Enter exit forward immediately; Shift+Enter exits backward. At a
/// cell edge Backspace/Delete move without deleting table syntax. Source mode
/// retains ordinary indentation and literal source editing.
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

/// A typed pipe is cell content, not a new column. Keep already escaped input
/// unchanged and account for syntax bytes in platform composition selections.
pub(super) fn input(
    source: &super::Rope,
    range: &Range<usize>,
    text: String,
    selected: Option<Range<usize>>,
) -> (String, Option<Range<usize>>) {
    if !text.contains('|') {
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
    let mut offset = 0;
    let original_selection = selected.clone();
    for ch in text.chars() {
        if ch == '|' && !escaped {
            output.push('\\');
            if let (Some(selection), Some(original)) = (&mut selected, &original_selection) {
                selection.start += usize::from(offset < original.start);
                selection.end += usize::from(offset < original.end);
            }
        }
        output.push(ch);
        escaped = ch == '\\' && !escaped;
        offset += ch.len_utf16();
    }
    (output, selected)
}

/// Leaf layout remains GPUI-owned. Extend short cells to their row's actual
/// height and their column's measured extent so wrapped neighbors cannot win
/// pointer placement merely because they have more lines of text.
pub(super) fn geometry(
    leaves: &[Leaf],
    geometry: &std::collections::HashMap<u64, gpui_kit::base::text::editor_host::LeafGeometry>,
) -> std::collections::HashMap<u64, gpui_kit::Bounds<gpui_kit::Pixels>> {
    use gpui_kit::{Bounds, Pixels, point, size};
    use std::collections::HashMap;
    let mut rows: HashMap<(usize, usize), Bounds<Pixels>> = HashMap::new();
    let mut columns: HashMap<(usize, usize), Bounds<Pixels>> = HashMap::new();
    for leaf in leaves {
        let Kind::TableCell(cell) = leaf.kind else {
            continue;
        };
        let Some(layout) = geometry.get(&leaf.id) else {
            continue;
        };
        let bounds = layout.bounds();
        rows.entry((cell.start, cell.row))
            .and_modify(|row| *row = row.union(&bounds))
            .or_insert(bounds);
        columns
            .entry((cell.start, cell.column))
            .and_modify(|column| *column = column.union(&bounds))
            .or_insert(bounds);
    }
    leaves
        .iter()
        .filter_map(|leaf| {
            let Kind::TableCell(cell) = leaf.kind else {
                return None;
            };
            let row = rows.get(&(cell.start, cell.row))?;
            let column = columns.get(&(cell.start, cell.column))?;
            Some((
                leaf.id,
                Bounds::new(
                    point(column.left(), row.top()),
                    size(column.size.width, row.size.height),
                ),
            ))
        })
        .collect()
}

gpui_kit::actions!(
    markdown_table,
    [
        InsertRowBefore,
        InsertRowAfter,
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
    DeleteRow,
    InsertColumnBefore,
    InsertColumnAfter,
    DeleteColumn,
    AlignDefault,
    AlignLeft,
    AlignCenter,
    AlignRight,
}

/// A structural command rewrites only the owning top-level GFM table. Cell
/// fragments come from mdast source spans, never from their rendered labels.
/// Deleting the sole row/column clears it, keeping a valid editable table;
/// deleting a header promotes the next row. New cells are empty insertion sites.
pub(super) fn edit(
    source: &str,
    selection: super::Selection,
    operation: Operation,
) -> Option<(Range<usize>, String, usize)> {
    let mut options = markdown::ParseOptions::gfm();
    options.constructs.frontmatter = true;
    let root = markdown::to_mdast(source, &options).ok()?;
    for node in root.children().into_iter().flatten() {
        let Node::Table(table) = node else {
            continue;
        };
        let position = node.position()?;
        let range = position.start.offset..position.end.offset;
        if selection.range().start < range.start || selection.range().end > range.end {
            continue;
        }
        let mut active = None;
        let mut rows = Vec::with_capacity(table.children.len() + 1);
        for (row, row_node) in table.children.iter().enumerate() {
            let mut cells = Vec::new();
            for (column, cell) in row_node.children().into_iter().flatten().enumerate() {
                let span = cell.position()?;
                if column < table.align.len()
                    && span.start.offset <= selection.head()
                    && selection.head() <= span.end.offset
                {
                    active = Some((row, column));
                }
                cells.push(source[span.start.offset..span.end.offset].trim().to_owned());
            }
            cells.resize(cells.len().max(table.align.len()), String::new());
            rows.push(cells);
        }
        let (mut row, mut column) = active?;
        let mut aligns = table.align.clone();
        match operation {
            Operation::InsertRowBefore | Operation::InsertRowAfter => {
                row += usize::from(matches!(operation, Operation::InsertRowAfter));
                rows.insert(row, vec![String::new(); aligns.len()]);
            }
            Operation::DeleteRow => {
                if rows.len() == 1 {
                    rows[0].iter_mut().for_each(String::clear);
                } else {
                    rows.remove(row);
                    row = row.min(rows.len() - 1);
                }
            }
            Operation::InsertColumnBefore | Operation::InsertColumnAfter => {
                column += usize::from(matches!(operation, Operation::InsertColumnAfter));
                aligns.insert(column, AlignKind::None);
                for cells in &mut rows {
                    cells.insert(column, String::new());
                }
            }
            Operation::DeleteColumn => {
                if aligns.len() == 1 {
                    for cells in &mut rows {
                        cells[0].clear();
                    }
                } else {
                    aligns.remove(column);
                    for cells in &mut rows {
                        cells.remove(column);
                    }
                    column = column.min(aligns.len() - 1);
                }
            }
            alignment => {
                aligns[column] = match alignment {
                    Operation::AlignLeft => AlignKind::Left,
                    Operation::AlignCenter => AlignKind::Center,
                    Operation::AlignRight => AlignKind::Right,
                    _ => AlignKind::None,
                }
            }
        }
        let mut output = String::new();
        let mut caret = 0;
        for (row_ix, cells) in rows.iter().enumerate() {
            if row_ix > 0 {
                output.push('\n');
            }
            output.push('|');
            for (column_ix, cell) in cells.iter().enumerate() {
                output.push(' ');
                if row_ix == row && column_ix == column {
                    caret = range.start + output.len();
                }
                output.push_str(cell);
                output.push_str(" |");
            }
            if row_ix == 0 {
                output.push_str("\n|");
                for align in &aligns {
                    output.push_str(match align {
                        AlignKind::Left => " :--- |",
                        AlignKind::Right => " ---: |",
                        AlignKind::Center => " :---: |",
                        AlignKind::None => " --- |",
                    });
                }
            }
        }
        return Some((range, output, caret));
    }
    None
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
