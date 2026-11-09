use crate::ui::app::test_window::harness_with_tabs;
use gpui_kit::{
    ElementInputHandler, Focusable as _, TestAppContext,
    test::{TestInput, TestWindowExt as _},
};

#[gpui_kit::test]
fn formula_structural_fraction_cancel_commit_and_shared_document_undo(cx: &mut TestAppContext) {
    use crate::ui::code_editor::test_host::FileHost;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("structural-formula.md");
    let source = "前 $\\frac{a}{b}+\\mathrm{keep}$ 后\r\n\r\n$$\n  x_i^2\n$$\n";
    std::fs::write(&path, source).unwrap();
    let (app, mut context, _streams) = harness_with_tabs(cx, 1);
    let host = FileHost::new(lyrel_core::host::HostId(219));
    app.update_in(&mut context, |app, window, cx| {
        app.editor_open_on_host(host.clone(), &path, window, cx)
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
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
        if !view.is_live() {
            view.toggle_mode(window, cx);
        }
    });
    let ready = "markdown-math-ready-\\frac{a}{b}+\\mathrm{keep}";
    loop {
        context
            .executor()
            .advance_clock(std::time::Duration::from_millis(100));
        context.run_until_parked();
        context.update(|window, cx| window.render_frame(cx));
        if context.update(|window, _| window.try_find(ready).is_some()) {
            break;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let identity = view.read_with(&context, |view, _| view.document().entity_id());
    context.update(|window, cx| {
        window.click(ready, cx);
        assert!(window.find("markdown-formula-editor").visible());
        assert!(view.read(cx).focus_handle(cx).is_focused(window));
        let start = if cfg!(target_os = "macos") {
            "cmd-up"
        } else {
            "ctrl-home"
        };
        let end = if cfg!(target_os = "macos") {
            "cmd-down"
        } else {
            "ctrl-end"
        };
        let select_start = if cfg!(target_os = "macos") {
            "cmd-shift-up"
        } else {
            "ctrl-shift-home"
        };
        let select_end = if cfg!(target_os = "macos") {
            "cmd-shift-down"
        } else {
            "ctrl-shift-end"
        };
        window.press(start, cx);
        window.press(select_end, cx);
        window.press("secondary-c", cx);
        assert_eq!(
            cx.read_from_clipboard().unwrap().text().as_deref(),
            Some(r"\frac{a}{b}+\mathrm{keep}")
        );
        window.press(end, cx);
        window.press(select_start, cx);
        window.press("secondary-c", cx);
        assert_eq!(
            cx.read_from_clipboard().unwrap().text().as_deref(),
            Some(r"\frac{a}{b}+\mathrm{keep}")
        );
        window.press("secondary-a", cx);
        window.press("secondary-c", cx);
        assert_eq!(
            cx.read_from_clipboard().unwrap().text().as_deref(),
            Some(r"\frac{a}{b}+\mathrm{keep}")
        );
        window.press(start, cx);
        window.press("right", cx); // Numerator, not a source byte inside \\frac.
        // Both selection directions copy the numerator, never TeX command bytes.
        window.press("shift-right", cx);
        window.press("secondary-c", cx);
        assert_eq!(
            cx.read_from_clipboard().unwrap().text().as_deref(),
            Some("a")
        );
        window.press("right", cx);
        window.press("shift-left", cx);
        window.press("secondary-c", cx);
        assert_eq!(
            cx.read_from_clipboard().unwrap().text().as_deref(),
            Some("a")
        );
        window.press("left", cx);
        assert!(window.find("markdown-formula-editor").visible());
        let bounds = window.find("markdown-source").bounds();
        let mut input = TestInput::new(ElementInputHandler::new(bounds, view.clone()), window);
        input.compose_text(None, "ni", Some(2..2), cx);
        assert_eq!(input.marked_text_range(cx), Some(0..2));
        input.compose_text(None, "", None, cx);
        assert!(input.marked_text_range(cx).is_none());
        window.input("q", cx);
        assert_eq!(
            view.read(cx).document().read(cx).encode().unwrap(),
            source.as_bytes()
        );
        window.press("escape", cx);
        assert!(window.try_find("markdown-formula-editor").is_none());
        assert!(view.read(cx).focus_handle(cx).is_focused(window));
    });
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            source.as_bytes()
        )
    });
    context.run_until_parked();
    context.update(|window, cx| {
        window.click("markdown-math-ready-  x_i^2", cx);
        assert!(window.find("markdown-formula-editor").visible());
        assert_eq!(
            view.read(cx).selection(),
            super::Selection::caret(source.find("x_i^2").unwrap()),
            "block formula entry skips the opening line and leading whitespace"
        );
        window.press("escape", cx);
        assert_eq!(
            view.read(cx).document().read(cx).encode().unwrap(),
            source.as_bytes()
        );
    });
    context.update(|window, cx| {
        window.render_frame(cx);
        window.click(ready, cx);
        assert!(window.find("markdown-formula-editor").visible());
        window.press(
            if cfg!(target_os = "macos") {
                "cmd-up"
            } else {
                "ctrl-home"
            },
            cx,
        );
        window.press("right", cx);
        window.input("q", cx);
        window.press("enter", cx);
        assert!(window.try_find("markdown-formula-editor").is_none());
        assert!(view.read(cx).focus_handle(cx).is_focused(window));
    });
    let expected = source.replacen("{a}", "{qa}", 1);
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            expected.as_bytes()
        )
    });
    context.dispatch_action(gpui_kit::component::input::Undo);
    context.run_until_parked();
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            source.as_bytes()
        )
    });
    context.dispatch_action(gpui_kit::component::input::Redo);
    context.run_until_parked();
    view.read_with(&context, |view, cx| {
        assert_eq!(view.document().entity_id(), identity);
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            expected.as_bytes()
        )
    });
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
    assert_eq!(std::fs::read(&path).unwrap(), expected.as_bytes());
    assert!(host.writes.load(std::sync::atomic::Ordering::SeqCst) > 0);
    app.update_in(&mut context, |app, window, cx| {
        app.editor_close_file(0, window, cx);
        app.editor_open_on_host(host.clone(), &path, window, cx);
    });
    loop {
        context.run_until_parked();
        if app.read_with(&context, |app, cx| {
            app.active_editor_input().is_some_and(|input| {
                input
                    .markdown()
                    .is_some_and(|view| view.read(cx).document().entity_id() != identity)
            })
        }) {
            break;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    app.read_with(&context, |app, cx| {
        assert_eq!(
            app.active_buffer().unwrap().host.id(),
            lyrel_core::host::HostId(219)
        );
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
            expected.as_bytes()
        );
    });
}

