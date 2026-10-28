//! Preserve terminal columns while grouping text into shaping segments.

use super::*;

#[derive(Debug, PartialEq)]
pub(super) enum RowSeg {
    Run {
        start: usize,
        cells: usize,
        text: String,
    },
    Wide {
        start: usize,
        cells: usize,
        text: SharedString,
    },
    Solo {
        col: usize,
    },
    Cluster {
        col: usize,
        cells: usize,
        text: String,
        wide_base: bool,
    },
}

fn push_cell(text: &mut String, cell: &RenderCell) {
    text.push(cell.c);
    text.extend(cell.marks.iter().flat_map(|marks| marks.iter()));
}

fn is_sara_am(c: char) -> bool {
    matches!(c, '\u{0E33}' | '\u{0EB3}')
}

fn sara_am_at(row: &[RenderCell], col: usize) -> Option<&RenderCell> {
    row.get(col)
        .filter(|cell| !cell.spacer && is_sara_am(cell.c))
}

fn is_regional_indicator(c: char) -> bool {
    matches!(c, '\u{1F1E6}'..='\u{1F1FF}')
}

fn regional_indicator_at(row: &[RenderCell], col: usize) -> Option<&RenderCell> {
    row.get(col)
        .filter(|cell| !cell.spacer && is_regional_indicator(cell.c))
}

pub(super) fn segment_row(row: &[RenderCell]) -> Vec<RowSeg> {
    let mut segs = Vec::new();
    let mut col = 0;
    while col < row.len() {
        let cell = &row[col];
        if cell.spacer {
            col += 1;
            continue;
        }
        if is_blank(cell) {
            let style = GlyphStyle::of(cell);
            if style.underline == UnderlineKind::None && !style.strikeout {
                col += 1;
                continue;
            }
            let start = col;
            let mut text = String::new();
            while col < row.len()
                && !row[col].spacer
                && is_blank(&row[col])
                && GlyphStyle::of(&row[col]) == style
            {
                text.push(' ');
                col += 1;
            }
            segs.push(RowSeg::Run {
                start,
                cells: col - start,
                text,
            });
            continue;
        }
        // Ahead of the marks branch: a stray mark on either half must not
        // split the pair, or the other half paints as a lettered box.
        if is_regional_indicator(cell.c)
            && let Some(next) = regional_indicator_at(row, col + 1)
        {
            let mut text = String::with_capacity(8);
            push_cell(&mut text, cell);
            push_cell(&mut text, next);
            segs.push(RowSeg::Cluster {
                col,
                cells: 2,
                text,
                wide_base: false,
            });
            col += 2;
            continue;
        }
        if let Some(marks) = &cell.marks {
            let wide_base = col + 1 < row.len() && row[col + 1].spacer;
            let mut cells = if wide_base { 2 } else { 1 };
            let mut text = String::with_capacity(1 + marks.len());
            push_cell(&mut text, cell);
            if !wide_base
                && !is_sara_am(cell.c)
                && let Some(am) = sara_am_at(row, col + 1)
            {
                push_cell(&mut text, am);
                cells = 2;
            }
            segs.push(RowSeg::Cluster {
                col,
                cells,
                text,
                wide_base,
            });
            col += cells;
            continue;
        }
        if !cell.c.is_ascii_graphic() {
            if col + 1 < row.len() && row[col + 1].spacer {
                segs.push(RowSeg::Wide {
                    start: col,
                    cells: 2,
                    text: char_string(cell.c),
                });
                col += 2;
            } else if !is_sara_am(cell.c)
                && let Some(am) = sara_am_at(row, col + 1)
            {
                let mut text = String::with_capacity(2);
                push_cell(&mut text, cell);
                push_cell(&mut text, am);
                segs.push(RowSeg::Cluster {
                    col,
                    cells: 2,
                    text,
                    wide_base: false,
                });
                col += 2;
            } else {
                segs.push(RowSeg::Solo { col });
                col += 1;
            }
            continue;
        }

        let style = GlyphStyle::of(cell);
        let start = col;
        let mut text = String::new();
        text.push(cell.c);
        let mut cells = 1;
        col += 1;
        let mut gap = 0;
        while col < row.len() {
            let c = &row[col];
            if is_blank(c) && !c.spacer {
                if style.draws_on_blanks() {
                    break;
                }
                gap += 1;
                col += 1;
                continue;
            }
            if c.spacer
                || c.marks.is_some()
                || !c.c.is_ascii_graphic()
                || GlyphStyle::of(c) != style
            {
                break;
            }
            for _ in 0..gap {
                text.push(' ');
            }
            cells += gap;
            gap = 0;
            text.push(c.c);
            cells += 1;
            col += 1;
        }
        segs.push(RowSeg::Run { start, cells, text });
    }
    segs
}

