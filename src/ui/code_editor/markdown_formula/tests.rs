//! Production workbench consumers: real hosted tree, shared history and owning Host saves.
use super::super::*;
use crate::ui::app::test_window::harness_with_tabs;
#[cfg(feature = "native-markdown-tests")]
use gpui_kit::test::TestEventExt as _;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{TestAppContext, VisualTestContext, size};
use std::time::{Duration, Instant};

fn settle(context: &mut VisualTestContext) {
    context.run_until_parked();
    context.update(|window, cx| {
        window.simulate_next_frame(cx);
        window.render_frame(cx);
    });
    context.run_until_parked();
}
fn until(context: &mut VisualTestContext, mut ready: impl FnMut(&mut VisualTestContext) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        settle(context);
        if ready(context) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "formula workbench consumer did not settle"
        );
        context.executor().advance_clock(Duration::from_millis(50));
        std::thread::sleep(Duration::from_millis(5));
    }
}
fn save(app: &Entity<LyrelApp>, context: &mut VisualTestContext) {
    context.dispatch_action(crate::core::actions::EditorSave);
    until(context, |context| {
        app.read_with(context, |app, _| {
            app.active_buffer()
                .is_some_and(|file| !file.dirty && !file.saving)
        })
    });
}
fn active(
    app: &Entity<LyrelApp>,
    context: &VisualTestContext,
) -> Entity<lyrel_markdown_editor::EditorState> {
    app.read_with(context, |app, _| {
        app.active_editor_input()
            .unwrap()
            .markdown()
            .unwrap()
            .clone()
    })
}
fn enter(
    view: &Entity<lyrel_markdown_editor::EditorState>,
    offset: usize,
    context: &mut VisualTestContext,
) {
    view.update_in(context, |view, window, cx| {
        view.select_range(offset..offset, cx);
        view.focus_handle(cx).focus(window, cx);
    });
    settle(context);
    context.update(|window, cx| window.press("right", cx));
    settle(context);
}

