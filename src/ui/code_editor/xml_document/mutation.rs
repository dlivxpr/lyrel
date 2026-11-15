//! #294 XML 的局部源编辑规划。
//!
//! 命令只针对当前投影里的一个确切 occurrence：目标用
//! `(path, kind, range, key_range, value_range)` 重新解析，可见行索引与元素名
//! 都不是身份。规划器在当前 LF 源码上重新取得精确 token 范围，算出最小替换
//! 清单；绝不把整篇 XML 转成对象再序列化，也不做全 source 格式化。
//!
//! 用户输入按源码语法处理（与 JSON 树的值字段一致）：文本／属性值／CDATA／
//! 注释各自校验，生成的片段先经过同一解析器复核，写入前再解析整篇结果。
//! 命名空间声明、前缀与 CDATA／注释边界都不会被无关编辑改动；删除或重命名
//! 若会留下未解析前缀或损坏文档，命令在写入前拒绝。
//!
//! 应用由调用方交给共享 `Document` 的 `replace_ranges`：一次命令是一条普通
//! history 记录，撤销／重做与其他源码编辑共用同一权威。

use std::ops::Range;
use std::sync::atomic::AtomicBool;

use xmlparser::{ElementEnd, Token, Tokenizer};

use super::projection::{
    ProjectionFailure, XML_INPUT_BYTES, XmlNode, XmlNodeKind, XmlProjection, project,
};

/// 目标节点的稳定身份。重新解析时必须全部匹配，不能只按 path 或元素名猜测。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct NodeTarget {
    pub(crate) path: String,
    pub(crate) kind: XmlNodeKind,
    pub(crate) range: Range<usize>,
    pub(crate) key_range: Option<Range<usize>>,
    pub(crate) value_range: Option<Range<usize>>,
}

