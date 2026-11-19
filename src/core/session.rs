//! `WorkspaceStore` and the window-view session facade live in
//! `lyrel-ui-base`; this module re-exports them by name (no glob over the
//! protocol `session`, so the store is the only `WorkspaceStore` here).

pub use lyrel_ui_base::session::{
    RemoteRef, RemoteTarget, Session, SessionAxis, SessionPane, SessionTab, WindowView,
    WindowViews, WorkspaceId, WorkspaceStore, crosses_machines, host_for,
};
pub use lyrel_core::host::HostId;
