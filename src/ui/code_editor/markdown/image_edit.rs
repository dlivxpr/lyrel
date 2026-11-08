//! Image objects are projections of the document, never separate buffers.
use super::*;

use gpui_kit::component::{
    Disableable as _,
    dialog::{DialogAction, DialogClose, DialogFooter},
};
use gpui_kit::component::{
    Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    input::{Input, InputState},
    menu::{ContextMenuExt as _, DropdownMenu as _},
};

actions!(
    markdown_image_objects,
    [EditImage, PreviewImage, DeleteImage, FitImages]
);

#[derive(Clone)]
struct Object {
    range: Range<usize>,
    image: super::super::images::Image,
    raw: String,
    block: bool,
}
impl Object {
    fn size_start(&self) -> usize {
        self.raw
            .rfind("{width=")
            .filter(|at| {
                super::super::images::width_suffix(&self.raw[*at..])
                    .is_some_and(|(length, _)| *at + length == self.raw.len())
            })
            .unwrap_or(self.raw.len())
    }
    fn sized(&self, width: Option<u32>) -> String {
        let mut raw = self.raw[..self.size_start()].to_owned();
        if let Some(width) = width {
            raw.push_str(&format!("{{width={width}}}"));
        }
        raw
    }
    fn alt_range(&self) -> Option<Range<usize>> {
        let mut depth = 1;
        let mut escaped = false;
        for (at, ch) in self.raw[2..].char_indices() {
            if escaped {
                escaped = false;
                continue;
            }
            match ch {
                '\\' => escaped = true,
                '[' => depth += 1,
                ']' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(2..at + 2);
                    }
                }
                _ => {}
            }
        }
        None
    }
    fn edited(&self, alt: &str, url: &str, width: Option<u32>) -> Option<String> {
        let alt_range = self.alt_range()?;
        let mut raw = self.raw[..self.size_start()].to_owned();
        if url != self.image.url {
            let escaped = percent_encoding::utf8_percent_encode(url, percent_encoding::CONTROLS)
                .to_string()
                .replace('<', "%3C")
                .replace('>', "%3E");
            let after_alt = alt_range.end + 1;
            if self.raw[after_alt..].starts_with('(') {
                let start = after_alt + 1;
                let start = start + self.raw[start..].len() - self.raw[start..].trim_start().len();
                let (start, end) = if self.raw[start..].starts_with('<') {
                    let end = self.raw[start + 1..].find('>')? + start + 1;
                    (start, end + 1)
                } else {
                    let mut depth = 0;
                    let mut escape = false;
                    let mut end = start;
                    for (offset, ch) in self.raw[start..].char_indices() {
                        if escape {
                            escape = false;
                            end = start + offset + ch.len_utf8();
                            continue;
                        }
                        if ch == '\\' {
                            escape = true;
                        }
                        if !escape {
                            if ch == '(' {
                                depth += 1;
                            }
                            if ch == ')' {
                                if depth == 0 {
                                    break;
                                }
                                depth -= 1;
                            }
                            if ch.is_whitespace() && depth == 0 {
                                break;
                            }
                        }
                        end = start + offset + ch.len_utf8();
                    }
                    (start, end)
                };
                raw.replace_range(start..end, &format!("<{escaped}>"));
            } else {
                // Editing one reference occurrence never rewrites its shared definition.
                raw.replace_range(after_alt.., &format!("(<{escaped}>)"));
            }
        }
        if alt != self.image.alt {
            let after_alt = alt_range.end + 1;
            if url == self.image.url && matches!(&raw[after_alt..], "" | "[]") {
                raw.replace_range(after_alt.., &format!("[{}]", &self.raw[alt_range.clone()]));
            }
            let escaped = alt
                .replace('\\', "\\\\")
                .replace('[', "\\[")
                .replace(']', "\\]");
            raw.replace_range(alt_range, &escaped);
        }
        if width == self.image.width {
            raw.push_str(&self.raw[self.size_start()..]);
        } else if let Some(width) = width {
            raw.push_str(&format!("{{width={width}}}"));
        }
        Some(raw)
    }
}

pub(super) struct Resize {
    object: Object,
    revision: u64,
    token: (EntityId, u64, u64, u64, Option<EntityId>),
    origin: Pixels,
    width: u32,
    original_width: u32,
    maximum: u32,
    resources: Option<crate::ui::code_editor::markdown::resources::ResourceContext>,
}

