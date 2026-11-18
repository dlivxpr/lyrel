//! #292 YAML 的局部源编辑规划。
//!
//! 每个命令只针对当前投影里的一个 occurrence：目标用 `(path, range)` 重新
//! 定位，调用方还要求安装 revision 仍等于源码权威。规划器在原始源码上算出
//! 最小 splice；用户输入是**一个 YAML scalar 片段的原拼写**，由 `yaml-edit`
//! 在探针里校验，整篇结果再解析一次后才允许写入。
//!
//! 这里不把 CST 重新序列化整篇文档，也不构造第二份可写模型。anchor／alias／
//! tag／注释只受目标字节的局部替换影响；删除会引用到被删 anchor 的节点时
//! 在写入前拒绝，而不是静默 detach。应用由调用方交给共享 `Document` 的
//! `replace_ranges`，一次命令是一条普通 history。

use std::ops::Range;

use yaml_edit::{AsYaml, Document, Scalar, YamlFile, YamlNode as AstYamlNode};

use super::projection::{ContainerKind, YamlNode, YamlNodeKind, YamlProjection};

/// 目标节点的稳定身份；可见行索引不是身份。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeTarget {
    pub path: String,
    pub range: Range<usize>,
}

impl NodeTarget {
    pub fn of(node: &YamlNode) -> Self {
        Self {
            path: node.path.clone(),
            range: node.range.clone(),
        }
    }
}

