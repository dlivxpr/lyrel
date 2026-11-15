//! #293 TOML 结构的局部源编辑规划。
//!
//! 命令只针对当前投影里的一个确切节点：目标用 `(path, range, kind)`
//! 重新解析当前 LF 源码后确认；可见行索引不是身份。规划只产生最小替换
//! 清单，写入仍由调用方交给共享 `Document::replace_ranges`，因此一次命令
//! 就是一条普通 history。
//!
//! 重点约束：
//!
//! - ordinary table / AoT 的删除不能使用 `Table::span()` 或
//!   `ArrayOfTables::span()` 作为连续区间：前者只覆盖 header，后者可能跨过
//!   无关的交错表。这里只删除目标自己的 header 行、自己的直接 key/value
//!   行和显式后代 header/value 行，并逐个区间合并。
//! - inline table 成员编辑只动成员与一个分隔逗号；不整篇 `to_string()`。
//! - 新值/新键先由 `toml_edit` 的正式解析器校验，写入前再解析整个结果并
//!   核对目标结构，任何会改错位置或留下无效 TOML 的规划都被拒绝。

use std::ops::Range;

use toml_edit::{Document, Item, Key};

use super::projection::{
    self, TomlNode, TomlNodeKind, TomlProjection, TomlValueKind, child_path, key_repr,
};

/// 目标节点的稳定身份。`range` 是值本身或 header；`key_range` 在可改名时
/// 进一步固定 occurrence。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TomlTarget {
    pub(crate) path: String,
    pub(crate) range: Range<usize>,
    pub(crate) kind: TomlNodeKind,
    pub(crate) key_range: Option<Range<usize>>,
}

