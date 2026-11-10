//! `ratex-gpui` — a structural, MathQuill-style math editor for GPUI, built on the
//! [RaTeX](https://github.com/erweixin/RaTeX) typesetting engine.
//!
//! Pinned MIT `ratex-gpui 0.4.1`, independently maintained inside the desktop editor.
//! GUI-free released RaTeX owns parsing/layout/rasterization; this module owns the native
//! structural tree, cursor, pointer geometry and GPUI Kit/Fast view. Source hashes and local
//! changes are recorded in `UPSTREAM.toml`; application orchestration remains Lyrel-owned.
//! Production structural entry is checked, never the upstream best-effort lossy parser.

pub mod editor;
pub mod render;

pub use editor::latex::parse_latex_checked;

pub use editor::view::{MathAlign, MathEditor, MathNav, MathTheme};
