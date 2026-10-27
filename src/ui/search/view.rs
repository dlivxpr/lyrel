//! The modal: a row of tabs over one searchable list.

use std::rc::Rc;

use gpui_kit::base::actions::{Cancel, Confirm, SelectDown, SelectUp};
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, IndexPath, Selectable, h_flex,
    input::{Input, InputEvent, InputState},
    list::{List, ListDelegate, ListEvent, ListState},
    v_flex,
};
use gpui_kit::{
    Action, App, ClickEvent, Context, Entity, EventEmitter, Focusable as _, FontWeight,
    MouseButton, MouseDownEvent, ScrollStrategy, Subscription, Task, TestSupportExt as _, Window,
    div, prelude::*, px, rems,
};

use super::command::{CommandKind, Item};
use super::sources::{Catalog, Row, Section, plain};
use super::{FileList, SearchTab};
use crate::core::actions::{SearchNextTab, SearchPrevTab};
use crate::ui::dialog::{CARD_RADIUS, FOOTER_H, keycap};
use crate::ui::i18n::{L10nKey, t, t_fmt};

/// What the list is showing: one of the tabs, or a list a row opens — the
/// theme picker one of the Actions rows opens, or what can be done with a
/// past session. Those have no tabs — Escape goes back to the one they came
/// from.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Scope {
    Tab(SearchTab),
    Themes,
    SessionActions,
}

pub(crate) struct SearchDelegate {
    catalog: Rc<Catalog>,
    scope: Scope,
    /// The rows of a list a row opens, built when it opens.
    themes: Vec<Item>,
    query: String,
    sections: Vec<Section>,
    selected: Option<IndexPath>,
}

impl SearchDelegate {
    fn new(catalog: Rc<Catalog>, scope: Scope, themes: Vec<Item>, cx: &App) -> Self {
        let mut this = Self {
            catalog,
            scope,
            themes,
            query: String::new(),
            sections: Vec::new(),
            selected: None,
        };
        this.refresh(cx);
        this
    }

    /// Re-runs the current query against the current scope.
    fn refresh(&mut self, cx: &App) {
        self.sections = match self.scope {
            Scope::Tab(tab) => self.catalog.sections(tab, &self.query, cx),
            Scope::Themes | Scope::SessionActions => plain(&self.themes, &self.query),
        };
    }

    fn row_at(&self, ix: IndexPath) -> Option<&Row> {
        self.sections.get(ix.section)?.rows.get(ix.row)
    }

    fn selected_item(&self) -> Option<&Item> {
        self.selected.and_then(|ix| self.row_at(ix)?.item())
    }

    fn position_of(&self, kind: &CommandKind) -> Option<IndexPath> {
        self.sections.iter().enumerate().find_map(|(s, section)| {
            let row = section
                .rows
                .iter()
                .position(|r| r.item().is_some_and(|i| &i.kind == kind))?;
            Some(IndexPath::new(row).section(s))
        })
    }

    /// How many rows the filter left, for a standalone tab's heading.
    fn row_count(&self) -> usize {
        self.sections
            .iter()
            .flat_map(|s| s.rows.iter())
            .filter(|r| r.item().is_some())
            .count()
    }

    fn first_row(&self) -> Option<IndexPath> {
        let section = self.sections.iter().position(|s| !s.rows.is_empty())?;
        Some(IndexPath::new(0).section(section))
    }

    fn render_row(&self, ix: IndexPath, item: &Item, cx: &App) -> gpui_kit::AnyElement {
        let picked = Some(ix) == self.selected;
        let (fg, muted) = {
            let t = cx.theme();
            (t.foreground, t.muted_foreground)
        };

        // The title holds its width longest, with what was typed picked out
        // in it; the subtitle goes to the right edge, in caption ink, and is
        // what truncates first.
        let mut left = h_flex().flex_1().min_w_0().items_center().gap(px(10.));
        if let Some(avatar) = item.avatar {
            left = left.child(crate::ui::tab_strip::avatar(
                ("search-avatar", ix.section * 1000 + ix.row),
                avatar,
                AVATAR,
                cx,
            ));
        }
        let title = gpui_kit::SharedString::from(item.title.clone());
        let title = match match_range(&item.title, &self.query) {
            Some(range) => gpui_kit::StyledText::new(title).with_highlights([(
                range,
                gpui_kit::HighlightStyle {
                    font_weight: Some(FontWeight::SEMIBOLD),
                    color: Some(fg),
                    ..Default::default()
                },
            )]),
            None => gpui_kit::StyledText::new(title),
        };
        left = left.child(
            div()
                .flex_shrink(1.)
                .min_w(px(40.))
                .truncate()
                .text_color(match self.query.trim().is_empty() {
                    true => fg,
                    false => fg.opacity(0.78),
                })
                .child(title),
        );
        if let Some(subtitle) = item.subtitle.clone() {
            left = left.child(
                div().flex_1().min_w(px(24.)).flex().justify_end().child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_size(rems(ROW_META))
                        .text_color(muted)
                        .child(subtitle),
                ),
            );
        }

        let mut right = h_flex().flex_shrink_0().items_center().gap(px(8.));
        if let Some(note) = item.note.clone() {
            right = right.child(
                div()
                    .text_size(rems(ROW_META))
                    .text_color(muted)
                    .child(note),
            );
        }
        let gesture_hint = match &item.kind {
            kind if kind.edit_variant().is_some() => Some(t(L10nKey::EditHint)),
            // Only on the row the keyboard is on: every session row has the
            // same gesture, and a column of identical hints is noise.
            CommandKind::ResumeSession { .. } if picked && self.scope != Scope::SessionActions => {
                Some(t(L10nKey::SessionActionsHint))
            }
            _ => None,
        };
        if let Some(hint) = gesture_hint {
            right = right.child(
                h_flex()
                    .items_center()
                    .gap(px(6.))
                    .text_size(rems(ROW_META))
                    .text_color(muted)
                    .child(hint)
                    .child(crate::ui::dialog::chord(
                        crate::ui::keymap::key_tokens(EDIT_GESTURE),
                        cx,
                    )),
            );
        }
        if let Some(spec) = item.kind.key_spec(cx) {
            let tokens = crate::ui::keymap::key_tokens(&spec);
            right = right.child(
                h_flex()
                    .gap(px(3.))
                    .children(tokens.into_iter().map(|t| keycap(t, cx))),
            );
        }

        h_flex()
            .w_full()
            .gap(px(12.))
            .items_center()
            .justify_between()
            .child(left)
            .child(right)
            .into_any_element()
    }
}

impl ListDelegate for SearchDelegate {
    type Item = SearchRow;

    fn sections_count(&self, _cx: &App) -> usize {
        self.sections.len().max(1)
    }

    fn items_count(&self, section: usize, _cx: &App) -> usize {
        self.sections
            .get(section)
            .map(|s| s.rows.len())
            .unwrap_or(0)
    }

    fn perform_search(
        &mut self,
        query: &str,
        window: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) -> Task<()> {
        self.query = query.to_string();
        if self.scope == Scope::Tab(SearchTab::Symbols)
            && let Some(live) = self.catalog.live_query.clone()
        {
            live(query, cx);
        }
        self.refresh(cx);
        // Through `set_selected_index`, not by hand: the row index may not have
        // moved, but the row under it has, and the theme picker previews the
        // row — not the index.
        self.selected = None;
        let first = self.first_row();
        self.set_selected_index(first, window, cx);
        Task::ready(())
    }

    fn render_section_header(
        &mut self,
        section: usize,
        _window: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) -> Option<impl IntoElement> {
        let title = self.sections.get(section)?.title.clone()?;
        // The sidebar's group heading: medium, a half step under the rows, in
        // caption ink — never capitals, never a rule under it.
        Some(
            h_flex()
                .h(px(HEADER_H))
                .px(px(LABEL_INSET))
                .items_center()
                .text_size(rems(crate::ui::right_panel::HEADING))
                .font_weight(FontWeight::MEDIUM)
                .text_color(cx.theme().muted_foreground)
                .child(title),
        )
    }

