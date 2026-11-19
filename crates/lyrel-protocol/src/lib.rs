//! Session wire, synchronous IPC clients, and the shared Host contract.
//!
//! The desktop and daemon share these types with thin consumers. Daemon-side
//! execution (including PTY, SSH and the read-only SQLite engine) lives in
//! `lyrel-core` and is not part of this crate's dependency graph.

pub mod client;
pub mod core;
pub mod daemon;
pub mod host;
pub mod terminal_width;
