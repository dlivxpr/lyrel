pub(crate) mod presentation;

use crate::core::session::{SessionPane, SessionTab};
use crate::terminal::view::TerminalView;
use crate::ui::pane::{Dir, Pane, PaneSlot};
use gpui_kit::{App, Entity, IntoElement, TestSupportExt as _, Window, div, prelude::*};
use std::rc::Rc;

pub(crate) struct Tab {
    pub(crate) pane: Pane,
    /// Durable presentation belongs to the machine; focus and geometry remain local.
    presentation: lyrel_core::core::machine::TabPresentation,
    foreign_presentation: Option<
        lyrel_core::core::machine::TabPresentation<lyrel_core::core::pane_sidecar::ScopedPane>,
    >,
    name: Option<String>,
    last_focused: Option<gpui_kit::EntityId>,
    /// The pane zoomed in this tab, stashed here by `activate` while another
    /// tab is on screen — zoom is a tab's view state, not the window's, so
    /// looking at another tab and coming back must not lose it (#599). `None`
    /// while the tab is active: then the zoom lives in `LyrelApp::maximized`.
    zoomed: Option<Entity<TerminalView>>,
    pub(crate) diff_overlay: Option<crate::ui::diff_overlay::DiffOverlayState>,
    pub(crate) code: Option<Box<crate::ui::code_editor::TabCode>>,
    /// The pinned sidebar group this tab was put in, by id, or `None` to
    /// leave it to auto grouping. A `Cell` because the sidebar files a tab
    /// that walks into a pinned folder while it is working out where to draw
    /// it, which it does from `&self`.
    pub(crate) group: std::cell::Cell<Option<crate::core::group_key::GroupId>>,
    /// The auto group this tab last resolved to. Stored nowhere — an auto
    /// group is derived — but remembered here so a tab whose repo probe is
    /// still in flight stays where it was instead of bouncing through
    /// Ungrouped for the frames the probe takes.
    pub(crate) auto_group: std::cell::RefCell<Option<crate::core::group_key::AutoKey>>,
    /// Which pinned folder this tab was last seen inside, so it joins one only
    /// on the way *in* — see [`crate::core::group_key::EntryWatch`].
    pub(crate) folder_watch: std::cell::Cell<crate::core::group_key::EntryWatch>,
    overlay_top: OverlayTop,
    /// Whether this tab's document fills the workspace or docks beside the
    /// terminal, once the tab has been told. `None` follows `document_layout`
    /// in the config, which is the default a fresh tab starts from and the
    /// last explicit choice anyone made.
    ///
    /// Per tab rather than per window because what you are doing differs per
    /// tab: reading a long file in one while an agent works in another wants
    /// the whole window here and half of it there, and a global switch made
    /// each of those flip the other.
    document_layout: Option<crate::core::config::DocumentLayout>,
    id: std::cell::Cell<lyrel_core::core::machine::TabId>,
    /// Monotonic stamp of when this tab was last activated, used to order the
    /// switcher's tab column most-recently-used first. Zero means never.
    last_used: std::cell::Cell<u64>,
    /// Where a directional focus move started, keyed by the pane it landed on
    /// and the direction that undoes it, so reversing a move comes back here
    /// instead of wherever geometry ranks first (#738). Per tab because the
    /// panes are.
    ///
    /// Keyed by pane and not by direction alone: one slot per direction is
    /// overwritten by the next move the same way, so a walk of two steps left
    /// and two back right ends somewhere other than it started — the very drift
    /// this is here to stop. A pane remembering its own way in retraces the
    /// whole walk.
    ///
    /// A recorded pane only ever breaks a tie between the panes already next to
    /// the one focus is leaving, so an entry left over from an older layout — or
    /// from before a click moved focus somewhere else entirely — can at worst
    /// pick a different neighbour, never a distant one. Entries naming a pane
    /// the tab no longer holds are dropped as the next one is written, so a
    /// closed pane leaves nothing behind either.
    focus_origin: std::collections::HashMap<(gpui_kit::EntityId, Dir), gpui_kit::EntityId>,
    /// Set while the tab is asleep (#762): its panes were stopped and `pane`
    /// is `Pane::Empty`. What is kept instead is what it takes to bring them
    /// back — see [`Asleep`].
    asleep: Option<Asleep>,
    pub(crate) pip_interaction: Rc<crate::ui::pip_geometry::PipInteraction>,
}

/// A sleeping tab: nothing of it runs, and everything a wake needs is here.
///
/// `layout` is the tab as the restore path reads a tab — pane ids, cwds,
/// shells, agent sessions — so waking is that restore, run on one tab: each
/// pane's id asks the daemon for the screen it stored when the pane was
/// stopped, and an agent with a known session is resumed the way a reboot
/// resumes it. `view` is what the tab is called in the meantime, read off
/// the panes as they went to sleep (or off the tree's records, after a
/// restart), since there is no terminal left to ask.
struct Asleep {
    pub(crate) layout: SessionPane,
    pub(crate) view: lyrel_core::core::tab_view::TabView,
    pub(crate) home: Option<std::path::PathBuf>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) enum OverlayTop {
    #[default]
    Code,
    Diff,
}

