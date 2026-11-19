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
use lyrel_core::core::machine::TabId;
use lyrel_ui_base::host_ops::HostId;
use lyrel_ui_base::i18n::{L10nKey, t_fmt};

pub use lyrel_ui_base::document_column::{
    HEADER_CELL_H as CELL_H, HEADER_CELL_MAX_W as CELL_MAX_W, HEADER_CELL_RADIUS as CELL_RADIUS,
    HEADER_CELL_TEXT as CELL_TEXT,
};

pub const PICKER_W: f32 = 340.;
pub const PICKER_LIST_MAX_H: f32 = 340.;
pub const PICKER_ROW_H: f32 = 28.;

/// The strip's own key context. The group focus pair is bound in
/// `Input || DocumentStrip` so it keeps working while the cells hold the
/// keyboard. Strip entry and overflow additionally accept `Terminal` at the
/// workbench Root; the text field's other bindings stay out of the strip.
pub const KEY_CONTEXT: &str = "DocumentStrip";

/// One file cell the header strip draws, and everything the cell needs to
/// say what it is and whether it holds the strip's keyboard focus.
pub struct StripCell {
    pub id: BufferId,
    pub name: SharedString,
    pub tip: String,
    pub active: bool,
    pub dirty: bool,
    /// This cell is the group's remembered document — the one Enter or Space
    /// would show.
    pub nav_focused: bool,
    /// The strip — not the text — holds the keyboard, so the focused cell is
    /// lit.
    pub strip_focused: bool,
    /// One-based place in the tab list, for assistive technology.
    pub position: usize,
    pub count: usize,
}

impl TabCode {
    /// The cell Enter or Space would activate among `visible` — the cells the
    /// strip draws now. The focused group's own remembered cell wins while it
    /// is still drawn and the group's front document is the one it was
    /// remembered against; otherwise the document in front, which the strip
    /// always draws (it keeps the most recent files, and the front one is
    /// first); otherwise the first cell.
    pub fn strip_target(&self, visible: &[BufferId]) -> Option<BufferId> {
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
    pub fn remember_strip_focus(&mut self, cell: BufferId) {
        if let Some(active) = self.active_id() {
            self.strip_nav.insert(self.group, StripNav { cell, active });
        }
    }

    /// Forgets remembered cells of groups this tab no longer has.
    pub fn retain_strip_nav(&mut self) {
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
    pub fn by_recency(&self) -> Vec<BufferId> {
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
    pub fn note_front(&mut self) {
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
    pub fn strip_split(&self, cap: usize) -> (Vec<(usize, BufferId)>, Vec<BufferId>) {
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
pub struct StripPicker {
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
pub struct PickerRow {
    pub id: BufferId,
    pub name: SharedString,
    pub dir: SharedString,
    pub dirty: bool,
    pub front: bool,
}

impl EditorPanelState {
    /// The cell Enter or Space would activate among `visible` — the focused
    /// group's remembered cell while it is still one of the cells on screen,
    /// otherwise the document in front, otherwise the first cell. See
    /// [`TabCode::strip_target`].
    pub fn strip_target(&self, code: Option<&TabCode>, visible: &[BufferId]) -> Option<BufferId> {
        code?.strip_target(visible)
    }

    /// Moves the keyboard into the header strip. The decision that the strip
    /// may take the keyboard — a visible panel, files in it, no modal above,
    /// and a surface that belongs to the panel holding the focus — is the
    /// shell's; this performs the focus move itself.
    pub fn focus_strip_keyboard(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.strip_keyboard.focus(window, cx);
    }

    /// The strip's keyboard, while the strip holds it: the focused cell moves
    /// with Left/Right — wrapping at either end — and Home/End, and only
    /// Enter or Space change the document in front. Escape hands the keyboard
    /// back to the group's text; anything else is not the strip's.
    ///
    /// `visible` is the cells the last draw put on screen. A document closed
    /// since is dropped here rather than acted on by position.
    pub fn strip_key(
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
    pub fn strip_activate(
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
    pub fn strip_select(&mut self, code: &mut TabCode, id: BufferId, cx: &mut Context<Self>) {
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
    pub fn strip_matches(
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
    pub fn toggle_strip_picker(
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
    pub fn close_strip_picker(
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
    pub fn strip_pick(
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
    pub fn strip_picker_key(
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
    pub fn strip_others(&self, code: Option<&TabCode>, saved_only: bool) -> Vec<BufferId> {
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
    pub fn strip_picker_open(&self) -> bool {
        self.strip_picker.is_some()
    }

    /// The highlighted row, clamped to the rows the query leaves.
    pub fn strip_picker_highlight(&self, rows: usize) -> usize {
        self.strip_picker
            .as_ref()
            .map_or(0, |p| p.hi.min(rows.saturating_sub(1)))
    }

    /// The pointer is on the `+N` button: a press there is the button's to
    /// answer, so the list does not also close itself on the press.
    pub fn set_strip_picker_hover(&mut self, over: bool) {
        if let Some(p) = self.strip_picker.as_mut() {
            p.over_button = over;
        }
    }

    /// The pointer moved onto a row: it becomes the highlighted one.
    pub fn set_strip_picker_highlight(&mut self, i: usize) {
        if let Some(p) = self.strip_picker.as_mut()
            && p.hi != i
        {
            p.hi = i;
        }
    }

    /// Drops the list without restoring the keyboard — the caller answers
    /// for where the keyboard goes next.
    pub fn dismiss_strip_picker(&mut self) {
        self.strip_picker = None;
    }

    /// The filter box's filter.
    pub fn strip_picker_query(&self, cx: &gpui_kit::App) -> String {
        self.strip_picker
            .as_ref()
            .map(|p| p.input.read(cx).value().trim().to_lowercase())
            .unwrap_or_default()
    }

    /// The list's own keyboard focus, so the shell can ask whether the list
    /// itself holds the keyboard before answering its closing chord.
    pub fn strip_picker_has_focus(&self, window: &Window, cx: &gpui_kit::App) -> bool {
        self.strip_picker
            .as_ref()
            .is_some_and(|picker| picker.input.read(cx).focus_handle(cx).is_focused(window))
    }

    /// The filter box the list renders, while the list is open.
    pub fn strip_picker_input(&self) -> Option<Entity<InputState>> {
        self.strip_picker.as_ref().map(|p| p.input.clone())
    }

    /// Whether the pointer is on the `+N` button: a press there is the
    /// button's to answer, so the list stays open through it.
    pub fn strip_picker_over_button(&self) -> bool {
        self.strip_picker.as_ref().is_some_and(|p| p.over_button)
    }
}
