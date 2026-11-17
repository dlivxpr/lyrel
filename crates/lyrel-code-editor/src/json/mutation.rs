//! #290 JSON／JSONC 的局部源编辑规划。
//!
//! 命令只针对当前投影里的一个确切 occurrence：目标用
//! `(path, range, key_range)` 重新解析，可见行索引或裸属性路径都不是身份。
//! 规划器在当前 LF 源码上重新取得 token 结构，算出最小的替换清单；用户
//! 输入的新值片段由同一个 `jsonc-parser` 校验为单个 JSON 值。绝不把整篇
//! 文档转成 Value 再序列化，也不做全 source 格式化。
//!
//! 应用由调用方交给共享 `Document` 的 `replace_ranges`：一次命令是一条
//! 普通 history 记录，撤销/重做与其他源码编辑共用同一权威。
//!
//! JSONL／NDJSON 的编辑只改选中的物理行：嵌套值／键／数组沿用同一
//! occurrence 身份，插入记录写入一整条新行，删除记录只移除该行；其余行、
//! 空行与混合行尾的 representation 不受影响。规划器仍以当前投影的
//! `(path, range, key_range)` 重新解析目标，并按 revision 拒绝过期命令。

use std::ops::Range;

use jsonc_parser::tokens::{Token, TokenAndRange};
use jsonc_parser::{CollectOptions, CommentCollectionStrategy};

use super::projection::{JsonFormat, JsonNode, JsonNodeKind, JsonProjection, options, value_range};
use crate::tree::Treelike;

/// 目标节点的稳定身份。`range` 是值本身的 LF byte 范围；对象属性另有
/// 原始键 token 的范围。重新解析时必须三者都匹配，不能只按属性名猜测。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeTarget {
    pub path: String,
    pub range: Range<usize>,
    pub key_range: Option<Range<usize>>,
}

impl NodeTarget {
    pub fn of(node: &super::projection::JsonNode) -> Self {
        Self {
            path: node.path.clone(),
            range: node.range.clone(),
            key_range: node.key_range.clone(),
        }
    }
}

