//! The header's file strip when there are more files than it has room for,
//! and the controls that share the header with it.
//!
//! **The strip is one keyboard stop.** Only the focused group's strip is
//! drawn, and it is one Tab stop: Left and Right move its focused cell,
//! wrapping at the ends, and Home and End reach them; moving focus does not
//! bring a file forward. Enter or Space does, and leaves the keyboard in the
//! strip, while a pointer click keeps its old habit of putting the keyboard
//! in the text. Escape hands it back to the group's text. Each group
//! remembers its own cell, so a group switch does not lose either one's
//! place.
//!
//! **The strip keeps the files you use.** A docked column fits two or three
//! names, a filled window six or seven; past that the strip used to scroll
//! sideways, which put the file you were just in out of sight as often as
//! not. Now it holds the most recently fronted files — in their strip order,
//! so nothing jumps about as you switch between them — and a `+N` button at
//! its end lists everything, hidden files first, with a filter box.
//!
//! **Dock / Fill** is a two-cell switch rather than a menu entry, and a filled
//! editor leads with a pill naming the terminal it covers, which docks it back
//! beside that terminal.

use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::prelude::*;
use gpui_kit::{Context, Entity, Focusable as _, SharedString, Subscription, Window};

use super::{BufferId, EditorPanelState, StripNav, TabCode};
#[cfg(test)]
use crate::ui::app::LyrelApp;
use crate::ui::host_ops::HostId;
use crate::ui::i18n::{L10nKey, t_fmt};
use lyrel_core::core::machine::TabId;

pub(crate) use crate::ui::document_column::{
    HEADER_CELL_H as CELL_H, HEADER_CELL_MAX_W as CELL_MAX_W, HEADER_CELL_RADIUS as CELL_RADIUS,
    HEADER_CELL_TEXT as CELL_TEXT,
};

pub(crate) const PICKER_W: f32 = 340.;
pub(crate) const PICKER_LIST_MAX_H: f32 = 340.;
pub(crate) const PICKER_ROW_H: f32 = 28.;

/// The strip's own key context. The group focus pair is bound in
/// `Input || DocumentStrip` so it keeps working while the cells hold the
/// keyboard. Strip entry and overflow additionally accept `Terminal` at the
/// workbench Root; the text field's other bindings stay out of the strip.
pub(crate) const KEY_CONTEXT: &str = "DocumentStrip";

/// One file cell the header strip draws, and everything the cell needs to
/// say what it is and whether it holds the strip's keyboard focus.
pub(crate) struct StripCell {
    pub(crate) id: BufferId,
    pub(crate) name: SharedString,
    pub(crate) tip: String,
    pub(crate) active: bool,
    pub(crate) dirty: bool,
    /// This cell is the group's remembered document — the one Enter or Space
    /// would show.
    pub(crate) nav_focused: bool,
    /// The strip — not the text — holds the keyboard, so the focused cell is
    /// lit.
    pub(crate) strip_focused: bool,
    /// One-based place in the tab list, for assistive technology.
    pub(crate) position: usize,
    pub(crate) count: usize,
}

impl TabCode {
    /// The cell Enter or Space would activate among `visible` — the cells the
    /// strip draws now. The focused group's own remembered cell wins while it
    /// is still drawn and the group's front document is the one it was
    /// remembered against; otherwise the document in front, which the strip
    /// always draws (it keeps the most recent files, and the front one is
    /// first); otherwise the first cell.
    pub(crate) fn strip_target(&self, visible: &[BufferId]) -> Option<BufferId> {
        let selected = self.active_id();
        let remembered = self
            .strip_nav
            .get(&self.group)
            .filter(|nav| Some(nav.active) == selected)
            .map(|nav| nav.cell)
            .filter(|cell| visible.contains(cell));
        remembered
            .or_else(|| selected.filter(|id| visible.contains(id)))
            .or_else(|| visible.first().copied())
    }

    /// Keeps `cell` as the focused group's strip cell, against the document
    /// that is in front right now.
    pub(crate) fn remember_strip_focus(&mut self, cell: BufferId) {
        if let Some(active) = self.active_id() {
            self.strip_nav.insert(self.group, StripNav { cell, active });
        }
    }

    /// Forgets remembered cells of groups this tab no longer has.
    pub(crate) fn retain_strip_nav(&mut self) {
        let focused = self.group;
        let other = self.split.as_ref().map(|group| group.id);
        self.strip_nav
            .retain(|group, _| *group == focused || Some(*group) == other);
    }
}

impl TabCode {
    /// Every file in the strip, the one in front first and the rest by when
    /// they last were. Files that have never been in front follow in strip
    /// order.
    pub(crate) fn by_recency(&self) -> Vec<BufferId> {
        let front = self.active_id();
        let mut out: Vec<BufferId> = front.into_iter().collect();
        for id in self.recent.iter().chain(self.files.iter()) {
            if self.files.contains(id) && !out.contains(id) {
                out.push(*id);
            }
        }
        out
    }

    /// Moves the file in front to the head of `recent`. Run every frame the
    /// editor draws, so whatever brought a file forward — a click, a link,
    /// back/forward, a restore — is counted without a hook of its own. The
    /// other group's files keep their places, so moving the focus between
    /// groups of a split does not wipe either one's order.
    pub(crate) fn note_front(&mut self) {
        let order = self.by_recency();
        let rest: Vec<BufferId> = self
            .recent
            .iter()
            .copied()
            .filter(|id| !order.contains(id) && self.shows(*id))
            .collect();
        self.recent = order;
        self.recent.extend(rest);
    }

