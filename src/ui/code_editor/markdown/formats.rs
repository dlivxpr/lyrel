//! Explicit inline commands. Source spelling and the document history remain authoritative.
use super::{Command, EditKind, Selection, SourcePresentation};
use crate::ui::i18n::L10nKey;
use gpui_kit::{Action, App, Context, KeyBinding, KeyBindingContextPredicate, Window};
use markdown::mdast::Node;
use std::ops::Range;

gpui_kit::actions!(
    markdown_formats,
    [
        Bold,
        Italic,
        Underline,
        Strike,
        InlineCode,
        Highlight,
        ClearHighlightColor,
        CustomHighlightColor,
        ClearTextColor,
        CustomTextColor,
        ApplyCustomColor,
        CancelCustomColor
    ]
);

#[derive(Clone, Copy, PartialEq, Eq)]
pub(in super::super) enum Format {
    Bold,
    Italic,
    Underline,
    Strike,
    Code,
    Highlight,
    TextColor,
    HighlightColor,
}
impl Format {
    fn pair(self) -> (&'static str, &'static str) {
        match self {
            Self::Bold => ("**", "**"),
            Self::Italic => ("*", "*"),
            Self::Underline => ("<u>", "</u>"),
            Self::Strike => ("~~", "~~"),
            Self::Code => ("`", "`"),
            Self::Highlight => ("==", "=="),
            _ => ("", ""),
        }
    }
    fn label(self) -> L10nKey {
        match self {
            Self::Bold => L10nKey::MarkdownFormatBold,
            Self::Italic => L10nKey::MarkdownFormatItalic,
            Self::Underline => L10nKey::MarkdownFormatUnderline,
            Self::Strike => L10nKey::MarkdownFormatStrike,
            Self::Code => L10nKey::MarkdownFormatCode,
            Self::Highlight => L10nKey::MarkdownFormatHighlight,
            Self::TextColor => L10nKey::MarkdownFormatTextColor,
            Self::HighlightColor => L10nKey::MarkdownFormatHighlightColor,
        }
    }
}

pub(crate) fn bindings(cx: &App) -> Vec<KeyBinding> {
    let bind = |keys: &str, action: Box<dyn Action>| {
        let predicate = KeyBindingContextPredicate::parse("Input && Markdown")
            .expect("format context")
            .into();
        KeyBinding::load(keys, action, Some(predicate), false, None, cx).expect("format shortcut")
    };
    vec![
        bind("secondary-b", Box::new(Bold)),
        bind("secondary-i", Box::new(Italic)),
        bind("secondary-e", Box::new(InlineCode)),
        bind("secondary-shift-x", Box::new(Strike)),
        bind("secondary-shift-u", Box::new(Underline)),
        bind("secondary-shift-h", Box::new(Highlight)),
    ]
}

#[derive(Clone)]
pub(in super::super) struct Decoration {
    pub open: Range<usize>,
    pub close: Range<usize>,
    pub format: Format,
    pub color: Option<u32>,
}
impl Decoration {
    pub(in super::super) fn content(&self) -> Range<usize> {
        self.open.end..self.close.start
    }
}

fn hex(value: &str) -> Option<u32> {
    let value = value.strip_prefix('#')?;
    if !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let n = u32::from_str_radix(value, 16).ok()?;
    match value.len() {
        3 => Some(
            ((n >> 8 & 15) * 17 << 24) | ((n >> 4 & 15) * 17 << 16) | ((n & 15) * 17 << 8) | 255,
        ),
        4 => Some(
            ((n >> 12 & 15) * 17 << 24)
                | ((n >> 8 & 15) * 17 << 16)
                | ((n >> 4 & 15) * 17 << 8)
                | ((n & 15) * 17),
        ),
        6 => Some(n << 8 | 255),
        8 => Some(n),
        _ => None,
    }
}

fn css_color(value: &str) -> Option<u32> {
    let value = value.trim();
    if value.starts_with('#') {
        return hex(value);
    }
    if let Some((function, arguments)) = value.split_once('(') {
        if !function.eq_ignore_ascii_case("rgb") && !function.eq_ignore_ascii_case("rgba") {
            return None;
        }
        let mut channels = arguments.strip_suffix(')')?.split(',');
        let mut next = |max: f32| -> Option<f32> {
            let value = channels.next()?.trim().parse::<f32>().ok()?;
            value.is_finite().then_some(value.clamp(0., max))
        };
        let r = next(255.)? as u32;
        let g = next(255.)? as u32;
        let b = next(255.)? as u32;
        let a = if function.eq_ignore_ascii_case("rgba") {
            (next(1.)? * 255.).round() as u32
        } else {
            255
        };
        if channels.next().is_some() {
            return None;
        }
        return Some(r << 24 | g << 16 | b << 8 | a);
    }
    let names = [
        ("black", 0x000000),
        ("white", 0xffffff),
        ("red", 0xff0000),
        ("green", 0x008000),
        ("blue", 0x0000ff),
        ("yellow", 0xffff00),
        ("orange", 0xffa500),
        ("purple", 0x800080),
        ("pink", 0xffc0cb),
        ("gray", 0x808080),
        ("grey", 0x808080),
        ("brown", 0xa52a2a),
        ("cyan", 0x00ffff),
        ("magenta", 0xff00ff),
        ("lime", 0x00ff00),
        ("navy", 0x000080),
        ("teal", 0x008080),
    ];
    names
        .into_iter()
        .find(|(name, _)| value.eq_ignore_ascii_case(name))
        .map(|(_, rgb)| rgb << 8 | 255)
}

// Deliberately accept only inert formatting tags, never arbitrary HTML/CSS or JSX values.
fn tag(raw: &str) -> Option<(Format, Option<u32>, &'static str)> {
    let inner = raw.trim().strip_prefix('<')?.strip_suffix('>')?;
    let end = inner.find(char::is_whitespace).unwrap_or(inner.len());
    let name = &inner[..end];
    let (format, close, property) = if name.eq_ignore_ascii_case("u") {
        (Format::Underline, "</u>", "")
    } else if name.eq_ignore_ascii_case("mark") {
        (Format::Highlight, "</mark>", "background")
    } else if name.eq_ignore_ascii_case("span") {
        (Format::TextColor, "</span>", "color")
    } else {
        return None;
    };
    let mut attributes = inner[end..].trim();
    let mut color = None;
    let mut inherits_color = false;
    while !attributes.is_empty() {
        let end = attributes
            .find(|c: char| !c.is_ascii_alphanumeric() && !matches!(c, '-' | ':' | '_'))
            .unwrap_or(attributes.len());
        if end == 0 {
            return None;
        }
        let key = &attributes[..end];
        attributes = attributes[end..].trim_start();
        if !attributes.starts_with('=') {
            continue;
        }
        attributes = attributes[1..].trim_start();
        let first = attributes.chars().next()?;
        let value;
        if matches!(first, '\'' | '"') {
            let end = attributes[1..].find(first)? + 1;
            value = &attributes[1..end];
            attributes = attributes[end + 1..].trim_start();
        } else {
            let end = attributes
                .find(char::is_whitespace)
                .unwrap_or(attributes.len());
            value = &attributes[..end];
            if value.contains(['{', '}', '<', '>']) {
                return None;
            }
            attributes = attributes[end..].trim_start();
        }
        if key.eq_ignore_ascii_case("style") {
            for declaration in value.split(';') {
                if let Some((key, value)) = declaration.split_once(':') {
                    if key.trim().eq_ignore_ascii_case(property)
                        || property == "background"
                            && key.trim().eq_ignore_ascii_case("background-color")
                    {
                        color = css_color(value);
                        inherits_color =
                            property == "color" && value.trim().eq_ignore_ascii_case("inherit");
                    }
                }
            }
        }
    }
    if format == Format::TextColor && color.is_none() && !inherits_color {
        return None;
    }
    Some((
        if format == Format::Highlight && color.is_some() {
            Format::HighlightColor
        } else {
            format
        },
        color,
        close,
    ))
}