    fn render_empty(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) -> impl IntoElement {
        // The SSH hint only where typing user@host would actually connect. A
        // theme picker with no matches teaching SSH is a crossed wire (#602).
        let mut headline = t(L10nKey::SearchNoResults);
        let hint = match self.scope {
            // An empty Files tab is rarely "nothing matches": say which of the
            // other reasons it is, since each has a different way out.
            Scope::Tab(SearchTab::Files) => match &self.catalog.files {
                FileList::NoRoots => {
                    headline = t(L10nKey::SearchFilesNoRoots);
                    t(L10nKey::SearchFilesNoRootsHint)
                }
                FileList::Indexing => {
                    headline = t(L10nKey::SearchFilesIndexing);
                    ""
                }
                FileList::Failed => t(L10nKey::SearchFilesFailed),
                FileList::Ready(_) => t(L10nKey::PaletteTryDifferentSearch),
            },
            Scope::Tab(SearchTab::All | SearchTab::Hosts) => t(L10nKey::ConnectSshHint),
            Scope::Tab(SearchTab::Sessions) if self.query.trim().is_empty() => {
                t(L10nKey::SearchSessionsEmptyHint)
            }
            Scope::Tab(SearchTab::Locations) if self.catalog.locations.is_empty() => {
                t(L10nKey::SearchLocationsNone)
            }
            Scope::Tab(SearchTab::Symbols) if self.catalog.symbols.is_empty() => {
                headline = t(L10nKey::SearchSymbolsNone);
                t(L10nKey::SearchSymbolsNoneHint)
            }
            _ => t(L10nKey::PaletteTryDifferentSearch),
        };
        // The headline in body ink and the way out under it in caption ink:
        // two greys of the same size read as one sentence cut in half.
        // Empty content follows the scope row in normal flow, sharing the
        // result rows' leading edge rather than reserving an overlay slot.
        let theme = cx.theme();
        v_flex()
            .pt(px(20.))
            .pb(px(24.))
            .px(px(LABEL_INSET))
            .gap(px(4.))
            .items_start()
            .text_size(rems(ROW_TEXT))
            .text_color(theme.foreground)
            .child(headline)
            .when(!hint.is_empty(), |d| {
                d.child(
                    div()
                        .text_size(rems(ROW_META))
                        .text_color(theme.muted_foreground)
                        .child(hint),
                )
            })
    }

    fn render_item(
        &mut self,
        ix: IndexPath,
        _window: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) -> Option<Self::Item> {
        let label: gpui_kit::SharedString = match self.row_at(ix)? {
            Row::Item(item) => match &item.subtitle {
                Some(sub) => format!("{}, {sub}", item.title).into(),
                None => item.title.clone().into(),
            },
            Row::More { tab, hidden } => t_fmt(
                L10nKey::SearchMoreIn,
                &[("count", &hidden.to_string()), ("tab", tab.title())],
            )
            .into(),
        };
        let content = match self.row_at(ix)? {
            Row::Item(item) => self.render_row(ix, item, cx),
            Row::More { tab, hidden } => h_flex()
                .w_full()
                .items_center()
                .justify_between()
                .text_color(cx.theme().muted_foreground)
                .child(t_fmt(
                    L10nKey::SearchMoreIn,
                    &[("count", &hidden.to_string()), ("tab", tab.title())],
                ))
                .child(keycap(crate::ui::keymap::key_tokens("tab").join(""), cx))
                .into_any_element(),
        };
        Some(SearchRow {
            id: ("search-row", ix.section * 1000 + ix.row).into(),
            label,
            selected: Some(ix) == self.selected,
            child: content,
        })
    }

    fn set_selected_index(
        &mut self,
        ix: Option<IndexPath>,
        window: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) {
        // After every search the list re-picks its row from the rows it drew
        // last frame, and when that frame drew none — the search had only just
        // opened, or the previous query found nothing — it picks none. Return
        // then did nothing, on a list with rows in it. Put the first row back
        // once the list is done.
        if ix.is_none()
            && let Some(first) = self.first_row()
        {
            cx.defer_in(window, move |state, window, cx| {
                if state.selected_index().is_none() {
                    state.set_selected_index(Some(first), window, cx);
                }
            });
        }
        let moved = self.selected != ix;
        self.selected = ix;
        // The list only emits `Select` for the arrow keys; a query that re-arms
        // the first row moves the highlight silently. The theme picker previews
        // whatever is highlighted, so it has to hear about both.
        if moved && let Some(ix) = ix {
            cx.emit(ListEvent::Select(ix));
        }
        cx.notify();
    }
}

pub enum SearchEvent {
    Confirm(CommandKind),
    /// Run this and leave the search open: what it changed is already
    /// reflected in the rows.
    RunInPlace(CommandKind),
    Dismiss,
    SessionError(lyrel_core::host::session_operations::SessionOperationError),
    /// Show the theme at this preset index without persisting it: the theme
    /// picker previews the highlighted row while it stays open.
    PreviewTheme(usize),
    /// Put back the theme that was live before the preview started.
    CancelThemePreview,
    /// Show this 0-based place in the file Go to Symbol is listing, leaving
    /// the keyboard in the search. Closing the search without a choice puts
    /// the caret back (`LyrelApp::close_search`).
    PreviewSymbol {
        line: u32,
        column: u32,
    },
    /// Move to another tab on the editor's row, which the app sets up.
    SwitchEditorTab(SearchTab),
    /// The same for a place a language server found (`ui::lsp`), which may
    /// be in another file.
    PreviewLocation {
        path: std::path::PathBuf,
        line: u32,
        column: u32,
    },
}

pub struct SearchView {
    list: Entity<ListState<SearchDelegate>>,
    // List 0.8.0 does not expose its input or a 40px search-row size.
    // Keep this input stable while List retains filtering and navigation.
    query_input: Entity<InputState>,
    catalog: Rc<Catalog>,
    /// The tab showing — or, in the theme picker, the one Escape returns to.
    tab: SearchTab,
    /// What was typed when a row's list (the theme picker, a session's
    /// actions) opened, put back when it closes.
    parked_query: Option<String>,
    /// Preset index the theme picker is currently previewing, so the same
    /// theme is not re-applied on every redundant selection event.
    previewing: Option<usize>,
    /// How many session lists have arrived since the search opened. A test
    /// waits on it: the scan runs on a real thread and lands when it lands.
    sessions_landed: usize,
    session_generation: u64,
    retired: bool,
    /// The symbol Go to Symbol opened on — the one around the caret — which
    /// is not previewed until the highlight has moved: opening the picker
    /// must not move the caret.
    symbol_initial: Option<CommandKind>,
    symbol_moved: bool,
    /// What a language server's list is of — References, Definitions — shown
    /// where the scope row would be. The other standalone tabs are named by
    /// their title.
    heading: Option<&'static str>,
    /// The editor's row as this window can fill it (`SearchTab::EDITOR_ORDER`
    /// less what cannot answer), set by whoever opened an editor tab.
    editor_tabs: Vec<SearchTab>,
    _sub: Subscription,
    _query_sub: Subscription,
}

impl SearchView {
    /// The search, on `tab`, with `query` already typed.
    ///
    /// The seed goes through the search field rather than around it, so what
    /// the reader sees is a search in the state they would have typed it into:
    /// the text is there, rows are ranked against it, and the next keystroke
    /// goes on refining instead of starting over.
    pub fn new(
        catalog: Catalog,
        tab: SearchTab,
        query: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let catalog = Rc::new(catalog);
        let delegate = SearchDelegate::new(catalog.clone(), Scope::Tab(tab), Vec::new(), cx);
        let list = Self::build_list(delegate, None, window, cx);
        let query_input = cx.new(|cx| InputState::new(window, cx));
        let _query_sub = cx.subscribe_in(&query_input, window, |this, input, event, window, cx| {
            if matches!(event, InputEvent::Change) {
                let query = input.read(cx).value();
                this.set_query(&query, window, cx);
            }
        });
        let _sub = cx.subscribe_in(&list, window, Self::on_list_event);
        let mut this = Self {
            list,
            query_input,
            catalog,
            tab,
            parked_query: None,
            previewing: None,
            sessions_landed: 0,
            session_generation: 0,
            retired: false,
            symbol_initial: None,
            symbol_moved: false,
            heading: None,
            editor_tabs: Vec::new(),
            _sub,
            _query_sub,
        };
        this.set_query(query, window, cx);
        this.focus(window, cx);
        this
    }

