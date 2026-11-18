//! GIF playback and multi-page TIFF decoding under explicit budgets.
//!
//! The decoders own pixels and frame/page state only. Scheduling, visibility,
//! texture upload and document-localization stay in the workbench.

pub mod gif;
pub mod tiff;

pub use self::gif::{GifError, GifPlayback};
pub use self::tiff::{TiffError, TiffPageError, TiffPages};

/// The pixel budget shared by the workbench's image documents and Markdown
/// previews. TIFF page dimensions and the animation canvas are bounded by it.
pub const MAX_PIXELS: u64 = 16 * 1024 * 1024;
