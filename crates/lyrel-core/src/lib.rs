pub use lyrel_protocol::{self, client, terminal_width};
pub mod core;
pub mod daemon;
pub mod host;

/// The fixed ConPTY pin and its verifier, shared by the build script, the
/// Windows pane startup, and the standalone updater: all three validate the
/// same pinned pair against the same manifest rules, and the updater accepts
/// a release-authenticated target manifest so a newer official pair installs
/// through an older helper. Windows-only like the pinned package itself.
#[cfg(windows)]
pub mod conpty_files;
