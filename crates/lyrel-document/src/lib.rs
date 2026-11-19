//! GUI-free document engines for Lyrel.
//!
//! One crate owns the pure engines behind the desktop document workbench:
//!
//! - [`text`] (always compiled): file encoding, byte-order mark, per-line
//!   line-ending representation and EditorConfig policy. It is the only module
//!   built with `default-features = false`, so `lyrel-markdown-editor` never
//!   waits on the heavy format parsers.
//! - With `formats`: lossless structured projections and local splice planners
//!   ([`json`], [`yaml`], [`toml`], [`xml`]), the [`csv`] logical table with
//!   cell/row/column mutations, [`image`] GIF playback and multi-page TIFF,
//!   the fixed [`drawio`] document probe, [`epub`] container/chapter
//!   sanitization/reading-state validation/full-book search, and read-only
//!   [`pdf`] page measurement/rasterization/text-layer selection/outline/form
//!   description.
//!
//! Everything here is pure — no GPUI, no `lyrel-core`, no `Host`, no paths.
//! The workbench keeps document identity, Host reads, history, saving and the
//! presentations; the one shared cancellation/revision guard for off-thread
//! work lives in [`state`] as [`CancelToken`] and [`ProjectionState`].
//!
//! Upstream provenance records sit beside the engines they describe:
//! `UPSTREAM-epub.toml` (Glim MIT EPUB reader), `UPSTREAM-pdf.toml` (Zorite
//! MIT `gpui-pdf`) and the per-format records referenced from the workbench;
//! the full third-party notices remain in the repository's `LICENSE`.

pub mod text;

#[cfg(feature = "formats")]
pub mod csv;
#[cfg(feature = "formats")]
pub mod drawio;
#[cfg(feature = "formats")]
pub mod epub;
#[cfg(feature = "formats")]
pub mod image;
#[cfg(feature = "formats")]
pub mod json;
#[cfg(feature = "formats")]
pub mod pdf;
#[cfg(feature = "formats")]
pub mod state;
#[cfg(feature = "formats")]
pub mod toml;
#[cfg(feature = "formats")]
pub mod tree;
#[cfg(feature = "formats")]
pub mod xml;
#[cfg(feature = "formats")]
pub mod yaml;

#[cfg(feature = "formats")]
pub use state::{CancelToken, ProjectionJob, ProjectionResult, ProjectionState};
