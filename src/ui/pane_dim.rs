//! How much of a pane's picture an inactive pane paints — the two opacity
//! values the pane chrome blends with the window background. Defined away
//! from the pane controller so nothing terminal-side needs the pane tree for
//! a number and a future split has them without dragging the pane along.

/// How opaque a dragged (lifted) pane's terminal paints, blended toward the
/// window background. The terminal reads it through `TerminalView::dim`; kept
/// here so that field's docs can point at the real numbers instead of
/// duplicating them.
pub const LIFTED_DIM: f32 = 0.45;
/// How opaque an unfocused pane in a split paints while `dim_inactive_panes`
/// is on.
pub const INACTIVE_DIM: f32 = 0.55;
