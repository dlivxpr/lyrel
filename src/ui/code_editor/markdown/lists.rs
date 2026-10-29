use super::{EditKind, MarkdownDocument, PresentationState, Selection};
use gpui_kit::base::input::RopeExt as _;
use std::ops::Range;

gpui_kit::actions!(markdown_lists, [ToggleTask]);
struct Bindings;
impl gpui_kit::Global for Bindings {}
pub(super) fn init(cx: &mut gpui_kit::App) {
    if !cx.has_global::<Bindings>() {
        cx.bind_keys([gpui_kit::KeyBinding::new("secondary-enter", ToggleTask, Some("Input && Markdown"))]);
        cx.set_global(Bindings);
    }
}

/// Presentation-only facts from a container's AST ancestry. Source spelling is
/// always read from the document when a structural command is executed.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub(super) struct Structure {
    pub depth: usize,
    pub quotes: usize,
    pub start: usize,
    pub marker: Option<Marker>,
    pub task: Option<bool>,
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Marker {
    Bullet,
    Ordered(u32, u8),
}

struct Prefix {
    content: usize,
    indent: Range<usize>,
    marker: Option<Range<usize>>,
    task: Option<usize>,
    quotes: smallvec::SmallVec<[Range<usize>; 2]>,
}
impl Prefix {
    // This only reads a line's exact source spelling after the AST identified
    // the container. It is not a second Markdown parser or a source-position search.
    fn read(line: &str) -> Self {
        let bytes = line.as_bytes();
        let mut at = 0;
        let mut quotes = smallvec::SmallVec::new();
        loop {
            let start = at;
            while matches!(bytes.get(at), Some(b' ' | b'\t')) {
                at += 1;
            }
            if bytes.get(at) != Some(&b'>') {
                at = start;
                break;
            }
            at += 1;
            if matches!(bytes.get(at), Some(b' ' | b'\t')) {
                at += 1;
            }
            quotes.push(start..at);
        }
        let indent_start = at;
        while matches!(bytes.get(at), Some(b' ' | b'\t')) {
            at += 1;
        }
        let indent = indent_start..at;
        let marker_start = at;
        let marker_end = match bytes.get(at) {
            Some(b'-' | b'+' | b'*') => Some(at + 1),
            Some(b'0'..=b'9') => {
                while bytes.get(at).is_some_and(u8::is_ascii_digit) {
                    at += 1;
                }
                (at - marker_start <= 9 && matches!(bytes.get(at), Some(b'.' | b')')))
                    .then_some(at + 1)
            }
            _ => None,
        };
        let marker = marker_end.filter(|end| {
            *end == bytes.len() || matches!(bytes.get(*end), Some(b' ' | b'\t'))
        }).map(|end| {
            at = end;
            while matches!(bytes.get(at), Some(b' ' | b'\t')) {
                at += 1;
            }
            marker_start..at
        });
        if marker.is_none() {
            at = indent.end;
        }
        let task = (marker.is_some()
            && bytes.get(at) == Some(&b'[')
            && matches!(bytes.get(at + 1), Some(b' ' | b'x' | b'X'))
            && bytes.get(at + 2) == Some(&b']')
            && (at + 3 == bytes.len() || matches!(bytes.get(at + 3), Some(b' ' | b'\t'))))
            .then_some(at + 1);
        if task.is_some() {
            at += 3;
            while matches!(bytes.get(at), Some(b' ' | b'\t')) {
                at += 1;
            }
        }
        // Quotes can also be children of a list item (`- > prose`). Their
        // prefix belongs to the same item, not to a new sibling on Enter.
        if marker.is_some() {
            while bytes.get(at) == Some(&b'>') {
                let start = at;
                at += 1;
                if matches!(bytes.get(at), Some(b' ' | b'\t')) { at += 1; }
                quotes.push(start..at);
            }
        }
        Self { content: at, indent, marker, task, quotes }
    }
    fn is_structure(&self) -> bool {
        self.marker.is_some() || !self.quotes.is_empty()
    }
    fn marker_style(&self, line: &str) -> Option<Marker> {
        let range = self.marker.as_ref()?;
        let token = line[range.clone()].trim_end();
        let first = token.as_bytes()[0];
        Some(if first.is_ascii_digit() {
            Marker::Ordered(token[..token.len() - 1].parse().ok()?, *token.as_bytes().last()?)
        } else {
            Marker::Bullet
        })
    }
}

