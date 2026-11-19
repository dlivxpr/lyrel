//! The editor split in two: two groups of files side by side, each with a
//! file in front, the way VS Code's ⌘\ splits an editor.
//!
//! **One group has the focus, and it is `TabCode`'s own `files`/`active`.**
//! The other one waits in [`OtherGroup`]. Moving the focus swaps the two, so
//! everything that acts on "the file in front" — saving, go to line, the
//! language servers, the change markers, multiple cursors, back/forward, the
//! strip in the header — acts on the focused group without knowing a split
//! exists. The header's strip is the focused group's; the other group shows
//! its file in front and its breadcrumbs.
//!
//! **The same file in both groups.** Every buffer has a document-owned
//! `BufferId`; language servers, gutter markers and async work retain that ID.
//! Ordinary code still has one `EditorState`: the focused group draws it and
//! the other shows the existing click-to-focus placeholder.
//!
//! Native Markdown retains an independent `lyrel_markdown_editor::EditorState`
//! for each tab/group relation. Both sides share a single `Document` and
//! undo history but not caret, selection, focus or scrolling. Neither side
//! mirrors text into another authoritative editor.
//!
//! A group whose last file closes goes away, and the split with it.

use gpui_kit::Focusable as _;
use gpui_kit::{Context, Window};
use lyrel_core::core::machine::TabId;

use super::{BufferId, EditorPanelState, TabCode};

/// Retained identity follows a group through focus swaps and split collapse.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct GroupId(u64);

impl Default for GroupId {
    fn default() -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

/// The group without the focus.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OtherGroup {
    pub id: GroupId,
    pub files: Vec<BufferId>,
    pub active: usize,
    /// Whether the group *with* the focus is the left one.
    pub focus_left: bool,
}

impl OtherGroup {
    fn active_id(&self) -> Option<BufferId> {
        self.files.get(self.active).copied()
    }
}

/// Takes `id` out of one group's strip, bringing its neighbour forward the
/// way `TabCode::forget` does.
fn remove(files: &mut Vec<BufferId>, active: &mut usize, id: BufferId) -> bool {
    let Some(pos) = files.iter().position(|f| *f == id) else {
        return false;
    };
    files.remove(pos);
    if *active > pos || *active >= files.len() {
        *active = active.saturating_sub(1);
    }
    true
}

/// One side of the editor, as it is drawn.
pub struct Side {
    pub files: Vec<BufferId>,
    pub active: usize,
    pub focused: bool,
}

impl TabCode {
    pub fn is_split(&self) -> bool {
        self.split.is_some()
    }

    /// Whether either group shows this buffer.
    pub fn shows(&self, id: BufferId) -> bool {
        self.files.contains(&id) || self.split.as_ref().is_some_and(|o| o.files.contains(&id))
    }

    /// How many groups show this buffer: 0, 1 or 2.
    pub fn views_of(&self, id: BufferId) -> usize {
        usize::from(self.files.contains(&id))
            + usize::from(self.split.as_ref().is_some_and(|o| o.files.contains(&id)))
    }

    /// Every buffer either group shows, once each, the focused group's first.
    pub fn all_files(&self) -> Vec<BufferId> {
        let mut all = self.files.clone();
        if let Some(other) = &self.split {
            for id in &other.files {
                if !all.contains(id) {
                    all.push(*id);
                }
            }
        }
        all
    }

    /// The file in front of the group without the focus.
    pub fn other_active_id(&self) -> Option<BufferId> {
        self.split.as_ref().and_then(OtherGroup::active_id)
    }

    /// Gives the focus to the other group.
    pub fn swap_focus(&mut self) {
        let Some(other) = self.split.as_mut() else {
            return;
        };
        std::mem::swap(&mut self.files, &mut other.files);
        std::mem::swap(&mut self.active, &mut other.active);
        std::mem::swap(&mut self.group, &mut other.id);
        other.focus_left = !other.focus_left;
    }

    /// Which side the focused group is drawn on: 0 left, 1 right. What tells
    /// the two groups' elements apart.
    pub fn focused_slot(&self) -> usize {
        usize::from(!self.focus_is_left())
    }

    /// Whether the focused group is on the left. `true` when not split.
    pub fn focus_is_left(&self) -> bool {
        self.split.as_ref().is_none_or(|o| o.focus_left)
    }

    pub fn focused_group(&self) -> GroupId {
        self.group
    }

