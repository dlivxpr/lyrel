//! Read-only AcroForm widget description.
//!
//! Adapted from the pinned Zorite `forms.rs` enumeration/geometry half only.
//! Retained: fully-qualified field names through `/Parent`, inherited `/FT`
//! `/Ff` `/V` `/DA` `/Q`, choice options, checkbox state names and widget
//! rectangles. Deliberately not imported: `set_form_value`,
//! `normalize_form_appearances`, `resolve_state_dict`, synthesized Helvetica
//! appearances and the whole write-back path.
//!
//! hayro 0.8 already renders existing `/AP /N` streams and `/AS` state
//! dictionaries, so state-dict normalization is not needed. A text/choice
//! widget with a value and no renderable appearance is reported with
//! decrypted geometry so the workbench can draw its real Unicode value as
//! read-only overlay text; the source PDF is never rewritten or re-serialized.

use std::collections::HashSet;

use hayro::hayro_interpret::TransformExt;
use hayro::hayro_syntax::Pdf;
use hayro::hayro_syntax::object::{
    Array, Dict, Object, ObjectIdentifier, Rect, String as PdfString,
};
use hayro::hayro_syntax::page::Page;
use hayro::kurbo::Point;

use super::{NormRect, decode_pdf_string};

/// What kind of input a form field takes. Pushbuttons are not represented.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FieldKind {
    #[default]
    Text,
    Checkbox,
    Radio,
    Choice,
    Signature,
}

/// `/Q` quadding: horizontal text alignment inside the field.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FieldAlignment {
    #[default]
    Left,
    Center,
    Right,
}

/// One form widget described for read-only display.
#[derive(Clone, Debug, Default)]
#[non_exhaustive]
pub struct FormField {
    /// Fully-qualified field name (`/T` up the `/Parent` chain, dot-joined).
    pub name: String,
    pub kind: FieldKind,
    /// 0-based page index.
    pub page: usize,
    /// Widget rectangle normalized to the displayed page (top-left origin).
    pub rect: NormRect,
    /// Current value. A password field's value is replaced by bullets before
    /// it leaves this module.
    pub value: String,
    pub read_only: bool,
    pub multiline: bool,
    pub password: bool,
    pub alignment: FieldAlignment,
    /// `/DA` font size in PDF points; `None` when absent or auto-sized.
    pub font_size: Option<f32>,
    /// Whether hayro has a renderable existing appearance (`/AP /N`, including
    /// an `/AS`-selected state). If false, the UI must draw `value` itself.
    pub has_appearance: bool,
}

impl FormField {
    /// A widget with its required identity and an empty value; callers add
    /// presentation fields as the document describes them.
    pub fn new(name: String, kind: FieldKind, page: usize, rect: NormRect) -> Self {
        Self {
            name,
            kind,
            page,
            rect,
            ..Self::default()
        }
    }

    /// A text/choice value with no appearance stream needs the workbench's
    /// read-only overlay. Checkboxes/radios/annotations are left alone: their
    /// native appearances (or blank Off state) are already correct.
    pub fn needs_text_overlay(&self) -> bool {
        !self.has_appearance
            && !self.value.is_empty()
            && matches!(self.kind, FieldKind::Text | FieldKind::Choice)
    }
}

/// Every form widget in page order. Hidden/no-view widgets are skipped.
pub fn form_fields(doc: &Pdf) -> Vec<FormField> {
    let xref = doc.xref();
    let default_da = xref
        .get::<Dict>(xref.root_id())
        .and_then(|catalog| catalog.get::<Dict>("AcroForm"))
        .and_then(|acro| acro.get::<PdfString>("DA"))
        .map(|da| String::from_utf8_lossy(da.as_bytes()).into_owned());
    let mut out = Vec::new();
    for (page_index, page) in doc.pages().iter().enumerate() {
        let Some(annots) = page.raw().get::<Array>("Annots") else {
            continue;
        };
        for annot in annots.iter::<Dict>() {
            if annot
                .get::<hayro::hayro_syntax::object::Name>("Subtype")
                .is_none_or(|subtype| subtype.as_str() != "Widget")
            {
                continue;
            }
            if hidden(&annot) {
                continue;
            }
            if let Some(field) = describe(&annot, &page, page_index, default_da.as_deref()) {
                out.push(field);
            }
        }
    }
    out
}

