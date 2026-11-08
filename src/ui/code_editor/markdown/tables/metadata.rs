//! Compatible table presentation comments. Parsing never writes source; only
//! explicit presentation/structure commands replace the relevant token.
use super::super::Rope;
use gpui_kit::base::input::RopeExt as _;
use std::ops::Range;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Style {
    #[default]
    Grid,
    Striped,
    Header,
    Minimal,
}
impl Style {
    pub(super) fn name(self) -> &'static str {
        match self { Self::Grid => "grid", Self::Striped => "striped", Self::Header => "header", Self::Minimal => "minimal" }
    }
    fn parse(value: &str) -> Option<Self> {
        Some(match value { "grid" => Self::Grid, "striped" => Self::Striped, "header" => Self::Header, "minimal" => Self::Minimal, _ => return None })
    }
}

#[derive(Clone)]
pub(crate) struct Marker {
    pub(super) range: Range<usize>,
    pub(super) style: Style,
    pub(super) widths: Vec<f32>,
    style_range: Range<usize>,
    widths_range: Option<Range<usize>>,
    attributes_end: usize,
}

// Unknown attributes and malformed width lists remain ordinary editable HTML.
// Retain ranges into the original spelling so a style command cannot normalize
// widths, whitespace or another token as an incidental side effect.
fn parse(line: &str, start: usize, end: usize) -> Option<Marker> {
    let text = line.trim();
    let body = text.strip_prefix("<!--")?.strip_suffix("-->")?.trim();
    let body = body.strip_prefix("table:")?.trim();
    let mut tokens = body.split_whitespace();
    let name = tokens.next()?;
    let style = Style::parse(name)?;
    let style_start = start + (body.as_ptr() as usize - line.as_ptr() as usize);
    let mut widths = Vec::new();
    let mut widths_range = None;
    for token in tokens {
        if widths_range.is_some() { return None; }
        let value = token.strip_prefix("cols=")?;
        widths = value.split(',').map(str::parse::<f32>).collect::<Result<Vec<_>, _>>().ok()?;
        if widths.is_empty() || widths.iter().any(|width| !width.is_finite() || *width <= 0.) { return None; }
        let offset = start + (value.as_ptr() as usize - line.as_ptr() as usize);
        widths_range = Some(offset..offset + value.len());
    }
    Some(Marker {
        range: start..end,
        style,
        widths,
        style_range: style_start..style_start + name.len(),
        widths_range,
        attributes_end: start + (body.as_ptr() as usize - line.as_ptr() as usize) + body.len(),
    })
}

pub(crate) fn read(source: &Rope, table_start: usize) -> Option<Marker> {
    let row = source.offset_to_point(table_start).row;
    let previous = row.checked_sub(1)?;
    let table_line = source.line_start_offset(row);
    let prefix = source.slice(table_line..table_start).to_string();
    let start = source.line_start_offset(previous);
    let line = source.slice(start..table_line - 1).to_string();
    let (content, prefix_len) = marker_content(&line, &prefix)?;
    let mut marker = parse(content, start + prefix_len, table_line)?;
    marker.range.start = start;
    Some(marker)
}

fn continuation(ch: char) -> char { if ch.is_ascii_whitespace() || ch == '>' { ch } else { ' ' } }

fn marker_content<'a>(line: &'a str, table_prefix: &str) -> Option<(&'a str, usize)> {
    // A list marker belongs only to its own first block, not to a sibling item.
    if table_prefix.chars().any(|ch| continuation(ch) != ch) { return None; }
    let len = super::super::lists::prefix_len(line);
    let prefix = &line[..len];
    prefix.chars().map(continuation).eq(table_prefix.chars()).then_some((&line[len..], len))
}

pub(in crate::ui::code_editor::markdown) fn marker_range(source: &Rope, table_start: usize) -> Option<Range<usize>> {
    read(source, table_start).map(|marker| marker.range)
}

pub(in crate::ui::code_editor::markdown) fn column_edit(source: &Rope, table_start: usize, column: usize, insert: bool) -> Option<(Range<usize>, String)> {
    let marker = read(source, table_start)?;
    let range = marker.widths_range?;
    // Preserve the spelling of every surviving column's width.
    let value = source.slice(range.clone()).to_string();
    let mut columns: Vec<_> = value.split(',').collect();
    if insert {
        if column > columns.len() { return None; }
        let width = *columns.get(column.min(columns.len().checked_sub(1)?))?;
        columns.insert(column.min(columns.len()), width);
    } else if column < columns.len() {
        columns.remove(column);
    } else { return None; }
    Some((range, columns.join(",")))
}

pub(crate) fn style_edit(source: &Rope, table_start: usize, style: Style) -> (Range<usize>, String) {
    if let Some(marker) = read(source, table_start) {
        (marker.style_range, style.name().into())
    } else {
        new_marker(source, table_start, style, None)
    }
}

pub(crate) fn widths_edit(source: &Rope, table_start: usize, widths: &[f32]) -> (Range<usize>, String) {
    let value = widths.iter().map(|width| format!("{:.0}", width.round().max(24.))).collect::<Vec<_>>().join(",");
    if let Some(marker) = read(source, table_start) {
        if let Some(range) = marker.widths_range { (range, value) }
        else { (marker.attributes_end..marker.attributes_end, format!(" cols={value}")) }
    } else {
        new_marker(source, table_start, Style::Grid, Some(value))
    }
}

fn new_marker(source: &Rope, table_start: usize, style: Style, widths: Option<String>) -> (Range<usize>, String) {
    let start = source.line_start_offset(source.offset_to_point(table_start).row);
    let prefix = source.slice(start..table_start).to_string();
    let continuation: String = prefix.chars().map(continuation).collect();
    let marker = format!("{prefix}<!-- table:{}{} -->\n", style.name(), widths.map_or(String::new(), |widths| format!(" cols={widths}")));
    if continuation == prefix { (start..start, marker) }
    else { (start..table_start, format!("{marker}{continuation}")) }
}

/// Hide only valid comments immediately preceding an actual parsed table.
pub(in crate::ui::code_editor::markdown) fn hide_markers(source: &str, leaves: &mut Vec<super::Leaf>) {
    let ranges: Vec<_> = leaves.iter().filter_map(|leaf| {
        let super::Kind::TableCell(cell) = leaf.kind else { return None; };
        if cell.row != 0 || cell.column != 0 { return None; }
        let start = source[..cell.start].rfind('\n').map_or(0, |ix| ix + 1);
        let prefix = &source[start..cell.start];
        let previous = start.checked_sub(1)?;
        let marker_start = source[..previous].rfind('\n').map_or(0, |ix| ix + 1);
        let (content, prefix_len) = marker_content(&source[marker_start..previous], prefix)?;
        parse(content, marker_start + prefix_len, start).map(|_| marker_start..start)
    }).collect();
    leaves.retain(|leaf| !matches!(leaf.kind, super::Kind::Html) || !ranges.iter().any(|range| range.start <= leaf.range.start && range.end >= leaf.range.end));
}
