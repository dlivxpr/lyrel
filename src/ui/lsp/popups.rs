//! Application-owned LSP surfaces. Kit supplies the editor, list interaction,
//! text history and geometry; the application owns lazy server resolution,
//! import edits, commands and signature-help state.
use std::ops::Range;
use std::rc::Rc;

use gpui_kit::EntityInputHandler as _;
use gpui_kit::Focusable as _;
use gpui_kit::component::input::{CompletionProvider, EditorState, InputEvent};
use gpui_kit::component::list::{List, ListDelegate, ListEvent, ListItem, ListState};
use gpui_kit::component::text::TextView;
use gpui_kit::component::{
    ActiveTheme as _, IndexPath, Rope, RopeExt as _, Sizable as _, h_flex, v_flex,
};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, App, AvailableSpace, Bounds, Context, Div, Element, ElementId, Entity,
    GlobalElementId, InspectorElementId, LayoutId, Pixels, Stateful, Style, Subscription, Task,
    WeakEntity, Window, anchored, deferred, div, point, px, size,
};
use lsp_types::{CompletionItem, CompletionResponse, CompletionTextEdit, TextEdit};

use super::providers::LspProvider;

pub(crate) fn init(cx: &mut App) {
    cx.bind_keys([gpui_kit::KeyBinding::new(
        "ctrl-space",
        gpui_kit::component::input::ShowCompletions,
        Some("Input && mode == editor"),
    )]);
}

struct Candidate {
    item: CompletionItem,
    // Only raw server items may enter resolve_completion: its result already
    // uses editor columns and must never be converted a second time.
    resolved: bool,
}

struct Candidates {
    items: Vec<Candidate>,
    selected: Option<IndexPath>,
    selection_seq: u64,
}

impl ListDelegate for Candidates {
    type Item = ListItem;
    fn items_count(&self, _: usize, _: &App) -> usize {
        self.items.len()
    }
    fn set_selected_index(
        &mut self,
        ix: Option<IndexPath>,
        _: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) {
        if self.selected != ix {
            self.selection_seq += 1;
        }
        self.selected = ix;
        cx.notify();
    }
    fn render_item(
        &mut self,
        ix: IndexPath,
        _: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) -> Option<Self::Item> {
        let item = &self.items.get(ix.row)?.item;
        Some(
            ListItem::new(ix).child(h_flex().gap_3().child(item.label.clone()).when_some(
                item.detail.clone(),
                |row, detail| {
                    row.child(
                        div()
                            .truncate()
                            .text_color(cx.theme().muted_foreground)
                            .child(detail),
                    )
                },
            )),
        )
    }
}

#[derive(Clone)]
struct CompletionTarget {
    text: Rope,
    cursor: usize,
    start: usize,
}

pub(crate) struct EditorPopups {
    input: WeakEntity<EditorState>,
    provider: Option<Rc<LspProvider>>,
    list: Entity<ListState<Candidates>>,
    target: Option<CompletionTarget>,
    completion_seq: u64,
    request: Option<Task<()>>,
    resolve_request: Option<Task<()>>,
    highlighted: Option<IndexPath>,
    highlight_seq: u64,
    accept_pending: bool,
    suppressed_text: Option<Rope>,
    pub(super) signature_seq: u64,
    signature: Option<String>,
    signature_pending: bool,
    observed_text: Rope,
    observed_cursor: usize,
    _subscriptions: Vec<Subscription>,
}

impl EditorPopups {
    pub(crate) fn new(
        input: &Entity<EditorState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let list = cx.new(|cx| {
            ListState::new(
                Candidates {
                    items: Vec::new(),
                    selected: None,
                    selection_seq: 0,
                },
                window,
                cx,
            )
        });
        let focus = list.focus_handle(cx);
        input.update(cx, |state, cx| state.set_selection_focus(Some(focus), cx));
        let subscriptions =
            vec![
                cx.subscribe_in(
                    input,
                    window,
                    |this, input, event, window, cx| match event {
                        InputEvent::Change => {
                            let state = input.read(cx);
                            if this
                                .suppressed_text
                                .take()
                                .is_some_and(|text| text == *state.text())
                            {
                                return;
                            }
                            let cursor = state.cursor();
                            let before = state.text().chars_at(cursor).reversed().next();
                            let trigger = this.provider.as_ref().zip(before).and_then(
                                |(provider, before)| {
                                    provider
                                        .is_completion_trigger(cursor, &before.to_string(), cx)
                                        .then_some(before)
                                },
                            );
                            if let Some(trigger) = trigger {
                                this.request_completions(Some(trigger), window, cx);
                            } else {
                                this.dismiss_completions(cx);
                            }
                        }
                        InputEvent::Blur => {
                            if !input.read(cx).has_selection_focus(window, cx) {
                                this.dismiss_all(cx);
                            }
                        }
                        _ => {}
                    },
                ),
                cx.subscribe_in(&list, window, |this, _, event, window, cx| {
                    if matches!(event, ListEvent::Confirm(_)) {
                        this.accept(window, cx);
                    }
                    if matches!(event, ListEvent::Cancel) {
                        this.dismiss_completions(cx);
                        if let Some(input) = this.input.upgrade() {
                            input.update(cx, |state, cx| state.focus(window, cx));
                        }
                    }
                }),
                cx.observe_in(&list, window, |this, _, window, cx| {
                    this.resolve_selection(window, cx);
                    cx.notify();
                }),
                cx.observe(input, |this, input, cx| {
                    let (text, cursor) = {
                        let state = input.read(cx);
                        (state.text().clone(), state.cursor())
                    };
                    let moved = cursor != this.observed_cursor;
                    let edited = text != this.observed_text;
                    if moved && !edited {
                        this.signature_seq += 1;
                        this.signature = None;
                        this.signature_pending = false;
                    }
                    // Change may request against this same snapshot before the
                    // observer runs. Only invalidate a genuinely different target.
                    if this
                        .target
                        .as_ref()
                        .is_some_and(|target| target.cursor != cursor || target.text != text)
                    {
                        this.dismiss_completions(cx);
                    }
                    this.observed_text = text;
                    this.observed_cursor = cursor;
                    if moved || edited {
                        cx.notify();
                    }
                }),
            ];
        Self {
            input: input.downgrade(),
            provider: None,
            list,
            target: None,
            completion_seq: 0,
            request: None,
            resolve_request: None,
            highlighted: None,
            highlight_seq: 0,
            accept_pending: false,
            suppressed_text: None,
            signature_seq: 0,
            signature: None,
            signature_pending: false,
            observed_text: input.read(cx).text().clone(),
            observed_cursor: input.read(cx).cursor(),
            _subscriptions: subscriptions,
        }
    }

