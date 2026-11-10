//! 通过生产 workbench lifecycle 验证替换后的文档，不访问旧 Markdown 内核。
use super::super::*;
use crate::ui::app::test_window::harness_with_tabs;
#[cfg(feature = "native-markdown-tests")]
use gpui_kit::test::TestEventExt as _;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{TestAppContext, VisualTestContext};
use std::time::{Duration, Instant};

pub(super) fn settle(context: &mut VisualTestContext) {
    context.run_until_parked();
    // Test windows have no platform frame loop; run the SDK's actual frame callbacks.
    context.update(|window, cx| {
        window.simulate_next_frame(cx);
        window.render_frame(cx);
    });
    context.run_until_parked();
}
#[track_caller]
pub(super) fn until(context: &mut VisualTestContext, mut ready: impl FnMut(&mut VisualTestContext) -> bool) {
    until_state(context, "foundation workflow", |context| {
        (ready(context), ())
    });
}
#[track_caller]
fn until_state<T: std::fmt::Debug>(
    context: &mut VisualTestContext,
    phase: &str,
    mut observe: impl FnMut(&mut VisualTestContext) -> (bool, T),
) {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        settle(context);
        let (ready, state) = observe(context);
        if ready {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "{phase} did not settle; last state: {state:?}"
        );
        context.executor().advance_clock(Duration::from_millis(50));
        std::thread::sleep(Duration::from_millis(5));
    }
}
pub(super) fn save(app: &Entity<LyrelApp>, context: &mut VisualTestContext) {
    context.dispatch_action(crate::core::actions::EditorSave);
    until_state(context, "owning document save", |context| {
        app.read_with(context, |app, _| {
            let state = app
                .active_buffer()
                .map(|file| (file.id(), file.dirty, file.saving, file.conflict));
            (
                state
                    .as_ref()
                    .is_some_and(|(_, dirty, saving, _)| !dirty && !saving),
                state,
            )
        })
    });
}

