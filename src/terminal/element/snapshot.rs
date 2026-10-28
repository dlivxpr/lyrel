//! Acquire one grid snapshot and resolve its cells, search and link annotations.
//! The pane retains the buffer; a contended steady-state read leaves it untouched.

use gpui_kit::{App, Rgba};
use rio_vt::ansi::CursorShape;
use rio_vt::config::colors::ColorRgb;
use rio_vt::crosswords::grid::Dimensions as _;
use rio_vt::crosswords::pos::{Column, Line, Pos};
use rio_vt::crosswords::square::{Square, Wide};
use rio_vt::crosswords::style::StyleFlags;
use rio_vt::crosswords::{Crosswords, Mode};
use rio_vt::selection::SelectionRange;

use super::{GridCursor, GridSnapshot, PaintColors, RenderCell, TerminalElement, UnderlineKind, osc8_underline};
use super::colors::{dim_cell, dim_fg, resolve, to_hsla};
use crate::core::config::Config;
use super::decorations::{cursor_style_from_shape, link_underline_color};

// A viewport owns its palette and theme for one locked grid snapshot. Reuse
// only the preceding resolved style; character extras and selection remain
// per-cell, and no cache survives a frame or a palette/theme change.
pub(super) struct CellPainter<'a, T: rio_vt::event::EventListener> {
    term: &'a Crosswords<T>,
    palette: &'a [ColorRgb; 256],
    colors: &'a PaintColors,
    selection: Option<&'a SelectionRange>,
    style: Option<(rio_vt::crosswords::style::StyleId, RenderCell)>,
}

impl<'a, T: rio_vt::event::EventListener> CellPainter<'a, T> {
    pub(super) fn new(
        term: &'a Crosswords<T>,
        palette: &'a [ColorRgb; 256],
        colors: &'a PaintColors,
        selection: Option<&'a SelectionRange>,
    ) -> Self {
        Self {
            term,
            palette,
            colors,
            selection,
            style: None,
        }
    }

    #[cfg(test)]
    pub(super) fn at(&mut self, position: Pos) -> RenderCell {
        self.snapshot(position, self.term.grid[position])
    }

    #[inline]
    fn snapshot(&mut self, position: Pos, cell: Square) -> RenderCell {
        let term = self.term;
        if matches!(cell.wide(), Wide::Spacer | Wide::LeadingSpacer) {
            return RenderCell {
                spacer: true,
                ..RenderCell::default()
            };
        }
        let style_id = cell.style_id_checked();
        let mut rendered = if let Some(id) = style_id
            && let Some((previous, rendered)) = &self.style
            && *previous == id
        {
            rendered.clone()
        } else {
            let style = term.grid.style_of(&cell);
            let palette = self.palette;
            let colors = self.colors;
            let flags = style.flags;
            let (mut foreground, _) = resolve(style.fg, palette, colors.fg_rgb, colors.bg_rgb);
            let (background, background_default) =
                resolve(style.bg, palette, colors.fg_rgb, colors.bg_rgb);
            let (foreground, background, draw_bg) = if flags.contains(StyleFlags::INVERSE) {
                (background, foreground, true)
            } else {
                if flags.contains(StyleFlags::HIDDEN) {
                    foreground = background;
                }
                (foreground, background, !background_default)
            };
            let mut rendered = RenderCell {
                fg: to_hsla(foreground),
                bg: to_hsla(background),
                draw_bg,
                bold: flags.contains(StyleFlags::BOLD),
                italic: flags.contains(StyleFlags::ITALIC),
                strikeout: flags.contains(StyleFlags::STRIKEOUT),
                underline: if flags.contains(StyleFlags::DOUBLE_UNDERLINE) {
                    UnderlineKind::Double
                } else if flags.contains(StyleFlags::UNDERCURL) {
                    UnderlineKind::Curly
                } else if flags.contains(StyleFlags::DOTTED_UNDERLINE) {
                    UnderlineKind::Dotted
                } else if flags.contains(StyleFlags::DASHED_UNDERLINE) {
                    UnderlineKind::Dashed
                } else if flags.contains(StyleFlags::UNDERLINE) {
                    UnderlineKind::Single
                } else {
                    UnderlineKind::None
                },
                underline_color: style
                    .underline_color
                    .map(|color| to_hsla(resolve(color, palette, colors.fg_rgb, colors.bg_rgb).0)),
                ..RenderCell::default()
            };
            if flags.contains(StyleFlags::DIM) {
                rendered.fg = dim_fg(rendered.fg, background, colors.legible_dim);
            }
            self.style = style_id.map(|id| (id, rendered.clone()));
            rendered
        };
        // Background-only squares reuse the packed extras bits for their colour.
        let extras = cell
            .extras_id_checked()
            .and_then(|id| term.grid.extras_table.get(id));
        rendered.c = cell.c();
        rendered.marks = extras
            .map(|extra| extra.zerowidth.as_slice())
            .filter(|marks| !marks.is_empty())
            .map(Box::from);
        // Placeholder identity is terminal data, not a font glyph. Even a
        // missing/deleted prototype leaves its cell background but no PUA ink.
        if rendered.c == '\u{10eeee}' {
            rendered.c = ' ';
            rendered.marks = None;
            rendered.underline = UnderlineKind::None;
            rendered.strikeout = false;
        }
        rendered.selected = self
            .selection
            .is_some_and(|selection| selection.contains(position));
        osc8_underline::mark(
            &mut rendered,
            extras.is_some_and(|extra| extra.hyperlink.is_some()),
        );
        rendered
    }
}

