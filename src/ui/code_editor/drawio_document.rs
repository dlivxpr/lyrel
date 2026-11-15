//! Editable draw.io document: Host-owned bytes, engine-owned rendering.
//!
//! The workbench owns identity, acquisition, dirty state and lifecycle. The
//! fixed engine renders and edits the diagram inside its isolated WebView and
//! never learns a path; every committed edit returns over the session as the
//! document's newest bytes, so save, save-as, conflict and unsaved-close all
//! read the same authority as a text buffer. Page selection, zoom and focus
//! belong to a presentation; the bytes belong to the document, shared by
//! every split.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, h_flex, v_flex,
};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, App, Bounds, Context, Entity, FocusHandle, Focusable, Pixels, Render, SharedString,
    Subscription, Task, TestSupportExt as _, Window, div, rems,
};
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::drawio_engine::assets::MAX_DOCUMENT_BYTES;
use super::drawio_engine::bridge::Command;
use super::drawio_engine::document::{DrawioError, DrawioSource};
#[cfg(not(test))]
use super::drawio_engine::session::PreviewTheme;
#[cfg(not(test))]
use super::drawio_engine::surface::{ParentWindow, SurfaceError};
use super::drawio_engine::surface::{SurfaceEvent, SurfaceHost};
use crate::ui::i18n::{L10nKey, t, t_fmt};

gpui_kit::actions!(
    drawio_document,
    [
        DrawioNextPage,
        DrawioPreviousPage,
        DrawioZoomIn,
        DrawioZoomOut,
        DrawioFit,
        DrawioActualSize,
        DrawioUndo,
        DrawioRedo,
        DrawioCopy,
        DrawioCut,
        DrawioPaste,
        DrawioDelete,
    ]
);

/// Only true while this document owns the keyboard, so the terminal keeps its
/// own page keys when focus is elsewhere.
pub(crate) const KEY_CONTEXT: &str = "DrawioDocument";

const VISIBILITY_POLL: Duration = Duration::from_millis(500);
/// A native child window does not inherit GPUI's clip. If the presentation
/// stops being painted (tab switch, close, overlay), hide the WebView after
/// this long instead of leaving it over unrelated content.
const VISIBLE_GRACE: Duration = Duration::from_millis(1500);

#[derive(Clone)]
enum DrawioContent {
    Ready {
        bytes: Arc<[u8]>,
        source: DrawioSource,
    },
    Failed(SharedString),
}

/// One draw.io file's authoritative bytes, page list and edit history.
///
/// The document owns three clocks the presentations read differently.
/// `revision` counts byte replacements — an open or an external reload — and
/// every presentation that served an older revision reloads its engine.
/// `edit` counts content-changing commits on top of the current bytes; a
/// sibling's commit bumps it so every stale surface reloads to the newer
/// document instead of overwriting it. `committed` counts every accepted
/// engine commit including byte-identical sync flushes, so a deferred save
/// can wait for a confirmed revision.
///
/// Undo and redo are document state, not engine state: every presentation
/// shares this one history, so an edit made in one split is undone through
/// the same stack in any other. Engine-internal undo is an adapter detail and
/// never crosses the bridge.
pub(crate) struct DrawioDocument {
    content: DrawioContent,
    reloading: bool,
    revision: u64,
    edit: u64,
    committed: u64,
    fingerprint: super::Fingerprint,
    past: std::collections::VecDeque<DrawioContent>,
    future: Vec<DrawioContent>,
    history_bytes: usize,
}

/// Undo depth caps: enough to keep long sessions, bounded so a run of
/// multi-megabyte diagrams cannot pin unbounded memory.
const MAX_HISTORY: usize = 128;
const MAX_HISTORY_BYTES: usize = 64 * 1024 * 1024;

impl DrawioDocument {
    pub(super) fn new(
        bytes: Arc<[u8]>,
        parsed: Result<DrawioSource, DrawioError>,
        _cx: &mut Context<Self>,
    ) -> Self {
        let fingerprint = fingerprint(&bytes);
        Self {
            content: match parsed {
                Ok(source) => DrawioContent::Ready { bytes, source },
                Err(error) => DrawioContent::Failed(SharedString::from(error.to_string())),
            },
            reloading: false,
            revision: 0,
            edit: 0,
            committed: 0,
            fingerprint,
            past: Default::default(),
            future: Vec::new(),
            history_bytes: 0,
        }
    }

    pub(super) fn replace(
        &mut self,
        bytes: Arc<[u8]>,
        parsed: Result<DrawioSource, DrawioError>,
        cx: &mut Context<Self>,
    ) {
        self.revision = self.revision.wrapping_add(1);
        self.edit = self.edit.wrapping_add(1);
        self.committed = self.committed.wrapping_add(1);
        self.fingerprint = fingerprint(&bytes);
        self.content = match parsed {
            Ok(source) => DrawioContent::Ready { bytes, source },
            Err(error) => DrawioContent::Failed(SharedString::from(error.to_string())),
        };
        // Fresh bytes are a new lineage: the old history must not undo back
        // into a document the disk no longer agrees with.
        self.past.clear();
        self.future.clear();
        self.history_bytes = 0;
        self.reloading = false;
        cx.notify();
    }

