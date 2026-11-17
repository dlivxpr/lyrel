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

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{
    Anchor, AnyElement, Context, Entity, Focusable as _, FontWeight, SharedString, Subscription,
    Window, anchored, deferred, div, px, rems,
};

use super::{BufferId, StripNav, TabCode};
use crate::ui::app::LyrelApp;
use crate::ui::document_column::DocumentChrome;
use crate::ui::i18n::{L10nKey, t, t_fmt};

pub(super) use crate::ui::document_column::{
    HEADER_CELL_H as CELL_H, HEADER_CELL_MAX_W as CELL_MAX_W, HEADER_CELL_RADIUS as CELL_RADIUS,
    HEADER_CELL_TEXT as CELL_TEXT,
};

const PICKER_W: f32 = 340.;
const PICKER_LIST_MAX_H: f32 = 340.;
const PICKER_ROW_H: f32 = 28.;

/// The strip's own key context. The group focus pair is bound in
/// `Input || DocumentStrip` so it keeps working while the cells hold the
/// keyboard. Strip entry and overflow additionally accept `Terminal` at the
/// workbench Root; the text field's other bindings stay out of the strip.
pub(crate) const KEY_CONTEXT: &str = "DocumentStrip";

/// One file cell the header strip draws, and everything the cell needs to
/// say what it is and whether it holds the strip's keyboard focus.
pub(super) struct StripCell {
    pub(super) id: BufferId,
    pub(super) name: SharedString,
    pub(super) tip: String,
    pub(super) active: bool,
    pub(super) dirty: bool,
    /// This cell is the group's remembered document — the one Enter or Space
    /// would show.
    pub(super) nav_focused: bool,
    /// The strip — not the text — holds the keyboard, so the focused cell is
    /// lit.
    pub(super) strip_focused: bool,
    /// One-based place in the tab list, for assistive technology.
    pub(super) position: usize,
    pub(super) count: usize,
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
struct PickerRow {
    id: BufferId,
    name: SharedString,
    dir: SharedString,
    dirty: bool,
    front: bool,
}

impl LyrelApp {
    /// How many names the strip holds. Fixed counts rather than a measure:
    /// names differ so much in width that a measured fit would reshuffle the
    /// strip whenever a long one came forward.
    pub(super) fn editor_strip_cap(&self, chrome: DocumentChrome, cx: &gpui_kit::App) -> usize {
        let (left, right) = (self.left_panel_open(cx), self.right_panel_open(cx));
        match chrome {
            DocumentChrome::Fill if left && right => 5,
            DocumentChrome::Fill => 7,
            _ if right => 2,
            _ => 3,
        }
    }

    /// The cell Enter or Space would activate: the focused group's remembered
    /// cell while it is still one of the cells on screen, otherwise the
    /// document in front, otherwise the first cell. See
    /// [`TabCode::strip_target`].
    pub(super) fn editor_strip_target(&self, visible: &[BufferId]) -> Option<BufferId> {
        self.tab_code()?.strip_target(visible)
    }

    /// Whether a real terminal or the ordinary editor content owns the
    /// keyboard. Navigation entry must not fire through a picker, an editor
    /// prompt, or another workbench input merely because it has Input context.
    fn document_navigation_content_has_focus(&self, window: &Window, cx: &Context<Self>) -> bool {
        if self.editor_has_focus(window, cx) || self.editor.strip_keyboard.is_focused(window) {
            return true;
        }
        self.tabs.get(self.tabs.active_index()).is_some_and(|tab| {
            tab.pane
                .leaves()
                .iter()
                .any(|leaf| leaf.contains_focused(window, cx))
        })
    }

    /// Moves the keyboard into the focused group's strip, on the document it
    /// remembers. Nothing is brought to the front: entering the strip shows
    /// what Enter would open, and leaves the text as it was.
    pub(crate) fn focus_document_tab_navigation(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.code_panel_visible()
            || self.tab_code().is_none_or(|code| code.files.is_empty())
            || self.sidebar_nav_overlay_open(window)
            || !self.document_navigation_content_has_focus(window, cx)
        {
            return false;
        }
        self.editor.strip_keyboard.focus(window, cx);
        cx.notify();
        true
    }

