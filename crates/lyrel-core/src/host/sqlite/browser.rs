//! The restricted query shapes, ported from Glim's SQLite browser.
//!
//! Everything here takes already-validated identifiers: the table, its
//! columns and its deterministic order come from [`super::SqliteColumns`],
//! which was read from this view's own schema. Identifiers still pass through
//! [`quote`] — that is the single exit for a name into SQL — and the search
//! text is always a bound parameter.
//!
//! The differences from the upstream module are deliberate: cell values keep
//! their SQLite storage class (Glim projected every value to a display
//! string), previews are bounded per storage class rather than via one CASE
//! that coerces to TEXT, and there is no snapshot/pool argument because one
//! read view owns one connection.

use rusqlite::{Connection, OptionalExtension, Row, params_from_iter};

use super::{
    BLOB_PREVIEW_BYTES, COLUMN_WINDOW, PAGE_ROWS, SqliteColumns, SqliteObject, SqliteObjectKind,
    SqlitePage, SqliteQuery, SqliteValue, TEXT_PREVIEW_BYTES,
};

/// The single place a schema identifier becomes SQL text.
pub(crate) fn quote(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// The schema and journal mode of the database, read inside the caller's
/// transaction.
pub(crate) fn opening(connection: &Connection) -> rusqlite::Result<(String, Vec<SqliteObject>)> {
    let objects = {
        let mut statement = connection.prepare(
            "SELECT type, name, coalesce(sql,'') FROM sqlite_schema \
             WHERE type IN ('table','view') ORDER BY name",
        )?;
        let rows = statement.query_map([], |row| {
            let kind: String = row.get(0)?;
            let name: String = row.get(1)?;
            let sql: String = row.get(2)?;
            let kind = match kind.as_str() {
                "table" => SqliteObjectKind::Table,
                // The SELECT above cannot yield anything else.
                _ => SqliteObjectKind::View,
            };
            Ok(SqliteObject { name, kind, sql })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    // A read, not `journal_mode = ...`: the mode is reported, never changed.
    let journal_mode: String = connection.query_row("PRAGMA journal_mode", [], |row| row.get(0))?;
    Ok((journal_mode, objects))
}

/// Column names and the deterministic order for one table or view.
///
/// Ported from Glim `Database::columns`: hidden virtual-table columns are not
/// browsable, generated columns stay visible, a rowid alias is chosen only
/// when no real column shadows it, and a `WITHOUT ROWID` table falls back to
/// its primary key (then every column).
pub(crate) fn columns(connection: &Connection, table: &str) -> rusqlite::Result<SqliteColumns> {
    let entries = {
        // A table-valued pragma initializes a virtual table and triggers
        // schema-write authorization even for this metadata read. Use the
        // direct read-only pragma instead; identifiers still use `quote`.
        let mut statement = connection.prepare(&format!("PRAGMA table_xinfo({})", quote(table)))?;
        let mut rows = statement.query([])?;
        let mut entries = Vec::new();
        while let Some(row) = rows.next()? {
            if row.get::<_, i64>(6)? != 1 {
                entries.push((row.get::<_, String>(1)?, row.get::<_, i64>(5)?));
            }
        }
        entries
    };
    let names = entries
        .iter()
        .map(|(name, _)| name.clone())
        .collect::<Vec<_>>();
    let mut keys = entries
        .into_iter()
        .filter(|(_, pk)| *pk > 0)
        .collect::<Vec<_>>();
    keys.sort_by_key(|(_, pk)| *pk);
    let (kind, definition): (String, String) = connection.query_row(
        "SELECT type, coalesce(sql,'') FROM sqlite_schema WHERE name=?",
        [table],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    let rowid = ["_rowid_", "rowid", "oid"]
        .into_iter()
        .find(|alias| !names.iter().any(|name| name.eq_ignore_ascii_case(alias)));
    let order = if kind == "table"
        && !definition.to_ascii_uppercase().contains("WITHOUT ROWID")
        && let Some(rowid) = rowid
    {
        vec![rowid.to_string()]
    } else if !keys.is_empty() {
        keys.into_iter().map(|(name, _)| name).collect()
    } else {
        names.clone()
    };
    Ok(SqliteColumns { names, order })
}

/// Reject a query whose identifiers are not part of the view's schema.
///
/// The Host compares names exactly as SQLite returned them, so a quoted name
/// cannot smuggle a second statement or a function call into the SQL.
pub(crate) fn validate(query: &SqliteQuery, columns: &SqliteColumns) -> rusqlite::Result<()> {
    if query.table.is_empty() {
        return Err(invalid("a page query must name a table or view"));
    }
    if query.columns.is_empty() {
        return Err(invalid("a page query must name at least one column"));
    }
    let known = |name: &str| columns.names.iter().any(|known| known == name);
    if let Some(unknown) = query.columns.iter().find(|column| !known(column)) {
        return Err(invalid(format!(
            "column {unknown:?} is not part of this table in this read view"
        )));
    }
    let order_known = |name: &str| known(name) || columns.order.iter().any(|order| order == name);
    if let Some(unknown) = query.order.iter().find(|column| !order_known(column)) {
        return Err(invalid(format!(
            "order column {unknown:?} is not part of this table in this read view"
        )));
    }
    if query
        .sort
        .is_some_and(|(index, _)| index as usize >= query.columns.len())
    {
        return Err(invalid("the sort column is outside the column list"));
    }
    Ok(())
}

fn invalid(message: impl Into<String>) -> rusqlite::Error {
    rusqlite::Error::SqliteFailure(
        rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_MISUSE),
        Some(message.into()),
    )
}

/// The `WHERE` clause of the single-column-substring search.
///
/// One `?1` is reused across every column so the query never approaches
/// SQLite's bound-variable limit; the parenthesis tree keeps the expression
/// depth logarithmic for very wide tables.
fn predicate(query: &SqliteQuery) -> (String, Vec<String>) {
    let mut values = Vec::new();
    if query.search.is_empty() {
        return (String::new(), values);
    }
    values.push(query.search.clone());
    fn disjunction(parts: &[String]) -> String {
        match parts.len() {
            0 => "0".into(),
            1 => parts[0].clone(),
            len => format!(
                "({} OR {})",
                disjunction(&parts[..len / 2]),
                disjunction(&parts[len / 2..])
            ),
        }
    }
    let conditions = disjunction(
        &query
            .columns
            .iter()
            .map(|column| {
                format!(
                    "instr(lower(CAST({} AS TEXT)), lower(?1)) > 0",
                    quote(column)
                )
            })
            .collect::<Vec<_>>(),
    );
    (format!(" WHERE {conditions}"), values)
}

/// `WHERE … ORDER BY …` and the bound search text.
fn tail(query: &SqliteQuery) -> (String, Vec<String>) {
    let (mut tail, values) = predicate(query);
    let mut order = Vec::new();
    if let Some((index, descending)) = query.sort
        && let Some(column) = query.columns.get(index as usize)
    {
        order.push(format!(
            "{} {}",
            quote(column),
            if descending { "DESC" } else { "ASC" }
        ));
    }
    order.extend(query.order.iter().map(|column| quote(column)));
    if !order.is_empty() {
        tail.push_str(&format!(" ORDER BY {}", order.join(",")));
    }
    (tail, values)
}

fn page_offset(query: &SqliteQuery) -> rusqlite::Result<u64> {
    query.page.checked_mul(PAGE_ROWS).ok_or_else(|| {
        invalid(format!(
            "page {} is beyond the rows a single query can skip",
            query.page
        ))
    })
}

/// Total rows matching the query, independent of the current page.
pub(crate) fn count(connection: &Connection, query: &SqliteQuery) -> rusqlite::Result<u64> {
    let (predicate, values) = predicate(query);
    let sql = format!("SELECT count(*) FROM {}{predicate}", quote(&query.table));
    let total: i64 =
        connection.query_row(&sql, params_from_iter(values.iter()), |row| row.get(0))?;
    Ok(u64::try_from(total).unwrap_or(0))
}

/// The projection for one bounded cell preview.
///
/// Three expressions per column: the storage class, the value (bounded for
/// TEXT and BLOB), and the full byte length. `CASE` returns a TEXT value's
/// bytes as a BLOB so the preview can never be truncated mid-codepoint in a
/// way the wire mistakes for the original text; the storage class column is
/// what tells the caller it was TEXT.
fn preview_projection(column: &str) -> [String; 3] {
    let name = quote(column);
    [
        format!("typeof({name})"),
        format!(
            "CASE typeof({name}) \
             WHEN 'text' THEN coalesce(substr(CAST({name} AS BLOB),1,{TEXT_PREVIEW_BYTES}),x'') \
             WHEN 'blob' THEN coalesce(substr({name},1,{BLOB_PREVIEW_BYTES}),x'') \
             WHEN 'integer' THEN {name} \
             WHEN 'real' THEN {name} \
             ELSE NULL END"
        ),
        format!("length(CAST({name} AS BLOB))"),
    ]
}

fn value_of_row(row: &Row<'_>, index: usize) -> rusqlite::Result<SqliteValue> {
    let kind: String = row.get(index * 3)?;
    let total: Option<i64> = row.get(index * 3 + 2)?;
    let total = total.map_or(0, |total| u64::try_from(total).unwrap_or(0));
    let value_index = index * 3 + 1;
    match kind.as_str() {
        "null" => Ok(SqliteValue::Null),
        "integer" => Ok(SqliteValue::Integer {
            value: row.get(value_index)?,
        }),
        "real" => Ok(SqliteValue::real(row.get::<_, f64>(value_index)?.to_bits())),
        "text" => Ok(SqliteValue::Text {
            preview: row.get(value_index)?,
            total,
        }),
        "blob" => Ok(SqliteValue::Blob {
            preview: row.get(value_index)?,
            total,
        }),
        // SQLite has exactly five storage classes; a sixth would mean the
        // library and this projection disagree, and a lossy value is worse
        // than a failed page.
        other => Err(rusqlite::Error::FromSqlConversionFailure(
            value_index,
            rusqlite::types::Type::Null,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("unexpected SQLite storage class {other}"),
            )),
        )),
    }
}

/// Read one page, plus a look-ahead row so `more` is truthful.
pub(crate) fn page(connection: &Connection, query: &SqliteQuery) -> rusqlite::Result<SqlitePage> {
    let start = query
        .start_column
        .min(query.columns.len().saturating_sub(1) as u32) as usize;
    let window = query
        .columns
        .iter()
        .skip(start)
        .take(COLUMN_WINDOW as usize)
        .collect::<Vec<_>>();
    let projection = window
        .iter()
        .flat_map(|column| preview_projection(column))
        .collect::<Vec<_>>()
        .join(",");
    let (tail, values) = tail(query);
    let sql = format!(
        "SELECT {projection} FROM {}{tail} LIMIT {} OFFSET {}",
        quote(&query.table),
        PAGE_ROWS + 1,
        page_offset(query)?
    );
    let mut statement = connection.prepare(&sql)?;
    let mut cursor = statement.query(params_from_iter(values.iter()))?;
    let mut rows = Vec::new();
    let mut more = false;
    while let Some(row) = cursor.next()? {
        if rows.len() as u64 == PAGE_ROWS {
            // The look-ahead row proves there is a next page; it is not part
            // of this one.
            more = true;
            break;
        }
        let mut cells = Vec::with_capacity(window.len());
        for index in 0..window.len() {
            cells.push(value_of_row(row, index)?);
        }
        rows.push(cells);
    }
    Ok(SqlitePage {
        page: query.page,
        start_column: start as u32,
        rows,
        more,
    })
}

/// One complete value, in a bounded chunk.
///
/// INTEGER and REAL come back whole and exact: their chunk offset is not
/// meaningful and `more` is false. TEXT and BLOB return up to `max_bytes`
/// from `offset`, with `total` still the full byte length.
pub(crate) fn value(
    connection: &Connection,
    query: &SqliteQuery,
    row: u32,
    column: u32,
    offset: u64,
    max_bytes: u64,
) -> rusqlite::Result<SqliteValue> {
    let name = quote(
        query
            .columns
            .get(column as usize)
            .ok_or_else(|| invalid("the requested column is outside this page's column list"))?,
    );
    let start = offset
        .checked_add(1)
        .filter(|start| *start <= i64::MAX as u64)
        .ok_or_else(|| invalid("the value offset is beyond what SQLite can address"))?;
    let size = max_bytes + 1;
    let row_offset = page_offset(query)? + u64::from(row);
    let (tail, values) = tail(query);
    let sql = format!(
        "SELECT typeof({name}), \
         CASE typeof({name}) \
         WHEN 'text' THEN coalesce(substr(CAST({name} AS BLOB),{start},{size}),x'') \
         WHEN 'blob' THEN coalesce(substr({name},{start},{size}),x'') \
         WHEN 'integer' THEN {name} \
         WHEN 'real' THEN {name} \
         ELSE NULL END, \
         length(CAST({name} AS BLOB)) \
         FROM {}{tail} LIMIT 1 OFFSET {row_offset}",
        quote(&query.table)
    );
    let row = connection
        .query_row(&sql, params_from_iter(values.iter()), |row| {
            let kind: String = row.get(0)?;
            let total: Option<i64> = row.get(2)?;
            let total = total.map_or(0, |total| u64::try_from(total).unwrap_or(0));
            let value = match kind.as_str() {
                "null" => SqliteValue::Null,
                "integer" => SqliteValue::Integer { value: row.get(1)? },
                "real" => SqliteValue::real(row.get::<_, f64>(1)?.to_bits()),
                "text" | "blob" => {
                    let mut chunk: Vec<u8> = row.get(1)?;
                    // The SQL asks for one byte past the chunk and the client
                    // compares `offset + preview.len()` with `total`.
                    if chunk.len() as u64 > max_bytes {
                        chunk.truncate(max_bytes as usize);
                    }
                    if kind.as_str() == "text" {
                        SqliteValue::Text {
                            preview: chunk,
                            total,
                        }
                    } else {
                        SqliteValue::Blob {
                            preview: chunk,
                            total,
                        }
                    }
                }
                other => {
                    return Err(rusqlite::Error::FromSqlConversionFailure(
                        1,
                        rusqlite::types::Type::Null,
                        Box::new(std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            format!("unexpected SQLite storage class {other}"),
                        )),
                    ));
                }
            };
            // `more` is not part of the value type; the chunk length against
            // `total` tells a caller the same thing without a second field.
            Ok(value)
        })
        .optional()?
        .ok_or(rusqlite::Error::QueryReturnedNoRows)?;
    Ok(row)
}