    /// The bytes one live engine just committed become the document. A body
    /// that no longer parses is refused — the last good bytes stay the
    /// authority rather than being overwritten by a truncated serialization.
    /// A byte-identical commit (a save sync flush) advances `committed` but
    /// leaves content, history and dirty state untouched. The revision does
    /// not move, so the committing presentation keeps its engine session
    /// while every sibling reloads to the newer bytes.
    pub(crate) fn commit(
        &mut self,
        bytes: Arc<[u8]>,
        cx: &mut Context<Self>,
    ) -> Result<(), DrawioError> {
        if bytes.is_empty() || bytes.len() > MAX_DOCUMENT_BYTES {
            return Err(DrawioError::TooLarge);
        }
        let fingerprint = fingerprint(&bytes);
        if fingerprint == self.fingerprint {
            self.committed = self.committed.wrapping_add(1);
            cx.notify();
            return Ok(());
        }
        let source = super::drawio_engine::parse(&bytes)?;
        self.push_past();
        self.edit = self.edit.wrapping_add(1);
        self.committed = self.committed.wrapping_add(1);
        self.fingerprint = fingerprint;
        self.content = DrawioContent::Ready { bytes, source };
        cx.notify();
        Ok(())
    }

    /// The one shared undo step: the previous committed revision becomes the
    /// document and every presentation — including the one that made the
    /// change — reloads to it. Returns false when there is nothing to undo.
    pub(crate) fn undo(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(previous) = self.past.pop_back() else {
            return false;
        };
        if let DrawioContent::Ready { bytes, .. } = &previous {
            self.history_bytes = self.history_bytes.saturating_sub(bytes.len());
        }
        let restored = std::mem::replace(&mut self.content, previous.clone());
        self.future.push(restored);
        self.edit = self.edit.wrapping_add(1);
        self.fingerprint = self.bytes().as_deref().map(fingerprint).unwrap_or_default();
        cx.notify();
        true
    }

    /// Re-applies the newest undone revision; a new commit clears it first.
    pub(crate) fn redo(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(next) = self.future.pop() else {
            return false;
        };
        let current = std::mem::replace(&mut self.content, next.clone());
        if let DrawioContent::Ready { bytes, .. } = &current {
            self.history_bytes = self.history_bytes.saturating_add(bytes.len());
        }
        self.past.push_back(current);
        self.trim_past();
        self.edit = self.edit.wrapping_add(1);
        self.fingerprint = self.bytes().as_deref().map(fingerprint).unwrap_or_default();
        cx.notify();
        true
    }

    #[cfg(test)]
    pub(crate) fn can_undo(&self) -> bool {
        !self.past.is_empty()
    }

    #[cfg(test)]
    pub(crate) fn can_redo(&self) -> bool {
        !self.future.is_empty()
    }

    /// Moves the current ready content into the undo stack, enforcing the
    /// entry and byte budgets. Nothing past a failed load is recoverable, so
    /// a Failed content is dropped rather than becoming an undo target.
    fn push_past(&mut self) {
        if let DrawioContent::Ready { bytes, .. } = &self.content {
            self.history_bytes = self.history_bytes.saturating_add(bytes.len());
        }
        match &self.content {
            DrawioContent::Ready { .. } => self.past.push_back(self.content.clone()),
            DrawioContent::Failed(_) => {}
        }
        self.trim_past();
        self.future.clear();
    }

    /// Drops the oldest undo entries until both budgets hold.
    fn trim_past(&mut self) {
        while self.past.len() > MAX_HISTORY || self.history_bytes > MAX_HISTORY_BYTES {
            let Some(evicted) = self.past.pop_front() else {
                break;
            };
            if let DrawioContent::Ready { bytes, .. } = &evicted {
                self.history_bytes = self.history_bytes.saturating_sub(bytes.len());
            }
        }
    }

    pub(super) fn begin_reload(&mut self, cx: &mut Context<Self>) {
        if !self.reloading {
            self.reloading = true;
            cx.notify();
        }
    }

    pub(crate) fn revision(&self) -> u64 {
        self.revision
    }

    /// The edit generation of the current bytes. `commit`, `undo`, `redo`
    /// and `replace` all advance it; a save captures it so a write that
    /// raced an engine commit cannot silently retire newer work.
    pub(crate) fn edit_revision(&self) -> u64 {
        self.edit
    }

    /// Every accepted engine commit, including byte-identical sync flushes.
    /// A deferred save waits on this advancing, not on `edit`, so an
    /// unchanged model still confirms its flush.
    pub(crate) fn committed(&self) -> u64 {
        self.committed
    }

    /// A cheap digest of the authoritative bytes, recomputed once per
    /// accepted revision so the dirty comparison stays a comparison — undo
    /// back to the saved bytes reads clean again.
    pub(super) fn fingerprint(&self) -> super::Fingerprint {
        self.fingerprint
    }

