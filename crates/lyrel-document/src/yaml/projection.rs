//! YAML 的无损结构投影。
//!
//! 源码是唯一权威。这里把 `yaml-edit` 的 rowan CST 一次性读成纯数据的节点
//! 索引：多文档、mapping／sequence、scalar 原拼写、anchor／alias／tag 与注释
//! 都不被转换成 JSON 或 `YamlValue`。CST 句柄只在后台线程内存在，跨线程
//! 返回的只有本投影的拥有型字符串和 byte range。
//!
//! 每个节点只保存源码范围与显示文本；编辑由 `mutation` 在同一份源码上规划
//! 局部 splice，绝不把整棵树重新序列化。解析失败或超出预算时源码仍然可
//! 编辑，视图只显示可见原因。

use std::collections::HashMap;
use std::ops::Range;

use rowan::SyntaxNode as RowanSyntaxNode;
use yaml_edit::anchor_resolution::ResolveError;
use yaml_edit::{
    Alias, AnchorRegistry, AsYaml, Document, Lang, Mapping, MappingEntry, Scalar, ScalarType,
    ScalarValue, Sequence, SyntaxKind, TaggedNode, YamlFile, YamlNode as AstYamlNode,
};

use crate::state::CancelToken;
use crate::tree::{Treelike, truncated_chars};

type SyntaxNode = RowanSyntaxNode<Lang>;

/// 结构投影能读取的最大源码字节数；更大的文件仍可作为源码打开与保存，
/// 只是不生成结构树。
pub const STRUCTURE_INPUT_BYTES: usize = 1024 * 1024;
/// 一份投影最多保留的节点数；超限时整份结构视图报预算错误。
pub const STRUCTURE_NODES: usize = 20_000;
/// 所有节点的完整路径字符串合计。
pub const STRUCTURE_PATH_BYTES: usize = 8 * 1024 * 1024;
/// 嵌套深度上限，避免病态输入在投影递归里耗尽栈。
pub const STRUCTURE_DEPTH: usize = 512;

