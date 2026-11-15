//! #295 的生产工作台接缝：SQLite 数据库作为真实文档打开、分页、刷新与失败。
//!
//! 这些测试只走工作台入口（`open_file_in_editor`、`editor_open_on_host`、
//! 真实按钮与快捷键）和真实 gpui 测试平台窗口，断言可见结果与源文件完整性。
//! 夹具在测试目录内生成，不依赖协调目录。它们不是原生 OS 窗口，真实远端
//! SSH、系统 IME 与 macOS 由独立验收覆盖。

use super::super::test_host::FileHost;
use super::super::*;
use super::*;
use crate::ui::app::test_window::harness_with_tabs;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{TestAppContext, VisualTestContext};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

fn settle(cx: &mut VisualTestContext) {
    cx.run_until_parked();
    cx.update(|window, cx| {
        window.simulate_next_frame(cx);
        window.render_frame(cx);
    });
    cx.run_until_parked();
}

fn until(cx: &mut VisualTestContext, ready: impl FnMut(&mut VisualTestContext) -> bool) {
    until_named(cx, "transition", ready);
}

fn until_named(
    cx: &mut VisualTestContext,
    name: &str,
    mut ready: impl FnMut(&mut VisualTestContext) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !ready(cx) {
        assert!(
            Instant::now() < deadline,
            "SQLite document transition '{name}' did not settle"
        );
        settle(cx);
        cx.executor().advance_clock(Duration::from_millis(50));
        std::thread::sleep(Duration::from_millis(5));
    }
    settle(cx);
}

/// The inspector's identity and why a read may have failed, for assertion
/// messages that would otherwise only say a predicate never became true.
fn detail_diagnostic(detail: &ValueDetail) -> String {
    let kind = match detail.value.as_ref() {
        None => "none",
        Some(SqliteValue::Null) => "null",
        Some(SqliteValue::Integer { .. }) => "integer",
        Some(SqliteValue::Real { .. }) => "real",
        Some(SqliteValue::Text { .. }) => "text",
        Some(SqliteValue::Blob { .. }) => "blob",
    };
    format!(
        "row={} column={} offset={} busy={} value={kind} error={:?} notice={:?}",
        detail.row, detail.column, detail.offset, detail.busy, detail.error, detail.notice
    )
}

fn active_id(app: &Entity<LyrelApp>, cx: &VisualTestContext) -> BufferId {
    app.read_with(cx, |app, _| {
        app.tab_code()
            .and_then(TabCode::active_id)
            .expect("a buffer in front")
    })
}

fn sqlite_view(app: &Entity<LyrelApp>, cx: &VisualTestContext) -> Entity<SqliteView> {
    app.read_with(cx, |app, _| {
        app.active_editor_input()
            .expect("a document in front")
            .sqlite()
            .expect("a SQLite document")
            .clone()
    })
}

fn open(app: &Entity<LyrelApp>, cx: &mut VisualTestContext, path: &Path) -> BufferId {
    app.update_in(cx, |app, window, cx| {
        app.open_file_in_editor(path, window, cx)
    });
    until(cx, |cx| {
        app.read_with(cx, |app, _| {
            app.active_buffer()
                .is_some_and(|file| file.path.as_path() == path && file.input.sqlite().is_some())
        })
    });
    active_id(app, cx)
}

fn dump(view: &Entity<SqliteView>, cx: &VisualTestContext) -> (Vec<String>, Vec<Vec<String>>) {
    view.read_with(cx, |view, cx| view.grid_dump(cx))
}

fn cells_contain(dump: &(Vec<String>, Vec<Vec<String>>), needle: &str) -> bool {
    dump.1
        .iter()
        .any(|row| row.iter().any(|cell| cell == needle))
}

