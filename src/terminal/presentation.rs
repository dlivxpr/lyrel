//! Pane presentation facts and borrowed access to already painted cells.
//! Pointer gestures and copy must not read the newer hidden VT state.
use std::ops::Range;

use rio_vt::crosswords::pos::{Column, Line, Pos};
use rio_vt::crosswords::square::Wide;

use super::element::RenderCell;

/// The existing parse/paint metadata mutex also carries the one-dimensional
/// tab map. Rio exposes tab mutations through Handler, but no tab-map reader.
pub(super) struct ParsePresentation {
    pub(super) cursor: Option<Pos>,
    pub(super) reading_generation: u64,
    pub(super) keyboard_generation: u64,
    pub(super) resize_revision: u64,
    pub(super) resize_images_revision: u64,
    /// This pane's application-claimed pointer shape (OSC 22), kept at the
    /// paint-seam facts the frame already reads. `None` is "no application
    /// owns the pointer": the terminal's own policy applies.
    pub(super) pointer: Option<gpui_kit::CursorStyle>,
    /// Whether this pane's negotiated endpoint admits static Sixel. DA1 is
    /// answered by the attached client, so it must describe the endpoint that
    /// actually decodes and retains the image — never this build's own
    /// capability when a routed remote server does not have it.
    pub(super) endpoint_sixel: bool,
    tab_stops: Vec<bool>,
}

impl ParsePresentation {
    pub(super) fn new(cols: usize) -> Self {
        let mut facts = Self {
            cursor: None,
            reading_generation: 0,
            keyboard_generation: 0,
            resize_revision: 0,
            resize_images_revision: 0,
            pointer: None,
            endpoint_sixel: false,
            tab_stops: Vec::new(),
        };
        facts.resize(cols);
        facts
    }

    /// Records this pane's application pointer claim (OSC 22). Returns
    /// whether the visible claim changed — a same-shape or already-released
    /// release owes no repaint.
    pub(super) fn set_pointer(&mut self, pointer: Option<gpui_kit::CursorStyle>) -> bool {
        let changed = self.pointer != pointer;
        self.pointer = pointer;
        changed
    }

    /// Destructive VT transitions are recorded while parsing, including an
    /// alternate-screen round trip entirely inside one output batch.
    pub(super) fn invalidate_reading(&mut self) {
        self.reading_generation = self.reading_generation.wrapping_add(1);
        self.cursor = None;
    }

    pub(super) fn invalidate_keyboard(&mut self) {
        self.keyboard_generation = self.keyboard_generation.wrapping_add(1);
    }

    pub(super) fn resize(&mut self, cols: usize) {
        self.resize_revision = self.resize_revision.wrapping_add(1);
        self.ensure_tab_stops(cols);
    }

    /// Keep the tab map the same width as the grid without claiming a resize.
    /// HTS/TBC and RIS only change tab stops; the ordered graphics `Screen`
    /// pairs with [`Self::resize_revision`], so a tab mutation must not
    /// advance it.
    pub(super) fn ensure_tab_stops(&mut self, cols: usize) {
        let previous = self.tab_stops.len();
        if previous == cols {
            return;
        }
        self.tab_stops.resize(cols, false);
        for col in previous..cols {
            self.tab_stops[col] = col.is_multiple_of(8);
        }
    }

    pub(super) fn reset_tabs(&mut self, cols: usize) {
        self.ensure_tab_stops(cols);
        for (col, stop) in self.tab_stops.iter_mut().enumerate() {
            *stop = col.is_multiple_of(8);
        }
    }

    pub(super) fn set_tab_stop(&mut self, col: usize) {
        self.tab_stops[col] = true;
    }

    pub(super) fn clear_tab_stop(&mut self, col: usize) {
        self.tab_stops[col] = false;
    }

    pub(super) fn clear_all_tab_stops(&mut self) {
        self.tab_stops.fill(false);
    }

    pub(super) fn is_tab_stop(&self, col: usize) -> bool {
        self.tab_stops
            .get(col)
            .copied()
            .unwrap_or_else(|| col.is_multiple_of(8))
    }
}

#[derive(Clone, Copy)]
enum PresentedCells<'a> {
    Rendered(&'a [RenderCell]),
    Reading(&'a super::reading::ReadingView, i64, i64),
}

pub(super) struct PresentedGrid<'a> {
    cells: PresentedCells<'a>,
    cols: usize,
    offset: i32,
    viewport_row: usize,
}