thread_local! {
    static CHAR_STRINGS: RefCell<HashMap<char, SharedString>> = RefCell::new(HashMap::new());
}

pub(super) fn char_string(c: char) -> SharedString {
    CHAR_STRINGS.with(|m| {
        let mut m = m.borrow_mut();
        if m.len() > 32_768 {
            m.clear();
        }
        m.entry(c)
            .or_insert_with(|| SharedString::from(c.to_string()))
            .clone()
    })
}

#[cfg(test)]
mod tests {
    use super::super::super::test_support::cell;
    use super::*;

    fn run(start: usize, cells: usize, text: &str) -> RowSeg {
        RowSeg::Run {
            start,
            cells,
            text: text.to_string(),
        }
    }

    fn wide(start: usize, cells: usize, text: &str) -> RowSeg {
        RowSeg::Wide {
            start,
            cells,
            text: text.into(),
        }
    }

    fn wide_cells(chars: &str) -> Vec<RenderCell> {
        let mut row = Vec::new();
        for c in chars.chars() {
            row.push(cell(c));
            let mut sp = cell(' ');
            sp.spacer = true;
            row.push(sp);
        }
        row
    }

    #[test]
    fn segment_row_batches_uniform_ascii() {
        let row: Vec<_> = "hello".chars().map(cell).collect();
        assert_eq!(segment_row(&row), [run(0, 5, "hello")]);
    }

    #[test]
    fn segment_row_joins_plain_runs_across_gaps_but_trims_edges() {
        let row: Vec<_> = " ab  cd  ".chars().map(cell).collect();
        assert_eq!(segment_row(&row), [run(1, 6, "ab  cd")]);
        let mut row: Vec<_> = "ab cd".chars().map(cell).collect();
        row[2].c = '\0';
        assert_eq!(segment_row(&row), [run(0, 5, "ab cd")]);
    }

    /// `paint_glyphs` reads a glyph's byte index as the column it came from
    /// and cuts a drifted run there, which only holds while a run is ASCII and
    /// one byte wide per cell. Pin that here rather than in the paint code.
    #[test]
    fn a_run_is_as_long_in_bytes_as_it_is_wide_in_cells() {
        let rows: [Vec<RenderCell>; 4] = [
            " ab  cd  ".chars().map(cell).collect(),
            "https://例/a".chars().map(cell).collect(),
            {
                let mut row: Vec<_> = "ab cd".chars().map(cell).collect();
                for c in &mut row {
                    c.underline = UnderlineKind::Single;
                }
                row
            },
            "x->y a//b".chars().map(cell).collect(),
        ];
        for row in rows {
            for seg in segment_row(&row) {
                if let RowSeg::Run { cells, text, .. } = seg {
                    assert!(text.is_ascii(), "{text:?} is not ASCII");
                    assert_eq!(text.len(), cells, "{text:?} does not span {cells} cells");
                }
            }
        }
    }