#[test]
fn formula_lossless_wrappers_scripts_and_matrix_keep_untouched_spelling() {
    use super::lossless::Seed;
    use ratex_gpui::editor::{
        cursor::{Cursor, Slot, Step},
        model::Atom,
    };
    let raw = r"\frac { a }{b}+\mathrm{keep}+x^2_i+\begin{bmatrix} p & q \\ r & s \end{bmatrix}";
    let seed = Seed::parse(raw);
    assert_eq!(seed.serialize(&seed.row), raw);
    let empty = Seed::parse(r"\frac{}{b}");
    assert_eq!(empty.serialize(&empty.row), r"\frac{}{b}");
    assert_eq!(empty.preview(&empty.row), r"\frac{\square}{b}");
    let mut row = seed.row.clone();
    let mut cursor = Cursor {
        path: vec![Step {
            atom: 0,
            slot: Slot::Num,
        }],
        index: 0,
    };
    cursor.insert(&mut row, Atom::Sym("z".into()));
    assert_eq!(seed.serialize(&row), raw.replacen(" a ", " za ", 1));
    let mut cursor = Cursor {
        path: vec![Step {
            atom: row.atoms.len() - 1,
            slot: Slot::Cell(0, 0),
        }],
        index: 0,
    };
    cursor.matrix_add_row(&mut row);
    let changed = seed.serialize(&row);
    assert!(changed.contains(r"\begin{bmatrix}"));
    assert!(changed.ends_with(r"\end{bmatrix}"));
    assert!(changed.contains(r"\mathrm{keep}+x^2_i"));
    let wrapped_seed = Seed::parse(r"\begin{bmatrix}a&b\end{bmatrix}");
    let mut wrapped = wrapped_seed.row.clone();
    let mut cursor = Cursor::start();
    cursor.wrap_sqrt(&mut wrapped, 0, 1);
    assert_eq!(
        wrapped_seed.serialize(&wrapped),
        r"\sqrt{\begin{bmatrix}a&b\end{bmatrix}}"
    );
}

