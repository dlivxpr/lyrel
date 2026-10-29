use super::projection::Kind;
use markdown::mdast::Node;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Code {
    Fenced { marker: u8, width: usize },
    Indented,
}

pub(super) fn classify(node: &Node, source: &str) -> Kind {
    match node {
        Node::Yaml(_) | Node::Toml(_) => Kind::Metadata,
        Node::Html(_) => Kind::Html,
        Node::Paragraph(_) if contains_html(node) => Kind::Html,
        Node::Paragraph(_) => Kind::Prose,
        Node::Heading(heading) => Kind::Heading(heading.depth),
        Node::ThematicBreak(_) => Kind::Rule,
        Node::Math(_) => Kind::Math,
        Node::Code(_) => {
            let opening = source.lines().next().unwrap_or_default().trim_start();
            let marker = opening.as_bytes().first().copied().unwrap_or_default();
            let width = opening.bytes().take_while(|byte| *byte == marker).count();
            Kind::Code(if matches!(marker, b'`' | b'~') && width >= 3 {
                Code::Fenced { marker, width }
            } else {
                Code::Indented
            })
        }
        _ => Kind::Source,
    }
}

fn contains_html(node: &Node) -> bool {
    matches!(node, Node::Html(_))
        || node
            .children()
            .is_some_and(|children| children.iter().any(contains_html))
}

pub(super) fn unfinished_metadata(source: &str, document_start: bool) -> bool {
    document_start
        && source.starts_with("---\n")
        && !source
            .lines()
            .skip(1)
            .any(|line| matches!(line, "---" | "..."))
}

pub(super) fn label(kind: Kind) -> Option<crate::ui::i18n::L10nKey> {
    use crate::ui::i18n::L10nKey;
    match kind {
        Kind::Code(_) => Some(L10nKey::EditorMarkdownCodeRegion),
        Kind::Metadata => Some(L10nKey::EditorMarkdownMetadataRegion),
        Kind::Html => Some(L10nKey::EditorMarkdownHtmlRegion),
        Kind::Source => Some(L10nKey::EditorMarkdownSourceRegion),
        _ => None,
    }
}

/// Delimiters stay ordinary editable text. The returned prefix is removed only
/// for a collapsed caret leaving an empty indented-code line.
pub(super) fn line_break(kind: Kind, text: &str, offset: usize) -> (usize, String) {
    let start = text[..offset].rfind('\n').map_or(0, |ix| ix + 1);
    let end = text[offset..]
        .find('\n')
        .map_or(text.len(), |ix| offset + ix);
    let line = &text[start..end];
    if let Kind::Code(Code::Fenced { marker, width }) = kind {
        let trimmed = line.trim_start();
        let count = trimmed.bytes().take_while(|byte| *byte == marker).count();
        if start > 0 && count >= width && trimmed[count..].trim().is_empty() && offset == end {
            return (0, "\n\n".into());
        }
    }
    if kind == Kind::Code(Code::Indented) && offset == end && line.trim().is_empty() {
        return (offset - start, "\n".into());
    }
    let before = &text[start..offset];
    let indent = &before[..before
        .find(|ch| !matches!(ch, ' ' | '\t'))
        .unwrap_or(before.len())];
    (0, format!("\n{indent}"))
}