    pub(crate) fn reloading(&self) -> bool {
        self.reloading
    }

    pub(crate) fn bytes(&self) -> Option<Arc<[u8]>> {
        match &self.content {
            DrawioContent::Ready { bytes, .. } => Some(bytes.clone()),
            DrawioContent::Failed(_) => None,
        }
    }

    fn source(&self) -> Option<&DrawioSource> {
        match &self.content {
            DrawioContent::Ready { source, .. } => Some(source),
            DrawioContent::Failed(_) => None,
        }
    }

    pub(crate) fn page_count(&self) -> usize {
        self.source().map_or(0, |source| source.pages.len())
    }

    pub(crate) fn failure(&self) -> Option<&SharedString> {
        match &self.content {
            DrawioContent::Failed(message) => Some(message),
            DrawioContent::Ready { .. } => None,
        }
    }
}

/// The workbench's saved-state digest, computed once per accepted revision
/// rather than on every dirty comparison.
fn fingerprint(bytes: &[u8]) -> super::Fingerprint {
    super::Fingerprint::of_bytes(bytes)
}

#[derive(Clone)]
enum EngineStatus {
    Idle,
    Starting,
    Ready,
    Failed(SharedString),
    Unsupported(SharedString),
}

pub(crate) struct DrawioView {
    document: Entity<DrawioDocument>,
    focus: FocusHandle,
    page: usize,
    zoom: f32,
    engine: EngineStatus,
    blocked: Option<SharedString>,
    surface: Option<SurfaceHost>,
    surface_events: Option<Task<()>>,
    /// Polls paint visibility for as long as this view lives. Dropping it ends
    /// the loop; the handle is owned for its lifetime, never inspected.
    _watchdog: Option<Task<()>>,
    surface_pending: bool,
    surface_attempted: bool,
    bounds: Option<Bounds<Pixels>>,
    last_paint: Instant,
    surface_visible: bool,
    revision: u64,
    /// The edit generation this presentation's engine session last rendered.
    /// A sibling's commit bumps the document's generation; this view reloads
    /// to the newer bytes instead of overwriting them on its next commit.
    edit: u64,
    /// Set when the engine asked the workbench to save (its own Ctrl+S); the
    /// editor's frame sweep drains it into the same Host save path as a
    /// toolbar save.
    save_requested: bool,
    _document_observed: Subscription,
}