pub(super) fn style(source: &str, at: usize, depth: usize, quotes: usize, owner: usize) -> Structure {
    let start = source[..at].rfind('\n').map_or(0, |ix| ix + 1);
    let end = source[start..].find('\n').map_or(source.len(), |ix| start + ix);
    let line = &source[start..end];
    let prefix = Prefix::read(line);
    Structure {
        depth,
        quotes,
        start: owner,
        marker: prefix.marker_style(line),
        task: prefix.task.map(|at| line.as_bytes()[at] != b' '),
    }
}

pub(super) fn prefix_len(line: &str) -> usize {
    Prefix::read(line.split('\n').next().unwrap_or_default()).content
}

fn line(document: &MarkdownDocument, at: usize) -> (usize, String, Prefix) {
    let row = document.source().offset_to_point(at).row;
    let start = document.source().line_start_offset(row);
    let end = document.source().line_end_offset(row);
    let text = document.source().slice(start..end).to_string();
    let prefix = Prefix::read(&text);
    (start, text, prefix)
}

pub(super) fn line_break(document: &mut MarkdownDocument, state: &mut PresentationState, hard: bool, at: Option<usize>) {
    let selected = state.selection().range();
    let (start, text, prefix) = line(document, selected.start);
    let content = start + prefix.content;
    let (text, prefix) = if prefix.marker.is_none() && !text[prefix.content..].trim().is_empty() {
        at.filter(|at| *at < start).map(|at| { let (_, text, prefix) = line(document, at); (text, prefix) }).unwrap_or((text, prefix))
    } else { (text, prefix) };
    if !prefix.is_structure() || selected.start < content {
        document.edit(state, selected, "\n".into(), EditKind::Command);
        return;
    }
    let inner_quote = prefix.marker.as_ref().is_some_and(|marker| {
        prefix.quotes.last().is_some_and(|quote| quote.start > marker.start)
    });
    if !hard && selected.is_empty() && text[prefix.content..].trim().is_empty() {
        if prefix.marker.is_some() && !inner_quote && !prefix.indent.is_empty() {
            indent(document, state, true, 2, None);
            return;
        }
        let remove = if inner_quote {
            prefix.quotes.last().unwrap().clone()
        } else if let Some(marker) = &prefix.marker {
            marker.start..prefix.content
        } else { prefix.quotes.last().unwrap().clone() };
        document.edit(state, start + remove.start..start + remove.end, String::new(), EditKind::Command);
        return;
    }
    let mut continuation = text[..prefix.content].to_owned();
    if hard || inner_quote {
        if let Some(marker) = &prefix.marker {
            let end = if inner_quote {
                prefix.quotes.iter().find(|quote| quote.start > marker.start).unwrap().start
            } else { prefix.content };
            continuation.replace_range(marker.start..end, &" ".repeat(end - marker.start));
        }
    } else {
        if let Some(at) = prefix.task {
            continuation.replace_range(at..at + 1, " ");
        }
        if let Some(Marker::Ordered(number, delimiter)) = prefix.marker_style(&text) {
            let marker = prefix.marker.as_ref().unwrap();
            let digits = text[marker.clone()].bytes().take_while(u8::is_ascii_digit).count();
            let next = if text.as_bytes()[marker.start] == b'0' && digits > 1 {
                format!("{:0digits$}", number.saturating_add(1))
            } else { number.saturating_add(1).to_string() };
            continuation.replace_range(marker.start..marker.start + digits + 1, &format!("{next}{}", delimiter as char));
        }
    }
    document.edit(state, selected, format!("{}\n{continuation}", if hard { "  " } else { "" }), EditKind::Command);
}

fn outdent_width(indent: &str, width: usize) -> usize {
    if indent.starts_with('\t') { 1 } else { indent.bytes().take(width).take_while(|byte| *byte == b' ').count() }
}

pub(super) fn backspace(document: &mut MarkdownDocument, state: &mut PresentationState) {
    let selected = state.selection().range();
    let (start, _, prefix) = line(document, selected.start);
    if selected.is_empty() && selected.start == start + prefix.content && prefix.is_structure() {
        if !prefix.indent.is_empty() && prefix.marker.is_some() {
            indent(document, state, true, 2, None);
            return;
        }
        let previous = start.checked_sub(1).map(|at| line(document, at).2);
        let compatible = previous.as_ref().is_some_and(|previous| {
            previous.is_structure() && previous.quotes.len() == prefix.quotes.len()
        });
        if compatible {
            document.edit(state, start - 1..start + prefix.content, String::new(), EditKind::Command);
        } else {
            let inner_quote = prefix.quotes.last().filter(|quote| prefix.marker.as_ref().is_none_or(|marker| quote.start > marker.start));
            let remove = if let Some(quote) = inner_quote {
                quote.clone()
            } else if let Some(marker) = &prefix.marker {
                marker.start..prefix.content
            } else { return; };
            document.edit(state, start + remove.start..start + remove.end, String::new(), EditKind::Command);
        }
    } else {
        let range = if selected.is_empty() { document.previous(selected.start)..selected.end } else { selected };
        document.edit(state, range, String::new(), EditKind::Command);
    }
}

