//! Explicit document content, not MDX attributes or a notebook property index.
use super::*;
use gpui_kit::component::menu::{ContextMenuExt as _, PopupMenuItem};
use gpui_kit::component::{
    Disableable as _, Sizable as _,
    button::{Button, ButtonVariants as _},
    input::{Input, InputState},
};
use markdown::mdast::Node;

actions!(
    markdown_properties,
    [EditDocumentProperties, AddDocumentProperty, AddDocumentTag]
);

#[derive(Clone)]
pub(super) struct Property {
    line: Range<usize>,
    key: Range<usize>,
    value: Range<usize>,
}

#[derive(Default)]
pub(in super::super) struct Index {
    properties: Vec<Property>,
    tags: Vec<Range<usize>>,
    insertion: Option<usize>,
}

fn valid_key(key: &str) -> bool {
    key.as_bytes().first().is_some_and(u8::is_ascii_alphabetic)
        && key
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.'))
}

fn property(line: &str, base: usize) -> Option<Property> {
    let leading = line.len() - line.trim_start_matches([' ', '\t']).len();
    let mut at = leading;
    let rest = &line[at..];
    if ["- ", "* ", "+ "]
        .iter()
        .any(|prefix| rest.starts_with(prefix))
    {
        at += 2;
    } else {
        let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
        if digits > 0
            && rest
                .as_bytes()
                .get(digits)
                .is_some_and(|c| matches!(c, b'.' | b')'))
            && rest.as_bytes().get(digits + 1) == Some(&b' ')
        {
            at += digits + 2;
        }
    }
    let body = &line[at..];
    let delimiter = body.find("::")?;
    if !valid_key(&body[..delimiter]) {
        return None;
    }
    let raw = &body[delimiter + 2..];
    let value_start = raw.len() - raw.trim_start_matches([' ', '\t']).len();
    let value_end = raw
        .trim_end_matches([' ', '\t', '\r', '\n'])
        .len()
        .max(value_start);
    Some(Property {
        line: base..base + line.len(),
        key: base + at..base + at + delimiter,
        value: base + at + delimiter + 2 + value_start..base + at + delimiter + 2 + value_end,
    })
}

impl Index {
    pub(super) fn reorder_ranges(&self) -> Vec<Range<usize>> {
        let mut runs: Vec<Range<usize>> = Vec::new();
        for property in &self.properties {
            if let Some(run) = runs.last_mut().filter(|run| run.end == property.line.start) {
                run.end = property.line.end;
            } else {
                runs.push(property.line.clone());
            }
        }
        runs
    }
    pub(in super::super) fn from_ast(source: &str, root: &Node) -> Self {
        fn walk(source: &str, node: &Node, index: &mut Index) {
            if matches!(
                node,
                Node::Code(_)
                    | Node::InlineCode(_)
                    | Node::Math(_)
                    | Node::InlineMath(_)
                    | Node::Html(_)
                    | Node::Yaml(_)
                    | Node::Toml(_)
                    | Node::MdxFlowExpression(_)
                    | Node::MdxTextExpression(_)
                    | Node::MdxjsEsm(_)
                    | Node::Link(_)
                    | Node::LinkReference(_)
                    | Node::Image(_)
                    | Node::ImageReference(_)
                    | Node::Definition(_)
            ) {
                return;
            }
            if let Some(position) = node.position() {
                let range = position.start.offset..position.end.offset;
                if matches!(node, Node::Paragraph(_)) {
                    let mut base = source[..range.start].rfind('\n').map_or(0, |at| at + 1);
                    while base < range.end {
                        let end = source[base..]
                            .find('\n')
                            .map_or(source.len(), |at| base + at + 1);
                        if let Some(row) = property(&source[base..end], base) {
                            // Only an AST Text key introduces a property. Code, expressions,
                            // attributes and metadata never acquire extension semantics.
                            let key_is_text = node.children().is_some_and(|children| {
                                children.iter().any(|child| {
                                    matches!(child, Node::Text(_))
                                        && child.position().is_some_and(|p| {
                                            p.start.offset <= row.key.start
                                                && p.end.offset >= row.key.end
                                        })
                                })
                            });
                            if key_is_text {
                                index.properties.push(row);
                            }
                        }
                        base = end;
                    }
                }
                if matches!(node, Node::Text(_)) {
                    let bytes = source.as_bytes();
                    let mut at = range.start;
                    while at < range.end {
                        if bytes[at] == b'[' && source[at..range.end].starts_with("[[") {
                            if let Some(end) = source[at + 2..range.end].find("]]") {
                                at += end + 4;
                                continue;
                            }
                        }
                        if bytes[at] == b'#'
                            && (at == 0
                                || !(bytes[at - 1].is_ascii_alphanumeric()
                                    || matches!(bytes[at - 1], b'_' | b'\\')))
                        {
                            let mut end = at + 1;
                            while end < range.end
                                && (bytes[end].is_ascii_alphanumeric()
                                    || matches!(bytes[end], b'_' | b'-' | b'/'))
                            {
                                end += 1;
                            }
                            if end > at + 1 {
                                index.tags.push(at..end);
                                at = end;
                                continue;
                            }
                        }
                        at += 1;
                    }
                }
            }
            if let Some(children) = node.children() {
                for child in children {
                    walk(source, child, index);
                }
            }
        }
        let mut index = Self::default();
        index.insertion = Some(
            root.children()
                .and_then(|children| {
                    children
                        .iter()
                        .find(|node| !matches!(node, Node::Yaml(_) | Node::Toml(_)))
                })
                .and_then(Node::position)
                .map_or(source.len(), |position| {
                    source[..position.start.offset]
                        .rfind('\n')
                        .map_or(0, |at| at + 1)
                }),
        );
        walk(source, root, &mut index);
        index.properties.sort_by_key(|row| row.key.start);
        index.properties.dedup_by_key(|row| row.key.start);
        index.tags.sort_by_key(|range| range.start);
        index.tags.dedup();
        index
    }
}

