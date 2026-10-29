use super::{Rope, Transaction};
use markdown::mdast::Node;
use std::ops::Range;

#[derive(Clone, Copy, Default, PartialEq, Eq)]
struct InlineStyle {
    bold: bool,
    italic: bool,
    strikethrough: bool,
    code: bool,
}

#[derive(Clone, PartialEq, Eq)]
struct InlineSpan {
    source: Range<usize>,
    revealed: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Kind {
    Prose,
    Heading(u8),
    List(super::lists::Structure),
    Quote(super::lists::Structure),
    Rule,
    Code(super::regions::Code),
    Metadata,
    Html,
    Source,
}
#[derive(Clone, PartialEq, Eq)]
struct Segment {
    source: Range<usize>,
    display: Range<usize>,
    content: Range<usize>,
    style: InlineStyle,
    delimiter: Option<usize>,
    raw: Option<String>,
    verbatim: bool,
}
#[derive(Clone)]
pub(super) struct Leaf {
    pub id: u64,
    pub version: u64,
    pub range: Range<usize>,
    pub text: String,
    pub kind: Kind,
    segments: Vec<Segment>,
    insertion: usize,
    content: String,
    container: Option<Range<usize>>,
    spans: Vec<InlineSpan>,
    style: InlineStyle,
}
impl Leaf {
    fn new(range: Range<usize>, kind: Kind) -> Self {
        Self {
            id: 0,
            version: 0,
            insertion: range.start,
            range,
            text: String::new(),
            kind,
            segments: Vec::new(),
            content: String::new(),
            container: None,
            spans: Vec::new(),
            style: InlineStyle::default(),
        }
    }
    fn push(&mut self, source: Range<usize>, text: &str) {
        self.push_segment(source, text, None, None);
    }
    fn push_normalized(&mut self, source: Range<usize>, text: &str, raw: &str) {
        self.push_segment(source, text, None, (text != raw).then(|| raw.to_owned()));
    }
    fn push_segment(
        &mut self,
        source: Range<usize>,
        text: &str,
        delimiter: Option<usize>,
        raw: Option<String>,
    ) {
        let start = self.content.len();
        self.content.push_str(text);
        if source.len() == text.len() && raw.is_none() {
            if let Some(last) = self.segments.last_mut().filter(|last| {
                last.source.end == source.start
                    && last.source.len() == last.content.len()
                    && last.style == self.style
                    && last.delimiter == delimiter
                    && last.raw.is_none()
            }) {
                last.source.end = source.end;
                last.content.end = self.content.len();
                return;
            }
        }
        self.segments.push(Segment {
            source,
            display: 0..0,
            content: start..self.content.len(),
            style: self.style,
            delimiter,
            raw,
            verbatim: false,
        });
    }
    fn rebuild(&mut self, marked: Option<&Range<usize>>) {
        self.text.clear();
        for segment in &mut self.segments {
            let start = self.text.len();
            segment.verbatim = segment.raw.is_some()
                && marked.is_some_and(|range| {
                    range.start < segment.source.end && range.end > segment.source.start
                });
            if segment.delimiter.is_none_or(|ix| self.spans[ix].revealed) {
                self.text.push_str(if segment.verbatim {
                    segment.raw.as_deref().unwrap()
                } else {
                    &self.content[segment.content.clone()]
                });
            }
            segment.display = start..self.text.len();
        }
    }
    pub fn contextual_reveal(
        &mut self,
        selection: super::Selection,
        extra: &[super::Selection],
        marked: Option<&Range<usize>>,
    ) -> bool {
        let mut changed = false;
        for span in &mut self.spans {
            let revealed = std::iter::once(&selection).chain(extra).any(|selection| {
                let range = selection.range();
                if range.is_empty() {
                    span.source.start < range.start && range.start < span.source.end
                } else {
                    range.start < span.source.end && range.end > span.source.start
                }
            }) || marked.is_some_and(|range| {
                range.start <= span.source.end && range.end >= span.source.start
            });
            changed |= revealed != span.revealed;
            span.revealed = revealed;
        }
        changed |= self.segments.iter().any(|segment| {
            segment.verbatim
                != (segment.raw.is_some()
                    && marked.is_some_and(|range| {
                        range.start < segment.source.end && range.end > segment.source.start
                    }))
        });
        if changed {
            self.rebuild(marked);
        }
        changed
    }
    pub fn inline_position(&self, at: usize) -> bool {
        self.spans
            .iter()
            .any(|span| span.source.start <= at && at <= span.source.end)
    }
    pub fn marks(
        &self,
        marked: Option<&Range<usize>>,
    ) -> Vec<(Range<usize>, gpui_kit::base::text::editor_host::TextMark)> {
        use gpui_kit::base::text::editor_host::TextMark;
        let marked =
            marked.map(|range| self.display_offset(range.start)..self.display_offset(range.end));
        let mut marks = Vec::new();
        for segment in &self.segments {
            if segment.display.is_empty() {
                continue;
            }
            let style = segment.style;
            let mark = TextMark {
                bold: style.bold,
                italic: style.italic,
                strikethrough: style.strikethrough,
                code: style.code,
                ..Default::default()
            };
            let range = segment.display.clone();
            if let Some(intersection) = marked
                .as_ref()
                .map(|marked| range.start.max(marked.start)..range.end.min(marked.end))
                .filter(|range| !range.is_empty())
            {
                if range.start < intersection.start {
                    marks.push((range.start..intersection.start, mark.clone()));
                }
                marks.push((
                    intersection.clone(),
                    TextMark {
                        underline: true,
                        ..mark.clone()
                    },
                ));
                if intersection.end < range.end {
                    marks.push((intersection.end..range.end, mark));
                }
            } else {
                marks.push((range, mark));
            }
        }
        marks
    }
    fn same_inline(&self, other: &Self) -> bool {
        self.content == other.content
            && self.segments == other.segments
            && self.spans == other.spans
    }
    fn prefix(&mut self, source: &str, range: Range<usize>) {
        if range.is_empty() { return; }
        let ix = self.spans.len();
        self.spans.push(InlineSpan { source: range.clone(), revealed: false });
        self.push_segment(range.clone(), &source[range], Some(ix), None);
    }
    fn raw(&mut self, source: &str, range: Range<usize>) {
        self.push(range.clone(), &source[range]);
    }
    pub fn display_offset(&self, source: usize) -> usize {
        for span in &self.segments {
            if source <= span.source.start {
                return span.display.start;
            }
            if source < span.source.end {
                return span.display.start
                    + if span.source.len() == span.display.len() {
                        source - span.source.start
                    } else {
                        0
                    };
            }
        }
        self.text.len()
    }
    pub fn source_offset(&self, display: usize) -> usize {
        // A shared visual edge belongs inside the preceding closing delimiter;
        // opening delimiters instead select the first visible content byte.
        if let Some(closing) = self.segments.iter().rev().find(|segment| {
            segment.display.is_empty()
                && segment.display.start == display
                && segment
                    .delimiter
                    .is_some_and(|ix| segment.source.start > self.spans[ix].source.start)
        }) {
            return closing.source.start;
        }
        self.segments
            .iter()
            .filter(|span| !span.display.is_empty())
            .find(|span| display < span.display.end)
            .map_or_else(
                || {
                    self.segments
                        .iter()
                        .rev()
                        .find(|span| !span.display.is_empty())
                        .map_or(self.insertion, |s| s.source.end)
                },
                |s| {
                    s.source.start
                        + if s.source.len() == s.display.len() {
                            display.saturating_sub(s.display.start)
                        } else {
                            0
                        }
                },
            )
    }
    pub fn content_start(&self) -> usize {
        self.source_offset(0)
    }
    pub fn content_end(&self) -> usize {
        self.source_offset(self.text.len())
    }
    fn shift(&mut self, old_start: usize, new_start: usize) {
        let shift = |at: usize| {
            if new_start >= old_start {
                at + (new_start - old_start)
            } else {
                at - (old_start - new_start)
            }
        };
        self.range = shift(self.range.start)..shift(self.range.end);
        self.insertion = shift(self.insertion);
        if let Kind::List(style) | Kind::Quote(style) = &mut self.kind {
            style.start = shift(style.start);
        }
        if let Some(container) = &mut self.container {
            *container = shift(container.start)..shift(container.end);
        }
        for segment in &mut self.segments {
            segment.source = shift(segment.source.start)..shift(segment.source.end);
        }
        for span in &mut self.spans {
            span.source = shift(span.source.start)..shift(span.source.end);
        }
    }
}

/// Derived source/display map. All offsets refer to the document Rope; no text
/// in this projection is ever written back to the document.
pub(super) struct Projection {
    pub revision: u64,
    pub leaves: Vec<Leaf>,
    next_id: u64,
}
impl Projection {
    pub fn source(source: &Rope, revision: u64) -> Self {
        let text = source.to_string();
        Self::parse(&text, revision, false)
    }
    pub fn parse(source: &str, revision: u64, live: bool) -> Self {
        Self::parse_fragment(source, revision, live, true)
    }
    fn parse_fragment(source: &str, revision: u64, live: bool, document_start: bool) -> Self {
        let mut leaves = Vec::new();
        if live {
            let mut options = markdown::ParseOptions::gfm();
            options.constructs.frontmatter = document_start;
            if super::regions::unfinished_metadata(source, document_start) {
                let mut leaf = Leaf::new(0..source.len(), Kind::Metadata);
                leaf.raw(source, 0..source.len());
                leaves.push(leaf);
            } else if let Ok(root) = markdown::to_mdast(source, &options) {
                let mut cursor = 0;
                for node in root.children().into_iter().flatten() {
                    let Some(position) = node.position() else {
                        continue;
                    };
                    let range = position.start.offset..position.end.offset;
                    if matches!(node, Node::List(_) | Node::Blockquote(_)) {
                        let start = source[..range.start].rfind('\n').map_or(0, |ix| ix + 1);
                        Self::gap(source, cursor..start, &mut leaves);
                        Self::container(source, node, 0, 0, range.start, start..range.end, &mut leaves);
                        cursor = range.end;
                        continue;
                    }
                    Self::gap(source, cursor..range.start, &mut leaves);
                    let kind = super::regions::classify(node, &source[range.clone()]);
                    let mut leaf = Leaf::new(range.clone(), kind);
                    if matches!(kind, Kind::Prose | Kind::Heading(_)) {
                        if let Some(children) = node.children() {
                            for child in children {
                                Self::inline(source, child, &mut leaf);
                            }
                        }
                        if leaf.segments.is_empty() {
                            leaf.insertion = range.end;
                        }
                    } else if kind != Kind::Rule {
                        leaf.raw(source, range.clone());
                    }
                    leaves.push(leaf);
                    cursor = range.end;
                }
                Self::gap(source, cursor..source.len(), &mut leaves);
            } else {
                Self::source_lines(source, 0, &mut leaves);
            }
        } else {
            Self::source_lines(source, 0, &mut leaves);
        }
        if leaves.is_empty() {
            leaves.push(Leaf::new(0..0, Kind::Prose));
        }
        let next_id = leaves.len() as u64 + 1;
        for (ix, leaf) in leaves.iter_mut().enumerate() {
            leaf.rebuild(None);
            leaf.id = ix as u64 + 1;
            leaf.version = leaf.id;
        }
        Self {
            revision,
            leaves,
            next_id,
        }
    }
    fn container(source: &str, node: &Node, depth: usize, quotes: usize, owner: usize, container: Range<usize>, leaves: &mut Vec<Leaf>) {
        let Some(position) = node.position() else { return; };
        let range = position.start.offset..position.end.offset;
        match node {
            Node::List(_) | Node::Blockquote(_) | Node::ListItem(_) => {
                let depth = depth + usize::from(matches!(node, Node::List(_)));
                let quotes = quotes + usize::from(matches!(node, Node::Blockquote(_)));
                let owner = if matches!(node, Node::ListItem(_)) || (depth == 0 && matches!(node, Node::Blockquote(_))) { range.start } else { owner };
                if let Some(children) = node.children().filter(|children| !children.is_empty()) {
                    for child in children {
                        Self::container(source, child, depth, quotes, owner, container.clone(), leaves);
                    }
                } else {
                    let start = source[..range.start].rfind('\n').map_or(0, |ix| ix + 1);
                    let style = super::lists::style(source, range.start, depth, quotes, owner);
                    let mut leaf = Leaf::new(start..range.end, if depth > 0 { Kind::List(style) } else { Kind::Quote(style) });
                    leaf.container = Some(container);
                    leaf.prefix(source, start..range.end);
                    leaf.insertion = range.end;
                    leaves.push(leaf);
                }
            }
            _ => {
                let start = source[..range.start].rfind('\n').map_or(0, |ix| ix + 1);
                let style = super::lists::style(source, range.start, depth, quotes, owner);
                let prose = matches!(node, Node::Paragraph(_) | Node::Heading(_));
                let kind = if prose {
                    if depth > 0 { Kind::List(style) } else { Kind::Quote(style) }
                } else { super::regions::classify(node, &source[range.clone()]) };
                if let Some(previous) = leaves.last().filter(|previous| previous.range.end + 1 < start) {
                    Self::gap(source, previous.range.end..start, leaves);
                }
                let mut leaf = Leaf::new(start..range.end, kind);
                leaf.container = Some(container);
                if prose {
                    leaf.prefix(source, start..range.start);
                    if let Some(children) = node.children() {
                        for child in children { Self::inline(source, child, &mut leaf); }
                    }
                    if leaf.segments.is_empty() { leaf.insertion = range.end; }
                } else { leaf.raw(source, start..range.end); }
                leaves.push(leaf);
            }
        }
    }
    fn source_lines(source: &str, base: usize, leaves: &mut Vec<Leaf>) {
        let mut offset = base;
        for line in source.split('\n') {
            let mut leaf = Leaf::new(offset..offset + line.len(), Kind::Source);
            leaf.push(offset..offset + line.len(), line);
            leaves.push(leaf);
            offset += line.len() + 1;
        }
    }
    fn gap(source: &str, range: Range<usize>, leaves: &mut Vec<Leaf>) {
        // A block's terminating LF is structural. Additional blank lines are
        // real editable insertion points, including the final empty line.
        let mut start = range.start;
        if !leaves.is_empty() && source.as_bytes().get(start) == Some(&b'\n') {
            start += 1;
        }
        if start < range.end {
            Self::source_lines(&source[start..range.end], start, leaves);
            if leaves
                .last()
                .is_some_and(|leaf| leaf.range.start == range.end && range.end < source.len())
            {
                leaves.pop();
            }
        } else if range.end == source.len() && source.ends_with('\n') {
            leaves.push(Leaf::new(range.end..range.end, Kind::Prose));
        }
    }
    fn inline(source: &str, node: &Node, leaf: &mut Leaf) {
        let Some(position) = node.position() else {
            return;
        };
        let range = position.start.offset..position.end.offset;
        match node {
            Node::Break(_) => leaf.push_normalized(range.clone(), "\n", &source[range]),
            Node::Text(_) => {
                let mut range = range;
                // The parser may consume a leading escape before locating Text.
                // Its prefix still belongs to this visible punctuation, unless a
                // preceding projected segment already owns that source byte.
                if range.start > leaf.range.start
                    && source.as_bytes()[range.start - 1] == b'\\'
                    && source
                        .as_bytes()
                        .get(range.start)
                        .is_some_and(u8::is_ascii_punctuation)
                    && leaf
                        .segments
                        .last()
                        .is_none_or(|segment| segment.source.end < range.start)
                {
                    range.start -= 1;
                }
                let raw = &source[range.clone()];
                let mut at = 0;
                while at < raw.len() {
                    let rest = &raw[at..];
                    let ch = rest.chars().next().unwrap();
                    if ch == '\\' && rest.as_bytes().get(1).is_some_and(u8::is_ascii_punctuation) {
                        leaf.push_normalized(
                            range.start + at..range.start + at + 2,
                            &rest[1..2],
                            &rest[..2],
                        );
                        at += 2;
                    } else if ch == '&' {
                        let decoded = rest.find(';').filter(|end| *end < 64).and_then(|end| {
                            let token = &rest[..=end];
                            let root =
                                markdown::to_mdast(token, &markdown::ParseOptions::default())
                                    .ok()?;
                            let value = root.to_string();
                            (value != token).then_some((end + 1, value))
                        });
                        if let Some((len, text)) = decoded {
                            leaf.push_normalized(
                                range.start + at..range.start + at + len,
                                &text,
                                &rest[..len],
                            );
                            at += len;
                        } else {
                            leaf.push(range.start + at..range.start + at + 1, "&");
                            at += 1;
                        }
                    } else {
                        leaf.push_normalized(
                            range.start + at..range.start + at + ch.len_utf8(),
                            if ch == '\n' {
                                " "
                            } else {
                                &rest[..ch.len_utf8()]
                            },
                            &rest[..ch.len_utf8()],
                        );
                        at += ch.len_utf8();
                        if ch == '\n' && matches!(leaf.kind, Kind::List(_) | Kind::Quote(_)) {
                            let width = super::lists::prefix_len(&raw[at..]);
                            leaf.prefix(source, range.start + at..range.start + at + width);
                            at += width;
                        }
                    }
                }
            }
            Node::Emphasis(_) | Node::Strong(_) | Node::Delete(_) => {
                let before = leaf.style;
                match node {
                    Node::Emphasis(_) => leaf.style.italic = true,
                    Node::Strong(_) => leaf.style.bold = true,
                    Node::Delete(_) => leaf.style.strikethrough = true,
                    _ => unreachable!(),
                }
                let children = node.children().unwrap();
                let width = match node {
                    Node::Strong(_) => 2,
                    Node::Delete(_) => source[range.clone()]
                        .bytes()
                        .take_while(|byte| *byte == b'~')
                        .count()
                        .min(2),
                    _ => 1,
                };
                let start = range.start + width;
                let end = range.end - width;
                let ix = leaf.spans.len();
                leaf.spans.push(InlineSpan {
                    source: range.clone(),
                    revealed: false,
                });
                leaf.push_segment(
                    range.start..start,
                    &source[range.start..start],
                    Some(ix),
                    None,
                );
                for child in children {
                    Self::inline(source, child, leaf);
                }
                leaf.push_segment(end..range.end, &source[end..range.end], Some(ix), None);
                leaf.style = before;
            }
            Node::InlineCode(_) => {
                let raw = &source[range.clone()];
                let ticks = raw.bytes().take_while(|byte| *byte == b'`').count();
                let inner = range.start + ticks..range.end - ticks;
                let bytes = source[inner.clone()].as_bytes();
                let trim = bytes
                    .first()
                    .is_some_and(|byte| matches!(byte, b' ' | b'\n'))
                    && bytes
                        .last()
                        .is_some_and(|byte| matches!(byte, b' ' | b'\n'))
                    && bytes.iter().any(|byte| !matches!(byte, b' ' | b'\n'));
                let start = inner.start + usize::from(trim);
                let end = inner.end - usize::from(trim);
                let ix = leaf.spans.len();
                leaf.spans.push(InlineSpan {
                    source: range.clone(),
                    revealed: false,
                });
                let before = leaf.style;
                leaf.style.code = true;
                leaf.push_segment(
                    range.start..start,
                    &source[range.start..start],
                    Some(ix),
                    None,
                );
                for (offset, ch) in source[start..end].char_indices() {
                    let at = start + offset;
                    leaf.push_normalized(
                        at..at + ch.len_utf8(),
                        if ch == '\n' {
                            " "
                        } else {
                            &source[at..at + ch.len_utf8()]
                        },
                        &source[at..at + ch.len_utf8()],
                    );
                }
                leaf.push_segment(end..range.end, &source[end..range.end], Some(ix), None);
                leaf.style = before;
            }
            // Other inline objects retain their source until their editing slice.
            _ => leaf.raw(source, range),
        }
    }
    pub fn leaf_at(&self, at: usize) -> Option<&Leaf> {
        self.leaves
            .iter()
            .find(|leaf| leaf.range.contains(&at) || leaf.range.start == at)
            .or_else(|| self.leaves.iter().rev().find(|leaf| leaf.range.end <= at))
    }
    pub fn install(&mut self, mut newer: Self) {
        let old_by_start: std::collections::HashMap<_, _> = self
            .leaves
            .iter()
            .map(|leaf| (leaf.range.start, leaf))
            .collect();
        for leaf in &mut newer.leaves {
            let old = old_by_start.get(&leaf.range.start).copied();
            leaf.id = old.map_or_else(
                || {
                    let id = self.next_id;
                    self.next_id += 1;
                    id
                },
                |old| old.id,
            );
            leaf.version = old
                .filter(|old| {
                    old.text == leaf.text && old.kind == leaf.kind && old.same_inline(leaf)
                })
                .map_or_else(
                    || {
                        let version = self.next_id;
                        self.next_id += 1;
                        version
                    },
                    |old| old.version,
                );
        }
        newer.next_id = self.next_id;
        *self = newer;
    }
    pub fn rebase(&mut self, source: &Rope, transaction: &Transaction, live: bool) {
        if transaction.edits.is_empty() {
            return;
        }
        // Only affected leaves and their immediate neighbours are reparsed on
        // input. The debounced background parse resolves larger syntax changes.
        let mut old = transaction
            .edits
            .iter()
            .map(|edit| edit.range.clone())
            .reduce(|a, b| a.start.min(b.start)..a.end.max(b.end))
            .unwrap();
        // Keep affected child leaves inside their original AST container while
        // reparsing, so a nested item cannot temporarily become indented code.
        for leaf in &self.leaves {
            if leaf.range.start <= old.end && leaf.range.end >= old.start {
                if let Some(container) = &leaf.container {
                    old.start = old.start.min(container.start);
                    old.end = old.end.max(container.end);
                }
            }
        }
        let first = self
            .leaves
            .iter()
            .position(|leaf| leaf.range.end >= old.start)
            .unwrap_or(0)
            .saturating_sub(1);
        let last = (self
            .leaves
            .iter()
            .rposition(|leaf| leaf.range.start <= old.end)
            .unwrap_or(first)
            + 2)
        .min(self.leaves.len());
        let map = |at| {
            transaction
                .map_selection(super::Selection::caret(at))
                .head()
        };
        let start = map(self.leaves[first].range.start);
        let mut end = self.leaves[last - 1].range.end;
        for edit in &transaction.edits {
            end = if end < edit.range.start {
                end
            } else if end >= edit.range.end {
                end - edit.range.len() + edit.inserted_len
            } else {
                edit.range.start + edit.inserted_len
            };
        }
        let end = end.max(start).min(source.len());
        let identities: Vec<_> = self.leaves[first..last]
            .iter()
            .map(|leaf| {
                (
                    map(leaf.range.start + usize::from(!leaf.range.is_empty())),
                    leaf.id,
                )
            })
            .collect();
        for leaf in self.leaves.iter_mut().skip(last) {
            leaf.shift(leaf.range.start, map(leaf.range.start));
        }
        let fragment = source.slice(start..end).to_string();
        let mut patch = Self::parse_fragment(&fragment, transaction.revision(), live, start == 0);
        for leaf in &mut patch.leaves {
            leaf.shift(0, start);
            leaf.id = identities
                .iter()
                .find(|(at, _)| leaf.range.contains(at) || leaf.range.end == *at)
                .map_or_else(
                    || {
                        let id = self.next_id;
                        self.next_id += 1;
                        id
                    },
                    |(_, id)| *id,
                );
            leaf.version = self.leaves[first..last]
                .iter()
                .find(|old| {
                    old.id == leaf.id
                        && old.text == leaf.text
                        && old.kind == leaf.kind
                        && old.same_inline(leaf)
                })
                .map_or_else(
                    || {
                        let version = self.next_id;
                        self.next_id += 1;
                        version
                    },
                    |old| old.version,
                );
        }
        self.leaves.splice(first..last, patch.leaves);
        self.revision = transaction.revision();
    }
}
