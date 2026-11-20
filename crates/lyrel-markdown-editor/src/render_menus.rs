//! The five mutually exclusive overlay menus rendered above the editor
//! surface, each anchored at its right-click position: Diagnostics
//! (suggestions + clipboard + inline format bar + color swatches +
//! turn-into), Table (row/column editing, alignment, styles), Property
//! (edit/delete), Image (delete), and CodeLang (the fence language picker).
//! Split from the `Render::render` monolith in `lib.rs`.

use super::*;

/// The right-click suggestions menu, absolutely positioned over the editor
/// (anchored at the click). `Option's `IntoIterator` renders zero or one
/// popup; clicking a row replaces the misspelled span.
pub(crate) fn diagnostics_menu(
    editor: &EditorState,
    cx: &mut Context<EditorState>,
) -> Option<gpui_kit::AnyElement> {
    editor.overlay_menu.diagnostics().cloned().map(|menu| {
        let DiagMenu {
            anchor,
            range,
            suggestions,
            scroll,
            turn_into: menu_turn_into,
        } = menu;
        let count = suggestions.len();
        // Menu chrome from the host's theme (fallbacks match the former
        // hardcoded dark menu when no markdown style is set).
        let st = editor.markdown_style.as_ref();
        let menu_bg = st.map_or(rgb(0x26262b).into(), |s| s.popover_bg);
        let menu_border = st.map_or(rgb(0x45454c).into(), |s| s.popover_border);
        let menu_fg = st.map_or(rgb(0xe6e6e6).into(), |s| s.popover_fg);
        let hover = st.map_or(rgba(0x2f6fd628).into(), |s| s.popover_hover);
        let mut thumb_c = st.map_or(rgba(0xffffff66).into(), |s| s.marker);
        thumb_c.a = 0.5;
        // Collected eagerly (not a lazy iterator) so `cx` is only
        // borrowed here and stays free for the menu's own listeners below.
        let rows: Vec<_> = suggestions
            .into_iter()
            .enumerate()
            .map(|(i, sugg)| {
                let range = range.clone();
                let replacement = sugg.to_string();
                div()
                    // A stable per-row id so gpui tracks hover state and
                    // repaints as the pointer moves between rows. Without
                    // an id, the hover style only shows on a forced
                    // repaint (e.g. while scrolling).
                    .id(("suggestion-row", i))
                    // Don't let the scroll container's max-height squeeze
                    // the rows; they keep their height and overflow.
                    .flex_shrink_0()
                    .px(px(10.))
                    .py(px(3.))
                    // Highlight the row under the pointer.
                    .hover(move |s| s.bg(hover))
                    .child(sugg)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |editor, _: &MouseDownEvent, window, cx| {
                            // Keep the editor's own mouse-down from clearing
                            // the menu / moving the caret out from under us.
                            cx.stop_propagation();
                            editor.apply_suggestion(range.clone(), &replacement, window, cx);
                        }),
                    )
            })
            .collect();
        // A thin scrollbar thumb, shown when the list overflows ~6 rows
        // so the scroll affordance is visible. Sized from the row count
        // (known now) and positioned from the live scroll offset — a
        // wheel scroll calls window.refresh(), which re-renders this.
        const ROW_H: f32 = 24.0;
        const PAD: f32 = 4.0;
        const MAX_H: f32 = 180.0;
        let rows_h = count as f32 * ROW_H;
        let view_h = MAX_H - 2.0 * PAD;
        let thumb = (rows_h > view_h).then(|| {
            let scrolled = (-f32::from(scroll.offset().y)).clamp(0.0, rows_h - view_h);
            let thumb_h = (view_h * view_h / rows_h).max(24.0);
            let thumb_top = PAD + scrolled / (rows_h - view_h) * (view_h - thumb_h);
            div()
                .absolute()
                .top(px(thumb_top))
                .right(px(2.))
                .w(px(6.))
                .h(px(thumb_h))
                .rounded(px(3.))
                .bg(thumb_c)
        });

        // Cut / Copy need a selection; Paste always applies (the caret
        // was seated at the click when it landed outside the selection).
        let has_sel = !editor.selection.selected_range.is_empty();
        let clip_item = |id: &'static str, label: SharedString| {
            div()
                .id(id)
                .test_support()
                .role(gpui_kit::Role::MenuItem)
                .aria_label(label.clone())
                .flex_shrink_0()
                .px(px(10.))
                .py(px(3.))
                .hover(move |s| s.bg(hover))
                .child(label)
        };
        let mut clipboard = div().flex().flex_col().py(px(4.));
        if has_sel {
            clipboard = clipboard
                .child(
                    clip_item("menu-cut", editor.labels.cut.clone()).on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|editor, _: &MouseDownEvent, window, cx| {
                            cx.stop_propagation();
                            editor.overlay_menu.dismiss();
                            editor.cut(&Cut, window, cx);
                        }),
                    ),
                )
                .child(
                    clip_item("menu-copy", editor.labels.copy.clone()).on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|editor, _: &MouseDownEvent, window, cx| {
                            cx.stop_propagation();
                            editor.overlay_menu.dismiss();
                            editor.copy(&Copy, window, cx);
                        }),
                    ),
                )
                // Plain-only: the raw markdown with no host flavors —
                // for pasting literal source into rich surfaces
                // (email, chat) where Copy's HTML flavor would win.
                .child(
                    clip_item("menu-copy-md", editor.labels.copy_as_markdown.clone())
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|editor, _: &MouseDownEvent, window, cx| {
                                cx.stop_propagation();
                                editor.overlay_menu.dismiss();
                                editor.copy_plain(window, cx);
                            }),
                        ),
                );
        }
        let clipboard = clipboard.child(
            clip_item("menu-paste", editor.labels.paste.clone()).on_mouse_down(
                MouseButton::Left,
                cx.listener(|editor, _: &MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    editor.overlay_menu.dismiss();
                    editor.paste(&Paste, window, cx);
                }),
            ),
        );

        // Inline-format bar (Cditor-style): with a selection, a strip of
        // B / I / S / <> buttons across the menu's top — each toggles its
        // markdown wrap on the selection and closes the menu.
        let fmt_btn = |id: &'static str| {
            div()
                .id(id)
                .w(px(28.))
                .h(px(24.))
                .rounded(px(4.))
                .flex()
                .items_center()
                .justify_center()
                .hover(move |s| s.bg(hover))
        };
        let format_bar = has_sel.then(|| {
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(2.))
                .px(px(6.))
                .py(px(4.))
                .child(
                    fmt_btn("menu-fmt-bold")
                        .font_weight(FontWeight::BOLD)
                        .child("B")
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|editor, _: &MouseDownEvent, window, cx| {
                                cx.stop_propagation();
                                editor.overlay_menu.dismiss();
                                editor.bold(&Bold, window, cx);
                            }),
                        ),
                )
                .child(
                    fmt_btn("menu-fmt-italic")
                        .italic()
                        .child("I")
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|editor, _: &MouseDownEvent, window, cx| {
                                cx.stop_propagation();
                                editor.overlay_menu.dismiss();
                                editor.italic(&Italic, window, cx);
                            }),
                        ),
                )
                .child(
                    fmt_btn("menu-fmt-underline")
                        .underline()
                        .child("U")
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|editor, _: &MouseDownEvent, window, cx| {
                                cx.stop_propagation();
                                editor.overlay_menu.dismiss();
                                editor.underline(&Underline, window, cx);
                            }),
                        ),
                )
                .child(
                    fmt_btn("menu-fmt-strike")
                        .line_through()
                        .child("S")
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|editor, _: &MouseDownEvent, window, cx| {
                                cx.stop_propagation();
                                editor.overlay_menu.dismiss();
                                editor.strike(&Strike, window, cx);
                            }),
                        ),
                )
                .child(
                    fmt_btn("menu-fmt-code")
                        .font_family(
                            editor
                                .markdown_style
                                .as_ref()
                                .map(|s| s.mono.family.clone())
                                .unwrap_or_else(|| "monospace".into()),
                        )
                        .text_size(px(12.))
                        .child("<>")
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|editor, _: &MouseDownEvent, window, cx| {
                                cx.stop_propagation();
                                editor.overlay_menu.dismiss();
                                editor.code(&Code, window, cx);
                            }),
                        ),
                )
        });

        // Color swatches under the format bar: text colors, then
        // highlights (theme tint first, then colored `<mark>`s).
        let mark_bg = st.map_or(rgba(0xFFD60066).into(), |s| s.mark_bg);
        let color_rows = has_sel.then(|| {
            div()
                .flex()
                .flex_col()
                .child(EditorState::color_row(
                    ColorKind::Text,
                    None,
                    menu_border,
                    hover,
                    editor.host_custom_colors,
                    cx,
                ))
                .child(EditorState::color_row(
                    ColorKind::Highlight,
                    Some(mark_bg),
                    menu_border,
                    hover,
                    editor.host_custom_colors,
                    cx,
                ))
        });

        // "Turn into" block conversion (Cditor-style): a row whose
        // hover opens a kind-list flyout beside the menu, the caret
        // block's current kind checked.
        let cur_kind =
            editor.block_kind_at(editor.row_col(editor.selection.selected_range.start).0);
        let turn_row = div()
            .id("menu-turn-into")
            .flex_shrink_0()
            .px(px(10.))
            .py(px(3.))
            .hover(move |s| s.bg(hover))
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap(px(16.))
            .child(editor.labels.turn_into.clone())
            .child(div().text_size(px(10.)).child("\u{25b8}"))
            .on_hover(cx.listener(|editor, hovered: &bool, _, cx| {
                if *hovered
                    && let Some(m) = editor.overlay_menu.diagnostics_mut()
                    && !m.turn_into
                {
                    m.turn_into = true;
                    cx.notify();
                }
            }));
        let turn_labels = editor.labels.clone();
        let turn_flyout = menu_turn_into.then(|| {
            let rows: Vec<_> = TurnKind::ALL
                .iter()
                .enumerate()
                .map(|(i, &k)| {
                    let checked = k == cur_kind;
                    div()
                        .id(("turn-kind", i))
                        .flex_shrink_0()
                        .px(px(10.))
                        .py(px(3.))
                        .hover(move |s| s.bg(hover))
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(6.))
                        .child(div().w(px(12.)).flex_shrink_0().child(if checked {
                            "\u{2713}"
                        } else {
                            ""
                        }))
                        .child(k.label(&turn_labels))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |editor, _: &MouseDownEvent, window, cx| {
                                cx.stop_propagation();
                                editor.overlay_menu.dismiss();
                                editor.turn_into(k, window, cx);
                            }),
                        )
                })
                .collect();
            // An absolute sibling of the (overflow-clipped) menu box —
            // out of flow, so it can't inflate the anchored bounds and
            // re-trigger the window snap (the slash-flyout lesson).
            div()
                .absolute()
                .left(gpui_kit::relative(1.))
                .bottom(px(0.))
                .ml(px(2.))
                .occlude()
                .min_w(px(140.))
                .cursor(CursorStyle::Arrow)
                .bg(menu_bg)
                .border_1()
                .border_color(menu_border)
                .rounded(px(6.))
                .shadow_md()
                .text_color(menu_fg)
                .text_size(px(13.))
                .flex()
                .flex_col()
                .py(px(4.))
                .children(rows)
        });

        // Deferred + anchored to a window-space top layer with `.occlude()`,
        // so it renders above the page chrome and captures the wheel — else a
        // scroll over the popup scrolls the page behind it.
        gpui_kit::deferred(
            gpui_kit::anchored()
                .position(anchor)
                .snap_to_window()
                .child(
                    div().relative().children(turn_flyout).child(
                        div()
                            .relative()
                            .occlude()
                            .min_w(px(150.))
                            // Override the editor's I-beam — the menu is a normal
                            // pointer surface (children inherit this hitbox's cursor).
                            .cursor(CursorStyle::Arrow)
                            .bg(menu_bg)
                            .border_1()
                            .border_color(menu_border)
                            .rounded(px(6.))
                            .shadow_md()
                            // Clip rows + thumb to the rounded box.
                            .overflow_hidden()
                            .text_color(menu_fg)
                            .text_size(px(13.))
                            // A click anywhere outside the menu dismisses it.
                            .on_mouse_down_out(cx.listener(|editor, _: &MouseDownEvent, _, cx| {
                                editor.overlay_menu.dismiss();
                                cx.notify();
                            }))
                            .children(format_bar)
                            .children(color_rows)
                            .children(has_sel.then(|| div().h(px(1.)).bg(menu_border)))
                            .children((count > 0).then(|| {
                                // The scroll viewport: shows ~6 rows, the rest scroll.
                                div()
                                    .id("suggestion-menu")
                                    .max_h(px(MAX_H))
                                    .overflow_y_scroll()
                                    .track_scroll(&scroll)
                                    .flex()
                                    .flex_col()
                                    .py(px(PAD))
                                    .children(rows)
                            }))
                            .children((count > 0).then(|| div().h(px(1.)).bg(menu_border)))
                            .child(clipboard)
                            .child(div().h(px(1.)).bg(menu_border))
                            .child(div().flex().flex_col().py(px(4.)).child(turn_row))
                            .children(thumb),
                    ),
                ),
        )
        .into_any_element()
    })
}

