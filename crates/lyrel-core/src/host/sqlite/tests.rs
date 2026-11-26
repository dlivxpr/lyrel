use super::*;
use crate::host::sqlite::SqliteCellRead;
use std::fs;
use std::time::{Duration, Instant};

fn temp_db() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("data.sqlite");
    (dir, path)
}

fn write_sql(path: &std::path::Path, sql: &str) -> rusqlite::Connection {
    let connection = rusqlite::Connection::open(path).unwrap();
    connection.execute_batch(sql).unwrap();
    connection
}

fn query(service: &SqliteService, view: u64, table: &str) -> SqliteQuery {
    let columns = service
        .columns(view, table, &SqliteCancel::default())
        .unwrap();
    SqliteQuery {
        table: table.to_string(),
        columns: columns.names,
        order: columns.order,
        ..SqliteQuery::default()
    }
}

fn text(value: &SqliteValue) -> Option<&[u8]> {
    match value {
        SqliteValue::Text { preview, .. } => Some(preview),
        _ => None,
    }
}

#[test]
fn identifiers_are_quoted_by_one_exit() {
    assert_eq!(browser::quote("plain"), "\"plain\"");
    assert_eq!(browser::quote("quoted\"table"), "\"quoted\"\"table\"");
    assert_eq!(browser::quote("名,称"), "\"名,称\"");
}

#[test]
fn schema_page_and_definitions_come_from_one_read_view() {
    let (_dir, path) = temp_db();
    write_sql(
        &path,
        "CREATE TABLE alpha(id INTEGER PRIMARY KEY, \"名,称\" TEXT, body BLOB);\
         CREATE VIEW alpha_names AS SELECT \"名,称\" FROM alpha;\
         INSERT INTO alpha VALUES (1, '中文', x'00ff');",
    );
    let service = SqliteService::new();
    let cancel = SqliteCancel::default();
    let opened = service.open(&path, &cancel).unwrap();
    assert_eq!(opened.journal_mode, "delete");
    assert_eq!(opened.objects.len(), 2);
    let alpha = opened.objects.iter().find(|o| o.name == "alpha").unwrap();
    assert_eq!(alpha.kind, SqliteObjectKind::Table);
    assert!(alpha.sql.starts_with("CREATE TABLE alpha"));
    let view = opened.view;
    let columns = service.columns(view, "alpha", &cancel).unwrap();
    assert_eq!(columns.names, ["id", "名,称", "body"]);
    assert_eq!(columns.order, ["_rowid_"]);
    let mut query = query(&service, view, "alpha");
    let page = service.page(view, &query, &cancel).unwrap();
    assert_eq!(page.page, 0);
    assert_eq!(page.start_column, 0);
    assert!(!page.more);
    assert_eq!(page.rows.len(), 1);
    assert_eq!(page.rows[0][0], SqliteValue::Integer { value: 1 });
    assert_eq!(text(&page.rows[0][1]).unwrap(), "中文".as_bytes());
    assert!(matches!(page.rows[0][2], SqliteValue::Blob { .. }));
    // A sort/order name that is not this table's is refused before SQL.
    query.columns = vec!["missing".into()];
    let error = service.page(view, &query, &cancel).unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
    // The view never leaves the schema list silently: a missing table is not
    // an empty page.
    let error = service.columns(view, "missing", &cancel).unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
}

