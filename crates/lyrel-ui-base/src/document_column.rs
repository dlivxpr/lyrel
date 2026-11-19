//! Layout constants for a document column's header cells — a file, the `+N`
//! button, the terminal pill. Owned here so the root shell's document column
//! and the workbench editor's strip picker give their header rows the same
//! height, corner, cap width and text step.

/// Height of a header cell — a file, the `+N` button, the terminal pill.
pub const HEADER_CELL_H: f32 = 28.;
/// Their corner.
pub const HEADER_CELL_RADIUS: f32 = 7.;
/// The widest a name may make its cell before it is elided.
pub const HEADER_CELL_MAX_W: f32 = 168.;
/// Header text: a half step under the body size, like the rail's rows.
pub const HEADER_CELL_TEXT: f32 = 12.5 / 16.;
