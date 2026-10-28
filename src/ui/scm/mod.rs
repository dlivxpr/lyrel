//! Source control: the panel, its file rows, the commit box, and the graph.
//!
//! `ScmPanelState` owns repository selection, drafts, commit lifecycle and
//! branch/history/detail transitions. Renderers borrow presentation; window
//! adapters retain confirmations, configuration and cross-capability intents.

pub(crate) mod actions;
pub(crate) mod detail;
pub(crate) mod graph;
pub(crate) mod panel;
pub(crate) mod path;
pub(crate) mod state;
pub(crate) mod status;
pub(crate) mod tree;

pub(crate) use actions::ScmIntent;
pub(crate) use state::ScmPanelState;