    /// Which files the strip shows when it has room for `cap`: the `cap` most
    /// recent, as `(position in files, id)` in strip order; and the rest,
    /// most recent first.
    pub(crate) fn strip_split(&self, cap: usize) -> (Vec<(usize, BufferId)>, Vec<BufferId>) {
        let order = self.by_recency();
        let cap = cap.max(1);
        let keep = &order[..cap.min(order.len())];
        let shown = self
            .files
            .iter()
            .copied()
            .enumerate()
            .filter(|(_, id)| keep.contains(id))
            .collect();
        let hidden = order.iter().skip(cap).copied().collect();
        (shown, hidden)
    }
}

/// The `+N` list while it is open.
pub(crate) struct StripPicker {
    input: Entity<InputState>,
    /// The highlighted row, an index into the rows the query leaves.
    hi: usize,
    /// Whether the pointer is on the `+N` button. A press there is the
    /// button's to answer — it closes the list on click — so the list does
    /// not also close itself on the press and have the click reopen it.
    over_button: bool,
    /// Where Escape puts the keyboard back: the strip, when the list was
    /// opened from the strip's own keyboard, otherwise the text it came from.
    restore_strip: bool,
    _sub: Subscription,
}

/// One row of the list.
pub(crate) struct PickerRow {
    pub(crate) id: BufferId,
    pub(crate) name: SharedString,
    pub(crate) dir: SharedString,
    pub(crate) dirty: bool,
    pub(crate) front: bool,
}

impl EditorPanelState {
    /// The cell Enter or Space would activate among `visible` — the focused
    /// group's remembered cell while it is still one of the cells on screen,
    /// otherwise the document in front, otherwise the first cell. See
    /// [`TabCode::strip_target`].
    pub(crate) fn strip_target(
        &self,
        code: Option<&TabCode>,
        visible: &[BufferId],
    ) -> Option<BufferId> {
        code?.strip_target(visible)
    }

    /// Moves the keyboard into the header strip. The decision that the strip
    /// may take the keyboard — a visible panel, files in it, no modal above,
    /// and a surface that belongs to the panel holding the focus — is the
    /// shell's; this performs the focus move itself.
    pub(crate) fn focus_strip_keyboard(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.strip_keyboard.focus(window, cx);
    }

    /// The strip's keyboard, while the strip holds it: the focused cell moves
    /// with Left/Right — wrapping at either end — and Home/End, and only
    /// Enter or Space change the document in front. Escape hands the keyboard
    /// back to the group's text; anything else is not the strip's.
    ///
    /// `visible` is the cells the last draw put on screen. A document closed
    /// since is dropped here rather than acted on by position.
    pub(crate) fn strip_key(
        &mut self,
        tab: TabId,
        code: &mut TabCode,
        visible: &[BufferId],
        ev: &gpui_kit::KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // A chord belongs to whoever bound it; the strip owns bare keys only.
        if ev.keystroke.modifiers.modified() {
            return;
        }
        let shown: Vec<BufferId> = visible
            .iter()
            .copied()
            .filter(|id| code.files.contains(id))
            .collect();
        let Some(current) = code.strip_target(&shown) else {
            return;
        };
        let Some(ix) = shown.iter().position(|id| *id == current) else {
            return;
        };
        let next = match ev.keystroke.key.as_str() {
            "left" => shown[(ix + shown.len() - 1) % shown.len()],
            "right" => shown[(ix + 1) % shown.len()],
            "home" => shown[0],
            "end" => *shown.last().expect("the strip draws a cell"),
            "enter" | "space" => {
                self.strip_select(code, current, cx);
                cx.stop_propagation();
                return;
            }
            "escape" => {
                self.focus_editor(tab, code, window, cx);
                cx.stop_propagation();
                return;
            }
            _ => return,
        };
        code.remember_strip_focus(next);
        cx.stop_propagation();
        cx.notify();
    }

    /// Pointer activation of a strip cell: the document comes to the front
    /// and its text takes the keyboard, as clicking a tab always did.
    pub(crate) fn strip_activate(
        &mut self,
        tab: TabId,
        code: &mut TabCode,
        id: BufferId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.strip_select(code, id, cx);
        self.focus_editor(tab, code, window, cx);
    }

    /// Brings a document to the front from the strip without moving the
    /// keyboard. Keyboard activation keeps the cells navigable.
    pub(crate) fn strip_select(
        &mut self,
        code: &mut TabCode,
        id: BufferId,
        cx: &mut Context<Self>,
    ) {
        let Some(pos) = code.files.iter().position(|file| *file == id) else {
            return;
        };
        code.active = pos;
        code.remember_strip_focus(id);
        self.dismiss_bar();
        cx.notify();
    }

    fn strip_rows(
        &self,
        code: Option<&TabCode>,
        local_host: HostId,
        _cx: &gpui_kit::App,
    ) -> Vec<PickerRow> {
        let Some(code) = code else {
            return Vec::new();
        };
        let front = code.active_id();
        let roots = code.roots.as_slice();
        code.by_recency()
            .into_iter()
            .filter_map(|id| {
                let f = self.buffers.get(id)?;
                let dir = if f.untitled.is_some() {
                    String::new()
                } else {
                    let local = f.host.id() == local_host;
                    let mut segs =
                        super::nav::path_segments(&f.path, if local { roots } else { &[] });
                    segs.pop();
                    segs.join("/")
                };
                Some(PickerRow {
                    id,
                    name: f.label(),
                    dir: dir.into(),
                    dirty: f.dirty,
                    front: Some(id) == front,
                })
            })
            .collect()
    }