pub(super) fn lifecycle(cx: &mut TestAppContext) {
    use super::super::test_host::{FileHost, canonical_tempdir};
    let remote = canonical_tempdir();
    let local = canonical_tempdir();
    let original =
        b"\xef\xbb\xbf# Title\r\nalpha  \n\r\nattached$$x$$literal\n<Unknown value={1} />\r\n";
    let remote_file = remote.path().join("foundation.md");
    let local_file = local.path().join("foundation.md");
    std::fs::write(&remote_file, original).unwrap();
    std::fs::write(&local_file, b"LOCAL MUST STAY UNCHANGED").unwrap();
    let host = FileHost::mapped(HostId(237), remote.path());
    let path = PathBuf::from("/foundation.md");
    let (app, mut context, _streams) = harness_with_tabs(cx, 1);
    context.simulate_resize(gpui_kit::size(px(1600.), px(1000.)));
    app.update_in(&mut context, |app, window, cx| {
        app.editor_open_on_host(host.clone(), &path, window, cx)
    });
    until(&mut context, |context| {
        app.read_with(context, |app, _| app.active_buffer().is_some())
    });
    let first = app.read_with(&context, |app, _| {
        app.active_editor_input()
            .unwrap()
            .markdown()
            .unwrap()
            .clone()
    });
    first.read_with(&context, |view, cx| {
        assert!(view.is_live());
        assert_eq!(view.document().read(cx).encode().unwrap(), original);
    });
    save(&app, &mut context);
    assert_eq!(std::fs::read(&remote_file).unwrap(), original);
    context.dispatch_action(crate::core::actions::EditorGoToLine);
    context.update(|window, cx| {
        window.input("2:3", cx);
        window.press("enter", cx);
    });
    settle(&mut context);
    app.read_with(&context, |app, cx| {
        assert_eq!(
            app.active_editor_input()
                .unwrap()
                .read(cx)
                .cursor_position(),
            Position::new(1, 2)
        )
    });
    first.update_in(&mut context, |view, _, cx| view.select_range(0..0, cx));
    first.read_with(&context, |view, _| {
        assert_eq!(
            view.selected_range(),
            0..0,
            "explicit selection replaces the prior go-to target"
        );
    });
    app.update_in(&mut context, |app, window, cx| {
        assert!(app.editor_split(window, cx));
    });
    settle(&mut context);
    first.read_with(&context, |view, _| {
        assert_eq!(
            view.selected_range(),
            0..0,
            "split frame callbacks must preserve the first view's newer selection"
        );
    });
    let second = app.read_with(&context, |app, _| {
        app.active_editor_input()
            .unwrap()
            .markdown()
            .unwrap()
            .clone()
    });
    assert_ne!(first.entity_id(), second.entity_id());
    second.update_in(&mut context, |view, window, cx| {
        view.select_range(8..8, cx);
        view.focus_handle(cx).focus(window, cx);
    });
    settle(&mut context);
    first.read_with(&context, |view, _| {
        assert_eq!(
            view.selected_range(),
            0..0,
            "focusing the second view must not restore the first view's old go-to target"
        );
    });
    context.update(|window, cx| window.input("中🎉", cx));
    settle(&mut context);
    app.read_with(&context, |app, cx| {
        assert!(app.active_buffer().unwrap().dirty);
        assert_eq!(
            app.active_editor_input()
                .unwrap()
                .read(cx)
                .cursor_position(),
            Position::new(1, 3)
        );
    });
    first.read_with(&context, |view, cx| {
        assert_eq!(view.selected_range(), 0..0);
        assert_eq!(view.text(), second.read(cx).text());
        assert_eq!(
            view.document().entity_id(),
            second.read(cx).document().entity_id()
        );
    });
    context.dispatch_action(crate::core::actions::ToggleDocumentPreview);
    context.dispatch_action(gpui_kit::component::input::Undo);
    settle(&mut context);
    second.read_with(&context, |view, cx| {
        assert!(!view.is_live());
        assert_eq!(view.document().read(cx).encode().unwrap(), original);
    });
    context.dispatch_action(gpui_kit::component::input::Redo);
    settle(&mut context);
    let expected = b"\xef\xbb\xbf# Title\r\n\xe4\xb8\xad\xf0\x9f\x8e\x89alpha  \n\r\nattached$$x$$literal\n<Unknown value={1} />\r\n";
    save(&app, &mut context);
    assert_eq!(std::fs::read(&remote_file).unwrap(), expected);
    // A real workbench host-initiated range action must use the same notification/history path.
    second.update_in(&mut context, |view, window, cx| {
        let at = view.text().find("alpha").unwrap();
        view.select_range(at..at + 5, cx);
        view.focus_handle(cx).focus(window, cx);
    });
    context.dispatch_action(commands::TransformToUppercase);
    settle(&mut context);
    app.read_with(&context, |app, _| {
        assert!(app.active_buffer().unwrap().dirty)
    });
    save(&app, &mut context);
    assert!(
        std::fs::read(&remote_file)
            .unwrap()
            .windows(5)
            .any(|window| window == b"ALPHA")
    );
    context.dispatch_action(gpui_kit::component::input::Undo);
    save(&app, &mut context);
    assert_eq!(std::fs::read(&remote_file).unwrap(), expected);
    assert_eq!(
        std::fs::read(&local_file).unwrap(),
        b"LOCAL MUST STAY UNCHANGED"
    );
    app.update_in(&mut context, |app, window, cx| {
        app.editor_close_file(0, window, cx);
        app.editor_close_file(0, window, cx);
        app.editor_open_on_host(host.clone(), &path, window, cx);
    });
    until(&mut context, |context| {
        app.read_with(context, |app, _| app.active_buffer().is_some())
    });
    app.read_with(&context, |app, cx| {
        let file = app.active_buffer().unwrap();
        assert_eq!(file.host.id(), HostId(237));
        assert_eq!(
            file.input
                .markdown()
                .unwrap()
                .read(cx)
                .document()
                .read(cx)
                .encode()
                .unwrap(),
            expected
        );
    });
}