    pub(super) fn set_provider(
        &mut self,
        provider: Option<Rc<LspProvider>>,
        cx: &mut Context<Self>,
    ) {
        self.dismiss_all(cx);
        self.provider = provider;
    }

    pub(super) fn signature_visible(&self) -> bool {
        self.signature.is_some() || self.signature_pending
    }
    pub(super) fn begin_signature(&mut self) -> u64 {
        self.signature_seq += 1;
        self.signature_pending = true;
        self.signature_seq
    }
    pub(super) fn show_signature(&mut self, markdown: Option<String>, cx: &mut Context<Self>) {
        self.signature_pending = false;
        self.signature = markdown;
        cx.notify();
    }

    fn dismiss_completions(&mut self, cx: &mut Context<Self>) {
        self.completion_seq += 1;
        self.request = None;
        self.resolve_request = None;
        self.highlighted = None;
        self.highlight_seq += 1;
        self.accept_pending = false;
        self.target = None;
        cx.notify();
    }
    fn dismiss_all(&mut self, cx: &mut Context<Self>) {
        self.signature_seq += 1;
        self.signature = None;
        self.signature_pending = false;
        self.dismiss_completions(cx);
    }
    fn dismiss_top(&mut self, cx: &mut Context<Self>) -> bool {
        if self.target.is_some() {
            self.dismiss_completions(cx);
            true
        } else if self.signature.is_some() || self.signature_pending {
            self.signature_seq += 1;
            self.signature = None;
            self.signature_pending = false;
            cx.notify();
            true
        } else {
            false
        }
    }

    fn request_completions(
        &mut self,
        trigger: Option<char>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (Some(input), Some(provider)) = (self.input.upgrade(), self.provider.clone()) else {
            return;
        };
        if input.update(cx, |state, cx| {
            state.marked_text_range(window, cx).is_some()
        }) {
            self.dismiss_completions(cx);
            return;
        }
        let state = input.read(cx);
        if state.presentation().is_disabled()
            || state.presentation().is_readonly()
            || !state.has_selection_focus(window, cx)
        {
            self.dismiss_completions(cx);
            return;
        }
        let text = state.text().clone();
        let cursor = state.cursor();
        let start = super::providers::completion_prefix_start(&text, cursor);
        let target = CompletionTarget {
            text: text.clone(),
            cursor,
            start,
        };
        self.completion_seq += 1;
        let seq = self.completion_seq;
        self.target = Some(target);
        self.resolve_request = None;
        self.highlighted = None;
        self.highlight_seq += 1;
        self.accept_pending = false;
        self.list.update(cx, |list, cx| {
            list.delegate_mut().items.clear();
            list.set_selected_index(None, window, cx);
            cx.notify();
        });
        let request = provider.completions(
            &text,
            cursor,
            lsp_types::CompletionContext {
                trigger_kind: if trigger.is_some() {
                    lsp_types::CompletionTriggerKind::TRIGGER_CHARACTER
                } else {
                    lsp_types::CompletionTriggerKind::INVOKED
                },
                trigger_character: trigger.map(String::from),
            },
            window,
            cx,
        );
        self.request = Some(cx.spawn_in(window, async move |popup, cx| {
            let result = request.await;
            let _ = popup.update_in(cx, |popup, window, cx| {
                if popup.completion_seq != seq {
                    return;
                }
                let Some(input) = popup.input.upgrade() else {
                    return;
                };
                let state = input.read(cx);
                if *state.text() != text
                    || state.cursor() != cursor
                    || !state.has_selection_focus(window, cx)
                {
                    popup.dismiss_completions(cx);
                    return;
                }
                let items = match result {
                    Ok(CompletionResponse::Array(items)) => items,
                    Ok(CompletionResponse::List(items)) => items.items,
                    Err(error) => {
                        log::debug!("lsp: completion: {error:#}");
                        popup.dismiss_completions(cx);
                        return;
                    }
                };
                if items.is_empty() {
                    popup.dismiss_completions(cx);
                    return;
                }
                popup.list.update(cx, |list, cx| {
                    list.delegate_mut().items = items
                        .into_iter()
                        .map(|item| Candidate {
                            item,
                            resolved: false,
                        })
                        .collect();
                    list.set_selected_index(Some(IndexPath::new(0)), window, cx);
                    list.scroll_to_selected_item(window, cx);
                    cx.notify();
                });
                cx.notify();
            });
        }));
    }

    fn navigate(&mut self, down: bool, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.target.is_none() || self.list.read(cx).delegate().items.is_empty() {
            return false;
        }
        self.list.update(cx, |list, cx| {
            let count = list.delegate().items.len();
            let ix = list.selected_index().map_or(0, |ix| ix.row);
            let next = if down {
                (ix + 1) % count
            } else {
                (ix + count - 1) % count
            };
            list.set_selected_index(Some(IndexPath::new(next)), window, cx);
            list.scroll_to_selected_item(window, cx);
            cx.notify();
        });
        cx.notify();
        true
    }

