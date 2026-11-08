use super::{Operation, Node};
use crate::ui::code_editor::markdown::{LineEnding, Selection, SourceText};
use std::ops::Range;

#[derive(Clone)]
struct Row {
    text: String,
    ending: Option<LineEnding>,
    prefix: usize,
}

// GFM treats only unescaped pipes as cell separators, including inside code.
fn cells(row: &Row) -> Vec<Range<usize>> {
    let mut pipes = Vec::new();
    let mut escaped = false;
    for (at, byte) in row.text.bytes().enumerate().skip(row.prefix) {
        if byte == b'|' && !escaped { pipes.push(at); }
        escaped = byte == b'\\' && !escaped;
    }
    let start = row.prefix;
    let end = row.text.trim_end_matches([' ', '\t']).len();
    let leading = pipes.first() == Some(&start);
    let trailing = pipes.last() == Some(&end.saturating_sub(1));
    let mut boundaries = vec![if leading { start + 1 } else { start }];
    boundaries.extend(pipes.iter().copied().filter(|at| (!leading || *at != start) && (!trailing || *at != end - 1)).map(|at| at + 1));
    let mut result: Vec<_> = boundaries.windows(2).map(|pair| pair[0]..pair[1] - 1).collect();
    result.push(*boundaries.last().unwrap()..if trailing { end - 1 } else { row.text.len() });
    result
}

pub(in crate::ui::code_editor::markdown) fn column_at(input: &SourceText, header: usize, at: usize) -> Option<usize> {
    let start = input.text[..at].rfind('\n').map_or(0, |at| at + 1);
    let end = input.text[at..].find('\n').map_or(input.text.len(), |end| at + end);
    let text = &input.text[start..end];
    let prefix = if start == 0 { header } else { text.len() - text.trim_start_matches([' ', '\t', '>']).len() };
    let row = Row { text: text.into(), ending: None, prefix };
    cells(&row).iter().position(|range| range.start <= at - start && at - start <= range.end)
}

fn content(row: &Row, column: usize, offset: usize) -> usize {
    let ranges = cells(row);
    let range = &ranges[column.min(ranges.len() - 1)];
    let text = &row.text[range.clone()];
    if text.trim_matches([' ', '\t']).is_empty() {
        return range.start + range.len() / 2;
    }
    let start = range.start + text.len() - text.trim_start_matches([' ', '\t']).len();
    let end = range.end - (text.len() - text.trim_end_matches([' ', '\t']).len());
    let mut at = (start + offset).min(end.max(start));
    while !row.text.is_char_boundary(at) { at -= 1; }
    at
}

