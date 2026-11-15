//! CSV/TSV 的生产工作台回归；自有及固定 Glim MIT 夹具，不声称真实 SSH 或原生窗口通过。
use super::super::test_host::{FileHost, canonical_tempdir};
use super::super::*;
use super::*;
use crate::ui::app::test_window::harness_with_tabs;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{TestAppContext, VisualTestContext, size};
use std::path::Path;
use std::time::{Duration, Instant};

fn settle(cx: &mut VisualTestContext) {
    cx.run_until_parked();
    cx.update(|window, cx| {
        window.simulate_next_frame(cx);
        window.render_frame(cx);
    });
    cx.run_until_parked();
}

#[track_caller]
fn until(cx: &mut VisualTestContext, mut ready: impl FnMut(&mut VisualTestContext) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !ready(cx) {
        assert!(
            Instant::now() < deadline,
            "CSV workbench transition did not settle"
        );
        settle(cx);
        cx.executor().advance_clock(Duration::from_millis(50));
        std::thread::sleep(Duration::from_millis(5));
    }
    settle(cx);
}

fn view(app: &Entity<LyrelApp>, cx: &VisualTestContext) -> Entity<CsvView> {
    app.read_with(cx, |app, _| {
        app.active_editor_input().unwrap().csv().unwrap().clone()
    })
}

fn open(app: &Entity<LyrelApp>, cx: &mut VisualTestContext, host: SharedHost, path: &Path) {
    app.update_in(cx, |app, window, cx| {
        app.editor_open_on_host(host, path, window, cx)
    });
    until(cx, |cx| {
        app.read_with(cx, |app, _| {
            app.active_buffer()
                .is_some_and(|file| file.path == path && file.input.csv().is_some())
        })
    });
}

fn ready(app: &Entity<LyrelApp>, cx: &mut VisualTestContext, records: usize) {
    until(cx, |cx| {
        view(app, cx).read_with(cx, |view, cx| {
            view.facts(cx).is_some_and(|(count, _)| count == records)
                && view.visible_count() == records
                && !view.is_parsing(cx)
        })
    });
}

fn source(app: &Entity<LyrelApp>, cx: &VisualTestContext) -> String {
    view(app, cx).read_with(cx, |view, cx| {
        view.document()
            .read(cx)
            .document()
            .read(cx)
            .source()
            .to_owned()
    })
}

fn save(app: &Entity<LyrelApp>, cx: &mut VisualTestContext) {
    cx.dispatch_action(crate::core::actions::EditorSave);
    until(cx, |cx| {
        app.read_with(cx, |app, _| {
            app.active_buffer()
                .is_some_and(|file| !file.dirty && !file.saving)
        })
    });
}

fn chord(keys: &str) -> String {
    format!(
        "{}-{keys}",
        if cfg!(target_os = "macos") {
            "cmd"
        } else {
            "ctrl"
        }
    )
}

fn enter_path(cx: &mut VisualTestContext, path: &Path) {
    cx.update(|window, cx| {
        window.press("secondary-a", cx);
        window.input(&path.display().to_string(), cx);
        window.press("enter", cx);
    });
}