#[test]
fn every_storage_class_survives_the_wire_value() {
    let (_dir, path) = temp_db();
    let long = "长".repeat(300);
    let connection = write_sql(
        &path,
        "CREATE TABLE values_(n, i, r, t, b);\
         INSERT INTO values_(n, i, r, t, b) VALUES (NULL, 9007199254740993, 3.5, '中文', x'00ff10');",
    );
    // Non-finite REAL and invalid UTF-8 TEXT are rows too; bind them so the
    // fixture is unambiguous about the storage class.
    connection
        .execute(
            "INSERT INTO values_(n, i, r, t, b) VALUES (NULL, 0, ?1, CAST(x'fffe61' AS TEXT), x'')",
            [f64::INFINITY],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO values_(n, i, r, t, b) VALUES (NULL, 0, ?1, ?2, x'')",
            rusqlite::params![f64::NEG_INFINITY, long],
        )
        .unwrap();
    // SQLite stores a bound NaN as NULL; the type still has to round-trip the
    // bit pattern over the wire for any value SQLite can hold.
    connection
        .execute(
            "INSERT INTO values_(n, i, r, t, b) VALUES (NULL, 0, ?1, 'nan-as-null', x'')",
            [f64::NAN],
        )
        .unwrap();
    drop(connection);
    let service = SqliteService::new();
    let cancel = SqliteCancel::default();
    let opened = service.open(&path, &cancel).unwrap();
    let view = opened.view;
    let query = query(&service, view, "values_");
    let page = service.page(view, &query, &cancel).unwrap();
    assert_eq!(page.rows.len(), 4);

    assert_eq!(page.rows[0][0], SqliteValue::Null);
    assert_eq!(
        page.rows[0][1],
        SqliteValue::Integer {
            value: 9007199254740993
        }
    );
    assert_eq!(page.rows[0][2].real_value(), Some(3.5));
    assert_eq!(text(&page.rows[0][3]).unwrap(), "中文".as_bytes());
    let SqliteValue::Blob { preview, total } = &page.rows[0][4] else {
        panic!("expected a blob");
    };
    assert_eq!(preview, &[0x00, 0xff, 0x10]);
    assert_eq!(*total, 3);

    let infinite = page.rows[1][2].real_value().unwrap();
    assert!(infinite.is_infinite() && infinite.is_sign_positive());
    assert_eq!(text(&page.rows[1][3]).unwrap(), &[0xff, 0xfe, b'a']);
    let negative = page.rows[2][2].real_value().unwrap();
    assert!(negative.is_infinite() && negative.is_sign_negative());
    let SqliteValue::Text { preview, total } = &page.rows[2][3] else {
        panic!("expected text");
    };
    assert_eq!(*total, long.len() as u64);
    assert_eq!(preview.len() as u64, TEXT_PREVIEW_BYTES);
    assert!(page.rows[2][3].is_truncated());
    assert_eq!(
        page.rows[3][2],
        SqliteValue::Null,
        "SQLite stores NaN as NULL"
    );
    // Every non-finite REAL bit pattern still crosses JSON reversibly.
    for bits in [
        f64::NAN.to_bits(),
        f64::INFINITY.to_bits(),
        f64::NEG_INFINITY.to_bits(),
    ] {
        let value = SqliteValue::real(bits);
        let json = serde_json::to_string(&value).unwrap();
        let back: SqliteValue = serde_json::from_str(&json).unwrap();
        assert_eq!(back, value);
    }

    // The complete value is raw bytes in bounded chunks, not a display string.
    let chunk = service
        .value(
            view,
            &query,
            SqliteCellRead {
                row: 2,
                column: 3,
                offset: 0,
                max_bytes: VALUE_CHUNK_BYTES,
            },
            &cancel,
        )
        .unwrap();
    let SqliteValue::Text { preview, total } = chunk else {
        panic!("expected text");
    };
    assert_eq!(total, long.len() as u64);
    assert_eq!(preview, long.as_bytes());
    let tail = service
        .value(
            view,
            &query,
            SqliteCellRead {
                row: 2,
                column: 3,
                offset: TEXT_PREVIEW_BYTES,
                max_bytes: 8,
            },
            &cancel,
        )
        .unwrap();
    let SqliteValue::Text { preview, .. } = tail else {
        panic!("expected text");
    };
    assert_eq!(preview.len(), 8);
    for (row, cells) in page.rows.iter().enumerate().skip(1).take(3) {
        assert_eq!(
            cells[4],
            SqliteValue::Blob {
                preview: Vec::new(),
                total: 0
            }
        );
        assert_eq!(
            service
                .value(
                    view,
                    &query,
                    SqliteCellRead {
                        row: row as u32,
                        column: 4,
                        offset: 0,
                        max_bytes: VALUE_CHUNK_BYTES
                    },
                    &cancel
                )
                .unwrap(),
            SqliteValue::Blob {
                preview: Vec::new(),
                total: 0
            },
        );
    }
}

#[test]
fn a_wal_writer_sees_the_read_view_keep_its_snapshot_until_refresh() {
    let (_dir, path) = temp_db();
    let writer = write_sql(
        &path,
        "PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0;\
         CREATE TABLE items(id INTEGER PRIMARY KEY, value TEXT);\
         INSERT INTO items(value) VALUES ('one'), ('two'), ('three'), ('four'), ('five');",
    );
    writer.busy_timeout(Duration::ZERO).unwrap();
    let service = SqliteService::new();
    let cancel = SqliteCancel::default();
    let opened = service.open(&path, &cancel).unwrap();
    let view = opened.view;
    assert_eq!(opened.journal_mode, "wal");
    let items_query = query(&service, view, "items");
    assert_eq!(service.count(view, &items_query, &cancel).unwrap(), 5);

    writer
        .execute("INSERT INTO items(value) VALUES ('six')", [])
        .unwrap();
    // The same transaction still sees the version it started on, even after
    // the writer committed.
    assert_eq!(service.count(view, &items_query, &cancel).unwrap(), 5);
    let page = service.page(view, &items_query, &cancel).unwrap();
    assert_eq!(page.rows.len(), 5);
    assert!(!page.more);

    // From here on only browsing runs, so any source change would be ours.
    // -shm is SQLite's read coordination and is deliberately not asserted
    // unchanged; main and -wal are the database.
    let main_before = fs::read(&path).unwrap();
    let wal_before = fs::read(sidecar(&path, "-wal")).unwrap();
    // A refresh is a new view, and only that one sees the new row.
    let refreshed = service.open(&path, &cancel).unwrap();
    assert_ne!(refreshed.view, view);
    let refreshed_query = query(&service, refreshed.view, "items");
    assert_eq!(
        service
            .count(refreshed.view, &refreshed_query, &cancel)
            .unwrap(),
        6
    );
    let refreshed_page = service
        .page(refreshed.view, &refreshed_query, &cancel)
        .unwrap();
    assert_eq!(refreshed_page.rows.len(), 6);
    assert_eq!(fs::read(&path).unwrap(), main_before);
    assert_eq!(fs::read(sidecar(&path, "-wal")).unwrap(), wal_before);
    assert!(!sidecar(&path, "-journal").exists());

    // A live read view does hold checkpointing back; closing it releases the
    // read transaction, so the writer can truncate the WAL again.
    let (busy, _) = checkpoint(&writer);
    assert_eq!(busy, 1, "the open read view still holds a read mark");
    service.close(view);
    service.close(refreshed.view);
    let (busy, _) = checkpoint(&writer);
    assert_eq!(
        busy, 0,
        "closing the views released their read transactions"
    );
}