/// The table right-click menu (Word-style row/column editing), anchored at
/// the click; each row runs its action on the caret's table cell.
pub(crate) fn table_menu(
    editor: &EditorState,
    cx: &mut Context<EditorState>,
) -> Option<gpui_kit::AnyElement> {
    editor.overlay_menu.table().map(|(anchor, menu_scroll)| {
        // Menu chrome from the host's theme (fallbacks match the former
        // hardcoded dark menu when no markdown style is set).
        let st = editor.markdown_style.as_ref();
        let menu_bg = st.map_or(rgb(0x26262b).into(), |s| s.popover_bg);
        let menu_border = st.map_or(rgb(0x45454c).into(), |s| s.popover_border);
        let menu_fg = st.map_or(rgb(0xe6e6e6).into(), |s| s.popover_fg);
        let hover = st.map_or(rgba(0x2f6fd628).into(), |s| s.popover_hover);
        let divider = st.map_or(rgba(0xffffff2e).into(), |s| s.popover_divider);
        let mut thumb_c = st.map_or(rgba(0xffffff66).into(), |s| s.marker);
        thumb_c.a = 0.5;
        const ROW_H: f32 = 24.0;
        const DIV_H: f32 = 9.0;
        const PAD: f32 = 4.0;
        const MAX_H: f32 = 480.0;
        // Cditor-style grouped rows: a glyph column, a checkmark on the
        // current align/style, and the destructive group in red.
        let danger = st.map_or(rgb(0xE5484D).into(), |s| s.popover_danger);
        let cur_align = editor.caret_table_align();
        let cur_style = editor
            .caret_table_region()
            .map(|r| r.style)
            .unwrap_or_default();
        use markdown_syntax::TableStyle as TS;
        enum Row {
            Div,
            Item {
                glyph: &'static str,
                label: SharedString,
                action: TableMenuAction,
                red: bool,
                checked: bool,
            },
        }
        let item = |glyph, label, action| Row::Item {
            glyph,
            label,
            action,
            red: false,
            checked: false,
        };
        let specs = [
            item(
                "↑",
                editor.labels.insert_row_above.clone(),
                TableMenuAction::InsertRowAbove,
            ),
            item(
                "↓",
                editor.labels.insert_row_below.clone(),
                TableMenuAction::InsertRowBelow,
            ),
            item(
                "⧉",
                editor.labels.duplicate_row.clone(),
                TableMenuAction::DuplicateRow,
            ),
            Row::Div,
            item(
                "←",
                editor.labels.insert_column_left.clone(),
                TableMenuAction::InsertColLeft,
            ),
            item(
                "→",
                editor.labels.insert_column_right.clone(),
                TableMenuAction::InsertColRight,
            ),
            Row::Div,
            Row::Item {
                glyph: "",
                label: editor.labels.align_left.clone(),
                action: TableMenuAction::AlignLeft,
                red: false,
                checked: cur_align == Some(CellAlign::Left),
            },
            Row::Item {
                glyph: "",
                label: editor.labels.align_center.clone(),
                action: TableMenuAction::AlignCenter,
                red: false,
                checked: cur_align == Some(CellAlign::Center),
            },
            Row::Item {
                glyph: "",
                label: editor.labels.align_right.clone(),
                action: TableMenuAction::AlignRight,
                red: false,
                checked: cur_align == Some(CellAlign::Right),
            },
            Row::Div,
            Row::Item {
                glyph: "▦",
                label: editor.labels.grid_style.clone(),
                action: TableMenuAction::SetStyle(None),
                red: false,
                checked: cur_style == TS::Grid,
            },
            Row::Item {
                glyph: "▤",
                label: editor.labels.striped_style.clone(),
                action: TableMenuAction::SetStyle(Some("striped")),
                red: false,
                checked: cur_style == TS::Striped,
            },
            Row::Item {
                glyph: "▥",
                label: editor.labels.header_style.clone(),
                action: TableMenuAction::SetStyle(Some("header")),
                red: false,
                checked: cur_style == TS::Header,
            },
            Row::Item {
                glyph: "─",
                label: editor.labels.minimal_style.clone(),
                action: TableMenuAction::SetStyle(Some("minimal")),
                red: false,
                checked: cur_style == TS::Minimal,
            },
            Row::Div,
            item(
                "⊞",
                editor.labels.copy_as_markdown.clone(),
                TableMenuAction::CopyTable,
            ),
            Row::Div,
            Row::Item {
                glyph: "✕",
                label: editor.labels.delete_row.clone(),
                action: TableMenuAction::DeleteRow,
                red: true,
                checked: false,
            },
            Row::Item {
                glyph: "✕",
                label: editor.labels.delete_column.clone(),
                action: TableMenuAction::DeleteColumn,
                red: true,
                checked: false,
            },
            Row::Item {
                glyph: "✕",
                label: editor.labels.delete_table.clone(),
                action: TableMenuAction::DeleteTable,
                red: true,
                checked: false,
            },
        ];
        let n_items = specs
            .iter()
            .filter(|r| matches!(r, Row::Item { .. }))
            .count();
        let n_divs = specs.len() - n_items;
        let mut rows: Vec<gpui_kit::AnyElement> = Vec::new();
        for (i, spec) in specs.into_iter().enumerate() {
            match spec {
                Row::Div => rows.push(
                    div()
                        .flex_shrink_0()
                        .h(px(1.))
                        .my(px(4.))
                        .mx(px(8.))
                        .bg(divider)
                        .into_any_element(),
                ),
                Row::Item {
                    glyph,
                    label,
                    action,
                    red,
                    checked,
                } => {
                    let fg = if red { danger } else { menu_fg };
                    let mut glyph_c = fg;
                    glyph_c.a *= 0.7;
                    rows.push(
                        div()
                            .id(("table-menu-row", i))
                            .flex_shrink_0()
                            .px(px(10.))
                            .py(px(3.))
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(6.))
                            .text_color(fg)
                            .hover(move |s| s.bg(hover))
                            .child(
                                div()
                                    .w(px(16.))
                                    .flex_none()
                                    .text_color(glyph_c)
                                    .child(glyph),
                            )
                            .child(div().flex_1().child(label))
                            .children(checked.then(|| div().text_color(glyph_c).child("✓")))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |editor, _: &MouseDownEvent, _, cx| {
                                    cx.stop_propagation();
                                    action.apply(editor, cx);
                                }),
                            )
                            .into_any_element(),
                    );
                }
            }
        }
        // Scrollbar thumb, shown when the items overflow the cap — sized from
        // the content height + positioned from the live scroll offset.
        let rows_h = n_items as f32 * ROW_H + n_divs as f32 * DIV_H;
        let view_h = MAX_H - 2.0 * PAD;
        let thumb = (rows_h > view_h).then(|| {
            let scrolled = (-f32::from(menu_scroll.offset().y)).clamp(0.0, rows_h - view_h);
            let thumb_h = (view_h * view_h / rows_h).max(24.0);
            let thumb_top = PAD + scrolled / (rows_h - view_h) * (view_h - thumb_h);
            div()
                .absolute()
                .top(px(thumb_top))
                .right(px(2.))
                .w(px(6.))
                .h(px(thumb_h))
                .rounded(px(3.))
                .bg(thumb_c)
        });
        gpui_kit::deferred(
            gpui_kit::anchored()
                .position(anchor)
                .snap_to_window()
                .child(
                    div()
                        .relative()
                        .occlude()
                        .min_w(px(190.))
                        .cursor(CursorStyle::Arrow)
                        .bg(menu_bg)
                        .border_1()
                        .border_color(menu_border)
                        .rounded(px(6.))
                        .shadow_md()
                        .overflow_hidden()
                        .text_color(menu_fg)
                        .text_size(px(13.))
                        .on_mouse_down_out(cx.listener(|editor, _: &MouseDownEvent, _, cx| {
                            editor.overlay_menu.dismiss();
                            cx.notify();
                        }))
                        .child(
                            // Inner scroll viewport: caps the height + scrolls the
                            // overflow (max_h on a separate flex-col div, like the
                            // suggestion menu — combining it with the styled box
                            // above doesn't cap).
                            div()
                                .id("table-menu")
                                .max_h(px(MAX_H))
                                .overflow_y_scroll()
                                .track_scroll(menu_scroll)
                                .flex()
                                .flex_col()
                                .py(px(PAD))
                                .children(rows),
                        )
                        .children(thumb),
                ),
        )
        .into_any_element()
    })
}

