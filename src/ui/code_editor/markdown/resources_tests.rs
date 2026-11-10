//! 工作台资源消费者；native runner 在 OS 主线程复用相同生产入口。
use super::super::*;
use crate::ui::app::test_window::harness_with_tabs;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{ClipboardEntry, ClipboardItem, TestAppContext, VisualTestContext};
use std::time::{Duration, Instant};

fn settle(cx: &mut VisualTestContext) {
    cx.run_until_parked();
    cx.update(|window, cx| {
        window.simulate_next_frame(cx);
        window.render_frame(cx);
    });
    cx.run_until_parked();
}
fn until(cx: &mut VisualTestContext, mut ready: impl FnMut(&mut VisualTestContext) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        settle(cx);
        if ready(cx) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "resource workflow did not settle"
        );
        cx.executor().advance_clock(Duration::from_millis(50));
        std::thread::sleep(Duration::from_millis(5));
    }
}
fn png() -> Vec<u8> {
    let mut out = std::io::Cursor::new(Vec::new());
    image::RgbaImage::from_pixel(120, 240, image::Rgba([50, 110, 190, 255]))
        .write_to(&mut out, image::ImageFormat::Png)
        .unwrap();
    out.into_inner()
}

pub(super) fn owning_host(cx: &mut TestAppContext) {
    use super::super::test_host::{FileHost, canonical_tempdir};
    let remote = canonical_tempdir();
    let local = canonical_tempdir();
    let source = b"\xef\xbb\xbf# Images\r\n\r\nunchanged$$x$$literal\n";
    std::fs::write(remote.path().join("images.md"), source).unwrap();
    std::fs::write(local.path().join("images.md"), b"LOCAL DECOY").unwrap();
    let host = FileHost::mapped(HostId(238), remote.path());
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    app.update_in(&mut vcx, |app, window, cx| {
        app.editor_open_on_host(host.clone(), Path::new("/images.md"), window, cx)
    });
    until(&mut vcx, |cx| {
        app.read_with(cx, |app, _| app.active_buffer().is_some())
    });
    app.update_in(&mut vcx, |app, window, cx| {
        assert!(app.editor_split(window, cx));
    });
    settle(&mut vcx);
    let second = app.read_with(&vcx, |app, _| {
        app.active_editor_input()
            .unwrap()
            .markdown()
            .unwrap()
            .clone()
    });
    second.update_in(&mut vcx, |view, window, cx| {
        let end = view.text().len();
        view.select_range(end..end, cx);
        view.focus_handle(cx).focus(window, cx);
        cx.write_to_clipboard(ClipboardItem {
            entries: vec![ClipboardEntry::Image(gpui_kit::Image {
                bytes: png(),
                format: gpui_kit::ImageFormat::Png,
                id: 238,
            })],
        });
    });
    vcx.dispatch_action(gpui_kit::component::input::Paste);
    until(&mut vcx, |cx| {
        second.read_with(cx, |view, _| view.text().contains("assets/image-"))
    });
    let (inserted, src) = second.read_with(&vcx, |view, cx| {
        (
            view.text().to_string(),
            view.document().read(cx).snapshot().image_sources()[0].clone(),
        )
    });
    assert_eq!(std::fs::read(remote.path().join(&src)).unwrap(), png());
    assert!(!local.path().join("assets").exists());
    assert_eq!(
        std::fs::read(local.path().join("images.md")).unwrap(),
        b"LOCAL DECOY"
    );
    until(&mut vcx, |cx| {
        app.read_with(cx, |app, cx| {
            app.active_buffer()
                .unwrap()
                .markdown_resources
                .as_ref()
                .unwrap()
                .read(cx)
                .is_loaded(&src)
        })
    });
    vcx.dispatch_action(gpui_kit::component::input::Undo);
    settle(&mut vcx);
    second.read_with(&vcx, |view, cx| {
        assert_eq!(view.document().read(cx).encode().unwrap(), source)
    });
    vcx.dispatch_action(gpui_kit::component::input::Redo);
    vcx.dispatch_action(crate::core::actions::EditorSave);
    until(&mut vcx, |cx| {
        app.read_with(cx, |app, _| {
            !app.active_buffer().unwrap().dirty && !app.active_buffer().unwrap().saving
        })
    });
    let saved = std::fs::read(remote.path().join("images.md")).unwrap();
    assert!(saved.starts_with(b"\xef\xbb\xbf# Images\r\n\r\nunchanged$$x$$literal\n"));
    app.update_in(&mut vcx, |app, window, cx| {
        app.editor_close_file(0, window, cx);
        app.editor_close_file(0, window, cx);
        app.editor_open_on_host(host.clone(), Path::new("/images.md"), window, cx);
    });
    until(&mut vcx, |cx| {
        app.read_with(cx, |app, cx| {
            app.active_buffer()
                .is_some_and(|file| file.input.read(cx).text().to_string() == inserted)
        })
    });
    app.read_with(&vcx, |app, cx| {
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
            saved
        )
    });
}