pub(super) fn composition(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("ime.md");
    std::fs::write(&path, "A🎉Z\r\ntail\n").unwrap();
    let (app, mut context, _streams) = harness_with_tabs(cx, 1);
    app.update_in(&mut context, |app, window, cx| {
        app.open_file_in_editor(&path, window, cx)
    });
    until(&mut context, |context| {
        app.read_with(context, |app, _| app.active_buffer().is_some())
    });
    let view = app.read_with(&context, |app, _| {
        app.active_editor_input()
            .unwrap()
            .markdown()
            .unwrap()
            .clone()
    });
    view.update_in(&mut context, |view, window, cx| {
        view.select_range(1..5, cx);
        view.focus_handle(cx).focus(window, cx);
        view.replace_and_mark_text_in_range(None, "zhong", Some(5..5), window, cx);
        view.replace_and_mark_text_in_range(None, "中", Some(1..1), window, cx);
        view.replace_text_in_range(None, "中", window, cx);
    });
    settle(&mut context);
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            "A中Z\r\ntail\n".as_bytes()
        )
    });
    context.dispatch_action(gpui_kit::component::input::Undo);
    settle(&mut context);
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            "A🎉Z\r\ntail\n".as_bytes()
        )
    });
    view.update_in(&mut context, |view, window, cx| {
        view.select_range(1..5, cx);
        view.replace_and_mark_text_in_range(None, "cancel", Some(6..6), window, cx);
        view.replace_and_mark_text_in_range(None, "", Some(0..0), window, cx);
    });
    settle(&mut context);
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            "A🎉Z\r\ntail\n".as_bytes()
        )
    });
    save(&app, &mut context);
    assert_eq!(std::fs::read(&path).unwrap(), "A🎉Z\r\ntail\n".as_bytes());
    app.update_in(&mut context, |app, window, cx| {
        assert!(app.editor_split(window, cx))
    });
    settle(&mut context);
    let owner = app.read_with(&context, |app, _| {
        app.active_editor_input()
            .unwrap()
            .markdown()
            .unwrap()
            .clone()
    });
    assert_ne!(owner.entity_id(), view.entity_id());
    owner.update_in(&mut context, |owner, window, cx| {
        owner.select_range(1..1, cx);
        owner.focus_handle(cx).focus(window, cx);
        owner.replace_and_mark_text_in_range(None, "pending", Some(7..7), window, cx);
    });
    settle(&mut context);
    app.update_in(&mut context, |app, window, cx| {
        app.editor_close_file(0, window, cx)
    });
    drop(owner);
    settle(&mut context);
    let remaining = app.read_with(&context, |app, _| {
        app.active_editor_input()
            .unwrap()
            .markdown()
            .unwrap()
            .clone()
    });
    remaining.update_in(&mut context, |remaining, window, cx| {
        remaining.select_range(0..0, cx);
        remaining.focus_handle(cx).focus(window, cx);
    });
    context.update(|window, cx| window.input("B", cx));
    settle(&mut context);
    remaining.read_with(&context, |remaining, cx| {
        assert!(!remaining.document().read(cx).has_composition());
        assert_eq!(
            remaining.document().read(cx).encode().unwrap(),
            "BApending🎉Z\r\ntail\n".as_bytes()
        );
    });
    context.dispatch_action(gpui_kit::component::input::Undo);
    settle(&mut context);
    remaining.read_with(&context, |remaining, cx| {
        assert_eq!(
            remaining.document().read(cx).encode().unwrap(),
            "Apending🎉Z\r\ntail\n".as_bytes()
        )
    });
    context.dispatch_action(gpui_kit::component::input::Undo);
    settle(&mut context);
    remaining.read_with(&context, |remaining, cx| {
        assert_eq!(
            remaining.document().read(cx).encode().unwrap(),
            "A🎉Z\r\ntail\n".as_bytes()
        )
    });
    save(&app, &mut context);
    assert_eq!(std::fs::read(&path).unwrap(), "A🎉Z\r\ntail\n".as_bytes());
}

#[cfg(feature = "native-markdown-tests")]
pub(super) fn extreme_line(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("complete-extreme-line.md");
    let original = "a".repeat(2_523_166);
    std::fs::write(&path, &original).unwrap();
    let (app, mut context, _streams) = harness_with_tabs(cx, 1);
    context.simulate_resize(gpui_kit::size(px(1600.), px(1000.)));
    app.update_in(&mut context, |app, window, cx| {
        app.open_file_in_editor(&path, window, cx)
    });
    until(&mut context, |context| {
        app.read_with(context, |app, _| app.active_buffer().is_some())
    });
    let view = app.read_with(&context, |app, _| {
        app.active_editor_input()
            .unwrap()
            .markdown()
            .unwrap()
            .clone()
    });
    view.update_in(&mut context, |view, window, cx| {
        view.set_soft_wrap(false, cx);
        view.focus_handle(cx).focus(window, cx);
    });
    for source_mode in [false, true] {
        if source_mode {
            context.dispatch_action(crate::core::actions::ToggleDocumentPreview);
        }
        for at in [0, original.len()] {
            view.update_in(&mut context, |view, _, cx| view.select_range(at..at, cx));
            until_state(
                &mut context,
                "complete extreme-line caret reveal",
                |context| {
                    context.update(|window, cx| {
                        let editor = view.read(cx);
                        let caret = editor.range_to_bounds(&(at..at));
                        let ready = caret.is_some_and(|caret| {
                            caret.top() >= px(0.)
                                && caret.bottom() <= window.viewport_size().height
                                && caret.left() >= px(0.)
                                && caret.right() <= window.viewport_size().width
                        });
                        (
                            ready,
                            (
                                source_mode,
                                at,
                                caret,
                                editor.scroll_offset(),
                                window.viewport_size(),
                                editor.focus_handle(cx).is_focused(window),
                            ),
                        )
                    })
                },
            );
            let caret =
                view.read_with(&context, |view, _| view.range_to_bounds(&(at..at)).unwrap());
            context.update(|window, cx| {
                window.pointer_move(caret.center(), None, cx);
                window.pointer_down(caret.center(), gpui_kit::MouseButton::Left, cx);
                window.pointer_up(caret.center(), gpui_kit::MouseButton::Left, cx);
            });
            view.read_with(&context, |view, _| {
                assert_eq!(
                    view.cursor(),
                    at,
                    "complete extreme-line pointer returns exact source offset"
                )
            });
            context.update(|window, cx| {
                window.press(if at == 0 { "shift-right" } else { "shift-left" }, cx)
            });
            view.read_with(&context, |view, _| {
                assert_eq!(
                    view.selected_range(),
                    if at == 0 { 0..1 } else { at - 1..at }
                )
            });
            view.update_in(&mut context, |view, _, cx| view.select_range(at..at, cx));
            context.update(|window, cx| window.input("XYZ", cx));
            settle(&mut context);
            let mut expected = original.clone();
            expected.insert_str(at, "XYZ");
            view.read_with(&context, |view, cx| {
                assert!(!view.is_soft_wrap());
                assert_eq!(
                    view.document().read(cx).encode().unwrap(),
                    expected.as_bytes()
                );
            });
            save(&app, &mut context);
            assert_eq!(std::fs::read(&path).unwrap(), expected.as_bytes());
            context.dispatch_action(gpui_kit::component::input::Undo);
            settle(&mut context);
            view.read_with(&context, |view, cx| {
                assert_eq!(
                    view.document().read(cx).encode().unwrap(),
                    original.as_bytes()
                )
            });
            save(&app, &mut context);
            assert_eq!(std::fs::read(&path).unwrap(), original.as_bytes());
        }
    }
    app.update_in(&mut context, |app, window, cx| {
        app.editor_close_file(0, window, cx);
        app.open_file_in_editor(&path, window, cx);
    });
    until(&mut context, |context| {
        app.read_with(context, |app, _| app.active_buffer().is_some())
    });
    app.read_with(&context, |app, cx| {
        assert_eq!(
            app.active_buffer()
                .unwrap()
                .input
                .read(cx)
                .text()
                .to_string(),
            original
        )
    });
}