pub(super) enum Target {
    Property(Property),
    Tag(Range<usize>),
    NewProperty(usize),
    NewTag(usize),
}
struct Draft {
    target: Target,
    revision: u64,
    key: Entity<InputState>,
    value: Entity<InputState>,
    error: Option<L10nKey>,
    focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

#[derive(Default)]
pub(super) struct State {
    pub(super) open: bool,
    draft: Option<Draft>,
}

impl SourcePresentation {
    pub(super) fn render_property_leaf(
        &self,
        leaf: &super::super::projection::Leaf,
        fallback: AnyElement,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let selection = self.state.selection.range();
        if self.is_composing()
            || (selection.start < selection.end
                && selection.start < leaf.range.end
                && selection.end > leaf.range.start)
        {
            return fallback;
        }
        if !self.live
            || self.projection.revision != self.document.read(cx).revision()
            || !matches!(leaf.kind, Kind::Prose | Kind::List(_) | Kind::Quote(_))
        {
            return fallback;
        }
        let first = self
            .projection
            .properties
            .properties
            .partition_point(|row| row.key.start < leaf.range.start);
        let end = self
            .projection
            .properties
            .properties
            .partition_point(|row| row.key.start < leaf.range.end);
        let rows = &self.projection.properties.properties[first..end];
        if rows.is_empty() {
            return fallback;
        }
        let document = self.document.read(cx);
        let mut at = leaf.range.start;
        for row in rows {
            if row.line.start > at
                && !document
                    .source()
                    .slice(at..row.line.start)
                    .chars()
                    .all(char::is_whitespace)
            {
                return fallback;
            }
            at = row.line.end.min(leaf.range.end);
        }
        if at < leaf.range.end
            && !document
                .source()
                .slice(at..leaf.range.end)
                .chars()
                .all(char::is_whitespace)
        {
            return fallback;
        }
        let mut panel = div()
            .id(("markdown-property-block", leaf.id))
            .v_flex()
            .gap_1()
            .p_2()
            .border_1()
            .border_color(cx.theme().border)
            .rounded(cx.theme().radius)
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());
        for row in rows {
            let edit_key = row.clone();
            let edit_value = row.clone();
            let context_row = row.clone();
            let view = cx.entity().downgrade();
            let revision = self.projection.revision;
            panel = panel.child(
                gpui_kit::component::h_flex()
                    .gap_2()
                    .min_w_0()
                    .child(
                        Button::new(("property-inline-key", row.key.start))
                            .small()
                            .ghost()
                            .label(document.source().slice(row.key.clone()).to_string())
                            .on_click(cx.listener(move |this, _, window, cx| {
                                if this.document.read(cx).revision() == revision {
                                    this.edit_property(
                                        Target::Property(edit_key.clone()),
                                        window,
                                        cx,
                                    );
                                }
                            })),
                    )
                    .child(
                        Button::new(("property-inline-value", row.key.start))
                            .small()
                            .ghost()
                            .label(document.source().slice(row.value.clone()).to_string())
                            .on_click(cx.listener(move |this, _, window, cx| {
                                if this.document.read(cx).revision() == revision {
                                    this.edit_property(
                                        Target::Property(edit_value.clone()),
                                        window,
                                        cx,
                                    );
                                    if let Some(draft) = &this.properties.draft {
                                        draft.value.update(cx, |input, cx| input.focus(window, cx));
                                    }
                                }
                            })),
                    )
                    .context_menu(move |menu, _, _| {
                        let edit_view = view.clone();
                        let delete_view = view.clone();
                        let edit = context_row.clone();
                        let remove = context_row.line.clone();
                        menu.item(PopupMenuItem::new(t(L10nKey::EditorPropertyEdit)).on_click(
                            move |_, window, cx| {
                                edit_view
                                    .update(cx, |this, cx| {
                                        if this.document.read(cx).revision() == revision {
                                            this.edit_property(
                                                Target::Property(edit.clone()),
                                                window,
                                                cx,
                                            );
                                        }
                                    })
                                    .ok();
                            },
                        ))
                        .item(
                            PopupMenuItem::new(t(L10nKey::EditorPropertyRemove)).on_click(
                                move |_, window, cx| {
                                    delete_view
                                        .update(cx, |this, cx| {
                                            if this.document.read(cx).revision() == revision {
                                                this.command(
                                                    Command::BatchReplaceSelected {
                                                        edits: vec![(
                                                            remove.clone(),
                                                            String::new(),
                                                        )],
                                                        selection: Selection::caret(remove.start),
                                                    },
                                                    window,
                                                    cx,
                                                );
                                                this.focus.focus(window, cx);
                                            }
                                        })
                                        .ok();
                                },
                            ),
                        )
                    }),
            );
        }
        panel.into_any_element()
    }
    pub(super) fn delete_document_property_boundary(
        &mut self,
        backward: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let selected = self.state.selection.range();
        if !self.live
            || self.is_composing()
            || selected.start != selected.end
            || self.projection.revision != self.document.read(cx).revision()
        {
            return false;
        }
        let at = selected.start;
        let adjacent = self.projection.properties.properties.iter().any(|row| {
            if backward {
                row.line.end == at
                    && row.line.start < at
                    && self.document.read(cx).source().char_at(at - 1) == Some('\n')
            } else {
                row.line.start == at + 1
            }
        });
        if !adjacent {
            return false;
        }
        self.delete_prose(backward, window, cx);
        let document = self.document.read(cx);
        let line = document
            .source()
            .offset_to_point(self.state.selection.head())
            .row;
        let start = document.source().line_start_offset(line);
        let end = document.source().line_end_offset(line);
        let end = end + usize::from(document.source().char_at(end) == Some('\n'));
        let row = property(&document.source().slice(start..end).to_string(), start);
        if let Some(row) = row {
            self.edit_property(Target::Property(row), window, cx);
        }
        true
    }

