//! Borrowed access to the pane's already painted cells. This owns no grid or
//! frame: pointer gestures and copy must not read the newer hidden VT state.
use std::ops::Range;

use rio_vt::crosswords::pos::{Column, Line, Pos};
use rio_vt::crosswords::square::Wide;

use super::element::RenderCell;

pub(super) struct PresentedGrid<'a> {
    cells: &'a [RenderCell],
    cols: usize,
    offset: i32,
}

impl<'a> PresentedGrid<'a> {
    pub(super) fn new(cells: &'a [RenderCell], cols: usize, offset: i32) -> Self {
        Self {
            cells,
            cols: cols.max(1),
            offset,
        }
    }

    fn rows(&self) -> usize {
        self.cells.len() / self.cols
    }

    pub(super) fn link_line(&self, col: usize, row: usize) -> Option<super::link::GridLink> {
        if let Some((uri, runs)) = self.hyperlink(col, row) {
            return Some(super::link::GridLink::Hyperlink(uri, runs));
        }
        let (text, points, click) = self.logical_line(col, row, true)?;
        Some(super::link::GridLink::Text(text, points, click))
    }

    pub(super) fn row_runs(&self, points: &[Pos]) -> Vec<(Pos, Pos)> {
        let mut runs: Vec<(Pos, Pos)> = Vec::new();
        for &point in points {
            let ix = self.index(point);
            let end = if self
                .cells
                .get(ix)
                .is_some_and(|cell| cell.wide == Wide::Wide)
            {
                Pos::new(point.row, Column((point.col.0 + 1).min(self.cols - 1)))
            } else {
                point
            };
            if let Some((_, previous)) = runs.last_mut()
                && previous.row == point.row
                && point.col.0 <= previous.col.0 + 1
            {
                previous.col = previous.col.max(end.col);
            } else {
                runs.push((point, end));
            }
        }
        runs
    }

    fn lead(&self, ix: usize) -> usize {
        let ix = ix.min(self.cells.len().saturating_sub(1));
        match self.cells.get(ix).map(|cell| cell.wide) {
            Some(Wide::Spacer) => ix.saturating_sub(1),
            Some(Wide::LeadingSpacer) => (ix + 1).min(self.cells.len().saturating_sub(1)),
            _ => ix,
        }
    }

    pub(super) fn boundary(&self, col: usize, row: usize, left: bool) -> usize {
        if self.cells.is_empty() {
            return 0;
        }
        let ix = self.lead(row.min(self.rows() - 1) * self.cols + col.min(self.cols - 1));
        ix + if left {
            0
        } else if self.cells[ix].wide == Wide::Wide {
            2
        } else {
            1
        }
    }

    fn wraps(&self, row: usize) -> bool {
        self.cells
            .get((row + 1) * self.cols - 1)
            .is_some_and(|cell| cell.wrapped)
    }

    fn point(&self, ix: usize) -> Pos {
        Pos::new(
            Line((ix / self.cols) as i32 - self.offset),
            Column(ix % self.cols),
        )
    }

    fn index(&self, point: Pos) -> usize {
        (point.row.0 + self.offset) as usize * self.cols + point.col.0
    }

    pub(super) fn is_blank(&self, col: usize, row: usize) -> bool {
        self.cells
            .get(row * self.cols + col)
            .is_none_or(|cell| !cell.spacer && cell.c.is_whitespace())
    }

    /// A logical line and its character-to-cell mapping, allocated only for a
    /// pointer action, never during paint. Hard joins match URL detection only.
    pub(super) fn logical_line(
        &self,
        col: usize,
        row: usize,
        hard: bool,
    ) -> Option<(String, Vec<Pos>, usize)> {
        if row >= self.rows() || col >= self.cols {
            return None;
        }
        let click = self.lead(row * self.cols + col);
        let joins = |row: usize| {
            self.wraps(row)
                || (hard
                    && row + 1 < self.rows()
                    && super::search::is_url_char(self.cells[(row + 1) * self.cols - 1].c)
                    && super::search::is_url_char(self.cells[(row + 1) * self.cols].c)
                    && self.cells[(row + 1) * self.cols].c != '@')
        };
        let mut first = row;
        while first > 0 && row - first < 32 && joins(first - 1) {
            first -= 1;
        }
        let mut last = row;
        while last + 1 < self.rows() && last - first < 32 && joins(last) {
            last += 1;
        }
        let mut text = String::new();
        let mut points = Vec::new();
        let mut clicked = None;
        for ix in first * self.cols..(last + 1) * self.cols {
            let cell = &self.cells[ix];
            if cell.spacer {
                continue;
            }
            if ix == click {
                clicked = Some(points.len());
            }
            text.push(if cell.c == '\0' { ' ' } else { cell.c });
            points.push(self.point(ix));
            if let Some(marks) = &cell.marks {
                for &mark in marks.iter() {
                    text.push(mark);
                    points.push(self.point(ix));
                }
            }
        }
        Some((text, points, clicked?))
    }

