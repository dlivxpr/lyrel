use super::{Rope, Transaction};
use markdown::mdast::Node;
use std::ops::Range;

#[derive(Clone, Copy, Default, PartialEq, Eq)]
struct InlineStyle {
    bold: bool,
    italic: bool,
    strikethrough: bool,
    code: bool,
    link: Option<usize>,
}

#[derive(Clone, PartialEq, Eq)]
struct InlineSpan {
    source: Range<usize>,
    revealed: bool,
    target: Option<gpui_kit::SharedString>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Kind {
    Prose,
    Heading(u8),
    List(super::lists::Structure),
    Quote(super::lists::Structure),
    Rule,
    Code(super::regions::Code),
    Mermaid(super::regions::Code),
    Math,
    Metadata,
    Html,
    Source,
    TableCell(super::tables::Cell),
    Mdx(super::mdx::Field),
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
    image: Option<super::images::Image>,
    math: Option<super::math::Formula>,
}
#[derive(Clone)]
pub(super) struct Leaf {
    pub id: u64,
    pub version: u64,
    pub range: Range<usize>,
    pub text: String,
    pub kind: Kind,
    pub deferred: bool,
    segments: Vec<Segment>,
    insertion: usize,
    content: String,
    container: Option<Range<usize>>,
    spans: Vec<InlineSpan>,
    style: InlineStyle,
    pub(super) image_definitions: std::sync::Arc<super::images::Definitions>,
}
impl Leaf {
    pub(super) fn new(range: Range<usize>, kind: Kind) -> Self {
        Self {
            id: 0,
            version: 0,
            insertion: range.start,
            range,
            text: String::new(),
            kind,
            deferred: false,
            segments: Vec::new(),
            content: String::new(),
            container: None,
            spans: Vec::new(),
            style: InlineStyle::default(),
            image_definitions: Default::default(),
        }
    }
    pub fn materialize(&mut self, source: &Rope) {
        if !self.deferred {
            return;
        }
        let text = source.slice(self.range.clone()).to_string();
        self.push(self.range.clone(), &text);
        self.rebuild(None);
        self.deferred = false;
    }
    pub(super) fn empty_insertion(&mut self, at: usize) {
        if self.segments.is_empty() {
            self.insertion = at;
        }
    }
    pub(super) fn container_range(&mut self, range: Range<usize>) {
        self.container = Some(range);
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
            image: None,
            math: None,
        });
    }
    fn rebuild(&mut self, marked: Option<&Range<usize>>) {
        self.text.clear();
        for segment in &mut self.segments {
            let start = self.text.len();
            if segment.image.is_none() && segment.math.is_none() {
                segment.verbatim = segment.raw.is_some()
                    && marked.is_some_and(|range| {
                        range.start < segment.source.end && range.end > segment.source.start
                    });
            }
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
        for segment in &mut self.segments {
            if segment.image.is_some() || segment.math.is_some() {
                let revealed = std::iter::once(&selection).chain(extra).any(|selection| {
                    let range = selection.range();
                    // Crossing the whole image selects its original Markdown as
                    // one atom. Reveal only when an editing endpoint is inside
                    // its syntax; otherwise reverse Shift navigation must cross
                    // the same boundary instead of entering the image URL.
                    segment.source.start < range.start && range.start < segment.source.end
                        || segment.source.start < range.end && range.end < segment.source.end
                }) || marked.is_some_and(|range| {
                    range.start < segment.source.end && range.end > segment.source.start
                });
                changed |= segment.verbatim != revealed;
                segment.verbatim = revealed;
            }
        }
        changed |= self.segments.iter().any(|segment| {
            segment.image.is_none()
                && segment.math.is_none()
                && segment.verbatim
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
            || self.segments.iter().any(|segment| {
                segment.math.is_some() && segment.source.start <= at && at <= segment.source.end
            })
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
                link: style
                    .link
                    .map(|ix| gpui_kit::base::text::editor_host::LinkMark {
                        url: self.spans[ix].target.clone().unwrap_or_default(),
                        ..Default::default()
                    }),
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
    pub fn image_urls(&self) -> impl Iterator<Item = &str> {
        self.segments
            .iter()
            .filter_map(|segment| segment.image.as_ref().map(|image| image.url.as_str()))
    }
    pub fn inline_nodes(
        &self,
        marked: Option<&Range<usize>>,
        math: &super::math::State,
    ) -> Vec<gpui_kit::base::text::editor_host::InlineNode> {
        use gpui_kit::base::text::editor_host::{ImageNode, InlineNode};
        let marks = self.marks(marked);
        self.segments
            .iter()
            .filter(|segment| !segment.display.is_empty())
            .map(|segment| {
                if let Some(image) = segment.image.as_ref().filter(|_| !segment.verbatim) {
                    let mut node = ImageNode::default();
                    node.url = image.url.clone().into();
                    node.alt = Some(image.alt.clone().into());
                    node.title = image.title.clone().map(Into::into);
                    // Reserve geometry before loading, so completion cannot move the
                    // caret or scroll anchor. Images fit inside this native atom.
                    node.width = Some(gpui_kit::rems(12.).into());
                    node.height = Some(gpui_kit::rems(8.).into());
                    InlineNode::image(node)
                } else if let Some(formula) = segment.math.as_ref().filter(|_| !segment.verbatim) {
                    math.inline_node(formula)
                } else {
                    let range = &segment.display;
                    InlineNode::new(self.text[range.clone()].to_owned()).marks(
                        marks
                            .iter()
                            .filter_map(|(mark_range, mark)| {
                                let start = range.start.max(mark_range.start);
                                let end = range.end.min(mark_range.end);
                                (start < end)
                                    .then(|| (start - range.start..end - range.start, mark.clone()))
                            })
                            .collect(),
                    )
                }
            })
            .collect()
    }
    pub fn formulas(&self) -> impl Iterator<Item = &super::math::Formula> {
        self.segments
            .iter()
            .filter_map(|segment| segment.math.as_ref())
    }
    pub fn math_source(&self, display: usize) -> Option<usize> {
        self.segments
            .iter()
            .find(|segment| {
                segment.math.is_some() && !segment.verbatim && segment.display.contains(&display)
            })
            .map(|segment| segment.source.start + 1)
    }
    pub fn editing_math(&self, at: usize) -> bool {
        self.segments.iter().any(|segment| {
            segment.math.is_some() && segment.source.start < at && at < segment.source.end
        })
    }
    fn same_inline(&self, other: &Self) -> bool {
        self.content == other.content
            && self.segments == other.segments
            && self.spans == other.spans
    }
    fn prefix(&mut self, source: &str, range: Range<usize>) {
        if range.is_empty() {
            return;
        }
        let ix = self.spans.len();
        self.spans.push(InlineSpan {
            source: range.clone(),
            revealed: false,
            target: None,
        });
        self.push_segment(range.clone(), &source[range], Some(ix), None);
    }
    pub(super) fn raw(&mut self, source: &str, range: Range<usize>) {
        self.push(range.clone(), &source[range]);
    }
    pub fn atomic_boundary(&self, at: usize, forward: bool) -> usize {
        self.segments
            .iter()
            .find(|segment| {
                segment.source.start < at
                    && at < segment.source.end
                    && !segment.display.is_empty()
                    && (segment.source.len() != segment.display.len()
                        || segment.math.is_some() && !segment.verbatim)
            })
            .map_or(at, |segment| {
                if forward {
                    segment.source.end
                } else {
                    segment.source.start
                }
            })
    }
    pub fn display_offset(&self, source: usize) -> usize {
        if self.deferred {
            return source
                .saturating_sub(self.range.start)
                .min(self.range.len());
        }
        for span in &self.segments {
            if source <= span.source.start {
                return span.display.start;
            }
            if source < span.source.end {
                return span.display.start
                    + if span.source.len() == span.display.len()
                        && !(span.math.is_some() && !span.verbatim)
                    {
                        source - span.source.start
                    } else {
                        0
                    };
            }
        }
        self.text.len()
    }
    pub fn source_offset(&self, display: usize) -> usize {
        if self.deferred {
            return (self.range.start + display).min(self.range.end);
        }
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
                        + if s.source.len() == s.display.len() && !(s.math.is_some() && !s.verbatim)
                        {
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
        if self.deferred {
            return self.range.end;
        }
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
        if let Kind::TableCell(cell) = &mut self.kind {
            cell.shift(shift);
        }
    }
}

/// Derived source/display map. All offsets refer to the document Rope; no text
/// in this projection is ever written back to the document.
pub(super) struct Projection {
    pub revision: u64,
    pub leaves: Vec<Leaf>,
    pub links: super::links::Index,
    pub grammar: super::mdx::Grammar,
    pub mdx: super::mdx::Index,
    pub error: Option<String>,
    live: bool,
    next_id: u64,
}
impl Projection {
    pub fn source(source: &Rope, revision: u64, grammar: super::mdx::Grammar) -> Self {
        let text = source.to_string();
        Self::parse(&text, revision, false, grammar)
    }
    pub fn parse(source: &str, revision: u64, live: bool, grammar: super::mdx::Grammar) -> Self {
        Self::parse_fragment(source, revision, live, true, grammar)
    }
    fn parse_fragment(
        source: &str,
        revision: u64,
        live: bool,
        document_start: bool,
        grammar: super::mdx::Grammar,
    ) -> Self {
        let mut leaves = Vec::new();
        let mut links = super::links::Index::default();
        let mut mdx = super::mdx::Index::default();
        let mut error = None;
        if live {
            if super::regions::unfinished_metadata(source, document_start) {
                let mut leaf = Leaf::new(0..source.len(), Kind::Metadata);
                leaf.raw(source, 0..source.len());
                leaves.push(leaf);
            } else {
                match super::parser::parse(source, grammar, document_start) {
                    Ok(root) => {
                        let image_definitions =
                            std::sync::Arc::new(super::images::definitions(&root));
                        links = super::links::Index::from_ast(source, &root);
                        mdx = super::mdx::Index::from_ast(source, &root);
                        Self::project_nodes(
                            source,
                            root.children().map_or(&[], Vec::as_slice),
                            0..source.len(),
                            &image_definitions,
                            &mut leaves,
                        );
                    }
                    Err(reason) => {
                        error = Some(reason.to_string());
                        Self::source_lines(source, 0, &mut leaves);
                    }
                }
            }
        } else {
            Self::source_lines(source, 0, &mut leaves);
        }
        if leaves.is_empty() {
            leaves.push(Leaf::new(0..0, Kind::Prose));
        }
        let next_id = leaves.len() as u64 + 1;
        let targets: std::collections::HashMap<_, _> = links
            .links
            .iter()
            .map(|link| (link.source.start, link))
            .collect();
        for (ix, leaf) in leaves.iter_mut().enumerate() {
            for span in &mut leaf.spans {
                if let Some(link) = targets
                    .get(&span.source.start)
                    .filter(|link| link.source == span.source)
                {
                    span.target = Some(link.target.clone().into());
                }
            }
            leaf.rebuild(None);
            leaf.id = ix as u64 + 1;
            leaf.version = leaf.id;
        }
        Self {
            revision,
            leaves,
            next_id,
            links,
            grammar,
            mdx,
            error,
            live,
        }
    }
    pub(super) fn project_nodes(
        source: &str,
        nodes: &[Node],
        bounds: Range<usize>,
        images: &std::sync::Arc<super::images::Definitions>,
        leaves: &mut Vec<Leaf>,
    ) {
        let mut cursor = bounds.start;
        for node in nodes {
            let Some(position) = node.position() else {
                continue;
            };
            let range = position.start.offset..position.end.offset;
            let start = if matches!(node, Node::List(_) | Node::Blockquote(_)) {
                source[..range.start].rfind('\n').map_or(0, |ix| ix + 1)
            } else {
                range.start
            };
            Self::gap(source, cursor..start, leaves);
            if super::mdx::project(source, node, images, leaves) {
                cursor = range.end;
                continue;
            }
            if matches!(node, Node::List(_) | Node::Blockquote(_)) {
                // Include the original indentation and marker, not a duplicate
                // source-gap leaf for the same prefix.
                Self::container(
                    source,
                    node,
                    0,
                    0,
                    range.start,
                    start..range.end,
                    images,
                    leaves,
                );
            } else if !super::tables::project(source, node, images, leaves) {
                let kind = match node {
                    Node::MdxjsEsm(_) | Node::MdxFlowExpression(_) | Node::MdxTextExpression(_) => {
                        Kind::Mdx(super::mdx::Field::Dynamic)
                    }
                    _ => super::regions::classify(node, &source[range.clone()]),
                };
                let mut leaf = Leaf::new(range.clone(), kind);
                leaf.image_definitions = images.clone();
                if matches!(kind, Kind::Prose | Kind::Heading(_)) {
                    if let Some(children) = node.children() {
                        for child in children {
                            Self::inline(source, child, &mut leaf);
                        }
                    }
                    if leaf.segments.is_empty() {
                        leaf.insertion = range.end;
                    }
                } else if kind == Kind::Math {
                    Self::inline(source, node, &mut leaf);
                } else if kind != Kind::Rule {
                    leaf.raw(source, range.clone());
                }
                leaves.push(leaf);
            }
            cursor = range.end;
        }
        Self::gap(source, cursor..bounds.end, leaves);
    }
    fn container(
        source: &str,
        node: &Node,
        depth: usize,
        quotes: usize,
        owner: usize,
        container: Range<usize>,
        images: &std::sync::Arc<super::images::Definitions>,
        leaves: &mut Vec<Leaf>,
    ) {
        let Some(position) = node.position() else {
            return;
        };
        let range = position.start.offset..position.end.offset;
        match node {
            Node::List(_) | Node::Blockquote(_) | Node::ListItem(_) => {
                let depth = depth + usize::from(matches!(node, Node::List(_)));
                let quotes = quotes + usize::from(matches!(node, Node::Blockquote(_)));
                let owner = if matches!(node, Node::ListItem(_))
                    || (depth == 0 && matches!(node, Node::Blockquote(_)))
                {
                    range.start
                } else {
                    owner
                };
                if let Some(children) = node.children().filter(|children| !children.is_empty()) {
                    for child in children {
                        Self::container(
                            source,
                            child,
                            depth,
                            quotes,
                            owner,
                            container.clone(),
                            images,
                            leaves,
                        );
                    }
                } else {
                    let start = source[..range.start].rfind('\n').map_or(0, |ix| ix + 1);
                    let style = super::lists::style(source, range.start, depth, quotes, owner);
                    let mut leaf = Leaf::new(
                        start..range.end,
                        if depth > 0 {
                            Kind::List(style)
                        } else {
                            Kind::Quote(style)
                        },
                    );
                    leaf.container = Some(container);
                    leaf.prefix(source, start..range.end);
                    leaf.insertion = range.end;
                    leaves.push(leaf);
                }
            }
            _ => {
                let first = leaves.len();
                if super::mdx::project(source, node, images, leaves) {
                    for leaf in &mut leaves[first..] {
                        leaf.container_range(container.clone());
                    }
                    return;
                }
                let start = source[..range.start].rfind('\n').map_or(0, |ix| ix + 1);
                let style = super::lists::style(source, range.start, depth, quotes, owner);
                let prose = matches!(node, Node::Paragraph(_) | Node::Heading(_));
                let kind = if prose {
                    if depth > 0 {
                        Kind::List(style)
                    } else {
                        Kind::Quote(style)
                    }
                } else {
                    super::regions::classify(node, &source[range.clone()])
                };
                if let Some(previous) = leaves
                    .last()
                    .filter(|previous| previous.range.end + 1 < start)
                {
                    let first = leaves.len();
                    Self::gap(source, previous.range.end..start, leaves);
                    if quotes > 0 {
                        for leaf in &mut leaves[first..] {
                            let raw = &source[leaf.range.clone()];
                            if raw.contains('>') && super::lists::prefix_len(raw) == raw.len() {
                                let style = super::lists::style(
                                    source,
                                    leaf.range.start,
                                    depth,
                                    quotes,
                                    owner,
                                );
                                let range = leaf.range.clone();
                                *leaf = Leaf::new(
                                    range.clone(),
                                    if depth > 0 {
                                        Kind::List(style)
                                    } else {
                                        Kind::Quote(style)
                                    },
                                );
                                leaf.prefix(source, range.clone());
                                leaf.insertion = range.end;
                                leaf.container = Some(container.clone());
                            }
                        }
                    }
                }
                let mut leaf = Leaf::new(start..range.end, kind);
                leaf.image_definitions = images.clone();
                leaf.container = Some(container);
                if prose {
                    let empty_prefix =
                        super::lists::prefix_len(&source[start..range.end]) == range.end - start;
                    if empty_prefix {
                        leaf.prefix(source, start..range.end);
                    } else {
                        leaf.prefix(source, start..range.start);
                        if let Some(children) = node.children() {
                            for child in children {
                                Self::inline(source, child, &mut leaf);
                            }
                        }
                    }
                    if empty_prefix || leaf.segments.is_empty() {
                        leaf.insertion = range.end;
                    }
                } else if kind == Kind::Math {
                    leaf.prefix(source, start..range.start);
                    Self::inline(source, node, &mut leaf);
                } else {
                    leaf.raw(source, start..range.end);
                }
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
    pub(super) fn inline(source: &str, node: &Node, leaf: &mut Leaf) {
        let Some(position) = node.position() else {
            return;
        };
        let range = position.start.offset..position.end.offset;
        if let Some(image) = super::images::image(node, &leaf.image_definitions) {
            leaf.push_normalized(range.clone(), "\u{FFFC}", &source[range]);
            leaf.segments.last_mut().unwrap().image = Some(image);
            return;
        }
        if let Some(formula) = super::math::Formula::from_node(node, &source[range.clone()]) {
            leaf.push_normalized(range.clone(), "\u{FFFC}", &source[range]);
            leaf.segments.last_mut().unwrap().math = Some(formula);
            return;
        }
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
                    target: None,
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
                    target: None,
                });
                let before = leaf.style;
                leaf.style.code = true;
                leaf.push_segment(
                    range.start..start,
                    &source[range.start..start],
                    Some(ix),
                    None,
                );
                let mut at = start;
                while at < end {
                    if matches!(leaf.kind, Kind::TableCell(_)) && source[at..end].starts_with("\\|")
                    {
                        leaf.push_normalized(at..at + 2, "|", "\\|");
                        at += 2;
                        continue;
                    }
                    let ch = source[at..end].chars().next().unwrap();
                    leaf.push_normalized(
                        at..at + ch.len_utf8(),
                        if ch == '\n' {
                            " "
                        } else {
                            &source[at..at + ch.len_utf8()]
                        },
                        &source[at..at + ch.len_utf8()],
                    );
                    at += ch.len_utf8();
                }
                leaf.push_segment(end..range.end, &source[end..range.end], Some(ix), None);
                leaf.style = before;
            }
            Node::Link(_) | Node::LinkReference(_) => {
                let label = super::links::label_range(source, node).unwrap_or_else(|| {
                    if source[range.clone()].starts_with('<') {
                        range.start + 1..range.end - 1
                    } else {
                        range.clone()
                    }
                });
                let ix = leaf.spans.len();
                leaf.spans.push(InlineSpan {
                    source: range.clone(),
                    revealed: false,
                    target: None,
                });
                leaf.push_segment(
                    range.start..label.start,
                    &source[range.start..label.start],
                    Some(ix),
                    None,
                );
                let before = leaf.style;
                leaf.style.link = Some(ix);
                if let Some(children) = node.children() {
                    for child in children {
                        Self::inline(source, child, leaf);
                    }
                }
                leaf.style = before;
                leaf.push_segment(
                    label.end..range.end,
                    &source[label.end..range.end],
                    Some(ix),
                    None,
                );
            }
            // Other inline objects retain their source until their editing slice.
            _ => leaf.raw(source, range),
        }
    }
    pub fn leaf_at(&self, at: usize) -> Option<&Leaf> {
        let ix = self
            .leaves
            .partition_point(|leaf| leaf.range.start <= at)
            .saturating_sub(1);
        self.leaves.get(ix)
    }
    pub fn install(&mut self, mut newer: Self) {
        if newer.error.is_some() && self.revision == newer.revision && self.grammar == newer.grammar
        {
            self.error = newer.error;
            return;
        }
        if self.grammar == newer.grammar {
            if newer.live {
                newer.mdx.reconcile(&self.mdx, &mut self.next_id);
            } else {
                newer.mdx = std::mem::take(&mut self.mdx);
            }
        }
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
    /// Reveal one component's exact source in the existing leaf/input geometry.
    /// This derived presentation never replaces the document or its history.
    pub(super) fn component_sources(&mut self, source: &Rope, state: &super::mdx::ViewState) {
        if !self.live || state.source_components.is_empty() {
            return;
        }
        let ranges: Vec<_> = self
            .mdx
            .components
            .iter()
            .filter(|component| state.source_components.contains(&component.id()))
            .map(|component| component.source_range())
            .collect();
        for range in &ranges {
            if ranges
                .iter()
                .any(|outer| outer != range && outer.start <= range.start && range.end <= outer.end)
            {
                continue;
            }
            let Some(first) = self
                .leaves
                .iter()
                .position(|leaf| leaf.range.end > range.start && leaf.range.start < range.end)
            else {
                continue;
            };
            let last = self
                .leaves
                .iter()
                .rposition(|leaf| leaf.range.start < range.end && leaf.range.end >= range.start)
                .unwrap();
            if first == last
                && self.leaves[first].range == *range
                && self.leaves[first].kind == Kind::Mdx(super::mdx::Field::Dynamic)
            {
                continue;
            }
            let start = self.leaves[first].range.start.min(range.start);
            let end = self.leaves[last].range.end.max(range.end);
            let id = self.leaves[first].id;
            let mut replacement = Vec::with_capacity(3);
            for part in [start..range.start, range.clone(), range.end..end] {
                if part.is_empty() {
                    continue;
                }
                let mut leaf = Leaf::new(part.clone(), Kind::Mdx(super::mdx::Field::Dynamic));
                leaf.push(part.clone(), &source.slice(part.clone()).to_string());
                leaf.rebuild(None);
                leaf.id = if part == *range {
                    id
                } else {
                    let id = self.next_id;
                    self.next_id += 1;
                    id
                };
                leaf.version = self.next_id;
                self.next_id += 1;
                replacement.push(leaf);
            }
            self.leaves.splice(first..=last, replacement);
        }
    }
    pub fn rebase(&mut self, source: &Rope, transaction: &Transaction, live: bool) {
        if transaction.edits.is_empty() {
            return;
        }
        // Absolute destinations and definitions must never be rebased against a
        // fragment. The revision-checked background parse restores this index.
        self.links = super::links::Index::default();
        self.mdx.rebase(transaction, source);
        // Only affected leaves and their immediate neighbours are reparsed on
        // input. The debounced background parse resolves larger syntax changes.
        let mut old = transaction
            .edits
            .iter()
            .map(|edit| edit.range.clone())
            .reduce(|a, b| a.start.min(b.start)..a.end.max(b.end))
            .unwrap();
        const INPUT_PARSE_LIMIT: usize = 32 * 1024;
        let mut local_live = live;
        // Keep affected child leaves inside their original AST container while
        // reparsing, so a nested item cannot temporarily become indented code.
        for leaf in &self.leaves {
            if leaf.range.start <= old.end && leaf.range.end >= old.start {
                if let Some(container) = &leaf.container {
                    if container.len() <= INPUT_PARSE_LIMIT {
                        old.start = old.start.min(container.start);
                        old.end = old.end.max(container.end);
                    } else {
                        // A document-sized list must not turn one keystroke
                        // into a synchronous document parse. Keep its edited
                        // source editable until the background projection lands.
                        local_live = false;
                    }
                }
            }
        }
        let mut first = self
            .leaves
            .iter()
            .position(|leaf| leaf.range.end >= old.start)
            .unwrap_or(0)
            .saturating_sub(1);
        let mut last = (self
            .leaves
            .iter()
            .rposition(|leaf| leaf.range.start <= old.end)
            .unwrap_or(first)
            + 2)
        .min(self.leaves.len());
        // A cell fragment is not a standalone GFM table. Reparse its entire
        // owning table, retaining the delimiter row and neighboring context.
        if let Kind::TableCell(cell) = self.leaves[first].kind {
            while first > 0
                && matches!(self.leaves[first - 1].kind, Kind::TableCell(other) if other.table() == cell.table())
            {
                first -= 1;
            }
        }
        if let Kind::TableCell(cell) = self.leaves[last - 1].kind {
            while last < self.leaves.len()
                && matches!(self.leaves[last].kind, Kind::TableCell(other) if other.table() == cell.table())
            {
                last += 1;
            }
        }
        let map = |at| {
            transaction
                .map_selection(super::Selection::caret(at))
                .head()
        };
        let first_start = match self.leaves[first].kind {
            Kind::TableCell(cell) => cell.table().start,
            _ => self.leaves[first].range.start.min(old.start),
        };
        let start = map(first_start);
        let mut end = match self.leaves[last - 1].kind {
            Kind::TableCell(cell) => cell.table().end,
            _ => self.leaves[last - 1].range.end.max(old.end),
        };
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
        let mut identities: Vec<_> = self.leaves[first..last]
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
        let mut patch = if end - start <= INPUT_PARSE_LIMIT {
            let fragment = source.slice(start..end).to_string();
            let mut leaves = Self::parse_fragment(
                &fragment,
                transaction.revision(),
                local_live,
                start == 0,
                self.grammar,
            )
            .leaves;
            for leaf in &mut leaves {
                leaf.shift(0, start);
            }
            leaves
        } else {
            // Do not copy a document-sized paragraph/table just to keep input
            // current. These range-only leaves borrow no second source; only
            // the visible chunks materialize from the authoritative Rope.
            let mut leaves = Vec::new();
            let mut at = start;
            while at < end {
                let next = super::MarkdownDocument::source_boundary(
                    source,
                    (at + INPUT_PARSE_LIMIT / 2).min(end),
                    true,
                )
                .min(end);
                let mut leaf = Leaf::new(at..next, Kind::Source);
                leaf.deferred = true;
                leaves.push(leaf);
                at = next;
            }
            leaves
        };
        for leaf in &mut patch {
            leaf.id = if let Some(ix) = identities
                .iter()
                .position(|(at, _)| leaf.range.contains(at) || leaf.range.end == *at)
            {
                // One old identity can belong to only one new leaf after a
                // split. In particular, adjacent empty leaves cannot share it.
                identities.remove(ix).1
            } else {
                let id = self.next_id;
                self.next_id += 1;
                id
            };
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
        self.leaves.splice(first..last, patch);
        self.revision = transaction.revision();
    }
}
