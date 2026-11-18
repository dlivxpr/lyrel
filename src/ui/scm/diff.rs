//! The diff model lives in `lyrel-core` (`core::git::diff`) so the headless
//! server and the GUI parse a patch with the same code. Nothing there has a
//! gpui dependency, and the SCM panel needs the same types the daemon does.
//!
//! This shim re-exports them under the SCM module, where the panel, the
//! overlay and the diff rows are.
pub use lyrel_core::core::git::diff::*;