/// The property right-click menu: edit / delete the `key:: value` block.
pub(crate) fn property_menu(
    editor: &EditorState,
    cx: &mut Context<EditorState>,
) -> Option<gpui_kit::AnyElement> {
    editor.overlay_menu.property().map(|(row, anchor)| {
        let st = editor.markdown_style.as_ref();
        let menu_bg = st.map_or(rgb(0x26262b).into(), |s| s.popover_bg);
        let menu_border = st.map_or(rgb(0x45454c).into(), |s| s.popover_border);
        let menu_fg = st.map_or(rgb(0xe6e6e6).into(), |s| s.popover_fg);
        let hover = st.map_or(rgba(0x2f6fd628).into(), |s| s.popover_hover);
        let item = |id: &'static str, label: SharedString| {
            div()
                .id(id)
                .px(px(10.))
                .py(px(3.))
                .hover(move |s| s.bg(hover))
                .child(label)
        };
        gpui_kit::deferred(
            gpui_kit::anchored()
                .position(anchor)
                .snap_to_window()
                .child(
                    div()
                        .occlude()
                        .min_w(px(160.))
                        .cursor(CursorStyle::Arrow)
                        .bg(menu_bg)
                        .border_1()
                        .border_color(menu_border)
                        .rounded(px(6.))
                        .shadow_md()
                        .overflow_hidden()
                        .text_color(menu_fg)
                        .text_size(px(13.))
                        .py(px(4.))
                        .on_mouse_down_out(cx.listener(|editor, _: &MouseDownEvent, _, cx| {
                            editor.overlay_menu.dismiss();
                            cx.notify();
                        }))
                        .child(
                            item("prop-menu-edit", editor.labels.edit_properties.clone())
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |editor, _: &MouseDownEvent, _, cx| {
                                        cx.stop_propagation();
                                        editor.overlay_menu.dismiss();
                                        if let Some((range, source)) = editor.property_block_at(row)
                                        {
                                            let block_row = row - editor.row_col(range.start).0;
                                            cx.emit(EditorEvent::EditProperties {
                                                range,
                                                source,
                                                at_end: false,
                                                row: Some(block_row),
                                            });
                                        }
                                    }),
                                ),
                        )
                        .child(
                            item("prop-menu-delete", editor.labels.delete_property.clone())
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |editor, _: &MouseDownEvent, _, cx| {
                                        cx.stop_propagation();
                                        editor.overlay_menu.dismiss();
                                        editor.delete_property_row(row, cx);
                                    }),
                                ),
                        ),
                ),
        )
        .into_any_element()
    })
}

