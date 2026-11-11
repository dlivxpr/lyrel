// Real font ink requires the OS-main-thread runner, not NoopTextSystem.
use super::*;
use crate::daemon::protocol::{ClientMsg, DaemonMsg};
use crate::daemon::transport::Stream;
use crate::terminal::remote::PtySource;
use gpui_kit::test::{TestEventExt as _, TestWindowExt as _};
use gpui_kit::{Bounds, Entity, TestAppContext, point};

mod graphics_pty_fixture {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/support/kitty_pty.rs"
    ));
}

include!("graphics_pty_consumer_support.rs");

// Child processes use the same real daemon and raw PTY sender as libtest.
// Match the exact fixture selector before interpreting inherited environment.
pub(crate) fn run_fixture() -> Option<std::process::ExitCode> {
    let scope = module_path!().split_once("::").unwrap().1;
    let arguments: Vec<_> = std::env::args().collect();
    if arguments
        .iter()
        .any(|argument| argument == &format!("{scope}::graphics_fixture_daemon"))
    {
        let dir = std::env::var_os("LYREL_GRAPHICS_TEST_DAEMON").expect("isolated daemon config");
        lyrel_core::core::config::set_config_dir(dir.into());
        lyrel_core::daemon::server::run_daemon().unwrap();
        return Some(std::process::ExitCode::SUCCESS);
    }
    if arguments
        .iter()
        .any(|argument| argument == &format!("{scope}::graphics_fixture_sender"))
    {
        graphics_pty_fixture::pty_sender();
        return Some(std::process::ExitCode::SUCCESS);
    }
    None
}

fn reading_painted_glyph_colors(
    window: gpui_kit::AnyWindowHandle,
    view: &Entity<TerminalView>,
    row: usize,
    col: usize,
    cx: &mut TestAppContext,
) -> Vec<gpui_kit::Hsla> {
    window
        .update(cx, |_, window, cx| {
            window.simulate_next_frame(cx);
            window.render_frame(cx);
            let geom = view.read(cx).cell_geom.expect("production frame");
            let cell = Bounds::new(
                point(
                    geom.origin.x + geom.cell_width * col as f32,
                    geom.origin.y + geom.line_height * row as f32,
                ),
                gpui_kit::size(geom.cell_width, geom.line_height),
            )
            .map(|pixel| gpui_kit::ScaledPixels(pixel.as_f32() * window.scale_factor()));
            let scene = window.painted_scene();
            scene
                .monochrome_sprites
                .iter()
                .map(|sprite| (sprite.bounds, sprite.content_mask.bounds, sprite.color))
                .chain(
                    scene
                        .subpixel_sprites
                        .iter()
                        .map(|sprite| (sprite.bounds, sprite.content_mask.bounds, sprite.color)),
                )
                .filter_map(|(bounds, mask, color)| {
                    let visible = bounds.intersect(&mask).intersect(&cell);
                    (visible.size.width.0 > 0. && visible.size.height.0 > 0.).then_some(color)
                })
                .collect()
        })
        .unwrap()
}

