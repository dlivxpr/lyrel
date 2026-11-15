//! JSON 家族的 GUI-free 无损结构投影。
//!
//! 源码是唯一权威。这里只把一份已规范化为 LF 的源码快照解析成节点索引：
//! 每个节点只保存显示文本（由源码切片得到）和源码范围，不保存第二份可变
//! 文档，也不把值转成 `serde_json::Value`。
//!
//! - JSON／JSONC 用 registry 的 `jsonc-parser`（MIT）做带偏移的 AST。对象
//!   属性按源码顺序保留，重复键各占一个节点；数字与字符串的显示直接取
//!   原源码切片，所以 `1.2300`、`1e400`、`\u4e2d` 和转义序列都不会被重写。
//! - JSONC 只放开注释与尾随逗号；JSON5 的其他扩展全部保持关闭。
//! - JSONL／NDJSON 逐行解析，每条记录保留 1-based 行号与行内范围；无效行
//!   成为带可见错误的记录行，而不是把整个文件投影成数组。
//!
//! 这个模块不依赖 GPUI，也不提供 mutation：格式编辑需要另行设计，不能把
//! 通用 Value 序列化回写冒充本地修改。

use std::collections::HashMap;
use std::ops::Range;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use jsonc_parser::ast::{ObjectPropName, Value};
use jsonc_parser::{CollectOptions, ParseOptions};

use super::super::structure_view::{Treelike, truncated_chars};

/// 结构投影能够读取的最大源码字节数。它和普通文本文件 4 MiB 的打开上限
/// 分开：更大的文件仍可作为源码打开与保存，只是不生成结构树。测试用一个
/// 超过此值的自生成文件验证失败可见且源码不丢。
pub(crate) const STRUCTURE_INPUT_BYTES: usize = 1024 * 1024;
/// 一份投影最多保留的节点数；超限时整份结构视图报预算错误，而不是静默
/// 截断树。源码访问不受影响。
pub(crate) const STRUCTURE_NODES: usize = 20_000;
/// JSONL／NDJSON 最多解析的记录数；空行不计。
pub(crate) const STRUCTURE_RECORDS: usize = 20_000;
/// 所有节点的完整路径字符串合计；长祖先键不能按子节点数量无界放大内存。
pub(crate) const STRUCTURE_PATH_BYTES: usize = 8 * 1024 * 1024;

/// 行内预览最多显示的字符数；完整原文仍可从节点范围读取。
const DISPLAY_CHARS: usize = 160;
/// 每处理这么多节点检查一次取消；解析库自身对 1 MiB 输入是有界的。
const CANCEL_INTERVAL: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum JsonFormat {
    Json,
    Jsonc,
    JsonLines,
}

impl JsonFormat {
    pub(crate) fn from_path(path: &Path) -> Option<Self> {
        let extension = path.extension()?.to_str()?.to_ascii_lowercase();
        match extension.as_str() {
            "json" => Some(Self::Json),
            "jsonc" => Some(Self::Jsonc),
            "jsonl" | "ndjson" => Some(Self::JsonLines),
            _ => None,
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Json => "JSON",
            Self::Jsonc => "JSONC",
            Self::JsonLines => "JSONL",
        }
    }

