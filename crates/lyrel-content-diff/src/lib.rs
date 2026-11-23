//! 内容感知 Diff 的结构匹配器（GUI-free）。
//!
//! 输入同一文件的两个文本版本，输出按内容结构组织的修改、移动、新增和
//! 删除块。每项差异指向相关版本中的源行范围；移动块的旧侧位置以
//! [`Item::Source`] 锚点单独标出，供审阅视图跳转。
//!
//! 匹配是保守的：只有可靠证据（内容完全一致，或同名同类且互相唯一最优
//! 的相似块）才建立移动/修改关系；其余一律保持为新增和删除，不断言无法
//! 确立的关系。仅格式（空白）变化的块标记为 [`Block::formatting_only`]，
//! 由视图折叠成可发现的提示。

mod languages;

use std::collections::HashMap;
use std::path::Path;

use languages::{KindSpec, LangSpec, NameRule};

/// One side of the comparison exceeded a budget.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ContentError {
    /// The language has no structural grammar in this build.
    Unsupported,
    /// The bytes are not reviewable text.
    Binary,
    /// A named budget was exceeded; the string names the limit for the
    /// fallback explanation.
    OverBudget(&'static str),
    /// The text did not parse cleanly (tree-sitter ERROR nodes), so no
    /// structural claim can be made. The line view is the right fallback.
    ParseFailed,
}

impl std::fmt::Display for ContentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ContentError::Unsupported => write!(f, "unsupported language"),
            ContentError::Binary => write!(f, "binary content"),
            ContentError::OverBudget(limit) => write!(f, "over the {limit} budget"),
            ContentError::ParseFailed => write!(f, "structural parse failed"),
        }
    }
}

/// A 1-based inclusive line range in one version of the file.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct LineSpan {
    pub start: u32,
    pub end: u32,
}

impl LineSpan {
    pub fn len(self) -> u32 {
        self.end - self.start + 1
    }
}

/// What happened to one structural item.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BlockKind {
    /// Same content at a different position; inner edits are empty.
    Moved,
    /// Same identity, changed content, at the same position.
    Modified,
    /// Same identity at a different position, with edits inside.
    MovedEdited,
    /// Only in the new version.
    Added,
    /// Only in the old version.
    Deleted,
}

/// One line inside a block's inner diff.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EditKind {
    Context,
    Added,
    Removed,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Edit {
    /// 1-based file line in the old version, if present.
    pub old: Option<u32>,
    /// 1-based file line in the new version, if present.
    pub new: Option<u32>,
    pub kind: EditKind,
}

/// One structural change, in new-file reading order (see
/// [`StructuralDiff::items`]).
#[derive(Debug)]
pub struct Block {
    pub kind: BlockKind,
    /// The item family label, e.g. `function` — data from the grammar, not
    /// user-facing copy.
    pub label: &'static str,
    /// The item's identity name, when the grammar provides one.
    pub name: Option<String>,
    pub old: Option<LineSpan>,
    pub new: Option<LineSpan>,
    /// The block's inner line diff. Empty for `Moved` and for
    /// `formatting_only` blocks.
    pub edits: Vec<Edit>,
    /// The two sides differ only in whitespace runs: comments, string
    /// contents and significant indentation are unchanged. Views collapse
    /// these blocks into a discoverable marker.
    pub formatting_only: bool,
}

/// One entry of the display sequence.
#[derive(Debug)]
pub enum Item {
    Block(Block),
    /// Old-side anchor: content that used to sit at `old` now lives at
    /// `new_start`. Views render this as a jumpable stub.
    Source {
        label: &'static str,
        name: Option<String>,
        old: LineSpan,
        new_start: u32,
    },
}

/// The structural comparison of one file's two versions.
#[derive(Debug)]
pub struct StructuralDiff {
    /// Display sequence, in reading order: blocks and old-side anchors.
    /// Ordering is by new-file position; deleted blocks and source anchors
    /// are placed by their old-file position relative to the blocks around
    /// them.
    pub items: Vec<Item>,
    pub old_lines: u32,
    pub new_lines: u32,
}

