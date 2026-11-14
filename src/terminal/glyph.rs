//! Per-pane derived rendering of daemon-accepted monochrome glyphs.
//!
//! The daemon owns the accepted glossary and re-emits accepted `r`/`c` bodies
//! into the ordered pane stream. This module is the consumer half: it decodes
//! those bodies once on the reader thread, keeps a pane-local store with a
//! per-registration revision, and paints the outline into the cell with the
//! effective foreground. It never accepts a registration the daemon stripped,
//! never installs a font, and never changes the cell's logical width or the
//! codepoint kept for selection/copy/search.
//!
//! The full v1.12 sizing/placement model is implemented here from the pinned
//! `GlyphLayout` values: pad → size → align, then a clip to the unpadded render
//! span. A `width=2` registration widens pixels only when the consumer lends a
//! free following cell; the codepoint's logical width, cursor advance,
//! selection and copied text stay one cell.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use gpui_kit::{
    App, Bounds, ContentMask, Font, Hsla, PathBuilder, Pixels, SharedString, TextRun, Window,
    point, px,
};
use parking_lot::RwLock;
use rio_graphics::glyph::glyf_decode::{Outline, PathCmd};
use rio_graphics::glyph_layout::{GlyphAlignH, GlyphAlignV, GlyphLayout, GlyphSize};

/// Protocol capacity: one registration per codepoint, 1024 simultaneous slots.
const CAPACITY: usize = 1024;

/// One accepted registration as the renderer sees it.
pub(crate) struct GlyphView {
    /// Changes on every accepted registration for this codepoint, so a cached
    /// picture can never be served for a superseded definition. Retained now
    /// for the follow-on layout/retained-frame cache keys.
    #[allow(dead_code)]
    pub revision: u64,
    pub outline: Arc<Outline>,
    pub upm: u16,
    /// Declared render span. This slice paints only the protocol default.
    pub width: u8,
    pub layout: GlyphLayout,
}

struct Entry {
    glyph: Arc<GlyphView>,
    insertion: u64,
}

#[derive(Default)]
struct Inner {
    by_cp: HashMap<u32, Entry>,
    next_insertion: u64,
    next_revision: u64,
    /// Last daemon snapshot applied on this link. A lower generation is stale.
    generation: u64,
}

/// One decoded entry of an authoritative `GlyphState` snapshot.
pub(crate) struct GlyphReplayEntry {
    pub cp: u32,
    pub outline: Outline,
    pub upm: u16,
    pub width: u8,
    pub layout: GlyphLayout,
}

/// Pane-local derived glyph store. The reader writes accepted registrations;
/// the paint pass reads. Both hold the lock only for map work, never for
/// decoding or tessellation.
#[derive(Default)]
pub(crate) struct GlyphStore {
    inner: RwLock<Inner>,
}

impl GlyphStore {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Apply an accepted registration. A replacement keeps the codepoint's
    /// FIFO position and takes a new revision; a new codepoint at capacity
    /// evicts the oldest entry exactly as the daemon's registry does.
    pub(crate) fn apply(
        &self,
        cp: u32,
        outline: Outline,
        upm: u16,
        width: u8,
        layout: GlyphLayout,
    ) {
        let mut inner = self.inner.write();
        let revision = inner.next_revision;
        inner.next_revision = inner.next_revision.wrapping_add(1);
        let glyph = Arc::new(GlyphView {
            revision,
            outline: Arc::new(outline),
            upm,
            width,
            layout,
        });
        if let Some(entry) = inner.by_cp.get_mut(&cp) {
            entry.glyph = glyph;
            return;
        }
        if inner.by_cp.len() == CAPACITY
            && let Some(oldest) = inner
                .by_cp
                .iter()
                .min_by_key(|(_, entry)| entry.insertion)
                .map(|(cp, _)| *cp)
        {
            inner.by_cp.remove(&oldest);
        }
        let insertion = inner.next_insertion;
        inner.next_insertion = inner.next_insertion.wrapping_add(1);
        inner.by_cp.insert(cp, Entry { glyph, insertion });
    }

