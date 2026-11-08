//! Native callbacks retain a document/interaction lease, never a text snapshot.
use super::*;

pub(super) type Token = (EntityId, u64, u64, u64, Option<EntityId>);

/// Geometry is valid only at the layout inputs that produced its shaped rows.
#[derive(Clone, Copy, PartialEq)]
pub(super) struct Frame {
    document: EntityId,
    revision: u64,
    offset: Point<Pixels>,
    viewport: Bounds<Pixels>,
    window_size: Size<Pixels>,
    rem: Pixels,
    scale: f32,
}

impl Frame {
    pub(super) fn new(view: &SourcePresentation, window: &Window, cx: &App) -> Self {
        Self {
            document: view.document.entity_id(),
            revision: view.document.read(cx).revision(),
            offset: view.scroll.offset(),
            viewport: view.scroll.bounds(),
            window_size: window.viewport_size(),
            rem: window.rem_size(),
            scale: window.scale_factor(),
        }
    }

    pub(super) fn is_current(self, view: &SourcePresentation, window: &Window, cx: &App) -> bool {
        self == Self::new(view, window, cx)
            && view.projection.revision == self.revision
            && view
                .geometry
                .as_ref()
                .is_some_and(|(revision, _)| *revision == self.revision)
    }
}

/// A weak handler cannot keep a closed split alive. Each callback verifies the
/// captured document, revision, command generation and composition owner before
/// interpreting any UTF-16 coordinates. Only its own callbacks renew the lease.
pub(super) struct Input {
    view: WeakEntity<SourcePresentation>,
    token: Token,
}

impl Input {
    pub(super) fn new(view: &Entity<SourcePresentation>, cx: &App) -> Self {
        Self {
            view: view.downgrade(),
            token: view.read(cx).input_token(cx),
        }
    }

    fn with_view<R>(
        &mut self,
        window: &mut Window,
        cx: &mut App,
        callback: impl FnOnce(
            &mut SourcePresentation,
            &mut Window,
            &mut Context<SourcePresentation>,
        ) -> R,
    ) -> Option<R> {
        let view = self.view.upgrade()?;
        let current = view.read(cx);
        if current.input_token(cx) != self.token || !current.focus.is_focused(window) {
            return None;
        }
        let result = view.update(cx, |view, cx| callback(view, window, cx));
        self.token = view.read(cx).input_token(cx);
        Some(result)
    }
}

impl InputHandler for Input {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        adjusted: &mut Option<Range<usize>>,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<String> {
        *adjusted = None;
        self.with_view(window, cx, |view, window, cx| {
            view.text_for_range(range, adjusted, window, cx)
        })
        .flatten()
    }

    fn selected_text_range(
        &mut self,
        ignore: bool,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<UTF16Selection> {
        self.with_view(window, cx, |view, window, cx| {
            view.selected_text_range(ignore, window, cx)
        })
        .flatten()
    }

    fn marked_text_range(&mut self, window: &mut Window, cx: &mut App) -> Option<Range<usize>> {
        self.with_view(window, cx, |view, window, cx| {
            view.marked_text_range(window, cx)
        })
        .flatten()
    }

    fn unmark_text(&mut self, window: &mut Window, cx: &mut App) {
        let _ = self.with_view(window, cx, |view, window, cx| view.unmark_text(window, cx));
    }

    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        window: &mut Window,
        cx: &mut App,
    ) {
        let _ = self.with_view(window, cx, |view, window, cx| {
            view.replace_text_in_range(range, text, window, cx)
        });
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        selected: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let _ = self.with_view(window, cx, |view, window, cx| {
            view.replace_and_mark_text_in_range(range, text, selected, window, cx)
        });
    }

    fn paste(&mut self, item: ClipboardItem, window: &mut Window, cx: &mut App) {
        let _ = self.with_view(window, cx, |view, window, cx| view.paste(item, window, cx));
    }