#[cfg(test)]
#[gpui_kit::test]
fn image_paste_uses_owning_host_shared_history_and_save_reopen(cx: &mut TestAppContext) {
    owning_host(cx);
}

#[cfg(test)]
#[gpui_kit::test]
fn shared_images_follow_latest_document_not_a_stale_presentation(cx: &mut TestAppContext) {
    use super::super::test_host::{FileHost, canonical_tempdir};
    let directory = canonical_tempdir();
    std::fs::write(directory.path().join("images.md"), "![old](old.png)\r\n").unwrap();
    for name in [
        "old.png",
        "new.png",
        "inline.png",
        "second.png",
        "other.png",
    ] {
        std::fs::write(directory.path().join(name), png()).unwrap();
    }
    let host = FileHost::mapped(HostId(2382), directory.path());
    let (app, mut context, _streams) = harness_with_tabs(cx, 1);
    app.update_in(&mut context, |app, window, cx| {
        app.editor_open_on_host(host, Path::new("/images.md"), window, cx)
    });
    until(&mut context, |cx| {
        app.read_with(cx, |app, _| app.active_buffer().is_some())
    });
    let first = app.read_with(&context, |app, _| {
        app.active_editor_input()
            .unwrap()
            .markdown()
            .unwrap()
            .clone()
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
    for (writer, final_writer, block, inline) in [
        (&first, &second, "new.png", "inline.png"),
        (&second, &first, "second.png", "other.png"),
    ] {
        let latest = format!(
            "![new]({block})\n\ntext ![inline]({inline}) tail\n\n```md\n![code](missing.png)\n```\n`![code](also-missing.png)`\n"
        );
        // Both notification orders matter: HashMap iteration must never pick
        // the intermediate writer as a new revision's resource authority.
        context.update(|_, cx| {
            writer.update(cx, |view, cx| {
                view.replace_range(0..view.text().len(), "intermediate\n", cx)
            });
            final_writer.update(cx, |view, cx| {
                view.replace_range(0.."intermediate\n".len(), &latest, cx)
            });
        });
        until(&mut context, |cx| {
            app.read_with(cx, |app, cx| {
                let resources = app
                    .active_buffer()
                    .unwrap()
                    .markdown_resources
                    .as_ref()
                    .unwrap()
                    .read(cx);
                resources.is_loaded(block) && resources.is_loaded(inline)
            })
        });
        app.read_with(&context, |app, cx| {
            let resources = app
                .active_buffer()
                .unwrap()
                .markdown_resources
                .as_ref()
                .unwrap()
                .read(cx);
            assert!(!resources.is_loaded("old.png"));
            assert!(!resources.is_loaded("missing.png"));
            assert!(!resources.is_loaded("also-missing.png"));
            assert_eq!(first.read(cx).text(), latest);
            assert_eq!(second.read(cx).text(), latest);
        });
    }
}

#[cfg(feature = "native-markdown-tests")]
pub(super) fn renamed_mdx_links_in_both_views(cx: &mut TestAppContext) {
    use super::super::test_host::{FileHost, canonical_tempdir};
    use gpui_kit::test::TestEventExt as _;
    let directory = canonical_tempdir();
    let source = "# Links\r\n\r\n[target](target.md)\r\n";
    std::fs::write(directory.path().join("links.mdx"), source).unwrap();
    std::fs::write(directory.path().join("target.md"), b"# Owning target\r\n").unwrap();
    let host = FileHost::mapped(HostId(2383), directory.path());
    let (app, mut context, _streams) = harness_with_tabs(cx, 1);
    context.simulate_resize(gpui_kit::size(px(1600.), px(900.)));
    app.update_in(&mut context, |app, window, cx| {
        app.editor_open_on_host(host, Path::new("/links.mdx"), window, cx);
    });
    until(&mut context, |cx| {
        app.read_with(cx, |app, _| app.active_buffer().is_some())
    });
    let first = app.read_with(&context, |app, _| {
        app.active_editor_input()
            .unwrap()
            .markdown()
            .unwrap()
            .clone()
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
    for view in [&first, &second] {
        assert!(view.read_with(&context, |view, _| view.is_source_only()));
    }
    std::fs::rename(
        directory.path().join("links.mdx"),
        directory.path().join("links.md"),
    )
    .unwrap();
    app.update_in(&mut context, |app, window, cx| {
        app.editor_path_moved(
            HostId(2383),
            Path::new("/links.mdx"),
            Path::new("/links.md"),
            cx,
        );
        app.editor_focus_group(true, window, cx);
        app.toggle_document_preview(window, cx);
        app.editor_focus_group(false, window, cx);
        app.toggle_document_preview(window, cx);
    });
    settle(&mut context);
    for view in [&first, &second] {
        view.update_in(&mut context, |view, window, cx| {
            assert!(!view.is_source_only());
            assert!(view.is_live());
            let end = view.text().len();
            view.select_range(end..end, cx);
            view.focus_handle(cx).focus(window, cx);
        });
        settle(&mut context);
        let target = view.read_with(&context, |view, _| {
            let at = view.text().find("[target]").unwrap() + 2;
            view.range_to_bounds(&(at..at)).unwrap().center()
        });
        context.update(|window, cx| window.pointer_move(target, None, cx));
        settle(&mut context);
        context.update(|window, _| {
            assert_eq!(
                window.find("markdown-link-hover").label(),
                Some("target.md")
            )
        });
        context.simulate_click(target, gpui_kit::Modifiers::none());
        until(&mut context, |cx| {
            app.read_with(cx, |app, _| {
                app.active_buffer().is_some_and(|file| {
                    file.path == Path::new("/target.md") && file.host.id() == HostId(2383)
                })
            })
        });
    }
    assert_eq!(
        std::fs::read(directory.path().join("links.md")).unwrap(),
        source.as_bytes()
    );
}

#[cfg(feature = "native-markdown-tests")]
fn wrapped_link(cx: &mut TestAppContext, tail: bool, drag: bool) {
    use super::super::test_host::{FileHost, canonical_tempdir};
    use gpui_kit::test::TestEventExt as _;
    use gpui_kit::{MouseButton, point};
    let remote = canonical_tempdir();
    let host = FileHost::mapped(HostId(2381), remote.path());
    let prefix = "prefix ".repeat(5);
    let label = "wrapped words ".repeat(160);
    let source = format!("# Links\n\n{prefix}[{label}](linked.md) tail\n");
    std::fs::write(remote.path().join("links.md"), &source).unwrap();
    std::fs::write(
        remote.path().join("linked.md"),
        b"# Owning remote target\r\n",
    )
    .unwrap();
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    vcx.simulate_resize(gpui_kit::size(px(900.), px(1000.)));
    app.update_in(&mut vcx, |app, window, cx| {
        app.editor_open_on_host(host.clone(), Path::new("/links.md"), window, cx)
    });
    until(&mut vcx, |cx| {
        app.read_with(cx, |app, _| app.active_buffer().is_some())
    });
    let view = app.read_with(&vcx, |app, _| {
        app.active_editor_input()
            .unwrap()
            .markdown()
            .unwrap()
            .clone()
    });
    view.update_in(&mut vcx, |view, window, cx| {
        view.select_range(0..0, cx);
        view.set_soft_wrap(true, cx);
        view.focus_handle(cx).focus(window, cx);
    });
    settle(&mut vcx);
    let label_start = source.find('[').unwrap() + 1;
    let first_y = view.read_with(&vcx, |view, _| {
        view.range_to_bounds(&(label_start..label_start))
            .unwrap()
            .top()
    });
    let wrap = view.read_with(&vcx, |view, _| {
        (label_start + 1..label_start + label.len())
            .find(|at| {
                view.range_to_bounds(&(*at..*at))
                    .is_some_and(|bounds| bounds.top() > first_y)
            })
            .unwrap()
    });
    // A fragment's tail glyph and the first downstream glyph exercise opposite
    // affinity sides; neither may borrow the adjacent visual row's hitbox.
    let at = if tail { wrap - 2 } else { wrap - 1 };
    let target = view.read_with(&vcx, |view, _| {
        let bounds = view
            .range_to_bounds(&(if tail { at..at } else { wrap..wrap }))
            .unwrap();
        let paragraph = source.find(&prefix).unwrap();
        let left = view
            .range_to_bounds(&(paragraph..paragraph))
            .unwrap()
            .left();
        point(
            if tail {
                bounds.left() + px(0.25)
            } else {
                (left + bounds.left()) / 2.
            },
            bounds.center().y,
        )
    });
    vcx.update(|window, cx| window.pointer_move(target, None, cx));
    settle(&mut vcx);
    vcx.update(|window, _| {
        assert_eq!(
            window
                .try_find("markdown-link-hover")
                .unwrap_or_else(|| panic!("native link hover: tail={tail}, drag={drag}, wrap={wrap}, at={at}, target={target:?}"))
                .label(),
            Some("linked.md")
        );
    });
    // A modified click edits the visible link glyph; ordinary release below still opens it.
    if !drag {
        vcx.update(|window, cx| {
            window.change_modifiers(
                gpui_kit::Modifiers {
                    control: true,
                    ..Default::default()
                },
                cx,
            );
            window.pointer_down(target, MouseButton::Left, cx);
            window.pointer_up(target, MouseButton::Left, cx);
            window.change_modifiers(gpui_kit::Modifiers::none(), cx);
            window.input("Q", cx);
        });
        settle(&mut vcx);
        view.read_with(&vcx, |view, _| assert!(view.text().contains('Q')));
        vcx.dispatch_action(gpui_kit::component::input::Undo);
        settle(&mut vcx);
        view.update_in(&mut vcx, |view, _, cx| view.select_range(0..0, cx));
        settle(&mut vcx);
        view.read_with(&vcx, |view, _| assert_eq!(view.text(), source));
    }
    vcx.update(|window, cx| window.pointer_down(target, MouseButton::Left, cx));
    settle(&mut vcx);
    if drag {
        let anchor = view.read_with(&vcx, |view, _| view.cursor());
        assert_eq!(
            anchor, at,
            "native glyph hit retains its exact source anchor"
        );
        let end = view.read_with(&vcx, |view, _| {
            let bounds = view.range_to_bounds(&(at + 30..at + 30)).unwrap();
            point(bounds.left() + px(0.25), bounds.center().y)
        });
        vcx.update(|window, cx| {
            window.pointer_move(end, Some(MouseButton::Left), cx);
            window.pointer_up(end, MouseButton::Left, cx);
        });
        settle(&mut vcx);
        view.read_with(&vcx, |view, _| {
            assert_eq!(view.selected_range().start, anchor);
            assert!(view.selected_range().end > anchor);
        });
        app.read_with(&vcx, |app, _| {
            assert_eq!(app.active_buffer().unwrap().path, Path::new("/links.md"))
        });
        let selected = view.read_with(&vcx, |view, _| view.selected_range());
        vcx.update(|window, cx| window.input("Q", cx));
        settle(&mut vcx);
        let mut expected = source.clone();
        expected.replace_range(selected, "Q");
        view.read_with(&vcx, |view, _| assert_eq!(view.text(), expected));
        vcx.dispatch_action(gpui_kit::component::input::Undo);
        settle(&mut vcx);
        view.read_with(&vcx, |view, _| assert_eq!(view.text(), source));
    } else {
        vcx.update(|window, cx| window.pointer_up(target, MouseButton::Left, cx));
        until(&mut vcx, |cx| {
            app.read_with(cx, |app, _| {
                app.active_buffer()
                    .is_some_and(|file| file.path == Path::new("/linked.md"))
            })
        });
        app.read_with(&vcx, |app, cx| {
            let file = app.active_buffer().unwrap();
            assert_eq!(file.host.id(), HostId(2381));
            assert_eq!(
                file.input.read(cx).text().to_string(),
                "# Owning remote target\n"
            );
        });
        assert_eq!(
            std::fs::read(remote.path().join("links.md")).unwrap(),
            source.as_bytes()
        );
    }
}

#[cfg(feature = "native-markdown-tests")]
pub(super) fn first_wrapped_link(cx: &mut TestAppContext) {
    wrapped_link(cx, false, false);
}
#[cfg(feature = "native-markdown-tests")]
pub(super) fn fragment_tail_link(cx: &mut TestAppContext) {
    wrapped_link(cx, true, false);
}
#[cfg(feature = "native-markdown-tests")]
pub(super) fn wrapped_link_drag(cx: &mut TestAppContext) {
    wrapped_link(cx, true, true);
}

#[cfg(feature = "native-markdown-tests")]
pub(super) fn image_preview_resize(cx: &mut TestAppContext) {
    use gpui_kit::component::ActiveTheme as _;
    use gpui_kit::test::TestEventExt as _;
    use gpui_kit::{MouseButton, point};
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("preview.md");
    let source = "# Image\r\n\r\n![tall](tall.png){width=120}\r\n\r\nuntouched$$x$$spelling\n";
    std::fs::write(&path, source).unwrap();
    std::fs::write(directory.path().join("tall.png"), png()).unwrap();
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    vcx.simulate_resize(gpui_kit::size(px(1300.), px(1000.)));
    app.update_in(&mut vcx, |app, window, cx| {
        app.open_file_in_editor(&path, window, cx)
    });
    until(&mut vcx, |cx| {
        app.read_with(cx, |app, _| app.active_buffer().is_some())
    });
    let view = app.read_with(&vcx, |app, _| {
        app.active_editor_input()
            .unwrap()
            .markdown()
            .unwrap()
            .clone()
    });
    until(&mut vcx, |cx| {
        view.read_with(cx, |view, _| view.image_bounds("tall.png").is_some())
    });
    let base_font = vcx.update(|_, cx| cx.theme().font_size);
    for font_size in [base_font * 1.25, base_font] {
        vcx.update(|_, cx| {
            gpui_kit::component::Theme::update(cx, |theme| theme.font_size = font_size)
        });
        settle(&mut vcx);
        view.read_with(&vcx, |view, cx| {
            let bounds = view
                .image_bounds("tall.png")
                .expect("bitmap stays live after application zoom");
            assert!(
                (f32::from(bounds.size.height) / f32::from(bounds.size.width) - 2.).abs() < 0.01
            );
            assert_eq!(
                view.document().read(cx).encode().unwrap(),
                source.as_bytes()
            );
        });
    }
    let image = view.read_with(&vcx, |view, _| view.image_bounds("tall.png").unwrap());
    vcx.update(|window, cx| {
        window.pointer_move(image.center(), None, cx);
        window.pointer_down(image.center(), MouseButton::Left, cx);
        window.pointer_up(image.center(), MouseButton::Left, cx);
    });
    settle(&mut vcx);
    let initial =
        vcx.update(|window, _| window.find("markdown-image-preview-content").bounds().size);
    vcx.update(|window, cx| window.click("image-zoom-in", cx));
    settle(&mut vcx);
    vcx.update(|window, _| {
        assert!(
            window
                .find("markdown-image-preview-content")
                .bounds()
                .size
                .width
                > initial.width
        )
    });
    vcx.update(|window, cx| window.press("escape", cx));
    settle(&mut vcx);
    let grip = point(image.right() - px(4.), image.bottom() - px(4.));
    let resized = point(grip.x + px(50.), grip.y);
    vcx.update(|window, cx| {
        window.pointer_move(grip, None, cx);
        window.pointer_down(grip, MouseButton::Left, cx);
        window.pointer_move(resized, Some(MouseButton::Left), cx);
        window.pointer_up(resized, MouseButton::Left, cx);
    });
    settle(&mut vcx);
    view.read_with(&vcx, |view, _| assert!(view.text().contains("{width=170}")));
    vcx.dispatch_action(gpui_kit::component::input::Undo);
    settle(&mut vcx);
    view.read_with(&vcx, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            source.as_bytes()
        )
    });
    assert_eq!(
        std::fs::read(directory.path().join("tall.png")).unwrap(),
        png()
    );
    let dropped = directory.path().join("dropped.png");
    std::fs::write(&dropped, png()).unwrap();
    let at = view.read_with(&vcx, |view, _| view.text().find("untouched").unwrap());
    view.update_in(&mut vcx, |view, _, cx| view.select_range(at..at, cx));
    settle(&mut vcx);
    let position = view.read_with(&vcx, |view, _| {
        view.range_to_bounds(&(at..at)).unwrap().center()
    });
    vcx.simulate_event(gpui_kit::FileDropEvent::Entered {
        position,
        paths: gpui_kit::ExternalPaths(vec![dropped].into()),
    });
    vcx.simulate_event(gpui_kit::FileDropEvent::Submit { position });
    until(&mut vcx, |cx| {
        view.read_with(cx, |view, cx| {
            view.document().read(cx).snapshot().image_sources().len() == 2
        })
    });
    let attachment = view.read_with(&vcx, |view, cx| {
        view.document()
            .read(cx)
            .snapshot()
            .image_sources()
            .into_iter()
            .find(|source| source.starts_with("assets/"))
            .unwrap()
    });
    assert_eq!(
        std::fs::read(directory.path().join(attachment)).unwrap(),
        png()
    );
    vcx.dispatch_action(gpui_kit::component::input::Undo);
    settle(&mut vcx);
    view.read_with(&vcx, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            source.as_bytes()
        )
    });
}

