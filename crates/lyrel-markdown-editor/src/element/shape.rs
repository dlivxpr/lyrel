//! The shaping stage: the per-logical-line WYSIWYG layout pass, split from the
//! former `element.rs` monolith by Markdown construct family.
//!
//! [`shape_document`] is the orchestrator: it builds the per-construct
//! pre-scans (Mermaid, `$$math$$`, property panels, fold regions, hidden
//! markers), walks the source lines, and delegates each line to its block
//! family's shaper — prose (hidden markers + inline math/images), tables,
//! fenced code, block widgets (images/chips/embeds), and gutter decorations
//! (blockquotes / callouts / tasks / lists). The quantized viewport-band
//! windowing (off-screen plain lines keep cached-height placeholders, the
//! caret's logical line always shapes for real) lives in
//! [`MarkdownLineShaper::shape`].

use super::props::build_prop_panel;
use super::*;

/// The WYSIWYG layout pass: shape `content` line-by-line so each logical line
/// can use its own font size (headings are larger — W2) and a standalone image
/// line can render as the image (W4). Returns, per logical line: the shaped
/// source [`WrappedLine`], its row height, and `Some(BlockImg)` when it paints
/// as an image. `md` drives the per-line size + inline styling (`None` = the
/// raw view: base size, no widgets); `diagnostics` are clipped + shifted to
/// each line. The caret's line (`caret_row`) always shows source, so an image
/// stays editable ("raw on caret"). A single line always shapes to one wrapped
/// line (incl. empty), so the counts match the logical lines and blank rows
/// stay positionable.
#[allow(clippy::too_many_arguments)]
pub(crate) fn shape_document(
    window: &mut Window,
    cx: &App,
    content: &str,
    base_font: &Font,
    base_color: Hsla,
    base_font_size: Pixels,
    diagnostics: &[Diagnostic],
    md: Option<&SyntaxStyle>,
    wrap_width: Option<Pixels>,
    // The visible page width inside the horizontal scroll owner. Tables fit
    // this even when unwrapped lines make the scroll content wider.
    page_width: Option<Pixels>,
    soft_wrap: bool,
    caret_row: Option<usize>,
    // The caret needs precise geometry even while unfocused or outside the shaping band.
    geometry_row: usize,
    block_image: Option<&BlockImageFn>,
    block_chip: Option<&BlockChipFn>,
    embed_view: Option<&EmbedViewFn>,
    block_mermaid: Option<&BlockMermaidFn>,
    block_math: Option<&BlockMathFn>,
    code_highlight: Option<&CodeHighlightFn>,
    tab_indent: usize,
    // The em the `block_math` provider rasterizes at, so inline `$…$` formulas can reuse those
    // rasters scaled to text size. `None` disables inline math.
    block_math_em: Option<f32>,
    editing_math: Option<(usize, usize, Pixels)>,
    scale_factor: f32,
    // The selected byte range; a line it touches keeps full source (markers
    // shown), the rest hide their markers (W6, reveal-on-caret).
    selection: (usize, usize),
    // An in-progress grip resize: the dragged image is *sized* to its live width
    // here (driving its row height) so the layout reflows live, rather than the
    // image painting over a stale, saved-size row.
    resize: Option<ImageResize>,
    // An in-progress column-border drag: that column takes the live width.
    col_resize: Option<TableColResize>,
    // The content's cached structural scans (see [`ScanData`]).
    scan: &ScanData,
    // Cross-frame shaping caches (see [`ShapeCaches`]).
    caches: &ShapeCaches,
    // The shaping window, in element-local y (quantized, one frame stale):
    // plain lines wholly outside it may skip shaping when their height is
    // cached. `None` = shape everything (first frame, raw view).
    band: Option<(Pixels, Pixels)>,
    // Collapsed headings (trimmed source lines) — their section lines fold to
    // height 0 like a folded callout's body.
    folded_headings: &std::collections::HashSet<String>,
) -> ShapedDoc {
    let mut out = ShapedDoc::default();
    let lines: Vec<&str> = content.split('\n').collect();
    // Ordered items paint their computed CommonMark position, not their
    // literal source digits (the reader renumbers the same way).
    let ordered_nums = &scan.ordered;
    // Rows a (non-empty) selection touches. A selected block reveals its raw
    // source just like the caret's block does, so what's highlighted is what a
    // copy carries — the per-line pass below already reveals inline markers on
    // selected lines; rendered BLOCKS (math, mermaid, panels, fences) need the
    // same region-wide, or a sweep silently selects invisible `$$`/fence text.
    let sel_rows: Option<Range<usize>> = (selection.0 != selection.1).then(|| {
        let (a, b) = (
            selection.0.min(selection.1).min(content.len()),
            selection.0.max(selection.1).min(content.len()),
        );
        let row_of = |off: usize| content[..off].matches('\n').count();
        row_of(a)..row_of(b) + 1
    });
    let sel_hits = |r: &Range<usize>| {
        sel_rows
            .as_ref()
            .is_some_and(|s| s.start < r.end && r.start < s.end)
    };
    // The Host owns the result state. Only a ready, inactive block collapses
    // to its diagram; loading/errors occupy the opening marker row while the
    // actual source remains editable underneath. Entering/selecting the marker
    // reveals its exact original spelling as well.
    // ---- Block-family pre-scans (each construct decides its rendered widget
    // here; the per-line loop below collapses to those widgets) ----
    let mermaid = mermaid_widgets(
        window,
        block_mermaid,
        md,
        scan,
        caret_row,
        &sel_hits,
        wrap_width,
        base_font_size,
        base_font,
        base_color,
    );
    let math = math_widgets(block_math, md, scan, caret_row, &sel_hits, wrap_width);
    let props = prop_panels(
        window,
        scan,
        md,
        caret_row,
        &sel_hits,
        &lines,
        base_font,
        base_font_size,
        base_color,
    );
    let alert_folds = collapsed_regions(md, scan, caret_row, content, folded_headings);
    let math_marker_lines = hidden_math_markers(md, content);
    // Table regions (W4c); content-fit column widths shared by each region's rows.
    let regions: &[markdown_syntax::TableRegion] = if md.is_some() { &scan.tables } else { &[] };
    // Shared component of the per-line run-cache key (see `CachedLineRuns`).
    let run_epoch = line_run_epoch(base_font, md);
    // Per-row content-key memo, valid for this (scan generation, epoch).
    let row_keys_epoch = {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        scan.generation.hash(&mut h);
        run_epoch.hash(&mut h);
        Some(h.finish())
    };
    {
        let mut rk = caches.row_keys.borrow_mut();
        if rk.0 != row_keys_epoch || rk.1.len() != lines.len() {
            *rk = (row_keys_epoch, vec![None; lines.len()]);
        }
    }
    // Measured column widths, cached across frames: rebuilt only when the
    let region_cols = region_column_widths(
        window,
        caches,
        scan,
        base_font_size,
        run_epoch,
        col_resize,
        wrap_width,
        page_width,
        regions,
        &lines,
        base_font,
        base_color,
    );
    let table_row_h = base_font_size * LINE_HEIGHT_RATIO + px(12.);
    // Fenced-code-block tracking: collect a block's line indices (so its box can
    // be sized to its widest line + the first/last line marked for rounding) and
    // the running max line width.
    // Direction of the last line that had a strong character, so blank lines
    // between paragraphs inherit it (#66).
    let mut last_strong_rtl = false;
    let mut code_block: Vec<usize> = Vec::new();
    let mut code_w = px(0.);
    // Token colors per code line (line index → in-line ranges), from the host
    // highlighter: each fenced block with a language tag is highlighted whole
    // (tree-sitter-style engines need full-block context), then split per line.
    let line_highlights = code_line_highlights(code_highlight, md, &lines, cx);
    let mut line_start = 0;
    let mut in_fence = false;
    // Active GitHub alert run (`> [!NOTE]` …): set by a marker line, carried
    // while blockquote lines continue, cleared by anything else — so every
    // line of the alert gets the kind's bar color.
    let mut alert_run: Option<Hsla> = None;
    let mut y_acc = px(0.);
    for (idx, &line) in lines.iter().enumerate() {
        let line_end = line_start + line.len();
        // Running top of this line — mirrors the prepaint `line_tops` walk
        // (including `line_pads`) so the band test is exact.
        if idx > 0 {
            let (tp, bp) = line_pads(out.backgrounds[idx - 1], out.tables[idx - 1].as_ref());
            y_acc += tp + bp + out.heights[idx - 1] * out.wrap_rows[idx - 1] as f32;
        }

        // Folded section visibility takes precedence over resource widgets.
        // Keep fence tracking intact for source rows omitted from the layout.
        if alert_folds
            .iter()
            .any(|r| r.contains(&idx) && idx != r.start)
        {
            if line.trim_start().starts_with("```") {
                in_fence = !in_fence;
            }
            out.push_placeholder(window, base_font_size, wrap_width, px(0.), None, None, 1);
            line_start = line_end + 1;
            alert_run = None;
            continue;
        }

        // A ready mermaid block renders as its diagram (on the first line) with the
        // rest of the block collapsed — bypassing the normal per-line handling. Its
        // ``` fences still toggle `in_fence` so later code blocks track correctly.
        if let Some((range, widget)) = mermaid.iter().find(|(r, _)| r.contains(&idx)) {
            if matches!(widget, Block::Image(_)) || idx == range.start {
                if line.trim_start().starts_with("```") {
                    in_fence = !in_fence;
                }
                let (h, widget) = if idx == range.start {
                    (widget.height(), Some(widget.clone()))
                } else {
                    (px(0.), None)
                };
                out.push_placeholder(window, base_font_size, wrap_width, h, widget, None, 1);
                line_start = line_end + 1;
                alert_run = None;
                continue;
            }
        }

        // An in-line-edited $$ block reserves a fixed gap; the host paints the live editor
        // there (positioned from this line's top/height). Takes precedence over the image.
        if let Some((start_row, end_row, gap_h)) = editing_math
            && (start_row..=end_row).contains(&idx)
        {
            let h = if idx == start_row { gap_h } else { px(0.) };
            out.push_placeholder(window, base_font_size, wrap_width, h, None, None, 1);
            line_start = line_end + 1;
            alert_run = None;
            continue;
        }

        // A ready $$…$$ math block renders as its equation on the first line, the rest
        // collapsed. Unlike mermaid it's not a ``` fence, so it never toggles `in_fence`.
        if let Some((range, bi)) = math.iter().find(|(r, _)| r.contains(&idx)) {
            let (h, widget) = if idx == range.start {
                (bi.height, Some(Block::Image(bi.clone())))
            } else {
                (px(0.), None)
            };
            out.push_placeholder(window, base_font_size, wrap_width, h, widget, None, 1);
            line_start = line_end + 1;
            alert_run = None;
            continue;
        }

        // A standalone `![[target]]` transclusion the host can render reserves
        // a gap the height the host asked for — the embed view overlays it
        // (see `embed_overlays`). Raw on caret; an unresolved target falls
        // through to the chip below.
        if md.is_some()
            && caret_row != Some(idx)
            && let Some(inner) = zorite_markdown::syntax::embed_line(line)
            && let Some(h) = embed_view.and_then(|f| f(inner).map(|(_, h)| h))
        {
            out.push_placeholder(window, base_font_size, wrap_width, h, None, None, 1);
            line_start = line_end + 1;
            continue;
        }

        // A property panel renders on the region's first line; the rest collapse.
        // (Like math, the region is filtered out above when the caret is inside,
        // so the raw `key:: value` lines show for editing.)
        if let Some((range, panel)) = props.iter().find(|(r, _)| r.contains(&idx)) {
            let (h, widget) = if idx == range.start {
                (panel.height, Some(Block::Properties(panel.clone())))
            } else {
                (px(0.), None)
            };
            out.push_placeholder(window, base_font_size, wrap_width, h, widget, None, 1);
            line_start = line_end + 1;
            alert_run = None;
            continue;
        }

        // Fenced code block (W4b): a ``` line toggles the fence; the delimiter
        // lines + the lines between render as monospace code over a content-fit
        // background (delimiters dimmed). Code is literal — no inline scanning,
        // no heading size, no squiggles. Styling-mode only.
        let is_fence = md.is_some() && line.trim_start().starts_with("```");
        let is_code = md.is_some() && (in_fence || is_fence);
        // This line's alert membership: `(color, is_marker_line)`.
        let alert_line: Option<(Hsla, bool)> = if let Some(st) = md.filter(|_| !is_code) {
            match markdown_syntax::blockquote_prefix(line) {
                Some(plen) => {
                    if let Some(kind) = markdown_syntax::alert_kind(&line[plen..]) {
                        let c = st.alert_color(kind);
                        alert_run = Some(c);
                        Some((c, true))
                    } else {
                        alert_run.map(|c| (c, false))
                    }
                }
                None => {
                    alert_run = None;
                    None
                }
            }
        } else {
            alert_run = None;
            None
        };
        if is_fence {
            in_fence = !in_fence;
        }
        // A ``` fence line collapses (height 0, no text) unless the caret is in
        // its block — so a code block reads as just its boxed body (W6), with the
        // fences re-appearing while you edit inside it.
        // A fence stays hidden even while editing inside the block (the card's
        // language tag shows/edits the language) — it reveals only with the
        // caret or a selection on the fence line itself.
        let collapse_fence = is_fence && caret_row != Some(idx) && !sel_hits(&(idx..idx + 1));
        // A `<!-- table:STYLE -->` or `<!-- math:ALIGN -->` marker line collapses (hidden)
        // too, unless the caret lands on it — so the marker stays out of the way but editable.
        let collapse_marker = caret_row != Some(idx)
            && (regions.iter().any(|r| r.marker_line == Some(idx))
                || math_marker_lines.contains(&idx));
        // Leaving a code block: size the box to its widest line (+ the inset on
        // each side, like a table) and mark its last line so the box rounds + pads
        // its bottom edge. The vertical padding is grown into the painted quad, so
        // line geometry — and the caret — stay untouched.
        if !is_code && !code_block.is_empty() {
            let bw = code_w + px(2. * CODE_INSET);
            let last = *code_block.last().unwrap();
            for &bi in &code_block {
                if let Some(cb) = &mut out.backgrounds[bi] {
                    cb.width = bw;
                    cb.bottom = bi == last;
                }
            }
            code_block.clear();
            code_w = px(0.);
        }
        let fs = if is_code {
            base_font_size
        } else {
            base_font_size * md.map_or(1.0, |_| markdown_syntax::line_scale(line))
        };

        // Block widget (non-code): a standalone `![](src)` line that isn't the
        // caret's renders as a file chip (if the host classifies `src` as one,
        // e.g. a PDF) or else its decoded image, fit to width.
        // A renderable image: a standalone `![](src)` line, or the sole body of a
        // list item (`- ![](src)`). For the list case `marker_len` > 0, so the
        // image renders inset past the bullet (still painted by the gutter) and
        // sized to the remaining width — instead of the row collapsing to the
        // image (losing its bullet) or falling back to raw source.
        let img_row = (!is_code)
            .then(|| markdown_syntax::image_row(line))
            .flatten();
        let img_inset = match img_row {
            Some((_, _, marker_len)) if marker_len > 0 => {
                measure_width(window, &line[..marker_len], base_font, base_font_size)
            }
            _ => px(0.),
        };
        let widget = block_widget(
            md,
            img_row,
            idx,
            caret_row,
            block_chip,
            block_image,
            resize,
            wrap_width,
            img_inset,
            scale_factor,
            fs,
            window,
        );
        let table = table_row(
            regions,
            &region_cols,
            idx,
            is_code,
            caret_row,
            line,
            line_start,
            selection,
            base_font,
            base_color,
            md,
        );
        // A line shows full source while a non-empty selection touches it (so the
        // markers are visible to select) or styling is off. Otherwise its markers
        // are hidden — except, on the caret's own line, the single construct the
        // caret sits in is revealed (per-construct reveal, #5: finer than the old
        // whole-line reveal, so the rest of the line stays rendered).
        let sel_empty = selection.0 == selection.1;
        let caret_col = (sel_empty && selection.0 >= line_start && selection.0 <= line_end)
            .then(|| selection.0 - line_start);
        // An `![](src)` chip line shows full raw source while the caret is on it,
        // so editing reveals the whole `![](src)` rather than the per-construct
        // view. Image rows are exempt — they keep rendering the picture with the
        // caret parked beside it (Word-style; see the widget gate above).
        let chip_line =
            md.is_some() && img_row.is_some() && !matches!(widget, Some(Block::Image(_)));
        let sel_touches = !sel_empty && selection.0 <= line_end && selection.1 >= line_start;
        // A selection no longer reveals raw source (Cditor-style: formatting
        // markers are hidden everywhere) — selected lines keep the hidden view;
        // structural markers still come back via `reveal_inline`.
        let full_source = caret_col.is_some() && chip_line;
        // This line's diagnostics, clipped + shifted to line-local byte offsets —
        // used as spell-check squiggles whether the line shows source or hides its
        // markers.
        let line_diags: Vec<Diagnostic> = diagnostics
            .iter()
            .filter_map(|d| {
                let s = d.range.start.max(line_start);
                let e = d.range.end.min(line_end);
                (s < e).then(|| Diagnostic {
                    range: (s - line_start)..(e - line_start),
                })
            })
            .collect();
        let gutter = gutter_mark(
            md,
            line,
            idx,
            is_code,
            widget.is_none() || img_inset > px(0.),
            table.is_none(),
            alert_line,
            ordered_nums,
            tab_indent,
            base_font,
            base_font_size,
            window,
        );
        let caret_here = caret_col.is_some();
        // A thematic break (`---`) renders as a full-width divider, but only while
        // the caret is off it; on the line it reads as the raw `---` (editable).
        let is_rule = !is_code
            && widget.is_none()
            && table.is_none()
            && !caret_here
            && !full_source
            && md.is_some()
            && markdown_syntax::thematic_break(line);
        // The caret sitting in the BODY keeps the list/quote mark — the line
        // used to reveal whole (raw prefix at raw indent), so every edit
        // jumped the text left. Only a caret inside the hidden prefix (left
        // arrow past the body start) reveals the raw marker for editing.
        let caret_in_prefix = gutter
            .as_ref()
            .zip(caret_col)
            .is_some_and(|(&(plen, _), col)| col < plen);
        // A selection across a gutter line keeps the mark too: the whole-line
        // raw reveal made list rows renumber (source digits) and jump to raw
        // indent mid-select. The BODY still reveals its inline markers (see
        // `reveal_inline` below) so what's highlighted is what's copied.
        let full_source = full_source && gutter.is_none();
        let reveal_inline = sel_touches;
        let mark = if is_rule {
            md.map(|st| LineMark::Rule(st.rule))
        } else {
            gutter
                .filter(|_| !caret_in_prefix && !full_source)
                .map(|(_, m)| m)
        };
        let reveal_prefix = gutter
            .filter(|_| caret_in_prefix)
            .map_or(0, |(plen, _)| plen);
        let hide_prefix = gutter
            .filter(|_| !caret_in_prefix)
            .map_or(0, |(plen, _)| plen);
        // Footnote definitions (`[^1]: …`) and raw-HTML lines render muted, the way
        // the reading view shows them — a whole-line color, no hidden markers.
        let muted_line = md
            .filter(|_| !is_code && widget.is_none() && table.is_none())
            .filter(|_| {
                markdown_syntax::footnote_def(line).is_some() || markdown_syntax::html_block(line)
            })
            .map(|st| st.quote);
        // A blockquote's body is muted; a list keeps the normal body color (only
        // its bullet is muted).
        let line_base = match mark {
            Some(LineMark::Quote { text, .. }) => text.unwrap_or(base_color),
            _ => muted_line.unwrap_or(base_color),
        };
        // Inline `$…$` formulas spliced into this line (populated by the hidden-markers branch).
        let mut line_inline_math: Vec<InlineMath> = Vec::new();
        // Set (in the markdown branch below) when this line is eligible for the
        // per-line height cache — its (row height, wrap rows) get recorded at
        // push time so a later frame can window it out without shaping.
        let mut plain_hkey: Option<u64> = None;
        let markdown_line_shaper = MarkdownLineShaper {
            caches,
            run_epoch,
            geometry_row,
            band,
            base_font,
            block_math,
            block_math_em,
            block_image,
        };
        let (shaped_text, runs, bg, map) = if collapse_fence || collapse_marker {
            // Hidden ``` fence line or table-style marker: nothing, zero height.
            (
                SharedString::default(),
                std::rc::Rc::new(Vec::new()),
                None,
                None,
            )
        } else if is_rule {
            // Thematic break: the divider is painted from the mark; no body text.
            (
                SharedString::default(),
                std::rc::Rc::new(Vec::new()),
                None,
                None,
            )
        } else if let Some(st) = md.filter(|_| is_code) {
            // Monospace runs; ``` delimiters dimmed, token-colored on highlight.
            let (text, runs, cb) = shape_code_line(
                st,
                line,
                idx,
                code_block.is_empty(),
                is_fence,
                &line_highlights,
            );
            (text, runs, Some(cb), None)
        } else if let Some(st) = md.filter(|_| widget.is_none() && table.is_none() && !full_source)
        {
            match markdown_line_shaper.shape(
                &mut out,
                window,
                line,
                idx,
                line_start,
                st,
                line_base,
                &line_diags,
                caret_col,
                reveal_prefix,
                hide_prefix,
                reveal_inline,
                mark,
                fs,
                wrap_width,
                base_font_size,
                y_acc,
                collapse_fence,
                collapse_marker,
            ) {
                // Windowed out: the placeholder is pushed; skip to the next line.
                None => {
                    line_start = line_end + 1;
                    continue;
                }
                Some((text, runs, map, inline, hkey)) => {
                    line_inline_math = inline;
                    plain_hkey = hkey;
                    (text, runs, None, map)
                }
            }
        } else {
            // Full source with diagnostics (the caret/selected line, or md off).
            (
                SharedString::from(line.to_string()),
                std::rc::Rc::new(markdown_syntax::styled_runs(
                    line,
                    base_font,
                    line_base,
                    &line_diags,
                    md,
                )),
                None,
                None,
            )
        };
        // Code lines are inset by CODE_INSET on each side; a gutter mark insets the
        // left only. Either wraps at a correspondingly narrower width. A table
        // row or widget line (image, chip) never wraps its raw source — a
        // grid row / the widget renders instead, and a wrapped hidden source
        // would multiply the row's advance by its wrap count. (Revealed-on-
        // caret lines have `table`/`widget` = None here and wrap normally.)
        let line_wrap = if table.is_some() || widget.is_some() {
            None
        } else if is_code {
            wrap_width.map(|w| (w - px(2. * CODE_INSET)).max(px(0.)))
        } else if let Some(m) = mark {
            wrap_width.map(|w| (w - m.inset()).max(px(0.)))
        } else {
            wrap_width
        };
        let shaped = shape_runs(window, &shaped_text, fs, &runs, line_wrap, soft_wrap);
        if let Some(wl) = shaped.into_iter().next() {
            let h = shaped_line_height(
                window,
                caches,
                run_epoch,
                base_font,
                base_font_size,
                table_row_h,
                collapse_fence,
                collapse_marker,
                table.as_ref(),
                widget.as_ref(),
                &line_inline_math,
                line,
                fs,
                md.is_some(),
            );
            let line_w = wl.width();
            let (line_rtl, rtl_rows, span) = line_rtl_rows(
                window,
                line,
                idx,
                &lines,
                &mut last_strong_rtl,
                bg,
                widget.as_ref(),
                table.as_ref(),
                &shaped_text,
                &runs,
                line_wrap,
                fs,
                &wl,
            );
            if let Some(hk) = plain_hkey {
                caches.line_heights.borrow_mut().insert(hk, (h, span));
            }
            out.wrap_rows.push(span);
            out.rtl_rows.push(rtl_rows.map(|rows| (line_rtl, rows)));
            out.wrapped.push(wl);
            out.heights.push(h);
            out.widgets.push(widget);
            out.backgrounds.push(bg);
            out.tables.push(table);
            out.maps.push(map);
            out.marks.push(mark);
            out.inline_maths.push(line_inline_math);
            // Track a (visible) code line + its width so the block's box can be
            // sized to its widest line and its last line marked.
            if is_code && !collapse_fence {
                code_block.push(out.backgrounds.len() - 1);
                code_w = code_w.max(line_w);
            }
        }
        line_start = line_end + 1; // skip the '\n'
    }
    // A code block running to the end of the document: size its box + mark its
    // last line (round the box bottom + pad).
    if !code_block.is_empty() {
        let bw = code_w + px(2. * CODE_INSET);
        let last = *code_block.last().unwrap();
        for &bi in &code_block {
            if let Some(cb) = &mut out.backgrounds[bi] {
                cb.width = bw;
                cb.bottom = bi == last;
            }
        }
    }
    // Cap the run cache: edits retire entries (each changed line re-keys), so
    // without a bound it grows one dead entry per keystroke. Clearing wholesale
    // is fine — the next frame rebuilds only what's visible… which is
    // everything today, but each rebuild re-primes the cache.
    {
        let mut cache = caches.line_runs.borrow_mut();
        if cache.len() > lines.len() * 2 + 64 {
            cache.clear();
        }
        let mut rows = caches.cell_rows.borrow_mut();
        if rows.len() > lines.len() * 2 + 64 {
            rows.clear();
        }
        let mut lh = caches.line_heights.borrow_mut();
        if lh.len() > lines.len() * 2 + 64 {
            lh.clear();
        }
    }
    out
}

