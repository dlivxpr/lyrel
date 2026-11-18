//! Host facade: `lyrel-protocol` owns the unified [`Host`] seam, the C-free
//! base `LocalHost`, the transfer/read/search/session-operation machinery and
//! every wire value; this crate re-exports them all at the paths the rest of
//! the workspace has always used, and adds the heavyweight adapters:
//!
//! - [`local`] — the full local Host constructor (read-only SQLite engine and
//!   whole-host metrics sampler attached; see the module for why production
//!   entries must use it instead of the protocol base's bare
//!   `LocalHost::new`),
//! - [`metrics`] — the sysinfo-backed whole-host sampler behind the wire
//!   values,
//! - [`sqlite`] — the read-only `rusqlite` browsing engine,
//! - [`remote`] — the SSH transport,
//! - [`server`] — the control/pane server machinery,
//! - [`conformance`] — the shared Host conformance suite.

#[cfg(any(test, feature = "test-support"))]
pub mod conformance;

pub use lyrel_protocol::host::*;

pub mod local;
pub mod metrics;
pub mod remote;
pub mod server;
pub mod sqlite;