impl TerminalElement {
    /// The shape `prompt_cursor_style` puts on the grid cursor while the shell
    /// waits at a prompt with its own line editor — `prompt_editor` off, or a
    /// line handed back to ZLE/readline. Shell integration has no mark for "a
    /// prompt plugin set this shape", so the one it does report, vi mode, keeps
    /// the shell's own insert/normal shapes.
    pub(in crate::terminal) fn prompt_cursor_shape(&self, cx: &App) -> Option<crate::core::config::CursorStyle> {
        let shape = cx.global::<Config>().prompt_cursor_style.shape()?;
        let terminal = &self.view.read(cx).terminal;
        (terminal.at_prompt() && !terminal.shell_vi_mode()).then_some(shape)
    }

    pub(in crate::terminal) fn build_grid(
        &self,
        colors: &PaintColors,
        buf: &mut Vec<RenderCell>,
        rows: usize,
        cols: usize,
        want_sliver: bool,
        cx: &App,
        dim: f32,
        under: Rgba,
        must_block: bool,
    ) -> Option<GridSnapshot> {
        let mut cursor: Option<GridCursor> = None;
        let mut sliver: Option<Vec<RenderCell>> = None;
        let mut any_selected = false;
        let display_offset;
        let history_size;
        let mut placeholders = Vec::new();
        let prompt_shape = self.prompt_cursor_shape(cx);
        {
            let mut palette = self.view.read(cx).terminal.palette;
            if let Some(active) = cx.try_global::<crate::terminal::palette::ActivePalette>() {
                palette[..16].copy_from_slice(&active.ansi16);
            }
            let term = self.view.read(cx).terminal.term.clone();
            // Rendering does not queue for the grid lock. Holding it is the
            // pane's own reader, part-way through feeding a batch of output
            // into the emulator — and one UI thread draws every pane in every
            // window, so waiting here wires one pane's write speed to the frame
            // rate of the whole window. That is the same illness as #709, which
            // was this thread parked in `write(2)` for a stalled link; this is
            // the read side of it. A frame that cannot have the lock paints the
            // one before it, and nobody can see a frame of lag.
            //
            // `try_lock_unfair` rather than a lease: a painter that queued
            // would make the reader wait for a frame it is not going to get
            // anyway. Skipping the queue is safe precisely because it never
            // waits.
            let term = match term.try_lock() {
                Some(term) => term,
                // The two frames that have to have it: the first one, with no
                // previous grid to fall back on, and the one after a resize,
                // where the previous grid is the wrong shape. Both are rare and
                // neither is in the steady state.
                None if must_block => term.lock(),
                None => return None,
            };
            // After the lock, not before: an early return must leave the
            // previous frame's cells intact for the caller to paint again.
            buf.resize_with(rows * cols, RenderCell::default);
            display_offset = term.display_offset() as i32;
            history_size = term.grid.history_size();
            let selection = term
                .selection
                .as_ref()
                .and_then(|selection| selection.to_range(&term));

            let mut cur = term.cursor();
            if let Some(position) = self.view.read(cx).terminal.paint_cursor_position() {
                cur.pos = position;
            }
            let cursor_row = cur.pos.row.0 + display_offset;
            let cursor_hidden = matches!(cur.content, CursorShape::Hidden);
            // TUIs that hide the hardware cursor (Kimi CLI, Ink apps) draw
            // their own caret as a reverse-video cell and park the real
            // cursor wherever the frame's last write ended, which strands
            // the IME candidate window there (#275). A lone short inverse
            // run on the cursor's row is that fake caret; collect runs so
            // the IME anchor can snap to it.
            let mut inverse_runs: Vec<(usize, usize)> = Vec::new();
            let mut painter = CellPainter::new(&term, &palette, colors, selection.as_ref());

            let rendered_rows = rows.min(term.screen_lines());
            let rendered_cols = cols.min(term.columns());
            for row in 0..rendered_rows {
                let line = Line(row as i32 - display_offset);
                let source = &term.grid[line][..];
                let slices = lyrel_core::core::kitty_graphics::placeholder_row(&term.grid, line);
                if !slices.is_empty() {
                    placeholders.push((row as i32, slices));
                }
                let source_cols = rendered_cols.min(source.len());
                for (col, cell) in source.iter().copied().take(source_cols).enumerate() {
                    let position = Pos::new(line, Column(col));
                    if cursor_hidden
                        && row as i32 == cursor_row
                        && term
                            .grid
                            .style_of(&cell)
                            .flags
                            .contains(StyleFlags::INVERSE)
                        && !matches!(cell.wide(), Wide::Spacer | Wide::LeadingSpacer)
                    {
                        match inverse_runs.last_mut() {
                            Some((start, len)) if *start + *len == col => *len += 1,
                            _ => inverse_runs.push((col, 1)),
                        }
                    }
                    let rendered = painter.snapshot(position, cell);
                    any_selected |= rendered.selected;
                    buf[row * cols + col] = if dim < 1. {
                        dim_cell(rendered, dim, under)
                    } else {
                        rendered
                    };
                }
                // Every covered cell was replaced above, including its old
                // marks and flags. Clear only padding outside the source grid.
                buf[row * cols + source_cols..(row + 1) * cols].fill_with(RenderCell::default);
            }
            buf[rendered_rows * cols..].fill_with(RenderCell::default);

            if want_sliver && (display_offset as usize) < term.grid.history_size() {
                let line = Line(-display_offset - 1);
                let slices = lyrel_core::core::kitty_graphics::placeholder_row(&term.grid, line);
                if !slices.is_empty() {
                    placeholders.push((-1, slices));
                }
                let mut row_buf = vec![RenderCell::default(); cols];
                for (col, (slot, &cell)) in row_buf
                    .iter_mut()
                    .zip(term.grid[line][..].iter())
                    .take(rendered_cols)
                    .enumerate()
                {
                    let position = Pos::new(line, Column(col));
                    let mut rc = painter.snapshot(position, cell);
                    if dim < 1. {
                        rc = dim_cell(rc, dim, under);
                    }
                    any_selected |= rc.selected;
                    *slot = rc;
                }
                sliver = Some(row_buf);
            }

            let row = cursor_row;
            let col = cur.pos.col.0;
            if row >= 0 && (row as usize) < rows && col < cols {
                // Snap the IME anchor to the fake caret when there is
                // exactly one caret-sized inverse run on the row.
                let ime_col = match inverse_runs.as_slice() {
                    [(start, len)] if cursor_hidden && *len <= 2 => *start,
                    _ => col,
                };
                cursor = Some(GridCursor {
                    row: row as usize,
                    col,
                    ime_col,
                    hidden: cursor_hidden,
                    // Checked under the grid lock: `on_alt_screen` would take
                    // it a second time.
                    style: prompt_shape
                        .filter(|_| !term.mode().contains(Mode::ALT_SCREEN))
                        .unwrap_or_else(|| cursor_style_from_shape(cur.content)),
                });
            }
        }

        let (any_match, any_current) =
            self.flag_search_matches(buf, rows, cols, display_offset, cx);
        self.flag_hovered_link(buf, rows, cols, display_offset, cx);
        Some(GridSnapshot {
            cursor,
            sliver,
            any_selected,
            any_match,
            any_current,
            display_offset,
            history_size,
            placeholders,
        })
    }

