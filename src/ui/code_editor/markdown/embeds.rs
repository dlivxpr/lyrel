//! Derived readonly fragments. All I/O is through the captured document Host.
use super::{
    links, mdx, parser,
    projection::{Kind, Projection},
};
use crate::ui::{editor_text, host_ops::Host, i18n::L10nKey};
use gpui_kit::base::text::editor_host::{BlockNode, CodeBlock, Paragraph};
use gpui_kit::{EntityId, RenderImage};
use std::{
    collections::{HashMap, HashSet},
    hash::{Hash, Hasher},
    ops::Range,
    path::{Path, PathBuf},
    sync::Arc,
};

const MAX_FILES_BYTES: usize = 8 * 1024 * 1024;
const MAX_ITEMS: usize = 64;
const MAX_LEAVES: usize = 4096;

pub(crate) struct Style {
    math: super::math::Style,
    diagrams: super::mermaid::Style,
    identity: EntityId,
}
impl Style {
    pub(crate) fn current(
        window: &gpui_kit::Window,
        cx: &gpui_kit::App,
        identity: EntityId,
    ) -> Self {
        use gpui_kit::component::ActiveTheme as _;
        Self {
            math: super::math::Style::new(cx.theme().foreground, window.rem_size()),
            diagrams: super::mermaid::Style::current(cx),
            identity,
        }
    }
}

