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
                    }
                }
            }
        }
    }
    if format == Format::TextColor && color.is_none() {
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
    let mut owner = range.clone();
    let mut body = range.clone();
    if matches!(
        format,
        Format::TextColor | Format::HighlightColor | Format::Highlight
    ) {
        let want = if format == Format::TextColor {
            Format::TextColor
        } else {
            Format::HighlightColor
        };
        if let Some(d) = styles.iter().find(|d| {
            (d.format == want
                || want == Format::HighlightColor
                    && d.format == Format::Highlight
                    && source[d.open.clone()].starts_with('<'))
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
    let (open, close) = if matches!(format, Format::TextColor | Format::HighlightColor) {
        if let Some(color) = color {
            let hex = if color & 255 == 255 {
                format!("#{:06x}", color >> 8)
            } else {
                format!("#{color:08x}")
            };
            if format == Format::TextColor {
                (
                    format!("<span style=\"color:{hex}\">"),
                    "</span>".to_owned(),
                )
            } else {
                (
                    format!("<mark style=\"background:{hex}\">"),
                    "</mark>".to_owned(),
                )
            }
        } else {
            (String::new(), String::new())
        }
    } else {
        let (open, close) = format.pair();
        if let Some((outer, inner)) = wrapped(source, body.clone(), open, close) {
            if owner == range {
                owner = outer;
            }
            body = inner;
            (String::new(), String::new())
        } else {
            (open.to_owned(), close.to_owned())
        }
    };
    let selected = owner.start + open.len()..owner.start + open.len() + body.len();
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
    if !open.is_empty() && wrapped(source, range.clone(), open, close).is_some() {
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

impl SourcePresentation {
    pub(super) fn format_command(
        &mut self,
        format: Format,
        color: Option<u32>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.is_composing() || !self.extra_selections.is_empty() {
            return;
        }
        if let Some(command) = self.plan_format(format, color, cx) {
            self.command(command, window, cx);
        }
        self.focus.focus(window, cx);
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
        view.update_in(&mut context, |view, _, cx| view.select_range(2..7, cx));
        context.dispatch_action(Italic);
        view.read_with(&context, |view, cx| {
            assert_eq!(
                view.document().read(cx).encode().unwrap(),
                "\u{feff}a *b* c\r\nuntouched __spelling__\n".as_bytes()
            )
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
            leaf.contextual_reveal(Selection::new(0, source.len()), &[], None, None);
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
    }

    #[test]
    fn mixed_inline_html_preserves_opaque_source_and_adjacent_prose_marks() {
        use super::super::super::{mdx::Grammar, projection::Projection};

        for prefix in ["", "# ", "- ", "> "] {
            for tag in ["code", "pre", "script", "style", "textarea"] {
                let opaque = format!(
                    "<{tag}>outer <{tag}>inner</{tag}> ==raw **字**== &amp;</{tag}>"
                );
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