impl SourcePresentation {
    pub(super) fn image_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let key = &event.keystroke;
        let secondary = if cfg!(target_os = "macos") {
            key.modifiers.platform
        } else {
            key.modifiers.control
        };
        if self.live && secondary && key.modifiers.shift && key.key == "i" {
            self.fit_images(window, cx);
            return true;
        }
        false
    }
    pub(super) fn image_surface(
        &self,
        leaf: &super::super::projection::Leaf,
        rendered: AnyElement,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some((range, image)) = leaf.image_objects().next() else {
            return rendered;
        };
        let source = self.document.read(cx).source();
        let before = source.slice(leaf.range.start..range.start).to_string();
        let block = source
            .slice(range.end..leaf.range.end)
            .chars()
            .all(char::is_whitespace)
            && (before.trim().is_empty()
                || matches!(leaf.kind, Kind::List(_)) && leaf.image_objects().count() == 1);
        let object = Object {
            raw: source.slice(range.clone()).to_string(),
            range,
            image: image.clone(),
            block,
        };
        let selected = object.range.start <= self.state.selection.head()
            && self.state.selection.head() <= object.range.end;
        let view = cx.entity().downgrade();
        let menu_object = object.clone();
        let surface = div()
            .id(("markdown-image-object", leaf.id))
            .relative()
            .child(rendered)
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |view, _, window, cx| {
                    view.command(
                        Command::Select(Selection::caret(menu_object.range.start)),
                        window,
                        cx,
                    );
                    view.focus.focus(window, cx);
                }),
            )
            .context_menu(move |menu, _, cx| match view.upgrade() {
                Some(view) => view.read(cx).image_menu(menu, cx),
                None => menu,
            });
        surface
            .when(block && selected, |surface| {
                surface.child(
                    Button::new(("markdown-image-resize", leaf.id))
                        .small()
                        .ghost()
                        .label(t(L10nKey::MarkdownImageResize))
                        .absolute()
                        .right_0()
                        .bottom_0()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |view, event: &MouseDownEvent, window, cx| {
                                cx.stop_propagation();
                                view.focus.focus(window, cx);
                                let maximum = view.image_maximum(window);
                                let width = object
                                    .image
                                    .width
                                    .unwrap_or(f32::from(window.rem_size() * 12.) as u32)
                                    .clamp(40, maximum);
                                view.image_resize = Some(Resize {
                                    object: object.clone(),
                                    revision: view.document.read(cx).revision(),
                                    token: view.input_token(cx),
                                    origin: event.position.x,
                                    original_width: width,
                                    width,
                                    maximum,
                                    resources: view.document.read(cx).resources().cloned(),
                                });
                            }),
                        )
                        .on_mouse_up(
                            MouseButton::Left,
                            cx.listener(|view, _, window, cx| {
                                if view.finish_image_resize(false, window, cx) {
                                    cx.stop_propagation();
                                }
                            }),
                        )
                        .on_mouse_up_out(
                            MouseButton::Left,
                            cx.listener(|view, _, window, cx| {
                                view.finish_image_resize(false, window, cx);
                            }),
                        )
                        .on_click(cx.listener(|view, event, window, cx| {
                            if !matches!(event, ClickEvent::Mouse(_)) {
                                view.edit_image(window, cx);
                            }
                        })),
                )
            })
            .into_any_element()
    }
    pub(super) fn move_image_resize(
        &mut self,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(resize) = &mut self.image_resize else {
            return false;
        };
        if resize.revision != self.document.read(cx).revision() {
            self.image_resize = None;
            return false;
        }
        let width = (resize.original_width as f32 + f32::from(position.x - resize.origin))
            .round()
            .clamp(40., resize.maximum as f32) as u32;
        if width == resize.width {
            return true;
        }
        resize.width = width;
        let ix = self
            .projection
            .leaves
            .partition_point(|leaf| leaf.range.start <= resize.object.range.start)
            .saturating_sub(1);
        if let Some(leaf) = self.projection.leaves.get_mut(ix) {
            leaf.resize_image(&resize.object.range, Some(resize.width));
        }
        self.invalidate_layout(cx);
        cx.notify();
        true
    }
    pub(super) fn finish_image_resize(
        &mut self,
        cancel: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(resize) = self.image_resize.take() else {
            return false;
        };
        if resize.revision != self.document.read(cx).revision() {
            return true;
        }
        let ix = self
            .projection
            .leaves
            .partition_point(|leaf| leaf.range.start <= resize.object.range.start)
            .saturating_sub(1);
        if let Some(leaf) = self.projection.leaves.get_mut(ix) {
            leaf.resize_image(&resize.object.range, resize.object.image.width);
        }
        self.invalidate_layout(cx);
        let current = self.input_token(cx) == resize.token
            && match &resize.resources {
                Some(resources) => resources.is_current(self.document.read(cx).resources()),
                None => self.document.read(cx).resources().is_none(),
            };
        if !cancel && current && resize.width != resize.original_width {
            self.replace_image(
                &resize.object,
                resize.object.sized(Some(resize.width)),
                window,
                cx,
            );
        }
        cx.notify();
        true
    }
    fn image_objects(&self, cx: &App) -> Vec<Object> {
        if !self.live || self.projection.revision != self.document.read(cx).revision() {
            return Vec::new();
        }
        let source = self.document.read(cx).source();
        self.projection
            .leaves
            .iter()
            .flat_map(|leaf| {
                leaf.image_objects().map(|(range, image)| {
                    let raw = source.slice(range.clone()).to_string();
                    let before = source.slice(leaf.range.start..range.start).to_string();
                    let after = source.slice(range.end..leaf.range.end).to_string();
                    let block = after.trim().is_empty()
                        && (before.trim().is_empty()
                            || matches!(leaf.kind, Kind::List(_))
                                && leaf.image_objects().count() == 1);
                    Object {
                        range,
                        image: image.clone(),
                        raw,
                        block,
                    }
                })
            })
            .collect()
    }
    fn selected_image(&self, cx: &App) -> Option<Object> {
        let selection = self.state.selection.range();
        self.image_objects(cx).into_iter().find(|object| {
            object.range.start <= selection.start && selection.end <= object.range.end
        })
    }
    fn replace_image(
        &mut self,
        object: &Object,
        text: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if text == object.raw
            || self
                .document
                .read(cx)
                .source()
                .slice(object.range.clone())
                .to_string()
                != object.raw
        {
            return;
        }
        let selection = Selection::new(object.range.start, object.range.start + text.len());
        self.command(
            Command::BatchReplaceSelected {
                edits: vec![(object.range.clone(), text)],
                selection,
            },
            window,
            cx,
        );
        self.focus.focus(window, cx);
    }
    pub(super) fn edit_image(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(object) = self.selected_image(cx) else {
            return;
        };
        self.command(Command::FinishComposition, window, cx);
        let revision = self.document.read(cx).revision();
        let token = self.input_token(cx);
        let resources = self.document.read(cx).resources().cloned();
        let alt = cx.new(|cx| InputState::new(window, cx).default_value(object.image.alt.clone()));
        let source =
            cx.new(|cx| InputState::new(window, cx).default_value(object.image.url.clone()));
        let width = cx.new(|cx| {
            InputState::new(window, cx).default_value(
                object
                    .image
                    .width
                    .map(|width| width.to_string())
                    .unwrap_or_default(),
            )
        });
        let initial_focus = std::cell::Cell::new(true);
        let error = std::rc::Rc::new(std::cell::RefCell::new(None::<SharedString>));
        let view = cx.entity().downgrade();
        let focus = self.focus.clone();
        window.open_dialog(cx, move |dialog, window, cx| {
            if initial_focus.replace(false) {
                alt.focus_handle(cx).focus(window, cx);
            }
            let view = view.clone();
            let object = object.clone();
            let alt = alt.clone();
            let source = source.clone();
            let width = width.clone();
            let resources = resources.clone();
            let message = error.borrow().clone();
            let error = error.clone();
            dialog
                .title(t(L10nKey::MarkdownImageEdit))
                .w(window.rem_size() * 30.)
                .child(
                    div()
                        .v_flex()
                        .gap_2()
                        .child(t(L10nKey::MarkdownImageAlt))
                        .child(
                            Input::new(&alt)
                                .id("markdown-image-alt")
                                .aria_label(t(L10nKey::MarkdownImageAlt)),
                        )
                        .child(t(L10nKey::MarkdownImageSource))
                        .child(
                            Input::new(&source)
                                .id("markdown-image-source")
                                .aria_label(t(L10nKey::MarkdownImageSource)),
                        )
                        .child(t(L10nKey::MarkdownImageWidth))
                        .child(
                            Input::new(&width)
                                .id("markdown-image-width")
                                .aria_label(t(L10nKey::MarkdownImageWidth)),
                        ),
                )
                .when_some(message, |dialog, message| {
                    dialog.child(div().text_sm().text_color(cx.theme().danger).child(message))
                })
                .footer(
                    DialogFooter::new()
                        .child(
                            DialogClose::new().trigger(|button| button.label(t(L10nKey::Cancel))),
                        )
                        .child(
                            DialogAction::new().child(
                                Button::new("markdown-image-apply")
                                    .primary()
                                    .label(t(L10nKey::MarkdownImageApply)),
                            ),
                        ),
                )
                .on_ok(move |_, window, cx| {
                    let width = width.read(cx).value();
                    let width = if width.trim().is_empty() {
                        None
                    } else {
                        match width.trim().parse::<u32>() {
                            Ok(width) if width > 0 => Some(width),
                            _ => {
                                *error.borrow_mut() = Some(t(L10nKey::MarkdownImageInvalid).into());
                                window.refresh();
                                return false;
                            }
                        }
                    };
                    let alt = alt.read(cx).value();
                    let source = source.read(cx).value();
                    if alt.contains(['\n', '\r']) || source.contains(['\n', '\r']) {
                        *error.borrow_mut() = Some(t(L10nKey::MarkdownImageInvalid).into());
                        window.refresh();
                        return false;
                    }
                    let applied = view
                        .update(cx, |view, cx| {
                            if view.document.read(cx).revision() != revision
                                || view.input_token(cx) != token
                                || !match &resources {
                                    Some(resources) => {
                                        resources.is_current(view.document.read(cx).resources())
                                    }
                                    None => view.document.read(cx).resources().is_none(),
                                }
                            {
                                return false;
                            }
                            let Some(text) = object.edited(&alt, &source, width) else {
                                return false;
                            };
                            view.replace_image(&object, text, window, cx);
                            true
                        })
                        .unwrap_or(false);
                    if !applied {
                        *error.borrow_mut() = Some(t(L10nKey::MarkdownImageChanged).into());
                        window.refresh();
                    }
                    applied
                })
                .on_close({
                    let focus = focus.clone();
                    move |_, window, cx| focus.focus(window, cx)
                })
        });
    }
    pub(super) fn preview_image(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(object) = self.selected_image(cx) else {
            return;
        };
        let Some(super::super::images::State::Ready(image)) =
            self.document.read(cx).images.states.get(&object.image.url)
        else {
            return;
        };
        let image = image.clone();
        let focus = self.focus.clone();
        window.open_dialog(cx, move |dialog, window, _| {
            dialog
                .title(t(L10nKey::MarkdownImagePreview))
                .w(window.viewport_size().width * 0.8)
                .child(
                    img(image.clone())
                        .max_w_full()
                        .max_h(window.viewport_size().height * 0.7)
                        .object_fit(ObjectFit::Contain),
                )
                .on_close({
                    let focus = focus.clone();
                    move |_, window, cx| focus.focus(window, cx)
                })
        });
    }
    pub(super) fn delete_image(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(object) = self.selected_image(cx) else {
            return;
        };
        self.remove_image(object, window, cx);
    }
    fn remove_image(&mut self, object: Object, window: &mut Window, cx: &mut Context<Self>) {
        let source = self.document.read(cx).source();
        let mut range = object.range;
        if object.block {
            let line = source.offset_to_point(range.start).row;
            range.start = source.line_start_offset(line);
            if source.slice(range.end..source.len()).chars().next() == Some('\n') {
                range.end += 1;
            }
        }
        self.command(
            Command::Replace {
                range,
                text: String::new(),
                kind: EditKind::Command,
            },
            window,
            cx,
        );
        self.focus.focus(window, cx);
    }
    pub(super) fn delete_adjacent_image(
        &mut self,
        backward: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.state.selection.range().is_empty() {
            return false;
        }
        let at = self.state.selection.head();
        let object = self.image_objects(cx).into_iter().find(|object| {
            object.block
                && if backward {
                    at == object.range.end || at == object.range.end + 1
                } else {
                    at == object.range.start || at + 1 == object.range.start
                }
        });
        if let Some(object) = object {
            self.remove_image(object, window, cx);
            true
        } else {
            false
        }
    }
    pub(super) fn fit_images(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let width = self.image_maximum(window) / 2;
        let edits = self
            .image_objects(cx)
            .into_iter()
            .filter(|object| {
                object.block
                    && object
                        .image
                        .width
                        .unwrap_or(f32::from(window.rem_size() * 12.) as u32)
                        > width
            })
            .map(|object| (object.range.clone(), object.sized(Some(width))))
            .collect::<Vec<_>>();
        if !edits.is_empty() {
            self.command(
                Command::BatchReplace {
                    edits,
                    kind: EditKind::Command,
                },
                window,
                cx,
            );
        }
    }
    fn image_maximum(&self, window: &Window) -> u32 {
        let available = self
            .viewport
            .bounds
            .map_or(window.viewport_size().width, |bounds| bounds.size.width);
        f32::from(typography::content_width(
            available,
            window.rem_size(),
            self.live,
        ))
        .max(40.) as u32
    }
    pub(super) fn image_menu(
        &self,
        menu: gpui_kit::component::menu::PopupMenu,
        cx: &App,
    ) -> gpui_kit::component::menu::PopupMenu {
        let ready = self.selected_image(cx).is_some_and(|object| {
            matches!(
                self.document.read(cx).images.states.get(&object.image.url),
                Some(super::super::images::State::Ready(_))
            )
        });
        menu.action_context(self.focus.clone())
            .menu(t(L10nKey::MarkdownImageEdit), Box::new(EditImage))
            .menu_with_disabled(
                t(L10nKey::MarkdownImagePreview),
                Box::new(PreviewImage),
                !ready,
            )
            .menu(t(L10nKey::MarkdownImageFit), Box::new(FitImages))
            .separator()
            .menu(t(L10nKey::MarkdownImageDelete), Box::new(DeleteImage))
    }
    pub(super) fn image_toolbar(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(object) = self.selected_image(cx) else {
            return div().into_any_element();
        };
        let focus = self.focus.clone();
        let view = cx.entity().downgrade();
        gpui_kit::component::h_flex()
            .gap_2()
            .flex_none()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                Button::new("markdown-image-edit")
                    .small()
                    .ghost()
                    .label(t(L10nKey::MarkdownImageEdit))
                    .on_click(cx.listener(|view, _, window, cx| view.edit_image(window, cx))),
            )
            .child(
                Button::new("markdown-image-preview")
                    .small()
                    .ghost()
                    .disabled(!matches!(
                        self.document.read(cx).images.states.get(&object.image.url),
                        Some(super::super::images::State::Ready(_))
                    ))
                    .label(t(L10nKey::MarkdownImagePreview))
                    .on_click(cx.listener(|view, _, window, cx| view.preview_image(window, cx))),
            )
            .child(
                Button::new("markdown-image-commands")
                    .small()
                    .ghost()
                    .label(t(L10nKey::MarkdownImageCommands))
                    .dropdown_menu(move |menu, _, cx| match view.upgrade() {
                        Some(view) => view
                            .read(cx)
                            .image_menu(menu.action_context(focus.clone()), cx),
                        None => menu,
                    }),
            )
            .when(object.block, |bar| {
                bar.child(
                    Button::new("markdown-image-size")
                        .small()
                        .ghost()
                        .label(t(L10nKey::MarkdownImageWidth))
                        .on_click(cx.listener(|view, _, window, cx| view.edit_image(window, cx))),
                )
            })
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{EditImage, FitImages};
    use gpui_kit::{TestAppContext, component::input as actions, test::TestWindowExt as _};

    #[gpui_kit::test]
    fn image_size_action_is_one_undo_and_preserves_original_syntax(cx: &mut TestAppContext) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("image.md");
        let original = "before\r\n\r\n![A](<missing.png> 'title'){width=1200px}\r\n\r\ntail\n";
        std::fs::write(&path, original).unwrap();
        let (app, mut context, _streams) = crate::ui::app::test_window::harness_with_tabs(cx, 1);
        app.update_in(&mut context, |app, window, cx| {
            app.open_file_in_editor(&path, window, cx)
        });
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let view = loop {
            context.run_until_parked();
            if let Some(view) = app.read_with(&context, |app, _| {
                app.active_editor_input()
                    .and_then(|input| input.markdown().cloned())
            }) {
                break view;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(5));
        };
        loop {
            context.run_until_parked();
            if view.read_with(&context, |view, cx| {
                view.projection.revision == view.document.read(cx).revision()
                    && view
                        .projection
                        .leaves
                        .iter()
                        .any(|leaf| leaf.image_objects().next().is_some())
            }) {
                break;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        view.update_in(&mut context, |view, window, cx| {
            view.select_range(8..8, cx);
            view.focus.focus(window, cx);
        });
        context.dispatch_action(EditImage);
        context.update(|window, cx| {
            window.render_frame(cx);
            window.click("markdown-image-apply", cx);
        });
        context.run_until_parked();
        view.read_with(&context, |view, cx| {
            assert_eq!(
                view.document.read(cx).encode().unwrap(),
                original.as_bytes()
            )
        });
        context.dispatch_action(FitImages);
        context.run_until_parked();
        view.read_with(&context, |view, cx| {
            assert!(
                view.document
                    .read(cx)
                    .source()
                    .to_string()
                    .contains("{width=")
            );
            assert!(
                !view
                    .document
                    .read(cx)
                    .source()
                    .to_string()
                    .contains("{width=1200px}")
            );
        });
        context.dispatch_action(actions::Undo);
        context.run_until_parked();
        view.read_with(&context, |view, cx| {
            assert_eq!(
                view.document.read(cx).encode().unwrap(),
                original.as_bytes()
            )
        });
    }
}