/// 行内预览最多显示的字符数；完整原文仍可从节点范围读取。
const DISPLAY_CHARS: usize = 160;
/// 每处理这么多节点检查一次取消。
const CANCEL_INTERVAL: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum YamlScalarKind {
    String,
    Integer,
    Float,
    Boolean,
    Null,
    Timestamp,
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum YamlNodeKind {
    File,
    Mapping,
    Sequence,
    Scalar(YamlScalarKind),
    Alias,
    Tagged,
    Empty,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContainerKind {
    Mapping,
    Sequence,
}

/// 一个可显示的节点。`key_range` 是 entry 的键内容范围（有 anchor／tag 时
/// 仍只指向被替换的 scalar）；`value_range` 是可在树中替换的 scalar 内容；
/// `container_range` 是 collection 自身的范围，用于增删成员。
#[derive(Clone, Debug)]
pub struct YamlNode {
    pub key: String,
    pub path: String,
    pub value: String,
    pub range: Range<usize>,
    pub key_range: Option<Range<usize>>,
    /// 完整键节点范围，包含复杂键／tag／anchor；用于把不可无损改写的键
    /// 精确定位到源码，而不是在树里猜一个新键。
    pub key_detail_range: Option<Range<usize>>,
    /// 键的原始源码文本（含引号），仅用于预填重命名字段。
    pub key_text: Option<String>,
    pub value_range: Option<Range<usize>>,
    /// 别名解析与 anchor 归属使用的值身份范围。
    pub content_range: Range<usize>,
    pub container_range: Option<Range<usize>>,
    pub kind: YamlNodeKind,
    pub children: Vec<usize>,
    pub parent: Option<usize>,
    pub document: usize,
    pub entry: bool,
    pub item: bool,
    pub container: Option<ContainerKind>,
    pub flow: bool,
    pub block_scalar: bool,
    pub key_editable: bool,
    pub key_anchor: Option<String>,
    pub key_alias_refs: usize,
    pub key_tag: Option<String>,
    pub anchor: Option<String>,
    pub alias_refs: usize,
    pub tag: Option<String>,
    pub alias: Option<String>,
    pub alias_target: Option<usize>,
    pub alias_error: Option<String>,
}

impl YamlNode {
    /// 选中该节点时在源码中定位的范围：entry 取键到值末尾，其余取节点本身。
    pub fn detail_range(&self) -> Range<usize> {
        match &self.key_range {
            Some(key) => key.start..self.range.end.max(key.end),
            None => self.range.clone(),
        }
    }

    pub fn has_source_detail(&self) -> bool {
        !self.detail_range().is_empty()
    }

    /// 可在树中编辑的 scalar 值（alias、缺失值、block scalar 除外）。
    pub fn value_editable(&self) -> bool {
        self.value_range.is_some()
            && self.container.is_none()
            && !self.block_scalar
            && self.alias.is_none()
    }

    /// 可在树中重命名的简单键。
    pub fn rename_key_editable(&self) -> bool {
        self.entry && self.key_editable && self.key_range.is_some()
    }

    pub fn can_add(&self) -> bool {
        self.container.is_some()
    }

    pub fn can_remove(&self) -> bool {
        (self.entry || self.item) && self.parent.is_some()
    }
}

#[derive(Clone, Debug)]
pub struct YamlParseFailure {
    pub line: usize,
    pub column: usize,
    pub message: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProjectionFailure {
    InputTooLarge,
    TooManyNodes,
    PathsTooLarge,
    TooDeep,
    Cancelled,
}

/// 已解析的一份源码快照；不持有源码本身，只引用其中的字节范围。
#[derive(Clone, Debug)]
pub struct YamlProjection {
    nodes: Vec<YamlNode>,
    root: Option<usize>,
    documents: usize,
    error: Option<YamlParseFailure>,
}

impl YamlProjection {
    pub fn nodes(&self) -> &[YamlNode] {
        &self.nodes
    }

    pub fn root(&self) -> Option<usize> {
        self.root
    }

    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    pub fn document_count(&self) -> usize {
        self.documents
    }

    pub fn error(&self) -> Option<&YamlParseFailure> {
        self.error.as_ref()
    }
}

impl Treelike for YamlProjection {
    fn root(&self) -> Option<usize> {
        self.root
    }

    fn children(&self, node: usize) -> &[usize] {
        self.nodes
            .get(node)
            .map_or(&[], |node| node.children.as_slice())
    }
}

/// 在后台线程把一份源码快照投影成拥有型节点列表。
pub fn project(text: &str, cancel: &CancelToken) -> Result<YamlProjection, ProjectionFailure> {
    if text.len() > STRUCTURE_INPUT_BYTES {
        return Err(ProjectionFailure::InputTooLarge);
    }
    let parse = YamlFile::parse(text);
    if let Some(first) = parse.positioned_errors().first() {
        let position = first.start_position(text);
        return Ok(YamlProjection {
            nodes: Vec::new(),
            root: None,
            documents: 0,
            error: Some(YamlParseFailure {
                line: position.line,
                column: position.column,
                message: first.message.clone(),
            }),
        });
    }
    let file = parse.tree();
    let documents: Vec<Document> = file.documents().collect();
    let mut builder = Builder {
        text,
        nodes: Vec::new(),
        path_bytes: 0,
        cancel,
        aliases: Vec::new(),
    };
    let mut registries = Vec::with_capacity(documents.len());
    let root = match documents.len() {
        0 => Some(builder.empty(0, 0..text.len(), String::new())?),
        1 => Some(builder.build_document(&documents[0], 0, String::new(), None, 0)?),
        count => {
            let root = builder.file_node(count, 0..text.len())?;
            let mut children = Vec::with_capacity(count);
            for (index, document) in documents.iter().enumerate() {
                let child = builder.build_document(
                    document,
                    index,
                    format!("#{}", index + 1),
                    Some(root),
                    1,
                )?;
                children.push(child);
            }
            builder.nodes[root].children = children;
            Some(root)
        }
    };
    for document in &documents {
        registries.push(AnchorRegistry::from_document(document));
    }
    builder.resolve_aliases(&registries);
    Ok(YamlProjection {
        nodes: builder.nodes,
        root,
        documents: documents.len(),
        error: None,
    })
}

struct AliasSource {
    node: usize,
    document: usize,
    alias: Alias,
}

struct Builder<'a> {
    text: &'a str,
    nodes: Vec<YamlNode>,
    path_bytes: usize,
    cancel: &'a CancelToken,
    aliases: Vec<AliasSource>,
}

#[derive(Default)]
struct Properties {
    anchor: Option<String>,
    tag: Option<String>,
}

struct KeyInfo {
    display: String,
    range: Range<usize>,
    raw_range: Option<Range<usize>>,
    editable: bool,
    anchor: Option<String>,
    tag: Option<String>,
}

impl<'a> Builder<'a> {
    fn check_cancel(&self) -> Result<(), ProjectionFailure> {
        if self.cancel.is_cancelled() {
            Err(ProjectionFailure::Cancelled)
        } else {
            Ok(())
        }
    }

    fn add(&mut self, node: YamlNode) -> Result<usize, ProjectionFailure> {
        if self.nodes.len() >= STRUCTURE_NODES {
            return Err(ProjectionFailure::TooManyNodes);
        }
        if (self.nodes.len() + 1).is_multiple_of(CANCEL_INTERVAL) {
            self.check_cancel()?;
        }
        self.path_bytes = self.path_bytes.saturating_add(node.path.len());
        if self.path_bytes > STRUCTURE_PATH_BYTES {
            return Err(ProjectionFailure::PathsTooLarge);
        }
        let index = self.nodes.len();
        self.nodes.push(node);
        Ok(index)
    }

    fn check_depth(depth: usize) -> Result<(), ProjectionFailure> {
        match depth > STRUCTURE_DEPTH {
            true => Err(ProjectionFailure::TooDeep),
            false => Ok(()),
        }
    }

    fn file_node(
        &mut self,
        documents: usize,
        range: Range<usize>,
    ) -> Result<usize, ProjectionFailure> {
        self.add(YamlNode {
            key: String::new(),
            path: "$".to_owned(),
            value: format!("{documents} documents"),
            range,
            key_range: None,
            key_detail_range: None,
            key_text: None,
            value_range: None,
            content_range: 0..0,
            container_range: None,
            kind: YamlNodeKind::File,
            children: Vec::new(),
            parent: None,
            document: 0,
            entry: false,
            item: false,
            container: None,
            flow: false,
            block_scalar: false,
            key_editable: false,
            key_anchor: None,
            key_alias_refs: 0,
            key_tag: None,
            anchor: None,
            alias_refs: 0,
            tag: None,
            alias: None,
            alias_target: None,
            alias_error: None,
        })
    }

    fn empty(
        &mut self,
        document: usize,
        range: Range<usize>,
        key: String,
    ) -> Result<usize, ProjectionFailure> {
        let path = match key.is_empty() {
            true => "$".to_owned(),
            false => format!("${key}"),
        };
        self.add(YamlNode {
            key,
            path,
            value: "(empty document)".to_owned(),
            range: range.clone(),
            key_range: None,
            key_detail_range: None,
            key_text: None,
            value_range: None,
            content_range: range,
            container_range: None,
            kind: YamlNodeKind::Empty,
            children: Vec::new(),
            parent: None,
            document,
            entry: false,
            item: false,
            container: None,
            flow: false,
            block_scalar: false,
            key_editable: false,
            key_anchor: None,
            key_alias_refs: 0,
            key_tag: None,
            anchor: None,
            alias_refs: 0,
            tag: None,
            alias: None,
            alias_target: None,
            alias_error: None,
        })
    }

    fn build_document(
        &mut self,
        document: &Document,
        document_index: usize,
        key: String,
        parent: Option<usize>,
        depth: usize,
    ) -> Result<usize, ProjectionFailure> {
        Self::check_depth(depth)?;
        let path = match key.is_empty() {
            true => "$".to_owned(),
            false => format!("${key}"),
        };
        let Some(value) = document_value(document) else {
            let index = self.empty(document_index, document_range(document), key)?;
            self.nodes[index].parent = parent;
            return Ok(index);
        };
        let index = self.add(YamlNode {
            key,
            path: path.clone(),
            value: String::new(),
            range: value_range(&value),
            key_range: None,
            key_detail_range: None,
            key_text: None,
            value_range: None,
            content_range: value_range(&value),
            container_range: None,
            kind: YamlNodeKind::Empty,
            children: Vec::new(),
            parent,
            document: document_index,
            entry: false,
            item: false,
            container: None,
            flow: false,
            block_scalar: false,
            key_editable: false,
            key_anchor: None,
            key_alias_refs: 0,
            key_tag: None,
            anchor: None,
            alias_refs: 0,
            tag: None,
            alias: None,
            alias_target: None,
            alias_error: None,
        })?;
        self.fill_value(index, &value, &path, document_index, depth + 1)?;
        Ok(index)
    }

    fn build_entry(
        &mut self,
        entry: MappingEntry,
        parent: usize,
        parent_path: &str,
        document: usize,
        occurrence: usize,
        depth: usize,
    ) -> Result<usize, ProjectionFailure> {
        Self::check_depth(depth)?;
        let key = key_info(&entry);
        let raw_text = key
            .raw_range
            .as_ref()
            .and_then(|range| self.text.get(range.clone()))
            .map(str::to_owned);
        let path = child_path(parent_path, &key.display, occurrence);
        let value = entry.value_node();
        let end = value
            .as_ref()
            .map(value_range)
            .map_or(key.range.end, |range| range.end);
        let index = self.add(YamlNode {
            key: key.display,
            path: path.clone(),
            value: String::new(),
            range: key.range.start..end,
            key_range: key.raw_range.clone(),
            key_detail_range: (!key.range.is_empty()).then_some(key.range.clone()),
            key_text: raw_text,
            value_range: None,
            content_range: key.range.clone(),
            container_range: None,
            kind: YamlNodeKind::Empty,
            children: Vec::new(),
            parent: Some(parent),
            document,
            entry: true,
            item: false,
            container: None,
            flow: false,
            block_scalar: false,
            key_editable: key.editable,
            key_anchor: key.anchor,
            key_alias_refs: 0,
            key_tag: key.tag,
            anchor: None,
            alias_refs: 0,
            tag: None,
            alias: None,
            alias_target: None,
            alias_error: None,
        })?;
        match value {
            Some(value) => self.fill_value(index, &value, &path, document, depth + 1)?,
            None => {
                self.nodes[index].value = "null".to_owned();
                self.nodes[index].kind = YamlNodeKind::Scalar(YamlScalarKind::Null);
            }
        }
        Ok(index)
    }

    fn build_item(
        &mut self,
        value: AstYamlNode,
        parent: usize,
        parent_path: &str,
        document: usize,
        index: usize,
        depth: usize,
    ) -> Result<usize, ProjectionFailure> {
        Self::check_depth(depth)?;
        let path = format!("{parent_path}[{index}]");
        let range = value_range(&value);
        let node = self.add(YamlNode {
            key: format!("[{index}]"),
            path: path.clone(),
            value: String::new(),
            range: range.clone(),
            key_range: None,
            key_detail_range: None,
            key_text: None,
            value_range: None,
            content_range: range,
            container_range: None,
            kind: YamlNodeKind::Empty,
            children: Vec::new(),
            parent: Some(parent),
            document,
            entry: false,
            item: true,
            container: None,
            flow: false,
            block_scalar: false,
            key_editable: false,
            key_anchor: None,
            key_alias_refs: 0,
            key_tag: None,
            anchor: None,
            alias_refs: 0,
            tag: None,
            alias: None,
            alias_target: None,
            alias_error: None,
        })?;
        self.fill_value(node, &value, &path, document, depth + 1)?;
        Ok(node)
    }

    fn fill_value(
        &mut self,
        index: usize,
        value: &AstYamlNode,
        path: &str,
        document: usize,
        depth: usize,
    ) -> Result<(), ProjectionFailure> {
        Self::check_depth(depth)?;
        self.check_cancel()?;
        let properties = value.as_node().map(node_properties).unwrap_or_default();
        self.nodes[index].anchor = properties.anchor;
        self.nodes[index].tag = properties.tag;
        match value {
            AstYamlNode::Scalar(scalar) => {
                self.fill_scalar(index, scalar);
            }
            AstYamlNode::Mapping(mapping) => {
                self.fill_mapping(index, mapping, path, document, depth)?;
            }
            AstYamlNode::Sequence(sequence) => {
                self.fill_sequence(index, sequence, path, document, depth)?;
            }
            AstYamlNode::TaggedNode(tagged) => {
                self.fill_tagged(index, tagged, path, document, depth)?;
            }
            AstYamlNode::Alias(alias) => {
                let range = alias_range(alias);
                self.nodes[index].kind = YamlNodeKind::Alias;
                self.nodes[index].alias = Some(alias.name());
                self.nodes[index].value = format!("*{}", alias.name());
                self.nodes[index].content_range = range.clone();
                self.nodes[index].range = match self.nodes[index].entry {
                    true => self.nodes[index].range.start..range.end,
                    false => range,
                };
                self.aliases.push(AliasSource {
                    node: index,
                    document,
                    alias: alias.clone(),
                });
            }
        }
        Ok(())
    }

    fn fill_scalar(&mut self, index: usize, scalar: &Scalar) {
        let range = scalar_range(scalar);
        let raw = self.text.get(range.clone()).unwrap_or_default();
        self.nodes[index].kind = YamlNodeKind::Scalar(scalar_kind(scalar));
        self.nodes[index].value = snippet(raw);
        self.nodes[index].content_range = range.clone();
        self.nodes[index].block_scalar = scalar.is_block();
        self.nodes[index].value_range = (!scalar.is_block()).then_some(range.clone());
        if !self.nodes[index].entry && !self.nodes[index].item {
            self.nodes[index].range = range;
        }
    }

    fn fill_mapping(
        &mut self,
        index: usize,
        mapping: &Mapping,
        path: &str,
        document: usize,
        depth: usize,
    ) -> Result<(), ProjectionFailure> {
        let range = mapping_range(mapping);
        self.nodes[index].kind = YamlNodeKind::Mapping;
        self.nodes[index].container = Some(ContainerKind::Mapping);
        self.nodes[index].container_range = Some(range.clone());
        self.nodes[index].content_range = range.clone();
        self.nodes[index].flow = mapping.is_flow_style();
        self.nodes[index].value = format!("{{ {} keys }}", mapping.len());
        if self.nodes[index].entry || self.nodes[index].item {
            self.nodes[index].value_range = Some(range.clone());
        } else {
            self.nodes[index].range = range.clone();
            self.nodes[index].value_range = Some(range);
        }
        let mut occurrences: HashMap<String, usize> = HashMap::new();
        let mut children = Vec::new();
        for entry in mapping.entries() {
            let info = key_info(&entry);
            let occurrence = occurrences.entry(info.display.clone()).or_insert(0);
            *occurrence += 1;
            let child = self.build_entry(entry, index, path, document, *occurrence, depth + 1)?;
            children.push(child);
        }
        self.nodes[index].children = children;
        Ok(())
    }

    fn fill_sequence(
        &mut self,
        index: usize,
        sequence: &Sequence,
        path: &str,
        document: usize,
        depth: usize,
    ) -> Result<(), ProjectionFailure> {
        let range = sequence_range(sequence);
        self.nodes[index].kind = YamlNodeKind::Sequence;
        self.nodes[index].container = Some(ContainerKind::Sequence);
        self.nodes[index].container_range = Some(range.clone());
        self.nodes[index].content_range = range.clone();
        self.nodes[index].flow = sequence.is_flow_style();
        self.nodes[index].value = format!("[ {} items ]", sequence.len());
        if self.nodes[index].entry || self.nodes[index].item {
            self.nodes[index].value_range = Some(range.clone());
        } else {
            self.nodes[index].range = range.clone();
            self.nodes[index].value_range = Some(range);
        }
        let mut children = Vec::new();
        for (item, value) in sequence.values().enumerate() {
            let child = self.build_item(value, index, path, document, item, depth + 1)?;
            children.push(child);
        }
        self.nodes[index].children = children;
        Ok(())
    }

    fn fill_tagged(
        &mut self,
        index: usize,
        tagged: &TaggedNode,
        path: &str,
        document: usize,
        depth: usize,
    ) -> Result<(), ProjectionFailure> {
        let range = tagged_range(tagged);
        self.nodes[index].kind = YamlNodeKind::Tagged;
        if let Some(tag) = tagged.tag() {
            self.nodes[index].tag = Some(tag);
        }
        self.nodes[index].content_range = range.clone();
        if let Some(mapping) = tagged.as_mapping() {
            self.nodes[index].container = Some(ContainerKind::Mapping);
            self.nodes[index].container_range = Some(mapping_range(&mapping));
            self.nodes[index].flow = mapping.is_flow_style();
            self.nodes[index].value = format!("{{ {} keys }}", mapping.len());
            self.nodes[index].value_range = Some(mapping_range(&mapping));
            let mut occurrences: HashMap<String, usize> = HashMap::new();
            let mut children = Vec::new();
            for entry in mapping.entries() {
                let info = key_info(&entry);
                let occurrence = occurrences.entry(info.display.clone()).or_insert(0);
                *occurrence += 1;
                let child =
                    self.build_entry(entry, index, path, document, *occurrence, depth + 1)?;
                children.push(child);
            }
            self.nodes[index].children = children;
        } else if let Some(sequence) = tagged.as_sequence() {
            self.nodes[index].container = Some(ContainerKind::Sequence);
            self.nodes[index].container_range = Some(sequence_range(&sequence));
            self.nodes[index].flow = sequence.is_flow_style();
            self.nodes[index].value = format!("[ {} items ]", sequence.len());
            self.nodes[index].value_range = Some(sequence_range(&sequence));
            let mut children = Vec::new();
            for (item, value) in sequence.values().enumerate() {
                let child = self.build_item(value, index, path, document, item, depth + 1)?;
                children.push(child);
            }
            self.nodes[index].children = children;
        } else if let Some(scalar) = tagged.value() {
            let scalar_range = scalar_range(&scalar);
            self.nodes[index].kind = YamlNodeKind::Scalar(match tagged.tag().as_deref() {
                Some("!!str") => YamlScalarKind::String,
                _ => scalar_kind(&scalar),
            });
            self.nodes[index].value =
                snippet(self.text.get(scalar_range.clone()).unwrap_or_default());
            self.nodes[index].block_scalar = scalar.is_block();
            self.nodes[index].value_range = (!scalar.is_block()).then_some(scalar_range);
        } else {
            self.nodes[index].value = snippet(self.text.get(range.clone()).unwrap_or_default());
        }
        if !self.nodes[index].entry && !self.nodes[index].item {
            self.nodes[index].range = range;
        }
        Ok(())
    }

    fn resolve_aliases(&mut self, registries: &[AnchorRegistry]) -> bool {
        let mut by_range: HashMap<(usize, usize, usize), usize> = HashMap::new();
        let mut by_key: HashMap<(usize, usize, usize), usize> = HashMap::new();
        for (index, node) in self.nodes.iter().enumerate() {
            by_range
                .entry((
                    node.document,
                    node.content_range.start,
                    node.content_range.end,
                ))
                .or_insert(index);
            if let Some(key) = &node.key_range {
                by_key
                    .entry((node.document, key.start, key.end))
                    .or_insert(index);
            }
        }
        let aliases = std::mem::take(&mut self.aliases);
        let mut limited = false;
        for source in aliases {
            let Some(registry) = registries.get(source.document) else {
                limited = true;
                continue;
            };
            match registry.resolve_alias(&source.alias) {
                Ok(target) => {
                    let Some(syntax) = target.as_node() else {
                        limited = true;
                        continue;
                    };
                    let range = held_range(syntax);
                    if let Some(&target_index) =
                        by_range.get(&(source.document, range.start, range.end))
                    {
                        self.nodes[source.node].alias_target = Some(target_index);
                        self.nodes[target_index].alias_refs += 1;
                        if self.nodes[target_index].anchor.is_none() {
                            self.nodes[target_index].anchor = Some(source.alias.name());
                        }
                    } else if let Some(&target_index) =
                        by_key.get(&(source.document, range.start, range.end))
                    {
                        self.nodes[target_index].key_alias_refs += 1;
                        if self.nodes[target_index].key_anchor.is_none() {
                            self.nodes[target_index].key_anchor = Some(source.alias.name());
                        }
                    } else {
                        limited = true;
                    }
                }
                Err(error) => {
                    self.nodes[source.node].alias_error = Some(alias_error(&error));
                }
            }
        }
        for (document, registry) in registries.iter().enumerate() {
            for name in registry.anchor_names() {
                let Some(syntax) = registry.resolve(name) else {
                    continue;
                };
                let range = held_range(syntax);
                if let Some(&index) = by_range.get(&(document, range.start, range.end)) {
                    self.nodes[index]
                        .anchor
                        .get_or_insert_with(|| name.to_owned());
                } else if let Some(&index) = by_key.get(&(document, range.start, range.end)) {
                    self.nodes[index]
                        .key_anchor
                        .get_or_insert_with(|| name.to_owned());
                } else {
                    limited = true;
                }
            }
        }
        limited
    }
}

fn document_value(document: &Document) -> Option<AstYamlNode> {
    if let Some(mapping) = document.as_mapping() {
        return Some(AstYamlNode::Mapping(mapping));
    }
    if let Some(sequence) = document.as_sequence() {
        return Some(AstYamlNode::Sequence(sequence));
    }
    if let Some(scalar) = document.as_scalar() {
        return Some(AstYamlNode::Scalar(scalar));
    }
    if let Some(tagged) = document.as_tagged() {
        return Some(AstYamlNode::TaggedNode(tagged));
    }
    let syntax = document.as_node()?;
    syntax.children().find_map(AstYamlNode::from_syntax)
}

fn key_info(entry: &MappingEntry) -> KeyInfo {
    let syntax = entry.key_node();
    let Some(key) = syntax else {
        return KeyInfo {
            display: String::new(),
            range: 0..0,
            raw_range: None,
            editable: false,
            anchor: None,
            tag: None,
        };
    };
    let properties = key.as_node().map(node_properties).unwrap_or_default();
    match &key {
        AstYamlNode::Scalar(scalar) => KeyInfo {
            display: scalar.as_string(),
            range: scalar_range(scalar),
            raw_range: Some(scalar_range(scalar)),
            editable: !scalar.is_block(),
            anchor: properties.anchor,
            tag: properties.tag,
        },
        AstYamlNode::TaggedNode(tagged) => {
            let scalar = tagged.value();
            KeyInfo {
                display: scalar
                    .as_ref()
                    .map(Scalar::as_string)
                    .unwrap_or_else(|| snippet(&tagged.to_string())),
                range: tagged_range(tagged),
                raw_range: scalar.as_ref().map(scalar_range),
                editable: scalar.as_ref().is_some_and(|scalar| !scalar.is_block()),
                anchor: properties.anchor,
                tag: tagged.tag().or(properties.tag),
            }
        }
        AstYamlNode::Mapping(mapping) => KeyInfo {
            display: snippet(&mapping.to_string()),
            range: mapping_range(mapping),
            raw_range: None,
            editable: false,
            anchor: properties.anchor,
            tag: properties.tag,
        },
        AstYamlNode::Sequence(sequence) => KeyInfo {
            display: snippet(&sequence.to_string()),
            range: sequence_range(sequence),
            raw_range: None,
            editable: false,
            anchor: properties.anchor,
            tag: properties.tag,
        },
        AstYamlNode::Alias(alias) => KeyInfo {
            display: format!("*{}", alias.name()),
            range: alias_range(alias),
            raw_range: None,
            editable: false,
            anchor: properties.anchor,
            tag: properties.tag,
        },
    }
}

fn node_properties(node: &SyntaxNode) -> Properties {
    let mut properties = Properties::default();
    let mut current = Some(node.clone());
    for depth in 0..3 {
        let Some(candidate) = current else {
            break;
        };
        // parser 把值／键的 anchor 与 tag 放在包着它的节点里（KEY、VALUE、
        // SEQUENCE_ENTRY、TAGGED_NODE、DOCUMENT），collection 自己的标注也在
        // 包着它的节点上，而不是 collection 的 token 子节点。祖先 collection
        // 一旦参与扫描，每个子节点都要走完整份兄弟列表：平铺的 20000 项文件
        // 会退化成二次方，节点预算还没命中就先耗尽消费者的等待期限。节点自身
        // 仍扫描一次，那是一次线性于它自己子节点数的成本。
        let scans_siblings = depth == 0
            || !matches!(
                candidate.kind(),
                SyntaxKind::MAPPING | SyntaxKind::SEQUENCE | SyntaxKind::ROOT
            );
        if scans_siblings {
            for child in candidate.children_with_tokens() {
                let Some(token) = child.as_token() else {
                    continue;
                };
                match token.kind() {
                    SyntaxKind::ANCHOR => {
                        if properties.anchor.is_none() {
                            properties.anchor = token.text().strip_prefix('&').map(str::to_owned);
                        }
                    }
                    SyntaxKind::TAG if properties.tag.is_none() => {
                        properties.tag = Some(token.text().to_owned());
                    }
                    _ => {}
                }
            }
        }
        current = candidate.parent();
    }
    properties
}

/// KEY／VALUE 是 parser 的包装节点；anchor registry 可能返回包装范围，
/// 而投影节点保存的是真正内容。这里剥掉一层，其他 kind 原样返回。
fn held_range(node: &SyntaxNode) -> Range<usize> {
    let mut node = node.clone();
    if matches!(node.kind(), SyntaxKind::KEY | SyntaxKind::VALUE)
        && let Some(child) = node.children().next()
    {
        node = child;
    }
    let range = yaml_edit::advanced::syntax_node_range(&node);
    u32::from(range.start()) as usize..u32::from(range.end()) as usize
}

fn alias_error(error: &ResolveError) -> String {
    match error {
        ResolveError::UndefinedAlias { alias } => {
            format!("alias *{alias} has no anchor before it")
        }
        ResolveError::RecursiveAlias { alias } => {
            format!("alias *{alias} is recursive")
        }
        other => other.to_string(),
    }
}

fn scalar_kind(scalar: &Scalar) -> YamlScalarKind {
    if scalar.is_quoted() || scalar.is_block() {
        return YamlScalarKind::String;
    }
    scalar_type_kind(ScalarValue::parse(scalar.value()).scalar_type())
}

fn scalar_type_kind(kind: ScalarType) -> YamlScalarKind {
    match kind {
        ScalarType::Integer => YamlScalarKind::Integer,
        ScalarType::Float => YamlScalarKind::Float,
        ScalarType::Boolean => YamlScalarKind::Boolean,
        ScalarType::Null => YamlScalarKind::Null,
        ScalarType::Timestamp => YamlScalarKind::Timestamp,
        ScalarType::String => YamlScalarKind::String,
        _ => YamlScalarKind::Other,
    }
}

fn child_path(parent: &str, key: &str, occurrence: usize) -> String {
    let mut path = format!("{parent}[{}]", quoted_key(key));
    if occurrence > 1 {
        path.push_str(&format!(" #{occurrence}"));
    }
    path
}

fn quoted_key(key: &str) -> String {
    let mut out = String::with_capacity(key.len() + 2);
    out.push('"');
    for ch in key.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            ch => out.push(ch),
        }
    }
    out.push('"');
    out
}

