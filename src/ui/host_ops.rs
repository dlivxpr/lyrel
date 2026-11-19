//! Off-thread `Host` execution and error notifications live in
//! `lyrel-ui-base`; this facade keeps `crate::ui::host_ops::` working.

pub use lyrel_ui_base::host_ops::{
    ByHost, Entry, Host, HostId, HostOps, InFlight, MTime, Meta, Output, SearchHit, SharedHost,
    WatchSub, explain_io, failure, off_thread,
};