/// Per-side byte budget. Files above this use the line-view fallback.
pub const MAX_FILE_BYTES: usize = 1 << 20;
/// Per-side line budget, mirroring the patch panel's whole-tree budget.
pub const MAX_LINES: u32 = 20_000;
/// Structural items per side beyond which the matcher declines.
pub const MAX_ITEMS: usize = 4_000;
/// Blocks larger than this on either side are not fuzzy-matched; they stay
/// additions/deletions rather than risk a pathological diff.
pub const MAX_FUZZY_BLOCK_LINES: usize = 10_000;
/// Minimum token similarity for a same-name pair to count as a match.
pub const SIMILARITY_THRESHOLD: f64 = 0.5;

/// The grammar name for a reviewable path, if this build supports one.
pub fn language_for_path(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?;
    languages::spec_for_extension(ext).map(|s| s.name)
}

/// Compare `old_text` with `new_text` structurally in `language`.
pub fn structural_diff(
    language: &str,
    old_text: &str,
    new_text: &str,
) -> Result<StructuralDiff, ContentError> {
    let spec = languages::spec(language).ok_or(ContentError::Unsupported)?;
    if old_text.len() > MAX_FILE_BYTES || new_text.len() > MAX_FILE_BYTES {
        return Err(ContentError::OverBudget("1 MiB"));
    }
    let old_lines = line_index(old_text);
    let new_lines = line_index(new_text);
    if old_lines.line_count() > MAX_LINES || new_lines.line_count() > MAX_LINES {
        return Err(ContentError::OverBudget("20,000 lines"));
    }

    let old_items = extract_items(spec, old_text)?;
    let new_items = extract_items(spec, new_text)?;
    if old_items.len() > MAX_ITEMS || new_items.len() > MAX_ITEMS {
        return Err(ContentError::OverBudget("4,000 items"));
    }

    Ok(assemble(old_items, new_items, &old_lines, &new_lines))
}

/// Byte offset of each line start, for mapping spans to line numbers.
struct LineIndex {
    starts: Vec<u32>,
    text_len: usize,
}

impl LineIndex {
    fn line_count(&self) -> u32 {
        self.starts.len() as u32
    }

    /// 1-based line number for `byte`.
    fn line_at(&self, byte: usize) -> u32 {
        match self.starts.binary_search(&(byte as u32)) {
            Ok(ix) => ix as u32 + 1,
            Err(ix) => ix as u32,
        }
    }
}

fn line_index(text: &str) -> LineIndex {
    let mut starts = vec![0u32];
    for (ix, b) in text.bytes().enumerate() {
        if b == b'\n' && ix + 1 < text.len() {
            starts.push((ix + 1) as u32);
        }
    }
    LineIndex { starts, text_len: text.len() }
}

/// One structural item before matching.
struct RawItem {
    label: &'static str,
    name: Option<String>,
    start_byte: usize,
    end_byte: usize,
    /// The exact text of the item.
    text: String,
    /// Whitespace-normalized token stream, for formatting comparisons.
    norm: String,
}

impl RawItem {
    fn span(&self, index: &LineIndex) -> LineSpan {
        LineSpan {
            start: index.line_at(self.start_byte),
            end: index.line_at(self.end_byte.saturating_sub(1).min(index.text_len)),
        }
    }

    /// The item's lines, for whole-block add/delete edit lists.
    fn lines(&self) -> Vec<&str> {
        self.text.lines().collect()
    }
}