    /// Puts the keyboard back in the search field (`palette`).
    pub(crate) fn focus(&self, window: &mut Window, cx: &mut App) {
        self.query_input
            .update(cx, |input, cx| input.focus(window, cx));
    }

    fn set_query(&mut self, query: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.query_input.read(cx).value() != query {
            self.query_input
                .update(cx, |input, cx| input.set_value(query, window, cx));
        }
        if self.list.read(cx).delegate().query != query.trim() {
            self.list
                .update(cx, |list, cx| list.set_query(query, window, cx));
        }
        cx.notify();
    }

    fn dispatch_list_action(&self, action: &dyn Action, window: &mut Window, cx: &mut App) {
        self.list
            .focus_handle(cx)
            .dispatch_action(action, window, cx);
    }

    /// Whether a row's list is showing rather than a tab.
    fn in_sub_list(&self) -> bool {
        self.parked_query.is_some()
    }

    fn scope(&self, cx: &App) -> Scope {
        self.list.read(cx).delegate().scope
    }

    fn build_list(
        delegate: SearchDelegate,
        selected: Option<IndexPath>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<ListState<SearchDelegate>> {
        let first = selected.or_else(|| delegate.first_row());
        let list = cx.new(|cx| ListState::new(delegate, window, cx));
        list.update(cx, |state, cx| {
            // `ListState::new` starts with nothing selected, and it only picks a
            // row once a query changes. Opening the search and pressing Return
            // therefore did nothing at all, and until then no row showed what
            // Return was aimed at.
            state.set_selected_index(first, window, cx);
            if selected.is_some() {
                state.scroll_to_selected_item(window, cx);
            }
        });
        list
    }

    fn replace_list(
        &mut self,
        list: Entity<ListState<SearchDelegate>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self._sub = cx.subscribe_in(&list, window, Self::on_list_event);
        self.list = list;
        let query = self.list.read(cx).delegate().query.clone();
        self.set_query(&query, window, cx);
        self.focus(window, cx);
        cx.notify();
    }

    /// Shows `tab`, keeping what is typed — or replacing it with `query`.
    ///
    /// In place, on the same list: the field keeps its text and focus, and
    /// only the rows under it change.
    pub(crate) fn set_tab(
        &mut self,
        tab: SearchTab,
        query: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.in_sub_list() {
            return;
        }
        self.tab = tab;
        if let Some(query) = query {
            self.set_query(query, window, cx);
        }
        self.list.update(cx, |state, cx| {
            state.delegate_mut().scope = Scope::Tab(tab);
            // `set_query` searches only when the text changed; the tab did.
            state.delegate_mut().refresh(cx);
            let first = state.delegate().first_row();
            state.set_selected_index(first, window, cx);
            state.scroll_to_item(IndexPath::default(), ScrollStrategy::Top, window, cx);
        });
        cx.notify();
    }

    pub(crate) fn refresh_sessions(
        &mut self,
        query: super::history::SessionQuery,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.session_generation += 1;
        query.start(self.session_generation, window, cx);
    }

    pub(super) fn accepts_session_query(&self, generation: u64) -> bool {
        !self.retired && self.session_generation == generation
    }

    pub(super) fn retire(&mut self) {
        self.retired = true;
        self.session_generation += 1;
    }

    /// The Sessions tab's rows, arrived from a scan that finished after the
    /// search opened.
    pub(crate) fn set_sessions(
        &mut self,
        sessions: Vec<Item>,
        here: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.sessions_landed += 1;
        self.update_catalog(
            |catalog| {
                catalog.sessions = sessions;
                catalog.sessions_here = here;
            },
            window,
            cx,
        );
    }

    /// The Files tab's list, arrived from a walk that finished after the
    /// search opened.
    pub(crate) fn set_files(
        &mut self,
        files: FileList,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.update_catalog(|catalog| catalog.files = files, window, cx);
    }

    /// Go to Symbol's rows, with the highlight on `selected` — the symbol the
    /// caret is in — when it is one of them.
    pub(crate) fn set_symbols(
        &mut self,
        symbols: Vec<(usize, Item)>,
        selected: Option<CommandKind>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.symbol_moved = false;
        self.symbol_initial = selected.clone();
        self.update_catalog(|catalog| catalog.symbols = symbols, window, cx);
        if let Some(kind) = selected {
            self.list.update(cx, |state, cx| {
                if let Some(ix) = state.delegate().position_of(&kind) {
                    state.set_selected_index(Some(ix), window, cx);
                    state.scroll_to_selected_item(window, cx);
                }
            });
        }
    }

    /// Find References' rows (`ui::lsp`). The first is highlighted and not
    /// previewed — opening the list must not move the caret — until the
    /// highlight moves.
    pub(crate) fn set_locations(
        &mut self,
        heading: Option<&'static str>,
        locations: Vec<Item>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.heading = heading;
        self.symbol_moved = false;
        self.symbol_initial = locations.first().map(|item| item.kind.clone());
        self.update_catalog(|catalog| catalog.locations = locations, window, cx);
    }

    /// The editor tabs this window can offer, for the editor's row.
    pub(crate) fn set_editor_tabs(&mut self, tabs: Vec<SearchTab>, cx: &mut Context<Self>) {
        self.editor_tabs = tabs;
        cx.notify();
    }

    /// The row over the list: the window's, the editor's (when it has more
    /// than one tab to offer), or none — the tab then stands alone under its
    /// name.
    fn scope_row(&self) -> Option<Vec<SearchTab>> {
        if !self.tab.stands_alone() {
            return Some(SearchTab::ORDER.to_vec());
        }
        (self.tab.in_editor_row() && self.editor_tabs.len() > 1).then(|| self.editor_tabs.clone())
    }

    /// Moves to `tab` on whichever row it is on. The editor's tabs each need
    /// the app to set them up (an outline, a server to ask), so those go by
    /// way of it.
    fn go_to_tab(&mut self, tab: SearchTab, window: &mut Window, cx: &mut Context<Self>) {
        if tab == self.tab {
            return;
        }
        if tab.in_editor_row() {
            cx.emit(SearchEvent::SwitchEditorTab(tab));
        } else {
            self.set_tab(tab, None, window, cx);
        }
    }

    /// What the language server found across the project for the Symbols
    /// tab's current query, listed after the file's own symbols.
    pub(crate) fn set_project_symbols(
        &mut self,
        symbols: Vec<Item>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.update_catalog(|catalog| catalog.project_symbols = symbols, window, cx);
    }

    /// Sets what the Symbols tab asks the language server as the query
    /// changes.
    pub(crate) fn set_live_query(
        &mut self,
        live: crate::ui::search::LiveQuery,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.update_catalog(|catalog| catalog.live_query = Some(live), window, cx);
    }

    /// Changes part of the catalog under an open list. The highlight stays on
    /// the row it was on when that row is still there, so a list that fills in
    /// under the cursor does not move what Return runs.
    fn update_catalog(
        &mut self,
        change: impl FnOnce(&mut Catalog),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut catalog = (*self.catalog).clone();
        change(&mut catalog);
        self.catalog = Rc::new(catalog);
        if self.in_sub_list() {
            return;
        }
        let catalog = self.catalog.clone();
        self.list.update(cx, |state, cx| {
            let before = state.delegate().selected_item().map(|i| i.kind.clone());
            let delegate = state.delegate_mut();
            delegate.catalog = catalog;
            delegate.refresh(cx);
            let keep = before.and_then(|kind| delegate.position_of(&kind));
            let target = keep.or_else(|| state.delegate().first_row());
            state.set_selected_index(target, window, cx);
        });
        cx.notify();
    }

    #[cfg(test)]
    pub(crate) fn symbol_count(&self) -> usize {
        self.catalog.symbols.len()
    }

    /// The tab showing — or, in a row's own list, the one Escape returns to.
    pub(crate) fn tab(&self) -> SearchTab {
        self.tab
    }

    fn step_tab(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(row) = self.scope_row() else { return };
        let n = row.len();
        let i = row.iter().position(|t| *t == self.tab).unwrap_or(0);
        let next = row[if forward {
            (i + 1) % n
        } else {
            (i + n - 1) % n
        }];
        self.go_to_tab(next, window, cx);
    }

    fn open_themes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Open on the theme already in use: the picker previews the
        // highlighted row, and merely opening it must not change what the
        // window looks like.
        self.previewing = Item::active_theme_index(cx);
        let selected = self.previewing.map(IndexPath::new);
        self.open_sub_list(Scope::Themes, Item::themes(cx), selected, window, cx);
    }