// ===== Mermaid diagrams: ready blocks collapse to their raster (or a
// loading/error status on the opening row); raw on caret/selection =====
#[allow(clippy::too_many_arguments)]
fn mermaid_widgets(
    window: &mut Window,
    block_mermaid: Option<&BlockMermaidFn>,
    md: Option<&SyntaxStyle>,
    scan: &ScanData,
    caret_row: Option<usize>,
    sel_hits: &dyn Fn(&Range<usize>) -> bool,
    wrap_width: Option<Pixels>,
    base_font_size: Pixels,
    base_font: &Font,
    base_color: Hsla,
) -> Vec<(Range<usize>, Block)> {
    // The Host owns the result state. Only a ready, inactive block collapses
    // to its diagram; loading/errors occupy the opening marker row while the
    // actual source remains editable underneath. Entering/selecting the marker
    // reveals its exact original spelling as well.
    match block_mermaid.filter(|_| md.is_some()) {
        Some(f) => scan
            .mermaid
            .iter()
            .filter_map(|(range, source)| {
                let presentation = f(range.start, source);
                let failed = matches!(&presentation, MermaidPresentation::Failed(_));
                match presentation {
                    MermaidPresentation::Ready(img, lw, lh)
                        if caret_row.is_none_or(|cr| !range.contains(&cr))
                            && !sel_hits(range)
                            && lw.is_finite()
                            && lh.is_finite()
                            && lw > 0.
                            && lh > 0. =>
                    {
                        let w = lw.min(wrap_width.map_or(lw, f32::from)).max(1.);
                        Some((
                            range.clone(),
                            Block::Image(BlockImg {
                                img,
                                width: px(w),
                                height: px(w * lh / lw),
                                resizable: false,
                                align: MathAlign::Left,
                            }),
                        ))
                    }
                    MermaidPresentation::Loading(message)
                    | MermaidPresentation::Failed(message)
                        if caret_row != Some(range.start) && !sel_hits(range) =>
                    {
                        let color = md.map_or(base_color, |style| {
                            if failed {
                                style.alert_caution
                            } else {
                                style.marker
                            }
                        });
                        let text = shape_all(
                            window,
                            &message,
                            base_font_size,
                            base_font.clone(),
                            color,
                            wrap_width,
                        );
                        let line_height = base_font_size * LINE_HEIGHT_RATIO;
                        let rows: usize = text
                            .iter()
                            .map(|line| line.wrap_boundaries().len() + 1)
                            .sum();
                        Some((
                            range.clone(),
                            Block::MermaidStatus {
                                text: std::rc::Rc::new(text),
                                line_height,
                                height: line_height * rows.max(1) as f32,
                            },
                        ))
                    }
                    _ => None,
                }
            })
            .collect(),
        None => Vec::new(),
    }
}

