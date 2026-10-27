/// One drawn cell of the input bar.
#[derive(Debug, PartialEq)]
#[non_exhaustive]
pub(in crate::terminal) struct InputCell {
    /// Character indices the cell covers, as `start..end`.
    pub(in crate::terminal) start: usize,
    pub(in crate::terminal) end: usize,
    /// What to draw in it — a base character and whatever rides along with it.
    pub(in crate::terminal) text: String,
    /// Columns it occupies. Zero for a newline, which ends the row rather than
    /// taking space on it.
    pub(in crate::terminal) width: usize,
}

/// Splits input-bar text using Rio's public mode-2027 measurement API.
/// Every character of a cluster is shaped together; geometry and clicks use
/// the same width. Bare marks remain visible and newlines end their own row.
pub(in crate::terminal) fn input_cells(chars: &[char]) -> Vec<InputCell> {
    let codepoints: smallvec::SmallVec<[u32; 64]> = chars.iter().copied().map(u32::from).collect();
    let mut start = 0;
    let mut cells = Vec::with_capacity(chars.len());
    for (len, width) in lyrel_core::terminal_width::cells(&codepoints) {
        let end = start + len;
        let text = if chars[start] == '\n' {
            String::new()
        } else {
            chars[start..end].iter().collect()
        };
        cells.push(InputCell {
            start,
            end,
            text,
            width,
        });
        start = end;
    }
    cells
}

/// The fewest columns the input is given beside the prompt, however wide the
/// pane. Below this nearly any command with an argument or two wraps on its
/// first row, and a row that short is harder to read than a row lower down.
const INPUT_MIN_BESIDE_PROMPT: usize = 20;

/// Where the input bar starts, as `(row, column)` with the prompt's own row as
/// row 0: right after the prompt, or — when the prompt has left too little of
/// its row — at column 0 of the row below, with the whole width to itself
/// (#767).
///
/// "Too little" is under a third of the pane, and never under
/// [`INPUT_MIN_BESIDE_PROMPT`]. A third keeps an ordinary prompt on its own
/// line in any pane you would type in (`user@host ~/src/app % ` is about 25
/// columns; an 80-column pane still leaves 55), and only moves the input when
/// a deep path or a busy theme has eaten most of the row — the report behind
/// this had a 119-column prompt in a 143-column pane, 24 columns left. The
/// floor covers narrow panes, where a third is a handful of columns.
///
/// The move also has to gain something: in a pane barely wider than the
/// floor, a short prompt leaves under 20 columns and a new row would hardly
/// give more. So it only happens when the prompt is at least as wide as what
/// it left — the new row at least doubles the room.
pub(in crate::terminal) fn input_start(scol: usize, cols: usize) -> (usize, usize) {
    let left = cols.saturating_sub(scol);
    let floor = INPUT_MIN_BESIDE_PROMPT.max(cols / 3);
    if left < floor && scol >= left {
        (1, 0)
    } else {
        (0, scol)
    }
}

/// Where each character of the input bar lands: `(row, column, width)`, one
/// entry per character, plus the row and column the text ends on. `start` is
/// where the first one goes, from [`input_start`].
///
/// Walks the same cells the bar draws rather than re-deriving widths per
/// character — an emoji presentation sequence is two columns and a stranded
/// combining mark is one, and a second width table would put clicks, wrapping
/// and the caret a column away from the glyph on screen.
///
/// Only the base carries width, so clicking any occupied column selects it.
/// All characters inside a cluster share its caret anchor, matching drawing;
/// the separately returned end position is the caret after the final cell.
pub(in crate::terminal) fn input_char_positions(
    chars: &[char],
    pub(in crate::terminal) start: (usize, usize),
    cols: usize,
) -> (Vec<(usize, usize, usize)>, usize, usize) {
    let mut positions: Vec<(usize, usize, usize)> = Vec::with_capacity(chars.len());
    let (mut r, mut c) = start;
    for cell in input_cells(chars) {
        if chars[cell.start] == '\n' {
            positions.push((r, c, 0));
            r += 1;
            c = 0;
            continue;
        }
        if c + cell.width > cols {
            r += 1;
            c = 0;
        }
        positions.push((r, c, cell.width));
        for _ in cell.start + 1..cell.end {
            positions.push((r, c, 0));
        }
        c += cell.width;
    }
    (positions, r, c)
}

pub(in crate::terminal) fn input_overlay_rows(
    chars: &[char],
    cursor: usize,
    marked: &str,
    pub(in crate::terminal) start: (usize, usize),
    cols: usize,
) -> (usize, usize) {
    let mut merged: Vec<char> = Vec::with_capacity(chars.len() + marked.len());
    let cursor = cursor.min(chars.len());
    merged.extend_from_slice(&chars[..cursor]);
    merged.extend(marked.chars());
    merged.extend_from_slice(&chars[cursor..]);
    let (positions, r, c) = input_char_positions(&merged, start, cols);
    let end_row = if cursor >= chars.len() && marked.is_empty() && c >= cols {
        r + 1
    } else {
        r
    };
    let caret_vrow = positions.get(cursor).map_or(end_row, |&(pr, _, _)| pr);
    (end_row + 1, caret_vrow)
}

pub(in crate::terminal) fn input_overflow_shift(crow: usize, caret_vrow: usize, visual_rows: usize, rows: usize) -> usize {
    (crow + visual_rows)
        .saturating_sub(rows)
        .min(crow + caret_vrow)
}

/// Whether `col` on input-relative row `target` sits before `start`, the cell
/// the input bar's first character goes in.
pub(in crate::terminal) fn before_input(start: (usize, usize), col: usize, target: usize) -> bool {
    target < start.0 || (target == start.0 && col < start.1)
}

pub(in crate::terminal) fn wrapped_click_index(
    chars: &[char],
    pub(in crate::terminal) start: (usize, usize),
    cols: usize,
    col: usize,
    target: usize,
    clamp: bool,
) -> Option<usize> {
    let len = chars.len();
    let (positions, r, c) = input_char_positions(chars, start, cols);
    let end_row = if c >= cols { r + 1 } else { r };
    if target > end_row {
        return clamp.then_some(len);
    }
    for (i, &(pr, pc, pw)) in positions.iter().enumerate() {
        if pr == target && col >= pc && col < pc + pw {
            return Some(i);
        }
    }
    if let Some(fi) = positions.iter().position(|&(pr, _, _)| pr == target) {
        if col < positions[fi].1 {
            return Some(fi);
        }
    }
    if let Some(last) = positions.iter().rposition(|&(pr, _, _)| pr == target) {
        if chars[last] == '\n' {
            return Some(last);
        }
    }
    match positions.iter().position(|&(pr, _, _)| pr > target) {
        Some(ni) => Some(ni),
        None => Some(len),
    }
}
