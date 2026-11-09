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

#[cfg(test)]
pub(super) fn native_cases() -> [crate::native_markdown::Case; 4] {
    tests::native_cases()
}

#[cfg(test)]
mod tests {
    use super::{Diagnostic, SourcePresentation, grapheme_line_box};
    use crate::ui::app::test_window::harness_with_tabs;
    use crate::ui::code_editor::markdown::projection::Kind;
    use gpui_kit::test::TestWindowExt as _;
    use gpui_kit::{
        Entity, Pixels, TestAppContext, VisualContext as _, VisualTestContext, point, px, size,
    };
    use unicode_segmentation::UnicodeSegmentation as _;

    pub(super) fn native_cases() -> [crate::native_markdown::Case; 4] {
        use crate::native_markdown::Case;
        [
            Case::new(
                "ui::code_editor::markdown::source::spelling::tests::spelling_long_code_remains_editable_with_offscreen_diagnostics",
                spelling_long_code_remains_editable_with_offscreen_diagnostics,
            ),
            Case::new(
                "ui::code_editor::markdown::source::spelling::tests::spelling_full_mixed_paragraph_keeps_visible_text_editable_around_tall_atoms",
                spelling_full_mixed_paragraph_keeps_visible_text_editable_around_tall_atoms,
            ),
            Case::new(
                "ui::code_editor::markdown::source::spelling::tests::spelling_wrapped_starts_follow_native_prose_heading_list_quote_and_zoom",
                spelling_wrapped_starts_follow_native_prose_heading_list_quote_and_zoom,
            ),
            Case::new(
                "ui::code_editor::markdown::source::spelling::tests::spelling_native_hard_breaks_and_image_formula_fragments_do_not_bridge_rows",
                spelling_native_hard_breaks_and_image_formula_fragments_do_not_bridge_rows,
            ),
        ]
    }