    /// The rows the query leaves, in the order Up/Down walks them.
    pub(crate) fn strip_matches(
        &self,
        code: Option<&TabCode>,
        local_host: HostId,
        cap: usize,
        cx: &gpui_kit::App,
    ) -> (Vec<PickerRow>, usize) {
        let q = self
            .strip_picker
            .as_ref()
            .map(|p| p.input.read(cx).value().trim().to_lowercase())
            .unwrap_or_default();
        let rows = self.strip_rows(code, local_host, cx);
        if !q.is_empty() {
            let hits: Vec<PickerRow> = rows
                .into_iter()
                .filter(|r| r.name.to_lowercase().contains(&q) || r.dir.to_lowercase().contains(&q))
                .collect();
            return (hits, 0);
        }
        // Hidden files first — they are what the list is for — then the ones
        // the strip already shows. Rows are in recency order, and the strip
        // shows the first `cap` of it.
        let cap = cap.max(1).min(rows.len());
        let mut rows = rows;
        let shown: Vec<PickerRow> = rows.drain(..cap).collect();
        let hidden = rows.len();
        rows.extend(shown);
        (rows, hidden)
    }

    /// Opens — or closes — the `+N` list. `file_count` names how many open
    /// documents the placeholder speaks of; `from_keyboard` says the chord,
    /// not the pointer, asked, which decides where Escape returns the
    /// keyboard.
    pub(crate) fn toggle_strip_picker(
        &mut self,
        tab: TabId,
        code: Option<&TabCode>,
        file_count: usize,
        from_keyboard: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.strip_picker.is_some() {
            self.close_strip_picker(tab, code, window, cx);
            return;
        }
        let restore_strip = from_keyboard && self.strip_keyboard.is_focused(window);
        let input = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t_fmt(
                L10nKey::EditorStripSearch,
                &[("n", &file_count.to_string())],
            ))
        });
        input.update(cx, |state, cx| state.focus(window, cx));
        let _sub = cx.subscribe_in(&input, window, |this, _, ev: &InputEvent, _, cx| {
            if matches!(ev, InputEvent::Change) {
                if let Some(p) = this.strip_picker.as_mut() {
                    p.hi = 0;
                }
                cx.notify();
            }
        });
        self.strip_picker = Some(StripPicker {
            input,
            hi: 0,
            over_button: true,
            restore_strip,
            _sub,
        });
        cx.notify();
    }

    /// Closes the list, returning the keyboard where it came from: the strip
    /// when the list was opened from the strip's own keyboard, the text it
    /// was pointing at otherwise.
    pub(crate) fn close_strip_picker(
        &mut self,
        tab: TabId,
        code: Option<&TabCode>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(picker) = self.strip_picker.take() else {
            return;
        };
        if picker.restore_strip {
            self.strip_keyboard.focus(window, cx);
        } else if let Some(code) = code {
            self.focus_editor(tab, code, window, cx);
        }
        cx.notify();
    }

    /// A row's choice: the document comes to the front, and the keyboard goes
    /// back to where the list was opened from — the strip's own keyboard
    /// keeps it, a pointer's choice returns it to the text.
    pub(crate) fn strip_pick(
        &mut self,
        tab: TabId,
        code: &mut TabCode,
        id: BufferId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let restore_strip = self
            .strip_picker
            .as_ref()
            .is_some_and(|picker| picker.restore_strip);
        self.strip_picker = None;
        if restore_strip {
            // Selection changes the front document, not the keyboard owner.
            // The select path also remembers this document against the group
            // that is focused now, so the next render exposes a valid cell.
            self.strip_select(code, id, cx);
            self.strip_keyboard.focus(window, cx);
        } else if let Some(pos) = code.files.iter().position(|f| *f == id) {
            self.activate(pos, tab, code, window, cx);
        }
        cx.notify();
    }

    /// The list's keyboard: Up/Down walk the rows, Enter chooses, Escape
    /// closes. `local_host` is the shell's answer to which machine the
    /// window's files live on, for the rows' directory names.
    pub(crate) fn strip_picker_key(
        &mut self,
        tab: TabId,
        code: &mut TabCode,
        local_host: HostId,
        ev: &gpui_kit::KeyDownEvent,
        cap: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (rows, _) = self.strip_matches(Some(&*code), local_host, cap, cx);
        let Some(p) = self.strip_picker.as_mut() else {
            return;
        };
        let n = rows.len();
        match ev.keystroke.key.as_str() {
            "escape" => self.close_strip_picker(tab, Some(code), window, cx),
            "up" if n > 0 => {
                p.hi = p.hi.min(n - 1).saturating_sub(1);
                cx.notify();
            }
            "down" if n > 0 => {
                p.hi = (p.hi + 1).min(n - 1);
                cx.notify();
            }
            "enter" => {
                if let Some(row) = rows.get(p.hi.min(n.saturating_sub(1))) {
                    let id = row.id;
                    self.strip_pick(tab, code, id, window, cx);
                }
            }
            _ => return,
        }
        cx.stop_propagation();
    }

    /// Every file in the strip but the one in front — only the saved ones
    /// when `saved_only`.
    pub(crate) fn strip_others(&self, code: Option<&TabCode>, saved_only: bool) -> Vec<BufferId> {
        let Some(code) = code else {
            return Vec::new();
        };
        let front = code.active_id();
        code.files
            .iter()
            .copied()
            .filter(|id| Some(*id) != front)
            .filter(|id| !saved_only || self.buffers.get(*id).is_some_and(|f| !f.dirty))
            .collect()
    }

    // ---- What the shell's `+N` rendering needs to ask ----

    /// Whether the list is open.
    pub(crate) fn strip_picker_open(&self) -> bool {
        self.strip_picker.is_some()
    }

    /// The highlighted row, clamped to the rows the query leaves.
    pub(crate) fn strip_picker_highlight(&self, rows: usize) -> usize {
        self.strip_picker
            .as_ref()
            .map_or(0, |p| p.hi.min(rows.saturating_sub(1)))
    }

    /// The pointer is on the `+N` button: a press there is the button's to
    /// answer, so the list does not also close itself on the press.
    pub(crate) fn set_strip_picker_hover(&mut self, over: bool) {
        if let Some(p) = self.strip_picker.as_mut() {
            p.over_button = over;
        }
    }

    /// The pointer moved onto a row: it becomes the highlighted one.
    pub(crate) fn set_strip_picker_highlight(&mut self, i: usize) {
        if let Some(p) = self.strip_picker.as_mut()
            && p.hi != i
        {
            p.hi = i;
        }
    }

    /// Drops the list without restoring the keyboard — the caller answers
    /// for where the keyboard goes next.
    pub(crate) fn dismiss_strip_picker(&mut self) {
        self.strip_picker = None;
    }

    /// The filter box's filter.
    pub(crate) fn strip_picker_query(&self, cx: &gpui_kit::App) -> String {
        self.strip_picker
            .as_ref()
            .map(|p| p.input.read(cx).value().trim().to_lowercase())
            .unwrap_or_default()
    }

    /// The list's own keyboard focus, so the shell can ask whether the list
    /// itself holds the keyboard before answering its closing chord.
    pub(crate) fn strip_picker_has_focus(&self, window: &Window, cx: &gpui_kit::App) -> bool {
        self.strip_picker
            .as_ref()
            .is_some_and(|picker| picker.input.read(cx).focus_handle(cx).is_focused(window))
    }

    /// The filter box the list renders, while the list is open.
    pub(crate) fn strip_picker_input(&self) -> Option<Entity<InputState>> {
        self.strip_picker.as_ref().map(|p| p.input.clone())
    }

    /// Whether the pointer is on the `+N` button: a press there is the
    /// button's to answer, so the list stays open through it.
    pub(crate) fn strip_picker_over_button(&self) -> bool {
        self.strip_picker.as_ref().is_some_and(|p| p.over_button)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(n: u64) -> Vec<BufferId> {
        (0..n).map(|_| BufferId::new()).collect()
    }

    fn code(files: &[BufferId], active: usize) -> TabCode {
        let mut code = TabCode::new();
        code.files = files.to_vec();
        code.active = active;
        code
    }

    #[test]
    fn the_strip_keeps_the_files_last_in_front_in_their_strip_order() {
        let ids = ids(5);
        let mut code = code(&ids, 0);
        for front in [3, 1, 4] {
            code.active = front;
            code.note_front();
        }
        let (shown, hidden) = code.strip_split(3);
        // 4, 1 and 3 were in front last; they keep their strip positions.
        assert_eq!(shown, vec![(1, ids[1]), (3, ids[3]), (4, ids[4])]);
        // The rest by recency: 0 was in front before any of them, 2 never.
        assert_eq!(hidden, vec![ids[0], ids[2]]);
    }

    #[test]
    fn the_file_in_front_always_has_a_place() {
        let ids = ids(4);
        let mut code = code(&ids, 0);
        code.note_front();
        // Brought forward from outside the strip's view (a link, the list)
        // without a frame drawn in between.
        code.active = 3;
        let (shown, _) = code.strip_split(1);
        assert_eq!(shown, vec![(3, ids[3])]);
    }

    #[test]
    fn a_split_keeps_the_other_groups_order() {
        let ids = ids(4);
        let mut code = code(&ids[..2], 1);
        code.split = Some(super::super::split::OtherGroup {
            id: super::super::split::GroupId::default(),
            files: ids[2..].to_vec(),
            active: 0,
            focus_left: true,
        });
        code.note_front();
        code.swap_focus();
        code.note_front();
        code.swap_focus();
        code.note_front();
        assert_eq!(code.recent, vec![ids[1], ids[0], ids[2], ids[3]]);
    }

    #[test]
    fn closed_files_leave_the_order() {
        let ids = ids(3);
        let mut code = code(&ids, 2);
        code.note_front();
        code.files.remove(1);
        code.active = 1;
        code.note_front();
        assert_eq!(code.recent, vec![ids[2], ids[0]]);
    }
}
#[cfg(test)]
mod gpui_tests {
    use std::path::PathBuf;