    fn resolve_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let list = self.list.read(cx);
        let ix = self.target.as_ref().and_then(|_| list.selected_index());
        let selection_seq = list.delegate().selection_seq;
        if ix != self.highlighted || selection_seq != self.highlight_seq {
            self.highlighted = ix;
            self.highlight_seq = selection_seq;
            self.resolve_request = None;
            self.accept_pending = false;
        }
        let (Some(ix), Some(target), Some(provider)) =
            (ix, self.target.as_ref(), self.provider.as_ref())
        else {
            return;
        };
        let Some(candidate) = self.list.read(cx).delegate().items.get(ix.row) else {
            return;
        };
        if candidate.resolved || self.resolve_request.is_some() {
            return;
        }
        let item = candidate.item.clone();
        let resolve = provider.resolve_completion(item, &target.text, cx);
        self.resolve_highlight(ix, resolve, window, cx);
    }

    fn resolve_highlight(
        &mut self,
        ix: IndexPath,
        resolve: Task<anyhow::Result<CompletionItem>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(target) = self.target.clone() else {
            return;
        };
        let seq = self.completion_seq;
        let highlight_seq = self.highlight_seq;
        self.resolve_request = Some(cx.spawn_in(window, async move |popup, cx| {
            let result = resolve.await;
            let _ = popup.update_in(cx, |popup, window, cx| {
                if popup.completion_seq != seq
                    || popup.highlight_seq != highlight_seq
                    || popup.list.read(cx).selected_index() != Some(ix)
                    || popup.list.read(cx).delegate().selection_seq != highlight_seq
                {
                    return;
                }
                let Some(input) = popup.input.upgrade() else {
                    return;
                };
                let state = input.read(cx);
                if *state.text() != target.text
                    || state.cursor() != target.cursor
                    || !state.has_selection_focus(window, cx)
                {
                    popup.dismiss_completions(cx);
                    return;
                }
                let item = match result {
                    Ok(item) => item,
                    Err(error) => {
                        log::debug!("lsp: resolve completion: {error:#}");
                        popup.dismiss_completions(cx);
                        return;
                    }
                };
                popup.list.update(cx, |list, cx| {
                    if let Some(candidate) = list.delegate_mut().items.get_mut(ix.row) {
                        candidate.item = item;
                        candidate.resolved = true;
                    }
                    cx.notify();
                });
                // Do not cancel the task currently delivering this result when
                // acceptance closes the popup. Commands have their own lifetime.
                if let Some(task) = popup.resolve_request.take() {
                    task.detach();
                }
                if popup.accept_pending {
                    popup.accept(window, cx);
                }
                cx.notify();
            });
        }));
    }

    fn accept(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(target) = self.target.clone() else {
            return false;
        };
        let Some(input) = self.input.upgrade() else {
            return false;
        };
        let state = input.read(cx);
        if *state.text() != target.text
            || state.cursor() != target.cursor
            || !state.has_selection_focus(window, cx)
        {
            self.dismiss_completions(cx);
            return false;
        }
        // A pointer confirmation can precede the list observer. Synchronize the
        // highlight now so a pending Enter can never commit the previous row.
        self.resolve_selection(window, cx);
        let list = self.list.read(cx);
        let Some(candidate) = list
            .selected_index()
            .and_then(|ix| list.delegate().items.get(ix.row))
        else {
            return false;
        };
        if !candidate.resolved {
            self.accept_pending = self.resolve_request.is_some();
            return self.accept_pending;
        }
        let item = candidate.item.clone();
        let command = item.command.clone();
        let provider = self.provider.clone();
        if command.is_some() && provider.is_none() {
            return false;
        }
        let Some((range, replacement, caret)) =
            completion_edit(&target.text, target.start..target.cursor, item)
        else {
            self.dismiss_completions(cx);
            return true;
        };
        self.dismiss_completions(cx);
        input.update(cx, |state, cx| {
            state.set_selected_range(range, cx);
            state.replace(replacement, window, cx);
            state.set_selected_range(caret..caret, cx);
            state.focus(window, cx);
            self.suppressed_text = Some(state.text().clone());
        });
        if let (Some(command), Some(provider)) = (command, provider) {
            let text = input.read(cx).text().clone();
            let task = provider.execute_completion_command(command, &text, cx);
            cx.spawn(async move |_, _| {
                if let Err(error) = task.await {
                    log::debug!("lsp: completion command: {error:#}");
                }
            })
            .detach();
        }
        true
    }

    pub(crate) fn decorate(root: Stateful<Div>, popups: &Entity<Self>) -> Stateful<Div> {
        use gpui_kit::component::input::{
            Enter, Escape, IndentInline, MoveDown, MoveUp, ShowCompletions,
        };
        macro_rules! capture {
            ($root:expr, $action:ty, $call:expr) => {{
                let popup = popups.downgrade();
                $root.capture_action(move |_: &$action, window, cx| {
                    if popup
                        .update(cx, |popup, cx| $call(popup, window, cx))
                        .unwrap_or(false)
                    {
                        cx.stop_propagation();
                    }
                })
            }};
        }
        let popup = popups.downgrade();
        let root = root.capture_action(move |action: &Enter, window, cx| {
            if Enter::is_primary(action)
                && popup
                    .update(cx, |popup, cx| popup.accept(window, cx))
                    .unwrap_or(false)
            {
                cx.stop_propagation();
            }
        });
        let root = capture!(root, IndentInline, |popup: &mut Self, window, cx| popup
            .accept(window, cx));
        let root = capture!(root, MoveUp, |popup: &mut Self, window, cx| popup
            .navigate(false, window, cx));
        let root = capture!(root, MoveDown, |popup: &mut Self, window, cx| popup
            .navigate(true, window, cx));
        let root = capture!(
            root,
            Escape,
            |popup: &mut Self, window: &mut Window, cx: &mut Context<Self>| {
                if !popup.dismiss_top(cx) {
                    return false;
                }
                if let Some(input) = popup.input.upgrade() {
                    input.update(cx, |state, cx| state.focus(window, cx));
                }
                true
            }
        );
        capture!(root, ShowCompletions, |popup: &mut Self, window, cx| {
            popup.request_completions(None, window, cx);
            true
        })
    }
}