/// Parse `text` and lift every top-level node into a raw item. Nodes the
/// kind table does not know become anonymous items, so changes to them are
/// still reported instead of vanishing from the view.
fn extract_items(spec: &LangSpec, text: &str) -> Result<Vec<RawItem>, ContentError> {
    let mut parser = tree_sitter::Parser::new();
    let language: tree_sitter::Language = (spec.language)();
    parser.set_language(&language).map_err(|_| ContentError::Unsupported)?;
    // The input is already capped at `MAX_FILE_BYTES`; tree-sitter's binding
    // no longer exposes a parse timeout, and the byte and line budgets are
    // what bound the work.
    let tree = parser.parse(text, None).ok_or(ContentError::ParseFailed)?;
    let root = tree.root_node();
    if root.kind() != spec.root || root.has_error() {
        return Err(ContentError::ParseFailed);
    }

    let mut items = Vec::new();
    let mut cursor = root.walk();
    for child in root.named_children(&mut cursor) {
        for node in item_nodes(spec, child) {
            items.push(raw_item(spec, text, node));
        }
    }
    Ok(items)
}

/// The item nodes one top-level node contributes. Most languages contribute
/// the node itself; Go contributes one item per spec inside a grouped
/// declaration, and wrappers (export statements, decorators) are unwrapped
/// before classification.
fn item_nodes<'tree>(
    spec: &LangSpec,
    node: tree_sitter::Node<'tree>,
) -> Vec<tree_sitter::Node<'tree>> {
    if node.kind() == "document" && spec.name == "yaml" {
        return languages::yaml_document_pairs(node);
    }
    // JSON wraps its top-level keys in one `object`; the keys, not the
    // wrapper, are the reviewable items.
    if spec.name == "json" && node.kind() == "object" {
        let mut cursor = node.walk();
        return node.named_children(&mut cursor).collect();
    }
    if (spec.unwrap)(node.kind()) {
        let mut cursor = node.walk();
        let inner: Vec<_> = node
            .named_children(&mut cursor)
            .into_iter()
            .filter(|n| n.kind() != "comment")
            .collect();
        let mut out = Vec::new();
        for n in inner {
            out.extend(item_nodes(spec, n));
        }
        if out.is_empty() {
            out.push(node);
        }
        return out;
    }
    if spec.name == "go"
        && matches!(
            node.kind(),
            "type_declaration" | "const_declaration" | "var_declaration"
        )
    {
        let mut cursor = node.walk();
        let specs: Vec<_> = node
            .named_children(&mut cursor)
            .into_iter()
            .filter(|n| n.kind().ends_with("_spec"))
            .collect();
        if !specs.is_empty() {
            return specs;
        }
    }
    vec![node]
}

fn raw_item(spec: &LangSpec, text: &str, node: tree_sitter::Node) -> RawItem {
    let start_byte = node.start_byte();
    let end_byte = node.end_byte();
    let slice = &text[start_byte..end_byte];
    let kind: Option<&KindSpec> = languages::kind_spec(spec, node.kind());
    let (label, name) = match kind {
        Some(k) => (k.label, read_name(k, text, node)),
        None => ("item", None),
    };
    RawItem {
        label,
        name,
        start_byte,
        end_byte,
        norm: normalize(slice),
        text: slice.to_string(),
    }
}

fn read_name(kind: &KindSpec, text: &str, node: tree_sitter::Node) -> Option<String> {
    match &kind.name {
        NameRule::Field(fields) => {
            for field in *fields {
                if let Some(n) = node.child_by_field_name(field) {
                    let name = text[n.start_byte()..n.end_byte()].trim().to_string();
                    if !name.is_empty() {
                        return Some(name);
                    }
                }
            }
            None
        }
        NameRule::Header => {
            let body_start = node
                .child_by_field_name("body")
                .map(|b| b.start_byte())
                .unwrap_or(node.end_byte());
            let header = text[node.start_byte()..body_start].trim();
            (!header.is_empty()).then(|| header.to_string())
        }
        NameRule::Text => None,
    }
}