    use gpui_kit::test::TestWindowExt as _;
    use gpui_kit::{Entity, TestAppContext, VisualTestContext};
    use lyrel_document::text::{EditorConfig, TextFormat};

    use super::super::nav::gpui_tests::open_rs;
    use super::super::{BufferKind, test_host};
    use super::*;
    use crate::core::actions::{
        FocusDocumentTabNavigation, FocusRightPanelNavigation, FocusTerminalTabNavigation,
    };
    use crate::ui::app::test_window::harness_with_tabs;
    use crate::ui::host_ops::HostId;

    fn settle(vcx: &mut VisualTestContext) {
        vcx.run_until_parked();
        vcx.update(|window, cx| window.render_frame(cx));
        vcx.run_until_parked();
    }

    fn wait_until(
        vcx: &mut VisualTestContext,
        mut ready: impl FnMut(&mut VisualTestContext) -> bool,
    ) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            settle(vcx);
            if ready(vcx) {
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the editor never settled"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    /// The document in front, by its path — the identity a user sees.
    fn front(app: &Entity<LyrelApp>, vcx: &mut VisualTestContext) -> PathBuf {
        app.read_with(vcx, |app, cx| {
            let code = app.tab_code().unwrap();
            app.editor.read(cx).active_file(code).unwrap().path.clone()
        })
    }

    fn strip_has_keys(app: &Entity<LyrelApp>, vcx: &mut VisualTestContext) -> bool {
        vcx.update(|window, cx| {
            app.read(cx)
                .editor
                .read(cx)
                .strip_keyboard
                .is_focused(window)
        })
    }

    fn content_has_keys(app: &Entity<LyrelApp>, vcx: &mut VisualTestContext) -> bool {
        vcx.update(|window, cx| {
            let app = app.read(cx);
            let Some(code) = app.tab_code() else {
                return false;
            };
            let Some(tab) = app.tabs.get(app.tabs.active_index()) else {
                return false;
            };
            app.editor
                .read(cx)
                .active_editor_input(tab.id(), code)
                .is_some_and(|input| input.read(cx).focus_handle(cx).contains_focused(window, cx))
        })
    }

    fn picker_has_keys(app: &Entity<LyrelApp>, vcx: &mut VisualTestContext) -> bool {
        vcx.update(|window, cx| {
            app.read(cx)
                .editor
                .read(cx)
                .strip_picker_has_focus(window, cx)
        })
    }

    fn sidebar_has_keys(app: &Entity<LyrelApp>, vcx: &mut VisualTestContext) -> bool {
        vcx.update(|window, cx| app.read(cx).sidebar_nav_focus.is_focused(window))
    }

    fn dirty(app: &Entity<LyrelApp>, vcx: &mut VisualTestContext, id: BufferId) -> bool {
        app.read_with(vcx, |app, cx| {
            app.editor.read(cx).buffers().get(id).unwrap().dirty
        })
    }

    #[gpui_kit::test]
    fn the_strip_is_one_tab_stop_and_activation_leaves_the_keys_in_it(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        open_rs(&app, &mut vcx, "/strip/a.rs");
        open_rs(&app, &mut vcx, "/strip/b.rs");
        open_rs(&app, &mut vcx, "/strip/c.rs");
        settle(&mut vcx);
        assert_eq!(front(&app, &mut vcx), PathBuf::from("/strip/c.rs"));
        assert!(
            content_has_keys(&app, &mut vcx),
            "the text starts with the keys"
        );
        assert!(!strip_has_keys(&app, &mut vcx));

        // Entering the focused group's strip takes the keyboard and brings
        // nothing else forward.
        vcx.simulate_keystrokes("secondary-alt-t");
        assert!(strip_has_keys(&app, &mut vcx));
        assert_eq!(front(&app, &mut vcx), PathBuf::from("/strip/c.rs"));

        // Tab leaves the one-stop group and Shift+Tab comes back to it.
        vcx.simulate_keystrokes("tab");
        assert!(!strip_has_keys(&app, &mut vcx));
        vcx.simulate_keystrokes("shift-tab");
        assert!(strip_has_keys(&app, &mut vcx));

        // Moving the focused cell does not activate.
        vcx.simulate_keystrokes("left");
        assert_eq!(front(&app, &mut vcx), PathBuf::from("/strip/c.rs"));

        // Enter activates the focused cell and keeps the strip navigable.
        vcx.simulate_keystrokes("enter");
        assert_eq!(front(&app, &mut vcx), PathBuf::from("/strip/b.rs"));
        assert!(strip_has_keys(&app, &mut vcx));

        // Home and End reach the ends.
        vcx.simulate_keystrokes("home");
        vcx.simulate_keystrokes("enter");
        assert_eq!(front(&app, &mut vcx), PathBuf::from("/strip/a.rs"));
        vcx.simulate_keystrokes("end");
        vcx.simulate_keystrokes("enter");
        assert_eq!(front(&app, &mut vcx), PathBuf::from("/strip/c.rs"));

        // Right wraps from the last cell to the first; Left wraps back.
        vcx.simulate_keystrokes("right");
        vcx.simulate_keystrokes("enter");
        assert_eq!(front(&app, &mut vcx), PathBuf::from("/strip/a.rs"));
        vcx.simulate_keystrokes("left");
        vcx.simulate_keystrokes("enter");
        assert_eq!(front(&app, &mut vcx), PathBuf::from("/strip/c.rs"));

        // Space activates as Enter does.
        vcx.simulate_keystrokes("left");
        vcx.simulate_keystrokes("space");
        assert_eq!(front(&app, &mut vcx), PathBuf::from("/strip/b.rs"));
        assert!(strip_has_keys(&app, &mut vcx));

        // Escape hands the keyboard back to the owning editor.
        vcx.simulate_keystrokes("escape");
        assert!(!strip_has_keys(&app, &mut vcx));
        assert!(content_has_keys(&app, &mut vcx));
        assert_eq!(front(&app, &mut vcx), PathBuf::from("/strip/b.rs"));

        // Keys typed in the text never move the strip's cell: the remembered
        // cell is still b, so re-entering and activating stays on b.
        vcx.simulate_keystrokes("left");
        vcx.simulate_keystrokes("secondary-alt-t");
        assert!(strip_has_keys(&app, &mut vcx));
        vcx.simulate_keystrokes("enter");
        assert_eq!(front(&app, &mut vcx), PathBuf::from("/strip/b.rs"));
    }

    #[gpui_kit::test]
    fn the_shared_strip_keeps_each_groups_cell_across_a_switch(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        open_rs(&app, &mut vcx, "/strip/left.rs");
        app.update_in(&mut vcx, |app, window, cx| {
            assert!(app.editor_split(window, cx), "the editor splits");
        });
        open_rs(&app, &mut vcx, "/strip/right.rs");
        open_rs(&app, &mut vcx, "/strip/right2.rs");
        settle(&mut vcx);
        assert_eq!(front(&app, &mut vcx), PathBuf::from("/strip/right2.rs"));
        assert!(!app.read_with(&vcx, |app, _| app.tab_code().unwrap().focus_is_left()));

        // The right group's strip, with its front file as the first cell.
        vcx.simulate_keystrokes("secondary-alt-t");
        assert!(strip_has_keys(&app, &mut vcx));
        // Move to the other cell without activating it.
        vcx.simulate_keystrokes("left");
        assert_eq!(front(&app, &mut vcx), PathBuf::from("/strip/right2.rs"));

        // Choosing the left group while the strip holds the keyboard keeps
        // the keyboard in the shared strip, which now shows that group.
        vcx.simulate_keystrokes("secondary-alt-left");
        assert!(
            strip_has_keys(&app, &mut vcx),
            "the strip follows the group"
        );
        assert!(app.read_with(&vcx, |app, _| app.tab_code().unwrap().focus_is_left()));
        assert_eq!(front(&app, &mut vcx), PathBuf::from("/strip/left.rs"));

        // Back in the right group, Enter still activates the cell it was on —
        // the group's own remembered document, not the one in front.
        vcx.simulate_keystrokes("secondary-alt-right");
        assert!(strip_has_keys(&app, &mut vcx));
        vcx.simulate_keystrokes("enter");
        assert_eq!(front(&app, &mut vcx), PathBuf::from("/strip/right.rs"));

        // The left group kept its own front, and Escape returns to its text.
        vcx.simulate_keystrokes("secondary-alt-left");
        assert_eq!(front(&app, &mut vcx), PathBuf::from("/strip/left.rs"));
        assert!(strip_has_keys(&app, &mut vcx));
        vcx.simulate_keystrokes("escape");
        assert!(content_has_keys(&app, &mut vcx));
        assert!(!strip_has_keys(&app, &mut vcx));
    }

    #[gpui_kit::test]
    fn the_hidden_document_list_opens_from_the_keyboard_and_restores_focus(
        cx: &mut TestAppContext,
    ) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        for name in ["alpha", "beta", "gamma", "delta", "epsilon"] {
            open_rs(&app, &mut vcx, &format!("/strip/{name}.rs"));
        }
        settle(&mut vcx);
        // A docked, unsplit editor keeps three names; beta and alpha are the
        // hidden pair.
        assert_eq!(front(&app, &mut vcx), PathBuf::from("/strip/epsilon.rs"));

        // Open the list from the strip's own keyboard and filter it down to a
        // document the strip does not show.
        vcx.simulate_keystrokes("secondary-alt-t");
        assert!(strip_has_keys(&app, &mut vcx));
        vcx.simulate_keystrokes("secondary-alt-shift-t");
        assert!(picker_has_keys(&app, &mut vcx));

        // The picker is above both navigation surfaces. Their entry actions
        // cannot pull the keyboard out of it or expose the strip behind it.
        vcx.dispatch_action(FocusTerminalTabNavigation);
        vcx.dispatch_action(FocusRightPanelNavigation);
        vcx.dispatch_action(FocusDocumentTabNavigation);
        assert!(picker_has_keys(&app, &mut vcx));
        assert!(!sidebar_has_keys(&app, &mut vcx));
        assert!(!strip_has_keys(&app, &mut vcx));

        vcx.simulate_keystrokes("a l p h a");
        vcx.simulate_keystrokes("enter");
        settle(&mut vcx);
        assert_eq!(
            front(&app, &mut vcx),
            PathBuf::from("/strip/alpha.rs"),
            "the hidden document came forward"
        );
        assert!(
            strip_has_keys(&app, &mut vcx),
            "confirming a keyboard-opened picker restores the strip"
        );

        // The newly-fronted hidden document is the strip's valid roving cell:
        // Right and Enter activate its visible neighbour, then Home returns.
        vcx.simulate_keystrokes("right");
        assert_eq!(front(&app, &mut vcx), PathBuf::from("/strip/alpha.rs"));
        vcx.simulate_keystrokes("enter");
        assert_eq!(front(&app, &mut vcx), PathBuf::from("/strip/delta.rs"));
        assert!(strip_has_keys(&app, &mut vcx));
        vcx.simulate_keystrokes("home");
        vcx.simulate_keystrokes("enter");
        assert_eq!(front(&app, &mut vcx), PathBuf::from("/strip/alpha.rs"));
        assert!(strip_has_keys(&app, &mut vcx));

        // Cancelling restores the keyboard to the strip it was opened from.
        vcx.simulate_keystrokes("secondary-alt-t");
        vcx.simulate_keystrokes("secondary-alt-shift-t");
        vcx.simulate_keystrokes("b e t a");
        vcx.simulate_keystrokes("escape");
        assert!(
            strip_has_keys(&app, &mut vcx),
            "cancelling returns to the strip"
        );
        assert_eq!(front(&app, &mut vcx), PathBuf::from("/strip/alpha.rs"));

        // Opened from the text, cancelling returns to the text.
        vcx.simulate_keystrokes("escape");
        assert!(content_has_keys(&app, &mut vcx));
        vcx.simulate_keystrokes("secondary-alt-shift-t");
        vcx.simulate_keystrokes("escape");
        assert!(content_has_keys(&app, &mut vcx));
        assert!(!strip_has_keys(&app, &mut vcx));
        assert_eq!(front(&app, &mut vcx), PathBuf::from("/strip/alpha.rs"));
    }

