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

/// Real fonts must answer the coverage query truthfully and the accepted
/// registration must paint with the pane's effective foreground. The source
/// codepoint is PUA-B: outside the Nerd Font ranges, so any custom `Path` in
/// its cell can only come from the registration.
///
/// The `system` bit is not assumed: whether the platform text stack covers a
/// codepoint is platform-dependent (macOS shapes PUA-B through its
/// last-resort cascade to a real glyph id; DirectWrite answers glyph 0), so
/// the expected reply is derived from independent shaping evidence below —
/// never from the query reply itself.
fn glyph_native_system_coverage_and_registered_paint(cx: &mut TestAppContext) {
    crate::core::config::pin_test_config_dir();
    cx.executor().allow_parking();
    let daemon = GraphicsDaemonProcess::start();
    let pane = daemon.spawn();
    cx.update(|cx| {
        crate::register_bundled_fonts(cx);
        gpui_kit::component::init(cx);
        cx.set_global(Config::default());
    });
    let (view, window) = graphics_rooted_consumer(cx, pane, daemon.stream(pane));
    let replies = tempfile::TempDir::new().unwrap();

    /// Independent oracle for the wire `system` bit: shape `cp` alone through
    /// the pane's real font and platform stack, and read whether any emitted
    /// glyph has a real id. A nonzero id — even a zero-ink one — is what the
    /// cell would paint, so it is coverage; 0 is `.notdef`.
    let system_covers = |cp: u32, cx: &mut TestAppContext| {
        let (font, font_size) = view.read_with(cx, |view, _| (view.font.clone(), view.font_size));
        cx.update(|cx| {
            let ch = char::from_u32(cp).unwrap();
            let text = gpui_kit::SharedString::from(ch.to_string());
            let run = gpui_kit::TextRun {
                len: text.len(),
                font,
                color: gpui_kit::Hsla::default(),
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            gpui_kit::WindowTextSystem::new(cx.text_system().clone())
                .shape_line(text, font_size, &[run], None)
                .runs
                .iter()
                .any(|run| run.glyphs.iter().any(|glyph| glyph.id.0 != 0))
        })
    };
    let expected_status = |system: bool, glossary: bool| match (system, glossary) {
        (true, true) => "system,glossary",
        (true, false) => "system",
        (false, true) => "glossary",
        (false, false) => "",
    };

    let reply = glyph_send(
        &view,
        cx,
        1,
        &replies.path().join("r1"),
        &glyph_query(GLYPH_CP),
        1,
    );
    assert_eq!(
        reply,
        format!(
            "\x1b_25a1;q;cp={GLYPH_CP:x};status={}\x1b\\",
            expected_status(system_covers(GLYPH_CP, cx), false)
        )
        .into_bytes(),
        "U+100000 reports this stack's real coverage before registration"
    );

    let reply = glyph_send(
        &view,
        cx,
        2,
        &replies.path().join("r2"),
        &glyph_query(0x41),
        1,
    );
    assert_eq!(
        reply,
        b"\x1b_25a1;q;cp=41;status=system\x1b\\".to_vec(),
        "A is covered by the real font stack"
    );

    // Before any registration the cell is painted by ordinary font shaping
    // only: no custom `Path` may appear inside it.
    let display = format!("\x1b[3;5H{cp}", cp = char::from_u32(GLYPH_CP).unwrap());
    graphics_send_fixture(&view, cx, 3, display.as_bytes());
    glyph_wait(cx, "native unregistered codepoint presented", |cx| {
        view.read_with(cx, |view, _| {
            view.cell_geom.is_some()
                && glyph_presented_row(view, 2).chars().nth(4) == Some('\u{100000}')
        })
    });
    let cell = glyph_cell_bounds(window, &view, cx, 2, 4);
    assert!(
        glyph_paths_in_cell(window, &view, cx, cell).is_empty(),
        "no custom path before the registration"
    );

    let reply = glyph_send(
        &view,
        cx,
        4,
        &replies.path().join("r4"),
        &glyph_register(GLYPH_CP, &glyph_square(1000), 1),
        1,
    );
    assert_eq!(
        reply,
        format!("\x1b_25a1;r;cp={GLYPH_CP:x};status=0\x1b\\").into_bytes(),
        "registration accepted"
    );

    let reply = glyph_send(
        &view,
        cx,
        5,
        &replies.path().join("r5"),
        &glyph_query(GLYPH_CP),
        1,
    );
    assert_eq!(
        reply,
        format!(
            "\x1b_25a1;q;cp={GLYPH_CP:x};status={}\x1b\\",
            expected_status(system_covers(GLYPH_CP, cx), true)
        )
        .into_bytes(),
        "the registration adds the glossary bit to this stack's coverage"
    );

    // The registered definition owns the cell: exactly one `Path` with the
    // cell's effective foreground.
    glyph_wait(cx, "native glyph painted", |cx| {
        !glyph_paths_in_cell(window, &view, cx, cell).is_empty()
    });
    let expected_fg = view.read_with(cx, |view, _| {
        let cols = view.cell_geom.unwrap().cols;
        view.grid_buf[2 * cols + 4].fg
    });
    let first = glyph_paths_in_cell(window, &view, cx, cell);
    assert_eq!(
        first.len(),
        1,
        "one native registered glyph path: {first:?}"
    );
    assert_eq!(
        first[0].1.as_solid(),
        Some(expected_fg),
        "native glyph uses the cell's effective foreground"
    );

    // A replacement redefines the same codepoint: the painted path changes
    // extent while staying a single registration-owned `Path`.
    let reply = glyph_send(
        &view,
        cx,
        6,
        &replies.path().join("r6"),
        &glyph_register(GLYPH_CP, &glyph_rect(2000, 500), 1),
        1,
    );
    assert_eq!(
        reply,
        format!("\x1b_25a1;r;cp={GLYPH_CP:x};status=0\x1b\\").into_bytes(),
        "replacement accepted"
    );
    glyph_wait(cx, "replacement repainted", |cx| {
        glyph_paths_in_cell(window, &view, cx, cell)
            .first()
            .is_some_and(|(bounds, _)| *bounds != first[0].0)
    });
    let second = glyph_paths_in_cell(window, &view, cx, cell);
    assert_eq!(second.len(), 1, "replacement is one glyph path: {second:?}");
    assert_ne!(
        second[0].0, first[0].0,
        "replacement changes the painted extent"
    );
    assert_eq!(
        second[0].1.as_solid(),
        Some(expected_fg),
        "replacement keeps the cell's effective foreground"
    );

    // Clear drops the derived picture: the cell returns to ordinary font
    // rendering — again no custom `Path` — and the query loses the glossary
    // bit.
    let reply = glyph_send(
        &view,
        cx,
        7,
        &replies.path().join("r7"),
        &glyph_clear(Some(GLYPH_CP)),
        1,
    );
    assert_eq!(
        reply,
        format!("\x1b_25a1;c;cp={GLYPH_CP:x};status=0\x1b\\").into_bytes(),
        "clear succeeds"
    );
    glyph_wait(cx, "clear drops the registered picture", |cx| {
        glyph_paths_in_cell(window, &view, cx, cell).is_empty()
    });
    let reply = glyph_send(
        &view,
        cx,
        8,
        &replies.path().join("r8"),
        &glyph_query(GLYPH_CP),
        1,
    );
    assert_eq!(
        reply,
        format!(
            "\x1b_25a1;q;cp={GLYPH_CP:x};status={}\x1b\\",
            expected_status(system_covers(GLYPH_CP, cx), false)
        )
        .into_bytes(),
        "query after clear no longer reports the glossary"
    );
}

/// Real fonts: a declared advance/baseline registration sits on the actual
/// primary baseline, then follows a font-size and display-scale change without
/// asking the application to register again. Prepared for the native slot; the
/// headless consumer covers the same inputs deterministically.
fn glyph_native_declared_layout_baseline_and_scale(cx: &mut TestAppContext) {
    crate::core::config::pin_test_config_dir();
    cx.executor().allow_parking();
    let daemon = GraphicsDaemonProcess::start();
    let pane = daemon.spawn();
    cx.update(|cx| {
        crate::register_bundled_fonts(cx);
        gpui_kit::component::init(cx);
        cx.set_global(Config::default());
    });
    let (view, window) = graphics_rooted_consumer(cx, pane, daemon.stream(pane));
    let replies = tempfile::TempDir::new().unwrap();
    let reply_path = |serial: usize| replies.path().join(format!("r{serial}"));
    let cp = GLYPH_CP;
    let glyph = char::from_u32(cp).unwrap();
    let mut serial = 0usize;

    serial += 1;
    glyph_register_ok(
        &view,
        cx,
        serial,
        &reply_path(serial),
        cp,
        &glyph_square(1000),
        1000,
        "aw=1000;lh=1000;ymin=-200;size=advance;align=center,baseline;pad=0.1,0.05,0,0.05",
    );
    serial += 1;
    glyph_show(&view, cx, serial, 2, 4, &glyph.to_string());
    glyph_wait(cx, "native declared glyph painted", |cx| {
        view.read_with(cx, |view, _| {
            view.cell_geom.is_some() && glyph_presented_row(view, 2).chars().nth(4) == Some(glyph)
        })
    });

    let cell = glyph_region_bounds(window, &view, cx, 2, 4, 1);
    let (font, font_size, line_height) = view.read_with(cx, |view, _| {
        (view.font.clone(), view.font_size, view.line_height)
    });
    let scale = window
        .update(cx, |_, window, _| window.scale_factor())
        .unwrap();
    let baseline = window
        .update(cx, |_, window, _| {
            let font_id = window.text_system().resolve_font(&font);
            window
                .text_system()
                .baseline_offset(font_id, font_size, line_height)
        })
        .unwrap();
    let painted = glyph_paths_with_clip_in_region(window, &view, cx, cell);
    assert_eq!(painted.len(), 1, "one native declared path: {painted:?}");
    let side = cell.size.width.0 * 0.9;
    assert!(
        (painted[0].0.size.width.0 - side).abs() <= 1.5
            && (painted[0].0.size.height.0 - side).abs() <= 1.5,
        "advance drives the declared extent: {:?} vs side {side}",
        painted[0].0
    );
    let baseline_scaled = baseline.as_f32() * scale;
    let bottom = painted[0].0.origin.y.0 + painted[0].0.size.height.0;
    assert!(
        (bottom - (cell.origin.y.0 + baseline_scaled)).abs() <= 2.,
        "authored y=0 lands on the real primary baseline: bottom={bottom}, baseline={}",
        cell.origin.y.0 + baseline_scaled
    );
    let first_revision = view.read_with(cx, |view, _| {
        view.terminal
            .glyphs()
            .get(cp)
            .expect("registration")
            .revision
    });

    // Font-size change: the same registration scales to the new real metrics.
    view.update(cx, |view, _| view.font_size = view.font_size * 2.);
    let _ = glyph_paths_with_clip_in_region(window, &view, cx, cell);
    let grown = glyph_region_bounds(window, &view, cx, 2, 4, 1);
    assert!(grown.size.width.0 > cell.size.width.0 + 1.5);
    let painted = glyph_paths_with_clip_in_region(window, &view, cx, grown);
    assert_eq!(painted.len(), 1, "one grown native path: {painted:?}");
    let side = grown.size.width.0 * 0.9;
    assert!(
        (painted[0].0.size.width.0 - side).abs() <= 1.5
            && (painted[0].0.size.height.0 - side).abs() <= 1.5,
        "the native declaration scales with the font size: {:?}",
        painted[0].0
    );
    assert_eq!(
        view.read_with(cx, |view, _| view
            .terminal
            .glyphs()
            .get(cp)
            .expect("registration")
            .revision),
        first_revision,
        "font-size change does not re-register"
    );

    // Display-scale change: same logical geometry, new painted scale.
    let before_scale = glyph_region_bounds(window, &view, cx, 2, 4, 1);
    let current_scale = window
        .update(cx, |_, window, _| window.scale_factor())
        .unwrap();
    let target_scale = if (current_scale - 2.0).abs() < f32::EPSILON {
        1.0
    } else {
        2.0
    };
    cx.simulate_window_scale_factor_change(window, target_scale);
    let _ = glyph_paths_with_clip_in_region(window, &view, cx, before_scale);
    let after_scale = glyph_region_bounds(window, &view, cx, 2, 4, 1);
    assert!(
        (after_scale.size.width.0 - before_scale.size.width.0).abs() > 1.5,
        "the display scale changed the painted cell: {after_scale:?} vs {before_scale:?}"
    );
    let painted = glyph_paths_with_clip_in_region(window, &view, cx, after_scale);
    assert_eq!(painted.len(), 1, "one rescaled native path: {painted:?}");
    let side = after_scale.size.width.0 * 0.9;
    assert!(
        (painted[0].0.size.width.0 - side).abs() <= 1.5
            && (painted[0].0.size.height.0 - side).abs() <= 1.5,
        "the native declaration follows the display scale: {:?}",
        painted[0].0
    );
    assert_eq!(
        view.read_with(cx, |view, _| view
            .terminal
            .glyphs()
            .get(cp)
            .expect("registration")
            .revision),
        first_revision,
        "display-scale change does not re-register"
    );
}

pub(crate) fn native_cases() -> impl Iterator<Item = crate::native_markdown::Case> {
    [
        crate::native_markdown::Case::new(
            "terminal::view::native_terminal_tests::glyph_native_system_coverage_and_registered_paint",
            glyph_native_system_coverage_and_registered_paint,
        ),
        crate::native_markdown::Case::new(
            "terminal::view::native_terminal_tests::glyph_native_declared_layout_baseline_and_scale",
            glyph_native_declared_layout_baseline_and_scale,
        ),
        crate::native_markdown::Case::new(
            "terminal::view::gpui_tests::reading_copy_links_theme_and_ime_use_the_painted_semantic_rows",
            reading_copy_links_theme_and_ime_use_the_painted_semantic_rows,
        ),
    ]
    .into_iter()
}
