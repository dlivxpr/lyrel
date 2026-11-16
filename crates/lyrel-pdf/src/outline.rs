//! PDF outline, named destinations and link annotations.
//!
//! Adapted from the pinned Zorite MIT traversal. Retained: bounded `/Next` /
//! `/First` walking, page-object indexing, normalized link rectangles and the
//! http(s)-only external URI allowlist. Added: the named-destination forms the
//! original explicitly left unresolved (catalog `/Names` name trees and legacy
//! `/Dests` dictionaries), destination coordinates, and rotation-aware link
//! rectangles.

use std::collections::{HashMap, HashSet};

use hayro::hayro_interpret::TransformExt;
use hayro::hayro_syntax::Pdf;
use hayro::hayro_syntax::object::{
    Array, Dict, MaybeRef, Name, ObjRef, Object, Rect, String as PdfString,
};
use hayro::hayro_syntax::page::Page;
use hayro::hayro_syntax::xref::XRef;
use hayro::kurbo::Point;

use crate::NormRect;
use crate::decode_pdf_string;

/// One outline entry, flattened depth-first.
#[derive(Clone, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct OutlineItem {
    pub title: String,
    /// 0 = top level.
    pub level: usize,
    /// The resolved destination, with an optional normalized top-left anchor.
    pub destination: Option<Destination>,
}

impl OutlineItem {
    pub fn page(&self) -> Option<usize> {
        self.destination
            .as_ref()
            .map(|destination| destination.page)
    }
}

/// An internal destination: a 0-based page plus a normalized point. `x`/`y`
/// are fractions of the target page's crop box with a top-left origin; they
/// are absent for `/Fit` and for arrays whose coordinates were null/invalid.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct Destination {
    pub page: usize,
    pub x: Option<f32>,
    pub y: Option<f32>,
}

/// Where a clickable link points.
#[derive(Clone, Debug, PartialEq)]
pub enum LinkTarget {
    Page(Destination),
    Uri(String),
}

/// A clickable `/Link` annotation in normalized page coordinates.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct PdfLink {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub target: LinkTarget,
}

impl PdfLink {
    /// A link rectangle with its resolved target.
    pub fn new(x: f32, y: f32, w: f32, h: f32, target: LinkTarget) -> Self {
        Self { x, y, w, h, target }
    }
}

const MAX_ITEMS: usize = 10_000;
const MAX_DEPTH: usize = 32;
const MAX_NAMED: usize = 10_000;

/// Extract the document outline, flattened depth-first.
pub fn outline(doc: &Pdf) -> Vec<OutlineItem> {
    let navigation = Navigation::new(doc);
    let xref = doc.xref();
    let Some(catalog) = xref.get::<Dict>(xref.root_id()) else {
        return Vec::new();
    };
    let Some(outlines) = catalog.get::<Dict>("Outlines") else {
        return Vec::new();
    };
    let Some(first) = outlines.get_ref("First") else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut visited = HashSet::new();
    walk_items(xref, first, 0, &navigation, &mut visited, &mut out);
    out
}

fn walk_items(
    xref: &XRef,
    start: ObjRef,
    level: usize,
    navigation: &Navigation<'_>,
    visited: &mut HashSet<ObjRef>,
    out: &mut Vec<OutlineItem>,
) {
    if level > MAX_DEPTH {
        return;
    }
    let mut current = Some(start);
    while let Some(id) = current {
        if out.len() >= MAX_ITEMS || !visited.insert(id) {
            return;
        }
        let Some(item) = xref.get::<Dict>(id.into()) else {
            return;
        };
        if let Some(title) = item.get::<PdfString>("Title") {
            out.push(OutlineItem {
                title: decode_pdf_string(title.as_bytes()),
                level,
                destination: item_destination(&item, navigation),
            });
        }
        if let Some(child) = item.get_ref("First") {
            walk_items(xref, child, level + 1, navigation, visited, out);
        }
        current = item.get_ref("Next");
    }
}

