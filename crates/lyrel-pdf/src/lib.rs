//! GUI-free PDF reading backend.
//!
//! This is the adapted, independently maintained half of the pinned Zorite
//! `crates/gpui-pdf` MIT implementation: document loading and password
//! mapping, page measurement, single-page rasterization, the page-window
//! geometry, the text layer, the outline/link traversal and the read-only form
//! description. The workbench owns document identity, Host acquisition,
//! per-presentation page/zoom state and bitmap leases; nothing here knows
//! GPUI, a path, or a save target.
//!
//! Original bytes stay authoritative. The backend never rewrites them: form
//! values without an appearance stream are reported with decrypted geometry so
//! the UI can draw read-only text, instead of synthesizing a lossy WinAnsi
//! appearance into the document.

use std::fmt;
use std::sync::Arc;

use hayro::hayro_interpret::InterpreterSettings;
use hayro::hayro_syntax::{DecryptionError, LoadPdfError, Pdf};

pub mod forms;
pub mod outline;
pub mod text;

#[cfg(any(test, feature = "test-fixtures"))]
pub mod fixtures;

pub use forms::{FieldAlignment, FieldKind, FormField};
pub use outline::{Destination, LinkTarget, OutlineItem, PdfLink};
pub use text::{NormPoint, PageText, Selection, TextSelection};

/// A rectangle in normalized page coordinates: fractions of the page's
/// displayed width/height with a top-left origin, independent of zoom and DPI.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct NormRect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl NormRect {
    pub(crate) fn right(&self) -> f32 {
        self.x + self.w
    }

    pub(crate) fn bottom(&self) -> f32 {
        self.y + self.h
    }

    pub(crate) fn center_y(&self) -> f32 {
        self.y + self.h / 2.0
    }
}

/// Why a document could not be read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoadError {
    /// The PDF is encrypted and the supplied password was missing or wrong.
    /// The caller may prompt and retry with the same bytes.
    Locked,
    /// Malformed input, unsupported encryption, or an internal syntax refusal.
    Other(String),
}

impl fmt::Display for LoadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Locked => formatter.write_str("password required"),
            Self::Other(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for LoadError {}

/// A parsed document: immutable source bytes, page sizes, navigation metadata
/// and read-only form descriptions, shared across background render tasks.
pub struct PdfDocument {
    pdf: Arc<Pdf>,
    bytes: Arc<Vec<u8>>,
    dims: Vec<(f32, f32)>,
    outline: Vec<OutlineItem>,
    links: Vec<Vec<PdfLink>>,
    fields: Vec<FormField>,
}

impl PdfDocument {
    /// Parse `bytes` with `password`, then measure pages and collect the
    /// metadata a viewer needs. The bytes are retained by the parsed document.
    pub fn load(bytes: Arc<Vec<u8>>, password: &str) -> Result<Self, LoadError> {
        let pdf = Pdf::new_with_password(bytes.clone(), password).map_err(map_load_error)?;
        let dims = page_dims(&pdf);
        let outline = outline::outline(&pdf);
        let links = outline::page_links(&pdf);
        let fields = forms::form_fields(&pdf);
        Ok(Self {
            pdf: Arc::new(pdf),
            bytes,
            dims,
            outline,
            links,
            fields,
        })
    }

    /// The exact bytes this document was parsed from. Read-only by contract.
    pub fn bytes(&self) -> &Arc<Vec<u8>> {
        &self.bytes
    }

    pub fn page_count(&self) -> usize {
        self.dims.len()
    }

    /// Each page's displayed `(width, height)` in PDF points, including
    /// rotation. Kept in document order.
    pub fn page_dims(&self) -> &[(f32, f32)] {
        &self.dims
    }

    pub fn page_size(&self, index: usize) -> Option<(f32, f32)> {
        self.dims.get(index).copied()
    }

    pub fn outline(&self) -> &[OutlineItem] {
        &self.outline
    }

    /// Clickable link annotations per page, in normalized page coordinates.
    pub fn links(&self) -> &[Vec<PdfLink>] {
        &self.links
    }

    /// Read-only form widgets in page order.
    pub fn fields(&self) -> &[FormField] {
        &self.fields
    }

    /// Rasterize one page (0-based) at `scale` PDF-points-to-pixels, composited
    /// onto white as opaque RGBA. The caller turns this into its own texture.
    ///
    /// `cancelled` is checked before and after the interpret pass. hayro's
    /// rasterizer cannot be interrupted mid-page, so cancellation retires the
    /// result and the scheduling generation rather than aborting the CPU pass;
    /// callers must not treat a completed result as current after cancelling.
    pub fn render_page(
        &self,
        index: usize,
        scale: f32,
        cancelled: &(dyn Fn() -> bool + Send + Sync),
    ) -> Result<RenderedPage, RenderError> {
        if index >= self.dims.len() {
            return Err(RenderError::NoPage(index));
        }
        if cancelled() {
            return Err(RenderError::Cancelled);
        }
        let scale = bounded_scale(self.dims[index], scale);
        let pixmaps = hayro::render_pdf(
            &self.pdf,
            scale,
            InterpreterSettings::default(),
            Some(index..=index),
        )
        .ok_or(RenderError::Failed(index))?;
        if cancelled() {
            return Err(RenderError::Cancelled);
        }
        let pixmap = pixmaps
            .into_iter()
            .next()
            .ok_or(RenderError::Failed(index))?;
        if cancelled() {
            return Err(RenderError::Cancelled);
        }
        let (width, height) = (u32::from(pixmap.width()), u32::from(pixmap.height()));
        let src = pixmap.data_as_u8_slice();
        let mut rgba = Vec::with_capacity(src.len());
        for pixel in src.as_chunks::<4>().0 {
            // Premultiplied source over white, then opaque RGBA. The UI's
            // existing bitmap path performs the final RGBA→BGRA swap.
            let add = 255 - pixel[3];
            rgba.push(pixel[0].saturating_add(add));
            rgba.push(pixel[1].saturating_add(add));
            rgba.push(pixel[2].saturating_add(add));
            rgba.push(255);
        }
        Ok(RenderedPage {
            width,
            height,
            rgba,
        })
    }

    /// Extract one page's text layer. Runs a non-rasterizing interpret pass, so
    /// callers cache the result per page and never extract the whole document
    /// just to show a page.
    pub fn text(&self, index: usize) -> Option<PageText> {
        text::extract_page_text(&self.pdf, index)
    }
}

impl fmt::Debug for PdfDocument {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PdfDocument")
            .field("pages", &self.dims.len())
            .field("outline", &self.outline.len())
            .field("fields", &self.fields.len())
            .finish_non_exhaustive()
    }
}