/// A real database with a table, a view and a second empty table, plus the
/// exact values the reader must preserve.
fn people_database(dir: &Path, name: &str, marker: &str, rows: usize) -> PathBuf {
    let path = dir.join(name);
    let connection = rusqlite::Connection::open(&path).expect("fixture database opens");
    connection
        .execute_batch(
            "CREATE TABLE people(
                 id INTEGER,
                 name TEXT,
                 score REAL,
                 avatar BLOB,
                 note TEXT
             );
             CREATE VIEW people_view AS
                 SELECT id, name FROM people WHERE id > 0;
             CREATE TABLE z_orders(id INTEGER, total REAL);",
        )
        .expect("fixture schema");
    let mut insert = connection
        .prepare("INSERT INTO people(id, name, score, avatar, note) VALUES(?1, ?2, ?3, ?4, ?5)")
        .expect("insert statement");
    insert
        .execute(rusqlite::params![
            9007199254740993_i64,
            marker,
            1.5_f64,
            &[0x00u8, 0xff][..],
            Option::<String>::None,
        ])
        .expect("first row");
    insert
        .execute(rusqlite::params![
            2_i64,
            "中文",
            f64::INFINITY,
            &[] as &[u8],
            "second",
        ])
        .expect("second row");
    insert
        .execute(rusqlite::params![
            3_i64,
            "Gamma",
            -0.0_f64,
            Option::<Vec<u8>>::None,
            "third",
        ])
        .expect("third row");
    for id in 4..=(rows as i64) {
        insert
            .execute(rusqlite::params![
                id,
                format!("row-{id}"),
                0.0_f64,
                Option::<Vec<u8>>::None,
                Option::<String>::None,
            ])
            .expect("generated row");
    }
    drop(insert);
    drop(connection);
    path
}

#[gpui_kit::test]
fn a_sqlite_database_opens_read_only_with_schema_and_paged_rows(tcx: &mut TestAppContext) {
    crate::ui::i18n::set_locale("en");
    let dir = super::super::test_host::canonical_tempdir();
    let path = people_database(dir.path(), "people.sqlite", "Ada", 501);
    let before = std::fs::read(&path).expect("source bytes");
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    let id = open(&app, &mut cx, &path);
    let view = sqlite_view(&app, &cx);

    until(&mut cx, |cx| {
        let dump = view.read_with(cx, |view, cx| view.grid_dump(cx));
        !dump.0.is_empty() && !dump.1.is_empty()
    });
    until(&mut cx, |cx| {
        view.read_with(cx, |view, _| view.state.total == Some(501))
    });

    app.read_with(&cx, |app, _| {
        let file = app.editor.buffers.get(id).expect("installed document");
        assert!(file.input.sqlite().is_some(), "a SQLite document");
        assert!(!file.dirty, "a database is never unsaved");
        assert!(!file.input.is_text(), "no fake text buffer");
        assert_eq!(file.path, path);
        assert!(app.sqlite_document_active());
        assert_eq!(
            app.editor_active_location()
                .map(|(host, path)| (host, path.to_path_buf())),
            Some((HostId::LOCAL, path.clone()))
        );
        assert!(
            app.editor.local_file(id).is_none(),
            "a database is not a language-server text file"
        );
    });

    let (headers, rows) = dump(&view, &cx);
    assert_eq!(
        headers,
        vec!["#", "id", "name", "score", "avatar", "note"],
        "the table's real columns, in schema order"
    );
    assert_eq!(rows[0][0], "1", "the first page starts at row 1");
    assert_eq!(rows[0][1], "9007199254740993", "INTEGER stays exact");
    assert_eq!(rows[0][2], "Ada");
    assert_eq!(rows[0][3], "1.5");
    assert_eq!(rows[0][4], "BLOB 2 B · 00 FF", "BLOB keeps its bytes");
    assert_eq!(rows[0][5], "NULL", "NULL is not empty text");
    assert_eq!(rows[1][2], "中文", "TEXT keeps its characters");
    assert_eq!(rows[1][3], "Infinity", "a non-finite REAL stays REAL");
    assert_eq!(rows.len(), 500, "the bounded page size");
    let status = view.read_with(&cx, |view, _| view.status_text());
    assert!(status.contains("501 rows"), "the real count: {status}");
    assert!(
        status.contains("Read view"),
        "the read view identity: {status}"
    );
    assert!(status.contains("Read-only"), "the document is read-only");

    cx.update(|window, _| {
        assert!(window.find("sqlite-table").visible());
        assert!(window.find("sqlite-schema").visible());
        assert!(window.find("status-sqlite-facts").visible());
        assert!(window.find("sqlite-status-bar").visible());
    });

    // 第二页是从同一读取视图读到的下一段行，不是拼接的另一个文件。
    cx.update(|window, cx| window.click("sqlite-next-page", cx));
    until(&mut cx, |cx| {
        view.read_with(cx, |view, _| view.page_number() == 1)
            && dump(&view, cx).1.first().is_some_and(|row| row[0] == "501")
    });
    assert_eq!(view.read_with(&cx, |view, _| view.page_number()), 1);

    // 刷新打开新的读取视图，旧身份不会继续冒充当前数据。
    let old_view = view.read_with(&cx, |view, _| view.view_id());
    cx.update(|window, cx| window.click("sqlite-refresh", cx));
    until(&mut cx, |cx| {
        view.read_with(cx, |view, _| view.view_id() != old_view) && !dump(&view, cx).1.is_empty()
    });
    assert_ne!(view.read_with(&cx, |view, _| view.view_id()), old_view);

    // 表与视图都出现在架构里；切到空表显示真实的空页。
    cx.update(|window, cx| window.click(SharedString::from("sqlite-object:z_orders"), cx));
    until(&mut cx, |cx| {
        view.read_with(cx, |view, _| view.selected_table() == Some("z_orders"))
            && dump(&view, cx).0.get(1).is_some_and(|name| name == "id")
    });
    assert!(dump(&view, &cx).1.is_empty());
    cx.update(|window, _| assert!(window.find("sqlite-no-rows").visible()));

    assert_eq!(
        std::fs::read(&path).expect("source bytes after browsing"),
        before,
        "browsing did not modify the database"
    );
    assert!(
        !path.with_extension("sqlite-journal").exists()
            && !path.with_extension("sqlite-wal").exists()
            && !path.with_extension("sqlite-shm").exists(),
        "a delete-journal database gained no sidecar files"
    );
}