pub(super) fn formula_commit_cancel_history_host_save(cx: &mut TestAppContext) {
    use super::super::test_host::{FileHost, canonical_tempdir};
    use lyrel_markdown_editor::math::editor::view::ConfirmFormula;
    let remote = canonical_tempdir();
    let local = canonical_tempdir();
    let original =
        b"\xef\xbb\xbflead attached$$x$$literal $a$ end\r\n\r\n$$\n\\sqrt{x}\r\n$$\r\nlast\n";
    let file = remote.path().join("formula.md");
    let decoy = local.path().join("formula.md");
    std::fs::write(&file, original).unwrap();
    std::fs::write(&decoy, b"LOCAL UNCHANGED").unwrap();
    let host = FileHost::mapped(HostId(241), remote.path());
    let path = PathBuf::from("/formula.md");
    let (app, mut context, _) = harness_with_tabs(cx, 1);
    context.simulate_resize(size(px(1400.), px(1000.)));
    app.update_in(&mut context, |app, window, cx| {
        app.editor_open_on_host(host.clone(), &path, window, cx)
    });
    until(&mut context, |context| {
        app.read_with(context, |app, _| app.active_buffer().is_some())
    });
    let first = active(&app, &context);
    save(&app, &mut context);
    assert_eq!(
        std::fs::read(&file).unwrap(),
        original,
        "displaying attached math does not normalize it"
    );
    let at = first.read_with(&context, |view, _| view.text().find("$a$").unwrap());
    enter(&first, at, &mut context);
    context.update(|window, cx| window.input("z", cx));
    settle(&mut context);
    first.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            original,
            "the structural draft is not a document edit"
        )
    });
    context.update(|window, cx| window.press("escape", cx));
    settle(&mut context);
    first.read_with(&context, |view, cx| {
        assert_eq!(view.document().read(cx).encode().unwrap(), original)
    });
    enter(&first, at, &mut context);
    context.update(|window, cx| {
        window.input("\\frac", cx);
        window.press("enter", cx);
        window.input("1", cx);
        window.press("down", cx);
        window.input("2", cx);
        window.press("enter", cx);
    });
    settle(&mut context);
    let expected = b"\xef\xbb\xbflead attached$$x$$literal $\\frac{1}{2} a$ end\r\n\r\n$$\n\\sqrt{x}\r\n$$\r\nlast\n";
    first.read_with(&context, |view, cx| {
        assert_eq!(view.document().read(cx).encode().unwrap(), expected)
    });
    app.read_with(&context, |app, _| {
        assert!(app.active_buffer().unwrap().dirty)
    });
    context.dispatch_action(crate::core::actions::ToggleDocumentPreview);
    context.dispatch_action(gpui_kit::component::input::Undo);
    settle(&mut context);
    first.read_with(&context, |view, cx| {
        assert!(!view.is_live());
        assert_eq!(view.document().read(cx).encode().unwrap(), original);
    });
    context.dispatch_action(gpui_kit::component::input::Redo);
    save(&app, &mut context);
    assert_eq!(std::fs::read(&file).unwrap(), expected);
    context.dispatch_action(crate::core::actions::ToggleDocumentPreview);
    // The second presentation gets its own structural seat, with the same Document/history.
    app.update_in(&mut context, |app, window, cx| {
        assert!(app.editor_split(window, cx))
    });
    settle(&mut context);
    let second = active(&app, &context);
    assert_ne!(first.entity_id(), second.entity_id());
    let display = second.read_with(&context, |view, _| view.text().find("$$\n").unwrap());
    enter(&second, display, &mut context);
    context.update(|window, cx| {
        window.input("y", cx);
        window.press("escape", cx);
    });
    settle(&mut context);
    second.read_with(&context, |view, cx| {
        assert_eq!(view.document().read(cx).encode().unwrap(), expected)
    });
    // A changed sibling revision expires the old seat; it may never commit to a shifted range.
    enter(&second, display, &mut context);
    context.update(|window, cx| window.input("q", cx));
    second.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            expected,
            "the second presentation keeps its structural draft out of the document"
        );
    });
    first.update_in(&mut context, |view, _, cx| {
        view.replace_ranges(vec![(0..0, "prefix ".into())], cx)
    });
    // 模拟原生菜单的迟到确认；兄弟 revision 已改变，旧草稿不能覆盖移动后的 range。
    context.dispatch_action(ConfirmFormula);
    settle(&mut context);
    // 原文的 \\sqrt 本来含 q；必须比较精确字节，不能按整篇是否含 q 判断草稿泄漏。
    let prefixed = [&expected[..3], b"prefix ", &expected[3..]].concat();
    second.read_with(&context, |view, cx| {
        assert_eq!(view.document().read(cx).encode().unwrap(), prefixed)
    });
    first.update_in(&mut context, |view, window, cx| {
        view.focus_handle(cx).focus(window, cx)
    });
    context.dispatch_action(gpui_kit::component::input::Undo);
    settle(&mut context);
    second.read_with(&context, |view, cx| {
        assert_eq!(view.document().read(cx).encode().unwrap(), expected)
    });
    // Ordinary display edit preserves the mixed fence line endings, not just surrounding prose.
    enter(&second, display, &mut context);
    context.update(|window, cx| {
        window.input("y", cx);
        window.press(
            if cfg!(target_os = "macos") {
                "cmd-s"
            } else {
                "ctrl-s"
            },
            cx,
        );
    });
    settle(&mut context);
    let display_expected = b"\xef\xbb\xbflead attached$$x$$literal $\\frac{1}{2} a$ end\r\n\r\n$$\ny \\sqrt{x}\r\n$$\r\nlast\n";
    save(&app, &mut context);
    assert_eq!(std::fs::read(&file).unwrap(), display_expected);
    assert_eq!(std::fs::read(&decoy).unwrap(), b"LOCAL UNCHANGED");
    app.update_in(&mut context, |app, window, cx| {
        app.editor_close_file(0, window, cx);
        app.editor_close_file(0, window, cx);
        app.editor_open_on_host(host.clone(), &path, window, cx);
    });
    until(&mut context, |context| {
        app.read_with(context, |app, _| app.active_buffer().is_some())
    });
    active(&app, &context).read_with(&context, |view, cx| {
        assert_eq!(view.document().read(cx).encode().unwrap(), display_expected)
    });
}

