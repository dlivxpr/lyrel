//! #294 XML 的有序、源码链接结构投影。
//!
//! 源码是唯一权威。这里只把一份已规范化为 LF 的源码快照解析成节点索引：
//! 每个节点保存显示文本和源码范围，不保存第二份可变文档，也不把 XML 转成
//! 通用对象或 `serde_json::Value`。
//!
//! - `xmlparser` 按字节给出 `ElementStart`／`Attribute`／`Text`／`Cdata`／
//!   `Comment`／PI／声明的精确范围；混合内容因此保持原始顺序，相邻 Text 与
//!   CDATA 不会被合并，属性的单引号／双引号、周围空白和实体拼写都留在源码里。
//! - `roxmltree` 只用于良构校验、命名空间解析和有界节点数检查。它默认拒绝
//!   DTD，本模块在任何解析前也独立拒绝 DTD／实体声明；两者都不会访问外部
//!   文件或网络。DTD 拒绝是可见错误，源码仍可编辑且字节不变。
//! - 元素自身与其属性、内容节点按源码顺序成为同一棵树的子节点；属性排在
//!   内容之前，但属性之间与内容之间各自保持原顺序。属性不会混进文本值。
//! - 节点预算、路径预算和取消都在后台解析路径上生效；超限时整份结构视图
//!   报明确的不可用，源码不受影响。

use std::collections::HashMap;
use std::ops::Range;
use std::sync::atomic::{AtomicBool, Ordering};

use roxmltree::ParsingOptions;
use xmlparser::{ElementEnd, Token, Tokenizer};

use super::super::structure_view::{Treelike, truncated_chars};

/// 结构投影读取的最大源码字节数；更大的文件仍作为源码打开与保存。
pub(crate) const XML_INPUT_BYTES: usize = 1024 * 1024;
/// 一份投影最多保留的节点数。XML 的属性、CDATA 与注释各自成节点，因此
/// 预算比 JSON 的 20,000 宽；3000 条混合内容记录仍应完整显示。
pub(crate) const XML_NODES: usize = 40_000;
/// 所有节点路径字符串合计；深层长名字不能按节点数无界放大内存。
pub(crate) const XML_PATH_BYTES: usize = 8 * 1024 * 1024;

const DISPLAY_CHARS: usize = 160;
const CANCEL_INTERVAL: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum XmlNodeKind {
    Document,
    Element,
    Attribute,
    Text,
    Cdata,
    Comment,
    ProcessingInstruction,
    Declaration,
}

impl XmlNodeKind {
    pub(crate) fn is_value(self) -> bool {
        matches!(
            self,
            Self::Attribute | Self::Text | Self::Cdata | Self::Comment
        )
    }
}

/// 一个可显示的结构节点。`range` 是揭示该节点时选中的完整源码范围；
/// `value_range` 是可编辑值的精确内部范围（属性值、文本、CDATA／注释正文）。
#[derive(Clone, Debug)]
pub(crate) struct XmlNode {
    /// 行首列显示：元素／属性名、`#text`、`#cdata`、`#comment` 或 PI 目标。
    pub(crate) key: String,
    pub(crate) path: String,
    pub(crate) value: String,
    pub(crate) range: Range<usize>,
    pub(crate) key_range: Option<Range<usize>>,
    pub(crate) value_range: Option<Range<usize>>,
    pub(crate) kind: XmlNodeKind,
    pub(crate) children: Vec<usize>,
    pub(crate) parent: Option<usize>,
    /// 元素／属性的解析后命名空间 URI；无命名空间或非命名节点为 `None`。
    pub(crate) namespace: Option<String>,
    /// 该属性本身是 `xmlns`／`xmlns:prefix` 声明。
    pub(crate) is_namespace: bool,
    pub(crate) self_closing: bool,
    /// 起始标签结束位置：`>` 或 `/>` 的起点。
    pub(crate) tag_close_start: usize,
    /// 起始标签结束后的第一个字节。
    pub(crate) start_tag_end: usize,
    /// 显式结束标签 `</qname>` 的起点；自闭合或文档节点为 `None`。
    pub(crate) close_tag_start: Option<usize>,
    /// 属性值的原始引号字符。
    pub(crate) quote: Option<char>,
}