#[gpui_kit::test]
fn the_mapped_host_shows_its_own_database(tcx: &mut TestAppContext) {
    crate::ui::i18n::set_locale("en");
    let dir = super::super::test_host::canonical_tempdir();
    let local_path = people_database(dir.path(), "data.sqlite", "local", 2);
    let local_bytes = std::fs::read(&local_path).expect("local source bytes");
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    let local = open(&app, &mut cx, &local_path);
    until(&mut cx, |cx| {
        cells_contain(&dump(&sqlite_view(&app, cx), cx), "local")
    });

    // 同一路径、另一个 Host：身份不同，不共用缓冲区。
    let second: SharedHost = FileHost::new(HostId(69));
    app.update_in(&mut cx, |app, window, cx| {
        app.editor_open_on_host(second, &local_path, window, cx)
    });
    until(&mut cx, |cx| {
        app.read_with(cx, |app, _| {
            app.tab_code()
                .and_then(TabCode::active_id)
                .is_some_and(|id| id != local)
                && app
                    .editor_active_location()
                    .is_some_and(|(host, _)| host == HostId(69))
        })
    });
    assert!(
        cells_contain(&dump(&sqlite_view(&app, &cx), &cx), "local"),
        "the plain host's own copy"
    );

    // 映射 Host 的根在另一个目录：读它自己的库，不是本机同名文件。
    let decoy = super::super::test_host::canonical_tempdir();
    let mapped_path = people_database(decoy.path(), "data.sqlite", "mapped", 2);
    let mapped_bytes = std::fs::read(&mapped_path).expect("mapped source bytes");
    let mapped: SharedHost = FileHost::mapped(HostId(70), decoy.path());
    app.update_in(&mut cx, |app, window, cx| {
        app.editor_open_on_host(mapped, Path::new("/data.sqlite"), window, cx)
    });
    until(&mut cx, |cx| {
        app.read_with(cx, |app, _| {
            app.editor_active_location()
                .is_some_and(|(host, _)| host == HostId(70))
        })
    });
    until(&mut cx, |cx| {
        cells_contain(&dump(&sqlite_view(&app, &cx), &cx), "mapped")
    });
    assert!(
        !cells_contain(&dump(&sqlite_view(&app, &cx), &cx), "local"),
        "the mapped Host's own rows, not the local namesake"
    );

    // 切回本机那份仍然是它自己的数据；两份源字节都没被碰过。
    app.update_in(&mut cx, |app, window, cx| {
        app.open_file_in_editor(&local_path, window, cx)
    });
    until(&mut cx, |cx| {
        app.read_with(cx, |app, _| {
            app.tab_code()
                .and_then(TabCode::active_id)
                .is_some_and(|id| id == local)
        })
    });
    until(&mut cx, |cx| {
        cells_contain(&dump(&sqlite_view(&app, &cx), &cx), "local")
    });
    assert_eq!(std::fs::read(&local_path).unwrap(), local_bytes);
    assert_eq!(std::fs::read(&mapped_path).unwrap(), mapped_bytes);
}