    pub(super) fn hyperlink(&self, col: usize, row: usize) -> Option<(String, Vec<(Pos, Pos)>)> {
        if row >= self.rows() || col >= self.cols {
            return None;
        }
        let ix = self.lead(row * self.cols + col);
        let uri = self.cells[ix].hyperlink.as_ref()?.uri();
        let same = |at: usize| {
            self.cells[self.lead(at)]
                .hyperlink
                .as_ref()
                .is_some_and(|link| link.uri() == uri)
        };
        let mut start = ix;
        while start > 0
            && (start % self.cols != 0 || self.wraps(start / self.cols - 1))
            && same(start - 1)
        {
            start -= 1;
        }
        let mut end = ix;
        while end + 1 < self.cells.len()
            && ((end + 1) % self.cols != 0 || self.wraps(end / self.cols))
            && same(end + 1)
        {
            end += 1;
        }
        Some((
            uri.to_owned(),
            super::smart_select::grid_runs(self.point(start), self.point(end), self.cols),
        ))
    }

    pub(super) fn click_range(
        &self,
        col: usize,
        row: usize,
        clicks: usize,
        cfg: &crate::core::config::Config,
    ) -> Range<usize> {
        let Some((text, points, click)) = self.logical_line(col, row, false) else {
            return 0..0;
        };
        let chars: Vec<_> = text.chars().collect();
        if clicks >= 3 {
            return self.index(points[0]) / self.cols * self.cols
                ..(self.index(*points.last().unwrap()) / self.cols + 1) * self.cols;
        }
        if cfg.smart_select
            && let Some((_, runs)) = self.hyperlink(col, row)
        {
            return self.index(runs[0].0)..self.index(runs.last().unwrap().1) + 1;
        }
        let smart = cfg
            .smart_select
            .then(|| {
                super::smart_select::pair_range(&chars, click)
                    .or_else(|| {
                        super::smart_select::is_cjk(chars[click])
                            .then(|| super::smart_select::cjk_word_range(&text, click))
                            .flatten()
                    })
                    .or_else(|| {
                        super::smart_select::smart_range(&text, &chars, click, &cfg.word_separators)
                    })
            })
            .flatten();
        let (start, end) = smart.unwrap_or_else(|| {
            let mut start = click;
            let mut end = click;
            while start > 0 && !cfg.word_separators.contains(chars[start - 1]) {
                start -= 1;
            }
            while end + 1 < chars.len() && !cfg.word_separators.contains(chars[end + 1]) {
                end += 1;
            }
            (start, end)
        });
        let end = self.index(points[end]);
        self.index(points[start])
            ..end
                + if self.cells[end].wide == Wide::Wide {
                    2
                } else {
                    1
                }
    }

    pub(super) fn selected_text(&self, lines: bool) -> Option<String> {
        let start = self.cells.iter().position(|cell| cell.selected)?;
        let end = self.cells.iter().rposition(|cell| cell.selected)?;
        let mut text = String::new();
        for row in start / self.cols..=end / self.cols {
            let from = start.max(row * self.cols);
            let to = (end + 1).min((row + 1) * self.cols);
            let line_start = text.len();
            let mut tab_suffix = false;
            for cell in &self.cells[from..to] {
                if cell.spacer || !cell.selected {
                    continue;
                }
                // Tab already encodes its visual expansion; packed NUL cells
                // following it must not become extra spaces in copied text.
                if tab_suffix && cell.c == '\0' {
                    continue;
                }
                tab_suffix = cell.c == '\t';
                text.push(if cell.c == '\0' { ' ' } else { cell.c });
                if let Some(marks) = &cell.marks {
                    text.extend(marks.iter());
                }
            }
            if !self.wraps(row) {
                let trimmed = text[line_start..].trim_end_matches(' ').len();
                text.truncate(line_start + trimmed);
                if row < end / self.cols {
                    text.push('\n');
                }
            }
        }
        if lines {
            text.push('\n');
        }
        Some(text)
    }
}

#[derive(Default)]
pub(super) struct PresentedSelection {
    anchor: Range<usize>,
    target: Range<usize>,
    clicks: usize,
}

impl PresentedSelection {
    pub(super) fn new(anchor: Range<usize>, clicks: usize) -> Self {
        Self {
            target: anchor.clone(),
            anchor,
            clicks,
        }
    }

    pub(super) fn update(&mut self, target: Range<usize>) {
        self.target = target;
    }

    pub(super) fn clicks(&self) -> usize {
        self.clicks
    }

    pub(super) fn apply(&self, cells: &mut [RenderCell]) -> bool {
        let range = self.anchor.start.min(self.target.start)..self.anchor.end.max(self.target.end);
        for (ix, cell) in cells.iter_mut().enumerate() {
            cell.selected = range.contains(&ix);
        }
        !range.is_empty()
    }
}