    #[test]
    fn segment_row_ends_underlined_runs_at_blanks() {
        let mut row: Vec<_> = "ab cd".chars().map(cell).collect();
        for c in &mut row {
            c.underline = UnderlineKind::Single;
        }
        assert_eq!(
            segment_row(&row),
            [run(0, 2, "ab"), run(2, 1, " "), run(3, 2, "cd")]
        );

        let mut row: Vec<_> = "ab cd".chars().map(cell).collect();
        for c in &mut row {
            c.link_hover = true;
        }
        assert_eq!(segment_row(&row), [run(0, 2, "ab"), run(3, 2, "cd")]);
    }

    #[test]
    fn segment_row_keeps_decorated_blank_runs() {
        let mut row: Vec<_> = "   ".chars().map(cell).collect();
        for c in &mut row {
            c.strikeout = true;
        }
        assert_eq!(segment_row(&row), [run(0, 3, "   ")]);

        for c in &mut row {
            c.strikeout = false;
            c.underline = UnderlineKind::Dotted;
        }
        assert_eq!(segment_row(&row), [run(0, 3, "   ")]);
    }

    #[test]
    fn segment_row_splits_on_style_changes() {
        let mut row: Vec<_> = "abcd".chars().map(cell).collect();
        row[2].fg = gpui_kit::red();
        row[3].fg = gpui_kit::red();
        assert_eq!(segment_row(&row), [run(0, 2, "ab"), run(2, 2, "cd")]);

        let mut row: Vec<_> = "abcd".chars().map(cell).collect();
        row[0].bold = true;
        assert_eq!(segment_row(&row), [run(0, 1, "a"), run(1, 3, "bcd")]);
    }

    #[test]
    fn segment_row_isolates_non_ascii() {
        let mut row: Vec<_> = "ok?字 no".chars().map(cell).collect();
        row.insert(4, {
            let mut sp = cell(' ');
            sp.spacer = true;
            sp
        });
        assert_eq!(
            segment_row(&row),
            [run(0, 3, "ok?"), wide(3, 2, "字"), run(6, 2, "no")]
        );

        let row: Vec<_> = "a─b".chars().map(cell).collect();
        assert_eq!(
            segment_row(&row),
            [run(0, 1, "a"), RowSeg::Solo { col: 1 }, run(2, 1, "b")]
        );
    }

    #[test]
    fn segment_row_keeps_powerline_separators_solo() {
        let row = vec![cell('a'), cell('\u{e0b0}'), cell('\u{e0b4}'), cell('b')];
        assert_eq!(
            segment_row(&row),
            [
                run(0, 1, "a"),
                RowSeg::Solo { col: 1 },
                RowSeg::Solo { col: 2 },
                run(3, 1, "b")
            ]
        );
    }

    #[test]
    fn segment_row_gives_each_wide_glyph_its_own_segment() {
        let row = wide_cells("你好世界");
        assert_eq!(
            segment_row(&row),
            [
                wide(0, 2, "你"),
                wide(2, 2, "好"),
                wide(4, 2, "世"),
                wide(6, 2, "界")
            ]
        );
    }

    #[test]
    fn segment_row_isolates_narrow_fullwidth_punctuation() {
        let row = wide_cells("（这样");
        assert_eq!(
            segment_row(&row),
            [wide(0, 2, "（"), wide(2, 2, "这"), wide(4, 2, "样")]
        );
    }

    #[test]
    fn segment_row_tracks_wide_columns_across_styles_and_gaps() {
        let mut row = wide_cells("你好世界");
        row[4].fg = gpui_kit::red();
        row[6].fg = gpui_kit::red();
        assert_eq!(
            segment_row(&row),
            [
                wide(0, 2, "你"),
                wide(2, 2, "好"),
                wide(4, 2, "世"),
                wide(6, 2, "界")
            ]
        );

        let mut row = wide_cells("你好");
        row.insert(2, cell(' '));
        assert_eq!(segment_row(&row), [wide(0, 2, "你"), wide(3, 2, "好")]);
    }