    fn open_sub_list(
        &mut self,
        scope: Scope,
        items: Vec<Item>,
        selected: Option<IndexPath>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.parked_query = Some(self.list.read(cx).delegate().query.clone());
        let delegate = SearchDelegate::new(self.catalog.clone(), scope, items, cx);
        let list = Self::build_list(delegate, selected, window, cx);
        self.replace_list(list, window, cx);
    }

    fn close_sub_list(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Backing out of the theme picker is not a choice: whatever was
        // previewed goes back to what it was.
        if self.scope(cx) == Scope::Themes {
            self.previewing = None;
            cx.emit(SearchEvent::CancelThemePreview);
        }
        let query = self.parked_query.take().unwrap_or_default();
        let delegate =
            SearchDelegate::new(self.catalog.clone(), Scope::Tab(self.tab), Vec::new(), cx);
        let list = Self::build_list(delegate, None, window, cx);
        if !query.is_empty() {
            list.update(cx, |state, cx| state.set_query(&query, window, cx));
        }
        self.replace_list(list, window, cx);
    }

    fn selected_edit_command(&self, cx: &App) -> Option<CommandKind> {
        self.list
            .read(cx)
            .delegate()
            .selected_item()
            .and_then(|item| item.kind.edit_variant())
    }

    /// The edit gesture on the selected row: edit it, or open what can be
    /// done with it. `false` when the row has neither.
    fn on_edit_gesture(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.in_sub_list() {
            return false;
        }
        if let Some(edit) = self.selected_edit_command(cx) {
            cx.emit(SearchEvent::Confirm(edit));
            return true;
        }
        let kind = self
            .list
            .read(cx)
            .delegate()
            .selected_item()
            .map(|item| item.kind.clone());
        let Some(actions) = kind.and_then(|kind| Item::session_actions(&kind, cx)) else {
            return false;
        };
        self.open_sub_list(Scope::SessionActions, actions, None, window, cx);
        true
    }

    /// Take a hidden session's row out of the lists at once, rather than
    /// waiting for the next scan to leave it out.
    fn drop_session(&mut self, agent: crate::core::cli_agent::CLIAgent, id: &str) {
        let mut catalog = (*self.catalog).clone();
        let at = catalog.sessions.iter().position(|item| {
            matches!(&item.kind, CommandKind::ResumeSession { agent: a, session_id, .. }
                if *a == agent && session_id == id)
        });
        let Some(at) = at else { return };
        catalog.sessions.remove(at);
        if at < catalog.sessions_here {
            catalog.sessions_here -= 1;
        }
        self.catalog = Rc::new(catalog);
    }

    fn on_list_event(
        &mut self,
        list: &Entity<ListState<SearchDelegate>>,
        ev: &ListEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match ev {
            ListEvent::Confirm(ix) => {
                let row = list.read(cx).delegate().row_at(*ix).cloned();
                match row {
                    Some(Row::More { tab, .. }) => self.set_tab(tab, None, window, cx),
                    Some(Row::Item(item)) => match item.kind {
                        CommandKind::OpenThemePicker => self.open_themes(window, cx),
                        CommandKind::HideSession { agent, session_id } => {
                            self.drop_session(agent, &session_id);
                            self.close_sub_list(window, cx);
                            cx.emit(SearchEvent::RunInPlace(CommandKind::HideSession {
                                agent,
                                session_id,
                            }));
                        }
                        // What was typed found this row; it is not an address.
                        CommandKind::SearchHosts => {
                            self.set_tab(SearchTab::Hosts, Some(""), window, cx)
                        }
                        CommandKind::QuickOpenFile => {
                            self.set_tab(SearchTab::Files, Some(""), window, cx)
                        }
                        kind => cx.emit(SearchEvent::Confirm(kind)),
                    },
                    None => cx.emit(SearchEvent::Dismiss),
                }
            }
            ListEvent::Cancel => match self.in_sub_list() {
                true => self.close_sub_list(window, cx),
                false => cx.emit(SearchEvent::Dismiss),
            },
            ListEvent::Select(ix) => {
                if !self.in_sub_list()
                    && self.tab == SearchTab::Symbols
                    && let Some(Row::Item(item)) = list.read(cx).delegate().row_at(*ix)
                    && let CommandKind::GoToSymbol { line, column } = item.kind
                {
                    if self.symbol_moved || self.symbol_initial.as_ref() != Some(&item.kind) {
                        self.symbol_moved = true;
                        cx.emit(SearchEvent::PreviewSymbol { line, column });
                    }
                    return;
                }
                if !self.in_sub_list()
                    && matches!(self.tab, SearchTab::Locations | SearchTab::Symbols)
                    && let Some(Row::Item(item)) = list.read(cx).delegate().row_at(*ix)
                    && let CommandKind::GoToLocation { path, line, column } = &item.kind
                {
                    if self.symbol_moved || self.symbol_initial.as_ref() != Some(&item.kind) {
                        self.symbol_moved = true;
                        cx.emit(SearchEvent::PreviewLocation {
                            path: path.clone(),
                            line: *line,
                            column: *column,
                        });
                    }
                    return;
                }
                if self.in_sub_list()
                    && let Some(Row::Item(item)) = list.read(cx).delegate().row_at(*ix)
                    && let CommandKind::SetTheme(i) = item.kind
                    && self.previewing != Some(i)
                {
                    self.previewing = Some(i);
                    cx.emit(SearchEvent::PreviewTheme(i));
                }
            }
        }
    }