impl Tab {
    pub(crate) fn new(pane: Pane) -> Self {
        Self::with_identity(pane, lyrel_core::core::machine::TabId::new())
    }

    fn with_identity(pane: Pane, id: lyrel_core::core::machine::TabId) -> Self {
        Self {
            pane,
            presentation: Default::default(),
            foreign_presentation: None,
            name: None,
            last_focused: None,
            zoomed: None,
            diff_overlay: None,
            code: None,
            overlay_top: OverlayTop::default(),
            document_layout: None,
            group: std::cell::Cell::new(None),
            auto_group: std::cell::RefCell::new(None),
            folder_watch: std::cell::Cell::new(crate::core::group_key::EntryWatch::fresh()),
            id: std::cell::Cell::new(id),
            last_used: std::cell::Cell::new(0),
            focus_origin: Default::default(),
            asleep: None,
            pip_interaction: Default::default(),
        }
    }

    pub(crate) fn from_tree(tree: &lyrel_core::core::machine::Tab, pane: Pane) -> Self {
        Self {
            pane,
            presentation: tree.presentation.clone(),
            foreign_presentation: None,
            name: tree.name.clone(),
            last_focused: None,
            zoomed: None,
            diff_overlay: None,
            code: None,
            overlay_top: OverlayTop::default(),
            document_layout: None,
            group: std::cell::Cell::new(tree.group),
            auto_group: std::cell::RefCell::new(tree.last_auto.clone()),
            // Only ever built for a tab that was just created elsewhere — by
            // `lyrel tab new`, or another window — so it is as new as one
            // opened here: opened inside a pinned folder, it joins it. Every
            // window that hears of it reaches the same answer.
            folder_watch: std::cell::Cell::new(crate::core::group_key::EntryWatch::fresh()),
            id: std::cell::Cell::new(tree.id),
            last_used: std::cell::Cell::new(0),
            focus_origin: Default::default(),
            asleep: None,
            pip_interaction: Default::default(),
        }
    }

    /// A restored sleeping tab owns its presentation and wake record, not views.
    pub(crate) fn from_asleep(st: &SessionTab, home: Option<std::path::PathBuf>) -> Self {
        let mut tab = Self::from_session(st, Pane::Empty, st.presentation.clone());
        tab.folder_watch
            .set(crate::core::group_key::EntryWatch::fresh());
        tab.asleep = Some(Asleep {
            layout: st.pane.clone(),
            view: st
                .asleep_view
                .clone()
                .unwrap_or_else(|| view_of_layout(tab.id(), &st.pane)),
            home,
        });
        tab
    }

    pub(crate) fn id(&self) -> lyrel_core::core::machine::TabId {
        self.id.get()
    }
    pub(crate) fn name(&self) -> &Option<String> {
        &self.name
    }

    pub(crate) fn from_session(
        st: &SessionTab,
        pane: Pane,
        presentation: lyrel_core::core::machine::TabPresentation,
    ) -> Self {
        let mut tab = Self::with_identity(
            pane,
            st.tree_id
                .unwrap_or_else(lyrel_core::core::machine::TabId::new),
        );
        tab.name = st.name.clone();
        tab.presentation = presentation;
        tab.group.set(st.group);
        *tab.auto_group.borrow_mut() = st.last_auto.clone();
        tab.folder_watch
            .set(crate::core::group_key::EntryWatch::baseline());
        tab
    }

    pub(crate) fn reopened(st: SessionTab, pane: Pane) -> Self {
        let mut tab = Self::with_identity(
            pane,
            st.tree_id
                .unwrap_or_else(lyrel_core::core::machine::TabId::new),
        );
        tab.name = st.name;
        tab.presentation = st.presentation;
        tab.group.set(st.group);
        *tab.auto_group.borrow_mut() = st.last_auto;
        tab
    }

    pub(crate) fn from_foreign(
        id: lyrel_core::core::machine::TabId,
        name: Option<String>,
        pane: Pane,
    ) -> Self {
        let mut tab = Self::with_identity(pane, id);
        tab.name = name;
        tab
    }

    /// Adopt a daemon name verbatim; interactive rename normalization is owned
    /// by TabSet::rename, not by the synchronization adapter.
    pub(crate) fn adopt_name(&mut self, name: Option<String>) {
        self.name = name;
    }

    pub(crate) fn remembered_pane(&self) -> Option<gpui_kit::EntityId> {
        self.last_focused
    }

    pub(crate) fn remember_focus(&mut self, window: &Window, cx: &App) {
        if let Some(leaf) = self.pane.focused_leaf(window, cx) {
            self.last_focused = Some(leaf.entity_id());
        }
    }

    pub(crate) fn remember_leaf(&mut self, leaf: gpui_kit::EntityId) -> bool {
        if !self
            .pane
            .leaves()
            .iter()
            .any(|slot| slot.entity_id() == leaf)
        {
            return false;
        }
        self.last_focused = Some(leaf);
        true
    }

    fn replace_leaf(&mut self, old: gpui_kit::EntityId, new: &PaneSlot) -> bool {
        if !self.pane.replace_leaf(old, new.clone()) {
            return false;
        }
        if self.last_focused == Some(old) {
            self.last_focused = Some(new.entity_id());
        }
        true
    }

