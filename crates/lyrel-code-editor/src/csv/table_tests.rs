//! 移植自 Glim CSV reader 的纯读取／分页／完整值回归，以及本票新增的源区间、
//! 稳定身份、显示预算与只读查询。输入全部由测试自身生成，不依赖协调目录夹具。

use super::*;

/// 规范源文（LF、无 BOM）的样本：quoted comma、quoted 多行、双引号转义、
/// Unicode 与不等长记录；BOM 与逐行磁盘行尾由工作台层的 Document 保存。
const SAMPLE: &str = "name,value,note\n\"中文\",\"a,b\",\"line 1\nline 2 \"\"yes\"\"\"\nalpha,1\n\"multi\nline\",2,3\n";

fn parse(text: &str) -> CsvTable {
    CsvTable::parse(
        Arc::new(text.to_owned()),
        7,
        b',',
        DEFAULT_LIMITS,
        &AtomicBool::new(false),
    )
    .expect("valid table")
}

#[test]
fn records_fields_and_source_spans_survive_quotes_multiline_and_ragged_rows() {
    let table = parse(SAMPLE);
    assert_eq!(table.revision(), 7);
    assert_eq!(table.record_count(), 4);
    assert_eq!(table.column_count(), 3);

    let row = |record: usize, column: usize| {
        table
            .cell_value(CsvCellId::new(record, column))
            .expect("field")
    };
    assert_eq!(row(0, 0), "name");
    assert_eq!(row(1, 0), "中文");
    assert_eq!(row(1, 1), "a,b");
    assert_eq!(row(1, 2), "line 1\nline 2 \"yes\"");
    assert_eq!(row(3, 0), "multi\nline");
    assert_eq!(row(2, 0), "alpha");
    assert_eq!(table.cell_value(CsvCellId::new(2, 2)), None);

    // 原始区间含引号与转义拼写，去掉分隔符；这是 #288 局部写回的前提。
    assert_eq!(table.cell_source(CsvCellId::new(1, 1)), Some("\"a,b\""));
    assert_eq!(
        table.cell_source(CsvCellId::new(1, 2)),
        Some("\"line 1\nline 2 \"\"yes\"\"\"")
    );
    assert_eq!(
        &SAMPLE[table.record_span(CsvRecordId(0)).unwrap()],
        "name,value,note"
    );
    assert_eq!(
        &SAMPLE[table.record_span(CsvRecordId(2)).unwrap()],
        "alpha,1"
    );
    let multiline = table.record_span(CsvRecordId(3)).unwrap();
    assert!(SAMPLE[multiline.clone()].contains('\n'));
    assert!(!SAMPLE[multiline].ends_with('\n'));

    // 物理行不是记录：两条 quoted 多行记录占两行，总记录数仍是 4。
    assert_eq!(table.record_line(CsvRecordId(0)), Some(1));
    assert_eq!(table.record_line(CsvRecordId(1)), Some(2));
    assert_eq!(table.record_line(CsvRecordId(2)), Some(4));
    assert_eq!(table.record_line(CsvRecordId(3)), Some(5));
    assert_eq!(SAMPLE.lines().count(), 6);
}

#[test]
fn complete_logical_columns_are_retained_without_a_page_window() {
    let line = (0..300)
        .map(|number| number.to_string())
        .collect::<Vec<_>>()
        .join(",");
    let input = format!("{line}\n{line}\n");
    let table = parse(&input);
    assert_eq!(table.column_count(), 300);
    assert_eq!(table.record_count(), 2);
    assert_eq!(
        table.cell_value(CsvCellId::new(1, 299)),
        Some("299".to_owned())
    );
}

#[test]
fn a_quoted_field_may_end_the_file_without_a_trailing_newline() {
    let table = parse("a,\"b\"");
    assert_eq!(table.record_count(), 1);
    assert_eq!(table.cell_value(CsvCellId::new(0, 1)), Some("b".to_owned()));
    let table = parse("a,\"b\"\"\"");
    assert_eq!(
        table.cell_value(CsvCellId::new(0, 1)),
        Some("b\"".to_owned())
    );
}

