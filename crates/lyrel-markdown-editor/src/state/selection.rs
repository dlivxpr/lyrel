//! The editor's selection + presentation-local IME state: the primary byte
//! range (the caret is one end), the extra multi-select ranges, the wrap
//! affinities, the vertical-movement goal column, and the marked (composing)
//! range + retired-composition latch. The `Document` remains the sole
//! authority for source text, history, and the single active `Composition`
//! lease — this is only the view-local side of IME.

use std::ops::Range;

use gpui_kit::Pixels;

use crate::WrapAffinity;

/// Selection + presentation-local IME state for one
/// [`EditorState`](crate::EditorState) presentation.
pub(crate) struct SelectionState {
    /// Extra selection ranges (multi-select / find occurrences), in addition
    /// to the primary `selected_range`.
    pub(crate) extra_selections: Vec<Range<usize>>,
    /// Selection as a byte range; the caret is one end (see
    /// [`EditorState::cursor_offset`](crate::EditorState::cursor_offset)).
    pub(crate) selected_range: Range<usize>,
    pub(crate) selection_reversed: bool,
    /// Which side of a soft-wrap the caret/anchor sits on, so hit-testing and
    /// arrow keys keep their position across a wrapped line.
    pub(crate) caret_affinity: WrapAffinity,
    pub(crate) anchor_affinity: WrapAffinity,
    /// The target x for vertical (Up/Down) movement, so the caret keeps its
    /// column across short lines. `Some` only during a run of Up/Down.
    pub(crate) goal_x: Option<Pixels>,
    /// IME composition range, if any.
    pub(crate) marked_range: Option<Range<usize>>,
    /// A finished lease cannot accept delayed native callbacks after focus
    /// handoff.
    pub(crate) retired_composition: bool,
    /// Whether a mouse drag is extending the selection.
    pub(crate) is_selecting: bool,
}

impl SelectionState {
    pub(crate) fn new() -> Self {
        Self {
            extra_selections: Vec::new(),
            selected_range: 0..0,
            selection_reversed: false,
            caret_affinity: WrapAffinity::Downstream,
            anchor_affinity: WrapAffinity::Downstream,
            goal_x: None,
            marked_range: None,
            retired_composition: false,
            is_selecting: false,
        }
    }
}