fn item_destination(item: &Dict<'_>, navigation: &Navigation<'_>) -> Option<Destination> {
    item.get::<Object>("Dest")
        .and_then(|dest| resolve_destination(&dest, navigation, 0))
        .or_else(|| {
            item.get::<Dict>("A")
                .and_then(|action| action.get::<Object>("D"))
                .and_then(|dest| resolve_destination(&dest, navigation, 0))
        })
}

fn resolve_destination(
    object: &Object<'_>,
    navigation: &Navigation<'_>,
    depth: usize,
) -> Option<Destination> {
    if depth > MAX_DEPTH {
        return None;
    }
    match object {
        Object::Array(array) => destination_array(array, navigation),
        Object::String(name) => {
            navigation.named_destination(&decode_pdf_string(name.as_bytes()), depth + 1)
        }
        Object::Name(name) => navigation.named_destination(name.as_str(), depth + 1),
        Object::Dict(dict) => dict
            .get::<Object>("D")
            .and_then(|dest| resolve_destination(&dest, navigation, depth + 1)),
        _ => None,
    }
}

fn destination_array(array: &Array<'_>, navigation: &Navigation<'_>) -> Option<Destination> {
    let page = match array.raw_iter().next()? {
        MaybeRef::Ref(page_ref) => navigation.page_index.get(&page_ref).copied()?,
        MaybeRef::NotRef(Object::Number(number)) => usize::try_from(number.as_i64()).ok()?,
        MaybeRef::NotRef(_) => return None,
    };
    let crop = navigation.crops.get(page).copied()?;
    let (width, height) = (crop.width(), crop.height());
    if width <= 0.0 || height <= 0.0 {
        return Some(Destination {
            page,
            x: None,
            y: None,
        });
    }
    let values: Vec<Object<'_>> = array.iter::<Object<'_>>().collect();
    let number = |index: usize| {
        values
            .get(index)
            .and_then(|value| value.clone().into_number())
            .map(|number| number.as_f32())
    };
    let kind = values
        .get(1)
        .and_then(|value| value.clone().into_name())
        .map(|name| name.as_str().to_string());
    let normalize_x = |x: f32| (((f64::from(x) - crop.x0) / width) as f32).clamp(0.0, 1.0);
    let normalize_y = |top: f32| (((crop.y1 - f64::from(top)) / height) as f32).clamp(0.0, 1.0);
    let (x, y) = match kind.as_deref() {
        Some("XYZ") => (number(2).map(normalize_x), number(3).map(normalize_y)),
        Some("FitH") | Some("FitBH") => (None, number(2).map(normalize_y)),
        Some("FitV") | Some("FitBV") => (number(2).map(normalize_x), None),
        Some("FitR") => (number(2).map(normalize_x), number(5).map(normalize_y)),
        _ => (None, None),
    };
    Some(Destination { page, x, y })
}

/// Clickable `/Link` annotations for every page.
pub fn page_links(doc: &Pdf) -> Vec<Vec<PdfLink>> {
    let navigation = Navigation::new(doc);
    let mut out = Vec::with_capacity(doc.pages().len());
    for page in doc.pages().iter() {
        let mut links = Vec::new();
        if let Some(annots) = page.raw().get::<Array>("Annots") {
            for annot in annots.iter::<Dict>() {
                if annot
                    .get::<Name>("Subtype")
                    .is_none_or(|name| name.as_str() != "Link")
                {
                    continue;
                }
                if hidden(&annot) {
                    continue;
                }
                let (Some(target), Some(rect)) =
                    (link_target(&annot, &navigation), annot.get::<Rect>("Rect"))
                else {
                    continue;
                };
                if let Some(rect) = normalized_rect(&page, rect) {
                    links.push(PdfLink {
                        x: rect.x,
                        y: rect.y,
                        w: rect.w,
                        h: rect.h,
                        target,
                    });
                }
            }
        }
        out.push(links);
    }
    out
}

