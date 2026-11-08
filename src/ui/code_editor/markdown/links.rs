//! Link syntax and destinations are derived from the document's positioned AST.
use crate::ui::host_ops::Host;
use crate::ui::i18n::L10nKey;
use markdown::mdast::Node;
use std::{
    ops::Range,
    path::{Path, PathBuf},
};

pub(crate) mod references;

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
    pub embeds: Vec<Link>,
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
            if matches!(node, Node::Text(_)) {
                for (opening, closing) in [("[[", "]]"), ("((", "))")] {
                    let raw = &source[range.clone()];
                    let mut cursor = 0;
                    while let Some(start) = raw[cursor..].find(opening).map(|at| cursor + at) {
                        let body = start + opening.len();
                        let Some(end) = raw[body..].find(closing).map(|at| body + at) else { break; };
                        cursor = end + closing.len();
                        if start > 0 && raw.as_bytes()[start - 1] == b'\\' { continue; }
                        let embed = start > 0 && raw.as_bytes()[start - 1] == b'!';
                        if embed && start > 1 && raw.as_bytes()[start - 2] == b'\\' { continue; }
                        let inside = &raw[body..end];
                        let (target, label) = inside.split_once('|').unwrap_or((inside, inside));
                        if target.is_empty() || target.contains('\n') || (opening == "((" && !target.contains('#')) { continue; }
                        let destination = range.start + body..range.start + body + target.len();
                        let label_start = if label == inside { body } else { body + target.len() + 1 };
                        let references = if embed { &mut index.embeds } else { &mut index.links };
                        references.push(Link {
                            source: range.start + start - usize::from(embed)..range.start + cursor,
                            label: range.start + label_start..range.start + end,
                            destination,
                            target: target.to_owned(),
                        });
                    }
                }
            }
            let attributes = match node {
                Node::MdxJsxFlowElement(element) => Some((&element.name, &element.attributes)),
                Node::MdxJsxTextElement(element) => Some((&element.name, &element.attributes)),
                _ => None,
            };
            if let Some((name, attributes)) = attributes {
                for attribute in attributes {
                    if let markdown::mdast::AttributeContent::Property(attribute) = attribute {
                        if attribute.name == "id" || (name.as_deref() == Some("a") && attribute.name == "name") {
                            if let Some(markdown::mdast::AttributeValue::Literal(id)) = &attribute.value {
                                if !id.is_empty() { index.headings.push((id.clone(), range.start)); }
                            }
                        }
                    }
                }
            }
            if let Node::Html(html) = node {
                for (offset, id) in explicit_ids(&html.value) {
                    index.headings.push((id, range.start + offset));
                }
            }
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
        index.headings.dedup();
        index
    }
    pub fn at(&self, at: usize) -> Option<&Link> {
        self.links
            .iter()
            .find(|link| link.source.start <= at && at < link.source.end)
            .or_else(|| {
                self.links
                    .iter()
                    .find(|link| link.destination.start <= at && at <= link.destination.end)
            })
    }
    pub fn anchor(&self, fragment: &str) -> Option<usize> {
        let fragment = percent_encoding::percent_decode_str(fragment)
            .decode_utf8()
            .ok()?;
        let mut matches = self.headings.iter().filter(|(slug, _)| slug == fragment.as_ref());
        let at = matches.next()?.1;
        matches.next().is_none().then_some(at)
    }
    /// Resolve the same explicit anchor used for navigation. Headings include
    /// their section; a literal id includes its positioned owning block only.
    pub(super) fn fragment(&self, source: &str, root: &Node, fragment: Option<&str>) -> Result<Range<usize>, L10nKey> {
        let Some(fragment) = fragment.filter(|value| !value.is_empty()) else { return Ok(0..source.len()); };
        let decoded = percent_encoding::percent_decode_str(fragment).decode_utf8().map_err(|_| L10nKey::MarkdownLinkInvalidDestination)?;
        let mut matches = self.headings.iter().filter(|(id, _)| id == decoded.as_ref());
        let at = matches.next().map(|(_, at)| *at).ok_or(L10nKey::MarkdownLinkAnchorNotFound)?;
        if matches.next().is_some() { return Err(L10nKey::MarkdownReferenceAmbiguous); }
        let mut heading = None;
        visit(root, &mut |node| {
            if let Node::Heading(value) = node {
                if value.position.as_ref().is_some_and(|position| position.start.offset == at) { heading = Some(value.depth); }
            }
        });
        if let Some(depth) = heading {
            let mut end = source.len();
            visit(root, &mut |node| {
                if let Node::Heading(value) = node {
                    if let Some(position) = &value.position {
                        if position.start.offset > at && value.depth <= depth { end = end.min(position.start.offset); }
                    }
                }
            });
            return Ok(at..end);
        }
        let block = root.children().into_iter().flatten().filter_map(Node::position)
            .find(|position| position.start.offset <= at && at < position.end.offset)
            .ok_or(L10nKey::MarkdownLinkAnchorNotFound)?;
        Ok(block.start.offset..block.end.offset)
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
/// Only called on positioned HTML nodes; code fences and code spans are excluded.
fn explicit_ids(html: &str) -> Vec<(usize, String)> {
    let mut ids = Vec::new();
    let bytes = html.as_bytes();
    let mut cursor = 0;
    while cursor < bytes.len() {
        let Some(open) = html[cursor..].find('<').map(|at| cursor + at) else { break; };
        if html[open..].starts_with("<!--") {
            cursor = html[open + 4..].find("-->").map_or(bytes.len(), |at| open + 4 + at + 3);
            continue;
        }
        let mut tag_end = open + 1;
        while tag_end < bytes.len() && bytes[tag_end].is_ascii_alphanumeric() { tag_end += 1; }
        if tag_end == open + 1 { cursor = open + 1; continue; }
        let tag = &bytes[open + 1..tag_end];
        let mut close = tag_end;
        let mut quote = None;
        while close < bytes.len() {
            let byte = bytes[close];
            if quote == Some(byte) { quote = None; }
            else if quote.is_none() {
                if matches!(byte, b'\'' | b'"') { quote = Some(byte); }
                else if byte == b'>' { break; }
            }
            close += 1;
        }
        if close == bytes.len() { break; }
        let mut at = tag_end;
        while at < close {
            while at < close && bytes[at].is_ascii_whitespace() { at += 1; }
            let name_start = at;
            while at < close && (bytes[at].is_ascii_alphanumeric() || matches!(bytes[at], b'-' | b'_' | b':')) { at += 1; }
            if name_start == at { at += 1; continue; }
            let name = &bytes[name_start..at];
            while at < close && bytes[at].is_ascii_whitespace() { at += 1; }
            if at == close || bytes[at] != b'=' { continue; }
            at += 1;
            while at < close && bytes[at].is_ascii_whitespace() { at += 1; }
            let quote = bytes.get(at).copied().filter(|byte| matches!(*byte, b'\'' | b'"'));
            at += usize::from(quote.is_some());
            let value = at;
            while at < close && if let Some(quote) = quote { bytes[at] != quote } else { !bytes[at].is_ascii_whitespace() } { at += 1; }
            if at > value && (name.eq_ignore_ascii_case(b"id") || (tag.eq_ignore_ascii_case(b"a") && name.eq_ignore_ascii_case(b"name"))) {
                ids.push((open, html[value..at].to_owned()));
            }
            at += usize::from(quote.is_some());
        }
        cursor = close + 1;
        if tag.eq_ignore_ascii_case(b"script") || tag.eq_ignore_ascii_case(b"style") {
            let closing = if tag.eq_ignore_ascii_case(b"script") { "</script" } else { "</style" };
            cursor = html[cursor..].to_ascii_lowercase().find(closing).map_or(bytes.len(), |at| cursor + at);
        }
    }
    ids
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
pub(crate) fn resolve(owner: &Path, host: &dyn Host, target: &str) -> Result<Target, L10nKey> {
    let local = host.id().is_local();
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
        let path = if host.is_absolute(path) {
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