    fn bounds_for_range(
        &mut self,
        range: Range<usize>,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Bounds<Pixels>> {
        self.with_view(window, cx, |view, window, cx| {
            view.bounds_for_range(range, view.scroll.bounds(), window, cx)
        })
        .flatten()
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<usize> {
        self.with_view(window, cx, |view, window, cx| {
            view.character_index_for_point(point, window, cx)
        })
        .flatten()
    }

    fn set_selected_text_range(&mut self, range: Range<usize>, window: &mut Window, cx: &mut App) {
        let _ = self.with_view(window, cx, |view, window, cx| {
            view.set_selected_text_range(range, window, cx)
        });
    }

    fn text_length_utf16(&mut self, window: &mut Window, cx: &mut App) -> Option<usize> {
        self.with_view(window, cx, |view, window, cx| {
            view.text_length_utf16(window, cx)
        })
        .flatten()
    }

    fn element_bounds(&mut self, window: &mut Window, cx: &mut App) -> Option<Bounds<Pixels>> {
        self.with_view(window, cx, |view, _, _| view.scroll.bounds())
    }

    fn accepts_text_input(&mut self, window: &mut Window, cx: &mut App) -> bool {
        self.with_view(window, cx, |_, _, _| true).unwrap_or(false)
    }

    fn prefers_ime_for_printable_keys(&mut self, window: &mut Window, cx: &mut App) -> bool {
        self.accepts_text_input(window, cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::app::test_window::harness_with_tabs;
    use gpui_kit::test::TestWindowExt as _;
    use gpui_kit::{InputHandler as _, VisualContext as _};

    #[gpui_kit::test]
    fn composition_lease_survives_workbench_handoffs_and_rejects_old_callbacks(
        cx: &mut TestAppContext,
    ) {
        use crate::ui::code_editor::test_host::FileHost;
        cx.update(crate::ui::ime::install);
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("composition.md");
        let original = "A🎉Z\r\ntail\n";
        std::fs::write(&path, original).unwrap();
        let host = FileHost::new(lyrel_core::host::HostId(208));
        let (app, mut context, _streams) = harness_with_tabs(cx, 1);
        app.update_in(&mut context, |app, window, cx| {
            app.editor_open_on_host(host.clone(), &path, window, cx)
        });
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let first = loop {
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
        first.update_in(&mut context, |view, window, cx| {
            view.select_range(1..5, cx);
            view.focus.focus(window, cx);
        });
        let mut old = context.update(|window, cx| {
            window.render_frame(cx);
            let mut input = Input::new(&first, cx);
            input.replace_and_mark_text_in_range(None, "你🎉", Some(3..3), window, cx);
            assert_eq!(input.marked_text_range(window, cx), Some(1..4));
            assert_eq!(
                input.selected_text_range(false, window, cx).unwrap().range,
                4..4
            );
            input
        });
        context.update(|window, cx| {
            window.render_frame(cx);
            window.render_frame(cx);
            assert!(
                first.read(cx).is_composing(),
                "registering the same owner again must retain preedit"
            );
        });
        // The production source/live action is a commit boundary, not a new editor.
        context.dispatch_action(crate::core::actions::ToggleDocumentPreview);
        context.update(|window, cx| {
            old.replace_text_in_range(None, "OBSOLETE", window, cx);
            assert_eq!(
                first.read(cx).document.read(cx).encode().unwrap(),
                "A你🎉Z\r\ntail\n".as_bytes()
            );
            assert!(!first.read(cx).is_composing());
        });
        context.dispatch_action(gpui_kit::component::input::Undo);
        context.run_until_parked();
        app.read_with(&context, |app, cx| {
            assert_eq!(
                first.read(cx).document.read(cx).encode().unwrap(),
                original.as_bytes()
            );
            assert!(!app.active_buffer().unwrap().dirty);
        });
        context.dispatch_action(gpui_kit::component::input::Redo);
        context.update(|window, cx| {
            first.update(cx, |view, cx| view.select_range(1..8, cx));
            let mut input = Input::new(&first, cx);
            input.replace_and_mark_text_in_range(None, "你🎉", Some(3..3), window, cx);
        });
        context.dispatch_action(gpui_kit::component::input::Search);
        context.run_until_parked();
        context.update(|window, cx| {
            window.render_frame(cx);
            assert!(!first.read(cx).is_composing());
            assert!(!first.read(cx).focus.is_focused(window));
        });
        context.dispatch_action(gpui_kit::component::input::Escape);
        context.run_until_parked();
        context.update(|window, cx| {
            assert!(first.read(cx).focus.is_focused(window));
            assert_eq!(
                first.read(cx).document.read(cx).encode().unwrap(),
                "A你🎉Z\r\ntail\n".as_bytes()
            );
        });
        app.update_in(&mut context, |app, window, cx| {
            assert!(app.editor_split(window, cx))
        });
        context.run_until_parked();
        let second = app.read_with(&context, |app, _| {
            app.active_editor_input()
                .unwrap()
                .markdown()
                .unwrap()
                .clone()
        });
        assert_ne!(first.entity_id(), second.entity_id());
        context.update(|window, cx| {
            window.render_frame(cx);
            second.update(cx, |view, cx| {
                view.select_range(0..0, cx);
                view.focus.focus(window, cx);
            });
            let mut input = Input::new(&second, cx);
            input.replace_text_in_range(None, "kept ", window, cx);
            input.replace_and_mark_text_in_range(None, "cancel", Some(6..6), window, cx);
            input.replace_and_mark_text_in_range(None, "", Some(0..0), window, cx);
            assert_eq!(
                second.read(cx).document.read(cx).encode().unwrap(),
                "kept A你🎉Z\r\ntail\n".as_bytes()
            );
            // A retained stale native handler cannot undo or commit through a new owner.
            old.unmark_text(window, cx);
            old.replace_text_in_range(Some(0..1), "bad", window, cx);
            assert_eq!(second.read(cx).selection(), Selection::caret(5));
        });
        context.run_until_parked();
        first.read_with(&context, |view, _| {
            assert_eq!(view.selection(), Selection::caret(13))
        });
        context.dispatch_action(gpui_kit::component::input::Undo);
        context.run_until_parked();
        context.update(|window, cx| {
            assert_eq!(
                second.read(cx).document.read(cx).encode().unwrap(),
                "A你🎉Z\r\ntail\n".as_bytes()
            );
            // Focus transfer commits only the previous Markdown owner, while a
            // subsequent candidate/cancel callback from that handler is obsolete.
            second.update(cx, |view, cx| view.select_range(0..0, cx));
            let mut input = Input::new(&second, cx);
            input.replace_and_mark_text_in_range(None, "hand", Some(4..4), window, cx);
        });
        app.update_in(&mut context, |app, window, cx| {
            assert!(app.editor_focus_group(true, window, cx));
        });
        context.run_until_parked();
        context.update(|window, cx| {
            window.render_frame(cx);
            assert!(!second.read(cx).is_composing());
            assert_eq!(
                first.read(cx).document.entity_id(),
                second.read(cx).document.entity_id()
            );
            assert_eq!(first.read(cx).selection(), Selection::caret(12));
            assert_eq!(second.read(cx).selection(), Selection::caret(4));
            first.update(cx, |view, cx| view.select_range(0..0, cx));
            let mut input = Input::new(&first, cx);
            input.replace_and_mark_text_in_range(None, "close", Some(5..5), window, cx);
        });
        app.update_in(&mut context, |app, window, cx| {
            app.editor_close_file(0, window, cx)
        });
        context.run_until_parked();
        app.update_in(&mut context, |app, window, cx| app.focus_editor(window, cx));
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
        let saved = "closehandA你🎉Z\r\ntail\n".as_bytes();
        first.read_with(&context, |view, cx| {
            assert_eq!(view.document.read(cx).encode().unwrap(), saved)
        });
        assert_eq!(std::fs::read(&path).unwrap(), saved);
        assert_eq!(host.writes.load(std::sync::atomic::Ordering::SeqCst), 1);
        app.update_in(&mut context, |app, window, cx| {
            app.editor_close_file(0, window, cx);
            app.editor_open_on_host(host.clone(), &path, window, cx);
        });
        loop {
            context.run_until_parked();
            if app.read_with(&context, |app, cx| {
                app.active_buffer().is_some_and(|file| {
                    file.input.read(cx).text().to_string() == "closehandA你🎉Z\ntail\n"
                })
            }) {
                break;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        context.update(|window, cx| old.replace_text_in_range(None, "replacement", window, cx));
        app.read_with(&context, |app, cx| {
            assert_eq!(
                app.active_buffer().unwrap().host.id(),
                lyrel_core::host::HostId(208)
            );
            assert_ne!(
                app.active_editor_input()
                    .unwrap()
                    .markdown()
                    .unwrap()
                    .read(cx)
                    .document
                    .entity_id(),
                first.read(cx).document.entity_id()
            );
            assert_eq!(
                app.active_editor_input()
                    .unwrap()
                    .markdown()
                    .unwrap()
                    .read(cx)
                    .document
                    .read(cx)
                    .encode()
                    .unwrap(),
                saved
            );
        });
        // Reuse the owning Host's delayed read seam: a reload started before
        // newer committed input/preedit must not land over that input.
        let owners = std::sync::Arc::strong_count(&host);
        let release = host.pause_next_read();
        app.update_in(&mut context, |app, window, cx| {
            app.editor_reload_from_disk(app.active_buffer().unwrap().id(), window, cx);
        });
        loop {
            context.run_until_parked();
            if host.entered.load(std::sync::atomic::Ordering::SeqCst) {
                break;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let replacement = app.read_with(&context, |app, _| {
            app.active_editor_input()
                .unwrap()
                .markdown()
                .unwrap()
                .clone()
        });
        context.update(|window, cx| {
            window.input("fresh ", cx);
            let mut input = Input::new(&replacement, cx);
            input.replace_and_mark_text_in_range(None, "cancel", Some(6..6), window, cx);
            input.replace_and_mark_text_in_range(None, "", Some(0..0), window, cx);
        });
        release.send(()).unwrap();
        loop {
            context.run_until_parked();
            if std::sync::Arc::strong_count(&host) <= owners {
                break;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        replacement.read_with(&context, |view, cx| {
            assert_eq!(
                view.document.read(cx).encode().unwrap(),
                "fresh closehandA你🎉Z\r\ntail\n".as_bytes()
            );
        });
        context.dispatch_action(gpui_kit::component::input::Undo);
        context.run_until_parked();
        app.read_with(&context, |app, cx| {
            assert_eq!(
                replacement.read(cx).document.read(cx).encode().unwrap(),
                saved
            );
            assert!(!app.active_buffer().unwrap().dirty);
        });
    }

    #[gpui_kit::test]
    fn composition_candidates_use_current_visible_utf16_caret_not_stale_frames(
        cx: &mut TestAppContext,
    ) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("candidate.md");
        std::fs::write(&path, format!("{}\n\ntail", "wrapped words ".repeat(80))).unwrap();
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
        view.update_in(&mut context, |view, window, cx| {
            view.select_range(140..140, cx);
            view.focus.focus(window, cx);
        });
        context.update(|window, cx| {
            window.render_frame(cx);
            let mut input = Input::new(&view, cx);
            input.replace_and_mark_text_in_range(None, "你🎉", Some(300..300), window, cx);
            assert_eq!(
                input.selected_text_range(false, window, cx).unwrap().range,
                143..143
            );
        });
        loop {
            context
                .executor()
                .advance_clock(std::time::Duration::from_millis(100));
            context.run_until_parked();
            if context.update(|window, cx| {
                window.render_frame(cx);
                window.render_frame(cx);
                let mut input = Input::new(&view, cx);
                input.bounds_for_range(140..143, window, cx).is_some()
            }) {
                break;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        context.update(|window, cx| {
            let mut input = Input::new(&view, cx);
            let caret = view.read(cx).caret_bounds(147, cx).unwrap();
            assert_eq!(input.bounds_for_range(140..143, window, cx), Some(caret));
            assert_eq!(
                input.character_index_for_point(caret.center(), window, cx),
                Some(143)
            );
            // ScrollHandle can move before the next layout. No old candidate is usable.
            view.read(cx).scroll.set_offset(point(px(0.), px(-20.)));
            assert!(input.bounds_for_range(140..143, window, cx).is_none());
            window.render_frame(cx);
            window.render_frame(cx);
            let previous = window.rem_size();
            window.set_rem_size(previous * 1.25);
            assert!(input.bounds_for_range(140..143, window, cx).is_none());
            window.set_rem_size(previous);
        });
        cx.simulate_window_resize(context.window_handle(), size(px(720.), px(640.)));
        loop {
            context.run_until_parked();
            if context.update(|window, cx| {
                window.render_frame(cx);
                window.render_frame(cx);
                let mut input = Input::new(&view, cx);
                let candidate = input.bounds_for_range(140..143, window, cx);
                candidate.is_some() && candidate == view.read(cx).caret_bounds(147, cx)
            }) {
                break;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }
}
