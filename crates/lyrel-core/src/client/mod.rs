//! The synchronous wire clients (`ControlClient`, `PaneClient`,
//! `PaneSession`) live in `lyrel-protocol`; this facade keeps the existing
//! `lyrel_core::client::*` paths working for the GUI, the CLI, and the
//! server tests.

pub use lyrel_protocol::client::*;
