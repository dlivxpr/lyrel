//! Glyph shaping, font fallback, ink fitting and painting for one borrowed frame.

use std::cell::RefCell;
use std::collections::HashMap;

use gpui_kit::{
    App, Bounds, ContentMask, Font, FontStyle, FontWeight, Hsla, Pixels, Point, SharedString,
    TextAlign, TextRun, Window, fill, point, px, size,
};

use super::{CellGeom, GlyphStyle, RenderCell, UnderlineKind, is_blank};
use powerline::{PowerlineShape, powerline_path, powerline_solid_edge};
use segment::{RowSeg, char_string, segment_row};

mod powerline;
mod segment;

fn build_font(base: &Font, bold: bool, italic: bool) -> Font {
    let mut f = base.clone();
    f.weight = if bold {
        FontWeight::BOLD
    } else {
        FontWeight::NORMAL
    };
    f.style = if italic {
        FontStyle::Italic
    } else {
        FontStyle::Normal
    };
    if f.features.tag_value_list().is_empty() {
        f.features = ligatures_off();
    }
    f
}

/// The feature set that actually turns ligatures off in a terminal grid.
///
/// gpui's own `FontFeatures::disable_ligatures()` emits `calt: 0` and nothing
/// else. `calt` is only the contextual-alternate feature a programming face
/// drives `=>` and `!=` from; the `fi`/`ffl`/`ffi` ligatures live in `liga` and
/// `clig`, which that call never mentions — so they were left at the shaper's
/// default, and CoreText, DirectWrite and rustybuzz all default them on. A
/// terminal cannot have them: a ligature is one glyph where the grid budgeted a
/// cell per character, so the row drifts.
///
/// Naming all three is therefore the whole fix, and it is not platform
/// specific. Windows is the sharpest case only because gpui's DirectWrite
/// backend always attaches an `IDWriteTypography` to the run: an empty feature
/// list leaves that object empty, which suppresses DirectWrite's own defaults,
/// while any non-empty list has `liga: 1`/`clig: 1` appended to it
/// (`gpui_windows/src/direct_write.rs`, `apply_font_features`). Asking for
/// `calt: 0` there bought ligatures that asking for nothing would not have.
///
/// `rlig` is deliberately left alone: it carries the required ligatures a
/// script cannot be written without.
fn ligatures_off() -> gpui_kit::FontFeatures {
    gpui_kit::FontFeatures(std::sync::Arc::new(vec![
        ("calt".to_string(), 0),
        ("liga".to_string(), 0),
        ("clig".to_string(), 0),
    ]))
}

thread_local! {
    /// Measured ink bounds and the font size they were measured at.
    static INK_BOUNDS: RefCell<(Pixels, HashMap<(gpui_kit::FontId, char), Option<Bounds<Pixels>>>)> =
        RefCell::new((px(0.), HashMap::new()));
}

fn native_cell_residue(style: &GlyphStyle) -> Option<char> {
    // Special underlines are painted directly from the cell buffer, so a
    // shaped blank is only needed for decorations GPUI owns.
    (style.strikeout
        || matches!(
            style.underline,
            UnderlineKind::Single | UnderlineKind::Curly
        )
        || style.link_hover)
        .then_some(' ')
}

/// How much room a segment's glyph gets before it is made to shrink.
///
/// No monospace face is wide enough for an emoji at the sizes people read at.
/// Apple Color Emoji is a bitmap face whose strikes are quantised, so its ink
/// is widest relative to the em exactly where it hurts: 1.25em from 12 to 16px,
/// tapering to 1.0em by 24px. Two cells only cover 1.25em from 0.63em advance
/// up, and the common faces sit at 0.60em (Menlo, SF Mono) or 0.50em (Ubuntu
/// Mono). Every terminal therefore has to choose between shrinking the glyph
/// and letting it spill. This follows WezTerm, whose answer shows the glyph
/// whole most often: a quarter cell of slack always, and a whole extra cell
/// when the neighbouring cell is blank and has nothing to lose.
fn seg_budget(solo: bool, measured: bool, cells: usize, room: bool, cell_width: Pixels) -> Pixels {
    if solo {
        // Single-cell glyphs are allowed to lean into the next cell — that is
        // what keeps the pile of symbols that look fine today from shrinking —
        // but only while that cell is empty. A Nerd Font icon pulled from a
        // fallback face inks well past a narrow primary's cell (1.6 cells is
        // typical), and where the next cell has a glyph of its own the lean is
        // not a lean, it is an overlap: the neighbour is painted afterwards
        // and lands on top of the overshoot. Hand those their own cell and let
        // `fit_scale` bring them down into it, the way kitty and ghostty do.
        //
        // Only where the ink was measured, though. This number is the clip as
        // well as the threshold to shrink at, so narrowing it for a segment
        // nothing could measure would cut the glyph instead of scaling it —
        // see `ink_covers_segment`.
        if room || !measured {
            cell_width * 2.
        } else {
            cell_width
        }
    } else if room {
        cell_width * (cells as f32 + 1.)
    } else {
        cell_width * (cells as f32 + 0.25)
    }
}

/// How much to shrink a segment so its glyph stops inside its budget.
///
/// Never more than 1. A fallback icon is not held to this at all:
/// [`icon_fit`] sizes it, up or down.
fn fit_scale(ink: Pixels, budget: Pixels) -> f32 {
    if ink <= budget || ink <= px(0.) {
        1.
    } else {
        budget.as_f32() / ink.as_f32()
    }
}

/// Whether the platform's `typographic_bounds` is the glyph's ink.
///
/// CoreText and DirectWrite answer with the ink box. cosmic-text answers with
/// the advance box pinned to the baseline, which says nothing about where an
/// icon's ink sits; sizing and centring on that would misplace it, so the
/// Linux build keeps the shrink-only rule for icons too.
const INK_BOUNDS_ARE_INK: bool = cfg!(any(target_os = "macos", target_os = "windows"));

/// The most an icon is ever enlarged by.
///
/// The cap keeps a glyph that is small by design — a dot, a bullet — from
/// being blown up to a cell-sized blob.
const MAX_ICON_GROWTH: f32 = 2.;