#[cfg(feature = "native-markdown-tests")]
pub(super) fn native_cases() -> impl Iterator<Item = crate::native_markdown::Case> {
    [
        crate::native_markdown::Case::new("ui::code_editor::markdown::resources::owning_host", owning_host),
        crate::native_markdown::Case::new("ui::code_editor::markdown::resources::first_soft_wrapped_link_grapheme_hovers_edits_and_opens", first_wrapped_link),
        crate::native_markdown::Case::new("ui::code_editor::markdown::resources::fragment_tail_link_grapheme_hovers_edits_and_opens", fragment_tail_link),
        crate::native_markdown::Case::new("ui::code_editor::markdown::resources::native_wrapped_link_drag_retains_upper_wrap_anchor", wrapped_link_drag),
        crate::native_markdown::Case::new("ui::code_editor::markdown::resources::native_image_preview_zoom_resize_history", image_preview_resize),
        crate::native_markdown::Case::new("ui::code_editor::markdown::resources::native_complete_resources_live_edit_save_undo", complete_resources),
        crate::native_markdown::Case::new("ui::code_editor::markdown::resources::native_mdx_rename_restores_both_views_link_navigation", renamed_mdx_links_in_both_views),
    ].into_iter()
}

#[cfg(test)]
#[gpui_kit::test]
fn unsaved_image_paste_explains_save_location_without_source_or_assets(cx: &mut TestAppContext) {
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    app.update_in(&mut vcx, |app, window, cx| {
        app.editor_new_markdown_file(window, cx)
    });
    settle(&mut vcx);
    vcx.update(|_, cx| {
        cx.write_to_clipboard(ClipboardItem {
            entries: vec![ClipboardEntry::Image(gpui_kit::Image {
                bytes: png(),
                format: gpui_kit::ImageFormat::Png,
                id: 2382,
            })],
        })
    });
    vcx.dispatch_action(gpui_kit::component::input::Paste);
    until(&mut vcx, |cx| {
        cx.update(|window, _| window.try_find("markdown-resource-notice").is_some())
    });
    vcx.update(|window, _| {
        assert_eq!(
            window.find("markdown-resource-notice").label(),
            Some(t(L10nKey::MarkdownImageChooseLocation))
        )
    });
    app.read_with(&vcx, |app, cx| {
        let file = app.active_buffer().unwrap();
        assert!(file.untitled.is_some());
        assert_eq!(file.input.read(cx).text().len(), 0);
        assert!(!file.dirty);
    });
}