/// A rasterized page composited over white, row-major opaque RGBA.
pub struct RenderedPage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl fmt::Debug for RenderedPage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RenderedPage")
            .field("width", &self.width)
            .field("height", &self.height)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RenderError {
    NoPage(usize),
    Failed(usize),
    Cancelled,
}

impl fmt::Display for RenderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoPage(index) => write!(formatter, "no page {index}"),
            Self::Failed(index) => write!(formatter, "render page {index}"),
            Self::Cancelled => formatter.write_str("render cancelled"),
        }
    }
}

impl std::error::Error for RenderError {}

fn map_load_error(error: LoadPdfError) -> LoadError {
    match error {
        LoadPdfError::Decryption(DecryptionError::PasswordProtected) => LoadError::Locked,
        LoadPdfError::Decryption(other) => LoadError::Other(format!("PDF encryption: {other:?}")),
        LoadPdfError::Invalid => LoadError::Other("invalid PDF".to_string()),
    }
}

fn page_dims(doc: &Pdf) -> Vec<(f32, f32)> {
    doc.pages()
        .iter()
        .map(|page| page.render_dimensions())
        .collect()
}

/// Base on-screen page width (logical points) at 100%.
pub const PAGE_WIDTH: f32 = 820.0;
/// Vertical gap between page slots.
pub const PAGE_GAP: f32 = 10.0;
/// Top/bottom padding of the page column.
pub const PAGE_PAD_Y: f32 = 16.0;
/// Extra pages kept rasterized beyond the viewport.
pub const MARGIN: usize = 2;
/// Hard cap on one page's rasterization scale, so high zoom/DPI cannot mint
/// runaway textures. Above this, pages soften instead of ballooning.
pub const MAX_RENDER_SCALE: f32 = 4.0;
/// One page's raster budget; larger pages are rendered at a lower scale.
pub const MAX_PAGE_PIXELS: u64 = 4 * 1024 * 1024;

/// A page's on-screen height at `page_width`, preserving the displayed aspect.
pub fn display_height((width, height): (f32, f32), page_width: f32) -> f32 {
    if width > 0.0 {
        page_width * (height / width)
    } else {
        page_width
    }
}