    fn flag_hovered_link(
        &self,
        buf: &mut [RenderCell],
        rows: usize,
        cols: usize,
        display_offset: i32,
        cx: &App,
    ) {
        let Some(link) = self.view.read(cx).links.hovered() else {
            return;
        };
        for &(start, end) in link.runs() {
            let grid_row = start.row.0 + display_offset;
            if grid_row < 0 || grid_row as usize >= rows {
                continue;
            }
            let row = grid_row as usize * cols;
            for col in start.col.0..=end.col.0.min(cols.saturating_sub(1)) {
                let cell = &mut buf[row + col];
                osc8_underline::unmark(cell);
                cell.link_hover = true;
                cell.underline_color = link_underline_color(cell, link.is_armed());
            }
        }
    }

    fn flag_search_matches(
        &self,
        buf: &mut [RenderCell],
        rows: usize,
        cols: usize,
        display_offset: i32,
        cx: &App,
    ) -> (bool, bool) {
        let Some(search) = self.view.read(cx).search.as_ref() else {
            return (false, false);
        };
        let (mut any_hit, mut any_current) = (false, false);
        let first = search
            .matches
            .partition_point(|m| m.end().row.0 + display_offset < 0);
        for (i, m) in search.matches.iter().enumerate().skip(first) {
            let is_current = search.current_index == Some(i);
            let start = *m.start();
            let end = *m.end();
            if start.row.0 + display_offset >= rows as i32 {
                break;
            }
            if is_current {
                any_current = true;
            } else {
                any_hit = true;
            }
            let mut line = start.row.0;
            while line <= end.row.0 {
                let row = line + display_offset;
                if row >= 0 && (row as usize) < rows {
                    let col_start = if line == start.row.0 { start.col.0 } else { 0 };
                    let col_end = if line == end.row.0 {
                        end.col.0
                    } else {
                        cols.saturating_sub(1)
                    };
                    let mut col = col_start;
                    while col <= col_end && col < cols {
                        let rc = &mut buf[row as usize * cols + col];
                        if is_current {
                            rc.match_current = true;
                        } else {
                            rc.match_hit = true;
                        }
                        col += 1;
                    }
                }
                line += 1;
            }
        }
        (any_hit, any_current)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::test_support::{painted_term, test_colors};
    use super::super::is_blank;
    use gpui_kit::component::ActiveTheme as _;

    #[test]
    fn selected_cells_keep_their_own_colors_for_the_translucent_wash() {
        let mut palette = [ColorRgb { r: 0, g: 0, b: 0 }; 256];
        palette[1] = ColorRgb {
            r: 0xcc,
            g: 0x22,
            b: 0x22,
        };
        palette[2] = ColorRgb {
            r: 0x22,
            g: 0x88,
            b: 0x22,
        };
        let colors = test_colors();
        let point = Pos::new(Line(0), Column(0));
        let range = SelectionRange::new(point, point, false);

        let swatch = painted_term("\x1b[41m ");
        let rc = CellPainter::new(&swatch, &palette, &colors, Some(&range)).at(point);
        assert!(rc.selected);
        assert!(rc.draw_bg, "the swatch background still paints");
        assert_eq!(rc.bg, to_hsla(palette[1]), "background not replaced");

        let text = painted_term("\x1b[32mx");
        let rc = CellPainter::new(&text, &palette, &colors, Some(&range)).at(point);
        assert!(rc.selected);
        assert_eq!(rc.fg, to_hsla(palette[2]), "foreground not forced");

        assert!(colors.selection_bg.a < 1.0);
    }

    #[test]
    fn inverse_swaps_colors_and_always_paints_the_background() {
        let mut palette = [ColorRgb { r: 0, g: 0, b: 0 }; 256];
        palette[2] = ColorRgb {
            r: 0x22,
            g: 0x88,
            b: 0x22,
        };
        let colors = test_colors();
        let point = Pos::new(Line(0), Column(0));

        let cell = painted_term("\x1b[32;7mx");
        let rc = CellPainter::new(&cell, &palette, &colors, None).at(point);
        assert_eq!(rc.fg, to_hsla(colors.bg_rgb), "fg takes the old background");
        assert_eq!(rc.bg, to_hsla(palette[2]), "bg takes the old foreground");
        assert!(
            rc.draw_bg,
            "inverse cells paint their background even on the default bg"
        );
    }

    #[test]
    fn hidden_paints_the_foreground_as_the_background() {
        let mut palette = [ColorRgb { r: 0, g: 0, b: 0 }; 256];
        palette[1] = ColorRgb {
            r: 0xcc,
            g: 0x22,
            b: 0x22,
        };
        let colors = test_colors();
        let point = Pos::new(Line(0), Column(0));

        let on_default = painted_term("\x1b[31;8ms");
        let rc = CellPainter::new(&on_default, &palette, &colors, None).at(point);
        assert_eq!(rc.fg, rc.bg, "concealed text is invisible");
        assert_eq!(rc.fg, to_hsla(colors.bg_rgb));
        assert!(!rc.draw_bg);

        let on_colored = painted_term("\x1b[41;8ms");
        let rc = CellPainter::new(&on_colored, &palette, &colors, None).at(point);
        assert_eq!(rc.fg, to_hsla(palette[1]));
        assert_eq!(rc.fg, rc.bg);
    }

    #[test]
    fn dim_flag_reduces_foreground_intensity() {
        let palette = [ColorRgb { r: 0, g: 0, b: 0 }; 256];
        let colors = test_colors();
        let point = Pos::new(Line(0), Column(0));
        let cell = painted_term("\x1b[2mf");

        let rc = CellPainter::new(&cell, &palette, &colors, None).at(point);

        assert_eq!(rc.fg.h, colors.default_fg.h);
        assert_eq!(rc.fg.s, colors.default_fg.s);
        assert_eq!(rc.fg.l, colors.default_fg.l);
        assert!(
            rc.fg.a < colors.default_fg.a,
            "SGR 2 text must paint with reduced intensity"
        );
    }

    #[test]
    fn sgr_underlines_preserve_their_paint_variants() {
        let palette = [ColorRgb { r: 0, g: 0, b: 0 }; 256];
        let colors = test_colors();
        let point = Pos::new(Line(0), Column(0));
        let kind = |sgr: &str| {
            let term = painted_term(&format!("\x1b[{sgr}mu"));
            CellPainter::new(&term, &palette, &colors, None)
                .at(point)
                .underline
        };
        assert_eq!(kind("0"), UnderlineKind::None);
        assert_eq!(kind("4"), UnderlineKind::Single);
        assert_eq!(kind("4:2"), UnderlineKind::Double);
        assert_eq!(kind("4:3"), UnderlineKind::Curly);
        assert_eq!(kind("4:4"), UnderlineKind::Dotted);
        assert_eq!(kind("4:5"), UnderlineKind::Dashed);
    }

    #[test]
    fn sgr58_underline_color_resolves_through_the_palette() {
        let mut palette = [ColorRgb { r: 0, g: 0, b: 0 }; 256];
        palette[5] = ColorRgb {
            r: 0xd0,
            g: 0x30,
            b: 0x30,
        };
        let colors = test_colors();
        let point = Pos::new(Line(0), Column(0));

        let cell = painted_term("\x1b[4:3;58;5;5me");
        let rc = CellPainter::new(&cell, &palette, &colors, None).at(point);
        assert_eq!(rc.underline_color, Some(to_hsla(palette[5])));

        let plain = painted_term("\x1b[4:3me");
        let rc = CellPainter::new(&plain, &palette, &colors, None).at(point);
        assert_eq!(
            rc.underline_color, None,
            "no SGR 58 → paint falls back to fg"
        );
    }

    #[test]
    fn bold_italic_flag_sets_both_emphases() {
        let palette = [ColorRgb { r: 0, g: 0, b: 0 }; 256];
        let colors = test_colors();
        let point = Pos::new(Line(0), Column(0));
        let emphases = |sgr: &str| {
            let term = painted_term(&format!("\x1b[{sgr}mb"));
            let rc = CellPainter::new(&term, &palette, &colors, None).at(point);
            (rc.bold, rc.italic)
        };
        assert_eq!(emphases("1"), (true, false));
        assert_eq!(emphases("3"), (false, true));
        assert_eq!(emphases("1;3"), (true, true));
    }

    #[test]
    fn wide_char_spacers_defer_to_the_leading_cell() {
        let mut palette = [ColorRgb { r: 0, g: 0, b: 0 }; 256];
        palette[1] = ColorRgb {
            r: 0xcc,
            g: 0x22,
            b: 0x22,
        };
        let colors = test_colors();
        for (input, point) in [
            ("\x1b[41;4m中", Pos::new(Line(0), Column(1))),
            ("\x1b[1;80H\x1b[41;4m中", Pos::new(Line(0), Column(79))),
        ] {
            let term = painted_term(input);
            let rc = CellPainter::new(&term, &palette, &colors, None).at(point);
            assert!(rc.spacer);
            assert!(!rc.draw_bg, "spacers never paint");
            assert_eq!(rc.underline, UnderlineKind::None);
        }
    }

    #[test]
    fn only_real_combining_marks_set_marks() {
        let palette = [ColorRgb { r: 0, g: 0, b: 0 }; 256];
        let colors = test_colors();
        let point = Pos::new(Line(0), Column(0));

        let colored = painted_term("\x1b[4:3;58;5;5me");
        assert_eq!(
            CellPainter::new(&colored, &palette, &colors, None)
                .at(point)
                .marks,
            None,
            "SGR 58 alone is not a combining mark"
        );

        let linked = painted_term("\x1b]8;id=id;https://example.com\x1b\\e");
        assert_eq!(
            CellPainter::new(&linked, &palette, &colors, None)
                .at(point)
                .marks,
            None,
            "OSC 8 alone is not a combining mark"
        );

        let marked = painted_term("\u{2764}\u{FE0F}");
        assert_eq!(
            CellPainter::new(&marked, &palette, &colors, None)
                .at(point)
                .marks
                .as_deref(),
            Some(&['\u{FE0F}'][..]),
        );
    }

    #[test]
    fn repeated_styles_keep_each_cells_text_links_marks_and_selection() {
        let mut palette = [ColorRgb { r: 0, g: 0, b: 0 }; 256];
        palette[1] = ColorRgb {
            r: 200,
            g: 20,
            b: 10,
        };
        palette[2] = ColorRgb {
            r: 10,
            g: 180,
            b: 30,
        };
        let colors = test_colors();
        let term = painted_term(
            "\x1b[31m\x1b]8;;https://example.com\x1b\\a\u{301}\x1b]8;;\x1b\\b\x1b[32;4mC",
        );
        let first = Pos::new(Line(0), Column(0));
        let selection = SelectionRange::new(first, first, false);
        let mut painter = CellPainter::new(&term, &palette, &colors, Some(&selection));
        let a = painter.at(first);
        let b = painter.at(Pos::new(Line(0), Column(1)));
        let c = painter.at(Pos::new(Line(0), Column(2)));
        assert_eq!((a.c, b.c, c.c), ('a', 'b', 'C'));
        assert_eq!(a.marks.as_deref(), Some(&['\u{301}'][..]));
        assert_eq!(b.marks, None);
        assert!(a.selected);
        assert!(!b.selected);
        assert!(a.osc8_dots);
        assert!(!b.osc8_dots);
        assert_eq!(b.underline, UnderlineKind::None);
        assert_eq!(b.fg, to_hsla(palette[1]));
        assert_eq!(c.fg, to_hsla(palette[2]));
        assert_eq!(c.underline, UnderlineKind::Single);

        // A new snapshot must resolve a changed palette, not reuse old ink.
        palette[1] = ColorRgb {
            r: 20,
            g: 30,
            b: 220,
        };
        let next = CellPainter::new(&term, &palette, &colors, None).at(first);
        assert_eq!(next.fg, to_hsla(palette[1]));
    }

    #[test]
    fn inline_backgrounds_do_not_alias_the_previous_style() {
        let mut palette = [ColorRgb { r: 0, g: 0, b: 0 }; 256];
        palette[1] = ColorRgb {
            r: 200,
            g: 20,
            b: 10,
        };
        let colors = test_colors();
        let mut term = painted_term("\x1b[31ma");
        let first = Pos::new(Line(0), Column(0));
        let second = Pos::new(Line(0), Column(1));
        let third = Pos::new(Line(0), Column(2));
        term.grid[second].set_bg_palette(1);
        term.grid[third].set_bg_rgb(20, 30, 220);
        let mut painter = CellPainter::new(&term, &palette, &colors, None);
        assert_eq!(painter.at(first).fg, to_hsla(palette[1]));
        let indexed = painter.at(second);
        let rgb = painter.at(third);
        assert_eq!(indexed.bg, to_hsla(palette[1]));
        assert_eq!(
            rgb.bg,
            to_hsla(ColorRgb {
                r: 20,
                g: 30,
                b: 220
            })
        );
        assert!(indexed.draw_bg && rgb.draw_bg);
        assert_eq!(painter.at(first).fg, to_hsla(palette[1]));
    }

    #[gpui_kit::test]
    fn reused_viewport_discards_erased_text_and_outside_grid_padding(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        let (app, mut vcx, _stream) = crate::ui::app::test_window::harness_with_pane(cx);
        let view = app.update_in(&mut vcx, |app, _, _| match &app.tabs[0].pane.leaves()[0] {
            crate::ui::pane::PaneSlot::Ready(view) => view.clone(),
            _ => panic!("the fixture must have a terminal"),
        });
        let terminal = cx.update(|cx| view.read(cx).terminal.term.clone());
        let (rows, cols) = {
            let mut term = terminal.lock();
            rio_vt::performer::handler::Processor::default().advance(
                &mut *term,
                "\x1b[H\x1b[31mA\u{301}\x1b[4;58;5;9m\u{10eeee}\u{305}\u{305}".as_bytes(),
            );
            (term.screen_lines(), term.columns())
        };
        let element = TerminalElement::new(view);
        let mut buf = Vec::new();
        let build = |buf: &mut Vec<RenderCell>, cx: &mut gpui_kit::TestAppContext| {
            cx.update(|cx| {
                let colors = PaintColors::resolve(cx.theme(), cx);
                assert!(
                    element
                        .build_grid(
                            &colors,
                            buf,
                            rows + 1,
                            cols + 1,
                            false,
                            cx,
                            1.,
                            Rgba::default(),
                            true
                        )
                        .is_some()
                );
            });
        };
        build(&mut buf, cx);
        assert_eq!(buf[0].c, 'A');
        assert_eq!(buf[0].marks.as_deref(), Some(&['\u{301}'][..]));
        assert_eq!(
            buf[1].c, ' ',
            "unresolved image placeholders never render PUA glyphs"
        );
        assert!(buf[1].marks.is_none());
        assert_eq!(buf[1].underline, UnderlineKind::None);
        // Simulate content left by a formerly wider/taller source grid.
        let old = buf[0].clone();
        buf[cols] = old.clone();
        buf[rows * (cols + 1)] = old;
        rio_vt::performer::handler::Processor::default()
            .advance(&mut *terminal.lock(), b"\x1b[H\x1b[0m\x1b[2K");
        build(&mut buf, cx);
        for cell in [&buf[0], &buf[cols], &buf[rows * (cols + 1)]] {
            assert!(
                is_blank(cell),
                "erased text or old padding must not reappear"
            );
            assert!(cell.marks.is_none());
            assert!(!cell.selected && !cell.link_hover && !cell.match_hit && !cell.match_current);
        }
    }
}