    /// Opens — or closes — the `+N` list from the keyboard. The list holds
    /// every open document, hidden ones first, and its filter box takes the
    /// typing; choosing one brings it to the front.
    pub(crate) fn editor_document_overflow(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.code_panel_visible() || self.tab_code().is_none_or(|code| code.files.is_empty()) {
            return false;
        }
        if let Some(picker) = &self.editor.strip_picker {
            // The same chord closes the list only while the list itself holds
            // the keyboard. A picker sitting under a search, a prompt or
            // another overlay does not answer for that surface's keys.
            if !picker.input.read(cx).focus_handle(cx).is_focused(window) {
                return false;
            }
        } else if self.sidebar_nav_overlay_open(window)
            || !self.document_navigation_content_has_focus(window, cx)
        {
            return false;
        }
        self.editor_toggle_strip_picker(window, cx, true);
        true
    }

    /// The strip's keyboard, while the strip holds it: the focused cell moves
    /// with Left/Right — wrapping at either end — and Home/End, and only
    /// Enter or Space change the document in front. Escape hands the keyboard
    /// back to the group's text; anything else is not the strip's.
    ///
    /// `visible` is the cells the last draw put on screen. A document closed
    /// since is dropped here rather than acted on by position.
    pub(super) fn editor_strip_key(
        &mut self,
        visible: &[BufferId],
        ev: &gpui_kit::KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // A chord belongs to whoever bound it; the strip owns bare keys only.
        if ev.keystroke.modifiers.modified() {
            return;
        }
        let Some(code) = self.tab_code() else {
            return;
        };
        let shown: Vec<BufferId> = visible
            .iter()
            .copied()
            .filter(|id| code.files.contains(id))
            .collect();
        let Some(current) = self.editor_strip_target(&shown) else {
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
                self.editor_strip_select(current, window, cx);
                cx.stop_propagation();
                return;
            }
            "escape" => {
                self.focus_editor(window, cx);
                cx.stop_propagation();
                return;
            }
            _ => return,
        };
        if let Some(code) = self.tab_code_mut() {
            code.remember_strip_focus(next);
        }
        cx.stop_propagation();
        cx.notify();
    }

    /// Pointer activation of a strip cell: the document comes to the front
    /// and its text takes the keyboard, as clicking a tab always did.
    pub(super) fn editor_strip_activate(
        &mut self,
        id: BufferId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.editor_strip_select(id, window, cx);
        self.focus_editor(window, cx);
    }

    /// Brings a document to the front from the strip without moving the
    /// keyboard. Keyboard activation keeps the cells navigable.
    fn editor_strip_select(&mut self, id: BufferId, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(pos) = self
            .tab_code()
            .and_then(|code| code.files.iter().position(|file| *file == id))
        else {
            return;
        };
        if let Some(code) = self.tab_code_mut() {
            code.active = pos;
            code.remember_strip_focus(id);
        }
        self.editor.dismiss_bar();
        cx.notify();
    }