    pub(crate) fn clear(&self, cp: Option<u32>) {
        let mut inner = self.inner.write();
        match cp {
            Some(cp) => {
                inner.by_cp.remove(&cp);
            }
            None => inner.by_cp.clear(),
        }
    }

    pub(crate) fn clear_all(&self) {
        self.clear(None);
    }

    pub(crate) fn get(&self, cp: u32) -> Option<Arc<GlyphView>> {
        self.inner
            .read()
            .by_cp
            .get(&cp)
            .map(|entry| entry.glyph.clone())
    }

    /// Whether any pane-local definition is live. A presentation snapshot uses
    /// this once so panes without glyphs pay no per-cell lookup.
    pub(crate) fn is_empty(&self) -> bool {
        self.inner.read().by_cp.is_empty()
    }

    /// A new link starts with no derived glossary and no generation claim; the
    /// daemon's first snapshot on that link is authoritative even if the pane
    /// was restored from an older image with a lower counter.
    pub(crate) fn reset_link(&self) {
        let mut inner = self.inner.write();
        inner.by_cp.clear();
        inner.generation = 0;
    }

    /// Replace the whole derived glossary in one write. Returns false for a
    /// stale generation; the map is left untouched then. Build all [`Arc`]s
    /// under the same lock so paint sees the old set or the new one, never an
    /// empty map between a clear and its entries.
    pub(crate) fn apply_snapshot(&self, generation: u64, glyphs: Vec<GlyphReplayEntry>) -> bool {
        let mut inner = self.inner.write();
        if generation < inner.generation {
            return false;
        }
        inner.by_cp.clear();
        for glyph in glyphs {
            let revision = inner.next_revision;
            inner.next_revision = inner.next_revision.wrapping_add(1);
            let insertion = inner.next_insertion;
            inner.next_insertion = inner.next_insertion.wrapping_add(1);
            inner.by_cp.insert(
                glyph.cp,
                Entry {
                    glyph: Arc::new(GlyphView {
                        revision,
                        outline: Arc::new(glyph.outline),
                        upm: glyph.upm,
                        width: glyph.width,
                        layout: glyph.layout,
                    }),
                    insertion,
                },
            );
        }
        inner.generation = generation;
        true
    }

    #[cfg(test)]
    pub(crate) fn contains(&self, cp: u32) -> bool {
        self.inner.read().by_cp.contains_key(&cp)
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.inner.read().by_cp.len()
    }
}

/// The parse-time handle a bridge uses to apply accepted bodies. `authority`
/// is the negotiated endpoint's `glyph-registry` feature; without it the
/// consumer refuses to derive or claim anything from producer bytes.
#[derive(Clone, Copy)]
pub(crate) struct GlyphApply<'a> {
    pub store: &'a GlyphStore,
    pub authority: &'a AtomicBool,
    pub denied: &'a AtomicBool,
    /// Current link generation, compared before every shared mutation so a
    /// retired reader's late parse cannot revive a cleared or replaced entry.
    pub generation: &'a AtomicU64,
    pub reader: u64,
}

impl GlyphApply<'_> {
    fn current(&self) -> bool {
        self.generation.load(Ordering::Relaxed) == self.reader
    }

    /// Apply one accepted `glyf` payload. Returns whether the daemon's
    /// authority was present; a missing authority records the refusal instead.
    pub(crate) fn register(&self, cp: u32, glyf: &[u8], upm: u16, width: u8, layout: GlyphLayout) {
        if !self.current() {
            return;
        }
        if !self.authority.load(Ordering::Relaxed) {
            self.denied.store(true, Ordering::Relaxed);
            return;
        }
        if let Ok(outline) = rio_graphics::glyph::glyf_decode::decode(glyf) {
            self.store.apply(cp, outline, upm, width, layout);
        }
    }

    pub(crate) fn clear(&self, cp: Option<u32>) {
        if self.current() {
            self.store.clear(cp);
        }
    }

    /// RIS. Unlike [`GlyphStore::reset_link`] this keeps the last applied
    /// snapshot generation; the link is still the same one.
    pub(crate) fn reset(&self) {
        if self.current() {
            self.store.clear_all();
        }
    }
}