/// Whether a character is an icon: a code point in one of Unicode's Private
/// Use Areas, where Nerd Fonts, Font Awesome, Codicons and Material Design
/// icons all live.
///
/// Nothing in these ranges is text, so sizing one to its cells cannot change
/// how prose reads — the reason the rule is keyed on the code point and not
/// on the face: a CJK or emoji fallback keeps its own metrics.
///
/// The Powerline separators (`U+E0B0`–`U+E0D7`) are not icons. They are
/// shapes meant to abut the cells around them, the solid ones are painted
/// natively, and the thin ones already reach the row's full height, so
/// fitting them to an em and centring them would only pull them away from
/// the segments they join.
fn is_icon(c: char) -> bool {
    matches!(
        c as u32,
        0xE000..=0xE0AF | 0xE0D8..=0xF8FF | 0xF_0000..=0xF_FFFD | 0x10_0000..=0x10_FFFD
    )
}

/// Whether the cell after an icon can lend it room, the way kitty lets a
/// Private Use Area glyph followed by a space take two cells.
///
/// Everything [`has_room_after`] lends, and one thing more: a blank that
/// paints the *same* background as the icon's own cell. That is the space
/// after every icon in a coloured prompt segment, and backgrounds are painted
/// before any glyph, so the icon lands on its own colour. Held back to one
/// cell there, a Nerd Font icon is shrunk to about two thirds of the text's
/// height, which is #866.
fn lends_to_icon(row: &[RenderCell], start: usize, cells: usize) -> bool {
    if has_room_after(row, start, cells) {
        return true;
    }
    let (Some(icon), Some(next)) = (row.get(start), row.get(start + cells)) else {
        return false;
    };
    is_blank(next)
        && !next.selected
        && icon.draw_bg
        && next.draw_bg
        && next.bg == icon.bg
        && next.match_hit == icon.match_hit
        && next.match_current == icon.match_current
        && !GlyphStyle::of(next).draws_on_blanks()
}

/// How to size an icon a fallback face supplied, and how many cells it spans.
///
/// A fallback face is drawn at the primary's font size on its own metrics, so
/// what an icon looks like beside the text depends on how the two faces'
/// cells compare. Symbols Nerd Font Mono inks a full em per icon, which is
/// 1.6 cells of a 0.6em primary; a face with a narrower cell than the
/// primary's inks less than one. Neither is the icon at the text's size.
///
/// So fit the ink, aspect ratio kept, to the cells it is given and one em of
/// height — the size the icon was drawn at in its own face, and never taller
/// than the text's line. That may shrink or grow it, growth capped at
/// [`MAX_ICON_GROWTH`] and ignored under 2%, which is not worth a reshape.
/// When `lend` says the next cell is free the icon may take it too, but only
/// if that lets it come out bigger: a narrow glyph that already fits one cell
/// keeps to it and stays beside what follows.
fn icon_fit(
    ink: Bounds<Pixels>,
    cell_width: Pixels,
    cells: usize,
    lend: bool,
    height: Pixels,
) -> (f32, usize) {
    let (w, h) = (ink.size.width.as_f32(), ink.size.height.as_f32());
    if !(w > 0. && h > 0.) {
        return (1., cells);
    }
    let fit = |cells: usize| {
        let scale = (cell_width.as_f32() * cells as f32 / w)
            .min(height.as_f32() / h)
            .min(MAX_ICON_GROWTH);
        if (1. ..=1.02).contains(&scale) {
            1.
        } else {
            scale
        }
    };
    let own = fit(cells);
    if lend && fit(cells + 1) > own {
        (fit(cells + 1), cells + 1)
    } else {
        (own, cells)
    }
}

/// Where to paint a fitted icon, relative to its cell's top-left corner, so
/// its ink sits centred in `room` — vertically always, and horizontally only
/// when `centre_x`. An icon spilling into a borrowed cell stays where its
/// face put it instead, as it would unfitted, so the gap before the text
/// after it is the one its face drew.
///
/// `ink` is the glyph's bounds at its native size (y up from the baseline);
/// `ascent` and `descent` are the line's as shaped at the fitted size, which
/// is what gpui centres the baseline in the row with.
fn icon_offset(
    ink: Bounds<Pixels>,
    scale: f32,
    room: gpui_kit::Size<Pixels>,
    centre_x: bool,
    ascent: Pixels,
    descent: Pixels,
) -> Point<Pixels> {
    let baseline = (room.height - ascent - descent) / 2. + ascent;
    let ink_top = baseline - (ink.origin.y + ink.size.height) * scale;
    let x = if centre_x {
        (room.width - ink.size.width * scale) / 2. - ink.origin.x * scale
    } else {
        // Nudged only as far as it takes to keep the ink inside the room.
        px(0.)
            .min(room.width - (ink.origin.x + ink.size.width) * scale)
            .max(-ink.origin.x * scale)
    };
    point(x, (room.height - ink.size.height * scale) / 2. - ink_top)
}

/// The ink bounds of a segment's first glyph, relative to its pen position,
/// y up from the baseline.
///
/// Advance is the wrong yardstick: Apple Color Emoji advances 1.31em but only
/// inks 1.25em, so going by advance shrinks glyphs that would have fit. Ask
/// for the glyph's own bounds instead, cached per (face, char) because the
/// lookup is a font query a screenful of CJK would otherwise repeat on every
/// cell of every frame.
fn ink_bounds(
    cx: &App,
    shaped: &gpui_kit::ShapedLine,
    text: &str,
    font_size: Pixels,
) -> Option<Bounds<Pixels>> {
    let font_id = shaped.runs.first()?.font_id;
    let c = text.chars().next()?;
    INK_BOUNDS.with(|cache| {
        let hit = {
            let mut cache = cache.borrow_mut();
            if cache.0 != font_size {
                cache.0 = font_size;
                cache.1.clear();
            }
            if cache.1.len() > 32_768 {
                cache.1.clear();
            }
            cache.1.get(&(font_id, c)).copied()
        };
        if let Some(extent) = hit {
            return extent;
        }
        let bounds = cx
            .text_system()
            .typographic_bounds(font_id, font_size, c)
            .ok();
        cache.borrow_mut().1.insert((font_id, c), bounds);
        bounds
    })
}

