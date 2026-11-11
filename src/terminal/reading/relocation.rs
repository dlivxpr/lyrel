//! Hard-line relocation over the existing GUI mirror, never a row-fetch protocol.
use rio_vt::crosswords::Crosswords;
use rio_vt::crosswords::grid::Dimensions;
use rio_vt::crosswords::pos::Column;
use rio_vt::crosswords::square::{CellFlags, Wide};
use rio_vt::event::EventListener;

use super::{ReadingView, live_start};

const PREFIX_CHARS: usize = 256;
const SEARCH_ROWS: i64 = 1024;

pub(super) struct Relocation {
    prefix: Vec<char>,
    offset: usize,
    row_offset: i64,
    blanks: (usize, usize),
    top: i64,
    oldest: i64,
    history: usize,
    cols: usize,
    rows: usize,
    target: (usize, usize),
    resize_revision: u64,
    images_known: bool,
    display_offset: usize,
    scroll: i64,
}

/// A borrowed row source keeps capture faithful to frozen cells, while the
/// candidate search deliberately reads only the newly reflowed mirror.
struct Rows<'a, T: EventListener> {
    term: &'a Crosswords<T>,
    frozen: Option<&'a ReadingView>,
}

impl<T: EventListener> Rows<'_, T> {
    fn wrapped(&self, row: i64) -> bool {
        self.frozen
            .and_then(|reading| reading.absolute_row(row))
            .map_or_else(
                || {
                    self.term.grid[super::grid_line(live_start(self.term), row)]
                        [Column(self.term.columns() - 1)]
                    .contains_cell_flag(CellFlags::WRAPLINE)
                },
                |cells| cells.last().is_some_and(|cell| cell.wrapped),
            )
    }

    /// Count Unicode characters, including attached marks, once per base cell.
    /// Wide/leading spacers have no character; final-row padding is not text.
    fn characters(&self, row: i64, mut visit: impl FnMut(char) -> bool) -> usize {
        let frozen = self.frozen.and_then(|reading| reading.absolute_row(row));
        let line = super::grid_line(live_start(self.term), row);
        let width = frozen.map_or(self.term.columns(), |cells| cells.len());
        let mut end = width;
        if !self.wrapped(row) {
            while end > 0 {
                let blank = if let Some(cells) = frozen {
                    let cell = &cells[end - 1];
                    (cell.c == '\0' || cell.c == ' ') && cell.marks.is_none()
                } else {
                    let cell = &self.term.grid[line][Column(end - 1)];
                    (cell.c() == '\0' || cell.c() == ' ')
                        && cell
                            .extras_id_checked()
                            .and_then(|id| self.term.grid.extras_table.get(id))
                            .is_none_or(|extra| extra.zerowidth.is_empty())
                };
                if !blank {
                    break;
                }
                end -= 1;
            }
        }
        let mut count = 0;
        for col in 0..end {
            let (c, wide, marks) = if let Some(cells) = frozen {
                let cell = &cells[col];
                (cell.c, cell.wide, cell.marks.as_deref().unwrap_or(&[]))
            } else {
                let cell = &self.term.grid[line][Column(col)];
                let marks = cell
                    .extras_id_checked()
                    .and_then(|id| self.term.grid.extras_table.get(id))
                    .map_or(&[][..], |extra| extra.zerowidth.as_slice());
                (cell.c(), cell.wide(), marks)
            };
            if matches!(wide, Wide::Spacer | Wide::LeadingSpacer) {
                continue;
            }
            for c in std::iter::once(if c == '\0' { ' ' } else { c }).chain(marks.iter().copied()) {
                count += 1;
                if !visit(c) {
                    return count;
                }
            }
        }
        count
    }

    fn prefix(&self, first: i64, end: i64) -> Vec<char> {
        let mut prefix = Vec::with_capacity(PREFIX_CHARS);
        for row in first..end {
            self.characters(row, |c| {
                prefix.push(c);
                prefix.len() < PREFIX_CHARS
            });
            if prefix.len() == PREFIX_CHARS || !self.wrapped(row) {
                break;
            }
        }
        prefix
    }

    fn matches_prefix(&self, first: i64, end: i64, prefix: &[char]) -> bool {
        let mut count = 0;
        let mut matched = true;
        for row in first..end {
            self.characters(row, |c| {
                matched = prefix.get(count) == Some(&c);
                count += 1;
                matched && count < PREFIX_CHARS
            });
            if !matched || count == PREFIX_CHARS || !self.wrapped(row) {
                break;
            }
        }
        matched && count == prefix.len()
    }

    fn blank_context(&self, first: i64, last: i64, oldest: i64, end: i64) -> (usize, usize) {
        let blank = |row| !self.wrapped(row) && self.characters(row, |_| false) == 0;
        let before = (oldest..first)
            .rev()
            .take(32)
            .take_while(|&row| blank(row))
            .count();
        let after = (last + 1..end)
            .take(32)
            .take_while(|&row| blank(row))
            .count();
        (before, after)
    }
}