impl XmlNode {
    pub(crate) fn is_element_root(&self) -> bool {
        self.kind == XmlNodeKind::Element && self.parent.is_some_and(|parent| parent == 0)
    }

    pub(crate) fn detail_range(&self) -> Range<usize> {
        match self.kind {
            XmlNodeKind::Document => 0..0,
            _ => self.range.clone(),
        }
    }

    pub(crate) fn has_source_detail(&self) -> bool {
        self.kind != XmlNodeKind::Document && !self.range.is_empty()
    }

    pub(crate) fn has_full_value(&self) -> bool {
        matches!(
            self.kind,
            XmlNodeKind::Attribute
                | XmlNodeKind::Text
                | XmlNodeKind::Cdata
                | XmlNodeKind::Comment
                | XmlNodeKind::ProcessingInstruction
                | XmlNodeKind::Declaration
        )
    }
}

#[derive(Clone, Debug)]
pub(crate) struct XmlProjection {
    nodes: Vec<XmlNode>,
    root: Option<usize>,
    element_count: usize,
    attribute_count: usize,
    error: Option<XmlParseFailure>,
}

impl XmlProjection {
    pub(crate) fn nodes(&self) -> &[XmlNode] {
        &self.nodes
    }

    pub(crate) fn root(&self) -> Option<usize> {
        self.root
    }

    pub(crate) fn element_count(&self) -> usize {
        self.element_count
    }

    pub(crate) fn attribute_count(&self) -> usize {
        self.attribute_count
    }

    pub(crate) fn error(&self) -> Option<&XmlParseFailure> {
        self.error.as_ref()
    }
}

impl Treelike for XmlProjection {
    fn root(&self) -> Option<usize> {
        self.root
    }

    fn children(&self, node: usize) -> &[usize] {
        &self.nodes[node].children
    }
}