#[derive(Clone, Debug)]
pub enum Mutation {
    /// 用单个 JSON 值片段替换节点的值。
    SetValue { target: NodeTarget, value: String },
    /// 只替换选中 occurrence 的属性名。
    RenameKey { target: NodeTarget, key: String },
    /// 在选中 object／array 的末尾加入成员；object 需要 key。
    AddMember {
        container: NodeTarget,
        key: Option<String>,
        value: String,
    },
    /// 删除选中的 object 成员或 array 元素，连同它必要的分隔逗号。
    RemoveMember { target: NodeTarget },
    /// 在选中记录（或文件末尾）之后插入一条新的单行记录。
    AddRecord {
        after: Option<NodeTarget>,
        value: String,
    },
    /// 删除选中记录所在的整个物理行，保留其他行与空行。
    RemoveRecord { target: NodeTarget },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MutationError {
    /// 文档 revision 已变化，或目标 occurrence 已不存在。
    Stale,
    /// 选中的节点不能执行这个命令（根、记录、非容器等）。
    Unsupported,
    /// 用户输入不是恰好一个 JSON 值。
    InvalidValue(String),
    /// JSONL／NDJSON 的编辑必须保持一条记录占一行。
    RecordNotSingleLine,
    /// 规划后的源码无法解析；拒绝写入而不是留下损坏文件。
    ResultInvalid {
        line: usize,
        column: usize,
        message: String,
    },
}

/// 规划一次局部修改，返回按源码顺序排列、互不重叠的替换清单。
///
/// 空清单表示命令是 no-op（例如值没有变化）。任何失败都发生在写入之前。
pub fn plan(
    text: &str,
    projection: &JsonProjection,
    format: JsonFormat,
    mutation: &Mutation,
) -> Result<Vec<(Range<usize>, String)>, MutationError> {
    match format {
        JsonFormat::JsonLines => plan_records(text, projection, mutation),
        JsonFormat::Json | JsonFormat::Jsonc => plan_document(text, projection, format, mutation),
    }
}

fn plan_document(
    text: &str,
    projection: &JsonProjection,
    format: JsonFormat,
    mutation: &Mutation,
) -> Result<Vec<(Range<usize>, String)>, MutationError> {
    let tokens = tokens(text, format)?;
    let replacements = match mutation {
        Mutation::SetValue { target, value } => {
            let index = resolve(projection, target)?;
            let node = &projection.nodes()[index];
            if !matches!(
                node.kind,
                JsonNodeKind::String
                    | JsonNodeKind::Number
                    | JsonNodeKind::Boolean
                    | JsonNodeKind::Null
            ) || node.range.is_empty()
            {
                return Err(MutationError::Unsupported);
            }
            let value = validate_value(value, format)?;
            if text.get(node.range.clone()) == Some(value.as_str()) {
                Vec::new()
            } else {
                vec![(node.range.clone(), value)]
            }
        }
        Mutation::RenameKey { target, key } => {
            let index = resolve(projection, target)?;
            let node = &projection.nodes()[index];
            let key_range = node.key_range.clone().ok_or(MutationError::Unsupported)?;
            if node.key_text.as_deref() == Some(key.as_str()) {
                Vec::new()
            } else {
                let quoted = quote_string(key);
                validate_key(&quoted, format)?;
                vec![(key_range, quoted)]
            }
        }
        Mutation::AddMember {
            container,
            key,
            value,
        } => {
            let index = resolve(projection, container)?;
            let node = &projection.nodes()[index];
            let value = validate_value(value, format)?;
            let (member, last_start) = match node.kind {
                JsonNodeKind::Object => {
                    let key = key.as_deref().ok_or(MutationError::Unsupported)?;
                    let quoted = quote_string(key);
                    validate_key(&quoted, format)?;
                    let colon = colon_style(text, projection, index);
                    let last_start = projection
                        .children(index)
                        .last()
                        .and_then(|child| projection.nodes()[*child].key_range.as_ref())
                        .map(|range| range.start);
                    (format!("{quoted}{colon}{value}"), last_start)
                }
                JsonNodeKind::Array => {
                    let last_start = projection
                        .children(index)
                        .last()
                        .map(|child| projection.nodes()[*child].range.start);
                    (value, last_start)
                }
                _ => return Err(MutationError::Unsupported),
            };
            insert_member(text, &tokens, projection, index, &member, last_start)?
        }
        Mutation::RemoveMember { target } => {
            let index = resolve(projection, target)?;
            remove_member(text, &tokens, projection, index)?
        }
        Mutation::AddRecord { .. } | Mutation::RemoveRecord { .. } => {
            return Err(MutationError::Unsupported);
        }
    };
    if !replacements.is_empty() {
        verify_result(text, &replacements, format)?;
    }
    Ok(replacements)
}

/// JSONL／NDJSON 的编辑以一条物理记录为边界：嵌套命令只解析目标行并
/// 重新解析修改后的该行；邻近的无效记录不参与校验，也不会被改写。
fn plan_records(
    text: &str,
    projection: &JsonProjection,
    mutation: &Mutation,
) -> Result<Vec<(Range<usize>, String)>, MutationError> {
    match mutation {
        Mutation::AddRecord { after, value } => plan_add_record(text, projection, after, value),
        Mutation::RemoveRecord { target } => plan_remove_record(text, projection, target),
        Mutation::SetValue { target, value } => {
            let index = resolve(projection, target)?;
            let record = projection
                .record_ancestor(index)
                .ok_or(MutationError::Unsupported)?;
            let node = &projection.nodes()[index];
            let record_root = index == record;
            if !record_root
                && (!matches!(
                    node.kind,
                    JsonNodeKind::String
                        | JsonNodeKind::Number
                        | JsonNodeKind::Boolean
                        | JsonNodeKind::Null
                ) || node.range.is_empty())
            {
                return Err(MutationError::Unsupported);
            }
            let value = validate_record_value(value)?;
            let replacements = match text.get(node.range.clone()) == Some(value.as_str()) {
                true => Vec::new(),
                false => vec![(node.range.clone(), value)],
            };
            verify_record_edit(text, projection, record, &replacements)?;
            Ok(replacements)
        }
        Mutation::RenameKey { target, key } => {
            let index = resolve(projection, target)?;
            let record = projection
                .record_ancestor(index)
                .ok_or(MutationError::Unsupported)?;
            let node = &projection.nodes()[index];
            let key_range = node.key_range.clone().ok_or(MutationError::Unsupported)?;
            let replacements = match node.key_text.as_deref() == Some(key.as_str()) {
                true => Vec::new(),
                false => {
                    let quoted = quote_string(key);
                    validate_key(&quoted, JsonFormat::Json)?;
                    vec![(key_range, quoted)]
                }
            };
            verify_record_edit(text, projection, record, &replacements)?;
            Ok(replacements)
        }
        Mutation::AddMember {
            container,
            key,
            value,
        } => {
            let index = resolve(projection, container)?;
            let record = projection
                .record_ancestor(index)
                .ok_or(MutationError::Unsupported)?;
            let record_node = &projection.nodes()[record];
            let tokens = record_tokens(text, record_node)?;
            let node = &projection.nodes()[index];
            let value = validate_record_value(value)?;
            let (member, last_start) = match node.kind {
                JsonNodeKind::Object => {
                    let key = key.as_deref().ok_or(MutationError::Unsupported)?;
                    let quoted = quote_string(key);
                    validate_key(&quoted, JsonFormat::Json)?;
                    let colon = colon_style(text, projection, index);
                    let last_start = projection
                        .children(index)
                        .last()
                        .and_then(|child| projection.nodes()[*child].key_range.as_ref())
                        .map(|range| range.start);
                    (format!("{quoted}{colon}{value}"), last_start)
                }
                JsonNodeKind::Array => {
                    let last_start = projection
                        .children(index)
                        .last()
                        .map(|child| projection.nodes()[*child].range.start);
                    (value, last_start)
                }
                _ => return Err(MutationError::Unsupported),
            };
            let replacements =
                insert_member(text, &tokens, projection, index, &member, last_start)?;
            verify_record_edit(text, projection, record, &replacements)?;
            Ok(replacements)
        }
        Mutation::RemoveMember { target } => {
            let index = resolve(projection, target)?;
            let record = projection
                .record_ancestor(index)
                .ok_or(MutationError::Unsupported)?;
            let record_node = &projection.nodes()[record];
            let tokens = record_tokens(text, record_node)?;
            let replacements = remove_member(text, &tokens, projection, index)?;
            verify_record_edit(text, projection, record, &replacements)?;
            Ok(replacements)
        }
    }
}

fn plan_add_record(
    text: &str,
    projection: &JsonProjection,
    after: &Option<NodeTarget>,
    value: &str,
) -> Result<Vec<(Range<usize>, String)>, MutationError> {
    let value = validate_record_value(value)?;
    let (at, fragment) = match after {
        Some(target) => {
            let index = resolve(projection, target)?;
            let record = projection
                .record_ancestor(index)
                .ok_or(MutationError::Unsupported)?;
            let node = &projection.nodes()[record];
            let line = record_line_range(text, node.range.start);
            let indent = line_indent(text, node.range.start);
            match line_ends_with_newline(text, &line) {
                true => (line.end, format!("{indent}{value}\n")),
                false => (line.end, format!("\n{indent}{value}")),
            }
        }
        None => match text.is_empty() {
            true => (0, value),
            false => match text.ends_with('\n') {
                true => (text.len(), format!("{value}\n")),
                false => (text.len(), format!("\n{value}")),
            },
        },
    };
    // The insertion point is always a physical line boundary, and the new
    // record is itself exactly one line. Neighboring bytes are untouched.
    if at > 0 && !text[..at].ends_with('\n') && !fragment.starts_with('\n') {
        return Err(MutationError::Unsupported);
    }
    Ok(vec![(at..at, fragment)])
}

fn plan_remove_record(
    text: &str,
    projection: &JsonProjection,
    target: &NodeTarget,
) -> Result<Vec<(Range<usize>, String)>, MutationError> {
    let index = resolve(projection, target)?;
    let record = projection
        .record_ancestor(index)
        .ok_or(MutationError::Unsupported)?;
    let node = &projection.nodes()[record];
    let line = record_line_range(text, node.range.start);
    if line.is_empty() {
        return Err(MutationError::Unsupported);
    }
    Ok(vec![(line, String::new())])
}

fn record_line_range(text: &str, record_start: usize) -> Range<usize> {
    let start = text[..record_start].rfind('\n').map_or(0, |at| at + 1);
    let end = text[record_start..]
        .find('\n')
        .map_or(text.len(), |at| record_start + at + 1);
    start..end
}

fn line_ends_with_newline(text: &str, line: &Range<usize>) -> bool {
    line.end > line.start && text.as_bytes().get(line.end - 1) == Some(&b'\n')
}

/// 解析目标记录自己的 token；offset 映射回整份 LF 源码。相邻记录即使
/// 无效也不会让这里的解析失败，因为它们不在输入范围内。
fn record_tokens<'a>(
    text: &'a str,
    record: &JsonNode,
) -> Result<Vec<TokenAndRange<'a>>, MutationError> {
    let body = text.get(record.range.clone()).ok_or(MutationError::Stale)?;
    let parsed = jsonc_parser::parse_to_ast(
        body,
        &CollectOptions {
            comments: CommentCollectionStrategy::Off,
            tokens: true,
        },
        &options(JsonFormat::Json),
    )
    .map_err(|_| MutationError::Stale)?;
    let mut tokens = parsed.tokens.ok_or(MutationError::Stale)?;
    for token in &mut tokens {
        token.range.start += record.range.start;
        token.range.end += record.range.start;
    }
    Ok(tokens)
}