    #[gpui_kit::test]
    fn terminal_content_enters_the_document_strip_through_the_root_keymap(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        for name in ["a", "b", "c"] {
            open_rs(&app, &mut vcx, &format!("/strip/{name}.rs"));
        }
        app.update_in(&mut vcx, |app, window, cx| {
            app.set_document_layout(crate::core::config::DocumentLayout::Dock, cx);
            app.focus_active(window, cx);
        });
        settle(&mut vcx);
        app.update_in(&mut vcx, |app, window, cx| {
            let tab = &app.tabs[app.tabs.active_index()];
            assert!(
                tab.pane
                    .leaves()
                    .iter()
                    .any(|leaf| leaf.contains_focused(window, cx))
            );
        });

        // This is the real terminal key context and Root action consumer, not
        // a direct call to the strip's focus method.
        vcx.simulate_keystrokes("secondary-alt-t");
        assert!(strip_has_keys(&app, &mut vcx));
        assert_eq!(front(&app, &mut vcx), PathBuf::from("/strip/c.rs"));

        vcx.simulate_keystrokes("right");
        assert_eq!(front(&app, &mut vcx), PathBuf::from("/strip/c.rs"));
        vcx.simulate_keystrokes("enter");
        assert_eq!(front(&app, &mut vcx), PathBuf::from("/strip/a.rs"));
        assert!(strip_has_keys(&app, &mut vcx));
        vcx.simulate_keystrokes("tab");
        assert!(!strip_has_keys(&app, &mut vcx));
        vcx.simulate_keystrokes("shift-tab");
        assert!(strip_has_keys(&app, &mut vcx));

        // Document navigation's explicit return route is the owning editor.
        vcx.simulate_keystrokes("escape");
        assert!(content_has_keys(&app, &mut vcx));
        assert!(!strip_has_keys(&app, &mut vcx));
    }