/// Where one outline sits inside its render span.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Placement {
    scale_x: f32,
    scale_y: f32,
    offset_x: f32,
    offset_y: f32,
}

/// §8.5 placement in logical pixels. `span_cells` is the render span the
/// consumer chose (one or two cells), and `baseline` is the primary text
/// baseline measured from the span's top edge.
///
/// Declared extents (`aw`/`lh`/`ymin`) are the frame for every member of a
/// coordinated set; only an undeclared extent falls back to the outline's
/// point-derived bounds, where `contain` is additionally capped by the em.
fn placement(
    outline: &Outline,
    upm: u16,
    span_cells: usize,
    layout: &GlyphLayout,
    cell_w: f32,
    cell_h: f32,
    baseline: f32,
) -> Option<Placement> {
    if upm == 0 || !(cell_w > 0. && cell_h > 0.) || !baseline.is_finite() {
        return None;
    }
    let span_cells = span_cells.clamp(1, 2);
    let span_w = cell_w * span_cells as f32;
    let span_h = cell_h;

    let declared = layout.aw.is_some() || layout.lh.is_some() || layout.ymin.is_some();
    let (extent_left, extent_bottom, extent_w, extent_h) = if declared {
        (
            0.,
            f32::from(layout.ymin.unwrap_or(0)),
            f32::from(layout.aw.unwrap_or(upm)),
            f32::from(layout.lh.unwrap_or(upm)),
        )
    } else {
        (
            outline.x_min as f32,
            outline.y_min as f32,
            (outline.x_max - outline.x_min) as f32,
            (outline.y_max - outline.y_min) as f32,
        )
    };
    if !(extent_w > 0. && extent_h > 0.) {
        return None;
    }

    // §8.5.2: a value outside 0..=1 is invalid, but one saturated axis means
    // the client's request is degenerate and all four insets become zero.
    let [top, right, bottom, left] = layout.pad;
    let valid = [top, right, bottom, left]
        .into_iter()
        .all(|pad| pad.is_finite() && (0.0..=1.0).contains(&pad));
    if !valid {
        return None;
    }
    let [top, right, bottom, left] = if left + right >= 1.0 || top + bottom >= 1.0 {
        [0.; 4]
    } else {
        [top, right, bottom, left]
    };
    let pad_left = left * span_w;
    let pad_right = right * span_w;
    let pad_top = top * span_h;
    let pad_bottom = bottom * span_h;
    let effective_w = span_w - pad_left - pad_right;
    let effective_h = span_h - pad_top - pad_bottom;
    if !(effective_w > 0. && effective_h > 0.) {
        return None;
    }

    let (scale_x, scale_y) = match layout.size {
        GlyphSize::Height => {
            let scale = effective_h / extent_h;
            (scale, scale)
        }
        GlyphSize::Advance => {
            let scale = effective_w / extent_w;
            (scale, scale)
        }
        GlyphSize::Contain => {
            let mut scale = (effective_w / extent_w).min(effective_h / extent_h);
            if !declared {
                scale = scale.min(effective_w.min(effective_h) / f32::from(upm));
            }
            (scale, scale)
        }
        GlyphSize::Cover => {
            let scale = (effective_w / extent_w).max(effective_h / extent_h);
            (scale, scale)
        }
        GlyphSize::Stretch => (effective_w / extent_w, effective_h / extent_h),
    };
    if !(scale_x > 0. && scale_y > 0. && scale_x.is_finite() && scale_y.is_finite()) {
        return None;
    }

    let left_edge = match layout.align_h {
        GlyphAlignH::Start => pad_left,
        GlyphAlignH::Center => pad_left + (effective_w - extent_w * scale_x) / 2.,
        GlyphAlignH::End => span_w - pad_right - extent_w * scale_x,
    };
    let origin_y = match layout.align_v {
        GlyphAlignV::Baseline => baseline - outline.y_max as f32 * scale_y,
        GlyphAlignV::Start => {
            span_h - pad_bottom - (outline.y_max as f32 - extent_bottom) * scale_y
        }
        GlyphAlignV::Center => {
            let placed_bottom = pad_bottom + (effective_h - extent_h * scale_y) / 2.;
            span_h - placed_bottom - (outline.y_max as f32 - extent_bottom) * scale_y
        }
        GlyphAlignV::End => {
            let placed_bottom = span_h - pad_top - extent_h * scale_y;
            span_h - placed_bottom - (outline.y_max as f32 - extent_bottom) * scale_y
        }
    };
    if !(left_edge.is_finite() && origin_y.is_finite()) {
        return None;
    }

    Some(Placement {
        scale_x,
        scale_y,
        // `walk(upm, upm)` keeps authored x and `y_max - y`, so these offsets
        // place that walk inside the span.
        offset_x: left_edge - extent_left * scale_x,
        offset_y: origin_y,
    })
}