/// Where a shaped run stops standing over the cells it came from.
///
/// `force_width` puts the *n*th glyph at *n × cell_width*, which is the whole
/// grid only while the shaper hands back one glyph per character. A ligature
/// substitution collapses several characters into one glyph, and from there on
/// every glyph is pulled left by the cells the substitution swallowed: the
/// text after it overlaps the ligature's tail, and the caret — drawn at the
/// grid column, not at the ink — stands past the end of the run with a gap.
///
/// Takes the glyphs' byte indices, which in a run are their columns, and
/// answers with the first column that has drifted. Only a glyph that arrives
/// later than its ordinal counts: a face that *adds* glyphs cannot be helped
/// by cutting the run, and treating it as drift would cut at column zero
/// forever.
fn run_drift(indices: impl IntoIterator<Item = usize>) -> Option<usize> {
    indices
        .into_iter()
        .enumerate()
        .find_map(|(ordinal, index)| (index > ordinal).then_some(index))
}

/// One thing [`paint_run`] asks of the caller, in bytes into the run's text.
///
/// A run is ASCII, one byte per cell, so `at` is also a column offset and
/// `len` is also a width in cells.
#[derive(Debug, PartialEq)]
enum RunStep {
    /// Shape `text[at..at + len]` and answer [`run_drift`] for it.
    Drift { at: usize, len: usize },
    /// Shape and paint `text[at..at + len]` starting at column `col`.
    Paint { col: usize, at: usize, len: usize },
}

/// Walk a run in the pieces that stand over their own cells.
///
/// A run that does not drift is one `Drift` and one `Paint` of the whole
/// thing, which is every run in a monospace face. Where it drifts, the part
/// in front of the drift is painted as a piece of its own — shaped on its
/// own, which is the point: clipping the tail away would not help, because
/// the tail drifted *left*, into the columns this piece is keeping. The
/// remainder then starts over in the column the grid puts it in.
///
/// The loop is separated from the shaping so it can be tested at all: gpui's
/// test text system hands back one glyph per character, so nothing shaped
/// through it ever drifts and the interesting half would never run.
fn paint_run(start: usize, len: usize, mut step: impl FnMut(RunStep) -> Option<usize>) {
    let mut col = start;
    let mut at = 0;
    loop {
        match step(RunStep::Drift { at, len: len - at }) {
            None => {
                step(RunStep::Paint {
                    col,
                    at,
                    len: len - at,
                });
                return;
            }
            Some(drift) => {
                step(RunStep::Paint {
                    col,
                    at,
                    len: drift,
                });
                col += drift;
                at += drift;
            }
        }
    }
}

/// Shape one piece of a segment's text.
///
/// The whole of it keeps the string it arrived in; only a piece cut out of a
/// drifted run has to be copied, and that happens where a ligature forced the
/// cut. Repeat calls for the same piece within a frame are answered from
/// gpui's line-layout cache rather than shaped again.
fn shape_piece(
    window: &mut Window,
    run_buf: &mut [TextRun; 1],
    text: &SharedString,
    at: usize,
    len: usize,
    font_size: Pixels,
    force_width: Option<Pixels>,
) -> gpui_kit::ShapedLine {
    let piece = if at == 0 && len == text.len() {
        text.clone()
    } else {
        SharedString::from(text[at..at + len].to_string())
    };
    run_buf[0].len = piece.len();
    window
        .text_system()
        .shape_line(piece, font_size, run_buf, force_width)
}

/// Whether [`ink_bounds`]'s answer speaks for the whole segment.
///
/// It measures the segment's first character in the run's first face. That is
/// all of a one-character segment and only part of anything longer: a
/// cluster's combining marks can ink well to the right of the base they hang
/// off — Devanagari `\u{915}` + `\u{93E}` — and are not in the number. `None`
/// is a face that answered no bounds at all, which is no measurement either.
///
/// Neither can be scaled to fit, so neither may be held to one cell: the
/// budget doubles as the clip, and a segment that cannot shrink into a
/// narrowed one is simply cut off at it.
fn ink_covers_segment(ink: Option<Pixels>, text: &str) -> bool {
    ink.is_some() && text.chars().nth(1).is_none()
}

/// Whether the cell after a segment is free for its glyph to lean into.
///
/// A blank still owns its cell if it paints anything there: a background, a
/// selection, or a rule of its own. An underlined or hovered blank becomes a
/// `Run` of its own and is painted after the segment beside it, so lending it
/// out would drag a stroke straight across the borrowed glyph.
fn has_room_after(row: &[RenderCell], start: usize, cells: usize) -> bool {
    match row.get(start + cells) {
        None => true,
        Some(next) => {
            is_blank(next)
                && !next.draw_bg
                && !next.selected
                && !GlyphStyle::of(next).draws_on_blanks()
        }
    }
}