fn sidecar(path: &std::path::Path, suffix: &str) -> std::path::PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    std::path::PathBuf::from(name)
}

fn checkpoint(connection: &rusqlite::Connection) -> (i64, i64) {
    connection
        .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .unwrap()
}

#[test]
fn idle_and_max_age_retire_views_without_a_later_request() {
    let (_dir, path) = temp_db();
    write_sql(&path, "CREATE TABLE items(id INTEGER);");
    let service = SqliteService::with_limits(SqliteLimits {
        idle: Duration::from_millis(40),
        max_age: Duration::from_secs(5),
    });
    let cancel = SqliteCancel::default();
    let opened = service.open(&path, &cancel).unwrap();
    let first_query = query(&service, opened.view, "items");
    std::thread::sleep(Duration::from_millis(80));
    service.sweep_now();
    assert_eq!(
        service.len(),
        0,
        "the idle view is gone without another request"
    );
    let error = service
        .count(opened.view, &first_query, &cancel)
        .unwrap_err();
    assert!(is_stale_view(&error), "{error}");

    // Continuous use does not renew the absolute age.
    let service = SqliteService::with_limits(SqliteLimits {
        idle: Duration::from_secs(5),
        max_age: Duration::from_millis(60),
    });
    let opened = service.open(&path, &cancel).unwrap();
    let aged_query = query(&service, opened.view, "items");
    let deadline = Instant::now() + Duration::from_millis(250);
    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(15));
        service.sweep_now();
    }
    assert_eq!(service.len(), 0, "a continuously used view still ages out");
    assert!(is_stale_view(
        &service
            .count(opened.view, &aged_query, &cancel)
            .unwrap_err()
    ));
}

#[test]
fn pagination_reads_the_column_window_and_reports_more() {
    let (_dir, path) = temp_db();
    write_sql(
        &path,
        include_str!("../../../tests/fixtures/glim-reader-sqlite.sql"),
    );
    let service = SqliteService::new();
    let cancel = SqliteCancel::default();
    let opened = service.open(&path, &cancel).unwrap();
    let view = opened.view;
    let mut query = query(&service, view, "alpha");
    assert_eq!(query.columns[1], "名,称");
    assert_eq!(service.count(view, &query, &cancel).unwrap(), 520);
    let page = service.page(view, &query, &cancel).unwrap();
    assert_eq!(page.rows.len(), PAGE_ROWS as usize);
    assert!(page.more);
    query.page = 1;
    let page = service.page(view, &query, &cancel).unwrap();
    assert_eq!(page.rows.len(), 20);
    assert!(!page.more);
    assert_eq!(text(&page.rows[19][1]).unwrap(), "中文 520".as_bytes());
    // A multi-line TEXT preview stays raw; it is not rewritten by the reader.
    query.page = 0;
    let page = service.page(view, &query, &cancel).unwrap();
    assert_eq!(text(&page.rows[0][2]).unwrap(), b"line 1\nline 2 \"yes\"");
    // Search is one bound parameter, reused across columns.
    query.search = "520".into();
    assert_eq!(service.count(view, &query, &cancel).unwrap(), 1);
    let page = service.page(view, &query, &cancel).unwrap();
    assert_eq!(page.rows.len(), 1);
}