    pub(crate) fn remember_fallback(&mut self, fallback: Option<&PaneSlot>) {
        self.last_focused = fallback.map(PaneSlot::entity_id);
    }

    pub(crate) fn is_hibernatable(&self, remote: bool, cx: &App) -> bool {
        if self.is_asleep() {
            return false;
        }
        let leaves = self.pane.leaves();
        !leaves.is_empty()
            && leaves.iter().all(|leaf| match leaf {
                PaneSlot::Ready(view) => !(remote && view.read(cx).ssh_spec().is_some()),
                PaneSlot::Connecting(_) => false,
            })
    }

    pub(crate) fn sleep(
        &mut self,
        layout: SessionPane,
        mut view: lyrel_core::core::tab_view::TabView,
        home: Option<std::path::PathBuf>,
    ) {
        view.live = false;
        view.status = None;
        let panes = std::mem::replace(&mut self.pane, Pane::Empty);
        self.zoomed = None;
        self.last_focused = None;
        self.focus_origin.clear();
        self.asleep = Some(Asleep { layout, view, home });
        drop(panes);
    }

    pub(crate) fn finish_wake(
        &mut self,
        pane: Pane,
        presentation: lyrel_core::core::machine::TabPresentation,
    ) {
        self.pane = pane;
        self.presentation = presentation;
        self.last_focused = None;
        self.asleep = None;
    }

    pub(crate) fn clear_zoom(&mut self) {
        self.zoomed = None;
    }
    pub(crate) fn zoomed(&self) -> Option<&Entity<TerminalView>> {
        self.zoomed.as_ref()
    }
    #[cfg(test)]
    pub(crate) fn park_zoom(&mut self, zoom: Option<Entity<TerminalView>>) {
        self.zoomed = zoom;
    }

    pub(crate) fn discard_parked_zoom(&mut self, cx: &App) {
        if self
            .zoomed
            .as_ref()
            .is_some_and(|view| self.is_parked_slot(&PaneSlot::Ready(view.clone()), cx))
        {
            self.zoomed = None;
        }
    }

    pub(crate) fn discard_detached_zoom(&mut self) {
        if self.zoomed.as_ref().is_some_and(|view| {
            !self
                .pane
                .leaves()
                .iter()
                .any(|slot| slot.entity_id() == view.entity_id())
        }) {
            self.zoomed = None;
        }
    }

    pub(crate) fn document_layout(&self) -> Option<crate::core::config::DocumentLayout> {
        self.document_layout
    }

    pub(crate) fn choose_document_layout(&mut self, layout: crate::core::config::DocumentLayout) {
        self.document_layout = Some(layout);
    }

    pub(crate) fn is_diff_on_top(&self) -> bool {
        self.overlay_top == OverlayTop::Diff
    }

    pub(crate) fn document_front(&self) -> Option<OverlayTop> {
        let code = self.code.as_ref().is_some_and(|code| code.visible);
        let diff = self.diff_overlay.is_some();
        match (self.overlay_top, code, diff) {
            (_, false, false) => None,
            (OverlayTop::Code, true, _) | (OverlayTop::Diff, true, false) => Some(OverlayTop::Code),
            (OverlayTop::Diff, _, true) | (OverlayTop::Code, false, true) => Some(OverlayTop::Diff),
        }
    }

    /// Raise the code surface and report whether it was visible but buried.
    pub(crate) fn reveal_code(&mut self) -> bool {
        let buried = self.overlay_top == OverlayTop::Diff
            && self.diff_overlay.is_some()
            && self.code.as_ref().is_some_and(|code| code.visible);
        self.overlay_top = OverlayTop::Code;
        buried
    }

    /// Raise the diff surface and report whether it already led the pair.
    pub(crate) fn reveal_diff(&mut self) -> bool {
        let was_front = self.overlay_top == OverlayTop::Diff
            || !self.code.as_ref().is_some_and(|code| code.visible);
        self.overlay_top = OverlayTop::Diff;
        was_front
    }

    pub(crate) fn presentation_slot(&self, pane: u64, cx: &App) -> Option<PaneSlot> {
        let leaves = self.pane.leaves();
        let matches = |slot: &&PaneSlot| match slot {
            PaneSlot::Ready(view) => view.read(cx).pane_id == pane,
            PaneSlot::Connecting(view) => view.read(cx).restore_pane() == Some(pane),
        };
        // A remote workspace may also contain client-local native SSH panes.
        // Their daemon IDs are not identities in the owning machine's tree.
        leaves.iter().filter(matches).find(|slot| {
            !matches!(slot, PaneSlot::Ready(view) if view.read(cx).pane_route().is_local())
        }).cloned().or_else(|| leaves.iter().find(matches).cloned())
    }

    pub(crate) fn pip_slot(&self, cx: &App) -> Option<PaneSlot> {
        if let Some(presentation) = &self.foreign_presentation {
            let pane = presentation.pip()?.pane();
            return self
                .pane
                .leaves()
                .into_iter()
                .find(|slot| crate::ui::pane_sidecar::scoped_pane(slot, cx) == Some(pane));
        }
        self.presentation_slot(self.presentation.pip()?.pane(), cx)
    }

