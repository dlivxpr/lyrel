//! Window-local result cursors. Daemon records and other clients are never mutated.
use crate::core::osc::NoteBudget;
use lyrel_core::core::program_status::{ProgramRecord, ProgramState, ProgramStatusFrame};
use lyrel_ui_base::host_ops::HostId;
use lyrel_ui_base::i18n::{L10nKey, t};
use std::collections::HashMap;
use std::time::Instant;

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct ProgramSource {
    pub(crate) host: HostId,
    pub(crate) pane: u64,
}

struct ResultCursor {
    state: ProgramState,
    observed: u64,
    acknowledged: u64,
}

impl ResultCursor {
    fn unread(&self) -> bool {
        matches!(
            self.state,
            ProgramState::Done | ProgramState::Error | ProgramState::Blocked
        ) && self.observed > self.acknowledged
    }
}

struct PaneObservation {
    session: String,
    semantic: u64,
    results: u64,
    records: HashMap<String, ResultCursor>,
}

impl PaneObservation {
    fn unread(&self) -> usize {
        self.records
            .values()
            .filter(|record| record.unread())
            .count()
    }

    fn update_record(&mut self, record: &ProgramRecord, cold: bool) -> bool {
        let version = record.result_version();
        let Some(cursor) = self.records.get_mut(record.id()) else {
            self.records.insert(
                record.id().to_owned(),
                ResultCursor {
                    state: record.state(),
                    observed: version,
                    acknowledged: if cold { version } else { 0 },
                },
            );
            return version != 0
                && matches!(record.state(), ProgramState::Blocked | ProgramState::Error);
        };
        let attention = version > cursor.observed
            && matches!(record.state(), ProgramState::Blocked | ProgramState::Error);
        cursor.state = record.state();
        cursor.observed = version;
        if cold {
            cursor.acknowledged = version;
        }
        attention
    }
}

#[derive(Default)]
pub(crate) struct ProgramAttention {
    panes: HashMap<ProgramSource, PaneObservation>,
}

pub(crate) struct AttentionChange {
    pub(crate) reminder_changed: bool,
    pub(crate) notices: Vec<String>,
}

impl ProgramAttention {
    pub(crate) fn forget(&mut self, source: ProgramSource) {
        self.panes.remove(&source);
    }
    pub(crate) fn observe(
        &mut self,
        source: ProgramSource,
        frame: &ProgramStatusFrame,
        deliver: bool,
    ) -> AttentionChange {
        let mut change = AttentionChange {
            reminder_changed: false,
            notices: Vec::new(),
        };
        let before = self.unread(source);
        match frame {
            ProgramStatusFrame::Baseline(snapshot) => {
                let cold = self
                    .panes
                    .get(&source)
                    .is_none_or(|pane| pane.session != snapshot.session_id());
                if cold {
                    self.panes.insert(
                        source,
                        PaneObservation {
                            session: snapshot.session_id().to_owned(),
                            semantic: snapshot.semantic_version(),
                            results: snapshot.result_version(),
                            records: HashMap::new(),
                        },
                    );
                }
                let pane = self
                    .panes
                    .get_mut(&source)
                    .expect("baseline establishes its observation");
                if snapshot.semantic_version() < pane.semantic
                    || snapshot.result_version() < pane.results
                {
                    return change;
                }
                pane.records
                    .retain(|id, _| snapshot.records().iter().any(|record| record.id() == id));
                for record in snapshot.records() {
                    pane.update_record(record, cold);
                }
                pane.semantic = snapshot.semantic_version();
                pane.results = snapshot.result_version();
                change.reminder_changed = before != pane.unread();
                // A baseline is observation, never a replayed desktop notice.
            }
            ProgramStatusFrame::Update(delta) => {
                let Some(pane) = self.panes.get_mut(&source) else {
                    return change;
                };
                if pane.session != delta.session_id()
                    || delta.semantic_version() < pane.semantic
                    || delta.result_version() < pane.results
                {
                    return change;
                }
                let before = pane.unread();
                for id in delta.removed() {
                    pane.records.remove(id);
                }
                for record in delta.changed() {
                    if pane.update_record(record, false) && deliver {
                        change.notices.push(record.id().to_owned());
                    }
                }
                pane.semantic = delta.semantic_version();
                pane.results = delta.result_version();
                change.reminder_changed = before != pane.unread();
            }
            _ => {}
        }
        change
    }