#[cfg(feature = "native-markdown-tests")]
pub(super) fn native_navigation(cx: &mut TestAppContext) {
    use gpui_kit::{EntityInputHandler, MouseButton, point};
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("native-navigation.md");
    let source = format!(
        "{}\n\n中文 English e\u{301} 👩‍💻\n\nlast\n{}",
        "W ".repeat(240),
        "scroll anchor\n".repeat(100)
    );
    std::fs::write(&path, &source).unwrap();
    let (app, mut context, _streams) = harness_with_tabs(cx, 1);
    context.simulate_resize(gpui_kit::size(px(900.), px(1000.)));
    app.update_in(&mut context, |app, window, cx| {
        app.open_file_in_editor(&path, window, cx)
    });
    until(&mut context, |context| {
        app.read_with(context, |app, _| app.active_buffer().is_some())
    });
    let view = app.read_with(&context, |app, _| {
        app.active_editor_input()
            .unwrap()
            .markdown()
            .unwrap()
            .clone()
    });
    view.update_in(&mut context, |view, window, cx| {
        view.select_range(0..0, cx);
        view.focus_handle(cx).focus(window, cx);
    });
    if !view.read_with(&context, |view, _| view.is_soft_wrap()) {
        context.dispatch_action(crate::core::actions::ToggleDocumentWrap);
    }
    view.read_with(&context, |view, _| {
        assert!(
            view.is_soft_wrap(),
            "this native navigation scenario explicitly enables wrap"
        )
    });
    settle(&mut context);
    until(&mut context, |context| {
        view.read_with(context, |view, _| view.range_to_bounds(&(0..0)).is_some())
    });
    let (first, wrap_at, previous_end) = view.read_with(&context, |view, _| {
        let first = view.range_to_bounds(&(0..0)).unwrap();
        // The ASCII fixture lets us find GPUI's first downstream byte, then
        // name the shared boundary byte which also ends the previous row.
        let wrap_at = (1..480)
            .find(|at| {
                view.range_to_bounds(&(*at..*at))
                    .is_some_and(|bounds| bounds.top() > first.top())
            })
            .expect("fixture wraps in native layout")
            - 1;
        (
            first,
            wrap_at,
            view.range_to_bounds(&(wrap_at..wrap_at)).unwrap(),
        )
    });
    view.update_in(&mut context, |view, _, cx| {
        view.select_range(wrap_at..wrap_at, cx)
    });
    settle(&mut context);
    let row_start = view.read_with(&context, |view, _| {
        view.range_to_bounds(&(wrap_at..wrap_at)).unwrap()
    });
    assert!(
        row_start.top() > previous_end.top(),
        "one source offset must retain both visual sides"
    );
    view.update_in(&mut context, |view, _, cx| view.select_range(0..0, cx));
    settle(&mut context);
    let mut previous = first;
    for _ in 0..3 {
        context.update(|window, cx| window.press("down", cx));
        settle(&mut context);
        let next = view.read_with(&context, |view, _| {
            view.range_to_bounds(&(view.cursor()..view.cursor()))
                .unwrap()
        });
        assert!(
            next.top() > previous.top(),
            "Down visits the next visual row; previous={previous:?}, next={next:?}"
        );
        assert!(
            next.top() - previous.top() < first.size.height * 2.,
            "Down must not skip a visual row; previous={previous:?}, next={next:?}"
        );
        previous = next;
    }
    for target in [previous_end.center(), row_start.center()] {
        context.update(|window, cx| {
            window.pointer_move(target, None, cx);
            assert_eq!(
                window.mouse_position(),
                target,
                "SDK click uses the moved pointer position"
            );
            window.pointer_down(target, MouseButton::Left, cx);
            window.pointer_up(target, MouseButton::Left, cx);
        });
        settle(&mut context);
        let before = view.read_with(&context, |view, _| {
            assert_eq!(view.cursor(), wrap_at, "both visual sides resolve to the same source byte");
            let caret = view.range_to_bounds(&(wrap_at..wrap_at)).unwrap();
            assert_eq!(caret.top(), if target.y < row_start.top() { previous_end.top() } else { row_start.top() }, "pointer retains its clicked boundary side; target={target:?}, first={first:?}, upper={previous_end:?}, lower={row_start:?}, caret={caret:?}, scroll={:?}", view.scroll_offset());
            caret
        });
        let ime = view.update_in(&mut context, |view, window, cx| {
            view.bounds_for_range(
                wrap_at..wrap_at,
                gpui_kit::Bounds::new(first.origin, window.viewport_size()),
                window,
                cx,
            )
            .unwrap()
        });
        assert_eq!(
            ime.top(),
            before.top(),
            "IME anchor follows the actual visual caret side"
        );
        context.update(|window, cx| window.press("down", cx));
        settle(&mut context);
        view.read_with(&context, |view, _| {
            assert!(
                view.range_to_bounds(&(view.cursor()..view.cursor()))
                    .unwrap()
                    .top()
                    > before.top()
            )
        });
        context.update(|window, cx| window.press("up", cx));
        settle(&mut context);
        view.read_with(&context, |view, _| {
            assert_eq!(view.cursor(), wrap_at);
            assert_eq!(
                view.range_to_bounds(&(wrap_at..wrap_at)).unwrap().top(),
                before.top(),
                "Down/Up returns to the clicked boundary side"
            );
        });
        if before.top() > first.top() {
            context.update(|window, cx| window.press("up", cx));
            settle(&mut context);
            context.update(|window, cx| window.press("down", cx));
            settle(&mut context);
            view.read_with(&context, |view, _| {
                assert_eq!(view.cursor(), wrap_at);
                assert_eq!(
                    view.range_to_bounds(&(wrap_at..wrap_at)).unwrap().top(),
                    before.top(),
                    "Up/Down returns to the lower boundary side"
                );
            });
        }
        context.update(|window, cx| window.press("shift-left", cx));
        settle(&mut context);
        view.read_with(&context, |view, _| {
            assert_eq!(view.selected_range(), wrap_at - 1..wrap_at);
            assert_eq!(
                view.range_to_bounds(&(wrap_at..wrap_at)).unwrap().top(),
                before.top(),
                "selection anchor retains its boundary side"
            );
        });
        context.update(|window, cx| {
            window.pointer_move(target, None, cx);
            window.pointer_down(target, MouseButton::Left, cx);
            window.pointer_up(target, MouseButton::Left, cx);
            window.input("Q", cx);
        });
        context.dispatch_action(gpui_kit::component::input::Undo);
        settle(&mut context);
        view.read_with(&context, |view, cx| {
            assert_eq!(
                view.document().read(cx).encode().unwrap(),
                source.as_bytes()
            );
            assert_eq!(
                view.range_to_bounds(&(wrap_at..wrap_at)).unwrap().top(),
                before.top(),
                "input/Undo preserves clicked boundary affinity"
            );
        });
    }
    view.update_in(&mut context, |view, _, cx| view.select_range(0..0, cx));
    settle(&mut context);
    let target = point(row_start.left() + px(1.), row_start.center().y);
    context.update(|window, cx| {
        window.pointer_move(target, None, cx);
        window.pointer_down(target, MouseButton::Left, cx);
        window.pointer_up(target, MouseButton::Left, cx);
    });
    settle(&mut context);
    let clicked = view.read_with(&context, |view, _| view.cursor());
    assert!(clicked >= wrap_at && clicked <= wrap_at + 2);
    let clicked_top = view.read_with(&context, |view, _| {
        view.range_to_bounds(&(clicked..clicked)).unwrap().top()
    });
    assert_eq!(clicked_top, row_start.top());
    context.update(|window, cx| window.input("界", cx));
    settle(&mut context);
    let mut inserted = source.clone();
    inserted.insert_str(clicked, "界");
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            inserted.as_bytes()
        )
    });
    context.dispatch_action(gpui_kit::component::input::Undo);
    settle(&mut context);
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            source.as_bytes()
        )
    });
    context.dispatch_action(crate::core::actions::ToggleDocumentPreview);
    let unicode = source.find("中文").unwrap();
    view.update_in(&mut context, |view, _, cx| {
        view.select_range(unicode + "中文 ".len()..unicode + "中文 ".len(), cx)
    });
    context.update(|window, cx| window.press("home", cx));
    view.read_with(&context, |view, _| assert_eq!(view.cursor(), unicode));
    context.update(|window, cx| window.press("end", cx));
    view.read_with(&context, |view, _| {
        assert_eq!(view.cursor(), source.find("\n\nlast").unwrap())
    });
    view.update_in(&mut context, |view, _, cx| {
        view.select_range(unicode..unicode, cx)
    });
    context.update(|window, cx| window.press("shift-right", cx));
    view.read_with(&context, |view, _| {
        assert_eq!(view.selected_range(), unicode..unicode + "中".len())
    });
    context.update(|window, cx| window.input("替", cx));
    context.dispatch_action(gpui_kit::component::input::Undo);
    settle(&mut context);
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            source.as_bytes()
        )
    });
    let word = unicode + "中文 ".len();
    view.update_in(&mut context, |view, _, cx| {
        view.select_range(word..word, cx)
    });
    context.update(|window, cx| {
        window.press(
            if cfg!(target_os = "macos") {
                "alt-right"
            } else {
                "ctrl-right"
            },
            cx,
        )
    });
    view.read_with(&context, |view, _| {
        assert!(view.cursor() >= word + "English".len())
    });
    save(&app, &mut context);
    assert_eq!(std::fs::read(&path).unwrap(), source.as_bytes());
    // Host go-to selection must reveal without stealing the other view's focus.
    app.update_in(&mut context, |app, window, cx| {
        assert!(app.editor_split(window, cx));
    });
    settle(&mut context);
    let second = app.read_with(&context, |app, _| {
        app.active_editor_input()
            .unwrap()
            .markdown()
            .unwrap()
            .clone()
    });
    second.update_in(&mut context, |view, window, cx| {
        view.select_range(source.len()..source.len(), cx);
        view.focus_handle(cx).focus(window, cx);
    });
    until(&mut context, |context| {
        second.read_with(context, |view, _| view.scroll_offset().y < px(0.))
    });
    let other_scroll = second.read_with(&context, |view, _| view.scroll_offset());
    view.update_in(&mut context, |view, _, cx| view.toggle_mode(cx));
    settle(&mut context);
    view.read_with(&context, |view, _| assert!(view.is_live()));
    let tail = source.len() - 1;
    view.update_in(&mut context, |view, _, cx| {
        view.select_range(tail..tail, cx)
    });
    until_state(
        &mut context,
        "unfocused offscreen nonempty line is revealed",
        |context| {
            context.update(|window, cx| {
                let view = view.read(cx);
                let caret = view.range_to_bounds(&(tail..tail));
                (
                    caret.is_some_and(|caret| {
                        caret.top() >= px(0.) && caret.bottom() <= window.viewport_size().height
                    }),
                    (caret, view.scroll_offset()),
                )
            })
        },
    );
    view.update_in(&mut context, |view, _, cx| {
        view.select_range(source.len()..source.len(), cx)
    });
    until(&mut context, |context| {
        context.update(|window, cx| {
            let view = view.read(cx);
            view.range_to_bounds(&(source.len()..source.len()))
                .is_some_and(|caret| {
                    caret.top() >= px(0.) && caret.bottom() <= window.viewport_size().height
                })
        })
    });
    let first_scroll = view.read_with(&context, |view, _| {
        assert!(view.scroll_offset().y < px(0.));
        view.scroll_offset()
    });
    second.read_with(&context, |view, _| {
        assert_eq!(view.scroll_offset(), other_scroll)
    });
    context.update(|window, cx| {
        assert!(
            second.read(cx).focus_handle(cx).is_focused(window),
            "a host selection in another presentation must not take focus"
        )
    });
    view.update_in(&mut context, |view, _, cx| view.select_range(0..0, cx));
    until(&mut context, |context| {
        context.update(|window, cx| {
            view.read(cx).range_to_bounds(&(0..0)).is_some_and(|caret| {
                caret.top() >= px(0.) && caret.bottom() <= window.viewport_size().height
            })
        })
    });
    // The user may scroll away from their caret; a foreign edit must not undo it.
    view.update_in(&mut context, |view, _, cx| {
        view.set_scroll_offset(first_scroll, cx)
    });
    settle(&mut context);
    context.update(|window, cx| window.input("X", cx));
    settle(&mut context);
    view.read_with(&context, |view, _| {
        assert_eq!(view.selected_range(), 0..0);
        assert_eq!(view.scroll_offset(), first_scroll);
    });
    context.dispatch_action(gpui_kit::component::input::Undo);
    save(&app, &mut context);
    assert_eq!(std::fs::read(&path).unwrap(), source.as_bytes());
}

