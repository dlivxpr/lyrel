//! Link syntax and destinations are derived from the document's positioned AST.
use crate::ui::i18n::L10nKey;
use markdown::mdast::Node;
use std::{
    ops::Range,
    path::{Path, PathBuf},
};

#[derive(Clone, PartialEq, Eq)]
pub(super) struct Link {
    pub source: Range<usize>,
    pub label: Range<usize>,
    pub destination: Range<usize>,
    pub target: String,
}
#[derive(Default)]
pub(super) struct Index {
    pub links: Vec<Link>,
    pub headings: Vec<(String, usize)>,
}
impl Index {
    pub fn from_ast(source: &str, root: &Node) -> Self {
        let mut definitions = std::collections::HashMap::new();
        visit(root, &mut |node| {
            if let Node::Definition(definition) = node {
                if let Some(position) = &definition.position {
                    let range = position.start.offset..position.end.offset;
                    if let Some(label) = label_range(source, node) {
                        if let Some(destination) =
                            destination_range(source, label.end + 2, range.end)
                        {
                            definitions
                                .entry(definition.identifier.clone())
                                .or_insert_with(|| (definition.url.clone(), destination));
                        }
                    }
                }
            }
        });
        let mut index = Self::default();
        let mut slugs = std::collections::HashMap::<String, usize>::new();
        visit(root, &mut |node| {
            let Some(position) = node.position() else {
                return;
            };
            let range = position.start.offset..position.end.offset;
            if let Node::Heading(_) = node {
                let slug = slug(&node.to_string());
                let count = slugs.entry(slug.clone()).or_default();
                let unique = if *count == 0 {
                    slug
                } else {
                    format!("{slug}-{count}")
                };
                *count += 1;
                index.headings.push((unique, range.start));
            }
            let (label, target, destination) = match node {
                Node::Link(link) => {
                    let raw = &source[range.clone()];
                    if raw.starts_with('<') {
                        let inner = range.start + 1..range.end - 1;
                        (inner.clone(), link.url.clone(), inner)
                    } else if let Some(label) = label_range(source, node) {
                        let Some(destination) = destination_range(source, label.end + 2, range.end)
                        else {
                            return;
                        };
                        (label, link.url.clone(), destination)
                    } else {
                        // GFM literal autolinks have no delimiters to reveal.
                        (range.clone(), link.url.clone(), range.clone())
                    }
                }
                Node::LinkReference(reference) => {
                    let Some((target, destination)) = definitions.get(&reference.identifier) else {
                        return;
                    };
                    let Some(label) = label_range(source, node) else {
                        return;
                    };
                    (label, target.clone(), destination.clone())
                }
                _ => return,
            };
            index.links.push(Link {
                source: range,
                label,
                destination,
                target,
            });
        });
        index
    }
    pub fn at(&self, at: usize) -> Option<&Link> {
        self.links
            .iter()
            .find(|link| link.source.start <= at && at < link.source.end)
    }
    pub fn anchor(&self, fragment: &str) -> Option<usize> {
        let fragment = percent_encoding::percent_decode_str(fragment)
            .decode_utf8()
            .ok()?;
        self.headings
            .iter()
            .find(|(slug, _)| slug == fragment.as_ref())
            .map(|(_, at)| *at)
    }
}
fn visit(node: &Node, f: &mut impl FnMut(&Node)) {
    f(node);
    for child in node.children().into_iter().flatten() {
        visit(child, f);
    }
}
fn slug(text: &str) -> String {
    text.chars()
        .flat_map(char::to_lowercase)
        .filter_map(|ch| {
            if ch.is_whitespace() {
                Some('-')
            } else if ch.is_alphanumeric() || ch == '-' || ch == '_' {
                Some(ch)
            } else {
                None
            }
        })
        .collect()
}
/// Only scans within an AST-identified link/definition, never searches rendered text.
pub(super) fn label_range(source: &str, node: &Node) -> Option<Range<usize>> {
    let position = node.position()?;
    let mut range = position.start.offset..position.end.offset;
    // Definition positions include their permitted leading indentation in
    // both grammars; inspect only that AST-owned region, not rendered labels.
    if matches!(node, Node::Definition(_)) {
        range.start += source[range.clone()]
            .bytes()
            .take_while(|byte| matches!(byte, b' ' | b'\t'))
            .count();
    }
    if source.as_bytes().get(range.start) != Some(&b'[') {
        return None;
    }
    // Positioned children already account for nested brackets, code spans,
    // escaped punctuation and empty labels. The following byte closes the label.
    if let Some(end) = node
        .children()
        .and_then(|children| children.last())
        .and_then(Node::position)
        .map(|position| position.end.offset)
    {
        if source.as_bytes().get(end) == Some(&b']') {
            return Some(range.start + 1..end);
        }
    }
    let mut escaped = false;
    for (ix, byte) in source.as_bytes()[range.clone()].iter().enumerate().skip(1) {
        if escaped {
            escaped = false;
            continue;
        }
        if *byte == b'\\' {
            escaped = true;
            continue;
        }
        if *byte == b']' {
            return Some(range.start + 1..range.start + ix);
        }
    }
    None
}
fn destination_range(source: &str, mut start: usize, end: usize) -> Option<Range<usize>> {
    let bytes = source.as_bytes();
    while start < end && bytes[start].is_ascii_whitespace() {
        start += 1;
    }
    if start >= end {
        return None;
    }
    if bytes[start] == b'<' {
        let mut at = start + 1;
        while at < end {
            if bytes[at] == b'\\' {
                at += 2;
                continue;
            }
            if bytes[at] == b'>' {
                return Some(start + 1..at);
            }
            at += 1;
        }
        return None;
    }
    let mut depth = 0usize;
    let mut at = start;
    while at < end {
        match bytes[at] {
            b'\\' => {
                at = (at + 2).min(end);
                continue;
            }
            b'(' => depth += 1,
            b')' if depth == 0 => break,
            b')' => depth -= 1,
            byte if byte.is_ascii_whitespace() && depth == 0 => break,
            _ => {}
        }
        at += 1;
    }
    Some(start..at)
}