#[gpui_kit::test]
fn failures_and_empty_schemas_are_visible_documents(tcx: &mut TestAppContext) {
    crate::ui::i18n::set_locale("en");
    let dir = super::super::test_host::canonical_tempdir();
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);

    // A file that claims to be a database but is not one keeps a place and
    // says why, with a retry — it does not fall back to text or disappear.
    let broken = dir.path().join("broken.sqlite");
    std::fs::write(&broken, b"this is not a sqlite database").unwrap();
    open(&app, &mut cx, &broken);
    until(&mut cx, |cx| {
        sqlite_view(&app, cx).read_with(cx, |view, _| view.error_text().is_some())
    });
    cx.update(|window, _| {
        assert!(window.find("sqlite-error").visible());
        assert!(window.find("sqlite-retry").visible());
        assert!(window.find("sqlite-failed").visible());
    });

    // An empty database has no fabricated tables: it says it is empty.
    let empty = dir.path().join("empty.sqlite");
    {
        let connection = rusqlite::Connection::open(&empty).expect("empty database");
        connection
            .execute_batch("CREATE TABLE seed(x); DROP TABLE seed;")
            .expect("empty schema");
    }
    app.update_in(&mut cx, |app, window, cx| {
        app.open_file_in_editor(&empty, window, cx)
    });
    until(&mut cx, |cx| {
        app.read_with(cx, |app, cx| {
            app.active_buffer()
                .filter(|file| file.path == empty)
                .and_then(|file| file.input.sqlite().cloned())
                .is_some_and(|view| {
                    view.read_with(cx, |view, _| {
                        view.state.view.is_some()
                            && !view.state.busy
                            && view.state.objects.is_empty()
                    })
                })
        })
    });
    cx.update(|window, _| assert!(window.find("sqlite-no-objects").visible()));
}

#[gpui_kit::test]
fn split_groups_keep_their_own_focus_and_table_state(tcx: &mut TestAppContext) {
    crate::ui::i18n::set_locale("en");
    let dir = super::super::test_host::canonical_tempdir();
    let path = people_database(dir.path(), "split.sqlite", "Ada", 3);
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    let id = open(&app, &mut cx, &path);
    until(&mut cx, |cx| !dump(&sqlite_view(&app, cx), cx).1.is_empty());

    app.update_in(&mut cx, |app, window, cx| {
        assert!(app.editor_split(window, cx), "the database splits");
    });
    until(&mut cx, |cx| {
        app.read_with(cx, |app, _| {
            let Some(file) = app.editor.buffers.get(id) else {
                return false;
            };
            let Some(code) = app.tab_code() else {
                return false;
            };
            let Some(other) = code.split.as_ref() else {
                return false;
            };
            let tab = app
                .tabs
                .get(app.tabs.active_index())
                .expect("visible tab")
                .id();
            file.presentations.sqlite_existing(tab, other.id).is_some()
        })
    });

    let (primary, second, first_group, second_group) = app.read_with(&cx, |app, _| {
        let file = app.editor.buffers.get(id).expect("retained document");
        let code = app.tab_code().expect("visible editor group");
        let tab = app
            .tabs
            .get(app.tabs.active_index())
            .expect("visible tab")
            .id();
        let other = code.split.as_ref().expect("second group").id;
        let primary = file
            .presentations
            .sqlite_existing(tab, code.group)
            .expect("the focused group rendered its presentation");
        let second = file
            .presentations
            .sqlite_existing(tab, other)
            .expect("the split group rendered its presentation");
        (primary, second, code.group, other)
    });
    assert!(!dump(&primary, &cx).1.is_empty());
    assert!(!dump(&second, &cx).1.is_empty());

    let primary_focus = primary.read_with(&cx, |view, cx| view.focus_handle(cx));
    let second_focus = second.read_with(&cx, |view, cx| view.focus_handle(cx));
    cx.update(|window, cx| primary_focus.focus(window, cx));
    settle(&mut cx);
    app.read_with(&cx, |app, _| {
        assert_eq!(app.tab_code().unwrap().group, first_group);
    });
    cx.update(|window, _| {
        assert!(primary_focus.is_focused(window));
        assert!(!second_focus.is_focused(window));
    });
    cx.update(|window, cx| second_focus.focus(window, cx));
    settle(&mut cx);
    app.read_with(&cx, |app, _| {
        assert_eq!(app.tab_code().unwrap().group, second_group);
    });
    cx.update(|window, _| {
        assert!(second_focus.is_focused(window));
        assert!(!primary_focus.is_focused(window));
    });
    app.update_in(&mut cx, |app, window, cx| {
        app.editor_close_file(0, window, cx)
    });
    settle(&mut cx);
    app.read_with(&cx, |app, _| {
        let code = app.tab_code().expect("the other presentation stays open");
        assert!(!code.is_split());
        assert_eq!(code.group, first_group);
        assert_eq!(code.active_id(), Some(id));
    });
}