    /// Shows `id` in the other group and gives that group the focus —
    /// splitting the editor first if it is not split yet, with the new group
    /// on the right.
    pub fn split_with(&mut self, id: BufferId) {
        match self.split.as_mut() {
            None => {
                self.split = Some(OtherGroup {
                    id: std::mem::take(&mut self.group),
                    files: std::mem::replace(&mut self.files, vec![id]),
                    active: std::mem::replace(&mut self.active, 0),
                    focus_left: false,
                });
            }
            Some(_) => {
                self.swap_focus();
                self.show(id);
            }
        }
    }

    /// Closes a file in the focused group only. The group goes if it was its
    /// last file, and the focus passes to the one that is left.
    pub fn close_in_focused(&mut self, id: BufferId) -> bool {
        let closed = self.forget(id);
        self.collapse();
        closed
    }

    /// A retired group cannot close a replacement, even on the same side.
    pub fn close_in_group(&mut self, id: BufferId, group: GroupId) -> bool {
        let closed = if self.group == group {
            self.forget(id)
        } else if let Some(other) = self.split.as_mut().filter(|other| other.id == group) {
            remove(&mut other.files, &mut other.active, id)
        } else {
            false
        };
        self.collapse();
        closed
    }

    /// Takes a buffer out of both groups, for a buffer that is going away.
    pub fn forget_everywhere(&mut self, id: BufferId) -> bool {
        let mut gone = self.forget(id);
        if let Some(other) = self.split.as_mut() {
            gone |= remove(&mut other.files, &mut other.active, id);
        }
        self.collapse();
        gone
    }

    /// A group with nothing left in it closes.
    fn collapse(&mut self) {
        let Some(other) = self.split.as_ref() else {
            return;
        };
        if other.files.is_empty() {
            self.split = None;
        } else if self.files.is_empty() {
            let other = self.split.take().unwrap_or_default();
            self.files = other.files;
            self.active = other.active;
            self.group = other.id;
        }
        self.retain_strip_nav();
    }

    /// Lays a tab's recorded groups back out once their files have loaded.
    ///
    /// `restored` are the buffers the restore opened, already listed in the
    /// focused group (they arrive in the background). Anything else the tab
    /// shows was opened while they loaded, and is kept:
    ///
    /// - A tab split in the meantime keeps its layout; the restored files
    ///   stay where they arrived, behind the file in front.
    /// - Otherwise the recorded layout is put back, the files opened in the
    ///   meantime join the focused group, and the last of them stays in front.
    ///
    /// `true` when nothing had been opened in the meantime.
    pub fn restore_groups(
        &mut self,
        restored: &[BufferId],
        left: (Vec<BufferId>, usize),
        right: Option<(Vec<BufferId>, usize, bool)>,
    ) -> bool {
        if self.is_split() {
            return false;
        }
        let extras: Vec<BufferId> = self
            .files
            .iter()
            .copied()
            .filter(|id| !restored.contains(id))
            .collect();
        let front = self.active_id().filter(|id| extras.contains(id));
        let (left, left_active) = left;
        let right = right.filter(|(files, ..)| !files.is_empty());
        match (left.is_empty(), right) {
            (false, Some((right, right_active, focused))) => {
                self.files = left;
                self.active = left_active;
                self.split = Some(OtherGroup {
                    id: GroupId::default(),
                    files: right,
                    active: right_active,
                    focus_left: true,
                });
                if focused {
                    self.swap_focus();
                }
            }
            (false, None) => {
                self.files = left;
                self.active = left_active;
            }
            (true, Some((right, right_active, _))) => {
                self.files = right;
                self.active = right_active;
            }
            (true, None) => {}
        }
        for id in &extras {
            if !self.files.contains(id) {
                self.files.push(*id);
            }
        }
        if let Some(front) = front {
            self.show(front);
        }
        self.retain_strip_nav();
        extras.is_empty()
    }

    /// The two sides, left first. One when the editor is not split.
    pub fn sides(&self) -> Vec<Side> {
        let focused = Side {
            files: self.files.clone(),
            active: self.active,
            focused: true,
        };
        let Some(other) = &self.split else {
            return vec![focused];
        };
        let other_side = Side {
            files: other.files.clone(),
            active: other.active,
            focused: false,
        };
        match other.focus_left {
            true => vec![focused, other_side],
            false => vec![other_side, focused],
        }
    }
}

