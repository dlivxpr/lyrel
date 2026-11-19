//! A read-only SQLite database as a workbench document.
//!
//! Ported from Glim `glim-app/src/views/reader/sqlite.rs` (MIT): the schema
//! list, table selection, the debounced filter, three-state column sort,
//! bounded page navigation, column windows, the complete-value inspector and
//! the loading/error/retry states. Replaced: the backend is the owning
//! Host's read view — not a local snapshot copy — every page names the view
//! it came from, and refresh opens a new view instead of silently switching
//! snapshot under the reader.
//!
//! The document owns the read view and the data read from it. Each split
//! group gets its own presentation ([`SqliteView`]) with its own focus,
//! table scroll, selection and value inspector, so two views of one
//! database do not move each other.

use std::io;
use std::path::PathBuf;
use std::time::Duration;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::scroll::ScrollbarMode;
use gpui_kit::component::table::{DataTable, TableEvent, TableSelection, TableState};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, h_flex, v_flex,
};
use gpui_kit::prelude::*;
use gpui_kit::{
    App, Context, Entity, FocusHandle, Focusable, MouseButton, SharedString, Subscription,
    TestSupportExt as _, Window, div, px, rems,
};

pub use lyrel_core::host::sqlite::SqliteValue;
use lyrel_core::host::sqlite::{
    COLUMN_WINDOW, PAGE_ROWS, QUERY_SECONDS, SqliteCancel, SqliteColumns, SqliteObject,
    SqliteObjectKind, SqliteOpened, SqlitePage, SqliteQuery, VALUE_CHUNK_BYTES, is_stale_view,
};

use lyrel_ui_base::display::human_bytes as byte_count;
use lyrel_ui_base::host_ops::{HostOps, SharedHost};
use lyrel_ui_base::i18n::{L10nKey, t, t_fmt};

use super::sqlite_grid::{SqliteGrid, value_text};

/// The text the inspector's viewer and its chunk paging work on: TEXT comes
/// back as its (lossy for display only) UTF-8 view, a BLOB as an offset hex
/// dump, and scalar values as their exact `value_text` — the same string
/// the copy button puts on the clipboard.
fn detail_text(value: &SqliteValue) -> String {
    match value {
        SqliteValue::Null => t(L10nKey::EditorSqliteNull).to_string(),
        SqliteValue::Integer { .. } | SqliteValue::Real { .. } => value_text(value),
        SqliteValue::Text { preview, .. } => match std::str::from_utf8(preview) {
            Ok(text) => text.to_string(),
            // Replacement characters would present bytes the database does
            // not hold; the raw bytes stay readable as a hex dump instead.
            Err(_) => hex_dump(preview),
        },
        SqliteValue::Blob { preview, .. } => hex_dump(preview),
    }
}

/// An offset/hex/ASCII dump, one 16-byte row per line, for BLOB chunks.
fn hex_dump(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 4);
    for (row, chunk) in bytes.chunks(16).enumerate() {
        if row > 0 {
            out.push('\n');
        }
        out.push_str(&format!("{:08x}  ", row * 16));
        for (i, byte) in chunk.iter().enumerate() {
            if i == 8 {
                out.push(' ');
            }
            out.push_str(&format!("{byte:02x} "));
        }
        for i in chunk.len()..16 {
            if i == 8 {
                out.push(' ');
            }
            out.push_str("   ");
        }
        out.push_str(" |");
        for byte in chunk {
            out.push(if byte.is_ascii_graphic() || *byte == b' ' {
                *byte as char
            } else {
                '.'
            });
        }
        out.push('|');
    }
    out
}

/// Lowercase hex with no separators: the exact byte content a BLOB copy
/// writes to the clipboard, re-parseable as the same value.
fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(DIGITS[(byte >> 4) as usize] as char);
        out.push(DIGITS[(byte & 0x0f) as usize] as char);
    }
    out
}

gpui_kit::actions!(
    sqlite_document,
    [Refresh, InspectValue, FocusFilter, CloseValue]
);

/// The keyboard context of a database presentation. The document's chords
/// are bound only while it holds the keyboard, so they stay with a text
/// field, shell, or terminal otherwise.
pub const KEY_CONTEXT: &str = "SqliteDocument";

/// The filter settles this long after the last keystroke, as Glim's does.
const SEARCH_DEBOUNCE: Duration = Duration::from_millis(250);

/// The largest complete value the inspector assembles for one clipboard
/// copy; bigger values stay pageable but are never copied in part.
const VALUE_COPY_MAX: u64 = 8 * 1024 * 1024;

/// The open view's schema and the values its pages share.
struct ViewState {
    id: u64,
    objects: Vec<SqliteObject>,
    journal_mode: String,
}

/// What one presentation draws. It is a copy of the document's small metadata
/// state (not of the page rows, which live in the presentation's grid), so a
/// render never borrows the document entity while it also builds listeners.
#[derive(Clone, Default)]
pub struct UiState {
    view: Option<u64>,
    journal_mode: SharedString,
    objects: Vec<SqliteObject>,
    selected: Option<String>,
    columns: Option<SqliteColumns>,
    search: String,
    sort: Option<(u32, bool)>,
    page_number: u64,
    start_column: u32,
    more: bool,
    total: Option<u64>,
    count_failed: bool,
    error: Option<SharedString>,
    notice: Option<SharedString>,
    stale: bool,
    busy: bool,
    rows: usize,
    object_sql: Option<SharedString>,
    idle_millis: u64,
    max_age_millis: u64,
    epoch: u64,
    query_rev: u64,
}

impl UiState {
    fn columns_count(&self) -> usize {
        self.columns
            .as_ref()
            .map(|columns| columns.names.len())
            .unwrap_or(0)
    }

    fn has_columns(&self) -> bool {
        self.columns.is_some()
    }
}

/// A database open on its owning Host: the read view, its schema and the
/// latest page of the selected object.
pub struct SqliteDocument {
    host: SharedHost,
    path: PathBuf,
    view: Option<ViewState>,
    selected: Option<String>,
    columns: Option<SqliteColumns>,
    page: Option<SqlitePage>,
    /// The filter applied to the current query, and the text still debouncing.
    search: String,
    pending_search: Option<String>,
    /// `(schema column index, descending)`, echoed in the header arrow.
    sort: Option<(u32, bool)>,
    page_number: u64,
    start_column: u32,
    more: bool,
    total: Option<u64>,
    count_failed: bool,

    error: Option<SharedString>,
    notice: Option<SharedString>,
    stale: bool,
    /// The schema/table identity every in-flight request is checked against.
    epoch: u64,
    /// Bumped when the filter or sort changes the rows a page names: a value
    /// inspected before the bump is not the cell the user sees now.
    query_rev: u64,
    search_serial: u64,
    page_serial: u64,
    count_serial: u64,
    refresh_serial: u64,
    columns_busy: bool,
    page_busy: bool,
    refresh_busy: bool,
    started: bool,

    page_cancel: Option<SqliteCancel>,
    count_cancel: Option<SqliteCancel>,
    columns_cancel: Option<SqliteCancel>,
    refresh_cancel: Option<SqliteCancel>,

    idle_millis: u64,
    max_age_millis: u64,
    /// Bumped for every state change a presentation must notice.
    revision: u64,
    /// Bumped only when the grid's contents or loading state changed.
    grid_revision: u64,
}

/// An opened view on its way to the document. Dropping it without taking it
/// closes the view, so a landing that never reaches a live document — closed
/// tab, replaced request — cannot leave a read transaction behind.
pub struct OpenedView {
    host: SharedHost,
    opened: Option<SqliteOpened>,
}

impl OpenedView {
    pub fn new(host: SharedHost, opened: SqliteOpened) -> Self {
        Self {
            host,
            opened: Some(opened),
        }
    }

    fn take(mut self) -> SqliteOpened {
        self.opened.take().expect("an opened view is taken once")
    }
}

impl Drop for OpenedView {
    fn drop(&mut self) {
        if let Some(opened) = self.opened.take() {
            self.host.sqlite_close(opened.view);
        }
    }
}

impl SqliteDocument {
    pub fn new(
        host: SharedHost,
        path: PathBuf,
        opened: io::Result<SqliteOpened>,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut document = Self {
            host,
            path,
            view: None,
            selected: None,
            columns: None,
            page: None,
            search: String::new(),
            pending_search: None,
            sort: None,
            page_number: 0,
            start_column: 0,
            more: false,
            total: None,
            count_failed: false,
            error: None,
            notice: None,
            stale: false,
            epoch: 0,
            query_rev: 0,
            search_serial: 0,
            page_serial: 0,
            count_serial: 0,
            refresh_serial: 0,
            columns_busy: false,
            page_busy: false,
            refresh_busy: false,
            started: false,
            page_cancel: None,
            count_cancel: None,
            columns_cancel: None,
            refresh_cancel: None,
            idle_millis: 0,
            max_age_millis: 0,
            revision: 0,
            grid_revision: 0,
        };
        document.apply(opened, cx);
        document
    }

