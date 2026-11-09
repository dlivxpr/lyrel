//! Debounced spelling diagnostics and the standard keyboard-accessible menu.
use super::navigation::grapheme_line_box;
use super::*;
use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};
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

fn visible_spelling_text(
    leaf: &super::super::projection::Leaf,
    geometry: &LeafGeometry,
    viewport: Bounds<Pixels>,
) -> smallvec::SmallVec<[Range<usize>; 1]> {
    let text = &leaf.text;
    let margin = geometry.line_height() * 2.;
    let top = viewport.top() - margin;
    let bottom = viewport.bottom() + margin;
    let bounds = geometry.bounds();
    if bounds.bottom() < top || bounds.top() > bottom {
        return smallvec::SmallVec::new();
    }
    let before = |range: Range<usize>, predicate: &dyn Fn(Point<Pixels>) -> bool| {
        let mut low = range.start;
        let mut high = range.end;
        while low < high {
            let middle = text.floor_char_boundary(low + (high - low) / 2);
            if predicate(geometry.position_for_index(middle)) {
                low = text.ceil_char_boundary(middle + 1);
            } else {
                high = middle;
            }
        }
        low
    };
    let mut visible = smallvec::SmallVec::<[Range<usize>; 1]>::new();
    // Only bisect text within its original atom/style boundaries. In
    // particular, never query fragment.end: that caret belongs to the next
    // piece, possibly a tall atom with a non-monotonic baseline.
    for fragment in leaf.spelling_fragments() {
        let start = before(fragment.clone(), &|p| p.y < top);
        let end = before(start..fragment.end, &|p| p.y < bottom);
        let mut row_start = start;
        while row_start < end {
            let y = geometry.position_for_index(row_start).y;
            let row_end = before(row_start..end, &|p| p.y < y + geometry.line_height() / 2.)
                .max(text.ceil_char_boundary(row_start + 1));
            for run in &leaf.direction_runs {
                let a = row_start.max(run.start);
                let b = row_end.min(run.end);
                if a >= b {
                    continue;
                }
                let last = text.floor_char_boundary(b - 1);
                let rtl = geometry.position_for_index(a).x > geometry.position_for_index(last).x;
                let left = viewport.left() - margin;
                let right = viewport.right() + margin;
                let from = before(a..b, &|p| if rtl { p.x > right } else { p.x < left });
                let through = before(from..b, &|p| if rtl { p.x >= left } else { p.x <= right });
                // Retain complete boundary graphemes; native line boxes and
                // pixel clipping still decide which part is actually drawn.
                let from = navigation::adjacent_grapheme(text, from, true)
                    .map_or(from, |grapheme| grapheme.start)
                    .max(fragment.start);
                let through = navigation::adjacent_grapheme(text, through, false)
                    .map_or(through, |grapheme| grapheme.end)
                    .min(fragment.end);
                if let Some(previous) = visible.last_mut().filter(|range| range.end >= from) {
                    previous.end = previous.end.max(through);
                } else {
                    visible.push(from..through);
                }
            }
            row_start = row_end;
        }
    }
    visible
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
            cx.background_executor()
                .timer(std::time::Duration::from_millis(250))
                .await;
            let diagnostics = cx
                .background_executor()
                .spawn(async move {
                    let value = source.to_string();
                    os::check(&value)
                        .into_iter()
                        .filter_map(|range| {
                            let word = value.get(range.clone())?.to_owned();
                            Some(Diagnostic {
                                ranges: vec![range],
                                word,
                            })
                        })
                        .collect::<Vec<_>>()
                })
                .await;
            let _ = view.update(cx, |view, cx| {
                let document = view.document.read(cx);
                let same_host = match (&resources, document.resources()) {
                    (Some(expected), current) => expected.is_current(current),
                    (None, None) => true,
                    _ => false,
                };
                if view.document.entity_id() != identity
                    || document.revision() != revision
                    || view.spelling.generation != generation
                    || view.live != live
                    || !same_host
                {
                    return;
                }
                view.set_spelling_diagnostics(diagnostics, cx);
                view.spelling.task = None;
            });
        }));
    }
    fn set_spelling_diagnostics(
        &mut self,
        mut diagnostics: Vec<Diagnostic>,
        cx: &mut Context<Self>,
    ) {
        diagnostics
            .sort_by_key(|diagnostic| diagnostic.ranges.first().map_or(0, |range| range.start));
        self.spelling.diagnostics = diagnostics;
        cx.notify();
    }
    pub(super) fn spelling_at_point(&self, point: Point<Pixels>, cx: &App) -> Option<usize> {
        let at = self.point_offset(point, cx)?;
        self.spelling
            .diagnostics
            .iter()
            .position(|diagnostic| diagnostic.ranges.iter().any(|range| range.contains(&at)))
    }
    fn spelling_at_selection(&self) -> Option<usize> {
        let at = self.state.selection.head();
        self.spelling.diagnostics.iter().position(|diagnostic| {
            diagnostic
                .ranges
                .iter()
                .any(|range| range.start <= at && at <= range.end)
        })
    }
    pub(super) fn spelling_menu(&self, menu: PopupMenu) -> PopupMenu {
        menu.separator().menu_with_disabled(
            t(L10nKey::EditorSpellingSuggestions),
            Box::new(SpellingSuggestions),
            self.spelling_at_selection().is_none(),
        )
    }
    fn install_spelling_menu(
        &mut self,
        diagnostic: Diagnostic,
        suggestions: Option<Vec<String>>,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let owner = cx.entity().downgrade();
        let token = self.input_token(cx);
        let generation = self.spelling.generation;
        let focus = self.focus.clone();
        let menu = PopupMenu::build(window, cx, move |mut menu, _, _| {
            menu = menu
                .action_context(focus)
                .label(t(L10nKey::EditorSpellingSuggestions));
            match suggestions {
                None => menu.label(t(L10nKey::EditorSpellingChecking)),
                Some(suggestions) if suggestions.is_empty() => {
                    menu.label(t(L10nKey::EditorSpellingNoSuggestions))
                }
                Some(suggestions) => {
                    for suggestion in suggestions {
                        let owner = owner.clone();
                        let diagnostic = diagnostic.clone();
                        menu = menu.item(PopupMenuItem::new(suggestion.clone()).on_click(
                            move |_, window, cx| {
                                let _ = owner.update(cx, |view, cx| {
                                    if view.input_token(cx) != token
                                        || view.spelling.generation != generation
                                    {
                                        return;
                                    }
                                    let edits = diagnostic
                                        .ranges
                                        .iter()
                                        .cloned()
                                        .enumerate()
                                        .map(|(ix, range)| {
                                            (
                                                range,
                                                if ix == 0 {
                                                    suggestion.clone()
                                                } else {
                                                    String::new()
                                                },
                                            )
                                        })
                                        .collect();
                                    view.command(
                                        Command::BatchReplace {
                                            edits,
                                            kind: EditKind::Command,
                                        },
                                        window,
                                        cx,
                                    );
                                    view.spelling.menu = None;
                                    view.focus.focus(window, cx);
                                });
                            },
                        ));
                    }
                    menu
                }
            }
        });
        self.spelling.subscription =
            Some(
                cx.subscribe_in(&menu, window, |view, _, _: &DismissEvent, window, cx| {
                    view.spelling.menu = None;
                    view.spelling.suggestions_task = None;
                    view.focus.focus(window, cx);
                    cx.notify();
                }),
            );
        menu.read(cx).focus_handle(cx).focus(window, cx);
        self.spelling.menu = Some((menu, position));
        cx.notify();
    }
    pub(super) fn open_spelling(
        &mut self,
        point: Option<Point<Pixels>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ix = point
            .and_then(|point| self.spelling_at_point(point, cx))
            .or_else(|| self.spelling_at_selection());
        let Some(diagnostic) = ix.and_then(|ix| self.spelling.diagnostics.get(ix).cloned()) else {
            return;
        };
        let position = point.or_else(|| {
            self.caret_bounds(self.state.selection.head(), cx)
                .map(|bounds| bounds.bottom_left())
        });
        let Some(position) = position else {
            return;
        };
        let composing = self.is_composing();
        self.command(Command::FinishComposition, window, cx);
        if composing {
            let token = self.input_token(cx);
            cx.defer_in(window, move |view, window, cx| {
                if view.input_token(cx) == token && view.focus.is_focused(window) {
                    view.open_spelling_diagnostic(diagnostic, position, window, cx);
                }
            });
        } else {
            self.open_spelling_diagnostic(diagnostic, position, window, cx);
        }
    }
    fn open_spelling_diagnostic(
        &mut self,
        diagnostic: Diagnostic,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.focus.focus(window, cx);
        self.install_spelling_menu(diagnostic.clone(), None, position, window, cx);
        let generation = self.spelling.generation;
        let token = self.input_token(cx);
        self.spelling.suggestions_task = Some(cx.spawn_in(window, async move |view, cx| {
            let word = diagnostic.word.clone();
            let suggestions = cx
                .background_executor()
                .spawn(async move { os::suggestions(&word) })
                .await;
            let _ = view.update_in(cx, |view, window, cx| {
                if view.spelling.generation != generation
                    || view.input_token(cx) != token
                    || !view.spelling.menu.as_ref().is_some_and(|(menu, _)| {
                        menu.read(cx).focus_handle(cx).contains_focused(window, cx)
                    })
                {
                    return;
                }
                view.install_spelling_menu(diagnostic, Some(suggestions), position, window, cx);
                view.spelling.suggestions_task = None;
            });
        }));
    }
    pub(super) fn spelling_overlay(&self, window: &Window, _: &App) -> Option<AnyElement> {
        self.spelling.menu.as_ref().map(|(menu, position)| {
            deferred(
                anchored()
                    .position(*position)
                    .snap_to_window_with_margin(window.rem_size() * 0.5)
                    .child(menu.clone()),
            )
            .into_any_element()
        })
    }
    pub(super) fn paint_spelling(&self, window: &mut Window, cx: &App) {
        if self.spelling.diagnostics.is_empty() || !self.geometry_is_current(cx) {
            return;
        }
        let Some((_, geometry)) = &self.geometry else {
            return;
        };
        let viewport = self.scroll.bounds();
        let unit: Pixels = window.rem_size() * (1. / 16.);
        let color = cx.theme().danger;
        for leaf in &self.projection.leaves {
            let Some(geometry) = geometry.get(&leaf.id) else {
                continue;
            };
            for visible in visible_spelling_text(leaf, geometry, viewport) {
                let source_start = leaf.source_offset(visible.start);
                let source_end = leaf.source_offset(visible.end);
                let first = self.spelling.diagnostics.partition_point(|diagnostic| {
                    diagnostic
                        .ranges
                        .last()
                        .is_none_or(|range| range.end <= source_start)
                });
                for diagnostic in
                    self.spelling.diagnostics[first..]
                        .iter()
                        .take_while(|diagnostic| {
                            diagnostic
                                .ranges
                                .first()
                                .is_some_and(|range| range.start < source_end)
                        })
                {
                    for source in &diagnostic.ranges {
                        let start = leaf
                            .display_offset(source.start.max(leaf.range.start))
                            .max(visible.start);
                        let end = leaf
                            .display_offset(source.end.min(leaf.range.end))
                            .min(visible.end);
                        if start >= end {
                            continue;
                        }
                        let Some(text) = leaf.text.get(start..end) else {
                            continue;
                        };
                        let mut path = PathBuilder::stroke(unit);
                        for (offset, grapheme) in text.grapheme_indices(true) {
                            let Some(segment) =
                                grapheme_line_box(geometry, start + offset, grapheme)
                            else {
                                continue;
                            };
                            let left = segment.left().max(viewport.left());
                            let right = segment.right().min(viewport.right());
                            let y = segment.bottom();
                            if y < viewport.top()
                                || y + unit * 2. > viewport.bottom()
                                || left >= right
                            {
                                continue;
                            }
                            path.move_to(point(left, y));
                            let mut x = left;
                            let mut up = true;
                            while x < right {
                                x = (x + unit * 2.).min(right);
                                path.line_to(point(x, y + if up { unit } else { Pixels::ZERO }));
                                up = !up;
                            }
                        }
                        if let Ok(path) = path.build() {
                            window.paint_path(path, color);
                        }
                    }
                }
            }
        }
    }
}