// ===== `$$…$$` math blocks ready to render: the equation paints on the
// block's first line, the rest collapse (like Mermaid) =====
fn math_widgets(
    block_math: Option<&BlockMathFn>,
    md: Option<&SyntaxStyle>,
    scan: &ScanData,
    caret_row: Option<usize>,
    sel_hits: &dyn Fn(&Range<usize>) -> bool,
    wrap_width: Option<Pixels>,
) -> Vec<(Range<usize>, BlockImg)> {
    match block_math.filter(|_| md.is_some()) {
        Some(f) => scan
            .math
            .iter()
            .filter(|r| caret_row.is_none_or(|cr| !r.range.contains(&cr)) && !sel_hits(&r.range))
            .cloned()
            .filter_map(|r| {
                // Sized by the provider's logical dimensions — NOT texture pixels
                // ÷ window scale factor (see [`BlockMathFn`]: the raster's density
                // is fixed, so that division is only correct on a 2× display).
                let (img, lw, lh) = f(&r.source)?;
                if lw <= 0. || lh <= 0. {
                    return None;
                }
                let w = lw.min(wrap_width.map_or(lw, f32::from)).max(1.);
                // Math renders at its natural typeset size — no resize grip (nothing to
                // persist a width to, and it goes inline eventually). It carries its
                // horizontal alignment (centered by default) for the paint to honor.
                Some((
                    r.range,
                    BlockImg {
                        img,
                        width: px(w),
                        height: px(w * lh / lw),
                        resizable: false,
                        align: r.align,
                    },
                ))
            })
            .collect(),
        None => Vec::new(),
    }
}