pub(super) fn unsupported_and_unchanged_are_lossless(cx: &mut TestAppContext) {
    use super::super::test_host::canonical_tempdir;
    let dir = canonical_tempdir();
    let path = dir.path().join("formula-unsupported.md");
    let original = "before $\\color{red}{x}$ after\n$\\frac{1}{2}$\n";
    std::fs::write(&path, original).unwrap();
    let (app, mut context, _) = harness_with_tabs(cx, 1);
    app.update_in(&mut context, |app, window, cx| {
        app.open_file_in_editor(&path, window, cx)
    });
    until(&mut context, |context| {
        app.read_with(context, |app, _| app.active_buffer().is_some())
    });
    let view = active(&app, &context);
    enter(&view, original.find('$').unwrap(), &mut context);
    view.update_in(&mut context, |view, window, cx| {
        assert!(
            view.focus_handle(cx).is_focused(window),
            "unsupported parse never steals document focus"
        )
    });
    assert!(!app.read_with(&context, |app, _| app.active_buffer().unwrap().dirty));
    // Supported entry followed by commit without edits retains the exact original spelling.
    enter(&view, original.rfind("$\\frac").unwrap(), &mut context);
    context.update(|window, cx| window.press("enter", cx));
    settle(&mut context);
    save(&app, &mut context);
    assert_eq!(std::fs::read(path).unwrap(), original.as_bytes());
}