// Compose the main insertion and non-overlapping import edits into one atomic
// editor replacement. Coordinates all refer to the response's source snapshot.
fn completion_edit(
    text: &Rope,
    fallback: Range<usize>,
    item: CompletionItem,
) -> Option<(Range<usize>, String, usize)> {
    let main = match item.text_edit {
        Some(CompletionTextEdit::Edit(edit)) => edit,
        Some(CompletionTextEdit::InsertAndReplace(edit)) => TextEdit {
            range: edit.replace,
            new_text: edit.new_text,
        },
        None => TextEdit {
            range: lsp_types::Range::new(
                text.offset_to_position(fallback.start),
                text.offset_to_position(fallback.end),
            ),
            new_text: item.insert_text.unwrap_or(item.label),
        },
    };
    let main_range =
        text.position_to_offset(&main.range.start)..text.position_to_offset(&main.range.end);
    let mut edits: Vec<_> = item
        .additional_text_edits
        .unwrap_or_default()
        .into_iter()
        .map(|edit| {
            (
                text.position_to_offset(&edit.range.start)
                    ..text.position_to_offset(&edit.range.end),
                edit.new_text,
                false,
            )
        })
        .collect();
    edits.push((main_range, main.new_text, true));
    if edits.iter().any(|(range, _, _)| range.start > range.end) {
        return None;
    }
    edits.sort_by_key(|(range, _, _)| (range.start, range.end));
    if edits
        .windows(2)
        .any(|edits| edits[0].0.end > edits[1].0.start)
    {
        return None;
    }
    let range = edits.first()?.0.start..edits.last()?.0.end;
    let mut replacement = String::new();
    let mut at = range.start;
    let mut caret = range.start;
    for (edit, inserted, main) in edits {
        replacement.push_str(&text.slice(at..edit.start).to_string());
        replacement.push_str(&inserted);
        if main {
            caret = range.start + replacement.len();
        }
        at = edit.end;
    }
    Some((range, replacement, caret))
}

impl Render for EditorPopups {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(input) = self.input.upgrade() else {
            return gpui_kit::Empty.into_any_element();
        };
        if !input.read(cx).has_selection_focus(window, cx) {
            return gpui_kit::Empty.into_any_element();
        }
        let completion = self.target.is_some() && !self.list.read(cx).delegate().items.is_empty();
        let documentation = if completion {
            let list = self.list.read(cx);
            list.selected_index()
                .and_then(|ix| list.delegate().items.get(ix.row))
                .and_then(|candidate| candidate.item.documentation.as_ref())
                .map(|doc| match doc {
                    lsp_types::Documentation::String(text) => text.clone(),
                    lsp_types::Documentation::MarkupContent(content) => content.value.clone(),
                })
        } else {
            self.signature.clone()
        };
        if !completion && documentation.is_none() {
            return gpui_kit::Empty.into_any_element();
        }
        let popup = cx.weak_entity();
        let card = v_flex()
            .id("editor-lsp-popup")
            .occlude()
            .w_80()
            .max_h_80()
            .overflow_hidden()
            .bg(cx.theme().popover)
            .text_color(cx.theme().popover_foreground)
            .border_1()
            .border_color(cx.theme().border)
            .rounded(cx.theme().radius)
            .shadow_lg()
            .when(completion, |card| {
                card.child(List::new(&self.list).small().max_h_64())
            })
            .when_some(documentation, |card, text| {
                card.child(
                    div().p_2().child(
                        TextView::markdown("editor-lsp-documentation", text)
                            .style(crate::ui::theme::markdown_style(cx)),
                    ),
                )
            })
            .on_mouse_down_out(move |_, _, cx| {
                let _ = popup.update(cx, |popup, cx| popup.dismiss_all(cx));
            });
        deferred(PopupAtCaret {
            input: self.input.clone(),
            content: Some(card.into_any_element()),
            laid_out: None,
        })
        .with_priority(2)
        .into_any_element()
    }
}