    pub(crate) fn unread(&self, source: ProgramSource) -> usize {
        self.panes.get(&source).map_or(0, PaneObservation::unread)
    }

    /// Acknowledge only versions this window actually observed. An incoming
    /// newer result is not covered by the cursor written here.
    pub(crate) fn acknowledge(&mut self, source: ProgramSource) -> bool {
        let Some(pane) = self.panes.get_mut(&source) else {
            return false;
        };
        let mut changed = false;
        for cursor in pane.records.values_mut() {
            if cursor.unread() {
                cursor.acknowledged = cursor.observed;
                changed = true;
            }
        }
        changed
    }
}

#[derive(Default)]
struct ProgramNotices {
    sources: HashMap<ProgramSource, NoticeSource>,
}

struct NoticeSource {
    session: String,
    /// Highest result version any viewer actually notified for, per record. A
    /// version becomes the floor only when a notice went out; a viewer that
    /// saw the change live advances its own `seen` instead, so a transition
    /// that was never delivered still reaches the next unwatched viewer.
    /// Keyed by record id: versions are pane-wide counters, so a scalar floor
    /// raised by one record would suppress an independent record that no
    /// window was ever notified about.
    delivered: HashMap<String, u64>,
    budget: NoteBudget,
    viewers: Vec<NoticeViewer>,
}

struct NoticeViewer {
    view: gpui_kit::WeakEntity<lyrel_terminal::view::TerminalView>,
    /// Highest result version this viewer saw while watching, so it is never
    /// replayed to this window later.
    seen: u64,
}

impl gpui_kit::Global for ProgramNotices {}

impl ProgramNotices {
    fn deliver(
        &mut self,
        source: ProgramSource,
        view: &gpui_kit::Entity<lyrel_terminal::view::TerminalView>,
        frame: &ProgramStatusFrame,
        wanted: &[String],
        now: Instant,
    ) -> Vec<String> {
        let ProgramStatusFrame::Update(delta) = frame else {
            return Vec::new();
        };
        self.sources.retain(|_, source| {
            source
                .viewers
                .retain(|viewer| viewer.view.upgrade().is_some());
            !source.viewers.is_empty()
        });
        let entry = self.sources.entry(source).or_insert_with(|| NoticeSource {
            session: delta.session_id().to_owned(),
            delivered: HashMap::new(),
            budget: NoteBudget::default(),
            viewers: Vec::new(),
        });
        if entry.session != delta.session_id() {
            entry.session = delta.session_id().to_owned();
            entry.delivered.clear();
            entry.budget = NoteBudget::default();
            for viewer in &mut entry.viewers {
                viewer.seen = 0;
            }
        }
        for id in delta.removed() {
            entry.delivered.remove(id);
        }
        let position = entry.viewers.iter().position(|viewer| {
            viewer
                .view
                .upgrade()
                .is_some_and(|viewer| viewer.entity_id() == view.entity_id())
        });
        let position = match position {
            Some(position) => position,
            None => {
                entry.viewers.push(NoticeViewer {
                    view: view.downgrade(),
                    seen: 0,
                });
                entry.viewers.len() - 1
            }
        };
        let mut notices = Vec::new();
        for record in delta.changed() {
            let version = record.result_version();
            if !wanted.iter().any(|id| id == record.id()) {
                // This viewer observed the transition live, so mark it seen.
                // The shared floor is untouched: a client that never got the
                // notice still delivers it the first time it is not watching.
                entry.viewers[position].seen = entry.viewers[position].seen.max(version);
                continue;
            }
            let floor = entry.delivered.get(record.id()).copied().unwrap_or(0);
            if version <= floor || version <= entry.viewers[position].seen {
                continue;
            }
            let (allowed, dropped) = entry.budget.admit(now, 1);
            if dropped {
                notices.push(t(L10nKey::ProgramNotesDropped).to_owned());
            }
            if allowed != 0 {
                notices.push(crate::ui::program_status::record_label(record));
            }
            entry.viewers[position].seen = version;
            entry
                .delivered
                .insert(record.id().to_owned(), floor.max(version));
        }
        notices
    }
}

