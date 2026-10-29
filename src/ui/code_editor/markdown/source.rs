use super::*;
use gpui_kit::base::text::editor_host::{
    BlockNode, FlowPosition, InlineNode, LeafGeometry, Paragraph, RenderContext,
    collect_leaf_geometry,
};
use gpui_kit::component::{ActiveTheme as _, input as actions};
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::collections::HashMap;

/// A native editing presentation. It never owns a second text buffer or history.
pub(crate) struct SourcePresentation {
    document: Entity<MarkdownDocument>,
    state: PresentationState,
    focus: FocusHandle,
    scroll: ScrollHandle,
    geometry: Option<(u64, LeafGeometry)>,
    dragging: bool,
    reveal_caret: bool,
    wrap: bool,
    projection: Option<(u64, BlockNode)>,
    _observe: Subscription,
}
impl SourcePresentation {
    pub(crate) fn new(
        document: Entity<MarkdownDocument>,
        wrap: bool,
        cx: &mut Context<Self>,
    ) -> Self {
        let observe = cx.subscribe(&document, |this, _, event: &DocumentEvent, cx| {
            if let DocumentEvent::Edited(transaction) = event {
                this.state.rebase(transaction);
                let doc = this.document.read(cx);
                if transaction.revision() == doc.revision() {
                    this.state.selection = Selection::new(
                        doc.boundary(this.state.selection.anchor, false),
                        doc.boundary(this.state.selection.head, false),
                    );
                }
                this.geometry = None;
                cx.notify();
            }
        });
        Self {
            document,
            state: PresentationState::default(),
            focus: cx.focus_handle().tab_stop(true),
            scroll: ScrollHandle::new(),
            geometry: None,
            dragging: false,
            reveal_caret: false,
            wrap,
            projection: None,
            _observe: observe,
        }
    }
    pub(crate) fn document(&self) -> &Entity<MarkdownDocument> {
        &self.document
    }
    pub(crate) fn finish_active_composition(
        document: &Entity<MarkdownDocument>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let owner = document
            .read(cx)
            .composition_owner
            .as_ref()
            .and_then(WeakEntity::upgrade);
        if let Some(owner) = owner {
            owner.update(cx, |view, cx| {
                view.command(Command::FinishComposition, window, cx)
            });
        }
    }
    pub(crate) fn select_range(&mut self, range: Range<usize>, cx: &mut Context<Self>) {
        self.document.update(cx, |document, cx| {
            document.command(
                &mut self.state,
                Command::Select(Selection::new(range.start, range.end)),
            );
            cx.emit(DocumentEvent::PresentationChanged);
        });
        cx.notify();
    }
    pub(crate) fn selection(&self) -> Selection {
        self.state.selection()
    }
    pub(crate) fn is_composing(&self) -> bool {
        self.state.composition.is_some()
    }
    pub(crate) fn is_soft_wrap(&self) -> bool {
        self.wrap
    }
    pub(crate) fn set_soft_wrap(&mut self, wrap: bool, cx: &mut Context<Self>) {
        self.wrap = wrap;
        self.geometry = None;
        cx.notify();
    }
    pub(crate) fn scroll_offset(&self) -> Point<Pixels> {
        self.scroll.offset()
    }
    pub(crate) fn set_scroll_offset(&mut self, offset: Point<Pixels>, cx: &mut Context<Self>) {
        self.scroll.set_offset(offset);
        self.geometry = None;
        self.reveal_caret = false;
        cx.notify();
    }
    pub(crate) fn command(
        &mut self,
        command: Command,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let owner = self
            .document
            .read(cx)
            .composition_owner
            .as_ref()
            .and_then(WeakEntity::upgrade);
        if let Some(owner) = owner.filter(|owner| owner.entity_id() != cx.entity().entity_id()) {
            owner.update(cx, |view, cx| {
                view.command(Command::FinishComposition, window, cx)
            });
        }
        let own_view = cx.entity().downgrade();
        let changed = self.document.update(cx, |document, cx| {
            let revision = document.revision();
            let transaction = document.command(&mut self.state, command);
            document.composition_owner = self.state.composition.as_ref().map(|_| own_view);
            let changed = document.revision() != revision;
            if changed {
                cx.emit(DocumentEvent::Edited(transaction));
                cx.notify();
            } else {
                cx.emit(DocumentEvent::PresentationChanged);
            }
            changed
        });
        if changed {
            self.geometry = None;
            window.notify_text_input_changed(&self.focus);
        } else {
            window.notify_text_selection_changed(&self.focus);
        }
        self.reveal_caret = true;
        cx.notify();
    }
    fn caret_bounds(&self, byte: usize, cx: &App) -> Option<Bounds<Pixels>> {
        let (revision, leaf) = self.geometry.as_ref()?;
        if *revision != self.document.read(cx).revision() {
            return None;
        }
        Some(Bounds::new(
            leaf.position_for_index(byte),
            size(px(1.), leaf.caret_height()),
        ))
    }
    pub(crate) fn range_to_bounds(&self, range: &Range<usize>, cx: &App) -> Option<Bounds<Pixels>> {
        self.caret_bounds(range.end, cx)
    }
    fn point_offset(&self, point: Point<Pixels>, cx: &App) -> Option<usize> {
        let (revision, leaf) = self.geometry.as_ref()?;
        let doc = self.document.read(cx);
        if *revision != doc.revision() {
            return None;
        }
        Some(doc.boundary(leaf.index_for_position(point), false))
    }
    fn select_point(
        &mut self,
        point: Point<Pixels>,
        extend: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(at) = self.point_offset(point, cx) else {
            return;
        };
        let anchor = if extend {
            self.state.selection.anchor
        } else {
            at
        };
        self.command(Command::Select(Selection::new(anchor, at)), window, cx);
    }
    fn move_cursor(
        &mut self,
        horizontal: i32,
        vertical: i32,
        extend: bool,
        edge: Option<bool>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let doc = self.document.read(cx);
        let current = doc.boundary(self.state.selection.head, false);
        let at = if let Some(end) = edge {
            let row = doc.source().offset_to_point(current).row;
            if end {
                doc.source().line_end_offset(row)
            } else {
                doc.source().line_start_offset(row)
            }
        } else if vertical != 0 {
            self.geometry
                .as_ref()
                .map(|(_, leaf)| {
                    let point = leaf.position_for_index(current);
                    leaf.index_for_position(
                        point + gpui_kit::point(px(0.), leaf.line_height() * vertical as f32),
                    )
                })
                .unwrap_or(current)
        } else if !extend && !self.state.selection.range().is_empty() {
            if horizontal < 0 {
                self.state.selection.range().start
            } else {
                self.state.selection.range().end
            }
        } else if horizontal < 0 {
            doc.previous(current)
        } else {
            doc.next(current)
        };
        let at = doc.boundary(at, false);
        let anchor = if extend {
            self.state.selection.anchor
        } else {
            at
        };
        self.command(Command::Select(Selection::new(anchor, at)), window, cx);
    }
    fn copy(&mut self, cut: bool, window: &mut Window, cx: &mut Context<Self>) {
        let range = self.state.selection.range();
        if range.is_empty() {
            return;
        }
        let text = self
            .document
            .read(cx)
            .source()
            .slice(range.clone())
            .to_string();
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        if cut {
            self.command(
                Command::Replace {
                    range,
                    text: String::new(),
                    kind: EditKind::Command,
                },
                window,
                cx,
            );
        }
    }
    fn paste_clipboard(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            // New clipboard line breaks follow the current codec's LF insertion
            // convention; this does not rewrite any existing document region.
            let text = if text.contains("\r\n") {
                text.replace("\r\n", "\n")
            } else {
                text
            };
            self.command(
                Command::Replace {
                    range: self.state.selection.range(),
                    text,
                    kind: EditKind::Paste,
                },
                window,
                cx,
            );
        }
    }
}
impl Focusable for SourcePresentation {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
impl EntityInputHandler for SourcePresentation {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        adjusted: &mut Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<String> {
        let doc = self.document.read(cx);
        let range = doc.utf16_range(range);
        *adjusted = Some(doc.utf16_offset(range.start)..doc.utf16_offset(range.end));
        Some(doc.source().slice(range).to_string())
    }
    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let doc = self.document.read(cx);
        let selected = self.state.selection;
        let range = selected.range();
        Some(UTF16Selection {
            range: doc.utf16_offset(range.start)..doc.utf16_offset(range.end),
            reversed: selected.is_reversed(),
        })
    }
    fn marked_text_range(&self, _: &mut Window, cx: &mut Context<Self>) -> Option<Range<usize>> {
        let doc = self.document.read(cx);
        let range = self.state.marked_range()?;
        Some(doc.utf16_offset(range.start)..doc.utf16_offset(range.end))
    }
    fn unmark_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.command(Command::FinishComposition, window, cx);
    }
    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(range) = range {
            let range = self.document.read(cx).utf16_range(range);
            if self.state.marked_range().as_ref() != Some(&range) {
                self.command(Command::FinishComposition, window, cx);
                self.command(
                    Command::Replace {
                        range,
                        text: text.into(),
                        kind: EditKind::Typing,
                    },
                    window,
                    cx,
                );
                return;
            }
        }
        self.command(Command::Commit(text.into()), window, cx);
    }
    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        selected: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range.map(|range| self.document.read(cx).utf16_range(range));
        self.command(
            Command::Compose {
                range,
                text: text.into(),
                selected,
            },
            window,
            cx,
        );
    }
    fn bounds_for_range(
        &mut self,
        range: Range<usize>,
        _: Bounds<Pixels>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let range = self.document.read(cx).utf16_range(range);
        self.caret_bounds(range.start, cx)
    }
    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<usize> {
        Some(
            self.document
                .read(cx)
                .utf16_offset(self.point_offset(point, cx)?),
        )
    }
    fn set_selected_text_range(
        &mut self,
        range: Range<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = self.document.read(cx).utf16_range(range);
        self.command(
            Command::Select(Selection::new(range.start, range.end)),
            window,
            cx,
        );
    }
    fn text_length_utf16(&mut self, _: &mut Window, cx: &mut Context<Self>) -> Option<usize> {
        let doc = self.document.read(cx);
        Some(doc.utf16_offset(doc.source().len()))
    }
}
impl Render for SourcePresentation {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let revision = self.document.read(cx).revision();
        if self
            .projection
            .as_ref()
            .is_none_or(|(at, _)| *at != revision)
        {
            let mut paragraph = Paragraph::default().with_leaf_key(1);
            paragraph.push(InlineNode::new(self.document.read(cx).source().to_string()));
            self.projection = Some((revision, BlockNode::Paragraph(paragraph)));
        }
        let node = &self.projection.as_ref().unwrap().1;
        let context = RenderContext::from_app(cx);
        let block = node.render_with_id(
            "markdown-source-text",
            FlowPosition::at(&[node], 0, true),
            &context,
            window,
            cx,
        );
        let root =
            div()
                .id("markdown-source")
                .size_full()
                .min_h_0()
                .key_context("Input")
                .track_focus(&self.focus)
                .cursor_text()
                .bg(cx.theme().background)
                .text_color(cx.theme().foreground)
                .font_family(cx.theme().mono_font_family.clone())
                .when(!self.wrap, |root| root.whitespace_nowrap())
                .role(Role::TextInput)
                .aria_label("Markdown source")
                .when(window.is_a11y_active() || cfg!(test), |root| {
                    root.aria_value(self.document.read(cx).source().to_string())
                })
                .on_a11y_action(AccessibleAction::Focus, {
                    let view = cx.entity();
                    move |_, window, cx| {
                        view.read(cx).focus.clone().focus(window, cx);
                    }
                })
                .on_a11y_action(AccessibleAction::SetValue, {
                    let view = cx.entity();
                    move |data, window, cx| {
                        if let Some(accesskit::ActionData::Value(text)) = data {
                            view.update(cx, |view, cx| {
                                let end = view.document.read(cx).source().len();
                                view.command(
                                    Command::Replace {
                                        range: 0..end,
                                        text: text.to_string(),
                                        kind: EditKind::Command,
                                    },
                                    window,
                                    cx,
                                );
                            });
                        }
                    }
                })
                .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                    if event.keystroke.key == "escape" && this.state.composition.is_some() {
                        this.command(Command::CancelComposition, window, cx);
                        cx.stop_propagation();
                    }
                }))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, event: &MouseDownEvent, window, cx| {
                        this.focus.focus(window, cx);
                        this.dragging = true;
                        this.select_point(event.position, event.modifiers.shift, window, cx);
                        cx.stop_propagation();
                    }),
                )
                .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, window, cx| {
                    if this.dragging {
                        this.select_point(event.position, true, window, cx);
                    }
                }))
                .on_mouse_up(
                    MouseButton::Left,
                    cx.listener(|this, _, _, _| this.dragging = false),
                )
                .on_mouse_up_out(
                    MouseButton::Left,
                    cx.listener(|this, _, _, _| this.dragging = false),
                )
                .on_action(
                    cx.listener(|this, _: &actions::Copy, window, cx| this.copy(false, window, cx)),
                )
                .on_action(
                    cx.listener(|this, _: &actions::Cut, window, cx| this.copy(true, window, cx)),
                )
                .on_action(cx.listener(|this, _: &actions::Paste, window, cx| {
                    this.paste_clipboard(window, cx)
                }))
                .on_action(cx.listener(|this, _: &actions::Undo, window, cx| {
                    this.command(Command::Undo, window, cx)
                }))
                .on_action(cx.listener(|this, _: &actions::Redo, window, cx| {
                    this.command(Command::Redo, window, cx)
                }))
                .on_action(cx.listener(|this, _: &actions::Backspace, window, cx| {
                    this.command(Command::Backspace, window, cx)
                }))
                .on_action(cx.listener(|this, _: &actions::Delete, window, cx| {
                    this.command(Command::Delete, window, cx)
                }))
                .on_action(cx.listener(|this, _: &actions::Enter, window, cx| {
                    this.command(Command::Insert("\n".into()), window, cx)
                }))
                .on_action(cx.listener(|this, _: &actions::Indent, window, cx| {
                    this.command(Command::Insert("\t".into()), window, cx)
                }))
                .on_action(cx.listener(|this, _: &actions::SelectAll, window, cx| {
                    let end = this.document.read(cx).source().len();
                    this.command(Command::Select(Selection::new(0, end)), window, cx);
                }))
                .on_action(cx.listener(|this, _: &actions::MoveLeft, window, cx| {
                    this.move_cursor(-1, 0, false, None, window, cx)
                }))
                .on_action(cx.listener(|this, _: &actions::MoveRight, window, cx| {
                    this.move_cursor(1, 0, false, None, window, cx)
                }))
                .on_action(cx.listener(|this, _: &actions::MoveUp, window, cx| {
                    this.move_cursor(0, -1, false, None, window, cx)
                }))
                .on_action(cx.listener(|this, _: &actions::MoveDown, window, cx| {
                    this.move_cursor(0, 1, false, None, window, cx)
                }))
                .on_action(cx.listener(
                    |this, _: &gpui_kit::base::actions::SelectLeft, window, cx| {
                        this.move_cursor(-1, 0, true, None, window, cx)
                    },
                ))
                .on_action(cx.listener(
                    |this, _: &gpui_kit::base::actions::SelectRight, window, cx| {
                        this.move_cursor(1, 0, true, None, window, cx)
                    },
                ))
                .on_action(cx.listener(
                    |this, _: &gpui_kit::base::actions::SelectUp, window, cx| {
                        this.move_cursor(0, -1, true, None, window, cx)
                    },
                ))
                .on_action(cx.listener(
                    |this, _: &gpui_kit::base::actions::SelectDown, window, cx| {
                        this.move_cursor(0, 1, true, None, window, cx)
                    },
                ))
                .on_action(
                    cx.listener(|this, _: &actions::MoveToStartOfLine, window, cx| {
                        this.move_cursor(0, 0, false, Some(false), window, cx)
                    }),
                )
                .on_action(
                    cx.listener(|this, _: &actions::MoveToEndOfLine, window, cx| {
                        this.move_cursor(0, 0, false, Some(true), window, cx)
                    }),
                )
                .on_action(cx.listener(|this, _: &actions::MoveHome, window, cx| {
                    this.move_cursor(0, 0, false, Some(false), window, cx)
                }))
                .on_action(cx.listener(|this, _: &actions::MoveEnd, window, cx| {
                    this.move_cursor(0, 0, false, Some(true), window, cx)
                }))
                .on_action(
                    cx.listener(|this, _: &actions::SelectToStartOfLine, window, cx| {
                        this.move_cursor(0, 0, true, Some(false), window, cx)
                    }),
                )
                .on_action(
                    cx.listener(|this, _: &actions::SelectToEndOfLine, window, cx| {
                        this.move_cursor(0, 0, true, Some(true), window, cx)
                    }),
                )
                .on_action(cx.listener(|this, _: &actions::MoveToStart, window, cx| {
                    this.command(Command::Select(Selection::caret(0)), window, cx)
                }))
                .on_action(cx.listener(|this, _: &actions::MoveToEnd, window, cx| {
                    let end = this.document.read(cx).source().len();
                    this.command(Command::Select(Selection::caret(end)), window, cx);
                }))
                .on_action(cx.listener(|this, _: &actions::SelectToStart, window, cx| {
                    this.command(
                        Command::Select(Selection::new(this.state.selection.anchor, 0)),
                        window,
                        cx,
                    )
                }))
                .on_action(cx.listener(|this, _: &actions::SelectToEnd, window, cx| {
                    let end = this.document.read(cx).source().len();
                    this.command(
                        Command::Select(Selection::new(this.state.selection.anchor, end)),
                        window,
                        cx,
                    );
                }))
                .on_action(cx.listener(|this, _: &actions::Escape, window, cx| {
                    if this.state.composition.is_some() {
                        this.command(Command::CancelComposition, window, cx);
                    } else {
                        cx.propagate();
                    }
                }))
                .child(crate::ui::scrollbar::with_vertical_scrollbar(
                    "markdown-source-scrollbar",
                    div()
                        .id("markdown-source-scroll")
                        .size_full()
                        .overflow_y_scroll()
                        .overflow_x_scroll()
                        .track_scroll(&self.scroll)
                        .child(
                            div()
                                .p_3()
                                .capture_any_mouse_down(cx.listener(
                                    |this, event: &MouseDownEvent, window, cx| {
                                        if event.button != MouseButton::Left {
                                            return;
                                        }
                                        this.focus.focus(window, cx);
                                        this.dragging = true;
                                        this.select_point(
                                            event.position,
                                            event.modifiers.shift,
                                            window,
                                            cx,
                                        );
                                        cx.stop_propagation();
                                    },
                                ))
                                .child(block),
                        ),
                    &self.scroll,
                ));
        SourceElement {
            child: root.into_any_element(),
            view: cx.entity(),
            revision,
            selection: self.state.selection.range(),
            color: cx.theme().selection,
            caret: cx.theme().caret,
        }
    }
}
struct SourceElement {
    child: AnyElement,
    view: Entity<SourcePresentation>,
    revision: u64,
    selection: Range<usize>,
    color: Hsla,
    caret: Hsla,
}
impl IntoElement for SourceElement {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
impl Element for SourceElement {
    type RequestLayoutState = ();
    type PrepaintState = ();
    fn id(&self) -> Option<ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }
    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        (self.child.request_layout(window, cx), ())
    }
    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let mut decorations = HashMap::new();
        decorations.insert(1, vec![(self.selection.clone(), self.color)]);
        let (_, mut geometry) =
            collect_leaf_geometry(decorations, || self.child.prepaint(window, cx));
        self.view.update(cx, |view, cx| {
            view.geometry = geometry.remove(&1).map(|leaf| (self.revision, leaf));
            if view.reveal_caret && view.focus.is_focused(window) {
                view.reveal_caret = false;
                if let Some(caret) = view.caret_bounds(view.state.selection.head, cx) {
                    let inset = window.rem_size() * 0.75;
                    let delta = if caret.top() < bounds.top() + inset {
                        bounds.top() + inset - caret.top()
                    } else if caret.bottom() > bounds.bottom() - inset {
                        bounds.bottom() - inset - caret.bottom()
                    } else {
                        px(0.)
                    };
                    let delta_x = if caret.left() < bounds.left() + inset {
                        bounds.left() + inset - caret.left()
                    } else if caret.right() > bounds.right() - inset {
                        bounds.right() - inset - caret.right()
                    } else {
                        px(0.)
                    };
                    if delta != px(0.) || delta_x != px(0.) {
                        let offset = view.scroll.offset();
                        view.scroll.set_offset(point(
                            (offset.x + delta_x).min(px(0.)),
                            (offset.y + delta).min(px(0.)),
                        ));
                        view.geometry = None;
                        window.refresh();
                    }
                }
            }
        });
    }
    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child.paint(window, cx);
        let (focus, caret) = {
            let view = self.view.read(cx);
            let caret = if view.focus.is_focused(window) && self.selection.is_empty() {
                view.caret_bounds(view.state.selection.head, cx)
            } else {
                None
            };
            (view.focus.clone(), caret)
        };
        window.handle_input(
            &focus,
            ElementInputHandler::new(bounds, self.view.clone()),
            cx,
        );
        if let Some(caret) = caret {
            if caret.top() >= bounds.top()
                && caret.bottom() <= bounds.bottom()
                && caret.left() >= bounds.left()
                && caret.right() <= bounds.right()
            {
                window.paint_quad(fill(caret, self.caret));
            }
        }
    }
}
