// Controlled real PTY -> daemon -> RemoteTerminal -> production TerminalElement.
// The buffer below is the actual completed paint input, not a live-grid capture.
fn reading_painted_rows(view: &Entity<TerminalView>, cx: &mut TestAppContext) -> Vec<String> {
    view.read_with(cx, |view, _| {
        let rows = view.cell_geom.expect("production frame").rows;
        (0..rows)
            .map(|row| sync_presented_row(view, row).trim_end().to_owned())
            .collect()
    })
}

#[gpui_kit::test]
fn reading_one_row_stays_fixed_while_real_pty_rewrites_and_appends(cx: &mut TestAppContext) {
    let (daemon, pane, view, window) = graphics_layout_harness(cx);
    let mut scene = b"\x1b[2J\x1b[H".to_vec();
    for row in 0..72 { scene.extend_from_slice(format!("READ-{row:03}\r\n").as_bytes()); }
    graphics_send_fixture(&view, cx, 1, &scene);
    view.update(cx, |view, cx| { view.smooth_scroll(1., cx); });
    graphics_assert_production_paint(window, &view, cx);
    let before = reading_painted_rows(&view, cx);
    assert_eq!(before.len(), 24);
    assert_eq!(before[0], "READ-048");
    graphics_send_fixture(&view, cx, 2, b"\x1b[2;1H\x1b[2KREWRITTEN\x1b[24;1H\r\nADVANCED\r\n");
    graphics_assert_production_paint(window, &view, cx);
    assert_eq!(reading_painted_rows(&view, cx), before, "all displayed rows, including the captured active screen, stay immutable");
    assert!(view.read_with(cx, |view, _| view.scrollback_text().contains("REWRITTEN")), "authoritative reader keeps parsing");
    view.update(cx, |view, _| view.jump_to_prompt());
    graphics_assert_production_paint(window, &view, cx);
    let fresh = reading_painted_rows(&view, cx);
    assert_ne!(fresh, before);
    assert!(fresh.iter().any(|row| row.contains("ADVANCED")));
    view.update(cx, |view, cx| { view.smooth_scroll(1., cx); });
    graphics_assert_production_paint(window, &view, cx);
    let second = reading_painted_rows(&view, cx);
    assert_ne!(second, before, "re-entry captures fresh cells");
    graphics_send_fixture(&view, cx, 3, b"\x1b[3;1H\x1b[2KFRESH-REWRITE");
    graphics_assert_production_paint(window, &view, cx);
    assert_eq!(reading_painted_rows(&view, cx), second);
    view.update(cx, |view, _| view.jump_to_prompt());
    view.update(cx, |view, cx| { view.smooth_scroll(24., cx); });
    graphics_assert_production_paint(window, &view, cx);
    let full_screen = reading_painted_rows(&view, cx);
    assert_ne!(full_screen, second);
    graphics_send_fixture(&view, cx, 4, b"\x1b[H\x1b[2J\x1b[24;1H\r\nNEW-LIVE\r\n");
    graphics_assert_production_paint(window, &view, cx);
    assert_eq!(reading_painted_rows(&view, cx), full_screen);
    daemon.panes().kill(pane).unwrap();
    graphics_recovery_destroy(view, window, cx);
}