/// Whitespace-insensitive token stream, for formatting-only detection and
/// fuzzy similarity. Comments and string contents survive as tokens, so a
/// change to either is never mistaken for a formatting change.
fn normalize(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Token multiset similarity in `[0, 1]`, cheap and formatting-robust.
fn similarity(a: &str, b: &str) -> f64 {
    let mut counts: HashMap<&str, i64> = HashMap::new();
    for tok in a.split_whitespace() {
        *counts.entry(tok).or_default() += 1;
    }
    let mut common = 0i64;
    let mut total_b = 0i64;
    for tok in b.split_whitespace() {
        total_b += 1;
        if let Some(c) = counts.get_mut(tok) {
            if *c > 0 {
                *c -= 1;
                common += 1;
            }
        }
    }
    let total_a = a.split_whitespace().count() as i64;
    let denom = total_a + total_b;
    if denom == 0 {
        return 1.0;
    }
    2.0 * common as f64 / denom as f64
}

/// Line diff of one block pair, as git-ordered edits: removals before their
/// additions, context between.
fn inner_edits(old_text: &str, new_text: &str, old_base: u32, new_base: u32) -> Vec<Edit> {
    let old_lines: Vec<&str> = old_text.lines().collect();
    let new_lines: Vec<&str> = new_text.lines().collect();
    let ops = similar::capture_diff_slices(similar::Algorithm::Myers, &old_lines, &new_lines);
    let mut edits = Vec::new();
    let mut old_no = old_base;
    let mut new_no = new_base;
    for op in ops {
        use similar::DiffOp as Op;
        match op {
            Op::Equal { len, .. } => {
                for _ in 0..len {
                    edits.push(Edit { old: Some(old_no), new: Some(new_no), kind: EditKind::Context });
                    old_no += 1;
                    new_no += 1;
                }
            }
            Op::Delete { old_len, .. } => {
                for _ in 0..old_len {
                    edits.push(Edit { old: Some(old_no), new: None, kind: EditKind::Removed });
                    old_no += 1;
                }
            }
            Op::Insert { new_len, .. } => {
                for _ in 0..new_len {
                    edits.push(Edit { old: None, new: Some(new_no), kind: EditKind::Added });
                    new_no += 1;
                }
            }
            Op::Replace { old_len, new_len, .. } => {
                for _ in 0..old_len {
                    edits.push(Edit { old: Some(old_no), new: None, kind: EditKind::Removed });
                    old_no += 1;
                }
                for _ in 0..new_len {
                    edits.push(Edit { old: None, new: Some(new_no), kind: EditKind::Added });
                    new_no += 1;
                }
            }
        }
    }
    edits
}

/// A matched pair's display kind: a block whose first line is where it was
/// is a modification (it may have grown or shrunk); one whose first line
/// moved is a move, edited or not.
fn pair_kind(old_span: LineSpan, new_span: LineSpan) -> BlockKind {
    match old_span.start == new_span.start {
        true => BlockKind::Modified,
        false => BlockKind::MovedEdited,
    }
}

/// An item's identity: what a reviewer would call "the same thing". Named
/// items answer to label + name; anonymous ones only to their exact text.
fn identity(item: &RawItem) -> (&'static str, Option<&str>) {
    match &item.name {
        Some(name) => (item.label, Some(name.as_str())),
        None => (item.label, Some(item.text.as_str())),
    }
}

/// How many untaken items on one side carry each identity.
fn identity_counts<'a>(items: &'a [RawItem], taken: &[bool]) -> HashMap<(&'static str, Option<&'a str>), usize> {
    let mut counts: HashMap<(&'static str, Option<&'a str>), usize> = HashMap::new();
    for (ix, item) in items.iter().enumerate() {
        if !taken[ix] {
            *counts.entry(identity(item)).or_default() += 1;
        }
    }
    counts
}