#[derive(Clone, Debug)]
pub(crate) struct XmlParseFailure {
    pub(crate) line: usize,
    pub(crate) column: usize,
    pub(crate) message: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ProjectionFailure {
    Cancelled,
    InputTooLarge,
    TooManyNodes,
    PathsTooLarge,
    /// 文档包含 DTD／实体声明。这里不会读取外部 DTD，也不会解析实体。
    DtdDenied {
        line: usize,
        column: usize,
    },
}

/// Parse one snapshot into a bounded, ordered, source-linked projection.
///
/// `cancel` is checked before parsing, between tokens and while applying the
/// namespace map. The tokenizer and `roxmltree` are synchronous with a bounded
/// input; a cancelled projection is never installed.
pub(crate) fn project(text: &str, cancel: &AtomicBool) -> Result<XmlProjection, ProjectionFailure> {
    if cancel.load(Ordering::Acquire) {
        return Err(ProjectionFailure::Cancelled);
    }
    if text.len() > XML_INPUT_BYTES {
        return Err(ProjectionFailure::InputTooLarge);
    }

    let mut nodes = Vec::new();
    let mut counts = Vec::new();
    nodes.push(XmlNode {
        key: "/".to_owned(),
        path: "/".to_owned(),
        value: String::new(),
        range: 0..text.len(),
        key_range: None,
        value_range: None,
        kind: XmlNodeKind::Document,
        children: Vec::new(),
        parent: None,
        namespace: None,
        is_namespace: false,
        self_closing: false,
        tag_close_start: 0,
        start_tag_end: 0,
        close_tag_start: None,
        quote: None,
    });
    counts.push(ParentCounts::default());
    let mut path_bytes = 1_usize;
    let mut element_count = 0_usize;
    let mut attribute_count = 0_usize;

    let mut stack = vec![0_usize];
    let mut pending: Option<usize> = None;
    let mut token_count = 0_usize;

    for token in Tokenizer::from(text) {
        if token_count % CANCEL_INTERVAL == 0 && cancel.load(Ordering::Acquire) {
            return Err(ProjectionFailure::Cancelled);
        }
        token_count += 1;
        let token = match token {
            Ok(token) => token,
            Err(error) => {
                let pos = error.pos();
                return Ok(error_projection(
                    pos.row as usize,
                    pos.col as usize,
                    error.to_string(),
                ));
            }
        };
        match token {
            Token::Declaration { span, .. } => {
                if pending.is_some() {
                    return Ok(XmlProjection {
                        error: Some(XmlParseFailure {
                            line: 1,
                            column: 1,
                            message: "XML declaration inside an element".to_owned(),
                        }),
                        ..empty_projection()
                    });
                }
                let parent = 0;
                push_child(
                    &mut nodes,
                    &mut counts,
                    &mut path_bytes,
                    parent,
                    XmlNode {
                        key: "?xml".to_owned(),
                        path: "/declaration".to_owned(),
                        value: preview(&text[span.range()]),
                        range: span.range(),
                        key_range: None,
                        value_range: Some(span.range()),
                        kind: XmlNodeKind::Declaration,
                        children: Vec::new(),
                        parent: Some(parent),
                        namespace: None,
                        is_namespace: false,
                        self_closing: false,
                        tag_close_start: 0,
                        start_tag_end: span.end(),
                        close_tag_start: None,
                        quote: None,
                    },
                )?;
            }
            Token::ProcessingInstruction {
                target,
                content,
                span,
            } => {
                let parent = match pending {
                    Some(element) => element,
                    None => *stack.last().unwrap_or(&0),
                };
                let occurrence = {
                    let counts = &mut counts[parent];
                    let count = counts
                        .instructions
                        .entry(target.as_str().to_owned())
                        .or_insert(0);
                    *count += 1;
                    *count
                };
                let path = format!(
                    "{}/processing-instruction({})[{}]",
                    nodes[parent].path,
                    target.as_str(),
                    occurrence
                );
                push_child(
                    &mut nodes,
                    &mut counts,
                    &mut path_bytes,
                    parent,
                    XmlNode {
                        key: format!("<?{}?>", target.as_str()),
                        path,
                        value: content
                            .map(|content| preview(&text[content.range()]))
                            .unwrap_or_default(),
                        range: span.range(),
                        key_range: None,
                        value_range: content.map(|content| content.range()),
                        kind: XmlNodeKind::ProcessingInstruction,
                        children: Vec::new(),
                        parent: Some(parent),
                        namespace: None,
                        is_namespace: false,
                        self_closing: false,
                        tag_close_start: 0,
                        start_tag_end: span.end(),
                        close_tag_start: None,
                        quote: None,
                    },
                )?;
            }
            Token::Comment { text: body, span } => {
                let parent = match pending {
                    Some(element) => element,
                    None => *stack.last().unwrap_or(&0),
                };
                let occurrence = {
                    let count = &mut counts[parent].comments;
                    *count += 1;
                    *count
                };
                let path = format!("{}/comment()[{}]", nodes[parent].path, occurrence);
                push_child(
                    &mut nodes,
                    &mut counts,
                    &mut path_bytes,
                    parent,
                    XmlNode {
                        key: "#comment".to_owned(),
                        path,
                        value: preview(&text[body.range()]),
                        range: span.range(),
                        key_range: None,
                        value_range: Some(body.range()),
                        kind: XmlNodeKind::Comment,
                        children: Vec::new(),
                        parent: Some(parent),
                        namespace: None,
                        is_namespace: false,
                        self_closing: false,
                        tag_close_start: 0,
                        start_tag_end: span.end(),
                        close_tag_start: None,
                        quote: None,
                    },
                )?;
            }
            Token::DtdStart { span, .. }
            | Token::EmptyDtd { span, .. }
            | Token::EntityDeclaration { span, .. } => {
                let (line, column) = line_column(text, span.start());
                return Err(ProjectionFailure::DtdDenied { line, column });
            }
            Token::DtdEnd { .. } => {}
            Token::ElementStart {
                prefix,
                local,
                span,
            } => {
                if pending.is_some() {
                    return Ok(error_projection(
                        1,
                        1,
                        "nested element start before the previous start tag ended".to_owned(),
                    ));
                }
                let parent = *stack.last().unwrap_or(&0);
                let qname = &text[prefix.start()..local.end()];
                let occurrence = {
                    let counts = &mut counts[parent];
                    let count = counts.elements.entry(qname.to_owned()).or_insert(0);
                    *count += 1;
                    *count
                };
                let path = if parent == 0 {
                    format!("/{qname}")
                } else {
                    format!("{}/{}[{}]", nodes[parent].path, qname, occurrence)
                };
                let index = nodes.len();
                push_child(
                    &mut nodes,
                    &mut counts,
                    &mut path_bytes,
                    parent,
                    XmlNode {
                        key: qname.to_owned(),
                        path,
                        value: String::new(),
                        range: span.start()..span.end(),
                        key_range: Some(prefix.start()..local.end()),
                        value_range: None,
                        kind: XmlNodeKind::Element,
                        children: Vec::new(),
                        parent: Some(parent),
                        namespace: None,
                        is_namespace: false,
                        self_closing: false,
                        tag_close_start: span.end(),
                        start_tag_end: span.end(),
                        close_tag_start: None,
                        quote: None,
                    },
                )?;
                element_count += 1;
                pending = Some(index);
            }
            Token::Attribute {
                prefix,
                local,
                value,
                span,
            } => {
                let Some(element) = pending else {
                    return Ok(error_projection(
                        1,
                        1,
                        "attribute outside a start tag".to_owned(),
                    ));
                };
                let qname = &text[prefix.start()..local.end()];
                let is_namespace = (prefix.as_str() == "xmlns")
                    || (prefix.is_empty() && local.as_str() == "xmlns");
                let quote_start = value.start().checked_sub(1);
                let quote = quote_start
                    .and_then(|at| text[at..].chars().next())
                    .filter(|quote| *quote == '"' || *quote == '\'');
                let path = format!("{}/@{}", nodes[element].path, qname);
                push_child(
                    &mut nodes,
                    &mut counts,
                    &mut path_bytes,
                    element,
                    XmlNode {
                        key: qname.to_owned(),
                        path,
                        value: format!(
                            "{}{}{}",
                            quote.unwrap_or('"'),
                            &text[value.range()],
                            quote.unwrap_or('"')
                        ),
                        range: span.range(),
                        key_range: Some(prefix.start()..local.end()),
                        value_range: Some(value.range()),
                        kind: XmlNodeKind::Attribute,
                        children: Vec::new(),
                        parent: Some(element),
                        namespace: None,
                        is_namespace,
                        self_closing: false,
                        tag_close_start: 0,
                        start_tag_end: span.end(),
                        close_tag_start: None,
                        quote,
                    },
                )?;
                attribute_count += 1;
            }
            Token::ElementEnd { end, span } => match end {
                ElementEnd::Open => {
                    let Some(element) = pending.take() else {
                        return Ok(error_projection(
                            span.start(),
                            span.start(),
                            "element end without a start".to_owned(),
                        ));
                    };
                    nodes[element].tag_close_start = span.start();
                    nodes[element].start_tag_end = span.end();
                    stack.push(element);
                }
                ElementEnd::Empty => {
                    let Some(element) = pending.take() else {
                        return Ok(error_projection(
                            span.start(),
                            span.start(),
                            "empty element end without a start".to_owned(),
                        ));
                    };
                    nodes[element].self_closing = true;
                    nodes[element].tag_close_start = span.start();
                    nodes[element].start_tag_end = span.end();
                    nodes[element].range.end = span.end();
                    finish_element(&mut nodes, element);
                }
                ElementEnd::Close(prefix, local) => {
                    let Some(element) = stack.pop() else {
                        return Ok(error_projection(
                            span.start(),
                            span.start(),
                            "close tag without an open element".to_owned(),
                        ));
                    };
                    if element == 0 {
                        return Ok(error_projection(
                            span.start(),
                            span.start(),
                            "close tag without an open element".to_owned(),
                        ));
                    }
                    nodes[element].range.end = span.end();
                    nodes[element].close_tag_start = Some(span.start());
                    let qname = &text[prefix.start()..local.end()];
                    if !nodes[element].key.ends_with(qname) {
                        // 精确的错配由 roxmltree 在下一次校验中拒绝；这里
                        // 不猜测修复，也不吞掉。
                        return Ok(error_projection(
                            span.start(),
                            span.start(),
                            format!("unexpected close tag </{qname}>"),
                        ));
                    }
                    finish_element(&mut nodes, element);
                }
            },
            Token::Text { text: body } => {
                let parent = match pending {
                    Some(element) => element,
                    None => *stack.last().unwrap_or(&0),
                };
                let occurrence = {
                    let count = &mut counts[parent].text;
                    *count += 1;
                    *count
                };
                let path = format!("{}/text()[{}]", nodes[parent].path, occurrence);
                push_child(
                    &mut nodes,
                    &mut counts,
                    &mut path_bytes,
                    parent,
                    XmlNode {
                        key: "#text".to_owned(),
                        path,
                        value: preview(&text[body.range()]),
                        range: body.range(),
                        key_range: None,
                        value_range: Some(body.range()),
                        kind: XmlNodeKind::Text,
                        children: Vec::new(),
                        parent: Some(parent),
                        namespace: None,
                        is_namespace: false,
                        self_closing: false,
                        tag_close_start: 0,
                        start_tag_end: body.end(),
                        close_tag_start: None,
                        quote: None,
                    },
                )?;
            }
            Token::Cdata { text: body, span } => {
                let parent = match pending {
                    Some(element) => element,
                    None => *stack.last().unwrap_or(&0),
                };
                let occurrence = {
                    let count = &mut counts[parent].cdata;
                    *count += 1;
                    *count
                };
                let path = format!("{}/cdata()[{}]", nodes[parent].path, occurrence);
                push_child(
                    &mut nodes,
                    &mut counts,
                    &mut path_bytes,
                    parent,
                    XmlNode {
                        key: "#cdata".to_owned(),
                        path,
                        value: preview(&text[body.range()]),
                        range: span.range(),
                        key_range: None,
                        value_range: Some(body.range()),
                        kind: XmlNodeKind::Cdata,
                        children: Vec::new(),
                        parent: Some(parent),
                        namespace: None,
                        is_namespace: false,
                        self_closing: false,
                        tag_close_start: 0,
                        start_tag_end: span.end(),
                        close_tag_start: None,
                        quote: None,
                    },
                )?;
            }
        }
    }

    if pending.is_some() || stack.len() != 1 {
        let (line, column) = line_column(text, text.len());
        return Ok(error_projection(
            line,
            column,
            "unclosed element".to_owned(),
        ));
    }

    let parsed = roxmltree::Document::parse_with_options(
        text,
        ParsingOptions {
            allow_dtd: false,
            nodes_limit: XML_NODES as u32,
            entity_resolver: None,
        },
    );
    let document = match parsed {
        Ok(document) => document,
        Err(roxmltree::Error::DtdDetected) => {
            let (line, column) = first_dtd_position(text).unwrap_or((1, 1));
            return Err(ProjectionFailure::DtdDenied { line, column });
        }
        Err(roxmltree::Error::NodesLimitReached) => {
            return Err(ProjectionFailure::TooManyNodes);
        }
        Err(error) => {
            let pos = error.pos();
            return Ok(error_projection(
                pos.row as usize,
                pos.col as usize,
                error.to_string(),
            ));
        }
    };

    let namespaces = collect_namespaces(&document, cancel)?;
    for index in 0..nodes.len() {
        if index % CANCEL_INTERVAL == 0 && cancel.load(Ordering::Acquire) {
            return Err(ProjectionFailure::Cancelled);
        }
        match nodes[index].kind {
            XmlNodeKind::Element => {
                if let Some(found) = namespaces.get(&nodes[index].range.start) {
                    nodes[index].namespace = found.element.clone();
                }
            }
            XmlNodeKind::Attribute => {
                if nodes[index].is_namespace {
                    continue;
                }
                let Some(parent) = nodes[index].parent else {
                    continue;
                };
                let qname_start = nodes[index]
                    .key_range
                    .as_ref()
                    .map(|range| range.start)
                    .unwrap_or(0);
                if let Some(found) = namespaces.get(&nodes[parent].range.start) {
                    nodes[index].namespace = found.attributes.get(&qname_start).cloned().flatten();
                }
            }
            _ => {}
        }
    }

    Ok(XmlProjection {
        nodes,
        root: Some(0),
        element_count,
        attribute_count,
        error: None,
    })
}

fn finish_element(nodes: &mut [XmlNode], index: usize) {
    let content = nodes[index]
        .children
        .iter()
        .filter(|child| nodes[**child].kind != XmlNodeKind::Attribute)
        .count();
    if nodes[index].self_closing {
        nodes[index].value = "/>".to_owned();
    } else if content == 0 {
        nodes[index].value = "element".to_owned();
    } else {
        nodes[index].value = format!("{content} nodes");
    }
}

#[derive(Default)]
struct ParentCounts {
    elements: HashMap<String, usize>,
    text: usize,
    cdata: usize,
    comments: usize,
    instructions: HashMap<String, usize>,
}

fn push_child(
    nodes: &mut Vec<XmlNode>,
    counts: &mut Vec<ParentCounts>,
    path_bytes: &mut usize,
    parent: usize,
    node: XmlNode,
) -> Result<usize, ProjectionFailure> {
    if nodes.len() >= XML_NODES {
        return Err(ProjectionFailure::TooManyNodes);
    }
    *path_bytes = path_bytes.saturating_add(node.path.len() + node.key.len());
    if *path_bytes > XML_PATH_BYTES {
        return Err(ProjectionFailure::PathsTooLarge);
    }
    let index = nodes.len();
    nodes.push(node);
    counts.push(ParentCounts::default());
    nodes[parent].children.push(index);
    Ok(index)
}

fn empty_projection() -> XmlProjection {
    XmlProjection {
        nodes: Vec::new(),
        root: None,
        element_count: 0,
        attribute_count: 0,
        error: None,
    }
}

fn error_projection(line: usize, column: usize, message: String) -> XmlProjection {
    XmlProjection {
        nodes: Vec::new(),
        root: None,
        element_count: 0,
        attribute_count: 0,
        error: Some(XmlParseFailure {
            line,
            column,
            message,
        }),
    }
}

/// The first DTD/entity token position, used for an observable denial without
/// asking either parser to resolve anything.
fn first_dtd_position(text: &str) -> Option<(usize, usize)> {
    for token in Tokenizer::from(text) {
        let Ok(token) = token else { return None };
        if matches!(
            token,
            Token::DtdStart { .. } | Token::EmptyDtd { .. } | Token::EntityDeclaration { .. }
        ) {
            return Some(line_column(text, token.span().start()));
        }
    }
    None
}

/// `xmlparser` yields byte offsets; the UI and other parse failures speak
/// 1-based rows and columns.
pub(crate) fn line_column(text: &str, offset: usize) -> (usize, usize) {
    let offset = offset.min(text.len());
    let mut line = 1;
    let mut column = 1;
    for ch in text[..offset].chars() {
        if ch == '\n' {
            line += 1;
            column = 1;
        } else {
            column += 1;
        }
    }
    (line, column)
}

fn preview(raw: &str) -> String {
    let mut escaped = String::with_capacity(raw.len());
    for ch in raw.chars() {
        match ch {
            '\n' => escaped.push('↵'),
            '\r' => escaped.push('␍'),
            '\t' => escaped.push('⇥'),
            ch if ch.is_control() => escaped.push(' '),
            ch => escaped.push(ch),
        }
    }
    truncated_chars(&escaped, DISPLAY_CHARS)
}

#[derive(Default)]
struct ElementNamespaces {
    element: Option<String>,
    attributes: HashMap<usize, Option<String>>,
}

/// roxmltree owns namespace resolution; this map only lends its result to the
/// token ranges that `xmlparser` preserved. It never reads a DTD.
fn collect_namespaces(
    document: &roxmltree::Document<'_>,
    cancel: &AtomicBool,
) -> Result<HashMap<usize, ElementNamespaces>, ProjectionFailure> {
    let mut result = HashMap::new();
    let mut pending = vec![document.root_element()];
    let mut visited = 0_usize;
    while let Some(node) = pending.pop() {
        if visited % CANCEL_INTERVAL == 0 && cancel.load(Ordering::Acquire) {
            return Err(ProjectionFailure::Cancelled);
        }
        visited += 1;
        let mut found = ElementNamespaces {
            element: node.tag_name().namespace().map(str::to_owned),
            attributes: HashMap::new(),
        };
        for attribute in node.attributes() {
            found.attributes.insert(
                attribute.range_qname().start,
                attribute.namespace().map(str::to_owned),
            );
        }
        result.insert(node.range().start, found);
        pending.extend(node.children().filter(|child| child.is_element()));
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cancel() -> AtomicBool {
        AtomicBool::new(false)
    }

    fn node<'a>(projection: &'a XmlProjection, path: &str) -> &'a XmlNode {
        projection
            .nodes()
            .iter()
            .find(|node| node.path == path)
            .unwrap_or_else(|| panic!("missing node {path}"))
    }