impl Relocation {
    pub(super) fn capture<T: EventListener>(
        reading: &ReadingView,
        term: &Crosswords<T>,
        target: (usize, usize),
        resize_revision: u64,
        images_known: bool,
    ) -> Self {
        let source = Rows {
            term,
            frozen: Some(reading),
        };
        let oldest = term.grid.lines_evicted() as i64;
        let end = live_start(term) + term.screen_lines() as i64;
        let mut first = reading.top;
        while first > oldest && source.wrapped(first - 1) {
            first -= 1;
        }
        let mut last = reading.top;
        while last + 1 < end && source.wrapped(last) {
            last += 1;
        }
        let offset = (first..reading.top)
            .map(|row| source.characters(row, |_| true))
            .sum();
        Self {
            prefix: source.prefix(first, end),
            offset,
            row_offset: reading.top - first,
            blanks: source.blank_context(first, last, oldest, end),
            top: reading.top,
            oldest,
            history: term.grid.history_size(),
            cols: reading.cols,
            rows: reading.captured_rows,
            target,
            resize_revision,
            images_known,
            display_offset: term.display_offset(),
            scroll: 0,
        }
    }

    pub(super) fn retarget(&mut self, target: (usize, usize), revision: u64, images_known: bool) {
        self.target = target;
        self.resize_revision = revision;
        self.images_known |= images_known;
    }

    pub(super) fn ready<T: EventListener>(
        &self,
        term: &Crosswords<T>,
        revision: u64,
        images_revision: u64,
    ) -> bool {
        (term.columns(), term.screen_lines()) == self.target
            && revision != self.resize_revision
            && (!self.images_known || images_revision == revision)
    }

    pub(super) fn sync_offset<T: EventListener>(&mut self, term: &Crosswords<T>) {
        self.display_offset = term.display_offset();
    }

    pub(super) fn did_scroll<T: EventListener>(&mut self, term: &Crosswords<T>) {
        self.scroll += self.display_offset as i64 - term.display_offset() as i64;
        self.sync_offset(term);
    }

    pub(super) fn locate<T: EventListener>(&self, term: &Crosswords<T>) -> i64 {
        let oldest = term.grid.lines_evicted() as i64;
        let live = live_start(term);
        let history = term.grid.history_size() as i64;
        // Retained-history geometry predicts where this fraction of history
        // moved. No absolute old row survives a reflow as an identity.
        let estimate_row = |top: i64| {
            if self.history == 0 {
                live - (self.oldest + self.history as i64 - top)
            } else {
                oldest + (top - self.oldest) * history / self.history as i64
            }
            .clamp(oldest, live)
        };
        let estimate = estimate_row(self.top);
        let hard_estimate = estimate_row(self.top - self.row_offset);
        let geometry = (history - self.history as i64).abs()
            + (self.cols as i64 - term.columns() as i64).abs()
            + self.rows.max(term.screen_lines()) as i64
            + (self.offset / term.columns().max(1)) as i64;
        let window = (geometry * 2 + 64).clamp(64, SEARCH_ROWS);
        let end = live + term.screen_lines() as i64;
        let center = hard_estimate;
        let first = (center - window / 2)
            .max(oldest)
            .min((end - window).max(oldest));
        let limit = (first + window).min(end);
        let source = Rows { term, frozen: None };
        let mut nearest: Option<(i64, i64)> = None;
        for row in first..limit {
            if row > oldest && source.wrapped(row - 1) {
                continue;
            }
            if !source.matches_prefix(row, end, &self.prefix) {
                continue;
            }
            let mut last = row;
            let mut remaining = self.offset;
            let mut top = row;
            loop {
                let count = source.characters(last, |_| true);
                if remaining >= count && count > 0 && source.wrapped(last) && last + 1 < end {
                    remaining -= count;
                    top = last + 1;
                }
                if !source.wrapped(last) || last + 1 == end {
                    break;
                }
                last += 1;
            }
            if source.blank_context(row, last, oldest, end) != self.blanks {
                continue;
            }
            if nearest.is_none_or(|(_, previous)| {
                (row - hard_estimate).abs() < (previous - hard_estimate).abs()
            }) {
                nearest = Some((top, row));
            }
        }
        nearest
            .map(|(top, _)| top)
            .unwrap_or(estimate)
            .saturating_add(self.scroll)
            .clamp(oldest, live)
    }
}