/// Merge the two sides' items into the display sequence.
///
/// Matching is conservative by construction:
///
/// 1. identical text at the identical span is context, whatever duplicates
///    exist;
/// 2. identical text at a different span pairs as a move only when the
///    identity occurs exactly once on each side;
/// 3. otherwise a pair needs the same identity, exactly once on each side,
///    and token similarity at or above [`SIMILARITY_THRESHOLD`];
/// 4. everything else is an addition or a deletion.
///
/// The uniqueness rule is what keeps duplicated or overloaded names from
/// being paired by guesswork: with more than one candidate per side there is
/// no reliable correspondence, and the view says so by leaving both sides
/// as additions and deletions.
fn assemble(
    old_items: Vec<RawItem>,
    new_items: Vec<RawItem>,
    old_index: &LineIndex,
    new_index: &LineIndex,
) -> StructuralDiff {
    let mut old_taken = vec![false; old_items.len()];
    let mut new_taken = vec![false; new_items.len()];
    let mut pairs: Vec<(usize, usize, BlockKind)> = Vec::new();

    // Pass 0 — unchanged in place, duplicates included: identical text at
    // the identical span cannot mean anything else.
    let mut new_by_text: HashMap<&str, Vec<usize>> = HashMap::new();
    for (ix, item) in new_items.iter().enumerate() {
        new_by_text.entry(item.text.as_str()).or_default().push(ix);
    }
    for (old_ix, old) in old_items.iter().enumerate() {
        let old_span = old.span(old_index);
        let Some(candidates) = new_by_text.get(old.text.as_str()) else {
            continue;
        };
        let hit = candidates.iter().copied().find(|&n| {
            !new_taken[n] && new_items[n].span(new_index) == old_span
        });
        if let Some(new_ix) = hit {
            old_taken[old_ix] = true;
            new_taken[new_ix] = true;
        }
    }

    let old_counts = identity_counts(&old_items, &old_taken);
    let new_counts = identity_counts(&new_items, &new_taken);
    let unique = |counts: &HashMap<(&'static str, Option<&str>), usize>, item: &RawItem| {
        counts.get(&identity(item)).copied() == Some(1)
    };

    // Pass 1 — exact-content moves.
    for (old_ix, old) in old_items.iter().enumerate() {
        if old_taken[old_ix] {
            continue;
        }
        let Some(candidates) = new_by_text.get(old.text.as_str()) else {
            continue;
        };
        let Some(&new_ix) = candidates.iter().find(|&&n| !new_taken[n]) else {
            continue;
        };
        if !unique(&old_counts, old) || !unique(&new_counts, &new_items[new_ix]) {
            continue;
        }
        old_taken[old_ix] = true;
        new_taken[new_ix] = true;
        pairs.push((old_ix, new_ix, BlockKind::Moved));
    }

    // Pass 2 — same identity, unique on both sides, similar enough.
    for (old_ix, old) in old_items.iter().enumerate() {
        if old_taken[old_ix] {
            continue;
        }
        if !unique(&old_counts, old) {
            continue;
        }
        let Some(candidate) = new_items.iter().enumerate().find(|(new_ix, new)| {
            !new_taken[*new_ix]
                && unique(&new_counts, new)
                && identity(new) == identity(old)
                && new.text.lines().count() <= MAX_FUZZY_BLOCK_LINES
                && old.text.lines().count() <= MAX_FUZZY_BLOCK_LINES
                && similarity(&old.norm, &new.norm) >= SIMILARITY_THRESHOLD
        }) else {
            continue;
        };
        let (new_ix, _) = candidate;
        old_taken[old_ix] = true;
        new_taken[new_ix] = true;
        let old_span = old.span(old_index);
        let new_span = new_items[new_ix].span(new_index);
        pairs.push((old_ix, new_ix, pair_kind(old_span, new_span)));
    }

    // Build blocks. Paired blocks know their inner edits; lone items become
    // additions/deletions with their full text as the edit list.
    let old_line_count = old_index.line_count();
    let new_line_count = new_index.line_count();
    let mut blocks: Vec<(SortKey, Block)> = Vec::new();
    let mut anchors: Vec<(SortKey, Item)> = Vec::new();

    for &(old_ix, new_ix, kind) in &pairs {
        let old_item = &old_items[old_ix];
        let new_item = &new_items[new_ix];
        let old_span = old_item.span(old_index);
        let new_span = new_item.span(new_index);

        let formatting_only = old_item.norm == new_item.norm;
        // A block with nothing but formatting to show is a plain move: there
        // are no edits inside it, so `MovedEdited` would promise rows that do
        // not exist. The formatting flag still drives the collapsed marker.
        let kind = match (kind, formatting_only) {
            (BlockKind::MovedEdited, true) => BlockKind::Moved,
            (kind, _) => kind,
        };
        let edits = match kind {
            BlockKind::Moved => Vec::new(),
            _ if formatting_only => Vec::new(),
            _ => inner_edits(
                &old_item.text,
                &new_item.text,
                old_span.start,
                new_span.start,
            ),
        };
        let block = Block {
            kind,
            label: new_item.label,
            name: new_item.name.clone().or_else(|| old_item.name.clone()),
            old: Some(old_span),
            new: Some(new_span),
            edits,
            formatting_only,
        };
        // A block that left its first line gets a source anchor at the old
        // position; the block itself sits at its new position. In-place
        // modifications (same first line) have no anchor.
        if old_span.start != new_span.start {
            anchors.push((
                SortKey::at(old_span.start),
                Item::Source {
                    label: block.label,
                    name: block.name.clone(),
                    old: old_span,
                    new_start: new_span.start,
                },
            ));
        }
        blocks.push((SortKey::at(new_span.start), block));
    }

    for (ix, item) in new_items.iter().enumerate() {
        if new_taken[ix] {
            continue;
        }
        let span = item.span(new_index);
        let edits = item
            .lines()
            .into_iter()
            .enumerate()
            .map(|(row, _)| Edit {
                old: None,
                new: Some(span.start + row as u32),
                kind: EditKind::Added,
            })
            .collect();
        blocks.push((
            SortKey::at(span.start),
            Block {
                kind: BlockKind::Added,
                label: item.label,
                name: item.name.clone(),
                old: None,
                new: Some(span),
                edits,
                formatting_only: false,
            },
        ));
    }
    for (ix, item) in old_items.iter().enumerate() {
        if old_taken[ix] {
            continue;
        }
        let span = item.span(old_index);
        let edits = item
            .lines()
            .into_iter()
            .enumerate()
            .map(|(row, _)| Edit {
                old: Some(span.start + row as u32),
                new: None,
                kind: EditKind::Removed,
            })
            .collect();
        // Priority 1: a deletion ties with an anchor (0) and yields to the
        // new-side blocks (2) at the same line.
        blocks.push((
            SortKey::at(span.start),
            Block {
                kind: BlockKind::Deleted,
                label: item.label,
                name: item.name.clone(),
                old: Some(span),
                new: None,
                edits,
                formatting_only: false,
            },
        ));
    }

    // Merge into one display sequence. New-position blocks order by new
    // start; deleted blocks and anchors order by old start. A tie (an old
    // position that is also a new position) puts the anchor first, then the
    // deletion, then new-side blocks — a reviewer meets "what left" before
    // "what arrived".
    let mut entries: Vec<(SortKey, usize, Item)> = Vec::new();
    for (key, block) in blocks {
        let priority = match block.kind {
            BlockKind::Deleted => 1,
            _ => 2,
        };
        entries.push((key, priority, Item::Block(block)));
    }
    for (key, anchor) in anchors {
        entries.push((key, 0, anchor));
    }
    entries.sort_by(|a, b| (a.0.line, a.1).cmp(&(b.0.line, b.1)));

    StructuralDiff {
        items: entries.into_iter().map(|(_, _, item)| item).collect(),
        old_lines: old_line_count,
        new_lines: new_line_count,
    }
}

/// The display-order key: the line of whichever version an entry sorts by.
/// Old-file and new-file line numbers are merged on purpose — deleted blocks
/// and anchors have no new position, and their old position relative to the
/// surrounding blocks' positions is the only ordering information there is.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
struct SortKey {
    line: u32,
}

impl SortKey {
    fn at(line: u32) -> Self {
        SortKey { line }
    }
}