    const MIXED: &str = "<?xml version=\"1.0\"?>\r\n<?fixture owned?>\r\n<r xmlns=\"urn:root\" xmlns:p='urn:part' p:flag = 'old&amp;raw'>前<![CDATA[甲\r\n乙]]><!--注--><p:item/>后</r>\r\n";

    #[test]
    fn ordered_mixed_content_and_every_spelling_keep_exact_ranges() {
        let projection = project(MIXED, &cancel()).unwrap();
        let element = node(&projection, "/r");
        assert_eq!(element.namespace.as_deref(), Some("urn:root"));
        assert_eq!(&MIXED[element.range.clone()], MIXED.trim_start().trim_end());
        let attribute = node(&projection, "/r/@p:flag");
        assert_eq!(
            &MIXED[attribute.value_range.clone().unwrap()],
            "old&amp;raw"
        );
        assert_eq!(attribute.quote, Some('\''));
        assert_eq!(attribute.namespace.as_deref(), Some("urn:part"));
        let namespace = node(&projection, "/r/@xmlns:p");
        assert!(namespace.is_namespace);
        assert_eq!(&MIXED[namespace.value_range.clone().unwrap()], "urn:part");
        let item = node(&projection, "/r/p:item[1]");
        assert!(item.self_closing);
        assert_eq!(item.namespace.as_deref(), Some("urn:part"));
        assert_eq!(&MIXED[item.range.clone()], "<p:item/>");
        let cdata = node(&projection, "/r/cdata()[1]");
        assert_eq!(&MIXED[cdata.value_range.clone().unwrap()], "甲\r\n乙");
        assert_eq!(&MIXED[cdata.range.clone()], "<![CDATA[甲\r\n乙]]>");
        let comment = node(&projection, "/r/comment()[1]");
        assert_eq!(&MIXED[comment.value_range.clone().unwrap()], "注");
        let texts = projection
            .nodes()
            .iter()
            .filter(|node| node.kind == XmlNodeKind::Text && node.path.starts_with("/r/"))
            .map(|node| node.value.clone())
            .collect::<Vec<_>>();
        assert_eq!(texts, ["前", "后"]);
        assert_eq!(projection.element_count(), 2);
        assert_eq!(projection.attribute_count(), 3);
    }