    fn touch(&mut self, cx: &mut Context<Self>) {
        self.revision = self.revision.wrapping_add(1);
        cx.notify();
    }

    fn touch_grid(&mut self, cx: &mut Context<Self>) {
        self.grid_revision = self.grid_revision.wrapping_add(1);
        self.touch(cx);
    }

    fn revision(&self) -> u64 {
        self.revision
    }

    fn grid_revision(&self) -> u64 {
        self.grid_revision
    }

    fn snapshot(&self) -> UiState {
        let object = self.object();
        let page = self.page.as_ref();
        UiState {
            view: self.view.as_ref().map(|view| view.id),
            journal_mode: self
                .view
                .as_ref()
                .map(|view| view.journal_mode.clone())
                .unwrap_or_default()
                .into(),
            objects: self
                .view
                .as_ref()
                .map(|view| view.objects.clone())
                .unwrap_or_default(),
            selected: self.selected.clone(),
            columns: self.columns.clone(),
            search: self.search.clone(),
            sort: self.sort,
            page_number: self.page_number,
            start_column: self.start_column,
            more: self.more,
            total: self.total,
            count_failed: self.count_failed,
            error: self.error.clone(),
            notice: self.notice.clone(),
            stale: self.stale,
            busy: self.is_busy(),
            rows: page.map_or(0, |page| page.rows.len()),
            object_sql: object.map(|object| SharedString::from(object.sql.clone())),
            idle_millis: self.idle_millis,
            max_age_millis: self.max_age_millis,
            epoch: self.epoch,
            query_rev: self.query_rev,
        }
    }

    fn apply(&mut self, opened: io::Result<SqliteOpened>, cx: &mut Context<Self>) {
        match opened {
            Ok(opened) => {
                self.idle_millis = opened.idle_millis;
                self.max_age_millis = opened.max_age_millis;
                self.selected = opened.objects.first().map(|object| object.name.clone());
                self.view = Some(ViewState {
                    id: opened.view,
                    objects: opened.objects,
                    journal_mode: opened.journal_mode,
                });
                self.columns = None;
                self.page = None;
                self.start_column = 0;
                self.more = false;
                self.total = None;
                self.count_failed = false;
                self.error = None;
                self.stale = false;
            }
            Err(error) => {
                self.view = None;
                self.selected = None;
                self.error =
                    Some(format!("{}: {error}", t(L10nKey::EditorSqliteOpenFailed)).into());
            }
        }
        self.columns_busy = false;
        self.page_busy = false;
        self.started = false;
        self.touch_grid(cx);
    }

    /// Select the first object and read its first page. Called once after the
    /// entity exists, when the first Window is available.
    pub fn start(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if self.started || self.view.is_none() {
            return;
        }
        self.started = true;
        if let Some(table) = self.selected.clone()
            && self.object_exists(&table)
        {
            self.load_columns(table, cx);
        }
    }

    /// Replace the document with a freshly opened view (external file change
    /// or an explicit refresh). The old view is closed first; the new one has
    /// a new identity, so a late page from the old view is rejected.
    pub fn replace(
        &mut self,
        opened: io::Result<SqliteOpened>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.cancel_queries();
        if let Some(view) = self.view.take() {
            self.host.sqlite_close(view.id);
        }
        let previous = self.selected.clone();
        self.apply(opened, cx);
        if self.view.is_some() {
            if let Some(previous) = previous
                && self.object_exists(&previous)
            {
                self.selected = Some(previous);
            }
            self.notice = self.view.as_ref().map(|view| {
                t_fmt(
                    L10nKey::EditorSqliteRefreshed,
                    &[("view", &view.id.to_string())],
                )
                .into()
            });
            self.start(window, cx);
        }
        self.touch(cx);
    }

    fn object_exists(&self, table: &str) -> bool {
        self.view
            .as_ref()
            .is_some_and(|view| view.objects.iter().any(|object| object.name == table))
    }

    fn object(&self) -> Option<&SqliteObject> {
        let name = self.selected.as_deref()?;
        self.view
            .as_ref()?
            .objects
            .iter()
            .find(|object| object.name == name)
    }

    fn cancel_queries(&mut self) {
        for token in [
            self.page_cancel.take(),
            self.count_cancel.take(),
            self.columns_cancel.take(),
            self.refresh_cancel.take(),
        ]
        .into_iter()
        .flatten()
        {
            token.cancel();
        }
    }

    fn select_object(&mut self, table: String, cx: &mut Context<Self>) {
        if self.refresh_busy || !self.object_exists(&table) {
            return;
        }
        if self.selected.as_deref() == Some(table.as_str()) && self.columns.is_some() {
            return;
        }
        self.selected = Some(table.clone());
        self.notice = None;
        // A sort names a column of the previous object's schema; the filter
        // text survives, matching the retained search input.
        if self.sort.is_some() {
            self.sort = None;
            self.query_rev = self.query_rev.wrapping_add(1);
        }
        self.load_columns(table, cx);
    }

    fn load_columns(&mut self, table: String, cx: &mut Context<Self>) {
        self.cancel_queries();
        self.epoch = self.epoch.wrapping_add(1);
        self.page_serial = self.page_serial.wrapping_add(1);
        self.count_serial = self.count_serial.wrapping_add(1);
        self.columns = None;
        self.page = None;
        self.page_number = 0;
        self.start_column = 0;
        self.more = false;
        self.total = None;
        self.count_failed = false;
        self.error = None;
        self.stale = false;
        self.columns_busy = true;
        self.touch_grid(cx);
        let Some(view) = self.view.as_ref().map(|view| view.id) else {
            self.columns_busy = false;
            return;
        };
        let epoch = self.epoch;
        let token = SqliteCancel::new();
        self.columns_cancel = Some(token.clone());
        let host = self.host.clone();
        let table_for_work = table.clone();
        HostOps::run(
            host,
            cx,
            move |h| h.sqlite_columns(view, &table_for_work, &token),
            move |document, result, cx| {
                document.on_columns(epoch, view, table, result, cx);
            },
        );
    }

    fn on_columns(
        &mut self,
        epoch: u64,
        view: u64,
        table: String,
        result: io::Result<SqliteColumns>,
        cx: &mut Context<Self>,
    ) {
        if self.epoch != epoch || self.view.as_ref().map(|view| view.id) != Some(view) {
            return;
        }
        self.columns_busy = false;
        let token_cancelled = self
            .columns_cancel
            .as_ref()
            .is_some_and(SqliteCancel::is_cancelled);
        self.columns_cancel = None;
        match result {
            Ok(columns) => {
                // A sort kept across a refresh can name a column this schema
                // no longer has.
                if let Some((index, _)) = self.sort
                    && index as usize >= columns.names.len()
                {
                    self.sort = None;
                    self.query_rev = self.query_rev.wrapping_add(1);
                }
                if columns.names.is_empty() {
                    self.columns = Some(columns);
                    self.touch_grid(cx);
                    return;
                }
                self.selected = Some(table);
                self.columns = Some(columns);
                self.touch_grid(cx);
                self.load_page(0, cx);
                self.load_count_inner(cx);
            }
            Err(error) => {
                if token_cancelled || error.kind() == io::ErrorKind::Interrupted {
                    self.notice = Some(t(L10nKey::EditorSqliteCancelled).into());
                } else if is_stale_view(&error) {
                    self.mark_stale(&error, cx);
                } else {
                    self.error = Some(error.to_string().into());
                }
                self.touch_grid(cx);
            }
        }
    }

    /// The query page requests and counts share: the selected object, its
    /// schema columns, the deterministic order, and the filter and sort the
    /// user asked for. Identifiers are validated and quoted on the Host;
    /// `search` stays a bound parameter, so this is never arbitrary SQL.
    fn query(&self, page: u64) -> Option<SqliteQuery> {
        let table = self.selected.clone()?;
        let columns = self.columns.as_ref()?;
        Some(SqliteQuery {
            table,
            columns: columns.names.clone(),
            order: columns.order.clone(),
            search: self.search.clone(),
            sort: self.sort,
            page,
            start_column: self.start_column,
            seconds: QUERY_SECONDS,
        })
    }

    /// The query as the value inspector issues it: the page the cell came
    /// from and the full column list, so a row/column pair names the same
    /// cell the page showed. This is `pub(super)` for [`SqliteView`]'s
    /// complete-value requests, which must send the same query shape.
    pub fn value_query(&self) -> Option<SqliteQuery> {
        self.query(self.page_number)
    }

    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    pub fn query_rev(&self) -> u64 {
        self.query_rev
    }