pub(in super::super) fn opaque_tag(raw: &str) -> Option<(&'static str, bool)> {
    let inner = raw.trim().strip_prefix('<')?.strip_suffix('>')?.trim();
    let closing = inner.starts_with('/');
    let name = inner.trim_start_matches('/').split_whitespace().next()?;
    ["code", "pre", "script", "style", "textarea"]
        .into_iter()
        .find(|candidate| name.eq_ignore_ascii_case(candidate))
        .map(|name| (name, closing))
}

fn opaque_jsx(node: &Node) -> bool {
    let name = match node {
        Node::MdxJsxTextElement(element) => element.name.as_deref(),
        Node::MdxJsxFlowElement(element) => element.name.as_deref(),
        _ => None,
    };
    name.is_some_and(|name| {
        ["code", "pre", "script", "style", "textarea"]
            .into_iter()
            .any(|candidate| name.eq_ignore_ascii_case(candidate))
    })
}

pub(in super::super) fn wrapper(source: &str, node: &Node) -> Option<Decoration> {
    if !matches!(
        node,
        Node::MdxJsxTextElement(_) | Node::MdxJsxFlowElement(_)
    ) {
        return None;
    }
    let p = node.position()?;
    let raw = &source[p.start.offset..p.end.offset];
    let end = raw.find('>')? + 1;
    let (format, color, close) = tag(&raw[..end])?;
    if raw.len() < end + close.len()
        || !raw
            .get(raw.len() - close.len()..)
            .is_some_and(|tail| tail.eq_ignore_ascii_case(close))
    {
        return None;
    }
    Some(Decoration {
        open: p.start.offset..p.start.offset + end,
        close: p.end.offset - close.len()..p.end.offset,
        format,
        color,
    })
}