/// Derive cells from the revision-owned table, but splice raw physical rows.
/// Unedited padding, delimiters, inline spelling and line terminators survive.
pub(in crate::ui::code_editor::markdown) fn edit(input: &SourceText, owner: Range<usize>, selection: Selection, operation: Operation, newline: LineEnding) -> Option<(Range<usize>, SourceText, usize)> {
    let source = &input.text;
    if selection.range().start < owner.start || selection.range().end > owner.end { return None; }
    fn owning<'a>(node: &'a Node, owner: &Range<usize>) -> Option<&'a markdown::mdast::Table> {
        if let Node::Table(table) = node {
            let position = node.position()?;
            if position.start.offset == owner.start && position.end.offset == owner.end { return Some(table); }
        }
        node.children()?.iter().find_map(|child| owning(child, owner))
    }
    let root = markdown::to_mdast(source, &markdown::ParseOptions::gfm()).ok()?;
    let mut plain = String::new();
    let mut plain_starts = Vec::new();
    let mut original_starts = Vec::new();
    let standalone;
    let (table, decontainered) = if let Some(table) = owning(&root, &owner) { (table, false) } else {
        let mut at = 0;
        for line in source.split_inclusive('\n') {
            plain_starts.push(plain.len());
            original_starts.push(at + owner.start);
            plain.push_str(line.get(owner.start..)?);
            at += line.len();
        }
        standalone = markdown::to_mdast(&plain, &markdown::ParseOptions::gfm()).ok()?;
        (owning(&standalone, &(0..plain.trim_end_matches('\n').len()))?, true)
    };
    let original = |at: usize| {
        if decontainered {
            let ix = plain_starts.partition_point(|start| *start <= at) - 1;
            original_starts[ix] + at - plain_starts[ix]
        } else { at }
    };
    let mut starts = vec![0];
    starts.extend(source.match_indices('\n').map(|(at, _)| at + 1));
    let line_at = |at: usize| starts.partition_point(|start| *start <= at) - 1;
    let mut rows: Vec<Row> = source.split_inclusive('\n').enumerate().map(|(ix, text)| Row {
        text: text.trim_end_matches('\n').into(), ending: input.endings.get(ix).copied(), prefix: 0,
    }).collect();
    let mut active = None;
    let mut row_lines = Vec::new();
    for node in &table.children {
        let at = original(node.position()?.start.offset);
        let line = line_at(at);
        rows[line].prefix = at - starts[line];
        row_lines.push(line);
        for (column, cell) in node.children()?.iter().enumerate().take(table.align.len()) {
            let span = cell.position()?;
            if original(span.start.offset) <= selection.head() && selection.head() <= original(span.end.offset) {
                let raw = cells(&rows[line]);
                let start = content(&rows[line], column, 0);
                active = Some((line, column, selection.head().saturating_sub(starts[line] + start)));
                if column >= raw.len() { return None; }
            }
        }
    }
    let (mut target, mut column, mut offset) = active?;
    let delimiter = row_lines[0] + 1;
    let prefix = rows[delimiter].text.len() - rows[delimiter].text.trim_start_matches([' ', '\t', '>']).len();
    rows[delimiter].prefix = prefix;
    let columns = table.align.len();
    match operation {
        Operation::InsertRowBefore | Operation::InsertRowAfter | Operation::DuplicateRow => {
            let insert = if target == row_lines[0] { delimiter + 1 } else { target + usize::from(!matches!(operation, Operation::InsertRowBefore)) };
            let row = if matches!(operation, Operation::DuplicateRow) {
                let mut copy = rows[target].clone();
                // A list's first-row marker belongs only to the header.
                copy.text.replace_range(..copy.prefix, &rows[delimiter].text[..prefix]);
                copy.prefix = prefix;
                copy.ending = Some(newline);
                copy
            } else {
                column = 0;
                offset = 0;
                Row { text: format!("{}|{}", &rows[delimiter].text[..prefix], "  |".repeat(columns)), ending: Some(newline), prefix }
            };
            rows.insert(insert, row);
            target = insert;
        }
        Operation::DeleteRow => {
            if target == row_lines[0] { return None; }
            rows.remove(target);
            target = row_lines.iter().copied().filter(|line| *line != target).map(|line| if line > target { line - 1 } else { line }).find(|line| *line >= target).unwrap_or_else(|| {
                row_lines.iter().copied().filter(|line| *line < target).next_back().unwrap_or(row_lines[0])
            });
        }
        Operation::DeleteTable => return Some((0..source.len(), SourceText { text: String::new(), endings: Vec::new() }, 0)),
        operation => {
            let insert = matches!(operation, Operation::InsertColumnBefore | Operation::InsertColumnAfter);
            let delete = matches!(operation, Operation::DeleteColumn);
            if delete && columns == 1 { return None; }
            let at = column + usize::from(matches!(operation, Operation::InsertColumnAfter));
            for line in row_lines.iter().copied().chain(std::iter::once(delimiter)) {
                let row = &mut rows[line];
                if !insert && !delete && line != delimiter { continue; }
                if delete && column >= cells(row).len() { continue; }
                // Missing GFM body cells are virtual until a structural edit needs them.
                while insert && cells(row).len() < at {
                    let ranges = cells(row);
                    let end = ranges.last()?.end;
                    row.text.insert_str(end, " | ");
                }
                let ranges = cells(row);
                if insert {
                    let value = if line == delimiter { " --- " } else { "  " };
                    if at < ranges.len() { row.text.insert_str(ranges[at].start, &format!("{value}|")); }
                    else { row.text.insert_str(ranges.last()?.end, &format!("|{value}")); }
                } else if delete {
                    if ranges.len() == 1 {
                        let raw = &row.text[ranges[0].clone()];
                        let start = ranges[0].start + raw.len() - raw.trim_start().len();
                        let end = ranges[0].end - (raw.len() - raw.trim_end().len());
                        if start < end { row.text.replace_range(start..end, ""); }
                    } else {
                        let range = if column + 1 < ranges.len() { ranges[column].start..ranges[column + 1].start }
                            else { ranges[column - 1].end..ranges[column].end };
                        row.text.replace_range(range, "");
                    }
                } else if line == delimiter {
                    let range = &ranges[column];
                    let raw = &row.text[range.clone()];
                    let token_start = range.start + raw.len() - raw.trim_start().len();
                    let token_end = range.end - (raw.len() - raw.trim_end().len());
                    let dashes = row.text[token_start..token_end].trim_matches(':').to_owned();
                    let token = match operation {
                        Operation::AlignLeft => format!(":{dashes}"), Operation::AlignRight => format!("{dashes}:"),
                        Operation::AlignCenter => format!(":{dashes}:"), _ => dashes,
                    };
                    row.text.replace_range(token_start..token_end, &token);
                }
            }
            if matches!(operation, Operation::InsertColumnBefore) { column += 1; }
            if delete { column = column.min(columns - 2); }
        }
    }
    let final_newline = source.ends_with('\n');
    let mut output = SourceText { text: String::new(), endings: Vec::new() };
    let mut caret = 0;
    for (ix, row) in rows.iter().enumerate() {
        if ix == target { caret = output.text.len() + content(row, column, offset); }
        output.text.push_str(&row.text);
        if ix + 1 < rows.len() || final_newline {
            output.text.push('\n');
            output.endings.push(row.ending.unwrap_or(newline));
        }
    }
    Some((0..source.len(), output, caret))
}