/// The y offset of page `index`'s top in the laid-out column.
pub fn page_top_y(dims: &[(f32, f32)], page_width: f32, index: usize) -> f32 {
    let mut y = PAGE_PAD_Y;
    for dim in dims.iter().take(index) {
        y += display_height(*dim, page_width) + PAGE_GAP;
    }
    y
}

/// Total scrollable height of the page column.
pub fn content_height(dims: &[(f32, f32)], page_width: f32) -> f32 {
    if dims.is_empty() {
        return 2.0 * PAGE_PAD_Y;
    }
    2.0 * PAGE_PAD_Y
        + dims
            .iter()
            .map(|dim| display_height(*dim, page_width))
            .sum::<f32>()
        + (dims.len().saturating_sub(1)) as f32 * PAGE_GAP
}

/// The inclusive page range to keep rasterized for the given scroll position:
/// pages intersecting the viewport padded by [`MARGIN`]. Pure and shared by
/// the production view and its tests.
pub fn keep_window(
    dims: &[(f32, f32)],
    page_width: f32,
    scroll_y: f32,
    viewport_h: f32,
) -> (usize, usize) {
    if dims.is_empty() {
        return (0, 0);
    }
    let vh = if viewport_h > 1.0 { viewport_h } else { 900.0 };
    let top = scroll_y.max(0.0);
    let bottom = top + vh;
    let mut y = PAGE_PAD_Y;
    let mut first = None;
    let mut last = 0;
    for (index, dim) in dims.iter().enumerate() {
        let page_top = y;
        let page_bottom = y + display_height(*dim, page_width);
        if page_bottom > top && page_top < bottom {
            first.get_or_insert(index);
            last = index;
        }
        y = page_bottom + PAGE_GAP;
    }
    let first = first.unwrap_or(0);
    (
        first.saturating_sub(MARGIN),
        (last + MARGIN).min(dims.len() - 1),
    )
}

/// The topmost page intersecting the viewport top — the page counter's value.
pub fn current_page(dims: &[(f32, f32)], page_width: f32, scroll_y: f32) -> usize {
    let top = scroll_y.max(0.0);
    let mut y = PAGE_PAD_Y;
    for (index, dim) in dims.iter().enumerate() {
        let page_bottom = y + display_height(*dim, page_width);
        if page_bottom > top {
            return index;
        }
        y = page_bottom + PAGE_GAP;
    }
    dims.len().saturating_sub(1)
}

/// The rasterization scale for one page at `page_width` logical points and the
/// display's pixel ratio, bounded by [`MAX_RENDER_SCALE`].
pub fn render_scale(page_width: f32, scale_factor: f32, page_pt_width: f32) -> f32 {
    if page_pt_width > 0.0 {
        (page_width * scale_factor / page_pt_width).clamp(0.5, MAX_RENDER_SCALE)
    } else {
        1.5
    }
}

/// Reduce a requested scale until the page fits [`MAX_PAGE_PIXELS`].
pub fn bounded_scale(page_pt: (f32, f32), scale: f32) -> f32 {
    let area = f64::from(page_pt.0.max(1.0)) * f64::from(page_pt.1.max(1.0));
    let max_scale = (MAX_PAGE_PIXELS as f64 / area).sqrt() as f32;
    scale.clamp(0.05, MAX_RENDER_SCALE).min(max_scale)
}

