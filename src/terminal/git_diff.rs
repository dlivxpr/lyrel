//! The diff model moved into `lyrel-core` (`core::git::diff`) so the headless
//! server and the GUI parse a patch with the same code. Nothing here has a gpui
//! dependency, and the SCM panel needs the same types the daemon does.
//!
//! This shim keeps every `crate::terminal::git_diff::…` path in the GUI working.
pub use lyrel_core::core::git::diff::*;