    #[test]
    fn invalid_source_and_external_dtd_are_visible_and_never_resolved() {
        let invalid = project("<r><a></r>", &cancel()).unwrap();
        assert!(invalid.error().is_some());
        let dtd = project(
            "<!DOCTYPE r SYSTEM 'http://127.0.0.1:9/never-fetched'><r/>",
            &cancel(),
        );
        assert!(matches!(
            dtd,
            Err(ProjectionFailure::DtdDenied { line: 1, column: 1 })
        ));
        let entity = project(
            "<!DOCTYPE r [<!ENTITY external SYSTEM 'file:///never-read'>]><r>&external;</r>",
            &cancel(),
        );
        assert!(matches!(
            entity,
            Err(ProjectionFailure::DtdDenied { line: 1, column: 1 })
        ));
        let unknown = project("<r>&external;</r>", &cancel()).unwrap();
        assert!(unknown.error().is_some());
    }

    #[test]
    fn budgets_are_explicit_and_never_truncate_silently() {
        let cancelled = AtomicBool::new(true);
        assert!(matches!(
            project("<r/>", &cancelled),
            Err(ProjectionFailure::Cancelled)
        ));
        let large = format!("<r>{}</r>", "x".repeat(XML_INPUT_BYTES));
        assert!(matches!(
            project(&large, &cancel()),
            Err(ProjectionFailure::InputTooLarge)
        ));
        let many = format!("<r>{}</r>", "<a/>".repeat(XML_NODES + 2));
        assert!(matches!(
            project(&many, &cancel()),
            Err(ProjectionFailure::TooManyNodes)
        ));
    }

    #[test]
    fn deep_long_names_exceed_the_path_budget_without_touching_source() {
        let name = "n".repeat(2048);
        let mut text = String::new();
        for _ in 0..200 {
            text.push('<');
            text.push_str(&name);
            text.push('>');
        }
        for _ in 0..200 {
            text.push_str("</");
            text.push_str(&name);
            text.push('>');
        }
        assert!(text.len() <= XML_INPUT_BYTES);
        assert!(matches!(
            project(&text, &cancel()),
            Err(ProjectionFailure::PathsTooLarge)
        ));
    }
}