// ===== `key:: value` property runs → two-column panels (reader parity) =====
#[allow(clippy::too_many_arguments)]
fn prop_panels(
    window: &mut Window,
    scan: &ScanData,
    md: Option<&SyntaxStyle>,
    caret_row: Option<usize>,
    sel_hits: &dyn Fn(&Range<usize>) -> bool,
    lines: &[&str],
    base_font: &Font,
    base_font_size: Pixels,
    base_color: Hsla,
) -> Vec<(Range<usize>, PropPanel)> {
    // Like a math block, the panel paints on the region's first line and the
    // rest of its lines collapse; the caret entering the region is filtered
    // out here so the raw `key:: value` lines show for editing.
    match md.filter(|style| style.property_icon.is_some()) {
        Some(st) => scan
            .props
            .iter()
            .filter(|r| caret_row.is_none_or(|cr| !r.contains(&cr)) && !sel_hits(r))
            .cloned()
            .map(|r| {
                let panel = build_prop_panel(
                    lines,
                    &r,
                    window,
                    base_font,
                    base_font_size,
                    st.marker,
                    base_color,
                    st.tag,
                    st.link,
                    st.property_icon.as_ref(),
                );
                (r, panel)
            })
            .collect(),
        None => Vec::new(),
    }
}

// ===== Folded regions: foldable callouts (`> [!NOTE]-`) + collapsed heading
// sections. Each region's BODY lines collapse (the marker line stays); the
// caret inside reveals — reveal-on-caret, so arrowing in unfolds for editing =====
fn collapsed_regions(
    md: Option<&SyntaxStyle>,
    scan: &ScanData,
    caret_row: Option<usize>,
    content: &str,
    folded_headings: &std::collections::HashSet<String>,
) -> Vec<Range<usize>> {
    let mut alert_folds: Vec<Range<usize>> = if md.is_some() {
        scan.alert_folds
            .iter()
            .filter(|(r, folded)| *folded && caret_row.is_none_or(|cr| !r.contains(&cr)))
            .map(|(r, _)| r.clone())
            .collect()
    } else {
        Vec::new()
    };
    // Collapsed heading sections fold the same way: the heading line stays
    // (its chevron paints there), the section's lines go to height 0, and the
    // caret STRICTLY INSIDE THE BODY reveals — the fold state itself is
    // view-local (`EditorState::folded_headings`), not in the source. Unlike a
    // callout (whose marker line reveals its body — that's how you reach the
    // fold char), the caret sitting on the heading line itself must not
    // reveal: it's exactly where the caret lands after clicking around a
    // section you then fold.
    if md.is_some() {
        alert_folds.extend(
            markdown_syntax::heading_fold_regions(content, folded_headings)
                .into_iter()
                .filter(|r| caret_row.is_none_or(|cr| cr <= r.start || cr >= r.end)),
        );
    }
    alert_folds
}

// ===== `<!-- math:ALIGN -->` marker lines to hide (revealed only when the
// caret lands on them), like table style markers =====
fn hidden_math_markers(md: Option<&SyntaxStyle>, content: &str) -> Vec<usize> {
    if md.is_some() {
        markdown_syntax::math_regions(content)
            .iter()
            .filter_map(|r| r.marker_line)
            .collect()
    } else {
        Vec::new()
    }
}

// ===== Tables: measured column widths, cached across frames (rebuilt only
// when the tables' source, the wrap width, the font epoch, or a live column
// drag changes — measuring shaped every cell of every table per call) =====
#[allow(clippy::too_many_arguments)]
fn region_column_widths(
    window: &mut Window,
    caches: &ShapeCaches,
    scan: &ScanData,
    base_font_size: Pixels,
    run_epoch: u64,
    col_resize: Option<TableColResize>,
    wrap_width: Option<Pixels>,
    page_width: Option<Pixels>,
    regions: &[markdown_syntax::TableRegion],
    lines: &[&str],
    base_font: &Font,
    base_color: Hsla,
) -> std::rc::Rc<Vec<Vec<Pixels>>> {
    let cols_key = {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        // The scan generation covers every table byte and the `cols=`
        // attr — no need to rehash the table text per frame.
        scan.generation.hash(&mut h);
        f32::from(base_font_size).to_bits().hash(&mut h);
        run_epoch.hash(&mut h);
        if let Some(r) = col_resize {
            r.header_row.hash(&mut h);
            r.col.hash(&mut h);
            r.width.to_bits().hash(&mut h);
        }
        // Table columns are fitted to the note width, so a resize that
        // crosses a fit boundary must rebuild them.
        wrap_width.map(|w| f32::from(w).to_bits()).hash(&mut h);
        page_width.map(|w| f32::from(w).to_bits()).hash(&mut h);
        h.finish()
    };
    let hit = caches
        .region_cols
        .borrow()
        .as_ref()
        .filter(|(k, cols)| *k == cols_key && cols.len() == regions.len())
        .map(|(_, cols)| cols.clone());
    match hit {
        Some(cols) => cols,
        None => {
            let cols: std::rc::Rc<Vec<Vec<Pixels>>> = std::rc::Rc::new(
                regions
                    .iter()
                    .map(|r| {
                        table_column_widths(
                            lines,
                            r,
                            window,
                            base_font,
                            base_font_size,
                            base_color,
                            col_resize,
                            page_width.or(wrap_width),
                        )
                    })
                    .collect(),
            );
            *caches.region_cols.borrow_mut() = Some((cols_key, cols.clone()));
            cols
        }
    }
}

// ===== Fenced code: token colors per code line (line index → in-line ranges),
// from the host highlighter — each fenced block with a language tag is
// highlighted whole (tree-sitter-style engines need full-block context),
// then split per line =====
fn code_line_highlights(
    code_highlight: Option<&CodeHighlightFn>,
    md: Option<&SyntaxStyle>,
    lines: &[&str],
    cx: &App,
) -> std::collections::HashMap<usize, Vec<(Range<usize>, HighlightStyle)>> {
    match (code_highlight, md) {
        (Some(hl), Some(_)) => {
            let mut map = std::collections::HashMap::new();
            let mut i = 0;
            while i < lines.len() {
                let Some(lang) = lines[i].trim_start().strip_prefix("```") else {
                    i += 1;
                    continue;
                };
                let lang = lang.split_whitespace().next().unwrap_or("");
                let mut j = i + 1;
                while j < lines.len() && !lines[j].trim_start().starts_with("```") {
                    j += 1;
                }
                // Mermaid blocks render as diagrams, not code.
                if !lang.is_empty() && lang != "mermaid" && j > i + 1 {
                    let block = lines[i + 1..j].join("\n");
                    let ranges = hl(lang, &block, cx);
                    let mut start = 0;
                    for (k, l) in lines[i + 1..j].iter().enumerate() {
                        let end = start + l.len();
                        let in_line: Vec<(Range<usize>, HighlightStyle)> = ranges
                            .iter()
                            .filter_map(|(r, style)| {
                                let (a, b) = (r.start.max(start), r.end.min(end));
                                // `then` (lazy), not `then_some`: the arg is
                                // evaluated eagerly, and `b - start` underflows
                                // for a token wholly before this line.
                                (a < b).then(|| (a - start..b - start, *style))
                            })
                            .collect();
                        if !in_line.is_empty() {
                            map.insert(i + 1 + k, in_line);
                        }
                        start = end + 1;
                    }
                }
                i = j + 1;
            }
            map
        }
        _ => std::collections::HashMap::new(),
    }
}