    /// The scope row, under the search field: plain words, the one showing
    /// on the popover's selected step. Go to File has no pill of its own —
    /// it is a chord, not a scope — so there none is lit.
    fn render_tabs(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let sf = cx.global::<crate::ui::presets::Surfaces>().popover;
        let (active_bg, hover_bg) = (gpui_kit::rgb(sf.selected), gpui_kit::rgb(sf.hover));
        let theme = cx.theme();
        let (fg, muted) = (theme.foreground, theme.muted_foreground);
        h_flex()
            .h(px(TABS_H))
            .px(px(SCOPE_PAD))
            .pb(px(6.))
            .gap(px(2.))
            .items_center()
            .bg(theme.popover)
            .border_b_1()
            .border_color(theme.border)
            .when(self.scope_row().is_none(), |row| {
                // Go to File's rows are a capped sample of the project, so a
                // count there would read as the size of something it is not.
                let count = match self.tab {
                    SearchTab::Files => 0,
                    _ => self.list.read(cx).delegate().row_count(),
                };
                let heading = match self.tab {
                    SearchTab::Locations => self.heading.unwrap_or_else(|| self.tab.title()),
                    _ => self.tab.title(),
                };
                row.child(
                    div()
                        .h(px(24.))
                        .px(px(9.))
                        .flex()
                        .items_center()
                        .rounded(px(6.))
                        .bg(active_bg)
                        .text_size(rems(SCOPE_TEXT))
                        .text_color(fg)
                        .font_weight(FontWeight::MEDIUM)
                        .child(heading),
                )
                .when(count > 0, |row| {
                    row.child(
                        div()
                            .px(px(6.))
                            .text_size(rems(SCOPE_TEXT))
                            .text_color(muted)
                            .child(count.to_string()),
                    )
                })
            })
            .when_some(self.scope_row(), |row, tabs| {
                row.children(tabs.into_iter().enumerate().map(|(i, tab)| {
                    let active = tab == self.tab;
                    div()
                        .id(("search-tab", i))
                        .h(px(24.))
                        .px(px(9.))
                        .flex()
                        .items_center()
                        .rounded(px(6.))
                        .text_size(rems(SCOPE_TEXT))
                        .cursor_pointer()
                        .map(|d| match active {
                            true => d
                                .bg(active_bg)
                                .text_color(fg)
                                .font_weight(FontWeight::MEDIUM),
                            false => d
                                .text_color(muted)
                                .hover(move |d| d.bg(hover_bg).text_color(fg)),
                        })
                        .child(tab.title())
                        .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                            this.go_to_tab(tab, window, cx);
                            this.focus(window, cx);
                        }))
                }))
            })
    }

    /// The switcher's footer, minus its New workspace button: the keys that
    /// move and run, as caps with a word after each.
    fn render_footer(&self, cx: &App) -> impl IntoElement + use<> {
        let theme = cx.theme();
        let hint = |keys: Vec<gpui_kit::AnyElement>, label: &'static str| {
            h_flex()
                .items_center()
                .gap(px(6.))
                .children(keys)
                .child(label)
        };
        let tabs = !self.in_sub_list() && self.scope_row().is_some();
        h_flex()
            .id("search-footer")
            .test_support()
            .flex_none()
            .items_center()
            .gap(px(16.))
            .h(px(FOOTER_H))
            .px(px(16.))
            .border_t_1()
            .border_color(theme.border)
            .overflow_hidden()
            .text_size(rems(ROW_META))
            .text_color(theme.muted_foreground)
            .when(tabs, |row| {
                row.child(hint(
                    vec![keycap(crate::ui::keymap::key_tokens("tab").join(""), cx)],
                    t(L10nKey::SearchHintNextScope),
                ))
            })
            .child(div().flex_1())
            .child(hint(
                vec![keycap("↑", cx), keycap("↓", cx)],
                t(L10nKey::SwitcherHintNavigate),
            ))
            .child(hint(vec![keycap("↵", cx)], t(L10nKey::SwitcherHintOpen)))
    }
}

impl EventEmitter<SearchEvent> for SearchView {}

// The search is the workspace switcher's sibling (see `switcher.rs`): the
// same card, the same corner, the same keycaps and footer, and the same
// neutral step for the row the keyboard is on. The numbers below are the ones
// the switcher uses wherever the two overlays show the same thing, so opening
// one after the other does not move the chrome.

/// How tall a row is. One line where the switcher's rows carry two; 32 keeps
/// an 18px keycap clear of the row's edges by the same 7px the switcher's
/// footer gives its own.
const ROW_H: f32 = 32.;

/// A section heading: a compact caption over its rows.
const HEADER_H: f32 = 24.;

/// How far the list sits in from the card's edges, and how far a row's text
/// then sits in from its own fill — the switcher's `COLUMN_PAD` and
/// `ROW_PAD`.
const LIST_PAD: f32 = 8.;
const ROW_PAD: f32 = 10.;

/// The scope row's inset, and its words: a step under the rows.
const SCOPE_PAD: f32 = 12.;
const SCOPE_TEXT: f32 = 12. / 16.;

/// Where a row's label starts, so a section heading lands on the same column
/// as the rows under it. A heading has neither the row's inset nor its
/// padding, so it carries the sum.
const LABEL_INSET: f32 = LIST_PAD + ROW_PAD;

/// The row fill's corner.
const ROW_RADIUS: f32 = 7.;

/// The card's breathing room from the window's bottom edge, and its width.
/// Its corner and its footer are `ui::dialog`'s, the switcher's numbers.
const CARD_MARGIN: f32 = 24.;
const CARD_MAX_W: f32 = 620.;

/// A row's title, and the subtitle, hints and keycaps beside it: the
/// switcher's row name and its metadata line.
const ROW_TEXT: f32 = 13. / 16.;
const ROW_META: f32 = 11.5 / 16.;

const AVATAR: f32 = 18.;

/// The scope row's height with its padding and rule, reserved out of the
/// list's.
const TABS_H: f32 = 34.;

/// Where `query` first appears in `title`, ignoring case: the stretch of the
/// title the row picks out. `None` for an empty query, or one the fuzzy
/// scorer matched as scattered letters — picking out stray letters reads as
/// noise, not as an answer.
fn match_range(title: &str, query: &str) -> Option<std::ops::Range<usize>> {
    let query = query.trim();
    if query.is_empty() {
        return None;
    }
    let lower = |c: char| c.to_lowercase().next().unwrap_or(c);
    let needle: Vec<char> = query.chars().map(lower).collect();
    let hay: Vec<(usize, char)> = title.char_indices().collect();
    (0..hay.len()).find_map(|i| {
        let window = hay.get(i..i + needle.len())?;
        window
            .iter()
            .zip(&needle)
            .all(|((_, c), n)| lower(*c) == *n)
            .then(|| {
                let end = hay.get(i + needle.len()).map_or(title.len(), |(b, _)| *b);
                window[0].0..end
            })
    })
}

/// The chord that opens the selected row for editing instead of running it.
///
/// It cannot be `→`: gpui-component's `Input` binds bare `right` to MoveRight
/// in its own key context, so with the query field focused the search never
/// sees the key — the old `→ edit` badge was advertising a gesture that could
/// not fire. `⌘↵` is no better; the app binds it to ToggleFullscreen, which
/// wins for the same reason. `secondary-e` is claimed by neither.
const EDIT_GESTURE: &str = "secondary-e";

/// Matches `EDIT_GESTURE` against a live keystroke. Keep the two in step.
fn is_edit_gesture(ks: &gpui_kit::Keystroke) -> bool {
    if ks.key != "e" {
        return false;
    }
    let m = &ks.modifiers;
    let secondary = if cfg!(target_os = "macos") {
        m.platform
    } else {
        m.control
    };
    secondary && !m.shift && !m.alt
}

const VISIBLE_ROWS: f32 = 12.;

/// The key context the search's own bindings (Tab, Shift-Tab) live in.
pub(crate) const KEY_CONTEXT: &str = "Search";

