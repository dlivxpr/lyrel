//! Package document, EPUB 3 navigation and EPUB 2 NCX.
//!
//! Retained from Glim `books/mod.rs::epub`: container rootfile, manifest
//! identity checks, spine media-type checks, `epub:type="toc"` navigation,
//! NCX fallback only when the nav is missing/empty, and chapter-title
//! backfill. Lyrel adds explicit fixed-layout detection (global metadata and
//! per-spine overrides, plus the common EPUB 2/Apple heuristic), rights and
//! encryption checks that verify the font-obfuscation algorithms target font
//! resources, and explicit TOC budgets.

use std::collections::HashMap;

use roxmltree::{Document, Node};

use super::archive::{Archive, resolve};
use super::decode::{decode, xml};
use super::limits::Limits;
use super::{CancelToken, EpubError, Metadata, SpineItem, TocEntry};

/// What `META-INF/container.xml` named.
pub(crate) fn rootfile(
    archive: &mut Archive<impl std::io::Read + std::io::Seek>,
    limits: &Limits,
    cancel: &CancelToken,
) -> Result<String, EpubError> {
    match archive.entry("mimetype", 256, cancel) {
        Ok(mimetype) if mimetype == b"application/epub+zip" => {}
        Ok(_) => return Err(EpubError::NotEpub),
        Err(EpubError::Missing { .. }) => {
            // A missing mimetype is not fatal: older books omit it, and the
            // container is the authoritative EPUB signature.
        }
        Err(error) => return Err(error),
    }
    let container = archive.has("META-INF/container.xml");
    if !container {
        return Err(EpubError::NotEpub);
    }
    let bytes = archive.entry("META-INF/container.xml", limits.xml_bytes as u64, cancel)?;
    let text = decode(&bytes)?;
    let document = xml(&text, limits)?;
    let root = descendants(document.root_element(), "rootfile")
        .find_map(|node| attribute(node, "full-path"))
        .ok_or_else(|| EpubError::Corrupt {
            detail: "the EPUB container names no package document".into(),
        })?;
    resolve("", root).ok_or_else(|| EpubError::PathEscapes {
        href: root.to_owned(),
    })
}

#[derive(Clone, Debug)]
pub(crate) struct ManifestItem {
    pub(crate) href: String,
    pub(crate) media_type: String,
}

#[derive(Clone, Debug)]
pub(crate) struct RawSpineItem {
    pub(crate) href: String,
}

pub(crate) struct Package {
    pub(crate) metadata: Metadata,
    pub(crate) manifest: HashMap<String, ManifestItem>,
    pub(crate) spine: Vec<RawSpineItem>,
    pub(crate) toc: Vec<TocEntry>,
    pub(crate) fixed_global: bool,
    pub(crate) fixed_items: Vec<usize>,
}

