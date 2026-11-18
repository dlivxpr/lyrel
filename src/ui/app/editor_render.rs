//! Shell-side rendering for the editor panel: `render_code_overlay` composes
//! the panel chrome around the document leaves that
//! `Entity<EditorPanelState>` renders (body, status line, save bar, gutter,
//! breadcrumbs, the empty panel and the conflict banner). The source
//! documents' right-click menus and the header's file strip live here too —
//! they answer with shell actions (closing buffers, revealing a path,
//! opening outside lyrel) exactly as they always have. Only document state
//! crosses the seam: reads through `Entity::read`, document mutations
//! through one `editor.update` per frame step, so the entity lease is never
//! held across a shell call.

use std::path::Path;

use gpui_kit::component::button::Button;
use gpui_kit::component::menu::{ContextMenuExt as _, PopupMenu, PopupMenuItem};
use gpui_kit::component::{Icon, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::{AnyElement, KeyDownEvent, MouseButton, SharedString, Window, div, px, rems};

use super::*;
use crate::ui::code_editor::commands;
use crate::ui::code_editor::strip::{self, StripCell};
use crate::ui::code_editor::{
    self, BookAddBookmark, BookNextChapter, BookNextPage, BookPreviousChapter, BookPreviousPage,
    BookSearch, BookToggleBookmarks, BookToggleContents, BufferId, DiskConflict, OpenFile, TabCode,
};
use crate::ui::document_column::DocumentChrome;
use crate::ui::i18n::{L10nKey, t};
use crate::ui::window_chrome::{TITLE_BAR_BORDER, TitleStripRow as _};

impl LyrelApp {
    pub(crate) fn render_code_overlay(
        &mut self,
        chrome: DocumentChrome,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if !self.code_panel_visible() {
            return None;
        }
        let tab = self.tabs.get(self.tabs.active_index()).map(|t| t.id());
        // The last file closed with the focus in it: keep the focus in the
        // panel, or gpui reports it lost and it falls back to the terminal.
        let (had_focus, no_file) = {
            let editor = self.editor.read(cx);
            let no_file = match self.tab_code() {
                Some(code) => editor.active_file(code).is_none(),
                None => true,
            };
            (editor.had_focus(), no_file)
        };
        if tab.is_some() && had_focus == tab && no_file {
            self.editor.update(cx, |editor, cx| {
                editor.empty_focus().focus(window, cx);
            });
        }
        let panel_has_focus = self.editor_panel_has_focus(window, cx);
        self.editor.update(cx, |editor, _cx| {
            editor.set_had_focus(tab.filter(|_| panel_has_focus));
        });
        self.editor_split_follow_focus(window, cx);
        self.editor_gutter_sync(cx);
        self.editor_nav_tick(cx);
        if let Some(code) = self.tab_code_mut() {
            code.note_front();
        }
        let breadcrumbs = self.render_editor_breadcrumbs(window, cx);
        let body_tab = tab.expect("visible editor tab");
        let body = self.render_editor_body(
            self.tab_code().and_then(TabCode::active_id),
            self.tab_code().map_or(0, TabCode::focused_slot),
            body_tab,
            window,
            cx,
        );
        let conflict = {
            let editor = self.editor.read(cx);
            self.tab_code()
                .and_then(TabCode::active_id)
                .and_then(|id| editor.buffers().get(id).map(|f| (id, f)))
                .and_then(|(id, f)| f.conflict.map(|c| (id, c, f.input.is_savable())))
        };
        let conflict_banner =
            conflict.map(|(id, c, text)| self.render_editor_conflict_banner(id, c, text, cx));
        let bar = self.render_editor_bar(cx);

        let header = self.render_editor_header(chrome, window, cx);
        // Every open document owns a status line at the bottom of its panel;
        // the window's global bar stays below it. Markdown used to be the only
        // editor with one, which left code files reading their cursor state
        // from the far-away global bar.
        let document_status = self.active_buffer(cx).is_some().then(|| {
            h_flex()
                .id(if self.markdown_document_active(cx) {
                    "markdown-status-bar"
                } else if self.svg_document_active(cx) {
                    "svg-status-bar"
                } else if self.json_document_active(cx) {
                    "json-status-bar"
                } else if self.yaml_document_active(cx) {
                    "yaml-status-bar"
                } else if self.toml_document_active(cx) {
                    "toml-status-bar"
                } else if self.csv_document_active(cx) {
                    "csv-status-bar"
                } else if self.xml_document_active(cx) {
                    "xml-status-bar"
                } else if self.image_document_active(cx) {
                    "image-status-bar"
                } else if self.sqlite_document_active(cx) {
                    "sqlite-status-bar"
                } else if self.pdf_document_active(cx) {
                    "pdf-status-bar"
                } else if self.drawio_document_active(cx) {
                    "drawio-status-bar"
                } else {
                    "code-status-bar"
                })
                .test_support()
                .flex_none()
                .w_full()
                .h(rems(1.625))
                .px_1()
                .border_t_1()
                .border_color(cx.theme().border)
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(self.render_document_status(cx))
        });
        let editor_col = v_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .child(header)
            .when_some(conflict_banner, |this, b| this.child(b))
            .children(bar)
            .child(self.render_editor_groups(breadcrumbs, body, window, cx))
            .children(self.render_editor_problems(window, cx))
            .when_some(document_status, |column, status| column.child(status));

        // The panel's own paint is the same either way; only the box is not.
        // Filling the workspace means stopping the window's translucency and
        // repainting the theme image the root's copy now sits under; docking
        // means sitting in the same plane as the right panel, which the column
        // wrapper has already painted.
        let shell = v_flex().id("code-panel");
        let shell = match chrome {
            DocumentChrome::Fill => shell
                .absolute()
                .inset_0()
                .occlude()
                // Opaque on purpose: this overlay covers the whole workspace
                // (everything but the detail panel) and an open file must never
                // let the window translucency / backdrop material show through
                // it. The preset's gradient fill is preserved, just with
                // alpha 1 — the same paint the settings overlay uses. The
                // theme background image is repainted on top of it, since the
                // root's copy now sits below this fill.
                .bg(crate::ui::theme::overlay_background(cx))
                .children(crate::ui::app::overlay_surface_layers(cx)),
            DocumentChrome::Dock => shell.size_full().min_w_0(),
        };
        let shell = self.editor_nav_actions(shell, cx);
        let shell = self.editor_split_command_sync(shell, cx);
        let empty_focus = self.editor.read(cx).empty_focus().clone();
        Some(
            shell
                .track_focus(&empty_focus)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, window, cx| {
                        if !this.editor_panel_has_focus(window, cx) {
                            this.editor.update(cx, |editor, cx| {
                                editor.empty_focus().focus(window, cx);
                            });
                        }
                        if let Some(tab) = this.tabs.get(this.tabs.active_index()) {
                            this.status_document_focus = Some((tab.id(), true));
                        }
                        cx.notify();
                    }),
                )
                .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                    if ev.keystroke.key != "escape" {
                        return;
                    }
                    if this.editor_gutter_close_peek(cx) {
                        cx.stop_propagation();
                        return;
                    }
                    // Escape in the go-to-line or Save As box dismisses the
                    // box, not the whole editor.
                    if this.editor.read(cx).has_bar() {
                        this.editor_close_bar(window, cx);
                        cx.stop_propagation();
                        return;
                    }
                    this.toggle_code_panel(window, cx);
                }))
                .child(h_flex().flex_1().min_h_0().w_full().child(editor_col))
                .into_any_element(),
        )
    }

    /// One group's text: the file `id`, rendered or as source, or the empty
    /// panel. `slot` tells the two groups' elements apart. `tab` and `code`
    /// are the shell-resolved active tab and its editor state; the document
    /// itself renders on the editor entity, and the source documents keep
    /// their right-click menu, attached here as before.
    pub(crate) fn render_editor_body(
        &mut self,
        id: Option<BufferId>,
        slot: usize,
        tab: TabId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(id) = id else {
            return self.render_editor_empty(cx).into_any_element();
        };
        let Some(code) = self.tab_code() else {
            return self.render_editor_empty(cx).into_any_element();
        };
        let body: gpui_kit::Stateful<gpui_kit::Div> = self.editor.update(cx, |editor, cx| {
            editor.render_body(id, slot, tab, code, window, cx)
        });
        // Only the plain-text surfaces carry the editor's own right-click
        // menu; the structured documents have none (as before the inversion).
        let source_menu = self.editor.read(cx).buffers().get(id).is_some_and(|f| {
            f.input.code().is_some() || f.input.xml().is_some() || f.input.csv().is_some()
        });
        if !source_menu {
            return body.into_any_element();
        }
        let app = cx.entity().downgrade();
        body.context_menu(move |menu, _window, cx| Self::editor_body_menu(menu, &app, id, cx))
            .into_any_element()
    }

    /// The editor's text area: one group, or two side by side.
    pub(crate) fn render_editor_groups(
        &mut self,
        focused_crumbs: Option<AnyElement>,
        focused_body: AnyElement,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let pane = |crumbs: Option<AnyElement>, body: AnyElement| {
            v_flex()
                .flex_1()
                .min_w_0()
                .min_h_0()
                .h_full()
                .children(crumbs)
                .child(div().flex_1().min_h_0().child(body))
        };
        let Some(code) = self.tab_code() else {
            return pane(focused_crumbs, focused_body).into_any_element();
        };
        if !code.is_split() {
            return pane(focused_crumbs, focused_body).into_any_element();
        }
        let focus_left = code.focus_is_left();
        let other = code.other_active_id();
        let same = other.is_some() && other == code.active_id();
        let other_slot = usize::from(focus_left);
        let tab = self
            .tabs
            .get(self.tabs.active_index())
            .expect("active editor tab")
            .id();
        let (other_crumbs, other_body) = match other {
            Some(id)
                if same
                    && self
                        .editor
                        .read(cx)
                        .buffers()
                        .get(id)
                        .is_some_and(|file| file.input.code().is_some()) =>
            {
                (None, self.render_editor_same_file(id, focus_left, cx))
            }
            Some(id) => (
                self.render_editor_breadcrumbs_for(id, other_slot, false, window, cx),
                self.render_editor_body(Some(id), other_slot, tab, window, cx),
            ),
            None => (None, self.render_editor_empty(cx).into_any_element()),
        };
        let accent = cx.theme().primary;
        let focused = pane(focused_crumbs, focused_body)
            // The physical group's path must survive a focus swap between
            // mouse-down and click, including clicks on toolbar controls.
            .id(("editor-group", usize::from(!focus_left)))
            .border_t_2()
            .border_color(accent)
            .into_any_element();
        let unfocused = pane(other_crumbs, other_body)
            .border_t_2()
            .border_color(gpui_kit::transparent_black())
            .id(("editor-group", other_slot))
            // Before the text sees the click, so what the click does lands in
            // the group it was made in.
            .capture_any_mouse_down(cx.listener(move |this, _, window, cx| {
                this.editor_focus_group(!focus_left, window, cx);
            }))
            .into_any_element();
        let divider = div()
            .flex_none()
            .w(px(1.))
            .h_full()
            .bg(cx.theme().sidebar_border);
        let (left, right) = match focus_left {
            true => (focused, unfocused),
            false => (unfocused, focused),
        };
        h_flex()
            .items_stretch()
            .flex_1()
            .min_h_0()
            .size_full()
            .child(left)
            .child(divider)
            .child(right)
            .into_any_element()
    }

    /// The other group's view of the file the focused group is showing: it is
    /// drawn once, there, and a click brings it here.
    fn render_editor_same_file(
        &self,
        id: BufferId,
        focus_left: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let name: SharedString = self
            .editor
            .read(cx)
            .buffers()
            .get(id)
            .map(|f| f.label())
            .unwrap_or_default();
        let theme = cx.theme();
        v_flex()
            .id("editor-group-same-file")
            .size_full()
            .items_center()
            .justify_center()
            .gap_1()
            .cursor_pointer()
            .text_size(gpui_kit::rems(crate::ui::right_panel::TEXT))
            .text_color(theme.muted_foreground)
            .child(div().text_color(theme.foreground).child(name))
            .child(t(L10nKey::EditorSplitSameFile))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, window, cx| {
                    this.editor_focus_group(!focus_left, window, cx);
                }),
            )
            .into_any_element()
    }

    fn render_editor_header(
        &mut self,
        chrome: DocumentChrome,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let row = h_flex().id("editor-header");
        // v6 chrome: no rule under the bar — the header, the breadcrumbs and
        // the text are one plane — and only as many file cells as the column
        // has room for, the rest behind a `+N` list (see `strip`).
        let (tile, glyph) = (
            crate::ui::tab_strip::RAIL_TILE,
            crate::ui::tab_strip::RAIL_TILE_GLYPH,
        );
        let cap = self.editor_strip_cap(chrome, cx);
        let (shown, hidden) = self
            .tab_code()
            .map(|c| c.strip_split(cap))
            .unwrap_or_default();
        let active_id = self.tab_code().and_then(TabCode::active_id);
        // The cells the strip actually draws, in strip order — the group's
        // remembered focus target is clamped to these, so closing a document
        // or a group switch never leaves the keyboard on a cell that is gone.
        let mut nav_ids: Vec<BufferId> = Vec::with_capacity(shown.len());
        let cells: Vec<(usize, BufferId, SharedString, String, bool, bool)> = shown
            .into_iter()
            .filter_map(|(pos, id)| {
                let editor = self.editor.read(cx);
                let f = editor.buffers().get(id)?;
                nav_ids.push(id);
                // A file that has never been saved is not anywhere yet: its
                // path is empty until the first save, so the label stands in
                // for a location.
                let tip = match f.path.as_os_str().is_empty() {
                    true => f.label().to_string(),
                    false => f.path.display().to_string(),
                };
                Some((pos, id, f.label(), tip, Some(id) == active_id, f.dirty))
            })
            .collect();
        let strip_focused = self.editor.read(cx).strip_keyboard().is_focused(window);
        let nav_focus = self.editor_strip_target(&nav_ids, cx);
        let strip = if cells.is_empty() {
            div()
                .min_w_0()
                .px(px(8.))
                .text_ellipsis()
                .text_size(gpui_kit::rems(strip::CELL_TEXT))
                .text_color(cx.theme().muted_foreground)
                .child(SharedString::from(t(L10nKey::EditorNoFileOpen)))
                .into_any_element()
        } else {
            let count = cells.len();
            let keys = nav_ids.clone();
            let strip_keyboard = self.editor.read(cx).strip_keyboard().clone();
            h_flex()
                .id("editor-file-tabs")
                // One Tab stop for the group, and the keyboard contract of a
                // tab list: Left/Right and Home/End move the focused cell
                // without activating it, Enter/Space activate, Escape hands
                // the keyboard back to the text. The focused cell is the
                // group's own remembered document (see `strip`).
                .key_context(strip::KEY_CONTEXT)
                .role(gpui_kit::Role::TabList)
                .aria_label(t(L10nKey::EditorDocumentTabs))
                .aria_orientation(gpui_kit::accesskit::Orientation::Horizontal)
                .track_focus(&strip_keyboard)
                .on_key_down(
                    cx.listener(move |this, ev: &gpui_kit::KeyDownEvent, window, cx| {
                        this.editor_strip_key(&keys, ev, window, cx);
                    }),
                )
                .min_w_0()
                .gap(px(2.))
                .overflow_hidden()
                .children(cells.into_iter().enumerate().map(
                    |(index, (_, id, name, tip, active, dirty))| {
                        self.render_file_tab(
                            strip::StripCell {
                                id,
                                name,
                                tip,
                                active,
                                dirty,
                                nav_focused: Some(id) == nav_focus,
                                strip_focused,
                                position: index + 1,
                                count,
                            },
                            cx,
                        )
                    },
                ))
                .into_any_element()
        };
        let back = self.render_document_terminal_button(chrome, window, cx);
        row.flex_none()
            // The border is spent on the box, not on the paint: this header
            // shows no rule, but its tiles sit in the band the traffic lights
            // hang over and have to keep the title bar's line.
            .title_strip_row(px(TITLE_BAR_BORDER), cx.theme().transparent)
            .gap(px(2.))
            // The whole reserve, not the reserve less the first control's own
            // inset. The filled editor leads with the back pill, whose 8pt of
            // inner padding is *inside* its hover box — carving it out of here
            // starts that box 8pt early and consumes part of the gap reserved
            // after the traffic lights, even when the lead follows their inset.
            // Docked, the row leads with the file tabs instead, and the same
            // subtraction put their first
            // glyph at 4pt rather than the `CONTENT_INSET` this bar is built on.
            .pl(px(crate::ui::window_chrome::CONTENT_INSET))
            // The glyph, not the tile, lands on `CONTENT_INSET`, the column
            // the file name starts on at the other end of the bar.
            .pr(px(
                crate::ui::window_chrome::CONTENT_INSET - (tile - glyph) / 2.
            ))
            .children(back)
            .child(strip)
            .children(self.render_strip_overflow(&hidden, cap, cx))
            .child(
                div().occlude().flex_none().child(
                    crate::ui::window_chrome::chrome_tile_sized(
                        Button::new("editor-new-file").icon(Icon::new(IconName::Plus)),
                        tile,
                        glyph,
                        false,
                        cx,
                    )
                    .rounded(px(crate::ui::tab_strip::RAIL_TILE_RADIUS))
                    .tooltip(t(L10nKey::EditorNewFile))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.editor_new_file(window, cx);
                    })),
                ),
            )
            // Whatever is left of the bar stays a place to drag the window by.
            .child(div().flex_1().h_full())
            .child(self.document_layout_switch(chrome, cx))
            .child(
                div().occlude().flex_none().ml(px(4.)).child(
                    crate::ui::window_chrome::chrome_tile_sized(
                        Button::new("editor-panel-close").icon(Icon::new(IconName::Close)),
                        tile,
                        glyph,
                        false,
                        cx,
                    )
                    .rounded(px(crate::ui::tab_strip::RAIL_TILE_RADIUS))
                    .tooltip(t(L10nKey::EditorBackToTerminal))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.toggle_code_panel(window, cx);
                    })),
                ),
            )
    }

    /// One file in the header's strip: its name, and a slot that shows the
    /// unsaved dot at rest and the close button under the pointer — the dot
    /// says there is something to lose before the × offers to lose it.
    fn render_file_tab(&self, cell: StripCell, cx: &mut Context<Self>) -> AnyElement {
        let StripCell {
            id,
            name,
            tip,
            active,
            dirty,
            nav_focused,
            strip_focused,
            position,
            count,
        } = cell;
        let group: SharedString = format!("editor-file-tab-{}", id.0).into();
        let slot = 16.;
        let muted = cx.theme().muted_foreground;
        let close = div()
            .id(("editor-file-tab-close", id.0))
            .role(gpui_kit::Role::Button)
            .aria_label(format!("{} {name}", t(L10nKey::Close)))
            .flex_none()
            .size(px(slot))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(4.))
            .hover(|s| s.bg(cx.theme().muted))
            .child(Icon::new(IconName::Close).xsmall().text_color(muted))
            .on_click(cx.listener(move |this, _, window, cx| {
                cx.stop_propagation();
                this.editor_close_buffer(id, window, cx);
            }));
        // Unsaved is a state, not an alarm: the dot is in caption ink, the
        // same as the `+N` button's, not the warning hue.
        let slot_el = div()
            .flex_none()
            .size(px(slot))
            .flex()
            .items_center()
            .justify_center()
            .when(dirty, |d| {
                d.child(
                    div()
                        .group_hover(group.clone(), |s| s.invisible())
                        .size(px(6.))
                        .rounded_full()
                        .bg(muted),
                )
            });
        div()
            .id(("editor-file-tab", id.0))
            .role(gpui_kit::Role::Tab)
            .aria_label(match dirty {
                true => format!("{name}, {}", t(L10nKey::SettingsUnsaved)),
                false => name.to_string(),
            })
            .aria_selected(active)
            .aria_position_in_set(position)
            .aria_size_of_set(count)
            .group(group.clone())
            .occlude()
            .flex_none()
            .min_w_0()
            .max_w(px(strip::CELL_MAX_W))
            .h(px(strip::CELL_H))
            .flex()
            .items_center()
            .gap(px(6.))
            .pl(px(10.))
            .pr(px(6.))
            .rounded(px(strip::CELL_RADIUS))
            .text_size(gpui_kit::rems(strip::CELL_TEXT))
            // The strip's keyboard focus, drawn inside the cell and distinct
            // from the selected treatment: the selected document is where
            // the text is, the focused cell is what Enter or Space will show.
            // The transparent border keeps the label from shifting when it
            // lights up.
            .border_1()
            .border_color(match nav_focused && strip_focused {
                true => cx.theme().primary,
                false => gpui_kit::transparent_black(),
            })
            .map(|d| match active {
                true => d
                    .font_weight(gpui_kit::FontWeight::MEDIUM)
                    .text_color(cx.theme().foreground)
                    .bg(cx.theme().sidebar_accent),
                false => d.text_color(muted).hover(|s| {
                    s.bg(gpui_kit::rgb(
                        cx.global::<crate::ui::presets::Surfaces>().sidebar.hover,
                    ))
                }),
            })
            .child(
                div()
                    .min_w_0()
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .text_ellipsis()
                    .child(name),
            )
            .child(
                div()
                    .relative()
                    .flex_none()
                    .size(px(slot))
                    .child(div().absolute().inset_0().child(slot_el))
                    .child(
                        div()
                            .absolute()
                            .inset_0()
                            .when(!active, |d| d.invisible())
                            .when(dirty, |d| d.invisible())
                            .group_hover(group, |s| s.visible())
                            .child(close),
                    ),
            )
            .tooltip(move |window, cx| {
                gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx)
            })
            .on_click(cx.listener(move |this, _, window, cx| {
                this.editor_strip_activate(id, window, cx);
            }))
            .on_mouse_down(
                MouseButton::Middle,
                cx.listener(move |this, _, window, cx| {
                    this.editor_close_buffer(id, window, cx);
                }),
            )
            .context_menu({
                let app = cx.entity().downgrade();
                move |menu, _window, cx| Self::editor_tab_menu(menu, &app, id, cx)
            })
            .into_any_element()
    }

    fn render_editor_bar(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        self.editor.read(cx).render_bar(cx)
    }

    fn render_editor_empty(&self, cx: &mut Context<Self>) -> gpui_kit::Div {
        self.editor.update(cx, |editor, cx| editor.render_empty(cx))
    }

    fn render_editor_conflict_banner(
        &self,
        id: BufferId,
        conflict: DiskConflict,
        text: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let root = self.editor_save_root();
        self.editor.update(cx, |editor, cx| {
            editor.render_conflict_banner(id, conflict, text, root, cx)
        })
    }

    pub(crate) fn render_document_status(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(code) = self.tab_code() else {
            return div().into_any_element();
        };
        let tab = self
            .tabs
            .get(self.tabs.active_index())
            .expect("visible editor tab")
            .id();
        // The LSP status element is shell-rendered (it reads the window's LSP
        // store); the document row itself renders on the editor entity.
        let lsp = self.render_lsp_status(cx).map(|(element, _)| element);
        self.editor
            .update(cx, |editor, cx| editor.render_status(tab, code, lsp, cx))
    }

    // ---- Once-per-frame group focus and gutter bookkeeping ----

    /// Gives the focus to the group on the left (or right). `false` when there
    /// is no such group to go to — not split, or already there — so the chord
    /// can carry on to the panes.
    pub(crate) fn editor_focus_group(
        &mut self,
        left: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        // The strip is shared: a group switch made while its cells hold the
        // keyboard moves the strip to the other group and keeps the keyboard
        // there, on that group's own remembered document, instead of
        // dropping the user into the text they did not ask to edit.
        let strip_has_keys = self.editor.read(cx).strip_keyboard().is_focused(window);
        let Some(code) = self.tab_code_mut() else {
            return false;
        };
        if !code.is_split() || code.focus_is_left() == left {
            return false;
        }
        code.swap_focus();
        self.editor.update(cx, |editor, _cx| editor.dismiss_bar());
        if !strip_has_keys {
            self.focus_editor(window, cx);
        }
        cx.notify();
        true
    }

    /// Keeps the focused group the one whose text has the keyboard: a click,
    /// or anything else that focuses the other group's editor, moves the
    /// group focus there too. Run on every draw, and before every editor
    /// command ([`Self::editor_split_command_sync`]): focus moves between
    /// frames, and F12 or F2 in the first frame after one must act on the
    /// group the caret is now in.
    pub(crate) fn editor_split_follow_focus(&mut self, window: &Window, cx: &mut Context<Self>) {
        let Some(other) = self.tab_code().and_then(TabCode::other_active_id) else {
            return;
        };
        let Some(code) = self.tab_code() else {
            return;
        };
        let tab = self
            .tabs
            .get(self.tabs.active_index())
            .expect("active editor tab")
            .id();
        let other_group = code.split.as_ref().expect("other editor group").id;
        let active = code.active_id();
        let focused =
            self.editor
                .read(cx)
                .split_follow_focus(other, active, tab, other_group, window, cx);
        if focused && let Some(code) = self.tab_code_mut() {
            code.swap_focus();
            cx.notify();
        }
    }

    /// Splits the editor: the file in front opens in a second group to the
    /// right, which takes the focus. Already split, it opens in the other
    /// group instead. `false` with no file to split.
    pub(crate) fn editor_split(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(code) = self.tab_code_mut() else {
            return false;
        };
        let Some(id) = code.active_id() else {
            return false;
        };
        code.split_with(id);
        let (Some(tab), Some(code)) = (
            self.tabs.get(self.tabs.active_index()).map(|t| t.id()),
            self.tab_code(),
        ) else {
            return true;
        };
        let other_group = code
            .split
            .as_ref()
            .expect("split keeps the origin group")
            .id;
        let group = code.group();
        self.editor.update(cx, |editor, cx| {
            editor.open_split_presentations(id, tab, other_group, group, window, cx);
            editor.dismiss_bar();
        });
        self.focus_editor(window, cx);
        cx.notify();
        true
    }

    /// Settles the group focus before any editor command runs, on the element
    /// that holds the editor.
    ///
    /// gpui draws a window whose focus moved before it dispatches the next
    /// key, which settles the group on its own; a command from the menu bar
    /// or a context menu is dispatched without that draw. A capture-phase
    /// listener runs before the command's own handler wherever that handler
    /// sits — here, in `app.rs` or in `ui::lsp` — and lets the command carry
    /// on.
    pub(crate) fn editor_split_command_sync<E: InteractiveElement>(
        &self,
        element: E,
        cx: &mut Context<Self>,
    ) -> E {
        use crate::core::actions::*;
        macro_rules! before {
            ($el:expr, $($action:ty),+ $(,)?) => {
                $el$(.capture_action(cx.listener(|this, _: &$action, window, cx| {
                    this.editor_split_follow_focus(window, cx);
                })))+
            };
        }
        before!(
            element,
            EditorSave,
            EditorSaveAs,
            EditorGoToLine,
            EditorGoToSymbol,
            EditorNavigateBack,
            EditorNavigateForward,
            EditorSplitRight,
            EditorGoToDefinition,
            EditorQuickFix,
            EditorRenameSymbol,
            EditorFormatDocument,
            EditorTransformUppercase,
            EditorTransformLowercase,
            EditorTransformTitleCase,
            EditorTrimTrailingWhitespace,
            EditorJoinLines,
            EditorRemoveSurroundingBrackets,
            EditorFindReferences,
            EditorNextChange,
            EditorPrevChange,
            EditorRevertChange,
            EditorPeekChange,
            ToggleDocumentPreview,
            ToggleDocumentWrap,
            FindInTerminal,
            FindNext,
            FindPrevious,
            gpui_kit::component::input::Search,
            gpui_kit::component::input::Replace,
        )
    }

    /// Once a frame while the editor is on screen: make sure the file in
    /// front has a base that is current for its repository. Both groups'
    /// files when the editor is split: the one without the focus is on
    /// screen too.
    pub(crate) fn editor_gutter_sync(&mut self, cx: &mut Context<Self>) {
        let shown = self
            .tab_code()
            .map(|c| [c.active_id(), c.other_active_id()])
            .unwrap_or_default();
        self.editor.update(cx, |editor, cx| {
            editor.gutter_sync(shown, cx);
        });
    }

    /// Escape closes the peek before it closes anything else.
    pub(crate) fn editor_gutter_close_peek(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(id) = self.tab_code().and_then(TabCode::active_id) else {
            return false;
        };
        self.editor
            .update(cx, |editor, cx| editor.close_peek(id, cx))
    }

    /// Whether the cursor sits in a change that could be reverted — for the
    /// right-click menu, which reads the hunks as they were last drawn.
    pub(crate) fn editor_gutter_can_revert(&self, id: BufferId, cx: &gpui_kit::App) -> bool {
        let (Some(code), Some(tab)) = (
            self.tab_code(),
            self.tabs.get(self.tabs.active_index()).map(|t| t.id()),
        ) else {
            return false;
        };
        self.editor
            .read(cx)
            .gutter_can_revert(id, tab, code.focused_group(), cx)
    }

    // ---- Shell reads of the editor entity ----

    /// The file in front of the editor panel, if there is one.
    pub(crate) fn active_buffer<'a>(&self, cx: &'a gpui_kit::App) -> Option<&'a OpenFile> {
        let code = self.tab_code()?;
        self.editor.read(cx).active_file(code)
    }

    pub(crate) fn markdown_document_active(&self, cx: &gpui_kit::App) -> bool {
        let Some(code) = self.tab_code() else {
            return false;
        };
        self.editor.read(cx).markdown_active(code)
    }

    pub(crate) fn svg_document_active(&self, cx: &gpui_kit::App) -> bool {
        let Some(code) = self.tab_code() else {
            return false;
        };
        self.editor.read(cx).svg_active(code)
    }

    pub(crate) fn json_document_active(&self, cx: &gpui_kit::App) -> bool {
        let Some(code) = self.tab_code() else {
            return false;
        };
        self.editor.read(cx).json_active(code)
    }

    pub(crate) fn yaml_document_active(&self, cx: &gpui_kit::App) -> bool {
        let Some(code) = self.tab_code() else {
            return false;
        };
        self.editor.read(cx).yaml_active(code)
    }

    pub(crate) fn toml_document_active(&self, cx: &gpui_kit::App) -> bool {
        let Some(code) = self.tab_code() else {
            return false;
        };
        self.editor.read(cx).toml_active(code)
    }

    pub(crate) fn csv_document_active(&self, cx: &gpui_kit::App) -> bool {
        let Some(code) = self.tab_code() else {
            return false;
        };
        self.editor.read(cx).csv_active(code)
    }

    pub(crate) fn xml_document_active(&self, cx: &gpui_kit::App) -> bool {
        let Some(code) = self.tab_code() else {
            return false;
        };
        self.editor.read(cx).xml_active(code)
    }

    pub(crate) fn image_document_active(&self, cx: &gpui_kit::App) -> bool {
        let Some(code) = self.tab_code() else {
            return false;
        };
        self.editor.read(cx).image_active(code)
    }

    pub(crate) fn sqlite_document_active(&self, cx: &gpui_kit::App) -> bool {
        let Some(code) = self.tab_code() else {
            return false;
        };
        self.editor.read(cx).sqlite_active(code)
    }

    pub(crate) fn pdf_document_active(&self, cx: &gpui_kit::App) -> bool {
        let Some(code) = self.tab_code() else {
            return false;
        };
        self.editor.read(cx).pdf_active(code)
    }

    pub(crate) fn drawio_document_active(&self, cx: &gpui_kit::App) -> bool {
        let Some(code) = self.tab_code() else {
            return false;
        };
        self.editor.read(cx).drawio_active(code)
    }

    // ---- Menus ----

    /// The source document's own right-click menu: the editing commands, the
    /// document-type entries, and what both menus offer for the file as a
    /// whole. Nothing for a file that has never been saved, since it is not
    /// anywhere yet.
    fn editor_body_menu(
        menu: PopupMenu,
        app: &gpui_kit::WeakEntity<Self>,
        id: BufferId,
        cx: &mut gpui_kit::App,
    ) -> PopupMenu {
        use gpui_kit::component::input as edit;
        let Some(owner) = app.upgrade() else {
            return menu;
        };
        let this = owner.read(cx);
        let Some(code) = this.tab_code() else {
            return menu;
        };
        let Some(tab) = this.tabs.get(this.tabs.active_index()).map(|t| t.id()) else {
            return menu;
        };
        let panel = this.editor.read(cx);
        let Some(f) = panel.buffers().get(id) else {
            return menu;
        };
        let Some(input) = panel.editor_input_for(id, tab, code.focused_group()) else {
            return menu;
        };
        let state = input.read(cx);
        let mut menu = menu.min_w(px(220.)).action_context(state.focus_handle(cx));
        // An unsaved buffer has no location to attach, reveal, or copy: its
        // path stays empty until the first save.
        if !f.path.as_os_str().is_empty() {
            menu = menu
                .item(
                    PopupMenuItem::new(t(L10nKey::FileTreeContextAttachAgent)).on_click({
                        let app = app.clone();
                        move |_, _window, cx| {
                            let _ = app.update(cx, |this, cx| {
                                let (Some(code), Some(tab)) = (
                                    this.tab_code(),
                                    this.tabs.get(this.tabs.active_index()).map(|t| t.id()),
                                ) else {
                                    return;
                                };
                                let group = code.focused_group();
                                this.editor.update(cx, |editor, cx| {
                                    editor.attach_to_agent(id, tab, group, cx);
                                });
                            });
                        }
                    }),
                )
                .separator();
        }
        // 一张图片没有撤销、查找、剪贴板和行命令可给：它只有取景与翻页。给它
        // 保留这些行，等于向读者承诺文档里有能编辑的文本。
        if let Some(image) = input.image() {
            let multipage = image.read(cx).page_count(cx).is_some_and(|count| count > 1);
            return code_editor::image_document::menu_entries(menu, multipage);
        }
        if input.book().is_some() {
            return menu
                .separator()
                .menu(t(L10nKey::EditorBookContents), Box::new(BookToggleContents))
                .menu(
                    t(L10nKey::EditorBookBookmarks),
                    Box::new(BookToggleBookmarks),
                )
                .menu(t(L10nKey::EditorBookSearch), Box::new(BookSearch))
                .separator()
                .menu(t(L10nKey::EditorBookNextPage), Box::new(BookNextPage))
                .menu(
                    t(L10nKey::EditorBookPreviousPage),
                    Box::new(BookPreviousPage),
                )
                .menu(t(L10nKey::EditorBookNextChapter), Box::new(BookNextChapter))
                .menu(
                    t(L10nKey::EditorBookPreviousChapter),
                    Box::new(BookPreviousChapter),
                )
                .separator()
                .menu(t(L10nKey::EditorBookAddBookmark), Box::new(BookAddBookmark));
        }
        if let Some(pdf) = input.pdf() {
            let has_selection = pdf.read(cx).has_selection();
            return code_editor::pdf_document::menu_entries(menu, has_selection);
        }
        // 数据库没有撤销、查找、剪贴板和行命令可给：它只有刷新与分页。
        if input.sqlite().is_some() {
            return code_editor::sqlite_document::menu_entries(menu);
        }
        // The diagram keeps its own undo, clipboard and page commands: every
        // entry lands on the shared document or the live engine, never on a
        // text input it does not have.
        if let Some(drawio) = input.drawio() {
            let multipage = drawio.read(cx).page_count(cx) > 1;
            return code_editor::drawio_document::menu_entries(menu, multipage);
        }
        if let Some(csv) = input.csv() {
            let menu = menu.action_context(csv.read(cx).focus_handle(cx));
            let menu = code_editor::csv_document::menu_entries(menu, csv, csv.read(cx).mode(), cx);
            return owner.read(cx).editor_file_menu_items(menu, id, app, cx);
        }
        let selected = input
            .text()
            .is_some_and(|text| !text.read(cx).selected_range().is_empty());
        let menu = menu
            .menu(t(L10nKey::AppMenuUndo), Box::new(edit::Undo))
            .menu(t(L10nKey::AppMenuRedo), Box::new(edit::Redo))
            .separator()
            .menu_with_disabled(t(L10nKey::AppMenuCut), Box::new(edit::Cut), !selected)
            .menu_with_disabled(t(L10nKey::AppMenuCopy), Box::new(edit::Copy), !selected)
            .menu_with_disabled(
                t(L10nKey::AppMenuPaste),
                Box::new(edit::Paste),
                cx.read_from_clipboard().is_none(),
            )
            .menu(t(L10nKey::AppMenuSelectAll), Box::new(edit::SelectAll));
        if let Some(markdown) = input.markdown() {
            let menu = menu
                .action_context(markdown.read(cx).focus_handle(cx))
                .menu(
                    t(L10nKey::EditorGoToLineAction),
                    Box::new(crate::core::actions::EditorGoToLine),
                );
            return owner.read(cx).editor_file_menu_items(menu, id, app, cx);
        }
        if let Some(svg) = input.svg() {
            let menu = menu.action_context(svg.read(cx).focus_handle(cx)).menu(
                t(L10nKey::EditorGoToLineAction),
                Box::new(crate::core::actions::EditorGoToLine),
            );
            let menu = code_editor::svg_document::menu_entries(menu);
            return owner.read(cx).editor_file_menu_items(menu, id, app, cx);
        }
        if let Some(json) = input.json() {
            let menu = menu.action_context(json.read(cx).focus_handle(cx)).menu(
                t(L10nKey::EditorGoToLineAction),
                Box::new(crate::core::actions::EditorGoToLine),
            );
            let menu = code_editor::json_document::menu_entries(menu);
            return owner.read(cx).editor_file_menu_items(menu, id, app, cx);
        }
        if let Some(yaml) = input.yaml() {
            let menu = menu.action_context(yaml.read(cx).focus_handle(cx)).menu(
                t(L10nKey::EditorGoToLineAction),
                Box::new(crate::core::actions::EditorGoToLine),
            );
            let menu = code_editor::yaml_document::menu_entries(menu);
            return owner.read(cx).editor_file_menu_items(menu, id, app, cx);
        }
        if let Some(toml) = input.toml() {
            let menu = menu.action_context(toml.read(cx).focus_handle(cx)).menu(
                t(L10nKey::EditorGoToLineAction),
                Box::new(crate::core::actions::EditorGoToLine),
            );
            let menu = code_editor::toml_document::menu_entries(menu);
            return owner.read(cx).editor_file_menu_items(menu, id, app, cx);
        }
        if let Some(xml) = input.xml() {
            let menu = menu.action_context(xml.read(cx).focus_handle(cx)).menu(
                t(L10nKey::EditorGoToLineAction),
                Box::new(crate::core::actions::EditorGoToLine),
            );
            let menu = code_editor::xml_document::menu_entries(menu);
            return owner.read(cx).editor_file_menu_items(menu, id, app, cx);
        }
        let menu = menu
            .separator()
            // The line commands act on every line a cursor or selection
            // touches. Their chords live on gpui-component's `CodeEditor` key
            // context and are re-added in `keymap::fixed_bindings`, so they
            // beat the app's own keys while the editor has focus.
            .menu(
                t(L10nKey::EditorToggleComment),
                Box::new(commands::ToggleLineComment),
            )
            .menu(t(L10nKey::EditorMoveLineUp), Box::new(commands::MoveLineUp))
            .menu(
                t(L10nKey::EditorMoveLineDown),
                Box::new(commands::MoveLineDown),
            )
            .menu(
                t(L10nKey::EditorDuplicateLine),
                Box::new(commands::CopyLineDown),
            )
            .menu(t(L10nKey::EditorDeleteLine), Box::new(commands::DeleteLine))
            .separator()
            .menu(t(L10nKey::AppMenuFind), Box::new(edit::Search))
            .menu(
                t(L10nKey::EditorGoToLineAction),
                Box::new(crate::core::actions::EditorGoToLine),
            )
            .menu_with_disabled(
                t(L10nKey::EditorGitRevertChange),
                Box::new(crate::core::actions::EditorRevertChange),
                !this.editor_gutter_can_revert(id, cx),
            )
            .menu(
                t(L10nKey::EditorGoToMatchingBracket),
                Box::new(commands::MoveToMatchingBracket),
            );
        let menu = this.lsp_menu_items(menu, id, cx);
        this.editor_file_menu_items(menu, id, app, cx)
    }

    /// Right-click on a file in the header's strip.
    fn editor_tab_menu(
        menu: PopupMenu,
        app: &gpui_kit::WeakEntity<Self>,
        id: BufferId,
        cx: &gpui_kit::App,
    ) -> PopupMenu {
        let Some(this) = app.upgrade() else {
            return menu;
        };
        let this = this.read(cx);
        let Some(files) = this.tab_code().map(|c| c.files.clone()) else {
            return menu;
        };
        let Some(pos) = files.iter().position(|f| *f == id) else {
            return menu;
        };
        let others: Vec<BufferId> = files.iter().copied().filter(|f| *f != id).collect();
        let right = files[pos + 1..].to_vec();
        let close = |label: L10nKey, ids: Vec<BufferId>| {
            let app = app.clone();
            PopupMenuItem::new(t(label))
                .disabled(ids.is_empty())
                .on_click(move |_, window, cx| {
                    let ids = ids.clone();
                    let _ = app.update(cx, |this, cx| this.editor_close_buffers(ids, window, cx));
                })
        };
        let menu = menu
            .min_w(px(220.))
            .item(
                PopupMenuItem::new(t(L10nKey::TabContextCloseTab)).on_click({
                    let app = app.clone();
                    move |_, window, cx| {
                        let _ = app.update(cx, |this, cx| this.editor_close_buffer(id, window, cx));
                    }
                }),
            )
            .item(close(L10nKey::AppMenuCloseOtherTabs, others))
            .item(close(L10nKey::AppMenuCloseTabsRight, right));
        this.editor_file_menu_items(menu, id, app, cx)
    }

    /// What both menus offer for the file as a whole: open it outside lyrel,
    /// show it in its folder, copy where it is. Nothing for a file that has
    /// never been saved, since it is not anywhere yet.
    fn editor_file_menu_items(
        &self,
        menu: PopupMenu,
        id: BufferId,
        app: &gpui_kit::WeakEntity<Self>,
        cx: &gpui_kit::App,
    ) -> PopupMenu {
        // The path stays empty until a file's first save; that is what
        // "untitled" means everywhere these menus look at it.
        let Some(f) = self
            .editor
            .read(cx)
            .buffers()
            .get(id)
            .filter(|f| !f.path.as_os_str().is_empty())
        else {
            return menu;
        };
        let path = f.path.clone();
        // Only this machine's desktop can open or show a file, and only a
        // file that is on this machine — the same rule as the file tree.
        let local = f.host.id().is_local();
        let mut menu = menu.separator();
        if local && self.can_spawn_locally(cx) && !code_editor::is_program(&path) {
            let browser = code_editor::opens_in_browser(&path);
            let label = match browser {
                true => t(L10nKey::PanelOpenInBrowser),
                false => t(L10nKey::AppMenuOpenLinkWithDefaultApp),
            };
            let opener: fn(&Path) -> std::io::Result<()> = match browser {
                true => code_editor::open_in_browser,
                false => crate::ui::desktop_file::open_file_path,
            };
            menu = menu.item(PopupMenuItem::new(label).on_click({
                let app = app.clone();
                let path = path.clone();
                move |_, window, cx| {
                    let _ = app.update(cx, |_, cx| Self::open_with(&path, opener, window, cx));
                }
            }));
        }
        if local {
            menu = menu.item(
                PopupMenuItem::new(crate::ui::right_panel::reveal_label()).on_click({
                    let path = path.clone();
                    move |_, _window, cx| {
                        cx.reveal_path(&crate::ui::path_display::native_separators(&path));
                    }
                }),
            );
        }
        let copy = |label: L10nKey, text: String| {
            PopupMenuItem::new(t(label)).on_click(move |_, _window, cx| {
                cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(text.clone()));
            })
        };
        // A remote host's paths are already spelled its own way; only one on
        // this machine is re-spelled with its separators.
        let full = match local {
            true => crate::ui::path_display::native_separators(&path)
                .display()
                .to_string(),
            false => path.display().to_string(),
        };
        menu = menu.item(copy(L10nKey::FileTreeContextCopyPath, full));
        if let Some(rel) = self.path_under_tree_root(&path) {
            let rel = match local {
                true => crate::ui::path_display::native_separators(&rel)
                    .display()
                    .to_string(),
                false => rel.display().to_string(),
            };
            menu = menu.item(copy(L10nKey::EditorCopyRelativePath, rel));
        }
        menu
    }
}
