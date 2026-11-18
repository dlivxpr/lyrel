//! Text layer: pull a page's text and per-glyph rectangles by running a
//! non-rasterizing hayro interpret pass.
//!
//! Adapted from the pinned Zorite `gpui-pdf` text layer. Kept: glyph collection,
//! normalized geometry, whitespace-insensitive quote matching, selection and
//! line grouping. Repaired here: byte-safe occurrence stepping for multibyte
//! text, clip-stack filtering, deterministic single-column reading order, and
//! Type3 advance/commitment geometry (the old 1-point box inserted spaces
//! between adjacent CJK glyphs).

use hayro::hayro_interpret::font::{Glyph, GlyphRun};
use hayro::hayro_interpret::hayro_cmap::BfString;
use hayro::hayro_interpret::{
    BlendMode, ClipPath, Context, Device, DrawMode, DrawProps, Image, ImageDrawProps,
    InterpreterCache, InterpreterSettings, SoftMask, TransformExt, interpret_page,
};
use hayro::hayro_syntax::Pdf;
use hayro::kurbo::{Affine, BezPath, Point, Rect, Shape};

use super::NormRect;

/// A point in normalized page coordinates (0..1, top-left origin).
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct NormPoint {
    pub x: f32,
    pub y: f32,
}

impl NormPoint {
    pub fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
}

/// A drag selection: the selected text as one quote, the occurrence that
/// re-locates it, and the rectangles to draw.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct Selection {
    pub quote: String,
    pub occurrence: usize,
    pub rects: Vec<NormRect>,
}

impl Selection {
    pub fn new(quote: String, occurrence: usize, rects: Vec<NormRect>) -> Self {
        Self {
            quote,
            occurrence,
            rects,
        }
    }
}

/// Compatible name for consumers that distinguish a PDF text selection from
/// other selections.
pub type TextSelection = Selection;

/// One extracted glyph cluster: its text (usually one char; a ligature may be
/// several) and its normalized bounding rect on the page.
struct Run {
    text: String,
    rect: NormRect,
}

/// Raw glyph evidence before reading order and Type3 advance settle the boxes.
struct RawRun {
    text: String,
    origin: Point,
    bbox: Option<Rect>,
    em: f64,
}

impl RawRun {
    fn center_y(&self) -> f64 {
        self.bbox
            .map(|bbox| (bbox.y0 + bbox.y1) / 2.0)
            .unwrap_or(self.origin.y - self.em * 0.3)
    }

    fn height(&self) -> f64 {
        self.bbox
            .map(|bbox| bbox.height())
            .unwrap_or(self.em)
            .max(self.em * 0.2)
    }

    fn x(&self) -> f64 {
        self.bbox.map(|bbox| bbox.x0).unwrap_or(self.origin.x)
    }
}

fn intersect(a: Rect, b: Rect) -> Rect {
    Rect::new(
        a.x0.max(b.x0),
        a.y0.max(b.y0),
        a.x1.min(b.x1),
        a.y1.min(b.y1),
    )
}

fn empty(rect: Rect) -> bool {
    rect.x1 <= rect.x0 || rect.y1 <= rect.y0
}

/// Whether a glyph box survives the active clip intersection. Partially
/// clipped glyphs stay selectable; fully clipped ones do not.
fn visible(clips: &[Rect], bbox: Rect) -> bool {
    match clips.last() {
        Some(clip) => !empty(intersect(*clip, bbox)),
        None => true,
    }
}

/// Records glyph text and geometry; everything else is ignored.
struct Collector {
    runs: Vec<RawRun>,
    /// Cumulative device-space clip intersection; the last entry is effective.
    clips: Vec<Rect>,
}

