//! 输入栏与 CLI 表格共用 Rio 默认 mode 2027 的占列规则。

use smallvec::SmallVec;

/// 依次返回 `(字符数, 列宽)`，不分配内存。
///
/// 分段和 emoji 占列直接调用正式 Rio API；这里只保留应用自己的两项
/// 编辑契约：换行单独结束一行，无 base 的 mark 仍占一列以便显示和定位。
/// CR 不与 LF 合并，保证输入栏的字符索引和硬换行保持一一对应。
pub fn cells(mut codepoints: &[u32]) -> impl Iterator<Item = (usize, usize)> + '_ {
    std::iter::from_fn(move || {
        let &first = codepoints.first()?;
        let (len, width) = match first {
            0x0A => (1, 0),
            0x0D => (1, 1),
            _ => {
                let (len, width) = rio_vt::grapheme_lut::cluster_width(codepoints);
                (len, usize::from(width).max(1))
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