impl crate::ui::app::LyrelApp {
    pub(crate) fn observe_program_attention(
        &mut self,
        view: &gpui_kit::Entity<lyrel_terminal::view::TerminalView>,
        frames: &[ProgramStatusFrame],
        window: &gpui_kit::Window,
        cx: &mut gpui_kit::Context<Self>,
    ) -> bool {
        if !self.tabs.contains_slot(view.entity_id()) {
            return false;
        }
        let source = view.read(cx).program_source();
        let watched = self.program_pane_is_viewed(view, window, cx);
        let deliver = cx
            .global::<lyrel_ui_base::config::Config>()
            .notify_on_command_finish
            .allows(watched);
        let now = Instant::now();
        let mut changed = false;
        for frame in frames {
            let update = self.program_attention.observe(source, frame, deliver);
            changed |= update.reminder_changed;
            let notices = cx.default_global::<ProgramNotices>().deliver(
                source,
                view,
                frame,
                &update.notices,
                now,
            );
            if !notices.is_empty() {
                // Same reports from different panes are indistinguishable
                // unless the title says which pane they came from; untrusted
                // program text stays in the body either way.
                let title = {
                    let view = view.read(cx);
                    let lead = match view.stated_title() {
                        Some(pane) => format!("{} · {pane}", t(L10nKey::ProgramSource)),
                        None => t(L10nKey::ProgramSource).to_string(),
                    };
                    view.notification_title(Some(&lead), cx)
                };
                for body in notices {
                    lyrel_terminal::desktop_notification::notify_desktop_for_pane(
                        Some(&title),
                        &body,
                        Some(view.entity_id()),
                    );
                }
            }
        }
        if watched {
            changed |= self.program_attention.acknowledge(source);
        }
        changed
    }

    fn program_pane_is_viewed(
        &self,
        view: &gpui_kit::Entity<lyrel_terminal::view::TerminalView>,
        window: &gpui_kit::Window,
        cx: &gpui_kit::App,
    ) -> bool {
        if !window.is_window_active()
            || !view.read(cx).focus_handle.contains_focused(window, cx)
            || self
                .maximized
                .as_ref()
                .is_some_and(|visible| visible.entity_id() != view.entity_id())
        {
            return false;
        }
        self.tabs.get(self.tabs.active_index()).is_some_and(|tab| {
            tab.pane.leaves().into_iter().any(|slot| {
                slot.terminal()
                    .is_some_and(|terminal| terminal.entity_id() == view.entity_id())
                    && !tab.is_parked_slot(&slot, cx)
            })
        })
    }

    /// Called before summary rendering. This only advances this window's
    /// cursor to an already observed version; it never changes daemon state.
    pub(crate) fn acknowledge_viewed_program(
        &mut self,
        window: &gpui_kit::Window,
        cx: &gpui_kit::App,
    ) {
        let Some(view) = self
            .tabs
            .get(self.tabs.active_index())
            .and_then(|tab| tab.pane.focused_or_first(window, cx))
        else {
            return;
        };
        if self.program_pane_is_viewed(&view, window, cx) {
            self.program_attention
                .acknowledge(view.read(cx).program_source());
        }
    }

    pub(crate) fn program_unread_count(
        &self,
        tab: &crate::ui::tab::Tab,
        cx: &gpui_kit::App,
    ) -> usize {
        tab.pane
            .terminals()
            .into_iter()
            .map(|view| {
                let view = view.read(cx);
                if view.terminal.endpoint_program_status() == Some(false) {
                    0
                } else {
                    self.program_attention.unread(view.program_source())
                }
            })
            .sum()
    }
}

/// The program-record identity a pane reports under, read off its
/// `TerminalView`. A trait rather than an inherent method because the view
/// now lives in `lyrel-terminal`; implemented here, where `ProgramSource`
/// itself lives.
pub(crate) trait TerminalViewProgramSource {
    fn program_source(&self) -> ProgramSource;
}

impl TerminalViewProgramSource for lyrel_terminal::view::TerminalView {
    fn program_source(&self) -> ProgramSource {
        ProgramSource {
            host: self.host_id(),
            pane: self.pane_id,
        }
    }
}