/// Parses the package document and its navigation.
pub(crate) fn package(
    archive: &mut Archive<impl std::io::Read + std::io::Seek>,
    root: &str,
    limits: &Limits,
    cancel: &CancelToken,
) -> Result<Package, EpubError> {
    let bytes = archive.entry(root, limits.xml_bytes as u64, cancel)?;
    let text = decode(&bytes)?;
    let document = xml(&text, limits)?;
    let metadata_node = descendants(document.root_element(), "metadata")
        .next()
        .ok_or_else(|| EpubError::Corrupt {
            detail: "the EPUB package has no metadata".into(),
        })?;
    let title = descendants(metadata_node, "title")
        .next()
        .map(text_of)
        .unwrap_or_default();
    let authors = descendants(metadata_node, "creator")
        .map(text_of)
        .filter(|author| !author.is_empty())
        .collect::<Vec<_>>();
    let language = descendants(metadata_node, "language")
        .next()
        .map(text_of)
        .filter(|language| !language.is_empty());
    let fixed_global = fixed_from_metadata(metadata_node);

    let mut manifest = HashMap::new();
    let mut nav = None;
    let mut ncx = None;
    for node in descendants(document.root_element(), "item") {
        let id = attribute(node, "id").ok_or_else(|| EpubError::Corrupt {
            detail: "an EPUB manifest item lacks an identity".into(),
        })?;
        let href = attribute(node, "href").ok_or_else(|| EpubError::Corrupt {
            detail: format!("EPUB manifest item {id} lacks a resource"),
        })?;
        let href = resolve(root, href).ok_or_else(|| EpubError::PathEscapes {
            href: href.to_owned(),
        })?;
        let media_type = attribute(node, "media-type").unwrap_or_default().to_owned();
        let properties = attribute(node, "properties").unwrap_or_default();
        if properties.split_whitespace().any(|value| value == "nav") {
            nav = Some(href.clone());
        }
        if media_type == "application/x-dtbncx+xml" {
            ncx = Some(href.clone());
        }
        if manifest
            .insert(id.to_owned(), ManifestItem { href, media_type })
            .is_some()
        {
            return Err(EpubError::Corrupt {
                detail: format!("duplicate EPUB manifest identity {id}"),
            });
        }
    }
    if manifest.len() > limits.archive_entries {
        return Err(EpubError::limit(
            "manifest items",
            limits.archive_entries as u64,
        ));
    }

    let mut spine = Vec::new();
    let mut fixed_items = Vec::new();
    for node in descendants(document.root_element(), "itemref") {
        let idref = attribute(node, "idref").ok_or_else(|| EpubError::Corrupt {
            detail: "an EPUB spine item lacks an idref".into(),
        })?;
        let item = manifest.get(idref).ok_or_else(|| EpubError::Corrupt {
            detail: format!("EPUB spine references missing item {idref}"),
        })?;
        if !matches!(
            item.media_type.as_str(),
            "application/xhtml+xml" | "text/html"
        ) {
            return Err(EpubError::Corrupt {
                detail: format!(
                    "EPUB spine item {idref} has unsupported media type {}",
                    item.media_type
                ),
            });
        }
        let properties = attribute(node, "properties").unwrap_or_default();
        let pre_paginated = properties
            .split_whitespace()
            .any(|value| value == "rendition:layout-pre-paginated");
        if pre_paginated {
            fixed_items.push(spine.len());
        }
        spine.push(RawSpineItem {
            href: item.href.clone(),
        });
    }
    if spine.is_empty() {
        return Err(EpubError::Corrupt {
            detail: "the EPUB spine has no chapters".into(),
        });
    }
    if spine.len() > limits.chapters {
        return Err(EpubError::limit("chapters", limits.chapters as u64));
    }

    let mut toc = if let Some(nav) = &nav {
        let bytes = archive.entry(nav, limits.xml_bytes as u64, cancel)?;
        let text = decode(&bytes)?;
        let document = xml(&text, limits)?;
        parse_nav(&document, nav, limits, cancel)?
    } else {
        Vec::new()
    };
    if toc.is_empty()
        && let Some(ncx) = &ncx
    {
        let bytes = archive.entry(ncx, limits.xml_bytes as u64, cancel)?;
        let text = decode(&bytes)?;
        let document = xml(&text, limits)?;
        toc = parse_ncx(&document, ncx, limits, cancel)?;
    }
    for entry in &toc {
        if entry.depth > limits.toc_depth {
            return Err(EpubError::limit("TOC depth", limits.toc_depth as u64));
        }
    }
    Ok(Package {
        metadata: Metadata {
            title,
            authors,
            language,
        },
        manifest,
        spine,
        toc,
        fixed_global,
        fixed_items,
    })
}