#[cfg(feature = "native-markdown-tests")]
pub(super) fn native_full_mixed_tall_formula_pointer_fragments(cx: &mut TestAppContext) {
    use super::super::test_host::canonical_tempdir;
    let dir = canonical_tempdir();
    let path = dir.path().join("mixed-tall-inline.md");
    // The complete fixed reference fixture: 400034 bytes, SHA-256 15623d6f02e1d9c6f951445996b87878250bd3b3aa8b4d41323ef2e8b82ae8ba.
    let original = format!(
        "A {}![tall](tall.png) $\\frac{{a}}{{b}}$ {}",
        "a ".repeat(100_000),
        "b ".repeat(100_000)
    );
    assert_eq!(original.len(), 400_034);
    std::fs::write(&path, &original).unwrap();
    image::RgbaImage::from_pixel(12, 96, image::Rgba([80, 100, 180, 255]))
        .save(dir.path().join("tall.png"))
        .unwrap();
    let (app, mut context, _) = harness_with_tabs(cx, 1);
    context.simulate_resize(size(px(1400.), px(900.)));
    app.update_in(&mut context, |app, window, cx| {
        app.open_file_in_editor(&path, window, cx)
    });
    until(&mut context, |context| {
        app.read_with(context, |app, _| app.active_buffer().is_some())
    });
    let view = active(&app, &context);
    until(&mut context, |context| {
        app.read_with(context, |app, cx| {
            app.active_buffer()
                .unwrap()
                .markdown_resources
                .as_ref()
                .unwrap()
                .read(cx)
                .is_loaded("tall.png")
        })
    });
    let formula = original.find("$\\frac").unwrap();
    let formula_range = formula..formula + "$\\frac{a}{b}$".len();
    for wrap in [true, false] {
        view.update_in(&mut context, |view, _, cx| view.set_soft_wrap(wrap, cx));
        view.update_in(&mut context, |view, window, cx| {
            view.select_range(formula_range.end + 1..formula_range.end + 1, cx);
            view.focus_handle(cx).focus(window, cx);
        });
        until(&mut context, |context| {
            view.read_with(context, |view, _| {
                view.formula_bounds(&formula_range)
                    .is_some_and(|bounds| bounds.size.height > px(20.))
            })
        });
        for at in [
            0,
            original.find("![tall]").unwrap() - 2,
            formula_range.end + 1,
            original.len() - 2,
        ] {
            view.update_in(&mut context, |view, window, cx| {
                view.select_range(at..at, cx);
                view.focus_handle(cx).focus(window, cx);
            });
            until(&mut context, |context| {
                view.read_with(context, |view, _| view.range_to_bounds(&(at..at)).is_some())
            });
            let target = view.read_with(&context, |view, _| {
                view.range_to_bounds(&(at..at)).unwrap().center()
            });
            context.update(|window, cx| {
                window.pointer_move(target, None, cx);
                window.pointer_down(target, MouseButton::Left, cx);
                window.pointer_up(target, MouseButton::Left, cx);
            });
            settle(&mut context);
            view.read_with(&context, |view, _| {
                assert_eq!(
                    view.cursor(),
                    at,
                    "native glyph after tall atoms/fragment boundary maps to exact source"
                )
            });
            context.update(|window, cx| window.input("Z", cx));
            settle(&mut context);
            view.read_with(&context, |view, _| {
                let mut expected = original.clone();
                expected.insert(at, 'Z');
                assert_eq!(view.text(), expected);
            });
            context.dispatch_action(gpui_kit::component::input::Undo);
            settle(&mut context);
        }
        view.update_in(&mut context, |view, window, cx| {
            view.select_range(formula_range.end + 1..formula_range.end + 1, cx);
            view.focus_handle(cx).focus(window, cx);
        });
        until(&mut context, |context| {
            view.read_with(context, |view, _| {
                view.formula_bounds(&formula_range).is_some()
            })
        });
        let bounds = view.read_with(&context, |view, _| {
            view.formula_bounds(&formula_range).unwrap()
        });
        assert!(
            bounds.size.height > px(20.),
            "the formula must be a real tall raster, not raw source"
        );
        context.update(|window, cx| {
            let at = bounds.center();
            window.pointer_move(at, None, cx);
            window.pointer_down(at, MouseButton::Left, cx);
            window.pointer_up(at, MouseButton::Left, cx);
        });
        settle(&mut context);
        context.update(|window, cx| {
            window.input("x", cx);
            window.press("escape", cx);
        });
        settle(&mut context);
        save(&app, &mut context);
        assert_eq!(std::fs::read(&path).unwrap(), original.as_bytes());
    }
}