    pub fn view_id(&self) -> Option<u64> {
        self.view.as_ref().map(|view| view.id)
    }

    pub fn host(&self) -> SharedHost {
        self.host.clone()
    }

    pub fn mark_stale(&mut self, error: &io::Error, cx: &mut Context<Self>) {
        self.cancel_queries();
        self.stale = true;
        self.error = Some(format!("{}: {error}", t(L10nKey::EditorSqliteExpired)).into());
        self.columns = None;
        self.page = None;
        self.more = false;
        self.total = None;
        self.count_failed = false;
        self.columns_busy = false;
        self.page_busy = false;
        self.touch_grid(cx);
    }

    /// Record the filter text a presentation's input produced; the query
    /// itself goes out after [`SEARCH_DEBOUNCE`] of quiet, as Glim's does.
    pub fn search_changed(&mut self, text: String, cx: &mut Context<Self>) {
        if self.stale || self.view.is_none() {
            return;
        }
        self.pending_search = Some(text);
        self.search_serial = self.search_serial.wrapping_add(1);
        let serial = self.search_serial;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SEARCH_DEBOUNCE).await;
            let _ = this.update(cx, |document, cx| {
                if document.search_serial == serial {
                    document.apply_pending_search(cx);
                }
            });
        })
        .detach();
    }

    /// Apply the newest filter text now (Enter or the debounce firing); older
    /// keystrokes still waiting are folded into it.
    pub fn apply_pending_search(&mut self, cx: &mut Context<Self>) {
        let Some(search) = self.pending_search.take() else {
            return;
        };
        if search == self.search {
            return;
        }
        self.search = search;
        self.query_rev = self.query_rev.wrapping_add(1);
        self.page_number = 0;
        self.total = None;
        self.count_failed = false;
        self.notice = None;
        self.load_page(0, cx);
        self.load_count_inner(cx);
    }

    /// The three-state header sort: descending, ascending, then back to the
    /// deterministic order, naming a column of this table's schema.
    pub fn apply_sort(&mut self, sort: Option<(u32, bool)>, cx: &mut Context<Self>) {
        if self.stale || self.columns.is_none() || sort == self.sort {
            return;
        }
        if sort.is_some_and(|(index, _)| index as usize >= self.columns_count()) {
            return;
        }
        self.sort = sort;
        self.query_rev = self.query_rev.wrapping_add(1);
        self.page_number = 0;
        self.notice = None;
        self.load_page(0, cx);
    }

    fn columns_count(&self) -> usize {
        self.columns
            .as_ref()
            .map(|columns| columns.names.len())
            .unwrap_or(0)
    }

    fn load_page(&mut self, page: u64, cx: &mut Context<Self>) {
        let Some(query) = self.query(page) else {
            return;
        };
        let Some(view) = self.view.as_ref().map(|view| view.id) else {
            return;
        };
        if let Some(previous) = self.page_cancel.take() {
            previous.cancel();
        }
        self.page_serial = self.page_serial.wrapping_add(1);
        let serial = self.page_serial;
        let epoch = self.epoch;
        let token = SqliteCancel::new();
        self.page_cancel = Some(token.clone());
        self.page_busy = true;
        self.error = None;
        self.notice = None;
        self.touch_grid(cx);
        let host = self.host.clone();
        HostOps::run(
            host,
            cx,
            move |h| h.sqlite_page(view, &query, &token),
            move |document, result, cx| {
                document.on_page_inner(epoch, serial, view, result, cx);
            },
        );
    }

    fn on_page_inner(
        &mut self,
        epoch: u64,
        serial: u64,
        view: u64,
        result: io::Result<SqlitePage>,
        cx: &mut Context<Self>,
    ) {
        if self.epoch != epoch
            || self.page_serial != serial
            || self.view.as_ref().map(|view| view.id) != Some(view)
        {
            return;
        }
        self.page_busy = false;
        let token_cancelled = self
            .page_cancel
            .as_ref()
            .is_some_and(SqliteCancel::is_cancelled);
        self.page_cancel = None;
        match result {
            Ok(page) => {
                self.page_number = page.page;
                self.start_column = page.start_column;
                self.more = page.more;
                self.page = Some(page);
            }
            Err(error) => {
                if token_cancelled || error.kind() == io::ErrorKind::Interrupted {
                    self.notice = Some(t(L10nKey::EditorSqliteCancelled).into());
                } else if is_stale_view(&error) {
                    self.mark_stale(&error, cx);
                } else {
                    self.error = Some(error.to_string().into());
                }
            }
        }
        self.touch_grid(cx);
    }

    fn load_count_inner(&mut self, cx: &mut Context<Self>) {
        let Some(mut query) = self.query(self.page_number) else {
            return;
        };
        let Some(view) = self.view.as_ref().map(|view| view.id) else {
            return;
        };
        query.page = 0;
        if let Some(previous) = self.count_cancel.take() {
            previous.cancel();
        }
        self.count_serial = self.count_serial.wrapping_add(1);
        let serial = self.count_serial;
        let epoch = self.epoch;
        let token = SqliteCancel::new();
        self.count_cancel = Some(token.clone());
        let host = self.host.clone();
        HostOps::run(
            host,
            cx,
            move |h| h.sqlite_count(view, &query, &token),
            move |document, result, cx| {
                if document.epoch != epoch
                    || document.count_serial != serial
                    || document.view.as_ref().map(|view| view.id) != Some(view)
                {
                    return;
                }
                document.count_cancel = None;
                match result {
                    Ok(total) => {
                        document.total = Some(total);
                        document.count_failed = false;
                    }
                    Err(error) => {
                        // A count is an accessory: the page remains usable and
                        // the status line says the number is unavailable.
                        document.count_failed = true;
                        if document.notice.is_none() && error.kind() != io::ErrorKind::Interrupted {
                            document.notice = Some(SharedString::from(error.to_string()));
                        }
                    }
                }
                document.touch(cx);
            },
        );
    }
    fn navigate_inner(&mut self, page: u64, cx: &mut Context<Self>) {
        if self.is_busy() || self.stale || self.columns.is_none() {
            return;
        }
        if page == self.page_number && self.page.is_some() {
            return;
        }
        self.load_page(page, cx);
    }

    // Whole-window tests deliver a late page from an older query generation.
    #[cfg(any(test, feature = "test-support"))]
    pub fn page_serial(&self) -> u64 {
        self.page_serial
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn navigate(&mut self, page: u64, cx: &mut Context<Self>) {
        self.navigate_inner(page, cx);
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn load_count(&mut self, cx: &mut Context<Self>) {
        self.load_count_inner(cx);
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn on_page(
        &mut self,
        epoch: u64,
        serial: u64,
        view: u64,
        result: io::Result<SqlitePage>,
        cx: &mut Context<Self>,
    ) {
        self.on_page_inner(epoch, serial, view, result, cx);
    }

    fn shift_columns(&mut self, forward: bool, cx: &mut Context<Self>) {
        if self.is_busy() || self.stale {
            return;
        }
        let Some(columns) = self.columns.as_ref() else {
            return;
        };
        let length = columns.names.len() as u32;
        if length <= COLUMN_WINDOW {
            return;
        }
        let next = if forward {
            self.start_column
                .saturating_add(COLUMN_WINDOW)
                .min(length - 1)
        } else {
            self.start_column.saturating_sub(COLUMN_WINDOW)
        };
        if next == self.start_column {
            return;
        }
        self.start_column = next;
        self.load_page(self.page_number, cx);
    }

    pub fn refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.refresh_busy {
            return;
        }
        self.cancel_queries();
        if let Some(view) = self.view.take() {
            self.host.sqlite_close(view.id);
        }
        let previous = self.selected.clone();
        self.columns = None;
        self.page = None;
        self.page_number = 0;
        self.start_column = 0;
        self.more = false;
        self.total = None;
        self.count_failed = false;
        self.selected = None;
        self.error = None;
        self.notice = None;
        self.stale = false;
        self.started = false;
        self.refresh_busy = true;
        self.refresh_serial = self.refresh_serial.wrapping_add(1);
        let serial = self.refresh_serial;
        let token = SqliteCancel::new();
        self.refresh_cancel = Some(token.clone());
        self.touch_grid(cx);
        let host = self.host.clone();
        let path = self.path.clone();
        let guard_host = self.host.clone();
        let work_token = token.clone();
        HostOps::run_in(
            host,
            window,
            cx,
            move |h| {
                h.sqlite_open(&path, &work_token)
                    .map(|opened| OpenedView::new(guard_host, opened))
            },
            move |document, result, window, cx| {
                if document.refresh_serial != serial {
                    // The guard inside `result` closes a view nobody claims.
                    return;
                }
                document.refresh_busy = false;
                document.refresh_cancel = None;
                let token_cancelled = token.is_cancelled();
                match result {
                    Ok(guard) if !token_cancelled => {
                        let opened = guard.take();
                        let view = opened.view;
                        document.apply(Ok(opened), cx);
                        if let Some(previous) = previous
                            && document.object_exists(&previous)
                        {
                            document.selected = Some(previous);
                        }
                        document.notice = Some(
                            t_fmt(
                                L10nKey::EditorSqliteRefreshed,
                                &[("view", &view.to_string())],
                            )
                            .into(),
                        );
                        document.start(window, cx);
                    }
                    Ok(_guard) => {
                        document.notice = Some(t(L10nKey::EditorSqliteCancelled).into());
                    }
                    Err(error) if token_cancelled || error.kind() == io::ErrorKind::Interrupted => {
                        document.notice = Some(t(L10nKey::EditorSqliteCancelled).into());
                    }
                    Err(error) => {
                        document.error =
                            Some(format!("{}: {error}", t(L10nKey::EditorSqliteOpenFailed)).into());
                    }
                }
                document.touch_grid(cx);
            },
        );
    }

    fn cancel(&mut self, cx: &mut Context<Self>) {
        if !self.is_busy() {
            return;
        }
        self.cancel_queries();
        self.notice = Some(t(L10nKey::EditorSqliteCancelled).into());
        self.touch_grid(cx);
    }

    fn is_busy(&self) -> bool {
        self.columns_busy || self.page_busy || self.refresh_busy
    }
}

impl Drop for SqliteDocument {
    fn drop(&mut self) {
        self.cancel_queries();
        if let Some(view) = &self.view {
            self.host.sqlite_close(view.id);
        }
    }
}

/// One presentation of a SQLite document: its focus, filter input, table
/// scroll, selection and value inspector. The page data comes from the
/// shared document; the inspector belongs to this presentation alone.
pub struct SqliteView {
    document: Entity<SqliteDocument>,
    focus: FocusHandle,
    grid: Entity<TableState<SqliteGrid>>,
    search_input: Entity<InputState>,
    pub state: UiState,
    synced_revision: u64,
    synced_grid_revision: Option<u64>,
    page_marker: Option<(u64, u32)>,
    pub detail: Option<ValueDetail>,
    detail_serial: u64,
    value_cancel: Option<SqliteCancel>,
    _observe: Subscription,
    _grid: Subscription,
    _search: Subscription,
}

/// One complete cell being inspected in this presentation: the cell's
/// identity in the query that produced it, the current chunk and the exact
/// byte extent the panel navigates. Pub for the retained whole-window tests,
/// which read the inspector's identity and busy flag directly.
pub struct ValueDetail {
    pub row: u32,
    pub column: u32,
    pub name: SharedString,
    pub query: SqliteQuery,
    pub view: u64,
    pub epoch: u64,
    pub query_rev: u64,
    pub offset: u64,
    pub value: Option<SqliteValue>,
    pub busy: bool,
    pub error: Option<SharedString>,
    pub notice: Option<SharedString>,
    pub editor: Entity<TextareaState>,
}

impl SqliteView {
    pub fn new(
        document: Entity<SqliteDocument>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let grid = cx.new(|cx| {
            TableState::new(SqliteGrid::new(), window, cx)
                .cell_selectable(true)
                .row_header(false)
        });
        let owner = cx.entity().downgrade();
        grid.update(cx, |grid, _| {
            grid.delegate_mut().set_owner(owner.clone());
        });
        let search_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t(L10nKey::EditorSqliteFilterPlaceholder))
        });
        let state = document.read(cx).snapshot();
        let revision = document.read(cx).revision();
        let observe = cx.observe(&document, |view, _, cx| {
            view.sync(cx);
            cx.notify();
        });
        let _grid = cx.subscribe_in(&grid, window, |view, _, event, window, cx| match event {
            TableEvent::DoubleClickedCell(row, col) => {
                view.inspect_cell(*row, *col, window, cx);
            }
            _ => {}
        });
        let _search = cx.subscribe_in(
            &search_input,
            window,
            |view, _, event: &InputEvent, window, cx| match event {
                InputEvent::Change => {
                    let text = view.search_input.read(cx).value().to_string();
                    view.document.update(cx, |document, cx| {
                        document.search_changed(text, cx);
                    });
                }
                InputEvent::PressEnter { .. } => {
                    view.document.update(cx, |document, cx| {
                        document.apply_pending_search(cx);
                    });
                    let handle = view.grid.read(cx).focus_handle(cx);
                    window.focus(&handle, cx);
                }
                InputEvent::Focus | InputEvent::Blur => {}
            },
        );
        let mut view = Self {
            document,
            focus: cx.focus_handle(),
            grid,
            search_input,
            state,
            synced_revision: revision,
            synced_grid_revision: None,
            page_marker: None,
            detail: None,
            detail_serial: 0,
            value_cancel: None,
            _observe: observe,
            _grid,
            _search,
        };
        view.sync(cx);
        view
    }

    pub fn document(&self) -> &Entity<SqliteDocument> {
        &self.document
    }
    fn sync(&mut self, cx: &mut Context<SqliteView>) {
        let (revision, grid_revision) = {
            let document = self.document.read(cx);
            (document.revision(), document.grid_revision())
        };

        if revision != self.synced_revision {
            self.synced_revision = revision;
            self.state = self.document.read(cx).snapshot();
            // A detail names a cell inside one read view of one query; a
            // table switch, filter, sort or refresh that changed the data
            // under it closes the panel rather than show another cell's
            // bytes as this value.
            if let Some(detail) = self.detail.as_ref()
                && (Some(detail.view) != self.state.view
                    || detail.epoch != self.state.epoch
                    || detail.query_rev != self.state.query_rev)
            {
                self.close_detail(cx);
            }
        }
        if self.synced_grid_revision != Some(grid_revision) {
            self.synced_grid_revision = Some(grid_revision);
            let (columns, page, sort, busy) = {
                let document = self.document.read(cx);
                (
                    document.columns.clone(),
                    document.page.clone(),
                    document.sort,
                    document.is_busy(),
                )
            };
            let marker = page.as_ref().map(|page| (page.page, page.start_column));
            self.grid.update(cx, |grid, cx| {
                let delegate = grid.delegate_mut();
                match (&columns, &page) {
                    (Some(columns), Some(page)) => delegate.set_page(&columns.names, page, sort),
                    _ => delegate.clear(),
                }
                delegate.set_loading(busy);
                grid.refresh(cx);
            });
            if marker != self.page_marker && marker.is_some() {
                self.grid.update(cx, |grid, cx| grid.scroll_to_row(0, cx));
            }
            self.page_marker = marker;
        }
    }

    fn select_object(&mut self, table: String, cx: &mut Context<Self>) {
        self.close_detail(cx);
        let document = self.document.clone();
        document.update(cx, |document, cx| document.select_object(table, cx));
    }

    fn navigate(&mut self, page: u64, cx: &mut Context<Self>) {
        let document = self.document.clone();
        document.update(cx, |document, cx| document.navigate_inner(page, cx));
    }

    fn shift_columns(&mut self, forward: bool, cx: &mut Context<Self>) {
        let document = self.document.clone();
        document.update(cx, |document, cx| document.shift_columns(forward, cx));
    }

    fn refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close_detail(cx);
        let document = self.document.clone();
        document.update(cx, |document, cx| document.refresh(window, cx));
    }

    fn cancel(&mut self, cx: &mut Context<Self>) {
        self.close_detail(cx);
        let document = self.document.clone();
        document.update(cx, |document, cx| document.cancel(cx));
    }

    /// A header sort click: the grid already mapped its window column back
    /// to the schema column this query names.
    pub fn apply_sort(&mut self, sort: Option<(u32, bool)>, cx: &mut Context<Self>) {
        self.close_detail(cx);
        let document = self.document.clone();
        document.update(cx, |document, cx| document.apply_sort(sort, cx));
    }

    fn focus_filter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.search_input
            .update(cx, |input, cx| input.focus(window, cx));
    }

    /// Open the complete value of a real data cell: `row` inside this page,
    /// `col` inside the rendered column window. The query snapshot pins the
    /// row's identity — a row that moves while the fetch is in flight is not
    /// the cell the user asked for.
    fn inspect_cell(
        &mut self,
        row: usize,
        col: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(schema_col) = col
            .checked_sub(1)
            .map(|window_col| window_col + self.state.start_column as usize)
        else {
            return;
        };
        if self.state.busy || self.state.stale || row >= self.state.rows {
            return;
        }
        let (view, epoch, query_rev, query, name) = {
            let document = self.document.read(cx);
            let (Some(view), Some(epoch), Some(query_rev), Some(query)) = (
                document.view_id(),
                Some(document.epoch()),
                Some(document.query_rev()),
                document.value_query(),
            ) else {
                return;
            };
            let name = query
                .columns
                .get(schema_col)
                .cloned()
                .unwrap_or_default()
                .into();
            (view, epoch, query_rev, query, name)
        };
        if schema_col as u32 != schema_col as usize as u32
            || query.columns.get(schema_col).is_none()
        {
            return;
        }
        self.close_detail(cx);
        self.detail = Some(ValueDetail {
            row: row as u32,
            column: schema_col as u32,
            name,
            query,
            view,
            epoch,
            query_rev,
            offset: 0,
            value: None,
            busy: true,
            error: None,
            notice: None,
            editor: cx.new(|cx| TextareaState::new(window, cx)),
        });
        self.fetch_detail(window, cx);
        cx.notify();
    }

    /// The selected cell's complete value, from the keyboard. Only the grid's
    /// own focus counts, so Enter on a toolbar button or the filter input
    /// cannot open a cell the user was not pointing at.
    fn inspect_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (selected, focused) = self.grid.read_with(cx, |grid, cx| {
            (
                grid.selection(),
                grid.focus_handle(cx).contains_focused(window, cx),
            )
        });
        if !focused {
            cx.propagate();
            return;
        }
        if let TableSelection::Cell(row, col) = selected {
            self.inspect_cell(row, col, window, cx);
        } else {
            cx.propagate();
        }
    }

    /// Fetch the chunk the detail's `offset` points at. The request keeps the
    /// query snapshot and the view it was opened against; a landing whose
    /// serial or identity no longer matches is dropped, not shown.
    fn fetch_detail(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(detail) = self.detail.as_mut() else {
            return;
        };
        detail.busy = true;
        detail.error = None;
        detail.notice = None;
        let (view, query, row, column, offset) = (
            detail.view,
            detail.query.clone(),
            detail.row,
            detail.column,
            detail.offset,
        );
        if let Some(previous) = self.value_cancel.take() {
            previous.cancel();
        }
        self.detail_serial = self.detail_serial.wrapping_add(1);
        let serial = self.detail_serial;
        let token = SqliteCancel::new();
        self.value_cancel = Some(token.clone());
        let host = self.document.read(cx).host();
        cx.notify();
        HostOps::run_in(
            host,
            window,
            cx,
            move |h| h.sqlite_value(view, &query, row, column, offset, VALUE_CHUNK_BYTES, &token),
            move |view, result, window, cx| view.on_detail(serial, result, window, cx),
        );
    }

    fn on_detail(
        &mut self,
        serial: u64,
        result: io::Result<SqliteValue>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if serial != self.detail_serial || self.detail.is_none() {
            return;
        }
        self.value_cancel = None;
        match result {
            Ok(value) => {
                let Some(detail) = self.detail.as_mut() else {
                    return;
                };
                detail.busy = false;
                detail.value = Some(value.clone());
                detail.editor.update(cx, |editor, cx| {
                    editor.set_value(detail_text(&value), window, cx);
                });
            }
            Err(error) if is_stale_view(&error) => {
                self.detail = None;
                let document = self.document.clone();
                document.update(cx, |document, cx| document.mark_stale(&error, cx));
            }
            Err(error) => {
                if error.kind() == io::ErrorKind::Interrupted {
                    self.detail = None;
                } else if let Some(detail) = self.detail.as_mut() {
                    detail.busy = false;
                    detail.error = Some(error.to_string().into());
                }
            }
        }
        cx.notify();
    }

    /// Move the inspector one chunk through the value; the offset is a byte
    /// position inside the value, never a display row.
    fn detail_page(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(detail) = self.detail.as_mut() else {
            return;
        };
        if detail.busy {
            return;
        }
        let next = match detail.value.as_ref() {
            Some(SqliteValue::Text { preview, .. } | SqliteValue::Blob { preview, .. })
                if forward =>
            {
                detail.offset.saturating_add(preview.len() as u64)
            }
            Some(SqliteValue::Text { .. } | SqliteValue::Blob { .. }) => {
                detail.offset.saturating_sub(VALUE_CHUNK_BYTES)
            }
            _ => return,
        };
        if next == detail.offset {
            return;
        }
        detail.offset = next;
        self.fetch_detail(window, cx);
    }

    /// Copy the complete value to the clipboard, assembling the chunks the
    /// detail does not have yet — and never silently passing a bounded
    /// preview off as the value. Values past [`VALUE_COPY_MAX`] stay
    /// pageable but are not copied in part.
    fn copy_detail(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(detail) = self.detail.as_ref() else {
            return;
        };
        if detail.busy {
            return;
        }
        let Some(value) = detail.value.clone() else {
            return;
        };
        match value {
            SqliteValue::Null => {
                let detail = self.detail.as_mut().expect("detail checked above");
                detail.notice = Some(t(L10nKey::EditorSqliteNullNotice).into());
                cx.notify();
            }
            SqliteValue::Integer { .. } | SqliteValue::Real { .. } => {
                cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(value_text(&value)));
            }
            SqliteValue::Text { total, .. } | SqliteValue::Blob { total, .. }
                if total > VALUE_COPY_MAX =>
            {
                let detail = self.detail.as_mut().expect("detail checked above");
                detail.notice = Some(
                    t_fmt(
                        L10nKey::EditorSqliteCopyTooLarge,
                        &[
                            ("bytes", &byte_count(total)),
                            ("limit", &byte_count(VALUE_COPY_MAX)),
                        ],
                    )
                    .into(),
                );
                cx.notify();
            }
            SqliteValue::Text { .. } | SqliteValue::Blob { .. } => {
                let (view, query, row, column) =
                    (detail.view, detail.query.clone(), detail.row, detail.column);
                if let Some(previous) = self.value_cancel.take() {
                    previous.cancel();
                }
                self.detail_serial = self.detail_serial.wrapping_add(1);
                let serial = self.detail_serial;
                let token = SqliteCancel::new();
                self.value_cancel = Some(token.clone());
                let work_token = token.clone();
                if let Some(detail) = self.detail.as_mut() {
                    detail.busy = true;
                }
                cx.notify();
                let host = self.document.read(cx).host();
                HostOps::run_in(
                    host,
                    window,
                    cx,
                    move |h| {
                        let mut offset = 0_u64;
                        let mut bytes = Vec::new();
                        let mut text = true;
                        loop {
                            work_token.check()?;
                            let chunk = h.sqlite_value(
                                view,
                                &query,
                                row,
                                column,
                                offset,
                                VALUE_CHUNK_BYTES,
                                &work_token,
                            )?;
                            let (preview, total) = match chunk {
                                SqliteValue::Text { preview, total } => {
                                    bytes.extend_from_slice(&preview);
                                    (preview, total)
                                }
                                SqliteValue::Blob { preview, total } => {
                                    text = false;
                                    bytes.extend_from_slice(&preview);
                                    (preview, total)
                                }
                                // The cell's storage class cannot change
                                // inside one read view.
                                other => {
                                    return Err(io::Error::new(
                                        io::ErrorKind::InvalidData,
                                        format!(
                                            "the value changed storage class mid-copy: {other:?}"
                                        ),
                                    ));
                                }
                            };
                            if bytes.len() as u64 >= total || preview.is_empty() {
                                break;
                            }
                            offset = offset.saturating_add(preview.len() as u64);
                        }
                        Ok::<_, io::Error>((bytes, text))
                    },
                    move |view, result, _window, cx| {
                        if serial != view.detail_serial || view.detail.is_none() {
                            return;
                        }
                        view.value_cancel = None;
                        let Some(detail) = view.detail.as_mut() else {
                            return;
                        };
                        detail.busy = false;
                        match result {
                            Ok((bytes, is_text)) => {
                                let utf8 = is_text && std::str::from_utf8(&bytes).is_ok();
                                let text = if utf8 {
                                    String::from_utf8(bytes).unwrap_or_default()
                                } else {
                                    hex(&bytes)
                                };
                                detail.notice = if utf8 {
                                    Some(t(L10nKey::EditorSqliteCopied).into())
                                } else {
                                    Some(t(L10nKey::EditorSqliteCopiedHex).into())
                                };
                                cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(text));
                            }
                            Err(error) => {
                                detail.notice = Some(error.to_string().into());
                            }
                        }
                        cx.notify();
                    },
                );
            }
        }
    }

    fn close_detail(&mut self, cx: &mut Context<Self>) {
        if let Some(token) = self.value_cancel.take() {
            token.cancel();
        }
        self.detail_serial = self.detail_serial.wrapping_add(1);
        if self.detail.take().is_some() {
            cx.notify();
        }
    }

    fn last_page(&self) -> Option<u64> {
        self.state
            .total
            .map(|total| total.saturating_sub(1) / PAGE_ROWS)
    }

    /// The complete-value panel under the table. The viewer is read-only,
    /// the chunk buttons walk exact byte offsets and the byte extents label
    /// every partial view — nothing here is a preview wearing a full
    /// value's name.
    fn detail_panel(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let muted = cx.theme().muted_foreground;
        let danger = cx.theme().danger;
        let border = cx.theme().border;
        let Some(detail) = self.detail.as_ref() else {
            return v_flex().into_any_element();
        };
        let (kind, total, chunk_len, prev_enabled, next_enabled, invalid_text) = match detail
            .value
            .as_ref()
        {
            Some(SqliteValue::Null) => ("NULL".to_string(), None, 0, false, false, false),
            Some(SqliteValue::Integer { .. }) => {
                ("INTEGER".to_string(), None, 0, false, false, false)
            }
            Some(SqliteValue::Real { .. }) => ("REAL".to_string(), None, 0, false, false, false),
            Some(SqliteValue::Text { preview, total }) => (
                "TEXT".to_string(),
                Some(*total),
                preview.len() as u64,
                detail.offset > 0,
                detail.offset.saturating_add(preview.len() as u64) < *total,
                std::str::from_utf8(preview).is_err(),
            ),
            Some(SqliteValue::Blob { preview, total }) => (
                "BLOB".to_string(),
                Some(*total),
                preview.len() as u64,
                detail.offset > 0,
                detail.offset.saturating_add(preview.len() as u64) < *total,
                false,
            ),
            None => (String::new(), None, 0, false, false, false),
        };
        let mut extents = String::new();
        if let Some(total) = total {
            extents = t_fmt(
                L10nKey::EditorSqliteValueExtent,
                &[
                    ("from", &(detail.offset + 1).to_string()),
                    ("to", &(detail.offset + chunk_len).to_string()),
                    ("total", &byte_count(total)),
                ],
            )
            .to_string();
        }
        let busy = detail.busy;
        v_flex()
            .id("sqlite-detail")
            .test_support()
            .flex_none()
            .h(px(280.))
            .w_full()
            .border_t_1()
            .border_color(border)
            .child(
                h_flex()
                    .flex_none()
                    .w_full()
                    .items_center()
                    .gap_1()
                    .px_2()
                    .py_1()
                    .child(
                        div()
                            .id("sqlite-detail-title")
                            .test_support()
                            .flex_none()
                            .text_xs()
                            .child(t_fmt(
                                L10nKey::EditorSqliteValueTitle,
                                &[("name", &detail.name)],
                            )),
                    )
                    .when(!kind.is_empty(), |row| {
                        row.child(div().flex_none().text_xs().text_color(muted).child(kind))
                    })
                    .when(!extents.is_empty(), |row| {
                        row.child(
                            div()
                                .id("sqlite-detail-extent")
                                .test_support()
                                .flex_none()
                                .text_xs()
                                .text_color(muted)
                                .child(extents),
                        )
                    })
                    .child(div().flex_1())
                    .child(
                        Button::new("sqlite-value-prev")
                            .ghost()
                            .xsmall()
                            .flex_none()
                            .disabled(busy || !prev_enabled)
                            .icon(Icon::new(IconName::ChevronLeft))
                            .accessibility_label(t(L10nKey::EditorSqlitePreviousChunk))
                            .tooltip(t(L10nKey::EditorSqlitePreviousChunk))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.detail_page(false, window, cx)
                            })),
                    )
                    .child(
                        Button::new("sqlite-value-next")
                            .ghost()
                            .xsmall()
                            .flex_none()
                            .disabled(busy || !next_enabled)
                            .icon(Icon::new(IconName::ChevronRight))
                            .accessibility_label(t(L10nKey::EditorSqliteNextChunk))
                            .tooltip(t(L10nKey::EditorSqliteNextChunk))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.detail_page(true, window, cx)
                            })),
                    )
                    .child(
                        Button::new("sqlite-value-copy")
                            .ghost()
                            .xsmall()
                            .flex_none()
                            .disabled(busy || detail.value.is_none())
                            .label(t(L10nKey::EditorSqliteCopyValue))
                            .tooltip(t(L10nKey::EditorSqliteCopyValue))
                            .on_click(
                                cx.listener(|this, _, window, cx| this.copy_detail(window, cx)),
                            ),
                    )
                    .child(
                        Button::new("sqlite-value-close")
                            .ghost()
                            .xsmall()
                            .flex_none()
                            .icon(Icon::new(IconName::Close))
                            .accessibility_label(t(L10nKey::EditorSqliteCloseValue))
                            .tooltip(t(L10nKey::EditorSqliteCloseValue))
                            .on_click(cx.listener(|this, _, _, cx| this.close_detail(cx))),
                    ),
            )
            .when_some(detail.error.clone(), |column, error| {
                column.child(
                    div()
                        .id("sqlite-detail-error")
                        .test_support()
                        .flex_none()
                        .px_2()
                        .py_1()
                        .text_xs()
                        .text_color(danger)
                        .child(error),
                )
            })
            .when(invalid_text, |column| {
                column.child(
                    div()
                        .id("sqlite-detail-hex")
                        .test_support()
                        .flex_none()
                        .px_2()
                        .py_1()
                        .text_xs()
                        .text_color(muted)
                        .child(t(L10nKey::EditorSqliteTextNotUtf8)),
                )
            })
            .when_some(detail.notice.clone(), |column, notice| {
                column.child(
                    div()
                        .id("sqlite-detail-notice")
                        .test_support()
                        .flex_none()
                        .px_2()
                        .py_1()
                        .text_xs()
                        .text_color(muted)
                        .child(notice),
                )
            })
            .child(
                div().flex_1().min_h_0().child(
                    Textarea::new(&detail.editor)
                        .readonly(true)
                        .bordered(false)
                        .w_full()
                        .h_full(),
                ),
            )
            .when(busy, |column| {
                column.child(
                    div()
                        .flex_none()
                        .px_2()
                        .py_1()
                        .text_xs()
                        .text_color(muted)
                        .child(t(L10nKey::EditorSqliteLoading)),
                )
            })
            .into_any_element()
    }

    pub fn status_text(&self) -> SharedString {
        let mut parts = Vec::new();
        if let Some(view) = self.state.view {
            parts.push(t_fmt(
                L10nKey::EditorSqliteReadView,
                &[("view", &view.to_string())],
            ));
            if self.state.idle_millis > 0 {
                parts.push(t_fmt(
                    L10nKey::EditorSqliteViewLimits,
                    &[
                        ("idle", &(self.state.idle_millis / 1000).to_string()),
                        ("age", &(self.state.max_age_millis / 1000).to_string()),
                    ],
                ));
            }
            if let Some(table) = self.state.selected.as_deref() {
                parts.push(table.to_string());
            }
            parts.push(t_fmt(
                L10nKey::EditorSqlitePage,
                &[("page", &(self.state.page_number + 1).to_string())],
            ));
            if self.state.rows > 0 {
                let start = self
                    .state
                    .page_number
                    .saturating_mul(PAGE_ROWS)
                    .saturating_add(1);
                let end = start.saturating_add(self.state.rows as u64 - 1);
                parts.push(t_fmt(
                    L10nKey::EditorSqliteRows,
                    &[("from", &start.to_string()), ("to", &end.to_string())],
                ));
            }
            match self.state.total {
                Some(total) => parts.push(t_fmt(
                    L10nKey::EditorSqliteRowCount,
                    &[("n", &total.to_string())],
                )),
                None if self.state.count_failed => {
                    parts.push(t(L10nKey::EditorSqliteCountUnavailable).to_string())
                }
                None => {}
            }
            parts.push(t_fmt(
                L10nKey::EditorSqliteJournal,
                &[("mode", &self.state.journal_mode)],
            ));
            parts.push(t(L10nKey::EditorSqliteReadOnly).to_string());
            if !self.state.search.is_empty() {
                parts.push(t_fmt(
                    L10nKey::EditorSqliteFilterStatus,
                    &[("filter", &self.state.search)],
                ));
            }
            if let Some((column, descending)) = self.state.sort {
                let direction = if descending {
                    t(L10nKey::EditorSqliteSortDescending)
                } else {
                    t(L10nKey::EditorSqliteSortAscending)
                };
                let name = self
                    .state
                    .columns
                    .as_ref()
                    .and_then(|columns| columns.names.get(column as usize))
                    .cloned()
                    .unwrap_or_default();
                parts.push(t_fmt(
                    L10nKey::EditorSqliteSortStatus,
                    &[("column", &name), ("direction", direction)],
                ));
            }
        } else if self.state.error.is_some() {
            parts.push(t(L10nKey::EditorSqliteFailed).to_string());
        }
        parts.join(" · ").into()
    }

    /// What the Kit table currently draws, for the workbench regression.
    #[cfg(any(test, feature = "test-support"))]
    pub fn grid_dump(&self, cx: &App) -> (Vec<String>, Vec<Vec<String>>) {
        self.grid.read(cx).dump(cx)
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn view_id(&self) -> Option<u64> {
        self.state.view
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn error_text(&self) -> Option<&SharedString> {
        self.state.error.as_ref()
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn page_number(&self) -> u64 {
        self.state.page_number
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn selected_table(&self) -> Option<&str> {
        self.state.selected.as_deref()
    }

    /// The table widget entity: the retained whole-window tests scroll
    /// columns and read the selection through the real grid.
    #[cfg(any(test, feature = "test-support"))]
    pub fn grid(&self) -> &Entity<TableState<SqliteGrid>> {
        &self.grid
    }

    /// 复制元数据的只读测试接缝：整窗保留测试在过渡 settle 期间断言它们。
    #[cfg(any(test, feature = "test-support"))]
    pub fn is_busy(&self) -> bool {
        self.state.busy
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn object_count(&self) -> usize {
        self.state.objects.len()
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn search(&self) -> &str {
        &self.state.search
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn sort(&self) -> Option<(u32, bool)> {
        self.state.sort
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn total(&self) -> Option<u64> {
        self.state.total
    }

    fn toolbar(&mut self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let muted = cx.theme().muted_foreground;
        let busy = self.state.busy;
        let length = self.state.columns_count();
        let start = self.state.start_column as usize;
        let page = self.state.page_number;
        let last = self.last_page();
        let has_columns = self.state.has_columns();
        let page_disabled = busy || self.state.stale || !has_columns;
        let nav = |id: &'static str, label: &'static str, enabled: bool, back: bool| {
            let mut button = Button::new(id)
                .ghost()
                .xsmall()
                .flex_none()
                .disabled(!enabled || page_disabled)
                .accessibility_label(label)
                .tooltip(label);
            button = if back {
                button.icon(Icon::new(IconName::ChevronLeft))
            } else {
                button.icon(Icon::new(IconName::ChevronRight))
            };
            button
        };
        let kind = self.state.selected.as_deref().and_then(|name| {
            self.state
                .objects
                .iter()
                .find(|object| object.name == name)
                .map(|object| match object.kind {
                    SqliteObjectKind::Table => t(L10nKey::EditorSqliteKindTable),
                    SqliteObjectKind::View => t(L10nKey::EditorSqliteKindView),
                })
        });
        let name: SharedString = self.state.selected.clone().unwrap_or_default().into();
        h_flex()
            .id("sqlite-toolbar")
            .test_support()
            .flex_none()
            .w_full()
            .items_center()
            .gap_1()
            .px_2()
            .py_1()
            .border_b_1()
            .border_color(cx.theme().border)
            .when(!name.is_empty(), |row| {
                row.child(
                    div()
                        .flex_none()
                        .whitespace_nowrap()
                        .text_sm()
                        .child(name.clone()),
                )
                .when_some(kind, |row, kind| {
                    row.child(div().flex_none().text_xs().text_color(muted).child(kind))
                })
            })
            .child(div().flex_1())
            .child(
                div().flex_none().w(rems(14.)).child(
                    Input::new(&self.search_input)
                        .id("sqlite-filter")
                        .xsmall()
                        .appearance(true),
                ),
            )
            .child(div().flex_1())
            .when(length > COLUMN_WINDOW as usize, |row| {
                row.child(
                    nav(
                        "sqlite-prev-columns",
                        t(L10nKey::EditorSqlitePreviousColumns),
                        start > 0,
                        true,
                    )
                    .on_click(cx.listener(|this, _, _, cx| this.shift_columns(false, cx))),
                )
                .child(
                    div()
                        .id("sqlite-column-window")
                        .test_support()
                        .flex_none()
                        .text_xs()
                        .text_color(muted)
                        .child(format!(
                            "{}-{}",
                            start + 1,
                            (start + COLUMN_WINDOW as usize).min(length)
                        )),
                )
                .child(
                    nav(
                        "sqlite-next-columns",
                        t(L10nKey::EditorSqliteNextColumns),
                        start + (COLUMN_WINDOW as usize) < length,
                        false,
                    )
                    .on_click(cx.listener(|this, _, _, cx| this.shift_columns(true, cx))),
                )
            })
            .child(
                nav(
                    "sqlite-first-page",
                    t(L10nKey::EditorSqliteFirstPage),
                    page > 0,
                    true,
                )
                .on_click(cx.listener(|this, _, _, cx| this.navigate(0, cx))),
            )
            .child(
                nav(
                    "sqlite-prev-page",
                    t(L10nKey::EditorSqlitePreviousPage),
                    page > 0,
                    true,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.navigate(this.state.page_number.saturating_sub(1), cx)
                })),
            )
            .child(
                div()
                    .id("sqlite-page")
                    .test_support()
                    .flex_none()
                    .text_xs()
                    .text_color(muted)
                    .child(t_fmt(
                        L10nKey::EditorSqlitePage,
                        &[("page", &(page + 1).to_string())],
                    )),
            )
            .child(
                nav(
                    "sqlite-next-page",
                    t(L10nKey::EditorSqliteNextPage),
                    self.state.more,
                    false,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.navigate(this.state.page_number.saturating_add(1), cx)
                })),
            )
            .child(
                nav(
                    "sqlite-last-page",
                    t(L10nKey::EditorSqliteLastPage),
                    last.is_some_and(|last| last > page),
                    false,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    let last = this.last_page().unwrap_or(this.state.page_number);
                    this.navigate(last, cx)
                })),
            )
            .child(div().flex_1())
            .when(busy, |row| {
                row.child(
                    div()
                        .id("sqlite-loading")
                        .test_support()
                        .flex_none()
                        .text_xs()
                        .text_color(muted)
                        .child(t(L10nKey::EditorSqliteLoading)),
                )
                .child(
                    Button::new("sqlite-cancel")
                        .ghost()
                        .xsmall()
                        .flex_none()
                        .label(t(L10nKey::EditorSqliteCancel))
                        .on_click(cx.listener(|this, _, _, cx| this.cancel(cx))),
                )
            })
            .child(
                Button::new("sqlite-refresh")
                    .ghost()
                    .xsmall()
                    .flex_none()
                    .icon(Icon::new(IconName::RefreshCw))
                    .accessibility_label(t(L10nKey::EditorSqliteRefresh))
                    .tooltip(t(L10nKey::EditorSqliteRefresh))
                    .on_click(cx.listener(|this, _, window, cx| this.refresh(window, cx))),
            )
    }

    fn banners(&self, cx: &Context<Self>) -> impl IntoElement + use<> {
        let muted = cx.theme().muted_foreground;
        let danger = cx.theme().danger;
        let warning = cx.theme().warning;
        v_flex()
            .flex_none()
            .w_full()
            .when_some(self.state.error.clone(), |column, error| {
                column.child(
                    div()
                        .id("sqlite-error")
                        .test_support()
                        .flex_none()
                        .w_full()
                        .px_2()
                        .py_1()
                        .text_xs()
                        .text_color(danger)
                        .child(error),
                )
            })
            .when(self.state.stale, |column| {
                column.child(
                    div()
                        .id("sqlite-stale")
                        .test_support()
                        .flex_none()
                        .w_full()
                        .px_2()
                        .py_1()
                        .text_xs()
                        .text_color(warning)
                        .child(t(L10nKey::EditorSqliteExpiredHint)),
                )
            })
            .when_some(self.state.notice.clone(), |column, notice| {
                column.child(
                    div()
                        .id("sqlite-notice")
                        .test_support()
                        .flex_none()
                        .w_full()
                        .px_2()
                        .py_1()
                        .text_xs()
                        .text_color(muted)
                        .child(notice),
                )
            })
    }

    fn sidebar(&mut self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let muted = cx.theme().muted_foreground;
        let selected = self.state.selected.clone();
        let objects = self.state.objects.clone();
        let definition = self.state.object_sql.clone();
        let disabled = self.state.busy;
        v_flex()
            .id("sqlite-schema")
            .test_support()
            .flex_none()
            .w(rems(16.25))
            .h_full()
            .min_h_0()
            .border_r_1()
            .border_color(cx.theme().border)
            .child(
                div()
                    .flex_none()
                    .px_2()
                    .pt_2()
                    .pb_1()
                    .text_xs()
                    .text_color(muted)
                    .child(t(L10nKey::EditorSqliteSchema)),
            )
            .child(
                v_flex()
                    .id("sqlite-objects")
                    .flex_1()
                    .min_h_0()
                    .py_1()
                    .px_1()
                    .gap(px(1.))
                    .overflow_y_scroll()
                    .children(objects.into_iter().map(|object| {
                        let active = selected.as_deref() == Some(object.name.as_str());
                        let kind = match object.kind {
                            SqliteObjectKind::Table => t(L10nKey::EditorSqliteKindTable),
                            SqliteObjectKind::View => t(L10nKey::EditorSqliteKindView),
                        };
                        let name = object.name;
                        Button::new(SharedString::from(format!("sqlite-object:{name}")))
                            .ghost()
                            .xsmall()
                            .flex_none()
                            .w_full()
                            .toggled(active)
                            .disabled(disabled)
                            .tooltip(SharedString::from(format!("{name} · {kind}")))
                            .on_click(cx.listener({
                                let name = name.clone();
                                move |this, _, _, cx| this.select_object(name.clone(), cx)
                            }))
                            .child(
                                h_flex()
                                    .w_full()
                                    .min_w_0()
                                    .items_center()
                                    .gap_1()
                                    .child(div().truncate().child(name))
                                    .child(
                                        div().flex_none().text_xs().text_color(muted).child(kind),
                                    ),
                            )
                            .into_any_element()
                    })),
            )
            .child(
                v_flex()
                    .id("sqlite-definition")
                    .flex_1()
                    .min_h_0()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .child(
                        div()
                            .flex_none()
                            .px_2()
                            .pt_1()
                            .pb_1()
                            .text_xs()
                            .text_color(muted)
                            .child(t(L10nKey::EditorSqliteDefinition)),
                    )
                    .child(
                        div()
                            .id("sqlite-object-sql")
                            .test_support()
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .px_2()
                            .pb_2()
                            .font_family(cx.theme().mono_font_family.clone())
                            .text_size(rems(0.75))
                            .text_color(cx.theme().foreground)
                            .children(definition.map(|definition| {
                                v_flex().children(
                                    definition
                                        .lines()
                                        .map(|line| {
                                            div().child(SharedString::from(line.to_string()))
                                        })
                                        .collect::<Vec<_>>(),
                                )
                            })),
                    ),
            )
    }

    fn body(&mut self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let muted = cx.theme().muted_foreground;
        let columns_empty = self.state.columns_count() == 0;
        let no_rows = self.state.rows == 0 && !self.state.busy && self.state.has_columns();
        let label = self
            .state
            .selected
            .clone()
            .unwrap_or_else(|| "SQLite".to_string());
        v_flex()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .child(self.banners(cx))
            .when(no_rows, |column| {
                column.child(
                    div()
                        .id("sqlite-no-rows")
                        .test_support()
                        .flex_none()
                        .px_2()
                        .py_1()
                        .text_xs()
                        .text_color(muted)
                        .child(t(L10nKey::EditorSqliteNoRows)),
                )
            })
            .when(
                columns_empty && !self.state.busy && self.state.view.is_some(),
                |column| {
                    column.child(
                        div()
                            .id("sqlite-no-columns")
                            .test_support()
                            .flex_none()
                            .px_2()
                            .py_1()
                            .text_xs()
                            .text_color(muted)
                            .child(t(L10nKey::EditorSqliteNoColumns)),
                    )
                },
            )
            .child(
                div()
                    .id("sqlite-table")
                    .test_support()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .child(
                        DataTable::new(&self.grid)
                            .scrollbar_mode(ScrollbarMode::Always)
                            .bordered(false)
                            .stripe(false)
                            .accessibility_label(label),
                    ),
            )
            .child(self.detail_panel(cx))
    }

    fn failed(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let muted = cx.theme().muted_foreground;
        v_flex()
            .id("sqlite-failed")
            .test_support()
            .size_full()
            .items_center()
            .justify_center()
            .gap_2()
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().foreground)
                    .child(t(L10nKey::EditorSqliteOpenFailed)),
            )
            .when_some(self.state.error.clone(), |column, error| {
                column.child(
                    div()
                        .id("sqlite-error")
                        .test_support()
                        .max_w(rems(35.))
                        .text_xs()
                        .text_color(muted)
                        .child(error),
                )
            })
            .child(
                Button::new("sqlite-retry")
                    .ghost()
                    .xsmall()
                    .label(t(L10nKey::EditorSqliteRefresh))
                    .on_click(cx.listener(|this, _, window, cx| this.refresh(window, cx))),
            )
    }

    fn empty(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let muted = cx.theme().muted_foreground;
        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .gap_2()
            .child(
                div()
                    .id("sqlite-no-objects")
                    .test_support()
                    .text_sm()
                    .text_color(muted)
                    .child(t(L10nKey::EditorSqliteNoObjects)),
            )
            .child(
                Button::new("sqlite-empty-refresh")
                    .ghost()
                    .xsmall()
                    .label(t(L10nKey::EditorSqliteRefresh))
                    .on_click(cx.listener(|this, _, window, cx| this.refresh(window, cx))),
            )
    }

    fn loading_state(&self, cx: &Context<Self>) -> impl IntoElement + use<> {
        let muted = cx.theme().muted_foreground;
        v_flex().size_full().items_center().justify_center().child(
            div()
                .id("sqlite-loading-view")
                .test_support()
                .text_sm()
                .text_color(muted)
                .child(t(L10nKey::EditorSqliteLoading)),
        )
    }
}