pub(super) fn paint_glyphs(
    window: &mut Window,
    cx: &mut App,
    geom: &CellGeom,
    buf: &[RenderCell],
    font_size: Pixels,
    base_font: &Font,
    bold_font: Option<&Font>,
    italic_font: Option<&Font>,
) {
    let faces = [
        build_font(base_font, false, false),
        build_font(bold_font.unwrap_or(base_font), true, false),
        build_font(italic_font.unwrap_or(base_font), false, true),
        build_font(bold_font.unwrap_or(base_font), true, true),
    ];

    // Resolved only once an icon asks, so a pane without one pays nothing.
    let mut primary_ids: [Option<gpui_kit::FontId>; 4] = [None; 4];

    let run_buf = &mut [TextRun {
        len: 0,
        font: base_font.clone(),
        color: Hsla::default(),
        background_color: None,
        underline: None,
        strikethrough: None,
    }];

    for row in 0..geom.rows {
        let row_base = row * geom.cols;
        let y = geom.origin.y + geom.line_height * (row as f32);

        let row_cells = &buf[row_base..row_base + geom.cols];

        for seg in segment_row(row_cells) {
            // A `Run` is one glyph per cell in the main font, which by
            // definition already fits; the rest can carry a glyph from a
            // fallback face that is wider than the cells it was handed.
            //
            // A run is also the only segment `segment_row` builds out of more
            // than one cell of plain text, and it only batches ASCII: one
            // byte, one character, one column. That is what lets `run_drift`
            // read a glyph's byte index as the column it came from.
            let run = matches!(seg, RowSeg::Run { .. });
            let (start, cells, text, force_width, solo) = match seg {
                RowSeg::Run { start, cells, text } => (
                    start,
                    cells,
                    SharedString::from(text),
                    Some(geom.cell_width),
                    false,
                ),
                RowSeg::Wide { start, cells, text } => {
                    (start, cells, text, Some(geom.cell_width * 2.), false)
                }
                RowSeg::Solo { col } => {
                    let cell = &buf[row_base + col];
                    let cell_bounds = Bounds::new(
                        point(geom.origin.x + geom.cell_width * (col as f32), y),
                        size(geom.cell_width, geom.line_height),
                    );
                    let native = if let Some(shape) = PowerlineShape::of(cell.c) {
                        let fg = GlyphStyle::of(cell).fg;
                        if let Some(edge) =
                            powerline_solid_edge(cell_bounds, shape, window.scale_factor(), fg.a)
                        {
                            window.paint_quad(fill(edge, fg));
                        }
                        let path = powerline_path(cell_bounds, shape);
                        window.paint_path(path, fg);
                        true
                    } else if let Some(ink) =
                        crate::terminal::boxdraw::glyph(cell.c, cell_bounds, window.scale_factor())
                    {
                        let fg = GlyphStyle::of(cell).fg;
                        for piece in ink {
                            match piece {
                                crate::terminal::boxdraw::Ink::Rect(r) => window.paint_quad(fill(r, fg)),
                                crate::terminal::boxdraw::Ink::Shade(r, alpha) => {
                                    let mut c = fg;
                                    c.a *= alpha;
                                    window.paint_quad(fill(r, c));
                                }
                                crate::terminal::boxdraw::Ink::Path(p) => window.paint_path(p, fg),
                            }
                        }
                        true
                    } else {
                        false
                    };
                    if !native {
                        (col, 1, char_string(cell.c), None, true)
                    } else {
                        match native_cell_residue(&GlyphStyle::of(cell)) {
                            None => continue,
                            Some(c) => (col, 1, char_string(c), None, false),
                        }
                    }
                }
                RowSeg::Cluster {
                    col,
                    cells,
                    text,
                    wide_base,
                } => (
                    col,
                    cells,
                    SharedString::from(text),
                    (cells == 2).then(|| geom.cell_width * if wide_base { 2. } else { 1. }),
                    cells == 1,
                ),
            };

            let style = GlyphStyle::of(&buf[row_base + start]);
            let face_ix = (style.bold as usize) | ((style.italic as usize) << 1);
            run_buf[0] = TextRun {
                len: text.len(),
                font: faces[face_ix].clone(),
                color: style.fg,
                background_color: None,
                underline: style.underline_style(),
                strikethrough: style.strikethrough_style(),
            };

            let x = geom.origin.x + geom.cell_width * (start as f32);

            // A run is walked in the pieces that stand over their own cells;
            // every other segment is one shaping and one paint.
            if run {
                paint_run(start, text.len(), |step| match step {
                    RunStep::Drift { at, len } => {
                        let shaped =
                            shape_piece(window, run_buf, &text, at, len, font_size, force_width);
                        run_drift(
                            shaped
                                .runs
                                .iter()
                                .flat_map(|r| r.glyphs.iter().map(|g| g.index)),
                        )
                    }
                    RunStep::Paint { col, at, len } => {
                        let shaped =
                            shape_piece(window, run_buf, &text, at, len, font_size, force_width);
                        let x = geom.origin.x + geom.cell_width * (col as f32);
                        let clip = Bounds::new(
                            point(x, y),
                            size(geom.cell_width * len as f32, geom.line_height),
                        );
                        window.with_content_mask(Some(ContentMask { bounds: clip }), |window| {
                            _ = shaped.paint(
                                point(x, y),
                                geom.line_height,
                                TextAlign::Left,
                                None,
                                window,
                                cx,
                            );
                        });
                        None
                    }
                });
                continue;
            }

            let mut shaped =
                window
                    .text_system()
                    .shape_line(text.clone(), font_size, run_buf, force_width);
            // Measured before the budget is set, because how far a solo glyph
            // may reach turns on whether its ink is known at all.
            let bounds = ink_bounds(cx, &shaped, &text, font_size);
            let ink = bounds.map(|b| b.origin.x + b.size.width);
            let measured = ink_covers_segment(ink, &text);
            let budget = seg_budget(
                solo,
                measured,
                cells,
                has_room_after(row_cells, start, cells),
                geom.cell_width,
            );
            let mut origin = point(x, y);
            let mut clip_width = budget;
            if let (Some(ink), Some(bounds)) = (ink, bounds) {
                // Only a lone icon that a fallback face supplied is fitted:
                // the primary's own icons were drawn for its cell already.
                let icon = INK_BOUNDS_ARE_INK
                    && measured
                    && text.chars().next().is_some_and(is_icon)
                    && shaped.runs.first().is_some_and(|r| {
                        r.font_id
                            != *primary_ids[face_ix].get_or_insert_with(|| {
                                window.text_system().resolve_font(&faces[face_ix])
                            })
                    });
                let (scale, room) = if icon {
                    let lend = lends_to_icon(row_cells, start, cells);
                    let height = font_size.min(geom.line_height);
                    let (scale, spans) = icon_fit(bounds, geom.cell_width, cells, lend, height);
                    let room = size(geom.cell_width * spans as f32, geom.line_height);
                    (scale, Some((room, spans == cells)))
                } else {
                    (fit_scale(ink, budget), None)
                };
                if scale != 1. {
                    shaped = window.text_system().shape_line(
                        text.clone(),
                        font_size * scale,
                        run_buf,
                        force_width,
                    );
                }
                if let Some((room, centre_x)) = room {
                    origin +=
                        icon_offset(bounds, scale, room, centre_x, shaped.ascent, shaped.descent);
                    clip_width = room.width;
                }
            }

            let clip = Bounds::new(point(x, y), size(clip_width, geom.line_height));
            window.with_content_mask(Some(ContentMask { bounds: clip }), |window| {
                _ = shaped.paint(origin, geom.line_height, TextAlign::Left, None, window, cx);
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::snapshot::CellPainter;
    use super::super::test_support::{cell, painted_term, test_colors};
    use rio_vt::config::colors::ColorRgb;
    use rio_vt::crosswords::pos::{Column, Line, Pos};

    #[test]
    fn build_font_sets_weight_and_style() {
        let base = gpui_kit::font("Courier");
        let plain = build_font(&base, false, false);
        assert_eq!(plain.weight, FontWeight::NORMAL);
        assert_eq!(plain.style, FontStyle::Normal);

        let bold_italic = build_font(&base, true, true);
        assert_eq!(bold_italic.weight, FontWeight::BOLD);
        assert_eq!(bold_italic.style, FontStyle::Italic);

        assert_eq!(bold_italic.family, base.family);
    }

    #[test]
    fn fit_scale_shrinks_what_overflows_and_never_enlarges() {
        let budget = px(15.);
        assert_eq!(fit_scale(px(19.2), budget), 15. / 19.2);
        // Whatever already fits keeps its own size, including a glyph that
        // lands exactly on the edge and a run that measured as empty. Making
        // a small glyph bigger is `icon_fit`'s rule, not this one's.
        assert_eq!(fit_scale(px(15.), budget), 1.);
        assert_eq!(fit_scale(px(12.), budget), 1.);
        assert_eq!(fit_scale(px(0.), budget), 1.);
    }

    #[test]
    fn an_emoji_only_shrinks_where_the_face_is_narrow_and_the_next_cell_is_taken() {
        // Apple Color Emoji inks 1.25em, which is 18.75px at a 15px font size.
        let ink = px(18.75);
        let scale = |advance_em: f32, room: bool| {
            let cell = px(15. * advance_em);
            fit_scale(ink, seg_budget(false, true, 2, room, cell))
        };

        // Menlo and friends: a quarter cell of slack is enough on its own.
        assert_eq!(scale(0.6, false), 1.);
        // Ubuntu Mono is narrow enough that a crowded neighbour forces a
        // shrink, but a blank one lends a whole cell and the emoji stays whole.
        assert!(scale(0.5, false) < 1.);
        assert_eq!(scale(0.5, true), 1.);
    }

    #[test]
    fn a_run_that_keeps_one_glyph_per_cell_is_painted_whole() {
        assert_eq!(run_drift([0, 1, 2, 3]), None);
        assert_eq!(run_drift([0]), None);
        assert_eq!(run_drift([0usize; 0]), None);
    }

    #[test]
    fn a_ligature_cuts_the_run_at_the_column_that_drifted() {
        // "office" through a face that ligates "ffi": the shaper answers with
        // o, ffi, c, e, and `force_width` sits them on columns 0..4 — so 'c'
        // lands two columns early, over the ligature's tail, and the row ends
        // two columns short of where the caret is drawn.
        assert_eq!(run_drift([0, 1, 4, 5]), Some(4));
        // The remainder, restarted at column 4, lines up on its own.
        assert_eq!(run_drift([0, 1]), None);
        // A two-character ligature drifts by one: "afib" -> a, fi, b.
        assert_eq!(run_drift([0, 1, 3]), Some(3));
        // And a run can drift on its very first pair: "fib" -> fi, b.
        assert_eq!(run_drift([0, 2]), Some(2));
    }

    /// Drives the loop `paint_glyphs` runs a `RowSeg::Run` through, with the
    /// shaping stubbed out. `drifts` is what the shaper is pretending to
    /// answer for each piece it is handed, in order.
    fn run_steps(start: usize, text: &str, drifts: &[Option<usize>]) -> Vec<RunStep> {
        let mut seen = Vec::new();
        let mut answers = drifts.iter().copied();
        paint_run(start, text.len(), |step| {
            let drift = matches!(step, RunStep::Drift { .. })
                .then(|| {
                    answers
                        .next()
                        .expect("asked to shape more pieces than scripted")
                })
                .flatten();
            seen.push(step);
            drift
        });
        seen
    }

    /// What the caller is told to paint, as `(column, text)`.
    fn painted<'a>(steps: &[RunStep], text: &'a str) -> Vec<(usize, &'a str)> {
        steps
            .iter()
            .filter_map(|step| match *step {
                RunStep::Paint { col, at, len } => Some((col, &text[at..at + len])),
                RunStep::Drift { .. } => None,
            })
            .collect()
    }

    #[test]
    fn a_run_that_does_not_drift_is_shaped_once_and_painted_whole() {
        let steps = run_steps(7, "hello", &[None]);
        assert_eq!(
            steps,
            [
                RunStep::Drift { at: 0, len: 5 },
                RunStep::Paint {
                    col: 7,
                    at: 0,
                    len: 5
                }
            ],
            "no drift is one shaping and one paint, at the column it started in"
        );
    }

    #[test]
    fn a_drifted_run_paints_the_head_alone_and_restarts_at_its_own_column() {
        // "office" through a face that ligates "ffi": the shaper answers with
        // o, ffi, c, e, so 'c' arrives at byte 4 as the third glyph and the
        // run has to be cut there.
        let steps = run_steps(0, "office", &[Some(4), None]);
        assert_eq!(
            painted(&steps, "office"),
            [(0, "offi"), (4, "ce")],
            "the head is a piece of its own, and the rest starts at column 4"
        );
        // The head must be *shaped* as "offi", not painted as the whole run
        // under a four-cell clip: 'c' and 'e' drifted left, into those very
        // cells, so a clip would leave them on top of the ligature.
        assert_eq!(
            steps[1],
            RunStep::Paint {
                col: 0,
                at: 0,
                len: 4
            }
        );
        // And the remainder is re-shaped on its own before it is painted, so
        // its own drift is measured from its own column.
        assert_eq!(steps[2], RunStep::Drift { at: 4, len: 2 });
    }

    #[test]
    fn a_run_that_drifts_twice_keeps_advancing_and_finishes() {
        // "affib" -> a, ffi, b cuts once; the tail "b" then stands on its own.
        assert_eq!(
            painted(&run_steps(3, "affib", &[Some(4), None]), "affib"),
            [(3, "affi"), (7, "b")]
        );
        // Two cuts in a row: every piece moves the column on by its own width
        // and the walk ends on the piece that does not drift.
        assert_eq!(
            painted(
                &run_steps(0, "offifie", &[Some(4), Some(2), None]),
                "offifie"
            ),
            [(0, "offi"), (4, "fi"), (6, "e")]
        );
    }

    #[test]
    fn a_face_that_adds_glyphs_is_left_alone() {
        // Two glyphs for one character walk ahead of their columns, not
        // behind them. Cutting there would restart the run where it already
        // is and never finish.
        assert_eq!(run_drift([0, 0, 1, 2]), None);
        assert_eq!(run_drift([0, 1, 1, 2]), None);
    }

    #[test]
    fn only_a_plain_blank_cell_counts_as_room() {
        let mut row: Vec<_> = "ab".chars().map(cell).collect();
        row.push(cell(' '));
        assert!(!has_room_after(&row, 0, 1), "a letter is not room");
        assert!(has_room_after(&row, 1, 1), "a blank is");
        assert!(has_room_after(&row, 2, 1), "so is the end of the row");

        // A blank that paints something of its own is not free real estate.
        row[2].draw_bg = true;
        assert!(!has_room_after(&row, 1, 1));
        row[2].draw_bg = false;
        row[2].selected = true;
        assert!(!has_room_after(&row, 1, 1));
        row[2].selected = false;

        // Nor is a blank that carries a rule of its own: it is painted after
        // the segment next to it, so the stroke would land on the glyph.
        row[2].underline = UnderlineKind::Single;
        assert!(!has_room_after(&row, 1, 1), "an underlined blank");
        row[2].underline = UnderlineKind::None;
        row[2].strikeout = true;
        assert!(!has_room_after(&row, 1, 1), "a struck-through blank");
        row[2].strikeout = false;
        row[2].link_hover = true;
        assert!(!has_room_after(&row, 1, 1), "a hovered link's blank");
    }

    #[test]
    fn seg_budget_frees_solo_symbols_and_lends_a_cell_only_when_one_is_free() {
        let cell = px(10.);
        assert_eq!(
            seg_budget(true, true, 1, true, cell),
            px(20.),
            "a solo glyph leans into a free cell"
        );
        assert_eq!(
            seg_budget(true, true, 1, false, cell),
            px(10.),
            "but keeps to its own once the next cell is taken"
        );
        assert_eq!(
            seg_budget(false, true, 2, false, cell),
            px(22.5),
            "a quarter cell"
        );
        assert_eq!(
            seg_budget(false, true, 2, true, cell),
            px(30.),
            "a whole cell"
        );
        assert_eq!(seg_budget(false, true, 1, false, cell), px(12.5));
    }

    #[test]
    fn a_fallback_icon_is_fitted_to_its_cell_only_when_the_next_one_is_taken() {
        // Measured on Windows: Maple Mono NF CN supplying U+F059 to a
        // 15px Cascadia Mono grid inks 13.85px across an 8.79px cell.
        let cell = px(8.789);
        let ink = px(13.845);

        let leaning = fit_scale(ink, seg_budget(true, true, 1, true, cell));
        assert_eq!(leaning, 1., "a blank neighbour still lends its cell");

        let crowded = fit_scale(ink, seg_budget(true, true, 1, false, cell));
        assert!(crowded < 1., "an occupied neighbour does not");
        assert!(
            ink * crowded <= cell,
            "and the icon has to end inside its own cell"
        );
    }

    #[test]
    fn a_solo_segment_is_held_to_its_cell_only_where_its_ink_was_measured() {
        let cell = px(10.);
        let budget = |ink, text| seg_budget(true, ink_covers_segment(ink, text), 1, false, cell);

        // One character the face answered bounds for: the whole of it was
        // measured, so `fit_scale` can bring it into one cell and the clip
        // may be drawn there.
        assert!(ink_covers_segment(Some(px(15.)), "\u{f059}"));
        assert_eq!(budget(Some(px(15.)), "\u{f059}"), px(10.));

        // A base with a combining mark hanging off it reaches paint_glyphs as
        // a one-cell `Cluster`, which counts as solo. `ink_extent` read the
        // base alone: Devanagari ka plus the aa matra inks to the right of the
        // base, and none of that overhang is in the 9px. Nothing would shrink
        // it, so a one-cell clip would cut the matra clean off.
        assert!(!ink_covers_segment(Some(px(9.)), "\u{915}\u{93E}"));
        assert_eq!(budget(Some(px(9.)), "\u{915}\u{93E}"), px(20.));

        // A face that answers no bounds at all is the same story from the
        // other side: `fit_scale` is never reached, so halving the clip only
        // clips.
        assert!(!ink_covers_segment(None, "\u{f059}"));
        assert_eq!(budget(None, "\u{f059}"), px(20.));

        // A free neighbour lends its cell either way \u2014 the measurement only
        // decides whether the lean can be withdrawn.
        for ink in [Some(px(15.)), None] {
            for text in ["\u{f059}", "\u{915}\u{93E}"] {
                assert_eq!(
                    seg_budget(true, ink_covers_segment(ink, text), 1, true, cell),
                    px(20.)
                );
            }
        }
    }

    /// The emitted feature set is the whole contract with the shaper, so pin it
    /// tag for tag rather than asking after one tag at a time.
    ///
    /// `calt: 0` alone is not "ligatures off" on any platform: it never asks
    /// for `liga`/`clig`, which every shaper defaults on. Shaping "office
    /// waffle fluffier" in Calibri through the real DirectWrite shaper gives 15
    /// glyphs for 22 characters with `calt: 0`, and 22 once all three are named
    /// zero.
    #[test]
    fn build_font_emits_the_pinned_feature_set_for_each_ligature_setting() {
        fn tags(font: &Font) -> Vec<(&str, u32)> {
            font.features
                .tag_value_list()
                .iter()
                .map(|(tag, value)| (tag.as_str(), *value))
                .collect()
        }

        // Default config, and the settings toggle switched off: both leave
        // `font_features` unset, so the terminal names its own.
        let font = build_font(&gpui_kit::font("Test"), false, false);
        assert_eq!(
            tags(&font),
            vec![("calt", 0), ("liga", 0), ("clig", 0)],
            "an unconfigured face must name every ligature feature off",
        );
        assert_eq!(font.features.is_calt_enabled(), Some(false));

        // Every face the grid paints with gets the same set, not just the plain one.
        for (bold, italic) in [(true, false), (false, true), (true, true)] {
            assert_eq!(
                tags(&build_font(&gpui_kit::font("Test"), bold, italic)),
                vec![("calt", 0), ("liga", 0), ("clig", 0)],
                "bold={bold} italic={italic}",
            );
        }

        // Explicitly on: what Settings → Appearance → Font ligatures writes.
        let mut configured = gpui_kit::font("Test");
        configured.features = crate::core::config::gpui_font_features(
            &serde_json::from_str(r#"{"calt":true,"liga":1}"#).unwrap(),
        );
        let font = build_font(&configured, false, false);
        assert_eq!(
            tags(&font),
            vec![("calt", 1), ("liga", 1)],
            "an explicit request for ligatures must survive untouched",
        );
        assert_eq!(font.features.is_calt_enabled(), Some(true));

        // Explicitly off in `config.json`: also passed through unchanged.
        let mut configured = gpui_kit::font("Test");
        configured.features = crate::core::config::gpui_font_features(
            &serde_json::from_str(r#"{"calt":0,"liga":0,"clig":0}"#).unwrap(),
        );
        assert_eq!(
            tags(&build_font(&configured, false, false)),
            vec![("calt", 0), ("liga", 0), ("clig", 0)],
        );
    }

    #[test]
    fn natively_drawn_cells_keep_decorations_owned_by_text_shaping() {
        let plain = GlyphStyle::of(&cell('│'));
        assert_eq!(
            native_cell_residue(&plain),
            None,
            "an unstyled box character has nothing left to shape"
        );

        for kind in [UnderlineKind::Single, UnderlineKind::Curly] {
            let mut c = cell('│');
            c.underline = kind;
            assert_eq!(
                native_cell_residue(&GlyphStyle::of(&c)),
                Some(' '),
                "{kind:?} underline dropped on a box-drawing cell"
            );
        }

        for ch in ['│', '─', '╭', '█', '\u{e0b0}'] {
            let mut c = cell(ch);
            c.link_hover = true;
            assert_eq!(
                native_cell_residue(&GlyphStyle::of(&c)),
                Some(' '),
                "hovered-link underline dropped on U+{:04X}",
                ch as u32
            );
        }
    }

    #[test]
    fn strikeout_flag_maps_to_a_text_decoration() {
        let palette = [ColorRgb { r: 0, g: 0, b: 0 }; 256];
        let colors = test_colors();
        let point = Pos::new(Line(0), Column(0));
        let cell = painted_term("\x1b[9ms");

        let rc = CellPainter::new(&cell, &palette, &colors, None).at(point);
        let style = GlyphStyle::of(&rc).strikethrough_style().unwrap();
        assert_eq!(style.thickness, px(1.));
        assert_eq!(style.color, Some(rc.fg));
    }

    #[test]
    fn special_underlines_are_not_overpainted_by_link_hover() {
        for kind in [
            UnderlineKind::Double,
            UnderlineKind::Dotted,
            UnderlineKind::Dashed,
        ] {
            let mut c = cell('l');
            c.underline = kind;
            c.link_hover = true;
            assert_eq!(GlyphStyle::of(&c).underline_style(), None, "{kind:?}");
        }
    }

    #[test]
    fn native_cells_keep_only_decorations_that_need_glyph_shaping() {
        for kind in [
            UnderlineKind::Double,
            UnderlineKind::Dotted,
            UnderlineKind::Dashed,
        ] {
            let mut c = cell('│');
            c.underline = kind;
            assert_eq!(native_cell_residue(&GlyphStyle::of(&c)), None);
        }

        let mut c = cell('│');
        c.strikeout = true;
        assert_eq!(native_cell_residue(&GlyphStyle::of(&c)), Some(' '));
    }

    fn ink(x: f32, y: f32, w: f32, h: f32) -> Bounds<Pixels> {
        Bounds::new(point(px(x), px(y)), size(px(w), px(h)))
    }

    /// Menlo at 22px: a 13.2px cell in a 30.8px row (line height 1.4).
    const CELL: f32 = 13.2;
    const EM: f32 = 22.;

    fn fit(ink: Bounds<Pixels>, cells: usize, lend: bool) -> (f32, usize) {
        icon_fit(ink, px(CELL), cells, lend, px(EM))
    }

    #[test]
    fn only_private_use_code_points_count_as_icons() {
        for c in [
            '\u{e000}',   // first of the BMP Private Use Area
            '\u{e0a0}',   // Powerline branch
            '\u{e0d8}',   // first past the Powerline separators
            '\u{f179}',   // Font Awesome apple
            '\u{f8ff}',   // last of the BMP Private Use Area
            '\u{f0001}',  // Material Design icon, Supplementary PUA-A
            '\u{10fffd}', // end of Supplementary PUA-B
        ] {
            assert!(is_icon(c), "U+{:04X}", c as u32);
        }
        // Text a fallback face supplies keeps its own metrics: CJK, emoji,
        // Latin, and the drawing characters painted natively anyway.
        for c in [
            '中',
            'あ',
            '한',
            '\u{1F600}',
            '\u{2764}',
            'M',
            '─',
            '█',
            '\u{f900}',
        ] {
            assert!(!is_icon(c), "U+{:04X}", c as u32);
        }
        // Nor do the Powerline separators, which are shapes, not icons.
        for c in ['\u{e0b0}', '\u{e0b1}', '\u{e0b3}', '\u{e0d7}'] {
            assert!(!is_icon(c), "U+{:04X}", c as u32);
        }
    }

    #[test]
    fn a_nerd_font_icon_before_a_space_takes_two_cells_at_full_size() {
        // Symbols Nerd Font Mono's apple, as CoreText measures it at 22px: a
        // full em tall and 1.4 cells wide. Held to one cell it is shrunk to
        // 0.71 of the text's size, which is what #866 saw; with the space
        // after it lent, it keeps its whole em.
        let apple = ink(1.76, -4.43, 18.48, 22.03);
        let (held, cells) = fit(apple, 1, false);
        assert_eq!(cells, 1);
        assert!((apple.size.width.as_f32() * held - CELL).abs() < 1e-3);
        let (lent, cells) = fit(apple, 1, true);
        assert_eq!(cells, 2);
        assert!((apple.size.height.as_f32() * lent - EM).abs() < 1e-3);
        assert!(apple.size.width.as_f32() * lent <= 2. * CELL);
    }

    #[test]
    fn a_narrow_icon_keeps_to_its_own_cell_even_when_the_next_is_free() {
        // The branch symbol is an em tall and 0.4em wide: one cell already
        // holds it at full height, so borrowing would only push it off
        // towards the text after it.
        let branch = ink(6.52, -4.4, 8.96, 22.);
        assert_eq!(fit(branch, 1, true), (1., 1));
    }

    #[test]
    fn a_small_icon_from_a_narrow_face_grows_to_fill_its_cell() {
        // A face with a narrower cell than the primary inks well under it.
        let icon = ink(0.5, -1., 8.8, 9.);
        let (scale, cells) = fit(icon, 1, false);
        assert_eq!(cells, 1);
        assert!(scale > 1.);
        assert!((icon.size.width.as_f32() * scale - CELL).abs() < 1e-3);
        // The same icon two cells wide may use both.
        let (two, cells) = fit(icon, 2, false);
        assert_eq!(cells, 2);
        assert!(two > scale);
    }

    #[test]
    fn a_fitted_icon_is_centred_in_its_room() {
        let icon = ink(0.4, -0.6, 5.3, 6.);
        let room = size(px(CELL), px(30.8));
        let (scale, _) = fit(icon, 1, false);
        let (ascent, descent) = (px(20.4 * scale), px(5.2 * scale));
        let at = icon_offset(icon, scale, room, true, ascent, descent);
        // Spilling into a borrowed cell, it stays where its face put it,
        // unless that would take its ink past either edge of the room.
        assert_eq!(
            icon_offset(icon, scale, room, false, ascent, descent).x,
            px(0.)
        );
        let lent = size(px(2. * CELL), px(30.8));
        let overhang = ink(-1., 0., 20., 20.);
        assert_eq!(
            icon_offset(overhang, 1., lent, false, ascent, descent).x,
            px(1.)
        );
        let pushed = ink(8., 0., 20., 20.);
        let x = icon_offset(pushed, 1., lent, false, ascent, descent).x;
        assert!((x.as_f32() - (2. * CELL - 28.)).abs() < 1e-4, "{x:?}");
        let left = at.x.as_f32() + icon.origin.x.as_f32() * scale;
        let right = left + icon.size.width.as_f32() * scale;
        assert!((left - (CELL - right)).abs() < 1e-3, "{left}..{right}");
        let baseline = at.y + (room.height - ascent - descent) / 2. + ascent;
        let top = (baseline - (icon.origin.y + icon.size.height) * scale).as_f32();
        let bottom = (baseline - icon.origin.y * scale).as_f32();
        assert!((top - (30.8 - bottom)).abs() < 1e-3, "{top}..{bottom}");
    }

    #[test]
    fn icon_fit_leaves_what_is_already_the_right_size_alone() {
        // Within 2% of the cell: not worth shaping the glyph again.
        assert_eq!(fit(ink(0., 0., 13., 13.), 1, false), (1., 1));
        // Nothing measured, nothing to scale by.
        assert_eq!(fit(ink(0., 0., 0., 0.), 1, true), (1., 1));
        // A dot is small by design and grows no more than the cap.
        assert_eq!(fit(ink(1., 2., 2., 2.), 1, false).0, MAX_ICON_GROWTH);
    }

    fn icon_row(next: RenderCell) -> Vec<RenderCell> {
        let icon = RenderCell {
            c: '\u{f179}',
            ..RenderCell::default()
        };
        vec![icon, next]
    }

    #[test]
    fn a_blank_in_the_same_colour_lends_an_icon_its_cell() {
        let blank = RenderCell::default();
        assert!(lends_to_icon(&icon_row(blank.clone()), 0, 1));
        // Whatever `has_room_after` lends, the end of the row included.
        assert!(lends_to_icon(&icon_row(blank.clone())[..1], 0, 1));
        // A prompt segment: icon and the space after it on one background.
        let bg = Hsla::blue();
        let mut row = icon_row(RenderCell {
            bg,
            draw_bg: true,
            ..RenderCell::default()
        });
        row[0].bg = bg;
        row[0].draw_bg = true;
        assert!(lends_to_icon(&row, 0, 1));
        // The next segment's colour, a glyph or a selection do not.
        row[1].bg = Hsla::red();
        assert!(!lends_to_icon(&row, 0, 1));
        let letter = RenderCell {
            c: 'x',
            ..RenderCell::default()
        };
        assert!(!lends_to_icon(&icon_row(letter), 0, 1));
        let selected = RenderCell {
            selected: true,
            ..RenderCell::default()
        };
        assert!(!lends_to_icon(&icon_row(selected), 0, 1));
        // Nor does a coloured blank after an icon on the default background.
        let coloured = RenderCell {
            bg,
            draw_bg: true,
            ..blank
        };
        assert!(!lends_to_icon(&icon_row(coloured), 0, 1));
    }
}
