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

/// Cell-pixel accessors stay at their `Default` zero: no client of this
/// terminal can see pixels (there is no window, the grid is mirrored over a
/// socket), and `rio-vt` keys its graphics-plane cursor math off these being
/// nonzero — leaving them zero is what keeps a parsed sixel/iTerm2/kitty
/// payload from moving the text cursor or reserving cells. `CSI 16 t` (the
/// cell-pixel query) is answered with silence by the bridge for the same
/// reason: the truthful answer is "not applicable", not "0x0".
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