#[gpui_kit::test]
fn formula_completion_alignment_context_history_and_host_reopen(cx: &mut TestAppContext) {
    use crate::ui::code_editor::test_host::FileHost;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("formula-tools.md");
    let original = "\u{feff}before\r\n\r\n<!--  math:right   -->\n$$\n\\frac{a}{b}\n$$\n\nafter  ";
    std::fs::write(&path, original).unwrap();
    let host = FileHost::new(lyrel_core::host::HostId(218));
    let (app, mut context, _streams) = harness_with_tabs(cx, 1);
    app.update_in(&mut context, |app, window, cx| {
        app.editor_open_on_host(host.clone(), &path, window, cx)
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
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
        if !view.is_live() {
            view.toggle_mode(window, cx);
        }
    });
    loop {
        context
            .executor()
            .advance_clock(std::time::Duration::from_millis(100));
        context.run_until_parked();
        context.update(|window, cx| window.render_frame(cx));
        if context.update(|window, _| {
            window
                .try_find("markdown-math-ready-\\frac{a}{b}")
                .is_some()
        }) {
            break;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    context.update(|window, cx| {
        window.click("markdown-math-ready-\\frac{a}{b}", cx);
        assert!(window.find("markdown-formula-editor").visible());
        assert!(view.read(cx).focus_handle(cx).is_focused(window));
        window.press(
            if cfg!(target_os = "macos") {
                "cmd-up"
            } else {
                "ctrl-home"
            },
            cx,
        );
        window.press("right", cx);
        assert!(window.find("markdown-formula-editor").visible());
        window.input("\\zzzzz", cx);
        window.press("escape", cx);
        window.input("\\", cx);
        window.press("escape", cx);
        window.input("\\alph", cx);
        let bounds = window.find("markdown-source").bounds();
        {
            let mut input = TestInput::new(ElementInputHandler::new(bounds, view.clone()), window);
            input.compose_text(None, "ni", Some(2..2), cx);
        }
        window.press("down", cx);
        window.press("enter", cx);
        window.press(
            if cfg!(target_os = "macos") {
                "cmd-up"
            } else {
                "ctrl-home"
            },
            cx,
        );
        let mut input = TestInput::new(ElementInputHandler::new(bounds, view.clone()), window);
        assert_eq!(input.marked_text_range(cx), Some(0..2));
    });
    context.dispatch_action(super::ExportFormulaSvg);
    assert!(
        !cx.did_prompt_for_new_path(),
        "formula commands cannot accept an active preedit"
    );
    context.update(|window, cx| {
        let bounds = window.find("markdown-source").bounds();
        let mut input = TestInput::new(ElementInputHandler::new(bounds, view.clone()), window);
        input.compose_text(None, "", None, cx);
        window.press("down", cx);
        window.press("up", cx);
        window.press("enter", cx);
        assert!(window.find("markdown-formula-editor").visible());
        window.press("shift-f10", cx);
    });
    context.run_until_parked();
    context.update(|window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("popup-menu").focused(), Some(true));
        window.press("down", cx);
        window.press("enter", cx); // Copy LaTeX from the production context menu.
    });
    context.run_until_parked();
    context.update(|window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("popup-menu").is_none());
        assert!(view.read(cx).focus_handle(cx).is_focused(window));
        assert_eq!(
            cx.read_from_clipboard().unwrap().text().unwrap(),
            "\n\\frac{\\alpha a}{b}\n"
        )
    });
    fn settle_export(context: &mut gpui_kit::VisualTestContext) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
        loop {
            context.run_until_parked();
            context.update(|window, cx| window.render_frame(cx));
            if context.update(|window, _| window.try_find("markdown-formula-exporting").is_none()) {
                break;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }
    context.dispatch_action(super::ExportFormulaPng);
    assert!(cx.did_prompt_for_new_path());
    cx.simulate_new_path_selection(|_| None);
    settle_export(&mut context);
    assert!(
        !context.update(|window, _| window.try_find("markdown-formula-export-error").is_some())
    );
    let png = directory.path().join("formula.png");
    context.dispatch_action(super::ExportFormulaPng);
    cx.simulate_new_path_selection(|_| Some(png.clone()));
    settle_export(&mut context);
    let decoded = image::load_from_memory(&std::fs::read(&png).unwrap()).unwrap();
    assert!(decoded.width() > 0 && decoded.height() > 0);
    let svg = directory.path().join("formula.svg");
    context.dispatch_action(super::ExportFormulaSvg);
    cx.simulate_new_path_selection(|_| Some(svg.clone()));
    settle_export(&mut context);
    let vector = std::fs::read_to_string(&svg).unwrap();
    assert!(vector.contains("<svg") && vector.contains("<path"));
    assert!(
        !vector.contains("<text")
            && !vector.contains("<image")
            && !vector.contains("<foreignObject")
    );
    let missing = directory.path().join("missing").join("formula.svg");
    context.dispatch_action(super::ExportFormulaSvg);
    cx.simulate_new_path_selection(|_| Some(missing.clone()));
    settle_export(&mut context);
    assert!(context.update(|window, _| window.try_find("markdown-formula-export-error").is_some()));
    assert!(!missing.exists());
    let sentinel = directory.path().join("existing.svg");
    std::fs::write(&sentinel, "keep existing bytes").unwrap();
    context.dispatch_action(super::ExportFormulaSvg);
    cx.simulate_new_path_selection(|_| Some(sentinel.clone()));
    loop {
        context.run_until_parked();
        if context.has_pending_prompt() {
            break;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    context.simulate_prompt_answer(crate::ui::i18n::t(crate::ui::i18n::L10nKey::Cancel));
    settle_export(&mut context);
    assert_eq!(std::fs::read(&sentinel).unwrap(), b"keep existing bytes");
    assert_eq!(host.writes.load(std::sync::atomic::Ordering::SeqCst), 0);
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            original.as_bytes()
        )
    });
    context.update(|window, cx| {
        window.render_frame(cx);
        window.press("shift-f10", cx);
    });
    context.run_until_parked();
    context.update(|window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("popup-menu").focused(), Some(true));
        for _ in 0..4 {
            window.press("down", cx);
        }
        window.press("enter", cx); // Align left via the same production menu.
    });
    context.run_until_parked();
    context.update(|window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("popup-menu").is_none());
        assert!(view.read(cx).focus_handle(cx).is_focused(window));
    });
    context.dispatch_action(super::CommitFormula);
    let expected = original
        .replace("<!--  math:right   -->", "<!-- math:left -->")
        .replace("{a}", "{\\alpha a}");
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            expected.as_bytes()
        )
    });
    loop {
        context
            .executor()
            .advance_clock(std::time::Duration::from_millis(100));
        context.run_until_parked();
        context.update(|window, cx| window.render_frame(cx));
        if context.update(|window, _| {
            window
                .try_find("markdown-math-ready-\\frac{\\alpha a}{b}")
                .is_some()
        }) {
            break;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    context.update(|window, cx| window.click("markdown-math-ready-\\frac{\\alpha a}{b}", cx));
    context.dispatch_action(super::AlignFormulaCenter);
    context.dispatch_action(super::CommitFormula);
    let centered = expected.replace("<!-- math:left -->\n", "");
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            centered.as_bytes()
        )
    });
    context.dispatch_action(gpui_kit::component::input::Undo);
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            expected.as_bytes()
        )
    });
    context.dispatch_action(gpui_kit::component::input::Undo);
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            original.as_bytes()
        )
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
    assert_eq!(std::fs::read(&path).unwrap(), expected.as_bytes());
    assert!(host.writes.load(std::sync::atomic::Ordering::SeqCst) > 0);
    let identity = view.read_with(&context, |view, _| view.document().entity_id());
    app.update_in(&mut context, |app, window, cx| {
        app.editor_close_file(0, window, cx);
        app.editor_open_on_host(host.clone(), &path, window, cx);
    });
    loop {
        context.run_until_parked();
        if app.read_with(&context, |app, cx| {
            app.active_editor_input().is_some_and(|input| {
                input
                    .markdown()
                    .is_some_and(|view| view.read(cx).document().entity_id() != identity)
            })
        }) {
            break;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    app.read_with(&context, |app, cx| {
        assert_eq!(
            app.active_buffer().unwrap().host.id(),
            lyrel_core::host::HostId(218)
        );
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
            expected.as_bytes()
        );
    });
}