#[gpui_kit::test]
fn reading_pill_is_centered_localized_focusable_and_returns_live(cx: &mut TestAppContext) {
    use gpui_kit::test::TestWindowExt as _;
    crate::ui::i18n::set_locale("en");
    let (daemon, pane, view, window) = graphics_layout_harness(cx);
    graphics_send_fixture(&view, cx, 10, &b"ROW\r\n".repeat(72));
    window.update(cx, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("terminal-return-live").is_none());
    }).unwrap();
    view.update(cx, |view, cx| { view.smooth_scroll(1., cx); });
    graphics_assert_production_paint(window, &view, cx);
    window.update(cx, |_, window, cx| {
        window.render_frame(cx);
        let pill = window.find("terminal-return-live");
        assert_eq!(pill.label(), Some("↓ 1 lines · Return to live"));
        assert!(pill.visible());
        let grid = view.read(cx).cell_geom.unwrap();
        let bounds = pill.bounds();
        let center = grid.origin.x + grid.cell_width * grid.cols as f32 / 2.;
        assert!((bounds.center().x - center).abs() <= px(1.));
        assert!(bounds.bottom() < grid.origin.y + grid.line_height * grid.rows as f32);
        assert!(bounds.top() > grid.origin.y + grid.line_height * (grid.rows / 2) as f32);
        window.press("tab", cx);
        assert_eq!(window.find("terminal-return-live").focused(), Some(true));
    }).unwrap();
    graphics_send_fixture(&view, cx, 11, b"\x1b[24;1H\r\n\r\n\r\n");
    graphics_assert_production_paint(window, &view, cx);
    window.update(cx, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("terminal-return-live").label(), Some("↓ 4 lines · Return to live"));
        window.press("enter", cx);
        assert!(window.try_find("terminal-return-live").is_none());
        assert_eq!(window.find("terminal-surface").focused(), Some(true));
    }).unwrap();
    assert_eq!(view.read_with(cx, |view, _| view.terminal.term.lock().display_offset()), 0);
    // A leaked Enter would terminate the real sender's next protocol request.
    graphics_send_fixture(&view, cx, 12, b"\x1b[0m");
    crate::ui::i18n::set_locale("zh-CN");
    view.update(cx, |view, cx| { view.smooth_scroll(1., cx); });
    graphics_assert_production_paint(window, &view, cx);
    window.update(cx, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("terminal-return-live").label(), Some("↓ 1 行 · 回到实时输出"));
        window.click("terminal-return-live", cx);
        assert!(window.try_find("terminal-return-live").is_none());
        assert_eq!(window.find("terminal-surface").focused(), Some(true));
    }).unwrap();
    crate::ui::i18n::set_locale("en");
    // Commit through the production input path, preserving input-driven return.
    view.update(cx, |view, cx| { view.smooth_scroll(1., cx); });
    graphics_assert_production_paint(window, &view, cx);
    view.update(cx, |view, cx| view.commit_text("SEND 13 0 1b5b306d 0\n", cx));
    graphics_assert_production_paint(window, &view, cx);
    window.update(cx, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("terminal-return-live").is_none());
    }).unwrap();
    graphics_send_fixture(&view, cx, 14, b"\x1b[0m");
    daemon.panes().kill(pane).unwrap();
    graphics_recovery_destroy(view, window, cx);
}

#[gpui_kit::test]
fn reading_pill_stays_inside_narrow_zoomed_panes_without_resizing_terminal_text(cx: &mut TestAppContext) {
    use gpui_kit::test::TestWindowExt as _;
    let (daemon, pane, view, window) = graphics_layout_harness(cx);
    graphics_send_fixture(&view, cx, 20, &b"ROW\r\n".repeat(72));
    let font = view.read_with(cx, |view, _| (view.font_size, view.line_height_mul));
    graphics_assert_production_paint_at(window, &view, cx, TermSize::new(24, 24));
    graphics_send_fixture(&view, cx, 21, b"\x1b[0m");
    view.update(cx, |view, cx| { view.smooth_scroll(1., cx); });
    for locale in ["en", "zh-CN"] {
        crate::ui::i18n::set_locale(locale);
        cx.update(|cx| gpui_kit::component::Theme::global_mut(cx).font_size = px(24.));
        graphics_assert_production_paint(window, &view, cx);
        window.update(cx, |_, window, cx| {
            window.render_frame(cx);
            let pill = window.find("terminal-return-live");
            let surface = window.find("terminal-surface").bounds();
            assert!(pill.bounds().left() >= surface.left());
            assert!(pill.bounds().right() <= surface.right());
            assert!(pill.bounds().bottom() < surface.bottom());
            assert!(pill.visible());
            assert_eq!((view.read(cx).font_size, view.read(cx).line_height_mul), font);
        }).unwrap();
    }
    crate::ui::i18n::set_locale("en");
    daemon.panes().kill(pane).unwrap();
    graphics_recovery_destroy(view, window, cx);
}