impl Render for SearchView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let scrim = crate::ui::presets::scrim_fill(cx);
        let tabs = !self.in_sub_list();

        let viewport = window.viewport_size();
        // The switcher's drop from the top on a full-size window; a short one
        // still brings the card up to keep its rows.
        let top = (viewport.height.as_f32() * 0.16).clamp(16., crate::ui::switcher::CARD_TOP);
        // Reserve space for the tab row, the search row, the footer and the
        // card's margin, including when a split-screen window is shorter than
        // the full list.
        let tabs_h = if tabs { TABS_H } else { 0. };
        let chrome = tabs_h + FOOTER_H * 2. + CARD_MARGIN + 2.;
        let list_max_h = px((viewport.height.as_f32() - top - chrome)
            .max(ROW_H)
            .min(ROW_H * VISIBLE_ROWS + LIST_PAD * 2.));
        let typed = !self.query_input.read(cx).value().is_empty();
        let placeholder = match self.scope(cx) {
            Scope::Tab(_) => self.tab.placeholder(),
            Scope::Themes => t(L10nKey::SearchTheme),
            Scope::SessionActions => t(L10nKey::SearchSessionActions),
        };
        self.query_input.update(cx, |input, cx| {
            input.set_placeholder(placeholder, window, cx);
        });
        let card = v_flex()
            .id("search-card")
            .test_support()
            .relative()
            .w(px((viewport.width.as_f32() - 32.).clamp(0., CARD_MAX_W)))
            .map(|panel| crate::ui::theme::floating_surface(panel, cx))
            .rounded(px(CARD_RADIUS))
            .overflow_hidden()
            .child(
                h_flex()
                    .id("search-header")
                    .test_support()
                    .key_context("List")
                    .flex_none()
                    .h(px(FOOTER_H))
                    .px(px(8.))
                    .border_b_1()
                    .border_color(cx.theme().border)
                    // Keep the framework's list navigation without moving focus
                    // out of the input. These handlers belong only to this row:
                    // the list's propagated Cancel must not loop back into them.
                    .on_action(cx.listener(|this, action: &SelectUp, window, cx| {
                        this.dispatch_list_action(action, window, cx);
                    }))
                    .on_action(cx.listener(|this, action: &SelectDown, window, cx| {
                        this.dispatch_list_action(action, window, cx);
                    }))
                    .on_action(cx.listener(|this, action: &Confirm, window, cx| {
                        this.dispatch_list_action(action, window, cx);
                    }))
                    .on_action(cx.listener(|this, action: &Cancel, window, cx| {
                        this.dispatch_list_action(action, window, cx);
                    }))
                    .child(
                        Input::new(&self.query_input)
                            .id("search-input")
                            .prefix(
                                Icon::new(IconName::Search).text_color(cx.theme().muted_foreground),
                            )
                            .cleanable(true)
                            .appearance(false)
                            .p_0()
                            // Input::h_full is multi-line-only; use Styled for
                            // the single-line frame, with its normal text size.
                            .map(gpui_kit::Styled::h_full)
                            .when(!typed, |input| {
                                input.suffix(
                                    div()
                                        .id("search-escape")
                                        .test_support()
                                        .child(keycap("esc", cx)),
                                )
                            }),
                    ),
            )
            .when(tabs, |card| card.child(self.render_tabs(cx)))
            .child(
                List::new(&self.list)
                    .pt(px(6.))
                    .pb(px(LIST_PAD))
                    .max_h(list_max_h),
            )
            .child(self.render_footer(cx));

        div()
            .absolute()
            .inset_0()
            .flex()
            .items_start()
            .justify_center()
            .pt(px(top))
            .bg(scrim)
            .key_context(KEY_CONTEXT)
            .on_action(
                cx.listener(|this, _: &SearchNextTab, window, cx| this.step_tab(true, window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &SearchPrevTab, window, cx| this.step_tab(false, window, cx)),
            )
            // Go to Symbol's chord puts Go to Symbol away, as Go to File's
            // does; anywhere else in the search it is the switcher's.
            .on_action(cx.listener(
                |this, _: &crate::core::actions::EditorGoToSymbol, _window, cx| {
                    if this.tab == SearchTab::Symbols && !this.in_sub_list() {
                        cx.emit(SearchEvent::Dismiss);
                    } else {
                        cx.propagate();
                    }
                },
            ))
            .on_key_down(
                cx.listener(|this, ev: &gpui_kit::KeyDownEvent, window, cx| {
                    if is_edit_gesture(&ev.keystroke) && this.on_edit_gesture(window, cx) {
                        cx.stop_propagation();
                    }
                }),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_this, _: &MouseDownEvent, _window, cx| {
                    // The scrim covers the title bar's search button too. A
                    // dismissing press is spent on the dismissal — the
                    // switcher's rule — or it carried on to that button and
                    // opened the search straight back up, so clicking it a
                    // second time looked like it did nothing.
                    cx.stop_propagation();
                    cx.emit(SearchEvent::Dismiss);
                }),
            )
            .child(div().occlude().child(card))
    }
}

/// A search row.
///
/// Its own element rather than gpui-component's `ListItem`, which paints the
/// keyboard row in the theme's `list_active` — the accent wash menus use. The
/// search's keyboard row is where the cursor is, not an action, so it takes
/// the popover's neutral selected step the switcher's rows do. The list moves
/// the selection under the pointer, so hover and selection are one bar; the
/// hover step here only shows while a row is under the pointer and the
/// selection has not caught up with it.
#[derive(IntoElement)]
pub struct SearchRow {
    id: gpui_kit::ElementId,
    /// What a screen reader reads for the row: its title, and its subtitle
    /// when there is one.
    label: gpui_kit::SharedString,
    selected: bool,
    child: gpui_kit::AnyElement,
}

impl Selectable for SearchRow {
    fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    fn is_selected(&self) -> bool {
        self.selected
    }
}

impl RenderOnce for SearchRow {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let sf = cx.global::<crate::ui::presets::Surfaces>().popover;
        let (hover, picked) = (gpui_kit::rgb(sf.hover), gpui_kit::rgb(sf.selected));
        h_flex()
            .id(self.id)
            .role(gpui_kit::Role::ListBoxOption)
            .aria_label(self.label)
            .aria_selected(self.selected)
            .items_center()
            .flex_none()
            .h(px(ROW_H))
            .mx(px(LIST_PAD))
            .px(px(ROW_PAD))
            .rounded(px(ROW_RADIUS))
            .overflow_hidden()
            .cursor_pointer()
            .text_size(rems(ROW_TEXT))
            .text_color(cx.theme().foreground)
            .when(self.selected, |r| r.bg(picked))
            .when(!self.selected, |r| r.hover(move |s| s.bg(hover)))
            .child(self.child)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::app::{LyrelApp, test_window::harness_with_tabs};
    use gpui_kit::test::TestWindowExt as _;
    use gpui_kit::{TestAppContext, VisualTestContext};

    fn open(app: &Entity<LyrelApp>, vcx: &mut VisualTestContext) -> Entity<SearchView> {
        app.read_with(vcx, |app, _| app.search.view().expect("the search is open"))
    }

    fn first_kind(view: &Entity<SearchView>, vcx: &mut VisualTestContext) -> Option<CommandKind> {
        view.read_with(vcx, |view, cx| {
            let delegate = view.list.read(cx).delegate();
            Some(delegate.row_at(delegate.first_row()?)?.item()?.kind.clone())
        })
    }

    #[gpui_kit::test]
    fn search_header_matches_footer_and_centers_escape_after_layout(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        app.update_in(&mut vcx, |app, window, cx| {
            app.open_search(SearchTab::Actions, "", window, cx)
        });
        for (font_size, scale, width, height) in [
            (14., 1., 900., 700.),
            (18., 1.25, 640., 480.),
            (22., 1.5, 520., 300.),
        ] {
            vcx.simulate_resize(gpui_kit::size(px(width), px(height)));
            vcx.simulate_scale_factor_change(scale);
            vcx.update(|_, cx| {
                cx.global_mut::<crate::core::config::Config>().ui_font_size = font_size;
            });
            vcx.run_until_parked();
            vcx.update(|window, cx| {
                window.render_frame(cx);
                let header = window.find("search-header").bounds();
                let footer = window.find("search-footer").bounds();
                let input = window.find("search-input").bounds();
                let escape = window.find("search-escape").bounds();
                let card = window.find("search-card").bounds();
                assert_eq!(header.size.height, px(40.));
                assert_eq!(header.size.height, footer.size.height);
                assert!((input.top() - header.top()).abs() * scale <= px(0.5));
                assert!((input.bottom() - header.bottom() + px(1.)).abs() * scale <= px(0.5));
                assert!(
                    (escape.center().y - input.center().y).abs() <= px(0.5),
                    "font={font_size}, scale={scale}, header={header:?}, input={input:?}, escape={escape:?}"
                );
                assert!((escape.center().y - header.center().y).abs() * scale <= px(1.));
                assert!(
                    card.bottom() <= window.viewport_size().height - px(CARD_MARGIN),
                    "font={font_size}, scale={scale}, card={card:?}, viewport={:?}",
                    window.viewport_size()
                );
                assert_eq!(window.find("search-input").focused(), Some(true));
            });
        }
    }