// ===== Standalone image sizing =====
/// Fit-to-width display size for an inline image from its natural (device) size:
/// cap to the content width (or an explicit `{width=N}`), preserving aspect.
fn block_img(
    img: Arc<RenderImage>,
    width_attr: Option<f32>,
    wrap_width: Option<Pixels>,
    scale_factor: f32,
) -> Option<BlockImg> {
    let dev = img.size(0);
    let (dw, dh) = (dev.width.0 as f32, dev.height.0 as f32);
    if dw <= 0. || dh <= 0. || scale_factor <= 0. {
        return None;
    }
    let natural_w = dw / scale_factor;
    let avail = wrap_width.map_or(natural_w, f32::from);
    let target_w = width_attr.unwrap_or(natural_w).min(avail).max(1.);
    Some(BlockImg {
        img,
        width: px(target_w),
        height: px(target_w * dh / dw),
        resizable: true,
        align: MathAlign::Left, // images stay left; math overrides with its marker
    })
}

// ===== Block widgets: a standalone `![](src)` line renders as a file chip (if
// the host classifies `src` as one, e.g. a PDF) or else its decoded image,
// fit to width. A renderable image: a standalone `![](src)` line, or the sole
// body of a list item (`- ![](src)`) — `marker_len` > 0 insets past the bullet =====
#[allow(clippy::too_many_arguments)]
fn block_widget(
    md: Option<&SyntaxStyle>,
    img_row: Option<(&str, Option<f32>, usize)>,
    idx: usize,
    caret_row: Option<usize>,
    block_chip: Option<&BlockChipFn>,
    block_image: Option<&BlockImageFn>,
    resize: Option<ImageResize>,
    wrap_width: Option<Pixels>,
    img_inset: Pixels,
    scale_factor: f32,
    fs: Pixels,
    _window: &mut Window,
) -> Option<Block> {
    if let Some(st) = md
        && let Some((src, w_attr, _)) = img_row
    {
        if let Some(label) = block_chip.and_then(|f| f(src)) {
            // A chip's line still edits as text: the caret's own row reveals
            // raw source instead of the chip.
            (Some(idx) != caret_row).then(|| Block::Chip {
                src: src.into(),
                label,
                link: st.link,
                bg: st.code_bg,
                border: st.marker,
                height: fs * LINE_HEIGHT_RATIO + px(CHIP_PAD * 2.),
                wiki: false,
            })
        } else {
            // An image renders even on the caret's own row — a Word-style
            // atomic object: the caret parks beside the picture (painted at
            // its edge) instead of revealing the markdown. Delete it via
            // Backspace/Delete or the right-click menu; WYSIWYG-off still
            // shows the raw source for hand-editing.
            block_image
                .and_then(|f| f(src))
                .and_then(|img| {
                    // A live grip resize sizes the image to the drag width, so
                    // its row height tracks the drag and the layout reflows.
                    let width_attr = match resize {
                        Some(r) if r.line == idx => Some(r.width),
                        _ => w_attr,
                    };
                    block_img(
                        img,
                        width_attr,
                        wrap_width.map(|w| (w - img_inset).max(px(1.))),
                        scale_factor,
                    )
                })
                .map(Block::Image)
        }
    } else {
        None
    }
}

// ===== Table rows (W4c + cell editing): render as a grid row; the caret on a
// header/body row edits in place (caret rendered inside the cell). Only the
// caret on the `|---|` separator drops the whole table to raw source (to edit
// alignment), avoiding a broken outer box around a half-raw table =====
#[allow(clippy::too_many_arguments)]
fn table_row(
    regions: &[markdown_syntax::TableRegion],
    region_cols: &[Vec<Pixels>],
    idx: usize,
    is_code: bool,
    caret_row: Option<usize>,
    line: &str,
    line_start: usize,
    selection: (usize, usize),
    base_font: &Font,
    base_color: Hsla,
    md: Option<&SyntaxStyle>,
) -> Option<TableRow> {
    regions
        .iter()
        .position(|r| r.lines.contains(&idx))
        .filter(|&ri| !is_code && caret_row != Some(regions[ri].lines.start + 1))
        .map(|ri| {
            let r = &regions[ri];
            let cell_ranges = markdown_syntax::table_cell_ranges(line);
            let source_cells = markdown_syntax::table_cells(line);
            let cell_presentations: Vec<_> = source_cells
                .iter()
                .enumerate()
                .map(|(cell, source)| {
                    let range = &cell_ranges[cell];
                    let caret = (caret_row == Some(idx)
                        && selection.0 == selection.1
                        && selection.0 >= line_start + range.start
                        && selection.0 <= line_start + range.end)
                        .then(|| selection.0 - line_start - range.start);
                    let selected = selection.0 != selection.1
                        && selection.0 <= line_start + range.end
                        && selection.1 >= line_start + range.start;
                    let font = if idx == r.lines.start {
                        Font {
                            weight: FontWeight::BOLD,
                            ..base_font.clone()
                        }
                    } else {
                        base_font.clone()
                    };
                    let (display, runs, map) = markdown_syntax::hidden_runs(
                        source,
                        &font,
                        base_color,
                        &[],
                        caret,
                        0,
                        0,
                        selected,
                        md.expect("table regions require Markdown styling"),
                    );
                    (
                        SharedString::from(display),
                        std::rc::Rc::new(runs),
                        std::rc::Rc::new(map),
                    )
                })
                .collect();
            TableRow {
                cells: cell_presentations
                    .iter()
                    .map(|(display, _, _)| display.clone())
                    .collect(),
                source_cells: source_cells.into_iter().map(SharedString::from).collect(),
                cell_runs: cell_presentations
                    .iter()
                    .map(|(_, runs, _)| runs.clone())
                    .collect(),
                cell_maps: cell_presentations
                    .iter()
                    .map(|(_, _, map)| map.clone())
                    .collect(),
                cell_ranges,
                aligns: r.aligns.clone(),
                col_widths: region_cols[ri].clone(),
                is_header: idx == r.lines.start,
                is_separator: idx == r.lines.start + 1,
                is_last: idx + 1 == r.lines.end,
                // 0 for the first body row; None for the header/separator.
                body_index: idx.checked_sub(r.lines.start + 2),
                style: r.style,
                border: md.map_or(base_color, |m| m.marker),
                shade: md.map_or(hsla(0., 0., 0., 0.), |m| m.code_bg),
                rtl: r.rtl,
            }
        })
}

