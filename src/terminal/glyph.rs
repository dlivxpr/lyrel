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
//! This is the first slice: a default-layout, one-cell `glyf` registration.
//! The full sizing/placement model, width-two spans and retained-frame
//! versions belong to the follow-on tickets, so a registration using any
//! non-default layout is retained by the daemon but not painted here yet.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use gpui_kit::{
    App, Bounds, ContentMask, Font, Hsla, PathBuilder, Pixels, SharedString, TextRun, Window,
    point, px,
};
use parking_lot::RwLock;
use rio_graphics::glyph::glyf_decode::{Outline, PathCmd};
use rio_graphics::glyph_layout::GlyphLayout;

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
}

impl GlyphApply<'_> {
    /// Apply one accepted `glyf` payload. Returns whether the daemon's
    /// authority was present; a missing authority records the refusal instead.
    pub(crate) fn register(&self, cp: u32, glyf: &[u8], upm: u16, width: u8, layout: GlyphLayout) {
        if !self.authority.load(Ordering::Relaxed) {
            self.denied.store(true, Ordering::Relaxed);
            return;
        }
        if let Ok(outline) = rio_graphics::glyph::glyf_decode::decode(glyf) {
            self.store.apply(cp, outline, upm, width, layout);
        }
    }
}

/// Where a default-layout outline sits inside one cell.
struct Placement {
    scale: f32,
    offset_x: f32,
    offset_y: f32,
}

/// Default layout: implicit extent, `contain`, `align=center,center`, no
/// padding. The outline's own control box is the extent, and `contain` is
/// capped by the em so a period stays smaller than an `M`.
fn placement(outline: &Outline, upm: u16, cell_w: f32, cell_h: f32) -> Option<Placement> {
    let bbox_w = (outline.x_max - outline.x_min) as f32;
    let bbox_h = (outline.y_max - outline.y_min) as f32;
    let mut scale = match (bbox_w > 0., bbox_h > 0.) {
        (true, true) => (cell_w / bbox_w).min(cell_h / bbox_h),
        (true, false) => cell_w / bbox_w,
        (false, true) => cell_h / bbox_h,
        (false, false) => return None,
    };
    if upm > 0 {
        scale = scale.min(cell_w.min(cell_h) / f32::from(upm));
    }
    if !scale.is_finite() || scale <= 0. {
        return None;
    }
    Some(Placement {
        scale,
        offset_x: (cell_w - bbox_w * scale) / 2. - outline.x_min as f32 * scale,
        offset_y: (cell_h - bbox_h * scale) / 2.,
    })
}

/// Paint one registered glyph into `bounds` with the cell's effective
/// foreground. Nothing is painted for a non-default layout or width, which
/// this slice does not claim to place.
pub(crate) fn paint(window: &mut Window, bounds: Bounds<Pixels>, glyph: &GlyphView, fg: Hsla) {
    if glyph.width != 1 || glyph.layout != GlyphLayout::default() {
        return;
    }
    let Some(place) = placement(
        &glyph.outline,
        glyph.upm,
        bounds.size.width.as_f32(),
        bounds.size.height.as_f32(),
    ) else {
        return;
    };
    let origin_x = bounds.origin.x.as_f32() + place.offset_x;
    let origin_y = bounds.origin.y.as_f32() + place.offset_y;
    let at = |x: f32, y: f32| point(px(origin_x + x), px(origin_y + y));
    let mut builder = PathBuilder::fill();
    for command in glyph
        .outline
        .walk(glyph.upm, place.scale * f32::from(glyph.upm))
    {
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
    window.with_content_mask(Some(ContentMask { bounds }), |window| {
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

    #[test]
    fn a_square_fills_the_shorter_cell_axis_and_is_centred() {
        let place = placement(&outline(0, 0, 1000, 1000), 1000, 10., 20.).unwrap();
        assert_eq!(place.scale, 0.01);
        assert_eq!(place.offset_x, 0.);
        assert_eq!(place.offset_y, 5.);
    }

    #[test]
    fn a_wide_outline_is_advance_bound_and_keeps_its_aspect() {
        let place = placement(&outline(0, 0, 2000, 500), 1000, 10., 20.).unwrap();
        assert_eq!(place.scale, 0.005);
        assert_eq!(place.offset_x, 0.);
        assert_eq!(place.offset_y, 8.75);
    }

    #[test]
    fn implicit_contain_is_capped_by_the_em() {
        // A tiny outline would otherwise scale to the whole cell; the em cap
        // keeps it at one em instead.
        let place = placement(&outline(0, 0, 100, 100), 1000, 10., 20.).unwrap();
        assert_eq!(place.scale, 0.01);
        assert_eq!(place.offset_x, 4.5);
        assert_eq!(place.offset_y, 9.5);
    }

    #[test]
    fn a_negative_origin_keeps_the_outline_centred() {
        let place = placement(&outline(-500, 0, 500, 1000), 1000, 10., 20.).unwrap();
        assert_eq!(place.scale, 0.01);
        assert_eq!(place.offset_x, 5.);
        assert_eq!(place.offset_y, 5.);
    }

    #[test]
    fn an_empty_outline_has_no_placement() {
        assert!(placement(&outline(0, 0, 0, 0), 1000, 10., 20.).is_none());
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