impl DrawioView {
    pub(super) fn new(document: Entity<DrawioDocument>, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        let (revision, edit) = {
            let document = document.read(cx);
            (document.revision(), document.edit_revision())
        };
        let observed = cx.observe(&document, |this, _, cx| this.sync(cx));
        let watchdog = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(VISIBILITY_POLL).await;
                let Ok(alive) = this.update(cx, |view, cx| view.poll_visibility(cx)) else {
                    break;
                };
                if !alive {
                    break;
                }
            }
        });
        Self {
            document,
            focus,
            page: 0,
            zoom: 1.,
            engine: EngineStatus::Idle,
            blocked: None,
            surface: None,
            surface_events: None,
            _watchdog: Some(watchdog),
            surface_pending: false,
            surface_attempted: false,
            bounds: None,
            last_paint: Instant::now(),
            surface_visible: false,
            revision,
            edit,
            save_requested: false,
            _document_observed: observed,
        }
    }

    pub(super) fn new_at(
        document: Entity<DrawioDocument>,
        page: usize,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut view = Self::new(document, cx);
        view.page = page;
        view
    }

    pub(crate) fn document(&self) -> &Entity<DrawioDocument> {
        &self.document
    }

    pub(crate) fn handle(&self) -> FocusHandle {
        self.focus.clone()
    }

    pub(crate) fn page(&self) -> usize {
        self.page
    }

    pub(crate) fn page_count(&self, cx: &App) -> usize {
        self.document.read(cx).page_count()
    }

    pub(crate) fn page_label(&self, cx: &App) -> Option<SharedString> {
        let count = self.page_count(cx);
        (count > 1).then(|| {
            t_fmt(
                L10nKey::EditorDrawioPageOf,
                &[
                    ("page", &(self.page + 1).to_string()),
                    ("pages", &count.to_string()),
                ],
            )
            .into()
        })
    }

    fn multipage(&self, cx: &App) -> bool {
        self.page_count(cx) > 1
    }

    pub(crate) fn next_page(&mut self, cx: &mut Context<Self>) {
        self.set_page(self.page.saturating_add(1), cx);
    }

    pub(crate) fn previous_page(&mut self, cx: &mut Context<Self>) {
        self.set_page(self.page.saturating_sub(1), cx);
    }

    pub(crate) fn set_page(&mut self, page: usize, cx: &mut Context<Self>) {
        let count = self.page_count(cx);
        if count == 0 {
            return;
        }
        let page = page.min(count - 1);
        if page == self.page {
            return;
        }
        self.page = page;
        if let Some(surface) = &mut self.surface {
            let _ = surface.command(Command::Page(page));
        }
        cx.notify();
    }

    pub(crate) fn zoom_in(&mut self, cx: &mut Context<Self>) {
        self.send_zoom(Command::ZoomIn, cx);
    }

    pub(crate) fn zoom_out(&mut self, cx: &mut Context<Self>) {
        self.send_zoom(Command::ZoomOut, cx);
    }

    pub(crate) fn fit(&mut self, cx: &mut Context<Self>) {
        self.send_zoom(Command::Fit, cx);
    }

    pub(crate) fn actual_size(&mut self, cx: &mut Context<Self>) {
        self.send_zoom(Command::ActualSize, cx);
    }

    fn send_zoom(&mut self, command: Command, cx: &mut Context<Self>) {
        if let Some(surface) = &mut self.surface {
            let _ = surface.command(command);
        } else if matches!(command, Command::ActualSize) {
            self.zoom = 1.;
            cx.notify();
        }
    }

    /// Workbench undo is document state: the previous committed revision
    /// becomes the document and every presentation — this one included —
    /// reloads its engine to it. There is no engine-local undo to race.
    pub(crate) fn undo(&mut self, cx: &mut Context<Self>) {
        self.document.update(cx, |document, cx| {
            document.undo(cx);
        });
    }

    pub(crate) fn redo(&mut self, cx: &mut Context<Self>) {
        self.document.update(cx, |document, cx| {
            document.redo(cx);
        });
    }

    pub(crate) fn delete_selection(&mut self, cx: &mut Context<Self>) {
        self.send_edit(Command::DeleteCells, cx);
    }

    pub(crate) fn copy(&mut self, cx: &mut Context<Self>) {
        self.send_edit(Command::CopyCells, cx);
    }

    pub(crate) fn cut(&mut self, cx: &mut Context<Self>) {
        self.send_edit(Command::CutCells, cx);
    }

    /// The system clipboard's text becomes diagram cells through the engine's
    /// own paste path; an empty clipboard is a no-op.
    pub(crate) fn paste(&mut self, cx: &mut Context<Self>) {
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            return;
        };
        if text.is_empty() {
            return;
        }
        if let Some(surface) = &mut self.surface {
            let _ = surface.command(Command::Paste(text));
            cx.notify();
        }
    }

    /// One engine-bound editing command, dropped when the surface is not up
    /// or has already retired.
    fn send_edit(&mut self, command: Command, cx: &mut Context<Self>) {
        if let Some(surface) = &mut self.surface {
            let _ = surface.command(command);
            cx.notify();
        }
    }

    /// Asks the live engine to serialize and commit its current model, so
    /// the deferred save captures what is actually on the canvas. False
    /// means no live session can answer, so the caller saves the committed
    /// bytes directly.
    pub(crate) fn flush(&mut self) -> bool {
        let Some(surface) = &mut self.surface else {
            return false;
        };
        matches!(surface.command(Command::Sync), Ok(true))
    }

    /// Test-only live surface: the production `flush`, clipboard and page
    /// paths then run through `SurfaceHost::command`, and the test answers
    /// with the same `SurfaceEvent` the native hosts deliver.
    #[cfg(test)]
    pub(crate) fn attach_test_surface(&mut self, cx: &mut Context<Self>) {
        self.surface = Some(SurfaceHost::test());
        self.engine = EngineStatus::Ready;
        cx.notify();
    }

    #[cfg(test)]
    pub(crate) fn test_surface_commands(&self) -> Vec<Command> {
        self.surface
            .as_ref()
            .map(SurfaceHost::test_commands)
            .unwrap_or_default()
    }

    /// Drained by the editor's frame sweep: the engine asked for the Host's
    /// save path (its own Ctrl+S), which only the owning workbench can run.
    pub(crate) fn take_save_request(&mut self) -> bool {
        std::mem::take(&mut self.save_requested)
    }

    pub(crate) fn zoom_label(&self) -> SharedString {
        format!("{:.0}%", self.zoom * 100.).into()
    }

    pub(crate) fn status_text(&self, cx: &App) -> SharedString {
        let document = self.document.read(cx);
        if let Some(message) = document.failure() {
            return message.clone();
        }
        let mut parts = vec![String::from("draw.io")];
        if let Some(label) = self.page_label(cx) {
            parts.push(label.to_string());
        }
        match &self.engine {
            EngineStatus::Idle | EngineStatus::Starting => {
                parts.push(t(L10nKey::EditorDrawioLoading).to_string())
            }
            EngineStatus::Ready => parts.push(self.zoom_label().to_string()),
            EngineStatus::Failed(message) | EngineStatus::Unsupported(message) => {
                parts.push(message.to_string());
            }
        }
        if document.reloading() {
            parts.push(t(L10nKey::EditorDrawioLoading).to_string());
        }
        parts.join(" · ").into()
    }

    fn sync(&mut self, cx: &mut Context<Self>) {
        let (revision, edit) = {
            let document = self.document.read(cx);
            (document.revision(), document.edit_revision())
        };
        if revision == self.revision && edit == self.edit {
            cx.notify();
            return;
        }
        self.revision = revision;
        self.edit = edit;
        let count = self.page_count(cx);
        self.page = if count == 0 {
            0
        } else {
            self.page.min(count - 1)
        };
        // A new document is a new session. The old WebView token and URL must
        // not survive a reload.
        self.surface = None;
        self.surface_events = None;
        self.surface_pending = false;
        self.surface_attempted = false;
        self.surface_visible = false;
        self.blocked = None;
        self.engine = EngineStatus::Idle;
        self.zoom = 1.;
        self.bounds = None;
        cx.notify();
    }

    fn poll_visibility(&mut self, cx: &mut Context<Self>) -> bool {
        if let Some(surface) = &mut self.surface {
            let visible = self.last_paint.elapsed() < VISIBLE_GRACE
                && self.bounds.is_some_and(|bounds| {
                    f32::from(bounds.size.width) > 1. && f32::from(bounds.size.height) > 1.
                });
            if visible != self.surface_visible {
                self.surface_visible = visible;
                surface.set_visible(visible);
            }
            // A static diagram does not otherwise repaint. Keep the live
            // presentation in the paint set so the watchdog can distinguish
            // "still shown" from "tab hidden": when this view leaves the tree
            // no paint reaches `prepared`, the timestamp goes stale and the
            // native child is hidden.
            if visible {
                cx.notify();
            }
        }
        true
    }

    fn prepared(&mut self, bounds: Bounds<Pixels>, window: &Window, cx: &mut Context<Self>) {
        self.last_paint = Instant::now();
        let changed = self.bounds.is_none_or(|old| {
            f32::from(old.origin.x - bounds.origin.x).abs() > 0.5
                || f32::from(old.origin.y - bounds.origin.y).abs() > 0.5
                || f32::from(old.size.width - bounds.size.width).abs() > 0.5
                || f32::from(old.size.height - bounds.size.height).abs() > 0.5
        });
        self.bounds = Some(bounds);
        let scale = window.scale_factor();
        if let Some(surface) = &mut self.surface {
            if changed {
                surface.set_bounds(bounds, scale);
            }
            if !self.surface_visible {
                self.surface_visible = true;
                surface.set_visible(true);
            }
            return;
        }
        if self.surface_pending || self.surface_attempted {
            return;
        }
        #[cfg(test)]
        {
            let _ = cx;
            self.surface_attempted = true;
            self.engine = EngineStatus::Unsupported(SharedString::from(
                "the native diagram surface is not started by automated tests",
            ));
            return;
        }
        #[cfg(not(test))]
        {
            let parent = match ParentWindow::from_window(window) {
                Ok(parent) => parent,
                Err(error) => {
                    self.engine = EngineStatus::Unsupported(SharedString::from(error.to_string()));
                    return;
                }
            };
            let (bytes, generation) = {
                let document = self.document.read(cx);
                let Some(bytes) = document.bytes() else {
                    return;
                };
                (bytes, (document.revision(), document.edit_revision()))
            };
            let theme = if cx.theme().is_dark() {
                PreviewTheme::Dark
            } else {
                PreviewTheme::Light
            };
            self.surface_pending = true;
            self.engine = EngineStatus::Starting;
            cx.spawn(async move |this, cx| {
                let _ = this.update(cx, |view, cx| {
                    // A reload between scheduling and this callback must not
                    // be answered by a stale surface: the late starter is
                    // dropped instead of replacing the new session.
                    let live = {
                        let document = view.document.read(cx);
                        (document.revision(), document.edit_revision())
                    };
                    if live == generation {
                        view.start_surface(parent, bounds, scale, bytes, theme, cx);
                    } else {
                        // The document moved on: un-latch so the next paint
                        // starts a surface for the current bytes.
                        view.surface_pending = false;
                    }
                });
            })
            .detach();
        }
    }

    #[cfg(not(test))]
    fn start_surface(
        &mut self,
        parent: ParentWindow,
        bounds: Bounds<Pixels>,
        scale: f32,
        bytes: Arc<[u8]>,
        theme: PreviewTheme,
        cx: &mut Context<Self>,
    ) {
        self.surface_pending = false;
        self.surface_attempted = true;
        let (sender, receiver) = smol::channel::unbounded();
        match SurfaceHost::start(parent, bounds, scale, bytes, theme, sender) {
            Ok(surface) => {
                self.surface = Some(surface);
                self.surface_visible = true;
                self.engine = EngineStatus::Starting;
                self.surface_events = Some(cx.spawn(async move |this, cx| {
                    while let Ok(event) = receiver.recv().await {
                        let Ok(alive) = this.update(cx, |view, cx| {
                            view.on_surface_event(event, cx);
                            true
                        }) else {
                            break;
                        };
                        if !alive {
                            break;
                        }
                    }
                }));
            }
            Err(error) => {
                self.engine = engine_error(error);
            }
        }
        cx.notify();
    }

    fn on_surface_event(&mut self, event: SurfaceEvent, cx: &mut Context<Self>) {
        match event {
            SurfaceEvent::Ready => {
                self.engine = EngineStatus::Ready;
                if self.page > 0
                    && let Some(surface) = &mut self.surface
                {
                    let _ = surface.command(Command::Page(self.page));
                }
            }
            SurfaceEvent::State { page, pages, zoom } => {
                self.engine = EngineStatus::Ready;
                if pages > 0 {
                    self.page = page.min(pages.saturating_sub(1));
                }
                if zoom.is_finite() && zoom > 0. {
                    self.zoom = zoom;
                }
            }
            SurfaceEvent::Commit { bytes } => {
                // One live engine's newest serialization becomes the document.
                // Pin this view's generation before observers flush so its own
                // session survives the notification while siblings reload.
                match self
                    .document
                    .update(cx, |document, cx| document.commit(bytes, cx))
                {
                    Ok(()) => {
                        let document = self.document.read(cx);
                        self.revision = document.revision();
                        self.edit = document.edit_revision();
                    }
                    Err(error) => {
                        self.blocked = Some(SharedString::from(format!(
                            "engine commit was refused: {error}"
                        )));
                    }
                }
            }
            SurfaceEvent::Save => {
                // The engine's own Ctrl+S: flush its live model into the
                // document, then hand the save to the owning workbench.
                if let Some(surface) = &mut self.surface {
                    let _ = surface.command(Command::Sync);
                }
                self.save_requested = true;
            }
            SurfaceEvent::Undo => {
                self.undo(cx);
            }
            SurfaceEvent::Redo => {
                self.redo(cx);
            }
            SurfaceEvent::Paste => {
                self.paste(cx);
            }
            SurfaceEvent::Clipboard { text } => {
                cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(text));
            }
            SurfaceEvent::Blocked { reason } => {
                self.blocked = Some(SharedString::from(reason));
            }
            SurfaceEvent::Failed { message } => {
                self.engine = EngineStatus::Failed(SharedString::from(message));
                self.surface = None;
                self.surface_events = None;
                self.surface_visible = false;
            }
            SurfaceEvent::Closed => {
                self.surface = None;
                self.surface_events = None;
                self.surface_visible = false;
                if matches!(self.engine, EngineStatus::Starting) {
                    self.engine = EngineStatus::Idle;
                }
            }
        }
        cx.notify();
    }

    fn retry(&mut self, cx: &mut Context<Self>) {
        self.surface = None;
        self.surface_events = None;
        self.surface_pending = false;
        self.surface_attempted = false;
        self.surface_visible = false;
        self.blocked = None;
        self.engine = EngineStatus::Idle;
        self.last_paint = Instant::now();
        cx.notify();
    }

    fn controls(&self, cx: &mut Context<Self>, multipage: bool, ready: bool) -> impl IntoElement {
        let count = self.page_count(cx);
        h_flex()
            .flex_none()
            .items_center()
            .gap_1()
            .when(multipage, |row| {
                row.child(
                    Button::new("drawio-page-previous")
                        .ghost()
                        .xsmall()
                        .flex_none()
                        .icon(IconName::ChevronLeft)
                        .accessibility_label(t(L10nKey::EditorDrawioPagePrevious))
                        .tooltip(t(L10nKey::EditorDrawioPagePrevious))
                        .disabled(self.page == 0)
                        .on_click(cx.listener(|this, _, _, cx| this.previous_page(cx))),
                )
                .child(
                    div()
                        .id("drawio-page-facts")
                        .test_support()
                        .flex_none()
                        .text_xs()
                        .child(self.page_label(cx).unwrap_or_default()),
                )
                .child(
                    Button::new("drawio-page-next")
                        .ghost()
                        .xsmall()
                        .flex_none()
                        .icon(IconName::ChevronRight)
                        .accessibility_label(t(L10nKey::EditorDrawioPageNext))
                        .tooltip(t(L10nKey::EditorDrawioPageNext))
                        .disabled(self.page + 1 >= count)
                        .on_click(cx.listener(|this, _, _, cx| this.next_page(cx))),
                )
            })
            .when(ready, |row| {
                row.child(
                    Button::new("drawio-fit")
                        .ghost()
                        .xsmall()
                        .flex_none()
                        .label(t(L10nKey::EditorImageFit))
                        .accessibility_label(t(L10nKey::EditorImageFitAction))
                        .tooltip(t(L10nKey::EditorImageFitAction))
                        .on_click(cx.listener(|this, _, _, cx| this.fit(cx))),
                )
                .child(
                    Button::new("drawio-actual")
                        .ghost()
                        .xsmall()
                        .flex_none()
                        .label(t(L10nKey::EditorImageActualSize))
                        .accessibility_label(t(L10nKey::EditorImageActualSizeAction))
                        .tooltip(t(L10nKey::EditorImageActualSizeAction))
                        .on_click(cx.listener(|this, _, _, cx| this.actual_size(cx))),
                )
            })
    }

    fn failure_panel(
        &self,
        title: SharedString,
        detail: Option<SharedString>,
        retry: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (muted, foreground, warning) = {
            let theme = cx.theme();
            (theme.muted_foreground, theme.foreground, theme.warning)
        };
        v_flex()
            .id("drawio-failed")
            .test_support()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .items_center()
            .justify_center()
            .gap_2()
            .px_4()
            .child(
                Icon::new(IconName::TriangleAlert)
                    .large()
                    .text_color(warning),
            )
            .child(div().text_sm().text_color(foreground).child(title))
            .when_some(detail, |panel, detail| {
                panel.child(
                    div()
                        .max_w(rems(34.))
                        .text_xs()
                        .text_color(muted)
                        .child(detail),
                )
            })
            .when(retry, |panel| {
                panel.child(
                    Button::new("drawio-retry")
                        .small()
                        .label(t(L10nKey::EditorDrawioRetry))
                        .on_click(cx.listener(|this, _, _, cx| this.retry(cx))),
                )
            })
            .into_any_element()
    }
}

