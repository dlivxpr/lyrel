//! Stable text ownership, separate from synchronized frame acquisition.
//! Absolute rows belong to the GUI mirror; the daemon remains authoritative.
use std::collections::BTreeMap;

use rio_vt::crosswords::Crosswords;
use rio_vt::crosswords::grid::{Dimensions, Scroll};
use rio_vt::crosswords::pos::Line;
use rio_vt::crosswords::square::{CellFlags, Hyperlink, Wide};
use rio_vt::crosswords::style::Style;
use rio_vt::event::EventListener;

/// Decoded values, never Square side-table IDs or theme-resolved colours.
#[derive(Clone)]
pub(super) struct ReadingCell {
    pub(super) c: char,
    pub(super) style: Style,
    pub(super) marks: Option<Box<[char]>>,
    pub(super) hyperlink: Option<Hyperlink>,
    pub(super) wide: Wide,
    pub(super) wrapped: bool,
}

pub(super) struct ReadingView {
    top: i64,
    captured_live_start: i64,
    cols: usize,
    rows: BTreeMap<i64, Vec<ReadingCell>>,
}

pub(super) fn live_start<T: EventListener>(term: &Crosswords<T>) -> i64 {
    term.grid.lines_evicted() as i64 + term.grid.history_size() as i64
}

impl ReadingView {
    pub(super) fn capture<T: EventListener>(term: &Crosswords<T>) -> Self {
        let live = live_start(term);
        let mut reading = Self {
            top: live - term.display_offset() as i64,
            captured_live_start: live,
            cols: term.columns(),
            rows: BTreeMap::new(),
        };
        // Even a one-row scroll exposes mutable active-screen rows. Capture
        // the entire active screen once, not only the currently visible part.
        for row in live..live + term.screen_lines() as i64 {
            reading.retain_row(term, row);
        }
        reading.retain_visible(term);
        reading
    }

    fn retain_row<T: EventListener>(&mut self, term: &Crosswords<T>, row: i64) {
        self.rows.entry(row).or_insert_with(|| {
            let line = Line((row - live_start(term)) as i32);
            term.grid[line][..].iter().map(|cell| {
                let extras = cell.extras_id_checked().and_then(|id| term.grid.extras_table.get(id));
                ReadingCell {
                    c: cell.c(),
                    style: term.grid.style_of(cell),
                    marks: extras.map(|extra| extra.zerowidth.as_slice())
                        .filter(|marks| !marks.is_empty()).map(Box::from),
                    hyperlink: extras.and_then(|extra| extra.hyperlink.clone()),
                    wide: cell.wide(),
                    wrapped: cell.contains_cell_flag(CellFlags::WRAPLINE),
                }
            }).collect()
        });
    }

    fn retain_visible<T: EventListener>(&mut self, term: &Crosswords<T>) {
        let oldest = term.grid.lines_evicted() as i64;
        let end = live_start(term) + term.screen_lines() as i64;
        // Include the preceding fractional-scroll sliver in the same source.
        for row in (self.top - 1).max(oldest)..(self.top + term.screen_lines() as i64).min(end) {
            self.retain_row(term, row);
        }
    }

    pub(super) fn row(&self, viewport_row: i32) -> Option<&[ReadingCell]> {
        self.rows.get(&(self.top + viewport_row as i64)).map(Vec::as_slice)
    }

    pub(super) fn distance<T: EventListener>(&self, term: &Crosswords<T>) -> usize {
        (live_start(term) - self.top).max(0) as usize
    }

    /// Reconcile automatic emulator offset changes without moving our anchor.
    /// History is read lazily, first-write-wins, and cannot outlive retention.
    pub(super) fn reconcile<T: EventListener>(&mut self, term: &mut Crosswords<T>) -> bool {
        let live = live_start(term);
        if live < self.captured_live_start { return false; }
        self.top = self.top.clamp(term.grid.lines_evicted() as i64, live);
        let oldest = term.grid.lines_evicted() as i64;
        while self.rows.first_key_value().is_some_and(|(row, _)| *row < oldest) {
            self.rows.pop_first();
        }
        let distance = self.distance(term);
        let delta = distance as i32 - term.display_offset() as i32;
        if delta != 0 {
            term.scroll_display(Scroll::Delta(delta));
        }
        true
    }

    pub(super) fn moved<T: EventListener>(&mut self, term: &Crosswords<T>) {
        self.top = live_start(term) - term.display_offset() as i64;
        self.retain_visible(term);
    }

    pub(super) fn columns(&self) -> usize { self.cols }
}
