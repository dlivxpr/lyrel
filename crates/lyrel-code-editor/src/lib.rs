//! GUI-free structured document engines for Lyrel's code editor.
//!
//! Each engine parses one source snapshot into an immutable, source-linked
//! projection and plans format-preserving local edits; none of them holds a
//! second writable document, and none depends on gpui. The workbench keeps the
//! shared text `Document`, history, saving, Host reads, localization and the
//! GPUI presentations; this crate is the part that can be tested without a
//! window or a Host.
//!
//! - [`json`], [`toml`], [`yaml`] and [`xml`] export a bounded projection with
//!   byte ranges, a mutation planner, and share [`state::ProjectionState`] for
//!   the revision/generation guards around an off-thread parse.
//! - [`csv`] owns the complete logical table, stable record/cell identities and
//!   local cell/row/column splices.
//! - [`image`] owns GIF playback and multi-page TIFF decoding under explicit
//!   byte and pixel budgets.
//! - [`drawio`] owns the fixed document probe and page list; rendering stays in
//!   the native engine.
//! - [`tree`] is the format-independent visible-row and source-location model.
//!
//! Glim-derived files keep their provenance records in the corresponding
//! `src/ui/code_editor/*/UPSTREAM.toml`; the full third-party notices remain
//! in the repository's `LICENSE`.

pub mod csv;
pub mod drawio;
pub mod image;
pub mod json;
pub mod state;
pub mod toml;
pub mod tree;
pub mod xml;
pub mod yaml;
