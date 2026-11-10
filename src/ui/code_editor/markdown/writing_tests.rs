//! 生产 workbench 写作消费者；由普通测试与 OS 主线程 native runner 共用。
use super::super::*;
use super::foundation::{save, settle, until};
use crate::ui::app::test_window::harness_with_tabs;
#[cfg(feature = "native-markdown-tests")]
use gpui_kit::test::TestEventExt as _;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{TestAppContext, VisualTestContext};

fn open(
    cx: &mut TestAppContext,
    source: &str,
) -> (
    tempfile::TempDir,
    PathBuf,
    Entity<LyrelApp>,
    VisualTestContext,
    Entity<lyrel_markdown_editor::EditorState>,
    Vec<crate::daemon::transport::Stream>,
) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("writing.md");
    std::fs::write(&path, source).unwrap();
    let (app, mut context, streams) = harness_with_tabs(cx, 1);
    context.simulate_resize(gpui_kit::size(px(1100.), px(800.)));
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
        view.focus_handle(cx).focus(window, cx)
    });
    settle(&mut context);
    (directory, path, app, context, view, streams)
}

pub(super) fn continuous_writing(cx: &mut TestAppContext) {
    let original =
        "# Heading\r\n\r\n- first\r\n\r\n> [!NOTE]+ Note\r\n> body\r\n\r\nkey:: intact\r\n";
    let (_directory, path, app, mut context, view, _streams) = open(cx, original);
    view.update_in(&mut context, |view, _, cx| {
        let at = view.text().find("first").unwrap();
        view.select_range(at..at + 5, cx);
    });
    context.dispatch_action(lyrel_markdown_editor::Bold);
    context.update(|window, cx| window.press("right", cx));
    context.update(|window, cx| window.press("end", cx));
    context.update(|window, cx| window.press("enter", cx));
    context.update(|window, cx| window.input("second", cx));
    context.update(|window, cx| window.press("tab", cx));
    context.update(|window, cx| window.press("shift-tab", cx));
    context.update(|window, cx| window.press("enter", cx));
    context.update(|window, cx| window.press("enter", cx));
    settle(&mut context);
    view.read_with(&context, |view, _| {
        assert!(view.text().contains("- **first**\n- second\n\n"));
        assert!(view.text().contains("key:: intact"));
    });
    view.update_in(&mut context, |view, _, cx| {
        let at = view.text().find("> body").unwrap() + "> body".len();
        view.select_range(at..at, cx);
    });
    context.update(|window, cx| {
        window.press("enter", cx);
        window.input("continued", cx);
        window.press("enter", cx);
        window.press("enter", cx);
    });
    settle(&mut context);
    assert!(view.read_with(&context, |view, _| {
        view.text().contains("> body\n> continued\n\n")
    }));
    save(&app, &mut context);
    assert!(
        std::fs::read(&path)
            .unwrap()
            .windows(12)
            .any(|s| s == b"key:: intact")
    );
    // A second presentation has the same writing host and shared source/history.
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
    second.update_in(&mut context, |view, window, cx| {
        view.select_range(view.text().len()..view.text().len(), cx);
        view.focus_handle(cx).focus(window, cx);
    });
    context.dispatch_action(lyrel_markdown_editor::InsertWritingBlock);
    settle(&mut context);
    // Standard menu keyboard path: first entry is a paragraph, second is H1.
    context.update(|window, cx| {
        window.press("down", cx);
        window.press("down", cx);
        window.press("enter", cx);
    });
    settle(&mut context);
    context.update(|window, cx| window.input("Shared heading", cx));
    settle(&mut context);
    view.read_with(&context, |view, cx| {
        assert_eq!(view.text(), second.read(cx).text())
    });
    assert!(view.read_with(&context, |view, _| view.text().contains("# Shared heading")));
    context.dispatch_action(gpui_kit::component::input::Undo);
    context.dispatch_action(gpui_kit::component::input::Undo);
    settle(&mut context);
    assert!(!view.read_with(&context, |view, _| view.text().contains("Shared heading")));
}

#[cfg(test)]
#[gpui_kit::test]
fn zorite_writing_workbench_lists_format_insert_shared_history(cx: &mut TestAppContext) {
    continuous_writing(cx);
}

#[cfg(test)]
#[gpui_kit::test]
fn insertion_cancel_restores_trigger_without_stealing_transferred_focus(cx: &mut TestAppContext) {
    let (_directory, _path, app, mut context, first, _streams) = open(cx, "body\n");
    context.dispatch_action(lyrel_markdown_editor::InsertWritingBlock);
    settle(&mut context);
    context.update(|window, cx| window.press("escape", cx));
    settle(&mut context);
    context.update(|window, cx| {
        assert!(first.read(cx).focus_handle(cx).is_focused(window));
        window.input("returned", cx);
    });
    assert!(first.read_with(&context, |view, _| view.text().contains("returned")));
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
    context.dispatch_action(lyrel_markdown_editor::InsertWritingBlock);
    settle(&mut context);
    context.update(|window, cx| {
        let menu_focus = window.focused(cx).unwrap();
        first.read(cx).focus_handle(cx).focus(window, cx);
        menu_focus.dispatch_action(&gpui_kit::base::actions::Cancel, window, cx);
    });
    settle(&mut context);
    context.update(|window, cx| {
        assert!(first.read(cx).focus_handle(cx).is_focused(window));
        assert!(!second.read(cx).focus_handle(cx).is_focused(window));
    });
}

