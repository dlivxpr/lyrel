//! What Lyrel itself needs to know about a draw.io file.
//!
//! Rendering belongs to the fixed engine. The workbench still needs to read
//! the document identity before the engine starts: is this really a draw.io
//! file, how many pages does it declare, and is the page content inline or
//! compressed? The original bytes are passed to the engine unchanged; this
//! module never rewrites them.

use std::fmt;

use roxmltree::{Document, Node, ParsingOptions};
use sha2::{Digest as _, Sha256};

use super::assets::MAX_DOCUMENT_BYTES;

pub(crate) const MAX_PAGES: usize = 512;
const NODE_LIMIT: u32 = 200_000;
const MAX_NAME_CHARS: usize = 200;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RootKind {
    /// `<mxfile>` with one `<diagram>` per page.
    File,
    /// A bare `<mxGraphModel>`: draw.io's single-page serialisation.
    Model,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DiagramContent {
    /// An `<mxGraphModel>` child, or XML stored as escaped text.
    Inline,
    /// Base64 + raw deflate, as draw.io writes it.
    Compressed,
    Empty,
    /// Non-empty content that is neither inline XML nor the compressed
    /// alphabet. The engine may still understand it; this only reports what
    /// the bytes looked like without guessing.
    Opaque,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PageInfo {
    pub name: Option<String>,
    pub id: Option<String>,
    pub content: DiagramContent,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DrawioSource {
    pub root: RootKind,
    pub pages: Vec<PageInfo>,
    pub sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum DrawioError {
    Empty,
    TooLarge,
    NotUtf8,
    Xml(String),
    UnsupportedRoot(String),
    TooManyPages,
    NoPages,
}

impl fmt::Display for DrawioError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => f.write_str("the file is empty"),
            Self::TooLarge => f.write_str("the diagram exceeds the preview budget"),
            Self::NotUtf8 => f.write_str("the diagram is not UTF-8 XML"),
            Self::Xml(detail) => write!(f, "the diagram is not well-formed XML: {detail}"),
            Self::UnsupportedRoot(root) => {
                write!(f, "the file is not a draw.io diagram root: {root}")
            }
            Self::TooManyPages => f.write_str("the diagram declares too many pages"),
            Self::NoPages => f.write_str("the diagram declares no pages"),
        }
    }
}

/// True when the opening bytes look like a draw.io document. This is a probe
/// for files whose name does not decide the format; it never executes or
/// fully parses the contents.
pub(crate) fn looks_like_drawio(bytes: &[u8]) -> bool {
    let bytes = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes);
    let prefix = &bytes[..bytes.len().min(4096)];
    let Ok(text) = std::str::from_utf8(prefix) else {
        return false;
    };
    let mut text = text.trim_start();
    if let Some(rest) = text.strip_prefix("<?xml") {
        let Some(end) = rest.find("?>") else {
            return false;
        };
        text = rest[end + 2..].trim_start();
    }
    text.starts_with("<mxfile") || text.starts_with("<mxGraphModel")
}

pub(crate) fn parse(bytes: &[u8]) -> Result<DrawioSource, DrawioError> {
    if bytes.len() > MAX_DOCUMENT_BYTES {
        return Err(DrawioError::TooLarge);
    }
    // Identity is the file's bytes, including a UTF-8 BOM. Only the XML text
    // view drops the BOM.
    let digest = sha256_hex(bytes);
    let bytes = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes);
    let text = std::str::from_utf8(bytes).map_err(|_| DrawioError::NotUtf8)?;
    if text.trim().is_empty() {
        return Err(DrawioError::Empty);
    }
    let document = Document::parse_with_options(
        text,
        ParsingOptions {
            allow_dtd: false,
            nodes_limit: NODE_LIMIT,
            entity_resolver: None,
        },
    )
    .map_err(|error| DrawioError::Xml(error.to_string()))?;
    let root = document.root_element();
    let pages = match root.tag_name().name() {
        "mxfile" => {
            let mut pages = Vec::new();
            for child in root.children().filter(Node::is_element) {
                if child.tag_name().name() != "diagram" {
                    continue;
                }
                if pages.len() == MAX_PAGES {
                    return Err(DrawioError::TooManyPages);
                }
                pages.push(page_info(child));
            }
            if pages.is_empty() {
                return Err(DrawioError::NoPages);
            }
            pages
        }
        "mxGraphModel" => vec![PageInfo {
            name: None,
            id: bounded_attr(root, "id", 128),
            content: DiagramContent::Inline,
        }],
        other => return Err(DrawioError::UnsupportedRoot(clean_label(other, 64))),
    };
    Ok(DrawioSource {
        root: if root.tag_name().name() == "mxfile" {
            RootKind::File
        } else {
            RootKind::Model
        },
        pages,
        sha256: digest,
    })
}