    #[gpui_kit::test]
    fn editor_content_enters_sidebar_navigation_and_returns_to_the_editor(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        open_rs(&app, &mut vcx, "/strip/editor.rs");
        settle(&mut vcx);
        assert!(content_has_keys(&app, &mut vcx));

        vcx.dispatch_action(FocusTerminalTabNavigation);
        assert!(sidebar_has_keys(&app, &mut vcx));
        assert_eq!(front(&app, &mut vcx), PathBuf::from("/strip/editor.rs"));
        vcx.simulate_keystrokes("enter");
        assert!(sidebar_has_keys(&app, &mut vcx));
        assert_eq!(front(&app, &mut vcx), PathBuf::from("/strip/editor.rs"));

        vcx.simulate_keystrokes("tab");
        assert!(!sidebar_has_keys(&app, &mut vcx));
        vcx.simulate_keystrokes("shift-tab");
        assert!(sidebar_has_keys(&app, &mut vcx));
        vcx.simulate_keystrokes("escape");
        assert!(content_has_keys(&app, &mut vcx));
        assert!(!sidebar_has_keys(&app, &mut vcx));
    }

    #[gpui_kit::test]
    fn pointer_opened_hidden_document_list_keeps_content_focus(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        for name in ["alpha", "beta", "gamma", "delta", "epsilon"] {
            open_rs(&app, &mut vcx, &format!("/strip/{name}.rs"));
        }
        settle(&mut vcx);
        vcx.simulate_keystrokes("secondary-alt-t");
        assert!(strip_has_keys(&app, &mut vcx));

        // Even when the strip had focus before the pointer press, opening the
        // picker is a pointer interaction and selection keeps pointer focus.
        vcx.update(|window, cx| window.click("editor-strip-more", cx));
        settle(&mut vcx);
        assert!(picker_has_keys(&app, &mut vcx));
        vcx.update(|window, cx| window.click("editor-strip-row-0", cx));
        settle(&mut vcx);
        assert_eq!(front(&app, &mut vcx), PathBuf::from("/strip/beta.rs"));
        assert!(content_has_keys(&app, &mut vcx));
        assert!(!strip_has_keys(&app, &mut vcx));
    }