#[cfg(test)]
#[gpui_kit::test]
fn attachment_completion_rejects_changed_source_and_selection(cx: &mut TestAppContext) {
    use super::super::test_host::{FileHost, canonical_tempdir};
    use std::sync::atomic::Ordering;
    let remote = canonical_tempdir();
    std::fs::write(remote.path().join("late.md"), b"# Original\n").unwrap();
    let host = FileHost::mapped(HostId(2383), remote.path());
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    app.update_in(&mut vcx, |app, window, cx| {
        app.editor_open_on_host(host.clone(), Path::new("/late.md"), window, cx)
    });
    until(&mut vcx, |cx| {
        app.read_with(cx, |app, _| app.active_buffer().is_some())
    });
    let gate = host.pause_next_write();
    vcx.update(|_, cx| {
        cx.write_to_clipboard(ClipboardItem {
            entries: vec![ClipboardEntry::Image(gpui_kit::Image {
                bytes: png(),
                format: gpui_kit::ImageFormat::Png,
                id: 2383,
            })],
        })
    });
    vcx.dispatch_action(gpui_kit::component::input::Paste);
    until(&mut vcx, |_| host.entered.load(Ordering::SeqCst));
    vcx.update(|window, cx| window.input("Changed ", cx));
    gate.send(()).unwrap();
    until(&mut vcx, |cx| {
        cx.update(|window, _| {
            window
                .try_find("markdown-resource-notice")
                .is_some_and(|notice| {
                    notice.label() == Some(t(L10nKey::MarkdownImageSelectionChanged))
                })
        })
    });
    app.read_with(&vcx, |app, cx| {
        assert!(
            !app.active_buffer()
                .unwrap()
                .input
                .read(cx)
                .text()
                .to_string()
                .contains("assets/")
        );
        assert!(
            app.active_buffer()
                .unwrap()
                .input
                .read(cx)
                .text()
                .to_string()
                .contains("Changed ")
        );
    });
    assert_eq!(
        std::fs::read(remote.path().join("late.md")).unwrap(),
        b"# Original\n"
    );
    let before = app.read_with(&vcx, |app, cx| {
        app.active_buffer()
            .unwrap()
            .input
            .read(cx)
            .text()
            .to_string()
    });
    host.fail_next_write();
    vcx.dispatch_action(gpui_kit::component::input::Paste);
    until(&mut vcx, |cx| {
        cx.update(|window, _| {
            window
                .try_find("markdown-resource-notice")
                .is_some_and(|notice| {
                    notice
                        .label()
                        .is_some_and(|label| label.contains(t(L10nKey::MarkdownImageWriteFailed)))
                })
        })
    });
    app.read_with(&vcx, |app, cx| {
        assert_eq!(
            app.active_buffer()
                .unwrap()
                .input
                .read(cx)
                .text()
                .to_string(),
            before
        )
    });
    assert_eq!(
        std::fs::read_dir(remote.path().join("assets"))
            .unwrap()
            .count(),
        1,
        "failed import removes only its reserved attachment, preserving the earlier stale result"
    );
}