    pub(super) fn add_document_property(
        &mut self,
        tag: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.command(Command::FinishComposition, window, cx);
        if self.projection.revision == self.document.read(cx).revision() {
            if let Some(at) = self.projection.properties.insertion {
                self.edit_property(
                    if tag {
                        Target::NewTag(at)
                    } else {
                        Target::NewProperty(at)
                    },
                    window,
                    cx,
                );
                return;
            }
        }
        // Invalid/incomplete syntax has no proven extension insertion point.
        // Keep the original document available rather than inserting into code/metadata.
        self.property_source(self.state.selection.range(), window, cx);
    }

    pub(super) fn enter_document_property(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.live
            || self.is_composing()
            || self.projection.revision != self.document.read(cx).revision()
            || !self.state.selection.range().is_empty()
        {
            return false;
        }
        let at = self.state.selection.head();
        let row = self
            .projection
            .properties
            .properties
            .iter()
            .find(|row| row.line.contains(&at))
            .cloned();
        if let Some(row) = row {
            self.edit_property(Target::Property(row), window, cx);
            true
        } else {
            false
        }
    }

    pub(super) fn open_properties(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.command(Command::FinishComposition, window, cx);
        self.properties.open = !self.properties.open;
        self.properties.draft = None;
        self.focus.focus(window, cx);
        cx.notify();
    }

    pub(super) fn escape_document_properties(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.properties.open {
            return false;
        }
        if self.properties.draft.is_some() {
            self.cancel_property(window, cx);
        } else {
            self.properties.open = false;
            self.focus.focus(window, cx);
            cx.notify();
        }
        true
    }