    #[gpui_kit::test]
    fn composed_input_keeps_list_navigation_clear_and_sublist_escape(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        app.update_in(&mut vcx, |app, window, cx| {
            app.open_search(SearchTab::Actions, "", window, cx)
        });
        vcx.run_until_parked();
        let view = open(&app, &mut vcx);
        let selected = view.read_with(&vcx, |view, cx| view.list.read(cx).selected_index());
        vcx.simulate_keystrokes("down");
        vcx.run_until_parked();
        assert_ne!(
            view.read_with(&vcx, |view, cx| view.list.read(cx).selected_index()),
            selected
        );
        vcx.simulate_keystrokes("up");
        vcx.run_until_parked();
        assert_eq!(
            view.read_with(&vcx, |view, cx| view.list.read(cx).selected_index()),
            selected
        );

        vcx.simulate_input("split right");
        vcx.run_until_parked();
        assert_eq!(first_kind(&view, &mut vcx), Some(CommandKind::SplitRight));
        vcx.update(|window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("search-escape").is_none());
            window.within("search-input").click("clean", cx);
        });
        vcx.run_until_parked();
        view.read_with(&vcx, |view, cx| {
            assert_eq!(view.query_input.read(cx).value(), "");
            assert_eq!(view.list.read(cx).delegate().query, "");
        });

        view.update_in(&mut vcx, |view, window, cx| {
            view.set_query("theme", window, cx);
            view.open_themes(window, cx);
        });
        vcx.run_until_parked();
        view.read_with(&vcx, |view, cx| {
            assert_eq!(view.scope(cx), Scope::Themes);
            assert_eq!(view.query_input.read(cx).value(), "");
        });
        vcx.simulate_keystrokes("escape");
        vcx.run_until_parked();
        view.read_with(&vcx, |view, cx| {
            assert_eq!(view.scope(cx), Scope::Tab(SearchTab::Actions));
            assert_eq!(view.query_input.read(cx).value(), "theme");
            assert_eq!(view.list.read(cx).delegate().query, "theme");
        });
        vcx.update(|window, cx| {
            window.render_frame(cx);
            assert_eq!(window.find("search-input").focused(), Some(true));
        });
        vcx.simulate_keystrokes("escape");
        vcx.run_until_parked();
        assert!(app.read_with(&vcx, |app, _| !app.search.is_open()));
    }

    #[gpui_kit::test]
    fn tab_walks_the_tabs_and_keeps_what_was_typed(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 2);
        crate::ui::i18n::set_locale("en");
        app.update_in(&mut vcx, |app, window, cx| {
            app.open_search(SearchTab::All, "split right", window, cx)
        });
        vcx.run_until_parked();
        let view = open(&app, &mut vcx);

        // Backwards round the end, to Commands, which comes last.
        vcx.simulate_keystrokes("shift-tab");
        vcx.run_until_parked();
        view.read_with(&vcx, |view, cx| {
            assert_eq!(view.tab, SearchTab::Actions);
            assert_eq!(view.list.read(cx).delegate().query, "split right");
        });
        assert_eq!(first_kind(&view, &mut vcx), Some(CommandKind::SplitRight));

        // Forwards, round the end again, and never through Files.
        vcx.simulate_keystrokes("tab tab tab");
        vcx.run_until_parked();
        view.read_with(&vcx, |view, _| assert_eq!(view.tab, SearchTab::Sessions));
        assert!(
            app.read_with(&vcx, |app, _| app.search.is_open()),
            "Tab stays inside the search instead of walking focus out of it"
        );
    }

    /// Opened with nothing typed, Return goes back to the tab you were last
    /// in — the All tab leads with this window's other tabs.
    #[gpui_kit::test]
    fn return_on_an_empty_search_goes_to_the_previous_tab(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 3);
        app.update_in(&mut vcx, |app, window, cx| {
            app.activate(1, window, cx);
            app.tabs.touch_active();
            app.activate(2, window, cx);
            app.tabs.touch_active();
            app.activate(0, window, cx);
            app.tabs.touch_active();
        });
        app.update_in(&mut vcx, |app, window, cx| {
            app.open_search(SearchTab::All, "", window, cx)
        });
        vcx.run_until_parked();
        let view = open(&app, &mut vcx);
        assert!(
            matches!(
                first_kind(&view, &mut vcx),
                Some(CommandKind::GoToTab { .. })
            ),
            "the first row is a tab"
        );

        vcx.simulate_keystrokes("enter");
        vcx.run_until_parked();
        app.read_with(&vcx, |app, _| {
            assert!(!app.search.is_open(), "picking a row closes the search");
            assert_eq!(
                app.tabs.active_index(),
                2,
                "back to the tab used before this one"
            );
        });
    }

    /// "SSH: Add Connection…" was a second input box. It is the Hosts tab
    /// now — and whatever found the row is not an address, so it goes.
    #[gpui_kit::test]
    fn add_connection_moves_to_an_empty_hosts_tab(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        app.update_in(&mut vcx, |app, window, cx| {
            app.open_search(SearchTab::Actions, "ssh-add-connection", window, cx)
        });
        vcx.run_until_parked();
        let view = open(&app, &mut vcx);
        assert_eq!(first_kind(&view, &mut vcx), Some(CommandKind::SearchHosts));

        vcx.simulate_keystrokes("enter");
        vcx.run_until_parked();
        view.read_with(&vcx, |view, cx| {
            assert_eq!(view.tab, SearchTab::Hosts);
            assert_eq!(view.list.read(cx).delegate().query, "");
        });
    }

    /// The quick-open chord lands on the Files tab, from nowhere or from
    /// another tab, and puts the search away when it is already there.
    #[gpui_kit::test]
    fn quick_open_goes_to_the_files_tab_and_back_out(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        app.update_in(&mut vcx, |app, window, cx| app.quick_open_file(window, cx));
        vcx.run_until_parked();
        let view = open(&app, &mut vcx);
        view.read_with(&vcx, |view, _| assert_eq!(view.tab, SearchTab::Files));

        view.update_in(&mut vcx, |view, window, cx| {
            view.set_tab(SearchTab::Actions, None, window, cx)
        });
        app.update_in(&mut vcx, |app, window, cx| app.quick_open_file(window, cx));
        vcx.run_until_parked();
        view.read_with(&vcx, |view, _| assert_eq!(view.tab, SearchTab::Files));

        app.update_in(&mut vcx, |app, window, cx| app.quick_open_file(window, cx));
        vcx.run_until_parked();
        assert!(app.read_with(&vcx, |app, _| !app.search.is_open()));
    }

    /// A tab reached by its own chord stands alone: Tab does not swap Go to
    /// File or the places a language server found for the terminals.
    #[gpui_kit::test]
    fn a_standalone_tab_keeps_its_list_on_tab(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        for tab in [SearchTab::Files, SearchTab::Symbols, SearchTab::Locations] {
            app.update_in(&mut vcx, |app, window, cx| {
                app.open_search(tab, "", window, cx)
            });
            vcx.run_until_parked();
            let view = open(&app, &mut vcx);
            view.update_in(&mut vcx, |view, window, cx| {
                view.step_tab(true, window, cx);
                view.step_tab(false, window, cx);
            });
            view.read_with(&vcx, |view, _| assert_eq!(view.tab, tab));
            app.update_in(&mut vcx, |app, window, cx| app.close_search(window, cx));
            vcx.run_until_parked();
        }
    }

    /// The Actions tab's "Go to File…" row moves the search to its Files tab
    /// rather than closing it, with whatever found the row cleared away.
    #[gpui_kit::test]
    fn go_to_file_moves_to_an_empty_files_tab(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        app.update_in(&mut vcx, |app, window, cx| {
            app.open_search(SearchTab::Actions, "go-to-file", window, cx)
        });
        vcx.run_until_parked();
        let view = open(&app, &mut vcx);
        assert_eq!(
            first_kind(&view, &mut vcx),
            Some(CommandKind::QuickOpenFile)
        );

        vcx.simulate_keystrokes("enter");
        vcx.run_until_parked();
        view.read_with(&vcx, |view, cx| {
            assert_eq!(view.tab, SearchTab::Files);
            assert_eq!(view.list.read(cx).delegate().query, "");
        });
    }

    /// A walk that lands after the search opened fills the Files tab in place.
    #[gpui_kit::test]
    fn a_file_list_that_lands_late_fills_the_tab(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        app.update_in(&mut vcx, |app, window, cx| {
            app.open_search(SearchTab::Files, "lib", window, cx)
        });
        vcx.run_until_parked();
        let view = open(&app, &mut vcx);
        assert_eq!(first_kind(&view, &mut vcx), None);

        let root = std::path::PathBuf::from("/repo");
        let index = super::super::files::build_index(
            std::slice::from_ref(&root),
            vec![root.join("src/lib.rs")],
            false,
        );
        view.update_in(&mut vcx, |view, window, cx| {
            view.set_files(FileList::Ready(std::sync::Arc::new(index)), window, cx)
        });
        vcx.run_until_parked();
        assert!(matches!(
            first_kind(&view, &mut vcx),
            Some(CommandKind::OpenFile { .. })
        ));
    }

    fn isolate_history(app: &Entity<LyrelApp>, vcx: &mut VisualTestContext) -> tempfile::TempDir {
        let home = tempfile::tempdir().unwrap();
        let settings = crate::ui::settings::test_window::open(app, vcx);
        settings.update_in(vcx, |settings, window, cx| {
            crate::ui::settings::skills::test_host::install(home.path(), cx);
            settings.close_settings(window, cx);
        });
        vcx.run_until_parked();
        home
    }

    fn wait_for_history(view: &Entity<SearchView>, vcx: &mut VisualTestContext) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while view.read_with(vcx, |view, _| view.sessions_landed) == 0 {
            assert!(
                std::time::Instant::now() < deadline,
                "the scan never landed"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
            vcx.run_until_parked();
        }
    }

    #[gpui_kit::test]
    fn replacing_search_keeps_the_original_keyboard_return_focus(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        app.update_in(&mut vcx, |app, window, cx| app.focus_active(window, cx));
        vcx.run_until_parked();
        let original = vcx.update(|window, cx| window.focused(cx).unwrap());
        let chord =
            vcx.update(|_, cx| crate::ui::keymap::effective_key("TogglePalette", cx).unwrap());
        vcx.simulate_keystrokes(&chord);
        vcx.run_until_parked();
        let old = open(&app, &mut vcx);
        app.update_in(&mut vcx, |app, window, cx| {
            app.open_search(SearchTab::Actions, "", window, cx);
        });
        assert_ne!(old, open(&app, &mut vcx));
        vcx.run_until_parked();
        vcx.simulate_keystrokes("escape");
        vcx.run_until_parked();
        assert!(!app.read_with(&vcx, |app, _| app.search.is_open()));
        vcx.update(|window, _| assert!(original.is_focused(window)));
    }

    #[gpui_kit::test]
    fn retired_search_rejects_history_and_navigation_after_reopen_or_workspace_change(
        cx: &mut TestAppContext,
    ) {
        for change_workspace in [false, true] {
            let (app, mut vcx, _streams) = harness_with_tabs(cx, 2);
            let _home = isolate_history(&app, &mut vcx);
            // Keep the old Entity alive, and replace it before pumping async
            // work: Host results must be refused even when upgrade succeeds.
            let old = app.update_in(&mut vcx, |app, window, cx| {
                app.open_search(SearchTab::Sessions, "", window, cx);
                let old = app.search.view().unwrap();
                if change_workspace {
                    let id = crate::core::session::WorkspaceStore::claim(cx, None);
                    app.adopt_workspace(id, crate::core::session::Session::default(), window, cx);
                }
                app.open_search(SearchTab::Sessions, "", window, cx);
                old
            });
            let current = open(&app, &mut vcx);
            wait_for_history(&current, &mut vcx);
            old.read_with(&vcx, |view, _| assert_eq!(view.sessions_landed, 0));
            let active = app.read_with(&vcx, |app, _| app.tabs.active_index());
            old.update(&mut vcx, |_, cx| {
                cx.emit(SearchEvent::Confirm(CommandKind::SelectNextTab));
                cx.emit(SearchEvent::Dismiss);
            });
            vcx.run_until_parked();
            assert_eq!(open(&app, &mut vcx), current);
            assert_eq!(
                app.read_with(&vcx, |app, _| app.tabs.active_index()),
                active
            );
        }
    }

    fn past_session(id: &str) -> Item {
        Item::new(
            format!("session {id}"),
            CommandKind::ResumeSession {
                agent: crate::core::cli_agent::CLIAgent::OhMyPi,
                session_id: id.into(),
                cwd: None,
                host: crate::ui::host_ops::HostId::LOCAL,
                scope: None,
            },
        )
    }

    fn row_kinds(view: &Entity<SearchView>, vcx: &mut VisualTestContext) -> Vec<CommandKind> {
        view.read_with(vcx, |view, cx| {
            let delegate = view.list.read(cx).delegate();
            delegate
                .sections
                .iter()
                .flat_map(|s| &s.rows)
                .filter_map(|r| Some(r.item()?.kind.clone()))
                .collect()
        })
    }

    /// The edit gesture on a past session opens what can be done with it;
    /// removing it takes the row away at once, keeps the search open, and
    /// is remembered.
    #[gpui_kit::test]
    fn a_past_session_opens_its_actions_and_can_be_removed(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        let _home = isolate_history(&app, &mut vcx);
        crate::ui::i18n::set_locale("en");
        app.update_in(&mut vcx, |app, window, cx| {
            app.open_search(SearchTab::Sessions, "", window, cx)
        });
        // The isolated Host scan still lands asynchronously. Let it finish
        // before installing the sessions used by the menu interaction.
        let view = open(&app, &mut vcx);
        wait_for_history(&view, &mut vcx);
        view.update_in(&mut vcx, |view, window, cx| {
            view.set_sessions(vec![past_session("a"), past_session("b")], 0, window, cx)
        });
        vcx.run_until_parked();

        vcx.simulate_keystrokes("secondary-e");
        vcx.run_until_parked();
        let actions = row_kinds(&view, &mut vcx);
        assert!(view.read_with(&vcx, |view, cx| view.scope(cx) == Scope::SessionActions));
        assert!(matches!(actions[0], CommandKind::ResumeSession { .. }));
        for wanted in [
            CommandKind::ForkSession {
                agent: crate::core::cli_agent::CLIAgent::OhMyPi,
                session_id: "a".into(),
                cwd: None,
                host: crate::ui::host_ops::HostId::LOCAL,
                scope: None,
            },
            CommandKind::CopySessionId("a".into()),
        ] {
            assert!(actions.contains(&wanted));
        }

        vcx.simulate_input("remove");
        vcx.run_until_parked();
        vcx.simulate_keystrokes("enter");
        vcx.run_until_parked();
        assert!(app.read_with(&vcx, |app, _| app.search.is_open()));
        view.read_with(&vcx, |view, cx| {
            assert_eq!(view.scope(cx), Scope::Tab(SearchTab::Sessions));
            assert_eq!(
                view.list.read(cx).delegate().query,
                "",
                "the query comes back"
            );
        });
        let left = row_kinds(&view, &mut vcx);
        assert_eq!(left.len(), 1);
        assert!(
            matches!(&left[0], CommandKind::ResumeSession { session_id, .. } if session_id == "b")
        );
        app.read_with(&vcx, |_, cx| {
            assert!(
                cx.global::<crate::core::config::Config>()
                    .hidden_agent_sessions
                    .contains("omp:a")
            );
        });
    }

    /// The list re-picks its row from what it drew last frame, and a query
    /// that found nothing drew nothing — so the query typed next came up with
    /// rows and nothing selected, and Return did nothing.
    #[gpui_kit::test]
    fn a_query_after_one_that_found_nothing_still_arms_its_first_row(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        app.update_in(&mut vcx, |app, window, cx| {
            app.open_search(SearchTab::Actions, "", window, cx)
        });
        vcx.run_until_parked();
        let view = open(&app, &mut vcx);

        vcx.simulate_input("split rightq");
        vcx.run_until_parked();
        assert_eq!(first_kind(&view, &mut vcx), None, "nothing matches");
        // One keystroke back to a query with rows: the one search that runs
        // right after a frame that drew nothing.
        vcx.simulate_keystrokes("backspace");
        vcx.run_until_parked();

        let selected = view.read_with(&vcx, |view, cx| view.list.read(cx).selected_index());
        assert!(selected.is_some(), "Return has a row to run");
        vcx.simulate_keystrokes("enter");
        vcx.run_until_parked();
        assert!(app.read_with(&vcx, |app, _| !app.search.is_open()));
    }
}
