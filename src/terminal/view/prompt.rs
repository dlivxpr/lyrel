//! Prompt geometry, selection hit testing, and inline input presentation.
//! Geometry remains a projection of PromptInput and the current terminal grid;
//! no editor, focus handle, frame buffer, or terminal attachment is recreated.

use super::history_adapter::{one_line, one_line_char};
use super::*;
use crate::terminal::highlight::{self, TokenKind};
use crate::terminal::prompt_input::layout::{
    before_input, input_cells, input_overflow_shift, input_overlay_rows, input_start,
    wrapped_click_index,
};

impl TerminalView {
    pub(super) fn cursor_cell(&self) -> Option<(usize, usize)> {
        let term = self.terminal.term.lock();
        let cursor = term.cursor().pos;
        let row = cursor.row.0 + term.display_offset() as i32;
        let col = cursor.col.0;
        (row >= 0).then_some((row as usize, col))
    }

    /// Where the IME should compose when the input bar has moved below the
    /// prompt (see [`input_start`]): the start of the bar's first row. `None`
    /// when the bar is beside the prompt, or not up at all — the prompt's own
    /// cursor cell is right then. Takes the cursor as the frame snapshot saw
    /// it rather than locking the terminal again mid-paint.
    pub(in crate::terminal) fn input_ime_cell(
        &self,
        row: usize,
        col: usize,
        cols: usize,
    ) -> Option<(usize, usize)> {
        if !self.input_active() || self.history.search().is_some() {
            return None;
        }
        match input_start(col, cols.max(1)) {
            (0, _) => None,
            (rows, col) => Some((row + rows, col)),
        }
    }

    pub(in crate::terminal) fn input_scroll_rows(&self) -> usize {
        if !self.input_active() || self.history.search().is_some() {
            return 0;
        }
        let Some((crow, ccol)) = self.cursor_cell() else {
            return 0;
        };
        let (rows, cols, offset) = {
            let term = self.terminal.term.lock();
            (
                term.screen_lines(),
                term.columns(),
                term.grid.display_offset(),
            )
        };
        if offset != 0 {
            return 0;
        }
        let chars: Vec<char> = self.input.editor().text().chars().collect();
        let (visual_rows, caret_vrow) = input_overlay_rows(
            &chars,
            self.input.editor().cursor(),
            &self.input.marked(),
            input_start(ccol, cols.max(1)),
            cols.max(1),
        );
        input_overflow_shift(crow, caret_vrow, visual_rows, rows)
    }

    pub(super) fn editor_char_index(&self, col: usize, row: usize, clamp: bool) -> Option<usize> {
        if !self.input_active() {
            return None;
        }
        let (srow, scol) = self.cursor_cell()?;
        if row < srow {
            return clamp.then_some(0);
        }
        let cols = self.terminal.term.lock().columns().max(1);
        let chars: Vec<char> = self.input.editor().text().chars().collect();
        wrapped_click_index(
            &chars,
            input_start(scol, cols),
            cols,
            col,
            row - srow,
            clamp,
        )
    }

    /// Whether a click on `row`/`col` lands on the prompt the shell drew,
    /// ahead of where the input bar's text begins.
    pub(super) fn click_on_prompt(&self, col: usize, row: usize) -> bool {
        let Some((srow, scol)) = self.cursor_cell() else {
            return false;
        };
        if row < srow {
            return false;
        }
        let cols = self.terminal.term.lock().columns().max(1);
        before_input(input_start(scol, cols), col, row - srow)
    }

    pub fn editor_click(
        &mut self,
        col: usize,
        row: usize,
        clicks: usize,
        shift: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        // A click on the prompt itself — the branch in `git:(main)`, the
        // directory before it — is on the prompt's text, not the editor's.
        // It goes to the grid's select like a click on any other output, and
        // leaves the caret where it was: the first half of a double click
        // must not have already thrown it to the start of the line.
        if self.click_on_prompt(col, row) {
            return false;
        }
        let Some(idx) = self.editor_char_index(col, row, false) else {
            return false;
        };
        let cfg = cx.global::<Config>();
        self.input
            .click(idx, clicks, shift, &cfg.word_separators, cfg.smart_select);
        self.close_completion();
        self.lifecycle.show_cursor();
        cx.notify();
        true
    }

    pub fn editor_drag(&mut self, col: usize, row: usize, cx: &mut Context<Self>) -> bool {
        if !self.input.is_selecting() {
            return false;
        }
        let Some(idx) = self.editor_char_index(col, row, true) else {
            return false;
        };
        let cfg = cx.global::<Config>();
        self.input.drag(idx, &cfg.word_separators, cfg.smart_select);
        self.lifecycle.show_cursor();
        cx.notify();
        true
    }