#[cfg(test)]
#[gpui_kit::test]
fn resource_errors_svg_isolation_and_explicit_network_no_redirect(cx: &mut TestAppContext) {
    use std::io::{Read as _, Write as _};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let seen = Arc::new(AtomicUsize::new(0));
    let stop = Arc::new(AtomicBool::new(false));
    let worker_seen = seen.clone();
    let worker_stop = stop.clone();
    let worker = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(20);
        while !worker_stop.load(Ordering::SeqCst) && Instant::now() < deadline {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    stream
                        .set_read_timeout(Some(Duration::from_secs(2)))
                        .unwrap();
                    let _ = stream.read(&mut [0; 8192]);
                    worker_seen.fetch_add(1, Ordering::SeqCst);
                    stream.write_all(b"HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:1/forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(5))
                }
                Err(error) => panic!("fixture listener: {error}"),
            }
        }
    });
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("policy.md");
    let url = format!("http://{address}/image.png");
    let source = format!(
        "# Policy\n\n![]({url})\n\n![](missing.png)\n\n![](unsafe.svg)\n\n`![](code.png)`\n"
    );
    std::fs::write(&path, &source).unwrap();
    std::fs::write(directory.path().join("unsafe.svg"), br#"<svg xmlns="http://www.w3.org/2000/svg" width="12" height="12"><image href="file:///unintended-local.png" width="12" height="12"/></svg>"#).unwrap();
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    app.update_in(&mut vcx, |app, window, cx| {
        app.open_file_in_editor(&path, window, cx)
    });
    until(&mut vcx, |cx| {
        cx.update(|window, _| {
            window
                .try_find(SharedString::from(format!("image-status-{url}")))
                .is_some()
        })
    });
    until(&mut vcx, |cx| {
        cx.update(|window, _| {
            window
                .try_find(SharedString::from("image-status-unsafe.svg"))
                .is_some_and(|status| {
                    status.label().is_some_and(|label| {
                        label.contains(t(L10nKey::MarkdownImageExternalReference))
                    })
                })
        })
    });
    assert_eq!(
        seen.load(Ordering::SeqCst),
        0,
        "opening never authorizes an HTTP request"
    );
    vcx.update(|window, cx| {
        assert!(
            window
                .try_find(SharedString::from("image-status-code.png"))
                .is_none()
        );
        window.click(SharedString::from(format!("image-retry-{url}")), cx);
    });
    until(&mut vcx, |cx| {
        cx.update(|window, _| {
            window
                .find(SharedString::from(format!("image-status-{url}")))
                .label()
                .is_some_and(|label| label.contains("302"))
        })
    });
    assert_eq!(
        seen.load(Ordering::SeqCst),
        1,
        "only the explicit resource URL is requested"
    );
    app.read_with(&vcx, |app, cx| {
        assert_eq!(
            app.active_buffer()
                .unwrap()
                .input
                .read(cx)
                .text()
                .to_string(),
            source
        )
    });
    assert_eq!(std::fs::read(&path).unwrap(), source.as_bytes());
    stop.store(true, Ordering::SeqCst);
    worker.join().unwrap();
}

