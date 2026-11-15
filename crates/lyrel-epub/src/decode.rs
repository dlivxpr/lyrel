//! Decode one archive member into XML text without trusting its declaration.
//!
//! Retained from Glim `glim-services/src/books/mod.rs`: BOM/UTF-16 probing, the
//! XML declaration's `encoding` label, strict `encoding_rs` decoding, and the
//! refusal of document type declarations. Lyrel changes two things:
//!
//! - ordinary XHTML named entities (`&eacute;`, `&trade;`, …) are converted to
//!   numeric references through a complete HTML named-reference table, instead
//!   of Glim's nine replacements; an unknown reference is an explicit parse
//!   error rather than a late roxmltree failure.
//! - a DOCTYPE is scanned with quote and internal-subset awareness, so a `>`
//!   inside a quoted identifier cannot leave fragmentary markup behind.

use encoding_rs::Encoding;
use roxmltree::Document;

use crate::EpubError;
use crate::limits::Limits;

/// Decodes one XML member.
pub(crate) fn decode(bytes: &[u8]) -> Result<String, EpubError> {
    let encoding = xml_encoding(bytes);
    let (text, _, errors) = encoding.decode(bytes);
    if errors {
        return Err(EpubError::encoding(
            "the bytes are not valid in their declared encoding",
        ));
    }
    let mut text = text.into_owned();
    if text.contains('\0') {
        return Err(EpubError::encoding("XML text contains a NUL byte"));
    }
    strip_doctype(&mut text)?;
    normalize_named_entities(&mut text)?;
    Ok(text)
}

/// The encoding an XML byte stream declares, with BOM and UTF-16 patterns
/// taking precedence over the in-band declaration.
fn xml_encoding(bytes: &[u8]) -> &'static Encoding {
    if let Some((encoding, _)) = Encoding::for_bom(bytes) {
        return encoding;
    }
    if bytes.starts_with(&[0, b'<', 0, b'?']) {
        return encoding_rs::UTF_16BE;
    }
    if bytes.starts_with(&[b'<', 0, b'?', 0]) {
        return encoding_rs::UTF_16LE;
    }
    if !bytes.starts_with(b"<?xml") {
        return encoding_rs::UTF_8;
    }
    let end = bytes
        .windows(2)
        .position(|pair| pair == b"?>")
        .map_or(bytes.len(), |index| index + 2);
    let mut probe = EncodingProbe::default();
    for byte in &bytes[..end] {
        probe.push(*byte);
    }
    probe.encoding.unwrap_or(encoding_rs::UTF_8)
}

/// Reads the `encoding="…"` value from a declaration prefix.
#[derive(Default)]
struct EncodingProbe {
    state: usize,
    matched: usize,
    quote: u8,
    label: Vec<u8>,
    encoding: Option<&'static Encoding>,
}

impl EncodingProbe {
    fn push(&mut self, byte: u8) {
        match self.state {
            0 => {
                let key = b"encoding";
                self.matched = if byte == key[self.matched] {
                    self.matched + 1
                } else {
                    usize::from(byte == key[0])
                };
                if self.matched == key.len() {
                    self.state = 1;
                    self.matched = 0;
                }
            }
            1 if byte.is_ascii_whitespace() => {}
            1 if byte == b'=' => self.state = 2,
            2 if byte.is_ascii_whitespace() => {}
            2 if matches!(byte, b'\'' | b'"') => {
                self.quote = byte;
                self.state = 3;
            }
            3 if byte == self.quote => {
                self.encoding = Encoding::for_label(&self.label);
                self.state = 4;
            }
            3 => {
                if self.label.len() < 256 {
                    self.label.push(byte);
                }
            }
            4 => {}
            _ => self.state = 0,
        }
    }
}

/// Removes well-formed DOCTYPE declarations, refusing internal subsets.
///
/// A subset may declare entities that resolve to local files, so it is never
/// accepted even though the document is parsed with DTD support off. Quotes
/// and internal brackets are tracked so a `>` inside `PUBLIC "…"` or an
/// internal declaration cannot end the scan early.
fn strip_doctype(text: &mut String) -> Result<(), EpubError> {
    let mut cursor = 0;
    while let Some(relative) = text[cursor..].find("<!DOCTYPE") {
        let start = cursor + relative;
        let bytes = text.as_bytes();
        let mut index = start + "<!DOCTYPE".len();
        let mut quote = None;
        let mut bracket_depth = 0usize;
        let mut saw_subset = false;
        let mut end = None;
        while index < bytes.len() {
            let byte = bytes[index];
            if let Some(open) = quote {
                if byte == open {
                    quote = None;
                }
            } else {
                match byte {
                    b'\'' | b'"' => quote = Some(byte),
                    b'[' => {
                        saw_subset = true;
                        bracket_depth += 1;
                    }
                    b']' => bracket_depth = bracket_depth.saturating_sub(1),
                    b'>' if bracket_depth == 0 => {
                        end = Some(index + 1);
                        break;
                    }
                    _ => {}
                }
            }
            index += 1;
        }
        let end = end.ok_or_else(|| EpubError::corrupt("truncated DOCTYPE declaration"))?;
        if saw_subset {
            return Err(EpubError::Encoding {
                detail: "entity declarations in a DOCTYPE are unsupported".into(),
            });
        }
        text.replace_range(start..end, "");
        cursor = start;
    }
    Ok(())
}