// Public current-frame geometry is read after the editor's prepaint, not from
// last-frame bounds. This is application placement, not editor internals.
struct PopupAtCaret {
    input: WeakEntity<EditorState>,
    content: Option<AnyElement>,
    laid_out: Option<AnyElement>,
}
impl IntoElement for PopupAtCaret {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
impl Element for PopupAtCaret {
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
        (window.request_layout(Style::default(), [], cx), ())
    }
    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let Some(input) = self.input.upgrade() else {
            return;
        };
        let Some(geometry) = input.read(cx).prepaint_caret_geometry() else {
            return;
        };
        // Published caret x already includes the input origin and horizontal
        // scroll. Only vertical scroll remains outside cursor_bounds.
        let caret = geometry.cursor_bounds();
        let anchor = point(
            caret.left(),
            caret.top() + geometry.scroll_offset().y + geometry.line_height(),
        );
        let Some(content) = self.content.take() else {
            return;
        };
        let mut popup = anchored()
            .position(anchor)
            .snap_to_window_with_margin(window.rem_size() * 0.5)
            .child(content)
            .into_any_element();
        let viewport = window.viewport_size();
        popup.prepaint_as_root(
            point(px(0.), px(0.)),
            size(
                AvailableSpace::Definite(viewport.width),
                AvailableSpace::Definite(viewport.height),
            ),
            window,
            cx,
        );
        self.laid_out = Some(popup);
    }
    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        if let Some(popup) = &mut self.laid_out {
            popup.paint(window, cx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::component::Root;
    use gpui_kit::{TestAppContext, WindowHandle};
    use lsp_types::{Position, Range as LspRange};
    use serde_json::{Value, json};
    use std::io::{Read as _, Write as _};

    // Exercise the production provider and client over framed pipes. Responses
    // are controlled independently of requests, including replies after a row
    // change; this is not an echo provider or a replacement popup implementation.
    struct PopupServer {
        requests: smol::channel::Receiver<Value>,
        responses: std::io::PipeWriter,
    }

    impl PopupServer {
        async fn request(&self, method: &str) -> Value {
            let request = smol::future::or(
                async {
                    loop {
                        let message = self.requests.recv().await.expect("client closed");
                        if message.get("id").is_some() {
                            return message;
                        }
                    }
                },
                async {
                    smol::Timer::after(std::time::Duration::from_secs(5)).await;
                    panic!("no {method} request");
                },
            )
            .await;
            assert_eq!(request["method"], method);
            request
        }

        fn reply(&mut self, request: &Value, result: Value) {
            let bytes = super::super::rpc::encode(&json!({
                "jsonrpc": "2.0", "id": request["id"], "result": result,
            }));
            self.responses.write_all(&bytes).unwrap();
            self.responses.flush().unwrap();
        }
    }

    struct PopupSetup {
        window: WindowHandle<Root>,
        input: Entity<EditorState>,
        popup: Entity<EditorPopups>,
        server: PopupServer,
    }

    struct PopupHost {
        input: Entity<EditorState>,
        popup: Entity<EditorPopups>,
    }

    impl Render for PopupHost {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            EditorPopups::decorate(div().id("popup-test-editor").size_full(), &self.popup)
                .child(gpui_kit::component::input::Editor::new(&self.input).size_full())
                .child(self.popup.clone())
        }
    }

    fn popup_setup(cx: &mut TestAppContext) -> PopupSetup {
        popup_setup_with_text(cx, "let 🎉 = fo;\n")
    }

    fn popup_setup_with_text(cx: &mut TestAppContext, text: &str) -> PopupSetup {
        use super::super::{Doc, Encoding, LspClient, LspStore, Phase, Server, ServerKey};

        crate::core::config::pin_test_config_dir();
        cx.executor().allow_parking();
        cx.update(|cx| {
            gpui_kit::component::init(cx);
            init(cx);
            cx.set_global(crate::core::config::Config::default());
        });
        // WindowHandle::update leases its root entity. Keep the editor below
        // Root so popup callbacks can read/update it without re-entering that
        // lease; this is also the production window ownership arrangement.
        let window = cx.add_window(|window, cx| {
            let input = cx.new(|cx| EditorState::new(window, cx).default_value(text.to_owned()));
            input.update(cx, |state, cx| {
                let cursor = state.text().len() - 2;
                state.set_selected_range(cursor..cursor, cx);
                state.focus(window, cx);
            });
            let popup = cx.new(|cx| EditorPopups::new(&input, window, cx));
            let host = cx.new(|_| PopupHost { input, popup });
            Root::new(host, window, cx)
        });
        let (input, popup) = window
            .update(cx, |root, _, cx| {
                let host = root
                    .view()
                    .clone()
                    .downcast::<PopupHost>()
                    .ok()
                    .expect("window root wraps the production editor and popup");
                let host = host.read(cx);
                (host.input.clone(), host.popup.clone())
            })
            .unwrap();
        let (client_reads, responses) = std::io::pipe().unwrap();
        let (mut requests, client_writes) = std::io::pipe().unwrap();
        let (client, _events) =
            LspClient::from_streams("popup-test", client_reads, client_writes).unwrap();
        let (send, receive) = smol::channel::unbounded();
        std::thread::spawn(move || {
            let mut frames = super::super::rpc::FrameReader::default();
            let mut chunk = [0u8; 4096];
            while let Ok(n) = requests.read(&mut chunk) {
                if n == 0 {
                    break;
                }
                frames.push(&chunk[..n]);
                while let Some(body) = frames.next_frame() {
                    let message: Value = serde_json::from_slice(&body).unwrap();
                    if send.send_blocking(message).is_err() {
                        return;
                    }
                }
            }
        });
        client.finish_initialization();
        let root = super::super::test_path("/tmp/lyrel-popup-test");
        let path = root.join("lib.rs");
        let key = ServerKey {
            name: "rust-analyzer",
            root,
        };
        let buffer = crate::ui::code_editor::BufferId::new();
        cx.update(|cx| {
            LspStore::ensure(cx);
            let text = input.read(cx).text().clone();
            let store = cx.global_mut::<LspStore>();
            store.servers.insert(
                key.clone(),
                Server {
                    spec: super::super::servers::spec_for_language("rust").unwrap(),
                    label: "popup-test".into(),
                    generation: 1,
                    phase: Phase::Running,
                    client: Some(client),
                    caps: Rc::new(lsp_types::ServerCapabilities {
                        completion_provider: Some(lsp_types::CompletionOptions {
                            resolve_provider: Some(true),
                            trigger_characters: Some(vec![".".into()]),
                            ..Default::default()
                        }),
                        ..Default::default()
                    }),
                    encoding: Encoding::Utf16,
                    crashes: Vec::new(),
                    idle_seq: 0,
                    _tasks: Vec::new(),
                },
            );
            store.docs.insert(
                path.clone(),
                Doc {
                    uri: super::super::path_to_uri(&path).unwrap(),
                    language_id: "rust",
                    server: key,
                    version: 0,
                    owner: input.downgrade(),
                    popups: popup.downgrade(),
                    owner_id: buffer,
                    app_id: input.entity_id(),
                    app: WeakEntity::new_invalid(),
                    window: Some(window.into()),
                    sent: Some(text),
                    dirty: false,
                    change_seq: 0,
                    symbols_seq: 0,
                    diagnostics: Vec::new(),
                    problems: (0, 0),
                },
            );
            super::super::providers::install(
                buffer,
                &input,
                &popup,
                &path,
                WeakEntity::new_invalid(),
                cx,
            );
        });
        PopupSetup {
            window,
            input,
            popup,
            server: PopupServer {
                requests: receive,
                responses,
            },
        }
    }

    async fn show_candidates(s: &mut PopupSetup, cx: &mut TestAppContext) -> Value {
        s.window
            .update(cx, |_, window, cx| {
                s.popup
                    .update(cx, |popup, cx| popup.request_completions(None, window, cx));
            })
            .unwrap();
        let request = s.server.request("textDocument/completion").await;
        s.server.reply(
            &request,
            json!([
                { "label": "Alpha", "filterText": "foAlpha", "textEdit": {
                    "range": { "start": { "line": 0, "character": 9 },
                        "end": { "line": 0, "character": 11 } },
                    "newText": "Alpha"
                } },
                { "label": "Beta", "filterText": "foBeta", "textEdit": {
                    "range": { "start": { "line": 0, "character": 9 },
                        "end": { "line": 0, "character": 11 } },
                    "newText": "Beta"
                } }
            ]),
        );
        // The initial highlight must resolve without Enter or a render request.
        s.server.request("completionItem/resolve").await
    }

    async fn resolved(s: &PopupSetup, row: usize, cx: &mut TestAppContext) {
        cx.condition(&s.popup, |popup, cx| {
            popup
                .list
                .read(cx)
                .delegate()
                .items
                .get(row)
                .is_some_and(|item| item.resolved)
        })
        .await;
    }

    #[gpui_kit::test]
    fn completion_shortcut_does_not_capture_plain_input_or_terminal_context(
        cx: &mut TestAppContext,
    ) {
        use gpui_kit::component::input::{Input, InputState, ShowCompletions};
        use gpui_kit::test::TestWindowExt as _;

        struct ShortcutHost {
            input: Entity<InputState>,
            terminal_focus: gpui_kit::FocusHandle,
            terminal: bool,
            completion_actions: usize,
            raw_control_spaces: usize,
        }
        impl Render for ShortcutHost {
            fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
                div()
                    .id("completion-shortcut-scope")
                    .size_full()
                    .on_action(cx.listener(|this, _: &ShowCompletions, _, _| {
                        this.completion_actions += 1;
                    }))
                    .on_key_down(cx.listener(|this, event: &gpui_kit::KeyDownEvent, _, _| {
                        if this.terminal
                            && event.keystroke.key == "space"
                            && event.keystroke.modifiers.control
                        {
                            this.raw_control_spaces += 1;
                        }
                    }))
                    .when(self.terminal, |root| {
                        root.track_focus(&self.terminal_focus)
                            .key_context("Terminal")
                    })
                    .when(!self.terminal, |root| root.child(Input::new(&self.input)))
            }
        }

        cx.update(|cx| {
            gpui_kit::component::init(cx);
            init(cx);
        });
        let window = cx.add_window(|window, cx| {
            let input = cx.new(|cx| InputState::new(window, cx));
            let host = cx.new(|cx| ShortcutHost {
                input,
                terminal_focus: cx.focus_handle(),
                terminal: false,
                completion_actions: 0,
                raw_control_spaces: 0,
            });
            Root::new(host, window, cx)
        });
        let host = window
            .update(cx, |root, _, _| {
                root.view().clone().downcast::<ShortcutHost>().ok().unwrap()
            })
            .unwrap();
        for terminal in [false, true] {
            cx.update_window(window.into(), |_, window, cx| {
                host.update(cx, |host, cx| {
                    host.terminal = terminal;
                    if terminal {
                        host.terminal_focus.focus(window, cx);
                    } else {
                        host.input.update(cx, |state, cx| state.focus(window, cx));
                    }
                    cx.notify();
                });
                window.press("ctrl-space", cx);
            })
            .unwrap();
        }
        cx.update(|cx| {
            let host = host.read(cx);
            assert_eq!(host.completion_actions, 0);
            assert_eq!(host.raw_control_spaces, 1);
        });
    }

    #[gpui_kit::test]
    async fn typing_refines_server_candidates_and_enter_accepts_only_the_current_prefix(
        cx: &mut TestAppContext,
    ) {
        use gpui_kit::test::TestWindowExt as _;

        let mut s = popup_setup_with_text(cx, "fn main(){let message=\"中文🎉\"; value;\n");
        let candidates = json!({ "isIncomplete": false, "items": [
            { "label": "split_off", "sortText": "0", "insertText": "split_off()" },
            { "label": "split", "sortText": "1" },
            { "label": "len()", "filterText": "len", "sortText": "9",
                "insertText": "len($0)", "insertTextFormat": 2 }
        ] });
        cx.update_window(s.window.into(), |_, window, cx| window.input(".", cx))
            .unwrap();
        let initial = s.server.request("textDocument/completion").await;
        assert_eq!(initial["params"]["context"]["triggerKind"], 2);
        assert_eq!(initial["params"]["context"]["triggerCharacter"], ".");
        s.server.reply(&initial, candidates.clone());
        let initial_resolve = s.server.request("completionItem/resolve").await;
        assert_eq!(initial_resolve["params"]["label"], "split_off");
        s.server.reply(
            &initial_resolve,
            json!({ "label": "split_off", "documentation": "split docs" }),
        );
        resolved(&s, 0, cx).await;

        // Keep older tasks alive: this tests revision rejection rather than
        // merely relying on cancellation to hide an out-of-order response.
        let mut old_requests = Vec::new();
        let mut old_tasks = Vec::new();
        for ch in ["l", "e"] {
            cx.update_window(s.window.into(), |_, window, cx| window.input(ch, cx))
                .unwrap();
            old_requests.push(s.server.request("textDocument/completion").await);
            old_tasks.push(cx.update(|cx| {
                s.popup.update(cx, |popup, cx| {
                    assert!(popup.list.read(cx).delegate().items.is_empty());
                    assert_eq!(popup.list.read(cx).selected_index(), None);
                    popup.request.take().expect("pending prefix request")
                })
            }));
        }
        cx.update_window(s.window.into(), |_, window, cx| window.input("n", cx))
            .unwrap();
        let latest = s.server.request("textDocument/completion").await;
        cx.update(|cx| {
            let state = s.input.read(cx);
            let position = super::super::convert::offset_to_lsp(
                state.text(),
                state.cursor(),
                super::super::Encoding::Utf16,
            );
            assert_eq!(
                latest["params"]["position"],
                serde_json::to_value(position).unwrap()
            );
        });
        s.server.reply(&latest, candidates.clone());
        let resolve = s.server.request("completionItem/resolve").await;
        assert_eq!(resolve["params"]["label"], "len()");
        assert_eq!(resolve["params"]["insertText"], "len($0)");
        s.server.reply(
            &resolve,
            json!({ "label": "len()", "documentation": "current len docs" }),
        );
        resolved(&s, 0, cx).await;
        for old in old_requests {
            s.server.reply(&old, candidates.clone());
        }
        for task in old_tasks {
            task.await;
        }
        cx.update(|cx| {
            let popup = s.popup.read(cx);
            let list = popup.list.read(cx);
            assert_eq!(list.delegate().items.len(), 1);
            assert_eq!(list.delegate().items[0].item.label, "len()");
            assert!(list.delegate().items[0].resolved);
        });

        // Explicit invocation uses the same current prefix, not the old dot
        // response, and keeps INVOKED distinct from automatic trigger chars.
        cx.update_window(s.window.into(), |_, window, cx| {
            window.press("ctrl-space", cx)
        })
        .unwrap();
        let explicit = s.server.request("textDocument/completion").await;
        assert_eq!(explicit["params"]["context"]["triggerKind"], 1);
        s.server.reply(&explicit, candidates);
        let resolve = s.server.request("completionItem/resolve").await;
        assert_eq!(resolve["params"]["label"], "len()");
        s.server.reply(
            &resolve,
            json!({ "label": "len()", "documentation": "explicit len docs" }),
        );
        resolved(&s, 0, cx).await;
        cx.update_window(s.window.into(), |_, window, cx| window.press("enter", cx))
            .unwrap();
        cx.update(|cx| {
            let state = s.input.read(cx);
            assert_eq!(
                state.text().to_string(),
                "fn main(){let message=\"中文🎉\"; value.len();\n"
            );
            assert_eq!(state.text().slice(state.cursor()..).to_string(), ";\n");
            assert!(s.popup.read(cx).target.is_none());
        });
    }

    #[gpui_kit::test]
    async fn newer_explicit_request_wins_even_when_text_and_cursor_are_unchanged(
        cx: &mut TestAppContext,
    ) {
        let mut s = popup_setup(cx);
        s.window
            .update(cx, |_, window, cx| {
                s.popup
                    .update(cx, |popup, cx| popup.request_completions(None, window, cx));
            })
            .unwrap();
        let old = s.server.request("textDocument/completion").await;
        let old_task = cx.update(|cx| s.popup.update(cx, |popup, _| popup.request.take().unwrap()));
        s.window
            .update(cx, |_, window, cx| {
                s.popup
                    .update(cx, |popup, cx| popup.request_completions(None, window, cx));
            })
            .unwrap();
        let new = s.server.request("textDocument/completion").await;
        s.server.reply(&new, json!([{ "label": "foo_current" }]));
        let resolve = s.server.request("completionItem/resolve").await;
        assert_eq!(resolve["params"]["label"], "foo_current");
        s.server.reply(
            &resolve,
            json!({ "label": "foo_current", "documentation": "current docs" }),
        );
        resolved(&s, 0, cx).await;
        s.server.reply(&old, json!([{ "label": "foo_obsolete" }]));
        old_task.await;
        cx.update(|cx| {
            let popup = s.popup.read(cx);
            let list = popup.list.read(cx);
            assert_eq!(list.delegate().items.len(), 1);
            assert_eq!(list.delegate().items[0].item.label, "foo_current");
            assert_eq!(
                list.delegate().items[0].item.documentation,
                Some(lsp_types::Documentation::String("current docs".into()))
            );
        });
    }

    #[gpui_kit::test]
    async fn backspacing_to_empty_prefix_refreshes_then_tab_accepts_the_selected_item(
        cx: &mut TestAppContext,
    ) {
        use gpui_kit::test::TestWindowExt as _;

        let mut s = popup_setup_with_text(cx, "中🎉 value.len;\n");
        let candidates = json!([
            { "label": "split_off", "sortText": "0", "insertText": "split_off()" },
            { "label": "len()", "filterText": "len", "sortText": "9", "insertText": "len()" }
        ]);
        let mut requests = Vec::new();
        for _ in 0..3 {
            cx.update_window(s.window.into(), |_, window, cx| {
                window.press("backspace", cx)
            })
            .unwrap();
            requests.push(s.server.request("textDocument/completion").await);
        }
        let empty = requests.last().unwrap();
        assert_eq!(empty["params"]["context"]["triggerKind"], 2);
        for request in requests {
            s.server.reply(&request, candidates.clone());
        }
        let resolve = s.server.request("completionItem/resolve").await;
        assert_eq!(resolve["params"]["label"], "split_off");
        s.server.reply(&resolve, json!({ "label": "split_off" }));
        resolved(&s, 0, cx).await;
        cx.update(|cx| {
            let popup = s.popup.read(cx);
            assert_eq!(popup.list.read(cx).delegate().items.len(), 2);
        });
        cx.update_window(s.window.into(), |_, window, cx| window.press("down", cx))
            .unwrap();
        let resolve = s.server.request("completionItem/resolve").await;
        assert_eq!(resolve["params"]["label"], "len()");
        s.server.reply(&resolve, json!({ "label": "len()" }));
        resolved(&s, 1, cx).await;
        cx.update_window(s.window.into(), |_, window, cx| window.press("tab", cx))
            .unwrap();
        cx.update(|cx| assert_eq!(s.input.read(cx).text().to_string(), "中🎉 value.len();\n"));
    }

    #[gpui_kit::test]
    async fn tab_accepts_the_filtered_current_item_instead_of_indenting(cx: &mut TestAppContext) {
        use gpui_kit::test::TestWindowExt as _;

        let mut s = popup_setup(cx);
        let resolve = show_candidates(&mut s, cx).await;
        s.server.reply(&resolve, json!({ "label": "Alpha" }));
        resolved(&s, 0, cx).await;
        cx.update_window(s.window.into(), |_, window, cx| window.press("tab", cx))
            .unwrap();
        cx.update(|cx| {
            assert_eq!(s.input.read(cx).text().to_string(), "let 🎉 = Alpha;\n");
            assert!(s.popup.read(cx).target.is_none());
        });
    }

    #[gpui_kit::test]
    async fn highlighting_resolves_then_accepts_cached_unicode_edits_and_command(
        cx: &mut TestAppContext,
    ) {
        let mut s = popup_setup(cx);
        let first = show_candidates(&mut s, cx).await;
        assert_eq!(first["params"]["label"], "Alpha");
        s.server.reply(
            &first,
            json!({
                "label": "Alpha", "detail": "struct Alpha", "documentation": "Alpha docs",
                "additionalTextEdits": [{ "range": {
                    "start": { "line": 0, "character": 0 },
                    "end": { "line": 0, "character": 0 }
                }, "newText": "use library::Alpha;\n" }],
                "command": { "title": "Finish Alpha", "command": "finishAlpha", "arguments": [7] }
            }),
        );
        resolved(&s, 0, cx).await;
        cx.update(|cx| {
            let popup = s.popup.read(cx);
            let list = popup.list.read(cx);
            let item = &list.delegate().items[0].item;
            assert_eq!(item.detail.as_deref(), Some("struct Alpha"));
            assert_eq!(
                item.documentation,
                Some(lsp_types::Documentation::String("Alpha docs".into()))
            );
            assert_eq!(s.input.read(cx).text().to_string(), "let 🎉 = fo;\n");
        });
        s.window
            .update(cx, |_, window, cx| {
                s.popup
                    .update(cx, |popup, cx| assert!(popup.navigate(true, window, cx)));
            })
            .unwrap();
        let second = s.server.request("completionItem/resolve").await;
        assert_eq!(second["params"]["label"], "Beta");
        s.server.reply(
            &second,
            json!({ "label": "Beta", "documentation": "Beta docs" }),
        );
        resolved(&s, 1, cx).await;
        s.window
            .update(cx, |_, window, cx| {
                s.popup.update(cx, |popup, cx| {
                    assert!(popup.navigate(false, window, cx));
                    assert!(popup.accept(window, cx));
                });
            })
            .unwrap();
        // A cached item must be accepted without another resolve/conversion.
        let command = s.server.request("workspace/executeCommand").await;
        assert_eq!(command["params"]["command"], "finishAlpha");
        assert_eq!(command["params"]["arguments"], json!([7]));
        s.server.reply(&command, Value::Null);
        cx.update(|cx| {
            let input = s.input.read(cx);
            let text = input.text().to_string();
            assert_eq!(text, "use library::Alpha;\nlet 🎉 = Alpha;\n");
            assert_eq!(&text[input.cursor()..], ";\n");
        });
    }

    #[gpui_kit::test]
    async fn a_late_highlight_reply_cannot_display_or_accept_the_previous_candidate(
        cx: &mut TestAppContext,
    ) {
        let mut s = popup_setup(cx);
        let first = show_candidates(&mut s, cx).await;
        s.window
            .update(cx, |_, window, cx| {
                s.popup.update(cx, |popup, cx| {
                    assert!(popup.accept(window, cx));
                    // Model an already-delivering response that cancellation cannot
                    // stop, rather than proving only that Task cancellation works.
                    popup.resolve_request.take().unwrap().detach();
                    popup.list.update(cx, |list, cx| {
                        list.set_selected_index(Some(IndexPath::new(1)), window, cx);
                    });
                });
            })
            .unwrap();
        let second = s.server.request("completionItem/resolve").await;
        s.server.reply(
            &first,
            json!({
                "label": "Alpha", "documentation": "stale Alpha docs",
                "additionalTextEdits": [{ "range": {
                    "start": { "line": 0, "character": 0 },
                    "end": { "line": 0, "character": 0 }
                }, "newText": "WRONG\n" }]
            }),
        );
        s.server.reply(
            &second,
            json!({ "label": "Beta", "detail": "struct Beta",
            "documentation": "Beta docs" }),
        );
        resolved(&s, 1, cx).await;
        cx.update(|cx| {
            let popup = s.popup.read(cx);
            let list = popup.list.read(cx);
            assert_eq!(list.selected_index(), Some(IndexPath::new(1)));
            assert!(!list.delegate().items[0].resolved);
            assert_eq!(
                list.delegate().items[1].item.documentation,
                Some(lsp_types::Documentation::String("Beta docs".into()))
            );
            assert_eq!(s.input.read(cx).text().to_string(), "let 🎉 = fo;\n");
        });
        s.window
            .update(cx, |_, window, cx| {
                s.popup
                    .update(cx, |popup, cx| assert!(popup.accept(window, cx)));
            })
            .unwrap();
        cx.update(|cx| assert_eq!(s.input.read(cx).text().to_string(), "let 🎉 = Beta;\n"));
    }

    #[gpui_kit::test]
    async fn accepting_while_documentation_loads_waits_for_imports_and_command(
        cx: &mut TestAppContext,
    ) {
        let mut s = popup_setup(cx);
        let first = show_candidates(&mut s, cx).await;
        s.window
            .update(cx, |_, window, cx| {
                s.popup
                    .update(cx, |popup, cx| assert!(popup.accept(window, cx)));
            })
            .unwrap();
        cx.update(|cx| assert_eq!(s.input.read(cx).text().to_string(), "let 🎉 = fo;\n"));
        s.server.reply(
            &first,
            json!({
                "label": "Alpha", "documentation": "Loaded with the acceptance edit",
                "additionalTextEdits": [{ "range": {
                    "start": { "line": 0, "character": 0 },
                    "end": { "line": 0, "character": 0 }
                }, "newText": "use library::Alpha;\n" }],
                "command": { "title": "Finish Alpha", "command": "finishAlpha" }
            }),
        );
        let command = s.server.request("workspace/executeCommand").await;
        assert_eq!(command["params"]["command"], "finishAlpha");
        s.server.reply(&command, Value::Null);
        cx.update(|cx| {
            assert_eq!(
                s.input.read(cx).text().to_string(),
                "use library::Alpha;\nlet 🎉 = Alpha;\n"
            );
            assert!(s.popup.read(cx).target.is_none());
        });
    }

    #[test]
    fn accepting_a_completion_and_import_is_one_unicode_safe_edit() {
        let text = Rope::from("// 🎉\nfn main() { Fo; }\n");
        let item = CompletionItem {
            label: "Foo".into(),
            text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                range: LspRange::new(Position::new(1, 12), Position::new(1, 14)),
                new_text: "Foo".into(),
            })),
            additional_text_edits: Some(vec![TextEdit {
                range: LspRange::new(Position::new(0, 0), Position::new(0, 0)),
                new_text: "use library::Foo;\n".into(),
            }]),
            ..Default::default()
        };
        let (range, replacement, caret) = completion_edit(&text, 0..0, item).unwrap();
        let mut actual = text.to_string();
        actual.replace_range(range, &replacement);
        assert_eq!(actual, "use library::Foo;\n// 🎉\nfn main() { Foo; }\n");
        assert!(actual[..caret].ends_with("{ Foo"));
        assert!(actual[caret..].starts_with("; }"));
    }

    #[test]
    fn completion_fallback_replaces_the_typed_prefix_and_refuses_overlapping_imports() {
        let text = Rope::from("中 fo");
        let item = CompletionItem {
            label: "foo".into(),
            insert_text: Some("foo()".into()),
            ..Default::default()
        };
        let (range, replacement, caret) = completion_edit(&text, 4..6, item.clone()).unwrap();
        assert_eq!(range, 4..6);
        assert_eq!(replacement, "foo()");
        assert_eq!(caret, 9);
        let overlapping = CompletionItem {
            additional_text_edits: Some(vec![TextEdit {
                range: LspRange::new(Position::new(0, 2), Position::new(0, 4)),
                new_text: "bad".into(),
            }]),
            ..item
        };
        assert!(completion_edit(&text, 4..6, overlapping).is_none());
    }
}