#[gpui_kit::test]
fn filtering_and_sorting_requery_the_same_view_in_place(tcx: &mut TestAppContext) {
    crate::ui::i18n::set_locale("en");
    let dir = super::super::test_host::canonical_tempdir();
    let path = people_database(dir.path(), "filter.sqlite", "Ada", 5);
    let before = std::fs::read(&path).expect("source bytes");
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    open(&app, &mut cx, &path);
    let view = sqlite_view(&app, &cx);
    until(&mut cx, |cx| !dump(&view, cx).1.is_empty());
    let view_id = view.read_with(&cx, |view, _| view.view_id());

    // 真实输入走防抖：输入 "Ada" 后同一读取视图只回一行。
    cx.update(|window, cx| {
        window.click("sqlite-filter", cx);
        window.input("Ada", cx);
    });
    until_named(&mut cx, "filter-ada", |cx| {
        view.read_with(cx, |view, _| view.state.search == "Ada") && dump(&view, cx).1.len() == 1
    });
    let rows = dump(&view, &cx).1;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0][2], "Ada");
    let status = view.read_with(&cx, |view, _| view.status_text());
    assert!(status.contains("filter"), "the filter is visible: {status}");
    assert_eq!(
        view.read_with(&cx, |view, _| view.state.total),
        Some(1),
        "the count answers the filtered rows"
    );

    // 大小写不敏感、覆盖每一列：全选替换为数字，筛出同一行。
    cx.update(|window, cx| {
        window.click("sqlite-filter", cx);
        window.press("secondary-a", cx);
        window.input("900719925474099", cx);
    });
    until_named(&mut cx, "filter-digits", |cx| {
        view.read_with(cx, |view, _| view.state.search == "900719925474099")
            && dump(&view, cx).1.len() == 1
    });
    assert_eq!(dump(&view, &cx).1[0][1], "9007199254740993");

    // 清空筛选恢复整页；筛选期间产生的迟到页不会覆盖新结果。
    cx.update(|window, cx| {
        window.click("sqlite-filter", cx);
        window.press("secondary-a", cx);
        window.press("backspace", cx);
    });
    until_named(&mut cx, "filter-cleared", |cx| {
        view.read_with(cx, |view, _| view.state.search.is_empty()) && dump(&view, cx).1.len() == 5
    });
    // 迟到的旧串行落地被丢弃：伪造一个旧代的页结果，当前数据不被覆盖。
    view.update(&mut cx, |view, cx| {
        view.document.update(cx, |document, cx| {
            document.on_page(
                document.epoch.wrapping_sub(1),
                document.page_serial,
                view_id.expect("a live read view"),
                Ok(lyrel_core::host::sqlite::SqlitePage {
                    page: 0,
                    start_column: 0,
                    more: true,
                    rows: Vec::new(),
                }),
                cx,
            );
        });
    });
    settle(&mut cx);
    assert_eq!(
        dump(&view, &cx).1.len(),
        5,
        "a late page from the old filter did not overwrite the live one"
    );

    // 表头三次点击走完降序→升序→还原。
    cx.update(|window, cx| window.click(("icon-sort", 2usize), cx));
    until_named(&mut cx, "sort-desc", |cx| {
        view.read_with(cx, |view, _| view.state.sort == Some((1, true)))
            && dump(&view, cx)
                .1
                .first()
                .is_some_and(|row| row[2] == "中文")
    });
    let status = view.read_with(&cx, |view, _| view.status_text());
    assert!(
        status.contains("sorted by name"),
        "the sort is visible: {status}"
    );
    cx.update(|window, cx| window.click(("icon-sort", 2usize), cx));
    until_named(&mut cx, "sort-asc", |cx| {
        view.read_with(cx, |view, _| view.state.sort == Some((1, false)))
            && dump(&view, cx).1.first().is_some_and(|row| row[2] == "Ada")
    });
    cx.update(|window, cx| window.click(("icon-sort", 2usize), cx));
    until_named(&mut cx, "sort-clear", |cx| {
        view.read_with(cx, |view, _| view.state.sort.is_none())
            && dump(&view, cx).1.first().is_some_and(|row| row[2] == "Ada")
    });

    // 筛选与排序叠加仍走同一个读取视图。
    cx.update(|window, cx| {
        window.click("sqlite-filter", cx);
        window.input("o", cx);
    });
    until_named(&mut cx, "filter-o", |cx| {
        view.read_with(cx, |view, _| view.state.search == "o") && !dump(&view, cx).1.is_empty()
    });
    cx.update(|window, cx| window.click(("icon-sort", 2usize), cx));
    until_named(&mut cx, "sort-with-filter", |cx| {
        view.read_with(cx, |view, _| view.state.sort == Some((1, true)))
    });
    assert_eq!(
        view.read_with(&cx, |view, _| view.view_id()),
        view_id,
        "browsing kept one read view"
    );

    assert_eq!(
        std::fs::read(&path).expect("source bytes after browsing"),
        before,
        "filtering and sorting did not modify the database"
    );
}

