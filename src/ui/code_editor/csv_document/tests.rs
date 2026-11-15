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
