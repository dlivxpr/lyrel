use super::{Rope, Transaction};
use markdown::mdast::Node;
use std::ops::Range;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Kind {
    Prose,
    Heading(u8),
    Rule,
    Source,
}
#[derive(Clone)]
struct Segment {
    source: Range<usize>,
    display: Range<usize>,
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
        }
    }
    fn push(&mut self, source: Range<usize>, text: &str) {
        let start = self.text.len();
        self.text.push_str(text);
        if source.len() == text.len() {
            if let Some(last) = self.segments.last_mut().filter(|last| {
                last.source.end == source.start && last.source.len() == last.display.len()
            }) {
                last.source.end = source.end;
                last.display.end = self.text.len();
                return;
            }
        }
        self.segments.push(Segment {
            source,
            display: start..self.text.len(),
        });
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
        self.segments
            .iter()
            .find(|span| display < span.display.end)
            .map_or_else(
                || {
                    self.segments
                        .last()
                        .map_or(self.insertion, |s| s.source.end)
                },
                |s| {
                    s.source.start
                        + if s.source.len() == s.display.len() {
                            display - s.display.start
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
        for segment in &mut self.segments {
            segment.source = shift(segment.source.start)..shift(segment.source.end);
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
            if let Ok(root) = markdown::to_mdast(source, &options) {
                let mut cursor = 0;
                for node in root.children().into_iter().flatten() {
                    let Some(position) = node.position() else {
                        continue;
                    };
                    let range = position.start.offset..position.end.offset;
                    Self::gap(source, cursor..range.start, &mut leaves);
                    let kind = match node {
                        Node::Paragraph(_) => Kind::Prose,
                        Node::Heading(heading) => Kind::Heading(heading.depth),
                        Node::ThematicBreak(_) => Kind::Rule,
                        _ => Kind::Source,
                    };
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
            leaf.id = ix as u64 + 1;
            leaf.version = leaf.id;
        }
        Self {
            revision,
            leaves,
            next_id,
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
            Node::Break(_) => leaf.push(range, "\n"),
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
                        leaf.push(range.start + at..range.start + at + 2, &rest[1..2]);
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
                            leaf.push(range.start + at..range.start + at + len, &text);
                            at += len;
                        } else {
                            leaf.push(range.start + at..range.start + at + 1, "&");
                            at += 1;
                        }
                    } else {
                        leaf.push(
                            range.start + at..range.start + at + ch.len_utf8(),
                            if ch == '\n' {
                                " "
                            } else {
                                &rest[..ch.len_utf8()]
                            },
                        );
                        at += ch.len_utf8();
                    }
                }
            }
            // Inline formatting, objects and structured blocks are deliberately
            // source-visible until their editing slices land. No preview claims.
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
                .filter(|old| old.text == leaf.text && old.kind == leaf.kind)
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
        let old = transaction
            .edits
            .iter()
            .map(|edit| edit.range.clone())
            .reduce(|a, b| a.start.min(b.start)..a.end.max(b.end))
            .unwrap();
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
                .find(|old| old.id == leaf.id && old.text == leaf.text && old.kind == leaf.kind)
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