    pub(crate) fn allows_comments(self) -> bool {
        matches!(self, Self::Jsonc)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum JsonNodeKind {
    Object,
    Array,
    String,
    Number,
    Boolean,
    Null,
    Records,
    InvalidRecord,
}

/// 一个可显示的节点。`range` 指向值本身；有 `key_range` 时它指向对象属性
/// 名，二者一起给出该 occurrence 的源码定位。
#[derive(Clone, Debug)]
pub(crate) struct JsonNode {
    pub(crate) key: String,
    pub(crate) path: String,
    pub(crate) value: String,
    pub(crate) range: Range<usize>,
    pub(crate) key_range: Option<Range<usize>>,
    pub(crate) kind: JsonNodeKind,
    pub(crate) children: Vec<usize>,
    pub(crate) record_line: Option<usize>,
    pub(crate) error: Option<String>,
}

impl JsonNode {
    /// 选中该节点时在源码中定位的范围。属性取“键到值末尾”，记录取整行，
    /// 数组元素与根取值本身。
    pub(crate) fn detail_range(&self) -> Range<usize> {
        match &self.key_range {
            Some(key) => key.start..self.range.end.max(key.end),
            None => self.range.clone(),
        }
    }

    pub(crate) fn has_source_detail(&self) -> bool {
        self.kind != JsonNodeKind::Records && !self.detail_range().is_empty()
    }
}

#[derive(Clone, Debug)]
pub(crate) struct JsonParseFailure {
    pub(crate) line: usize,
    pub(crate) column: usize,
    pub(crate) message: String,
}

/// 已解析的一份源码快照；不持有源码本身，只引用其中的字节范围。
#[derive(Clone, Debug)]
pub(crate) struct JsonProjection {
    nodes: Vec<JsonNode>,
    root: Option<usize>,
    format: JsonFormat,
    error: Option<JsonParseFailure>,
    invalid_records: usize,
    record_count: usize,
}

impl JsonProjection {
    pub(crate) fn nodes(&self) -> &[JsonNode] {
        &self.nodes
    }

    pub(crate) fn root(&self) -> Option<usize> {
        self.root
    }

    pub(crate) fn format(&self) -> JsonFormat {
        self.format
    }

    pub(crate) fn error(&self) -> Option<&JsonParseFailure> {
        self.error.as_ref()
    }

    pub(crate) fn invalid_records(&self) -> usize {
        self.invalid_records
    }

    pub(crate) fn record_count(&self) -> usize {
        self.record_count
    }

    pub(crate) fn node_count(&self) -> usize {
        self.nodes.len()
    }
}

impl Treelike for JsonProjection {
    fn root(&self) -> Option<usize> {
        self.root
    }

    fn children(&self, node: usize) -> &[usize] {
        &self.nodes[node].children
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ProjectionFailure {
    Cancelled,
    InputTooLarge,
    TooManyNodes,
    TooManyRecords,
    PathsTooLarge,
}

/// Parse one snapshot into a bounded, source-linked projection.
///
/// `cancel` is checked before the parse, while walking the AST/line records and
/// after parsing. The parser itself is synchronous; its input is bounded by
/// [`STRUCTURE_INPUT_BYTES`], and a cancelled projection is never installed.
pub(crate) fn project(
    text: &str,
    format: JsonFormat,
    cancel: &AtomicBool,
) -> Result<JsonProjection, ProjectionFailure> {
    if cancel.load(Ordering::Acquire) {
        return Err(ProjectionFailure::Cancelled);
    }
    if text.len() > STRUCTURE_INPUT_BYTES {
        return Err(ProjectionFailure::InputTooLarge);
    }
    match format {
        JsonFormat::JsonLines => project_lines(text, cancel),
        JsonFormat::Json | JsonFormat::Jsonc => project_document(text, format, cancel),
    }
}

fn project_document(
    text: &str,
    format: JsonFormat,
    cancel: &AtomicBool,
) -> Result<JsonProjection, ProjectionFailure> {
    let mut nodes = Vec::new();
    let mut path_bytes = 0;
    let mut projection = JsonProjection {
        nodes: Vec::new(),
        root: None,
        format,
        error: None,
        invalid_records: 0,
        record_count: 0,
    };
    let parsed = jsonc_parser::parse_to_ast(text, &CollectOptions::default(), &options(format));
    match parsed {
        Ok(result) => match result.value {
            Some(value) => {
                let root = add_value(
                    &value,
                    0,
                    "$".to_owned(),
                    "$".to_owned(),
                    None,
                    text,
                    &mut nodes,
                    &mut path_bytes,
                    cancel,
                )?;
                projection.root = Some(root);
            }
            None => {
                projection.error = Some(JsonParseFailure {
                    line: 1,
                    column: 1,
                    message: "No JSON value".to_owned(),
                });
            }
        },
        Err(error) => {
            projection.error = Some(JsonParseFailure {
                line: error.line_display(),
                column: error.column_display(),
                message: error.kind().to_string(),
            });
        }
    }
    projection.nodes = nodes;
    Ok(projection)
}

fn project_lines(text: &str, cancel: &AtomicBool) -> Result<JsonProjection, ProjectionFailure> {
    let mut nodes = Vec::new();
    let mut path_bytes = 0;
    let root = push_node(
        &mut nodes,
        JsonNode {
            key: "JSONL".to_owned(),
            path: "JSONL".to_owned(),
            value: String::new(),
            range: 0..0,
            key_range: None,
            kind: JsonNodeKind::Records,
            children: Vec::new(),
            record_line: None,
            error: None,
        },
    )?;
    let mut children = Vec::new();
    let mut records = 0_usize;
    let mut invalid_records = 0_usize;
    let mut offset = 0_usize;
    let mut line_number = 1_usize;
    for raw in text.split_inclusive('\n') {
        if cancel.load(Ordering::Acquire) {
            return Err(ProjectionFailure::Cancelled);
        }
        let line = raw.strip_suffix('\n').unwrap_or(raw);
        let line = line.strip_suffix('\r').unwrap_or(line);
        let start = line.len() - line.trim_start().len();
        let end = line.trim_end().len();
        if end > start {
            records += 1;
            if records > STRUCTURE_RECORDS {
                return Err(ProjectionFailure::TooManyRecords);
            }
            let value_start = offset + start;
            let value_end = offset + end;
            let value_text = &text[value_start..value_end];
            match jsonc_parser::parse_to_ast(
                value_text,
                &CollectOptions::default(),
                &strict_options(),
            ) {
                Ok(result) => match result.value {
                    Some(value) => {
                        let record = add_value(
                            &value,
                            value_start,
                            format!("Line {line_number}"),
                            format!("line {line_number}"),
                            None,
                            text,
                            &mut nodes,
                            &mut path_bytes,
                            cancel,
                        )?;
                        nodes[record].record_line = Some(line_number);
                        children.push(record);
                    }
                    None => {
                        invalid_records += 1;
                        children.push(push_node(
                            &mut nodes,
                            JsonNode {
                                key: format!("Line {line_number}"),
                                path: format!("line {line_number}"),
                                value: "No JSON value".to_owned(),
                                range: value_start..value_end,
                                key_range: None,
                                kind: JsonNodeKind::InvalidRecord,
                                children: Vec::new(),
                                record_line: Some(line_number),
                                error: Some("No JSON value".to_owned()),
                            },
                        )?);
                    }
                },
                Err(error) => {
                    invalid_records += 1;
                    children.push(push_node(
                        &mut nodes,
                        JsonNode {
                            key: format!("Line {line_number}"),
                            path: format!("line {line_number}"),
                            value: error.kind().to_string(),
                            range: value_start..value_end,
                            key_range: None,
                            kind: JsonNodeKind::InvalidRecord,
                            children: Vec::new(),
                            record_line: Some(line_number),
                            error: Some(error.kind().to_string()),
                        },
                    )?);
                }
            }
        }
        offset += raw.len();
        line_number += 1;
    }
    nodes[root].children = children;
    nodes[root].value = format!("{records} records");
    Ok(JsonProjection {
        nodes,
        root: Some(root),
        format: JsonFormat::JsonLines,
        error: None,
        invalid_records,
        record_count: records,
    })
}

fn add_value(
    value: &Value<'_>,
    offset: usize,
    key: String,
    path: String,
    key_range: Option<Range<usize>>,
    text: &str,
    nodes: &mut Vec<JsonNode>,
    path_bytes: &mut usize,
    cancel: &AtomicBool,
) -> Result<usize, ProjectionFailure> {
    if nodes.len() % CANCEL_INTERVAL == 0 && cancel.load(Ordering::Acquire) {
        return Err(ProjectionFailure::Cancelled);
    }
    *path_bytes = path_bytes.saturating_add(path.len());
    if *path_bytes > STRUCTURE_PATH_BYTES {
        return Err(ProjectionFailure::PathsTooLarge);
    }
    let local_range = value_range(value);
    let range = offset + local_range.start..offset + local_range.end;
    let (kind, preview) = describe(value, text, &range);
    let index = push_node(
        nodes,
        JsonNode {
            key,
            path,
            value: preview,
            range,
            key_range,
            kind,
            children: Vec::new(),
            record_line: None,
            error: None,
        },
    )?;
    let mut children = Vec::new();
    match value {
        Value::Object(object) => {
            let mut occurrences: HashMap<&str, usize> = HashMap::new();
            for property in &object.properties {
                let name = property.name.as_str();
                let occurrence = occurrences.entry(name).or_default();
                let raw_key = match &property.name {
                    ObjectPropName::String(literal) => absolute(literal.range, offset),
                    ObjectPropName::Word(literal) => absolute(literal.range, offset),
                };
                let raw_key_text = &text[raw_key.clone()];
                let display = sanitized(name);
                let path = match *occurrence {
                    0 => format!("{}[{raw_key_text}]", nodes[index].path),
                    count => format!("{}[{raw_key_text}] #{}", nodes[index].path, count + 1),
                };
                *occurrence += 1;
                children.push(add_value(
                    &property.value,
                    offset,
                    display,
                    path,
                    Some(raw_key),
                    text,
                    nodes,
                    path_bytes,
                    cancel,
                )?);
            }
        }
        Value::Array(array) => {
            for (element_index, element) in array.elements.iter().enumerate() {
                children.push(add_value(
                    element,
                    offset,
                    format!("[{element_index}]"),
                    format!("{}[{element_index}]", nodes[index].path),
                    None,
                    text,
                    nodes,
                    path_bytes,
                    cancel,
                )?);
            }
        }
        Value::StringLit(_)
        | Value::NumberLit(_)
        | Value::BooleanLit(_)
        | Value::NullKeyword(_) => {}
    }
    nodes[index].children = children;
    Ok(index)
}

fn push_node(nodes: &mut Vec<JsonNode>, node: JsonNode) -> Result<usize, ProjectionFailure> {
    if nodes.len() >= STRUCTURE_NODES {
        return Err(ProjectionFailure::TooManyNodes);
    }
    let index = nodes.len();
    nodes.push(node);
    Ok(index)
}

fn describe(value: &Value<'_>, text: &str, range: &Range<usize>) -> (JsonNodeKind, String) {
    match value {
        Value::Object(object) => (
            JsonNodeKind::Object,
            format!("{{ {} keys }}", object.properties.len()),
        ),
        Value::Array(array) => (
            JsonNodeKind::Array,
            format!("[ {} items ]", array.elements.len()),
        ),
        Value::StringLit(_) => (
            JsonNodeKind::String,
            truncated_chars(&text[range.clone()], DISPLAY_CHARS),
        ),
        Value::NumberLit(literal) => (JsonNodeKind::Number, literal.value.to_owned()),
        Value::BooleanLit(literal) => (
            JsonNodeKind::Boolean,
            if literal.value { "true" } else { "false" }.to_owned(),
        ),
        Value::NullKeyword(_) => (JsonNodeKind::Null, "null".to_owned()),
    }
}

fn value_range(value: &Value<'_>) -> Range<usize> {
    match value {
        Value::Object(node) => node.range.start..node.range.end,
        Value::Array(node) => node.range.start..node.range.end,
        Value::StringLit(node) => node.range.start..node.range.end,
        Value::NumberLit(node) => node.range.start..node.range.end,
        Value::BooleanLit(node) => node.range.start..node.range.end,
        Value::NullKeyword(node) => node.range.start..node.range.end,
    }
}

fn absolute(range: jsonc_parser::common::Range, offset: usize) -> Range<usize> {
    offset + range.start..offset + range.end
}

fn sanitized(value: &str) -> String {
    let text: String = value
        .chars()
        .map(|ch| match ch.is_control() {
            true => ' ',
            false => ch,
        })
        .collect();
    truncated_chars(&text, DISPLAY_CHARS)
}

/// JSON 只接受严格语法；JSONC 只额外接受注释和尾随逗号，不打开 JSON5
/// 的宽松名字、单引号、十六进制、非有限数或扩展转义。
fn options(format: JsonFormat) -> ParseOptions {
    let jsonc = format.allows_comments();
    ParseOptions {
        allow_comments: jsonc,
        allow_loose_object_property_names: false,
        allow_trailing_commas: jsonc,
        allow_missing_commas: false,
        allow_single_quoted_strings: false,
        allow_hexadecimal_numbers: false,
        allow_unary_plus_numbers: false,
        allow_bare_decimal_point_numbers: false,
        allow_non_finite_numbers: false,
        allow_extended_string_escapes: false,
    }
}

fn strict_options() -> ParseOptions {
    options(JsonFormat::Json)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cancel() -> AtomicBool {
        AtomicBool::new(false)
    }

    fn node<'a>(projection: &'a JsonProjection, path: &str) -> &'a JsonNode {
        projection
            .nodes()
            .iter()
            .find(|node| node.path == path)
            .unwrap_or_else(|| panic!("missing node {path}"))
    }

    #[test]
    fn comments_duplicate_keys_escapes_and_exact_numbers_survive_in_source_ranges() {
        let text = "{\n  // duplicate keys stay separate\n  \"a\": \"first\",\n  \"a\": \"second\\u4e2d\",\n  \"n\": 12345678901234567890.00,\n  \"escaped\": \"line\\nbreak\",\n}";
        let projection = project(text, JsonFormat::Jsonc, &cancel()).unwrap();
        let first = node(&projection, "$[\"a\"]");
        let second = node(&projection, "$[\"a\"] #2");
        assert_eq!(&text[first.range.clone()], "\"first\"");
        assert_eq!(&text[second.range.clone()], "\"second\\u4e2d\"");
        assert_ne!(first.detail_range(), second.detail_range());
        assert_eq!(
            node(&projection, "$[\"n\"]").value,
            "12345678901234567890.00"
        );
        assert_eq!(
            node(&projection, "$[\"escaped\"]").value,
            "\"line\\nbreak\""
        );
        let strict = project("{\"a\": 1,}", JsonFormat::Json, &cancel()).unwrap();
        assert!(strict.error().is_some());
        let jsonc = project("{\"a\": 1,}", JsonFormat::Jsonc, &cancel()).unwrap();
        assert!(jsonc.error().is_none());
    }

    #[test]
    fn jsonl_keeps_line_identity_and_invalid_lines_without_becoming_an_array() {
        let text = "{\"id\": 1}\n\n{\"id\": 2,}\n{\"id\": 3}\n";
        let projection = project(text, JsonFormat::JsonLines, &cancel()).unwrap();
        assert_eq!(
            projection.root().map(|root| projection.nodes()[root].kind),
            Some(JsonNodeKind::Records)
        );
        assert_eq!(projection.record_count(), 3);
        assert_eq!(projection.invalid_records(), 1);
        let records = projection
            .nodes()
            .iter()
            .filter(|node| node.record_line.is_some() && node.kind != JsonNodeKind::InvalidRecord)
            .collect::<Vec<_>>();
        assert_eq!(
            records
                .iter()
                .map(|node| node.record_line)
                .collect::<Vec<_>>(),
            [Some(1), Some(4)]
        );
        assert_eq!(&text[records[1].range.clone()], "{\"id\": 3}");
        let invalid = node(&projection, "line 3");
        assert_eq!(invalid.kind, JsonNodeKind::InvalidRecord);
        assert_eq!(&text[invalid.range.clone()], "{\"id\": 2,}");
        assert!(invalid.error.is_some());
    }

    #[test]
    fn cancellation_and_budgets_are_explicit_and_never_truncate_silently() {
        let cancelled = AtomicBool::new(true);
        assert!(matches!(
            project("{}", JsonFormat::Json, &cancelled),
            Err(ProjectionFailure::Cancelled)
        ));
        let large = format!("[{}]", "0,".repeat(STRUCTURE_INPUT_BYTES));
        assert!(matches!(
            project(&large, JsonFormat::Json, &cancel()),
            Err(ProjectionFailure::InputTooLarge)
        ));
        let nodes = format!(
            "[{}]",
            (0..STRUCTURE_NODES + 2)
                .map(|index| index.to_string())
                .collect::<Vec<_>>()
                .join(",")
        );
        assert!(matches!(
            project(&nodes, JsonFormat::Json, &cancel()),
            Err(ProjectionFailure::TooManyNodes)
        ));
    }

    #[test]
    fn long_ancestor_paths_refuse_structure_without_changing_source() {
        let text = format!(
            "{{\"{}\":[{}]}}",
            "x".repeat(65_536),
            "0,".repeat(255) + "0"
        );
        assert!(matches!(
            project(&text, JsonFormat::Json, &cancel()),
            Err(ProjectionFailure::PathsTooLarge)
        ));
        let recovered = project(
            "{\"kept\":12345678901234567890}",
            JsonFormat::Json,
            &cancel(),
        )
        .unwrap();
        assert_eq!(
            node(&recovered, "$[\"kept\"]").value,
            "12345678901234567890"
        );
    }
}