struct PresentedCell<'a> {
    c: char,
    marks: Option<&'a [char]>,
    hyperlink: Option<&'a rio_vt::crosswords::square::Hyperlink>,
    wide: Wide,
    spacer: bool,
    wrapped: bool,
}

impl<'a> PresentedGrid<'a> {
    pub(super) fn new(cells: &'a [RenderCell], cols: usize, offset: i32) -> Self {
        Self {
            cells: PresentedCells::Rendered(cells),
            cols: cols.max(1),
            offset,
            viewport_row: 0,
        }
    }

    pub(super) fn reading(reading: &'a super::reading::ReadingView) -> Self {
        let (first, last) = reading.bounds();
        let viewport_row = (reading.top() - first) as usize;
        Self {
            cells: PresentedCells::Reading(reading, first, last),
            cols: reading.columns(),
            offset: viewport_row as i32,
            viewport_row,
        }
    }

    pub(super) fn absolute_origin(&self) -> usize {
        match self.cells {
            PresentedCells::Rendered(_) => 0,
            PresentedCells::Reading(_, first, _) => first as usize * self.cols,
        }
    }

    fn len(&self) -> usize {
        match self.cells {
            PresentedCells::Rendered(cells) => cells.len(),
            PresentedCells::Reading(_, first, last) => (last - first + 1) as usize * self.cols,
        }
    }

    fn cell(&self, ix: usize) -> PresentedCell<'a> {
        match self.cells {
            PresentedCells::Rendered(cells) => {
                if let Some(cell) = cells.get(ix) {
                    return PresentedCell {
                        c: cell.c,
                        marks: cell.marks.as_deref(),
                        hyperlink: cell.hyperlink.as_ref(),
                        wide: cell.wide,
                        spacer: cell.spacer,
                        wrapped: cell.wrapped,
                    };
                }
            }
            PresentedCells::Reading(reading, first, _) => {
                if let Some(cell) = reading
                    .absolute_row(first + (ix / self.cols) as i64)
                    .and_then(|row| row.get(ix % self.cols))
                {
                    let placeholder = cell.c == '\u{10eeee}';
                    return PresentedCell {
                        c: if placeholder { ' ' } else { cell.c },
                        marks: if placeholder {
                            None
                        } else {
                            cell.marks.as_deref()
                        },
                        hyperlink: cell.hyperlink.as_ref(),
                        wide: cell.wide,
                        spacer: matches!(cell.wide, Wide::Spacer | Wide::LeadingSpacer),
                        wrapped: cell.wrapped,
                    };
                }
            }
        }
        PresentedCell {
            c: '\0',
            marks: None,
            hyperlink: None,
            wide: Wide::Narrow,
            spacer: false,
            wrapped: false,
        }
    }

    fn rows(&self) -> usize {
        self.len() / self.cols
    }

    pub(super) fn link_line(&self, col: usize, row: usize) -> Option<super::link::GridLink> {
        let row = row + self.viewport_row;
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
            let end = if self.cell(ix).wide == Wide::Wide {
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
        let ix = ix.min(self.len().saturating_sub(1));
        match self.cell(ix).wide {
            Wide::Spacer => ix.saturating_sub(1),
            Wide::LeadingSpacer => (ix + 1).min(self.len().saturating_sub(1)),
            _ => ix,
        }
    }

    pub(super) fn boundary(&self, col: usize, row: usize, left: bool) -> usize {
        if self.len() == 0 {
            return 0;
        }
        let ix =
            (row + self.viewport_row).min(self.rows() - 1) * self.cols + col.min(self.cols - 1);
        // A leading spacer still belongs to this physical row. Simple
        // selection must preserve that row's break before the wide glyph.
        if self.cell(ix).wide == Wide::LeadingSpacer {
            return ix + usize::from(!left);
        }
        let ix = self.lead(ix);
        ix + if left {
            0
        } else if self.cell(ix).wide == Wide::Wide {
            2
        } else {
            1
        }
    }

    fn wraps(&self, row: usize) -> bool {
        self.cell((row + 1) * self.cols - 1).wrapped
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
        let cell = self.cell((row + self.viewport_row) * self.cols + col);
        !cell.spacer && (cell.c == '\0' || cell.c.is_whitespace())
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
                    && super::search::continues_url(
                        self.cell((row + 1) * self.cols - 1).c,
                        self.cell((row + 1) * self.cols).c,
                    ))
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
            let cell = self.cell(ix);
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
        let uri = self.cell(ix).hyperlink?.uri();
        let same = |at: usize| {
            self.cell(self.lead(at))
                .hyperlink
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
        while end + 1 < self.len()
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
        let row = row + self.viewport_row;
        if row >= self.rows() || col >= self.cols {
            return 0..0;
        }
        if clicks >= 3 {
            // Lines can include a completely blank row or a lone leading
            // spacer, neither of which has a character-to-cell click point.
            let mut first = row;
            while first > 0 && self.wraps(first - 1) {
                first -= 1;
            }
            let mut last = row;
            while last + 1 < self.rows() && self.wraps(last) {
                last += 1;
            }
            return first * self.cols..(last + 1) * self.cols;
        }
        let Some((text, points, click)) = self.logical_line(col, row, false) else {
            return 0..0;
        };
        let chars: Vec<_> = text.chars().collect();
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
                + if self.cell(end).wide == Wide::Wide {
                    2
                } else {
                    1
                }
    }

    pub(super) fn selected_text(&self, lines: bool) -> Option<String> {
        let PresentedCells::Rendered(cells) = self.cells else {
            return None;
        };
        let start = cells.iter().position(|cell| cell.selected)?;
        let end = cells.iter().rposition(|cell| cell.selected)?;
        Some(copy_rows(
            (start / self.cols..=end / self.cols).map(|row| {
                let from = start.max(row * self.cols);
                let to = (end + 1).min((row + 1) * self.cols);
                (
                    self.wraps(row),
                    cells[from..to].iter().map(|cell| CopyCell {
                        c: cell.c,
                        marks: cell.marks.as_deref(),
                        spacer: cell.spacer,
                        selected: cell.selected,
                        tab_stop: cell.tab_stop,
                    }),
                )
            }),
            lines,
        ))
    }
}