#[gpui_kit::test]
fn reading_scrollbar_uses_the_same_stable_rows_and_paste_releases_them(cx: &mut TestAppContext) {
    use gpui_kit::component::scroll::ScrollbarHandle as _;
    use gpui_kit::test::TestWindowExt as _;
    let (daemon, pane, view, window) = graphics_layout_harness(cx);
    graphics_send_fixture(&view, cx, 30, &b"SCROLLBAR\r\n".repeat(72));
    graphics_assert_production_paint(window, &view, cx);
    let live = reading_painted_rows(&view, cx);
    view.update(cx, |view, _| {
        let history = view.terminal.term.lock().grid.history_size();
        view.scroll_handle.set_offset(point(px(0.), -(view.line_height * (history - 24) as f32)));
    });
    graphics_assert_production_paint(window, &view, cx);
    let before = reading_painted_rows(&view, cx);
    assert_ne!(before, live, "the scrollbar must leave live before output can test capture stability");
    window.update(cx, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("terminal-return-live").is_some());
    }).unwrap();
    graphics_send_fixture(&view, cx, 31, b"\x1b[H\x1b[2J\x1b[24;1H\r\nSCROLLBAR-LIVE\r\n");
    graphics_assert_production_paint(window, &view, cx);
    assert_eq!(reading_painted_rows(&view, cx), before);
    cx.update(|cx| cx.write_to_clipboard(ClipboardItem::new_string("SEND 32 0 1b5b306d 0".into())));
    window.update(cx, |_, window, cx| {
        window.dispatch_action(Box::new(PasteText), cx);
    }).unwrap();
    cx.run_until_parked();
    view.read_with(cx, |view, _| view.terminal.write(b"\n".to_vec()));
    graphics_send_fixture(&view, cx, 33, b"\x1b[0m");
    graphics_assert_production_paint(window, &view, cx);
    assert_ne!(reading_painted_rows(&view, cx), before);
    window.update(cx, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("terminal-return-live").is_none());
    }).unwrap();
    daemon.panes().kill(pane).unwrap();
    graphics_recovery_destroy(view, window, cx);
}

#[gpui_kit::test]
fn reading_placeholders_paint_from_captured_rows_after_live_cells_are_erased(cx: &mut TestAppContext) {
    use std::sync::atomic::Ordering;
    let (daemon, pane, view, window) = graphics_layout_harness(cx);
    graphics_send_fixture(&view, cx, 40, &b"PLACEHOLDER\r\n".repeat(72));
    graphics_send_fixture(&view, cx, 41, "\x1b[8;5H\x1b_Ga=T,i=8,p=2,U=1,c=2,r=1,f=32,s=1,v=1,q=2;AAD//w==\x1b\\\x1b[38;5;8;58;5;2m\u{10eeee}\u{305}\u{305}\u{10eeee}\x1b[0m".as_bytes());
    graphics_layout_wait_image(&view, cx, 8);
    view.update(cx, |view, cx| { view.smooth_scroll(1., cx); });
    graphics_assert_production_paint(window, &view, cx);
    let before = reading_painted_rows(&view, cx);
    let (painted, bounds) = view.read_with(cx, |view, _| {
        let image = view.grid_snap.as_ref().unwrap().images.iter()
            .find(|image| image.placement.image_id == 8)
            .expect("captured placeholder must enter the production display list");
        assert_eq!(image.data.as_ref().unwrap().as_bytes(0).unwrap(), &[0, 0, 255, 255]);
        assert!(image.painted.load(Ordering::Acquire));
        let geom = view.cell_geom.unwrap();
        let offset = view.terminal.term.lock().display_offset() as i32;
        (image.painted.clone(), image.paint_bounds(geom.origin, geom.cell_width, geom.line_height, 0, offset, 1.))
    });
    painted.store(false, Ordering::Release);
    graphics_send_fixture(&view, cx, 42, b"\x1b[8;1H\x1b[2KERASED\x1b[24;1H\r\nADVANCED\r\n");
    graphics_assert_production_paint(window, &view, cx);
    assert_eq!(reading_painted_rows(&view, cx), before);
    view.read_with(cx, |view, _| {
        let image = view.grid_snap.as_ref().unwrap().images.iter()
            .find(|image| image.placement.image_id == 8)
            .expect("erasing live cells must not erase captured placeholder runs");
        assert_eq!(image.data.as_ref().unwrap().as_bytes(0).unwrap(), &[0, 0, 255, 255]);
        assert!(painted.load(Ordering::Acquire), "a new production frame must paint the captured image");
        let geom = view.cell_geom.unwrap();
        let offset = view.terminal.term.lock().display_offset() as i32;
        assert_eq!(image.paint_bounds(geom.origin, geom.cell_width, geom.line_height, 0, offset, 1.), bounds);
    });
    daemon.panes().kill(pane).unwrap();
    graphics_recovery_destroy(view, window, cx);
}