/// The image right-click menu: Word-style object actions on an inline image
/// (Delete), anchored at the click. Chrome matches the table menu.
pub(crate) fn image_menu(
    editor: &EditorState,
    cx: &mut Context<EditorState>,
) -> Option<gpui_kit::AnyElement> {
    editor.overlay_menu.image().map(|(line, anchor)| {
        let st = editor.markdown_style.as_ref();
        let menu_bg = st.map_or(rgb(0x26262b).into(), |s| s.popover_bg);
        let menu_border = st.map_or(rgb(0x45454c).into(), |s| s.popover_border);
        let menu_fg = st.map_or(rgb(0xe6e6e6).into(), |s| s.popover_fg);
        let hover = st.map_or(rgba(0x2f6fd628).into(), |s| s.popover_hover);
        gpui_kit::deferred(
            gpui_kit::anchored()
                .position(anchor)
                .snap_to_window()
                .child(
                    div()
                        .occlude()
                        .min_w(px(140.))
                        .cursor(CursorStyle::Arrow)
                        .bg(menu_bg)
                        .border_1()
                        .border_color(menu_border)
                        .rounded(px(6.))
                        .shadow_md()
                        .overflow_hidden()
                        .text_color(menu_fg)
                        .text_size(px(13.))
                        .py(px(4.))
                        .on_mouse_down_out(cx.listener(|editor, _: &MouseDownEvent, _, cx| {
                            editor.overlay_menu.dismiss();
                            cx.notify();
                        }))
                        .child(
                            div()
                                .id("image-menu-delete")
                                .px(px(10.))
                                .py(px(3.))
                                .hover(move |s| s.bg(hover))
                                .child(editor.labels.delete_image.clone())
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |editor, _: &MouseDownEvent, _, cx| {
                                        cx.stop_propagation();
                                        editor.overlay_menu.dismiss();
                                        editor.delete_image_row(line, cx);
                                    }),
                                ),
                        ),
                ),
        )
        .into_any_element()
    })
}

