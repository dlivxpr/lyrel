//! The only retained state here is derived content and presentation-local scroll.
use super::super::embeds::{Entry, Fragment, Loaded};
use super::*;
use gpui_kit::component::{
    Sizable as _,
    button::{Button, ButtonVariants as _},
};
use std::sync::Arc;

#[derive(Default)]
pub(super) struct State {
    generation: u64,
    pending: bool,
    lease: Option<(u64, super::super::resources::ResourceContext)>,
    installed: Option<u64>,
    loaded: Option<Arc<Loaded>>,
    controls: HashMap<(usize, String), (ScrollHandle, FocusHandle)>,
    task: Option<Task<()>>,
    hover: Option<super::super::links::Link>,
    hover_position: Point<Pixels>,
    hover_open: bool,
}
impl SourcePresentation {
    pub(super) fn start_embed_refresh(&mut self, cx: &mut Context<Self>) {
        self.embeds.task = Some(cx.spawn(async move |view, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(750))
                    .await;
                if view.update(cx, |view, cx| view.request_embeds(cx)).is_err() {
                    break;
                }
            }
        }));
    }
    pub(super) fn reset_embeds(&mut self) {
        self.embeds.generation = self.embeds.generation.wrapping_add(1);
        self.embeds.pending = false;
        self.embeds.lease = None;
        self.embeds.loaded = None;
        self.embeds.installed = None;
        self.embeds.controls.clear();
        self.embeds.hover = None;
        self.embeds.hover_open = false;
    }
    pub(super) fn request_embeds(&mut self, cx: &mut Context<Self>) {
        if !self.live
            || self.embeds.pending
            || self.projection.revision != self.document.read(cx).revision()
        {
            return;
        }
        if self.projection.links.embeds.is_empty() && self.embeds.hover.is_none() {
            if self.embeds.loaded.is_some() {
                self.reset_embeds();
            }
            return;
        }
        let document = self.document.read(cx);
        let Some(resources) = document
            .resources()
            .cloned()
            .filter(|resources| resources.path().is_some())
        else {
            return;
        };
        let revision = document.revision();
        self.embeds.generation = self.embeds.generation.wrapping_add(1);
        self.embeds.pending = true;
        self.embeds.lease = Some((revision, resources.clone()));
        let presentation = cx.weak_entity();
        let generation = self.embeds.generation;
        let resource_epoch = Some((resources.host().id(), resources.epoch()));
        self.document.update(cx, |_, cx| {
            cx.emit(DocumentEvent::RequestEmbeds {
                presentation,
                generation,
                revision,
                resource_epoch,
            })
        });
    }
    pub(crate) fn embed_request(&self, cx: &App) -> super::super::embeds::Request {
        super::super::embeds::Request::new(
            self.document.read(cx).revision(),
            &self.projection.links.embeds,
        )
        .with_preview(self.embeds.hover.clone())
    }
    pub(crate) fn previous_embeds(&self) -> Option<Arc<Loaded>> {
        self.embeds.loaded.clone()
    }
    pub(crate) fn discard_embed_results(&mut self, generation: u64) {
        if self.embeds.generation == generation {
            self.embeds.pending = false;
        }
    }
    pub(crate) fn install_embed_results(
        &mut self,
        generation: u64,
        loaded: Arc<Loaded>,
        cx: &mut Context<Self>,
    ) {
        if self.embeds.generation != generation {
            return;
        }
        self.embeds.pending = false;
        let document = self.document.read(cx);
        if !self.live
            || self
                .embeds
                .lease
                .as_ref()
                .is_none_or(|(revision, resources)| {
                    *revision != document.revision() || !resources.is_current(document.resources())
                })
        {
            return;
        }
        let revision = document.revision();
        if self.embeds.installed == Some(revision)
            && self
                .embeds
                .loaded
                .as_ref()
                .is_some_and(|old| Arc::ptr_eq(old, &loaded))
        {
            return;
        }
        let keys: std::collections::HashSet<_> = loaded
            .entries
            .iter()
            .map(|entry| (entry.reference.source.start, entry.reference.target.clone()))
            .collect();
        self.embeds.controls.retain(|key, _| keys.contains(key));
        for key in keys {
            self.embeds
                .controls
                .entry(key)
                .or_insert_with(|| (ScrollHandle::new(), cx.focus_handle().tab_stop(true)));
        }
        let layout_changed = self.embeds.loaded.as_ref().map_or(
            !loaded.entries.is_empty(),
            |previous| {
                previous.entries.len() != loaded.entries.len()
                    || previous.entries.iter().zip(&loaded.entries).any(|(old, new)| {
                        old.reference != new.reference
                            || match (&old.result, &new.result) {
                                (Ok(old), Ok(new)) => !Arc::ptr_eq(old, new),
                                (Err(old), Err(new)) => old != new,
                                _ => true,
                            }
                    })
            },
        );
        self.embeds.loaded = Some(loaded);
        self.embeds.installed = Some(revision);
        // A hover excerpt changes only the popup. Clearing source geometry
        // would unmount its trigger while its delayed open is still pending.
        if layout_changed {
            self.invalidate_layout(cx);
        }
        cx.notify();
    }
    pub(super) fn leave_link_hover(&mut self, window: &Window, cx: &mut Context<Self>) {
        // Installing the deferred trigger can end the document's hover without
        // the pointer leaving the label. Do not destroy its pending 450ms timer.
        if !self.embeds.hover_open && self.pointer_link_at(window.mouse_position(), cx).is_none() {
            if self.embeds.hover.take().is_some() {
                self.embeds.generation = self.embeds.generation.wrapping_add(1);
                self.embeds.pending = false;
                cx.notify();
            }
        }
    }
    pub(super) fn hover_link(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        let link = if event.pressed_button.is_none() {
            self.pointer_link_at(event.position, cx)
                .map(|(link, _)| link.clone())
        } else {
            None
        };
        let previous_position = self.embeds.hover_position;
        self.embeds.hover_position = event.position;
        if self.embeds.hover.as_ref() == link.as_ref() {
            if let Some(link) = &link
                && !self.link_label_regions(link, cx).iter().any(|bounds| {
                    bounds.contains(&previous_position) && bounds.contains(&event.position)
                })
            {
                cx.notify();
            }
            return;
        }
        self.embeds.generation = self.embeds.generation.wrapping_add(1);
        self.embeds.pending = false;
        self.embeds.hover = link;
        self.embeds.hover_open = false;
        self.request_embeds(cx);
        cx.notify();
    }
    pub(super) fn link_preview(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let link = self.embeds.hover.as_ref()?;
        let document = self.document.read(cx);
        if !self.live || self.projection.revision != document.revision() {
            return None;
        }
        let bounds = self
            .link_label_regions(link, cx)
            .into_iter()
            .find(|bounds| bounds.contains(&self.embeds.hover_position))?;
        let entry = self
            .embeds
            .loaded
            .as_ref()
            .filter(|_| self.embeds.installed == Some(document.revision()))
            .and_then(|loaded| loaded.preview.as_ref())
            .filter(|entry| {
                entry.reference.source == link.source
                    && entry.reference.target == link.target.split('#').next().unwrap_or("")
            });
        let fragment = entry.and_then(|entry| entry.result.as_ref().ok()).cloned();
        let reason = entry.and_then(|entry| entry.result.as_ref().err()).copied();
        let external = url::Url::parse(&link.target)
            .ok()
            .filter(|url| matches!(url.scheme(), "https" | "http" | "mailto"))
            .map(|_| link.target.clone());
        let view = cx.weak_entity();
        let expected = link.clone();
        Some(
            deferred(
                anchored().position(bounds.origin).child(
                    gpui_kit::component::hover_card::HoverCard::new("markdown-link-preview-card")
                        .anchor(Anchor::TopLeft)
                        .open_delay(std::time::Duration::from_millis(450))
                        .trigger(
                            div().w(bounds.size.width).h(bounds.size.height)
                                .on_mouse_move(cx.listener(|view, event: &MouseMoveEvent, _, cx| view.hover_link(event, cx))),
                        )
                        .on_open_change(move |open, _, cx| {
                            let _ = view.update(cx, |view, cx| {
                                if view.embeds.hover.as_ref() != Some(&expected) {
                                    return;
                                }
                                view.embeds.hover_open = *open;
                                if !open {
                                    view.embeds.hover = None;
                                }
                                cx.notify();
                            });
                        })
                        .content(move |_, window, cx| {
                            let mut body = div()
                                .id("markdown-link-preview")
                                .test_support()
                                .v_flex()
                                .gap_2()
                                .w_80()
                                .max_h(window.rem_size() * 20.)
                                .overflow_y_scroll();
                            // Preview content owns its pointer; moving into it
                            // must not look like leaving the source label.
                            body = body.on_mouse_move(|_, _, cx| cx.stop_propagation());
                            if let Some(fragment) = &fragment {
                                body =
                                    body.role(Role::Group)
                                        .aria_label(SharedString::from(format!(
                                            "{}\n{}",
                                            fragment.title, fragment.text
                                        )));
                                body = body
                                    .child(
                                        div()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .child(fragment.title.clone()),
                                    )
                                    .child(render_fragment(
                                        fragment,
                                        "markdown-link-preview",
                                        window,
                                        cx,
                                    ));
                            } else {
                                body = body.child(external.clone().unwrap_or_else(|| {
                                    t(reason.unwrap_or(L10nKey::MarkdownReferenceLoading))
                                        .to_owned()
                                }));
                            }
                            body
                        }),
                ),
            )
            .into_any_element(),
        )
    }
    pub(super) fn render_embeds(
        &self,
        leaf: &super::super::projection::Leaf,
        source: AnyElement,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if !self.live {
            return source;
        }
        let mut surface = div().w_full().child(source);
        for reference in self
            .projection
            .links
            .embeds
            .iter()
            .filter(|reference| {
                leaf.range.start <= reference.source.start
                    && reference.source.start < leaf.range.end
            })
            .take(65)
        {
            let range = reference.source.clone();
            let revision = self.projection.revision;
            let resources = self.document.read(cx).resources().cloned();
            let id = reference.source.start;
            let label = self
                .document
                .read(cx)
                .source()
                .slice(reference.label.clone())
                .to_string();
            let mut embed = div()
                .id(SharedString::from(format!("markdown-embed-{id}")))
                .test_support()
                .w_full()
                .v_flex()
                .gap_2()
                .border_l_1()
                .border_color(cx.theme().border)
                .pl_3()
                .py_2()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                    if event.keystroke.key == "escape" {
                        this.focus.focus(window, cx);
                        cx.stop_propagation();
                    }
                }))
                .child(
                    gpui_kit::component::h_flex()
                        .gap_2()
                        .flex_wrap()
                        .child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(t(L10nKey::MarkdownEmbedReadonly)),
                        )
                        .child(div().text_sm().child(label))
                        .child(
                            Button::new(SharedString::from(format!("markdown-embed-source-{id}")))
                                .small()
                                .ghost()
                                .label(t(L10nKey::MarkdownEmbedSource))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    let document = this.document.read(cx);
                                    if document.revision() != revision
                                        || resources.as_ref().is_none_or(|resources| {
                                            !resources.is_current(document.resources())
                                        })
                                    {
                                        return;
                                    }
                                    this.command(
                                        Command::Select(Selection::new(range.start, range.end)),
                                        window,
                                        cx,
                                    );
                                    this.focus.focus(window, cx);
                                })),
                        ),
                );
            let entry = self
                .embeds
                .loaded
                .as_ref()
                .filter(|_| self.embeds.installed == Some(revision))
                .and_then(|loaded| {
                    loaded
                        .entries
                        .iter()
                        .find(|entry| entry.reference == *reference)
                });
            if let Some(entry) = entry {
                let content = render_entry(entry, &format!("root-{id}"), window, cx);
                let mut body = div()
                    .id(SharedString::from(format!("markdown-embed-content-{id}")))
                    .test_support()
                    .role(Role::Group)
                    .aria_label(t(L10nKey::MarkdownEmbedReadonly))
                    .w_full()
                    .min_w_0()
                    .max_h_96()
                    .overflow_hidden()
                    .border_1()
                    .border_color(cx.theme().border)
                    .focus_visible(|style| style.border_color(cx.theme().primary));
                if let Ok(fragment) = &entry.result {
                    body = body.aria_value(fragment.text.clone());
                }
                if let Some((scroll, focus)) =
                    self.embeds.controls.get(&(id, reference.target.clone()))
                {
                    let pointer_focus = focus.clone();
                    let wheel = scroll.clone();
                    let keyboard = scroll.clone();
                    body = body
                        .track_scroll(scroll)
                        .track_focus(focus)
                        .key_context("Input mode=editor MarkdownEmbed")
                        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                            pointer_focus.focus(window, cx);
                            cx.stop_propagation();
                        })
                        .on_scroll(move |event, window, cx| {
                            let delta = event
                                .delta
                                .pixel_delta(window.rem_size() * typography::LINE_HEIGHT);
                            if scroll_by(&wheel, delta.y) {
                                window.refresh();
                                cx.stop_propagation();
                            }
                        })
                        .on_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
                            let distance = match event.keystroke.key.as_str() {
                                "up" => Some(window.rem_size() * typography::LINE_HEIGHT),
                                "down" => Some(-window.rem_size() * typography::LINE_HEIGHT),
                                "pageup" => Some(keyboard.bounds().size.height),
                                "pagedown" => Some(-keyboard.bounds().size.height),
                                "home" => Some(keyboard.max_offset().y),
                                "end" => Some(-keyboard.max_offset().y),
                                _ => None,
                            };
                            if let Some(distance) = distance {
                                if !scroll_by(&keyboard, distance) {
                                    scroll_by(&this.scroll, distance);
                                }
                                window.refresh();
                                cx.stop_propagation();
                            }
                        }));
                }
                embed = embed.child(body.child(content));
            } else {
                let reason = if self.embeds.installed == Some(revision) {
                    L10nKey::MarkdownEmbedLimit
                } else {
                    L10nKey::MarkdownEmbedLoading
                };
                embed = embed.child(
                    div()
                        .id(("markdown-embed-status", id as u64))
                        .role(Role::Status)
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(t(reason)),
                );
            }
            surface = surface.child(embed);
        }
        surface.into_any_element()
    }
}
fn scroll_by(scroll: &ScrollHandle, delta: Pixels) -> bool {
    let old = scroll.offset();
    let next = point(
        old.x,
        (old.y + delta).clamp(-scroll.max_offset().y, Pixels::ZERO),
    );
    if next == old {
        return false;
    }
    scroll.set_offset(next);
    true
}
fn render_entry(entry: &Entry, namespace: &str, window: &mut Window, cx: &mut App) -> AnyElement {
    match &entry.result {
        Ok(fragment) => render_fragment(fragment, namespace, window, cx),
        Err(reason) => div()
            .id(SharedString::from(format!(
                "markdown-embed-{}-{namespace}",
                if *reason == L10nKey::MarkdownEmbedCycle {
                    "cycle"
                } else {
                    "error"
                }
            )))
            .test_support()
            .role(Role::Status)
            .aria_label(t(*reason))
            .text_sm()
            .text_color(cx.theme().muted_foreground)
            .child(t(*reason))
            .into_any_element(),
    }
}
fn render_fragment(
    fragment: &Fragment,
    namespace: &str,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let images = fragment.images.clone();
    let context = typography::context(
        RenderContext::from_app(cx)
            .with_body_font_size(window.rem_size())
            .with_markdown_extensions(super::super::math::extensions())
            .with_image_source(move |uri| {
                images.get(uri.as_ref()).map_or_else(
                    || ImageSource::Custom(Arc::new(|_, _| None)),
                    |image| ImageSource::Render(image.clone()),
                )
            }),
        true,
        cx,
    );
    let references: Vec<_> = fragment.blocks.iter().collect();
    let mut body = div().w_full().min_w_0().v_flex();
    for (ix, (leaf, block)) in fragment
        .projection
        .leaves
        .iter()
        .zip(&fragment.blocks)
        .enumerate()
    {
        let rendered = block
            .render_with_id(
                SharedString::from(format!("{namespace}-{}", leaf.range.start)),
                FlowPosition::at(&references, ix, ix + 1 == references.len()),
                &context,
                window,
                cx,
            )
            .into_any_element();
        let rendered = if matches!(leaf.kind, Kind::Mermaid(_)) {
            fragment
                .diagrams
                .render(leaf, false, rendered, Empty.into_any_element(), cx)
        } else {
            rendered
        };
        body = body.child(rendered);
        for (nested_ix, entry) in fragment.nested.iter().enumerate().filter(|(_, entry)| {
            leaf.range.start <= entry.reference.source.start
                && entry.reference.source.start < leaf.range.end
        }) {
            body = body.child(
                div()
                    .pl_3()
                    .border_l_1()
                    .border_color(cx.theme().border)
                    .child(render_entry(
                        entry,
                        &format!("{namespace}-{nested_ix}"),
                        window,
                        cx,
                    )),
            );
        }
    }
    for reason in &fragment.image_errors {
        body = body.child(
            div()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(t(*reason)),
        );
    }
    body.into_any_element()
}