#[cfg(feature = "native-markdown-tests")]
pub(super) fn complete_resources(cx: &mut TestAppContext) {
    use super::super::test_host::{FileHost, canonical_tempdir};
    use gpui_kit::test::TestEventExt as _;
    use sha2::{Digest as _, Sha256};
    let directory = canonical_tempdir();
    let host = FileHost::mapped(HostId(2384), directory.path());
    let mut source = String::from("# Complete resource document\n\n");
    for ix in 0..32 {
        source.push_str(&format!("## Resource {ix}\n\n![sample {ix}](sample.svg)\n\nInline $x_{{{ix}}}^2+1$ remains editable.\n\n$$\n\\frac{{{ix}+x}}{{1+x^2}}\n$$\n\n"));
        if ix % 4 == 0 {
            source.push_str(&format!(
                "```mermaid\nflowchart LR\n    A{ix} --> B{ix}\n```\n\n"
            ));
        }
    }
    assert_eq!(
        source.len(),
        3768,
        "complete fixed resource fixture, never truncated"
    );
    assert_eq!(
        Sha256::digest(source.as_bytes()).as_slice(),
        &[
            0x97, 0x7a, 0x50, 0x98, 0x4b, 0xdd, 0xf6, 0x08, 0xaf, 0xb8, 0x96, 0x5a, 0xb4, 0x9d,
            0x13, 0xfd, 0xa9, 0xbe, 0x78, 0x84, 0x82, 0xd7, 0xc2, 0xee, 0xf9, 0xf8, 0xe0, 0x9d,
            0x95, 0x7c, 0xb0, 0x61,
        ]
    );
    let svg = b"<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"160\" height=\"96\" viewBox=\"0 0 160 96\"><rect x=\"12\" y=\"12\" width=\"136\" height=\"72\" rx=\"8\" fill=\"#4477aa\"/><text x=\"80\" y=\"56\" text-anchor=\"middle\" fill=\"white\" font-size=\"18\">image</text></svg>\n";
    assert_eq!(
        Sha256::digest(svg).as_slice(),
        &[
            0xf7, 0x9f, 0x0b, 0xb6, 0xe6, 0xc0, 0xd6, 0x8d, 0x04, 0xdf, 0x5e, 0x4e, 0xca, 0xaa,
            0x70, 0xe7, 0x0c, 0x6b, 0x96, 0x5d, 0xa9, 0x5c, 0xa0, 0x3b, 0x1d, 0x5f, 0xf7, 0x55,
            0xd1, 0xa9, 0x42, 0xaa,
        ]
    );
    std::fs::write(directory.path().join("resources.md"), &source).unwrap();
    std::fs::write(directory.path().join("sample.svg"), svg).unwrap();
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    vcx.simulate_resize(gpui_kit::size(px(1400.), px(1000.)));
    app.update_in(&mut vcx, |app, window, cx| {
        app.editor_open_on_host(host.clone(), Path::new("/resources.md"), window, cx)
    });
    until(&mut vcx, |cx| {
        app.read_with(cx, |app, _| app.active_buffer().is_some())
    });
    let view = app.read_with(&vcx, |app, _| {
        app.active_editor_input()
            .unwrap()
            .markdown()
            .unwrap()
            .clone()
    });
    until(&mut vcx, |cx| {
        view.read_with(cx, |view, _| view.image_bounds("sample.svg").is_some())
    });
    let at = source.rfind("remains editable.").unwrap() + "remains editable.".len();
    view.update_in(&mut vcx, |view, window, cx| {
        view.select_range(at..at, cx);
        view.focus_handle(cx).focus(window, cx);
    });
    settle(&mut vcx);
    let caret = view.read_with(&vcx, |view, _| view.range_to_bounds(&(at..at)).unwrap());
    vcx.update(|window, cx| {
        let point = caret.center();
        window.pointer_move(point, None, cx);
        window.pointer_down(point, MouseButton::Left, cx);
        window.pointer_up(point, MouseButton::Left, cx);
        window.input("Q", cx);
    });
    settle(&mut vcx);
    let mut expected = source.clone();
    expected.insert(at, 'Q');
    view.read_with(&vcx, |view, _| {
        assert!(view.is_live());
        assert_eq!(view.text(), expected);
    });
    vcx.dispatch_action(crate::core::actions::EditorSave);
    until(&mut vcx, |cx| {
        app.read_with(cx, |app, _| {
            !app.active_buffer().unwrap().dirty && !app.active_buffer().unwrap().saving
        })
    });
    assert_eq!(
        std::fs::read(directory.path().join("resources.md")).unwrap(),
        expected.as_bytes()
    );
    vcx.dispatch_action(gpui_kit::component::input::Undo);
    vcx.dispatch_action(crate::core::actions::EditorSave);
    until(&mut vcx, |cx| {
        app.read_with(cx, |app, _| {
            !app.active_buffer().unwrap().dirty && !app.active_buffer().unwrap().saving
        })
    });
    assert_eq!(
        std::fs::read(directory.path().join("resources.md")).unwrap(),
        source.as_bytes()
    );
    assert_eq!(
        std::fs::read(directory.path().join("sample.svg")).unwrap(),
        svg
    );
}