    fn editor_strip_rows(&self, cx: &gpui_kit::App) -> Vec<PickerRow> {
        let Some(code) = self.tab_code() else {
            return Vec::new();
        };
        let front = code.active_id();
        let roots = code.roots.as_slice();
        let local_host = self.spawn_host(cx);
        code.by_recency()
            .into_iter()
            .filter_map(|id| {
                let f = self.editor.buffers.get(id)?;
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
    fn editor_strip_matches(&self, cap: usize, cx: &gpui_kit::App) -> (Vec<PickerRow>, usize) {
        let q = self
            .editor
            .strip_picker
            .as_ref()
            .map(|p| p.input.read(cx).value().trim().to_lowercase())
            .unwrap_or_default();
        let rows = self.editor_strip_rows(cx);
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

    pub(super) fn editor_toggle_strip_picker(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        from_keyboard: bool,
    ) {
        if self.editor.strip_picker.is_some() {
            self.editor_close_strip_picker(window, cx);
            return;
        }
        let n = self.tab_code().map_or(0, |c| c.files.len());
        let restore_strip = from_keyboard && self.editor.strip_keyboard.is_focused(window);
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t_fmt(L10nKey::EditorStripSearch, &[("n", &n.to_string())]))
        });
        input.update(cx, |state, cx| state.focus(window, cx));
        let _sub = cx.subscribe_in(&input, window, |this, _, ev: &InputEvent, _, cx| {
            if matches!(ev, InputEvent::Change) {
                if let Some(p) = this.editor.strip_picker.as_mut() {
                    p.hi = 0;
                }
                cx.notify();
            }
        });
        self.editor.strip_picker = Some(StripPicker {
            input,
            hi: 0,
            over_button: true,
            restore_strip,
            _sub,
        });
        cx.notify();
    }

    pub(super) fn editor_close_strip_picker(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(picker) = self.editor.strip_picker.take() else {
            return;
        };
        if picker.restore_strip {
            self.editor.strip_keyboard.focus(window, cx);
        } else {
            self.focus_editor(window, cx);
        }
        cx.notify();
    }

    fn editor_strip_pick(&mut self, id: BufferId, window: &mut Window, cx: &mut Context<Self>) {
        let restore_strip = self
            .editor
            .strip_picker
            .as_ref()
            .is_some_and(|picker| picker.restore_strip);
        self.editor.strip_picker = None;
        if restore_strip {
            // Selection changes the front document, not the keyboard owner.
            // The select path also remembers this document against the group
            // that is focused now, so the next render exposes a valid cell.
            self.editor_strip_select(id, window, cx);
            self.editor.strip_keyboard.focus(window, cx);
        } else if let Some(pos) = self
            .tab_code()
            .and_then(|c| c.files.iter().position(|f| *f == id))
        {
            self.editor_activate(pos, window, cx);
        }
        cx.notify();
    }

    fn on_strip_picker_key(
        &mut self,
        ev: &gpui_kit::KeyDownEvent,
        cap: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (rows, _) = self.editor_strip_matches(cap, cx);
        let Some(p) = self.editor.strip_picker.as_mut() else {
            return;
        };
        let n = rows.len();
        match ev.keystroke.key.as_str() {
            "escape" => self.editor_close_strip_picker(window, cx),
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
                    self.editor_strip_pick(id, window, cx);
                }
            }
            _ => return,
        }
        cx.stop_propagation();
    }

    /// The `+N` button at the end of the strip, with its list when open.
    pub(super) fn render_strip_overflow(
        &self,
        hidden: &[BufferId],
        cap: usize,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let open = self.editor.strip_picker.is_some();
        // The chip exists only when there is something behind it, but the
        // list can be opened from the keyboard with every file in the strip,
        // so it is still anchored at the strip's end then.
        if hidden.is_empty() && !open {
            return None;
        }
        let theme = cx.theme();
        let hidden_dirty = hidden
            .iter()
            .any(|id| self.editor.buffers.get(*id).is_some_and(|f| f.dirty));
        let button = h_flex()
            .id("editor-strip-more")
            .occlude()
            .flex_none()
            .h(px(CELL_H))
            .px(px(8.))
            .gap(px(5.))
            .items_center()
            .rounded(px(CELL_RADIUS))
            .text_size(rems(12. / 16.))
            .font_features(crate::ui::theme::tabular_figures())
            .text_color(theme.muted_foreground)
            .role(gpui_kit::Role::Button)
            .aria_label(t(L10nKey::EditorStripAllFiles))
            .when(open, |d| d.bg(theme.sidebar_accent))
            .hover(|s| s.bg(theme.sidebar_accent).text_color(theme.foreground))
            .when(hidden_dirty, |d| {
                d.child(
                    div()
                        .size(px(crate::ui::tab_strip::ROW_STATUS_DOT))
                        .rounded_full()
                        .bg(theme.muted_foreground),
                )
            })
            .child(format!("+{}", hidden.len()))
            .child(
                Icon::new(IconName::ChevronDown)
                    .xsmall()
                    .text_color(theme.muted_foreground),
            )
            .tooltip(|window, cx| {
                gpui_kit::component::tooltip::Tooltip::new(t(L10nKey::EditorStripAllFiles))
                    .build(window, cx)
            })
            .on_hover(cx.listener(|this, over: &bool, _, _| {
                if let Some(p) = this.editor.strip_picker.as_mut() {
                    p.over_button = *over;
                }
            }))
            .on_click(cx.listener(|this, _, window, cx| {
                this.editor_toggle_strip_picker(window, cx, false);
            }));
        Some(
            div()
                .relative()
                .flex_none()
                .children((!hidden.is_empty()).then(|| button.into_any_element()))
                .children(self.render_strip_picker(cap, cx))
                .into_any_element(),
        )
    }

    fn render_strip_picker(&self, cap: usize, cx: &mut Context<Self>) -> Option<AnyElement> {
        let picker = self.editor.strip_picker.as_ref()?;
        let (rows, hidden) = self.editor_strip_matches(cap, cx);
        let querying = !picker.input.read(cx).value().trim().is_empty();
        let hi = picker.hi.min(rows.len().saturating_sub(1));
        let theme = cx.theme();
        let (fg, muted) = (theme.foreground, theme.muted_foreground);
        let row_hover = gpui_kit::rgb(cx.global::<crate::ui::presets::Surfaces>().popover.hover);

        let heading = |label: SharedString| {
            div()
                .h(px(24.))
                .pt(px(4.))
                .px(px(8.))
                .flex()
                .items_center()
                .text_size(rems(11. / 16.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(muted)
                .child(label)
        };
        let empty = rows.is_empty();
        let mut list = v_flex()
            .id("editor-strip-list")
            .max_h(px(PICKER_LIST_MAX_H))
            .overflow_y_scroll()
            .gap(px(1.));
        for (i, row) in rows.into_iter().enumerate() {
            if !querying && i == 0 && hidden > 0 {
                list = list.child(heading(
                    t_fmt(L10nKey::EditorStripHidden, &[("n", &hidden.to_string())]).into(),
                ));
            }
            if !querying && i == hidden {
                list = list.child(heading(t(L10nKey::EditorStripInBar).into()));
            }
            let id = row.id;
            let group: SharedString = format!("editor-strip-row-{i}").into();
            let slot = div()
                .id(("editor-strip-row-close", i))
                .relative()
                .flex_none()
                .size(px(18.))
                .rounded(px(4.))
                .hover(|s| s.bg(theme.muted))
                .when(row.dirty, |d| {
                    d.child(
                        div()
                            .absolute()
                            .inset_0()
                            .flex()
                            .items_center()
                            .justify_center()
                            .group_hover(group.clone(), |s| s.invisible())
                            .child(div().size(px(6.)).rounded_full().bg(muted)),
                    )
                })
                .child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .when(row.dirty, |d| d.invisible())
                        .group_hover(group.clone(), |s| s.visible())
                        .child(Icon::new(IconName::Close).xsmall().text_color(muted)),
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    cx.stop_propagation();
                    this.editor_close_buffer(id, window, cx);
                }));
            list = list.child(
                h_flex()
                    .id(("editor-strip-row", i))
                    .group(group)
                    .flex_none()
                    .h(px(PICKER_ROW_H))
                    .pl(px(8.))
                    .pr(px(4.))
                    .gap(px(8.))
                    .items_center()
                    .rounded(px(6.))
                    .when(i == hi, |d| d.bg(row_hover))
                    .on_mouse_move(cx.listener(move |this, _, _, cx| {
                        if let Some(p) = this.editor.strip_picker.as_mut()
                            && p.hi != i
                        {
                            p.hi = i;
                            cx.notify();
                        }
                    }))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.editor_strip_pick(id, window, cx);
                    }))
                    .child(
                        div()
                            .min_w(px(40.))
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .overflow_hidden()
                            .text_color(fg)
                            .when(row.front, |d| d.font_weight(FontWeight::MEDIUM))
                            .child(row.name),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .overflow_hidden()
                            .text_size(rems(11.5 / 16.))
                            .text_color(muted)
                            .child(row.dir),
                    )
                    .child(slot),
            );
        }
        if empty {
            list = list.child(
                div()
                    .h(px(PICKER_ROW_H))
                    .px(px(8.))
                    .flex()
                    .items_center()
                    .text_color(muted)
                    .child(t(L10nKey::EditorStripNoMatch)),
            );
        }

        let rule = || div().h(px(1.)).mx(px(6.)).my(px(4.)).bg(theme.border);
        let footer_button = |key: &'static str, label: L10nKey| {
            div()
                .id(key)
                .text_color(muted)
                .hover(|s| s.text_color(fg))
                .child(t(label))
        };
        let card = v_flex()
            .occlude()
            .w(px(PICKER_W))
            .p(px(5.))
            .bg(theme.popover)
            .text_color(theme.popover_foreground)
            .border_1()
            .border_color(theme.border)
            .rounded(px(10.))
            .shadow_lg()
            .text_size(rems(CELL_TEXT))
            .on_key_down(
                cx.listener(move |this, ev: &gpui_kit::KeyDownEvent, window, cx| {
                    this.on_strip_picker_key(ev, cap, window, cx);
                }),
            )
            .on_mouse_down_out(cx.listener(|this, _, window, cx| {
                if this
                    .editor
                    .strip_picker
                    .as_ref()
                    .is_some_and(|p| p.over_button)
                {
                    return;
                }
                this.editor_close_strip_picker(window, cx);
            }))
            .child(
                h_flex()
                    .h(px(30.))
                    .px(px(4.))
                    .gap(px(3.))
                    .items_center()
                    .child(Icon::new(IconName::Search).xsmall().text_color(muted))
                    .child(
                        div().flex_1().min_w_0().child(
                            Input::new(&picker.input)
                                .appearance(false)
                                .small()
                                .text_size(rems(CELL_TEXT)),
                        ),
                    ),
            )
            .child(rule())
            .child(list)
            .child(rule())
            .child(
                h_flex()
                    .h(px(28.))
                    .px(px(8.))
                    .gap(px(14.))
                    .items_center()
                    .text_size(rems(12. / 16.))
                    .child(
                        footer_button("editor-strip-close-saved", L10nKey::EditorStripCloseSaved)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.editor.strip_picker = None;
                                let ids = this.editor_strip_others(true);
                                this.editor_close_buffers(ids, window, cx);
                                this.focus_editor(window, cx);
                            })),
                    )
                    .child(
                        footer_button("editor-strip-close-others", L10nKey::EditorStripCloseOthers)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.editor.strip_picker = None;
                                let ids = this.editor_strip_others(false);
                                this.editor_close_buffers(ids, window, cx);
                                this.focus_editor(window, cx);
                            })),
                    ),
            );
        Some(
            div()
                .absolute()
                .right_0()
                .top(px(CELL_H + 4.))
                .child(
                    deferred(
                        anchored()
                            .anchor(Anchor::TopRight)
                            .snap_to_window_with_margin(px(8.))
                            .child(card),
                    )
                    .with_priority(1),
                )
                .into_any_element(),
        )
    }

    /// Every file in the strip but the one in front — only the saved ones
    /// when `saved_only`.
    fn editor_strip_others(&self, saved_only: bool) -> Vec<BufferId> {
        let Some(code) = self.tab_code() else {
            return Vec::new();
        };
        let front = code.active_id();
        code.files
            .iter()
            .copied()
            .filter(|id| Some(*id) != front)
            .filter(|id| !saved_only || self.editor.buffers.get(*id).is_some_and(|f| !f.dirty))
            .collect()
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
    use lyrel_text::{EditorConfig, TextFormat};

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
        app.read_with(vcx, |app, _| app.active_buffer().unwrap().path.clone())
    }

    fn strip_has_keys(app: &Entity<LyrelApp>, vcx: &mut VisualTestContext) -> bool {
        vcx.update(|window, cx| app.read(cx).editor.strip_keyboard.is_focused(window))
    }

    fn content_has_keys(app: &Entity<LyrelApp>, vcx: &mut VisualTestContext) -> bool {
        vcx.update(|window, cx| {
            let app = app.read(cx);
            app.active_editor_input()
                .is_some_and(|input| input.read(cx).focus_handle(cx).contains_focused(window, cx))
        })
    }

    fn picker_has_keys(app: &Entity<LyrelApp>, vcx: &mut VisualTestContext) -> bool {
        vcx.update(|window, cx| {
            app.read(cx)
                .editor
                .strip_picker
                .as_ref()
                .is_some_and(|picker| picker.input.read(cx).focus_handle(cx).is_focused(window))
        })
    }

    fn sidebar_has_keys(app: &Entity<LyrelApp>, vcx: &mut VisualTestContext) -> bool {
        vcx.update(|window, cx| app.read(cx).sidebar_nav_focus.is_focused(window))
    }

    fn dirty(app: &Entity<LyrelApp>, vcx: &mut VisualTestContext, id: BufferId) -> bool {
        app.read_with(vcx, |app, _| app.editor.buffers.get(id).unwrap().dirty)
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
            app.editor_close_buffer(b, window, cx);
        });
        settle(&mut vcx);
        app.read_with(&vcx, |app, _| {
            let code = app.tab_code().unwrap();
            assert!(!code.files.contains(&b), "the closed document is gone");
            assert_eq!(
                app.active_buffer().unwrap().path,
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
            app.editor_close_buffer(edited, window, cx);
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
        vcx.simulate_prompt_answer(t(L10nKey::Cancel));
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
            let id = app.editor_new_buffer(
                host.clone(),
                path.clone(),
                None,
                BufferKind::Text,
                "fn main() {}\n".into(),
                TextFormat::default(),
                EditorConfig::default(),
                lyrel_text::detect_indent("", "rust"),
                None,
                window,
                cx,
            );
            app.editor_show_in_tab(tab, id, true, window, cx);
        });
        settle(&mut vcx);
        vcx.update(|window, cx| window.input("// edit", cx));
        settle(&mut vcx);
        assert!(app.read_with(&vcx, |app, _| app.active_buffer().unwrap().dirty));

        // The strip has the keyboard, and the document commands still reach
        // the document in front.
        vcx.simulate_keystrokes("secondary-alt-t");
        assert!(strip_has_keys(&app, &mut vcx));
        vcx.update(|window, cx| window.press("secondary-s", cx));
        wait_until(&mut vcx, |vcx| {
            app.read_with(vcx, |app, _| !app.active_buffer().unwrap().dirty)
        });
        assert_eq!(host.writes.load(std::sync::atomic::Ordering::SeqCst), 1);
    }
}