/// Paint one registered glyph into its render span with the cell's effective
/// foreground. Every output-affecting input — outline, upm, declared layout,
/// span, cell geometry, baseline and foreground — is read on this call; no
/// derived picture is retained, so a superseded registration or a resized /
/// recoloured cell cannot be served from a stale cache.
pub(crate) fn paint(
    window: &mut Window,
    cell: Bounds<Pixels>,
    span_cells: usize,
    baseline: Pixels,
    glyph: &GlyphView,
    fg: Hsla,
) {
    let span_cells = if glyph.width == 2 {
        span_cells.clamp(1, 2)
    } else {
        1
    };
    let Some(place) = placement(
        &glyph.outline,
        glyph.upm,
        span_cells,
        &glyph.layout,
        cell.size.width.as_f32(),
        cell.size.height.as_f32(),
        baseline.as_f32(),
    ) else {
        return;
    };
    let span = Bounds::new(
        cell.origin,
        gpui_kit::size(cell.size.width * span_cells as f32, cell.size.height),
    );
    let origin_x = cell.origin.x.as_f32() + place.offset_x;
    let origin_y = cell.origin.y.as_f32() + place.offset_y;
    let at = |x: f32, y: f32| {
        point(
            px(origin_x + x * place.scale_x),
            px(origin_y + y * place.scale_y),
        )
    };
    let mut builder = PathBuilder::fill();
    // A upm-sized walk keeps authored coordinates: x, and `y_max - y`.
    for command in glyph.outline.walk(glyph.upm, f32::from(glyph.upm)) {
        match command {
            PathCmd::MoveTo { x, y } => builder.move_to(at(x, y)),
            PathCmd::LineTo { x, y } => builder.line_to(at(x, y)),
            PathCmd::QuadTo { cx, cy, x, y } => builder.curve_to(at(x, y), at(cx, cy)),
            PathCmd::Close => builder.close(),
        }
    }
    let Ok(path) = builder.build() else {
        return;
    };
    // §8.5.7: the clip is the render span before padding.
    window.with_content_mask(Some(ContentMask { bounds: span }), |window| {
        window.paint_path(path, fg);
    });
}