pub(crate) struct Request {
    revision: u64,
    references: Vec<links::Link>,
    preview: Option<links::Link>,
}
impl Request {
    pub(super) fn new(revision: u64, references: &[links::Link]) -> Self {
        Self {
            revision,
            references: references.to_vec(),
            preview: None,
        }
    }
}
impl Request {
    pub(super) fn with_preview(mut self, preview: Option<links::Link>) -> Self {
        self.preview = preview;
        self
    }
}
pub(crate) struct Loaded {
    pub(super) entries: Vec<Entry>,
    pub(super) preview: Option<Entry>,
    stamp: u64,
    revision: u64,
    math_style: super::math::Style,
    diagram_style: super::mermaid::Style,
    files: HashMap<PathBuf, Arc<Indexed>>,
}
pub(super) struct Entry {
    pub(super) reference: links::Link,
    pub(super) result: Result<Arc<Fragment>, L10nKey>,
}
pub(super) struct Fragment {
    pub(super) title: String,
    pub(super) projection: Projection,
    pub(super) text: gpui_kit::SharedString,
    pub(super) blocks: Vec<BlockNode>,
    pub(super) nested: Vec<Entry>,
    pub(super) images: Arc<HashMap<String, Arc<RenderImage>>>,
    pub(super) image_errors: Vec<L10nKey>,
    pub(super) diagrams: super::mermaid::State,
}
struct Indexed {
    source: Arc<str>,
    root: markdown::mdast::Node,
    index: links::Index,
}
struct Plan {
    source: Arc<str>,
    title: String,
    path: PathBuf,
    range: Range<usize>,
    nested: Vec<(links::Link, Result<Plan, L10nKey>)>,
}
struct Budget {
    leaves: usize,
    raster_bytes: usize,
    images: HashMap<PathBuf, Arc<RenderImage>>,
    formulas: HashMap<super::math::Formula, super::math::Output>,
}
fn raster_bytes(image: &RenderImage) -> usize {
    let size = image.size(0);
    (size.width.0 as usize)
        .saturating_mul(size.height.0 as usize)
        .saturating_mul(4)
}
struct Resolver<'a> {
    host: &'a dyn Host,
    open: &'a HashMap<PathBuf, super::Rope>,
    files: HashMap<PathBuf, Arc<Indexed>>,
    cached: Option<&'a HashMap<PathBuf, Arc<Indexed>>>,
    stack: Vec<(PathBuf, String)>,
    seen: HashSet<(PathBuf, String)>,
    remaining: usize,
    bytes: usize,
    stamp: std::collections::hash_map::DefaultHasher,
}
// 与参考 hover card 一致：最多八个非空行、360 个字符，保留段落，跳过隐藏注释。
fn preview_excerpt(source: &str) -> String {
    let mut excerpt = String::new();
    let mut lines = 0;
    let mut chars = 0;
    for line in source.lines().map(str::trim_end).filter(|line| !line.trim_start().starts_with("<!--")) {
        if !line.is_empty() { lines += 1; if lines > 8 { break; } }
        if !excerpt.is_empty() { excerpt.push('\n'); chars += 1; }
        for ch in line.chars() {
            if chars == 360 { excerpt.push('…'); return excerpt; }
            excerpt.push(ch);
            chars += 1;
        }
    }
    excerpt
}
impl Resolver<'_> {
    fn target(
        &mut self,
        owner: &Path,
        reference: &links::Link,
        depth: usize,
        preview: bool,
    ) -> Result<Plan, L10nKey> {
        if depth > 3 || self.remaining == 0 {
            return Err(L10nKey::MarkdownEmbedLimit);
        }
        self.remaining -= 1;
        let path = match links::resolve(owner, self.host, &reference.target)? {
            links::Target::File(path) => path,
            links::Target::Anchor => owner.to_path_buf(),
            links::Target::External(_) => return Err(L10nKey::MarkdownLinkUnsupportedScheme),
        };
        let path = if self.open.contains_key(&path) {
            path
        } else {
            links::references::file_path(self.host, &path)?
        };
        if !super::super::is_markdown_path(&path) {
            return Err(L10nKey::MarkdownEmbedUnsupported);
        }
        let fragment = reference
            .target
            .split_once('#')
            .map(|(_, fragment)| fragment.to_owned())
            .unwrap_or_default();
        let decoded = percent_encoding::percent_decode_str(&fragment)
            .decode_utf8()
            .map_err(|_| L10nKey::MarkdownLinkInvalidDestination)?
            .into_owned();
        let key = (path.clone(), decoded);
        if self.stack.contains(&key) {
            return Err(L10nKey::MarkdownEmbedCycle);
        }
        if !self.seen.insert(key.clone()) {
            return Err(L10nKey::MarkdownEmbedDuplicate);
        }
        if !self.files.contains_key(&path) {
            if self.bytes >= MAX_FILES_BYTES {
                return Err(L10nKey::MarkdownEmbedLimit);
            }
            let source = if let Some(current) = self.open.get(&path) {
                current.to_string()
            } else {
                let before = self
                    .host
                    .stat(&path)
                    .map_err(|_| L10nKey::MarkdownReferenceUnreadable)?;
                let bytes = self
                    .host
                    .read_file(&path, 4 * 1024 * 1024)
                    .map_err(|_| L10nKey::MarkdownReferenceUnreadable)?;
                let after = self
                    .host
                    .stat(&path)
                    .map_err(|_| L10nKey::MarkdownReferenceUnreadable)?;
                if before.mtime != after.mtime || before.len != after.len {
                    return Err(L10nKey::MarkdownEmbedChanged);
                }
                editor_text::decode_source(&bytes)
                    .map_err(|_| L10nKey::MarkdownReferenceUnreadable)?
                    .ok_or(L10nKey::MarkdownReferenceUnreadable)?
                    .text
            };
            self.bytes = self.bytes.saturating_add(source.len());
            if self.bytes > MAX_FILES_BYTES {
                return Err(L10nKey::MarkdownEmbedLimit);
            }
            path.hash(&mut self.stamp);
            source.hash(&mut self.stamp);
            let indexed = if let Some(cached) = self
                .cached
                .and_then(|files| files.get(&path))
                .filter(|cached| cached.source.as_ref() == source)
            {
                cached.clone()
            } else {
                let root = parser::parse(&source, mdx::Grammar::for_path(Some(&path)), true)
                    .map_err(|_| L10nKey::EditorMdxParseError)?;
                let index = links::Index::from_ast(&source, &root);
                Arc::new(Indexed {
                    source: source.into(),
                    root,
                    index,
                })
            };
            self.files.insert(path.clone(), indexed);
        }
        let indexed = &self.files[&path];
        let title = if preview {
            indexed.root.children().into_iter().flatten()
                .find(|node| matches!(node, markdown::mdast::Node::Heading(_)))
                .map(|node| node.to_string())
                .unwrap_or_else(|| path.file_stem().unwrap_or_default().to_string_lossy().into_owned())
        } else { String::new() };
        let source = if preview { Arc::from(preview_excerpt(&indexed.source)) } else { indexed.source.clone() };
        let range = if preview { 0..source.len() } else {
            indexed.index.fragment(&indexed.source, &indexed.root, Some(&fragment))?
        };
        let references: Vec<_> = indexed
            .index
            .embeds
            .iter()
            .filter(|reference| {
                !preview &&
                range.start <= reference.source.start && reference.source.end <= range.end
            })
            .take(MAX_ITEMS + 1)
            .cloned()
            .collect();
        self.stack.push(key);
        let nested = references
            .into_iter()
            .take(MAX_ITEMS + 1)
            .map(|mut reference| {
                let result = self.target(&path, &reference, depth + 1, false);
                reference.source =
                    reference.source.start - range.start..reference.source.end - range.start;
                reference.label =
                    reference.label.start - range.start..reference.label.end - range.start;
                reference.destination = reference.destination.start - range.start
                    ..reference.destination.end - range.start;
                (reference, result)
            })
            .collect();
        self.stack.pop();
        Ok(Plan {
            source,
            title,
            path,
            range,
            nested,
        })
    }
}
impl Plan {
    fn prepare(
        self,
        host: &dyn Host,
        math_style: super::math::Style,
        diagram_style: &super::mermaid::Style,
        identity: EntityId,
        budget: &mut Budget,
    ) -> Result<Arc<Fragment>, L10nKey> {
        let mut projection = Projection::parse(
            &self.source[self.range],
            0,
            true,
            mdx::Grammar::for_path(Some(&self.path)),
        );
        if projection.leaves.len() > budget.leaves {
            return Err(L10nKey::MarkdownEmbedLimit);
        }
        budget.leaves -= projection.leaves.len();
        for leaf in &mut projection.leaves {
            leaf.readonly();
        }
        let mut math = super::math::State::default();
        for formula in projection.leaves.iter().flat_map(|leaf| leaf.formulas()) {
            let output = budget.formulas.entry(formula.clone()).or_insert_with(|| {
                if budget.raster_bytes == 0 {
                    return super::math::Output::Failed(
                        crate::ui::i18n::t(L10nKey::MarkdownEmbedLimit).to_owned(),
                    );
                }
                let output = super::math::render(formula, math_style);
                if let super::math::Output::Ready { image, .. } = &output {
                    let bytes = raster_bytes(image);
                    if bytes > budget.raster_bytes {
                        return super::math::Output::Failed(
                            crate::ui::i18n::t(L10nKey::MarkdownEmbedLimit).to_owned(),
                        );
                    }
                    budget.raster_bytes -= bytes;
                }
                output
            });
            math.outputs.insert(formula.clone(), output.clone());
        }
        let mut diagrams = super::mermaid::State::default();
        diagrams.request(&projection, identity, diagram_style.clone());
        if let Some(job) = diagrams.take_job() {
            diagrams.complete(job.run_bounded(&mut budget.raster_bytes));
        }
        diagrams.install(&projection, identity, diagram_style);
        let mut images = HashMap::new();
        let mut image_errors = Vec::new();
        let mut seen = HashSet::new();
        for url in projection
            .leaves
            .iter()
            .flat_map(|leaf| leaf.image_urls())
            .filter(|url| seen.insert((*url).to_owned()))
        {
            let result = (|| {
                let links::Target::File(path) = links::resolve(&self.path, host, url)? else {
                    return Err(L10nKey::MarkdownEmbedExternal);
                };
                if let Some(image) = budget.images.get(&path) {
                    return Ok(image.clone());
                }
                if budget.raster_bytes == 0 {
                    return Err(L10nKey::MarkdownEmbedLimit);
                }
                let bytes = host
                    .read_file(&path, budget.raster_bytes.min(32 * 1024 * 1024) as u64)
                    .map_err(|_| L10nKey::MarkdownReferenceUnreadable)?;
                let image = super::images::decode(&bytes)
                    .map_err(|_| L10nKey::MarkdownReferenceUnreadable)?;
                let bytes = raster_bytes(&image);
                if bytes > budget.raster_bytes {
                    return Err(L10nKey::MarkdownEmbedLimit);
                }
                budget.raster_bytes -= bytes;
                budget.images.insert(path, image.clone());
                Ok(image)
            })();
            match result {
                Ok(image) => {
                    images.insert(url.to_owned(), image);
                }
                Err(reason) => image_errors.push(reason),
            }
        }
        let dimensions = images
            .iter()
            .map(|(url, image)| (url.clone(), super::images::State::Ready(image.clone())))
            .collect();
        for leaf in &mut projection.leaves {
            leaf.image_dimensions(&dimensions);
        }
        let text = projection
            .leaves
            .iter()
            .map(|leaf| leaf.text.as_str())
            .collect::<Vec<_>>()
            .join("\n")
            .into();
        let mut blocks = Vec::with_capacity(projection.leaves.len());
        let mut ix = 0;
        while ix < projection.leaves.len() {
            let leaf = &projection.leaves[ix];
            if let Kind::TableCell(first) = leaf.kind {
                use gpui_kit::base::text::editor_host::{ColumnAlign, Table};
                let mut rows = Vec::new();
                let mut align = Vec::new();
                let start = ix;
                while ix < projection.leaves.len()
                    && matches!(projection.leaves[ix].kind, Kind::TableCell(cell) if cell.table() == first.table())
                {
                    let mut row = Vec::new();
                    let Kind::TableCell(row_cell) = projection.leaves[ix].kind else {
                        unreachable!()
                    };
                    while ix < projection.leaves.len()
                        && matches!(projection.leaves[ix].kind, Kind::TableCell(cell) if cell.same_row(row_cell))
                    {
                        let cell_leaf = &projection.leaves[ix];
                        if let Kind::TableCell(cell) = cell_leaf.kind {
                            if cell.is_header() {
                                align.push(match cell.alignment() {
                                    markdown::mdast::AlignKind::Left => ColumnAlign::Left,
                                    markdown::mdast::AlignKind::Right => ColumnAlign::Right,
                                    markdown::mdast::AlignKind::Center => ColumnAlign::Center,
                                    markdown::mdast::AlignKind::None => ColumnAlign::None,
                                });
                            }
                        }
                        let mut paragraph = Paragraph::default();
                        for node in cell_leaf.inline_nodes(None, &math) {
                            paragraph.push(node);
                        }
                        row.push(paragraph);
                        ix += 1;
                    }
                    rows.push(row);
                }
                blocks.push(BlockNode::Table(Table::new(rows, align)));
                blocks.extend((start + 1..ix).map(|_| BlockNode::Unknown));
                continue;
            }
            let mut paragraph = Paragraph::default(); // Unkeyed: never enter the outer editor geometry collector.
            if let Kind::List(style) = leaf.kind {
                if let Some(checked) = style.task {
                    paragraph.push(gpui_kit::base::text::editor_host::InlineNode::new(
                        if checked { "☑ " } else { "☐ " },
                    ));
                }
            }
            for node in leaf.inline_nodes(None, &math) {
                paragraph.push(node);
            }
            let mut block = match leaf.kind {
                Kind::Heading(level) => BlockNode::Heading {
                    level,
                    children: paragraph,
                    span: None,
                },
                Kind::Rule => BlockNode::HorizontalRule { span: None },
                Kind::Code(_) | Kind::Mermaid(_) => BlockNode::CodeBlock(CodeBlock::from_code(
                    leaf.text.clone(),
                    None::<gpui_kit::SharedString>,
                )),
                _ => BlockNode::Paragraph(paragraph),
            };
            if let Kind::List(style) | Kind::Quote(style) = leaf.kind {
                if style.task.is_none() {
                    if let Some(marker) = style.marker {
                        block = BlockNode::List {
                            children: vec![BlockNode::ListItem {
                                children: vec![block],
                                spread: false,
                                checked: None,
                                span: None,
                            }],
                            ordered: matches!(marker, super::lists::Marker::Ordered(..)),
                            start: match marker {
                                super::lists::Marker::Ordered(number, _) => Some(number),
                                _ => None,
                            },
                            span: None,
                        };
                    }
                }
                for _ in 0..style.quotes {
                    block = BlockNode::Blockquote {
                        children: vec![block],
                        span: None,
                    };
                }
            }
            blocks.push(block);
            ix += 1;
        }
        let nested = self
            .nested
            .into_iter()
            .map(|(reference, result)| Entry {
                reference,
                result: result.and_then(|plan| {
                    plan.prepare(host, math_style, diagram_style, identity, budget)
                }),
            })
            .collect();
        Ok(Arc::new(Fragment {
            title: self.title,
            projection,
            text,
            blocks,
            nested,
            images: Arc::new(images),
            image_errors,
            diagrams,
        }))
    }
}
/// Called only on the Host worker. Open buffers are immutable snapshots of the
/// existing MarkdownDocument authority, not fragment editing buffers.
pub(crate) fn load(
    host: &dyn Host,
    owner: &Path,
    request: Request,
    open: &HashMap<PathBuf, super::Rope>,
    previous: Option<Arc<Loaded>>,
    style: Style,
) -> Arc<Loaded> {
    let Style {
        math: math_style,
        diagrams: diagram_style,
        identity,
    } = style;
    let mut resolver = Resolver {
        host,
        open,
        files: HashMap::new(),
        cached: previous.as_ref().map(|previous| &previous.files),
        stack: Vec::new(),
        seen: HashSet::new(),
        remaining: MAX_ITEMS,
        bytes: 0,
        stamp: Default::default(),
    };
    let plans: Vec<_> = request
        .references
        .into_iter()
        .take(MAX_ITEMS + 1)
        .map(|reference| {
            let result = resolver.target(owner, &reference, 1, false);
            (reference, result)
        })
        .collect();
    let preview = request.preview.map(|mut reference| {
        reference.target = reference.target.split('#').next().unwrap_or("").to_owned();
        let result = resolver.target(owner, &reference, 1, true);
        (reference, result)
    });
    // Error changes participate in cache invalidation, including disappearance,
    // ambiguity and permission recovery with unchanged owner bytes.
    fn hash_results(plans: &[(links::Link, Result<Plan, L10nKey>)], stamp: &mut impl Hasher) {
        for (_, result) in plans {
            match result {
                Ok(plan) => {
                    0u8.hash(stamp);
                    hash_results(&plan.nested, stamp);
                }
                Err(reason) => {
                    format!("{reason:?}").hash(stamp);
                }
            }
        }
    }
    hash_results(&plans, &mut resolver.stamp);
    if let Some(preview) = &preview { hash_results(std::slice::from_ref(preview), &mut resolver.stamp); }
    let stamp = resolver.stamp.finish();
    if let Some(previous) = previous.as_ref().filter(|previous| {
        previous.revision == request.revision
            && previous.stamp == stamp
            && previous.preview.as_ref().map(|entry| &entry.reference) == preview.as_ref().map(|(reference, _)| reference)
            && previous.math_style == math_style
            && previous.diagram_style == diagram_style
            && previous.files.len() == resolver.files.len()
            && resolver.files.iter().all(|(path, current)| {
                previous
                    .files
                    .get(path)
                    .is_some_and(|old| old.source == current.source)
            })
    }) {
        return previous.clone();
    }
    let mut budget = Budget {
        leaves: MAX_LEAVES,
        raster_bytes: 32 * 1024 * 1024,
        images: HashMap::new(),
        formulas: HashMap::new(),
    };
    let entries = plans
        .into_iter()
        .map(|(reference, result)| Entry {
            reference,
            result: result.and_then(|plan| {
                plan.prepare(host, math_style, &diagram_style, identity, &mut budget)
            }),
        })
        .collect();
    let preview = preview.map(|(reference, result)| Entry {
        reference,
        result: result.and_then(|plan| plan.prepare(host, math_style, &diagram_style, identity, &mut budget)),
    });
    Arc::new(Loaded {
        entries,
        preview,
        stamp,
        revision: request.revision,
        math_style,
        diagram_style,
        files: resolver.files,
    })
}