#[cfg(feature = "native-markdown-tests")]
pub(super) fn native_viewport_jump(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("viewport-jump.md");
    let source = format!(
        "head\nvisible 中文 neighbor\n{}",
        "scroll anchor\n".repeat(300)
    );
    std::fs::write(&path, &source).unwrap();
    let (app, mut context, _streams) = harness_with_tabs(cx, 1);
    app.update_in(&mut context, |app, window, cx| {
        app.open_file_in_editor(&path, window, cx)
    });
    until(&mut context, |context| {
        app.read_with(context, |app, _| app.active_buffer().is_some())
    });
    let view = app.read_with(&context, |app, _| {
        app.active_editor_input()
            .unwrap()
            .markdown()
            .unwrap()
            .clone()
    });
    view.update_in(&mut context, |view, window, cx| {
        view.set_soft_wrap(true, cx);
        view.select_range(source.len()..source.len(), cx);
        view.focus_handle(cx).focus(window, cx);
    });
    until(&mut context, |context| {
        view.read_with(context, |view, _| view.scroll_offset().y < px(0.))
    });
    settle(&mut context);
    view.update_in(&mut context, |view, _, cx| view.select_range(0..0, cx));
    context.run_until_parked();
    context.update(|window, cx| window.render_frame(cx));
    // Deliver only requested frames: forced redraws would mask a viewport stuck on placeholders.
    for _ in 0..16 {
        let requested = context.update(|window, cx| window.simulate_next_frame(cx));
        if requested == 0 {
            break;
        }
        context.run_until_parked();
        context.update(|window, cx| window.render_frame(cx));
    }
    let at = "head\nvisible 中".len();
    let caret = context.update(|window, cx| {
        let caret = view
            .read(cx)
            .range_to_bounds(&(at..at))
            .expect("the visible non-caret row must repaint without extra input");
        assert!(caret.top() >= px(0.) && caret.bottom() <= window.viewport_size().height);
        caret
    });
    context.update(|window, cx| {
        window.pointer_move(caret.center(), None, cx);
        window.pointer_down(caret.center(), gpui_kit::MouseButton::Left, cx);
        window.pointer_up(caret.center(), gpui_kit::MouseButton::Left, cx);
    });
    view.read_with(&context, |view, _| assert_eq!(view.cursor(), at));
    context.update(|window, cx| window.input("界", cx));
    save(&app, &mut context);
    let mut expected = source.clone();
    expected.insert_str(at, "界");
    assert_eq!(std::fs::read(&path).unwrap(), expected.as_bytes());
    context.dispatch_action(gpui_kit::component::input::Undo);
    save(&app, &mut context);
    assert_eq!(std::fs::read(&path).unwrap(), source.as_bytes());
}

