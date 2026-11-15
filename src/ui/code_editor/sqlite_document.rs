//! A read-only SQLite database as a workbench document.
//!
//! Ported from Glim `glim-app/src/views/reader/sqlite.rs` (MIT): the schema
//! list, table selection, bounded page navigation, column windows, the
//! loading/error/retry states and the grid. Replaced: the backend is the
//! owning Host's read view — not a local snapshot copy — every page names the
//! view it came from, and refresh opens a new view instead of silently
//! switching snapshot under the reader.
//!
//! The document owns the read view and the data read from it. Each split
//! group gets its own presentation ([`SqliteView`]) with its own focus,
//! table scroll and selection, so two views of one database do not move each
//! other.
//!
//! #295 draws schema, definitions and basic paging. Search, sorting and the
//! complete-value inspector are #296: no control here promises them.

use std::io;
use std::path::PathBuf;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::table::{DataTable, TableState};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, h_flex, v_flex,
};
use gpui_kit::prelude::*;
use gpui_kit::{
    App, Context, Entity, FocusHandle, Focusable, MouseButton, SharedString, Subscription,
    TestSupportExt as _, Window, div, px, rems,
};

use lyrel_core::host::sqlite::{
    COLUMN_WINDOW, PAGE_ROWS, QUERY_SECONDS, SqliteCancel, SqliteColumns, SqliteObject,
    SqliteObjectKind, SqliteOpened, SqlitePage, SqliteQuery, is_stale_view,
};

use crate::ui::host_ops::{HostOps, SharedHost};
use crate::ui::i18n::{L10nKey, t, t_fmt};

use super::sqlite_grid::SqliteGrid;

gpui_kit::actions!(sqlite_document, [Refresh]);