#[cfg(feature = "native-markdown-tests")]
pub(super) fn native_cases() -> [crate::native_markdown::Case; 4] {
    native::native_cases()
}

#[cfg(feature = "native-markdown-tests")]
#[path = "spelling_native.rs"]
mod native;
#[cfg(any(test, feature = "native-markdown-tests"))]
#[path = "spelling_support.rs"]
mod support;

#[cfg(test)]
mod tests {
    use super::Diagnostic;
    use super::support::{settle_geometry, spelling_fixture};
    use crate::ui::app::test_window::harness_with_tabs;
    use gpui_kit::test::TestWindowExt as _;
    use gpui_kit::{TestAppContext, point};

    #[gpui_kit::test]
    fn spelling_menu_keyboard_replacement_shares_history_and_saved_bytes(cx: &mut TestAppContext) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("spelling.md");
        let original = b"\xef\xbb\xbf**wrng**\r\nuntouched\n";
        std::fs::write(&path, original).unwrap();
        let (app, mut context, _streams) = harness_with_tabs(cx, 1);
        app.update_in(&mut context, |app, window, cx| {
            app.open_file_in_editor(&path, window, cx)
        });
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let view = loop {
            context.run_until_parked();
            if let Some(view) = app.read_with(&context, |app, _| {
                app.active_editor_input()
                    .and_then(|input| input.markdown().cloned())
            }) {
                break view;
            }
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
            let diagnostic = Diagnostic {
                ranges: vec![2..6],
                word: "wrng".into(),
            };
            view.set_spelling_diagnostics(vec![diagnostic.clone()], cx);
            view.install_spelling_menu(
                diagnostic,
                Some(vec!["wrong".into()]),
                point(window.rem_size(), window.rem_size()),
                window,
                cx,
            );
        });
        context.update(|window, cx| window.render_frame(cx));
        context.update(|window, cx| {
            window.press("down", cx);
            window.press("enter", cx);
        });
        context.run_until_parked();
        view.read_with(&context, |view, cx| {
            assert_eq!(
                view.document.read(cx).source().to_string(),
                "**wrong**\nuntouched\n"
            );
        });
        view.update_in(&mut context, |view, window, _| {
            assert!(view.focus.is_focused(window))
        });
        context.dispatch_action(gpui_kit::component::input::Undo);
        context.run_until_parked();
        view.read_with(&context, |view, cx| {
            assert_eq!(view.document.read(cx).encode().unwrap(), original)
        });
        context.dispatch_action(gpui_kit::component::input::Redo);
        context.dispatch_action(crate::core::actions::ToggleDocumentPreview);
        context.dispatch_action(crate::core::actions::EditorSave);
        loop {
            context.run_until_parked();
            if app.read_with(&context, |app, _| {
                app.active_buffer()
                    .is_some_and(|file| !file.dirty && !file.saving)
            }) {
                break;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let expected = b"\xef\xbb\xbf**wrong**\r\nuntouched\n";
        assert_eq!(std::fs::read(&path).unwrap(), expected);
        app.update_in(&mut context, |app, window, cx| {
            app.editor_close_file(0, window, cx);
            app.open_file_in_editor(&path, window, cx);
        });
        loop {
            context.run_until_parked();
            if app.read_with(&context, |app, _| app.active_buffer().is_some()) {
                break;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        app.read_with(&context, |app, cx| {
            assert_eq!(
                app.active_editor_input()
                    .unwrap()
                    .markdown()
                    .unwrap()
                    .read(cx)
                    .document()
                    .read(cx)
                    .encode()
                    .unwrap(),
                expected
            )
        });
    }

    #[gpui_kit::test]
    fn final_review_spelling_menu_survives_own_composition_commit(cx: &mut TestAppContext) {
        use gpui_kit::test::TestInput;
        let original = b"wrog\n";
        let (_directory, _path, _app, mut context, _streams, view) =
            spelling_fixture!(cx, original);
        settle_geometry(&view, &mut context);
        view.update_in(&mut context, |view, window, cx| {
            view.select_range(0..4, cx);
            view.focus.focus(window, cx);
        });
        context.update(|window, cx| {
            let bounds = window.find("markdown-source").bounds();
            TestInput::new(
                gpui_kit::ElementInputHandler::new(bounds, view.clone()),
                window,
            )
            .compose_text(None, "wrng", Some(4..4), cx);
        });
        context.run_until_parked();
        settle_geometry(&view, &mut context);
        view.update_in(&mut context, |view, _, cx| {
            view.spelling.invalidate();
            view.set_spelling_diagnostics(
                vec![Diagnostic {
                    ranges: vec![0..4],
                    word: "wrng".into(),
                }],
                cx,
            );
        });
        context.dispatch_action(super::SpellingSuggestions);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            context.run_until_parked();
            if view.read_with(&context, |view, _| view.spelling.suggestions_task.is_none()) {
                break;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        context.update(|window, cx| {
            window.render_frame(cx);
            assert!(window.find("popup-menu").visible());
            assert_eq!(window.find("popup-menu").focused(), Some(true));
            window.press("escape", cx);
        });
        context.run_until_parked();
        view.read_with(&context, |view, cx| {
            assert!(!view.is_composing());
            assert_eq!(view.document().read(cx).encode().unwrap(), b"wrng\n");
        });
        context.update(|window, cx| assert!(view.read(cx).focus.is_focused(window)));
        context.dispatch_action(gpui_kit::component::input::Undo);
        view.read_with(&context, |view, cx| {
            assert_eq!(view.document().read(cx).encode().unwrap(), original)
        });
    }
}
