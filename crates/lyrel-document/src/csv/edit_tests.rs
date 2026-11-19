//! 局部编辑的纯字节回归：只验证“哪些字节被替换、替换成什么”，
//! 不经过 GUI。工作台层的真实输入／历史／保存另见 `tests.rs`。

use super::super::CancelToken;
use super::super::table::DEFAULT_LIMITS;
use super::*;

fn parse(text: &str, revision: u64) -> CsvTable {
    CsvTable::parse(
        std::sync::Arc::new(text.to_owned()),
        revision,
        b',',
        DEFAULT_LIMITS,
        &CancelToken::new(),
    )
    .expect("valid table")
}

fn tsv(text: &str, revision: u64) -> CsvTable {
    CsvTable::parse(
        std::sync::Arc::new(text.to_owned()),
        revision,
        b'\t',
        DEFAULT_LIMITS,
        &CancelToken::new(),
    )
    .expect("valid TSV table")
}

fn cell(record: usize, column: usize) -> CsvCellId {
    CsvCellId::new(record, column)
}

/// 按 `Document::replace_ranges` 的降序规则应用替换，得到编辑后的源文。
fn apply(source: &str, replacements: &[(Range<usize>, String)]) -> String {
    let mut ordered: Vec<_> = replacements.to_vec();
    ordered.sort_by_key(|(range, _)| std::cmp::Reverse(range.start));
    let mut text = source.to_owned();
    let mut end = text.len();
    for (range, replacement) in ordered {
        assert!(range.end <= end, "replacements overlap: {range:?}");
        end = range.start;
        text.replace_range(range, &replacement);
    }
    text
}

fn edited(table: &CsvTable, edit: CsvEdit) -> String {
    let mutation = table.edit(table.revision(), edit).expect("edit");
    apply(table.source(), &mutation.replacements)
}

#[test]
fn a_cell_edit_rewrites_only_that_field_and_keeps_other_spelling() {
    let source = "\"a\",\"b,c\",\"line 1\nline 2\"\n\"q\",2,3\n";
    let table = parse(source, 7);
    assert_eq!(
        edited(
            &table,
            CsvEdit::Cell {
                cell: cell(0, 1),
                value: "new".into(),
            }
        ),
        "\"a\",new,\"line 1\nline 2\"\n\"q\",2,3\n"
    );
    assert_eq!(
        edited(
            &table,
            CsvEdit::Cell {
                cell: cell(0, 0),
                value: "a,b".into(),
            }
        ),
        "\"a,b\",\"b,c\",\"line 1\nline 2\"\n\"q\",2,3\n"
    );
    // 写回相同值不产生替换：已有引号拼写保持原样。
    let mutation = table
        .edit(
            7,
            CsvEdit::Cell {
                cell: cell(0, 0),
                value: "a".into(),
            },
        )
        .unwrap();
    assert!(!mutation.changed());
    // 单格编辑不碰其他记录、其他字段或终止符。
    let mutation = table
        .edit(
            7,
            CsvEdit::Cell {
                cell: cell(1, 1),
                value: "22".into(),
            },
        )
        .unwrap();
    assert_eq!(mutation.replacements.len(), 1);
    assert_eq!(mutation.replacements[0].0, 30..31);
}

#[test]
fn a_quoted_delimiter_and_newline_are_escaped_only_for_the_edited_field() {
    let table = parse("a,b\nc,d\n", 1);
    assert_eq!(
        edited(
            &table,
            CsvEdit::Cell {
                cell: cell(1, 0),
                value: "x,y".into(),
            }
        ),
        "a,b\n\"x,y\",d\n"
    );
    assert_eq!(
        edited(
            &table,
            CsvEdit::Cell {
                cell: cell(0, 1),
                value: "line 1\nline 2".into(),
            }
        ),
        "a,\"line 1\nline 2\"\nc,d\n"
    );
    assert_eq!(
        edited(
            &table,
            CsvEdit::Cell {
                cell: cell(0, 1),
                value: "say \"hi\"".into(),
            }
        ),
        "a,\"say \"\"hi\"\"\"\nc,d\n"
    );
}