#[derive(Clone, Debug)]
pub enum Mutation {
    SetValue {
        target: NodeTarget,
        value: String,
    },
    RenameKey {
        target: NodeTarget,
        key: String,
    },
    AddMember {
        container: NodeTarget,
        key: String,
        value: String,
    },
    AddItem {
        container: NodeTarget,
        value: String,
    },
    Remove {
        target: NodeTarget,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnsupportedEdit {
    Alias,
    MissingValue,
    MultiLine,
    NotContainer,
    NotScalar,
    EmptyDocument,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MutationError {
    Stale,
    Unsupported(UnsupportedEdit),
    InvalidFragment(String),
    ResultInvalid {
        line: usize,
        column: usize,
        message: String,
    },
    AliasesReference {
        name: Option<String>,
        count: usize,
    },
}

/// 规划一次局部修改，返回按源码顺序排列、互不重叠的替换清单。
pub fn plan(
    text: &str,
    projection: &YamlProjection,
    mutation: &Mutation,
) -> Result<Vec<(Range<usize>, String)>, MutationError> {
    let replacements = match mutation {
        Mutation::SetValue { target, value } => {
            let index = resolve(projection, target)?;
            let node = &projection.nodes()[index];
            if node.alias.is_some() {
                return Err(MutationError::Unsupported(UnsupportedEdit::Alias));
            }
            if node.container.is_some() {
                return Err(MutationError::Unsupported(UnsupportedEdit::NotScalar));
            }
            if node.block_scalar {
                let range = node.content_range.clone();
                let original = text.get(range.clone()).unwrap_or_default();
                let raw = value.trim_end_matches(['\r', '\n']);
                validate_block_value(raw)?;
                let mut fragment = value.to_owned();
                if original.ends_with('\n') && !fragment.ends_with('\n') {
                    fragment.push('\n');
                }
                if original == fragment {
                    Vec::new()
                } else {
                    vec![(range, fragment)]
                }
            } else {
                let range = node
                    .value_range
                    .clone()
                    .ok_or(MutationError::Unsupported(UnsupportedEdit::MissingValue))?;
                let raw = single_line(value)?;
                validate_value(raw)?;
                if text.get(range.clone()) == Some(raw) {
                    Vec::new()
                } else {
                    vec![(range, raw.to_owned())]
                }
            }
        }
        Mutation::RenameKey { target, key } => {
            let index = resolve(projection, target)?;
            let node = &projection.nodes()[index];
            if !node.entry {
                return Err(MutationError::Unsupported(UnsupportedEdit::NotScalar));
            }
            let (range, raw) = if node.key_editable {
                let range = node
                    .key_range
                    .clone()
                    .ok_or(MutationError::Unsupported(UnsupportedEdit::NotScalar))?;
                let raw = single_line(key)?;
                validate_key(raw)?;
                (range, raw)
            } else {
                let range = node
                    .key_detail_range
                    .clone()
                    .ok_or(MutationError::Unsupported(UnsupportedEdit::NotScalar))?;
                let raw = single_line(key)?;
                validate_complex_key(raw)?;
                (range, raw)
            };
            if text.get(range.clone()) == Some(raw) {
                Vec::new()
            } else {
                vec![(range, raw.to_owned())]
            }
        }
        Mutation::AddMember {
            container,
            key,
            value,
        } => {
            let index = resolve(projection, container)?;
            let node = &projection.nodes()[index];
            if node.kind == YamlNodeKind::Empty {
                return Err(MutationError::Unsupported(UnsupportedEdit::EmptyDocument));
            }
            let Some(kind) = node.container else {
                return Err(MutationError::Unsupported(UnsupportedEdit::NotContainer));
            };
            if kind != ContainerKind::Mapping {
                return Err(MutationError::Unsupported(UnsupportedEdit::NotContainer));
            }
            let key = single_line(key)?;
            let value = single_line(value)?;
            validate_entry_fragment(key, value, node.flow)?;
            insert_member(text, projection, index, key, value)?
        }
        Mutation::AddItem { container, value } => {
            let index = resolve(projection, container)?;
            let node = &projection.nodes()[index];
            if node.container != Some(ContainerKind::Sequence) {
                return Err(MutationError::Unsupported(UnsupportedEdit::NotContainer));
            }
            let value = single_line(value)?;
            validate_item_fragment(value, node.flow)?;
            insert_item(text, projection, index, value)?
        }
        Mutation::Remove { target } => {
            let index = resolve(projection, target)?;
            remove(text, projection, index)?
        }
    };
    if !replacements.is_empty() {
        verify_result(text, &replacements)?;
    }
    Ok(replacements)
}

fn resolve(projection: &YamlProjection, target: &NodeTarget) -> Result<usize, MutationError> {
    projection
        .nodes()
        .iter()
        .position(|node| node.path == target.path && node.range == target.range)
        .ok_or(MutationError::Stale)
}

fn single_line(fragment: &str) -> Result<&str, MutationError> {
    let trimmed = fragment.trim();
    if trimmed.is_empty() {
        return Err(MutationError::InvalidFragment(
            "a YAML scalar is required".to_owned(),
        ));
    }
    if trimmed.contains(['\n', '\r']) {
        return Err(MutationError::Unsupported(UnsupportedEdit::MultiLine));
    }
    Ok(trimmed)
}

fn parse_probe(probe: &str) -> Result<Document, MutationError> {
    let parse = YamlFile::parse(probe);
    if let Some(first) = parse.positioned_errors().first() {
        return Err(MutationError::InvalidFragment(first.message.clone()));
    }
    parse
        .tree()
        .documents()
        .next()
        .ok_or_else(|| MutationError::InvalidFragment("no YAML document".to_owned()))
}

fn value_range(value: &AstYamlNode) -> Option<Range<usize>> {
    let scalar = match value {
        AstYamlNode::Scalar(scalar) => scalar,
        _ => return None,
    };
    let range = scalar.byte_range();
    Some(range.start as usize..range.end as usize)
}

/// 值必须是一个 scalar 片段，且探针里解析出的值恰好覆盖用户输入；注释、
/// 尾随内容、集合或 alias 都不会被当作 scalar 写入。
fn validate_value(raw: &str) -> Result<(), MutationError> {
    let probe = format!("__lyrel_value__: {raw}\n");
    let document = parse_probe(&probe)?;
    let entries: Vec<_> = document
        .as_mapping()
        .ok_or_else(|| MutationError::InvalidFragment("not one mapping entry".to_owned()))?
        .entries()
        .collect();
    if entries.len() != 1 {
        return Err(MutationError::InvalidFragment(
            "only one scalar value is allowed".to_owned(),
        ));
    }
    let value = entries[0]
        .value_node()
        .ok_or_else(|| MutationError::InvalidFragment("no scalar value".to_owned()))?;
    let range = value_range(&value)
        .ok_or_else(|| MutationError::InvalidFragment("not one scalar value".to_owned()))?;
    if probe.get(range) != Some(raw) {
        return Err(MutationError::InvalidFragment(
            "only one scalar value is allowed".to_owned(),
        ));
    }
    Ok(())
}

/// A block scalar fragment must itself parse as exactly one block scalar in
/// value position. The user edits the raw header and body; anchors, tags,
/// comments and unrelated source stay outside the replaced range.
fn validate_block_value(raw: &str) -> Result<(), MutationError> {
    if !(raw.starts_with('|') || raw.starts_with('>')) {
        return Err(MutationError::InvalidFragment(
            "a block scalar fragment must start with | or >".to_owned(),
        ));
    }
    let probe = format!("__lyrel_value__: {raw}\n");
    let document = parse_probe(&probe)?;
    let entries: Vec<_> = document
        .as_mapping()
        .ok_or_else(|| MutationError::InvalidFragment("not one mapping entry".to_owned()))?
        .entries()
        .collect();
    if entries.len() != 1 {
        return Err(MutationError::InvalidFragment(
            "only one scalar value is allowed".to_owned(),
        ));
    }
    let value = entries[0]
        .value_node()
        .ok_or_else(|| MutationError::InvalidFragment("no scalar value".to_owned()))?;
    let scalar = match &value {
        AstYamlNode::Scalar(scalar) if scalar.is_block() => scalar,
        _ => {
            return Err(MutationError::InvalidFragment(
                "not one block scalar value".to_owned(),
            ));
        }
    };
    let range = scalar_range(scalar);
    let parsed = probe
        .get(range)
        .unwrap_or_default()
        .trim_end_matches(['\r', '\n']);
    if parsed != raw.trim_end_matches(['\r', '\n']) {
        return Err(MutationError::InvalidFragment(
            "only one block scalar value is allowed".to_owned(),
        ));
    }
    Ok(())
}

fn scalar_range(scalar: &Scalar) -> Range<usize> {
    let range = scalar.byte_range();
    range.start as usize..range.end as usize
}

/// A flow collection key can be replaced as one complete key fragment. Scalar
/// keys use the scalar path; multi-line or opaque keys stay in Source.
fn validate_complex_key(raw: &str) -> Result<(), MutationError> {
    let probe = format!("{raw}: __lyrel_probe__\n");
    let document = parse_probe(&probe)?;
    let entries: Vec<_> = document
        .as_mapping()
        .ok_or_else(|| MutationError::InvalidFragment("not one mapping entry".to_owned()))?
        .entries()
        .collect();
    if entries.len() != 1 {
        return Err(MutationError::InvalidFragment(
            "only one mapping key is allowed".to_owned(),
        ));
    }
    let key = entries[0]
        .key_node()
        .ok_or_else(|| MutationError::InvalidFragment("no mapping key".to_owned()))?;
    if matches!(key, AstYamlNode::Scalar(_)) {
        return Err(MutationError::InvalidFragment(
            "a scalar key uses the scalar rename path".to_owned(),
        ));
    }
    let range = key
        .as_node()
        .map(yaml_edit::advanced::syntax_node_range)
        .map(|range| u32::from(range.start()) as usize..u32::from(range.end()) as usize)
        .ok_or_else(|| MutationError::InvalidFragment("no mapping key".to_owned()))?;
    if probe.get(range) != Some(raw) {
        return Err(MutationError::InvalidFragment(
            "only one mapping key is allowed".to_owned(),
        ));
    }
    Ok(())
}

/// 键必须是一个 scalar 片段；complex key／mapping／sequence 不允许从树里
/// 猜测重命名。
fn validate_key(raw: &str) -> Result<(), MutationError> {
    let probe = format!("{raw}: __lyrel_probe__\n");
    let document = parse_probe(&probe)?;
    let entries: Vec<_> = document
        .as_mapping()
        .ok_or_else(|| MutationError::InvalidFragment("not one mapping entry".to_owned()))?
        .entries()
        .collect();
    if entries.len() != 1 {
        return Err(MutationError::InvalidFragment(
            "only one scalar key is allowed".to_owned(),
        ));
    }
    let key = entries[0]
        .key_node()
        .ok_or_else(|| MutationError::InvalidFragment("no scalar key".to_owned()))?;
    if !matches!(key, AstYamlNode::Scalar(_)) {
        return Err(MutationError::InvalidFragment(
            "only one scalar key is allowed".to_owned(),
        ));
    }
    let range = key
        .as_node()
        .map(yaml_edit::advanced::syntax_node_range)
        .map(|range| u32::from(range.start()) as usize..u32::from(range.end()) as usize)
        .ok_or_else(|| MutationError::InvalidFragment("no scalar key".to_owned()))?;
    if probe.get(range) != Some(raw) {
        return Err(MutationError::InvalidFragment(
            "only one scalar key is allowed".to_owned(),
        ));
    }
    Ok(())
}

fn validate_entry_fragment(key: &str, value: &str, flow: bool) -> Result<(), MutationError> {
    let probe = match flow {
        true => format!("{{{key}: {value}}}\n"),
        false => format!("{key}: {value}\n"),
    };
    let document = parse_probe(&probe)?;
    let entries: Vec<_> = document
        .as_mapping()
        .ok_or_else(|| MutationError::InvalidFragment("not one mapping entry".to_owned()))?
        .entries()
        .collect();
    if entries.len() != 1 {
        return Err(MutationError::InvalidFragment(
            "only one mapping entry is allowed".to_owned(),
        ));
    }
    match (entries[0].key_node(), entries[0].value_node()) {
        (Some(AstYamlNode::Scalar(_)), Some(value)) if value_range(&value).is_some() => Ok(()),
        _ => Err(MutationError::InvalidFragment(
            "only scalar keys and values are added from the tree".to_owned(),
        )),
    }
}

fn validate_item_fragment(value: &str, flow: bool) -> Result<(), MutationError> {
    let probe = match flow {
        true => format!("[{value}]\n"),
        false => format!("- {value}\n"),
    };
    let document = parse_probe(&probe)?;
    let sequence = document
        .as_sequence()
        .ok_or_else(|| MutationError::InvalidFragment("not one sequence item".to_owned()))?;
    if sequence.len() != 1 {
        return Err(MutationError::InvalidFragment(
            "only one sequence item is allowed".to_owned(),
        ));
    }
    match sequence.get(0) {
        Some(value) if value_range(&value).is_some() => Ok(()),
        _ => Err(MutationError::InvalidFragment(
            "only scalar sequence items are added from the tree".to_owned(),
        )),
    }
}

fn insert_member(
    text: &str,
    projection: &YamlProjection,
    index: usize,
    key: &str,
    value: &str,
) -> Result<Vec<(Range<usize>, String)>, MutationError> {
    let node = &projection.nodes()[index];
    let range = node
        .container_range
        .clone()
        .ok_or(MutationError::Unsupported(UnsupportedEdit::NotContainer))?;
    let line = format!("{key}: {value}");
    if node.flow {
        let close = flow_close(text, range.clone())?;
        let prefix = flow_insert_prefix(text, close, !node.children.is_empty());
        return Ok(vec![(close..close, format!("{prefix}{line}"))]);
    }
    let Some(last) = node
        .children
        .last()
        .and_then(|&child| projection.nodes().get(child))
    else {
        return Err(MutationError::Unsupported(UnsupportedEdit::NotContainer));
    };
    let insert = block_insert_position(text, last);
    let indent = line_indent(text, last.range.start);
    let line_ending = line_ending_at(text, insert, last.range.end);
    let prefix = newline_prefix(text, insert, line_ending);
    Ok(vec![(
        insert..insert,
        format!("{prefix}{indent}{line}{line_ending}"),
    )])
}

fn insert_item(
    text: &str,
    projection: &YamlProjection,
    index: usize,
    value: &str,
) -> Result<Vec<(Range<usize>, String)>, MutationError> {
    let node = &projection.nodes()[index];
    let range = node
        .container_range
        .clone()
        .ok_or(MutationError::Unsupported(UnsupportedEdit::NotContainer))?;
    let line = match node.flow {
        true => value.to_owned(),
        false => format!("- {value}"),
    };
    if node.flow {
        let close = flow_close(text, range.clone())?;
        let prefix = flow_insert_prefix(text, close, !node.children.is_empty());
        return Ok(vec![(close..close, format!("{prefix}{line}"))]);
    }
    let Some(last) = node
        .children
        .last()
        .and_then(|&child| projection.nodes().get(child))
    else {
        return Err(MutationError::Unsupported(UnsupportedEdit::NotContainer));
    };
    let insert = block_insert_position(text, last);
    let indent = line_indent(text, last.range.start);
    let line_ending = line_ending_at(text, insert, last.range.end);
    let prefix = newline_prefix(text, insert, line_ending);
    Ok(vec![(
        insert..insert,
        format!("{prefix}{indent}{line}{line_ending}"),
    )])
}

fn remove(
    text: &str,
    projection: &YamlProjection,
    index: usize,
) -> Result<Vec<(Range<usize>, String)>, MutationError> {
    let node = &projection.nodes()[index];
    if !(node.entry || node.item) {
        return Err(MutationError::Unsupported(UnsupportedEdit::NotContainer));
    }
    if let Some(name) = &node.anchor
        && node.alias_refs > 0
    {
        return Err(MutationError::AliasesReference {
            name: Some(name.clone()),
            count: node.alias_refs,
        });
    }
    if let Some(name) = &node.key_anchor
        && node.key_alias_refs > 0
    {
        return Err(MutationError::AliasesReference {
            name: Some(name.clone()),
            count: node.key_alias_refs,
        });
    }
    let parent = node
        .parent
        .and_then(|parent| projection.nodes().get(parent))
        .ok_or(MutationError::Unsupported(UnsupportedEdit::NotContainer))?;
    let range = if parent.flow {
        flow_removal_range(text, parent, node)?
    } else {
        if parent.children.len() == 1 {
            let replacements = empty_last_block(text, parent)?;
            if let Some((name, count)) = alias_conflict(projection, &replacements[0].0, index) {
                return Err(MutationError::AliasesReference {
                    name: Some(name),
                    count,
                });
            }
            return Ok(replacements);
        }
        let start = line_start(text, node.range.start);
        let end = block_entry_end(text, node.range.end);
        start..end
    };
    if let Some((name, count)) = alias_conflict(projection, &range, index) {
        return Err(MutationError::AliasesReference {
            name: Some(name),
            count,
        });
    }
    Ok(vec![(range, String::new())])
}

/// Removing the last member of a block collection leaves no block syntax to
/// represent it. Replace the member range with the empty flow form instead of
/// refusing a legitimate edit; the key line, indentation and comments stay.
fn empty_last_block(
    text: &str,
    parent: &YamlNode,
) -> Result<Vec<(Range<usize>, String)>, MutationError> {
    let kind = parent
        .container
        .ok_or(MutationError::Unsupported(UnsupportedEdit::NotContainer))?;
    let empty = match kind {
        ContainerKind::Mapping => "{}",
        ContainerKind::Sequence => "[]",
    };
    let range = parent
        .container_range
        .clone()
        .ok_or(MutationError::Unsupported(UnsupportedEdit::NotContainer))?;
    if parent.flow {
        return Err(MutationError::Stale);
    }
    // The collection range starts at its first member, so replacing just that
    // range keeps the key line, its indentation and any comment intact. A block
    // collection node may own the line break after its last member; keep that
    // terminator instead of gluing the next key onto the empty flow form.
    let ending = match text.get(range.clone()).unwrap_or_default() {
        span if span.ends_with("\r\n") => "\r\n",
        span if span.ends_with('\n') => "\n",
        _ => "",
    };
    Ok(vec![(range, format!("{empty}{ending}"))])
}

fn alias_conflict(
    projection: &YamlProjection,
    range: &Range<usize>,
    removed: usize,
) -> Option<(String, usize)> {
    let mut conflict: Option<(String, usize)> = None;
    for node in projection.nodes() {
        let Some(name) = &node.alias else {
            continue;
        };
        if node.alias_target == Some(removed) || range.contains(&node.range.start) {
            continue;
        }
        let Some(target) = node
            .alias_target
            .and_then(|target| projection.nodes().get(target))
        else {
            continue;
        };
        if !overlaps(&target.content_range, range) {
            continue;
        }
        match &mut conflict {
            Some((existing, count)) if existing == name => *count += 1,
            Some(_) => {}
            None => conflict = Some((name.clone(), 1)),
        }
    }
    conflict
}

fn overlaps(left: &Range<usize>, right: &Range<usize>) -> bool {
    left.start < right.end && right.start < left.end
}

fn flow_close(text: &str, range: Range<usize>) -> Result<usize, MutationError> {
    let span = text.get(range.clone()).ok_or(MutationError::Stale)?;
    let trimmed = span.trim_end_matches([' ', '\t', '\r', '\n']);
    let close = range
        .start
        .checked_add(trimmed.len())
        .and_then(|end| end.checked_sub(1))
        .ok_or(MutationError::Stale)?;
    match text.as_bytes().get(close) {
        Some(b'}') | Some(b']') => Ok(close),
        _ => Err(MutationError::Unsupported(UnsupportedEdit::NotContainer)),
    }
}

fn flow_insert_prefix(text: &str, close: usize, has_children: bool) -> &'static str {
    if !has_children {
        return "";
    }
    match text.get(..close).is_some_and(|before| {
        before
            .trim_end_matches([' ', '\t', '\r', '\n'])
            .ends_with(',')
    }) {
        true => " ",
        false => ", ",
    }
}

fn flow_removal_range(
    text: &str,
    parent: &YamlNode,
    node: &YamlNode,
) -> Result<Range<usize>, MutationError> {
    let range = parent
        .container_range
        .clone()
        .ok_or(MutationError::Unsupported(UnsupportedEdit::NotContainer))?;
    let close = flow_close(text, range.clone())?;
    let open = range.start.saturating_add(1);
    let after = text
        .get(node.range.end..close)
        .and_then(|tail| tail.find(','))
        .map(|offset| node.range.end + offset);
    if let Some(comma) = after {
        let mut end = comma + 1;
        while let Some(byte) = text.as_bytes().get(end) {
            match byte {
                b' ' | b'\t' => end += 1,
                _ => break,
            }
        }
        return Ok(node.range.start..end);
    }
    let before = text
        .get(open..node.range.start)
        .and_then(|head| head.rfind(','))
        .map(|offset| open + offset);
    match before {
        Some(comma) => Ok(comma..node.range.end),
        None => Ok(node.range.clone()),
    }
}

fn block_insert_position(text: &str, last: &YamlNode) -> usize {
    block_entry_end(text, last.range.end)
}

fn block_entry_end(text: &str, end: usize) -> usize {
    let end = end.min(text.len());
    if end > 0 && text.as_bytes().get(end - 1) == Some(&b'\n') {
        return end;
    }
    text[end..]
        .find('\n')
        .map_or(text.len(), |offset| end + offset + 1)
}

fn newline_prefix(text: &str, insert: usize, line_ending: &'static str) -> &'static str {
    if insert == 0 || text.as_bytes().get(insert.saturating_sub(1)) == Some(&b'\n') {
        ""
    } else {
        line_ending
    }
}

fn line_ending_at(text: &str, insert: usize, fallback: usize) -> &'static str {
    let at = insert.min(text.len());
    match text.get(at..).and_then(|tail| tail.find('\n')) {
        Some(offset) => {
            let newline = at + offset;
            match newline > 0 && text.as_bytes().get(newline - 1) == Some(&b'\r') {
                true => "\r\n",
                false => "\n",
            }
        }
        None => {
            let base = fallback.min(text.len());
            match text.get(base..).and_then(|tail| tail.find('\n')) {
                Some(offset) => {
                    let newline = base + offset;
                    match newline > 0 && text.as_bytes().get(newline - 1) == Some(&b'\r') {
                        true => "\r\n",
                        false => "\n",
                    }
                }
                None => "\n",
            }
        }
    }
}

fn line_start(text: &str, offset: usize) -> usize {
    let offset = offset.min(text.len());
    text[..offset].rfind('\n').map_or(0, |at| at + 1)
}

fn line_indent(text: &str, offset: usize) -> &str {
    let start = line_start(text, offset);
    let rest = &text[start..];
    let len = rest
        .as_bytes()
        .iter()
        .take_while(|byte| matches!(byte, b' ' | b'\t'))
        .count();
    &rest[..len]
}

fn verify_result(text: &str, replacements: &[(Range<usize>, String)]) -> Result<(), MutationError> {
    let mut result = text.to_owned();
    let mut ordered: Vec<_> = replacements.iter().collect();
    ordered.sort_by_key(|(range, _)| std::cmp::Reverse(range.start));
    for (range, inserted) in ordered {
        if range.end > result.len()
            || !result.is_char_boundary(range.start)
            || !result.is_char_boundary(range.end)
        {
            return Err(MutationError::Stale);
        }
        result.replace_range(range.clone(), inserted);
    }
    let parse = YamlFile::parse(&result);
    let Some(first) = parse.positioned_errors().first() else {
        return Ok(());
    };
    let position = first.start_position(&result);
    Err(MutationError::ResultInvalid {
        line: position.line,
        column: position.column,
        message: first.message.clone(),
    })
}