pub(crate) enum Target {
    External(String),
    File(PathBuf),
    Anchor,
}
/// Resolve against the owning Host, without filesystem access on the UI thread.
pub(crate) fn resolve(owner: &Path, local: bool, target: &str) -> Result<Target, L10nKey> {
    if target.starts_with('#') {
        return Ok(Target::Anchor);
    }
    if let Ok(url) = url::Url::parse(target) {
        return match url.scheme() {
            "http" | "https" | "mailto" => Ok(Target::External(url.into())),
            "file" if local => url
                .to_file_path()
                .map(Target::File)
                .map_err(|_| L10nKey::MarkdownLinkInvalidDestination),
            _ => Err(L10nKey::MarkdownLinkUnsupportedScheme),
        };
    }
    if target.starts_with("//") {
        return Err(L10nKey::MarkdownLinkUnsupportedScheme);
    }
    let path = target.split(['#', '?']).next().unwrap_or("");
    if path.is_empty() {
        return Ok(Target::File(owner.to_path_buf()));
    }
    let decoded = percent_encoding::percent_decode_str(path)
        .decode_utf8()
        .map_err(|_| L10nKey::MarkdownLinkInvalidDestination)?;
    if decoded.contains('\0') {
        return Err(L10nKey::MarkdownLinkInvalidDestination);
    }
    if local {
        let path = Path::new(decoded.as_ref());
        let path = if path.is_absolute() {
            path.to_path_buf()
        } else {
            owner
                .parent()
                .ok_or(L10nKey::MarkdownLinkSaveFirst)?
                .join(path)
        };
        let mut normalized = PathBuf::new();
        for component in path.components() {
            if component == std::path::Component::ParentDir {
                normalized.pop();
            } else if component != std::path::Component::CurDir {
                normalized.push(component);
            }
        }
        return Ok(Target::File(normalized));
    }
    // SSH paths always use POSIX semantics, including on a Windows client.
    if decoded.contains('\\') || decoded.as_bytes().get(1) == Some(&b':') {
        return Err(L10nKey::MarkdownLinkRemotePath);
    }
    let owner = owner
        .to_str()
        .ok_or(L10nKey::MarkdownLinkInvalidDestination)?;
    let joined = if decoded.starts_with('/') {
        decoded.into_owned()
    } else {
        let (parent, _) = owner
            .rsplit_once('/')
            .ok_or(L10nKey::MarkdownLinkSaveFirst)?;
        format!("{parent}/{decoded}")
    };
    let mut parts = Vec::new();
    for part in joined.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            part => parts.push(part),
        }
    }
    Ok(Target::File(PathBuf::from(format!("/{}", parts.join("/")))))
}