#[test]
fn a_missing_trailing_field_is_appended_at_its_column() {
    let table = parse("a,b\nc\n", 2);
    assert_eq!(
        edited(
            &table,
            CsvEdit::Cell {
                cell: cell(1, 1),
                value: "z".into(),
            }
        ),
        "a,b\nc,z\n"
    );
    assert_eq!(
        edited(
            &table,
            CsvEdit::Cell {
                cell: cell(1, 1),
                value: String::new(),
            }
        ),
        "a,b\nc,\n"
    );
}

#[test]
fn paste_rewrites_each_target_record_once_and_keeps_untouched_fields() {
    let table = parse("a,b,c\nd,e,f\n", 3);
    let mutation = table
        .edit(
            3,
            CsvEdit::Paste {
                origin: cell(0, 1),
                rows: vec![
                    vec!["x,y".into(), "line 1\nline 2".into()],
                    vec!["q".into(), "r".into()],
                ],
            },
        )
        .unwrap();
    // 每条记录各自独立替换，不把整张表串行化。
    assert_eq!(mutation.replacements.len(), 4);
    assert_eq!(
        apply(table.source(), &mutation.replacements),
        "a,\"x,y\",\"line 1\nline 2\"\nd,q,r\n"
    );
}

#[test]
fn paste_preserves_identical_values_without_normalizing_their_quotes() {
    let table = parse("\"a\",\"b,c\"\nq,2\n", 4);
    let mutation = table
        .edit(
            4,
            CsvEdit::Paste {
                origin: cell(0, 0),
                rows: vec![vec!["a".into(), "b,c".into()]],
            },
        )
        .unwrap();
    assert!(!mutation.changed());
}

#[test]
fn paste_beyond_the_last_record_appends_aligned_rows() {
    let table = parse("a,b\nc,d\n", 5);
    let mutation = table
        .edit(
            5,
            CsvEdit::Paste {
                origin: cell(1, 1),
                rows: vec![vec!["q".into(), "r".into()], vec!["s".into(), "t".into()]],
            },
        )
        .unwrap();
    assert_eq!(
        apply(table.source(), &mutation.replacements),
        "a,b\nc,q,r\n,s,t\n"
    );
    // 没有末尾换行的文件不会凭空多出一个空行。
    let table = parse("a,b\nc,d", 6);
    let mutation = table
        .edit(
            6,
            CsvEdit::Paste {
                origin: cell(1, 0),
                rows: vec![vec!["x".into(), "y".into()]],
            },
        )
        .unwrap();
    assert_eq!(apply(table.source(), &mutation.replacements), "a,b\nx,y");
}

#[test]
fn paste_extends_short_records_with_empty_intermediate_fields() {
    let table = parse("a\nb,c\n", 7);
    let mutation = table
        .edit(
            7,
            CsvEdit::Paste {
                origin: cell(0, 1),
                rows: vec![vec!["p".into(), "q".into()]],
            },
        )
        .unwrap();
    assert_eq!(
        apply(table.source(), &mutation.replacements),
        "a,p,q\nb,c\n"
    );
    // 粘贴起点必须落在当前逻辑列内；短记录在起点之前的缺失列成为空字段。
    let table = parse("a\nb,c,d\n", 8);
    let mutation = table
        .edit(
            8,
            CsvEdit::Paste {
                origin: cell(0, 2),
                rows: vec![vec!["z".into()]],
            },
        )
        .unwrap();
    assert_eq!(
        apply(table.source(), &mutation.replacements),
        "a,,z\nb,c,d\n"
    );
    // 起点超出逻辑列时明确拒绝，而不是猜测新列位置。
    assert_eq!(
        parse("a\n", 9)
            .edit(
                9,
                CsvEdit::Paste {
                    origin: cell(0, 1),
                    rows: vec![vec!["z".into()]],
                },
            )
            .err(),
        Some(CsvEditError::Missing)
    );
}