fn validate_record_value(fragment: &str) -> Result<String, MutationError> {
    if fragment.contains(['\r', '\n']) {
        return Err(MutationError::RecordNotSingleLine);
    }
    validate_value(fragment, JsonFormat::Json)
}

/// 验证一次记录内修改：所有替换都在该行内，修改后的整行仍是恰好一个
/// JSON 值。不会解析或要求邻近记录有效。
fn verify_record_edit(
    text: &str,
    projection: &JsonProjection,
    record: usize,
    replacements: &[(Range<usize>, String)],
) -> Result<(), MutationError> {
    if replacements.is_empty() {
        return Ok(());
    }
    let node = &projection.nodes()[record];
    let line = record_line_range(text, node.range.start);
    if replacements.iter().any(|(range, inserted)| {
        range.start < line.start
            || range.end > line.end
            || inserted.contains(['\r', '\n'])
            || (inserted.is_empty()
                && text
                    .get(range.clone())
                    .is_some_and(|removed| removed.ends_with('\n')))
    }) {
        return Err(MutationError::Unsupported);
    }
    let result = apply_replacements(text, replacements);
    let line_end = result[line.start..]
        .find('\n')
        .map_or(result.len(), |at| line.start + at);
    let content = result[line.start..line_end].trim();
    if content.is_empty() {
        return Err(MutationError::ResultInvalid {
            line: node.record_line.unwrap_or(1),
            column: 1,
            message: "The record would become empty".to_owned(),
        });
    }
    validate_value(content, JsonFormat::Json).map_err(|error| match error {
        MutationError::InvalidValue(message) => MutationError::ResultInvalid {
            line: node.record_line.unwrap_or(1),
            column: 1,
            message,
        },
        other => other,
    })?;
    Ok(())
}

fn resolve(projection: &JsonProjection, target: &NodeTarget) -> Result<usize, MutationError> {
    projection
        .nodes()
        .iter()
        .position(|node| {
            node.path == target.path
                && node.range == target.range
                && node.key_range == target.key_range
        })
        .ok_or(MutationError::Stale)
}

