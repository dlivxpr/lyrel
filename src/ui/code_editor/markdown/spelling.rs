//! Debounced spelling diagnostics and the standard keyboard-accessible menu.
use super::*;
use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};
use unicode_segmentation::UnicodeSegmentation as _;
#[path = "spelling_os.rs"]
mod os;

actions!(markdown_spelling, [SpellingSuggestions]);

#[derive(Clone)]
struct Diagnostic {
    ranges: Vec<Range<usize>>,
    word: String,
}
#[derive(Default)]
pub(super) struct State {
    pub(super) surface_generation: u64,
    generation: u64,
    diagnostics: Vec<Diagnostic>,
    task: Option<Task<()>>,
    suggestions_task: Option<Task<()>>,
    menu: Option<(Entity<PopupMenu>, Point<Pixels>)>,
    subscription: Option<Subscription>,
}
impl State {
    pub(super) fn invalidate(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.diagnostics.clear();
        self.task = None;
        self.suggestions_task = None;
        self.menu = None;
        self.subscription = None;
    }
}
impl SourcePresentation {
    pub(super) fn schedule_spelling(&mut self, cx: &mut Context<Self>) {
        self.spelling.invalidate();
        let generation = self.spelling.generation;
        let identity = self.document.entity_id();
        let document = self.document.read(cx);
        let revision = document.revision();
        let resources = document.resources().cloned();
        let source = document.source().clone();
        let live = self.live;
        self.spelling.task = Some(cx.spawn(async move |view, cx| {
            cx.background_executor().timer(std::time::Duration::from_millis(250)).await;
            let diagnostics = cx.background_executor().spawn(async move {
                let value = source.to_string();
                os::check(&value).into_iter().filter_map(|range| {
                    let word = value.get(range.clone())?.to_owned();
                    Some(Diagnostic { ranges: vec![range], word })
                }).collect::<Vec<_>>()
            }).await;
            let _ = view.update(cx, |view, cx| {
                let document = view.document.read(cx);
                let same_host = match (&resources, document.resources()) {
                    (Some(expected), current) => expected.is_current(current),
                    (None, None) => true,
                    _ => false,
                };
                if view.document.entity_id() != identity || document.revision() != revision
                    || view.spelling.generation != generation || view.live != live || !same_host { return; }
                view.set_spelling_diagnostics(diagnostics, cx);
                view.spelling.task = None;
            });
        }));
    }
    fn set_spelling_diagnostics(&mut self, mut diagnostics: Vec<Diagnostic>, cx: &mut Context<Self>) {
        diagnostics.sort_by_key(|diagnostic| diagnostic.ranges.first().map_or(0, |range| range.start));
        self.spelling.diagnostics = diagnostics;
        cx.notify();
    }
    pub(super) fn spelling_at_point(&self, point: Point<Pixels>, cx: &App) -> Option<usize> {
        let at = self.point_offset(point, cx)?;
        self.spelling.diagnostics.iter().position(|diagnostic|
            diagnostic.ranges.iter().any(|range| range.contains(&at)))
    }
    fn spelling_at_selection(&self) -> Option<usize> {
        let at = self.state.selection.head();
        self.spelling.diagnostics.iter().position(|diagnostic|
            diagnostic.ranges.iter().any(|range| range.start <= at && at <= range.end))
    }
    pub(super) fn spelling_menu(&self, menu: PopupMenu) -> PopupMenu {
        menu.separator().menu_with_disabled(t(L10nKey::EditorSpellingSuggestions),
            Box::new(SpellingSuggestions), self.spelling_at_selection().is_none())
    }
    fn install_spelling_menu(&mut self, diagnostic: Diagnostic, suggestions: Option<Vec<String>>,
        position: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let owner = cx.entity().downgrade();
        let token = self.input_token(cx);
        let generation = self.spelling.generation;
        let focus = self.focus.clone();
        let menu = PopupMenu::build(window, cx, move |mut menu, _, _| {
            menu = menu.action_context(focus).label(t(L10nKey::EditorSpellingSuggestions));
            match suggestions {
                None => menu.label(t(L10nKey::EditorSpellingChecking)),
                Some(suggestions) if suggestions.is_empty() => menu.label(t(L10nKey::EditorSpellingNoSuggestions)),
                Some(suggestions) => {
                    for suggestion in suggestions {
                        let owner = owner.clone();
                        let diagnostic = diagnostic.clone();
                        menu = menu.item(PopupMenuItem::new(suggestion.clone()).on_click(move |_, window, cx| {
                            let _ = owner.update(cx, |view, cx| {
                                if view.input_token(cx) != token || view.spelling.generation != generation { return; }
                                let edits = diagnostic.ranges.iter().cloned().enumerate().map(|(ix, range)|
                                    (range, if ix == 0 { suggestion.clone() } else { String::new() })).collect();
                                view.command(Command::BatchReplace { edits, kind: EditKind::Command }, window, cx);
                                view.spelling.menu = None;
                                view.focus.focus(window, cx);
                            });
                        }));
                    }
                    menu
                }
            }
        });
        self.spelling.subscription = Some(cx.subscribe_in(&menu, window, |view, _, _: &DismissEvent, window, cx| {
            view.spelling.menu = None;
            view.spelling.suggestions_task = None;
            view.focus.focus(window, cx);
            cx.notify();
        }));
        menu.read(cx).focus_handle(cx).focus(window, cx);
        self.spelling.menu = Some((menu, position));
        cx.notify();
    }
    pub(super) fn open_spelling(&mut self, point: Option<Point<Pixels>>, window: &mut Window, cx: &mut Context<Self>) {
        // Finish only the owning presentation's preedit through the normal command.
        self.command(Command::FinishComposition, window, cx);
        let ix = point.and_then(|point| self.spelling_at_point(point, cx)).or_else(|| self.spelling_at_selection());
        let Some(diagnostic) = ix.and_then(|ix| self.spelling.diagnostics.get(ix).cloned()) else { return; };
        let position = point.or_else(|| self.caret_bounds(self.state.selection.head(), cx).map(|bounds| bounds.bottom_left()));
        let Some(position) = position else { return; };
        self.focus.focus(window, cx);
        self.install_spelling_menu(diagnostic.clone(), None, position, window, cx);
        let generation = self.spelling.generation;
        let token = self.input_token(cx);
        self.spelling.suggestions_task = Some(cx.spawn_in(window, async move |view, cx| {
            let word = diagnostic.word.clone();
            let suggestions = cx.background_executor().spawn(async move { os::suggestions(&word) }).await;
            let _ = view.update_in(cx, |view, window, cx| {
                if view.spelling.generation != generation || view.input_token(cx) != token
                    || !view.spelling.menu.as_ref().is_some_and(|(menu, _)| menu.read(cx).focus_handle(cx).contains_focused(window, cx)) {
                    return;
                }
                view.install_spelling_menu(diagnostic, Some(suggestions), position, window, cx);
                view.spelling.suggestions_task = None;
            });
        }));
    }
    pub(super) fn spelling_overlay(&self, window: &Window, _: &App) -> Option<AnyElement> {
        self.spelling.menu.as_ref().map(|(menu, position)| deferred(anchored().position(*position)
            .snap_to_window_with_margin(window.rem_size() * 0.5).child(menu.clone())).into_any_element())
    }
    pub(super) fn paint_spelling(&self, window: &mut Window, cx: &App) {
        if !self.geometry_is_current(cx) { return; }
        let Some((_, geometry)) = &self.geometry else { return; };
        let viewport = self.scroll.bounds();
        let unit = window.rem_size() / 16.;
        let color = cx.theme().danger;
        for leaf in &self.projection.leaves {
            let Some(geometry) = geometry.get(&leaf.id) else { continue; };
            let first = self.spelling.diagnostics.partition_point(|diagnostic|
                diagnostic.ranges.last().is_none_or(|range| range.end <= leaf.range.start));
            for diagnostic in self.spelling.diagnostics[first..].iter().take_while(|diagnostic|
                diagnostic.ranges.first().is_some_and(|range| range.start < leaf.range.end)) {
                for source in &diagnostic.ranges {
                    let start = leaf.display_offset(source.start.max(leaf.range.start));
                    let end = leaf.display_offset(source.end.min(leaf.range.end));
                    let Some(text) = leaf.text.get(start..end) else { continue; };
                    let mut path = PathBuilder::stroke(unit);
                    for (offset, grapheme) in text.grapheme_indices(true) {
                        let a = geometry.position_for_index(start + offset);
                        let b = geometry.position_for_index(start + offset + grapheme.len());
                        if a.y != b.y || a.x == b.x { continue; }
                        let left = a.x.min(b.x).max(viewport.left());
                        let right = a.x.max(b.x).min(viewport.right());
                        let y = a.y + geometry.caret_height();
                        if y < viewport.top() || y + unit * 2. > viewport.bottom() || left >= right { continue; }
                        path.move_to(point(left, y));
                        let mut x = left;
                        let mut up = true;
                        while x < right {
                            x = (x + unit * 2.).min(right);
                            path.line_to(point(x, y + if up { unit } else { Pixels::ZERO }));
                            up = !up;
                        }
                    }
                    if let Ok(path) = path.build() { window.paint_path(path, color); }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::app::test_window::harness_with_tabs;
    use gpui_kit::test::TestWindowExt as _;

    #[gpui_kit::test]
    fn spelling_menu_keyboard_replacement_shares_history_and_saved_bytes(cx: &mut TestAppContext) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("spelling.md");
        let original = b"\xef\xbb\xbf**wrng**\r\nuntouched\n";
        std::fs::write(&path, original).unwrap();
        let (app, mut context, _streams) = harness_with_tabs(cx, 1);
        app.update_in(&mut context, |app, window, cx| app.open_file_in_editor(&path, window, cx));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let view = loop {
            context.run_until_parked();
            if let Some(view) = app.read_with(&context, |app, _| app.active_editor_input().and_then(|input| input.markdown().cloned())) { break view; }
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(5));
        };
        // Inject the OS result at the production diagnostics/menu boundaries:
        // dictionaries vary by installed OS language and must not be mocked by
        // an application-owned word list or require network access in tests.
        view.update_in(&mut context, |view, window, cx| {
            view.spelling.invalidate();
            view.select_range(3..3, cx);
            view.focus.focus(window, cx);
            let diagnostic = Diagnostic { ranges: vec![2..6], word: "wrng".into() };
            view.set_spelling_diagnostics(vec![diagnostic.clone()], cx);
            view.install_spelling_menu(diagnostic, Some(vec!["wrong".into()]), point(window.rem_size(), window.rem_size()), window, cx);
        });
        context.update(|window, cx| window.render_frame(cx));
        context.update(|window, cx| { window.press("down", cx); window.press("enter", cx); });
        context.run_until_parked();
        view.read_with(&context, |view, cx| {
            assert_eq!(view.document.read(cx).source().to_string(), "**wrong**\nuntouched\n");
        });
        view.update_in(&mut context, |view, window, _| assert!(view.focus.is_focused(window)));
        context.dispatch_action(gpui_kit::component::input::Undo);
        context.run_until_parked();
        view.read_with(&context, |view, cx| assert_eq!(view.document.read(cx).encode().unwrap(), original));
        context.dispatch_action(gpui_kit::component::input::Redo);
        context.dispatch_action(crate::core::actions::ToggleDocumentPreview);
        context.dispatch_action(crate::core::actions::EditorSave);
        loop {
            context.run_until_parked();
            if app.read_with(&context, |app, _| app.active_buffer().is_some_and(|file| !file.dirty && !file.saving)) { break; }
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let expected = b"\xef\xbb\xbf**wrong**\r\nuntouched\n";
        assert_eq!(std::fs::read(&path).unwrap(), expected);
        app.update_in(&mut context, |app, window, cx| { app.editor_close_file(0, window, cx); app.open_file_in_editor(&path, window, cx); });
        loop {
            context.run_until_parked();
            if app.read_with(&context, |app, _| app.active_buffer().is_some()) { break; }
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        app.read_with(&context, |app, cx| assert_eq!(app.active_editor_input().unwrap().markdown().unwrap().read(cx).document().read(cx).encode().unwrap(), expected));
    }
}
