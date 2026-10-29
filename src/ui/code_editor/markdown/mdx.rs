//! JSX metadata is derived from the single positioned mdast tree. The small
//! tokenizer below only visits AST-confirmed opening/closing tags; it never
//! parses Markdown, evaluates JavaScript, or serializes decoded attribute values.
use super::projection::{Kind, Leaf, Projection};
use markdown::mdast::{AttributeContent, AttributeValue, Node};
use std::{
    collections::{HashMap, HashSet},
    ops::Range,
    path::Path,
};

pub(super) mod callouts;
pub(super) mod view;
pub(super) mod cards;
mod steps;
pub(super) mod accordions;
pub(super) mod tabs;
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Grammar {
    #[default]
    Markdown,
    Mdx,
}
impl Grammar {
    pub(super) fn for_path(path: Option<&Path>) -> Self {
        if path
            .and_then(Path::extension)
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("mdx"))
        {
            Self::Mdx
        } else {
            Self::Markdown
        }
    }
    pub(super) fn options(self, document_start: bool) -> markdown::ParseOptions {
        let mut options = markdown::ParseOptions::gfm();
        options.constructs.frontmatter = document_start;
        options.constructs.math_flow = true;
        options.constructs.math_text = true;
        if self == Self::Mdx {
            let constructs = &mut options.constructs;
            constructs.html_flow = false;
            constructs.html_text = false;
            constructs.autolink = false;
            constructs.code_indented = false;
            constructs.mdx_jsx_flow = true;
            constructs.mdx_jsx_text = true;
            constructs.mdx_expression_flow = true;
            constructs.mdx_expression_text = true;
            constructs.mdx_esm = true;
            options.mdx_expression_parse = Some(Box::new(|source, _| shape_signal(source)));
            options.mdx_esm_parse = Some(Box::new(shape_signal));
        }
        options
    }
}

/// A lexical shape check, not a JavaScript parser or runtime. Incomplete shapes
/// ask markdown-rs for more source; errors remain editable source in the view.
fn shape_signal(source: &str) -> markdown::MdxSignal {
    if balanced_end(source.as_bytes(), 0, None).is_some()
        && !source.trim_end().ends_with([',', '=', '.', ':'])
    {
        markdown::MdxSignal::Ok
    } else {
        markdown::MdxSignal::Eof(
            "Incomplete JavaScript source".into(),
            Box::new("lyrel".into()),
            Box::new("source-shape".into()),
        )
    }
}

