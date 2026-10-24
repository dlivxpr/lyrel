//! Shared Kitty placeholder interpretation over Rio's actual grid cells.
//! Both daemon retention and the desktop's viewport projection use these rules.

use rio_vt::ansi::kitty_virtual::{IncompletePlacement, PlaceholderRun};
use rio_vt::crosswords::grid::Grid;
use rio_vt::crosswords::pos::Line;
use rio_vt::crosswords::square::Square;

use super::Placement;

/// A consecutive source-cell slice and its destination column in one grid row.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct PlaceholderSlice {
    pub run: PlaceholderRun,
    pub column: usize,
}

/// Interpret one real grid row left-to-right, without consulting the palette.
/// Inheritance depends on the encoded color attributes, not resolved RGB values.
pub fn placeholder_row(grid: &Grid<Square>, line: Line) -> Vec<PlaceholderSlice> {
    let mut slices: Vec<PlaceholderSlice> = Vec::new();
    let mut previous = None;
    for (column, cell) in grid[line][..].iter().enumerate() {
        if cell.c() != '\u{10eeee}' {
            previous = None;
            continue;
        }
        let style = grid.style_of(cell);
        if matches!(style.fg, rio_vt::config::colors::AnsiColor::Named(_)) {
            previous = None;
            continue;
        }
        let marks = cell.extras_id_checked()
            .and_then(|id| grid.extras_table.get(id))
            .map(|extra| extra.zerowidth.as_slice())
            .unwrap_or(&[]);
        let mut decoded = IncompletePlacement::from_cell(style.fg, style.underline_color, marks);
        // Malformed marks are not omitted coordinates. In particular an invalid
        // high byte must not alias the low-24-bit image or inherit real pixels.
        if marks.len() > 3 || marks.iter().enumerate().any(|(index, mark)| {
            rio_vt::ansi::kitty_virtual::diacritic_to_index(*mark)
                .is_none_or(|value| index == 2 && value > 255)
        }) {
            previous = None;
            continue;
        }
        if let Some((left_fg, left_underline, left)) = previous {
            let left: PlaceholderRun = left;
            if style.fg == left_fg && style.underline_color == left_underline {
                let consecutive = left.col.checked_add(1);
                let inherit = match marks.len() {
                    0 => {
                        decoded.row = Some(left.row);
                        decoded.col = consecutive;
                        true
                    }
                    1 if decoded.row == Some(left.row) => {
                        decoded.col = consecutive;
                        true
                    }
                    2 => decoded.row == Some(left.row) && decoded.col == consecutive,
                    _ => false,
                };
                if inherit {
                    decoded.image_id_high = Some((left.image_id >> 24) as u8);
                }
            }
        }
        let run = decoded.complete();
        previous = Some((style.fg, style.underline_color, run));
        if let Some(last) = slices.last_mut()
            && last.column + last.run.width as usize == column
            && last.run.image_id == run.image_id
            && last.run.placement_id == run.placement_id
            && last.run.row == run.row
            && last.run.col.checked_add(last.run.width) == Some(run.col)
        {
            last.run.width += 1;
        } else {
            slices.push(PlaceholderSlice { run, column });
        }
    }
    slices
}

/// Resolve only virtual prototypes on the selected screen. A zero placement id
/// deterministically selects the lowest protocol placement id, then oldest key.
pub fn placeholder_prototype<'a>(
    placements: impl Iterator<Item = &'a Placement>,
    screen: u64,
    run: &PlaceholderRun,
) -> Option<&'a Placement> {
    placements.filter(|placement| {
        placement.virtual_placement && placement.screen == screen
            && run.image_id != 0 && placement.image_id == run.image_id
            && (run.placement_id == 0 || run.placement_id == placement.placement_id)
    }).min_by_key(|placement| (placement.placement_id, placement.key))
}