#[cfg(not(test))]
fn engine_error(error: SurfaceError) -> EngineStatus {
    match error {
        #[cfg(not(any(windows, target_os = "macos")))]
        SurfaceError::Unsupported(message) => {
            EngineStatus::Unsupported(SharedString::from(message))
        }
        other => EngineStatus::Failed(SharedString::from(other.to_string())),
    }
}

impl Focusable for DrawioView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for DrawioView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.last_paint = Instant::now();
        let (border, muted, foreground, warning, primary) = {
            let theme = cx.theme();
            (
                theme.border,
                theme.muted_foreground,
                theme.foreground,
                theme.warning,
                theme.primary,
            )
        };
        let focused = self.focus.contains_focused(window, cx);
        let ring = if focused {
            primary
        } else {
            gpui_kit::transparent_black()
        };
        let multipage = self.multipage(cx);
        let page_label = self.page_label(cx);
        let ready = matches!(self.engine, EngineStatus::Ready);
        let document_failure = self.document.read(cx).failure().cloned();
        let reloading = self.document.read(cx).reloading();
        let engine = self.engine.clone();
        let blocked = self.blocked.clone();
        let zoom = self.zoom_label();
        let surface_started = self.surface.is_some();

        let toolbar = h_flex()
            .flex_none()
            .w_full()
            .items_center()
            .gap_2()
            .px_2()
            .py_1()
            .border_b_1()
            .border_color(border)
            .child(
                div()
                    .id("drawio-format")
                    .test_support()
                    .flex_none()
                    .text_xs()
                    .text_color(muted)
                    .child("draw.io"),
            )
            .when_some(page_label, |row, label| {
                row.child(
                    div()
                        .id("drawio-page-label")
                        .test_support()
                        .flex_none()
                        .text_xs()
                        .text_color(muted)
                        .child(label),
                )
            })
            .child(div().flex_1())
            .when(reloading, |row| {
                row.child(
                    div()
                        .flex_none()
                        .text_xs()
                        .text_color(muted)
                        .child(t(L10nKey::EditorDrawioLoading)),
                )
            })
            .when(ready, |row| {
                row.child(
                    div()
                        .id("drawio-zoom")
                        .test_support()
                        .flex_none()
                        .text_xs()
                        .text_color(muted)
                        .child(zoom),
                )
            })
            .child(self.controls(cx, multipage, ready));

        let body = if let Some(message) = document_failure {
            self.failure_panel(
                t(L10nKey::EditorDrawioFailed).into(),
                Some(message),
                false,
                cx,
            )
        } else {
            match &engine {
                EngineStatus::Unsupported(message) => self.failure_panel(
                    t(L10nKey::EditorDrawioUnavailable).into(),
                    Some(message.clone()),
                    false,
                    cx,
                ),
                EngineStatus::Failed(message) => self.failure_panel(
                    t(L10nKey::EditorDrawioFailed).into(),
                    Some(message.clone()),
                    true,
                    cx,
                ),
                EngineStatus::Idle | EngineStatus::Starting | EngineStatus::Ready => {
                    let canvas = div()
                        .flex_1()
                        .min_w_0()
                        .min_h_0()
                        .relative()
                        .border_1()
                        .border_color(ring)
                        .on_children_prepainted({
                            let view = cx.entity();
                            move |bounds, window, cx| {
                                if let Some(bounds) = bounds.first() {
                                    let bounds = *bounds;
                                    view.update(cx, |view, cx| view.prepared(bounds, window, cx));
                                }
                            }
                        })
                        // `on_children_prepainted` is delivered for child
                        // bounds; the empty child gives the native surface a
                        // real paint event to anchor to.
                        .child(div().size_full())
                        .id("drawio-canvas")
                        .test_support();
                    let overlay = if !matches!(engine, EngineStatus::Ready) && !surface_started {
                        Some(
                            v_flex()
                                .absolute()
                                .inset_0()
                                .items_center()
                                .justify_center()
                                .child(
                                    div()
                                        .id("drawio-loading")
                                        .test_support()
                                        .text_sm()
                                        .text_color(muted)
                                        .child(t(L10nKey::EditorDrawioLoading)),
                                )
                                .into_any_element(),
                        )
                    } else {
                        None
                    };
                    let blocked_note = blocked.map(|reason| {
                        v_flex()
                            .absolute()
                            .left_2()
                            .right_2()
                            .bottom_2()
                            .px_2()
                            .py_1()
                            .border_1()
                            .border_color(warning)
                            .child(
                                div()
                                    .id("drawio-blocked")
                                    .test_support()
                                    .text_xs()
                                    .text_color(foreground)
                                    .child(t_fmt(
                                        L10nKey::EditorDrawioBlocked,
                                        &[("what", reason.as_ref())],
                                    )),
                            )
                            .into_any_element()
                    });
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .min_h_0()
                        .child(canvas)
                        .when_some(overlay, |col, overlay| col.child(overlay))
                        .when_some(blocked_note, |col, note| col.child(note))
                        .into_any_element()
                }
            }
        };

        v_flex()
            .id(("drawio-document", cx.entity_id()))
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &DrawioNextPage, _, cx| this.next_page(cx)))
            .on_action(cx.listener(|this, _: &DrawioPreviousPage, _, cx| this.previous_page(cx)))
            .on_action(cx.listener(|this, _: &DrawioZoomIn, _, cx| this.zoom_in(cx)))
            .on_action(cx.listener(|this, _: &DrawioZoomOut, _, cx| this.zoom_out(cx)))
            .on_action(cx.listener(|this, _: &DrawioFit, _, cx| this.fit(cx)))
            .on_action(cx.listener(|this, _: &DrawioActualSize, _, cx| this.actual_size(cx)))
            .on_action(cx.listener(|this, _: &DrawioUndo, _, cx| this.undo(cx)))
            .on_action(cx.listener(|this, _: &DrawioRedo, _, cx| this.redo(cx)))
            .on_action(cx.listener(|this, _: &DrawioCopy, _, cx| this.copy(cx)))
            .on_action(cx.listener(|this, _: &DrawioCut, _, cx| this.cut(cx)))
            .on_action(cx.listener(|this, _: &DrawioPaste, _, cx| this.paste(cx)))
            .on_action(cx.listener(|this, _: &DrawioDelete, _, cx| this.delete_selection(cx)))
            .on_key_down(cx.listener(|this, event: &gpui_kit::KeyDownEvent, _, cx| {
                let key = event.keystroke.key.as_str();
                if event.keystroke.modifiers.secondary(cx) && matches!(key, "pageup" | "pagedown") {
                    return;
                }
                if this.multipage(cx) {
                    match key {
                        "pageup" => {
                            this.previous_page(cx);
                            cx.stop_propagation();
                        }
                        "pagedown" => {
                            this.next_page(cx);
                            cx.stop_propagation();
                        }
                        "home" => {
                            this.set_page(0, cx);
                            cx.stop_propagation();
                        }
                        "end" => {
                            let last = this.page_count(cx).saturating_sub(1);
                            this.set_page(last, cx);
                            cx.stop_propagation();
                        }
                        _ => {}
                    }
                }
            }))
            .on_mouse_down(
                gpui_kit::MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    this.focus.focus(window, cx);
                    if let Some(surface) = &mut this.surface {
                        surface.focus();
                    }
                }),
            )
            .size_full()
            .min_w_0()
            .min_h_0()
            .child(toolbar)
            .child(body)
    }
}

