//! Retained search ownership: one view, its event lease, and return focus.

use gpui_kit::{App, AppContext as _, Context, Entity, FocusHandle, Subscription, Window};

use super::{SearchCatalog, SearchEvent, SearchTab, SearchView};

#[derive(Default)]
pub(crate) struct SearchState {
    open: Option<OpenSearch>,
}

struct OpenSearch {
    view: Entity<SearchView>,
    _events: Subscription,
    return_focus: Option<FocusHandle>,
}

impl SearchState {
    pub(crate) fn is_open(&self) -> bool {
        self.open.is_some()
    }

    pub(crate) fn view(&self) -> Option<Entity<SearchView>> {
        self.open.as_ref().map(|open| open.view.clone())
    }

    pub(crate) fn owns(&self, view: &Entity<SearchView>) -> bool {
        self.open.as_ref().is_some_and(|open| &open.view == view)
    }

    pub(crate) fn open<T: 'static>(
        &mut self,
        catalog: SearchCatalog,
        tab: SearchTab,
        query: &str,
        on_event: fn(&mut T, &Entity<SearchView>, &SearchEvent, &mut Window, &mut Context<T>),
        window: &mut Window,
        cx: &mut Context<T>,
    ) -> Entity<SearchView> {
        // A direct reopen replaces the view but keeps the original content
        // focus, never the input belonging to the search being replaced.
        let return_focus = match self.open.take() {
            Some(open) => {
                open.view.update(cx, |view, _| view.retire());
                open.return_focus
            }
            None => window.focused(cx),
        };
        let (catalog, session_query) = catalog.prepare(cx);
        let view = cx.new(|cx| SearchView::new(catalog, tab, query, window, cx));
        let events = cx.subscribe_in(&view, window, on_event);
        self.open = Some(OpenSearch {
            view: view.clone(),
            _events: events,
            return_focus,
        });
        if let Some(query) = session_query {
            view.update(cx, |view, cx| view.refresh_sessions(query, window, cx));
        }
        view
    }

    /// Retire before releasing handles: even a retained obsolete view cannot
    /// accept an in-flight result. Return the original focus for the workbench's
    /// final handoff, including a toolbar button when opened by pointer.
    pub(crate) fn close(&mut self, cx: &mut App) -> Option<FocusHandle> {
        let open = self.open.take()?;
        open.view.update(cx, |view, _| view.retire());
        open.return_focus
    }
}