/// EPUB 3.3 global layout plus the common EPUB 2/Apple fixed-layout flag.
fn fixed_from_metadata(metadata: Node<'_, '_>) -> bool {
    for node in descendants(metadata, "meta") {
        if let Some(property) = attribute(node, "property")
            && property == "rendition:layout"
            && text_of(node) == "pre-paginated"
        {
            return true;
        }
        if attribute(node, "name") == Some("fixed-layout")
            && attribute(node, "content").is_some_and(|value| value == "true")
        {
            return true;
        }
    }
    false
}

/// Applies per-spine reflowable overrides to the global layout declaration.
pub(crate) fn fixed_layout(package: &Package) -> Option<EpubError> {
    if package.fixed_items.is_empty() && !package.fixed_global {
        return None;
    }
    if package.fixed_items.is_empty() {
        return Some(EpubError::FixedLayout {
            global: true,
            items: package.spine.len(),
        });
    }
    Some(EpubError::FixedLayout {
        global: package.fixed_global,
        items: package.fixed_items.len(),
    })
}

/// Validates rights and encryption declarations against manifest resources.
pub(crate) fn check_protection(
    archive: &mut Archive<impl std::io::Read + std::io::Seek>,
    package: &Package,
    limits: &Limits,
    cancel: &CancelToken,
) -> Result<(), EpubError> {
    for name in ["META-INF/rights.xml", "META-INF/license.lcpl"] {
        if archive.has(name) {
            return Err(EpubError::Protected {
                detail: format!("{name} declares rights management this reader does not process"),
            });
        }
    }
    if !archive.has("META-INF/encryption.xml") {
        return Ok(());
    }
    let bytes = archive.entry("META-INF/encryption.xml", limits.xml_bytes as u64, cancel)?;
    let text = decode(&bytes)?;
    let document = xml(&text, limits)?;
    let mut seen = false;
    let encrypted_data = descendants(document.root_element(), "EncryptedData").count();
    for method in descendants(document.root_element(), "EncryptionMethod") {
        seen = true;
        let algorithm = attribute(method, "Algorithm").unwrap_or_default();
        if !matches!(
            algorithm,
            "http://www.idpf.org/2008/embedding" | "http://ns.adobe.com/pdf/enc#RC"
        ) {
            return Err(EpubError::Protected {
                detail: format!("encrypted EPUB resource uses unsupported algorithm {algorithm}"),
            });
        }
    }
    if seen {
        // Font obfuscation is allowed only when every reference names a font
        // resource. An IDPF algorithm on a chapter must not be mistaken for a
        // font and read as plain XHTML.
        let mut references = 0usize;
        for reference in descendants(document.root_element(), "CipherReference") {
            references += 1;
            let uri = attribute(reference, "URI").ok_or_else(|| EpubError::Protected {
                detail: "an encrypted resource has no URI".into(),
            })?;
            let resolved = resolve("", uri).ok_or_else(|| EpubError::Protected {
                detail: format!("encrypted resource path escapes the container: {uri}"),
            })?;
            let path = resolved.split('#').next().unwrap_or(&resolved);
            let font = package.manifest.values().any(|item| {
                item.href.split('#').next().unwrap_or(&item.href) == path
                    && is_font_media_type(&item.media_type)
            });
            if !font {
                return Err(EpubError::Protected {
                    detail: format!("encrypted resource {path} is not a font"),
                });
            }
        }
        if references == 0 {
            return Err(EpubError::Protected {
                detail: "an encryption declaration names no cipher reference".into(),
            });
        }
    } else if encrypted_data > 0 {
        return Err(EpubError::Protected {
            detail: "an encryption declaration names no method".into(),
        });
    }
    Ok(())
}

pub(crate) fn is_font_media_type(media_type: &str) -> bool {
    matches!(
        media_type,
        "application/vnd.ms-opentype"
            | "application/font-sfnt"
            | "application/font-woff"
            | "application/x-font-ttf"
            | "application/x-font-otf"
            | "application/x-font-truetype"
            | "application/x-font-opentype"
            | "font/ttf"
            | "font/otf"
            | "font/woff"
            | "font/woff2"
    )
}

