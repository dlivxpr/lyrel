//! Shared terminal resizing for the daemon layout mirror and desktop consumer.

use rio_vt::crosswords::grid::Dimensions;
use rio_vt::crosswords::pos::Line;
use rio_vt::crosswords::{Crosswords, CrosswordsSize, Mode};
use rio_vt::event::EventListener;

/// The PTY's row model, not the operating system of the desktop displaying it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResizeMode {
    Raw,
    LocalConpty,
}

/// Resize both terminal mirrors using the row model of their actual PTY.
pub fn resize<T: EventListener>(
    term: &mut Crosswords<T>,
    size: impl Dimensions,
    mode: ResizeMode,
) {
    if mode == ResizeMode::Raw {
        term.resize(size);
        return;
    }
    resize_conpty(term, size);
}

/// Conhost adds blank rows at the bottom when growing, and scrolls the last
/// written row (which can be below the cursor) to the bottom when shrinking.
/// Rio instead pulls history during growth and anchors shrinking to the cursor.
/// Keep the row and column steps separate so content below the cursor survives
/// and cursor anchors are restored before any column reflow.
fn resize_conpty<T: EventListener>(term: &mut Crosswords<T>, size: impl Dimensions) {
    let old_rows = term.grid.screen_lines();
    let new_rows = size.screen_lines().max(1);
    // Column-only changes and alternate-screen programs use ordinary reflow.
    if new_rows == old_rows || term.mode().contains(Mode::ALT_SCREEN) {
        term.resize(size);
        return;
    }

    let old_cols = term.columns();
    let history_before = term.grid.history_size();
    let display_offset = term.grid.display_offset();
    let cursor_row = term.grid.cursor.pos.row.0 as usize;
    let saved_row = term.grid.saved_cursor.pos.row;
    let extra = if new_rows < old_rows {
        let last_written = (0..old_rows)
            .rev()
            .find(|&line| !term.grid[Line(line as i32)].is_clear())
            .unwrap_or(0)
            .max(cursor_row);
        let vanilla = (cursor_row + 1).saturating_sub(new_rows);
        let by_content = (last_written + 1).saturating_sub(new_rows);
        vanilla.max(by_content.min(cursor_row)) - vanilla
    } else {
        0
    };
    if extra > 0 {
        term.grid
            .scroll_up(&(Line(0)..Line(old_rows as i32)), extra);
    }

    term.resize(CrosswordsSize::new(old_cols, new_rows));

    if new_rows > old_rows {
        let pulled = history_before.min(new_rows - old_rows);
        if pulled > 0 {
            term.grid
                .scroll_up(&(Line(0)..Line(new_rows as i32)), pulled);
        }
        term.grid.cursor.pos.row = Line(cursor_row as i32);
        term.grid.saved_cursor.pos.row = saved_row;
        term.grid
            .scroll_display(rio_vt::crosswords::grid::Scroll::Bottom);
        term.grid
            .scroll_display(rio_vt::crosswords::grid::Scroll::Delta(
                display_offset.saturating_sub(new_rows - old_rows) as i32,
            ));
    } else {
        term.grid.cursor.pos.row -= extra;
        let scrolled = (cursor_row + 1).saturating_sub(new_rows) + extra;
        term.grid.saved_cursor.pos.row = Line(
            (saved_row.0 as usize)
                .saturating_sub(scrolled)
                .min(new_rows - 1) as i32,
        );
    }
    term.resize(size);
}