#[cfg(feature = "native-markdown-tests")]
pub(super) fn native_hard_break_formula_fragments(cx: &mut TestAppContext) {
    use super::super::test_host::canonical_tempdir;
    let dir = canonical_tempdir();
    let path = dir.path().join("hard-break-formula.md");
    let original = format!(
        "before ![tall](tall.png) $\\frac{{1}}{{\\sqrt{{x}}}}$  \nnext {}![tall](tall.png) $\\left(\\frac{{a}}{{b}}\\right)$ tail\n",
        "fragment ".repeat(20_000)
    );
    std::fs::write(&path, &original).unwrap();
    image::RgbaImage::from_pixel(12, 96, image::Rgba([80, 100, 180, 255]))
        .save(dir.path().join("tall.png"))
        .unwrap();
    let (app, mut context, _) = harness_with_tabs(cx, 1);
    context.simulate_resize(size(px(1400.), px(900.)));
    app.update_in(&mut context, |app, window, cx| {
        app.open_file_in_editor(&path, window, cx)
    });
    until(&mut context, |context| {
        app.read_with(context, |app, _| app.active_buffer().is_some())
    });
    let view = active(&app, &context);
    until(&mut context, |context| {
        app.read_with(context, |app, cx| {
            app.active_buffer()
                .unwrap()
                .markdown_resources
                .as_ref()
                .unwrap()
                .read(cx)
                .is_loaded("tall.png")
        })
    });
    let formula_start = original.find('$').unwrap();
    let formula_end = original[formula_start + 1..].find('$').unwrap() + formula_start + 2;
    view.update_in(&mut context, |view, window, cx| {
        let next = original.find("next").unwrap();
        view.select_range(next..next, cx);
        view.focus_handle(cx).focus(window, cx);
    });
    until(&mut context, |context| {
        view.read_with(context, |view, _| {
            view.formula_bounds(&(formula_start..formula_end)).is_some()
        })
    });
    view.read_with(&context, |view, _| {
        let formula = view.formula_bounds(&(formula_start..formula_end)).unwrap();
        let next = original.find("next").unwrap();
        let caret = view.range_to_bounds(&(next..next)).unwrap();
        assert!(formula.size.height > px(20.), "use the real FormulaProvider raster");
        assert!(caret.top() >= formula.bottom(), "a hard break cannot bridge a tall image/formula row");
    });
    for at in [
        original.find("next").unwrap(),
        original.rfind("tail").unwrap(),
    ] {
        let end = if at == original.find("next").unwrap() {
            formula_end
        } else {
            original.rfind('$').unwrap() + 1
        };
        let start = original[..end - 1].rfind('$').unwrap();
        view.update_in(&mut context, |view, window, cx| {
            view.select_range(at..at, cx);
            view.focus_handle(cx).focus(window, cx);
        });
        until(&mut context, |context| {
            view.read_with(context, |view, _| {
                view.range_to_bounds(&(at..at)).is_some()
                    && view.formula_bounds(&(start..end))
                        .is_some_and(|bounds| bounds.size.height > px(20.))
            })
        });
        let target = view.read_with(&context, |view, _| {
            view.range_to_bounds(&(at..at)).unwrap().center()
        });
        context.update(|window, cx| {
            window.pointer_move(target, None, cx);
            window.pointer_down(target, MouseButton::Left, cx);
            window.pointer_up(target, MouseButton::Left, cx);
            window.input("Z", cx);
        });
        settle(&mut context);
        view.read_with(&context, |view, _| {
            let mut expected = original.clone();
            expected.insert(at, 'Z');
            assert_eq!(view.text(), expected);
        });
        context.dispatch_action(gpui_kit::component::input::Undo);
        settle(&mut context);
    }
    save(&app, &mut context);
    assert_eq!(std::fs::read(path).unwrap(), original.as_bytes());
}

#[cfg(test)]
#[gpui_kit::test]
fn formula_workbench_commit_cancel_history_save_reopen(cx: &mut TestAppContext) {
    formula_commit_cancel_history_host_save(cx);
}
#[cfg(test)]
#[gpui_kit::test]
fn formula_workbench_unsupported_and_unchanged_preserve_source(cx: &mut TestAppContext) {
    unsupported_and_unchanged_are_lossless(cx);
}
#[cfg(feature = "native-markdown-tests")]
pub(super) fn native_cases() -> impl Iterator<Item = crate::native_markdown::Case> {
    [
        crate::native_markdown::Case::new("ui::code_editor::markdown_formula::formula_workbench_commit_cancel_history_save_reopen", formula_commit_cancel_history_host_save),
        crate::native_markdown::Case::new("ui::code_editor::markdown_formula::formula_workbench_unsupported_and_unchanged_preserve_source", unsupported_and_unchanged_are_lossless),
        crate::native_markdown::Case::new("ui::code_editor::markdown_formula::native_full_mixed_tall_formula_pointer_fragments", native_full_mixed_tall_formula_pointer_fragments),
        crate::native_markdown::Case::new("ui::code_editor::markdown_formula::native_hard_break_formula_fragments", native_hard_break_formula_fragments),
    ].into_iter()
}
