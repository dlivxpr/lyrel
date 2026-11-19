//! Which files each tab had open in the editor, kept across restarts.
//!
//! The tab tree itself lives in the daemon, and the daemon has no business
//! knowing about editor buffers — a file open in the GUI is not something a
//! shell or a remote server can act on. So this is the GUI's own record, keyed
//! by the tree's [`TabId`], which is stable across restarts: the tab that comes
//! back after a relaunch is the same tab, and gets its files back.
//!
//! The value types and the persistent store live with the workbench editor
//! that produces them; this module keeps `crate::ui::editor_session::` working
//! for the root package.

#[cfg(test)]
pub(crate) use lyrel_workbench_editor::editor_session::get;
pub(crate) use lyrel_workbench_editor::editor_session::{SplitEditor, TabEditor};
pub(crate) use lyrel_workbench_editor::editor_session::{put, remove};
