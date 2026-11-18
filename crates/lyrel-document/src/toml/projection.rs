//! TOML 的 GUI-free 无损结构投影。
//!
//! 源码是唯一权威。这里用正式 registry 的无损 `toml_edit` 解析当前 LF
//! 源码，只把结构、键/值原始范围和显示文本复制进一份不可变投影；不转
//! JSON、不保存第二份可写文档，也不把整篇 `DocumentMut` 序列化回写。
//!
//! 关键约束：
//!
//! - 原区间层必须保留 `Document::parse` 的结果；`DocumentMut` 的
//!   `into_mut()` 会无条件 `despan()`，不能用于定位原源码。
//! - `Table::span()` 只覆盖该表自己的 `[header]`，不是表体；`ArrayOfTables`
//!   的 span 从第一个 header 到最后一个 header，可能跨过无关表。因此删除
//!   规划只使用每个 header 自己的行、该表自己的直接 key/value 行和显式子表
//!   的 header/value 行，绝不把容器 span 当连续删除区间。
//! - ordinary table、inline table、array of tables、dotted/quoted key 与
//!   date/time 等标量全部保留原拼写和形态；显示文本直接取源码切片。

use std::ops::Range;

use toml_edit::{ArrayOfTables, Document, InlineTable, Item, Key, Table, TableLike, Value};

use crate::state::CancelToken;
use crate::tree::{Treelike, line_column, truncated_chars};

/// 结构投影能够读取的最大源码字节数。更大的文件仍可作为源码打开与保存，
/// 只是不生成结构树。
pub const STRUCTURE_INPUT_BYTES: usize = 1024 * 1024;
/// 一份投影最多保留的节点数；超限时整份结构视图报预算错误，而不是静默
/// 截断树。
pub const STRUCTURE_NODES: usize = 20_000;
/// 所有节点路径字符串合计；长祖先键不能按子节点数量无界放大内存。
pub const STRUCTURE_PATH_BYTES: usize = 8 * 1024 * 1024;