#[gpui_kit::test]
fn mapped_csv_sort_filter_full_value_and_source_save_preserve_original_bytes(
    tcx: &mut TestAppContext,
) {
    crate::ui::i18n::set_locale("en");
    let root = canonical_tempdir();
    let remote = canonical_tempdir();
    let path = Path::new("/owned.csv");
    let decoy = root.path().join("owned.csv");
    std::fs::write(&decoy, b"LOCAL DECOY").unwrap();
    let owner_file = remote.path().join("owned.csv");
    let long = "full value beyond the visible excerpt ".repeat(8);
    let original =
        format!("\u{feff}b,\"quoted,comma\",\"line 1\r\nline 2\"\na,中文,{long}\r\nragged,only")
            .into_bytes();
    std::fs::write(&owner_file, &original).unwrap();
    let host = FileHost::mapped(HostId(287101), remote.path());
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    cx.simulate_resize(size(px(1800.), px(1000.)));
    open(&app, &mut cx, host, &path);
    ready(&app, &mut cx, 3);
    let csv = view(&app, &cx);
    let id = csv.entity_id();
    cx.update(|window, cx| {
        window.click(cell_element_id(id, 0, 1, TAG_CELL), cx);
    });
    settle(&mut cx);
    assert_eq!(
        csv.read_with(&cx, |view, cx| view.selected_value(cx)),
        Some("quoted,comma".into())
    );
    cx.update(|window, cx| window.click(column_element_id(id, 0), cx));
    until(&mut cx, |cx| {
        csv.read_with(cx, |view, _| view.order() == [1, 0, 2])
    });
    cx.update(|window, cx| {
        window.click(cell_element_id(id, 1, 2, TAG_CELL), cx);
    });
    settle(&mut cx);
    cx.update(|window, cx| window.click(("csv-copy", id), cx));
    cx.update(|_, cx| {
        assert_eq!(
            cx.read_from_clipboard().and_then(|item| item.text()),
            Some(long.clone())
        )
    });
    let (release, entered) = test_support::pause_next_query();
    cx.update(|window, cx| {
        window.click(("csv-filter", id), cx);
        assert_eq!(
            window.find(("csv-filter", id)).focused(),
            Some(true),
            "clicking the filter keeps text input focus"
        );
        // One keystroke starts the query that is deliberately held below.
        // Typing two characters could cancel that query before its worker starts.
        window.input("中", cx);
        assert_eq!(window.find(("csv-filter", id)).value(), Some("中"));
    });
    until(&mut cx, |_| entered.load(Ordering::Acquire));
    cx.update(|window, cx| {
        window.press(&chord("a"), cx);
        window.input("ragged", cx);
    });
    release.send(()).unwrap();
    until(&mut cx, |cx| {
        csv.read_with(cx, |view, _| view.order() == [2])
    });
    cx.update(|window, cx| {
        window.press(&chord("a"), cx);
        window.input("中文", cx);
    });
    until(&mut cx, |cx| {
        csv.read_with(cx, |view, _| view.order() == [1])
    });
    assert_eq!(std::fs::read(&owner_file).unwrap(), original);
    cx.update(|window, cx| window.click(("csv-filter-clear", id), cx));
    until(&mut cx, |cx| {
        csv.read_with(cx, |view, _| view.visible_count() == 3)
    });
    cx.update(|window, cx| {
        window.click(cell_element_id(id, 1, 1, TAG_CELL), cx);
        window.press("enter", cx);
    });
    settle(&mut cx);
    assert_eq!(csv.read_with(&cx, |view, _| view.mode()), CsvMode::Source);
    cx.update(|window, cx| window.input("更新", cx));
    until(&mut cx, |cx| source(&app, cx).contains("a,更新,"));
    save(&app, &mut cx);
    let expected = String::from_utf8(original.clone())
        .unwrap()
        .replace("a,中文,", "a,更新,")
        .into_bytes();
    assert_eq!(std::fs::read(&owner_file).unwrap(), expected);
    assert_eq!(std::fs::read(&decoy).unwrap(), b"LOCAL DECOY");
    app.update_in(&mut cx, |app, window, cx| {
        app.editor_close_file(0, window, cx)
    });
    settle(&mut cx);
    open(
        &app,
        &mut cx,
        FileHost::mapped(HostId(287101), remote.path()),
        &path,
    );
    ready(&app, &mut cx, 3);
    assert_eq!(
        view(&app, &cx).read_with(&cx, |view, cx| view
            .table(cx)
            .unwrap()
            .cell_value(CsvCellId::new(1, 1))),
        Some("更新".into())
    );
}