fn tokens(text: &str, format: JsonFormat) -> Result<Vec<TokenAndRange<'_>>, MutationError> {
    let parsed = jsonc_parser::parse_to_ast(
        text,
        &CollectOptions {
            comments: CommentCollectionStrategy::Off,
            tokens: true,
        },
        &options(format),
    )
    .map_err(|_| MutationError::Stale)?;
    parsed.tokens.ok_or(MutationError::Stale)
}

/// 单个 JSON 值片段：去掉首尾空白后必须恰好覆盖解析出的值，不能带注释、
/// 尾随内容或多个根值。数字与转义按用户输入原样保留。
fn validate_value(fragment: &str, format: JsonFormat) -> Result<String, MutationError> {
    let trimmed = fragment.trim();
    if trimmed.is_empty() {
        return Err(MutationError::InvalidValue("No JSON value".to_owned()));
    }
    let parsed = jsonc_parser::parse_to_ast(trimmed, &CollectOptions::default(), &options(format))
        .map_err(|error| MutationError::InvalidValue(error.kind().to_string()))?;
    let value = parsed
        .value
        .ok_or_else(|| MutationError::InvalidValue("No JSON value".to_owned()))?;
    let range = value_range(&value);
    if range.start != 0 || range.end != trimmed.len() {
        return Err(MutationError::InvalidValue(
            "Only one JSON value is allowed".to_owned(),
        ));
    }
    Ok(trimmed.to_owned())
}

/// 新键必须是当前格式允许的字符串 token；生成值由同一解析器复核。
fn validate_key(quoted: &str, format: JsonFormat) -> Result<(), MutationError> {
    let probe = format!("{{{quoted}: null}}");
    match jsonc_parser::parse_to_ast(&probe, &CollectOptions::default(), &options(format)) {
        Ok(parsed) if parsed.value.is_some() => Ok(()),
        _ => Err(MutationError::InvalidValue("Invalid key".to_owned())),
    }
}

/// JSON 字符串转义。新键由这里生成，再经解析器复核；不借用 Value 序列化。
fn quote_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            ch if (ch as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", ch as u32));
            }
            ch => out.push(ch),
        }
    }
    out.push('"');
    out
}

fn colon_style(text: &str, projection: &JsonProjection, object: usize) -> &'static str {
    projection
        .children(object)
        .last()
        .and_then(|child| {
            let node = &projection.nodes()[*child];
            let key = node.key_range.as_ref()?;
            Some(&text[key.end..node.range.start])
        })
        .filter(|between| !between.contains(' ') && !between.contains('\t'))
        .map_or(": ", |_| ":")
}

struct ContainerInfo {
    open: usize,
    close: usize,
    commas: Vec<Range<usize>>,
    multiline: bool,
}

/// 容器自己的 token 结构。只统计该容器层级（depth 1）的逗号，嵌套值里的
/// 逗号不影响分隔判断。注释不参与 token，因此不会被删除或移动。
fn container_info(
    text: &str,
    tokens: &[TokenAndRange<'_>],
    range: &Range<usize>,
) -> Option<ContainerInfo> {
    let mut depth = 0_usize;
    let mut open = None;
    let mut close = None;
    let mut commas = Vec::new();
    for token in tokens {
        if token.range.start < range.start || token.range.end > range.end {
            continue;
        }
        match &token.token {
            Token::OpenBrace | Token::OpenBracket => {
                if open.is_none() {
                    open = Some(token.range.end);
                }
                depth += 1;
            }
            Token::CloseBrace | Token::CloseBracket => {
                if depth == 1 {
                    close = Some(token.range.start);
                }
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    break;
                }
            }
            Token::Comma if depth == 1 => commas.push(token.range.start..token.range.end),
            _ => {}
        }
    }
    Some(ContainerInfo {
        open: open?,
        close: close?,
        commas,
        multiline: text
            .get(range.clone())
            .is_some_and(|text| text.contains('\n')),
    })
}

