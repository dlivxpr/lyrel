//! The SQLite page as a real data table.
//!
//! Ported from Glim `glim-app/src/views/reader/sqlite/view.rs` (MIT): a fixed
//! row-number column, one column per visible schema column, bounded cell text
//! and a loading state. Sortable headers report the document's three-state
//! sort back through the owner, and double-clicked cells open the complete
//! value; a header or cell that cannot act advertises nothing.

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::table::{Column, ColumnSort, TableDelegate, TableState};
use gpui_kit::prelude::*;
use gpui_kit::{App, Context, WeakEntity, Window, div, px, rems};

use lyrel_core::host::sqlite::{COLUMN_WINDOW, PAGE_ROWS, SqlitePage, SqliteValue};

use crate::ui::i18n::{L10nKey, t};

use super::sqlite_document::SqliteView;

/// The display text of one value.
///
/// This is presentation only: the typed value keeps exact integers, REAL bit
/// patterns and raw TEXT/BLOB bytes. A truncated preview ends in `…`, and a
/// multi-line text is shown on one line, as Glim's grid does.
pub(super) fn value_text(value: &SqliteValue) -> String {
    match value {
        SqliteValue::Null => t(L10nKey::EditorSqliteNull).to_string(),
        SqliteValue::Integer { value } => value.to_string(),
        SqliteValue::Real { bits } => {
            let value = f64::from_bits(*bits);
            if value.is_nan() {
                "NaN".into()
            } else if value == f64::INFINITY {
                "Infinity".into()
            } else if value == f64::NEG_INFINITY {
                "-Infinity".into()
            } else {
                value.to_string()
            }
        }
        SqliteValue::Text { preview, total } => {
            let mut text = String::from_utf8_lossy(preview)
                .replace(['\r', '\n', '\t'], " ")
                .to_string();
            if (preview.len() as u64) < *total {
                text.push('…');
            }
            text
        }
        SqliteValue::Blob { preview, total } => {
            let mut text = format!("BLOB {total} B · ");
            for (index, byte) in preview.iter().enumerate() {
                if index > 0 {
                    text.push(' ');
                }
                text.push_str(&format!("{byte:02X}"));
            }
            if (preview.len() as u64) < *total {
                text.push('…');
            }
            text
        }
    }
}

/// One page of rows, presented through the Kit table.
///
/// The rows and the column window belong to this presentation, so two groups
/// can show different pages of the same read view without sharing a scroll
/// position or a table selection. The page data itself belongs to the
/// document.
pub(super) struct SqliteGrid {
    columns: Vec<String>,
    rows: Vec<Vec<SqliteValue>>,
    offset: u64,
    start_column: u32,
    sort: Option<(u32, bool)>,
    loading: bool,
    owner: Option<WeakEntity<SqliteView>>,
}

impl SqliteGrid {
    pub(super) fn new() -> Self {
        Self {
            columns: Vec::new(),
            rows: Vec::new(),
            offset: 0,
            start_column: 0,
            sort: None,
            loading: true,
            owner: None,
        }
    }

    /// Bind this grid to the presentation that owns it, once, so header sort
    /// clicks reach the document the presentation reads.
    pub(super) fn set_owner(&mut self, owner: WeakEntity<SqliteView>) {
        self.owner = Some(owner);
    }

    pub(super) fn clear(&mut self) {
        self.columns.clear();
        self.rows.clear();
        self.offset = 0;
    }

    pub(super) fn set_loading(&mut self, loading: bool) {
        self.loading = loading;
    }

    /// Install one page, with the exact column window and sort it was read
    /// for. `sort` names a column of the full schema list — the header arrow
    /// must map back from this page's window to the same column.
    pub(super) fn set_page(
        &mut self,
        columns: &[String],
        page: &SqlitePage,
        sort: Option<(u32, bool)>,
    ) {
        self.start_column = page.start_column;
        self.sort = sort;
        self.columns = columns
            .iter()
            .skip(page.start_column as usize)
            .take(COLUMN_WINDOW as usize)
            .cloned()
            .collect();
        self.offset = page.page.saturating_mul(PAGE_ROWS);
        self.rows = page.rows.clone();
        self.loading = false;
    }