    pub(super) fn render_input_bar(
        &self,
        focused: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let (crow, ccol) = self.cursor_cell().unwrap_or((0, 0));
        let cx_left = px(GRID_PAD_X) + self.cell_width * (ccol as f32);
        let shift = self.input_scroll_rows();
        let cy_top = px(GRID_PAD_Y) + self.line_height * (crow as f32 - shift as f32);

        if let Some(rs) = self.history.search() {
            let label = format!("(reverse-i-search)`{}': ", rs.query());
            let matched = one_line(self.history.selected_line().unwrap_or_default());
            return div()
                .absolute()
                .left(cx_left)
                .top(cy_top)
                .right_4()
                .h(self.line_height)
                // The row floats over live grid cells; a bare div inserts no
                // hitbox, so a click aimed at it started a selection in the
                // text underneath (#541).
                .occlude()
                .flex()
                .items_center()
                .font_family(self.font.family.clone())
                .text_size(self.font_size)
                .child(
                    div()
                        .whitespace_nowrap()
                        .text_color(cx.theme().muted_foreground)
                        .child(label),
                )
                .child(
                    div()
                        .whitespace_nowrap()
                        .text_color(cx.theme().foreground)
                        .child(matched),
                );
        }

        let chars: Vec<char> = self.input.editor().text().chars().collect();
        let len = chars.len();
        let cursor = self.input.editor().cursor();
        let marked = self.input.marked().to_owned();
        let has_marked = !marked.is_empty();
        let selection = self.input.editor().selection();

        let theme = cx.theme();
        let fg = theme.foreground;
        let caret_col = theme.caret;
        let muted = theme.muted_foreground;
        let mut sel_bg = theme.selection;
        sel_bg.a = 0.55;
        let cell_w = self.cell_width;
        let lh = self.line_height;
        let caret_h = px((self.font_size.as_f32() * 1.2).min(lh.as_f32()));
        let caret_top = px((lh.as_f32() - caret_h.as_f32()) / 2.0);

        let line: String = chars.iter().collect();
        let mut colors: Vec<gpui_kit::Hsla> = Vec::with_capacity(len);
        for span in highlight::highlight(&line) {
            let c = self.kind_color(span.kind, cx);
            for _ in span.text.chars() {
                colors.push(c);
            }
        }

        // This caret only exists at a prompt, so the prompt's own shape wins.
        let cursor_style = {
            let cfg = cx.global::<Config>();
            cfg.prompt_cursor_style.shape().unwrap_or(cfg.cursor_style)
        };
        let cursor_paint =
            input_caret_paint(focused, self.lifecycle.cursor_visible(), cursor_style);
        let cursor_on = cursor_paint.is_some();
        let block_cursor = cursor_paint == Some(InputCaretPaint::Block);
        // A block caret is drawn as reverse video on the cell it covers, not as
        // a translucent tint over it — the way every other terminal draws one,
        // and the only way the caret keeps the contrast the theme gave it.
        let caret_ink = crate::ui::presets::caret_ink(caret_col, theme.background, fg);
        let caret_bar = move || {
            let base = div().absolute().left_0();
            match cursor_paint.expect("caret is only rendered when it has a paint style") {
                // An inactive terminal keeps a steady hollow block, independent
                // of the configured focused-caret shape and current blink phase.
                InputCaretPaint::Outline => base
                    .top(px(0.))
                    .w_full()
                    .h(lh)
                    .border_1()
                    .border_color(caret_col),
                InputCaretPaint::Block => base.top(px(0.)).w_full().h(lh).bg(caret_col),
                InputCaretPaint::Bar => base.top(caret_top).w(px(1.5)).h(caret_h).bg(caret_col),
                InputCaretPaint::Underline => {
                    let uh = px(2.);
                    base.top(lh - uh).w_full().h(uh).bg(caret_col)
                }
            }
        };
        let cell = |color: gpui_kit::Hsla,
                    text: String,
                    width: usize,
                    selected: bool,
                    caret: bool,
                    underline: bool| {
            let inverted = caret && block_cursor;
            let w = cell_w * (width as f32);
            let mut d = div()
                .relative()
                .flex_none()
                .w(w)
                .h(lh)
                .flex()
                .items_center()
                .text_color(if inverted { caret_ink } else { color });
            if inverted {
                d = d.bg(caret_col);
            } else if selected {
                d = d.bg(sel_bg);
            }
            if underline {
                d = d.border_b_1().border_color(fg);
            }
            d = d.child(text);
            if caret && !inverted {
                d = d.child(caret_bar());
            }
            d.into_any_element()
        };

        let blank = move |w: gpui_kit::Pixels| div().flex_none().w(w).h(lh);

        // The prompt's row, then the input's. Starting below the prompt leaves
        // that row empty — the prompt shows through it — and the input begins
        // at column 0 of the next.
        let cols = self.terminal.term.lock().columns().max(1);
        let (start_row, start_col) = input_start(ccol, cols);
        let mut lines: Vec<Vec<gpui_kit::AnyElement>> =
            (0..start_row).map(|_| Vec::new()).collect();
        lines.push(vec![blank(cell_w * (start_col as f32)).into_any_element()]);

        let is_multiline = chars.contains(&'\n');

        let marked_cells = input_cells(&marked.chars().collect::<Vec<char>>());
        for c in input_cells(&chars) {
            // The caret sits on the base of a cell, so an IME's in-flight text
            // opens wherever the caret is drawn — anywhere inside the cell, not
            // only on its first character.
            if has_marked && (c.start..c.end).contains(&cursor) {
                for mc in &marked_cells {
                    lines.last_mut().unwrap().push(cell(
                        fg,
                        mc.text.clone(),
                        mc.width,
                        false,
                        false,
                        true,
                    ));
                }
            }
            let i = c.start;
            if chars[i] == '\n' {
                if selection.is_none() && !has_marked && cursor_on && cursor == i {
                    lines.last_mut().unwrap().push(
                        blank(cell_w)
                            .relative()
                            .child(caret_bar())
                            .into_any_element(),
                    );
                } else if selection.is_some_and(|(s, e)| i >= s && i < e) {
                    lines
                        .last_mut()
                        .unwrap()
                        .push(blank(cell_w).bg(sel_bg).into_any_element());
                }
                lines.push(Vec::new());
                continue;
            }
            // A cell is one glyph, so it tints as a unit: a selection that
            // covers any character in it — the base or a mark riding on it —
            // covers the whole thing.
            let selected = selection.is_some_and(|(s, e)| s < c.end && c.start < e);
            let caret = selection.is_none()
                && !has_marked
                && cursor_on
                && (c.start..c.end).contains(&cursor);
            lines
                .last_mut()
                .unwrap()
                .push(cell(colors[i], c.text, c.width, selected, caret, false));
        }

        let ghost: Option<String> = if selection.is_none() && !has_marked && !is_multiline {
            self.ghost_suggestion()
                .map(|full| full.chars().skip(len).collect::<String>())
                .filter(|r| !r.is_empty())
        } else {
            None
        };

        if cursor == len {
            let last = lines.last_mut().unwrap();
            if has_marked {
                for mc in &marked_cells {
                    last.push(cell(fg, mc.text.clone(), mc.width, false, false, true));
                }
            } else if ghost.is_none() {
                let mut tail = blank(cell_w).relative();
                if selection.is_none() && cursor_on {
                    tail = tail.child(caret_bar());
                }
                last.push(tail.into_any_element());
            }
        }

        if let Some(rem) = ghost {
            let last = lines.last_mut().unwrap();
            let flat: Vec<char> = rem.chars().map(one_line_char).collect();
            for (gi, gc) in input_cells(&flat).into_iter().enumerate() {
                let caret = gi == 0 && cursor == len && cursor_on;
                last.push(cell(muted, gc.text, gc.width, false, caret, false));
            }
        }

        let rows = lines.into_iter().map(move |cells| {
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .w_full()
                .min_h(lh)
                .children(cells)
        });

        div()
            .absolute()
            .left(px(GRID_PAD_X))
            .top(cy_top)
            .right_4()
            .min_h(lh)
            .flex()
            .flex_col()
            .font_family(self.font.family.clone())
            .text_size(self.font_size)
            .line_height(lh)
            .text_color(fg)
            .children(rows)
    }