    pub(crate) fn is_parked_slot(&self, slot: &PaneSlot, cx: &App) -> bool {
        if let Some(presentation) = &self.foreign_presentation {
            return crate::ui::pane_sidecar::scoped_pane(slot, cx)
                .is_some_and(|pane| presentation.is_parked(pane));
        }
        let pane = match slot {
            PaneSlot::Ready(view) => Some(view.read(cx).pane_id),
            PaneSlot::Connecting(view) => view.read(cx).restore_pane(),
        };
        pane.filter(|pane| self.presentation.is_parked(*pane))
            .and_then(|pane| self.presentation_slot(pane, cx))
            .is_some_and(|parked| parked.entity_id() == slot.entity_id())
    }

    pub(crate) fn parked_slots(&self, cx: &App) -> Vec<PaneSlot> {
        if let Some(presentation) = &self.foreign_presentation {
            let leaves = self.pane.leaves();
            return presentation
                .parked()
                .iter()
                .filter_map(|entry| {
                    leaves
                        .iter()
                        .find(|slot| {
                            crate::ui::pane_sidecar::scoped_pane(slot, cx) == Some(entry.pane())
                        })
                        .cloned()
                })
                .collect();
        }
        self.presentation
            .parked()
            .iter()
            .filter_map(|entry| self.presentation_slot(entry.pane(), cx))
            .collect()
    }

    pub(crate) fn render_panes(
        &self,
        chrome: &crate::ui::pane::PaneChrome,
        window: &mut Window,
        cx: &mut App,
    ) -> gpui_kit::AnyElement {
        let pip = self.pip_slot(cx);
        if pip.is_none() {
            self.pip_interaction.cancel();
        }
        let hidden = self
            .pane
            .leaves()
            .into_iter()
            .filter(|slot| self.is_parked_slot(slot, cx))
            .map(|slot| slot.entity_id())
            .chain(pip.as_ref().map(PaneSlot::entity_id))
            .collect::<Vec<_>>();
        let tiled = self.pane.render_without(&hidden, chrome, window, cx);
        div()
            .id("pane-presentations")
            .test_support()
            .size_full()
            .relative()
            .overflow_hidden()
            .child(tiled)
            .when_some(pip, |body, slot| {
                body.child(crate::ui::pip_geometry::render(
                    slot,
                    self.id(),
                    self.foreign_presentation
                        .as_ref()
                        .map_or(self.presentation.pip_geometry(), |presentation| {
                            presentation.pip_geometry()
                        }),
                    self.pip_interaction.clone(),
                    chrome,
                    window,
                    cx,
                ))
            })
            .into_any_element()
    }

    pub(crate) fn is_asleep(&self) -> bool {
        self.asleep.is_some()
    }

    /// The layout a sleeping tab will wake into; `None` for a tab that is awake.
    pub(crate) fn asleep_layout(&self) -> Option<&SessionPane> {
        self.asleep.as_ref().map(|a| &a.layout)
    }

    /// The agent a sleeping tab was running when it went to sleep, so its row
    /// still says what it is while nothing in it can.
    pub(crate) fn asleep_agent(&self) -> Option<crate::core::cli_agent::CLIAgent> {
        self.asleep.as_ref().and_then(|a| a.view.agent)
    }

    pub(crate) fn focus_target(&self, cx: &App) -> Option<crate::ui::pane::PaneSlot> {
        let leaves = self.pane.leaves();
        let visible = |slot: &&PaneSlot| !self.is_parked_slot(slot, cx);
        leaves
            .iter()
            .filter(visible)
            .find(|slot| Some(slot.entity_id()) == self.last_focused)
            .or_else(|| leaves.iter().find(visible))
            .cloned()
    }

    /// The pane a move in `dir` out of `at` should return to, if it reverses
    /// the move that brought focus to `at`.
    pub(crate) fn focus_origin(
        &self,
        at: gpui_kit::EntityId,
        dir: Dir,
    ) -> Option<gpui_kit::EntityId> {
        self.focus_origin.get(&(at, dir)).copied()
    }

    /// Remember that a move in `dir` carried focus from `from` to `to`, so the
    /// move back out of `to` returns. `live` names the panes the tab holds now:
    /// anything the layout has moved on from is forgotten here rather than
    /// accumulating for the life of the window.
    pub(crate) fn remember_focus_origin(
        &mut self,
        from: gpui_kit::EntityId,
        to: gpui_kit::EntityId,
        dir: Dir,
        live: &[gpui_kit::EntityId],
    ) {
        self.focus_origin
            .retain(|(at, _), origin| live.contains(at) && live.contains(origin));
        self.focus_origin.insert((to, dir.opposite()), from);
    }

    pub(crate) fn detail_pane(
        &self,
        window: &Window,
        cx: &gpui_kit::App,
    ) -> Option<Entity<TerminalView>> {
        self.pane
            .focused_leaf(window, cx)
            .or_else(|| self.focus_target(cx))
            .and_then(|slot| slot.terminal().cloned())
    }