/// Normalize a PDF user-space rectangle through the page's display transform,
/// so rotated pages line up with the rendered bitmap too.
fn normalized_rect(page: &Page<'_>, rect: Rect) -> Option<NormRect> {
    let (width, height) = page.render_dimensions();
    if width <= 0.0 || height <= 0.0 {
        return None;
    }
    let matrix = page.initial_transform(true).to_kurbo();
    let corners = [
        matrix * Point::new(rect.x0, rect.y0),
        matrix * Point::new(rect.x1, rect.y0),
        matrix * Point::new(rect.x1, rect.y1),
        matrix * Point::new(rect.x0, rect.y1),
    ];
    let mut min = corners[0];
    let mut max = corners[0];
    for corner in &corners[1..] {
        min.x = min.x.min(corner.x);
        min.y = min.y.min(corner.y);
        max.x = max.x.max(corner.x);
        max.y = max.y.max(corner.y);
    }
    Some(NormRect {
        x: ((min.x as f32) / width).clamp(0.0, 1.0),
        y: ((min.y as f32) / height).clamp(0.0, 1.0),
        w: (((max.x - min.x) as f32) / width).clamp(0.0, 1.0),
        h: (((max.y - min.y) as f32) / height).clamp(0.0, 1.0),
    })
}

fn hidden(annot: &Dict<'_>) -> bool {
    let flags = annot.get::<i64>("F").unwrap_or(0);
    flags & ((1 << 1) | (1 << 5)) != 0
}

fn link_target(annot: &Dict<'_>, navigation: &Navigation<'_>) -> Option<LinkTarget> {
    if let Some(dest) = annot.get::<Object>("Dest")
        && let Some(destination) = resolve_destination(&dest, navigation, 0)
    {
        return Some(LinkTarget::Page(destination));
    }
    let action = annot.get::<Dict>("A")?;
    match action.get::<Name>("S").as_ref().map(Name::as_str) {
        Some("URI") => {
            let uri = decode_pdf_string(action.get::<PdfString>("URI")?.as_bytes());
            if !is_safe_external_uri(&uri) {
                log::warn!("pdf: dropped link annotation with unsupported URI scheme");
                return None;
            }
            Some(LinkTarget::Uri(uri))
        }
        Some("GoTo") => action
            .get::<Object>("D")
            .and_then(|dest| resolve_destination(&dest, navigation, 0))
            .map(LinkTarget::Page),
        _ => None,
    }
}

/// Only `http://` and `https://` may reach the OS URL opener. Whitespace and
/// control characters anywhere are rejected, not trimmed.
fn is_safe_external_uri(uri: &str) -> bool {
    !uri.chars().any(|ch| ch.is_whitespace() || ch.is_control())
        && ["http://", "https://"].iter().any(|scheme| {
            uri.as_bytes()
                .get(..scheme.len())
                .is_some_and(|got| got.eq_ignore_ascii_case(scheme.as_bytes()))
        })
}

struct Navigation<'a> {
    page_index: HashMap<ObjRef, usize>,
    named: HashMap<String, Object<'a>>,
    crops: Vec<Rect>,
}

impl<'a> Navigation<'a> {
    fn new(doc: &'a Pdf) -> Self {
        let xref = doc.xref();
        let page_index = build_page_index(doc);
        let crops = doc.pages().iter().map(Page::crop_box).collect();
        let mut named = HashMap::new();
        if let Some(catalog) = xref.get::<Dict>(xref.root_id()) {
            if let Some(names) = catalog.get::<Dict>("Names")
                && let Some(dests) = names.get::<Dict>("Dests")
            {
                let mut visited = HashSet::new();
                collect_name_tree(xref, &dests, &mut named, &mut visited, 0);
            }
            if let Some(dests) = catalog.get::<Dict>("Dests") {
                collect_legacy_dests(xref, &dests, &mut named);
            }
        }
        Self {
            page_index,
            named,
            crops,
        }
    }