/// The code block's language picker (Cditor-inspired): the host's
/// highlighter languages, scrollable past the cap, current one checked.
/// Selecting rewrites the opening fence (one undo step).
pub(crate) fn code_lang_menu(
    editor: &EditorState,
    cx: &mut Context<EditorState>,
) -> Option<gpui_kit::AnyElement> {
    editor
        .overlay_menu
        .code_lang()
        .map(|(row, anchor, menu_scroll)| {
            // The code block's language picker (Cditor-inspired): the host's
            // highlighter languages, scrollable past the cap, current one
            // checked. Selecting rewrites the opening fence (one undo step).
            let st = editor.markdown_style.as_ref();
            let menu_bg = st.map_or(rgb(0x26262b).into(), |s| s.popover_bg);
            let menu_border = st.map_or(rgb(0x45454c).into(), |s| s.popover_border);
            let menu_fg = st.map_or(rgb(0xe6e6e6).into(), |s| s.popover_fg);
            let hover = st.map_or(rgba(0x2f6fd628).into(), |s| s.popover_hover);
            let mut thumb_c = st.map_or(rgba(0xffffff66).into(), |s| s.marker);
            thumb_c.a = 0.5;
            let current = editor
                .code_block_at(row)
                .map(|(l, _)| l)
                .unwrap_or_default();
            const ROW_H: f32 = 22.0;
            const MAX_H: f32 = 260.0;
            const PAD: f32 = 4.0;
            let langs = editor.code_langs.clone();
            let rows_h = langs.len() as f32 * ROW_H;
            let view_h = MAX_H - 2.0 * PAD;
            let thumb = (rows_h > view_h).then(|| {
                let scrolled = (-f32::from(menu_scroll.offset().y)).clamp(0.0, rows_h - view_h);
                let thumb_h = (view_h * view_h / rows_h).max(24.0);
                let thumb_top = PAD + scrolled / (rows_h - view_h) * (view_h - thumb_h);
                div()
                    .absolute()
                    .top(px(thumb_top))
                    .right(px(2.))
                    .w(px(6.))
                    .h(px(thumb_h))
                    .rounded(px(3.))
                    .bg(thumb_c)
            });
            gpui_kit::deferred(
                gpui_kit::anchored()
                    .position(anchor)
                    .snap_to_window()
                    .child(
                        div()
                            .relative()
                            .occlude()
                            .min_w(px(140.))
                            .cursor(CursorStyle::Arrow)
                            .bg(menu_bg)
                            .border_1()
                            .border_color(menu_border)
                            .rounded(px(6.))
                            .shadow_md()
                            .overflow_hidden()
                            .text_color(menu_fg)
                            .text_size(px(13.))
                            .py(px(PAD))
                            .on_mouse_down_out(cx.listener(|editor, _: &MouseDownEvent, _, cx| {
                                editor.overlay_menu.dismiss();
                                cx.notify();
                            }))
                            .child(
                                div()
                                    .id("code-lang-list")
                                    .max_h(px(MAX_H - 2.0 * PAD))
                                    .overflow_y_scroll()
                                    .track_scroll(menu_scroll)
                                    .children(langs.into_iter().enumerate().map(|(i, lang)| {
                                        let is_current = *lang == current
                                            || (current.is_empty() && *lang == *"text");
                                        let label: SharedString = if is_current {
                                            format!("{lang} ✓").into()
                                        } else {
                                            lang.clone()
                                        };
                                        div()
                                            .id(("code-lang-row", i))
                                            .flex_shrink_0()
                                            .h(px(ROW_H))
                                            .px(px(10.))
                                            .py(px(2.))
                                            .hover(move |s| s.bg(hover))
                                            .child(label)
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(
                                                    move |editor, _: &MouseDownEvent, _, cx| {
                                                        cx.stop_propagation();
                                                        editor.overlay_menu.dismiss();
                                                        editor.set_code_lang(row, &lang, cx);
                                                    },
                                                ),
                                            )
                                            .into_any_element()
                                    })),
                            )
                            .children(thumb),
                    ),
            )
            .into_any_element()
        })
}