/// Whether the pane's real font stack can render `cp`.
///
/// The production shaping path is the source of truth: a shaped glyph id of 0
/// is `.notdef` (no font in the fallback chain had the codepoint), while a
/// real id — including a zero-ink one such as a space — is coverage. Ink
/// bounds are deliberately not used: a covered blank has none.
pub(crate) fn font_covers(cx: &App, font: &Font, font_size: Pixels, cp: u32) -> bool {
    let Some(ch) = char::from_u32(cp) else {
        return false;
    };
    let text = SharedString::from(ch.to_string());
    let run = TextRun {
        len: text.len(),
        font: font.clone(),
        color: Hsla::default(),
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let shaped = gpui_kit::WindowTextSystem::new(cx.text_system().clone()).shape_line(
        text,
        font_size,
        &[run],
        None,
    );
    shaped
        .runs
        .iter()
        .any(|run| run.glyphs.iter().any(|glyph| glyph.id.0 != 0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outline(x_min: i32, y_min: i32, x_max: i32, y_max: i32) -> Outline {
        Outline {
            contours: Vec::new(),
            x_min,
            y_min,
            x_max,
            y_max,
        }
    }

    fn layout(
        aw: Option<u16>,
        lh: Option<u16>,
        ymin: Option<i16>,
        size: GlyphSize,
        align_h: GlyphAlignH,
        align_v: GlyphAlignV,
        pad: [f32; 4],
    ) -> GlyphLayout {
        GlyphLayout {
            aw,
            lh,
            ymin,
            size,
            align_h,
            align_v,
            pad,
        }
    }

    fn close(left: f32, right: f32) {
        assert!(
            (left - right).abs() < 1e-5,
            "expected {right}, found {left}"
        );
    }

    #[test]
    fn a_square_fills_the_shorter_cell_axis_and_is_centred() {
        let place = placement(
            &outline(0, 0, 1000, 1000),
            1000,
            1,
            &GlyphLayout::default(),
            10.,
            20.,
            10.,
        )
        .unwrap();
        close(place.scale_x, 0.01);
        close(place.scale_y, 0.01);
        close(place.offset_x, 0.);
        close(place.offset_y, 5.);
    }

    #[test]
    fn a_wide_outline_is_advance_bound_and_keeps_its_aspect() {
        let place = placement(
            &outline(0, 0, 2000, 500),
            1000,
            1,
            &GlyphLayout::default(),
            10.,
            20.,
            10.,
        )
        .unwrap();
        close(place.scale_x, 0.005);
        close(place.scale_y, 0.005);
        close(place.offset_x, 0.);
        close(place.offset_y, 8.75);
    }

    #[test]
    fn implicit_contain_is_capped_by_the_em() {
        let place = placement(
            &outline(0, 0, 100, 100),
            1000,
            1,
            &GlyphLayout::default(),
            10.,
            20.,
            10.,
        )
        .unwrap();
        close(place.scale_x, 0.01);
        close(place.scale_y, 0.01);
        close(place.offset_x, 4.5);
        close(place.offset_y, 9.5);
    }

    #[test]
    fn a_negative_origin_keeps_the_outline_centred() {
        let place = placement(
            &outline(-500, 0, 500, 1000),
            1000,
            1,
            &GlyphLayout::default(),
            10.,
            20.,
            10.,
        )
        .unwrap();
        close(place.scale_x, 0.01);
        close(place.scale_y, 0.01);
        close(place.offset_x, 5.);
        close(place.offset_y, 5.);
    }

    #[test]
    fn an_empty_outline_has_no_placement() {
        assert!(
            placement(
                &outline(0, 0, 0, 0),
                1000,
                1,
                &GlyphLayout::default(),
                10.,
                20.,
                10.
            )
            .is_none()
        );
    }

    #[test]
    fn declared_extent_does_not_follow_each_outline_bbox() {
        let layout = layout(
            Some(1000),
            Some(1000),
            Some(0),
            GlyphSize::Contain,
            GlyphAlignH::Center,
            GlyphAlignV::Center,
            [0.; 4],
        );
        let square =
            placement(&outline(0, 0, 1000, 1000), 1000, 1, &layout, 10., 20., 10.).unwrap();
        let inset = placement(
            &outline(200, 300, 800, 900),
            1000,
            1,
            &layout,
            10.,
            20.,
            10.,
        )
        .unwrap();
        close(square.scale_x, inset.scale_x);
        close(square.scale_y, inset.scale_y);
        // offset_y is relative to each outline's own y_max; compare the
        // screen position of the shared authored y=0 instead.
        close(
            square.offset_y + 1000. * square.scale_y,
            inset.offset_y + 900. * inset.scale_y,
        );
        close(square.offset_x, inset.offset_x);
    }

    #[test]
    fn every_size_mode_uses_the_declared_extent() {
        let mode = |size| {
            let layout = layout(
                Some(1000),
                Some(1000),
                Some(0),
                size,
                GlyphAlignH::Start,
                GlyphAlignV::Start,
                [0.; 4],
            );
            placement(&outline(0, 0, 1000, 1000), 1000, 1, &layout, 100., 40., 0.).unwrap()
        };
        let height = mode(GlyphSize::Height);
        close(height.scale_x, 0.04);
        close(height.scale_y, 0.04);
        close(height.offset_x, 0.);
        close(height.offset_y, 0.);

        let advance = mode(GlyphSize::Advance);
        close(advance.scale_x, 0.1);
        close(advance.scale_y, 0.1);
        close(advance.offset_x, 0.);
        close(advance.offset_y, -60.);

        let contain = mode(GlyphSize::Contain);
        close(contain.scale_x, 0.04);
        close(contain.scale_y, 0.04);
        close(contain.offset_x, 0.);
        close(contain.offset_y, 0.);

        let cover = mode(GlyphSize::Cover);
        close(cover.scale_x, 0.1);
        close(cover.scale_y, 0.1);
        close(cover.offset_x, 0.);
        close(cover.offset_y, -60.);

        let stretch = mode(GlyphSize::Stretch);
        close(stretch.scale_x, 0.1);
        close(stretch.scale_y, 0.04);
        close(stretch.offset_x, 0.);
        close(stretch.offset_y, 0.);
    }

    #[test]
    fn baseline_anchors_authored_zero_and_lets_descenders_hang() {
        let layout = layout(
            Some(1000),
            Some(1000),
            Some(-200),
            GlyphSize::Stretch,
            GlyphAlignH::Center,
            GlyphAlignV::Baseline,
            [0.; 4],
        );
        let place = placement(
            &outline(0, -200, 1000, 800),
            1000,
            1,
            &layout,
            100.,
            40.,
            30.,
        )
        .unwrap();
        close(place.scale_y, 0.04);
        close(place.offset_y, 30. - 800. * 0.04);
        let authored_zero = place.offset_y + 800. * place.scale_y;
        close(authored_zero, 30.);
        let descender = place.offset_y + 1000. * place.scale_y;
        assert!(descender > 30., "descender is below the baseline");
    }

    #[test]
    fn padding_and_alignment_use_the_effective_span() {
        let padded = layout(
            Some(1000),
            Some(1000),
            Some(0),
            GlyphSize::Stretch,
            GlyphAlignH::End,
            GlyphAlignV::Start,
            [0.1, 0.2, 0.0, 0.3],
        );
        let place = placement(&outline(0, 0, 1000, 1000), 1000, 1, &padded, 100., 40., 0.).unwrap();
        close(place.scale_x, 0.05);
        close(place.scale_y, 0.036);
        close(place.offset_x, 30.);
        close(place.offset_y, 4.);

        let centred = layout(
            Some(1000),
            Some(1000),
            Some(0),
            GlyphSize::Stretch,
            GlyphAlignH::Center,
            GlyphAlignV::Center,
            [0.2; 4],
        );
        let place =
            placement(&outline(0, 0, 1000, 1000), 1000, 1, &centred, 100., 40., 0.).unwrap();
        close(place.scale_x, 0.06);
        close(place.scale_y, 0.024);
        close(place.offset_x, 20.);
        close(place.offset_y, 8.);
    }

    #[test]
    fn one_saturated_padding_axis_zeroes_all_four_insets() {
        for pad in [
            [0.4, 0.2, 0.6, 0.1], // top + bottom = 1.0
            [0.1, 0.5, 0.2, 0.5], // left + right = 1.0
        ] {
            let layout = layout(
                Some(1000),
                Some(1000),
                Some(0),
                GlyphSize::Stretch,
                GlyphAlignH::Start,
                GlyphAlignV::Start,
                pad,
            );
            let place =
                placement(&outline(0, 0, 1000, 1000), 1000, 1, &layout, 100., 40., 0.).unwrap();
            close(place.scale_x, 0.1);
            close(place.scale_y, 0.04);
            close(place.offset_x, 0.);
            close(place.offset_y, 0.);
        }
    }

    #[test]
    fn invalid_padding_and_zero_extents_have_no_placement() {
        let invalid = layout(
            Some(1000),
            Some(1000),
            Some(0),
            GlyphSize::Contain,
            GlyphAlignH::Center,
            GlyphAlignV::Center,
            [0., 0., 0., 1.5],
        );
        assert!(placement(&outline(0, 0, 1000, 1000), 1000, 1, &invalid, 10., 20., 10.).is_none());
        assert!(
            placement(
                &outline(0, 0, 0, 0),
                1000,
                1,
                &GlyphLayout::default(),
                10.,
                20.,
                10.
            )
            .is_none()
        );
        assert!(
            placement(
                &outline(0, 0, 1000, 1000),
                0,
                1,
                &GlyphLayout::default(),
                10.,
                20.,
                10.
            )
            .is_none()
        );
    }

    #[test]
    fn a_wide_span_scales_with_two_cells() {
        let layout = layout(
            Some(1000),
            Some(1000),
            Some(0),
            GlyphSize::Stretch,
            GlyphAlignH::Start,
            GlyphAlignV::Start,
            [0.; 4],
        );
        let place = placement(&outline(0, 0, 1000, 1000), 1000, 2, &layout, 100., 40., 0.).unwrap();
        close(place.scale_x, 0.2);
        close(place.scale_y, 0.04);
        close(place.offset_x, 0.);
        close(place.offset_y, 0.);
    }

    #[test]
    fn a_store_replaces_in_place_and_evicts_oldest_at_capacity() {
        let store = GlyphStore::new();
        store.apply(
            0x100000,
            outline(0, 0, 10, 10),
            1000,
            1,
            GlyphLayout::default(),
        );
        let first = store.get(0x100000).unwrap();
        store.apply(
            0x100000,
            outline(0, 0, 20, 20),
            1000,
            1,
            GlyphLayout::default(),
        );
        let second = store.get(0x100000).unwrap();
        assert_ne!(first.revision, second.revision);
        assert_eq!(store.len(), 1);
        for cp in 0x100001..0x100001 + (CAPACITY as u32 - 1) {
            store.apply(cp, outline(0, 0, 10, 10), 1000, 1, GlyphLayout::default());
        }
        assert_eq!(store.len(), CAPACITY);
        store.apply(
            0x10fffd,
            outline(0, 0, 10, 10),
            1000,
            1,
            GlyphLayout::default(),
        );
        assert_eq!(store.len(), CAPACITY);
        assert!(!store.contains(0x100000), "oldest registration was evicted");
        assert!(store.contains(0x10fffd));
    }

    #[test]
    fn glyph_captured_definition_outlives_replacement_and_clear_until_its_owner_drops() {
        let store = GlyphStore::new();
        let cp = 0x100000;
        store.apply(cp, outline(0, 0, 10, 10), 1000, 1, GlyphLayout::default());
        let captured = store.get(cp).unwrap();
        let weak = Arc::downgrade(&captured);

        store.apply(cp, outline(0, 0, 20, 20), 1000, 1, GlyphLayout::default());
        assert_eq!(
            captured.outline.x_max, 10,
            "a captured definition is never rewritten in place"
        );
        assert!(
            weak.upgrade().is_some(),
            "replacing the live entry cannot release a captured version"
        );

        store.clear(Some(cp));
        assert!(store.get(cp).is_none(), "the live registration is gone");
        assert!(
            weak.upgrade().is_some(),
            "clearing the live entry cannot release a captured version"
        );

        drop(captured);
        assert!(
            weak.upgrade().is_none(),
            "the last captured owner retires the definition"
        );
    }

    #[test]
    fn glyph_superseded_definition_is_not_revived_by_a_later_read() {
        let store = GlyphStore::new();
        let cp = 0x100000;
        store.apply(cp, outline(0, 0, 10, 10), 1000, 1, GlyphLayout::default());
        let captured = store.get(cp).unwrap();
        store.apply(cp, outline(0, 0, 20, 20), 1000, 1, GlyphLayout::default());
        let live = store.get(cp).unwrap();
        assert_ne!(
            captured.revision, live.revision,
            "a later read must resolve the superseding definition"
        );
        assert_eq!(live.outline.x_max, 20);
        assert_eq!(
            captured.outline.x_max, 10,
            "the stale owner keeps only its own captured version"
        );
    }
}