fn describe(
    widget: &Dict<'_>,
    page: &Page<'_>,
    page_index: usize,
    default_da: Option<&str>,
) -> Option<FormField> {
    let kind = kind_of(widget)?;
    let name = qualified_name(widget)?;
    let rect = normalized_rect(page, widget.get::<Rect>("Rect")?)?;
    let flags = field_attr(widget, "Ff")
        .and_then(|value| match value {
            Object::Number(number) => Some(number.as_i64()),
            _ => None,
        })
        .unwrap_or(0);
    let read_only = flags & 1 != 0;
    let multiline = flags & (1 << 12) != 0;
    let password = flags & (1 << 13) != 0;
    let alignment = match field_attr(widget, "Q").and_then(|value| match value {
        Object::Number(number) => Some(number.as_i64()),
        _ => None,
    }) {
        Some(1) => FieldAlignment::Center,
        Some(2) => FieldAlignment::Right,
        _ => FieldAlignment::Left,
    };
    let options = match kind {
        FieldKind::Choice => choice_options(widget),
        FieldKind::Checkbox | FieldKind::Radio => checkbox_states(widget),
        _ => Vec::new(),
    };
    let mut value = match kind {
        FieldKind::Text | FieldKind::Choice | FieldKind::Signature => {
            text_value(widget).unwrap_or_default()
        }
        FieldKind::Checkbox | FieldKind::Radio => field_attr(widget, "V")
            .or_else(|| widget.get::<Object>("AS"))
            .and_then(|value| match value {
                Object::Name(name) => Some(name.as_str().to_string()),
                Object::String(text) => Some(decode_pdf_string(text.as_bytes())),
                _ => None,
            })
            .unwrap_or_else(|| "Off".to_string()),
    };
    if kind == FieldKind::Choice && value.is_empty() {
        if let Some(index) = selected_choice_index(widget)
            && let Some(option) = options.get(index)
        {
            value = option.clone();
        }
    }
    if password && !value.is_empty() {
        value = "•".repeat(value.chars().count());
    }
    let da = field_attr(widget, "DA").and_then(|value| match value {
        Object::String(text) => Some(String::from_utf8_lossy(text.as_bytes()).into_owned()),
        _ => None,
    });
    let font_size = da.as_deref().or(default_da).and_then(parse_da_font_size);
    let as_name = widget
        .get::<hayro::hayro_syntax::object::Name>("AS")
        .map(|name| name.as_str().to_string());
    Some(FormField {
        name,
        kind,
        page: page_index,
        rect,
        value,
        read_only,
        multiline,
        password,
        alignment,
        font_size,
        has_appearance: has_appearance(widget, as_name.as_deref()),
    })
}

fn kind_of(widget: &Dict<'_>) -> Option<FieldKind> {
    let field_type = field_attr(widget, "FT")?;
    let name = match field_type {
        Object::Name(name) => name.as_str().to_string(),
        _ => return None,
    };
    let flags = field_attr(widget, "Ff")
        .and_then(|value| match value {
            Object::Number(number) => Some(number.as_i64()),
            _ => None,
        })
        .unwrap_or(0);
    match name.as_str() {
        "Tx" => Some(FieldKind::Text),
        "Ch" => Some(FieldKind::Choice),
        "Sig" => Some(FieldKind::Signature),
        "Btn" if flags & (1 << 16) != 0 => None,
        "Btn" if flags & (1 << 15) != 0 => Some(FieldKind::Radio),
        "Btn" => Some(FieldKind::Checkbox),
        _ => None,
    }
}

fn qualified_name(widget: &Dict<'_>) -> Option<String> {
    let mut parts = Vec::new();
    let mut current = widget.clone();
    let mut seen = HashSet::new();
    for _ in 0..32 {
        if let Some(id) = current.obj_id()
            && !seen.insert(id)
        {
            break;
        }
        if let Some(Object::String(title)) = current.get::<Object>("T") {
            parts.push(decode_pdf_string(title.as_bytes()));
        }
        match current.get::<Dict>("Parent") {
            Some(parent) => current = parent,
            None => break,
        }
    }
    if parts.is_empty() {
        return None;
    }
    parts.reverse();
    Some(parts.join("."))
}