impl TomlTarget {
    pub(crate) fn of(node: &TomlNode) -> Self {
        Self {
            path: node.path.clone(),
            range: node.range.clone(),
            kind: node.kind,
            key_range: node.key_range.clone(),
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) enum Mutation {
    SetValue {
        target: TomlTarget,
        value: String,
    },
    Rename {
        target: TomlTarget,
        key: String,
    },
    AddKey {
        container: TomlTarget,
        key: String,
        value: String,
    },
    AddTable {
        container: TomlTarget,
        key: String,
    },
    AddArray {
        container: TomlTarget,
        key: String,
    },
    AddEntry {
        array: TomlTarget,
    },
    Remove {
        target: TomlTarget,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum MutationError {
    /// 当前源码已无法重新解析出该目标，或目标 occurrence 已变化。
    Stale,
    /// 该节点不支持这个命令（根、隐式/内联表边界等）。
    Unsupported,
    /// 新键不是恰好一个 TOML key。
    InvalidKey(String),
    /// 新值不是恰好一个 TOML value。
    InvalidValue(String),
    /// 目标容器已有同名键。
    DuplicateKey(String),
    /// 规划结果不是有效 TOML。
    ResultInvalid {
        line: usize,
        column: usize,
        message: String,
    },
    /// 规划结果能解析，但没有落在目标结构上。
    ResultShape,
}

/// 规划一次局部修改，返回按源码顺序排列、互不重叠的替换清单。
///
/// 空清单表示命令是 no-op（例如值没有变化）。任何失败都发生在写入之前。
pub(crate) fn plan(
    text: &str,
    mutation: &Mutation,
) -> Result<Vec<(Range<usize>, String)>, MutationError> {
    let projection =
        projection::project_for_mutation(text).map_err(|(line, column, message)| {
            MutationError::ResultInvalid {
                line,
                column,
                message,
            }
        })?;
    match mutation {
        Mutation::SetValue { target, value } => {
            let node = resolve(&projection, target)?;
            if !node.kind.is_value() || node.range.is_empty() {
                return Err(MutationError::Unsupported);
            }
            let value = validate_value(value)?;
            if text.get(node.range.clone()) == Some(value.as_str()) {
                return Ok(Vec::new());
            }
            let replacements = vec![(node.range.clone(), value.clone())];
            verify_result(text, &replacements, |result, applied| {
                result.nodes().iter().any(|candidate| {
                    candidate.path == target.path
                        && candidate.kind == target.kind
                        && applied.get(candidate.range.clone()) == Some(value.as_str())
                })
            })?;
            Ok(replacements)
        }
        Mutation::Rename { target, key } => {
            let node = resolve(&projection, target)?;
            let repr = validate_key(key)?;
            let replacements = match node.kind {
                TomlNodeKind::Value(_) => {
                    let key_range = node.key_range.clone().ok_or(MutationError::Unsupported)?;
                    if node.key_text.as_deref() == Some(key.as_str()) {
                        return Ok(Vec::new());
                    }
                    vec![(key_range, repr)]
                }
                TomlNodeKind::Table if node.explicit_table => {
                    if node.key_text.as_deref() == Some(key.as_str()) {
                        return Ok(Vec::new());
                    }
                    rename_header_path(&projection, node, repr)?
                }
                TomlNodeKind::ArrayOfTables => {
                    if node.key_text.as_deref() == Some(key.as_str()) {
                        return Ok(Vec::new());
                    }
                    rename_header_path(&projection, node, repr)?
                }
                _ => return Err(MutationError::Unsupported),
            };
            let mut expected = node.key_path.clone();
            if expected.is_empty() {
                return Err(MutationError::Unsupported);
            }
            expected.pop();
            expected.push(key.clone());
            verify_result(text, &replacements, |result, _| {
                result.nodes().iter().any(|candidate| {
                    candidate.key_path == expected && candidate.kind == target.kind
                })
            })?;
            Ok(replacements)
        }
        Mutation::AddKey {
            container,
            key,
            value,
        } => {
            let node = resolve(&projection, container)?;
            let repr = validate_key(key)?;
            let value = validate_value(value)?;
            if node.kind.is_value() && !node.is_inline_table() {
                return Err(MutationError::Unsupported);
            }
            if !matches!(
                node.kind,
                TomlNodeKind::Root
                    | TomlNodeKind::Table
                    | TomlNodeKind::ArrayOfTablesEntry
                    | TomlNodeKind::Value(TomlValueKind::InlineTable)
            ) || node.inline_context && !node.is_inline_table()
            {
                return Err(MutationError::Unsupported);
            }
            if direct_child_key(&projection, node, key) {
                return Err(MutationError::DuplicateKey(key.clone()));
            }
            let expected_path = child_path(&node.path, key);
            let replacements = if node.is_inline_table() {
                inline_insert(text, node, &repr, &value)?
            } else {
                if node.kind == TomlNodeKind::Table && !node.explicit_table {
                    return Err(MutationError::Unsupported);
                }
                let at = node.own_insert.ok_or(MutationError::Unsupported)?;
                let mut inserted = format!("{repr} = {value}\n");
                if at > 0 && !text[..at].ends_with('\n') {
                    inserted.insert(0, '\n');
                }
                vec![(at..at, inserted)]
            };
            verify_result(text, &replacements, |result, _| {
                result
                    .nodes()
                    .iter()
                    .any(|candidate| candidate.path == expected_path && candidate.kind.is_value())
            })?;
            Ok(replacements)
        }
        Mutation::AddTable { container, key } | Mutation::AddArray { container, key } => {
            let array = matches!(mutation, Mutation::AddArray { .. });
            let node = resolve(&projection, container)?;
            validate_key(key)?;
            if !matches!(
                node.kind,
                TomlNodeKind::Root | TomlNodeKind::Table | TomlNodeKind::ArrayOfTablesEntry
            ) || (node.kind == TomlNodeKind::Table && !node.explicit_table)
                || node.inline_context
            {
                return Err(MutationError::Unsupported);
            }
            if direct_child_key(&projection, node, key) {
                return Err(MutationError::DuplicateKey(key.clone()));
            }
            let mut path = node.key_path.clone();
            path.push(key.clone());
            let inner = path
                .iter()
                .map(|segment| key_repr(segment))
                .collect::<Vec<_>>()
                .join(".");
            let at = insertion_after_subtree(text, node);
            let mut inserted = if array {
                format!("[[{inner}]]\n")
            } else {
                format!("[{inner}]\n")
            };
            if at > 0 && !text[..at].ends_with('\n') {
                inserted.insert(0, '\n');
            }
            let replacements = vec![(at..at, inserted)];
            let expected_path = child_path(&node.path, key);
            let expected_kind = if array {
                TomlNodeKind::ArrayOfTables
            } else {
                TomlNodeKind::Table
            };
            verify_result(text, &replacements, |result, _| {
                result.nodes().iter().any(|candidate| {
                    candidate.path == expected_path && candidate.kind == expected_kind
                })
            })?;
            Ok(replacements)
        }
        Mutation::AddEntry { array } => {
            let node = resolve(&projection, array)?;
            if node.kind != TomlNodeKind::ArrayOfTables {
                return Err(MutationError::Unsupported);
            }
            let old_len = node.children.len();
            let inner = node
                .key_path
                .iter()
                .map(|segment| key_repr(segment))
                .collect::<Vec<_>>()
                .join(".");
            let at = insertion_after_subtree(text, node);
            let mut inserted = format!("[[{inner}]]\n");
            if at > 0 && !text[..at].ends_with('\n') {
                inserted.insert(0, '\n');
            }
            let replacements = vec![(at..at, inserted)];
            verify_result(text, &replacements, |result, _| {
                result.nodes().iter().any(|candidate| {
                    candidate.path == node.path
                        && candidate.kind == TomlNodeKind::ArrayOfTables
                        && candidate.children.len() == old_len + 1
                })
            })?;
            Ok(replacements)
        }
        Mutation::Remove { target } => {
            let node = resolve(&projection, target)?;
            let replacements = match node.kind {
                TomlNodeKind::Value(_) if node.inline_context => {
                    if !node.inline_direct {
                        return Err(MutationError::Unsupported);
                    }
                    inline_remove(text, &projection, node)?
                }
                TomlNodeKind::Value(_) => {
                    let statement = node.statement.clone().ok_or(MutationError::Unsupported)?;
                    vec![(statement, String::new())]
                }
                TomlNodeKind::Root => return Err(MutationError::Unsupported),
                _ if node.inline_context => return Err(MutationError::Unsupported),
                _ => removal_ranges(&projection, node),
            };
            if replacements.is_empty() {
                return Err(MutationError::Unsupported);
            }
            let absent = target.kind != TomlNodeKind::ArrayOfTablesEntry;
            let path = target.path.clone();
            let kind = target.kind;
            verify_result(text, &replacements, |result, _| {
                if !absent {
                    return true;
                }
                !result
                    .nodes()
                    .iter()
                    .any(|candidate| candidate.path == path && candidate.kind == kind)
            })?;
            Ok(replacements)
        }
    }
}

fn resolve<'a>(
    projection: &'a TomlProjection,
    target: &TomlTarget,
) -> Result<&'a TomlNode, MutationError> {
    projection
        .nodes()
        .iter()
        .find(|node| {
            node.path == target.path
                && node.range == target.range
                && node.kind == target.kind
                && node.key_range == target.key_range
        })
        .ok_or(MutationError::Stale)
}

fn direct_child_key(projection: &TomlProjection, node: &TomlNode, key: &str) -> bool {
    node.children.iter().any(|child| {
        projection
            .nodes()
            .get(*child)
            .is_some_and(|child| child.key_text.as_deref() == Some(key))
    })
}

fn validate_key(key: &str) -> Result<String, MutationError> {
    let repr = key_repr(key);
    let keys =
        Key::parse(&repr).map_err(|error| MutationError::InvalidKey(error.message().to_owned()))?;
    if keys.len() != 1 || keys[0].get() != key {
        return Err(MutationError::InvalidKey(key.to_owned()));
    }
    Ok(repr)
}

fn validate_value(value: &str) -> Result<String, MutationError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(MutationError::InvalidValue("empty value".to_owned()));
    }
    let wrapped = format!("x = {value}");
    let document = Document::parse(wrapped.as_str())
        .map_err(|error| MutationError::InvalidValue(error.message().to_owned()))?;
    let Some((_key, item)) = document.as_table().get_key_value("x") else {
        return Err(MutationError::InvalidValue("not one TOML value".to_owned()));
    };
    let Item::Value(value_node) = item else {
        return Err(MutationError::InvalidValue("not one TOML value".to_owned()));
    };
    let Some(span) = value_node.span() else {
        return Err(MutationError::InvalidValue(
            "value has no source span".to_owned(),
        ));
    };
    if span.start != 4 || span.end != 4 + value.len() {
        return Err(MutationError::InvalidValue(
            "extra content after the TOML value".to_owned(),
        ));
    }
    Ok(value.to_owned())
}