impl<'a> Device<'a> for Collector {
    fn draw_path(&mut self, _: &BezPath, _: DrawProps<'a>, _: &DrawMode) {}

    fn push_clip_path(&mut self, clip_path: &ClipPath) {
        let bbox = clip_path.path.bounding_box();
        let cumulative = match self.clips.last() {
            Some(active) => intersect(*active, bbox),
            None => bbox,
        };
        self.clips.push(cumulative);
    }

    fn push_transparency_group(&mut self, _: f32, _: Option<SoftMask<'a>>, _: BlendMode) {}

    fn draw_image(&mut self, _: Image<'a, '_>, _: ImageDrawProps<'a>) {}

    fn pop_clip(&mut self) {
        self.clips.pop();
    }

    fn pop_transparency_group(&mut self) {}

    fn draw_glyph_run(
        &mut self,
        run: &GlyphRun<'_, 'a>,
        props: DrawProps<'a>,
        // Invisible glyphs are still searchable text (OCR layers, Tr 3). Clip
        // paths decide what is actually off-page.
        _draw_mode: &DrawMode,
    ) {
        for glyph in run.glyphs() {
            self.glyph(glyph, props.transform * glyph.transform());
        }
    }
}

impl Collector {
    fn glyph(&mut self, glyph: &Glyph<'_>, matrix: Affine) {
        let text = match glyph.as_unicode() {
            Some(BfString::Char(ch)) => ch.to_string(),
            Some(BfString::String(text)) => text,
            None => return,
        };
        let origin = matrix * Point::new(0., 0.);
        let em = {
            let top = matrix * Point::new(0.0, 1000.0);
            (top - origin).hypot()
        }
        .max(0.001);
        let (text, bbox, bbox_rect) = if text.chars().all(char::is_whitespace) {
            // The PDF's own space glyph marks a word boundary; give it a thin
            // box at the pen origin so it is preserved without a huge rect.
            let rect = Rect::new(origin.x, origin.y, origin.x + 1.0, origin.y + 1.0);
            (" ".to_string(), Some(rect), Some(rect))
        } else {
            match glyph {
                Glyph::Outline(outline) => {
                    let glyph_box = outline.outline().bounding_box();
                    let corners = [
                        matrix * Point::new(glyph_box.x0, glyph_box.y0),
                        matrix * Point::new(glyph_box.x1, glyph_box.y0),
                        matrix * Point::new(glyph_box.x1, glyph_box.y1),
                        matrix * Point::new(glyph_box.x0, glyph_box.y1),
                    ];
                    let mut min = corners[0];
                    let mut max = corners[0];
                    for corner in &corners[1..] {
                        min.x = min.x.min(corner.x);
                        min.y = min.y.min(corner.y);
                        max.x = max.x.max(corner.x);
                        max.y = max.y.max(corner.y);
                    }
                    let rect = Rect::new(min.x, min.y, max.x, max.y);
                    (text, Some(rect), Some(rect))
                }
                // Type3 glyphs expose no outline here. Record the pen and em;
                // the advance to the next glyph becomes the box in `finish`.
                Glyph::Type3(_) => (text, None, None),
            }
        };
        let probe = bbox_rect.unwrap_or_else(|| {
            Rect::new(
                origin.x,
                origin.y - em * 0.8,
                origin.x + em * 0.5,
                origin.y + em * 0.2,
            )
        });
        if !visible(&self.clips, probe) {
            return;
        }
        self.runs.push(RawRun {
            text,
            origin,
            bbox,
            em,
        });
    }
}

/// Extract one page's text layer. Returns `None` when the page does not exist.
pub fn extract_page_text(doc: &Pdf, index: usize) -> Option<PageText> {
    let page = doc.pages().get(index)?;
    let (page_w, page_h) = page.base_dimensions();
    let crop = page.intersected_crop_box();
    // Establish text order before page rotation. The interpreter's clipping
    // and glyphs use the same unrotated, top-left coordinate system.
    let initial = Affine::new([1., 0., 0., -1., -crop.x0, f64::from(page_h) + crop.y0]);
    let cache = InterpreterCache::new();
    let mut ctx = Context::new(
        initial,
        Rect::new(0.0, 0.0, f64::from(page_w), f64::from(page_h)),
        &cache,
        page.xref(),
        InterpreterSettings::default(),
    );
    let mut collector = Collector {
        runs: Vec::new(),
        clips: Vec::new(),
    };
    interpret_page(page, &mut ctx, &mut collector);
    let (display_w, display_h) = page.render_dimensions();
    let mut text = PageText::new(finish(collector.runs, page_w, page_h));
    text.display = Affine::scale_non_uniform(1. / f64::from(display_w), 1. / f64::from(display_h))
        * page.initial_transform(true).to_kurbo()
        * initial.inverse()
        * Affine::scale_non_uniform(f64::from(page_w), f64::from(page_h));
    Some(text)
}

/// Single-column reading order: cluster glyphs into baselines, then sort each
/// line from left to right and the lines from top to bottom. This replaces raw
/// content-stream draw order, which frequently interleaves words on one line.
/// Two-column layouts remain content-order within their baselines and are not
/// claimed as fully column-aware.
fn finish(mut raw: Vec<RawRun>, page_w: f32, page_h: f32) -> Vec<Run> {
    let mut ids: Vec<usize> = (0..raw.len()).collect();
    ids.sort_by(|&a, &b| {
        raw[a]
            .center_y()
            .partial_cmp(&raw[b].center_y())
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(
                raw[a]
                    .x()
                    .partial_cmp(&raw[b].x())
                    .unwrap_or(std::cmp::Ordering::Equal),
            )
    });
    let mut lines: Vec<(f64, f64, usize)> = Vec::new();
    let mut line_of = vec![0usize; raw.len()];
    for &id in &ids {
        let run = &raw[id];
        let center = run.center_y();
        let tolerance = run.height().max(run.em * 0.5) * 0.6;
        let line = match lines.last_mut() {
            Some((line_center, line_height, line))
                if (center - *line_center).abs() <= tolerance.max(*line_height * 0.6) =>
            {
                *line_center = (*line_center + center) / 2.0;
                *line_height = (*line_height).max(run.height());
                *line
            }
            _ => {
                let line = lines.len();
                lines.push((center, run.height(), line));
                line
            }
        };
        line_of[id] = line;
    }
    let mut ordered: Vec<usize> = (0..raw.len()).collect();
    ordered.sort_by(|&a, &b| {
        line_of[a].cmp(&line_of[b]).then(
            raw[a]
                .x()
                .partial_cmp(&raw[b].x())
                .unwrap_or(std::cmp::Ordering::Equal),
        )
    });

    let mut runs = Vec::with_capacity(ordered.len());
    for (position, &id) in ordered.iter().enumerate() {
        let run = &raw[id];
        let next = ordered.get(position + 1).map(|&next| &raw[next]);
        let rect = match run.bbox {
            Some(bbox) => bbox,
            None => {
                let same_line = next.is_some_and(|_| line_of[ordered[position + 1]] == line_of[id]);
                let advance = match next {
                    Some(next) if same_line && next.origin.x > run.origin.x => {
                        next.origin.x - run.origin.x
                    }
                    _ => run.em * 0.5,
                };
                Rect::new(
                    run.origin.x,
                    run.origin.y - run.em * 0.8,
                    run.origin.x + advance.max(run.em * 0.2),
                    run.origin.y + run.em * 0.2,
                )
            }
        };
        let rect = NormRect {
            x: (rect.x0 as f32 / page_w).clamp(0.0, 1.0),
            y: (rect.y0 as f32 / page_h).clamp(0.0, 1.0),
            w: ((rect.x1 - rect.x0) as f32 / page_w).clamp(0.0, 1.0),
            h: ((rect.y1 - rect.y0) as f32 / page_h).clamp(0.0, 1.0),
        };
        runs.push(Run {
            text: run.text.clone(),
            rect,
        });
    }
    // Drop the unused mutable buffer's aliases explicitly: the ordered pass
    // copies text, so the raw evidence can be released before the page text.
    raw.clear();
    runs
}

/// A page's extracted text in reading order, plus a whitespace-stripped,
/// lowercased index for robust matching.
pub struct PageText {
    runs: Vec<Run>,
    letters: String,
    /// `owner[byte]` is the run index that produced that byte of `letters`.
    owner: Vec<usize>,
    /// Normalized reading coordinates to the page's rotated display.
    display: Affine,
}

impl PageText {
    fn new(runs: Vec<Run>) -> Self {
        let mut letters = String::new();
        let mut owner = Vec::new();
        for (run_index, run) in runs.iter().enumerate() {
            for ch in run.text.chars() {
                if ch.is_whitespace() {
                    continue;
                }
                for lowered in ch.to_lowercase() {
                    let before = letters.len();
                    letters.push(lowered);
                    for _ in before..letters.len() {
                        owner.push(run_index);
                    }
                }
            }
        }
        Self {
            runs,
            letters,
            owner,
            display: Affine::IDENTITY,
        }
    }

    /// Whether the page has any extractable text. A pure scan has none; that is
    /// not evidence that the scanned words are absent.
    pub fn is_empty(&self) -> bool {
        self.runs.is_empty()
    }

    fn search_key(text: &str) -> String {
        text.chars()
            .filter(|ch| !ch.is_whitespace())
            .flat_map(char::to_lowercase)
            .collect()
    }

    /// A readable reconstruction in reading order, with newlines for baseline
    /// changes and spaces for real horizontal gaps.
    pub fn text(&self) -> String {
        Self::join_runs(&self.runs, true)
    }

    fn join_runs(runs: &[Run], newlines: bool) -> String {
        let mut out = String::new();
        let mut previous: Option<&Run> = None;
        for run in runs {
            if let Some(previous) = previous
                && run.text != " "
                && previous.text != " "
            {
                let line_height = run.rect.h.max(previous.rect.h).max(0.001);
                let new_line =
                    (run.rect.center_y() - previous.rect.center_y()).abs() > line_height * 0.6;
                let gap = run.rect.x - previous.rect.right() > line_height * 0.6;
                // CJK scripts do not separate every character with a space;
                // an approximate glyph box must not invent one.
                let cjk_pair = previous.text.chars().next_back().is_some_and(is_cjk)
                    && run.text.chars().next().is_some_and(is_cjk);
                if newlines {
                    if new_line {
                        out.push('\n');
                    } else if gap && !cjk_pair && !out.ends_with(char::is_whitespace) {
                        out.push(' ');
                    }
                } else if new_line || (gap && !cjk_pair) {
                    out.push(' ');
                }
            }
            out.push_str(&run.text);
            previous = Some(run);
        }
        out
    }

    /// Locate the `occurrence`-th (0-based) case- and whitespace-insensitive
    /// match and return one normalized rect per line it spans.
    pub fn locate(&self, needle: &str, occurrence: usize) -> Vec<NormRect> {
        let key = Self::search_key(needle);
        if key.is_empty() {
            return Vec::new();
        }
        let mut from = 0;
        let mut hit = None;
        for _ in 0..=occurrence {
            match self.letters[from..].find(&key) {
                Some(relative) => {
                    let start = from + relative;
                    hit = Some(start);
                    // Advancing by one BYTE can land inside a multibyte char
                    // and panic on the next slice. Advance to the next
                    // character boundary, preserving overlapping matches.
                    let mut next = start + 1;
                    while next < self.letters.len() && !self.letters.is_char_boundary(next) {
                        next += 1;
                    }
                    from = next;
                }
                None => {
                    hit = None;
                    break;
                }
            }
        }
        let Some(start) = hit else {
            return Vec::new();
        };
        let end = start + key.len();
        let mut run_ids: Vec<usize> = self.owner[start..end].to_vec();
        run_ids.dedup();
        self.group_lines(&run_ids)
    }

    /// Every non-overlapping match, each as one rect per line, in reading order.
    pub fn find_matches(&self, needle: &str) -> Vec<Vec<NormRect>> {
        let key = Self::search_key(needle);
        if key.is_empty() {
            return Vec::new();
        }
        let mut out = Vec::new();
        let mut from = 0;
        while let Some(relative) = self.letters[from..].find(&key) {
            let start = from + relative;
            let end = start + key.len();
            let mut run_ids: Vec<usize> = self.owner[start..end].to_vec();
            run_ids.dedup();
            out.push(self.group_lines(&run_ids));
            from = end;
        }
        out
    }

    fn group_lines(&self, run_ids: &[usize]) -> Vec<NormRect> {
        let mut rects: Vec<NormRect> = run_ids.iter().map(|&index| self.runs[index].rect).collect();
        rects.sort_by(|a, b| {
            a.y.partial_cmp(&b.y)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.x.partial_cmp(&b.x).unwrap_or(std::cmp::Ordering::Equal))
        });
        let mut lines: Vec<NormRect> = Vec::new();
        for rect in rects {
            if let Some(last) = lines.last_mut() {
                let line_height = last.h.max(rect.h).max(0.001);
                if (rect.center_y() - last.center_y()).abs() <= line_height * 0.6 {
                    let x = last.x.min(rect.x);
                    let y = last.y.min(rect.y);
                    let right = last.right().max(rect.right());
                    let bottom = last.bottom().max(rect.bottom());
                    *last = NormRect {
                        x,
                        y,
                        w: right - x,
                        h: bottom - y,
                    };
                    continue;
                }
            }
            lines.push(rect);
        }
        for rect in &mut lines {
            // Page rotation is a quarter-turn, so opposite rectangle corners
            // still determine its exact axis-aligned display bounds.
            let first = self.display * Point::new(f64::from(rect.x), f64::from(rect.y));
            let last = self.display * Point::new(f64::from(rect.right()), f64::from(rect.bottom()));
            *rect = NormRect {
                x: first.x.min(last.x) as f32,
                y: first.y.min(last.y) as f32,
                w: (last.x - first.x).abs() as f32,
                h: (last.y - first.y).abs() as f32,
            };
        }
        lines
    }

    /// Select the reading line before its glyph: a descender must not make a
    /// distant `g` nearer than the actual line-end character. Match the line
    /// grouping used for text reconstruction, before page rotation.
    fn nearest_run(&self, point: NormPoint) -> Option<usize> {
        let point = self.display.inverse() * Point::new(f64::from(point.x), f64::from(point.y));
        let point = NormPoint {
            x: point.x as f32,
            y: point.y as f32,
        };
        let mut nearest = None;
        let mut nearest_distance = (f32::INFINITY, f32::INFINITY);
        let mut start = 0;
        for line in self.runs.chunk_by(|a, b| {
            let height = a.rect.h.max(b.rect.h).max(0.001);
            (a.rect.center_y() - b.rect.center_y()).abs() <= height * 0.6
        }) {
            let mut top = f32::INFINITY;
            let mut bottom = f32::NEG_INFINITY;
            let mut horizontal = f32::INFINITY;
            let mut line_nearest = start;
            for (index, run) in line.iter().enumerate() {
                top = top.min(run.rect.y);
                bottom = bottom.max(run.rect.bottom());
                let distance = (run.rect.x - point.x)
                    .max(point.x - run.rect.right())
                    .max(0.);
                if distance < horizontal {
                    horizontal = distance;
                    line_nearest = start + index;
                }
            }
            let vertical = (top - point.y).max(point.y - bottom).max(0.);
            if (vertical, horizontal) < nearest_distance {
                nearest_distance = (vertical, horizontal);
                nearest = Some(line_nearest);
            }
            start += line.len();
        }
        nearest
    }

    fn runs_text(&self, low: usize, high: usize) -> String {
        let joined = Self::join_runs(&self.runs[low..=high], false);
        joined.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    fn occurrence_at(&self, low: usize, quote: &str) -> usize {
        let key = Self::search_key(quote);
        if key.is_empty() {
            return 0;
        }
        let start = self
            .owner
            .iter()
            .position(|&index| index == low)
            .unwrap_or(0);
        self.letters
            .get(..start)
            .map_or(0, |prefix| prefix.matches(&key).count())
    }

    /// The text a drag from `from` to `to` covers, for copying.
    pub fn selected_text(&self, from: NormPoint, to: NormPoint) -> Option<String> {
        let first = self.nearest_run(from)?;
        let second = self.nearest_run(to)?;
        let raw = Self::join_runs(&self.runs[first.min(second)..=first.max(second)], true);
        let text = raw
            .lines()
            .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
            .collect::<Vec<_>>()
            .join("\n");
        (!text.trim().is_empty()).then_some(text)
    }

    /// Resolve a drag into a quote, its occurrence and highlight rectangles.
    pub fn select(&self, from: NormPoint, to: NormPoint) -> Option<Selection> {
        let first = self.nearest_run(from)?;
        let second = self.nearest_run(to)?;
        let (low, high) = (first.min(second), first.max(second));
        let quote = self.runs_text(low, high);
        if quote.trim().is_empty() {
            return None;
        }
        let occurrence = self.occurrence_at(low, &quote);
        let ids: Vec<usize> = (low..=high).collect();
        let rects = self.group_lines(&ids);
        Some(Selection {
            quote,
            occurrence,
            rects,
        })
    }
}

fn is_cjk(ch: char) -> bool {
    matches!(
        ch as u32,
        0x2E80..=0x2FFF
            | 0x3040..=0x30FF
            | 0x3100..=0x312F
            | 0x3400..=0x4DBF
            | 0x4E00..=0x9FFF
            | 0xA960..=0xA97F
            | 0xAC00..=0xD7AF
            | 0xF900..=0xFAFF
            | 0x20000..=0x2FA1F
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(text: &str, x: f32, y: f32, w: f32, h: f32) -> Run {
        Run {
            text: text.to_string(),
            rect: NormRect { x, y, w, h },
        }
    }

    fn sample() -> PageText {
        PageText::new(vec![
            run("Hello", 0.10, 0.10, 0.10, 0.02),
            run("World", 0.22, 0.10, 0.10, 0.02),
            run("second", 0.10, 0.14, 0.12, 0.02),
        ])
    }

    #[test]
    fn locate_ignores_case_and_whitespace() {
        let rects = sample().locate("hello world", 0);
        assert_eq!(rects.len(), 1);
        assert!((rects[0].x - 0.10).abs() < 1e-4);
        assert!((rects[0].right() - 0.32).abs() < 1e-4);
    }

    #[test]
    fn locate_multibyte_occurrence_does_not_panic() {
        let text = PageText::new(vec![
            run("中文", 0.10, 0.10, 0.10, 0.02),
            run("中文", 0.10, 0.20, 0.10, 0.02),
        ]);
        assert_eq!(text.locate("中文", 0)[0].y, 0.10);
        assert_eq!(text.locate("中文", 1)[0].y, 0.20);
        assert!(text.locate("中文", 2).is_empty());
        assert_eq!(text.find_matches("中文").len(), 2);
    }

    #[test]
    fn cjk_adjacent_glyphs_keep_no_invented_space() {
        let mut runs = Vec::new();
        for (index, ch) in ["中", "文", "中", "文"].into_iter().enumerate() {
            runs.push(run(ch, 0.10 + index as f32 * 0.05, 0.10, 0.02, 0.02));
        }
        let text = PageText::new(runs);
        assert_eq!(text.text(), "中文中文");
    }

    #[test]
    fn multi_line_match_yields_one_rect_per_line() {
        let rects = sample().locate("HelloWorldsecond", 0);
        assert_eq!(rects.len(), 2);
        assert!(rects[0].y < rects[1].y);
    }

    #[test]
    fn selected_text_keeps_line_breaks() {
        let text = sample();
        assert_eq!(
            text.selected_text(
                NormPoint { x: 0.11, y: 0.11 },
                NormPoint { x: 0.2, y: 0.15 }
            )
            .as_deref(),
            Some("Hello World\nsecond")
        );
    }

    #[test]
    fn clip_intersection_hides_fully_clipped_glyphs() {
        let clip = Rect::new(0.0, 0.0, 100.0, 100.0);
        assert!(visible(&[clip], Rect::new(90.0, 90.0, 120.0, 120.0)));
        assert!(!visible(&[clip], Rect::new(110.0, 10.0, 130.0, 20.0)));
        assert!(!visible(
            &[clip, Rect::new(0.0, 0.0, 50.0, 50.0)],
            Rect::new(60.0, 60.0, 70.0, 70.0)
        ));
    }

    #[test]
    fn reading_order_sorts_a_line_by_x_then_the_next_line() {
        let raw = vec![
            RawRun {
                text: "second".to_string(),
                origin: Point::new(10.0, 120.0),
                bbox: Some(Rect::new(10.0, 110.0, 60.0, 130.0)),
                em: 20.0,
            },
            RawRun {
                text: "World".to_string(),
                origin: Point::new(100.0, 20.0),
                bbox: Some(Rect::new(100.0, 10.0, 150.0, 30.0)),
                em: 20.0,
            },
            RawRun {
                text: "Hello".to_string(),
                origin: Point::new(10.0, 20.0),
                bbox: Some(Rect::new(10.0, 10.0, 80.0, 30.0)),
                em: 20.0,
            },
        ];
        let runs = finish(raw, 200.0, 200.0);
        let text = PageText::new(runs).text();
        assert!(text.starts_with("Hello World"), "got {text:?}");
        assert!(text.ends_with("second"), "got {text:?}");
    }
}
