//! Shared machine-tree interface for daemon transactions and client projections.
//!
//! Implementations stay private: domain operations do not own effects, and only
//! `MachineStore` coordinates authoritative acceptance, persistence, and publication.

mod appearance;
mod layout;
mod model;
mod operations;
mod paths;
mod persistence;
mod store;

pub use appearance::{Appearance, appearance, note_appearance};
pub use layout::{Axis, PaneNode, PipGeometry, PipPane, Side, Tab, TabId, TabPresentation};
pub use model::{
    AgentFacts, Attachment, CLOSED_TAB_TTL, ClosedTab, LayoutDelta, MAX_CLOSED_TABS, MAX_PANES,
    MAX_WORKSPACES, Machine, PaneRecord, PaneSeed, Remembered, ReopenedTab, TabCloseReason,
    Workspace, unix_now,
};
pub use paths::{
    APPEARANCE_FILE, DATA_DIR_ENV, MACHINE_FILE, adopt_legacy_data_dir, appearance_path,
    default_machine_path,
};
pub use store::{
    FACT_FLUSH_INTERVAL, LivenessProbe, MachineStore, Notify, SubscriberId, Subscription,
    observe_pane, observed_store, publish_observations,
};

#[cfg(test)]
pub(crate) use store::{OBSERVE_SLOT, withdraw_observations};

#[cfg(test)]
mod test_support;
