//! Source control: the panel, its file rows, the commit box, and the graph.
//!
//! Every file here hangs `impl LyrelApp` blocks, the same shape `sftp.rs` and
//! `file_tree.rs` use. The directory only keeps the surface from piling into
//! `right_panel.rs`.

pub(crate) mod actions;
pub(crate) mod detail;
pub(crate) mod graph;
pub(crate) mod panel;
pub(crate) mod path;
pub(crate) mod state;
pub(crate) mod status;
pub(crate) mod tree;

pub(crate) use actions::ScmIntent;
pub(crate) use state::{GraphState, ScmPanelState};