// ===== Gutter decorations (blockquote / list): a non-code/widget/table line
// with a `>` or list marker. The decoration (border / bullet, marker hidden,
// body inset) shows only while the caret is OFF the line; on the line it reads
// as plain source with the prefix revealed (a line-level reveal — the whole
// prefix shows wherever the caret sits, unlike inline #5). `bullet_x` =
// measured width of the leading whitespace; `text_inset` = measured width of
// the whole source prefix (matching the raw line so render + edit stay in
// sync). `widget_or_img` keeps a list-item image's bullet alongside its
// (inset) image widget =====
#[allow(clippy::too_many_arguments)]
fn gutter_mark(
    md: Option<&SyntaxStyle>,
    line: &str,
    idx: usize,
    is_code: bool,
    widget_or_img: bool,
    table_none: bool,
    alert_line: Option<(Hsla, bool)>,
    ordered_nums: &[(u32, usize)],
    tab_indent: usize,
    base_font: &Font,
    base_font_size: Pixels,
    window: &mut Window,
) -> Option<(usize, LineMark)> {
    if let Some(st) = md.filter(|_| !is_code && table_none && widget_or_img) {
        if let Some(plen) = markdown_syntax::blockquote_prefix(line) {
            if let Some((kind, mlen, fold)) = markdown_syntax::alert_prefix(&line[plen..]) {
                // The alert's marker line: hide `> [!NOTE] ` (the scan
                // does the same) and paint a bold label in its place; a
                // same-line body insets past the label's measured width.
                let color = st.alert_color(kind);
                let label_font = Font {
                    weight: FontWeight::BOLD,
                    ..base_font.clone()
                };
                let label_w = measure_width(window, kind.label(), &label_font, base_font_size);
                // The icon (when the host supplies paths) sits before the
                // label; both shift the same-line body's inset.
                let icon_w = if st.alert_icons.is_some() {
                    base_font_size * (1. + 6. / 16.)
                } else {
                    px(0.)
                };
                // A foldable callout's chevron sits after the label and
                // pushes any same-line body further right.
                let chevron_x =
                    base_font_size * (QUOTE_INSET / 16.) + icon_w + label_w + base_font_size * 0.5;
                let chevron_w = if fold.is_some() {
                    measure_width(window, "▾", base_font, base_font_size) + base_font_size * 0.5
                } else {
                    px(0.)
                };
                Some((
                    plen + mlen,
                    LineMark::Alert {
                        bar: color,
                        label: kind.label(),
                        kind,
                        text_inset: chevron_x + chevron_w,
                        fold,
                        chevron_x,
                    },
                ))
            } else {
                // Continuation lines of an alert keep its colored bar with
                // normal body text; plain quotes are muted throughout.
                let (bar, text) = match alert_line {
                    Some((c, _)) => (c, None),
                    None => (st.quote, Some(st.quote)),
                };
                Some((
                    plen,
                    LineMark::Quote {
                        bar,
                        text,
                        text_inset: base_font_size * (QUOTE_INSET / 16.),
                    },
                ))
            }
        } else if let Some((plen, indent, checked)) = markdown_syntax::task_prefix(line) {
            // Reader-style geometry: each nesting level advances by
            // marker + gap + (spaces × 4.5), not the raw spaces' width.
            let depth = indent as f32 / tab_indent.max(1) as f32;
            let box_w = base_font_size * CHECKBOX_SCALE;
            let level = box_w
                + base_font_size * (LIST_TEXT_GAP / 16.)
                + base_font_size * (LIST_LEVEL_PER_SPACE / 16.) * tab_indent as f32;
            let bullet_x = level * depth;
            let text_inset = bullet_x + box_w + base_font_size * (LIST_TEXT_GAP / 16.);
            Some((
                plen,
                LineMark::Check {
                    bullet_x,
                    text_inset,
                    checked,
                    color: st.quote,
                    accent: st.link,
                },
            ))
        } else if let Some((plen, indent, ordered, _)) = markdown_syntax::list_prefix(line) {
            let depth = indent as f32 / tab_indent.max(1) as f32;
            let (num, level) = ordered_nums[idx];
            let glyph = if ordered {
                // Word-style depth markers (1. -> a. -> i.), shared with
                // the reader. The level is structural (from the
                // renumbering pass), not an indent-width guess.
                zorite_markdown::syntax::ordered_marker(level, num)
            } else {
                "\u{2022}".to_string()
            };
            let glyph_w = measure_width(window, &glyph, base_font, base_font_size);
            // The per-level step comes from a REPRESENTATIVE marker, not
            // this line's own glyph: sub-pixel width differences between
            // sibling numbers ("3." vs "4.") would nudge bullet_x apart
            // and the nesting guides would mistake siblings for ancestors
            // (drawing a guide through the numbers).
            let marker_w = if ordered {
                measure_width(window, "9.", base_font, base_font_size)
            } else {
                glyph_w
            };
            let step = marker_w.max(base_font_size * (7. / 16.))
                + base_font_size * (LIST_TEXT_GAP / 16.)
                + base_font_size * (LIST_LEVEL_PER_SPACE / 16.) * tab_indent as f32;
            let bullet_x = step * depth;
            let text_inset = bullet_x + glyph_w + base_font_size * (LIST_TEXT_GAP / 16.);
            Some((
                plen,
                LineMark::List {
                    bullet_x,
                    text_inset,
                    ordered,
                    num,
                    level,
                    color: st.quote,
                },
            ))
        } else {
            None
        }
    } else {
        None
    }
}

// ===== Fenced-code line runs: monospace; ``` delimiters dimmed; a highlighted
// line (from the host's code highlighter) splits into token-colored runs
// with the base code color filling the gaps. `top` = first visible line of
// the block (rounds the card's top corners); the box width is back-patched
// once the block's extent is known =====
fn shape_code_line(
    st: &SyntaxStyle,
    line: &str,
    idx: usize,
    top: bool,
    is_fence: bool,
    line_highlights: &std::collections::HashMap<usize, Vec<(Range<usize>, HighlightStyle)>>,
) -> (SharedString, std::rc::Rc<Vec<TextRun>>, CodeBg) {
    // Monospace runs; ``` delimiters dimmed. A highlighted line (from
    // the host's code highlighter) splits into token-colored runs with
    // the base code color filling the gaps.
    let base_run = |len: usize| TextRun {
        len,
        font: st.mono.clone(),
        color: if is_fence { st.marker } else { st.code },
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let runs = if line.is_empty() {
        Vec::new()
    } else if let Some(tokens) = line_highlights.get(&idx).filter(|_| !is_fence) {
        let mut runs = Vec::new();
        let mut pos = 0;
        for (r, h) in tokens {
            if r.start > pos {
                runs.push(base_run(r.start - pos));
            }
            let font = Font {
                weight: h.font_weight.unwrap_or(st.mono.weight),
                style: h.font_style.unwrap_or(st.mono.style),
                ..st.mono.clone()
            };
            runs.push(TextRun {
                len: r.end - r.start,
                font,
                color: h.color.unwrap_or(st.code),
                background_color: None,
                underline: None,
                strikethrough: None,
            });
            pos = r.end;
        }
        if pos < line.len() {
            runs.push(base_run(line.len() - pos));
        }
        runs
    } else {
        vec![base_run(line.len())]
    };
    (
        SharedString::from(line.to_string()),
        std::rc::Rc::new(runs),
        CodeBg {
            color: st.code_bg,
            width: px(0.), // back-patched to the block's widest line
            top,
            bottom: false,
        },
    )
}

// ===== Prose lines (markers hidden, inline math + inline images) =====
/// Immutable per-pass inputs for the hidden-markers prose-line shaper — the
/// shaping caches + the raster providers + the windowed-shaping band.
struct MarkdownLineShaper<'a> {
    caches: &'a ShapeCaches,
    run_epoch: u64,
    /// The caret's logical line always shapes for real (even unfocused), so
    /// off-screen go-to-line keeps exact caret geometry (see `geometry_row`).
    geometry_row: usize,
    /// The quantized viewport band: off-screen plain lines may skip shaping
    /// when their height is cached (invariant: empty placeholder + preserved
    /// line height; the caret's line and inline-math/image lines never skip).
    band: Option<(Pixels, Pixels)>,
    base_font: &'a Font,
    block_math: Option<&'a BlockMathFn>,
    block_math_em: Option<f32>,
    block_image: Option<&'a BlockImageFn>,
}
impl MarkdownLineShaper<'_> {
    /// Shape one WYSIWYG prose line: markers hidden (except the caret's
    /// construct), with the cross-frame run cache and the windowed off-screen
    /// skip. Returns `None` when the line was windowed out — its placeholder
    /// is already pushed and the caller continues to the next line.
    #[allow(clippy::too_many_arguments)]
    fn shape(
        &self,
        out: &mut ShapedDoc,
        window: &mut Window,
        line: &str,
        idx: usize,
        line_start: usize,
        st: &SyntaxStyle,
        line_base: Hsla,
        line_diags: &[Diagnostic],
        caret_col: Option<usize>,
        reveal_prefix: usize,
        hide_prefix: usize,
        reveal_inline: bool,
        mark: Option<LineMark>,
        fs: Pixels,
        wrap_width: Option<Pixels>,
        base_font_size: Pixels,
        y_acc: Pixels,
        collapse_fence: bool,
        collapse_marker: bool,
    ) -> Option<(
        SharedString,
        std::rc::Rc<Vec<TextRun>>,
        Option<std::rc::Rc<Vec<usize>>>,
        Vec<InlineMath>,
        Option<u64>,
    )> {
        let mut line_inline_math: Vec<InlineMath> = Vec::new();
        // Markers hidden (except the caret's construct): shape the display
        // string + keep a map back to source.
        // Cross-frame cache: most repaints (caret blink, hover, another
        // editor's edit) rebuild identical lines — reuse the built display
        // string + runs when the line and its reveal state are unchanged.
        // CONTENT part of the key, memoized per row for the current
        // (generation, epoch) — steady-state frames skip rehashing the
        // line's bytes. The caret/reveal parts vary per frame and are
        // cheap (a hash of four small values).
        let ckey = {
            let cached = self.caches.row_keys.borrow().1.get(idx).copied().flatten();
            match cached {
                Some(k) => k,
                None => {
                    use std::hash::{Hash, Hasher};
                    let mut h = std::collections::hash_map::DefaultHasher::new();
                    line.hash(&mut h);
                    line_base.a.to_bits().hash(&mut h);
                    line_base.h.to_bits().hash(&mut h);
                    line_base.s.to_bits().hash(&mut h);
                    line_base.l.to_bits().hash(&mut h);
                    for d in line_diags {
                        d.range.start.hash(&mut h);
                        d.range.end.hash(&mut h);
                    }
                    self.run_epoch.hash(&mut h);
                    let k = h.finish();
                    if let Some(slot) = self.caches.row_keys.borrow_mut().1.get_mut(idx) {
                        *slot = Some(k);
                    }
                    k
                }
            }
        };
        let key = {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            ckey.hash(&mut h);
            caret_col.hash(&mut h);
            reveal_prefix.hash(&mut h);
            hide_prefix.hash(&mut h);
            reveal_inline.hash(&mut h);
            h.finish()
        };
        // Windowed shaping: an offscreen plain text line whose height is
        // already cached skips run-building and shaping entirely — the
        // cached (row height, wrap rows) keep the layout exact, and its
        // placeholder WrappedLine is never painted (paint culls the same
        // band). Never skipped: lines with inline math/images (height
        // depends on async raster providers, not just the text) and
        // collapsed lines (fold state isn't in the key).
        let line_wrap_plain = match mark {
            Some(m) => wrap_width.map(|w| (w - m.inset()).max(px(0.))),
            None => wrap_width,
        };
        let skip_key = (idx != self.geometry_row
            && !line.contains('$')
            && !line.contains("![")
            && !collapse_fence
            && !collapse_marker)
            .then(|| {
                use std::hash::{Hash, Hasher};
                let mut h = std::collections::hash_map::DefaultHasher::new();
                key.hash(&mut h);
                f32::from(fs).to_bits().hash(&mut h);
                line_wrap_plain
                    .map(f32::from)
                    .unwrap_or(-1.)
                    .to_bits()
                    .hash(&mut h);
                h.finish()
            });
        if let (Some((lo, hi)), Some(hk)) = (self.band, skip_key) {
            let hit = self.caches.line_heights.borrow().get(&hk).copied();
            if let Some((lh_c, wr_c)) = hit {
                let total = lh_c * wr_c as f32;
                if y_acc > hi || y_acc + total < lo {
                    // The REAL mark: paint's nesting-guide bookkeeping
                    // walks offscreen ancestors' marks.
                    out.push_placeholder(
                        window,
                        base_font_size,
                        wrap_width,
                        lh_c,
                        None,
                        mark,
                        wr_c,
                    );
                    return None;
                }
            }
        }
        let cached = self
            .caches
            .line_runs
            .borrow()
            .get(&key)
            .filter(|c| c.src == line && c.line_base == line_base)
            .map(|c| (c.disp.clone(), c.runs.clone(), c.map.clone()));
        let (disp, runs, m) = match cached {
            Some(v) => v,
            None => {
                let (disp, runs, m) = markdown_syntax::hidden_runs(
                    line,
                    self.base_font,
                    line_base,
                    line_diags,
                    caret_col,
                    reveal_prefix,
                    hide_prefix,
                    reveal_inline,
                    st,
                );
                let v = (
                    SharedString::from(disp),
                    std::rc::Rc::new(runs),
                    std::rc::Rc::new(m),
                );
                self.caches.line_runs.borrow_mut().insert(
                    key,
                    CachedLineRuns {
                        src: line.to_string(),
                        line_base,
                        disp: v.0.clone(),
                        runs: v.1.clone(),
                        map: v.2.clone(),
                    },
                );
                v
            }
        };
        // A checked task's body renders struck through + muted (the reader
        // does the same) — a whole-line restyle over the finished runs.
        let (disp, runs, m) = if matches!(mark, Some(LineMark::Check { checked: true, .. })) {
            let runs = std::rc::Rc::new(
                runs.iter()
                    .cloned()
                    .map(|mut r| {
                        r.strikethrough = Some(gpui_kit::StrikethroughStyle {
                            thickness: px(1.0),
                            color: None,
                        });
                        r.color = st.quote;
                        r
                    })
                    .collect(),
            );
            (disp, runs, m)
        } else {
            (disp, runs, m)
        };
        // Inline `$…$` math AND `![](src)` images: swap each ready one's
        // glyphs for a spacer to paint the raster over (shared machinery).
        // Span checks gate the calls, so span-less lines (the common case)
        // keep their shared payloads untouched.
        let (disp, runs, m) = match (self.block_math, self.block_math_em) {
            (Some(mathf), Some(em)) if !markdown_syntax::inline_math_spans(line).is_empty() => {
                let (disp, runs, m, im) = shape_inline_math(
                    window,
                    line,
                    line_start,
                    disp.to_string(),
                    runs.as_ref().clone(),
                    m.as_ref().clone(),
                    caret_col,
                    self.base_font,
                    fs,
                    mathf,
                    em,
                );
                line_inline_math = im;
                (
                    SharedString::from(disp),
                    std::rc::Rc::new(runs),
                    std::rc::Rc::new(m),
                )
            }
            _ => (disp, runs, m),
        };
        let (text, runs, map) = match self.block_image {
            Some(imgf) if !markdown_syntax::inline_image_spans(line).is_empty() => {
                let (disp, runs, m, imgs) = shape_inline_images(
                    window,
                    line,
                    line_start,
                    disp.to_string(),
                    runs.as_ref().clone(),
                    m.as_ref().clone(),
                    caret_col,
                    self.base_font,
                    fs,
                    imgf,
                );
                line_inline_math.extend(imgs);
                (
                    SharedString::from(disp),
                    std::rc::Rc::new(runs),
                    Some(std::rc::Rc::new(m)),
                )
            }
            _ => (disp, runs, Some(m)),
        };
        Some((text, runs, map, line_inline_math, skip_key))
    }
}