    fn settle_geometry(view: &Entity<SourcePresentation>, context: &mut VisualTestContext) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            context
                .executor()
                .advance_clock(std::time::Duration::from_millis(100));
            context.run_until_parked();
            context.update(|window, cx| window.render_frame(cx));
            if view.read_with(context, |view, cx| {
                view.live
                    && view.geometry_is_current(cx)
                    && !view
                        .projection
                        .leaves
                        .iter()
                        .any(|leaf| matches!(leaf.kind, Kind::PendingSource))
            }) {
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "spelling geometry did not settle"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    macro_rules! spelling_fixture {
        ($cx:expr, $original:expr) => {{
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("waves.md");
            std::fs::write(&path, $original).unwrap();
            let (app, mut context, streams) = harness_with_tabs($cx, 1);
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
            view.update_in(&mut context, |view, window, cx| {
                if !view.live {
                    view.toggle_mode(window, cx);
                }
                view.set_soft_wrap(true, cx);
                let end = view.document.read(cx).source().len();
                view.select_range(end..end, cx);
                view.focus.focus(window, cx);
                view.spelling.invalidate();
                view.set_spelling_diagnostics(
                    vec![Diagnostic {
                        ranges: vec![0..end],
                        word: "deterministic OS diagnostic".into(),
                    }],
                    cx,
                );
            });
            (directory, path, app, context, streams, view)
        }};
    }

    fn spelling_long_code_remains_editable_with_offscreen_diagnostics(cx: &mut TestAppContext) {
        use gpui_kit::test::TestEventExt as _;
        let original = format!("# Huge code\n\n```text\n{}\n```\n", "wide ".repeat(80_000));
        let body = original.find("wide").unwrap();
        let (_directory, path, _app, mut context, _streams, view) =
            spelling_fixture!(cx, original.as_bytes());
        view.update_in(&mut context, |view, _, cx| {
            view.set_soft_wrap(false, cx);
            view.select_range(body..body, cx);
        });
        settle_geometry(&view, &mut context);
        view.update_in(&mut context, |view, _, cx| {
            view.spelling.invalidate();
            view.set_spelling_diagnostics(
                vec![Diagnostic {
                    ranges: vec![body..body + 400_000],
                    word: "deterministic OS diagnostic".into(),
                }],
                cx,
            );
        });
        context.update(|window, cx| window.render_frame(cx));
        let position = view.read_with(&context, |view, _| {
            let leaf = view.projection.leaf_at(body).unwrap();
            let geometry = &view.geometry.as_ref().unwrap().1[&leaf.id];
            geometry.position_for_index(0) + point(px(0.), geometry.caret_height() / 2.)
        });
        context.update(|window, cx| {
            window.pointer_down(position, gpui_kit::MouseButton::Left, cx);
            window.pointer_up(position, gpui_kit::MouseButton::Left, cx);
            window.input("X", cx);
        });
        settle_geometry(&view, &mut context);
        let mut expected = original.clone();
        expected.insert(body, 'X');
        view.read_with(&context, |view, cx| {
            assert_eq!(
                view.document.read(cx).encode().unwrap(),
                expected.as_bytes()
            );
        });
        context.dispatch_action(gpui_kit::component::input::Undo);
        settle_geometry(&view, &mut context);
        view.read_with(&context, |view, cx| {
            assert_eq!(
                view.document.read(cx).encode().unwrap(),
                original.as_bytes()
            );
        });
        assert_eq!(std::fs::read(path).unwrap(), original.as_bytes());
    }

    fn spelling_full_mixed_paragraph_keeps_visible_text_editable_around_tall_atoms(
        cx: &mut TestAppContext,
    ) {
        use gpui_kit::test::TestEventExt as _;
        let original = format!(
            "A {}![tall](tall.png) $\\frac{{a}}{{b}}$ {}",
            "a ".repeat(100_000),
            "b ".repeat(100_000),
        );
        assert_eq!(original.len(), 400_034);
        let body = original.find("$ ").unwrap() + 2;
        let (_directory, path, _app, mut context, _streams, view) =
            spelling_fixture!(cx, original.as_bytes());
        image::RgbaImage::from_pixel(12, 96, image::Rgba([0, 0, 255, 255]))
            .save(path.parent().unwrap().join("tall.png"))
            .unwrap();
        settle_geometry(&view, &mut context);
        context.update(|window, cx| window.click("markdown-load-images", cx));
        for wrap in [true, false] {
            view.update_in(&mut context, |view, window, cx| {
                view.set_soft_wrap(wrap, cx);
                view.select_range(body..body, cx);
                // Loading is a command surface with its own focus. A source
                // selection is revealed only by its focused presentation.
                view.focus.focus(window, cx);
            });
            settle_geometry(&view, &mut context);
            view.update_in(&mut context, |view, _, cx| {
                view.spelling.invalidate();
                view.set_spelling_diagnostics(
                    vec![Diagnostic {
                        ranges: vec![0..original.len()],
                        word: "deterministic OS diagnostic".into(),
                    }],
                    cx,
                );
            });
            context.update(|window, cx| window.render_frame(cx));
            let position = view.read_with(&context, |view, cx| {
                let leaf = view.projection.leaf_at(body).unwrap();
                let geometry = &view.geometry.as_ref().unwrap().1[&leaf.id];
                let display = leaf.display_offset(body);
                let caret = view.caret_bounds(body, cx).unwrap();
                let viewport = view.scroll.bounds();
                let visible = super::visible_spelling_text(leaf, geometry, viewport);
                if !wrap {
                    assert!(
                        view.scroll.offset().x < gpui_kit::Pixels::ZERO
                            && view.scroll.max_offset().x > viewport.size.width,
                        "nowrap native text must publish its real horizontal extent: offset={:?}, maximum={:?}, viewport={viewport:?}",
                        view.scroll.offset(),
                        view.scroll.max_offset(),
                    );
                }
                assert!(
                    caret.top() >= viewport.top()
                        && caret.bottom() <= viewport.bottom()
                        && caret.left() >= viewport.left()
                        && caret.right() <= viewport.right(),
                    "post-atom caret must actually be visible: wrap={wrap}, caret={caret:?}, viewport={viewport:?}, display={display}, visible={visible:?}"
                );
                assert!(
                    visible.iter().any(|range| range.contains(&display)),
                    "native visible text after the high atoms retains its diagnostic: wrap={wrap}, caret={caret:?}, viewport={viewport:?}, display={display}, visible={visible:?}"
                );
                caret.center()
            });
            context.update(|window, cx| {
                window.pointer_down(position, gpui_kit::MouseButton::Left, cx);
                window.pointer_up(position, gpui_kit::MouseButton::Left, cx);
                window.input("X", cx);
            });
            let mut expected = original.clone();
            expected.insert(body, 'X');
            view.read_with(&context, |view, cx| {
                assert_eq!(
                    view.document.read(cx).encode().unwrap(),
                    expected.as_bytes()
                );
            });
            context.dispatch_action(gpui_kit::component::input::Undo);
            settle_geometry(&view, &mut context);
            view.read_with(&context, |view, cx| {
                assert_eq!(
                    view.document.read(cx).encode().unwrap(),
                    original.as_bytes()
                );
            });
        }
        assert_eq!(std::fs::read(path).unwrap(), original.as_bytes());
    }

    fn spelling_wrapped_starts_follow_native_prose_heading_list_quote_and_zoom(
        cx: &mut TestAppContext,
    ) {
        // Native font shaping, still in the real workbench's test window; this
        // is not a GPU screenshot or an operating-system window acceptance.
        let original = format!(
            "\u{feff}{}\r\n\r\n# {}\n\n- {}\r\n\r\n> {}\n\nkeep\r\n",
            "W ".repeat(80),
            "W ".repeat(80),
            "田".repeat(140),
            "e\u{301} ".repeat(100),
        );
        let (_directory, path, _app, mut context, _streams, view) =
            spelling_fixture!(cx, original.as_bytes());
        cx.simulate_window_resize(context.window_handle(), size(px(1000.), px(1800.)));
        let revision = view.read_with(&context, |view, cx| view.document.read(cx).revision());
        let mut previous_heights: Option<Vec<Pixels>> = None;
        for rem in [16., 22.] {
            // Match the production typography setting consumed by the root;
            // setting Window rem alone is overwritten by the next root render.
            context.update(|_, cx| {
                cx.global_mut::<crate::core::config::Config>().ui_font_size = rem;
                gpui_kit::component::Theme::update(cx, |theme| theme.font_size = px(rem));
            });
            settle_geometry(&view, &mut context);
            let heights = view.read_with(&context, |view, cx| {
                assert!(view.geometry_is_current(cx));
                let (_, geometries) = view.geometry.as_ref().unwrap();
                let mut heights = Vec::new();
                for kind in [Kind::Prose, Kind::Heading(1)] {
                    let leaf = view
                        .projection
                        .leaves
                        .iter()
                        .find(|leaf| leaf.kind == kind)
                        .unwrap();
                    heights.push(geometries[&leaf.id].caret_height());
                }
                let mut covered = [false; 4];
                for leaf in &view.projection.leaves {
                    let slot = match leaf.kind {
                        Kind::Prose if leaf.text.starts_with('W') => 0,
                        Kind::Heading(1) => 1,
                        Kind::List(_) => 2,
                        Kind::Quote(_) => 3,
                        _ => continue,
                    };
                    let geometry = &geometries[&leaf.id];
                    let origin = geometry.bounds().left();
                    assert_eq!(geometry.position_for_index(0).x, origin);
                    for (ix, grapheme) in leaf.text.grapheme_indices(true) {
                        if grapheme.chars().all(char::is_whitespace) {
                            continue;
                        }
                        let a = geometry.position_for_index(ix);
                        let b = geometry.position_for_index(ix + grapheme.len());
                        if a.y == b.y {
                            continue;
                        }
                        let segment = grapheme_line_box(geometry, ix, grapheme).expect(
                            "a whole paragraph's continuation starts with a spelling segment",
                        );
                        // Independent caret hit tests prove that the emitted
                        // segment belongs to this grapheme on the next row.
                        assert_eq!(segment.left(), origin);
                        assert_eq!(segment.top(), b.y);
                        let leading = point(
                            segment.left() + segment.size.width * 0.25,
                            segment.center().y,
                        );
                        let raw_leading = geometry.index_for_position(leading);
                        assert!(leaf.text.is_char_boundary(raw_leading));
                        assert!(ix <= raw_leading && raw_leading < ix + grapheme.len());
                        assert_eq!(
                            super::super::navigation::hit_test_caret(
                                &leaf.text,
                                geometry,
                                leading,
                            ).index,
                            ix
                        );
                        let trailing = geometry.index_for_position(point(
                            segment.right() - segment.size.width * 0.25,
                            segment.center().y,
                        ));
                        // Native shaping can return a raw caret inside a
                        // combining grapheme, whose physical point need not be
                        // its end. The editor must normalize the right-hand hit
                        // to the complete grapheme boundary below.
                        assert!(leaf.text.is_char_boundary(trailing));
                        assert!(ix < trailing && trailing <= ix + grapheme.len());
                        assert_eq!(
                            super::super::navigation::hit_test_caret(
                                &leaf.text,
                                geometry,
                                point(
                                    segment.right() - segment.size.width * 0.25,
                                    segment.center().y,
                                ),
                            )
                            .index,
                            ix + grapheme.len()
                        );
                        // The same repeated grapheme on the first row is an
                        // independent, native-shaped full-advance reference.
                        let reference = leaf
                            .text
                            .grapheme_indices(true)
                            .find_map(|(start, text)| {
                                let a = geometry.position_for_index(start);
                                let b = geometry.position_for_index(start + text.len());
                                (text == grapheme && a.y == b.y && b.x > a.x).then_some(b.x - a.x)
                            })
                            .unwrap();
                        assert!((segment.size.width - reference).abs() < px(0.01));
                        covered[slot] = true;
                    }
                }
                assert_eq!(
                    covered, [true; 4],
                    "each real layout must exercise soft wrap"
                );
                assert_eq!(
                    view.document.read(cx).encode().unwrap(),
                    original.as_bytes()
                );
                assert_eq!(view.document.read(cx).revision(), revision);
                heights
            });
            if let Some(previous) = previous_heights {
                for (before, after) in previous.into_iter().zip(&heights) {
                    assert!(*after > before, "zoom must produce fresh native geometry");
                }
            }
            previous_heights = Some(heights);
        }
        context.dispatch_action(gpui_kit::component::input::Undo);
        context.dispatch_action(gpui_kit::component::input::Redo);
        context.run_until_parked();
        view.read_with(&context, |view, cx| {
            assert_eq!(
                view.document.read(cx).encode().unwrap(),
                original.as_bytes()
            );
            assert_eq!(view.document.read(cx).revision(), revision);
        });
        assert_eq!(std::fs::read(path).unwrap(), original.as_bytes());
    }

    fn spelling_native_hard_breaks_and_image_formula_fragments_do_not_bridge_rows(
        cx: &mut TestAppContext,
    ) {
        let original = format!(
            "\u{feff}hard first  \r\nhard second\n\n{}![alt](missing.png){}\r\n\r\n{}$x${}\n\nkeep\r\n",
            "image words ".repeat(25),
            " after words".repeat(25),
            "formula words ".repeat(25),
            " after words".repeat(25),
        );
        let (_directory, path, _app, mut context, _streams, view) =
            spelling_fixture!(cx, original.as_bytes());
        cx.simulate_window_resize(context.window_handle(), size(px(1000.), px(1800.)));
        settle_geometry(&view, &mut context);
        let revision = view.read_with(&context, |view, cx| {
            assert!(view.geometry_is_current(cx));
            let (_, geometries) = view.geometry.as_ref().unwrap();
            let mut hard_breaks = 0;
            let mut fragmented = [false; 2];
            let mut local_segments = [0; 2];
            for leaf in &view.projection.leaves {
                let Some(geometry) = geometries.get(&leaf.id) else {
                    continue;
                };
                let slot = if leaf.image_urls().next().is_some() {
                    Some(0)
                } else if leaf.formulas().next().is_some() {
                    Some(1)
                } else {
                    None
                };
                for (ix, grapheme) in leaf.text.grapheme_indices(true) {
                    if grapheme.contains(['\r', '\n']) {
                        assert!(grapheme_line_box(geometry, ix, grapheme).is_none());
                        hard_breaks += 1;
                        continue;
                    }
                    let Some(slot) = slot else {
                        continue;
                    };
                    // Atoms retain their own native bounds; this regression is
                    // about the surrounding InlineFlow text fragments.
                    if leaf.atom_source(ix).is_some() {
                        continue;
                    }
                    let a = geometry.position_for_index(ix);
                    let b = geometry.position_for_index(ix + grapheme.len());
                    let segment = grapheme_line_box(geometry, ix, grapheme);
                    if a.y != b.y {
                        assert_ne!(
                            geometry.index_for_position(point(
                                geometry.position_for_index(0).x,
                                b.y + geometry.caret_height() / 2.,
                            )),
                            ix,
                            "fragment-end index must not masquerade as a whole-leaf wrapped start"
                        );
                        let segment =
                            segment.expect("fragment tail retains its own row's spelling segment");
                        assert_eq!(segment.top(), a.y);
                        assert_eq!(segment.left(), a.x);
                        assert!(segment.right() > segment.left());
                        fragmented[slot] = true;
                    } else if a.x != b.x {
                        let segment =
                            segment.expect("inline fragment keeps its ordinary spelling segment");
                        assert_eq!(segment.left(), a.x.min(b.x));
                        assert_eq!(segment.right(), a.x.max(b.x));
                        assert_eq!(segment.top(), a.y);
                        local_segments[slot] += 1;
                    }
                }
            }
            assert!(
                hard_breaks > 0,
                "hard break must remain in native displayed geometry"
            );
            assert_eq!(
                fragmented, [true; 2],
                "image and formula must each exercise a fragment row boundary"
            );
            assert!(local_segments.into_iter().all(|count| count > 0));
            assert_eq!(
                view.document.read(cx).encode().unwrap(),
                original.as_bytes()
            );
            view.document.read(cx).revision()
        });
        context.dispatch_action(gpui_kit::component::input::Undo);
        context.dispatch_action(gpui_kit::component::input::Redo);
        context.run_until_parked();
        view.read_with(&context, |view, cx| {
            assert_eq!(
                view.document.read(cx).encode().unwrap(),
                original.as_bytes()
            );
            assert_eq!(view.document.read(cx).revision(), revision);
        });
        assert_eq!(std::fs::read(path).unwrap(), original.as_bytes());
    }

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
