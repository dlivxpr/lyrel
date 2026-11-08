//! Positioned block commands. Only prefix/wrapper spans are rewritten; body
//! bytes and their original per-line terminators stay in MarkdownDocument.
use super::{Command, Selection};
use markdown::mdast::Node;
use std::ops::Range;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Kind { Text, Heading(u8), Quote, Callout(&'static str), Bullet, Numbered, Task, Code, Math }

#[derive(Clone)]
pub(super) struct Block {
    pub(super) range: Range<usize>,
    pub(super) kind: Kind,
}
pub(super) struct Conversion {
    edits: Vec<(Range<usize>, String)>,
    caret: usize,
}
impl Conversion {
    pub(super) fn command(mut self, base: usize) -> Command {
        for (range, _) in &mut self.edits { *range = range.start + base..range.end + base; }
        Command::BatchReplaceSelected { edits: self.edits, selection: Selection::caret(base + self.caret) }
    }
}
#[derive(Clone)]
pub(super) struct Fold {
    pub(super) header: Range<usize>,
    pub(super) body: Range<usize>,
    pub(super) persisted: Option<bool>,
    pub(super) marker: Option<Range<usize>>,
}
#[derive(Default)]
pub(super) struct Index {
    pub(super) blocks: Vec<Block>,
    pub(super) folds: Vec<Fold>,
    pub(super) ready: bool,
}

impl Index {
    pub(super) fn from_ast(source: &str, root: &Node) -> Self {
        let mut index = Self::default();
        index.collect(source, root);
        index.ready = true;
        for (ix, block) in index.blocks.iter().enumerate() {
            let Kind::Heading(level) = block.kind else { continue };
            let end = index.blocks[ix + 1..].iter().find(|next| matches!(next.kind, Kind::Heading(other) if other <= level))
                .map_or(source.len(), |next| next.range.start);
            let header_end = line_end(source, block.range.start);
            let body = (header_end + usize::from(header_end < source.len())).min(end)..end;
            if !body.is_empty() {
                index.folds.push(Fold { header: block.range.start..header_end, body, persisted: None, marker: None });
            }
        }
        index
    }
    fn collect(&mut self, source: &str, node: &Node) {
        let kind = match node {
            Node::Heading(value) => Some(Kind::Heading(value.depth)),
            Node::Paragraph(_) => Some(Kind::Text),
            Node::Blockquote(_) => Some(Kind::Quote),
            Node::Code(_) => Some(Kind::Code),
            Node::Math(_) => Some(Kind::Math),
            Node::ListItem(value) => Some(if value.checked.is_some() { Kind::Task } else if node.position().is_some_and(|position| source.as_bytes().get(position.start.offset).is_some_and(u8::is_ascii_digit)) { Kind::Numbered } else { Kind::Bullet }),
            _ => None,
        };
        if let (Some(kind), Some(position)) = (kind, node.position()) {
            let start = line_start(source, position.start.offset);
            let end = if matches!(node, Node::ListItem(_)) { line_end(source, start) } else { line_end(source, position.end.offset) };
            let mut kind = kind;
            if kind == Kind::Quote {
                let first_end = line_end(source, start);
                let prefix = quote_prefix(&source[start..first_end]);
                let content = &source[start + prefix..first_end];
                let callout = callout_token(content);
                if let Some((name, _)) = callout { kind = Kind::Callout(name); }
                let marker = callout.and_then(|(_, length)| {
                    let at = start + prefix + length;
                    matches!(source.as_bytes().get(at), Some(b'+' | b'-')).then_some(at..at + 1)
                });
                let body_start = (first_end + usize::from(first_end < source.len())).min(end);
                if body_start < end || matches!(kind, Kind::Callout(_)) {
                    self.folds.push(Fold {
                        header: start..first_end,
                        body: body_start..end,
                        persisted: marker.as_ref().map(|range| &source[range.clone()] == "-"),
                        marker,
                    });
                }
            }
            self.blocks.push(Block { range: start..end, kind });
        }
        // MDX/HTML/math/code are opaque to block commands. Container children
        // come from the same positioned parser used by the live projection.
        if matches!(node, Node::Root(_) | Node::Blockquote(_) | Node::List(_) | Node::ListItem(_)) {
            if let Some(children) = node.children() {
                for child in children { self.collect(source, child); }
            }
        }
    }
    pub(super) fn at(&self, at: usize) -> Option<&Block> {
        // Reference conversions operate on an entire quote or fenced block,
        // never the current selection and never the neighboring block.
        self.blocks.iter().filter(|block| block.range.start <= at && at <= block.range.end)
            .find(|block| matches!(block.kind, Kind::Quote | Kind::Callout(_) | Kind::Code | Kind::Math))
            .or_else(|| self.blocks.iter().find(|block| block.range.start <= at && at <= block.range.end && matches!(block.kind, Kind::Bullet | Kind::Numbered | Kind::Task)))
            .or_else(|| self.blocks.iter().rev().find(|block| block.range.start <= at && at <= block.range.end))
    }
    pub(super) fn rebase(&mut self, transaction: &super::Transaction) {
        self.ready = false;
        let map = |range: &Range<usize>| transaction.map_selection(Selection::new(range.start, range.end)).range();
        for block in &mut self.blocks { block.range = map(&block.range); }
        for fold in &mut self.folds {
            fold.header = map(&fold.header);
            fold.body = map(&fold.body);
            if let Some(marker) = &mut fold.marker { *marker = map(marker); }
        }
    }
}

pub(super) fn line_start(source: &str, at: usize) -> usize { source[..at].rfind('\n').map_or(0, |ix| ix + 1) }
pub(super) fn line_end(source: &str, at: usize) -> usize { source[at..].find('\n').map_or(source.len(), |ix| at + ix) }
fn quote_prefix(line: &str) -> usize {
    let indent = line.bytes().take_while(|byte| matches!(byte, b' ' | b'\t')).count();
    if line.as_bytes().get(indent) == Some(&b'>') { indent + 1 + usize::from(line.as_bytes().get(indent + 1) == Some(&b' ')) } else { 0 }
}
fn callout_token(line: &str) -> Option<(&'static str, usize)> {
    [("NOTE", "[!NOTE]"), ("TIP", "[!TIP]"), ("IMPORTANT", "[!IMPORTANT]"), ("WARNING", "[!WARNING]"), ("CAUTION", "[!CAUTION]")].into_iter().find_map(|(name, token)| {
        line.get(..token.len()).is_some_and(|prefix| prefix.eq_ignore_ascii_case(token)).then_some((name, token.len()))
    })
}
fn dressing(line: &str) -> usize {
    let indent = line.bytes().take_while(|byte| matches!(byte, b' ' | b'\t')).count();
    let rest = &line[indent..];
    let hashes = rest.bytes().take_while(|byte| *byte == b'#').count();
    if (1..=6).contains(&hashes) && rest.as_bytes().get(hashes) == Some(&b' ') { return indent + hashes + 1; }
    let marker = if rest.starts_with("- ") || rest.starts_with("+ ") || rest.starts_with("* ") { 2 } else {
        let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
        if digits > 0 && matches!(rest.as_bytes().get(digits), Some(b'.' | b')')) && rest.as_bytes().get(digits + 1) == Some(&b' ') { digits + 2 } else { 0 }
    };
    if marker > 0 {
        return indent + marker + if rest[marker..].starts_with("[ ] ") || rest[marker..].starts_with("[x] ") || rest[marker..].starts_with("[X] ") { 4 } else { 0 };
    }
    0
}

pub(super) fn convert(source: &str, block: &Block, target: Kind, at: usize) -> Option<Conversion> {
    if block.kind == target { return None; }
    if let (Kind::Callout(_), Kind::Callout(name)) = (block.kind, target) {
        let end = line_end(source, block.range.start);
        let prefix = quote_prefix(&source[block.range.start..end]);
        let start = block.range.start + prefix;
        let (_, len) = callout_token(&source[start..end])?;
        let text = format!("[!{name}]");
        let mapped = if at >= start + len { at - len + text.len() } else { at };
        return Some(Conversion { edits: vec![(start..start + len, text)], caret: mapped });
    }
    let mut edits = Vec::new();
    let mut body = block.range.clone();
    let terminated = source.as_bytes().get(body.end.saturating_sub(1)) == Some(&b'\n');
    if terminated { body.end -= 1; }
    let mut indented_code = false;
    if matches!(block.kind, Kind::Code | Kind::Math) {
        let opening_end = line_end(source, body.start);
        let opening = source[body.start..opening_end].trim_start();
        let delimiter = if opening.starts_with("```") { '`' } else if opening.starts_with("~~~") { '~' } else if opening.starts_with("$$") { '$' } else { '\0' };
        indented_code = block.kind == Kind::Code && delimiter == '\0';
        if delimiter != '\0' {
            let width = opening.chars().take_while(|ch| *ch == delimiter).count();
            if opening_end == body.end { return None; }
            let closing_start = line_start(source, body.end);
            let closing = source[closing_start..body.end].trim();
            let closing_width = closing.chars().take_while(|ch| *ch == delimiter).count();
            if closing_start == opening_end + 1 && closing_width >= width && closing.chars().all(|ch| ch == delimiter) {
                let mut text = match target {
                    Kind::Text => String::new(), Kind::Heading(level) => format!("{} ", "#".repeat(usize::from(level))),
                    Kind::Quote => "> ".into(), Kind::Callout(name) => format!("> [!{name}]\n> "),
                    Kind::Bullet => "- ".into(), Kind::Numbered => "1. ".into(), Kind::Task => "- [ ] ".into(),
                    Kind::Code => "```\n\n```".into(), Kind::Math => "$$\n\n$$".into(),
                };
                if terminated && !text.is_empty() { text.push('\n'); }
                let caret = block.range.start + text.find('\n').map_or(text.len(), |ix| ix + 1);
                return Some(Conversion { edits: vec![(block.range.clone(), text)], caret });
            }
            edits.push((body.start..opening_end + 1, String::new()));
            body.start = opening_end + 1;
            if closing_start > opening_end && closing_width >= width && closing.chars().all(|ch| ch == delimiter) {
                edits.push((closing_start..block.range.end, String::new()));
                body.end = closing_start.saturating_sub(1).max(body.start);
            }
        }
    }
    if matches!(block.kind, Kind::Heading(_)) {
        let first_end = line_end(source, body.start);
        if first_end < body.end {
            let underline = source[first_end + 1..body.end].trim();
            if !underline.is_empty() && (underline.chars().all(|ch| ch == '=') || underline.chars().all(|ch| ch == '-')) {
                let end = body.end + usize::from(source.as_bytes().get(body.end) == Some(&b'\n'));
                edits.push((first_end + 1..end, String::new()));
                body.end = first_end;
            }
        }
    }
    let quote = matches!(block.kind, Kind::Quote | Kind::Callout(_));
    let mut start = body.start;
    let mut row = 0;
    while start <= body.end {
        let end = line_end(source, start).min(body.end);
        let line = &source[start..end];
        let prefix = if quote { quote_prefix(line) }
            else if indented_code { if line.starts_with('\t') { 1 } else { line.bytes().take_while(|byte| *byte == b' ').count().min(4) } }
            else if row == 0 && !matches!(block.kind, Kind::Code | Kind::Math) { dressing(line) }
            else { 0 };
        let mut remove = prefix;
        let mut discard_header = false;
        if row == 0 && matches!(block.kind, Kind::Callout(_)) {
            if let Some((_, len)) = callout_token(&line[prefix..]) {
                remove += len;
                if matches!(line.as_bytes().get(remove), Some(b'+' | b'-')) { remove += 1; }
                remove += line[remove..].bytes().take_while(|byte| *byte == b' ').count();
                discard_header = remove == line.len() && end < body.end;
            }
        }
        if discard_header {
            edits.push((start..end + 1, String::new()));
        } else {
            let prefix = match target {
                Kind::Text | Kind::Code | Kind::Math => String::new(),
                Kind::Heading(level) if row == 0 => format!("{} ", "#".repeat(usize::from(level))),
                Kind::Heading(_) => String::new(),
                Kind::Quote | Kind::Callout(_) => "> ".into(),
                Kind::Bullet => "- ".into(),
                Kind::Numbered => format!("{}. ", row + 1),
                Kind::Task => "- [ ] ".into(),
            };
            if source[start..start + remove] != prefix { edits.push((start..start + remove, prefix)); }
        }
        if !discard_header { row += 1; }
        if end == body.end { break; }
        start = end + 1;
    }
    // Merge an opening insertion with a prefix replacement at the same gap.
    let code_fence = (target == Kind::Code).then(|| "`".repeat(source[body.clone()].split(|ch| ch != '`').map(str::len).max().unwrap_or(0).saturating_add(1).max(3)));
    let opening = match target { Kind::Callout(name) => format!("> [!{name}]\n"), Kind::Code => format!("{}\n", code_fence.as_deref().unwrap()), Kind::Math => "$$\n".into(), _ => String::new() };
    if !opening.is_empty() {
        if let Some((_, text)) = edits.iter_mut().find(|(range, _)| range.start == body.start) { text.insert_str(0, &opening); }
        else { edits.push((body.start..body.start, opening)); }
    }
    if matches!(target, Kind::Code | Kind::Math) {
        let body_ending = source.as_bytes().get(body.end) == Some(&b'\n');
        let gap = body.end + usize::from(body_ending);
        let delimiter = if target == Kind::Code { code_fence.as_deref().unwrap() } else { "$$" };
        let text = format!("{}{delimiter}{}", if body_ending { "" } else { "\n" }, if terminated { "\n" } else { "" });
        if let Some((_, closing)) = edits.iter_mut().find(|(range, _)| range.start == gap) { closing.push_str(&text); }
        else { edits.push((gap..gap, text)); }
    }
    edits.sort_by_key(|(range, _)| range.start);
    if edits.is_empty() { return None; }
    let mut mapped = at;
    for (range, text) in edits.iter().rev() {
        if mapped >= range.end { mapped = mapped - range.len() + text.len(); }
        else if mapped >= range.start { mapped = range.start + text.len(); }
    }
    Some(Conversion { edits, caret: mapped })
}