    pub(super) fn kind_color(&self, kind: TokenKind, cx: &App) -> gpui_kit::Hsla {
        let theme = cx.theme();
        match kind {
            TokenKind::Command => theme.green,
            TokenKind::Flag => theme.cyan,
            TokenKind::Path => theme.blue,
            TokenKind::StringLit => theme.yellow,
            TokenKind::Operator => theme.magenta,
            TokenKind::Comment => theme.muted_foreground,
            TokenKind::Arg | TokenKind::Whitespace => theme.foreground,
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum InputCaretPaint {
    Outline,
    Bar,
    Block,
    Underline,
}

fn input_caret_paint(
    focused: bool,
    blink_on: bool,
    style: crate::core::config::CursorStyle,
) -> Option<InputCaretPaint> {
    use crate::core::config::CursorStyle;

    if !focused {
        return Some(InputCaretPaint::Outline);
    }
    blink_on.then_some(match style {
        CursorStyle::Bar => InputCaretPaint::Bar,
        CursorStyle::Block => InputCaretPaint::Block,
        CursorStyle::Underline => InputCaretPaint::Underline,
    })
}

pub(super) fn menu_layout(
    total_rows: usize,
    srow: usize,
    count: usize,
    sel: usize,
    max_rows: usize,
) -> (bool, usize, usize) {
    let want = count.min(max_rows);
    let below = total_rows.saturating_sub(srow + 1);
    let above = srow;
    let footers = if count > want { 2 } else { 0 };
    let need = want + footers;
    let (place_above, visible) = if below >= need {
        (false, want)
    } else if above >= need {
        (true, want)
    } else {
        let squeeze = |room: usize| room.saturating_sub(2).max(1);
        if above > below {
            (true, squeeze(above))
        } else {
            (false, squeeze(below))
        }
    };
    let visible = visible.min(count);
    let first = sel
        .saturating_sub(visible.saturating_sub(1))
        .min(count.saturating_sub(visible));
    (place_above, visible, first)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::prompt_input::layout::input_char_positions;
    #[test]
    fn an_unfocused_input_caret_is_always_a_steady_outline() {
        use super::{InputCaretPaint, input_caret_paint};
        use crate::core::config::CursorStyle;

        for blink_on in [false, true] {
            for style in [CursorStyle::Bar, CursorStyle::Block, CursorStyle::Underline] {
                assert_eq!(
                    input_caret_paint(false, blink_on, style),
                    Some(InputCaretPaint::Outline),
                );
            }
        }
    }

    #[test]
    fn a_focused_input_caret_keeps_its_configured_shape_and_blink_phase() {
        use super::{InputCaretPaint, input_caret_paint};
        use crate::core::config::CursorStyle;

        assert_eq!(input_caret_paint(true, false, CursorStyle::Block), None);
        assert_eq!(
            input_caret_paint(true, true, CursorStyle::Bar),
            Some(InputCaretPaint::Bar),
        );
        assert_eq!(
            input_caret_paint(true, true, CursorStyle::Block),
            Some(InputCaretPaint::Block),
        );
        assert_eq!(
            input_caret_paint(true, true, CursorStyle::Underline),
            Some(InputCaretPaint::Underline),
        );
    }
    #[test]
    fn input_clusters_caret_clicks_and_wraps_match_rio_grid() {
        use rio_vt::ansi::CursorShape;
        use rio_vt::crosswords::pos::{Column, Line};
        use rio_vt::crosswords::{Crosswords, CrosswordsSize};
        use rio_vt::event::{VoidListener, WindowId};
        use rio_vt::performer::handler::Processor;

        for cluster in ["👩‍💻", "❤️", "中", "e\u{301}", "🇨🇳", "🇨🇳🇺🇸", "👍🏽", "❤︎", "1️⃣"]
        {
            for (prefix, cols) in [("", 80), ("abc", 4), ("ab\n", 4)] {
                let text = format!("{prefix}{cluster}X");
                let chars: Vec<char> = text.chars().collect();
                let mut term = Crosswords::new(
                    CrosswordsSize::new(cols, 8),
                    CursorShape::Block,
                    VoidListener,
                    WindowId::from(0),
                    0,
                    0,
                );
                let mut parser: Processor = Processor::default();
                parser.advance(&mut term, text.replace('\n', "\r\n").as_bytes());
                let (positions, end_row, end_col) = input_char_positions(&chars, (0, 0), cols);
                let marker = chars.len() - 1;
                let (row, col, _) = positions[marker];
                assert_eq!(
                    term.grid[Line(row as i32)][Column(col)].c(),
                    'X',
                    "{text:?}"
                );
                assert_eq!((end_row, end_col), (row, col + 1), "{text:?}");
                assert_eq!(
                    lyrel_core::terminal_width::width(cluster),
                    if cluster == "e\u{301}" || cluster == "❤︎" {
                        1
                    } else if cluster == "🇨🇳🇺🇸" {
                        4
                    } else {
                        2
                    }
                );
                for cell in input_cells(&chars) {
                    let (row, col, width) = positions[cell.start];
                    if width == 0 {
                        continue;
                    }
                    assert_eq!(
                        cell.text,
                        chars[cell.start..cell.end].iter().collect::<String>()
                    );
                    assert_eq!(
                        term.grid[Line(row as i32)][Column(col)].c(),
                        chars[cell.start],
                        "{text:?}"
                    );
                    for column in col..col + width {
                        assert_eq!(
                            click(&text, 0, cols, column, row),
                            Some(cell.start),
                            "{text:?}"
                        );
                    }
                    for caret in cell.start..cell.end {
                        assert_eq!(
                            (positions[caret].0, positions[caret].1),
                            (row, col),
                            "{text:?}"
                        );
                        assert_eq!(input_overlay_rows(&chars, caret, "", (0, 0), cols).1, row);
                    }
                }
            }
        }
    }

    #[test]
    fn input_text_presentation_keeps_the_next_character_and_caret_narrow() {
        let chars: Vec<char> = "⌚︎X".chars().collect();
        let (positions, row, col) = input_char_positions(&chars, (0, 0), 80);
        assert_eq!(positions[2], (0, 1, 1));
        assert_eq!((row, col), (0, 2));
        assert_eq!(click("⌚︎X", 0, 80, 0, 0), Some(0));
        assert_eq!(click("⌚︎X", 0, 80, 1, 0), Some(2));
    }

    #[test]
    fn input_cluster_indices_are_char_offsets_not_utf8_bytes() {
        let text = "中👩‍💻❤️e\u{301}🇨🇳X";
        let chars: Vec<char> = text.chars().collect();
        let byte_offsets: Vec<usize> = text
            .char_indices()
            .map(|(byte, _)| byte)
            .chain(std::iter::once(text.len()))
            .collect();
        let mut rebuilt = String::new();
        for cell in input_cells(&chars) {
            assert_eq!(
                cell.text,
                &text[byte_offsets[cell.start]..byte_offsets[cell.end]]
            );
            rebuilt.push_str(&cell.text);
        }
        assert_eq!(rebuilt, text);
        let (positions, _, _) = input_char_positions(&chars, (0, 0), 80);
        assert_eq!(positions[chars.len() - 1], (0, 9, 1));
    }

    /// The width table feeds the bar's own line breaking, so a character
    /// counted short pulls everything after it one column left.
    #[test]
    fn input_char_positions_reserve_two_columns_for_wide_chars() {
        let chars: Vec<char> = "a🀄b".chars().collect();
        let (positions, _, _) = input_char_positions(&chars, (0, 0), 80);
        assert_eq!(positions, vec![(0, 0, 1), (0, 1, 2), (0, 3, 1)]);
    }

    /// Geometry and drawing read the same cells, so a sequence the bar draws
    /// two columns wide is two columns wide to wrapping and clicks as well. Two
    /// width tables would put `X` under the right half of the heart.
    #[test]
    fn input_char_positions_agree_with_the_cells_the_bar_draws() {
        for text in [
            "a🀄b",
            "e\u{0301}X",
            "\u{2764}\u{FE0F}X",
            "\u{0301}ab",
            "a\nb",
        ] {
            let chars: Vec<char> = text.chars().collect();
            let (positions, _, _) = input_char_positions(&chars, (0, 0), 80);
            assert_eq!(positions.len(), chars.len(), "{text:?}");
            for cell in input_cells(&chars) {
                let drawn = if chars[cell.start] == '\n' {
                    0
                } else {
                    cell.width
                };
                assert_eq!(positions[cell.start].2, drawn, "{text:?} at {}", cell.start);
                for i in cell.start + 1..cell.end {
                    assert_eq!(positions[i].2, 0, "{text:?} at {i}");
                }
            }
        }
    }

    /// `❤️` is two columns in the bar, so the character after it starts at
    /// column 2 — and a click on either half of it lands on the heart.
    #[test]
    fn input_char_positions_reserve_two_columns_for_an_emoji_presentation_sequence() {
        let chars: Vec<char> = "\u{2764}\u{FE0F}X".chars().collect();
        let (positions, _, _) = input_char_positions(&chars, (0, 0), 80);
        assert_eq!(positions[2], (0, 2, 1));
        assert_eq!(click("\u{2764}\u{FE0F}X", 0, 80, 0, 0), Some(0));
        assert_eq!(click("\u{2764}\u{FE0F}X", 0, 80, 1, 0), Some(0));
        assert_eq!(click("\u{2764}\u{FE0F}X", 0, 80, 2, 0), Some(2));
    }

    /// A mark with no base gets a cell of its own on screen, so it has to get a
    /// column here too — otherwise everything after it clicks one column off.
    #[test]
    fn input_char_positions_give_a_stranded_combining_mark_a_column() {
        let chars: Vec<char> = "\u{0301}ab".chars().collect();
        let (positions, _, _) = input_char_positions(&chars, (0, 0), 80);
        assert_eq!(positions, vec![(0, 0, 1), (0, 1, 1), (0, 2, 1)]);
    }

    /// A cell wraps whole. Splitting `❤️` across rows would draw its two
    /// columns on one row and count them on two.
    #[test]
    fn input_char_positions_wrap_a_cell_without_splitting_it() {
        let chars: Vec<char> = "abc\u{2764}\u{FE0F}".chars().collect();
        let (positions, r, c) = input_char_positions(&chars, (0, 0), 4);
        assert_eq!(positions[3], (1, 0, 2));
        assert_eq!((r, c), (1, 2));
    }

    /// Clicking the right half of a wide character lands on that character, not
    /// on the one after it.
    #[test]
    fn wrapped_click_index_hits_both_halves_of_a_wide_char() {
        assert_eq!(click("a🀄b", 0, 80, 1, 0), Some(1));
        assert_eq!(click("a🀄b", 0, 80, 2, 0), Some(1));
        assert_eq!(click("a🀄b", 0, 80, 3, 0), Some(2));
    }

    /// A combining mark shares its base's column, so a click there hits the
    /// base — there is nowhere on screen that is the mark and not the base.
    #[test]
    fn wrapped_click_index_lands_on_the_base_not_its_combining_mark() {
        // e + COMBINING ACUTE ACCENT + X.
        assert_eq!(click("e\u{0301}X", 0, 80, 0, 0), Some(0));
        assert_eq!(click("e\u{0301}X", 0, 80, 1, 0), Some(2));
    }

    fn cells(text: &str) -> Vec<(usize, usize, String, usize)> {
        let chars: Vec<char> = text.chars().collect();
        input_cells(&chars)
            .into_iter()
            .map(|c| (c.start, c.end, c.text, c.width))
            .collect()
    }

    #[test]
    fn input_cells_keep_a_combining_mark_with_its_base() {
        assert_eq!(
            cells("e\u{0301}X"),
            vec![
                (0, 2, "e\u{0301}".to_string(), 1),
                (2, 3, "X".to_string(), 1),
            ]
        );
    }

    /// An emoji presentation sequence is two columns wide even though its base
    /// is one on its own — the same re-scoring the terminal grid does.
    #[test]
    fn input_cells_widen_an_emoji_presentation_sequence() {
        assert_eq!(
            cells("\u{2764}\u{FE0F}X"),
            vec![
                (0, 2, "\u{2764}\u{FE0F}".to_string(), 2),
                (2, 3, "X".to_string(), 1),
            ]
        );
        // U+FE0E asks for the text glyph, and stays one column.
        assert_eq!(
            cells("\u{2764}\u{FE0E}"),
            vec![(0, 2, "\u{2764}\u{FE0E}".to_string(), 1)]
        );
    }

    /// A mark with no base ahead of it still needs a column, or it is invisible
    /// and the caret has nowhere to sit. A newline is not a base to hang one on.
    #[test]
    fn input_cells_give_a_stranded_combining_mark_a_column() {
        assert_eq!(cells("\u{0301}"), vec![(0, 1, "\u{0301}".to_string(), 1)]);
        assert_eq!(
            cells("\n\u{0301}"),
            vec![(0, 1, String::new(), 0), (1, 2, "\u{0301}".to_string(), 1),]
        );
    }

    fn click(text: &str, scol: usize, cols: usize, col: usize, row: usize) -> Option<usize> {
        let chars: Vec<char> = text.chars().collect();
        wrapped_click_index(&chars, (0, scol), cols, col, row, false)
    }

    #[test]
    fn before_input_covers_the_prompt_and_nothing_after_it() {
        // ` lyrel git:(main) ` — the editor starts at column 17.
        assert!(before_input((0, 17), 12, 0));
        assert!(before_input((0, 17), 0, 0));
        assert!(!before_input((0, 17), 17, 0));
        assert!(!before_input((0, 17), 30, 0));
        assert!(!before_input((0, 17), 0, 1));
        // A prompt too long to share its row pushes the editor down one.
        assert!(before_input((1, 0), 70, 0));
        assert!(!before_input((1, 0), 0, 1));
    }

    #[test]
    fn wrapped_click_index_hits_chars_on_the_first_row() {
        assert_eq!(click("git", 4, 80, 4, 0), Some(0));
        assert_eq!(click("git", 4, 80, 6, 0), Some(2));
        assert_eq!(click("git", 4, 80, 1, 0), Some(0));
        assert_eq!(click("git", 4, 80, 40, 0), Some(3));
    }

    #[test]
    fn wrapped_click_index_maps_wrapped_rows() {
        assert_eq!(click("abcdef", 8, 10, 9, 0), Some(1));
        assert_eq!(click("abcdef", 8, 10, 0, 1), Some(2));
        assert_eq!(click("abcdef", 8, 10, 3, 1), Some(5));
        assert_eq!(click("a你", 2, 4, 3, 0), Some(1));
        assert_eq!(click("abcdef", 8, 10, 9, 1), Some(6));
    }

    #[test]
    fn wrapped_click_index_respects_wide_chars() {
        assert_eq!(click("你好", 2, 80, 2, 0), Some(0));
        assert_eq!(click("你好", 2, 80, 3, 0), Some(0));
        assert_eq!(click("你好", 2, 80, 4, 0), Some(1));
        assert_eq!(click("你", 4, 5, 0, 1), Some(0));
        assert_eq!(click("你", 4, 5, 1, 1), Some(0));
    }

    #[test]
    fn wrapped_click_index_rows_past_the_input_need_clamp() {
        let chars: Vec<char> = "ls".chars().collect();
        assert_eq!(wrapped_click_index(&chars, (0, 4), 80, 3, 2, false), None);
        assert_eq!(wrapped_click_index(&chars, (0, 4), 80, 3, 2, true), Some(2));
        assert_eq!(wrapped_click_index(&[], (0, 4), 80, 30, 0, false), Some(0));
        assert_eq!(wrapped_click_index(&chars, (0, 4), 80, 3, 1, false), None);
    }

    #[test]
    fn wrapped_click_index_covers_the_wrapped_caret_slot() {
        assert_eq!(click("abcdef", 4, 10, 0, 1), Some(6));
        assert_eq!(click("abcdef", 4, 10, 7, 1), Some(6));
        let chars: Vec<char> = "abcdef".chars().collect();
        assert_eq!(wrapped_click_index(&chars, (0, 4), 10, 0, 2, false), None);
    }

    #[test]
    fn wrapped_click_index_treats_newlines_as_hard_breaks() {
        assert_eq!(click("a\nbc", 4, 80, 4, 0), Some(0));
        assert_eq!(click("a\nbc", 4, 80, 0, 1), Some(2));
        assert_eq!(click("a\nbc", 4, 80, 1, 1), Some(3));
        assert_eq!(click("a\nbc", 4, 80, 40, 0), Some(1));
        assert_eq!(click("a\nbc", 4, 80, 40, 1), Some(4));
        assert_eq!(click("a\n\nb", 4, 80, 3, 1), Some(2));
        assert_eq!(click("a\n\nb", 4, 80, 0, 2), Some(3));
    }

    #[test]
    fn input_overlay_rows_counts_wraps_slot_marked_and_newlines() {
        let rows = |text: &str, cursor: usize, marked: &str, scol: usize, cols: usize| {
            let chars: Vec<char> = text.chars().collect();
            input_overlay_rows(&chars, cursor, marked, (0, scol), cols)
        };
        assert_eq!(rows("", 0, "", 3, 8), (1, 0));
        assert_eq!(rows("aaaaaaaaaa", 10, "", 6, 8), (3, 2));
        assert_eq!(rows("aaaaaaaaaa", 3, "", 6, 8), (2, 1));
        assert_eq!(rows("ab\ncd", 5, "", 0, 8), (2, 1));
        assert_eq!(rows("ab", 1, "漢", 6, 8), (2, 1));
    }

    #[test]
    fn the_input_stays_beside_an_ordinary_prompt() {
        // `user@host ~/src/app % ` in an 80-column pane: 55 columns left.
        assert_eq!(input_start(25, 80), (0, 25));
        // No prompt at all, and a bare `$ `.
        assert_eq!(input_start(0, 80), (0, 0));
        assert_eq!(input_start(2, 143), (0, 2));
        // Exactly a third left is enough.
        assert_eq!(input_start(60, 90), (0, 60));
    }

    #[test]
    fn a_prompt_that_eats_the_row_pushes_the_input_below_it() {
        // #767: a 119-column prompt in a 143-column pane left 24.
        assert_eq!(input_start(119, 143), (1, 0));
        // One column short of a third.
        assert_eq!(input_start(61, 90), (1, 0));
        // A narrow pane falls back to the 20-column floor, not a third of it.
        assert_eq!(input_start(25, 40), (1, 0));
        assert_eq!(input_start(20, 40), (0, 20));
        // A cursor parked in the last column, or past it, has no room at all.
        assert_eq!(input_start(79, 80), (1, 0));
        assert_eq!(input_start(80, 80), (1, 0));
    }

    #[test]
    fn a_new_row_has_to_gain_something() {
        // A 20-column pane: a `$ ` prompt leaves 18, under the floor, but a
        // fresh row would give only two more.
        assert_eq!(input_start(2, 20), (0, 2));
        // Half the row is the break-even point.
        assert_eq!(input_start(9, 20), (0, 9));
        assert_eq!(input_start(10, 20), (1, 0));
    }

    /// Everything that reads the layout — the overlay's height, the caret's
    /// row, clicks — sees the input a row down, with the prompt's row empty.
    #[test]
    fn input_below_the_prompt_is_laid_out_from_the_next_row() {
        let start = input_start(119, 143);
        let chars: Vec<char> = "git status".chars().collect();
        let (positions, r, c) = input_char_positions(&chars, start, 143);
        assert_eq!(positions[0], (1, 0, 1));
        assert_eq!((r, c), (1, 10));
        // Two rows tall — the prompt's and the input's — with the caret on
        // the second, so an overflow shift keeps the input on screen.
        assert_eq!(input_overlay_rows(&chars, 10, "", start, 143), (2, 1));
        assert_eq!(input_overlay_rows(&[], 0, "", start, 143), (2, 1));
        // Wrapping uses the whole width of the new row.
        let long: Vec<char> = "a".repeat(150).chars().collect();
        assert_eq!(input_overlay_rows(&long, 150, "", start, 143), (3, 2));
        // A click on the prompt's row lands before the first character; on
        // the input row it hits the character under it.
        assert_eq!(
            wrapped_click_index(&chars, start, 143, 130, 0, false),
            Some(0)
        );
        assert_eq!(
            wrapped_click_index(&chars, start, 143, 4, 1, false),
            Some(4)
        );
        assert_eq!(
            wrapped_click_index(&chars, start, 143, 60, 1, false),
            Some(10)
        );
        assert_eq!(wrapped_click_index(&chars, start, 143, 0, 2, false), None);
    }

    #[test]
    fn input_overflow_shift_keeps_the_tail_and_caret_visible() {
        assert_eq!(input_overflow_shift(5, 2, 3, 22), 0);
        assert_eq!(input_overflow_shift(20, 2, 3, 22), 1);
        assert_eq!(input_overflow_shift(21, 29, 30, 22), 29);
        assert_eq!(input_overflow_shift(21, 0, 30, 22), 21);
    }
    #[test]
    fn menu_layout_prefers_below_and_flips_above_when_cramped() {
        assert_eq!(menu_layout(24, 3, 5, 0, 10), (false, 5, 0));
        assert_eq!(menu_layout(24, 22, 5, 0, 10), (true, 5, 0));
        assert_eq!(menu_layout(6, 4, 10, 0, 10), (true, 2, 0));
        assert_eq!(menu_layout(6, 1, 10, 0, 10), (false, 2, 0));
        let (_, visible, _) = menu_layout(1, 0, 8, 0, 10);
        assert_eq!(visible, 1);
    }

    #[test]
    fn menu_layout_budgets_the_overflow_footers() {
        let (place_above, visible, first) = menu_layout(24, 13, 30, 17, 10);
        assert!(
            place_above,
            "12 needed lines don't fit in the 10 rows below"
        );
        assert_eq!(visible, 10);
        assert!((first..first + visible).contains(&17));
    }

    #[test]
    fn menu_layout_caps_rows_and_windows_around_the_selection() {
        let (_, visible, first) = menu_layout(40, 0, 30, 17, 10);
        assert_eq!(visible, 10);
        assert!((first..first + visible).contains(&17));
        assert_eq!(first, 8);
        let (_, visible, first) = menu_layout(40, 0, 30, 29, 10);
        assert_eq!(first, 20);
        assert_eq!(first + visible, 30);
        assert_eq!(menu_layout(40, 0, 30, 3, 10).2, 0);
    }
}