#[cfg(test)]
#[gpui_kit::test]
fn mdx_source_only_survives_mode_action_split_history_save_reopen(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("components.mdx");
    let source =
        b"\xef\xbb\xbf---\r\ntitle: Literal\n---\r\n<Cards value={1 + 2} />\nattached$$x$$literal";
    std::fs::write(&path, source).unwrap();
    let (app, mut context, _streams) = harness_with_tabs(cx, 1);
    app.update_in(&mut context, |app, window, cx| {
        app.open_file_in_editor(&path, window, cx)
    });
    until(&mut context, |context| {
        app.read_with(context, |app, _| app.active_buffer().is_some())
    });
    let first = app.read_with(&context, |app, _| {
        app.active_editor_input()
            .unwrap()
            .markdown()
            .unwrap()
            .clone()
    });
    context.dispatch_action(crate::core::actions::ToggleDocumentPreview);
    first.read_with(&context, |first, cx| {
        assert!(first.is_source_only());
        assert!(!first.is_live());
        assert_eq!(first.document().read(cx).encode().unwrap(), source);
    });
    app.update_in(&mut context, |app, window, cx| {
        assert!(app.editor_split(window, cx))
    });
    settle(&mut context);
    let second = app.read_with(&context, |app, _| {
        app.active_editor_input()
            .unwrap()
            .markdown()
            .unwrap()
            .clone()
    });
    second.update_in(&mut context, |second, window, cx| {
        assert!(second.is_source_only());
        assert!(!second.is_live());
        second.select_range(0..0, cx);
        second.focus_handle(cx).focus(window, cx);
    });
    context.dispatch_action(crate::core::actions::ToggleDocumentPreview);
    second.read_with(&context, |second, _| assert!(!second.is_live()));
    context.update(|window, cx| window.input("注", cx));
    context.dispatch_action(gpui_kit::component::input::Undo);
    settle(&mut context);
    second.read_with(&context, |second, cx| {
        assert_eq!(second.document().read(cx).encode().unwrap(), source)
    });
    save(&app, &mut context);
    assert_eq!(std::fs::read(&path).unwrap(), source);
    app.update_in(&mut context, |app, window, cx| {
        app.editor_close_file(0, window, cx);
        app.editor_close_file(0, window, cx);
        app.open_file_in_editor(&path, window, cx);
    });
    until(&mut context, |context| {
        app.read_with(context, |app, _| app.active_buffer().is_some())
    });
    app.read_with(&context, |app, cx| {
        let view = app
            .active_editor_input()
            .unwrap()
            .markdown()
            .unwrap()
            .read(cx);
        assert!(view.is_source_only());
        assert!(!view.is_live());
        assert_eq!(view.document().read(cx).encode().unwrap(), source);
    });
}