fn insert_member(
    text: &str,
    tokens: &[TokenAndRange<'_>],
    projection: &JsonProjection,
    index: usize,
    member: &str,
    last_child_start: Option<usize>,
) -> Result<Vec<(Range<usize>, String)>, MutationError> {
    let node = &projection.nodes()[index];
    let info = container_info(text, tokens, &node.range).ok_or(MutationError::Unsupported)?;
    let unit = indent_unit(text, &node.range);
    let child_indent = child_indent(text, &info, last_child_start, unit);
    let insertion = if projection.children(index).is_empty() {
        let fragment = match info.multiline {
            true => format!("\n{child_indent}{member}"),
            false => member.to_owned(),
        };
        (info.open, fragment)
    } else {
        let last = *projection
            .children(index)
            .last()
            .ok_or(MutationError::Unsupported)?;
        let last_end = projection.nodes()[last].range.end;
        let trailing = info
            .commas
            .iter()
            .find(|comma| comma.start >= last_end)
            .cloned();
        match trailing {
            Some(comma) => {
                let fragment = match info.multiline {
                    true => format!("\n{child_indent}{member}"),
                    false => format!(" {member}"),
                };
                (comma.end, fragment)
            }
            None => {
                let fragment = match info.multiline {
                    true => format!(",\n{child_indent}{member}"),
                    false => format!(", {member}"),
                };
                (last_end, fragment)
            }
        }
    };
    Ok(vec![(insertion.0..insertion.0, insertion.1)])
}

fn remove_member(
    text: &str,
    tokens: &[TokenAndRange<'_>],
    projection: &JsonProjection,
    index: usize,
) -> Result<Vec<(Range<usize>, String)>, MutationError> {
    let node = &projection.nodes()[index];
    let parent = node.parent.ok_or(MutationError::Unsupported)?;
    let parent_node = &projection.nodes()[parent];
    let info =
        container_info(text, tokens, &parent_node.range).ok_or(MutationError::Unsupported)?;
    let start = node
        .key_range
        .as_ref()
        .map_or(node.range.start, |key| key.start);
    let end = node.range.end;
    if start >= end {
        return Err(MutationError::Unsupported);
    }
    let following = info.commas.iter().find(|comma| comma.start >= end).cloned();
    let delete = match following {
        Some(comma) => start..comma.end,
        None => match info
            .commas
            .iter()
            .rev()
            .find(|comma| comma.end <= start)
            .cloned()
        {
            Some(comma) => comma.start..end,
            None => start..end,
        },
    };
    Ok(vec![(clean_line(text, delete), String::new())])
}

/// 删除整行时把行首缩进与行尾换行一起去掉；只删行内片段时不碰换行，
/// 避免把下一行或闭合括号挪到奇怪的位置。
fn clean_line(text: &str, range: Range<usize>) -> Range<usize> {
    let line_start = text[..range.start].rfind('\n').map_or(0, |at| at + 1);
    let leading_is_indent = line_start <= range.start
        && text[line_start..range.start]
            .chars()
            .all(|ch| ch == ' ' || ch == '\t');
    if !leading_is_indent {
        return range;
    }
    let rest = &text[range.end..];
    let line_end = rest.find('\n').unwrap_or(rest.len());
    if rest[..line_end]
        .chars()
        .all(|ch| ch == ' ' || ch == '\t' || ch == '\r')
    {
        let mut end = range.end + line_end;
        if text[end..].starts_with('\n') {
            end += 1;
        }
        line_start..end
    } else {
        line_start..range.end
    }
}

fn line_indent(text: &str, at: usize) -> String {
    let start = text[..at].rfind('\n').map_or(0, |index| index + 1);
    text[start..at]
        .chars()
        .take_while(|ch| *ch == ' ' || *ch == '\t')
        .collect()
}

fn indent_unit(text: &str, range: &Range<usize>) -> &'static str {
    match text
        .get(range.clone())
        .is_some_and(|text| text.lines().any(|line| line.starts_with('\t')))
    {
        true => "\t",
        false => "  ",
    }
}

fn child_indent(
    text: &str,
    info: &ContainerInfo,
    last_child_start: Option<usize>,
    unit: &str,
) -> String {
    let close_indent = line_indent(text, info.close);
    if let Some(start) = last_child_start {
        let child = line_indent(text, start);
        if child.len() > close_indent.len() {
            return child;
        }
    }
    format!("{close_indent}{unit}")
}

/// 写入前用同一解析器复核规划结果：任何会破坏文档的替换都拒绝。
fn verify_result(
    text: &str,
    replacements: &[(Range<usize>, String)],
    format: JsonFormat,
) -> Result<(), MutationError> {
    for (range, _) in replacements {
        if range.start > range.end
            || range.end > text.len()
            || !text.is_char_boundary(range.start)
            || !text.is_char_boundary(range.end)
        {
            return Err(MutationError::Stale);
        }
    }
    let result = apply_replacements(text, replacements);
    match jsonc_parser::parse_to_ast(&result, &CollectOptions::default(), &options(format)) {
        Ok(parsed) if parsed.value.is_some() => Ok(()),
        Ok(_) => Err(MutationError::ResultInvalid {
            line: 1,
            column: 1,
            message: "No JSON value".to_owned(),
        }),
        Err(error) => Err(MutationError::ResultInvalid {
            line: error.line_display(),
            column: error.column_display(),
            message: error.kind().to_string(),
        }),
    }
}

/// 应用替换清单，供 GUI-free 测试和 verify 复用。调用方先验证范围。
fn apply_replacements(text: &str, replacements: &[(Range<usize>, String)]) -> String {
    let mut result = text.to_owned();
    let mut ordered: Vec<_> = replacements.to_vec();
    ordered.sort_by_key(|(range, _)| std::cmp::Reverse(range.start));
    for (range, inserted) in ordered {
        result.replace_range(range, &inserted);
    }
    result
}

#[cfg(test)]
fn apply(text: &str, replacements: &[(Range<usize>, String)]) -> String {
    apply_replacements(text, replacements)
}

#[cfg(test)]
mod tests {
    use super::super::projection::project;
    use super::*;
    use std::sync::atomic::AtomicBool;

    fn cancel() -> AtomicBool {
        AtomicBool::new(false)
    }

    fn build_projection(text: &str, format: JsonFormat) -> JsonProjection {
        project(text, format, &cancel()).expect("fixture projects")
    }

    fn target(projection: &JsonProjection, path: &str) -> NodeTarget {
        let node = projection
            .nodes()
            .iter()
            .find(|node| node.path == path)
            .unwrap_or_else(|| panic!("missing {path}"));
        NodeTarget::of(node)
    }

    fn mutate(
        text: &str,
        format: JsonFormat,
        projection: &JsonProjection,
        mutation: Mutation,
    ) -> String {
        let replacements = plan(text, projection, format, &mutation).expect("mutation plans");
        apply(text, &replacements)
    }

    #[test]
    fn duplicate_occurrence_rename_and_value_edit_are_local() {
        let text = "{\n  // duplicate keys stay separate\n  \"a\": \"first\",\n  \"a\": \"second\\u4e2d\",\n  \"n\": 12345678901234567890.00,\n  \"escaped\": \"line\\nbreak\",\n}";
        let projection = build_projection(text, JsonFormat::Jsonc);
        let second = target(&projection, "$[\"a\"] #2");
        let edited = mutate(
            text,
            JsonFormat::Jsonc,
            &projection,
            Mutation::SetValue {
                target: second.clone(),
                value: "\"changed\"".to_owned(),
            },
        );
        assert_eq!(edited.matches("\"a\": \"first\"").count(), 1);
        assert!(edited.contains("\"a\": \"changed\""));
        assert!(edited.contains("12345678901234567890.00"));
        assert!(edited.contains("// duplicate keys stay separate"));

        let projection = build_projection(&edited, JsonFormat::Jsonc);
        let renamed = mutate(
            &edited,
            JsonFormat::Jsonc,
            &projection,
            Mutation::RenameKey {
                target: target(&projection, "$[\"a\"] #2"),
                key: "renamed".to_owned(),
            },
        );
        assert!(renamed.contains("\"a\": \"first\""));
        assert!(renamed.contains("\"renamed\": \"changed\""));
        assert!(renamed.contains("\"escaped\": \"line\\nbreak\""));
    }

    #[test]
    fn exact_numeric_spelling_and_unrelated_escapes_survive_edits() {
        let text = "{\n  \"exp\": 1e2,\n  \"neg\": -0,\n  \"big\": 184467440737095516170,\n  \"\\u0064up\": \"\\u006bey\",\n  \"keep\": 1\n}";
        let projection = build_projection(text, JsonFormat::Jsonc);
        let edited = mutate(
            text,
            JsonFormat::Jsonc,
            &projection,
            Mutation::SetValue {
                target: target(&projection, "$[\"keep\"]"),
                value: "1e400".to_owned(),
            },
        );
        assert!(edited.contains("\"exp\": 1e2"));
        assert!(edited.contains("\"neg\": -0"));
        assert!(edited.contains("\"big\": 184467440737095516170"));
        assert!(edited.contains("\"\\u0064up\": \"\\u006bey\""));
        assert!(edited.contains("\"keep\": 1e400"));
    }

    #[test]
    fn add_and_remove_keep_jsonc_comments_order_and_trailing_commas() {
        let text = "{\n  // keep this\n  \"a\": 1, // trailing\n  \"nested\": { \"x\": 1, /* inner */ },\n}";
        let projection = build_projection(text, JsonFormat::Jsonc);
        let edited = mutate(
            text,
            JsonFormat::Jsonc,
            &projection,
            Mutation::AddMember {
                container: target(&projection, "$"),
                key: Some("b".to_owned()),
                value: "2".to_owned(),
            },
        );
        assert!(edited.contains("// keep this"));
        assert!(edited.contains("// trailing"));
        assert!(edited.contains("\"a\": 1"));
        assert!(edited.contains("\"b\": 2"));
        assert!(
            edited.find("\"a\"").unwrap() < edited.find("\"nested\"").unwrap(),
            "key order is retained"
        );
        assert!(
            jsonc_parser::parse_to_ast(
                &edited,
                &CollectOptions::default(),
                &options(JsonFormat::Jsonc)
            )
            .is_ok()
        );

        let projection = build_projection(&edited, JsonFormat::Jsonc);
        let removed = mutate(
            &edited,
            JsonFormat::Jsonc,
            &projection,
            Mutation::RemoveMember {
                target: target(&projection, "$[\"a\"]"),
            },
        );
        assert!(!removed.contains("\"a\""));
        assert!(removed.contains("// keep this"));
        assert!(removed.contains("// trailing"));
        assert!(removed.contains("\"nested\""));
        assert!(
            jsonc_parser::parse_to_ast(
                &removed,
                &CollectOptions::default(),
                &options(JsonFormat::Jsonc)
            )
            .is_ok()
        );
    }

    #[test]
    fn empty_containers_and_arrays_are_filled_without_touching_neighbors() {
        let text = "{\n  \"object\": {},\n  \"lines\": {\n  },\n  \"array\": [],\n  \"linesArray\": [\n  ],\n  \"keep\": 1e2\n}";
        let projection = build_projection(text, JsonFormat::Jsonc);
        let edited = mutate(
            text,
            JsonFormat::Jsonc,
            &projection,
            Mutation::AddMember {
                container: target(&projection, "$[\"object\"]"),
                key: Some("added".to_owned()),
                value: "1".to_owned(),
            },
        );
        assert!(edited.contains("\"object\": {\"added\": 1}"));
        let projection = build_projection(&edited, JsonFormat::Jsonc);
        let edited = mutate(
            &edited,
            JsonFormat::Jsonc,
            &projection,
            Mutation::AddMember {
                container: target(&projection, "$[\"lines\"]"),
                key: Some("added".to_owned()),
                value: "2".to_owned(),
            },
        );
        assert!(edited.contains("\"lines\": {\n    \"added\": 2\n  }"));
        let projection = build_projection(&edited, JsonFormat::Jsonc);
        let edited = mutate(
            &edited,
            JsonFormat::Jsonc,
            &projection,
            Mutation::AddMember {
                container: target(&projection, "$[\"array\"]"),
                key: None,
                value: "1e2".to_owned(),
            },
        );
        assert!(edited.contains("\"array\": [1e2]"));
        let projection = build_projection(&edited, JsonFormat::Jsonc);
        let edited = mutate(
            &edited,
            JsonFormat::Jsonc,
            &projection,
            Mutation::AddMember {
                container: target(&projection, "$[\"linesArray\"]"),
                key: None,
                value: "-0".to_owned(),
            },
        );
        assert!(edited.contains("\"linesArray\": [\n    -0\n  ]"));
        assert!(edited.contains("\"keep\": 1e2"));
    }

    #[test]
    fn array_member_edit_insert_and_remove_keep_other_spelling() {
        let text = "{\n  \"items\": [1e2, 184467440737095516170, -0]\n}";
        let projection = build_projection(text, JsonFormat::Jsonc);
        let edited = mutate(
            text,
            JsonFormat::Jsonc,
            &projection,
            Mutation::SetValue {
                target: target(&projection, "$[\"items\"][1]"),
                value: "\"changed\"".to_owned(),
            },
        );
        assert!(edited.contains("[1e2, \"changed\", -0]"));
        let projection = build_projection(&edited, JsonFormat::Jsonc);
        let removed = mutate(
            &edited,
            JsonFormat::Jsonc,
            &projection,
            Mutation::RemoveMember {
                target: target(&projection, "$[\"items\"][0]"),
            },
        );
        assert!(!removed.contains("1e2"));
        assert!(removed.contains("\"changed\""));
        assert!(removed.contains("-0"));
        assert!(
            jsonc_parser::parse_to_ast(
                &removed,
                &CollectOptions::default(),
                &options(JsonFormat::Jsonc)
            )
            .is_ok()
        );
    }

    #[test]
    fn invalid_fragments_and_stale_targets_never_produce_replacements() {
        let text = "{\"a\": 1, \"b\": 2}";
        let projection = build_projection(text, JsonFormat::Json);
        let a = target(&projection, "$[\"a\"]");
        for bad in ["", "1 2", "1 // comment", "/*c*/ 1", "{", "\"unterminated"] {
            assert!(
                matches!(
                    plan(
                        text,
                        &projection,
                        JsonFormat::Json,
                        &Mutation::SetValue {
                            target: a.clone(),
                            value: bad.to_owned(),
                        },
                    ),
                    Err(MutationError::InvalidValue(_))
                ),
                "{bad:?} must be refused"
            );
        }
        let mut stale = a.clone();
        stale.range = 0..1;
        assert_eq!(
            plan(
                text,
                &projection,
                JsonFormat::Json,
                &Mutation::RemoveMember { target: stale }
            ),
            Err(MutationError::Stale)
        );
        assert_eq!(
            plan(
                text,
                &projection,
                JsonFormat::Json,
                &Mutation::AddRecord {
                    after: None,
                    value: "2".to_owned(),
                },
            ),
            Err(MutationError::Unsupported)
        );
    }

    #[test]
    fn remove_last_member_without_trailing_comma_removes_the_separator() {
        let text = "{\n  \"a\": 1,\n  \"b\": 2\n}";
        let projection = build_projection(text, JsonFormat::Json);
        let removed = mutate(
            text,
            JsonFormat::Json,
            &projection,
            Mutation::RemoveMember {
                target: target(&projection, "$[\"b\"]"),
            },
        );
        assert_eq!(removed, "{\n  \"a\": 1\n}");
        let projection = build_projection(&removed, JsonFormat::Json);
        let removed = mutate(
            &removed,
            JsonFormat::Json,
            &projection,
            Mutation::RemoveMember {
                target: target(&projection, "$[\"a\"]"),
            },
        );
        assert_eq!(removed, "{\n}");
    }

    #[test]
    fn adding_a_key_reuses_the_last_member_colon_style() {
        let text = "{\"a\":1}";
        let projection = build_projection(text, JsonFormat::Json);
        let edited = mutate(
            text,
            JsonFormat::Json,
            &projection,
            Mutation::AddMember {
                container: target(&projection, "$"),
                key: Some("b".to_owned()),
                value: "2".to_owned(),
            },
        );
        assert_eq!(edited, "{\"a\":1, \"b\":2}");
    }

    #[test]
    fn jsonl_mutations_touch_only_the_target_record_and_keep_invalid_neighbors() {
        let text = "{\"a\": 1, \"dup\": 1, \"dup\": 2}\n\n{\"broken\": ,}\n{\"items\": [1e2, -0, \"\\u4e2d\"]}\n42\n";
        let projection = build_projection(text, JsonFormat::JsonLines);
        let edited = mutate(
            text,
            JsonFormat::JsonLines,
            &projection,
            Mutation::SetValue {
                target: target(&projection, "line 1[\"dup\"] #2"),
                value: "\"changed\"".to_owned(),
            },
        );
        assert_eq!(
            edited,
            "{\"a\": 1, \"dup\": 1, \"dup\": \"changed\"}\n\n{\"broken\": ,}\n{\"items\": [1e2, -0, \"\\u4e2d\"]}\n42\n"
        );
        let projection = build_projection(&edited, JsonFormat::JsonLines);
        let edited = mutate(
            &edited,
            JsonFormat::JsonLines,
            &projection,
            Mutation::RenameKey {
                target: target(&projection, "line 1[\"a\"]"),
                key: "键".to_owned(),
            },
        );
        assert!(edited.starts_with("{\"键\": 1, \"dup\": 1, \"dup\": \"changed\"}\n"));
        let projection = build_projection(&edited, JsonFormat::JsonLines);
        let edited = mutate(
            &edited,
            JsonFormat::JsonLines,
            &projection,
            Mutation::AddMember {
                container: target(&projection, "line 4[\"items\"]"),
                key: None,
                value: "\"追加\"".to_owned(),
            },
        );
        assert!(edited.contains("\"items\": [1e2, -0, \"\\u4e2d\", \"追加\"]"));
        let projection = build_projection(&edited, JsonFormat::JsonLines);
        let edited = mutate(
            &edited,
            JsonFormat::JsonLines,
            &projection,
            Mutation::AddRecord {
                after: Some(target(&projection, "line 1")),
                value: "{\"id\": 9, \"name\": \"你好🙂\"}".to_owned(),
            },
        );
        assert!(edited.starts_with(
            "{\"键\": 1, \"dup\": 1, \"dup\": \"changed\"}\n{\"id\": 9, \"name\": \"你好🙂\"}\n\n"
        ));
        let projection = build_projection(&edited, JsonFormat::JsonLines);
        let edited = mutate(
            &edited,
            JsonFormat::JsonLines,
            &projection,
            Mutation::RemoveRecord {
                target: target(&projection, "line 4"),
            },
        );
        assert_eq!(
            edited,
            "{\"键\": 1, \"dup\": 1, \"dup\": \"changed\"}\n{\"id\": 9, \"name\": \"你好🙂\"}\n\n{\"items\": [1e2, -0, \"\\u4e2d\", \"追加\"]}\n42\n"
        );
        assert_eq!(
            build_projection(&edited, JsonFormat::JsonLines).invalid_records(),
            0
        );
    }

    #[test]
    fn jsonl_add_record_handles_append_interior_and_missing_final_newline() {
        for (text, after, expected) in [
            ("", None, "{\"b\":2}"),
            ("{\"a\":1}", None, "{\"a\":1}\n{\"b\":2}"),
            ("{\"a\":1}\n", None, "{\"a\":1}\n{\"b\":2}\n"),
            ("{\"a\":1}", Some("line 1"), "{\"a\":1}\n{\"b\":2}"),
            ("{\"a\":1}\n", Some("line 1"), "{\"a\":1}\n{\"b\":2}\n"),
            (
                "  {\"a\":1}\n",
                Some("line 1"),
                "  {\"a\":1}\n  {\"b\":2}\n",
            ),
        ] {
            let projection = build_projection(text, JsonFormat::JsonLines);
            let after = after.map(|path| target(&projection, path));
            let edited = mutate(
                text,
                JsonFormat::JsonLines,
                &projection,
                Mutation::AddRecord {
                    after: after.clone(),
                    value: "{\"b\":2}".to_owned(),
                },
            );
            assert_eq!(edited, expected, "text={text:?} after={after:?}");
        }
    }

    #[test]
    fn jsonl_rejects_multiline_values_and_stale_record_targets() {
        let text = "{\"a\":1}\n{\"b\":2}\n";
        let projection = build_projection(text, JsonFormat::JsonLines);
        let target = target(&projection, "line 1");
        assert_eq!(
            plan(
                text,
                &projection,
                JsonFormat::JsonLines,
                &Mutation::SetValue {
                    target: target.clone(),
                    value: "{\n}".to_owned(),
                },
            ),
            Err(MutationError::RecordNotSingleLine)
        );
        assert_eq!(
            plan(
                text,
                &projection,
                JsonFormat::JsonLines,
                &Mutation::AddRecord {
                    after: Some(target.clone()),
                    value: "1\n2".to_owned(),
                },
            ),
            Err(MutationError::RecordNotSingleLine)
        );
        let mut stale = target;
        stale.range = 0..2;
        assert_eq!(
            plan(
                text,
                &projection,
                JsonFormat::JsonLines,
                &Mutation::RemoveRecord { target: stale },
            ),
            Err(MutationError::Stale)
        );
    }

    #[test]
    fn jsonl_invalid_record_can_be_replaced_as_a_whole_line() {
        let text = "{\"a\":1}\n{broken\n";
        let projection = build_projection(text, JsonFormat::JsonLines);
        assert_eq!(projection.invalid_records(), 1);
        let edited = mutate(
            text,
            JsonFormat::JsonLines,
            &projection,
            Mutation::SetValue {
                target: target(&projection, "line 2"),
                value: "{\"fixed\":true}".to_owned(),
            },
        );
        assert_eq!(edited, "{\"a\":1}\n{\"fixed\":true}\n");
        assert_eq!(
            build_projection(&edited, JsonFormat::JsonLines).invalid_records(),
            0
        );
    }
}