/// Right-click menu: the same page and zoom actions as the toolbar and keys.
pub(super) fn menu_entries(
    menu: gpui_kit::component::menu::PopupMenu,
    multipage: bool,
) -> gpui_kit::component::menu::PopupMenu {
    let menu = menu
        .separator()
        .menu(t(L10nKey::AppMenuUndo), Box::new(DrawioUndo))
        .menu(t(L10nKey::AppMenuRedo), Box::new(DrawioRedo))
        .separator()
        .menu(t(L10nKey::AppMenuCopy), Box::new(DrawioCopy))
        .menu(t(L10nKey::AppMenuCut), Box::new(DrawioCut))
        .menu(t(L10nKey::AppMenuPaste), Box::new(DrawioPaste))
        .menu(t(L10nKey::EditorDrawioDelete), Box::new(DrawioDelete))
        .separator()
        .menu(t(L10nKey::EditorImageZoomIn), Box::new(DrawioZoomIn))
        .menu(t(L10nKey::EditorImageZoomOut), Box::new(DrawioZoomOut))
        .menu(t(L10nKey::EditorImageFit), Box::new(DrawioFit))
        .menu(
            t(L10nKey::EditorImageActualSize),
            Box::new(DrawioActualSize),
        );
    if multipage {
        menu.separator()
            .menu(
                t(L10nKey::EditorDrawioPagePrevious),
                Box::new(DrawioPreviousPage),
            )
            .menu(t(L10nKey::EditorDrawioPageNext), Box::new(DrawioNextPage))
    } else {
        menu
    }
}

#[cfg(test)]
#[path = "drawio_document/tests.rs"]
mod tests;