    /// The schema column behind a table column, or `None` for the row number.
    fn schema_column(&self, col: usize) -> Option<usize> {
        col.checked_sub(1)
            .map(|window| window + self.start_column as usize)
    }
}

impl TableDelegate for SqliteGrid {
    fn columns_count(&self, _: &App) -> usize {
        self.columns.len() + 1
    }

    fn rows_count(&self, _: &App) -> usize {
        self.rows.len()
    }

    fn column(&self, index: usize, _cx: &App) -> Column {
        if index == 0 {
            Column::new("row", "#")
                .width(px(64.))
                .fixed_left()
                .movable(false)
        } else {
            let schema = index - 1 + self.start_column as usize;
            let mut column = Column::new(index.to_string(), self.columns[index - 1].clone())
                .width(px(220.))
                .movable(false)
                .resizable(true)
                .sortable();
            if let Some((sorted, descending)) = self.sort
                && sorted as usize == schema
            {
                column = column.sort(if descending {
                    ColumnSort::Descending
                } else {
                    ColumnSort::Ascending
                });
            }
            column
        }
    }

    fn cell_text(&self, row: usize, col: usize, _: &App) -> String {
        if col == 0 {
            return (self.offset + row as u64 + 1).to_string();
        }
        self.rows
            .get(row)
            .and_then(|cells| cells.get(col - 1))
            .map(value_text)
            .unwrap_or_default()
    }

    fn loading(&self, _: &App) -> bool {
        self.loading && self.rows.is_empty()
    }

    fn render_td(
        &mut self,
        row: usize,
        col: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        if col == 0 {
            return div()
                .truncate()
                .text_color(muted)
                .child((self.offset + row as u64 + 1).to_string())
                .into_any_element();
        }
        let value = self.rows.get(row).and_then(|cells| cells.get(col - 1));
        let (text, muted_value) = match value {
            Some(value @ (SqliteValue::Null | SqliteValue::Blob { .. })) => {
                (value_text(value), true)
            }
            Some(value) => (value_text(value), false),
            None => (String::new(), true),
        };
        div()
            .truncate()
            .font_family(cx.theme().mono_font_family.clone())
            .text_size(rems(0.8125))
            .when(muted_value, |cell| cell.text_color(muted))
            .child(text)
            .into_any_element()
    }

    fn perform_sort(
        &mut self,
        col: usize,
        sort: ColumnSort,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) {
        let Some(schema) = self.schema_column(col) else {
            return;
        };
        let sort = match sort {
            ColumnSort::Default => None,
            ColumnSort::Ascending => Some((schema as u32, false)),
            ColumnSort::Descending => Some((schema as u32, true)),
        };
        let Some(owner) = self.owner.clone() else {
            return;
        };
        // The table is mid-update; the sort itself asks the document for a
        // new page, which rebuilds this grid.
        cx.defer(move |cx| {
            if let Some(owner) = owner.upgrade() {
                owner.update(cx, |view, cx| view.apply_sort(sort, cx));
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn value_text_is_typed_and_marks_truncation() {
        assert_eq!(value_text(&SqliteValue::Null), "NULL");
        assert_eq!(
            value_text(&SqliteValue::Integer {
                value: 9007199254740993
            }),
            "9007199254740993"
        );
        assert_eq!(
            value_text(&SqliteValue::real(f64::INFINITY.to_bits())),
            "Infinity"
        );
        assert_eq!(
            value_text(&SqliteValue::real(f64::NEG_INFINITY.to_bits())),
            "-Infinity"
        );
        assert_eq!(
            value_text(&SqliteValue::Text {
                preview: b"line\nbreak".to_vec(),
                total: 10
            }),
            "line break"
        );
        assert_eq!(
            value_text(&SqliteValue::Text {
                preview: vec![b'a'; 4],
                total: 9
            }),
            "aaaa…"
        );
        assert_eq!(
            value_text(&SqliteValue::Blob {
                preview: vec![0x00, 0xff],
                total: 2
            }),
            "BLOB 2 B · 00 FF"
        );
    }

    #[test]
    fn sort_requests_name_the_schema_column_not_the_window_slot() {
        let mut grid = SqliteGrid::new();
        grid.start_column = 3;
        assert_eq!(grid.schema_column(0), None);
        assert_eq!(grid.schema_column(1), Some(3));
        assert_eq!(grid.schema_column(2), Some(4));
    }
}
