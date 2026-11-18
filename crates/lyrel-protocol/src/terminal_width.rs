//! 输入栏与 CLI 表格共用 Rio 默认 mode 2027 的占列规则。

use rio_grapheme_width::emoji::Presentation;
use rio_unicode::UnicodeWidthChar;
use rio_unicode::grapheme::{BreakState, grapheme_class, is_break};
use smallvec::SmallVec;

/// 依次返回 `(字符数, 列宽)`，不分配内存。
///
/// 分段和 emoji 占列直接驱动 Rio 同款锁定注册表原语（`rio-unicode` 的
/// UAX #29 状态机与 EastAsianWidth 表、`rio-grapheme-width` 的 emoji
/// variation 表），与 released parser 的 mode-2027 打印路径一致；这里只保留
/// 应用自己的两项编辑契约：换行单独结束一行，无 base 的 mark 仍占一列以便
/// 显示和定位。CR 不与 LF 合并，保证输入栏的字符索引和硬换行保持一一对应。
pub fn cells(mut codepoints: &[u32]) -> impl Iterator<Item = (usize, usize)> + '_ {
    std::iter::from_fn(move || {
        let &first = codepoints.first()?;
        let (len, width) = match first {
            0x0A => (1, 0),
            0x0D => (1, 1),
            _ => {
                let (len, width) = cluster_width(codepoints);
                (len, width.max(1))
            }
        };
        codepoints = &codepoints[len..];
        Some((len, width))
    })
}

/// 文本占用的列数；短表格字段的码点缓冲保存在栈上。
pub fn width(text: &str) -> usize {
    let codepoints: SmallVec<[u32; 64]> = text.chars().map(u32::from).collect();
    cells(&codepoints).map(|(_, width)| width).sum()
}

/// Measure the first grapheme cluster in `codepoints`: how many codepoints it
/// spans and how many terminal cells it occupies, following the same
/// segmentation and width rules as Rio's mode-2027 print path.
///
/// The rules are evaluated directly against the released registry primitives
/// that path is built from — `rio-unicode`'s grapheme state machine and width
/// tables plus `rio-grapheme-width`'s emoji variation table — so this helper
/// cannot drift from the grid it is compared against in the dev-only test
/// below. Behavior notes kept in parity with that path:
///
/// - a variation selector flips the width of a valid emoji base (U+FE0F wide,
///   U+FE0E narrow) and is consumed without effect anywhere else;
/// - any other width-bearing continuation makes the whole cluster wide;
/// - zero-width continuations change nothing;
/// - this is not a streaming call: the slice must contain a complete first
///   cluster or the logical end of the text;
/// - values that are not Unicode scalars measure as one single-width
///   codepoint when first and terminate the cluster when later.
fn cluster_width(codepoints: &[u32]) -> (usize, usize) {
    let Some(&first) = codepoints.first() else {
        return (0, 0);
    };
    let Some(base) = char::from_u32(first) else {
        return (1, 1);
    };
    let mut width = UnicodeWidthChar::width(base).unwrap_or(1);
    let mut prev_class = grapheme_class(base);
    let mut state = BreakState::start(prev_class);
    let mut last_cp = base;
    let mut len = 1;
    while len < codepoints.len() {
        let Some(c) = char::from_u32(codepoints[len]) else {
            break;
        };
        let class = grapheme_class(c);
        let state_before = state;
        if is_break(prev_class, class, &mut state) {
            break;
        }
        match c {
            '\u{FE0F}' | '\u{FE0E}' => {
                if vs_is_valid_base(last_cp, c) {
                    width = if c == '\u{FE0F}' { 2 } else { 1 };
                    prev_class = class;
                    last_cp = c;
                } else {
                    // Ignored selector: consumed without effect, exactly like
                    // the released print path.
                    state = state_before;
                }
            }
            _ => {
                if UnicodeWidthChar::width(c).unwrap_or(0) > 0 {
                    width = 2;
                }
                prev_class = class;
                last_cp = c;
            }
        }
        len += 1;
    }
    (len, width)
}

/// True when (`base`, `vs`) appears in Unicode's emoji-variation-sequences,
/// i.e. the selector is defined to have an effect on this base. Queried
/// through the same `rio-grapheme-width` table Rio's grid uses.
fn vs_is_valid_base(base: char, vs: char) -> bool {
    let mut sequence = String::with_capacity(base.len_utf8() + vs.len_utf8());
    sequence.push(base);
    sequence.push(vs);
    Presentation::for_grapheme(&sequence).1.is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rio_vt::ansi::CursorShape;
    use rio_vt::crosswords::pos::{Column, Line};
    use rio_vt::crosswords::{Crosswords, CrosswordsSize};
    use rio_vt::event::{VoidListener, WindowId};
    use rio_vt::performer::handler::Processor;

    #[test]
    fn measured_columns_match_rio_processor_grid() {
        for text in [
            "a",
            "中",
            "한",
            "あ",
            "　",
            "é",
            "©",
            "±",
            "🀄",
            "🃏",
            "⌚",
            "⏰",
            "✅",
            "❌",
            "🚀",
            "🎉",
            "e\u{301}",
            "❤️",
            "👩‍💻",
            "🇨🇳",
            "🇨🇳🇺🇸",
            "👍🏽",
            "1️⃣",
            "⌚︎",
        ] {
            let mut term = Crosswords::new(
                CrosswordsSize::new(80, 3),
                CursorShape::Block,
                VoidListener,
                WindowId::from(0),
                0,
                0,
            );
            let mut parser: Processor = Processor::default();
            parser.advance(&mut term, format!("{text}X").as_bytes());
            let column = (0..80)
                .find(|&col| term.grid[Line(0)][Column(col)].c() == 'X')
                .unwrap();
            assert_eq!(width(text), column, "{text:?}");
        }
    }

    #[test]
    fn editing_keeps_orphan_marks_visible_and_newlines_separate() {
        assert_eq!(width("\u{301}"), 1);
        assert_eq!(width("\u{301}\n\u{301}"), 2);
        assert_eq!(
            cells(&[0x0D, 0x0A, 0x301]).collect::<Vec<_>>(),
            [(1, 1), (1, 0), (1, 1)]
        );
    }
}
