use super::*;
use gpui_kit::Focusable as _;

/// One active projection, never a per-project listing store. Host listings remain
/// authoritative; selection and decoration only affect the viewport renderer.
pub(super) struct DerivedRows {
    host: HostId,
    roots: Vec<PathBuf>,
    expanded: HashSet<PathBuf>,
    hidden: bool,
    searching: bool,
    rows: Rc<Vec<TreeRow>>,
}

impl FileTreeState {
    pub(super) fn invalidate_rows(&mut self) {
        self.derived_rows.get_mut().take();
    }

    pub(super) fn logical_rows(
        &self,
        host: HostId,
        roots: &[PathBuf],
        expanded: &HashSet<PathBuf>,
        searching: bool,
    ) -> Rc<Vec<TreeRow>> {
        let mut cached = self.derived_rows.borrow_mut();
        if let Some(held) = cached.as_ref()
            && held.host == host
            && held.roots == roots
            && held.expanded == *expanded
            && held.hidden == self.show_hidden
            && held.searching == searching
        {
            return Rc::clone(&held.rows);
        }
        let rows = Rc::new(if searching {
            search_rows(&self.search)
        } else {
            self.visible_rows(host, roots, expanded)
        });
        *cached = Some(DerivedRows {
            host,
            roots: roots.to_vec(),
            expanded: expanded.clone(),
            hidden: self.show_hidden,
            searching,
            rows: Rc::clone(&rows),
        });
        rows
    }
}

#[derive(PartialEq)]
pub(super) enum PresentedRow {
    Tree(usize),
    Names(usize),
    Contents(crate::ui::panel_search::PanelSearchRow),
    Gap,
    Blank,
}

pub(super) struct TreeLayout {
    logical: Rc<Vec<TreeRow>>,
    contents: Rc<Vec<crate::ui::panel_search::PanelSearchRow>>,
    edit: Option<(TreeEditKind, PathBuf, gpui_kit::EntityId)>,
    rows: Rc<Vec<PresentedRow>>,
}

impl LyrelApp {
    pub(super) fn sync_tree_layout(
        &mut self,
        logical: Rc<Vec<TreeRow>>,
        contents: Rc<Vec<crate::ui::panel_search::PanelSearchRow>>,
        searching: bool,
        cx: &App,
    ) -> Rc<Vec<PresentedRow>> {
        let edit = self.file_tree.editing.as_ref().map(|edit| match edit {
            TreeEdit::NewFile { dir, input } => (TreeEditKind::NewFile, dir.clone(), input.entity_id()),
            TreeEdit::NewFolder { dir, input } => (TreeEditKind::NewFolder, dir.clone(), input.entity_id()),
            TreeEdit::Rename { path, input } => (TreeEditKind::Rename, path.clone(), input.entity_id()),
        });
        if let Some(held) = &self.file_tree.layout
            && Rc::ptr_eq(&held.logical, &logical)
            && Rc::ptr_eq(&held.contents, &contents)
            && held.edit == edit
        {
            return Rc::clone(&held.rows);
        }
        let mut rows = Vec::new();
        if searching && !logical.is_empty() {
            rows.push(PresentedRow::Names(logical.iter().filter(|r| r.note.is_none()).count()));
        }
        if logical.is_empty() && !searching {
            rows.push(PresentedRow::Blank);
        }
        rows.extend((0..logical.len()).map(PresentedRow::Tree));
        if searching && !contents.is_empty() {
            rows.push(PresentedRow::Gap);
            rows.extend(contents.iter().cloned().map(PresentedRow::Contents));
        }
        let held = self.file_tree.layout.as_ref();
        let old = held.map(|held| held.rows.as_slice()).unwrap_or(&[]);
        let same = |a: &PresentedRow, b: &PresentedRow| match (a, b) {
            (PresentedRow::Tree(a), PresentedRow::Tree(b)) => {
                held.is_some_and(|held| held.logical[*a] == logical[*b])
            }
            _ => a == b,
        };
        let prefix = old.iter().zip(&rows).take_while(|(a, b)| same(a, b)).count();
        let rest = old.len().min(rows.len()) - prefix;
        let suffix = old[old.len() - rest..].iter().rev()
            .zip(rows[rows.len() - rest..].iter().rev())
            .take_while(|(a, b)| same(a, b)).count();
        // Splice preserves the logical top row when entries are inserted above
        // the viewport. Unlike reset, it does not undo deliberate scrolling.
        let list = &self.file_tree.list;
        let top = list.logical_scroll_top();
        let anchor = old.get(top.item_ix).and_then(|old| {
            rows.iter().position(|row| same(old, row))
                .map(|item_ix| gpui_kit::ListOffset { item_ix, offset_in_item: top.offset_in_item })
        });
        list.splice(prefix..old.len() - suffix, rows.len() - prefix - suffix);
        if held.is_some_and(|held| held.edit != edit) {
            for (ix, row) in rows.iter().enumerate() {
                if let PresentedRow::Tree(n) = row {
                    let path = &logical[*n].entry.path;
                    if edit.as_ref().is_some_and(|(_, edited, _)| edited == path)
                        || held.and_then(|held| held.edit.as_ref()).is_some_and(|(_, edited, _)| edited == path)
                    {
                        // The same domain row now measures a different height.
                        list.splice(ix..ix + 1, 1);
                    }
                }
            }
        }
        if let Some(edit) = &self.file_tree.editing {
            let path = match edit {
                TreeEdit::NewFile { dir, .. } | TreeEdit::NewFolder { dir, .. } => dir,
                TreeEdit::Rename { path, .. } => path,
            };
            if let Some(ix) = rows.iter().position(|row| matches!(row,
                PresentedRow::Tree(n) if logical[*n].note.is_none() && logical[*n].entry.path == *path))
            {
                // Keep the single measured folder+input item alive while its
                // input owns focus, even if the user scrolls it off screen.
                list.splice_focusable(ix..ix + 1, [Some(edit.input().read(cx).focus_handle(cx))]);
            }
        }
        list.clone().with_uniform_item_height(px(TREE_ROW_H));
        if let Some(anchor) = anchor {
            list.scroll_to(anchor);
        }
        if let Some((_, path, _)) = &edit
            && held.map(|held| &held.edit) != Some(&edit)
            && let Some(ix) = rows.iter().position(|row| matches!(row,
                PresentedRow::Tree(n) if logical[*n].note.is_none() && logical[*n].entry.path == *path))
        {
            // The item now includes the input below its folder. Keep that box,
            // not merely the folder's old height, within the viewport.
            list.scroll_to(gpui_kit::ListOffset { item_ix: ix, offset_in_item: px(0.) });
        }
        let rows = Rc::new(rows);
        self.file_tree.layout = Some(TreeLayout { logical, contents, edit, rows: Rc::clone(&rows) });
        rows
    }
}