    pub(super) fn edit_property(
        &mut self,
        target: Target,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.command(Command::FinishComposition, window, cx);
        let at = match &target {
            Target::Property(row) => row.key.start,
            Target::Tag(range) => range.start,
            Target::NewProperty(at) | Target::NewTag(at) => *at,
        };
        self.command(Command::Select(Selection::caret(at)), window, cx);
        let document = self.document.read(cx);
        let (key, value) = match &target {
            Target::Property(row) => (
                document.source().slice(row.key.clone()).to_string(),
                document.source().slice(row.value.clone()).to_string(),
            ),
            Target::Tag(range) => (
                String::new(),
                document
                    .source()
                    .slice(range.start + 1..range.end)
                    .to_string(),
            ),
            _ => (String::new(), String::new()),
        };
        let revision = document.revision();
        let is_tag = matches!(&target, Target::Tag(_) | Target::NewTag(_));
        let key = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t(L10nKey::EditorPropertyKey))
                .default_value(key)
        });
        let value = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t(if is_tag {
                    L10nKey::EditorDocumentTag
                } else {
                    L10nKey::EditorPropertyValue
                }))
                .default_value(value)
        });
        let owner = key.entity_id();
        let mut subscriptions = Vec::new();
        for input in [&key, &value] {
            subscriptions.push(cx.subscribe_in(
                input,
                window,
                move |_this, _, event: &gpui_kit::component::input::InputEvent, window, cx| {
                    if matches!(event, gpui_kit::component::input::InputEvent::Change) {
                        cx.notify();
                    }
                    if matches!(event, gpui_kit::component::input::InputEvent::Blur) {
                        cx.defer_in(window, move |this, window, cx| {
                            if this.properties.draft.as_ref().is_some_and(|draft| {
                                draft.key.entity_id() == owner
                                    && !draft.focus.contains_focused(window, cx)
                            }) {
                                this.commit_property(false, window, cx);
                            }
                        });
                    }
                },
            ));
        }
        let initial = if is_tag { &value } else { &key };
        initial.update(cx, |input, cx| input.focus(window, cx));
        self.properties.open = true;
        self.properties.draft = Some(Draft {
            target,
            revision,
            key,
            value,
            error: None,
            focus: cx.focus_handle(),
            _subscriptions: subscriptions,
        });
        cx.notify();
    }

    fn cancel_property(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.properties.draft = None;
        self.focus.focus(window, cx);
        cx.notify();
    }

    fn apply_property(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.commit_property(true, window, cx);
    }

    fn property_composing(&self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        self.properties.draft.as_ref().is_some_and(|draft| {
            [&draft.key, &draft.value].into_iter().any(|input| {
                input.update(cx, |input, cx| {
                    input.marked_text_range(window, cx).is_some()
                })
            })
        })
    }

    fn property_field_move(&mut self, backward: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.property_composing(window, cx) {
            return;
        }
        let Some(draft) = &self.properties.draft else {
            return;
        };
        let tag = matches!(draft.target, Target::Tag(_) | Target::NewTag(_));
        if !tag
            && ((!backward && draft.key.read(cx).focus_handle(cx).is_focused(window))
                || (backward && draft.value.read(cx).focus_handle(cx).is_focused(window)))
        {
            let input = if backward { &draft.key } else { &draft.value };
            input.update(cx, |input, cx| input.focus(window, cx));
            return;
        }
        if backward
            && self
                .document
                .read(cx)
                .source()
                .offset_to_point(self.state.selection.head())
                .row
                == 0
        {
            return;
        }
        self.apply_property(window, cx);
        if self.properties.draft.is_some() {
            return;
        }
        let document = self.document.read(cx);
        let row = document
            .source()
            .offset_to_point(self.state.selection.head())
            .row;
        let at = if backward {
            if row == 0 {
                return;
            }
            document.source().line_end_offset(row - 1)
        } else {
            let end = document.source().line_end_offset(row);
            (end + usize::from(document.source().char_at(end) == Some('\n')))
                .min(document.source().len())
        };
        self.command(Command::Select(Selection::caret(at)), window, cx);
    }

    fn commit_property(&mut self, return_focus: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(draft) = self.properties.draft.as_mut() else {
            return;
        };
        for input in [&draft.key, &draft.value] {
            if input.update(cx, |input, cx| {
                input.marked_text_range(window, cx).is_some()
            }) {
                return;
            }
        }
        if draft.revision != self.document.read(cx).revision() {
            draft.error = Some(L10nKey::EditorPropertyChanged);
            cx.notify();
            return;
        }
        let key = draft.key.read(cx).value().to_string();
        let mut value = draft.value.read(cx).value().to_string();
        if value.contains(['\r', '\n']) {
            value = value.replace("\r\n", " ").replace(['\r', '\n'], " ");
        }
        let is_tag = matches!(draft.target, Target::Tag(_) | Target::NewTag(_));
        if (is_tag
            && (value.is_empty()
                || !value
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-' | b'/'))))
            || (!is_tag && !valid_key(&key))
        {
            draft.error = Some(L10nKey::EditorPropertyInvalid);
            cx.notify();
            return;
        }
        let document = self.document.read(cx);
        let mut edits = Vec::new();
        let caret;
        match &draft.target {
            Target::Property(row) => {
                caret = row.value.start + key.len() - row.key.len() + value.len();
                if document.source().slice(row.key.clone()).to_string() != key {
                    edits.push((row.key.clone(), key));
                }
                if document.source().slice(row.value.clone()).to_string() != value {
                    edits.push((row.value.clone(), value));
                }
            }
            Target::Tag(range) => {
                caret = range.start + 1 + value.len();
                let text = format!("#{value}");
                if document.source().slice(range.clone()).to_string() != text {
                    edits.push((range.clone(), text));
                }
            }
            Target::NewProperty(at) => {
                let before = if *at > 0
                    && document.source().char_at(document.boundary(at - 1, false)) != Some('\n')
                {
                    "\n"
                } else {
                    ""
                };
                caret = at + before.len() + key.len() + 3 + value.len();
                edits.push((*at..*at, format!("{before}{key}:: {value}\n\n")));
            }
            Target::NewTag(at) => {
                let before = if *at > 0
                    && document.source().char_at(document.boundary(at - 1, false)) != Some('\n')
                {
                    "\n"
                } else {
                    ""
                };
                caret = at + before.len() + 1 + value.len();
                edits.push((*at..*at, format!("{before}#{value}\n\n")));
            }
        }
        self.properties.draft = None;
        if !edits.is_empty() {
            let selection = if return_focus {
                Selection::caret(caret)
            } else {
                let mut effects: smallvec::SmallVec<[SourceChange; 1]> = edits
                    .iter()
                    .map(|(range, text)| SourceChange {
                        range: range.clone(),
                        inserted_len: text.len(),
                    })
                    .collect();
                effects.sort_by_key(|edit| std::cmp::Reverse(edit.range.start));
                Transaction {
                    revision: self.document.read(cx).revision(),
                    origin: self.state.id,
                    edits: effects,
                    snapshot: None,
                }
                .map_selection(self.state.selection)
            };
            self.command(
                Command::BatchReplaceSelected { edits, selection },
                window,
                cx,
            );
        }
        if return_focus {
            self.focus.focus(window, cx);
        }
        cx.notify();
    }

    fn property_source(
        &mut self,
        range: Range<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.properties.draft = None;
        self.properties.open = false;
        if self.live {
            self.toggle_mode(window, cx);
        }
        self.command(
            Command::Select(Selection::new(range.start, range.end)),
            window,
            cx,
        );
        self.focus.focus(window, cx);
    }

    pub(super) fn render_properties(&self, cx: &mut Context<Self>) -> AnyElement {
        let ready = self.projection.revision == self.document.read(cx).revision();
        let mut panel = div()
            .id("markdown-property-panel")
            .v_flex()
            .gap_2()
            .px_3()
            .py_2()
            .max_h(rems(18.))
            .overflow_y_scroll()
            .test_support()
            .role(Role::Group)
            .aria_label(t(L10nKey::EditorDocumentProperties))
            .when_some(self.properties.draft.as_ref(), |panel, draft| {
                panel.track_focus(&draft.focus)
            })
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .capture_action(cx.listener(|this, _: &actions::Undo, window, cx| {
                if this.property_composing(window, cx) {
                    return;
                }
                this.cancel_property(window, cx);
                this.command(Command::Undo, window, cx);
                cx.stop_propagation();
            }))
            .capture_action(cx.listener(|this, _: &actions::Redo, window, cx| {
                if this.property_composing(window, cx) {
                    return;
                }
                this.cancel_property(window, cx);
                this.command(Command::Redo, window, cx);
                cx.stop_propagation();
            }))
            .capture_action(cx.listener(|this, _: &actions::Escape, window, cx| {
                if this.property_composing(window, cx) {
                    return;
                }
                if this.properties.draft.is_some() {
                    this.cancel_property(window, cx);
                } else {
                    this.properties.open = false;
                    this.focus.focus(window, cx);
                    cx.notify();
                }
                cx.stop_propagation();
            }))
            .capture_action(cx.listener(|this, _: &actions::Enter, window, cx| {
                if this.property_composing(window, cx) {
                    return;
                }
                if let Some(draft) = &this.properties.draft {
                    if draft.key.read(cx).focus_handle(cx).is_focused(window) {
                        draft.value.update(cx, |input, cx| input.focus(window, cx));
                    } else {
                        this.apply_property(window, cx);
                    }
                    cx.stop_propagation();
                }
            }))
            .capture_action(cx.listener(|this, _: &actions::MoveUp, window, cx| {
                if this.properties.draft.is_some() && !this.property_composing(window, cx) {
                    this.property_field_move(true, window, cx);
                    cx.stop_propagation();
                }
            }))
            .capture_action(cx.listener(|this, _: &actions::MoveDown, window, cx| {
                if this.properties.draft.is_some() && !this.property_composing(window, cx) {
                    this.property_field_move(false, window, cx);
                    cx.stop_propagation();
                }
            }))
            .capture_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                let key = &event.keystroke;
                if let Some(draft) = &this.properties.draft {
                    for input in [&draft.key, &draft.value] {
                        if input.update(cx, |input, cx| {
                            input.marked_text_range(window, cx).is_some()
                        }) {
                            return;
                        }
                    }
                    if key.key == "enter" && draft.key.read(cx).focus_handle(cx).is_focused(window)
                    {
                        draft.value.update(cx, |input, cx| input.focus(window, cx));
                        cx.stop_propagation();
                        return;
                    }
                }
                if key.key == "escape" {
                    if this.properties.draft.is_some() {
                        this.cancel_property(window, cx);
                    } else {
                        this.properties.open = false;
                        this.focus.focus(window, cx);
                        cx.notify();
                    }
                    cx.stop_propagation();
                } else if this.properties.draft.is_some()
                    && key.key == "enter"
                    && !key.modifiers.shift
                {
                    this.apply_property(window, cx);
                    cx.stop_propagation();
                } else if this.properties.draft.is_some()
                    && key.modifiers.secondary(cx)
                    && matches!(key.key.as_str(), "z" | "y")
                {
                    this.cancel_property(window, cx);
                    this.command(
                        if key.key == "y" || key.modifiers.shift {
                            Command::Redo
                        } else {
                            Command::Undo
                        },
                        window,
                        cx,
                    );
                    cx.stop_propagation();
                }
            }));
        if let Some(draft) = &self.properties.draft {
            if !matches!(draft.target, Target::Tag(_) | Target::NewTag(_)) {
                panel = panel
                    .child(div().text_sm().child(t(L10nKey::EditorPropertyKey)))
                    .child(
                        div()
                            .id("markdown-property-key")
                            .test_support()
                            .child(Input::new(&draft.key).small()),
                    );
            }
            panel = panel
                .child(div().text_sm().child(t(
                    if matches!(draft.target, Target::Tag(_) | Target::NewTag(_)) {
                        L10nKey::EditorDocumentTag
                    } else {
                        L10nKey::EditorPropertyValue
                    },
                )))
                .child(
                    div()
                        .id("markdown-property-value")
                        .test_support()
                        .child(Input::new(&draft.value).small()),
                );
            let tag = matches!(draft.target, Target::Tag(_) | Target::NewTag(_));
            let input = if tag { &draft.value } else { &draft.key };
            let query = input.read(cx).value().to_string();
            let candidates = self
                .projection
                .properties
                .tags
                .iter()
                .filter(|_| ready && tag)
                .map(|range| range.start + 1..range.end)
                .chain(
                    self.projection
                        .properties
                        .properties
                        .iter()
                        .filter(|_| ready && !tag)
                        .map(|row| row.key.clone()),
                );
            let mut shown = Vec::<String>::new();
            for range in candidates {
                let candidate = self.document.read(cx).source().slice(range).to_string();
                if candidate == query
                    || !candidate.starts_with(&query)
                    || shown.contains(&candidate)
                {
                    continue;
                }
                let input = input.clone();
                let next = candidate.clone();
                panel = panel.child(
                    Button::new(SharedString::from(format!(
                        "document-property-suggestion:{candidate}"
                    )))
                    .small()
                    .ghost()
                    .label(candidate.clone())
                    .on_click(move |_, window, cx| {
                        input.update(cx, |input, cx| {
                            input.set_value(next.clone(), window, cx);
                            input.focus(window, cx);
                        })
                    }),
                );
                shown.push(candidate);
                if shown.len() == 8 {
                    break;
                }
            }
            return panel
                .when_some(draft.error, |panel, error| {
                    panel.child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().danger)
                            .child(t(error)),
                    )
                })
                .child(
                    gpui_kit::component::h_flex()
                        .gap_2()
                        .child(
                            Button::new("markdown-property-apply")
                                .small()
                                .label(t(L10nKey::EditorPropertyApply))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.apply_property(window, cx)
                                })),
                        )
                        .child(
                            Button::new("markdown-property-cancel")
                                .small()
                                .ghost()
                                .label(t(L10nKey::EditorPropertyCancel))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.cancel_property(window, cx)
                                })),
                        ),
                )
                .into_any_element();
        }
        if ready {
            for row in &self.projection.properties.properties {
                let edit = row.clone();
                let source = row.line.clone();
                let remove = row.line.clone();
                let revision = self.projection.revision;
                let document = self.document.read(cx);
                panel = panel.child(
                    gpui_kit::component::h_flex()
                        .gap_2()
                        .min_w_0()
                        .id(("document-property", row.key.start))
                        .child(
                            Button::new(SharedString::from(format!(
                                "markdown-property-edit-{}",
                                row.key.start
                            )))
                            .small()
                            .ghost()
                            .label(document.source().slice(row.key.clone()).to_string())
                            .on_click(cx.listener(
                                move |this, _, window, cx| {
                                    if this.document.read(cx).revision() == revision {
                                        this.edit_property(
                                            Target::Property(edit.clone()),
                                            window,
                                            cx,
                                        );
                                    }
                                },
                            )),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_sm()
                                .child(document.source().slice(row.value.clone()).to_string()),
                        )
                        .child(
                            Button::new(("property-source", row.key.start))
                                .small()
                                .ghost()
                                .label(t(L10nKey::EditorPropertySource))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    if this.document.read(cx).revision() == revision {
                                        this.property_source(source.clone(), window, cx);
                                    }
                                })),
                        )
                        .child(
                            Button::new(("property-remove", row.key.start))
                                .small()
                                .ghost()
                                .label(t(L10nKey::EditorPropertyRemove))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    if this.document.read(cx).revision() == revision {
                                        this.command(
                                            Command::BatchReplaceSelected {
                                                edits: vec![(remove.clone(), String::new())],
                                                selection: Selection::caret(remove.start),
                                            },
                                            window,
                                            cx,
                                        );
                                        this.focus.focus(window, cx);
                                    }
                                })),
                        ),
                );
            }
            for range in &self.projection.properties.tags {
                let edit = range.clone();
                let remove = range.clone();
                let revision = self.projection.revision;
                panel = panel.child(
                    gpui_kit::component::h_flex()
                        .gap_2()
                        .child(
                            Button::new(("document-tag", range.start))
                                .small()
                                .ghost()
                                .label(
                                    self.document
                                        .read(cx)
                                        .source()
                                        .slice(range.clone())
                                        .to_string(),
                                )
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    if this.document.read(cx).revision() == revision {
                                        this.edit_property(Target::Tag(edit.clone()), window, cx);
                                    }
                                })),
                        )
                        .child(
                            Button::new(SharedString::from(format!(
                                "markdown-tag-remove-{}",
                                range.start
                            )))
                            .small()
                            .ghost()
                            .label(t(L10nKey::EditorPropertyRemove))
                            .on_click(cx.listener(
                                move |this, _, window, cx| {
                                    if this.document.read(cx).revision() == revision {
                                        this.command(
                                            Command::BatchReplaceSelected {
                                                edits: vec![(remove.clone(), String::new())],
                                                selection: Selection::caret(remove.start),
                                            },
                                            window,
                                            cx,
                                        );
                                        this.focus.focus(window, cx);
                                    }
                                },
                            )),
                        ),
                );
            }
        }
        panel
            .child(
                gpui_kit::component::h_flex()
                    .gap_2()
                    .child(
                        Button::new("markdown-property-add")
                            .small()
                            .ghost()
                            .label(t(L10nKey::EditorPropertyAdd))
                            .disabled(!ready || self.projection.properties.insertion.is_none())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.add_document_property(false, window, cx)
                            })),
                    )
                    .child(
                        Button::new("markdown-tag-add")
                            .small()
                            .ghost()
                            .label(t(L10nKey::EditorTagAdd))
                            .disabled(!ready || self.projection.properties.insertion.is_none())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.add_document_property(true, window, cx)
                            })),
                    ),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(t(L10nKey::EditorPropertyHelp)),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use crate::ui::app::test_window::harness_with_tabs;
    use crate::ui::code_editor::test_host::FileHost;
    use gpui_kit::test::TestWindowExt as _;

    #[gpui_kit::test]
    fn properties_consumer_preserves_opaque_source_and_shared_history(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("properties.md");
        let original = "---\r\ntags: [leave, YAML]\ncustom: {x: 1}\r\n---\n\nStatus::  old  \r\nunknown.key:: {opaque: 'x'}\n\nbody #area/sub `#not-a-tag`\r\n";
        std::fs::write(&path, original).unwrap();
        let host = FileHost::new(lyrel_core::host::HostId(820));
        let (app, mut context, _streams) = harness_with_tabs(cx, 1);
        app.update_in(&mut context, |app, window, cx| {
            app.editor_open_on_host(host.clone(), &path, window, cx)
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
            context
                .executor()
                .advance_clock(std::time::Duration::from_millis(100));
            context.run_until_parked();
            if context.update(|window, cx| {
                window.render_frame(cx);
                window.try_find("markdown-properties").is_some()
            }) {
                break;
            }
            assert!(std::time::Instant::now() < deadline);
        }
        let edit_id: gpui_kit::SharedString = format!(
            "markdown-property-edit-{}",
            original.replace("\r\n", "\n").find("Status").unwrap()
        )
        .into();
        context.update(|window, cx| window.click("markdown-properties", cx));
        loop {
            context
                .executor()
                .advance_clock(std::time::Duration::from_millis(100));
            context.run_until_parked();
            if context.update(|window, cx| {
                window.render_frame(cx);
                window.try_find(edit_id.clone()).is_some()
            }) {
                break;
            }
            assert!(std::time::Instant::now() < deadline);
        }
        context.update(|window, cx| {
            window.click(edit_id.clone(), cx);
            window.render_frame(cx);
            window.click("markdown-property-apply", cx);
            window.render_frame(cx);
            window.click(edit_id.clone(), cx);
            window.render_frame(cx);
            window.click("markdown-property-cancel", cx);
        });
        view.read_with(&context, |view, cx| {
            assert_eq!(
                view.document().read(cx).encode().unwrap(),
                original.as_bytes()
            )
        });
        app.read_with(&context, |app, _| {
            assert!(!app.active_buffer().unwrap().dirty)
        });
        assert_eq!(host.writes.load(std::sync::atomic::Ordering::SeqCst), 0);
        context.update(|window, cx| {
            window.render_frame(cx);
            window.click(edit_id.clone(), cx);
            window.render_frame(cx);
            window.click("markdown-property-value", cx);
            window.press("secondary-a", cx);
            window.input("新的 #area/sub", cx);
            window.click("markdown-property-apply", cx);
        });
        let edited = original.replace("  old  ", "  新的 #area/sub  ");
        view.read_with(&context, |view, cx| {
            assert_eq!(
                view.document().read(cx).encode().unwrap(),
                edited.as_bytes()
            )
        });
        context.dispatch_action(gpui_kit::component::input::Undo);
        view.read_with(&context, |view, cx| {
            assert_eq!(
                view.document().read(cx).encode().unwrap(),
                original.as_bytes()
            )
        });
        context.dispatch_action(gpui_kit::component::input::Redo);
        view.read_with(&context, |view, cx| {
            assert_eq!(
                view.document().read(cx).encode().unwrap(),
                edited.as_bytes()
            )
        });
        context.dispatch_action(crate::core::actions::ToggleDocumentPreview);
        context.dispatch_action(crate::core::actions::EditorSave);
        loop {
            context.run_until_parked();
            if app.read_with(&context, |app, _| {
                app.active_buffer()
                    .is_some_and(|buffer| !buffer.dirty && !buffer.saving)
            }) {
                break;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(std::fs::read(&path).unwrap(), edited.as_bytes());
        assert_eq!(host.writes.load(std::sync::atomic::Ordering::SeqCst), 1);
        app.update_in(&mut context, |app, window, cx| {
            app.editor_close_file(0, window, cx);
            app.editor_open_on_host(host.clone(), &path, window, cx);
        });
        let reopened = loop {
            context.run_until_parked();
            if let Some(reopened) = app.read_with(&context, |app, _| {
                app.active_editor_input()
                    .and_then(|input| input.markdown().cloned())
            }) {
                reopened.read_with(&context, |view, cx| {
                    assert_eq!(
                        view.document().read(cx).encode().unwrap(),
                        edited.as_bytes()
                    )
                });
                break reopened;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(5));
        };
        context.update(|window, cx| {
            window.render_frame(cx);
            window.click("markdown-properties", cx);
        });
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            context
                .executor()
                .advance_clock(std::time::Duration::from_millis(100));
            context.run_until_parked();
            if context.update(|window, cx| {
                window.render_frame(cx);
                window.try_find(edit_id.clone()).is_some()
            }) {
                break;
            }
            assert!(std::time::Instant::now() < deadline);
        }
        context.update(|window, cx| {
            window.click("markdown-tag-add", cx);
            window.render_frame(cx);
            window.input("area/new", cx);
            window.click("markdown-property-apply", cx);
        });
        let insertion = edited.find("Status").unwrap();
        let tagged = format!(
            "{}#area/new\n\n{}",
            &edited[..insertion],
            &edited[insertion..]
        );
        reopened.read_with(&context, |view, cx| {
            assert_eq!(
                view.document().read(cx).encode().unwrap(),
                tagged.as_bytes()
            )
        });
        let remove_id: gpui_kit::SharedString = format!(
            "markdown-tag-remove-{}",
            edited.replace("\r\n", "\n").find("Status").unwrap()
        )
        .into();
        loop {
            context
                .executor()
                .advance_clock(std::time::Duration::from_millis(100));
            context.run_until_parked();
            if context.update(|window, cx| {
                window.render_frame(cx);
                window.try_find(remove_id.clone()).is_some()
            }) {
                break;
            }
            assert!(std::time::Instant::now() < deadline);
        }
        context.update(|window, cx| window.click(remove_id.clone(), cx));
        reopened.read_with(&context, |view, cx| {
            assert_eq!(
                view.document().read(cx).encode().unwrap(),
                tagged.replacen("#area/new", "", 1).as_bytes()
            )
        });
        context.dispatch_action(gpui_kit::component::input::Undo);
        reopened.read_with(&context, |view, cx| {
            assert_eq!(
                view.document().read(cx).encode().unwrap(),
                tagged.as_bytes()
            )
        });
    }
}