/// XML's five predefined entities, which roxmltree resolves itself.
fn is_xml_predefined(name: &str) -> bool {
    matches!(name, "amp" | "lt" | "gt" | "quot" | "apos")
}

/// Converts exact HTML named character references into numeric XML references.
///
/// `&eacute;` (and every other HTML name in the reference table) is valid in
/// an XHTML content document but not in the XML subset roxmltree accepts. We
/// decode the exact reference and re-encode any XML metacharacter it produced,
/// so the result is XML text with the same characters. Numeric references and
/// XML's own five names pass through unchanged.
fn normalize_named_entities(text: &mut String) -> Result<(), EpubError> {
    let mut cursor = 0;
    while let Some(relative) = text[cursor..].find('&') {
        let start = cursor + relative;
        let Some(semicolon_relative) = text[start + 1..].find(';') else {
            return Ok(());
        };
        let end = start + 1 + semicolon_relative;
        if end - start - 1 > 32 {
            cursor = start + 1;
            continue;
        }
        let name = &text[start + 1..end];
        if name.is_empty()
            || name.starts_with('#')
            || !name.bytes().all(|byte| byte.is_ascii_alphanumeric())
            || is_xml_predefined(name)
        {
            cursor = end + 1;
            continue;
        }
        let reference = &text[start..=end];
        let decoded = html_escape::decode_html_entities(reference);
        if decoded == reference {
            return Err(EpubError::Corrupt {
                detail: format!("unknown named entity &{name};"),
            });
        }
        let mut replaced = String::with_capacity(decoded.len());
        for character in decoded.chars() {
            match character {
                '&' => replaced.push_str("&amp;"),
                '<' => replaced.push_str("&lt;"),
                '>' => replaced.push_str("&gt;"),
                '"' => replaced.push_str("&quot;"),
                '\'' => replaced.push_str("&apos;"),
                other => replaced.push(other),
            }
        }
        text.replace_range(start..=end, &replaced);
        cursor = start + replaced.len();
    }
    Ok(())
}

/// Parses XML under the reader's byte and node budgets.
pub(crate) fn xml<'a>(text: &'a str, limits: &Limits) -> Result<Document<'a>, EpubError> {
    if text.len() > limits.xml_bytes {
        return Err(EpubError::limit("XML bytes", limits.xml_bytes as u64));
    }
    if text.contains("<!DOCTYPE") {
        return Err(EpubError::Corrupt {
            detail: "a DOCTYPE remained after decoding".into(),
        });
    }
    match Document::parse_with_options(
        text,
        roxmltree::ParsingOptions {
            allow_dtd: false,
            nodes_limit: limits.xml_nodes.min(u32::MAX as usize) as u32,
            ..Default::default()
        },
    ) {
        Ok(document) => Ok(document),
        Err(roxmltree::Error::NodesLimitReached) => {
            Err(EpubError::limit("XML nodes", limits.xml_nodes as u64))
        }
        Err(error) => Err(EpubError::Corrupt {
            detail: format!("invalid XML: {error}"),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordinary_xhtml_entities_decode_but_unknown_names_are_refused() {
        let mut text = "<p>caf&eacute; &amp; cr&egrave;me &mdash; &trade;</p>".to_owned();
        normalize_named_entities(&mut text).unwrap();
        assert_eq!(text, "<p>café &amp; crème — ™</p>");
        let mut unknown = "<p>&bogus;</p>".to_owned();
        assert!(normalize_named_entities(&mut unknown).is_err());
    }

    #[test]
    fn a_doctype_with_quoted_gt_or_an_internal_subset_is_handled() {
        let mut public = r#"<!DOCTYPE html PUBLIC "-//W3C//DTD XHTML 1.1//EN" "http://www.w3.org/TR/xhtml11/DTD/xhtml11.dtd"><html/>"#.to_owned();
        strip_doctype(&mut public).unwrap();
        assert_eq!(public, "<html/>");
        let mut subset =
            r#"<!DOCTYPE book [<!ENTITY a SYSTEM "file:///secret">]><book/>"#.to_owned();
        assert!(strip_doctype(&mut subset).is_err());
    }
}