impl EditorPanelState {
    /// Opens the second group's default presentation for a file the reader
    /// just split: Markdown keeps both sides live, SVG/YAML/CSV open their
    /// other mode by default, an image gets its own framing. The tab's
    /// `split_with` has already moved the group state: `other_group` names
    /// the group the file came from (where its live Markdown presentation
    /// resolves), `group` the new focused group the second view is for.
    pub fn open_split_presentations(
        &mut self,
        id: BufferId,
        tab: TabId,
        other_group: GroupId,
        group: GroupId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(origin) = self
            .editor_input_for(id, tab, other_group)
            .and_then(|input| input.markdown().cloned())
        {
            self.buffers
                .get(id)
                .expect("split buffer")
                .presentations
                .markdown_view(&origin, tab, group, window, cx);
        }
        // An SVG's second group opens the other side by default: source on the
        // left and its synchronized preview on the right, over one Document.
        if let Some(primary) = self
            .buffers
            .get(id)
            .and_then(|file| file.input.svg().cloned())
        {
            self.buffers
                .get(id)
                .expect("split buffer")
                .presentations
                .svg_view(&primary, tab, group, cx);
        }
        // A picture's second group gets its own framing, around the same
        // decoded document.
        if let Some(primary) = self
            .buffers
            .get(id)
            .and_then(|file| file.input.image().cloned())
        {
            self.buffers
                .get(id)
                .expect("split buffer")
                .presentations
                .image_view(&primary, tab, group, cx);
        }
        // YAML's second group opens the other side by default: tree and source
        // over the same Document and the same parsed projection.
        if let Some(primary) = self
            .buffers
            .get(id)
            .and_then(|file| file.input.yaml().cloned())
        {
            self.buffers
                .get(id)
                .expect("split buffer")
                .presentations
                .yaml_view(&primary, tab, group, cx);
        }
        // CSV/TSV's second group opens the other side by default: table and
        // source over the same Document and the same parsed records.
        if let Some(primary) = self
            .buffers
            .get(id)
            .and_then(|file| file.input.csv().cloned())
        {
            self.buffers
                .get(id)
                .expect("split buffer")
                .presentations
                .csv_view(&primary, tab, group, window, cx);
        }
    }

    /// Whether the *other* group's surface currently holds the keyboard, so a
    /// command about to run belongs to that group: the shell swaps the group
    /// focus when this is true. `active` is the focused group's front buffer,
    /// `other` the other group's, `other_group` the other's identity.
    pub fn split_follow_focus(
        &self,
        other: BufferId,
        active: Option<BufferId>,
        tab: TabId,
        other_group: GroupId,
        window: &Window,
        cx: &gpui_kit::App,
    ) -> bool {
        let Some(file) = self.buffers.get(other) else {
            return false;
        };
        if file.input.markdown().is_some() {
            file.presentations
                .markdown_existing(tab, other_group)
                .is_some_and(|view| {
                    view.read(cx).focus_handle(cx).contains_focused(window, cx)
                        || file
                            .presentations
                            .search(&view)
                            .is_some_and(|search| search.read(cx).has_focus(window, cx))
                })
        } else if file.input.svg().is_some() {
            file.presentations
                .svg_existing(tab, other_group)
                .is_some_and(|view| view.read(cx).focus_handle(cx).contains_focused(window, cx))
        } else if file.input.json().is_some() {
            file.presentations
                .json_existing(tab, other_group)
                .is_some_and(|view| view.read(cx).focus_handle(cx).contains_focused(window, cx))
        } else if file.input.yaml().is_some() {
            file.presentations
                .yaml_existing(tab, other_group)
                .is_some_and(|view| view.read(cx).focus_handle(cx).contains_focused(window, cx))
        } else if file.input.toml().is_some() {
            file.presentations
                .toml_existing(tab, other_group)
                .is_some_and(|view| view.read(cx).focus_handle(cx).contains_focused(window, cx))
        } else if file.input.csv().is_some() {
            file.presentations
                .csv_existing(tab, other_group)
                .is_some_and(|view| view.read(cx).focus_handle(cx).contains_focused(window, cx))
        } else if file.input.xml().is_some() {
            file.presentations
                .xml_existing(tab, other_group)
                .is_some_and(|view| view.read(cx).focus_handle(cx).contains_focused(window, cx))
        } else if file.input.image().is_some() {
            file.presentations
                .image_existing(tab, other_group)
                .is_some_and(|view| view.read(cx).handle().contains_focused(window, cx))
        } else if file.input.sqlite().is_some() {
            file.presentations
                .sqlite_existing(tab, other_group)
                .is_some_and(|view| view.read(cx).focus_handle(cx).contains_focused(window, cx))
        } else if active != Some(other) {
            file.input
                .read(cx)
                .focus_handle(cx)
                .contains_focused(window, cx)
        } else {
            false
        }
    }
}