    #[gpui_kit::test]
    fn closing_a_document_leaves_a_live_strip_cell(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        open_rs(&app, &mut vcx, "/strip/a.rs");
        let b = open_rs(&app, &mut vcx, "/strip/b.rs");
        open_rs(&app, &mut vcx, "/strip/c.rs");
        settle(&mut vcx);

        vcx.simulate_keystrokes("secondary-alt-t");
        vcx.simulate_keystrokes("left");
        vcx.simulate_keystrokes("enter");
        assert_eq!(front(&app, &mut vcx), PathBuf::from("/strip/b.rs"));
        assert!(strip_has_keys(&app, &mut vcx));

        // The × the strip offers on the cell.
        app.update_in(&mut vcx, |app, window, cx| {
            app.editor_close_file(1, window, cx);
        });
        settle(&mut vcx);
        app.read_with(&vcx, |app, cx| {
            let code = app.tab_code().unwrap();
            assert!(!code.files.contains(&b), "the closed document is gone");
            assert_eq!(
                app.editor.read(cx).active_file(code).unwrap().path,
                PathBuf::from("/strip/c.rs"),
                "the neighbour in front took over"
            );
        });

        // The remembered cell went with it; the strip lands on a live cell and
        // still navigates and activates.
        vcx.simulate_keystrokes("secondary-alt-t");
        assert!(strip_has_keys(&app, &mut vcx));
        vcx.simulate_keystrokes("home");
        vcx.simulate_keystrokes("enter");
        assert_eq!(front(&app, &mut vcx), PathBuf::from("/strip/a.rs"));
        vcx.simulate_keystrokes("right");
        vcx.simulate_keystrokes("enter");
        assert_eq!(front(&app, &mut vcx), PathBuf::from("/strip/c.rs"));
    }