#[cfg(feature = "native-markdown-tests")]
pub(super) fn native_long_code(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("native-long-code.md");
    let original = format!("# Huge code\n\n```text\n{}\n```\n", "wide ".repeat(80_000));
    let body = original.find("wide").unwrap();
    std::fs::write(&path, &original).unwrap();
    let (app, mut context, _streams) = harness_with_tabs(cx, 1);
    context.simulate_resize(gpui_kit::size(px(1600.), px(1000.)));
    app.update_in(&mut context, |app, window, cx| {
        app.open_file_in_editor(&path, window, cx)
    });
    until(&mut context, |context| {
        app.read_with(context, |app, _| app.active_buffer().is_some())
    });
    let view = app.read_with(&context, |app, _| {
        app.active_editor_input()
            .unwrap()
            .markdown()
            .unwrap()
            .clone()
    });
    view.update_in(&mut context, |view, window, cx| {
        view.set_soft_wrap(false, cx);
        view.select_range(body..body, cx);
        view.focus_handle(cx).focus(window, cx);
        view.set_diagnostics(
            vec![lyrel_markdown_editor::Diagnostic {
                range: body..body + 400_000,
            }],
            cx,
        );
    });
    until(&mut context, |context| {
        context.update(|window, cx| {
            view.read(cx)
                .range_to_bounds(&(body..body))
                .is_some_and(|caret| {
                    caret.top() >= px(0.) && caret.bottom() <= window.viewport_size().height
                })
        })
    });
    let position = view.read_with(&context, |view, _| {
        view.range_to_bounds(&(body..body)).unwrap().center()
    });
    context.update(|window, cx| {
        window.pointer_move(position, None, cx);
        window.pointer_down(position, gpui_kit::MouseButton::Left, cx);
        window.pointer_up(position, gpui_kit::MouseButton::Left, cx);
    });
    view.read_with(&context, |view, cx| {
        assert_eq!(view.cursor(), body);
        assert!(!view.is_soft_wrap());
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            original.as_bytes()
        );
    });
    context.update(|window, cx| window.input("X", cx));
    settle(&mut context);
    let mut expected = original.clone();
    expected.insert(body, 'X');
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            expected.as_bytes()
        )
    });
    save(&app, &mut context);
    assert_eq!(std::fs::read(&path).unwrap(), expected.as_bytes());
    context.dispatch_action(gpui_kit::component::input::Undo);
    settle(&mut context);
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            original.as_bytes()
        )
    });
    save(&app, &mut context);
    assert_eq!(std::fs::read(&path).unwrap(), original.as_bytes());
    app.update_in(&mut context, |app, window, cx| {
        app.editor_close_file(0, window, cx);
        app.open_file_in_editor(&path, window, cx);
    });
    until(&mut context, |context| {
        app.read_with(context, |app, _| app.active_buffer().is_some())
    });
    app.read_with(&context, |app, cx| {
        assert_eq!(
            app.active_buffer()
                .unwrap()
                .input
                .markdown()
                .unwrap()
                .read(cx)
                .document()
                .read(cx)
                .encode()
                .unwrap(),
            original.as_bytes()
        )
    });
}