    fn named_destination(&self, key: &str, depth: usize) -> Option<Destination> {
        let object = self.named.get(key)?;
        resolve_destination(object, self, depth + 1)
    }
}

fn collect_name_tree<'a>(
    xref: &'a XRef,
    node: &Dict<'a>,
    out: &mut HashMap<String, Object<'a>>,
    visited: &mut HashSet<hayro::hayro_syntax::object::ObjectIdentifier>,
    depth: usize,
) {
    if depth > MAX_DEPTH || out.len() >= MAX_NAMED {
        return;
    }
    if let Some(id) = node.obj_id()
        && !visited.insert(id)
    {
        return;
    }
    if let Some(names) = node.get::<Array>("Names") {
        let values: Vec<Object<'a>> = names.iter::<Object<'a>>().collect();
        for pair in values.chunks(2) {
            if out.len() >= MAX_NAMED {
                break;
            }
            let [key, value] = pair else { break };
            if let Object::String(key) = key {
                out.insert(decode_pdf_string(key.as_bytes()), value.clone());
            }
        }
    }
    if let Some(kids) = node.get::<Array>("Kids") {
        for kid in kids.raw_iter() {
            if let MaybeRef::Ref(kid) = kid
                && let Some(child) = xref.get::<Dict>(kid.into())
            {
                collect_name_tree(xref, &child, out, visited, depth + 1);
            }
        }
    }
}

fn collect_legacy_dests<'a>(
    xref: &'a XRef,
    dests: &Dict<'a>,
    out: &mut HashMap<String, Object<'a>>,
) {
    for (key, value) in dests.entries() {
        if out.len() >= MAX_NAMED {
            break;
        }
        let Some(value) = (match value {
            MaybeRef::Ref(id) => xref.get::<Object>(id.into()),
            MaybeRef::NotRef(value) => Some(value),
        }) else {
            continue;
        };
        out.insert(decode_pdf_string(key.as_ref()), value.clone());
    }
}

fn build_page_index(doc: &Pdf) -> HashMap<ObjRef, usize> {
    let xref = doc.xref();
    let mut map = HashMap::new();
    let Some(catalog) = xref.get::<Dict>(xref.root_id()) else {
        return map;
    };
    let Some(root) = catalog.get_ref("Pages") else {
        return map;
    };
    let mut index = 0;
    let mut visited = HashSet::new();
    walk_pages(xref, root, 0, &mut index, &mut visited, &mut map);
    map
}

fn walk_pages(
    xref: &XRef,
    id: ObjRef,
    depth: usize,
    index: &mut usize,
    visited: &mut HashSet<ObjRef>,
    map: &mut HashMap<ObjRef, usize>,
) {
    if depth > MAX_DEPTH || !visited.insert(id) {
        return;
    }
    let Some(dict) = xref.get::<Dict>(id.into()) else {
        return;
    };
    if let Some(kids) = dict.get::<Array>("Kids") {
        for kid in kids.raw_iter() {
            if let MaybeRef::Ref(kid) = kid {
                walk_pages(xref, kid, depth + 1, index, visited, map);
            }
        }
    } else {
        map.insert(id, *index);
        *index += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf16be_titles_decode() {
        assert_eq!(
            decode_pdf_string(&[0xFE, 0xFF, 0x00, b'H', 0x00, b'i']),
            "Hi"
        );
        assert_eq!(decode_pdf_string(b"  Intro  "), "Intro");
    }

    #[test]
    fn only_http_uris_survive() {
        assert!(is_safe_external_uri("https://example.com"));
        assert!(is_safe_external_uri("HTTP://EXAMPLE.COM"));
        for bad in [
            "file:///etc/passwd",
            "smb://evil/share",
            "javascript:alert(1)",
            "mailto:a@b.c",
            "//evil.com",
            r"\\evil\share",
            " javascript:alert(1)",
            "java\tscript:x",
            "",
        ] {
            assert!(!is_safe_external_uri(bad), "should reject {bad:?}");
        }
    }
}