fn field_attr<'a>(widget: &Dict<'a>, key: &str) -> Option<Object<'a>> {
    let mut current = widget.clone();
    let mut seen = HashSet::<ObjectIdentifier>::new();
    for _ in 0..32 {
        if let Some(value) = current.get::<Object>(key) {
            return Some(value);
        }
        if let Some(id) = current.obj_id()
            && !seen.insert(id)
        {
            return None;
        }
        current = current.get::<Dict>("Parent")?;
    }
    None
}

fn text_value(widget: &Dict<'_>) -> Option<String> {
    match field_attr(widget, "V")? {
        Object::String(text) => Some(decode_pdf_string(text.as_bytes())),
        Object::Name(name) => Some(name.as_str().to_string()),
        Object::Array(values) => {
            let parts: Vec<String> = values
                .iter::<Object>()
                .filter_map(|value| match value {
                    Object::String(text) => Some(decode_pdf_string(text.as_bytes())),
                    Object::Name(name) => Some(name.as_str().to_string()),
                    _ => None,
                })
                .collect();
            Some(parts.join(", "))
        }
        _ => None,
    }
}

fn choice_options(widget: &Dict<'_>) -> Vec<String> {
    let Some(Object::Array(options)) = field_attr(widget, "Opt") else {
        return Vec::new();
    };
    options
        .iter::<Object>()
        .filter_map(|option| match option {
            Object::String(text) => Some(decode_pdf_string(text.as_bytes())),
            Object::Array(pair) => pair.iter::<Object>().next().and_then(|first| match first {
                Object::String(text) => Some(decode_pdf_string(text.as_bytes())),
                Object::Name(name) => Some(name.as_str().to_string()),
                _ => None,
            }),
            _ => None,
        })
        .collect()
}

fn selected_choice_index(widget: &Dict<'_>) -> Option<usize> {
    match field_attr(widget, "I")? {
        Object::Number(number) => usize::try_from(number.as_i64()).ok(),
        Object::Array(indexes) => indexes
            .iter::<Object>()
            .next()
            .and_then(|index| match index {
                Object::Number(number) => usize::try_from(number.as_i64()).ok(),
                _ => None,
            }),
        _ => None,
    }
}

fn checkbox_states(widget: &Dict<'_>) -> Vec<String> {
    let Some(ap) = widget.get::<Dict>("AP") else {
        return Vec::new();
    };
    let Some(Object::Dict(states)) = ap.get::<Object>("N") else {
        return Vec::new();
    };
    states
        .keys()
        .filter(|key| key.as_ref() != b"Off")
        .map(|key| String::from_utf8_lossy(key.as_ref()).into_owned())
        .collect()
}

fn has_appearance(widget: &Dict<'_>, as_name: Option<&str>) -> bool {
    let Some(ap) = widget.get::<Dict>("AP") else {
        return false;
    };
    match ap.get::<Object>("N") {
        Some(Object::Stream(_)) => true,
        Some(Object::Dict(states)) => as_name
            .and_then(|name| states.get::<Object>(name))
            .is_some_and(|value| matches!(value, Object::Stream(_))),
        _ => false,
    }
}

fn hidden(widget: &Dict<'_>) -> bool {
    let flags = widget.get::<i64>("F").unwrap_or(0);
    flags & ((1 << 1) | (1 << 5)) != 0
}

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

/// `/Helv 18 Tf` → 18. A zero/auto size is treated as absent.
fn parse_da_font_size(da: &str) -> Option<f32> {
    let tokens: Vec<&str> = da.split_whitespace().collect();
    let index = tokens.iter().position(|token| *token == "Tf")?;
    let size = tokens.get(index.checked_sub(1)?)?.parse::<f32>().ok()?;
    (size.is_finite() && size > 0.0).then_some(size)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_da_font_size() {
        assert_eq!(parse_da_font_size("/Helv 18 Tf 0 g"), Some(18.0));
        assert_eq!(parse_da_font_size("/F1 0 Tf"), None);
        assert_eq!(parse_da_font_size("0 g"), None);
    }
}