/// 行内预览最多显示的字符数；完整原文仍可从节点范围读取。
const DISPLAY_CHARS: usize = 160;
/// 每处理这么多节点检查一次取消。
const CANCEL_INTERVAL: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TomlValueKind {
    String,
    Integer,
    Float,
    Boolean,
    Datetime,
    Array,
    InlineTable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TomlNodeKind {
    Root,
    Table,
    ArrayOfTables,
    ArrayOfTablesEntry,
    Value(TomlValueKind),
}

impl TomlNodeKind {
    pub fn is_value(self) -> bool {
        matches!(self, Self::Value(_))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeaderSegment {
    pub raw: Range<usize>,
    pub key: String,
}

#[derive(Clone, Debug)]
pub struct TomlHeader {
    /// `[a.child]` 或 `[[jobs]]`，不含行尾注释。
    pub full: Range<usize>,
    /// header 所在物理行（含行尾注释与换行）。
    pub line: Range<usize>,
    pub segments: Vec<HeaderSegment>,
}

/// 一个可显示的结构节点。`range` 是值本身或 header；`detail` 是选中时
/// 在源码中定位的完整键/值或 header；`statement` 是该 key/value 占用的
/// 完整物理行范围（inline table 成员为 `None`）。
#[derive(Clone, Debug)]
pub struct TomlNode {
    pub key: String,
    /// 解码后的键名（仅用于重命名预填）；显示仍用 `key`。
    pub key_text: Option<String>,
    pub path: String,
    pub value: String,
    pub kind: TomlNodeKind,
    pub range: Range<usize>,
    pub detail: Range<usize>,
    pub key_range: Option<Range<usize>>,
    pub statement: Option<Range<usize>>,
    pub header: Option<TomlHeader>,
    /// 该节点位于某个 inline table 内。
    pub inline_context: bool,
    /// 该值直接属于 enclosing inline table（不是其 dotted 子表）。
    pub inline_direct: bool,
    pub inline_parent: Option<usize>,
    /// dotted key 产生的隐式表；没有自己的 header。
    pub table_is_dotted: bool,
    /// 表或 AoT entry 有真实 header。
    pub explicit_table: bool,
    pub key_path: Vec<String>,
    /// 该 key/value 的物理行归属的显式表/entry/root。
    pub section_owner: Option<usize>,
    /// 向该表追加直接 key/value 的位置（行尾之后）。
    pub own_insert: Option<usize>,
    /// 该表/AoT 子树最后一行之后的位置。
    pub subtree_end: Option<usize>,
    pub sort_start: usize,
    pub children: Vec<usize>,
}

impl TomlNode {
    pub fn has_source_detail(&self) -> bool {
        !self.detail.is_empty()
    }

    pub fn is_inline_table(&self) -> bool {
        self.kind == TomlNodeKind::Value(TomlValueKind::InlineTable)
    }
}

#[derive(Clone, Debug)]
pub struct TomlParseFailure {
    pub line: usize,
    pub column: usize,
    pub message: String,
}

/// 已解析的一份源码快照；不持有源码本身，只引用其中的字节范围。
#[derive(Clone, Debug)]
pub struct TomlProjection {
    nodes: Vec<TomlNode>,
    root: Option<usize>,
    error: Option<TomlParseFailure>,
}

impl TomlProjection {
    pub fn nodes(&self) -> &[TomlNode] {
        &self.nodes
    }

    pub fn root(&self) -> Option<usize> {
        self.root
    }

    pub fn error(&self) -> Option<&TomlParseFailure> {
        self.error.as_ref()
    }

    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }
}

impl Treelike for TomlProjection {
    fn root(&self) -> Option<usize> {
        self.root
    }

    fn children(&self, node: usize) -> &[usize] {
        &self.nodes[node].children
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProjectionFailure {
    Cancelled,
    InputTooLarge,
    TooManyNodes,
    PathsTooLarge,
}

/// Parse one snapshot into a bounded, source-linked projection.
pub fn project(text: &str, cancel: &CancelToken) -> Result<TomlProjection, ProjectionFailure> {
    if cancel.is_cancelled() {
        return Err(ProjectionFailure::Cancelled);
    }
    if text.len() > STRUCTURE_INPUT_BYTES {
        return Err(ProjectionFailure::InputTooLarge);
    }
    let parsed = Document::parse(text);
    match parsed {
        Ok(document) => {
            let mut builder = Builder::new(text, cancel, true);
            let root = builder.push(TomlNode {
                key: "$".to_owned(),
                key_text: None,
                path: "$".to_owned(),
                value: "TOML document".to_owned(),
                kind: TomlNodeKind::Root,
                range: 0..0,
                detail: 0..0,
                key_range: None,
                statement: None,
                header: None,
                inline_context: false,
                inline_direct: false,
                inline_parent: None,
                table_is_dotted: false,
                explicit_table: false,
                key_path: Vec::new(),
                section_owner: Some(0),
                own_insert: None,
                subtree_end: None,
                sort_start: 0,
                children: Vec::new(),
            })?;
            builder.walk_items(document.as_table(), root, "$", &[], root, None, None)?;
            let mut projection = TomlProjection {
                nodes: builder.nodes,
                root: Some(root),
                error: None,
            };
            compute_layout(&mut projection, text);
            Ok(projection)
        }
        Err(error) => {
            let (line, column) = error
                .span()
                .map(|span| line_column(text, span.start))
                .unwrap_or((1, 1));
            Ok(TomlProjection {
                nodes: Vec::new(),
                root: None,
                error: Some(TomlParseFailure {
                    line,
                    column,
                    message: error.message().to_owned(),
                }),
            })
        }
    }
}

/// 供 mutation 规划使用的无预算投影；解析失败也返回错误文本。
pub fn project_for_mutation(text: &str) -> Result<TomlProjection, (usize, usize, String)> {
    let document = Document::parse(text).map_err(|error| {
        let (line, column) = error
            .span()
            .map(|span| line_column(text, span.start))
            .unwrap_or((1, 1));
        (line, column, error.message().to_owned())
    })?;
    let cancel = CancelToken::new();
    let mut builder = Builder::new(text, &cancel, false);
    let root = builder
        .push(TomlNode {
            key: "$".to_owned(),
            key_text: None,
            path: "$".to_owned(),
            value: "TOML document".to_owned(),
            kind: TomlNodeKind::Root,
            range: 0..0,
            detail: 0..0,
            key_range: None,
            statement: None,
            header: None,
            inline_context: false,
            inline_direct: false,
            inline_parent: None,
            table_is_dotted: false,
            explicit_table: false,
            key_path: Vec::new(),
            section_owner: Some(0),
            own_insert: None,
            subtree_end: None,
            sort_start: 0,
            children: Vec::new(),
        })
        .expect("mutation projection has no node budget");
    builder
        .walk_items(document.as_table(), root, "$", &[], root, None, None)
        .expect("mutation projection has no node budget");
    let mut projection = TomlProjection {
        nodes: builder.nodes,
        root: Some(root),
        error: None,
    };
    compute_layout(&mut projection, text);
    Ok(projection)
}

struct Builder<'a> {
    text: &'a str,
    cancel: &'a CancelToken,
    limited: bool,
    nodes: Vec<TomlNode>,
    path_bytes: usize,
}

#[derive(Clone, Copy)]
struct InlineContext {
    node: usize,
    direct_parent: usize,
}

impl<'a> Builder<'a> {
    fn new(text: &'a str, cancel: &'a CancelToken, limited: bool) -> Self {
        Self {
            text,
            cancel,
            limited,
            nodes: Vec::new(),
            path_bytes: 0,
        }
    }

    fn push(&mut self, node: TomlNode) -> Result<usize, ProjectionFailure> {
        if self.limited {
            if self.nodes.len() >= STRUCTURE_NODES {
                return Err(ProjectionFailure::TooManyNodes);
            }
            if self.nodes.len() % CANCEL_INTERVAL == 0 && self.cancel.is_cancelled() {
                return Err(ProjectionFailure::Cancelled);
            }
            self.path_bytes += node.path.len();
            if self.path_bytes > STRUCTURE_PATH_BYTES {
                return Err(ProjectionFailure::PathsTooLarge);
            }
        }
        self.nodes.push(node);
        Ok(self.nodes.len() - 1)
    }

    fn attach(&mut self, parent: usize, child: usize) {
        self.nodes[parent].children.push(child);
    }

    fn walk_items(
        &mut self,
        table: &dyn TableLike,
        parent: usize,
        parent_path: &str,
        key_path: &[String],
        section_owner: usize,
        dotted_start: Option<usize>,
        inline: Option<InlineContext>,
    ) -> Result<(), ProjectionFailure> {
        let names: Vec<String> = table.iter().map(|(name, _)| name.to_owned()).collect();
        for name in names {
            let Some((key, item)) = table.get_key_value(&name) else {
                continue;
            };
            let mut child_key_path = key_path.to_vec();
            child_key_path.push(name.clone());
            match item {
                Item::Value(value) => self.add_value(
                    key,
                    &name,
                    value,
                    parent,
                    parent_path,
                    &child_key_path,
                    section_owner,
                    dotted_start,
                    inline,
                )?,
                Item::Table(child) => self.add_table(
                    key,
                    &name,
                    child,
                    parent,
                    parent_path,
                    &child_key_path,
                    section_owner,
                    dotted_start,
                    inline,
                )?,
                Item::ArrayOfTables(array) => self.add_array_of_tables(
                    key,
                    &name,
                    array,
                    parent,
                    parent_path,
                    &child_key_path,
                    section_owner,
                    inline,
                )?,
                Item::None => {}
            }
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn add_table(
        &mut self,
        key: &Key,
        name: &str,
        table: &Table,
        parent: usize,
        parent_path: &str,
        key_path: &[String],
        section_owner: usize,
        dotted_start: Option<usize>,
        inline: Option<InlineContext>,
    ) -> Result<(), ProjectionFailure> {
        let explicit = table.position().is_some();
        let header = if explicit {
            table.span().and_then(|span| parse_header(self.text, span))
        } else {
            None
        };
        let key_span = header
            .as_ref()
            .and_then(|header| header.segments.last().map(|segment| segment.raw.clone()))
            .or_else(|| key.span());
        let key_text = header
            .as_ref()
            .and_then(|header| header.segments.last().map(|segment| segment.key.clone()))
            .or_else(|| Some(name.to_owned()));
        let range = header
            .as_ref()
            .map(|header| header.full.clone())
            .or_else(|| table.span())
            .or_else(|| key.span())
            .unwrap_or(0..0);
        let line = header
            .as_ref()
            .map(|header| header.line.clone())
            .unwrap_or_else(|| line_start(self.text, range.start)..line_end(self.text, range.end));
        let sort_start = line.start;
        let path = append_path(parent_path, name);
        let label = header
            .as_ref()
            .and_then(|header| self.text.get(header.full.clone()))
            .unwrap_or(name)
            .to_owned();
        let dotted = table.is_dotted();
        let value = if dotted {
            "dotted key".to_owned()
        } else if explicit {
            "table".to_owned()
        } else {
            "implicit table".to_owned()
        };
        let is_inline_context = inline.is_some() && header.is_none();
        let node = TomlNode {
            key: label,
            key_text,
            path: path.clone(),
            value,
            kind: TomlNodeKind::Table,
            range: range.clone(),
            detail: header
                .as_ref()
                .map(|header| header.full.clone())
                .unwrap_or_else(|| range.clone()),
            key_range: key_span,
            statement: None,
            header,
            inline_context: is_inline_context,
            inline_direct: inline.is_some_and(|context| context.direct_parent == parent),
            inline_parent: inline.map(|context| context.node),
            table_is_dotted: dotted,
            explicit_table: explicit,
            key_path: key_path.to_vec(),
            section_owner: Some(section_owner),
            own_insert: None,
            subtree_end: None,
            sort_start,
            children: Vec::new(),
        };
        let id = self.push(node)?;
        self.attach(parent, id);
        let child_owner = if explicit { id } else { section_owner };
        let next_dotted = if dotted && !explicit {
            Some(dotted_start.map_or(range.start, |start| start.min(range.start)))
        } else {
            None
        };
        self.walk_items(table, id, &path, key_path, child_owner, next_dotted, inline)
    }

    #[allow(clippy::too_many_arguments)]
    fn add_array_of_tables(
        &mut self,
        key: &Key,
        name: &str,
        array: &ArrayOfTables,
        parent: usize,
        parent_path: &str,
        key_path: &[String],
        _section_owner: usize,
        _inline: Option<InlineContext>,
    ) -> Result<(), ProjectionFailure> {
        let first_header = array
            .iter()
            .next()
            .and_then(|entry| entry.span())
            .and_then(|span| parse_header(self.text, span));
        let range = first_header
            .as_ref()
            .map(|header| header.full.clone())
            .or_else(|| array.span())
            .or_else(|| key.span())
            .unwrap_or(0..0);
        let line = first_header
            .as_ref()
            .map(|header| header.line.clone())
            .unwrap_or_else(|| line_start(self.text, range.start)..line_end(self.text, range.end));
        let key_range = first_header
            .as_ref()
            .and_then(|header| header.segments.last().map(|segment| segment.raw.clone()))
            .or_else(|| key.span());
        let key_text = first_header
            .as_ref()
            .and_then(|header| header.segments.last().map(|segment| segment.key.clone()))
            .or_else(|| Some(name.to_owned()));
        let path = append_path(parent_path, name);
        let label = first_header
            .as_ref()
            .and_then(|header| self.text.get(header.full.clone()))
            .unwrap_or(name)
            .to_owned();
        let node = TomlNode {
            key: label,
            key_text,
            path: path.clone(),
            value: format!("{} entries", array.len()),
            kind: TomlNodeKind::ArrayOfTables,
            range: range.clone(),
            detail: range.clone(),
            key_range,
            statement: None,
            header: None,
            inline_context: false,
            inline_direct: false,
            inline_parent: None,
            table_is_dotted: false,
            explicit_table: false,
            key_path: key_path.to_vec(),
            section_owner: None,
            own_insert: None,
            subtree_end: None,
            sort_start: line.start,
            children: Vec::new(),
        };
        let id = self.push(node)?;
        self.attach(parent, id);
        for (index, entry) in array.iter().enumerate() {
            self.add_array_entry(entry, id, &path, key_path, index)?;
        }
        Ok(())
    }

    fn add_array_entry(
        &mut self,
        entry: &Table,
        parent: usize,
        parent_path: &str,
        key_path: &[String],
        index: usize,
    ) -> Result<(), ProjectionFailure> {
        let header = entry.span().and_then(|span| parse_header(self.text, span));
        let range = header
            .as_ref()
            .map(|header| header.full.clone())
            .or_else(|| entry.span())
            .unwrap_or(0..0);
        let line = header
            .as_ref()
            .map(|header| header.line.clone())
            .unwrap_or_else(|| line_start(self.text, range.start)..line_end(self.text, range.end));
        let key_range = header
            .as_ref()
            .and_then(|header| header.segments.last().map(|segment| segment.raw.clone()));
        let key_text = header
            .as_ref()
            .and_then(|header| header.segments.last().map(|segment| segment.key.clone()));
        let label = header
            .as_ref()
            .and_then(|header| self.text.get(header.full.clone()))
            .unwrap_or("[[entry]]")
            .to_owned();
        let path = format!("{parent_path}[{index}]");
        let node = TomlNode {
            key: label,
            key_text,
            path: path.clone(),
            value: format!("entry {}", index + 1),
            kind: TomlNodeKind::ArrayOfTablesEntry,
            range: range.clone(),
            detail: range.clone(),
            key_range,
            statement: None,
            header,
            inline_context: false,
            inline_direct: false,
            inline_parent: None,
            table_is_dotted: false,
            explicit_table: true,
            key_path: key_path.to_vec(),
            section_owner: None,
            own_insert: None,
            subtree_end: None,
            sort_start: line.start,
            children: Vec::new(),
        };
        let id = self.push(node)?;
        self.attach(parent, id);
        self.walk_items(entry, id, &path, key_path, id, None, None)
    }

    #[allow(clippy::too_many_arguments)]
    fn add_value(
        &mut self,
        key: &Key,
        name: &str,
        value: &Value,
        parent: usize,
        parent_path: &str,
        key_path: &[String],
        section_owner: usize,
        dotted_start: Option<usize>,
        inline: Option<InlineContext>,
    ) -> Result<(), ProjectionFailure> {
        let range = value.span().or_else(|| key.span()).unwrap_or(0..0);
        let key_range = key.span();
        let kind = value_kind(value);
        let is_inline = inline.is_some();
        let statement = if is_inline {
            None
        } else {
            let start = dotted_start
                .or_else(|| key_range.as_ref().map(|range| range.start))
                .unwrap_or(range.start);
            let suffix_end = value
                .decor()
                .suffix()
                .and_then(|suffix| suffix.span())
                .map(|span| span.end)
                .unwrap_or(range.end);
            Some(line_start(self.text, start)..line_end(self.text, range.end.max(suffix_end)))
        };
        let (detail_start, detail_end) = if is_inline {
            (
                key_range
                    .as_ref()
                    .map(|range| range.start)
                    .unwrap_or(range.start),
                range.end,
            )
        } else {
            let start = statement
                .as_ref()
                .map(|range| range.start)
                .unwrap_or(range.start);
            let end = statement
                .as_ref()
                .map(|range| range.end)
                .unwrap_or(range.end);
            let end = if end > start && self.text.as_bytes().get(end - 1) == Some(&b'\n') {
                end - 1
            } else {
                end
            };
            (start, end)
        };
        let path = append_path(parent_path, name);
        let key_text = Some(key.get().to_owned());
        let display = match kind {
            TomlValueKind::InlineTable => {
                format!("{{ {} keys }}", inline_key_count(value))
            }
            _ => truncated_chars(self.text.get(range.clone()).unwrap_or(""), DISPLAY_CHARS),
        };
        let node = TomlNode {
            key: self
                .text
                .get(key_range.clone().unwrap_or(0..0))
                .unwrap_or(name)
                .to_owned(),
            key_text,
            path: path.clone(),
            value: display,
            kind: TomlNodeKind::Value(kind),
            range: range.clone(),
            detail: detail_start..detail_end,
            key_range: key_range.clone(),
            statement: statement.clone(),
            header: None,
            inline_context: is_inline,
            inline_direct: inline.is_some_and(|context| context.direct_parent == parent),
            inline_parent: inline.map(|context| context.node),
            table_is_dotted: false,
            explicit_table: false,
            key_path: key_path.to_vec(),
            section_owner: (!is_inline).then_some(section_owner),
            own_insert: None,
            subtree_end: None,
            sort_start: statement
                .as_ref()
                .map(|range| range.start)
                .unwrap_or_else(|| {
                    key_range
                        .as_ref()
                        .map(|range| range.start)
                        .unwrap_or(range.start)
                }),
            children: Vec::new(),
        };
        let id = self.push(node)?;
        self.attach(parent, id);
        if let Value::InlineTable(table) = value {
            let context = InlineContext {
                node: id,
                direct_parent: id,
            };
            self.walk_items(table, id, &path, key_path, id, None, Some(context))?;
        }
        Ok(())
    }
}

fn value_kind(value: &Value) -> TomlValueKind {
    match value {
        Value::String(_) => TomlValueKind::String,
        Value::Integer(_) => TomlValueKind::Integer,
        Value::Float(_) => TomlValueKind::Float,
        Value::Boolean(_) => TomlValueKind::Boolean,
        Value::Datetime(_) => TomlValueKind::Datetime,
        Value::Array(_) => TomlValueKind::Array,
        Value::InlineTable(_) => TomlValueKind::InlineTable,
    }
}

fn inline_key_count(value: &Value) -> usize {
    match value {
        Value::InlineTable(table) => <InlineTable as TableLike>::iter(table).count(),
        _ => 0,
    }
}

fn compute_layout(projection: &mut TomlProjection, text: &str) {
    let count = projection.nodes.len();
    let mut statements: Vec<Vec<usize>> = vec![Vec::new(); count];
    for node in &projection.nodes {
        if let (Some(owner), Some(statement)) = (node.section_owner, node.statement.as_ref()) {
            if owner < count {
                statements[owner].push(statement.end);
            }
        }
    }
    let mut headers: Vec<usize> = projection
        .nodes
        .iter()
        .enumerate()
        .filter(|(_, node)| node.header.is_some())
        .map(|(index, _)| index)
        .collect();
    headers.sort_by_key(|index| projection.nodes[*index].header.as_ref().unwrap().line.start);
    for (position, index) in headers.iter().enumerate() {
        let next_start = headers
            .get(position + 1)
            .and_then(|next| {
                projection.nodes[*next]
                    .header
                    .as_ref()
                    .map(|header| header.line.start)
            })
            .unwrap_or(text.len());
        let header_end = projection.nodes[*index]
            .header
            .as_ref()
            .map(|header| header.line.end)
            .unwrap_or(0);
        let insert = statements[*index]
            .iter()
            .copied()
            .filter(|end| *end <= next_start)
            .fold(header_end, usize::max);
        projection.nodes[*index].own_insert = Some(insert);
    }
    if let Some(root) = projection.root {
        let first_header = headers
            .first()
            .and_then(|index| projection.nodes[*index].header.as_ref())
            .map(|header| header.line.start)
            .unwrap_or(text.len());
        let insert = statements[root]
            .iter()
            .copied()
            .filter(|end| *end <= first_header)
            .fold(0, usize::max);
        projection.nodes[root].own_insert = Some(insert);
    }
    if let Some(root) = projection.root {
        compute_subtree_end(&mut projection.nodes, root, text.len());
        sort_children(&mut projection.nodes, root);
    }
}

fn compute_subtree_end(nodes: &mut [TomlNode], index: usize, text_len: usize) -> usize {
    let children = nodes[index].children.clone();
    let mut end = match nodes[index].kind {
        TomlNodeKind::Root => text_len,
        TomlNodeKind::Table | TomlNodeKind::ArrayOfTablesEntry => {
            nodes[index].own_insert.unwrap_or(nodes[index].range.end)
        }
        TomlNodeKind::ArrayOfTables => nodes[index].range.end,
        TomlNodeKind::Value(_) => nodes[index]
            .statement
            .as_ref()
            .map(|statement| statement.end)
            .unwrap_or(nodes[index].range.end),
    };
    for child in children {
        end = end.max(compute_subtree_end(nodes, child, text_len));
    }
    nodes[index].subtree_end = Some(end);
    end
}

fn sort_children(nodes: &mut [TomlNode], index: usize) {
    let mut children = nodes[index].children.clone();
    children.sort_by_key(|child| (nodes[*child].sort_start, *child));
    for child in &children {
        sort_children(nodes, *child);
    }
    nodes[index].children = children;
}

fn append_path(parent: &str, key: &str) -> String {
    let repr = key_repr(key);
    if is_bare_key_text(&repr) {
        format!("{parent}.{repr}")
    } else {
        format!("{parent}[{repr}]")
    }
}

/// 新成员在结构中的稳定路径；与投影内部的路径拼写一致。
pub fn child_path(parent: &str, key: &str) -> String {
    append_path(parent, key)
}

fn is_bare_key_text(key: &str) -> bool {
    !key.is_empty()
        && key
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
}

/// 生成单个 TOML key 的源码拼写；需要引用时使用 basic string 转义。
pub fn key_repr(key: &str) -> String {
    Key::new(key).display_repr().into_owned()
}

/// 解码一个 header/键片段。返回 None 表示该片段不是单个合法 key。
pub fn decode_key(raw: &str) -> Option<String> {
    let mut keys = Key::parse(raw).ok()?;
    (keys.len() == 1).then(|| keys.remove(0).get().to_owned())
}

fn parse_header(text: &str, span: Range<usize>) -> Option<TomlHeader> {
    let raw = text.get(span.clone())?;
    let is_array = raw.starts_with("[[");
    let inner_start = span.start + if is_array { 2 } else { 1 };
    let inner_end = span.end.checked_sub(if is_array { 2 } else { 1 })?;
    if inner_start > inner_end {
        return None;
    }
    let mut segments = Vec::new();
    let mut cursor = inner_start;
    loop {
        while cursor < inner_end
            && text
                .get(cursor..)?
                .chars()
                .next()
                .is_some_and(|ch| ch == ' ' || ch == '\t')
        {
            cursor += 1;
        }
        if cursor >= inner_end {
            break;
        }
        let start = cursor;
        let bytes = text.as_bytes();
        match bytes.get(cursor).copied()? {
            b'"' => {
                cursor += 1;
                let mut escaped = false;
                while cursor < inner_end {
                    let byte = bytes[cursor];
                    cursor += 1;
                    if escaped {
                        escaped = false;
                    } else if byte == b'\\' {
                        escaped = true;
                    } else if byte == b'"' {
                        break;
                    }
                }
            }
            b'\'' => {
                cursor += 1;
                while cursor < inner_end {
                    let byte = bytes[cursor];
                    cursor += 1;
                    if byte == b'\'' {
                        break;
                    }
                }
            }
            _ => {
                while cursor < inner_end {
                    let byte = bytes[cursor];
                    if byte == b'.' || byte == b' ' || byte == b'\t' {
                        break;
                    }
                    cursor += 1;
                }
            }
        }
        if cursor <= start {
            return None;
        }
        let raw_segment = text.get(start..cursor)?.to_owned();
        let key = decode_key(&raw_segment)?;
        segments.push(HeaderSegment {
            raw: start..cursor,
            key,
        });
        while cursor < inner_end
            && text
                .get(cursor..)?
                .chars()
                .next()
                .is_some_and(|ch| ch == ' ' || ch == '\t')
        {
            cursor += 1;
        }
        if cursor >= inner_end {
            break;
        }
        if bytes.get(cursor).copied()? != b'.' {
            return None;
        }
        cursor += 1;
    }
    if segments.is_empty() {
        return None;
    }
    Some(TomlHeader {
        full: span.clone(),
        line: line_start(text, span.start)..line_end(text, span.end),
        segments,
    })
}

/// 行首字节偏移（`\n` 之后）。
pub fn line_start(text: &str, at: usize) -> usize {
    let at = at.min(text.len());
    text[..at].rfind('\n').map_or(0, |index| index + 1)
}

/// 包含 `at` 的那一行末尾（含 `\n`）；`at` 在文件末尾时返回长度。
pub fn line_end(text: &str, at: usize) -> usize {
    let at = at.min(text.len());
    text[at..]
        .find('\n')
        .map_or(text.len(), |index| at + index + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(text: &str) -> TomlProjection {
        super::project(text, &CancelToken::new()).expect("valid projection")
    }

    fn node<'a>(projection: &'a TomlProjection, path: &str) -> &'a TomlNode {
        projection
            .nodes()
            .iter()
            .find(|node| node.path == path)
            .unwrap_or_else(|| panic!("node {path}"))
    }

    #[test]
    fn ordinary_tables_dotted_and_quoted_keys_keep_exact_source_spans() {
        let text = "# header\n\"dotted.key\" = 1979-05-27T07:32:00Z\ndotted.value = 0x10\n[a]\nvalue = +1_024 # trailing\n[a.child]\ndate = 1979-05-27\n[[jobs]]\nname = \"one\"\n[b]\nunrelated = true\n[a.late]\nkeep = \"a\"\n[[jobs]]\nname = \"two\"\n";
        let projection = project(text);
        assert_eq!(node(&projection, "$").children.len(), 5);
        let quoted = node(&projection, "$[\"dotted.key\"]");
        assert_eq!(quoted.kind, TomlNodeKind::Value(TomlValueKind::Datetime));
        assert_eq!(
            text.get(quoted.range.clone()).unwrap(),
            "1979-05-27T07:32:00Z"
        );
        let dotted = node(&projection, "$.dotted.value");
        assert_eq!(dotted.kind, TomlNodeKind::Value(TomlValueKind::Integer));
        assert_eq!(text.get(dotted.range.clone()).unwrap(), "0x10");
        assert_eq!(
            text.get(dotted.detail.clone()).unwrap(),
            "dotted.value = 0x10"
        );
        let table = node(&projection, "$.a");
        assert!(table.explicit_table);
        assert_eq!(
            text.get(table.header.as_ref().unwrap().line.clone())
                .unwrap(),
            "[a]\n"
        );
        assert_eq!(
            text.get(table.header.as_ref().unwrap().full.clone())
                .unwrap(),
            "[a]"
        );
        let child = node(&projection, "$.a.child");
        assert_eq!(
            text.get(child.header.as_ref().unwrap().full.clone())
                .unwrap(),
            "[a.child]"
        );
        assert_eq!(child.key_path, vec!["a".to_owned(), "child".to_owned()]);
        assert_eq!(node(&projection, "$.jobs").children.len(), 2);
        assert_eq!(
            text.get(node(&projection, "$.jobs[1]").range.clone())
                .unwrap(),
            "[[jobs]]"
        );
        // The AoT container range is the first header, never the bytes between
        // interleaved unrelated tables and the last entry.
        assert_eq!(
            text.get(node(&projection, "$.jobs").range.clone()).unwrap(),
            "[[jobs]]"
        );
    }

    #[test]
    fn layout_uses_own_segments_not_contiguous_aot_spans() {
        let text = "[a]\nvalue = 1\n[a.child]\nchild = 2\n[b]\nunrelated = true\n[a.late]\nlate = 3\n[[jobs]]\nname = \"one\"\n[[jobs]]\nname = \"two\"\n";
        let projection = project(text);
        let a = node(&projection, "$.a");
        assert_eq!(
            text.get(a.header.as_ref().unwrap().line.clone()).unwrap(),
            "[a]\n"
        );
        let child = node(&projection, "$.a.child");
        assert_eq!(
            text.get(child.header.as_ref().unwrap().line.clone())
                .unwrap(),
            "[a.child]\n"
        );
        let jobs = node(&projection, "$.jobs");
        assert_eq!(jobs.children.len(), 2);
        let last_entry = node(&projection, "$.jobs[1]");
        assert_eq!(
            text.get(last_entry.header.as_ref().unwrap().line.clone())
                .unwrap(),
            "[[jobs]]\n"
        );
        assert_eq!(jobs.subtree_end, Some(text.len()));
    }

    #[test]
    fn inline_tables_expose_members_without_losing_inline_form() {
        let text = "inline = { x = 1, \"y.z\" = 2, when = 1979-05-27 }\n";
        let projection = project(text);
        let inline = node(&projection, "$.inline");
        assert!(inline.is_inline_table());
        assert_eq!(inline.children.len(), 3);
        let quoted = node(&projection, "$.inline[\"y.z\"]");
        assert_eq!(quoted.kind, TomlNodeKind::Value(TomlValueKind::Integer));
        assert_eq!(text.get(quoted.range.clone()).unwrap(), "2");
        assert!(quoted.inline_direct);
        let when = node(&projection, "$.inline.when");
        assert_eq!(when.kind, TomlNodeKind::Value(TomlValueKind::Datetime));
        assert_eq!(text.get(when.range.clone()).unwrap(), "1979-05-27");
    }

    #[test]
    fn invalid_source_keeps_a_location_and_no_tree() {
        let projection = project("a = \n");
        assert!(projection.error().is_some());
        assert!(projection.root().is_none());
        assert_eq!(projection.node_count(), 0);
    }
}