/// The keyboard context of a database presentation. Refresh is bound to
/// `secondary-r` only while this document holds the keyboard, so the chord
/// stays with a text field, shell, or terminal otherwise.
pub(crate) const KEY_CONTEXT: &str = "SqliteDocument";

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
struct UiState {
    view: Option<u64>,
    journal_mode: SharedString,
    objects: Vec<SqliteObject>,
    selected: Option<String>,
    columns: Option<SqliteColumns>,
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
pub(crate) struct SqliteDocument {
    host: SharedHost,
    path: PathBuf,
    view: Option<ViewState>,
    selected: Option<String>,
    columns: Option<SqliteColumns>,
    page: Option<SqlitePage>,
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
pub(super) struct OpenedView {
    host: SharedHost,
    opened: Option<SqliteOpened>,
}

impl OpenedView {
    pub(super) fn new(host: SharedHost, opened: SqliteOpened) -> Self {
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
    pub(super) fn new(
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
            page_number: 0,
            start_column: 0,
            more: false,
            total: None,
            count_failed: false,
            error: None,
            notice: None,
            stale: false,
            epoch: 0,
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
                self.page_number = 0;
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
    pub(super) fn start(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.started || self.view.is_none() {
            return;
        }
        self.started = true;
        if let Some(table) = self.selected.clone()
            && self.object_exists(&table)
        {
            self.load_columns(table, window, cx);
        }
    }

    /// Replace the document with a freshly opened view (external file change
    /// or an explicit refresh). The old view is closed first; the new one has
    /// a new identity, so a late page from the old view is rejected.
    pub(super) fn replace(
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

    fn select_object(&mut self, table: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.refresh_busy || !self.object_exists(&table) {
            return;
        }
        if self.selected.as_deref() == Some(table.as_str()) && self.columns.is_some() {
            return;
        }
        self.selected = Some(table.clone());
        self.notice = None;
        self.load_columns(table, window, cx);
    }

    fn load_columns(&mut self, table: String, window: &mut Window, cx: &mut Context<Self>) {
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
        HostOps::run_in(
            host,
            window,
            cx,
            move |h| h.sqlite_columns(view, &table_for_work, &token),
            move |document, result, window, cx| {
                document.on_columns(epoch, view, table, result, window, cx);
            },
        );
    }

    fn on_columns(
        &mut self,
        epoch: u64,
        view: u64,
        table: String,
        result: io::Result<SqliteColumns>,
        window: &mut Window,
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
                if columns.names.is_empty() {
                    self.columns = Some(columns);
                    self.touch_grid(cx);
                    return;
                }
                self.selected = Some(table);
                self.columns = Some(columns);
                self.touch_grid(cx);
                self.load_page(0, window, cx);
                self.load_count(window, cx);
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

    fn query(&self, page: u64) -> Option<SqliteQuery> {
        let table = self.selected.clone()?;
        let columns = self.columns.as_ref()?;
        Some(SqliteQuery {
            table,
            columns: columns.names.clone(),
            order: columns.order.clone(),
            search: String::new(),
            sort: None,
            page,
            start_column: self.start_column,
            seconds: QUERY_SECONDS,
        })
    }

    fn load_page(&mut self, page: u64, window: &mut Window, cx: &mut Context<Self>) {
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
        HostOps::run_in(
            host,
            window,
            cx,
            move |h| h.sqlite_page(view, &query, &token),
            move |document, result, _window, cx| {
                document.on_page(epoch, serial, view, result, cx);
            },
        );
    }

    fn on_page(
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

    fn load_count(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
        HostOps::run_in(
            host,
            window,
            cx,
            move |h| h.sqlite_count(view, &query, &token),
            move |document, result, _window, cx| {
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

    fn mark_stale(&mut self, error: &io::Error, cx: &mut Context<Self>) {
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

    fn navigate(&mut self, page: u64, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_busy() || self.stale || self.columns.is_none() {
            return;
        }
        if page == self.page_number && self.page.is_some() {
            return;
        }
        self.load_page(page, window, cx);
    }

    fn shift_columns(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
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
        self.load_page(self.page_number, window, cx);
    }

    pub(super) fn refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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

/// One presentation of a SQLite document: its focus, table scroll, selection
/// and column window. The page data comes from the shared document.
pub(crate) struct SqliteView {
    document: Entity<SqliteDocument>,
    focus: FocusHandle,
    grid: Entity<TableState<SqliteGrid>>,
    state: UiState,
    synced_revision: u64,
    synced_grid_revision: Option<u64>,
    page_marker: Option<(u64, u32)>,
    _observe: Subscription,
}

impl SqliteView {
    pub(super) fn new(
        document: Entity<SqliteDocument>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let grid = cx.new(|cx| TableState::new(SqliteGrid::new(), window, cx));
        let state = document.read(cx).snapshot();
        let revision = document.read(cx).revision();
        let observe = cx.observe(&document, |view, _, cx| {
            view.sync(cx);
            cx.notify();
        });
        let mut view = Self {
            document,
            focus: cx.focus_handle(),
            grid,
            state,
            synced_revision: revision,
            synced_grid_revision: None,
            page_marker: None,
            _observe: observe,
        };
        view.sync(cx);
        view
    }

    pub(super) fn document(&self) -> &Entity<SqliteDocument> {
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
        }
        if self.synced_grid_revision != Some(grid_revision) {
            self.synced_grid_revision = Some(grid_revision);
            let (columns, page, busy) = {
                let document = self.document.read(cx);
                (
                    document.columns.clone(),
                    document.page.clone(),
                    document.is_busy(),
                )
            };
            let marker = page.as_ref().map(|page| (page.page, page.start_column));
            self.grid.update(cx, |grid, cx| {
                let delegate = grid.delegate_mut();
                match (&columns, &page) {
                    (Some(columns), Some(page)) => delegate.set_page(&columns.names, page),
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

    fn select_object(&mut self, table: String, window: &mut Window, cx: &mut Context<Self>) {
        let document = self.document.clone();
        document.update(cx, |document, cx| document.select_object(table, window, cx));
    }

    fn navigate(&mut self, page: u64, window: &mut Window, cx: &mut Context<Self>) {
        let document = self.document.clone();
        document.update(cx, |document, cx| document.navigate(page, window, cx));
    }

    fn shift_columns(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        let document = self.document.clone();
        document.update(cx, |document, cx| {
            document.shift_columns(forward, window, cx)
        });
    }

    fn refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let document = self.document.clone();
        document.update(cx, |document, cx| document.refresh(window, cx));
    }

    fn cancel(&mut self, cx: &mut Context<Self>) {
        let document = self.document.clone();
        document.update(cx, |document, cx| document.cancel(cx));
    }

    fn last_page(&self) -> Option<u64> {
        self.state
            .total
            .map(|total| total.saturating_sub(1) / PAGE_ROWS)
    }

    pub(super) fn status_text(&self) -> SharedString {
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
        } else if self.state.error.is_some() {
            parts.push(t(L10nKey::EditorSqliteFailed).to_string());
        }
        parts.join(" · ").into()
    }

    /// What the Kit table currently draws, for the workbench regression.
    #[cfg(test)]
    pub(super) fn grid_dump(&self, cx: &App) -> (Vec<String>, Vec<Vec<String>>) {
        self.grid.read(cx).dump(cx)
    }

    #[cfg(test)]
    pub(super) fn view_id(&self) -> Option<u64> {
        self.state.view
    }

    #[cfg(test)]
    pub(super) fn error_text(&self) -> Option<&SharedString> {
        self.state.error.as_ref()
    }

    #[cfg(test)]
    pub(super) fn page_number(&self) -> u64 {
        self.state.page_number
    }

    #[cfg(test)]
    pub(super) fn selected_table(&self) -> Option<&str> {
        self.state.selected.as_deref()
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
            .when(length > COLUMN_WINDOW as usize, |row| {
                row.child(
                    nav(
                        "sqlite-prev-columns",
                        t(L10nKey::EditorSqlitePreviousColumns),
                        start > 0,
                        true,
                    )
                    .on_click(
                        cx.listener(|this, _, window, cx| this.shift_columns(false, window, cx)),
                    ),
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
                    .on_click(
                        cx.listener(|this, _, window, cx| this.shift_columns(true, window, cx)),
                    ),
                )
            })
            .child(
                nav(
                    "sqlite-first-page",
                    t(L10nKey::EditorSqliteFirstPage),
                    page > 0,
                    true,
                )
                .on_click(cx.listener(|this, _, window, cx| this.navigate(0, window, cx))),
            )
            .child(
                nav(
                    "sqlite-prev-page",
                    t(L10nKey::EditorSqlitePreviousPage),
                    page > 0,
                    true,
                )
                .on_click(cx.listener(|this, _, window, cx| {
                    this.navigate(this.state.page_number.saturating_sub(1), window, cx)
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
                .on_click(cx.listener(|this, _, window, cx| {
                    this.navigate(this.state.page_number.saturating_add(1), window, cx)
                })),
            )
            .child(
                nav(
                    "sqlite-last-page",
                    t(L10nKey::EditorSqliteLastPage),
                    last.is_some_and(|last| last > page),
                    false,
                )
                .on_click(cx.listener(|this, _, window, cx| {
                    let last = this.last_page().unwrap_or(this.state.page_number);
                    this.navigate(last, window, cx)
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
                                move |this, _, window, cx| {
                                    this.select_object(name.clone(), window, cx)
                                }
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
                            .bordered(false)
                            .stripe(false)
                            .accessibility_label(label),
                    ),
            )
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
pub(super) fn menu_entries(
    menu: gpui_kit::component::menu::PopupMenu,
) -> gpui_kit::component::menu::PopupMenu {
    menu.separator()
        .menu(t(L10nKey::EditorSqliteRefresh), Box::new(Refresh))
}

#[cfg(test)]
#[path = "sqlite_document/tests.rs"]
mod tests;