#[cfg(feature = "native-markdown-tests")]
pub(super) fn native_colors(cx: &mut TestAppContext) {
    let original = "# Colors\r\n\r\ncolored text\r\n";
    let (_directory, path, app, mut context, first, _streams) = open(cx, original);
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
    second.update_in(&mut context, |view, window, cx| {
        let at = view.text().find("colored").unwrap();
        view.select_range(at..at + 7, cx);
        view.focus_handle(cx).focus(window, cx);
    });
    for cancel in [true, false] {
        context.dispatch_action(lyrel_markdown_editor::TextColorPicker);
        settle(&mut context);
        let field = context
            .debug_bounds("writing-color-hex")
            .expect("real custom color hex input");
        assert!(field.left() >= px(0.) && field.right() <= px(1100.));
        context.simulate_click(field.center(), gpui_kit::Modifiers::none());
        context.update(|window, cx| {
            window.press("secondary-a", cx);
            window.input("#12345678", cx);
        });
        settle(&mut context);
        first.read_with(&context, |view, cx| {
            assert_eq!(
                view.document().read(cx).encode().unwrap(),
                original.as_bytes(),
                "picker draft must not mutate shared source"
            )
        });
        if cancel {
            context.update(|window, cx| window.press("escape", cx));
            settle(&mut context);
            assert!(context.debug_bounds("writing-color-apply").is_none());
        } else {
            let apply = context.debug_bounds("writing-color-apply").unwrap();
            context.simulate_click(apply.center(), gpui_kit::Modifiers::none());
            settle(&mut context);
        }
    }
    let expected = "# Colors\r\n\r\n<span style=\"color:#12345678\">colored</span> text\r\n";
    first.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            expected.as_bytes()
        );
        assert_eq!(view.text(), second.read(cx).text());
    });
    save(&app, &mut context);
    assert_eq!(std::fs::read(&path).unwrap(), expected.as_bytes());
    context.dispatch_action(lyrel_markdown_editor::TextColorPicker);
    settle(&mut context);
    let clear = context.debug_bounds("writing-color-clear").unwrap();
    context.simulate_click(clear.center(), gpui_kit::Modifiers::none());
    settle(&mut context);
    let apply = context.debug_bounds("writing-color-apply").unwrap();
    context.simulate_click(apply.center(), gpui_kit::Modifiers::none());
    settle(&mut context);
    first.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            original.as_bytes()
        )
    });
    context.dispatch_action(gpui_kit::component::input::Undo);
    settle(&mut context);
    first.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            expected.as_bytes()
        )
    });
    context.dispatch_action(gpui_kit::component::input::Undo);
    settle(&mut context);
    first.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            original.as_bytes()
        )
    });
    save(&app, &mut context);
    assert_eq!(std::fs::read(&path).unwrap(), original.as_bytes());
}