/// A database with a >1 MiB TEXT, a small BLOB and NULLs for the inspector.
fn big_value_database(dir: &Path) -> PathBuf {
    let path = dir.join("values.sqlite");
    let connection = rusqlite::Connection::open(&path).expect("fixture database opens");
    connection
        .execute_batch("CREATE TABLE items(id INTEGER, name TEXT, data BLOB);")
        .expect("fixture schema");
    connection
        .execute(
            "INSERT INTO items VALUES (1, replace(hex(zeroblob(1050000)), '0', 'x'), x'00ff10')",
            [],
        )
        .expect("big row");
    connection
        .execute("INSERT INTO items VALUES (2, NULL, NULL)", [])
        .expect("null row");
    drop(connection);
    path
}

#[gpui_kit::test]
fn the_value_inspector_reads_and_copies_complete_typed_values(tcx: &mut TestAppContext) {
    crate::ui::i18n::set_locale("en");
    let dir = super::super::test_host::canonical_tempdir();
    let path = big_value_database(dir.path());
    let before = std::fs::read(&path).expect("source bytes");
    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    open(&app, &mut cx, &path);
    let view = sqlite_view(&app, &cx);
    until(&mut cx, |cx| dump(&view, cx).1.len() == 2);

    // 真实双击打开 name 列的完整值：第一块、类型与字节区间都可见。
    cx.update(|window, cx| window.double_click("table-cell-0:2", cx));
    until_named(&mut cx, "detail-open", |cx| {
        view.read_with(cx, |view, _| {
            view.detail.as_ref().is_some_and(|detail| !detail.busy)
        })
    });
    cx.update(|window, _| {
        assert!(window.find("sqlite-detail").visible());
        assert!(window.find("sqlite-detail-extent").visible());
        assert!(window.find("sqlite-value-next").visible());
    });
    let (offset, total) = view.read_with(&cx, |view, _| {
        let detail = view.detail.as_ref().expect("the inspector is open");
        let SqliteValue::Text { total, .. } = detail.value.as_ref().expect("a TEXT value") else {
            panic!("the name column is TEXT: {}", detail_diagnostic(detail))
        };
        (detail.offset, *total)
    });
    assert_eq!(offset, 0);
    assert_eq!(total, 2_100_000);
    let shown = view.read_with(&cx, |view, cx| {
        view.detail
            .as_ref()
            .unwrap()
            .editor
            .read(cx)
            .value()
            .to_string()
    });
    assert_eq!(shown.len(), 1_048_576, "the first bounded chunk");
    assert!(shown.bytes().all(|byte| byte == b'x'));

    // 下一块按精确字节偏移推进，块尾正好到值末。
    cx.update(|window, cx| window.click("sqlite-value-next", cx));
    until_named(&mut cx, "detail-next-chunk", |cx| {
        view.read_with(cx, |view, _| {
            view.detail
                .as_ref()
                .is_some_and(|detail| detail.offset == 1_048_576 && !detail.busy)
        })
    });
    let shown = view.read_with(&cx, |view, cx| {
        view.detail
            .as_ref()
            .unwrap()
            .editor
            .read(cx)
            .value()
            .to_string()
    });
    assert_eq!(shown.len(), 1_048_576, "the second bounded chunk");

    // 第三块只返回剩余字节；块尾正好到值末。
    cx.update(|window, cx| window.click("sqlite-value-next", cx));
    until_named(&mut cx, "detail-last-chunk", |cx| {
        view.read_with(cx, |view, _| {
            view.detail
                .as_ref()
                .is_some_and(|detail| detail.offset == 2_097_152 && !detail.busy)
        })
    });
    let shown = view.read_with(&cx, |view, cx| {
        view.detail
            .as_ref()
            .unwrap()
            .editor
            .read(cx)
            .value()
            .to_string()
    });
    assert_eq!(shown.len(), 2_100_000 - 2_097_152);
    assert!(shown.bytes().all(|byte| byte == b'x'));

    // 复制组装全部块，落剪贴板的是完整值，不是预览。
    cx.update(|window, cx| window.click("sqlite-value-copy", cx));
    until_named(&mut cx, "copy-full", |cx| {
        cx.update(|_window, cx| cx.read_from_clipboard().and_then(|item| item.text()))
            .is_some_and(|text| text.len() == 2_100_000)
    });
    let copied = cx
        .update(|_window, cx| cx.read_from_clipboard().and_then(|item| item.text()))
        .expect("the complete value reached the clipboard");
    assert!(copied.bytes().all(|byte| byte == b'x'));

    // BLOB 单元格以十六进制展示与复制；复制的 hex 精确还原字节。
    cx.update(|window, cx| window.double_click("table-cell-0:3", cx));
    until_named(&mut cx, "blob-value", |cx| {
        view.read_with(cx, |view, _| {
            view.detail.as_ref().is_some_and(|detail| !detail.busy)
        })
    });
    view.read_with(&cx, |view, cx| {
        assert_eq!(
            view.grid.read(cx).selection(),
            TableSelection::Cell(0, 3),
            "the double-click selected the payload cell before inspecting it"
        );
        let detail = view.detail.as_ref().expect("the inspector opened");
        assert_eq!(
            (detail.row, detail.column),
            (0, 2),
            "the payload cell: {}",
            detail_diagnostic(detail)
        );
        assert!(
            matches!(detail.value, Some(SqliteValue::Blob { .. })),
            "the payload stays a BLOB: {}",
            detail_diagnostic(detail)
        );
    });
    let shown = view.read_with(&cx, |view, cx| {
        view.detail
            .as_ref()
            .unwrap()
            .editor
            .read(cx)
            .value()
            .to_string()
    });
    assert!(shown.contains("00 ff 10"), "the BLOB's real bytes: {shown}");
    cx.update(|window, cx| window.click("sqlite-value-copy", cx));
    until_named(&mut cx, "copy-blob", |cx| {
        cx.update(|_window, cx| cx.read_from_clipboard().and_then(|item| item.text()))
            .is_some_and(|text| text == "00ff10")
    });
    view.read_with(&cx, |view, _| {
        let detail = view.detail.as_ref().unwrap();
        assert!(
            detail.notice.is_some(),
            "the hex copy is labelled, not passed off as text"
        );
    });

    // NULL 单元格显示 NULL 且复制只提示，不把空字符串当值。
    cx.update(|window, cx| window.double_click("table-cell-1:2", cx));
    until_named(&mut cx, "null-open", |cx| {
        view.read_with(cx, |view, _| {
            view.detail.as_ref().is_some_and(|detail| !detail.busy)
        })
    });
    view.read_with(&cx, |view, _| {
        let detail = view.detail.as_ref().expect("the inspector opened");
        assert_eq!(
            (detail.row, detail.column),
            (1, 1),
            "the NULL name cell: {}",
            detail_diagnostic(detail)
        );
        assert_eq!(
            detail.value,
            Some(SqliteValue::Null),
            "{}",
            detail_diagnostic(detail)
        );
    });
    cx.update(|window, cx| window.click("sqlite-value-copy", cx));
    settle(&mut cx);
    view.read_with(&cx, |view, _| {
        let detail = view.detail.as_ref().unwrap();
        assert!(
            detail
                .notice
                .as_deref()
                .is_some_and(|notice| notice.contains("NULL")),
            "the NULL copy explains itself"
        );
    });

    // Escape 关闭检查器；筛选一启动，打开的检查器也不会冒充新查询的单元格。
    cx.simulate_keystrokes("escape");
    settle(&mut cx);
    view.read_with(&cx, |view, _| assert!(view.detail.is_none()));
    cx.update(|window, cx| window.double_click("table-cell-0:2", cx));
    until_named(&mut cx, "detail-reopen", |cx| {
        view.read_with(cx, |view, _| {
            view.detail.as_ref().is_some_and(|d| d.value.is_some())
        })
    });
    cx.update(|window, cx| {
        window.click("sqlite-filter", cx);
        window.input("zzz", cx);
    });
    until_named(&mut cx, "detail-closed-by-filter", |cx| {
        view.read_with(cx, |view, _| {
            view.state.search == "zzz" && view.detail.is_none()
        })
    });
    cx.update(|window, _| {
        assert!(
            !window.find("sqlite-detail").visible(),
            "the panel is gone, not merely hidden"
        );
    });

    assert_eq!(
        std::fs::read(&path).expect("source bytes after inspecting"),
        before,
        "inspecting values did not modify the database"
    );
}