#[test]
fn row_insert_keeps_neighbors_and_represents_a_single_empty_field() {
    let table = parse("a,b\nc,d", 9);
    assert_eq!(
        edited(
            &table,
            CsvEdit::InsertRow {
                at: CsvRecordId::from_index(0),
                below: true,
            }
        ),
        "a,b\n,\nc,d"
    );
    assert_eq!(
        edited(
            &table,
            CsvEdit::InsertRow {
                at: CsvRecordId::from_index(1),
                below: false,
            }
        ),
        "a,b\n,\nc,d"
    );
    // 单列空行必须写成 `""`，否则会被读取器当作空行跳过。
    let table = parse("x\ny\n", 10);
    assert_eq!(
        edited(
            &table,
            CsvEdit::InsertRow {
                at: CsvRecordId::from_index(0),
                below: true,
            }
        ),
        "x\n\"\"\ny\n"
    );
    let inserted = parse(
        &edited(
            &table,
            CsvEdit::InsertRow {
                at: CsvRecordId::from_index(0),
                below: true,
            },
        ),
        11,
    );
    assert_eq!(inserted.record_count(), 3);
    assert_eq!(inserted.cell_value(cell(1, 0)).as_deref(), Some(""));
}

#[test]
fn row_delete_removes_one_record_and_its_terminator_only() {
    let table = parse("a,b\nc,d\ne,f\n", 12);
    assert_eq!(
        edited(
            &table,
            CsvEdit::DeleteRow {
                at: CsvRecordId::from_index(1),
            }
        ),
        "a,b\ne,f\n"
    );
    let table = parse("a\nb", 13);
    assert_eq!(
        edited(
            &table,
            CsvEdit::DeleteRow {
                at: CsvRecordId::from_index(1),
            }
        ),
        "a"
    );
    // 只有 CR 终止符的旧式文件同样只删除该记录。
    let table = parse("a\rb\r", 14);
    assert_eq!(
        edited(
            &table,
            CsvEdit::DeleteRow {
                at: CsvRecordId::from_index(0),
            }
        ),
        "b\r"
    );
    // 引用字段内部的换行属于记录内容，不参与记录边界。
    let table = parse("\"a\nb\",c\nd,e\n", 15);
    assert_eq!(
        edited(
            &table,
            CsvEdit::DeleteRow {
                at: CsvRecordId::from_index(0),
            }
        ),
        "d,e\n"
    );
}

#[test]
fn column_insert_keeps_ragged_records_ragged() {
    let table = parse("a,b\nc\n", 16);
    assert_eq!(
        edited(&table, CsvEdit::InsertColumn { at: 0, right: true }),
        "a,,b\nc,\n"
    );
    assert_eq!(
        edited(
            &table,
            CsvEdit::InsertColumn {
                at: 0,
                right: false,
            }
        ),
        ",a,b\n,c\n"
    );
    // 短记录没有目标列时不补出中间字段。
    let table = parse("a\nb,c,d\n", 17);
    assert_eq!(
        edited(&table, CsvEdit::InsertColumn { at: 2, right: true }),
        "a\nb,c,d,\n"
    );
}

#[test]
fn column_delete_removes_one_separator_and_keeps_single_field_records() {
    let table = parse("a,b,c\nx,y\nq\n", 18);
    assert_eq!(
        edited(&table, CsvEdit::DeleteColumn { at: 1 }),
        "a,c\nx\nq\n"
    );
    assert_eq!(
        edited(&table, CsvEdit::DeleteColumn { at: 2 }),
        "a,b\nx,y\nq\n"
    );
    let table = parse("q\nr\n", 19);
    assert_eq!(
        edited(&table, CsvEdit::DeleteColumn { at: 0 }),
        "\"\"\n\"\"\n"
    );
    let after = parse("\"\"\n", 20);
    assert_eq!(after.record_count(), 1);
}