pub(in super::super) fn decorations(source: &str, root: &Node) -> Vec<Decoration> {
    fn visit(source: &str, node: &Node, output: &mut Vec<Decoration>) {
        if opaque_jsx(node) {
            return;
        }
        if let Some(decoration) = wrapper(source, node) {
            output.push(decoration);
        }
        if matches!(
            node,
            Node::Code(_)
                | Node::InlineCode(_)
                | Node::Math(_)
                | Node::InlineMath(_)
                | Node::MdxjsEsm(_)
                | Node::MdxFlowExpression(_)
                | Node::MdxTextExpression(_)
        ) {
            return;
        }
        let Some(children) = node.children() else {
            return;
        };
        let mut tags: Vec<(Range<usize>, Format, Option<u32>, &'static str)> = Vec::new();
        let mut highlight = None;
        let mut opaque = None;
        for child in children {
            let Some(p) = child.position() else {
                continue;
            };
            let range = p.start.offset..p.end.offset;
            let raw = &source[range.clone()];
            if matches!(child, Node::Html(_)) {
                if let Some((name, closing)) = opaque_tag(raw) {
                    if let Some((opened, depth)) = opaque.as_mut() {
                        if *opened == name {
                            if closing {
                                *depth -= 1;
                                if *depth == 0 {
                                    opaque = None;
                                }
                            } else {
                                *depth += 1;
                            }
                        }
                    } else if !closing {
                        opaque = Some((name, 1usize));
                        highlight = None;
                    }
                    continue;
                }
            }
            if opaque.is_some() {
                continue;
            }
            if matches!(child, Node::Html(_)) {
                if let Some((format, color, close)) = tag(raw) {
                    tags.push((range.clone(), format, color, close));
                } else if tags
                    .last()
                    .is_some_and(|(_, _, _, close)| raw.trim().eq_ignore_ascii_case(close))
                {
                    let (open, format, color, _) = tags.pop().unwrap();
                    output.push(Decoration {
                        open,
                        close: range.clone(),
                        format,
                        color,
                    });
                }
            }
            if matches!(child, Node::Text(_)) {
                let mut at = 0;
                while at + 1 < raw.len() {
                    if raw.as_bytes()[at..].starts_with(b"==")
                        && raw.as_bytes()[..at]
                            .iter()
                            .rev()
                            .take_while(|byte| **byte == b'\\')
                            .count()
                            % 2
                            == 0
                    {
                        let token = range.start + at..range.start + at + 2;
                        if let Some(open) = highlight.take() {
                            output.push(Decoration {
                                open,
                                close: token,
                                format: Format::Highlight,
                                color: None,
                            });
                        } else {
                            highlight = Some(token);
                        }
                        at += 2;
                    } else {
                        at += 1;
                    }
                }
            }
            visit(source, child, output);
        }
    }
    let mut output = Vec::new();
    visit(source, root, &mut output);
    output.sort_by_key(|d| (d.open.start, std::cmp::Reverse(d.close.end)));
    output
}

fn trim(source: &str, mut range: Range<usize>) -> Range<usize> {
    loop {
        let text = &source[range.clone()];
        if let Some(at) = text.rfind("</").filter(|_| text.ends_with('>')) {
            let name = &text[at + 2..text.len() - 1];
            if !name.is_empty()
                && name.chars().all(|c| c.is_ascii_alphanumeric())
                && !text[..at].contains(&format!("<{name}"))
            {
                range.end = range.start + at;
                continue;
            }
        }
        if let Some(marker) = ["==", "**", "~~"].into_iter().find(|m| {
            text.len() > m.len()
                && (text.starts_with(m) || text.ends_with(m))
                && text.matches(m).count() == 1
        }) {
            if text.ends_with(marker) {
                range.end -= marker.len();
            } else {
                range.start += marker.len();
            }
            continue;
        }
        if text.starts_with('<') && !text.starts_with("</") {
            if let Some(end) = text.find('>') {
                let name = text[1..end]
                    .split(|c: char| !c.is_ascii_alphanumeric())
                    .next()
                    .unwrap_or("");
                if !name.is_empty() && !text[end..].contains(&format!("</{name}>")) {
                    range.start += end + 1;
                    continue;
                }
            }
        }
        return range;
    }
}

fn wrapped(
    source: &str,
    range: Range<usize>,
    open: &str,
    close: &str,
) -> Option<(Range<usize>, Range<usize>)> {
    let text = &source[range.clone()];
    if text.len() >= open.len() + close.len() && text.starts_with(open) && text.ends_with(close) {
        Some((
            range.clone(),
            range.start + open.len()..range.end - close.len(),
        ))
    } else if source[..range.start].ends_with(open) && source[range.end..].starts_with(close) {
        Some((range.start - open.len()..range.end + close.len(), range))
    } else {
        None
    }
}

fn markdown_wrapper(
    node: &Node,
    range: &Range<usize>,
    format: Format,
) -> Option<(Range<usize>, Range<usize>)> {
    fn visible(node: &Node) -> Option<Range<usize>> {
        if matches!(node, Node::Strong(_) | Node::Emphasis(_) | Node::Delete(_)) {
            let children = node.children()?;
            return Some(visible(children.first()?)?.start..visible(children.last()?)?.end);
        }
        let p = node.position()?;
        Some(p.start.offset..p.end.offset)
    }
    fn selected_scope(node: &Node, range: &Range<usize>) -> Option<Range<usize>> {
        let p = node.position()?;
        if range.start < p.start.offset || p.end.offset < range.end {
            return None;
        }
        if range.start == p.start.offset
            && range.end == p.end.offset
            && matches!(node, Node::Strong(_) | Node::Emphasis(_) | Node::Delete(_))
        {
            return visible(node);
        }
        node.children()?
            .iter()
            .find_map(|child| selected_scope(child, range))
    }
    fn find(
        node: &Node,
        range: &Range<usize>,
        scope: &Option<Range<usize>>,
        format: Format,
    ) -> Option<(Range<usize>, Range<usize>)> {
        let p = node.position()?;
        if p.start.offset >= range.end || p.end.offset <= range.start {
            return None;
        }
        if let Some(found) = node.children().and_then(|children| {
            children
                .iter()
                .find_map(|child| find(child, range, scope, format))
        }) {
            return Some(found);
        }
        let width = match (node, format) {
            (Node::Strong(_), Format::Bold) | (Node::Delete(_), Format::Strike) => 2,
            (Node::Emphasis(_), Format::Italic) => 1,
            _ => return None,
        };
        let outer = p.start.offset..p.end.offset;
        let body = outer.start + width..outer.end - width;
        let content = visible(node);
        let selected_nested = range.start <= outer.start
            && outer.end <= range.end
            && scope.is_some()
            && content == *scope;
        (*range == outer || *range == body || content.as_ref() == Some(range) || selected_nested)
            .then_some((outer, body))
    }
    find(node, range, &selected_scope(node, range), format)
}

fn eligible(source: &str, root: &Node, range: &Range<usize>, format: Format) -> bool {
    fn forbidden(source: &str, node: &Node, range: &Range<usize>, format: Format) -> bool {
        let Some(p) = node.position() else {
            return false;
        };
        if p.start.offset >= range.end || p.end.offset <= range.start {
            return false;
        }
        if opaque_jsx(node) {
            return true;
        }
        if matches!(node, Node::Link(_) | Node::LinkReference(_)) {
            let body = node.children().and_then(|children| {
                Some(
                    children.first()?.position()?.start.offset
                        ..children.last()?.position()?.end.offset,
                )
            });
            let whole = range.start <= p.start.offset && p.end.offset <= range.end;
            if matches!(node, Node::Link(_))
                && super::super::links::label_range(source, node).is_none()
            {
                let delimited = source[p.start.offset..p.end.offset].starts_with('<');
                if !whole
                    || (!delimited && range.start == p.start.offset && range.end == p.end.offset)
                {
                    return true;
                }
            }
            if !whole && body.is_none_or(|body| range.start < body.start || body.end < range.end) {
                return true;
            }
        }
        if matches!(
            node,
            Node::Image(_) | Node::ImageReference(_) | Node::FootnoteReference(_)
        ) {
            return true;
        }
        if let Node::InlineCode(_) = node {
            return format != Format::Code
                || !wrapped(source, range.clone(), "`", "`").is_some_and(|(outer, _)| {
                    p.start.offset <= outer.start && outer.end <= p.end.offset
                });
        }
        if matches!(
            node,
            Node::Code(_)
                | Node::Math(_)
                | Node::InlineMath(_)
                | Node::Yaml(_)
                | Node::Toml(_)
                | Node::MdxjsEsm(_)
                | Node::MdxFlowExpression(_)
                | Node::MdxTextExpression(_)
                | Node::Definition(_)
        ) {
            return true;
        }
        if matches!(
            node,
            Node::MdxJsxTextElement(_) | Node::MdxJsxFlowElement(_)
        ) {
            if let Some(d) = wrapper(source, node) {
                if !(d.content().start <= range.start && range.end <= d.content().end)
                    && !(range.start <= d.open.start && d.close.end <= range.end)
                {
                    return true;
                }
            } else {
                let body = node.children().and_then(|children| {
                    Some(
                        children.first()?.position()?.start.offset
                            ..children.last()?.position()?.end.offset,
                    )
                });
                if body.is_none_or(|body| body.start > range.start || range.end > body.end) {
                    return true;
                }
            }
        }
        if matches!(node, Node::Html(_))
            && tag(&source[p.start.offset..p.end.offset]).is_none()
            && !["</u>", "</span>", "</mark>"].into_iter().any(|close| {
                source[p.start.offset..p.end.offset]
                    .trim()
                    .eq_ignore_ascii_case(close)
            })
        {
            return true;
        }
        if let Some(children) = node.children() {
            let mut opening = None;
            for child in children {
                if let Node::Html(_) = child {
                    if let Some(p) = child.position() {
                        if let Some((name, closing)) =
                            opaque_tag(&source[p.start.offset..p.end.offset])
                        {
                            if let Some((opened, start, depth)) = opening.as_mut() {
                                if *opened == name {
                                    if closing {
                                        *depth -= 1;
                                        if *depth == 0 {
                                            if *start < range.end && range.start < p.start.offset {
                                                return true;
                                            }
                                            opening = None;
                                        }
                                    } else {
                                        *depth += 1;
                                    }
                                }
                            } else if !closing {
                                opening = Some((name, p.end.offset, 1usize));
                            }
                        }
                    }
                }
            }
            if opening.is_some_and(|(_, start, _)| start < range.end) {
                return true;
            }
        }
        node.children().is_some_and(|children| {
            children
                .iter()
                .any(|child| forbidden(source, child, range, format))
        })
    }
    range.start < range.end && !forbidden(source, root, range, format)
}

fn selection_crosses_mark(node: &Node, range: &Range<usize>) -> bool {
    let crossing = node.position().is_some_and(|position| {
        matches!(
            node,
            Node::Strong(_)
                | Node::Emphasis(_)
                | Node::Delete(_)
                | Node::Link(_)
                | Node::LinkReference(_)
        ) && (position.start.offset < range.start
            && range.start < position.end.offset
            && position.end.offset <= range.end
            || range.start <= position.start.offset
                && position.start.offset < range.end
                && range.end < position.end.offset)
    });
    crossing
        || node.children().is_some_and(|children| {
            children
                .iter()
                .any(|node| selection_crosses_mark(node, range))
        })
}

fn selection_crosses_blocks(node: &Node, range: &Range<usize>) -> bool {
    fn count(node: &Node, range: &Range<usize>) -> usize {
        if let Some(position) = node.position() {
            if position.end.offset <= range.start || range.end <= position.start.offset {
                return 0;
            }
        }
        if matches!(
            node,
            Node::Paragraph(_) | Node::Heading(_) | Node::TableCell(_)
        ) {
            return 1;
        }
        node.children().map_or(0, |children| {
            children.iter().map(|node| count(node, range)).sum()
        })
    }
    count(node, range) > 1
}

fn color_crosses_mark(root: &Node, range: &Range<usize>, styles: &[Decoration]) -> bool {
    fn contains_mark(node: &Node, at: usize, outer: usize) -> bool {
        let Some(position) = node.position() else {
            return false;
        };
        if at <= position.start.offset || position.end.offset <= at {
            return false;
        }
        if matches!(
            node,
            Node::Strong(_)
                | Node::Emphasis(_)
                | Node::Delete(_)
                | Node::Link(_)
                | Node::LinkReference(_)
        ) && outer < position.start.offset
        {
            return true;
        }
        node.children()
            .is_some_and(|children| children.iter().any(|node| contains_mark(node, at, outer)))
    }
    [range.start, range.end].into_iter().any(|at| {
        styles
            .iter()
            .find(|d| {
                d.format == Format::TextColor
                    && d.color.is_some()
                    && d.open.end < at
                    && at < d.close.start
            })
            .is_some_and(|color| {
                contains_mark(root, at, color.open.start)
                    || styles.iter().any(|d| {
                        d.format != Format::TextColor
                            && color.open.start < d.open.start
                            && d.open.end <= at
                            && at <= d.close.start
                    })
            })
    })
}

#[derive(Clone)]
struct ColorRun {
    range: Range<usize>,
    owner: Option<usize>,
}

fn color_runs(
    source: &str,
    root: &Node,
    scope: &Range<usize>,
    styles: &[Decoration],
) -> Vec<ColorRun> {
    fn collect(node: &Node, source: &str, scope: &Range<usize>, runs: &mut Vec<Range<usize>>) {
        let Some(position) = node.position() else {
            return;
        };
        let span = position.start.offset..position.end.offset;
        if span.end <= scope.start || scope.end <= span.start {
            return;
        }
        if opaque_jsx(node)
            || matches!(
                node,
                Node::Text(_)
                    | Node::InlineCode(_)
                    | Node::InlineMath(_)
                    | Node::Image(_)
                    | Node::ImageReference(_)
                    | Node::FootnoteReference(_)
                    | Node::Html(_)
            )
        {
            let raw = &source[span.clone()];
            if !matches!(node, Node::Html(_))
                || tag(raw).is_none()
                    && !["</u>", "</mark>", "</span>"]
                        .iter()
                        .any(|close| raw.trim().eq_ignore_ascii_case(close))
            {
                runs.push(span);
            }
            return;
        }
        let Some(children) = node.children() else {
            return;
        };
        let mut opaque = None;
        for child in children {
            let Some(position) = child.position() else {
                continue;
            };
            if matches!(child, Node::Html(_)) {
                if let Some((name, closing)) =
                    opaque_tag(&source[position.start.offset..position.end.offset])
                {
                    if let Some((opened, start, depth)) = opaque.as_mut() {
                        if *opened == name {
                            if closing {
                                *depth -= 1;
                                if *depth == 0 {
                                    runs.push(*start..position.end.offset);
                                    opaque = None;
                                }
                            } else {
                                *depth += 1;
                            }
                        }
                    } else if !closing {
                        opaque = Some((name, position.start.offset, 1usize));
                    }
                    continue;
                }
            }
            if opaque.is_none() {
                collect(child, source, scope, runs);
            }
        }
        if let Some((_, start, _)) = opaque {
            runs.push(start..span.end);
        }
    }
    // AST text/atom runs exclude block gaps and parser-confirmed Markdown
    // delimiters, including both GFM strike spellings. Resolve all colors and
    // extension delimiters with one monotonic boundary sweep.
    let mut text = Vec::new();
    collect(root, source, scope, &mut text);
    let mut boundaries = Vec::new();
    for (ix, style) in styles
        .iter()
        .enumerate()
        .filter(|(_, d)| d.open.start < scope.end && scope.start < d.close.end)
    {
        if style.format == Format::TextColor
            && style.color.is_some()
            && style.open.end < style.close.start
        {
            boundaries.extend([
                (style.open.end, true, Some(ix)),
                (style.close.start, false, Some(ix)),
            ]);
        }
        for hidden in [&style.open, &style.close] {
            boundaries.extend([(hidden.start, true, None), (hidden.end, false, None)]);
        }
    }
    boundaries.sort_unstable_by_key(|(at, start, _)| (*at, *start));
    let mut active = std::collections::BTreeSet::new();
    let mut hidden = 0usize;
    let mut next = 0;
    let mut result: Vec<ColorRun> = Vec::new();
    for run in text {
        let mut at = run.start.max(scope.start);
        let end = run.end.min(scope.end);
        while at < end {
            while let Some((_, start, owner)) = boundaries
                .get(next)
                .copied()
                .filter(|(boundary, _, _)| *boundary <= at)
            {
                if let Some(owner) = owner {
                    if start {
                        active.insert(owner);
                    } else {
                        active.remove(&owner);
                    }
                } else if start {
                    hidden += 1;
                } else {
                    hidden -= 1;
                }
                next += 1;
            }
            let through = boundaries
                .get(next)
                .map_or(end, |(boundary, _, _)| end.min(*boundary));
            if hidden == 0 && at < through {
                let owner = active.last().copied();
                if let Some(previous) = result
                    .last_mut()
                    .filter(|previous| previous.range.end == at && previous.owner == owner)
                {
                    previous.range.end = through;
                } else {
                    result.push(ColorRun {
                        range: at..through,
                        owner,
                    });
                }
            }
            at = through;
        }
    }
    result
}

fn preserve_crossed_colors(
    source: &str,
    root: &Node,
    range: &Range<usize>,
    styles: &[Decoration],
    selected_open: Option<&str>,
    retained: &std::collections::HashMap<usize, String>,
    changes: &mut std::collections::BTreeMap<usize, (usize, String)>,
) {
    let scope = styles
        .iter()
        .filter(|d| {
            d.format == Format::TextColor
                && d.color.is_some()
                && d.open.end < range.end
                && range.start < d.close.start
        })
        .fold(range.clone(), |scope, d| {
            scope.start.min(d.open.start)..scope.end.max(d.close.end)
        });
    for run in color_runs(source, root, &scope, styles) {
        let mut cuts =
            smallvec::SmallVec::<[usize; 4]>::from_slice(&[run.range.start, run.range.end]);
        cuts.extend(
            [range.start, range.end]
                .into_iter()
                .filter(|at| run.range.start < *at && *at < run.range.end),
        );
        cuts.sort_unstable();
        for pair in cuts.windows(2) {
            let part = pair[0]..pair[1];
            let selected = part.start < range.end && range.start < part.end;
            let original = run
                .owner
                .map(|ix| &styles[ix])
                .filter(|style| style.open.end < range.end && range.start < style.close.start);
            let (open, close) = if selected {
                let Some(open) = selected_open else {
                    continue;
                };
                (open, "</span>")
            } else {
                let Some(style) = original else {
                    continue;
                };
                (
                    run.owner
                        .and_then(|ix| retained.get(&ix))
                        .map_or(&source[style.open.clone()], String::as_str),
                    &source[style.close.clone()],
                )
            };
            changes
                .entry(part.start)
                .or_insert_with(|| (part.start, String::new()))
                .1
                .push_str(open);
            changes
                .entry(part.end)
                .or_insert_with(|| (part.end, String::new()))
                .1
                .push_str(close);
        }
    }
}

fn color_hex(color: u32) -> String {
    if color & 255 == 255 {
        format!("#{:06x}", color >> 8)
    } else {
        format!("#{color:08x}")
    }
}

// Return original style-value byte ranges and whether this opening has any
// non-color attributes/declarations. Never serialize an HTML opening.
fn color_properties(raw: &str) -> Option<(Vec<Range<usize>>, bool)> {
    let inner = raw.strip_prefix('<')?.strip_suffix('>')?;
    let end = inner.find(char::is_whitespace).unwrap_or(inner.len());
    let mut attributes = inner[end..].trim_start();
    let mut properties = Vec::new();
    let mut other = false;
    while !attributes.is_empty() {
        let end = attributes
            .find(|c: char| !c.is_ascii_alphanumeric() && !matches!(c, '-' | ':' | '_'))
            .unwrap_or(attributes.len());
        if end == 0 {
            return None;
        }
        let key = &attributes[..end];
        attributes = attributes[end..].trim_start();
        if !attributes.starts_with('=') {
            other = true;
            continue;
        }
        attributes = attributes[1..].trim_start();
        let first = attributes.chars().next()?;
        let value;
        let value_start;
        if matches!(first, '\'' | '"') {
            let end = attributes[1..].find(first)? + 1;
            value_start = raw.len() - 1 - attributes.len() + 1;
            value = &attributes[1..end];
            attributes = attributes[end + 1..].trim_start();
        } else {
            let end = attributes
                .find(char::is_whitespace)
                .unwrap_or(attributes.len());
            value_start = raw.len() - 1 - attributes.len();
            value = &attributes[..end];
            attributes = attributes[end..].trim_start();
        }
        if !key.eq_ignore_ascii_case("style") {
            other = true;
            continue;
        }
        let mut offset = 0;
        for declaration in value.split_inclusive(';') {
            let text = declaration.strip_suffix(';').unwrap_or(declaration);
            if let Some((key, value)) = text.split_once(':') {
                if key.trim().eq_ignore_ascii_case("color") {
                    let at = value_start + offset + key.len() + 1 + value.len()
                        - value.trim_start().len();
                    properties.push(at..at + value.trim().len());
                } else {
                    other |= !text.trim().is_empty();
                }
            } else {
                other |= !text.trim().is_empty();
            }
            offset += declaration.len();
        }
    }
    Some((properties, other))
}

fn color_command(
    source: &str,
    range: Range<usize>,
    reversed: bool,
    leading: usize,
    changes: std::collections::BTreeMap<usize, (usize, String)>,
) -> Option<Command> {
    let mut start = range.start as isize + leading as isize;
    let mut end = range.end as isize;
    let mut edits = Vec::with_capacity(changes.len());
    for (at, (through, text)) in changes {
        let delta = text.len() as isize - (through - at) as isize;
        if through <= range.start && at < range.start {
            start += delta;
        }
        if at < range.end {
            end += delta;
        }
        if source[at..through] != text {
            edits.push((at..through, text));
        }
    }
    let selection = if reversed {
        Selection::new(end as usize, start as usize)
    } else {
        Selection::new(start as usize, end as usize)
    };
    (!edits.is_empty()).then_some(Command::BatchReplaceSelected {
        edits,
        selection,
        kind: EditKind::Command,
    })
}

fn text_color_edit(
    source: &str,
    root: &Node,
    range: Range<usize>,
    reversed: bool,
    color: Option<u32>,
    styles: &[Decoration],
) -> Option<Command> {
    // Split enclosing colors at selection edges. Other formatting and all
    // unselected source spelling remain owned by the original document.
    let mut changes = std::collections::BTreeMap::<usize, (usize, String)>::new();
    let mut affected = Vec::new();
    for (ix, decoration) in styles.iter().enumerate().filter(|(_, d)| {
        d.format == Format::TextColor
            && d.color.is_some()
            && d.content().start < range.end
            && range.start < d.content().end
    }) {
        let (properties, other) = color_properties(&source[decoration.open.clone()])?;
        affected.push((ix, decoration, properties, other));
    }
    if let Some(color) = color {
        if let Some((_, owner, _, _)) = affected
            .iter()
            .find(|(_, d, _, _)| d.content() == range || (d.open.start..d.close.end) == range)
        {
            let hex = color_hex(color);
            for (_, decoration, properties, _) in &affected {
                if owner.open.start <= decoration.open.start
                    && decoration.close.end <= owner.close.end
                {
                    for property in properties {
                        changes.insert(
                            decoration.open.start + property.start,
                            (decoration.open.start + property.end, hex.clone()),
                        );
                    }
                }
            }
            let selected = if range.start == owner.open.start {
                owner.content()
            } else {
                range
            };
            return color_command(source, selected, reversed, 0, changes);
        }
    }
    let split_selection = affected.iter().any(|(_, _, _, other)| *other)
        || selection_crosses_blocks(root, &range)
        || selection_crosses_mark(root, &range)
        || styles.iter().any(|d| {
            d.format != Format::TextColor
                && (d.open.start < range.start
                    && range.start < d.close.end
                    && d.close.end < range.end
                    || range.start < d.open.start
                        && d.open.start < range.end
                        && range.end < d.close.end)
        });
    let distribute = split_selection || color_crosses_mark(root, &range, styles);
    let mut prefix = String::new();
    let mut suffix = String::new();
    let mut retained = std::collections::HashMap::new();
    for (ix, decoration, properties, other) in affected {
        if other {
            for property in properties {
                changes.insert(
                    decoration.open.start + property.start,
                    (decoration.open.start + property.end, "inherit".into()),
                );
            }
            retained.insert(
                ix,
                format!(
                    "<span style=\"color:{}\">",
                    color_hex(decoration.color.unwrap())
                ),
            );
            continue;
        }
        let body = decoration.content();
        if distribute {
            changes.insert(decoration.open.start, (decoration.open.end, String::new()));
            changes.insert(
                decoration.close.start,
                (decoration.close.end, String::new()),
            );
            continue;
        }
        if body.start < range.start {
            prefix.push_str(&source[decoration.close.clone()]);
        } else {
            changes.insert(decoration.open.start, (decoration.open.end, String::new()));
        }
        if range.end < body.end {
            suffix.push_str(&source[decoration.open.clone()]);
        } else {
            changes.insert(
                decoration.close.start,
                (decoration.close.end, String::new()),
            );
        }
    }
    let (mut open, mut close) = color.map_or_else(
        || (String::new(), ""),
        |color| {
            let hex = color_hex(color);
            (format!("<span style=\"color:{hex}\">"), "</span>")
        },
    );
    if distribute {
        preserve_crossed_colors(
            source,
            root,
            &range,
            styles,
            split_selection
                .then_some(open.as_str())
                .filter(|open| !open.is_empty()),
            &retained,
            &mut changes,
        );
    }
    if split_selection {
        open.clear();
        close = "";
    }
    let leading =
        prefix.len() + open.len() + changes.get(&range.start).map_or(0, |(_, text)| text.len());
    prefix.push_str(&open);
    if !prefix.is_empty() {
        changes
            .entry(range.start)
            .or_insert_with(|| (range.start, String::new()))
            .1
            .push_str(&prefix);
    }
    if !close.is_empty() || !suffix.is_empty() {
        let replacement = &mut changes
            .entry(range.end)
            .or_insert_with(|| (range.end, String::new()))
            .1;
        if distribute {
            replacement.insert_str(0, close);
        } else {
            replacement.push_str(close);
            replacement.push_str(&suffix);
        }
    }
    color_command(source, range, reversed, leading, changes)
}

fn edit(
    source: &str,
    root: &Node,
    selection: Selection,
    format: Format,
    color: Option<u32>,
    styles: &[Decoration],
) -> Option<Command> {
    let range = trim(source, selection.range());
    if !eligible(source, root, &range, format) {
        return None;
    }
    if format == Format::TextColor {
        return text_color_edit(source, root, range, selection.is_reversed(), color, styles);
    }
    let mut owner = range.clone();
    let mut body = range.clone();
    let mut converted = false;
    if matches!(format, Format::HighlightColor | Format::Highlight) {
        if let Some(d) = styles.iter().find(|d| {
            (d.format == Format::HighlightColor
                || d.format == Format::Highlight && source[d.open.clone()].starts_with('<'))
                && (d.content() == range || d.open.start == range.start && d.close.end == range.end)
        }) {
            owner = d.open.start..d.close.end;
            body = d.content();
        }
        if format == Format::HighlightColor {
            if let Some((outer, inner)) = wrapped(source, body.clone(), "==", "==") {
                if owner == range {
                    owner = outer;
                }
                body = inner;
            }
        }
    }
    let (open, close) = if format == Format::HighlightColor {
        if let Some(color) = color {
            let hex = if color & 255 == 255 {
                format!("#{:06x}", color >> 8)
            } else {
                format!("#{color:08x}")
            };
            (
                format!("<mark style=\"background:{hex}\">"),
                "</mark>".to_owned(),
            )
        } else {
            (String::new(), String::new())
        }
    } else {
        let (open, close) = format.pair();
        let delimiter = styles
            .iter()
            .rev()
            .find(|d| {
                d.format == format
                    && (d.content() == body
                        || d.open.start == body.start && d.close.end == body.end)
            })
            .map(|d| (d.open.start..d.close.end, d.content()));
        let delimiter = if matches!(format, Format::Bold | Format::Italic | Format::Strike) {
            markdown_wrapper(root, &body, format)
        } else {
            delimiter.or_else(|| wrapped(source, body.clone(), open, close))
        };
        if let Some((outer, inner)) = delimiter {
            if owner == range {
                owner = outer;
            }
            body = inner;
            (String::new(), String::new())
        } else {
            let other = match format {
                Format::Bold => Some(Format::Italic),
                Format::Italic => Some(Format::Bold),
                _ => None,
            };
            let conversion = other
                .and_then(|other| markdown_wrapper(root, &body, other))
                .filter(|(outer, _)| *outer == body);
            if let Some((outer, inner)) = conversion {
                // A complete source owner is an explicit conversion. Selecting
                // only its text instead adds the requested mark without removing
                // the existing format.
                owner = outer;
                body = inner;
                converted = true;
            }
            (open.to_owned(), close.to_owned())
        }
    };
    let selected = if range.start <= owner.start && owner.end <= range.end && range != owner {
        range.start..range.end - owner.len() + open.len() + body.len() + close.len()
    } else if converted {
        owner.start..owner.start + open.len() + body.len() + close.len()
    } else {
        owner.start + open.len()..owner.start + open.len() + body.len()
    };
    let selection = if selection.is_reversed() {
        Selection::new(selected.end, selected.start)
    } else {
        Selection::new(selected.start, selected.end)
    };
    let mut edits = Vec::with_capacity(2);
    if source[owner.start..body.start] != open {
        edits.push((owner.start..body.start, open));
    }
    if source[body.end..owner.end] != close {
        edits.push((body.end..owner.end, close));
    }
    (!edits.is_empty()).then_some(Command::BatchReplaceSelected {
        edits,
        selection,
        kind: EditKind::Command,
    })
}

fn coverage(root: &Node, decorations: &[Decoration]) -> Vec<(Format, Range<usize>)> {
    fn collect(node: &Node, ranges: &mut Vec<(Format, Range<usize>)>) {
        if opaque_jsx(node) {
            return;
        }
        let format = match node {
            Node::Strong(_) => Some(Format::Bold),
            Node::Emphasis(_) => Some(Format::Italic),
            Node::Delete(_) => Some(Format::Strike),
            Node::InlineCode(_) => Some(Format::Code),
            _ => None,
        };
        if let Some(format) = format {
            if let Some(p) = node.position() {
                ranges.push((format, p.start.offset..p.end.offset));
            }
        }
        if let Some(children) = node.children() {
            for child in children {
                collect(child, ranges);
            }
        }
    }
    let mut ranges = decorations
        .iter()
        .map(|d| (d.format, d.open.start..d.close.end))
        .collect();
    collect(root, &mut ranges);
    ranges
}

fn state(
    source: &str,
    range: Range<usize>,
    format: Format,
    coverage: &[(Format, Range<usize>)],
) -> u8 {
    if range.start == range.end {
        return 0;
    }
    let (open, close) = format.pair();
    if !matches!(format, Format::Bold | Format::Italic | Format::Strike)
        && !open.is_empty()
        && wrapped(source, range.clone(), open, close).is_some()
    {
        return 2;
    }
    let applicable = |candidate| {
        candidate == format || format == Format::Highlight && candidate == Format::HighlightColor
    };
    if coverage
        .iter()
        .any(|(f, r)| applicable(*f) && r.start <= range.start && range.end <= r.end)
    {
        2
    } else if coverage
        .iter()
        .any(|(f, r)| applicable(*f) && r.start < range.end && range.start < r.end)
    {
        1
    } else {
        0
    }
}

#[path = "formats_ui.rs"]
mod ui;
pub(super) use ui::SetHighlightColor;
pub(super) use ui::State;
pub(in super::super) use ui::Syntax;

impl SourcePresentation {
    pub(super) fn format_command(
        &mut self,
        format: Format,
        color: Option<u32>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.dispatch_format_command(format, color, window, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::{Bold, Highlight, Italic, Underline};
    use crate::ui::app::test_window::harness_with_tabs;
    use gpui_kit::test::{TestEventExt as _, TestQueryExt as _, TestWindowExt as _};
    use gpui_kit::{Focusable as _, MouseButton, TestAppContext};

    #[gpui_kit::test]
    fn workbench_format_shortcuts_keep_selection_bytes_and_shared_history(cx: &mut TestAppContext) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("formats.md");
        let original = "\u{feff}a **b** c\r\nuntouched __spelling__\n";
        std::fs::write(&path, original).unwrap();
        let (app, mut context, _streams) = harness_with_tabs(cx, 1);
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
        view.update_in(&mut context, |view, window, cx| {
            view.focus_handle(cx).focus(window, cx);
            view.select_range(0..0, cx);
        });
        context.dispatch_action(Bold);
        view.read_with(&context, |view, cx| {
            assert_eq!(
                view.document().read(cx).encode().unwrap(),
                original.as_bytes()
            )
        });
        view.update_in(&mut context, |view, _, cx| view.select_range(0..9, cx));
        context.update(|window, cx| window.press("secondary-b", cx));
        context.run_until_parked();
        view.read_with(&context, |view, cx| {
            assert_eq!(
                view.document().read(cx).encode().unwrap(),
                "\u{feff}**a **b** c**\r\nuntouched __spelling__\n".as_bytes()
            );
            assert_eq!(view.selection().range(), 2..11);
        });
        context.dispatch_action(gpui_kit::component::input::Undo);
        context.dispatch_action(gpui_kit::component::input::Redo);
        context.dispatch_action(Bold);
        view.read_with(&context, |view, cx| {
            assert_eq!(
                view.document().read(cx).encode().unwrap(),
                original.as_bytes()
            );
        });
        view.update_in(&mut context, |view, _, cx| view.select_range(2..7, cx));
        context.dispatch_action(Italic);
        view.read_with(&context, |view, cx| {
            assert_eq!(
                view.document().read(cx).encode().unwrap(),
                "\u{feff}a *b* c\r\nuntouched __spelling__\n".as_bytes()
            );
            assert_eq!(view.selection().range(), 2..5);
        });
        context.dispatch_action(Underline);
        context.dispatch_action(Highlight);
        context.run_until_parked();
        let expected = "\u{feff}a <u>==*b*==</u> c\r\nuntouched __spelling__\n";
        view.read_with(&context, |view, cx| {
            assert_eq!(
                view.document().read(cx).encode().unwrap(),
                expected.as_bytes()
            )
        });
        // A real menu pointer command converts the plain highlight into the
        // pinned yellow highlight without touching its nested italic/underline.
        context.update(|window, cx| {
            window.render_frame(cx);
            window.click("markdown-format-menu", cx);
        });
        context.run_until_parked();
        context.update(|window, cx| {
            let label = format!(
                "{} · #ffd54f80",
                crate::ui::i18n::t(crate::ui::i18n::L10nKey::MarkdownColorYellow)
            );
            let item = window.find_by_label(&label);
            window.pointer_down(item.bounds().center(), MouseButton::Left, cx);
            window.pointer_up(item.bounds().center(), MouseButton::Left, cx);
        });
        context.run_until_parked();
        let expected = "\u{feff}a <u><mark style=\"background:#ffd54f80\">*b*</mark></u> c\r\nuntouched __spelling__\n";
        view.read_with(&context, |view, cx| {
            assert_eq!(
                view.document().read(cx).encode().unwrap(),
                expected.as_bytes()
            )
        });
        context.update(|window, cx| assert!(view.read(cx).focus_handle(cx).is_focused(window)));
        context.dispatch_action(crate::core::actions::ToggleDocumentPreview);
        context.dispatch_action(gpui_kit::component::input::Undo);
        context.dispatch_action(gpui_kit::component::input::Redo);
        context.dispatch_action(crate::core::actions::EditorSave);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            context.run_until_parked();
            if app.read_with(&context, |app, _| {
                app.active_buffer()
                    .is_some_and(|file| !file.dirty && !file.saving)
            }) {
                break;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(std::fs::read(&path).unwrap(), expected.as_bytes());
        app.update_in(&mut context, |app, window, cx| {
            app.editor_close_file(0, window, cx)
        });
        app.update_in(&mut context, |app, window, cx| {
            app.open_file_in_editor(&path, window, cx)
        });
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            context.run_until_parked();
            if let Some(bytes) = app.read_with(&context, |app, cx| {
                app.active_editor_input().and_then(|input| {
                    input
                        .markdown()
                        .map(|view| view.read(cx).document().read(cx).encode().unwrap())
                })
            }) {
                assert_eq!(bytes, expected.as_bytes());
                break;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    #[test]
    fn extension_projection_hides_only_prose_markers_and_preserves_nested_marks() {
        use super::super::super::{Selection, mdx::Grammar, projection::Projection};
        let source =
            "A <u>under</u> ==mark **bold**== and `==code==` <code>==raw **字**== &amp;</code>.";
        let mut projection = Projection::parse(source, 1, true, Grammar::Markdown);
        projection.format_theme(gpui_kit::Hsla::default());
        for leaf in &mut projection.leaves {
            leaf.contextual_reveal(Selection::new(0, source.len()), &[], None, None, None);
        }
        let leaf = projection
            .leaves
            .iter()
            .find(|leaf| leaf.text.starts_with("A "))
            .unwrap();
        assert_eq!(
            leaf.text,
            "A under mark bold and ==code== <code>==raw **字**== &amp;</code>."
        );
        let marks = leaf.marks(None);
        assert!(
            marks
                .iter()
                .any(|(range, mark)| &leaf.text[range.clone()] == "under" && mark.underline)
        );
        assert!(
            marks
                .iter()
                .any(|(range, mark)| &leaf.text[range.clone()] == "bold"
                    && mark.bold
                    && mark.highlight.is_some())
        );
        assert!(
            marks
                .iter()
                .any(|(range, mark)| &leaf.text[range.clone()] == "==code=="
                    && mark.code
                    && mark.highlight.is_none())
        );
        let opaque = leaf.text.find("<code>").unwrap()..leaf.text.find("</code>").unwrap() + 7;
        for (range, mark) in &marks {
            if range.start < opaque.end && opaque.start < range.end {
                assert!(!mark.bold && !mark.code && !mark.underline && mark.highlight.is_none());
            }
        }
        for text in ["under", "bold", "==code==", "**字**", "&amp;"] {
            let display = leaf.text.find(text).unwrap();
            let source_at = source.find(text).unwrap();
            assert_eq!(
                leaf.visible_range(display..display + text.len()),
                source_at..source_at + text.len(),
                "mixed inline content must retain its document byte range"
            );
        }
        let mdx = "<Note title=\"==attribute==\">\n\n==body== {\"==expression==\"}\n\n</Note>\n";
        let root = super::super::super::parser::parse(mdx, Grammar::Mdx, true).unwrap();
        let styles = super::decorations(mdx, &root);
        assert_eq!(styles.len(), 1);
        assert_eq!(&mdx[styles[0].content()], "body");

        let source = "A <span style=\"color:#123456\"><u>中 **bold**</u> <span style=\"color:#abcdef80\">e&#x301;🎉</span></span> tail";
        let projection = Projection::parse(source, 1, true, Grammar::Markdown);
        let leaf = &projection.leaves[0];
        assert_eq!(leaf.text, "A 中 bold e\u{301}🎉 tail");
        let ranges = leaf.foreground_ranges();
        assert_eq!(
            ranges,
            vec![
                (2..11, gpui_kit::rgba(0x123456ff).into()),
                (11..18, gpui_kit::rgba(0xabcdef80).into()),
            ]
        );
        assert!(leaf.marks(None).iter().any(|(range, mark)| {
            &leaf.text[range.clone()] == "bold" && mark.bold && mark.underline
        }));
        let source_projection = Projection::parse(source, 1, false, Grammar::Markdown);
        assert_eq!(source_projection.leaves[0].text, source);
        assert!(source_projection.leaves[0].foreground_ranges().is_empty());

        let atom_source = "A <span style=\"color:#123456\">中 $x$ 🎉</span>";
        let atom_projection = Projection::parse(atom_source, 1, true, Grammar::Markdown);
        let leaf = &atom_projection.leaves[0];
        assert_eq!(leaf.text, "A 中 \u{fffc} 🎉");
        assert_eq!(
            leaf.foreground_ranges(),
            vec![(2..14, gpui_kit::rgba(0x123456ff).into())]
        );

        let mdx = "<Note>\n\nA <span style=\"color:#123456\">中🎉</span>\n\n</Note>";
        let mut projection = Projection::parse(mdx, 1, true, Grammar::Mdx);
        let mut state = super::super::super::mdx::ViewState::default();
        let note = projection
            .mdx
            .components
            .iter()
            .find(|component| &mdx[component.name_range()] == "Note")
            .unwrap();
        state.source_components.insert(note.id());
        projection.component_sources(&super::super::super::Rope::from(mdx), &state);
        assert!(projection.leaves.iter().any(|leaf| leaf.text == mdx));
        assert!(
            projection
                .leaves
                .iter()
                .all(|leaf| leaf.foreground_ranges().is_empty())
        );
    }

    #[test]
    fn text_color_selection_clear_and_nested_recolor_keep_document_history() {
        use super::super::super::{
            Command, MarkdownDocument, PresentationState, Selection, TextFormat, mdx::Grammar,
            projection::Projection,
        };
        for grammar in [Grammar::Markdown, Grammar::Mdx] {
            for (source, selected, color, expected) in [
                (
                    "A <span style=\"color:red\">left middle right</span> Z",
                    "middle",
                    None,
                    "A <span style=\"color:red\">left </span>middle<span style=\"color:red\"> right</span> Z",
                ),
                (
                    "A <span style=\"color:red\">left <span style=\"color:blue\">middle</span> right</span> Z",
                    "middle",
                    None,
                    "A <span style=\"color:red\">left </span>middle<span style=\"color:red\"> right</span> Z",
                ),
                (
                    "A <span style=\"color:red\"><u>left <span style=\"color:blue\">middle</span> right</u></span> Z",
                    "<u>left <span style=\"color:blue\">middle</span> right</u>",
                    Some(0x7c3aed80),
                    "A <span style=\"color:#7c3aed80\"><u>left <span style=\"color:#7c3aed80\">middle</span> right</u></span> Z",
                ),
            ] {
                let root = super::super::super::parser::parse(source, grammar, true).unwrap();
                let styles = super::decorations(source, &root);
                let at = source.find(selected).unwrap();
                let selection = Selection::new(at + selected.len(), at);
                let command = super::edit(
                    source,
                    &root,
                    selection,
                    super::Format::TextColor,
                    color,
                    &styles,
                )
                .unwrap();
                let mut document = MarkdownDocument::new(source.into(), TextFormat::default());
                let mut state = PresentationState::new(selection);
                document.command(&mut state, command);
                assert_eq!(document.encode().unwrap(), expected.as_bytes());
                assert!(state.selection().is_reversed());
                let projection = Projection::parse(expected, 1, true, grammar);
                assert_eq!(projection.leaves[0].text, "A left middle right Z");
                document.command(&mut state, Command::Undo);
                assert_eq!(document.encode().unwrap(), source.as_bytes());
                assert_eq!(state.selection(), selection);
                document.command(&mut state, Command::Redo);
                assert_eq!(document.encode().unwrap(), expected.as_bytes());
            }
        }
    }

    #[test]
    fn text_color_partial_nested_marks_keep_unicode_and_original_undo_bytes() {
        use super::super::super::{
            Command, MarkdownDocument, PresentationState, Selection, TextFormat, mdx::Grammar,
            projection::Projection,
        };
        let source =
            "A <span style=\"color:red\" data-keep=\"原样\">**left *中🎉 middle* right**</span> Z";
        for grammar in [Grammar::Markdown, Grammar::Mdx] {
            for (color, expected) in [
                (
                    None,
                    "A <span style=\"color:inherit\" data-keep=\"原样\">**<span style=\"color:#ff0000\">left </span>*中🎉<span style=\"color:#ff0000\"> middle</span>*<span style=\"color:#ff0000\"> right</span>**</span> Z",
                ),
                (
                    Some(0x7c3aed80),
                    "A <span style=\"color:inherit\" data-keep=\"原样\">**<span style=\"color:#ff0000\">left </span>*<span style=\"color:#7c3aed80\">中🎉</span><span style=\"color:#ff0000\"> middle</span>*<span style=\"color:#ff0000\"> right</span>**</span> Z",
                ),
            ] {
                let root = super::super::super::parser::parse(source, grammar, true).unwrap();
                let styles = super::decorations(source, &root);
                let at = source.find('中').unwrap();
                let selection = Selection::new(at, at + "中🎉".len());
                let command = super::edit(
                    source,
                    &root,
                    selection,
                    super::Format::TextColor,
                    color,
                    &styles,
                )
                .unwrap();
                let mut document = MarkdownDocument::new(source.into(), TextFormat::default());
                let mut state = PresentationState::new(selection);
                document.command(&mut state, command);
                assert_eq!(document.encode().unwrap(), expected.as_bytes());
                let projection = Projection::parse(expected, 1, true, grammar);
                let leaf = &projection.leaves[0];
                assert_eq!(leaf.text, "A left 中🎉 middle right Z");
                let glyph = leaf.text.find('中').unwrap();
                assert!(leaf.marks(None).iter().any(|(range, mark)| {
                    range.start <= glyph
                        && glyph + "中🎉".len() <= range.end
                        && mark.bold
                        && mark.italic
                }));
                assert_eq!(
                    leaf.foreground_ranges()
                        .iter()
                        .find(|(range, _)| {
                            range.start <= glyph && glyph + "中🎉".len() <= range.end
                        })
                        .map(|(_, color)| *color),
                    color.map(|color| gpui_kit::rgba(color).into())
                );
                document.command(&mut state, Command::Undo);
                assert_eq!(document.encode().unwrap(), source.as_bytes());
                assert_eq!(state.selection(), selection);
                document.command(&mut state, Command::Redo);
                assert_eq!(document.encode().unwrap(), expected.as_bytes());
            }
        }
    }

    #[test]
    fn text_color_crossing_bold_boundary_keeps_bold_and_plain_text() {
        use super::super::super::{
            MarkdownDocument, PresentationState, Selection, TextFormat, mdx::Grammar,
            projection::Projection,
        };
        let source = "A **left 中🎉** plain Z";
        let start = source.find('中').unwrap();
        let end = source.find(" Z").unwrap();
        let root = super::super::super::parser::parse(source, Grammar::Markdown, true).unwrap();
        let styles = super::decorations(source, &root);
        let selection = Selection::new(start, end);
        let mut state = PresentationState::new(selection);
        let mut document = MarkdownDocument::new(source.into(), TextFormat::default());
        document.command(
            &mut state,
            super::edit(
                source,
                &root,
                selection,
                super::Format::TextColor,
                Some(0x123456ff),
                &styles,
            )
            .unwrap(),
        );
        let expected = "A **left <span style=\"color:#123456\">中🎉</span>**<span style=\"color:#123456\"> plain</span> Z";
        assert_eq!(document.encode().unwrap(), expected.as_bytes());
        let projection = Projection::parse(expected, 1, true, Grammar::Markdown);
        let leaf = &projection.leaves[0];
        assert_eq!(leaf.text, "A left 中🎉 plain Z");
        let glyph = leaf.text.find('中').unwrap();
        assert!(
            leaf.marks(None)
                .iter()
                .any(|(range, mark)| range.contains(&glyph) && mark.bold)
        );
        let plain = leaf.text.find("plain").unwrap();
        assert!(
            leaf.marks(None)
                .iter()
                .any(|(range, mark)| range.contains(&plain) && !mark.bold)
        );
        assert_eq!(
            leaf.foreground_ranges(),
            vec![(
                glyph..leaf.text.find(" Z").unwrap(),
                gpui_kit::rgba(0x123456ff).into()
            )]
        );
    }

    #[test]
    fn text_color_block_mark_and_attribute_boundaries_preserve_semantics_and_undo() {
        use super::super::super::{
            Command, MarkdownDocument, PresentationState, Selection, TextFormat, mdx::Grammar,
            projection::Projection,
        };
        for (source, selected, color, expected, text) in [
            (
                "a\n\nb",
                "a\n\nb",
                Some(0x123456ff),
                "<span style=\"color:#123456\">a</span>\n\n<span style=\"color:#123456\">b</span>",
                "ab",
            ),
            (
                "*ab*",
                "*a",
                Some(0x123456ff),
                "*<span style=\"color:#123456\">a</span>b*",
                "ab",
            ),
            (
                "*ab*",
                "b*",
                Some(0x123456ff),
                "*a<span style=\"color:#123456\">b</span>*",
                "ab",
            ),
            (
                "<span data-keep='x' style='background:#fff; COLOR : red ;'>ab</span>",
                "ab",
                Some(0x123456ff),
                "<span data-keep='x' style='background:#fff; COLOR : #123456 ;'>ab</span>",
                "ab",
            ),
        ] {
            let at = source.find(selected).unwrap();
            let selection = Selection::new(at, at + selected.len());
            let root = super::super::super::parser::parse(source, Grammar::Markdown, true).unwrap();
            let styles = super::decorations(source, &root);
            let mut document = MarkdownDocument::new(source.into(), TextFormat::default());
            let mut state = PresentationState::new(selection);
            document.command(
                &mut state,
                super::edit(
                    source,
                    &root,
                    selection,
                    super::Format::TextColor,
                    color,
                    &styles,
                )
                .unwrap(),
            );
            assert_eq!(document.encode().unwrap(), expected.as_bytes());
            let projection = Projection::parse(expected, 1, true, Grammar::Markdown);
            assert_eq!(
                projection
                    .leaves
                    .iter()
                    .map(|leaf| leaf.text.as_str())
                    .collect::<String>(),
                text
            );
            if source == "*ab*" {
                let leaf = &projection.leaves[0];
                assert!(leaf.marks(None).iter().all(|(_, mark)| mark.italic));
                assert_eq!(
                    leaf.foreground_ranges(),
                    vec![(
                        if selected == "*a" { 0..1 } else { 1..2 },
                        gpui_kit::rgba(0x123456ff).into()
                    )]
                );
            } else {
                for leaf in &projection.leaves {
                    let expected = if leaf.text.is_empty() {
                        Vec::new()
                    } else {
                        vec![(0..leaf.text.len(), gpui_kit::rgba(0x123456ff).into())]
                    };
                    assert_eq!(
                        leaf.foreground_ranges(),
                        expected,
                        "source {source:?}, leaf {:?}",
                        leaf.text
                    );
                }
            }
            document.command(&mut state, Command::Undo);
            assert_eq!(document.encode().unwrap(), source.as_bytes());
            assert_eq!(state.selection(), selection);
            document.command(&mut state, Command::Redo);
            assert_eq!(document.encode().unwrap(), expected.as_bytes());
        }
        let source = "<span data-keep='x' style='background:#fff; COLOR : red ;'>ab</span>";
        for (color, selected) in [
            (None, "b"),
            (Some(0x123456ff), "<span style=\"color:#123456\">b</span>"),
        ] {
            let at = source.find(">ab").unwrap() + 2;
            let selection = Selection::new(at, at + 1);
            let root = super::super::super::parser::parse(source, Grammar::Markdown, true).unwrap();
            let styles = super::decorations(source, &root);
            let mut document = MarkdownDocument::new(source.into(), TextFormat::default());
            let mut state = PresentationState::new(selection);
            document.command(
                &mut state,
                super::edit(
                    source,
                    &root,
                    selection,
                    super::Format::TextColor,
                    color,
                    &styles,
                )
                .unwrap(),
            );
            let expected = format!(
                "<span data-keep='x' style='background:#fff; COLOR : inherit ;'><span style=\"color:#ff0000\">a</span>{selected}</span>"
            );
            assert_eq!(document.encode().unwrap(), expected.as_bytes());
            let projection = Projection::parse(&expected, 1, true, Grammar::Markdown);
            assert_eq!(projection.leaves[0].text, "ab");
            assert_eq!(
                projection.leaves[0].foreground_ranges(),
                color.map_or_else(
                    || vec![(0..1, gpui_kit::rgba(0xff0000ff).into())],
                    |color| vec![
                        (0..1, gpui_kit::rgba(0xff0000ff).into()),
                        (1..2, gpui_kit::rgba(color).into())
                    ],
                )
            );
            document.command(&mut state, Command::Undo);
            assert_eq!(document.encode().unwrap(), source.as_bytes());
            assert_eq!(state.selection(), selection);
        }
        let unknown = "before <span style=\"white-space:pre\">**raw**</span> after";
        let projection = Projection::parse(unknown, 1, true, Grammar::Markdown);
        assert_eq!(projection.leaves[0].text, unknown);
        assert!(
            projection.leaves[0]
                .marks(None)
                .iter()
                .all(|(_, mark)| !mark.bold)
        );
    }

    #[test]
    fn mixed_inline_html_preserves_opaque_source_and_adjacent_prose_marks() {
        use super::super::super::{mdx::Grammar, projection::Projection};

        for prefix in ["", "# ", "- ", "> "] {
            for tag in ["code", "pre", "script", "style", "textarea"] {
                let opaque =
                    format!("<{tag}>outer <{tag}>inner</{tag}> ==raw **字**== &amp;</{tag}>");
                let source = format!("{prefix}before **strong** {opaque} after *em*");
                let projection = Projection::parse(&source, 1, true, Grammar::Markdown);
                let leaf = projection
                    .leaves
                    .iter()
                    .find(|leaf| leaf.text.starts_with("before "))
                    .unwrap_or_else(|| panic!("missing prose for {source:?}"));
                assert_eq!(leaf.text, format!("before strong {opaque} after em"));
                let marks = leaf.marks(None);
                assert!(
                    marks.iter().any(|(range, mark)| {
                        &leaf.text[range.clone()] == "strong" && mark.bold
                    })
                );
                assert!(
                    marks
                        .iter()
                        .any(|(range, mark)| { &leaf.text[range.clone()] == "em" && mark.italic })
                );
                let start = leaf.text.find(&opaque).unwrap();
                let source_at = source.find(&opaque).unwrap();
                assert_eq!(
                    leaf.visible_range(start..start + opaque.len()),
                    source_at..source_at + opaque.len()
                );
                for (range, mark) in &marks {
                    if range.start < start + opaque.len() && start < range.end {
                        assert!(
                            !mark.bold && !mark.italic && !mark.code && mark.highlight.is_none(),
                            "opaque HTML must not inherit markup parsed from its content: {source:?}"
                        );
                    }
                }
            }
        }

        let source = "before **strong** <code>**literal** ==raw== &amp;";
        let projection = Projection::parse(source, 1, true, Grammar::Markdown);
        assert_eq!(
            projection.leaves[0].text,
            "before strong <code>**literal** ==raw== &amp;"
        );
    }
}
