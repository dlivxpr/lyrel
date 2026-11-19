//! The editor shell's `Render`: the scroll container + input wiring + the
//! overlay-menu attachments. Split from `lib.rs`; the five overlay menus
//! (Diagnostics / Table / Property / Image / CodeLang) are built in
//! [`render_menus`].

use super::*;

impl Render for EditorState {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(provider) = &self.presentation_style_provider {
            let style = provider(cx);
            self.live_style = Some(style.clone());
            if self.markdown_style.is_some() {
                self.markdown_style = Some(style);
            }
        }
        let scroll = div()
            .relative()
            .id("markdown-document-scroll")
            .test_support()
            .overflow_scroll()
            .track_scroll(&self.frame.scroll)
            .size_full()
            // While a `$$` block OR an inline `$…$` formula is being edited, the hosted math
            // editor is focused but lives *inside* this element — so the editor's own
            // keybindings (arrows, typing, …) would capture keys before they reach it. Drop the
            // key context for the duration so raw keys flow to the math editor's on_key_down.
            .key_context(
                if self.editing_block.is_some() || self.editing_inline.is_some() {
                    ""
                } else {
                    CONTEXT
                },
            )
            .track_focus(&self.focus_handle)
            .cursor(CursorStyle::IBeam)
            .on_action(cx.listener(|_, _: &InsertImage, _, cx| {
                cx.emit(EditorEvent::InsertImages(Vec::new()))
            }))
            .on_drop(
                cx.listener(|this, paths: &gpui_kit::ExternalPaths, window, cx| {
                    cx.stop_propagation();
                    let position = window.mouse_position();
                    let offset = this.index_for_mouse_position(position);
                    let affinity = this.affinity_for_mouse_position(offset, position);
                    this.move_to_with_affinity(offset, affinity, cx);
                    this.focus(window, cx);
                    cx.emit(EditorEvent::InsertImages(paths.paths().to_vec()));
                }),
            )
            .on_action(
                cx.listener(|this, _: &gpui_kit::component::input::Undo, window, cx| {
                    this.undo(&Undo, window, cx)
                }),
            )
            .on_action(
                cx.listener(|this, _: &gpui_kit::component::input::Redo, window, cx| {
                    this.redo(&Redo, window, cx)
                }),
            )
            .on_action(
                cx.listener(|this, _: &gpui_kit::component::input::Copy, window, cx| {
                    this.copy(&Copy, window, cx)
                }),
            )
            .on_action(
                cx.listener(|this, _: &gpui_kit::component::input::Cut, window, cx| {
                    this.cut(&Cut, window, cx)
                }),
            )
            .on_action(
                cx.listener(|this, _: &gpui_kit::component::input::Paste, window, cx| {
                    this.paste(&Paste, window, cx)
                }),
            )
            .on_action(cx.listener(
                |this, _: &gpui_kit::component::input::SelectAll, window, cx| {
                    this.select_all(&SelectAll, window, cx)
                },
            ))
            .on_action(cx.listener(
                |this, _: &gpui_kit::component::input::MoveToStart, window, cx| {
                    this.doc_start(&DocStart, window, cx)
                },
            ))
            .on_action(cx.listener(
                |this, _: &gpui_kit::component::input::Backspace, window, cx| {
                    this.backspace(&Backspace, window, cx)
                },
            ))
            .on_action(
                cx.listener(|this, _: &gpui_kit::component::input::Delete, window, cx| {
                    this.delete(&Delete, window, cx)
                }),
            )
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::up))
            .on_action(cx.listener(Self::down))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_up))
            .on_action(cx.listener(Self::select_down))
            .on_action(cx.listener(Self::word_left))
            .on_action(cx.listener(Self::word_right))
            .on_action(cx.listener(Self::select_word_left))
            .on_action(cx.listener(Self::select_word_right))
            .on_action(cx.listener(Self::delete_word_left))
            .on_action(cx.listener(Self::delete_word_right))
            .on_action(cx.listener(Self::select_home))
            .on_action(cx.listener(Self::select_end))
            .on_action(cx.listener(Self::doc_start))
            .on_action(cx.listener(Self::doc_end))
            .on_action(cx.listener(Self::select_doc_start))
            .on_action(cx.listener(Self::select_doc_end))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::newline))
            .on_action(cx.listener(Self::bold))
            .on_action(cx.listener(Self::italic))
            .on_action(cx.listener(Self::underline))
            .on_action(cx.listener(Self::strike))
            .on_action(cx.listener(Self::highlight))
            .on_action(cx.listener(Self::code))
            .on_action(cx.listener(Self::indent))
            .on_action(cx.listener(Self::outdent))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::show_character_palette))
            .on_action(cx.listener(Self::undo))
            .on_action(cx.listener(Self::redo))
            .on_action(cx.listener(Self::dismiss))
            .on_action(cx.listener(Self::insert_writing_block))
            .on_action(cx.listener(Self::text_color_picker))
            .on_action(cx.listener(Self::highlight_color_picker))
            .on_action(cx.listener(Self::toggle_heading_disclosure))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_down(MouseButton::Right, cx.listener(Self::on_right_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_scroll(cx.listener(Self::on_scroll_wheel))
            .child({
                let content = div().w_full().min_w_0();
                // With soft wrap off, unwrapped lines become the scroll
                // content's minimum width instead of being clipped.
                let content = if !self.soft_wrap && self.frame.natural_width.get() > px(0.) {
                    content.min_w(
                        self.frame.natural_width.get()
                            + window.rem_size() * (self.document_inset.0 * 2.),
                    )
                } else {
                    content
                };
                content
                    .px(gpui_kit::rems(self.document_inset.0))
                    .py(gpui_kit::rems(self.document_inset.1))
                    .child(
                        div()
                            .relative()
                            .w_full()
                            .min_w_0()
                            .child(EditorElement {
                                editor: cx.entity(),
                            })
                            .children(self.embed_overlays(window))
                            .children(self.editing_block_overlay())
                            .children(self.editing_inline_overlay()),
                    )
            })
            .children(self.writing_overlay.clone())
            .children(render_menus::diagnostics_menu(self, cx))
            .children(render_menus::table_menu(self, cx))
            .children(render_menus::property_menu(self, cx))
            .children(render_menus::image_menu(self, cx))
            .children(render_menus::code_lang_menu(self, cx));
        // Wide tables, code and unbroken lines can exceed the note width; the
        // bar is the only pointer affordance for that axis and stays visible so
        // the overflow is discoverable without touching the wheel.
        div().relative().size_full().child(scroll).child(
            div().absolute().inset_0().child(
                Scrollbar::horizontal(&self.frame.scroll)
                    .id("markdown-horizontal-scrollbar")
                    .mode(ScrollbarMode::Always),
            ),
        )
    }
}