fn rename_header_path(
    projection: &TomlProjection,
    node: &TomlNode,
    repr: String,
) -> Result<Vec<(Range<usize>, String)>, MutationError> {
    if node.key_path.is_empty() {
        return Err(MutationError::Unsupported);
    }
    let index = node.key_path.len() - 1;
    let old_key = node
        .key_text
        .clone()
        .or_else(|| node.key_path.last().cloned())
        .ok_or(MutationError::Unsupported)?;
    let mut replacements = Vec::new();
    collect_header_renames(projection, node, index, &old_key, &repr, &mut replacements);
    if replacements.is_empty() {
        return Err(MutationError::Unsupported);
    }
    // 同一个 header 可能同时出现在容器和 entry 的遍历里；去重。
    replacements.sort_by_key(|(range, _)| range.start);
    replacements
        .dedup_by(|(left, _), (right, _)| left.start == right.start && left.end == right.end);
    Ok(replacements)
}

fn collect_header_renames(
    projection: &TomlProjection,
    node: &TomlNode,
    index: usize,
    old_key: &str,
    repr: &str,
    replacements: &mut Vec<(Range<usize>, String)>,
) {
    if let Some(header) = node.header.as_ref() {
        if let Some(segment) = header.segments.get(index)
            && segment.key == old_key
        {
            replacements.push((segment.raw.clone(), repr.to_owned()));
        }
    }
    for child in &node.children {
        if let Some(child) = projection.nodes().get(*child) {
            collect_header_renames(projection, child, index, old_key, repr, replacements);
        }
    }
}