pub(super) fn indent(document: &mut MarkdownDocument, state: &mut PresentationState, outdent: bool, width: usize, at: Option<usize>) {
    let selected = state.selection().range();
    let first_row = document.source().offset_to_point(at.unwrap_or(selected.start).min(selected.start)).row;
    let mut last_row = document.source().offset_to_point(selected.end).row;
    if last_row > first_row && document.source().line_start_offset(last_row) == selected.end { last_row -= 1; }
    let (_, _, first) = line(document, document.source().line_start_offset(first_row));
    if !first.is_structure() { return; }
    let mut step = width.max(1);
    let mut parent = false;
    if first.marker.is_some() {
        // Match the preceding item's source convention, not EditorConfig's
        // unrelated indentation default. Existing markers are never rewritten.
        for row in (0..first_row).rev() {
            let (_, text, previous) = line(document, document.source().line_start_offset(row));
            if text.trim().is_empty() { continue; }
            if previous.quotes.len() != first.quotes.len() { break; }
            if let Some(marker) = &previous.marker {
                if outdent && previous.indent.len() < first.indent.len() {
                    step = first.indent.len() - previous.indent.len();
                    break;
                }
                if !outdent && previous.indent.len() == first.indent.len() {
                    step = marker.len();
                    parent = true;
                    break;
                }
            }
            if previous.indent.len() < first.indent.len() { break; }
        }
        if !outdent && !parent { return; }
        if outdent { step = step.min(first.indent.len()); }
    }
    // The last explicitly selected item owns its descendants. Move that
    // subtree with it, stopping before the next sibling's untouched marker.
    let (_, _, last) = line(document, document.source().line_start_offset(last_row));
    let tail = if last.marker.is_none() && first.marker.is_some() { &first } else { &last };
    let mut row = last_row + 1;
    loop {
        let start = document.source().line_start_offset(row);
        if start >= document.source().len() { break; }
        let (_, text, candidate) = line(document, start);
        if text.trim().is_empty() { row += 1; continue; }
        let descendant = candidate.quotes.len() > tail.quotes.len()
            || (candidate.quotes.len() == tail.quotes.len() && candidate.indent.len() > tail.indent.len());
        if !descendant { break; }
        last_row = row;
        row += 1;
    }
    let mut edits = Vec::new();
    for row in first_row..=last_row {
        let start = document.source().line_start_offset(row);
        let (_, text, prefix) = line(document, start);
        if text.trim().is_empty() { continue; }
        if outdent {
            if first.marker.is_some() {
                let remove = outdent_width(&text[prefix.indent.clone()], step);
                if remove > 0 { edits.push((start + prefix.indent.start..start + prefix.indent.start + remove, String::new())); }
            } else if let Some(quote) = prefix.quotes.get(first.quotes.len().saturating_sub(1)) {
                edits.push((start + quote.start..start + quote.end, String::new()));
            }
        } else {
            let insertion = start + prefix.indent.start;
            edits.push((insertion..insertion, if first.marker.is_some() { " ".repeat(step) } else { "> ".into() }));
        }
    }
    let mut after = state.selection();
    edits.sort_by_key(|(range, _)| std::cmp::Reverse(range.start));
    for (range, text) in &edits {
        let map = |at: usize| if at < range.start { at } else if at >= range.end { at - range.len() + text.len() } else { range.start + text.len() };
        after = Selection::new(map(after.anchor), map(after.head));
    }
    document.batch_edit_selected(state, edits, EditKind::Command, Some(after));
}

pub(super) fn toggle(document: &mut MarkdownDocument, state: &mut PresentationState, at: usize) {
    let (start, text, prefix) = line(document, at);
    if let Some(task) = prefix.task {
        document.batch_edit_selected(state, vec![(start + task..start + task + 1, if text.as_bytes()[task] == b' ' { "x" } else { " " }.into())], EditKind::Command, Some(state.selection()));
    }
}