#[gpui_kit::test]
fn a_wal_writer_shares_one_snapshot_until_the_view_refreshes(tcx: &mut TestAppContext) {
    crate::ui::i18n::set_locale("en");
    let dir = super::super::test_host::canonical_tempdir();
    let path = dir.path().join("wal.sqlite");
    let writer = rusqlite::Connection::open(&path).expect("the writer opens");
    writer
        .execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0;\
             CREATE TABLE items(id INTEGER, name TEXT);\
             INSERT INTO items VALUES (1, 'one'), (2, 'two');",
        )
        .expect("WAL fixture");
    let main_before = std::fs::read(&path).expect("main file bytes");

    let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
    open(&app, &mut cx, &path);
    let view = sqlite_view(&app, &cx);
    until_named(&mut cx, "wal-open", |cx| {
        view.read_with(cx, |view, _| view.state.total == Some(2)) && dump(&view, cx).1.len() == 2
    });
    assert!(
        view.read_with(&cx, |view, _| view.status_text())
            .contains("wal"),
        "the status line reports the WAL journal"
    );

    // 并发 writer 提交后：同一读取视图保持打开时的快照。
    writer
        .execute("INSERT INTO items VALUES (3, 'three')", [])
        .expect("the writer commits while the reader is open");
    view.update(&mut cx, |view, cx| {
        view.document.update(cx, |document, cx| {
            document.navigate(0, cx);
            document.load_count(cx);
        });
    });
    settle(&mut cx);
    assert_eq!(
        view.read_with(&cx, |view, _| view.state.total),
        Some(2),
        "the open read view keeps its snapshot"
    );

    // 刷新打开新视图才看到新行；浏览过程不写主库文件。
    cx.update(|window, cx| window.click("sqlite-refresh", cx));
    until_named(&mut cx, "wal-refresh", |cx| {
        view.read_with(cx, |view, _| view.state.total == Some(3))
            && cells_contain(&dump(&view, cx), "three")
    });
    assert_eq!(
        std::fs::read(&path).expect("main file after browsing"),
        main_before,
        "the reader did not write or checkpoint the main file"
    );
    assert!(
        !path.with_extension("sqlite-journal").exists(),
        "no rollback journal appeared"
    );
    drop(writer);
}

/// A TEXT value that is not valid UTF-8 must not be shown through
/// replacement characters: the viewer keeps the bytes visible as hex, and
/// the panel says why.
#[test]
fn invalid_utf8_text_is_shown_as_its_bytes() {
    let value = SqliteValue::Text {
        preview: b"ok\xffA".to_vec(),
        total: 4,
    };
    let text = super::detail_text(&value);
    assert!(
        !text.contains('\u{fffd}'),
        "no replacement characters stood in for bytes: {text}"
    );
    assert!(
        text.contains("ff"),
        "the invalid byte stays visible as hex: {text}"
    );
    assert!(
        text.contains("ok"),
        "the printable bytes keep their ASCII column: {text}"
    );
}