#[gpui_kit::test]
fn utf16_tsv_source_history_and_stale_parse_keep_exact_values(tcx: &mut TestAppContext) {
    let dir = canonical_tempdir();
    let path = dir.path().join("history.tsv");
    let text = "甲\t\"quoted\tvalue\"\r\n乙\tsecond\n";
    let mut original = vec![0xff, 0xfe];
    for unit in text.encode_utf16() {
        original.extend_from_slice(&unit.to_le_bytes());
    }
    std::fs::write(&path, &original).unwrap();
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    open(&app, &mut cx, FileHost::new(HostId(287102)), &path);
    ready(&app, &mut cx, 2);
    save(&app, &mut cx);
    assert_eq!(std::fs::read(&path).unwrap(), original);
    let csv = view(&app, &cx);
    let id = csv.entity_id();
    cx.update(|window, cx| window.click(("csv-mode-toggle", id), cx));
    settle(&mut cx);
    let (release, entered) = test_support::pause_next_parse();
    app.update_in(&mut cx, |app, window, cx| {
        app.focus_editor(window, cx);
        let active = app.active_editor_input().unwrap();
        let input = active.text().unwrap();
        input.replace(0.."甲".len(), "丙", window, cx);
    });
    until(&mut cx, |_| entered.load(Ordering::Acquire));
    let at = source(&app, &cx).find("second").unwrap();
    app.update_in(&mut cx, |app, window, cx| {
        app.active_editor_input()
            .unwrap()
            .text()
            .unwrap()
            .replace(at..at + 6, "third", window, cx);
    });
    release.send(()).unwrap();
    ready(&app, &mut cx, 2);
    csv.read_with(&cx, |view, cx| {
        let table = view.table(cx).unwrap();
        assert_eq!(
            table.cell_value(CsvCellId::new(0, 0)).as_deref(),
            Some("丙")
        );
        assert_eq!(
            table.cell_value(CsvCellId::new(1, 1)).as_deref(),
            Some("third")
        );
    });
    cx.update(|window, cx| window.press(&chord("z"), cx));
    ready(&app, &mut cx, 2);
    assert!(source(&app, &cx).contains("second"));
    cx.update(|window, cx| window.press(&chord("shift-z"), cx));
    ready(&app, &mut cx, 2);
    assert!(source(&app, &cx).contains("third"));
    let end = source(&app, &cx).len();
    app.update_in(&mut cx, |app, window, cx| {
        app.active_editor_input().unwrap().text().unwrap().replace(
            end..end,
            "\"unfinished",
            window,
            cx,
        );
    });
    until(&mut cx, |cx| {
        csv.read_with(cx, |view, cx| view.parse_error(cx).is_some())
    });
    cx.update(|window, cx| window.click(("csv-mode-toggle", id), cx));
    settle(&mut cx);
    cx.update(|window, _| assert!(window.find(("csv-table-error", id)).visible()));
    cx.update(|window, cx| window.click(("csv-show-source", id), cx));
    cx.update(|window, cx| window.press(&chord("z"), cx));
    ready(&app, &mut cx, 2);
    save(&app, &mut cx);
    let expected = text.replace('甲', "丙").replace("second", "third");
    let mut bytes = vec![0xff, 0xfe];
    for unit in expected.encode_utf16() {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
}

#[gpui_kit::test]
fn upstream_table_contract_keeps_multiline_missing_and_empty_fields(tcx: &mut TestAppContext) {
    let dir = canonical_tempdir();
    let path = dir.path().join("contract.csv");
    let original = include_bytes!("fixtures/table.csv");
    let expected: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/table.json")).unwrap();
    let rows = expected["rows"].as_array().unwrap();
    let columns = expected["columns"].as_u64().unwrap() as usize;
    std::fs::write(&path, original).unwrap();
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    open(&app, &mut cx, FileHost::new(HostId(287103)), &path);
    ready(&app, &mut cx, rows.len());
    let csv = view(&app, &cx);
    let id = csv.entity_id();
    csv.read_with(&cx, |view, cx| {
        assert_eq!(view.facts(cx), Some((rows.len(), columns)));
        let table = view.table(cx).unwrap();
        for (record, row) in rows.iter().enumerate() {
            for column in 0..columns {
                assert_eq!(
                    table.cell_value(CsvCellId::new(record, column)).as_deref(),
                    row.get(column).and_then(serde_json::Value::as_str),
                    "logical record {record}, column {column}"
                );
            }
        }
    });
    cx.update(|window, cx| {
        window.click(cell_element_id(id, 1, 2, TAG_CELL), cx);
    });
    settle(&mut cx);
    assert_eq!(
        csv.read_with(&cx, |view, cx| view.selected_value(cx))
            .as_deref(),
        rows[1][2].as_str()
    );
    cx.update(|window, cx| window.click(("csv-copy", id), cx));
    cx.update(|_, cx| {
        assert_eq!(
            cx.read_from_clipboard()
                .and_then(|item| item.text())
                .as_deref(),
            rows[1][2].as_str()
        );
    });
    save(&app, &mut cx);
    assert_eq!(std::fs::read(&path).unwrap(), original.as_slice());
}

fn cell_id(view: gpui_kit::EntityId, record: usize, column: usize) -> gpui_kit::ElementId {
    cell_element_id(view, record, column, TAG_CELL)
}

fn editor_id(view: gpui_kit::EntityId, record: usize, column: usize) -> gpui_kit::ElementId {
    cell_element_id(view, record, column, TAG_EDITOR)
}

/// 真实指针开始单元格编辑，真实文本输入写入，回车提交。
fn edit_cell(
    cx: &mut VisualTestContext,
    view: gpui_kit::EntityId,
    record: usize,
    column: usize,
    text: &str,
) {
    cx.update(|window, cx| window.double_click(cell_id(view, record, column), cx));
    settle(cx);
    cx.update(|window, _| {
        assert!(
            window.try_find(editor_id(view, record, column)).is_some(),
            "double-click opens the real cell input"
        );
    });
    cx.update(|window, cx| {
        window.input(text, cx);
        window.press("enter", cx);
    });
    settle(cx);
}

fn value(
    app: &Entity<LyrelApp>,
    cx: &VisualTestContext,
    record: usize,
    column: usize,
) -> Option<String> {
    view(app, cx).read_with(cx, |view, cx| {
        view.table(cx)
            .and_then(|table| table.cell_value(CsvCellId::new(record, column)))
    })
}

fn table_state(app: &Entity<LyrelApp>, cx: &VisualTestContext) -> Option<(usize, usize)> {
    view(app, cx).read_with(cx, |view, cx| view.facts(cx))
}

#[gpui_kit::test]
fn cell_input_undo_redo_save_and_reopen_preserve_untouched_bytes(tcx: &mut TestAppContext) {
    crate::ui::i18n::set_locale("en");
    let root = canonical_tempdir();
    let remote = canonical_tempdir();
    let path = Path::new("/editable.csv");
    let decoy = root.path().join("editable.csv");
    std::fs::write(&decoy, b"LOCAL DECOY").unwrap();
    let owner_file = remote.path().join("editable.csv");
    let original =
        "\u{feff}name,value,note\r\na,\"b,c\",\"line 1\nline 2\"\r\nq,1\nragged,only\r\n"
            .as_bytes()
            .to_vec();
    std::fs::write(&owner_file, &original).unwrap();
    let host = FileHost::mapped(HostId(288101), remote.path());
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    cx.simulate_resize(size(px(1800.), px(1000.)));
    open(&app, &mut cx, host, &path);
    ready(&app, &mut cx, 4);
    let csv = view(&app, &cx);
    let id = csv.entity_id();

    edit_cell(&mut cx, id, 1, 1, "更新");
    until(&mut cx, |cx| {
        value(&app, cx, 1, 1).as_deref() == Some("更新")
            && !view(&app, cx).read_with(cx, |v, cx| v.is_parsing(cx))
    });
    assert_eq!(
        source(&app, &cx),
        "name,value,note\na,更新,\"line 1\nline 2\"\nq,1\nragged,only\n"
    );
    assert_eq!(
        std::fs::read(&owner_file).unwrap(),
        original,
        "a table edit is not an implicit save"
    );
    assert!(app.read_with(&cx, |app, _| app.active_buffer().unwrap().dirty));

    // Table-mode undo/redo are the shared Document history, not a table copy.
    cx.update(|window, cx| window.press(&chord("z"), cx));
    until(&mut cx, |cx| {
        value(&app, cx, 1, 1).as_deref() == Some("b,c")
            && !view(&app, cx).read_with(cx, |v, cx| v.is_parsing(cx))
    });
    assert!(source(&app, &cx).contains("\"b,c\""));
    cx.update(|window, cx| window.press(&chord("shift-z"), cx));
    until(&mut cx, |cx| {
        value(&app, cx, 1, 1).as_deref() == Some("更新")
            && !view(&app, cx).read_with(cx, |v, cx| v.is_parsing(cx))
    });

    save(&app, &mut cx);
    let expected = "\u{feff}name,value,note\r\na,更新,\"line 1\nline 2\"\r\nq,1\nragged,only\r\n"
        .as_bytes()
        .to_vec();
    assert_eq!(std::fs::read(&owner_file).unwrap(), expected);
    assert_eq!(std::fs::read(&decoy).unwrap(), b"LOCAL DECOY");

    app.update_in(&mut cx, |app, window, cx| {
        app.editor_close_file(0, window, cx)
    });
    until(&mut cx, |cx| {
        app.read_with(cx, |app, _| app.editor.buffers.is_empty())
    });
    open(
        &app,
        &mut cx,
        FileHost::mapped(HostId(288101), remote.path()),
        &path,
    );
    ready(&app, &mut cx, 4);
    assert_eq!(value(&app, &cx, 1, 1).as_deref(), Some("更新"));
    assert_eq!(std::fs::read(&owner_file).unwrap(), expected);
    assert_eq!(std::fs::read(&decoy).unwrap(), b"LOCAL DECOY");
}

#[gpui_kit::test]
fn tabular_paste_uses_file_record_identity_under_sort_and_is_one_history_step(
    tcx: &mut TestAppContext,
) {
    crate::ui::i18n::set_locale("en");
    let dir = canonical_tempdir();
    let path = dir.path().join("order.csv");
    let original = b"c,b\nc,a\na,b\n";
    std::fs::write(&path, original).unwrap();
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    cx.simulate_resize(size(px(1600.), px(1000.)));
    open(&app, &mut cx, FileHost::new(HostId(288102)), &path);
    ready(&app, &mut cx, 3);
    let csv = view(&app, &cx);
    let id = csv.entity_id();

    // View-only sorting changes the visible order, never the file order.
    cx.update(|window, cx| window.click(column_element_id(id, 0), cx));
    until(&mut cx, |cx| {
        csv.read_with(cx, |view, _| view.order() == [2, 0, 1])
    });

    // A rectangle copy follows logical records, not visible positions.
    cx.update(|window, cx| window.click(cell_id(id, 0, 0), cx));
    settle(&mut cx);
    cx.update(|window, cx| {
        window.click_with_modifiers(
            cell_id(id, 1, 1),
            gpui_kit::Modifiers {
                shift: true,
                ..Default::default()
            },
            cx,
        );
    });
    settle(&mut cx);
    cx.update(|window, cx| window.press(&chord("c"), cx));
    cx.update(|_, cx| {
        assert_eq!(
            cx.read_from_clipboard().and_then(|item| item.text()),
            Some("c\tb\nc\ta".to_owned())
        );
    });

    // Paste starts at the selected logical record (record 1) and advances in
    // file order even though it is shown in the middle of the sorted view.
    cx.update(|window, cx| window.click(cell_id(id, 1, 0), cx));
    settle(&mut cx);
    cx.update(|_, cx| {
        cx.write_to_clipboard(ClipboardItem::new_string("中\t文\n行\t值".to_owned()))
    });
    cx.update(|window, cx| window.press(&chord("v"), cx));
    until(&mut cx, |cx| {
        source(&app, cx) == "c,b\n中,文\n行,值\n"
            && !view(&app, cx).read_with(cx, |v, cx| v.is_parsing(cx))
    });
    assert_eq!(value(&app, &cx, 1, 0).as_deref(), Some("中"));
    assert_eq!(value(&app, &cx, 2, 1).as_deref(), Some("值"));

    // Both pasted records were one logical operation: one undo reverts them.
    cx.update(|window, cx| window.press(&chord("z"), cx));
    until(&mut cx, |cx| {
        source(&app, cx) == "c,b\nc,a\na,b\n"
            && !view(&app, cx).read_with(cx, |v, cx| v.is_parsing(cx))
    });
    assert_eq!(value(&app, &cx, 1, 0).as_deref(), Some("c"));
    assert_eq!(value(&app, &cx, 2, 1).as_deref(), Some("b"));
}

#[gpui_kit::test]
fn row_and_column_operations_are_single_atomic_source_edits(tcx: &mut TestAppContext) {
    crate::ui::i18n::set_locale("en");
    let dir = canonical_tempdir();
    let path = dir.path().join("rows.csv");
    let original = "a,b\nc,d\ne\n";
    std::fs::write(&path, original).unwrap();
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    cx.simulate_resize(size(px(1600.), px(1000.)));
    open(&app, &mut cx, FileHost::new(HostId(288103)), &path);
    ready(&app, &mut cx, 3);
    let csv = view(&app, &cx);
    let id = csv.entity_id();

    // Insert below the selected record, then undo and redo the same operation.
    cx.update(|window, cx| window.click(cell_id(id, 1, 0), cx));
    settle(&mut cx);
    cx.update(|window, cx| window.press(&chord("shift-down"), cx));
    until(&mut cx, |cx| {
        table_state(&app, cx) == Some((4, 2)) && source(&app, cx) == "a,b\nc,d\n,\ne\n"
    });
    assert_eq!(value(&app, &cx, 2, 0).as_deref(), Some(""));
    cx.update(|window, cx| window.press(&chord("z"), cx));
    until(&mut cx, |cx| {
        table_state(&app, cx) == Some((3, 2)) && source(&app, cx) == original
    });
    cx.update(|window, cx| window.press(&chord("shift-z"), cx));
    until(&mut cx, |cx| {
        table_state(&app, cx) == Some((4, 2)) && source(&app, cx) == "a,b\nc,d\n,\ne\n"
    });
    cx.update(|window, cx| window.press(&chord("z"), cx));
    until(&mut cx, |cx| {
        table_state(&app, cx) == Some((3, 2)) && source(&app, cx) == original
    });

    // Insert above keeps the original records and adds one explicit row.
    cx.update(|window, cx| window.click(cell_id(id, 0, 0), cx));
    settle(&mut cx);
    cx.update(|window, cx| window.press(&chord("shift-up"), cx));
    until(&mut cx, |cx| {
        table_state(&app, cx) == Some((4, 2)) && source(&app, cx) == ",\na,b\nc,d\ne\n"
    });
    cx.update(|window, cx| window.press(&chord("z"), cx));
    until(&mut cx, |cx| {
        table_state(&app, cx) == Some((3, 2)) && source(&app, cx) == original
    });

    // Insert a column to the right of the selected one. The ragged record has
    // no such column and stays ragged.
    cx.update(|window, cx| window.click(cell_id(id, 0, 1), cx));
    settle(&mut cx);
    cx.update(|window, cx| window.press(&chord("shift-right"), cx));
    until(&mut cx, |cx| {
        table_state(&app, cx) == Some((3, 3)) && source(&app, cx) == "a,b,\nc,d,\ne\n"
    });
    cx.update(|window, cx| window.press(&chord("z"), cx));
    until(&mut cx, |cx| {
        table_state(&app, cx) == Some((3, 2)) && source(&app, cx) == original
    });

    // Insert a column to the left: every record that has that position gains
    // an empty field there; the short record gains a trailing field.
    cx.update(|window, cx| window.click(cell_id(id, 0, 1), cx));
    settle(&mut cx);
    cx.update(|window, cx| window.press(&chord("shift-left"), cx));
    until(&mut cx, |cx| {
        table_state(&app, cx) == Some((3, 3)) && source(&app, cx) == "a,,b\nc,,d\ne,\n"
    });
    cx.update(|window, cx| window.press(&chord("z"), cx));
    until(&mut cx, |cx| {
        table_state(&app, cx) == Some((3, 2)) && source(&app, cx) == original
    });

    // Delete the ragged last record without leaving a blank line.
    cx.update(|window, cx| window.click(cell_id(id, 2, 0), cx));
    settle(&mut cx);
    cx.update(|window, cx| window.press(&chord("shift-backspace"), cx));
    until(&mut cx, |cx| {
        table_state(&app, cx) == Some((2, 2)) && source(&app, cx) == "a,b\nc,d\n"
    });
    cx.update(|window, cx| window.press(&chord("z"), cx));
    until(&mut cx, |cx| {
        table_state(&app, cx) == Some((3, 2)) && source(&app, cx) == original
    });

    // Delete a column; the one-field record keeps one explicit empty field.
    cx.update(|window, cx| window.click(cell_id(id, 0, 0), cx));
    settle(&mut cx);
    cx.update(|window, cx| window.press(&chord("shift-delete"), cx));
    until(&mut cx, |cx| {
        table_state(&app, cx) == Some((3, 1)) && source(&app, cx) == "b\nd\n\"\"\n"
    });
    cx.update(|window, cx| window.press(&chord("z"), cx));
    until(&mut cx, |cx| {
        table_state(&app, cx) == Some((3, 2)) && source(&app, cx) == original
    });

    assert_eq!(std::fs::read(&path).unwrap(), original.as_bytes());
}

#[gpui_kit::test]
fn split_source_and_table_keep_their_own_selection_and_share_history(tcx: &mut TestAppContext) {
    crate::ui::i18n::set_locale("en");
    let dir = canonical_tempdir();
    let path = dir.path().join("split.csv");
    std::fs::write(&path, "a,b\nc,d\n").unwrap();
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    cx.simulate_resize(size(px(1600.), px(1000.)));
    open(&app, &mut cx, FileHost::new(HostId(288104)), &path);
    ready(&app, &mut cx, 2);

    app.update_in(&mut cx, |app, window, cx| {
        assert!(app.editor_split(window, cx), "the CSV document splits");
    });
    settle(&mut cx);
    let (tab, left, right) = app.read_with(&cx, |app, _| {
        let code = app.tab_code().expect("editor group");
        let tab = app.tabs.get(app.tabs.active_index()).unwrap().id();
        let right = code.focused_group();
        let left = match code.group == right {
            true => code.split.as_ref().expect("two groups").id,
            false => code.group,
        };
        (tab, left, right)
    });
    let (front, other) = app.read_with(&cx, |app, _| {
        let file = app.active_buffer().expect("CSV buffer");
        (
            file.presentations.csv_existing(tab, right).expect("front"),
            file.presentations.csv_existing(tab, left).expect("other"),
        )
    });
    assert_eq!(front.read_with(&cx, |view, _| view.mode()), CsvMode::Source);
    assert_eq!(other.read_with(&cx, |view, _| view.mode()), CsvMode::Table);

    // Give the source presentation a selection that the table edit must not
    // reset, and keep focus out of it.
    cx.update(|_, cx| {
        front.update(cx, |view, cx| view.select_source_range(0..2, cx));
    });
    settle(&mut cx);

    let other_id = other.entity_id();
    edit_cell(&mut cx, other_id, 1, 1, "X");
    until(&mut cx, |cx| {
        other.read_with(cx, |view, cx| {
            !view.is_parsing(cx)
                && view.table(cx).is_some_and(|table| {
                    table.cell_value(CsvCellId::new(1, 1)).as_deref() == Some("X")
                })
        })
    });
    // Both presentations read the same authoritative source.
    assert_eq!(
        front.read_with(&cx, |view, cx| view
            .document()
            .read(cx)
            .document()
            .read(cx)
            .source()
            .to_owned()),
        "a,b\nc,X\n"
    );
    assert_eq!(
        front.read_with(&cx, |view, cx| view.selected_range(cx)),
        0..2
    );
    cx.update(|window, cx| {
        assert!(
            other.read(cx).focus_handle(cx).contains_focused(window, cx),
            "the edited table keeps its own focus"
        );
        assert!(
            !front.read(cx).focus_handle(cx).contains_focused(window, cx),
            "the other presentation's focus is untouched"
        );
    });

    // Undo and redo from the table are the shared document history.
    cx.update(|window, cx| window.press(&chord("z"), cx));
    until(&mut cx, |cx| {
        front.read_with(cx, |view, cx| {
            view.document()
                .read(cx)
                .document()
                .read(cx)
                .source()
                .to_owned()
                == "a,b\nc,d\n"
        })
    });
    assert_eq!(
        front.read_with(&cx, |view, cx| view.selected_range(cx)),
        0..2
    );
    cx.update(|window, cx| window.press(&chord("shift-z"), cx));
    until(&mut cx, |cx| {
        front.read_with(cx, |view, cx| {
            view.document()
                .read(cx)
                .document()
                .read(cx)
                .source()
                .to_owned()
                == "a,b\nc,X\n"
        })
    });
    assert_eq!(
        front.read_with(&cx, |view, cx| view.selected_range(cx)),
        0..2
    );
}

#[gpui_kit::test]
fn a_stale_table_cannot_write_back_while_the_source_is_reparsing(tcx: &mut TestAppContext) {
    crate::ui::i18n::set_locale("en");
    let dir = canonical_tempdir();
    let path = dir.path().join("stale.csv");
    let original = b"a,b\nc,d\n";
    std::fs::write(&path, original).unwrap();
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    cx.simulate_resize(size(px(1600.), px(1000.)));
    open(&app, &mut cx, FileHost::new(HostId(288105)), &path);
    ready(&app, &mut cx, 2);
    let csv = view(&app, &cx);
    let id = csv.entity_id();
    cx.update(|window, cx| window.click(cell_id(id, 0, 0), cx));
    settle(&mut cx);

    let (release, entered) = test_support::pause_next_parse();
    app.update_in(&mut cx, |app, window, cx| {
        app.focus_editor(window, cx);
        app.active_editor_input()
            .unwrap()
            .text()
            .unwrap()
            .replace(0..1, "z", window, cx);
    });
    until(&mut cx, |_| entered.load(Ordering::Acquire));
    // Back to the table while its parse is deliberately held: there is no
    // table that matches the current revision.
    cx.update(|window, cx| window.click(("csv-mode-toggle", id), cx));
    settle(&mut cx);
    let after_source_edit = source(&app, &cx);
    assert_eq!(after_source_edit, "z,b\nc,d\n");

    // Neither an edit action nor a structural action may write a stale table
    // back into the newer source.
    cx.update(|window, cx| window.press("f2", cx));
    settle(&mut cx);
    cx.update(|window, cx| window.press(&chord("shift-down"), cx));
    settle(&mut cx);
    assert_eq!(source(&app, &cx), after_source_edit);
    assert_eq!(std::fs::read(&path).unwrap(), original);

    release.send(()).unwrap();
    ready(&app, &mut cx, 2);
    assert_eq!(source(&app, &cx), after_source_edit);
    assert_eq!(value(&app, &cx, 0, 0).as_deref(), Some("z"));
    assert_eq!(std::fs::read(&path).unwrap(), original);
}

#[gpui_kit::test]
fn table_edit_save_as_paused_save_and_close_keep_the_owning_host(tcx: &mut TestAppContext) {
    crate::ui::i18n::set_locale("en");
    let root = canonical_tempdir();
    let remote = canonical_tempdir();
    let path = Path::new("/owned.csv");
    let decoy = root.path().join("owned.csv");
    std::fs::write(&decoy, b"LOCAL DECOY").unwrap();
    let owner_file = remote.path().join("owned.csv");
    std::fs::write(&owner_file, b"a,b\nc,d\n").unwrap();
    let copy = Path::new("/copy.csv");
    let copy_file = remote.path().join("copy.csv");
    let host = FileHost::mapped(HostId(288106), remote.path());
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    cx.simulate_resize(size(px(1600.), px(1000.)));
    open(&app, &mut cx, host.clone(), &path);
    ready(&app, &mut cx, 2);
    let csv = view(&app, &cx);
    let id = csv.entity_id();

    edit_cell(&mut cx, id, 0, 0, "first");
    until(&mut cx, |cx| {
        value(&app, cx, 0, 0).as_deref() == Some("first")
            && !view(&app, cx).read_with(cx, |v, cx| v.is_parsing(cx))
    });
    cx.dispatch_action(crate::core::actions::EditorSaveAs);
    settle(&mut cx);
    enter_path(&mut cx, copy);
    until(&mut cx, |cx| {
        app.read_with(cx, |app, _| {
            app.active_buffer()
                .is_some_and(|file| file.path == copy && !file.saving && !file.dirty)
        })
    });
    assert_eq!(std::fs::read(&copy_file).unwrap(), b"first,b\nc,d\n");
    assert_eq!(std::fs::read(&owner_file).unwrap(), b"a,b\nc,d\n");
    assert_eq!(std::fs::read(&decoy).unwrap(), b"LOCAL DECOY");

    // A paused write, followed by a newer table edit, must not mark the newer
    // edit clean when the old save completes.
    edit_cell(&mut cx, id, 1, 1, "second");
    until(&mut cx, |cx| {
        value(&app, cx, 1, 1).as_deref() == Some("second")
            && !view(&app, cx).read_with(cx, |v, cx| v.is_parsing(cx))
    });
    let release = host.pause_next_write();
    cx.dispatch_action(crate::core::actions::EditorSave);
    until(&mut cx, |_| host.entered.load(Ordering::SeqCst));
    edit_cell(&mut cx, id, 0, 1, "third");
    until(&mut cx, |cx| {
        value(&app, cx, 0, 1).as_deref() == Some("third")
            && !view(&app, cx).read_with(cx, |v, cx| v.is_parsing(cx))
    });
    assert_eq!(
        std::fs::read(&copy_file).unwrap(),
        b"first,b\nc,d\n",
        "the paused write has not reached the owning host"
    );
    release.send(()).unwrap();
    until(&mut cx, |cx| {
        app.read_with(cx, |app, _| {
            app.active_buffer().is_some_and(|file| !file.saving)
        })
    });
    assert_eq!(
        std::fs::read(&copy_file).unwrap(),
        b"first,b\nc,second\n",
        "the paused save wrote the snapshot it started with"
    );
    assert!(
        app.read_with(&cx, |app, _| app.active_buffer().unwrap().dirty),
        "a completed old save does not mark newer edits clean"
    );
    save(&app, &mut cx);
    assert_eq!(
        std::fs::read(&copy_file).unwrap(),
        b"first,third\nc,second\n"
    );
    assert_eq!(std::fs::read(&owner_file).unwrap(), b"a,b\nc,d\n");
    assert_eq!(std::fs::read(&decoy).unwrap(), b"LOCAL DECOY");

    app.update_in(&mut cx, |app, window, cx| {
        app.editor_close_file(0, window, cx)
    });
    until(&mut cx, |cx| {
        app.read_with(cx, |app, _| app.editor.buffers.is_empty())
    });
    open(&app, &mut cx, host, copy);
    ready(&app, &mut cx, 2);
    assert_eq!(value(&app, &cx, 0, 1).as_deref(), Some("third"));
    assert_eq!(value(&app, &cx, 1, 1).as_deref(), Some("second"));
}

#[gpui_kit::test]
fn table_edit_failed_save_cancelled_close_and_dirty_conflict_preserve_edits(
    tcx: &mut TestAppContext,
) {
    crate::ui::i18n::set_locale("en");
    let dir = canonical_tempdir();
    let path = dir.path().join("guard.csv");
    let original = b"a,b\nc,d\n";
    std::fs::write(&path, original).unwrap();
    let host = FileHost::new(HostId::LOCAL);
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    cx.simulate_resize(size(px(1600.), px(1000.)));
    open(&app, &mut cx, host.clone(), &path);
    ready(&app, &mut cx, 2);
    let csv = view(&app, &cx);
    let id = csv.entity_id();
    edit_cell(&mut cx, id, 0, 0, "mine");
    until(&mut cx, |cx| {
        value(&app, cx, 0, 0).as_deref() == Some("mine")
            && !view(&app, cx).read_with(cx, |v, cx| v.is_parsing(cx))
    });
    assert!(app.read_with(&cx, |app, _| app.active_buffer().unwrap().dirty));

    // Cancelling an unsaved close keeps the document and its edit.
    app.update_in(&mut cx, |app, window, cx| {
        app.editor_close_file(0, window, cx)
    });
    settle(&mut cx);
    cx.simulate_prompt_answer(t(L10nKey::Cancel));
    settle(&mut cx);
    assert!(app.read_with(&cx, |app, _| app.active_buffer().is_some()));
    assert!(app.read_with(&cx, |app, _| app.active_buffer().unwrap().dirty));
    assert_eq!(value(&app, &cx, 0, 0).as_deref(), Some("mine"));

    // A failed Host write preserves the edit, the dirty state and the original.
    cx.update(|window, cx| window.click(cell_id(id, 0, 0), cx));
    settle(&mut cx);
    host.fail_next_write();
    cx.dispatch_action(crate::core::actions::EditorSave);
    until(&mut cx, |cx| {
        host.entered.load(Ordering::SeqCst)
            && app.read_with(cx, |app, _| {
                app.active_buffer().is_some_and(|file| !file.saving)
            })
    });
    assert!(app.read_with(&cx, |app, _| app.active_buffer().unwrap().dirty));
    assert_eq!(value(&app, &cx, 0, 0).as_deref(), Some("mine"));
    assert_eq!(std::fs::read(&path).unwrap(), original);

    // An external write to a dirty CSV reports a conflict and never replaces
    // the in-memory table edit with the disk content.
    std::fs::write(&path, b"x,y\nz,w\n").unwrap();
    until(&mut cx, |cx| {
        app.read_with(cx, |app, _| {
            app.active_buffer()
                .is_some_and(|file| file.conflict.is_some())
        })
    });
    assert!(app.read_with(&cx, |app, _| app.active_buffer().unwrap().dirty));
    assert_eq!(value(&app, &cx, 0, 0).as_deref(), Some("mine"));
    assert!(app.read_with(&cx, |app, _| {
        app.active_buffer().unwrap().input.csv().is_some()
    }));
}

#[gpui_kit::test]
fn a_cell_editor_opened_before_a_reload_does_not_write_its_stale_value(tcx: &mut TestAppContext) {
    crate::ui::i18n::set_locale("en");
    let dir = canonical_tempdir();
    let path = dir.path().join("reload.csv");
    std::fs::write(&path, "a,b\nc,d\n").unwrap();
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    cx.simulate_resize(size(px(1600.), px(1000.)));
    open(&app, &mut cx, FileHost::new(HostId::LOCAL), &path);
    ready(&app, &mut cx, 2);
    let csv = view(&app, &cx);
    let id = csv.entity_id();
    cx.update(|window, cx| window.double_click(cell_id(id, 0, 0), cx));
    settle(&mut cx);
    cx.update(|window, _| {
        assert!(window.try_find(editor_id(id, 0, 0)).is_some());
    });
    cx.update(|window, cx| window.input("stale", cx));
    settle(&mut cx);

    // A clean external reload replaces the authoritative source while the cell
    // input is still open.
    std::fs::write(&path, "x,b\nc,d\n").unwrap();
    let buffer = app.read_with(&cx, |app, _| app.active_buffer().unwrap().id());
    app.update_in(&mut cx, |app, window, cx| {
        app.editor_reload_from_disk(buffer, window, cx)
    });
    until(&mut cx, |cx| {
        source(&app, cx) == "x,b\nc,d\n" && !view(&app, cx).read_with(cx, |v, cx| v.is_parsing(cx))
    });

    // The open input belongs to the previous revision; committing it must not
    // overwrite the reloaded cell.
    cx.update(|window, cx| window.press("enter", cx));
    settle(&mut cx);
    assert_eq!(source(&app, &cx), "x,b\nc,d\n");
    until(&mut cx, |cx| value(&app, cx, 0, 0).as_deref() == Some("x"));
    assert_eq!(std::fs::read(&path).unwrap(), b"x,b\nc,d\n");
}