// ===== Per-line assembly helpers =====
/// A shaped line's row height: collapsed fence/marker lines are zero; a table
/// row is the fixed cell height plus per-cell wrap growth; a widget line takes
/// its widget height; a text row grows to its tallest inline `$…$` formula and
/// list items breathe (LIST_ROW_GAP) like the reader.
#[allow(clippy::too_many_arguments)]
fn shaped_line_height(
    window: &mut Window,
    caches: &ShapeCaches,
    run_epoch: u64,
    base_font: &Font,
    base_font_size: Pixels,
    table_row_h: Pixels,
    collapse_fence: bool,
    collapse_marker: bool,
    table: Option<&TableRow>,
    widget: Option<&Block>,
    line_inline_math: &[InlineMath],
    line: &str,
    fs: Pixels,
    styled: bool,
) -> Pixels {
    if collapse_fence || collapse_marker {
        px(0.)
    } else {
        match table {
            // The `|---|` separator collapses in grid mode — the old
            // renderer doesn't show it; the first body row's top divider
            // becomes the header/body border.
            Some(t) if t.is_separator => px(0.),
            // A drag-narrowed column wraps its cells — the row grows to
            // the tallest cell's wrap rows (memoized: this shaped every
            // cell per row per frame).
            Some(t) => {
                let row_key = {
                    use std::hash::{Hash, Hasher};
                    let mut h = std::collections::hash_map::DefaultHasher::new();
                    line.hash(&mut h);
                    for w in &t.col_widths {
                        f32::from(*w).to_bits().hash(&mut h);
                    }
                    t.is_header.hash(&mut h);
                    run_epoch.hash(&mut h);
                    h.finish()
                };
                let hit = caches.cell_rows.borrow().get(&row_key).copied();
                let rows = match hit {
                    Some(r) => r,
                    None => {
                        let r = table_row_wrap_rows(
                            window,
                            &t.cells,
                            &t.cell_runs,
                            &t.col_widths,
                            t.is_header,
                            base_font,
                            base_font_size,
                        );
                        caches.cell_rows.borrow_mut().insert(row_key, r);
                        r
                    }
                };
                table_row_h + base_font_size * LINE_HEIGHT_RATIO * (rows - 1) as f32
            }
            None => match widget {
                // Reserve a little space around an inline image so a list
                // of photos doesn't stack edge-to-edge.
                Some(Block::Image(i)) => i.height + px(IMG_ROW_PAD),
                Some(b) => b.height(),
                // A text row grows to fit its tallest inline `$…$` formula (a fraction
                // is taller than the text), so the formula doesn't overlap neighbours.
                None => {
                    let math_h = line_inline_math
                        .iter()
                        .map(|im| im.height)
                        .max()
                        .unwrap_or(px(0.));
                    let mut h = (fs * LINE_HEIGHT_RATIO).max(math_h + px(INLINE_MATH_ROW_PAD));
                    // List items breathe like the reader's (LIST_ROW_GAP).
                    if styled && markdown_syntax::list_prefix(line).is_some() {
                        h += base_font_size * (LIST_ROW_GAP / 16.);
                    }
                    h
                }
            },
        }
    }
}