fn reading_copy_links_theme_and_ime_use_the_painted_semantic_rows(cx: &mut TestAppContext) {
    crate::core::config::pin_test_config_dir();
    cx.executor().allow_parking();
    let daemon = GraphicsDaemonProcess::start();
    std::fs::write(daemon.dir.path().join("frozen.txt"), b"owned old file").unwrap();
    std::fs::write(daemon.dir.path().join("new.txt"), b"owned replacement file").unwrap();
    // The sender really runs here: an injected git-status cwd would be
    // replaced by the next production foreground-process poll.
    let pane = daemon.spawn_with_cwd(Some(daemon.dir.path().to_path_buf()));
    cx.update(|cx| {
        crate::register_bundled_fonts(cx);
        gpui_kit::component::init(cx);
        cx.set_global(Config::default());
    });
    let (view, window) = graphics_rooted_consumer(cx, pane, daemon.stream(pane));
    graphics_send_fixture(&view, cx, 60, &b"HISTORY\r\n".repeat(72));
    graphics_send_fixture(&view, cx, 61, "\x1b[5;1H\x1b[2Kfrozen 宽e\u{301}\x1b[6;1H\x1b[2K\x1b]8;;https://old.example/\x1b\\old-link\x1b]8;;\x1b\\\x1b[7;1H\x1b[2K\x1b[3gA\x1b[5G\x1bHB\r\x1b[2C\tC\x1b[9;1H\x1b[2K\x1b[31mansi\x1b[0m\x1b[12;1H\x1b[2Kfrozen.txt:9:2".as_bytes());
    view.update(cx, |view, cx| {
        view.smooth_scroll(1., cx);
    });
    graphics_assert_production_paint(window, &view, cx);
    let before = reading_painted_rows(&view, cx);
    graphics_send_fixture(&view, cx, 62, b"\x1b[5;1H\x1b[2Klive-replacement\x1b[6;1H\x1b[2K\x1b]8;;https://new.example/\x1b\\new-link\x1b]8;;\x1b\\\x1b[12;1H\x1b[2Knew.txt:3:4\x1b[3g\x1b[24;1H\r\nADVANCE\r\n");
    graphics_assert_production_paint(window, &view, cx);
    let geom = view.read_with(cx, |view, _| view.cell_geom.unwrap());
    let start = point(
        geom.origin.x + geom.cell_width * 0.1,
        geom.origin.y + geom.line_height * 5.5,
    );
    let end = point(geom.origin.x + geom.cell_width * 9.9, start.y);
    window
        .update(cx, |_, window, cx| {
            window.pointer_move(start, None, cx);
            window.pointer_down(start, MouseButton::Left, cx);
            window.pointer_move(end, Some(MouseButton::Left), cx);
            window.pointer_up(end, MouseButton::Left, cx);
        })
        .unwrap();
    window
        .update(cx, |_, window, cx| {
            window.dispatch_action(Box::new(CopyText), cx)
        })
        .unwrap();
    assert_eq!(
        cx.update(|cx| cx.read_from_clipboard().and_then(|item| item.text()))
            .as_deref(),
        Some("frozen 宽e\u{301}")
    );
    graphics_assert_production_paint(window, &view, cx);
    assert_eq!(
        view.read_with(cx, |view, _| view.selection_text())
            .as_deref(),
        Some("frozen 宽e\u{301}"),
        "a production redraw must not discard reading selection"
    );
    window
        .update(cx, |_, window, cx| {
            view.update(cx, |view, cx| {
                view.open_search(window, cx);
                assert_eq!(
                    view.search.as_ref().unwrap().input.read(cx).value(),
                    "frozen 宽e\u{301}"
                );
                assert_eq!(view.selection_text().as_deref(), Some("frozen 宽e\u{301}"));
                view.close_search(window, cx);
            });
        })
        .unwrap();
    window
        .update(cx, |_, window, cx| {
            view.update(cx, |view, cx| assert!(view.open_link_at(2, 6, window, cx)));
        })
        .unwrap();
    assert_eq!(cx.opened_url().as_deref(), Some("https://old.example/"));
    let opened = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let seen = opened.clone();
    view.update(cx, |_, cx| {
        let entity = cx.entity();
        cx.subscribe(&entity, move |_, _, event: &OpenFileRequested, _| {
            seen.borrow_mut()
                .push((event.path.clone(), event.line, event.column));
        })
        .detach();
    });
    assert_eq!(reading_painted_rows(&view, cx)[12], "frozen.txt:9:2");
    window
        .update(cx, |_, window, cx| {
            assert_eq!(
                view.read(cx).effective_host_cwd().as_deref(),
                Some(daemon.dir.path())
            );
            view.update(cx, |view, cx| assert!(view.open_link_at(2, 12, window, cx)));
        })
        .unwrap();
    cx.run_until_parked();
    assert_eq!(
        &*opened.borrow(),
        &[(daemon.dir.path().join("frozen.txt"), Some(9), Some(2))]
    );
    let foreground = reading_painted_glyph_colors(window, &view, 5, 0, cx);
    assert!(
        !foreground.is_empty(),
        "the retained text must actually paint glyphs"
    );
    cx.update(|cx| {
        gpui_kit::component::Theme::update(cx, |theme| theme.foreground = gpui_kit::white())
    });
    cx.update(|cx| {
        let mut ansi16: [rio_vt::config::colors::ColorRgb; 16] =
            view.read(cx).terminal.palette[..16].try_into().unwrap();
        ansi16[1] = rio_vt::config::colors::ColorRgb { r: 0, g: 255, b: 0 };
        cx.set_global(crate::terminal::palette::ActivePalette {
            ansi16,
            sel_bg: rio_vt::config::colors::ColorRgb { r: 0, g: 0, b: 0 },
        });
    });
    graphics_assert_production_paint(window, &view, cx);
    assert_eq!(reading_painted_rows(&view, cx), before);
    let changed_foreground = reading_painted_glyph_colors(window, &view, 5, 0, cx);
    assert!(!changed_foreground.is_empty());
    assert_ne!(
        changed_foreground, foreground,
        "retained glyph ink follows the changed theme"
    );
    assert!(
        changed_foreground
            .iter()
            .all(|color| *color == gpui_kit::white())
    );
    assert!(view.read_with(cx, |view, _| {
        view.grid_snap.as_ref().unwrap().painted_cursor().is_none()
    }));
    let ansi_ink = reading_painted_glyph_colors(window, &view, 9, 0, cx);
    assert!(!ansi_ink.is_empty());
    assert!(
        ansi_ink
            .iter()
            .all(|color| crate::terminal::palette::hsla_to_rgb(*color)
                == rio_vt::config::colors::ColorRgb { r: 0, g: 255, b: 0 }),
        "retained ANSI glyph ink follows the active palette"
    );
    view.update(cx, |view, cx| {
        view.on_select_start(0, 7, true, 1, false, cx);
        view.on_select_update(4, 7, false, cx);
        view.copy_selection(cx);
    });
    assert_eq!(
        cx.update(|cx| cx.read_from_clipboard().and_then(|item| item.text()))
            .as_deref(),
        Some("A \tC"),
        "copy uses captured custom stops after live TBC"
    );
    daemon.panes().kill(pane).unwrap();
    graphics_recovery_destroy(view, window, cx);
}

pub(crate) fn native_cases() -> impl Iterator<Item = crate::native_markdown::Case> {
    [crate::native_markdown::Case::new(
        "terminal::view::gpui_tests::reading_copy_links_theme_and_ime_use_the_painted_semantic_rows",
        reading_copy_links_theme_and_ime_use_the_painted_semantic_rows,
    )].into_iter()
}