#[test]
fn incomplete_or_unrepresentable_quoting_is_reported_with_its_record_line() {
    let error = CsvTable::parse(
        Arc::new("a,b\n\"unterminated\n".to_owned()),
        1,
        b',',
        DEFAULT_LIMITS,
        &AtomicBool::new(false),
    )
    .err()
    .expect("unterminated quote");
    assert_eq!(
        error,
        CsvTableError::Issue(CsvIssue {
            kind: CsvIssueKind::UnterminatedQuote,
            line: 2,
        })
    );

    let error = CsvTable::parse(
        Arc::new("a,b\n\"a\"x,c\n".to_owned()),
        1,
        b',',
        DEFAULT_LIMITS,
        &AtomicBool::new(false),
    )
    .err()
    .expect("characters after a closing quote");
    assert_eq!(
        error,
        CsvTableError::Issue(CsvIssue {
            kind: CsvIssueKind::UnsupportedSyntax,
            line: 2,
        })
    );
}

#[test]
fn every_budget_refuses_the_table_instead_of_returning_a_partial_one() {
    let cases = [
        (
            CsvLimits {
                source_bytes: SAMPLE.len() - 1,
                ..DEFAULT_LIMITS
            },
            TableLimit::SourceBytes,
        ),
        (
            CsvLimits {
                records: 3,
                ..DEFAULT_LIMITS
            },
            TableLimit::Records,
        ),
        (
            CsvLimits {
                columns: 2,
                ..DEFAULT_LIMITS
            },
            TableLimit::Columns,
        ),
        (
            CsvLimits {
                cells: 2,
                ..DEFAULT_LIMITS
            },
            TableLimit::Cells,
        ),
    ];
    for (limits, expected) in cases {
        let error = CsvTable::parse(
            Arc::new(SAMPLE.to_owned()),
            1,
            b',',
            limits,
            &AtomicBool::new(false),
        )
        .err()
        .expect("budget refusal");
        assert_eq!(error, CsvTableError::Budget(expected));
    }
}

#[test]
fn cancellation_returns_no_table() {
    let cancel = AtomicBool::new(true);
    assert_eq!(
        CsvTable::parse(
            Arc::new(SAMPLE.to_owned()),
            1,
            b',',
            DEFAULT_LIMITS,
            &cancel,
        )
        .err()
        .expect("cancelled"),
        CsvTableError::Cancelled
    );
}

#[test]
fn queries_change_only_the_visible_order() {
    let table = parse("b,2\na,1\nc,3\n");
    let cancel = AtomicBool::new(false);
    let all = |sort| table.query("", sort, &cancel).expect("query").records;
    assert_eq!(all(None), vec![0, 1, 2]);
    assert_eq!(
        all(Some(CsvSort {
            column: 0,
            descending: false
        })),
        vec![1, 0, 2]
    );
    assert_eq!(
        all(Some(CsvSort {
            column: 0,
            descending: true
        })),
        vec![2, 0, 1]
    );
    assert_eq!(
        table
            .query(
                "A",
                Some(CsvSort {
                    column: 1,
                    descending: false
                }),
                &cancel
            )
            .expect("filtered")
            .records,
        vec![1]
    );

    // 过滤和排序没有重排文件顺序，也没有移动字段区间。
    assert_eq!(table.cell_value(CsvCellId::new(0, 0)), Some("b".into()));
    assert_eq!(table.cell_source(CsvCellId::new(0, 1)), Some("2"));
}

#[test]
fn display_budgets_keep_full_values_available() {
    let long = "x".repeat(DETAIL_CHARS + 100);
    let input = format!("head\n\"{long}\"\n");
    let table = parse(&input);
    let cell = CsvCellId::new(1, 0);
    assert_eq!(
        table.cell_value(cell).expect("full value").chars().count(),
        DETAIL_CHARS + 100
    );
    assert_eq!(
        table.cell_excerpt(cell).expect("excerpt").chars().count(),
        CELL_EXCERPT_CHARS
    );
    let (shown, truncated) = table.detail_text(cell).expect("detail");
    assert!(truncated);
    assert_eq!(shown.chars().count(), DETAIL_CHARS);
}