    /// The leaf a tab names itself after — its title and, with it, the home
    /// that title's path is measured against.
    fn title_leaf(&self, window: Option<&Window>, cx: &App) -> Option<Entity<TerminalView>> {
        let leaf = match window {
            Some(window) => self
                .pane
                .focused_leaf(window, cx)
                .or_else(|| self.focus_target(cx)),
            None => self.focus_target(cx),
        };
        leaf.or_else(|| self.pane.first_leaf())
            .and_then(|l| l.terminal().cloned())
    }

    pub(crate) fn leaf_title(&self, window: Option<&Window>, cx: &App) -> String {
        self.title_leaf(window, cx)
            .map(|l| l.read(cx).title.clone())
            .unwrap_or_default()
    }

    /// This tab as the shared label ladder reads it, together with what a `~`
    /// in whatever it ends up named would mean.
    ///
    /// [`TabView`](lyrel_core::core::tab_view::TabView) is how a tab looks to
    /// someone who is *not* the window showing it — the switcher listing
    /// another window's workspace, `lyrel tab ls` on the far side of a socket.
    /// Building one here from the live pane is what stops this window having a
    /// second opinion: both sides then rank a given name, a title, an agent and
    /// a directory through
    /// [`TabView::label`](lyrel_core::core::tab_view::TabView::label), so the
    /// strip's answer to "which repo is this?" is the switcher's answer too.
    ///
    /// Everything comes off the one leaf the tab names itself after, so the
    /// title and the directory standing in for it can never describe different
    /// panes (#580).
    pub(crate) fn label_view(
        &self,
        window: Option<&Window>,
        cx: &App,
    ) -> (
        lyrel_core::core::tab_view::TabView,
        Option<std::path::PathBuf>,
    ) {
        let name = self.name.clone();
        if let Some(asleep) = &self.asleep {
            let view = lyrel_core::core::tab_view::TabView {
                id: self.id(),
                name,
                ..asleep.view.clone()
            };
            return (view, asleep.home.clone());
        }
        let Some(leaf) = self.title_leaf(window, cx) else {
            return (
                lyrel_core::core::tab_view::TabView {
                    id: self.id(),
                    name,
                    title: String::new(),
                    osc_title: None,
                    cwd: None,
                    agent: None,
                    status: None,
                    live: false,
                    panes: 0,
                },
                None,
            );
        };
        let leaf = leaf.read(cx);
        let view = lyrel_core::core::tab_view::TabView {
            id: self.id(),
            name,
            // The tree's `title` is the foreground process name — what it falls
            // back on once a pane has said nothing about itself. A live pane's
            // equivalent is the placeholder it answers to unprompted: any
            // *other* default it was given (an SSH host, a workspace name) is a
            // name lyrel chose for it deliberately, and `stated_title` hands
            // those up as the title the pane is showing.
            title: crate::terminal::view::DEFAULT_TITLE.to_string(),
            osc_title: leaf.stated_title().map(str::to_string),
            cwd: leaf.cwd().map(|p| p.display().to_string()),
            agent: leaf.agent(),
            status: leaf.agent_session().map(|s| s.status),
            live: !leaf.terminal.exited,
            panes: self.pane.terminals().len(),
        };
        (view, leaf.display_home(cx))
    }

    pub(crate) fn git_status(
        &self,
        window: Option<&Window>,
        cx: &App,
    ) -> Option<crate::terminal::git_status::GitStatus> {
        let leaf = match window {
            Some(window) => self.pane.focused_or_first(window, cx),
            None => self.pane.first_leaf().and_then(|s| s.terminal().cloned()),
        }?;
        leaf.read(cx).git_status(cx)
    }

    pub(crate) fn agent(&self, cx: &App) -> Option<crate::core::cli_agent::CLIAgent> {
        self.pane
            .terminals()
            .into_iter()
            .find_map(|l| l.read(cx).agent())
    }

    /// The tab's most urgent agent leaf, named and reported by that one leaf.
    ///
    /// `agent` and `agent_status` answer independently — the *first* leaf
    /// carrying an agent, and the highest urgency found *anywhere* in the tab
    /// — so reading them as a pair can put one pane's name beside another
    /// pane's state, a row no leaf ever had. Anywhere both halves are shown
    /// at once reads them from here instead (#543).
    pub(crate) fn agent_row(
        &self,
        cx: &App,
    ) -> Option<(
        crate::core::cli_agent::CLIAgent,
        crate::core::cli_agent::AgentStatus,
    )> {
        use crate::core::cli_agent::AgentStatus;
        let urgency = |s: AgentStatus| match s {
            AgentStatus::Waiting => 3,
            AgentStatus::Working => 2,
            AgentStatus::Done => 1,
            AgentStatus::Idle => 0,
        };
        self.pane
            .terminals()
            .into_iter()
            .filter_map(|l| {
                let view = l.read(cx);
                let agent = view.agent()?;
                // A pane whose agent is running but has never reported a
                // session reads as idle, the same reading the badge has always
                // given it.
                let status = view
                    .agent_session()
                    .map(|s| s.status)
                    .unwrap_or(AgentStatus::Idle);
                Some((agent, status))
            })
            .max_by_key(|(_, status)| urgency(*status))
    }

