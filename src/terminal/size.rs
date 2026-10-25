use rio_vt::crosswords::grid::Dimensions;

/// The old emulator exported these and rio-vt does not; neither ever clamped
/// a grid to them internally. They survive only as inputs to the resize-echo
/// check (`remote::grid_holds`), which reasons about what sizes a pane can
/// meaningfully ask for.
pub(crate) const MIN_COLUMNS: usize = 2;
pub(crate) const MIN_SCREEN_LINES: usize = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TermSize {
    pub cols: usize,
    pub rows: usize,
}

impl TermSize {
    pub fn new(cols: usize, rows: usize) -> Self {
        Self {
            cols: cols.max(1),
            rows: rows.max(1),
        }
    }
}

/// The desktop mirror deliberately leaves graphics cell accessors at zero:
/// unsupported sixel/iTerm2 payloads still cannot reserve cells or move its
/// cursor. Kitty geometry and CSI 14t/16t replies use the daemon's ordered
/// `WinSize`, populated with the controller's measured device-pixel cell size.
impl Dimensions for TermSize {
    fn total_lines(&self) -> usize {
        self.rows
    }

    fn screen_lines(&self) -> usize {
        self.rows
    }

    fn columns(&self) -> usize {
        self.cols
    }
}