impl Focusable for SqliteView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for SqliteView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let content = if self.state.view.is_none() {
            if self.state.busy {
                self.loading_state(cx).into_any_element()
            } else {
                self.failed(cx).into_any_element()
            }
        } else if self.state.objects.is_empty() {
            self.empty(cx).into_any_element()
        } else {
            let toolbar = self.toolbar(cx);
            let sidebar = self.sidebar(cx);
            let body = self.body(cx);
            v_flex()
                .size_full()
                .min_h_0()
                .child(toolbar)
                .child(
                    h_flex()
                        .items_stretch()
                        .flex_1()
                        .min_h_0()
                        .w_full()
                        .child(sidebar)
                        .child(body),
                )
                .into_any_element()
        };
        let focused = self.focus.contains_focused(window, cx);
        v_flex()
            .id("sqlite-document")
            .test_support()
            .size_full()
            .min_h_0()
            .border_1()
            .border_color(if focused {
                cx.theme().primary
            } else {
                gpui_kit::transparent_black()
            })
            .track_focus(&self.focus)
            .key_context(KEY_CONTEXT)
            .on_action(cx.listener(|this, _: &Refresh, window, cx| this.refresh(window, cx)))
            .on_action(
                cx.listener(|this, _: &FocusFilter, window, cx| this.focus_filter(window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &InspectValue, window, cx| {
                    this.inspect_selection(window, cx)
                }),
            )
            .on_action(cx.listener(|this, _: &CloseValue, _, cx| {
                if this.detail.is_some() {
                    this.close_detail(cx);
                    return;
                }
                // With no inspector open, Escape keeps the table's own
                // meaning: clear its cell selection, then let it bubble.
                let grid = this.grid.clone();
                let had_selection = grid.update(cx, |grid, cx| {
                    let had = grid.selection() != TableSelection::None;
                    if had {
                        grid.clear_selection(cx);
                    }
                    had
                });
                if !had_selection {
                    cx.propagate();
                }
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    if !this.focus.contains_focused(window, cx) {
                        this.focus.focus(window, cx);
                    }
                }),
            )
            .child(content)
    }
}

/// The context menu of a database document: what its data can do, nothing a
/// text editor would offer on a document with no text.
pub fn menu_entries(
    menu: gpui_kit::component::menu::PopupMenu,
) -> gpui_kit::component::menu::PopupMenu {
    menu.separator()
        .menu(t(L10nKey::EditorSqliteRefresh), Box::new(Refresh))
        .menu(
            t(L10nKey::EditorSqliteFilterPlaceholder),
            Box::new(FocusFilter),
        )
        .menu(t(L10nKey::EditorSqliteCopyValue), Box::new(InspectValue))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A TEXT value that is not valid UTF-8 must not be shown through
    /// replacement characters: the viewer keeps the bytes visible as hex,
    /// and the panel says why.
    #[test]
    fn invalid_utf8_text_is_shown_as_its_bytes() {
        let value = SqliteValue::Text {
            preview: b"ok\xffA".to_vec(),
            total: 4,
        };
        let text = detail_text(&value);
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
}