    pub(crate) fn agent_status(&self, cx: &App) -> Option<crate::core::cli_agent::AgentStatus> {
        self.agent_row(cx).map(|(_, status)| status)
    }

    pub(crate) fn agent_unread_count(&self, cx: &App) -> usize {
        use crate::core::cli_agent::AgentStatus;
        if self.agent_status(cx) != Some(AgentStatus::Done) {
            return 0;
        }
        self.pane
            .terminals()
            .into_iter()
            .filter(|l| {
                let v = l.read(cx);
                v.agent_session().map(|s| s.status) == Some(AgentStatus::Done)
                    && v.agent_result_unread()
            })
            .count()
    }
}

/// What a rename box's contents mean for the tab's name.
#[derive(Debug, PartialEq, Eq)]
enum Rename {
    /// The box still holds what it was seeded with, so nothing was asked for.
    Unchanged,
    /// Emptied on purpose: the tab goes back to following its pane.
    Cleared,
    /// A name the user typed.
    Named(String),
}

/// Read a rename box against the label it was seeded with.
///
/// The box is prefilled with the label as rendered, so it is never empty when
/// it opens and a commit cannot tell "left alone" from "typed the same thing"
/// by looking at the value alone. `Blur` commits as readily as Enter does, so
/// without the comparison, opening the box and clicking away stored the label
/// as a name — and a name is a different thing from the title it was copied
/// from: it stops following the pane, freezing the tab on whatever it happened
/// to say at that moment, with no way to undo it.
fn rename_outcome(value: &str, prefill: &str) -> Rename {
    let value = value.trim();
    match value {
        v if v == prefill.trim() => Rename::Unchanged,
        "" => Rename::Cleared,
        v => Rename::Named(v.to_string()),
    }
}

/// The window's ordered tabs and navigation state. Daemon presentation is held
/// by each Tab; the active position, MRU stamps and pending reveal are client-only.
#[derive(Default)]
pub(crate) struct TabSet {
    tabs: Vec<Tab>,
    active: usize,
    use_seq: std::cell::Cell<u64>,
    pending: Option<lyrel_core::core::machine::TabId>,
}

impl TabSet {
    pub(crate) fn new(tabs: Vec<Tab>, active: usize) -> Self {
        let mut state = Self {
            tabs,
            ..Self::default()
        };
        state.select(active);
        state
    }

    pub(crate) fn active_index(&self) -> usize {
        self.active
    }

    /// Select a structural landing (restore, insertion or close). Interactive
    /// switching uses activate so outgoing pane focus and zoom travel together.
    pub(crate) fn select(&mut self, index: usize) {
        self.active = index.min(self.tabs.len().saturating_sub(1));
    }

    pub(crate) fn replace(&mut self, tabs: Vec<Tab>, active: usize) {
        self.tabs = tabs;
        self.select(active);
    }