fn removal_ranges(projection: &TomlProjection, node: &TomlNode) -> Vec<(Range<usize>, String)> {
    let mut ranges = Vec::new();
    collect_removal_ranges(projection, node, &mut ranges);
    ranges.sort_by_key(|range| range.start);
    let mut merged: Vec<Range<usize>> = Vec::new();
    for range in ranges {
        if let Some(last) = merged.last_mut()
            && range.start <= last.end
        {
            last.end = last.end.max(range.end);
            continue;
        }
        merged.push(range);
    }
    merged
        .into_iter()
        .map(|range| (range, String::new()))
        .collect()
}

fn collect_removal_ranges(
    projection: &TomlProjection,
    node: &TomlNode,
    ranges: &mut Vec<Range<usize>>,
) {
    if let Some(header) = node.header.as_ref() {
        ranges.push(header.line.clone());
    }
    if node.kind.is_value() {
        if !node.inline_context {
            if let Some(statement) = node.statement.clone() {
                ranges.push(statement);
            }
        }
        // inline members are inside their container's statement; never remove
        // them again through a descendant walk.
        if node.is_inline_table() {
            return;
        }
    }
    for child in &node.children {
        if let Some(child) = projection.nodes().get(*child) {
            collect_removal_ranges(projection, child, ranges);
        }
    }
}

fn inline_insert(
    text: &str,
    node: &TomlNode,
    key: &str,
    value: &str,
) -> Result<Vec<(Range<usize>, String)>, MutationError> {
    let open = node.range.start + 1;
    let close = node
        .range
        .end
        .checked_sub(1)
        .ok_or(MutationError::Unsupported)?;
    if open > close || text.get(close..close + 1) != Some("}") {
        return Err(MutationError::Unsupported);
    }
    let inner = text.get(open..close).ok_or(MutationError::Unsupported)?;
    if inner.trim().is_empty() {
        return Ok(vec![(open..close, format!(" {key} = {value} "))]);
    }
    let mut at = close;
    while at > open {
        let previous = text[..at]
            .chars()
            .next_back()
            .ok_or(MutationError::Unsupported)?;
        if !previous.is_whitespace() {
            break;
        }
        at -= previous.len_utf8();
    }
    let trailing = &text[at..close];
    if trailing.contains('#') {
        return Err(MutationError::Unsupported);
    }
    Ok(vec![(at..at, format!(", {key} = {value}"))])
}