/// Decode a PDF text string: UTF-16BE with a BOM, otherwise PDFDocEncoding
/// treated as Latin-1 (the same lossy stand-in the pinned viewer uses).
pub(crate) fn decode_pdf_string(bytes: &[u8]) -> String {
    if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        let units: Vec<u16> = rest
            .as_chunks::<2>()
            .0
            .iter()
            .map(|chunk| u16::from_be_bytes(*chunk))
            .collect();
        String::from_utf16_lossy(&units).trim().to_string()
    } else {
        bytes
            .iter()
            .map(|&byte| byte as char)
            .collect::<String>()
            .trim()
            .to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_and_page_geometry_are_pure() {
        let dims = [(612.0, 792.0); 4];
        let width = 612.0;
        assert!((display_height(dims[0], width) - 792.0).abs() < 0.01);
        assert_eq!(current_page(&dims, width, 0.0), 0);
        assert_eq!(current_page(&dims, width, 900.0), 1);
        assert_eq!(keep_window(&dims, width, 0.0, 900.0), (0, 3));
        assert_eq!(keep_window(&dims, width, 3_000.0, 900.0), (1, 3));
        assert!(content_height(&dims, width) > 4.0 * 792.0);
    }

    #[test]
    fn render_scale_respects_dpi_and_pixel_budget() {
        let scale = render_scale(820.0, 2.0, 612.0);
        assert!(scale > 2.5 && scale <= MAX_RENDER_SCALE);
        let bounded = bounded_scale((612.0, 792.0), 4.0);
        assert!(bounded < 4.0);
        let pixels = (612.0 * bounded) as u64 * (792.0 * bounded) as u64;
        assert!(pixels <= MAX_PAGE_PIXELS);
    }

    #[test]
    fn pdf_text_strings_decode_utf16_and_latin1() {
        assert_eq!(
            decode_pdf_string(&[0xFE, 0xFF, 0x00, b'H', 0x00, b'i']),
            "Hi"
        );
        assert_eq!(decode_pdf_string(&[b'C', 0xE9]), "Cé");
    }

    use crate::fixtures;

    fn never_cancelled() -> bool {
        false
    }

    fn load_reading() -> PdfDocument {
        PdfDocument::load(Arc::new(fixtures::reading_document()), "").expect("reading fixture")
    }

    #[test]
    fn reading_fixture_measures_pages_and_resolves_named_destinations() {
        let document = load_reading();
        assert_eq!(document.page_count(), 12);
        assert_eq!(document.page_size(2), Some((792.0, 612.0)));
        let outline = document.outline();
        assert_eq!(outline.len(), 3);
        assert_eq!(outline[0].page(), Some(0));
        assert_eq!(outline[1].page(), Some(5));
        assert_eq!(outline[2].page(), Some(11));
        let middle = outline[1].destination.unwrap();
        assert!((middle.y.unwrap() - (792.0 - 680.0) / 792.0).abs() < 0.001);
        assert!((middle.x.unwrap() - 0.0).abs() < 0.001);
    }

    #[test]
    fn reading_fixture_renders_a_page_and_keeps_blue_pixels() {
        let document = load_reading();
        let page = document
            .render_page(0, 1.0, &never_cancelled)
            .expect("renders page");
        assert_eq!((page.width, page.height), (612, 792));
        assert_eq!(page.rgba.len(), 612 * 792 * 4);
        let pixel = |x: usize, y: usize| {
            let offset = (y * 612 + x) * 4;
            &page.rgba[offset..offset + 4]
        };
        let blue = pixel(100, 100);
        assert!(blue[2] > 150 && blue[0] < 80, "blue rectangle: {blue:?}");
        assert_eq!(blue[3], 255);
    }

    #[test]
    fn cancellation_retires_a_page_render() {
        let document = load_reading();
        assert_eq!(
            document.render_page(0, 1.0, &|| true).unwrap_err(),
            RenderError::Cancelled
        );
        assert_eq!(
            document.render_page(99, 1.0, &never_cancelled).unwrap_err(),
            RenderError::NoPage(99)
        );
    }

    #[test]
    fn encrypted_documents_require_the_password_and_keep_their_content() {
        let bytes = fixtures::encrypted_reading_document();
        assert_eq!(
            PdfDocument::load(Arc::new(bytes.clone()), "").unwrap_err(),
            LoadError::Locked
        );
        assert_eq!(
            PdfDocument::load(Arc::new(bytes.clone()), fixtures::WRONG_PASSWORD).unwrap_err(),
            LoadError::Locked
        );
        let document =
            PdfDocument::load(Arc::new(bytes), fixtures::TEST_PASSWORD).expect("unlocked");
        assert_eq!(document.page_count(), 12);
        assert!(document.text(0).unwrap().text().contains("unique-page-01"));
        assert_eq!(document.outline().len(), 3);
    }

    #[test]
    fn unicode_text_extracts_matches_and_locates_repeated_multibyte_text() {
        let document =
            PdfDocument::load(Arc::new(fixtures::unicode_text_document()), "").expect("unicode");
        let text = document.text(0).expect("text page");
        assert_eq!(text.text(), "中文 中文\n中文");
        assert_eq!(text.find_matches("中文").len(), 3);
        assert_eq!(text.locate("中文", 1).len(), 1);
        assert!(text.locate("中文", 3).is_empty());
    }

    #[test]
    fn clipped_text_omits_the_fully_clipped_run() {
        let document =
            PdfDocument::load(Arc::new(fixtures::clipped_text_document()), "").expect("clipped");
        let text = document.text(0).expect("text page").text();
        assert!(text.contains("inside clip"), "got {text:?}");
        assert!(!text.contains("outside clip"), "got {text:?}");
    }

    #[test]
    fn draw_order_is_replaced_by_visual_reading_order() {
        let document =
            PdfDocument::load(Arc::new(fixtures::drawing_order_document()), "").expect("order");
        let text = document.text(0).expect("text page").text();
        assert!(text.starts_with("Hello World"), "got {text:?}");
    }

    #[test]
    fn scan_only_pages_have_no_invented_text() {
        let document =
            PdfDocument::load(Arc::new(fixtures::scan_only_document()), "").expect("scan");
        assert!(document.text(0).expect("page").is_empty());
        let page = document
            .render_page(0, 1.0, &never_cancelled)
            .expect("renders the image page");
        assert_eq!((page.width, page.height), (612, 792));
    }

    #[test]
    fn existing_appearances_render_and_do_not_need_an_overlay() {
        let document =
            PdfDocument::load(Arc::new(fixtures::forms_document(false)), "").expect("forms");
        let fields = document.fields();
        assert_eq!(fields.len(), 2);
        assert!(fields.iter().all(|field| field.has_appearance));
        assert!(fields.iter().all(|field| !field.needs_text_overlay()));
        assert_eq!(fields[0].value, "FILLED AP 279");
        assert_eq!(fields[1].value, "Yes");
        let page = document
            .render_page(0, 1.0, &never_cancelled)
            .expect("renders appearance page");
        let pixel = |x: usize, y: usize| {
            let offset = (y * 612 + x) * 4;
            &page.rgba[offset..offset + 4]
        };
        // Checkbox /AS=Yes: green fill at PDF (60,552) → display y 240.
        let green = pixel(60, 240);
        assert!(
            green[1] > 100 && green[0] < 80,
            "green check state: {green:?}"
        );
        // Text field AP background 0.85,0.9,1 at PDF (50,620) → display y 172.
        let field = pixel(50, 172);
        assert!(field[0] > 180 && field[2] > 220, "field AP: {field:?}");
    }

    #[test]
    fn missing_appearances_report_unicode_geometry_for_read_only_overlay() {
        let document =
            PdfDocument::load(Arc::new(fixtures::forms_document(true)), "").expect("missing");
        let fields = document.fields();
        assert_eq!(fields.len(), 2);
        assert_eq!(fields[0].value, "中文表单279");
        assert_eq!(fields[1].value, "第一行\n第二行");
        assert!(fields[1].multiline);
        assert_eq!(fields[1].alignment, forms::FieldAlignment::Right);
        assert_eq!(fields[0].font_size, Some(18.0));
        assert!(fields.iter().all(|field| field.needs_text_overlay()));
        for field in fields {
            assert!(field.rect.w > 0.0 && field.rect.h > 0.0);
            assert!(field.rect.x + field.rect.w <= 1.0);
            assert!(field.rect.y + field.rect.h <= 1.0);
        }
        let page = document
            .render_page(0, 1.0, &never_cancelled)
            .expect("renders page body");
        assert!(!page.rgba.is_empty());
    }

    #[test]
    fn encrypted_forms_still_expose_decrypted_values() {
        let document = PdfDocument::load(
            Arc::new(fixtures::encrypted_forms_document()),
            fixtures::TEST_PASSWORD,
        )
        .expect("encrypted forms");
        let fields = document.fields();
        assert_eq!(fields[0].value, "中文表单279");
        assert!(fields[0].needs_text_overlay());
    }

    #[test]
    fn rotated_page_links_and_named_destinations_resolve() {
        let document = PdfDocument::load(Arc::new(fixtures::links_document()), "").expect("links");
        let links = document.links();
        assert_eq!(links[0].len(), 2);
        assert!(links[0].iter().all(|link| {
            link.x >= 0.0 && link.y >= 0.0 && link.x + link.w <= 1.0 && link.y + link.h <= 1.0
        }));
        assert!(links[0].iter().any(|link| {
            matches!(&link.target, LinkTarget::Page(destination) if destination.page == 1)
        }));
        assert!(links[0].iter().any(|link| {
            matches!(&link.target, LinkTarget::Uri(uri) if uri == "https://example.com/lyrel-279")
        }));
        assert!(links[1].iter().any(|link| {
            matches!(&link.target, LinkTarget::Page(destination) if destination.page == 1)
        }));
    }

    #[test]
    fn page_dimensions_and_text_use_the_same_rotated_page() {
        let document = load_reading();
        let rotated = document.text(2).expect("rotated page text");
        assert!(rotated.text().contains("unique-page-03"));
        let page = document
            .render_page(2, 1.0, &never_cancelled)
            .expect("rotated render");
        assert_eq!((page.width, page.height), (792, 612));
    }
}