fn page_info(diagram: Node<'_, '_>) -> PageInfo {
    let has_model = diagram
        .children()
        .filter(Node::is_element)
        .any(|child| child.tag_name().name() == "mxGraphModel");
    let text = diagram
        .children()
        .filter(Node::is_text)
        .filter_map(|node| node.text())
        .collect::<String>();
    let text = text.trim();
    let content = if has_model {
        DiagramContent::Inline
    } else if text.is_empty() {
        DiagramContent::Empty
    } else if text.starts_with('<') {
        DiagramContent::Inline
    } else if looks_compressed(text) {
        DiagramContent::Compressed
    } else {
        DiagramContent::Opaque
    };
    PageInfo {
        name: bounded_attr(diagram, "name", MAX_NAME_CHARS),
        id: bounded_attr(diagram, "id", 128),
        content,
    }
}

fn looks_compressed(text: &str) -> bool {
    text.len() >= 8
        && text.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || byte == b'+' || byte == b'/' || byte == b'='
        })
}

fn bounded_attr(node: Node<'_, '_>, name: &str, limit: usize) -> Option<String> {
    node.attribute(name)
        .filter(|value| !value.trim().is_empty())
        .map(|value| clean_label(value, limit))
}

fn clean_label(value: &str, limit: usize) -> String {
    let text: String = value
        .chars()
        .filter(|character| !character.is_control())
        .take(limit)
        .collect();
    text.trim().to_string()
}

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut text = String::with_capacity(digest.len() * 2);
    for byte in digest {
        text.push(char::from(b"0123456789abcdef"[(byte >> 4) as usize]));
        text.push(char::from(b"0123456789abcdef"[(byte & 0xf) as usize]));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    const UNCOMPRESSED: &str = r#"<mxfile host="lyrel.test">
  <diagram id="one" name="Flow"><mxGraphModel><root><mxCell id="0"/><mxCell id="1" parent="0"/></root></mxGraphModel></diagram>
  <diagram id="two" name="Second"><mxGraphModel><root><mxCell id="0"/></root></mxGraphModel></diagram>
</mxfile>"#;

    const COMPRESSED: &str = r#"<mxfile host="lyrel.test">
  <diagram id="a" name="Packed">rVZdb9owEP01SLwUKeTDBB5b6NqHqVK7TkXiCbkQa0mVxFnstN2/3zlOIAk0oFTCSnD33t3z2OfXjx9XBT8GqQ</diagram>
  <diagram id="b" name="Packed 2">eNpVj8FqAzEMRP9F5x5s2bK9WfpDaW5NwmY30G5TbA0t/fuuL+1UJLO8</diagram>
</mxfile>"#;

    #[test]
    fn both_serialisations_report_their_real_pages() {
        let file = parse(UNCOMPRESSED.as_bytes()).unwrap();
        assert_eq!(file.root, RootKind::File);
        assert_eq!(file.pages.len(), 2);
        assert_eq!(file.pages[0].name.as_deref(), Some("Flow"));
        assert_eq!(file.pages[0].id.as_deref(), Some("one"));
        assert_eq!(file.pages[0].content, DiagramContent::Inline);
        assert_eq!(file.pages[1].name.as_deref(), Some("Second"));

        let model = r#"<mxGraphModel><root><mxCell id="0"/></root></mxGraphModel>"#;
        let model = parse(model.as_bytes()).unwrap();
        assert_eq!(model.root, RootKind::Model);
        assert_eq!(model.pages.len(), 1);
        assert_eq!(model.pages[0].content, DiagramContent::Inline);
    }

    #[test]
    fn compressed_page_content_is_recognised_without_being_rewritten() {
        let file = parse(COMPRESSED.as_bytes()).unwrap();
        assert_eq!(file.pages.len(), 2);
        assert_eq!(file.pages[0].content, DiagramContent::Compressed);
        assert_eq!(file.pages[1].content, DiagramContent::Compressed);
        // The digest is of the original bytes, including the BOM below.
        assert_eq!(file.sha256.len(), 64);
        let with_bom = format!("\u{feff}{COMPRESSED}");
        let bom = parse(with_bom.as_bytes()).unwrap();
        assert_ne!(bom.sha256, file.sha256);
        assert_eq!(bom.pages.len(), 2);
    }

    #[test]
    fn non_diagrams_are_named_instead_of_rendered() {
        assert_eq!(parse(b""), Err(DrawioError::Empty));
        assert!(matches!(parse(b"plain text"), Err(DrawioError::Xml(_))));
        assert!(matches!(
            parse(b"<html><body/></html>"),
            Err(DrawioError::UnsupportedRoot(_))
        ));
        let dtd = b"<!DOCTYPE mxfile [<!ENTITY x \"y\">]><mxfile/>";
        assert!(matches!(parse(dtd), Err(DrawioError::Xml(_))));
        assert_eq!(parse(b"<mxfile></mxfile>"), Err(DrawioError::NoPages));
        let utf16 = "\u{feff}<mxfile/>"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>();
        assert!(matches!(
            parse(&utf16),
            Err(DrawioError::NotUtf8) | Err(DrawioError::Xml(_))
        ));
    }

    #[test]
    fn probes_only_accept_the_two_roots() {
        assert!(looks_like_drawio(UNCOMPRESSED.as_bytes()));
        assert!(looks_like_drawio(b"<mxGraphModel><root/></mxGraphModel>"));
        assert!(!looks_like_drawio(b"<svg><mxfile></svg>"));
        assert!(!looks_like_drawio(b"<html><body/></html>"));
        assert!(!looks_like_drawio(b"plain"));
    }
}
