//! Shared machine-tree interface for daemon transactions and client projections.
//!
//! The values, the pure reducer and the instance paths live in
//! `lyrel-protocol` and are re-exported here unchanged so that
//! `lyrel_core::core::machine` keeps its existing paths. This crate owns the
//! effectful half: the on-disk persistence ([`persistence`]), the appearance
//! hint, and the `MachineStore` that coordinates authoritative acceptance,
//! persistence, and publication.

mod appearance;
mod persistence;
mod store;
#[cfg(test)]
mod test_support;

pub use appearance::{Appearance, appearance, note_appearance};
pub use lyrel_protocol::core::machine::*;
pub use store::{
    FACT_FLUSH_INTERVAL, LivenessProbe, MachineStore, Notify, SubscriberId, Subscription,
    observe_pane, observed_store, publish_observations,
};

#[cfg(test)]
pub(crate) use store::{OBSERVE_SLOT, withdraw_observations};