/// #66: an RTL line can't use gpui's wrapping. gpui slices the
/// already-reordered glyph run by ascending x, so its rows come out
/// in reverse reading order, and its break candidates come from an
/// `is_word_char` that doesn't know Arabic — so words split down the
/// middle. Lay the line out in LOGICAL order instead. This has to
/// happen here, not later with the rest of the RTL geometry: our
/// break points give a different row COUNT, and the row span and
/// height are decided right here.
/// A blank line has no strong character, so it has no direction of
/// its own — it inherits the paragraph it sits in. Without this the
/// caret jumps to the LEFT margin the moment you press Enter after a
/// Persian paragraph, and a selection running through the blank line
/// puts its marker on the wrong side.
/// A line with no strong character of its own — blank, or nothing
/// but markers like `> [!NOTE]` — inherits the paragraph it sits in.
/// Otherwise a Persian callout's title sat on the opposite side from
/// its body, and pressing Enter after a Persian paragraph dropped the
/// caret on the left margin.
///
/// ponytail: it inherits from ABOVE, so an alert whose title follows
/// English text but whose body is Persian still splits. Looking ahead
/// to the body would fix it; no one has hit that yet.
/// Lay out any line CONTAINING right-to-left text, not just one that
/// reads that way: an English sentence quoting a Persian name still
/// needs the mapping, or the caret misplaces inside that run. Such a
/// line stays left-aligned (`line_rtl` drives the shift, this drives
/// the mapping).
///
/// ponytail: word breaking is space-based, so a line mixing RTL with
/// unspaced CJK would wrap poorly. Lines without any RTL keep gpui's
/// wrapping, so plain CJK is untouched.
#[allow(clippy::too_many_arguments)]
fn line_rtl_rows(
    window: &mut Window,
    line: &str,
    idx: usize,
    lines: &[&str],
    last_strong_rtl: &mut bool,
    bg: Option<CodeBg>,
    widget: Option<&Block>,
    table: Option<&TableRow>,
    shaped_text: &SharedString,
    runs: &[TextRun],
    line_wrap: Option<Pixels>,
    fs: Pixels,
    wl: &WrappedLine,
) -> (bool, Option<Vec<crate::bidi::paragraph::Row>>, usize) {
    let line_rtl = match zorite_markdown::syntax::content_direction_opt(line) {
        Some(d) => {
            *last_strong_rtl = d.is_rtl();
            d.is_rtl()
        }
        // Nothing but markers (`> [!NOTE]`, a bare `- `): its content is
        // the line BELOW, so take that one's direction — a callout's
        // title has to land on the same side as its body. Only within
        // the block: a blank line ends it, and there the line above is
        // what matters (pressing Enter after a Persian paragraph should
        // keep the caret on the right).
        None if !line.trim().is_empty() => lines
            .get(idx + 1)
            .filter(|next| !next.trim().is_empty())
            .and_then(|next| zorite_markdown::syntax::content_direction_opt(next))
            .map_or(*last_strong_rtl, |d| d.is_rtl()),
        None => *last_strong_rtl,
    };
    // Lay out any line CONTAINING right-to-left text, not just one that
    // reads that way: an English sentence quoting a Persian name still
    // needs the mapping, or the caret misplaces inside that run. Such a
    // line stays left-aligned (`line_rtl` drives the shift, this drives
    // the mapping).
    //
    // ponytail: word breaking is space-based, so a line mixing RTL with
    // unspaced CJK would wrap poorly. Lines without any RTL keep gpui's
    // wrapping, so plain CJK is untouched.
    let has_rtl = line_rtl || zorite_markdown::syntax::contains_rtl(line);
    let rtl_rows = (bg.is_none() && widget.is_none() && table.is_none() && has_rtl)
        .then(|| crate::bidi::paragraph::layout_rows(shaped_text, runs, line_wrap, fs, window))
        .filter(|rows| !rows.is_empty());
    let span = rtl_rows
        .as_ref()
        .map_or(wl.wrap_boundaries().len() + 1, Vec::len);
    (line_rtl, rtl_rows, span)
}

// ===== Inline `$…$` math + `![](src)` image spacers =====
/// Splice inline `$…$` spacers into one line's shaped output. For each formula whose raster is
/// ready (`block_math`) and that the caret isn't inside (left raw for editing), reserve a spacer
/// of whole spaces ≥ the raster's text-em width and record where to paint it. The raster is
/// rasterized at `em`; scaling by `fs/em` puts it at this line's text size. Returns the
/// (possibly unchanged) display/runs/map plus the line's formula placements.
#[allow(clippy::too_many_arguments)]
fn shape_inline_math(
    window: &mut Window,
    line: &str,
    line_start: usize,
    disp: String,
    runs: Vec<TextRun>,
    map: Vec<usize>,
    caret_col: Option<usize>,
    base_font: &Font,
    fs: Pixels,
    block_math: &BlockMathFn,
    em: f32,
) -> (String, Vec<TextRun>, Vec<usize>, Vec<InlineMath>) {
    let spans = markdown_syntax::inline_math_spans(line);
    if spans.is_empty() || em <= 0. {
        return (disp, runs, map, Vec::new());
    }
    let space_w = f32::from(measure_width(window, "\u{a0}", base_font, fs)).max(1.);
    let scale = f32::from(fs) / em;
    let mut formulas: Vec<(Range<usize>, usize)> = Vec::new();
    let mut imgs: Vec<(Arc<RenderImage>, Pixels, Pixels, SharedString)> = Vec::new();
    for span in spans {
        // A caret STRICTLY inside the span keeps it raw (a fallback — normally arrowing/clicking
        // into a formula opens its structural editor before the caret lands here). A caret AT a
        // boundary (just before/after the `$…$`, e.g. after exiting the editor) leaves it
        // rendered, so sitting beside a formula doesn't flip it to raw.
        if caret_col.is_some_and(|c| span.start < c && c < span.end) {
            continue;
        }
        let latex = markdown_syntax::inline_math_latex(line, &span);
        let Some((img, lw, lh)) = block_math(latex) else {
            continue; // not yet rasterized — leave the raw source until it lands
        };
        if lw <= 0. || lh <= 0. {
            continue;
        }
        // The provider's logical size, scaled from the typeset em down to text
        // size — no window-scale-factor division (see [`BlockMathFn`]).
        let (w, h) = (lw * scale, lh * scale);
        let n = ((w / space_w).ceil() as usize).max(1);
        let latex: SharedString = latex.to_string().into();
        formulas.push((span, n));
        imgs.push((img, px(w), px(h), latex));
    }
    if formulas.is_empty() {
        return (disp, runs, map, Vec::new());
    }
    let gap = TextRun {
        len: 0,
        font: base_font.clone(),
        color: Hsla::default(),
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let (nd, nr, nm, places) =
        markdown_syntax::splice_inline_math(&disp, &runs, &map, &formulas, &gap);
    debug_assert_eq!(places.len(), imgs.len());
    let inline = places
        .into_iter()
        .zip(imgs)
        .map(|(p, (img, width, height, latex))| InlineMath {
            display_off: p.display_off,
            len: p.len,
            // Absolute byte range in the document, for hit-test / seating / commit.
            source: line_start + p.source.start..line_start + p.source.end,
            latex,
            img,
            width,
            height,
        })
        .collect();
    (nd, nr, nm, inline)
}

/// Inline `![](src)` images: swap each ready image's glyphs for a spacer to
/// paint the raster over, reusing the inline-math machinery — the returned
/// [`InlineMath`] entries carry an empty `latex` (so the click-to-edit path
/// treats them as images, not formulas). A caret strictly inside an image's
/// `![…](…)` leaves it raw for editing. Sizing matches the reader: ~40px tall,
/// capped 240px wide, aspect from the raster's pixels.
#[allow(clippy::too_many_arguments)]
fn shape_inline_images(
    window: &mut Window,
    line: &str,
    line_start: usize,
    disp: String,
    runs: Vec<TextRun>,
    map: Vec<usize>,
    caret_col: Option<usize>,
    base_font: &Font,
    fs: Pixels,
    block_image: &BlockImageFn,
) -> (String, Vec<TextRun>, Vec<usize>, Vec<InlineMath>) {
    let spans = markdown_syntax::inline_image_spans(line);
    if spans.is_empty() {
        return (disp, runs, map, Vec::new());
    }
    let space_w = f32::from(measure_width(window, "\u{a0}", base_font, fs)).max(1.);
    let mut places: Vec<(Range<usize>, usize)> = Vec::new();
    let mut imgs: Vec<(Arc<RenderImage>, Pixels, Pixels)> = Vec::new();
    for (full, src) in spans {
        if caret_col.is_some_and(|c| full.start < c && c < full.end) {
            continue; // editing the raw `![](src)`
        }
        let Some(img) = block_image(&line[src]) else {
            continue; // remote / PDF / not-yet-decoded → leave raw
        };
        let sz = img.size(0);
        let (pw, ph) = (sz.width.0 as f32, sz.height.0 as f32);
        if pw <= 0. || ph <= 0. {
            continue;
        }
        let mut h = 40.0_f32;
        let mut w = h * pw / ph;
        if w > 240. {
            let s = 240. / w;
            w *= s;
            h *= s;
        }
        let n = ((w / space_w).ceil() as usize).max(1);
        places.push((full, n));
        imgs.push((img, px(w), px(h)));
    }
    if places.is_empty() {
        return (disp, runs, map, Vec::new());
    }
    let gap = TextRun {
        len: 0,
        font: base_font.clone(),
        color: Hsla::default(),
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let (nd, nr, nm, placed) =
        markdown_syntax::splice_inline_math(&disp, &runs, &map, &places, &gap);
    let inline = placed
        .into_iter()
        .zip(imgs)
        .map(|(p, (img, width, height))| InlineMath {
            display_off: p.display_off,
            len: p.len,
            source: line_start + p.source.start..line_start + p.source.end,
            latex: SharedString::default(), // empty = image, not a formula
            img,
            width,
            height,
        })
        .collect();
    (nd, nr, nm, inline)
}