    #[gpui_kit::test]
    fn unsaved_work_survives_strip_navigation_and_still_asks_before_closing(
        cx: &mut TestAppContext,
    ) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        open_rs(&app, &mut vcx, "/strip/clean.rs");
        let edited = open_rs(&app, &mut vcx, "/strip/edited.rs");
        settle(&mut vcx);
        vcx.update(|window, cx| window.input("edited", cx));
        settle(&mut vcx);
        assert!(dirty(&app, &mut vcx, edited));

        // The strip brings the clean document forward without touching the
        // unsaved one.
        vcx.simulate_keystrokes("secondary-alt-t");
        vcx.simulate_keystrokes("home");
        vcx.simulate_keystrokes("enter");
        assert_eq!(front(&app, &mut vcx), PathBuf::from("/strip/clean.rs"));
        assert!(strip_has_keys(&app, &mut vcx));
        assert!(
            dirty(&app, &mut vcx, edited),
            "the unsaved edits are still there"
        );

        // Closing it still asks first, and the document stays open.
        app.update_in(&mut vcx, |app, window, cx| {
            app.editor_close_file(1, window, cx);
        });
        settle(&mut vcx);
        assert!(
            vcx.has_pending_prompt(),
            "the unsaved-change question is still asked"
        );
        app.read_with(&vcx, |app, _| {
            assert!(
                app.tab_code().unwrap().files.contains(&edited),
                "a cancelled close keeps the document"
            );
        });
        vcx.simulate_prompt_answer(crate::ui::i18n::t(L10nKey::Cancel));
        settle(&mut vcx);
        assert!(dirty(&app, &mut vcx, edited));
    }

    #[gpui_kit::test]
    fn the_document_in_front_still_saves_with_the_strip_holding_the_keyboard(
        cx: &mut TestAppContext,
    ) {
        let dir = test_host::canonical_tempdir();
        let host = test_host::FileHost::new(HostId(737));
        let path = dir.path().join("saved.rs");
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        app.update_in(&mut vcx, |app, window, cx| {
            let tab = app.tabs[0].id();
            let id = app.editor.update(cx, |editor, cx| {
                editor.new_buffer(
                    host.clone(),
                    path.clone(),
                    None,
                    BufferKind::Text,
                    "fn main() {}\n".into(),
                    TextFormat::default(),
                    EditorConfig::default(),
                    lyrel_document::text::detect_indent("", "rust"),
                    None,
                    window,
                    cx,
                )
            });
            app.editor_show_in_tab(tab, id, true, window, cx);
        });
        settle(&mut vcx);
        vcx.update(|window, cx| window.input("// edit", cx));
        settle(&mut vcx);
        assert!(app.read_with(&vcx, |app, cx| {
            let code = app.tab_code().unwrap();
            app.editor.read(cx).active_file(code).unwrap().dirty
        }));

        // The strip has the keyboard, and the document commands still reach
        // the document in front.
        vcx.simulate_keystrokes("secondary-alt-t");
        assert!(strip_has_keys(&app, &mut vcx));
        vcx.update(|window, cx| window.press("secondary-s", cx));
        wait_until(&mut vcx, |vcx| {
            app.read_with(vcx, |app, cx| {
                let code = app.tab_code().unwrap();
                !app.editor.read(cx).active_file(code).unwrap().dirty
            })
        });
        assert_eq!(host.writes.load(std::sync::atomic::Ordering::SeqCst), 1);
    }
}
