//! Shared test fixtures for the lyrel workspace.
//!
//! This crate consolidates the PTY producers, graphics/theme/mouse/glyph and
//! synchronized-capture fixtures, the native-SSH acceptance fixture, and the
//! `lyrel-graphics-fixture` process binary that used to be pulled across
//! package boundaries with `include!` and `#[path]`. Everything here is
//! GPUI-free so `lyrel-core`, `lyrel-server`, `lyrel-cli` and the root GUI
//! crate can consume it through ordinary `[dev-dependencies]` imports.

pub mod fixture_identity;
pub mod glyph_pty;
pub mod hex;
pub mod kitty_media;
pub mod mouse_pty;
pub mod native_ssh;
pub mod pty;
pub mod synchronized_capture;
pub mod theme_pty;

/// Directory holding the shared fixture scripts (`.mjs`) that consumers run on
/// real local or remote PTYs, resolved against this crate's manifest so it
/// works from any consumer package and working directory.
pub fn support_dir() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("support")
}

/// Exact checked-in script payloads, uploaded to fixture hosts through the
/// owning `Host` and verified byte-for-byte there.
pub const KITTY_SSH_MEDIA_MJS: &[u8] = include_bytes!("../support/kitty_ssh_media.mjs");
pub const KITTY_REMOTE_SENDER_MJS: &[u8] = include_bytes!("../support/kitty_remote_sender.mjs");
pub const RING_FLOOD_MJS: &[u8] = include_bytes!("../support/ring_flood.mjs");

/// The shared Kitty helper uploaded next to every `kitty-*` script
/// (`scripts/graphics-fixture-utils.mjs` stays the single source of truth in
/// the workspace root).
pub const GRAPHICS_FIXTURE_UTILS_MJS: &[u8] =
    include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../scripts/graphics-fixture-utils.mjs"));
