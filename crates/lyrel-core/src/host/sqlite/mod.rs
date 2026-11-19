//! Read-only SQLite browsing on the owning Host.
//!
//! `lyrel-protocol` owns the wire values, the bounded-lifetime constants, the
//! operation-scoped [`SqliteCancel`] and the [`SqliteEngine`] seam the Host
//! trait speaks through; this crate owns the engine itself — the bundled-C
//! `rusqlite` read view pool, the identifier-quoting query builder and the
//! read-view tests. A full local Host gets [`SqliteService`] injected at
//! construction; the protocol base Host without an engine reports
//! `Unsupported` instead of pretending to browse.

pub use lyrel_protocol::host::sqlite::*;

mod browser;
mod service;

pub use service::{SqliteLimits, SqliteService};

#[cfg(test)]
mod tests;
