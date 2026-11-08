//! Alignment comments are metadata only immediately before a parsed block formula.
use super::super::*;

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(in crate::ui::code_editor::markdown) enum Align { Left, #[default] Center, Right }
impl Align {
    fn name(self) -> &'static str { match self { Self::Left => "left", Self::Center => "center", Self::Right => "right" } }
}
fn parse(text: &str) -> Option<Align> {
    match text.trim().strip_prefix("<!--")?.strip_suffix("-->")?.trim().strip_prefix("math:")?.trim() {
        "left" => Some(Align::Left), "center" => Some(Align::Center), "right" => Some(Align::Right), _ => None,
    }
}
fn marker(source: &str, at: usize) -> Option<(Range<usize>, Align)> {
    let start = source[..at].rfind('\n').map_or(0, |ix| ix + 1);
    let prefix = &source[start..at];
    let previous = start.checked_sub(1)?;
    let before = source[..previous].rfind('\n').map_or(0, |ix| ix + 1);
    let align = parse(source[before..previous].strip_prefix(prefix)?)?;
    Some((before..start, align))
}
pub(super) fn edit(source: &Rope, at: usize, align: Align) -> Option<(Range<usize>, String)> {
    let row = source.offset_to_point(at).row;
    let start = source.line_start_offset(row);
    let prefix = source.slice(start..at).to_string();
    let existing = if row == 0 { None } else {
        let previous = source.line_start_offset(row - 1);
        source.slice(previous..start - 1).to_string().strip_prefix(&prefix).and_then(parse).map(|_| previous..start)
    };
    if align == Align::Center { return existing.map(|range| (range, String::new())); }
    let text = format!("{prefix}<!-- math:{} -->\n", align.name());
    Some((existing.unwrap_or(start..start), text))
}
pub(in crate::ui::code_editor::markdown) fn hide_markers(source: &str, leaves: &mut Vec<super::super::super::projection::Leaf>) {
    let ranges: Vec<_> = leaves.iter_mut().filter(|leaf| leaf.kind == Kind::Math).filter_map(|leaf| {
        let (range, align) = marker(source, leaf.range.start)?;
        leaf.math_alignment = align;
        Some(range)
    }).collect();
    leaves.retain(|leaf| !matches!(leaf.kind, Kind::Html) || !ranges.iter().any(|range| range.start <= leaf.range.start && range.end >= leaf.range.end));
}