fn inline_remove(
    text: &str,
    projection: &TomlProjection,
    node: &TomlNode,
) -> Result<Vec<(Range<usize>, String)>, MutationError> {
    let parent = node
        .inline_parent
        .and_then(|parent| projection.nodes().get(parent))
        .filter(|parent| parent.is_inline_table())
        .ok_or(MutationError::Unsupported)?;
    let open = parent.range.start + 1;
    let close = parent
        .range
        .end
        .checked_sub(1)
        .ok_or(MutationError::Unsupported)?;
    let start = node
        .key_range
        .as_ref()
        .map(|range| range.start)
        .ok_or(MutationError::Unsupported)?;
    let end = node.range.end;
    if start < open || end > close {
        return Err(MutationError::Unsupported);
    }
    let before = text.get(open..start).ok_or(MutationError::Unsupported)?;
    let after = text.get(end..close).ok_or(MutationError::Unsupported)?;
    if before.contains('#') || after.contains('#') {
        return Err(MutationError::Unsupported);
    }
    let mut previous_comma = None;
    for (index, ch) in before.char_indices() {
        if ch == ',' {
            previous_comma = Some(open + index);
        }
    }
    let next_comma = after.find(',').map(|index| end + index);
    if let Some(comma) = next_comma {
        let mut after_comma = comma + 1;
        while after_comma < close {
            let ch = text[after_comma..]
                .chars()
                .next()
                .ok_or(MutationError::Unsupported)?;
            if !ch.is_whitespace() {
                break;
            }
            after_comma += ch.len_utf8();
        }
        return Ok(vec![(start..after_comma, String::new())]);
    }
    if let Some(comma) = previous_comma {
        return Ok(vec![(comma..end, String::new())]);
    }
    // Only member: keep the inline table itself and drop its inner content.
    Ok(vec![(open..close, String::new())])
}

fn insertion_after_subtree(text: &str, node: &TomlNode) -> usize {
    match node.kind {
        TomlNodeKind::Root => text.len(),
        _ => node.subtree_end.unwrap_or(node.range.end).min(text.len()),
    }
}

fn verify_result(
    text: &str,
    replacements: &[(Range<usize>, String)],
    verify: impl FnOnce(&TomlProjection, &str) -> bool,
) -> Result<(), MutationError> {
    let applied = apply(text, replacements)?;
    let projection =
        projection::project_for_mutation(&applied).map_err(|(line, column, message)| {
            MutationError::ResultInvalid {
                line,
                column,
                message,
            }
        })?;
    if verify(&projection, &applied) {
        Ok(())
    } else {
        Err(MutationError::ResultShape)
    }
}

fn apply(text: &str, replacements: &[(Range<usize>, String)]) -> Result<String, MutationError> {
    let mut sorted = replacements.to_vec();
    sorted.sort_by_key(|(range, _)| range.start);
    let mut capacity = text.len();
    for (range, inserted) in &sorted {
        if range.start > range.end || range.end > text.len() {
            return Err(MutationError::Unsupported);
        }
        capacity = capacity
            .saturating_sub(range.end - range.start)
            .saturating_add(inserted.len());
    }
    let mut output = String::with_capacity(capacity);
    let mut cursor = 0;
    for (range, inserted) in sorted {
        if range.start < cursor {
            return Err(MutationError::Unsupported);
        }
        output.push_str(
            text.get(cursor..range.start)
                .ok_or(MutationError::Unsupported)?,
        );
        output.push_str(&inserted);
        cursor = range.end;
    }
    output.push_str(text.get(cursor..).ok_or(MutationError::Unsupported)?);
    Ok(output)
}