fn snippet(text: &str) -> String {
    let compact = text.replace("\r\n", " ⏎ ").replace(['\r', '\n'], " ⏎ ");
    truncated_chars(&compact, DISPLAY_CHARS)
}

fn value_range(value: &AstYamlNode) -> Range<usize> {
    match value {
        AstYamlNode::Scalar(scalar) => scalar_range(scalar),
        AstYamlNode::Mapping(mapping) => mapping_range(mapping),
        AstYamlNode::Sequence(sequence) => sequence_range(sequence),
        AstYamlNode::Alias(alias) => alias_range(alias),
        AstYamlNode::TaggedNode(tagged) => tagged_range(tagged),
    }
}

fn document_range(document: &Document) -> Range<usize> {
    let range = document.byte_range();
    range.start as usize..range.end as usize
}

fn scalar_range(scalar: &Scalar) -> Range<usize> {
    let range = scalar.byte_range();
    range.start as usize..range.end as usize
}

fn mapping_range(mapping: &Mapping) -> Range<usize> {
    let range = mapping.byte_range();
    range.start as usize..range.end as usize
}

fn sequence_range(sequence: &Sequence) -> Range<usize> {
    let range = sequence.byte_range();
    range.start as usize..range.end as usize
}

fn tagged_range(tagged: &TaggedNode) -> Range<usize> {
    tagged.as_node().map(syntax_range).unwrap_or_default()
}

fn alias_range(alias: &Alias) -> Range<usize> {
    alias.as_node().map(syntax_range).unwrap_or_default()
}

fn syntax_range(node: &SyntaxNode) -> Range<usize> {
    let range = yaml_edit::advanced::syntax_node_range(node);
    u32::from(range.start()) as usize..u32::from(range.end()) as usize
}
