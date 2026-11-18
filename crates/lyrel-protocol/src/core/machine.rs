//! Shared machine-tree interface for daemon transactions and client projections.
//!
//! Implementations stay private: domain operations do not own effects, and only
//! `lyrel-core`'s `MachineStore` coordinates authoritative acceptance,
//! persistence, and publication.
//!
//! The values, the pure reducer ([`operations`]) and the instance paths live
//! here; `lyrel-core::core::machine` re-exports them alongside the
//! persistence-owning store.

mod layout;
mod model;
pub mod operations;
mod paths;

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