    pub(crate) fn get_mut(&mut self, index: usize) -> Option<&mut Tab> {
        self.tabs.get_mut(index)
    }
    pub(crate) fn active_mut(&mut self) -> Option<&mut Tab> {
        self.tabs.get_mut(self.active)
    }
    pub(crate) fn iter_mut(&mut self) -> std::slice::IterMut<'_, Tab> {
        self.tabs.iter_mut()
    }
    pub(crate) fn push(&mut self, tab: Tab) {
        self.tabs.push(tab);
    }
    pub(crate) fn insert(&mut self, at: usize, tab: Tab) {
        let had_tabs = !self.tabs.is_empty();
        self.tabs.insert(at, tab);
        if had_tabs && at <= self.active {
            self.active += 1;
        }
    }
    pub(crate) fn remove(&mut self, at: usize) -> Tab {
        let tab = self.tabs.remove(at);
        if at < self.active {
            self.active -= 1;
        }
        self.select(self.active);
        tab
    }
    pub(crate) fn clear(&mut self) {
        self.tabs.clear();
        self.active = 0;
    }
    pub(crate) fn retain(&mut self, mut keep: impl FnMut(&Tab) -> bool) {
        let active = self.tabs.get(self.active).map(Tab::id);
        self.tabs.retain(|tab| keep(tab));
        self.select(
            active
                .and_then(|id| self.index_of(id))
                .unwrap_or(self.active),
        );
    }
    pub(crate) fn index_of(&self, id: lyrel_core::core::machine::TabId) -> Option<usize> {
        self.tabs.iter().position(|tab| tab.id() == id)
    }
    /// Legacy session tabs acquire the machine identity on the first matching
    /// snapshot. Already-known ids and ids another visible tab holds never move.
    pub(crate) fn adopt_machine_ids(&self, machine: &[lyrel_core::core::machine::Tab], cx: &App) {
        let known: Vec<_> = self.tabs.iter().map(Tab::id).collect();
        for tab in &self.tabs {
            if machine.iter().any(|candidate| candidate.id == tab.id()) {
                continue;
            }
            let panes: Vec<_> = tab
                .pane
                .terminals()
                .iter()
                .map(|view| view.read(cx).pane_id)
                .collect();
            if panes.is_empty() {
                continue;
            }
            if let Some(matched) = machine.iter().find(|candidate| {
                !known.contains(&candidate.id)
                    && panes.iter().any(|pane| candidate.root.contains(*pane))
            }) {
                tab.id.set(matched.id);
            }
        }
    }

    pub(crate) fn move_to(&mut self, id: lyrel_core::core::machine::TabId, to: usize) {
        let Some(from) = self.index_of(id) else {
            return;
        };
        let active = self.tabs.get(self.active).map(Tab::id);
        let moved = self.tabs.remove(from);
        self.tabs.insert(to.min(self.tabs.len()), moved);
        if let Some(index) = active.and_then(|id| self.index_of(id)) {
            self.active = index;
        }
    }

    pub(crate) fn request(&mut self, id: lyrel_core::core::machine::TabId) -> Option<usize> {
        let index = self.index_of(id);
        if index.is_none() {
            self.pending = Some(id);
        }
        index
    }
    pub(crate) fn claim_pending(&mut self) -> Option<usize> {
        let index = self.index_of(self.pending?)?;
        self.pending = None;
        Some(index)
    }
    pub(crate) fn clear_pending(&mut self) {
        self.pending = None;
    }

    pub(crate) fn activate(
        &mut self,
        index: usize,
        zoom: &mut Option<Entity<TerminalView>>,
        window: &Window,
        cx: &App,
    ) -> bool {
        if index >= self.tabs.len() || index == self.active {
            return false;
        }
        if let Some(outgoing) = self.tabs.get_mut(self.active) {
            outgoing.remember_focus(window, cx);
            outgoing.pip_interaction.cancel();
            outgoing.zoomed = zoom.take();
        }
        self.active = index;
        let incoming = &mut self.tabs[index];
        *zoom = incoming.zoomed.take().filter(|leaf| {
            incoming.pane.leaves().iter().any(|slot| {
                slot.entity_id() == leaf.entity_id() && !incoming.is_parked_slot(slot, cx)
            })
        });
        true
    }
    pub(crate) fn touch_active(&self) {
        let Some(tab) = self.tabs.get(self.active) else {
            return;
        };
        let top = self.use_seq.get();
        if top != 0 && tab.last_used.get() == top {
            return;
        }
        let next = top + 1;
        self.use_seq.set(next);
        tab.last_used.set(next);
    }
    pub(crate) fn by_mru(&self) -> Vec<usize> {
        let stamps: Vec<u64> = self.tabs.iter().map(|tab| tab.last_used.get()).collect();
        mru_order(&stamps, self.active)
    }
    pub(crate) fn next_in(&self, order: &[usize], forward: bool) -> Option<usize> {
        step_in_order(order, self.active, forward)
    }
    pub(crate) fn move_permutation(&self, run: &[usize], right: bool) -> Option<Vec<usize>> {
        one_slot_move(run, self.tabs.len(), self.active, right)
    }
    pub(crate) fn reorder(&mut self, order: &[usize]) -> bool {
        if order.len() != self.tabs.len() || order.iter().enumerate().all(|(i, &o)| i == o) {
            return false;
        }
        let was_active = self.active;
        let mut slots: Vec<Option<Tab>> = std::mem::take(&mut self.tabs)
            .into_iter()
            .map(Some)
            .collect();
        self.tabs = order.iter().filter_map(|&i| slots[i].take()).collect();
        self.active = order.iter().position(|&i| i == was_active).unwrap_or(0);
        true
    }
    pub(crate) fn awake_besides(&self, index: usize) -> Option<usize> {
        (0..self.tabs.len())
            .filter(|&i| i != index && !self.tabs[i].is_asleep())
            .max_by_key(|&i| {
                (
                    self.tabs[i].last_used.get(),
                    std::cmp::Reverse(i.abs_diff(index)),
                )
            })
    }
    pub(crate) fn remember_leaf(
        &mut self,
        leaf: gpui_kit::EntityId,
    ) -> Option<lyrel_core::core::machine::TabId> {
        for tab in &mut self.tabs {
            if tab.remember_leaf(leaf) {
                return Some(tab.id());
            }
        }
        None
    }
    pub(crate) fn rename(
        &mut self,
        id: lyrel_core::core::machine::TabId,
        value: &str,
        prefill: &str,
    ) -> bool {
        let outcome = rename_outcome(value, prefill);
        if outcome == Rename::Unchanged {
            return false;
        }
        if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id() == id) {
            tab.name = match outcome {
                Rename::Named(name) => Some(name),
                _ => None,
            };
        }
        true
    }
    pub(crate) fn contains_slot(&self, slot: gpui_kit::EntityId) -> bool {
        fn contains(pane: &Pane, slot: gpui_kit::EntityId) -> bool {
            match pane {
                Pane::Leaf(leaf) => leaf.entity_id() == slot,
                Pane::Split { a, b, .. } => contains(a, slot) || contains(b, slot),
                Pane::Empty => false,
            }
        }
        self.tabs.iter().any(|tab| contains(&tab.pane, slot))
    }
    pub(crate) fn replace_leaf(&mut self, old: gpui_kit::EntityId, new: PaneSlot) {
        for tab in &mut self.tabs {
            if tab.replace_leaf(old, &new) {
                break;
            }
        }
    }
}

