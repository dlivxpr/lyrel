//! Source control: the panel, its file rows, the commit box, and the graph.
//!
//! `ScmPanelState` owns repository selection, commit drafts and commit lifecycle.
//! Window adapters compose Host work, confirmations and cross-capability intents;
//! branch, graph and detail keep their existing refresh policies.

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