#[test]
fn a_stale_revision_is_refused_without_producing_edits() {
    let table = parse("a,b\n", 21);
    assert_eq!(
        table
            .edit(
                22,
                CsvEdit::Cell {
                    cell: cell(0, 0),
                    value: "x".into(),
                }
            )
            .err(),
        Some(CsvEditError::Stale)
    );
    assert_eq!(
        table
            .edit(
                22,
                CsvEdit::DeleteRow {
                    at: CsvRecordId::from_index(0),
                }
            )
            .err(),
        Some(CsvEditError::Stale)
    );
}

#[test]
fn out_of_range_targets_are_missing_not_guessed() {
    let table = parse("a,b\n", 23);
    assert_eq!(
        table
            .edit(
                23,
                CsvEdit::Cell {
                    cell: cell(1, 0),
                    value: "x".into(),
                }
            )
            .err(),
        Some(CsvEditError::Missing)
    );
    assert_eq!(
        table
            .edit(
                23,
                CsvEdit::Cell {
                    cell: cell(0, 2),
                    value: "x".into(),
                }
            )
            .err(),
        Some(CsvEditError::Missing)
    );
    assert_eq!(
        table.edit(23, CsvEdit::DeleteColumn { at: 2 }).err(),
        Some(CsvEditError::Missing)
    );
}

#[test]
fn tsv_selection_is_exact_for_one_cell_and_quoted_for_many() {
    let table = tsv("\"a\tb\"\t\"line 1\nline 2\"\nx\ty\n", 24);
    assert_eq!(
        table.selection_text(cell(0, 0), cell(0, 0)).as_deref(),
        Some("a\tb")
    );
    assert_eq!(
        table.selection_text(cell(0, 0), cell(1, 1)).as_deref(),
        Some("\"a\tb\"\t\"line 1\nline 2\"\nx\ty")
    );
    // 缺失字段在多格复制中成为空字段；单格复制仍返回 None。
    let ragged = parse("a,b\nc\n", 25);
    assert_eq!(ragged.selection_text(cell(1, 1), cell(1, 1)), None);
    assert_eq!(
        ragged.selection_text(cell(0, 0), cell(1, 1)).as_deref(),
        Some("a\tb\nc\t")
    );
}

#[test]
fn clipboard_parsing_handles_quoted_tabs_newlines_and_ragged_rows() {
    assert_eq!(
        parse_clipboard("\"a\tb\"\t\"line 1\nline 2\"\nx\ty"),
        vec![
            vec!["a\tb".to_owned(), "line 1\nline 2".to_owned()],
            vec!["x".to_owned(), "y".to_owned()],
        ]
    );
    assert_eq!(parse_clipboard("one\n"), vec![vec!["one".to_owned()]]);
    assert_eq!(parse_clipboard(""), Vec::<Vec<String>>::new());
    // 无法解析的引号退回单格原文，不丢弃用户复制的内容。
    assert_eq!(
        parse_clipboard("\"unterminated"),
        vec![vec!["\"unterminated".to_owned()]]
    );
}

#[test]
fn deleting_the_only_record_leaves_an_empty_source() {
    for source in ["a,b\n", "a,b"] {
        let table = parse(source, 27);
        let mutation = table
            .edit(
                27,
                CsvEdit::DeleteRow {
                    at: CsvRecordId::from_index(0),
                },
            )
            .unwrap();
        let edited = apply(table.source(), &mutation.replacements);
        assert_eq!(edited, "", "source {source:?}");
        assert_eq!(parse(&edited, 28).record_count(), 0);
    }
}

#[test]
fn a_copied_selection_pastes_back_into_the_same_values() {
    let source = "\"a\tb\"\t\"line 1\nline 2\"\nx\ty\n";
    let table = tsv(source, 26);
    let text = table
        .selection_text(cell(0, 0), cell(1, 1))
        .expect("selection");
    let rows = parse_clipboard(&text);
    let mutation = table
        .edit(
            26,
            CsvEdit::Paste {
                origin: cell(0, 0),
                rows,
            },
        )
        .unwrap();
    assert!(!mutation.changed(), "round-trip paste is a no-op");
}