impl NodeTarget {
    pub(crate) fn of(node: &XmlNode) -> Self {
        Self {
            path: node.path.clone(),
            kind: node.kind,
            range: node.range.clone(),
            key_range: node.key_range.clone(),
            value_range: node.value_range.clone(),
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) enum Mutation {
    /// 替换属性值、文本、CDATA 正文或注释正文的原始源码片段。
    SetValue { target: NodeTarget, value: String },
    /// 只替换选中元素或属性的限定名。
    Rename { target: NodeTarget, name: String },
    /// 在选中元素起始标签内加入属性。
    AddAttribute {
        element: NodeTarget,
        name: String,
        value: String,
    },
    /// 在选中元素内容末尾加入空子元素。
    AddChild { parent: NodeTarget, name: String },
    /// 在选中元素内容末尾加入文本。
    AddText { parent: NodeTarget, text: String },
    /// 删除选中的属性、文本、CDATA、注释或非根元素。
    Remove { target: NodeTarget },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum MutationError {
    /// 文档 revision 已变化，或目标 occurrence 已不存在。
    Stale,
    /// 选中的节点不能执行这个命令（文档、根元素、PI 等）。
    Unsupported,
    /// 输入不是合法的 XML 名称。
    InvalidName(String),
    /// 输入不是合法的属性值／文本。
    InvalidValue(String),
    /// CDATA 不能包含 `]]>`。
    InvalidCdata,
    /// 注释不能包含 `--` 或以 `-` 结尾。
    InvalidComment,
    /// 规划后的整篇源码无法解析或超出结构预算。
    ResultInvalid {
        line: usize,
        column: usize,
        message: String,
    },
}

/// 规划一次局部修改，返回按源码顺序排列、互不重叠的替换清单。
/// 空清单表示命令是 no-op。
pub(crate) fn plan(
    text: &str,
    projection: &XmlProjection,
    mutation: &Mutation,
) -> Result<Vec<(Range<usize>, String)>, MutationError> {
    let replacements = match mutation {
        Mutation::SetValue { target, value } => plan_set_value(text, projection, target, value)?,
        Mutation::Rename { target, name } => plan_rename(projection, target, name)?,
        Mutation::AddAttribute {
            element,
            name,
            value,
        } => plan_add_attribute(text, projection, element, name, value)?,
        Mutation::AddChild { parent, name } => plan_add_child(text, projection, parent, name)?,
        Mutation::AddText { parent, text: body } => plan_add_text(text, projection, parent, body)?,
        Mutation::Remove { target } => plan_remove(text, projection, target)?,
    };
    if !replacements.is_empty() {
        verify_result(text, &replacements)?;
    }
    Ok(replacements)
}

fn plan_set_value(
    text: &str,
    projection: &XmlProjection,
    target: &NodeTarget,
    value: &str,
) -> Result<Vec<(Range<usize>, String)>, MutationError> {
    let index = resolve(projection, target)?;
    let node = &projection.nodes()[index];
    let range = node.value_range.clone().ok_or(MutationError::Unsupported)?;
    match node.kind {
        XmlNodeKind::Attribute => {
            let quote = node.quote.ok_or(MutationError::Unsupported)?;
            validate_attribute_value(value, quote)?;
            if text.get(range.clone()) == Some(value) {
                return Ok(Vec::new());
            }
            Ok(vec![(range, value.to_owned())])
        }
        XmlNodeKind::Text => {
            validate_text(value)?;
            if text.get(range.clone()) == Some(value) {
                return Ok(Vec::new());
            }
            Ok(vec![(range, value.to_owned())])
        }
        XmlNodeKind::Cdata => {
            validate_cdata(value)?;
            if text.get(range.clone()) == Some(value) {
                return Ok(Vec::new());
            }
            Ok(vec![(range, value.to_owned())])
        }
        XmlNodeKind::Comment => {
            validate_comment(value)?;
            if text.get(range.clone()) == Some(value) {
                return Ok(Vec::new());
            }
            Ok(vec![(range, value.to_owned())])
        }
        _ => Err(MutationError::Unsupported),
    }
}

fn plan_rename(
    projection: &XmlProjection,
    target: &NodeTarget,
    name: &str,
) -> Result<Vec<(Range<usize>, String)>, MutationError> {
    let index = resolve(projection, target)?;
    let node = &projection.nodes()[index];
    if !matches!(node.kind, XmlNodeKind::Element | XmlNodeKind::Attribute) {
        return Err(MutationError::Unsupported);
    }
    let range = node.key_range.clone().ok_or(MutationError::Unsupported)?;
    validate_name(name)?;
    if node.key == name {
        return Ok(Vec::new());
    }
    Ok(vec![(range, name.to_owned())])
}

fn plan_add_attribute(
    text: &str,
    projection: &XmlProjection,
    element: &NodeTarget,
    name: &str,
    value: &str,
) -> Result<Vec<(Range<usize>, String)>, MutationError> {
    let index = resolve(projection, element)?;
    let node = &projection.nodes()[index];
    if node.kind != XmlNodeKind::Element {
        return Err(MutationError::Unsupported);
    }
    validate_name(name)?;
    let quote = choose_quote(value);
    validate_attribute_value(value, quote)?;
    let at = node.tag_close_start;
    if at == 0 || at > text.len() {
        return Err(MutationError::Stale);
    }
    let separator = match text[..at].chars().next_back() {
        Some(ch) if ch.is_whitespace() => "",
        _ => " ",
    };
    let fragment = format!("{separator}{name}={quote}{value}{quote}");
    Ok(vec![(at..at, fragment)])
}

fn plan_add_child(
    text: &str,
    projection: &XmlProjection,
    parent: &NodeTarget,
    name: &str,
) -> Result<Vec<(Range<usize>, String)>, MutationError> {
    let index = resolve(projection, parent)?;
    let node = &projection.nodes()[index];
    if node.kind != XmlNodeKind::Element {
        return Err(MutationError::Unsupported);
    }
    validate_name(name)?;
    let content = format!("<{name}/>");
    insert_content(text, node, content)
}

fn plan_add_text(
    text: &str,
    projection: &XmlProjection,
    parent: &NodeTarget,
    body: &str,
) -> Result<Vec<(Range<usize>, String)>, MutationError> {
    let index = resolve(projection, parent)?;
    let node = &projection.nodes()[index];
    if node.kind != XmlNodeKind::Element {
        return Err(MutationError::Unsupported);
    }
    if body.is_empty() {
        return Ok(Vec::new());
    }
    validate_text(body)?;
    insert_content(text, node, body.to_owned())
}

fn insert_content(
    text: &str,
    node: &XmlNode,
    content: String,
) -> Result<Vec<(Range<usize>, String)>, MutationError> {
    if node.self_closing {
        let range = node.tag_close_start..node.start_tag_end;
        if text.get(range.clone()) != Some("/>") {
            return Err(MutationError::Stale);
        }
        let qname = node.key.as_str();
        return Ok(vec![(range, format!(">{content}</{qname}>"))]);
    }
    let close = node.close_tag_start.ok_or(MutationError::Unsupported)?;
    let line_start = text[..close].rfind('\n').map_or(0, |at| at + 1);
    let indent = &text[line_start..close];
    if indent.chars().all(|ch| ch == ' ' || ch == '\t') {
        let unit = indent_unit(text, node.start_tag_end, close);
        let child_indent = format!("{indent}{unit}");
        Ok(vec![(
            line_start..line_start,
            format!("{child_indent}{content}\n{indent}"),
        )])
    } else {
        Ok(vec![(close..close, content)])
    }
}

fn plan_remove(
    text: &str,
    projection: &XmlProjection,
    target: &NodeTarget,
) -> Result<Vec<(Range<usize>, String)>, MutationError> {
    let index = resolve(projection, target)?;
    let node = &projection.nodes()[index];
    match node.kind {
        XmlNodeKind::Document | XmlNodeKind::Declaration | XmlNodeKind::ProcessingInstruction => {
            Err(MutationError::Unsupported)
        }
        XmlNodeKind::Element if node.is_element_root() => Err(MutationError::Unsupported),
        XmlNodeKind::Attribute => {
            let parent = node.parent.ok_or(MutationError::Stale)?;
            let parent_node = &projection.nodes()[parent];
            let position = parent_node
                .children
                .iter()
                .position(|child| *child == index)
                .ok_or(MutationError::Stale)?;
            let start = match position.checked_sub(1) {
                Some(previous) => {
                    let previous = &projection.nodes()[parent_node.children[previous]];
                    if previous.kind == XmlNodeKind::Attribute {
                        previous.range.end
                    } else {
                        parent_node
                            .key_range
                            .as_ref()
                            .map(|range| range.end)
                            .ok_or(MutationError::Stale)?
                    }
                }
                None => parent_node
                    .key_range
                    .as_ref()
                    .map(|range| range.end)
                    .ok_or(MutationError::Stale)?,
            };
            if start > node.range.start || node.range.end > text.len() {
                return Err(MutationError::Stale);
            }
            Ok(vec![(start..node.range.end, String::new())])
        }
        XmlNodeKind::Element | XmlNodeKind::Comment | XmlNodeKind::Cdata => {
            let range = clean_line(text, node.range.clone());
            Ok(vec![(range, String::new())])
        }
        XmlNodeKind::Text => Ok(vec![(node.range.clone(), String::new())]),
    }
}

fn resolve(projection: &XmlProjection, target: &NodeTarget) -> Result<usize, MutationError> {
    projection
        .nodes()
        .iter()
        .position(|node| {
            node.path == target.path
                && node.kind == target.kind
                && node.range == target.range
                && node.key_range == target.key_range
                && node.value_range == target.value_range
        })
        .ok_or(MutationError::Stale)
}

/// 一个合法 XML 名称：probe 的起始标签必须恰好覆盖 `<name`，且没有
/// 额外的属性或内容。前缀绑定由整篇结果复核，不在名称探针里猜测。
fn validate_name(name: &str) -> Result<(), MutationError> {
    if name.is_empty() {
        return Err(MutationError::InvalidName("empty XML name".to_owned()));
    }
    let probe = format!("<{name}/>");
    let mut tokens = Tokenizer::from(probe.as_str());
    let first = tokens.next();
    let second = tokens.next();
    match (first, second) {
        (
            Some(Ok(Token::ElementStart { span, .. })),
            Some(Ok(Token::ElementEnd {
                end: ElementEnd::Empty,
                ..
            })),
        ) if span.as_str() == format!("<{name}") && tokens.next().is_none() => Ok(()),
        _ => Err(MutationError::InvalidName(format!(
            "not an XML name: {name}"
        ))),
    }
}

/// 文本内容按源码语法校验：不能有裸 `<` 或 `]]>`，实体必须可解析。
fn validate_text(value: &str) -> Result<(), MutationError> {
    if value.contains('<') {
        return Err(MutationError::InvalidValue(
            "text cannot contain a raw <".to_owned(),
        ));
    }
    if value.contains("]]>") {
        return Err(MutationError::InvalidValue(
            "text cannot contain ]]>".to_owned(),
        ));
    }
    let probe = format!("<r>{value}</r>");
    roxmltree::Document::parse(&probe)
        .map(|_| ())
        .map_err(|error| MutationError::InvalidValue(error.to_string()))
}

fn validate_attribute_value(value: &str, quote: char) -> Result<(), MutationError> {
    if value.contains('<') {
        return Err(MutationError::InvalidValue(
            "an attribute value cannot contain <".to_owned(),
        ));
    }
    if value.contains(quote) {
        return Err(MutationError::InvalidValue(format!(
            "the value contains its {quote} quote"
        )));
    }
    let probe = format!("<r a={quote}{value}{quote}/>");
    roxmltree::Document::parse(&probe)
        .map(|_| ())
        .map_err(|error| MutationError::InvalidValue(error.to_string()))
}

fn validate_cdata(value: &str) -> Result<(), MutationError> {
    if value.contains("]]>") {
        return Err(MutationError::InvalidCdata);
    }
    let probe = format!("<r><![CDATA[{value}]]></r>");
    roxmltree::Document::parse(&probe)
        .map(|_| ())
        .map_err(|error| MutationError::InvalidValue(error.to_string()))
}

fn validate_comment(value: &str) -> Result<(), MutationError> {
    if value.contains("--") || value.ends_with('-') {
        return Err(MutationError::InvalidComment);
    }
    let probe = format!("<r><!--{value}--></r>");
    roxmltree::Document::parse(&probe)
        .map(|_| ())
        .map_err(|error| MutationError::InvalidValue(error.to_string()))
}

fn choose_quote(value: &str) -> char {
    if value.contains('"') && !value.contains('\'') {
        '\''
    } else {
        '"'
    }
}

fn indent_unit(text: &str, start: usize, end: usize) -> &'static str {
    match text
        .get(start..end)
        .is_some_and(|content| content.lines().any(|line| line.starts_with('\t')))
    {
        true => "\t",
        false => "  ",
    }
}

/// 删除整个元素／注释／CDATA 时把行首缩进与行尾换行一起去掉；只删行内片段
/// 时不碰换行，避免把父元素的闭合标签挪到奇怪的位置。
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

/// 写入前用同一投影复核整篇结果：任何会破坏良构性、命名空间绑定或结构
/// 预算的替换都拒绝，而不是留下损坏文件。
fn verify_result(text: &str, replacements: &[(Range<usize>, String)]) -> Result<(), MutationError> {
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
    if result.len() > XML_INPUT_BYTES {
        return Err(MutationError::ResultInvalid {
            line: 1,
            column: 1,
            message: "the edited source exceeds the XML structure budget".to_owned(),
        });
    }
    match project(&result, &AtomicBool::new(false)) {
        Ok(projection) => match projection.error() {
            Some(error) => Err(MutationError::ResultInvalid {
                line: error.line,
                column: error.column,
                message: error.message.clone(),
            }),
            None => Ok(()),
        },
        Err(ProjectionFailure::DtdDenied { line, column }) => Err(MutationError::ResultInvalid {
            line,
            column,
            message: "DTDs and external entities are not accessed".to_owned(),
        }),
        Err(ProjectionFailure::InputTooLarge) => Err(MutationError::ResultInvalid {
            line: 1,
            column: 1,
            message: "the edited source exceeds the XML structure budget".to_owned(),
        }),
        Err(ProjectionFailure::TooManyNodes) => Err(MutationError::ResultInvalid {
            line: 1,
            column: 1,
            message: "the edited source exceeds the XML node budget".to_owned(),
        }),
        Err(ProjectionFailure::PathsTooLarge) => Err(MutationError::ResultInvalid {
            line: 1,
            column: 1,
            message: "the edited source exceeds the XML path budget".to_owned(),
        }),
        Err(ProjectionFailure::Cancelled) => Ok(()),
    }
}

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

    fn cancel() -> AtomicBool {
        AtomicBool::new(false)
    }

    fn build(text: &str) -> XmlProjection {
        project(text, &cancel()).expect("fixture projects")
    }

    fn target(projection: &XmlProjection, path: &str) -> NodeTarget {
        NodeTarget::of(
            projection
                .nodes()
                .iter()
                .find(|node| node.path == path)
                .unwrap_or_else(|| panic!("missing {path}")),
        )
    }

    fn mutate(text: &str, projection: &XmlProjection, mutation: Mutation) -> String {
        let replacements = plan(text, projection, &mutation).expect("mutation plans");
        apply(text, &replacements)
    }

    const MIXED: &str = "<r xmlns=\"urn:root\" xmlns:p=\"urn:part\" p:flag='old&amp;raw'>前<![CDATA[甲\n乙]]><!--注--><p:item/>后</r>";

    #[test]
    fn local_edits_preserve_namespaces_mixed_order_and_untouched_spelling() {
        let projection = build(MIXED);
        let edited = mutate(
            MIXED,
            &projection,
            Mutation::SetValue {
                target: target(&projection, "/r/@p:flag"),
                value: "new&lt;raw".to_owned(),
            },
        );
        assert!(edited.contains("p:flag='new&lt;raw'"));
        assert!(edited.contains("xmlns=\"urn:root\""));
        assert!(edited.contains("<![CDATA[甲\n乙]]>"));
        assert!(edited.contains("<!--注-->"));
        assert!(edited.contains("<p:item/>"));
        assert!(edited.find("前").unwrap() < edited.find("<![CDATA[").unwrap());
        assert!(edited.find("<![CDATA[").unwrap() < edited.find("<!--注-->").unwrap());

        let projection = build(&edited);
        let edited = mutate(
            &edited,
            &projection,
            Mutation::SetValue {
                target: target(&projection, "/r/cdata()[1]"),
                value: "甲\n丙".to_owned(),
            },
        );
        assert!(edited.contains("<![CDATA[甲\n丙]]>"));
        assert!(edited.contains("<!--注-->"));
        assert!(edited.contains("p:flag='new&lt;raw'"));

        let projection = build(&edited);
        let edited = mutate(
            &edited,
            &projection,
            Mutation::SetValue {
                target: target(&projection, "/r/comment()[1]"),
                value: "新注释".to_owned(),
            },
        );
        assert!(edited.contains("<!--新注释-->"));
        assert!(edited.contains("<p:item/>"));
    }

    #[test]
    fn add_child_attribute_and_text_keep_other_bytes_and_order() {
        let text = "<r>\n  <a/>\n</r>\n";
        let projection = build(text);
        let edited = mutate(
            text,
            &projection,
            Mutation::AddChild {
                parent: target(&projection, "/r"),
                name: "b".to_owned(),
            },
        );
        assert_eq!(edited, "<r>\n  <a/>\n  <b/>\n</r>\n");
        let projection = build(&edited);
        let edited = mutate(
            &edited,
            &projection,
            Mutation::AddAttribute {
                element: target(&projection, "/r/a[1]"),
                name: "id".to_owned(),
                value: "1&amp;2".to_owned(),
            },
        );
        assert_eq!(edited, "<r>\n  <a id=\"1&amp;2\"/>\n  <b/>\n</r>\n");
        let projection = build(&edited);
        let edited = mutate(
            &edited,
            &projection,
            Mutation::AddText {
                parent: target(&projection, "/r/b[1]"),
                text: "text".to_owned(),
            },
        );
        assert_eq!(edited, "<r>\n  <a id=\"1&amp;2\"/>\n  <b>text</b>\n</r>\n");

        let projection = build(&edited);
        let edited = mutate(
            &edited,
            &projection,
            Mutation::Remove {
                target: target(&projection, "/r/a[1]/@id"),
            },
        );
        assert_eq!(edited, "<r>\n  <a/>\n  <b>text</b>\n</r>\n");
        let projection = build(&edited);
        let edited = mutate(
            &edited,
            &projection,
            Mutation::Rename {
                target: target(&projection, "/r/b[1]"),
                name: "c".to_owned(),
            },
        );
        assert_eq!(edited, "<r>\n  <a/>\n  <c>text</c>\n</r>\n");
    }

    #[test]
    fn namespace_breaking_and_invalid_fragments_are_rejected_before_write() {
        let text = "<r xmlns:p=\"urn:part\"><p:a/></r>";
        let projection = build(text);
        assert!(matches!(
            plan(
                text,
                &projection,
                &Mutation::Remove {
                    target: target(&projection, "/r/@xmlns:p"),
                }
            ),
            Err(MutationError::ResultInvalid { .. })
        ));
        assert!(matches!(
            plan(
                text,
                &projection,
                &Mutation::Rename {
                    target: target(&projection, "/r/p:a[1]"),
                    name: "q:a".to_owned(),
                }
            ),
            Err(MutationError::ResultInvalid { .. })
        ));
        assert!(matches!(
            plan(
                text,
                &projection,
                &Mutation::AddText {
                    parent: target(&projection, "/r"),
                    text: "a < b".to_owned(),
                }
            ),
            Err(MutationError::InvalidValue(_))
        ));
        let broken_comment = "<r><!--ok--></r>";
        let projection = build(broken_comment);
        assert!(matches!(
            plan(
                broken_comment,
                &projection,
                &Mutation::SetValue {
                    target: target(&projection, "/r/comment()[1]"),
                    value: "a--b".to_owned(),
                }
            ),
            Err(MutationError::InvalidComment)
        ));
    }
}
