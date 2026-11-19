//! The file tree's click-selection behavior, kept in the root package because
//! it reasons about [`TreeRow`]s — a file-tree rendering type. The selection
//! record itself lives in `lyrel-workbench-editor`, retained per tab by the
//! editor panel state; this module adapts the two.

use std::path::Path;

use super::{HostId, TreeRow};

pub(crate) use lyrel_workbench_editor::code_editor::TreeSelection;

/// A click (or shift-click range, or toggle) on one row. `rows` is the visible
/// row order the range is measured in.
pub(super) fn click(
    selection: &mut TreeSelection,
    host: HostId,
    path: &Path,
    toggle: bool,
    range: bool,
    rows: &[TreeRow],
) {
    selection.ensure_host(host);
    if range {
        let anchor = selection.anchor(host).or(selection.cursor(host));
        if let Some((a, b)) = anchor.and_then(|anchor| {
            Some((
                rows.iter().position(|r| r.entry.path == anchor)?,
                rows.iter().position(|r| r.entry.path == path)?,
            ))
        }) {
            if !toggle {
                selection.clear_paths();
            }
            let paths = rows[a.min(b)..=a.max(b)]
                .iter()
                .filter(|r| r.note.is_none())
                .map(|r| r.entry.path.clone())
                .collect();
            selection.replace_range(paths, path.to_path_buf());
            return;
        }
    }
    if toggle {
        selection.toggle_path(path);
    } else {
        selection.single(host, path.to_path_buf());
    }
}

pub(super) fn primary_modifier(modifiers: gpui_kit::Modifiers) -> bool {
    if cfg!(target_os = "macos") {
        modifiers.platform
    } else {
        modifiers.control
    }
}