#[cfg(test)]
#[gpui_kit::test]
fn zorite_foundation_workbench_shared_host_edit_save_reopen(cx: &mut TestAppContext) {
    lifecycle(cx);
}
#[cfg(test)]
#[gpui_kit::test]
fn zorite_foundation_workbench_composition_commit_cancel_undo(cx: &mut TestAppContext) {
    composition(cx);
}
#[cfg(feature = "native-markdown-tests")]
pub(super) fn native_cases() -> impl Iterator<Item = crate::native_markdown::Case> {
    [
        crate::native_markdown::Case::new("ui::code_editor::markdown::foundation::zorite_foundation_workbench_shared_host_edit_save_reopen", lifecycle),
        crate::native_markdown::Case::new("ui::code_editor::markdown::foundation::zorite_foundation_workbench_composition_commit_cancel_undo", composition),
        crate::native_markdown::Case::new("ui::code_editor::markdown::foundation::zorite_foundation_complete_extreme_line_paint_caret_input_save_undo", extreme_line),
        crate::native_markdown::Case::new("ui::code_editor::markdown::foundation::zorite_foundation_native_wrapped_pointer_navigation_unicode_history", native_navigation),
        crate::native_markdown::Case::new("ui::code_editor::markdown::foundation::zorite_foundation_native_distant_jump_repaints_visible_neighbor_without_input", native_viewport_jump),
        crate::native_markdown::Case::new("ui::code_editor::markdown::source::spelling::tests::spelling_long_code_remains_editable_with_offscreen_diagnostics", native_long_code),
    ].into_iter()
}