/// Borrow only copy semantics. Frozen rows never allocate a second styled
/// RenderCell buffer (or clone marks) just to serialize a large selection.
pub(super) struct CopyCell<'a> {
    pub(super) c: char,
    pub(super) marks: Option<&'a [char]>,
    pub(super) spacer: bool,
    pub(super) selected: bool,
    pub(super) tab_stop: bool,
}

pub(super) fn copy_rows<'a, Rows, Cells>(rows: Rows, lines: bool) -> String
where
    Rows: IntoIterator<Item = (bool, Cells)>,
    Cells: IntoIterator<Item = CopyCell<'a>>,
{
    let mut text = String::new();
    let mut pending_spaces = 0;
    let mut pending_breaks = 0;
    for (wraps, cells) in rows {
        let mut row_has_content = false;
        let mut tab_suffix = false;
        for cell in cells {
            if cell.spacer || !cell.selected {
                continue;
            }
            // A Tab encodes only the run up to the captured next stop.
            if tab_suffix && cell.c == '\0' && !cell.tab_stop {
                continue;
            }
            tab_suffix = cell.c == '\t';
            if matches!(cell.c, ' ' | '\0') && cell.marks.is_none_or(|marks| marks.is_empty()) {
                pending_spaces += 1;
                continue;
            }
            // Commit interior blanks only when later selected text exists.
            text.extend(std::iter::repeat_n('\n', pending_breaks));
            pending_breaks = 0;
            text.extend(std::iter::repeat_n(' ', pending_spaces));
            pending_spaces = 0;
            text.push(if cell.c == '\0' { ' ' } else { cell.c });
            if let Some(marks) = cell.marks {
                text.extend(marks.iter());
            }
            row_has_content = true;
        }
        if !row_has_content || !wraps {
            pending_breaks += 1;
        }
        if !wraps {
            pending_spaces = 0;
        }
    }
    if lines {
        text.push('\n');
    }
    text
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

    pub(super) fn range(&self) -> Range<usize> {
        self.anchor.start.min(self.target.start)..self.anchor.end.max(self.target.end)
    }

    pub(super) fn apply_from(&self, cells: &mut [RenderCell], start: usize) -> bool {
        let range = self.range();
        let mut selected = false;
        for (ix, cell) in cells.iter_mut().enumerate() {
            cell.selected = range.contains(&(start + ix));
            selected |= cell.selected;
        }
        selected
    }
}
