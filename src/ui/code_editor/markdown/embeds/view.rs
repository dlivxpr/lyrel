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
    }
    pub(super) fn request_embeds(&mut self, cx: &mut Context<Self>) {
        if !self.live
            || self.embeds.pending
            || self.projection.revision != self.document.read(cx).revision()
        {
            return;
        }
        if self.projection.links.embeds.is_empty() {
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
        self.embeds.loaded = Some(loaded);
        self.embeds.installed = Some(revision);
        self.invalidate_layout(cx);
        cx.notify();
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