#[test]
fn a_wide_table_pages_its_columns_and_keeps_without_rowid_order() {
    let (_dir, path) = temp_db();
    let columns = (0..300)
        .map(|index| format!("c{index} TEXT"))
        .collect::<Vec<_>>()
        .join(",");
    write_sql(
        &path,
        &format!(
            "CREATE TABLE wide({columns});\
             INSERT INTO wide(c299) VALUES ('last-column');\
             CREATE TABLE keyed(a TEXT, b INTEGER, PRIMARY KEY (a, b)) WITHOUT ROWID;\
             INSERT INTO keyed VALUES ('x', 1);"
        ),
    );
    let service = SqliteService::new();
    let cancel = SqliteCancel::default();
    let opened = service.open(&path, &cancel).unwrap();
    let view = opened.view;
    let mut query = query(&service, view, "wide");
    assert_eq!(query.columns.len(), 300);
    query.start_column = 299;
    let page = service.page(view, &query, &cancel).unwrap();
    assert_eq!(page.start_column, 299);
    assert_eq!(
        page.rows[0][0],
        SqliteValue::Text {
            preview: b"last-column".to_vec(),
            total: 11,
        }
    );
    // Start columns beyond the last one clamp into the table instead of
    // reading nothing.
    query.start_column = 10_000;
    assert_eq!(
        service.page(view, &query, &cancel).unwrap().start_column,
        299
    );

    let keyed = service.columns(view, "keyed", &cancel).unwrap();
    assert_eq!(keyed.order, ["a", "b"]);
}

#[test]
fn a_cancelled_query_stops_and_a_later_query_on_the_same_view_still_runs() {
    let (_dir, path) = temp_db();
    write_sql(
        &path,
        "CREATE TABLE items(id INTEGER);\
         CREATE VIEW slow AS WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM n) SELECT x FROM n;",
    );
    let service = SqliteService::new();
    let cancel = SqliteCancel::default();
    let opened = service.open(&path, &cancel).unwrap();
    let view = opened.view;
    let mut slow_query = query(&service, view, "slow");
    slow_query.seconds = QUERY_SECONDS_MAX;

    let page_cancel = SqliteCancel::new();
    let worker = {
        let service = service.clone();
        let page_cancel = page_cancel.clone();
        let slow_query = slow_query.clone();
        std::thread::spawn(move || service.page(view, &slow_query, &page_cancel))
    };
    std::thread::sleep(Duration::from_millis(120));
    page_cancel.cancel();
    let error = worker.join().unwrap().unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::Interrupted, "{error}");

    // A fresh operation on the same view is not that cancellation's target.
    let fresh = SqliteCancel::default();
    let items = query(&service, view, "items");
    assert!(service.page(view, &items, &fresh).is_ok());
    // Cancelling the old token afterwards cannot reach the new operation.
    page_cancel.cancel();
    assert!(service.page(view, &items, &fresh).is_ok());
}

#[test]
fn opening_refuses_files_that_are_not_databases_and_never_creates_missing_ones() {
    let (_dir, path) = temp_db();
    fs::write(&path, b"not a database").unwrap();
    let service = SqliteService::new();
    let error = service.open(&path, &SqliteCancel::default()).unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidData, "{error}");
    let missing = path.with_file_name("missing.sqlite");
    let error = service
        .open(&missing, &SqliteCancel::default())
        .unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
    assert!(
        !missing.exists(),
        "a read-only open must not create a database"
    );
}

#[cfg(unix)]
#[test]
fn a_wal_needing_recovery_is_reported_instead_of_repaired() {
    use std::os::unix::fs::PermissionsExt as _;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("recovery.sqlite");
    let connection = write_sql(
        &path,
        "PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0; CREATE TABLE items(id INTEGER); INSERT INTO items VALUES (1);",
    );
    connection
        .set_db_config(
            rusqlite::config::DbConfig::SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE,
            true,
        )
        .unwrap();
    let main_before = fs::read(&path).unwrap();
    let wal = sidecar(&path, "-wal");
    let wal_before = fs::read(&wal).unwrap();
    drop(connection);
    // NO_CKPT_ON_CLOSE intentionally keeps the WAL and its coordination
    // file. Model a crashed writer by removing only this fixture's SHM.
    fs::remove_file(sidecar(&path, "-shm")).unwrap();
    assert!(wal.exists(), "the writer left a WAL to recover");
    assert!(!sidecar(&path, "-shm").exists());
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o555)).unwrap();
    let service = SqliteService::new();
    let result = service.open(&path, &SqliteCancel::default());
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o755)).unwrap();
    let error = result.unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::Unsupported, "{error}");
    assert!(
        error.to_string().contains("recovery"),
        "the refusal must say what it will not do: {error}"
    );
    assert_eq!(fs::read(&path).unwrap(), main_before);
    assert_eq!(fs::read(&wal).unwrap(), wal_before);
}

#[test]
fn closing_an_unknown_view_is_idempotent() {
    let service = SqliteService::new();
    assert!(!service.close(42));
    assert!(!service.close(42));
}
