//! Column-width measurement for the GUI input bar and CLI tables.
//!
//! The implementation lives in `lyrel-protocol` and is re-exported here so
//! `lyrel_core::terminal_width` keeps its existing path; the
//! `terminal-width` Cargo feature was removed and the module is now
//! unconditional (see #326).

pub use lyrel_protocol::terminal_width::*;