fn parse_nav(
    document: &Document<'_>,
    nav_path: &str,
    limits: &Limits,
    cancel: &CancelToken,
) -> Result<Vec<TocEntry>, EpubError> {
    let Some(toc) = descendants(document.root_element(), "nav").find(|node| {
        attribute(*node, "type").is_some_and(|value| {
            value
                .split_whitespace()
                .any(|token| token == "toc" || token == "doc-toc")
        }) || attribute(*node, "role") == Some("doc-toc")
    }) else {
        return Ok(Vec::new());
    };
    let mut entries = Vec::new();
    for link in descendants(toc, "a") {
        cancel.check()?;
        if entries.len() >= limits.toc_entries {
            return Err(EpubError::limit("TOC entries", limits.toc_entries as u64));
        }
        let Some(href) = attribute(link, "href").and_then(|href| resolve(nav_path, href)) else {
            continue;
        };
        let depth = link
            .ancestors()
            .take_while(|node| *node != toc)
            .filter(|node| node.is_element() && node.tag_name().name() == "ol")
            .count()
            .saturating_sub(1);
        entries.push(TocEntry {
            title: text_of(link),
            href,
            depth,
        });
    }
    Ok(entries)
}

fn parse_ncx(
    document: &Document<'_>,
    ncx_path: &str,
    limits: &Limits,
    cancel: &CancelToken,
) -> Result<Vec<TocEntry>, EpubError> {
    let mut entries = Vec::new();
    for point in descendants(document.root_element(), "navPoint") {
        cancel.check()?;
        if entries.len() >= limits.toc_entries {
            return Err(EpubError::limit("TOC entries", limits.toc_entries as u64));
        }
        let label = descendants(point, "navLabel")
            .next()
            .map(text_of)
            .unwrap_or_default();
        let Some(href) = point
            .children()
            .find(|node| node.is_element() && node.tag_name().name() == "content")
            .and_then(|node| attribute(node, "src"))
            .and_then(|src| resolve(ncx_path, src))
        else {
            continue;
        };
        let depth = point
            .ancestors()
            .skip(1)
            .filter(|node| node.is_element() && node.tag_name().name() == "navPoint")
            .count();
        entries.push(TocEntry {
            title: label,
            href,
            depth,
        });
    }
    Ok(entries)
}

/// Completes chapter titles from the TOC and fills a TOC from the spine when
/// the book has none.
pub(crate) fn finalize(
    spine: &mut [SpineItem],
    toc: &mut Vec<TocEntry>,
    limits: &Limits,
) -> Result<(), EpubError> {
    for chapter in spine.iter_mut() {
        if let Some(label) = toc
            .iter()
            .find(|entry| entry.href.split('#').next() == Some(chapter.href.as_str()))
        {
            chapter.title.clone_from(&label.title);
        }
    }
    if toc.is_empty() {
        if spine.len() > limits.toc_entries {
            return Err(EpubError::limit("TOC entries", limits.toc_entries as u64));
        }
        *toc = spine
            .iter()
            .map(|chapter| TocEntry {
                title: chapter.title.clone(),
                href: chapter.href.clone(),
                depth: 0,
            })
            .collect();
    }
    Ok(())
}

fn descendants<'a, 'input>(
    node: Node<'a, 'input>,
    name: &str,
) -> impl Iterator<Item = Node<'a, 'input>> {
    let name = name.to_owned();
    node.descendants()
        .filter(move |node| node.is_element() && node.tag_name().name() == name)
}

fn attribute<'a, 'input>(node: Node<'a, 'input>, name: &str) -> Option<&'a str> {
    node.attributes()
        .find(|attribute| attribute.name() == name)
        .map(|attribute| attribute.value())
}

fn text_of(node: Node<'_, '_>) -> String {
    node.descendants()
        .filter(|node| node.is_text())
        .filter_map(|node| node.text())
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .to_owned()
}