/// Returns the byte after the matching delimiter (or EOF for a whole shape).
fn balanced_end(bytes: &[u8], start: usize, closing: Option<u8>) -> Option<usize> {
    let mut stack = Vec::new();
    if let Some(closing) = closing {
        stack.push(closing);
    }
    let mut quote = None;
    let mut at = start;
    while at < bytes.len() {
        let byte = bytes[at];
        if let Some(delimiter) = quote {
            if byte == b'\\' {
                at += 2;
                continue;
            }
            if byte == delimiter {
                quote = None;
            }
        } else if matches!(byte, b'\'' | b'"' | b'`') {
            quote = Some(byte);
        } else if byte == b'/' && bytes.get(at + 1) == Some(&b'/') {
            at += 2;
            while at < bytes.len() && bytes[at] != b'\n' {
                at += 1;
            }
            continue;
        } else if byte == b'/' && bytes.get(at + 1) == Some(&b'*') {
            at += 2;
            while at + 1 < bytes.len() && &bytes[at..at + 2] != b"*/" {
                at += 1;
            }
            if at + 1 == bytes.len() {
                return None;
            }
            at += 2;
            continue;
        } else {
            match byte {
                b'{' => stack.push(b'}'),
                b'[' => stack.push(b']'),
                b'(' => stack.push(b')'),
                b'}' | b']' | b')' => {
                    if stack.pop() != Some(byte) {
                        return None;
                    }
                    if closing.is_some() && stack.is_empty() {
                        return Some(at + 1);
                    }
                }
                _ => {}
            }
        }
        at += 1;
    }
    (quote.is_none() && stack.is_empty() && closing.is_none()).then_some(at)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct ComponentId(u64);
impl ComponentId {
    pub(super) fn value(self) -> u64 {
        self.0
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PropertyKind {
    Boolean,
    Literal,
    Expression,
    Spread,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Field {
    Name,
    Property(PropertyKind),
    Children,
    Closing,
    Dynamic,
}
#[derive(Clone)]
pub(super) struct Property {
    source: Range<usize>,
    name: Option<Range<usize>>,
    value: Range<usize>,
    kind: PropertyKind,
}
impl Property {
    pub(super) fn source_range(&self) -> Range<usize> {
        self.source.clone()
    }
    pub(super) fn name_range(&self) -> Option<Range<usize>> {
        self.name.clone()
    }
    pub(super) fn value_range(&self) -> Range<usize> {
        self.value.clone()
    }
    pub(super) fn kind(&self) -> PropertyKind {
        self.kind
    }
}
#[derive(Clone)]
pub(super) struct Component {
    id: ComponentId,
    source: Range<usize>,
    name: Range<usize>,
    closing: Option<Range<usize>>,
    opener_end: usize,
    children: Range<usize>,
    properties: Vec<Property>,
    child_ids: Vec<ComponentId>,
    flow: bool,
}
impl Component {
    pub(super) fn id(&self) -> ComponentId {
        self.id
    }
    pub(super) fn source_range(&self) -> Range<usize> {
        self.source.clone()
    }
    pub(super) fn name_range(&self) -> Range<usize> {
        self.name.clone()
    }
    pub(super) fn closing_name_range(&self) -> Option<Range<usize>> {
        self.closing.clone()
    }
    pub(super) fn properties(&self) -> &[Property] {
        &self.properties
    }
    pub(super) fn is_flow(&self) -> bool {
        self.flow
    }
    pub(super) fn children_range(&self) -> Range<usize> {
        self.children.clone()
    }
    pub(super) fn child_ids(&self) -> &[ComponentId] {
        &self.child_ids
    }
}
#[derive(Default)]
pub(super) struct Index {
    pub(super) components: Vec<Component>,
}
impl Index {
    pub(super) fn from_ast(source: &str, root: &Node) -> Self {
        fn visit(
            source: &str,
            node: &Node,
            components: &mut Vec<Component>,
            parent: Option<usize>,
        ) {
            let parent = if let Some(component) = component(source, node) {
                let ix = components.len();
                if let Some(parent) = parent {
                    components[parent].child_ids.push(component.id);
                }
                components.push(component);
                Some(ix)
            } else {
                parent
            };
            if let Some(children) = node.children() {
                for child in children {
                    visit(source, child, components, parent);
                }
            }
        }
        let mut result = Self::default();
        visit(source, root, &mut result.components, None);
        result
    }
    pub(super) fn at_name(&self, range: &Range<usize>) -> Option<&Component> {
        self.components.iter().find(|component| {
            component.name.start <= range.start && range.end <= component.name.end
        })
    }
    pub(super) fn field(&self, range: &Range<usize>) -> Option<(&Component, Option<&Property>)> {
        self.components.iter().find_map(|component| {
            if component.name == *range {
                Some((component, None))
            } else {
                component
                    .properties
                    .iter()
                    .find(|property| property.source == *range)
                    .map(|property| (component, Some(property)))
            }
        })
    }
    pub(super) fn rebase(&mut self, transaction: &super::Transaction) {
        // Starts stay before inserted field text, ends follow it. In particular,
        // typing another character at a component-name end must extend both
        // name ranges before the debounced AST result is available.
        let map = |mut at, after: bool| {
            for edit in &transaction.edits {
                if at < edit.range.start || (at == edit.range.start && !after) {
                    continue;
                }
                at = if at >= edit.range.end {
                    at - edit.range.len() + edit.inserted_len
                } else {
                    edit.range.start + edit.inserted_len
                };
            }
            at
        };
        for component in &mut self.components {
            component.source = map(component.source.start, true)..map(component.source.end, true);
            component.name = map(component.name.start, false)..map(component.name.end, true);
            component.closing = component
                .closing
                .take()
                .map(|range| map(range.start, false)..map(range.end, true));
            component.opener_end = map(component.opener_end, true);
            component.children =
                map(component.children.start, false)..map(component.children.end, true);
            for property in &mut component.properties {
                property.source = map(property.source.start, false)..map(property.source.end, true);
                property.name = property
                    .name
                    .take()
                    .map(|range| map(range.start, false)..map(range.end, true));
                property.value = map(property.value.start, false)..map(property.value.end, true);
            }
        }
    }
    pub(super) fn reconcile(&mut self, old: &Self, next_id: &mut u64) {
        let mut ids = HashMap::new();
        for component in &mut self.components {
            let parsed_id = component.id;
            component.id = old
                .components
                .iter()
                .find(|old| old.source.start == component.source.start)
                .map_or_else(
                    || {
                        let id = ComponentId(*next_id);
                        *next_id += 1;
                        id
                    },
                    Component::id,
                );
            ids.insert(parsed_id, component.id);
        }
        for component in &mut self.components {
            for id in &mut component.child_ids {
                *id = ids[id];
            }
        }
    }
}

/// Presentation-only state for dedicated component adapters. Never stores text,
/// focus, composition, or undo; those continue to belong to SourcePresentation.
#[derive(Default)]
pub(super) struct ViewState {
    pub(super) collapsed: HashSet<ComponentId>,
    pub(super) active_child: HashMap<ComponentId, ComponentId>,
    pub(super) source_components: HashSet<ComponentId>,
}

fn jsx(node: &Node) -> Option<(&Option<String>, &[AttributeContent])> {
    match node {
        Node::MdxJsxFlowElement(element) => Some((&element.name, &element.attributes)),
        Node::MdxJsxTextElement(element) => Some((&element.name, &element.attributes)),
        _ => None,
    }
}
fn whitespace(bytes: &[u8], at: &mut usize) {
    while bytes.get(*at).is_some_and(u8::is_ascii_whitespace) {
        *at += 1;
    }
}
fn component(source: &str, node: &Node) -> Option<Component> {
    let (name, attributes) = jsx(node)?;
    let position = node.position()?;
    let range = position.start.offset..position.end.offset;
    let bytes = source.get(range.clone())?.as_bytes();
    let base = range.start;
    let mut at = 1;
    let name_start = at;
    if let Some(name) = name {
        at += name.len();
    }
    let name_range = base + name_start..base + at;
    let mut properties = Vec::with_capacity(attributes.len());
    for attribute in attributes {
        whitespace(bytes, &mut at);
        let start = at;
        let (property_name, value, kind) = match attribute {
            AttributeContent::Expression(_) => {
                if bytes.get(at) != Some(&b'{') {
                    return None;
                }
                at = balanced_end(bytes, at + 1, Some(b'}'))?;
                (None, base + start..base + at, PropertyKind::Spread)
            }
            AttributeContent::Property(property) => {
                at += property.name.len();
                let property_name = Some(base + start..base + at);
                whitespace(bytes, &mut at);
                match &property.value {
                    None => {
                        at = start + property.name.len();
                        (
                            property_name,
                            base + start..base + at,
                            PropertyKind::Boolean,
                        )
                    }
                    Some(value) => {
                        if bytes.get(at) != Some(&b'=') {
                            return None;
                        }
                        at += 1;
                        whitespace(bytes, &mut at);
                        let delimiter = *bytes.get(at)?;
                        at += 1;
                        let value_start = at;
                        let kind = match value {
                            AttributeValue::Literal(_) => {
                                while *bytes.get(at)? != delimiter {
                                    at += 1;
                                }
                                at += 1;
                                PropertyKind::Literal
                            }
                            AttributeValue::Expression(_) => {
                                at = balanced_end(bytes, at, Some(b'}'))?;
                                PropertyKind::Expression
                            }
                        };
                        (property_name, base + value_start..base + at - 1, kind)
                    }
                }
            }
        };
        properties.push(Property {
            source: base + start..base + at,
            name: property_name,
            value,
            kind,
        });
    }
    whitespace(bytes, &mut at);
    let self_closing = bytes.get(at) == Some(&b'/');
    if self_closing {
        at += 1;
    }
    if bytes.get(at) != Some(&b'>') {
        return None;
    }
    at += 1;
    let opener_end = base + at;
    let closing_start = if self_closing {
        range.end
    } else {
        // Only the AST element's tail is inspected; this is never a rendered
        // name search and cannot find a similarly named nested component.
        let tail = bytes.iter().rposition(|byte| *byte == b'<')?;
        if bytes.get(tail + 1) != Some(&b'/') {
            return None;
        }
        base + tail
    };
    let closing = (!self_closing)
        .then(|| closing_start + 2..closing_start + 2 + name.as_ref().map_or(0, String::len));
    Some(Component {
        id: ComponentId(base as u64 + 1),
        source: range,
        name: name_range,
        closing,
        opener_end,
        children: opener_end..closing_start,
        properties,
        child_ids: Vec::new(),
        flow: matches!(node, Node::MdxJsxFlowElement(_)),
    })
}

pub(super) fn project(
    source: &str,
    node: &Node,
    images: &std::sync::Arc<super::images::Definitions>,
    leaves: &mut Vec<Leaf>,
) -> bool {
    if let Some(component) = component(source, node) {
        let first = leaves.len();
        let mut field = |range: Range<usize>, kind| {
            let mut leaf = Leaf::new(range.clone(), Kind::Mdx(kind));
            leaf.raw(source, range);
            leaf.container_range(component.source.clone());
            leaves.push(leaf);
        };
        field(component.name.clone(), Field::Name);
        for property in &component.properties {
            field(property.source.clone(), Field::Property(property.kind));
        }
        field(
            component.children.start..component.children.start,
            Field::Children,
        );
        if !component.children.is_empty() {
            let children = node.children().map_or(&[][..], Vec::as_slice);
            if matches!(node, Node::MdxJsxTextElement(_)) {
                inline_children(source, children, images, leaves);
            } else {
                Projection::project_nodes(
                    source,
                    children,
                    component.children.clone(),
                    images,
                    leaves,
                );
            }
        }
        if let Some(closing) = component.closing {
            let mut leaf = Leaf::new(closing.clone(), Kind::Mdx(Field::Closing));
            leaf.raw(source, closing);
            leaf.container_range(component.source.clone());
            leaves.push(leaf);
        }
        for leaf in &mut leaves[first..] {
            leaf.container_range(component.source.clone());
        }
        return true;
    }
    if let Node::Paragraph(paragraph) = node {
        if paragraph.children.iter().any(|child| jsx(child).is_some()) {
            let first = leaves.len();
            inline_children(source, &paragraph.children, images, leaves);
            if let Some(position) = node.position() {
                for leaf in &mut leaves[first..] {
                    leaf.container_range(position.start.offset..position.end.offset);
                }
            }
            return true;
        }
    }
    false
}

fn inline_children(
    source: &str,
    children: &[Node],
    images: &std::sync::Arc<super::images::Definitions>,
    leaves: &mut Vec<Leaf>,
) {
    let mut paragraph: Option<Leaf> = None;
    for child in children {
        if jsx(child).is_some() {
            if let Some(leaf) = paragraph.take() {
                leaves.push(leaf);
            }
            project(source, child, images, leaves);
        } else if let Some(position) = child.position() {
            let leaf = paragraph.get_or_insert_with(|| {
                let mut leaf = Leaf::new(position.start.offset..position.end.offset, Kind::Prose);
                leaf.image_definitions = images.clone();
                leaf
            });
            leaf.range.end = position.end.offset;
            Projection::inline(source, child, leaf);
        }
    }
    if let Some(leaf) = paragraph {
        leaves.push(leaf);
    }
}

pub(super) fn name_command(
    index: &Index,
    document: &super::MarkdownDocument,
    state: &super::PresentationState,
    command: super::Command,
) -> super::Command {
    use super::Command;
    if state.marked_range().is_some() {
        return command;
    }
    if let Command::Compose {
        range,
        text,
        selected,
    } = &command
    {
        let range = range.clone().unwrap_or_else(|| state.selection().range());
        if let Some(component) = index.at_name(&range) {
            if let Some(closing) = component.closing_name_range() {
                let mirror = closing.start + range.start - component.name.start
                    ..closing.start + range.end - component.name.start;
                return Command::ComposePaired {
                    range,
                    mirror,
                    text: text.clone(),
                    selected: selected.clone(),
                };
            }
        }
    }
    let replacement = match &command {
        Command::Insert(text) | Command::Commit(text) => {
            Some((state.selection().range(), text.as_str()))
        }
        Command::Replace { range, text, .. } => Some((range.clone(), text.as_str())),
        Command::Backspace | Command::Delete => {
            let mut range = state.selection().range();
            if range.is_empty() {
                if matches!(command, Command::Backspace) {
                    range.start = document.previous(range.start);
                } else {
                    range.end = document.next(range.end);
                }
            }
            Some((range, ""))
        }
        _ => None,
    };
    if let Some((range, text)) = replacement {
        if let Some(component) = index.at_name(&range) {
            if range == component.name_range() {
                return Command::RenameComponent {
                    opening: range,
                    closing: component.closing_name_range(),
                    name: text.to_owned(),
                };
            }
            let mut edits = Vec::with_capacity(2);
            if let Some(closing) = component.closing_name_range() {
                let mirror = closing.start + range.start - component.name.start
                    ..closing.start + range.end - component.name.start;
                edits.push((mirror, text.to_owned()));
            }
            edits.push((range, text.to_owned()));
            return Command::BatchReplace {
                edits,
                kind: super::EditKind::Command,
            };
        }
    }
    command
}