#[cfg(feature = "native-markdown-tests")]
pub(super) fn native_rhythm(cx: &mut TestAppContext) {
    use gpui_kit::component::Theme;
    let prose = "ordinary readable words ".repeat(70);
    let source = format!("# Heading {prose}\n\n{prose}\n\n- {prose}\n\n> {prose}\n\nend");
    let (_directory, _path, app, mut context, view, _streams) = open(cx, &source);
    if !view.read_with(&context, |view, _| view.is_soft_wrap()) {
        context.dispatch_action(crate::core::actions::ToggleDocumentWrap);
    }
    for font_size in [16., 20., 14.] {
        context.update(|_, cx| Theme::update(cx, |theme| theme.font_size = px(font_size)));
        for width in [1100., 850.] {
            context.simulate_resize(gpui_kit::size(px(width), px(900.)));
            for (needle, prefix_len) in [
                ("# Heading", 2),
                ("\nordinary", 0),
                ("\n- ordinary", 2),
                ("\n> ordinary", 2),
            ] {
                let at = source.find(needle).unwrap() + usize::from(needle.starts_with('\n'));
                // Keep the structural prefix rendered, as it is during ordinary
                // body editing; revealing a list/quote marker changes its wrap.
                let body = at + prefix_len;
                view.update_in(&mut context, |view, _, cx| {
                    view.select_range(body..body, cx)
                });
                settle(&mut context);
                let (first, next) = view.read_with(&context, |view, _| {
                    let first = view.range_to_bounds(&(at..at)).unwrap();
                    let next = (at + 1
                        ..source[at..].find('\n').map_or(source.len(), |end| at + end))
                        .find_map(|offset| {
                            view.range_to_bounds(&(offset..offset))
                                .filter(|bounds| bounds.top() > first.top())
                                .map(|bounds| (offset, bounds))
                        })
                        .expect("complete block wraps in the native presentation");
                    (first, next)
                });
                assert!(next.1.top() > first.top());
                // Layout can already describe an offscreen row. Reveal the
                // target through the presentation's normal selection request
                // and deliver its scheduled platform frame before clicking.
                // Park one byte later so an ignored pointer cannot pass the
                // exact caret assertion, without revealing selection markers.
                view.update_in(&mut context, |view, _, cx| {
                    view.select_range(next.0 + 1..next.0 + 1, cx)
                });
                until(&mut context, |context| {
                    context.update(|window, cx| {
                        let viewport = window.find("markdown-document-scroll").bounds();
                        view.read(cx)
                            .range_to_bounds(&(next.0..next.0))
                            .is_some_and(|bounds| {
                                bounds.top() >= viewport.top()
                                    && bounds.bottom() <= viewport.bottom()
                            })
                    })
                });
                let target = view.read_with(&context, |view, _| {
                    let first = view.range_to_bounds(&(at..at)).unwrap();
                    let next = view.range_to_bounds(&(next.0..next.0)).unwrap();
                    assert!(next.top() > first.top(), "target stays on a wrapped row");
                    next.center()
                });
                context.update(|window, cx| {
                    window.pointer_move(target, None, cx);
                    window.pointer_down(target, gpui_kit::MouseButton::Left, cx);
                    window.pointer_up(target, gpui_kit::MouseButton::Left, cx);
                });
                settle(&mut context);
                let before = view.read_with(&context, |view, _| view.cursor());
                assert_eq!(
                    before, next.0,
                    "native wrap hit: font={font_size}, width={width}, block={needle:?}, first={first:?}, next={next:?}, target={target:?}"
                );
                view.read_with(&context, |view, _| {
                    assert_eq!(view.selected_range(), next.0..next.0);
                });
                context.update(|window, cx| window.input("X", cx));
                let mut expected = source.clone();
                expected.insert_str(next.0, "X");
                view.read_with(&context, |view, _| assert_eq!(view.text(), expected));
                context.dispatch_action(gpui_kit::component::input::Undo);
                settle(&mut context);
                view.read_with(&context, |view, _| assert_eq!(view.text(), source));
            }
        }
    }
    // Folding a heading is view-local, never a dirty/source mutation.
    view.update_in(&mut context, |view, _, cx| view.select_range(0..0, cx));
    context.dispatch_action(lyrel_markdown_editor::ToggleHeadingDisclosure);
    settle(&mut context);
    app.read_with(&context, |app, cx| {
        assert!(!app.active_buffer().unwrap().dirty);
        assert_eq!(view.read(cx).text(), source);
    });
    context.dispatch_action(lyrel_markdown_editor::ToggleHeadingDisclosure);
    settle(&mut context);
}

#[cfg(feature = "native-markdown-tests")]
pub(super) fn context_menu_follows_language_in_existing_views(cx: &mut TestAppContext) {
    use crate::core::config::Config;
    let (_directory, _path, app, mut context, first, _streams) = open(cx, "body text\n");
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
    for (locale, cut, copy, paste) in [
        ("zh-CN", "剪切", "复制", "粘贴"),
        ("en", "Cut", "Copy", "Paste"),
    ] {
        context.update(|_, cx| {
            crate::ui::i18n::set_locale(locale);
            let mut config = cx.global::<Config>().clone();
            config.gui_language = locale.into();
            cx.set_global(config);
        });
        settle(&mut context);
        for view in [&first, &second] {
            view.update_in(&mut context, |view, window, cx| {
                view.select_range(0..4, cx);
                view.focus_handle(cx).focus(window, cx);
            });
            settle(&mut context);
            let target = view.read_with(&context, |view, _| {
                view.range_to_bounds(&(1..1)).unwrap().center()
            });
            context.update(|window, cx| {
                window.pointer_move(target, None, cx);
                window.pointer_down(target, gpui_kit::MouseButton::Right, cx);
                window.pointer_up(target, gpui_kit::MouseButton::Right, cx);
            });
            settle(&mut context);
            context.update(|window, _| {
                assert_eq!(window.find("menu-cut").label(), Some(cut));
                assert_eq!(window.find("menu-copy").label(), Some(copy));
                assert_eq!(window.find("menu-paste").label(), Some(paste));
            });
            context.update(|window, cx| window.press("escape", cx));
            settle(&mut context);
        }
    }
    assert!(!app.read_with(&context, |app, _| app.active_buffer().unwrap().dirty));
}

#[cfg(feature = "native-markdown-tests")]
pub(super) fn native_cases() -> impl Iterator<Item = crate::native_markdown::Case> {
    [
        crate::native_markdown::Case::new("ui::code_editor::markdown::writing::continuous_writing", continuous_writing),
        crate::native_markdown::Case::new("ui::code_editor::markdown::writing::native_text_color_picker_clear_geometry_and_shared_history", native_colors),
        crate::native_markdown::Case::new("ui::code_editor::markdown::writing::native_wrapped_starts_prose_heading_list_quote_zoom", native_rhythm),
        crate::native_markdown::Case::new("ui::code_editor::markdown::writing::native_existing_views_bilingual_context_menu", context_menu_follows_language_in_existing_views),
    ].into_iter()
}
