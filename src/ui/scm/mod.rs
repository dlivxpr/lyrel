//! Source control: the panel, its file rows, the commit box, and the graph.
//!
//! `ScmPanelState` owns repository selection, drafts, commit lifecycle and
//! branch/history/detail transitions. Renderers borrow presentation; window
//! adapters retain confirmations, configuration and cross-capability intents.
//!
//! The data half lives here too: [`data`] owns the expensive
//! `status --porcelain=v2 -uall` snapshot, its epoch/watch lifecycle and the
//! shell wiring that answers it ([`LyrelApp::scm_refresh`] and friends);
//! [`status_cache`] owns the cheap per-pane branch/`+N −M` badge and the
//! wiring that answers a pane's typed events; [`diff`] re-exports the shared
//! diff model from `lyrel-core`. Everything read-only that renders or polls
//! reads through the [`crate::ui::scm_snapshot`] seam, never the controller.

pub(crate) mod actions;
pub(crate) mod data;
pub(crate) mod detail;
pub(crate) mod diff;
pub(crate) mod graph;
pub(crate) mod panel;
pub(crate) mod path;
pub(crate) mod state;
pub(crate) mod status;
pub(crate) mod status_cache;
pub(crate) mod tree;

pub(crate) use actions::ScmIntent;
pub(crate) use state::ScmPanelState;