impl std::ops::Deref for TabSet {
    type Target = [Tab];
    fn deref(&self) -> &Self::Target {
        &self.tabs
    }
}
impl std::ops::Index<usize> for TabSet {
    type Output = Tab;
    fn index(&self, index: usize) -> &Tab {
        &self.tabs[index]
    }
}
impl std::ops::IndexMut<usize> for TabSet {
    fn index_mut(&mut self, index: usize) -> &mut Tab {
        &mut self.tabs[index]
    }
}
impl<'a> IntoIterator for &'a TabSet {
    type Item = &'a Tab;
    type IntoIter = std::slice::Iter<'a, Tab>;
    fn into_iter(self) -> Self::IntoIter {
        self.tabs.iter()
    }
}
impl<'a> IntoIterator for &'a mut TabSet {
    type Item = &'a mut Tab;
    type IntoIter = std::slice::IterMut<'a, Tab>;
    fn into_iter(self) -> Self::IntoIter {
        self.tabs.iter_mut()
    }
}

/// The tab after (or before) `active` in `order`, wrapping round. `None`
/// when there is nowhere else to go; a tab missing from `order` starts from
/// its end, so the step still lands on a tab the user can see.
fn step_in_order(order: &[usize], active: usize, forward: bool) -> Option<usize> {
    let n = order.len();
    if n < 2 {
        return None;
    }
    let pos = order.iter().position(|&i| i == active);
    let next = match (pos, forward) {
        (Some(p), true) => (p + 1) % n,
        (Some(p), false) => (p + n - 1) % n,
        (None, true) => 0,
        (None, false) => n - 1,
    };
    Some(order[next]).filter(|&i| i != active)
}

/// The `apply_tab_order` permutation that moves the active tab one slot
/// within `run` — a move, not a trade: the tab is lifted out and put back at
/// the neighbour's place, so through a wrap the run it crossed shifts one slot
/// (`[a, b, c]` with `c` moved right becomes `[c, a, b]`, not `[c, b, a]`).
/// `run` is the tabs drawn together (a sidebar section, or every tab on a top
/// bar) as ascending `self.tabs` indices out of `n`; only their slots are
/// reassigned, so no tab in another run moves, not even out of sight. `None`
/// when there is nothing to move: a run of fewer than two tabs, or one that
/// does not hold the active tab.
fn one_slot_move(run: &[usize], n: usize, active: usize, right: bool) -> Option<Vec<usize>> {
    let len = run.len();
    if len < 2 {
        return None;
    }
    let pos = run.iter().position(|&i| i == active)?;
    let mut moved = run.to_vec();
    match (right, pos) {
        (true, p) if p + 1 == len => moved.rotate_right(1),
        (false, 0) => moved.rotate_left(1),
        (true, p) => moved.swap(p, p + 1),
        (false, p) => moved.swap(p, p - 1),
    }
    let mut permutation: Vec<usize> = (0..n).collect();
    for (&slot, &tab) in run.iter().zip(&moved) {
        permutation[slot] = tab;
    }
    Some(permutation)
}

fn mru_order(stamps: &[u64], active: usize) -> Vec<usize> {
    let mut order: Vec<usize> = (0..stamps.len()).collect();
    order.sort_by_key(|&i| (stamps[i] == 0, std::cmp::Reverse(stamps[i]), i));
    if let Some(pos) = order.iter().position(|&i| i == active) {
        let lead = order.remove(pos);
        order.insert(0, lead);
    }
    order
}

/// What to call a sleeping tab when the tree had nothing better to say: the
/// first directory in its layout, and the agent in it if there was one.
fn view_of_layout(
    id: lyrel_core::core::machine::TabId,
    layout: &SessionPane,
) -> lyrel_core::core::tab_view::TabView {
    fn leaves<'a>(pane: &'a SessionPane, out: &mut Vec<&'a SessionPane>) {
        match pane {
            SessionPane::Leaf { .. } => out.push(pane),
            SessionPane::Split { a, b, .. } => {
                leaves(a, out);
                leaves(b, out);
            }
        }
    }
    let mut all = Vec::new();
    leaves(layout, &mut all);
    let cwd = all.iter().find_map(|leaf| match leaf {
        SessionPane::Leaf { cwd, .. } => cwd.as_ref().map(|p| p.display().to_string()),
        SessionPane::Split { .. } => None,
    });
    let agent = all.iter().find_map(|leaf| match leaf {
        SessionPane::Leaf { agent, .. } => *agent,
        SessionPane::Split { .. } => None,
    });
    lyrel_core::core::tab_view::TabView {
        id,
        name: None,
        title: String::new(),
        osc_title: None,
        cwd,
        agent,
        status: None,
        live: false,
        panes: all.len(),
    }
}

#[cfg(test)]
#[path = "tab/tests_focus_origin.rs"]
mod focus_origin_tests;
#[cfg(test)]
#[path = "tab/tests_name.rs"]
mod name_tests;
#[cfg(test)]
#[path = "tab/tests_navigation.rs"]
mod navigation_tests;
#[cfg(test)]
#[path = "tab/tests_launch.rs"]
mod launch_tests;