    #[test]
    fn segment_row_leaves_spacerless_wide_char_solo() {
        let row = vec![cell('a'), cell('字')];
        assert_eq!(segment_row(&row), [run(0, 1, "a"), RowSeg::Solo { col: 1 }]);
    }

    #[test]
    fn segment_row_ignores_blank_rows() {
        let row: Vec<_> = "  \0 ".chars().map(cell).collect();
        assert!(segment_row(&row).is_empty());
    }

    fn cluster(col: usize, cells: usize, text: &str) -> RowSeg {
        RowSeg::Cluster {
            col,
            cells,
            text: text.to_string(),
            wide_base: false,
        }
    }

    fn wide_cluster(col: usize, cells: usize, text: &str) -> RowSeg {
        RowSeg::Cluster {
            col,
            cells,
            text: text.to_string(),
            wide_base: true,
        }
    }

    #[test]
    fn segment_row_shapes_combining_marks_with_their_base() {
        let mut row = vec![cell('a'), cell('e'), cell('b')];
        row[1].marks = Some(Box::from(['\u{0301}']));
        assert_eq!(
            segment_row(&row),
            [run(0, 1, "a"), cluster(1, 1, "e\u{0301}"), run(2, 1, "b"),]
        );

        let mut row = wide_cells("\u{2764}");
        row[0].marks = Some(Box::from(['\u{FE0F}']));
        assert_eq!(segment_row(&row), [wide_cluster(0, 2, "\u{2764}\u{FE0F}")]);

        let mut row = vec![cell('\u{0E17}'), cell('a')];
        row[0].marks = Some(Box::from(['\u{0E35}', '\u{0E48}']));
        assert_eq!(
            segment_row(&row),
            [cluster(0, 1, "\u{0E17}\u{0E35}\u{0E48}"), run(1, 1, "a")]
        );
    }

    #[test]
    fn segment_row_absorbs_sara_am_into_its_base() {
        let mut row = vec![cell('\u{0E19}'), cell('\u{0E33}'), cell('a')];
        row[0].marks = Some(Box::from(['\u{0E49}']));
        assert_eq!(
            segment_row(&row),
            [cluster(0, 2, "\u{0E19}\u{0E49}\u{0E33}"), run(2, 1, "a")]
        );

        let row = vec![cell('\u{0E01}'), cell('\u{0E33}')];
        assert_eq!(segment_row(&row), [cluster(0, 2, "\u{0E01}\u{0E33}")]);

        let row = vec![cell('\u{0E81}'), cell('\u{0EB3}')];
        assert_eq!(segment_row(&row), [cluster(0, 2, "\u{0E81}\u{0EB3}")]);

        let mut row = vec![cell('\u{0E01}'), cell('\u{0E33}')];
        row[1].fg = gpui_kit::red();
        assert_eq!(segment_row(&row), [cluster(0, 2, "\u{0E01}\u{0E33}")]);
    }

    #[test]
    fn segment_row_leaves_a_baseless_sara_am_alone() {
        let row = vec![cell('\u{0E33}'), cell('a')];
        assert_eq!(segment_row(&row), [RowSeg::Solo { col: 0 }, run(1, 1, "a")]);

        let row = vec![cell(' '), cell('\u{0E33}')];
        assert_eq!(segment_row(&row), [RowSeg::Solo { col: 1 }]);

        let row = vec![cell('\u{0E33}'), cell('\u{0E33}')];
        assert_eq!(
            segment_row(&row),
            [RowSeg::Solo { col: 0 }, RowSeg::Solo { col: 1 }]
        );
    }

    #[test]
    fn segment_row_joins_a_regional_indicator_pair() {
        let row = vec![cell('\u{1F1E8}'), cell('\u{1F1F3}')];
        assert_eq!(segment_row(&row), [cluster(0, 2, "\u{1F1E8}\u{1F1F3}")]);

        let row = vec![cell('a'), cell('\u{1F1E8}'), cell('\u{1F1F3}'), cell('b')];
        assert_eq!(
            segment_row(&row),
            [
                run(0, 1, "a"),
                cluster(1, 2, "\u{1F1E8}\u{1F1F3}"),
                run(3, 1, "b"),
            ]
        );

        let row = vec![
            cell('\u{1F1E8}'),
            cell('\u{1F1F3}'),
            cell('\u{1F1FA}'),
            cell('\u{1F1F8}'),
        ];
        assert_eq!(
            segment_row(&row),
            [
                cluster(0, 2, "\u{1F1E8}\u{1F1F3}"),
                cluster(2, 2, "\u{1F1FA}\u{1F1F8}"),
            ]
        );

        let mut row = vec![cell('\u{1F1E8}'), cell('\u{1F1F3}')];
        row[1].fg = gpui_kit::red();
        assert_eq!(segment_row(&row), [cluster(0, 2, "\u{1F1E8}\u{1F1F3}")]);

        let mut row = vec![cell('\u{1F1E8}'), cell('\u{1F1F3}')];
        row[0].marks = Some(Box::from(['\u{FE0F}']));
        assert_eq!(
            segment_row(&row),
            [cluster(0, 2, "\u{1F1E8}\u{FE0F}\u{1F1F3}")]
        );

        let mut row = vec![cell('\u{1F1E8}'), cell('\u{1F1F3}')];
        row[1].marks = Some(Box::from(['\u{FE0F}']));
        assert_eq!(
            segment_row(&row),
            [cluster(0, 2, "\u{1F1E8}\u{1F1F3}\u{FE0F}")]
        );
    }

    #[test]
    fn segment_row_leaves_an_unpaired_regional_indicator_alone() {
        let row = vec![cell('\u{1F1E8}')];
        assert_eq!(segment_row(&row), [RowSeg::Solo { col: 0 }]);

        let row = vec![cell('\u{1F1E8}'), cell('\u{1F1F3}'), cell('\u{1F1FA}')];
        assert_eq!(
            segment_row(&row),
            [cluster(0, 2, "\u{1F1E8}\u{1F1F3}"), RowSeg::Solo { col: 2 }]
        );

        let row = vec![cell('\u{1F1E8}'), cell('a')];
        assert_eq!(segment_row(&row), [RowSeg::Solo { col: 0 }, run(1, 1, "a")]);

        let row = vec![cell('\u{1F1E8}'), cell(' '), cell('\u{1F1F3}')];
        assert_eq!(
            segment_row(&row),
            [RowSeg::Solo { col: 0 }, RowSeg::Solo { col: 2 }]
        );
    }

    #[test]
    fn segment_row_never_batches_a_marked_cell() {
        let mut row: Vec<_> = "abc".chars().map(cell).collect();
        row[1].marks = Some(Box::from(['\u{0301}']));
        assert_eq!(
            segment_row(&row),
            [run(0, 1, "a"), cluster(1, 1, "b\u{0301}"), run(2, 1, "c"),]
        );

        let mut row = wide_cells("你好世");
        row[2].marks = Some(Box::from(['\u{FE0F}']));
        assert_eq!(
            segment_row(&row),
            [
                wide(0, 2, "你"),
                wide_cluster(2, 2, "好\u{FE0F}"),
                wide(4, 2, "世"),
            ]
        );
    }

    #[test]
    fn segment_row_keeps_a_blank_that_carries_marks() {
        let mut row = vec![cell(' '), cell(' ')];
        row[0].marks = Some(Box::from(['\u{0301}']));
        assert_eq!(segment_row(&row), [cluster(0, 1, " \u{0301}")]);
    }

    #[test]
    fn char_string_memoizes_per_char() {
        let a = char_string('界');
        let b = char_string('界');
        assert_eq!(a, b);
        assert_eq!(a.as_ref(), "界");
    }
}
