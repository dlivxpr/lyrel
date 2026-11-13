// Real PTY -> daemon -> RemoteTerminal -> production TerminalElement.
// Native glyph/theme/cursor coverage lives in the native_terminal target;
// NoopTextSystem deliberately produces no glyph raster bounds in this target.

// These fixtures have no application-painted cell backgrounds. Full-cell
// fills therefore observe the search decorations emitted by TerminalElement.
fn reading_painted_search_runs(
    window: gpui_kit::AnyWindowHandle,
    view: &Entity<TerminalView>,
    cx: &mut TestAppContext,
) -> Vec<(String, gpui_kit::Hsla)> {
    window
        .update(cx, |_, window, cx| {
            let view = view.read(cx);
            let geom = view.cell_geom.expect("production frame");
            let scale = window.scale_factor();
            let mut runs = Vec::new();
            for quad in window.painted_quads() {
                let Some(color) = quad.background.as_solid() else {
                    continue;
                };
                let bounds = quad.bounds.map(|pixel| px(pixel.0 / scale));
                let row = ((bounds.origin.y - geom.origin.y) / geom.line_height).round();
                let col = ((bounds.origin.x - geom.origin.x) / geom.cell_width).round();
                let len = (bounds.size.width / geom.cell_width).round();
                if row < 0.
                    || row >= geom.rows as f32
                    || col < 0.
                    || len < 1.
                    || col + len > geom.cols as f32
                {
                    continue;
                }
                let (row, col, len) = (row as usize, col as usize, len as usize);
                let expected = Bounds::new(
                    point(
                        geom.origin.x + geom.cell_width * col as f32,
                        geom.origin.y + geom.line_height * row as f32,
                    ),
                    gpui_kit::size(geom.cell_width * len as f32, geom.line_height),
                );
                if (bounds.origin.x - expected.origin.x).abs() > px(0.1)
                    || (bounds.origin.y - expected.origin.y).abs() > px(0.1)
                    || (bounds.size.width - expected.size.width).abs() > px(0.1)
                    || (bounds.size.height - expected.size.height).abs() > px(0.1)
                {
                    continue;
                }
                let text = view.grid_buf[row * geom.cols + col..row * geom.cols + col + len]
                    .iter()
                    .map(|cell| cell.c)
                    .collect();
                runs.push((row, col, text, color));
            }
            runs.sort_by_key(|(row, col, _, _)| (*row, *col));
            runs.into_iter()
                .map(|(_, _, text, color)| (text, color))
                .collect()
        })
        .unwrap()
}

// Establish the intended reading row from the authoritative live grid before
// any resize. Hard-coded history distances do not account for soft wrapping.
fn reading_scroll_to_live_row(
    view: &Entity<TerminalView>,
    cx: &mut TestAppContext,
    needle: &str,
    row_delta: i64,
) {
    view.update(cx, |view, cx| {
        let (distance, current) = {
            let term = view.terminal.term.lock();
            let live = term.grid.lines_evicted() as i64 + term.grid.history_size() as i64;
            let history = term.grid.history_size() as i32;
            let mut found = None;
            for line in -history..term.grid.screen_lines() as i32 {
                let text = term.grid[rio_vt::crosswords::pos::Line(line)][..]
                    .iter()
                    .filter(|cell| {
                        !matches!(
                            cell.wide(),
                            rio_vt::crosswords::square::Wide::Spacer
                                | rio_vt::crosswords::square::Wide::LeadingSpacer
                        )
                    })
                    .map(|cell| cell.c())
                    .collect::<String>();
                if text.contains(needle) {
                    found = Some(live + line as i64);
                    break;
                }
            }
            let target =
                found.unwrap_or_else(|| panic!("live row containing {needle:?}")) + row_delta;
            ((live - target) as i32, term.display_offset())
        };
        let delta = distance.saturating_sub(current.min(i32::MAX as usize) as i32);
        view.smooth_scroll(delta as f32, cx);
    });
}

#[gpui_kit::test]
fn reading_one_row_stays_fixed_while_real_pty_rewrites_and_appends(cx: &mut TestAppContext) {
    let (daemon, pane, view, window) = graphics_layout_harness(cx);
    let mut scene = b"\x1b[2J\x1b[H".to_vec();
    for row in 0..72 {
        scene.extend_from_slice(format!("READ-{row:03}\r\n").as_bytes());
    }
    graphics_send_fixture(&view, cx, 1, &scene);
    view.update(cx, |view, cx| {
        view.smooth_scroll(1., cx);
    });
    graphics_assert_production_paint(window, &view, cx);
    let before = reading_painted_rows(&view, cx);
    assert_eq!(before.len(), 24);
    assert_eq!(before[0], "READ-048");
    graphics_send_fixture(
        &view,
        cx,
        2,
        b"\x1b[2;1H\x1b[2KREWRITTEN\x1b[24;1H\r\nADVANCED\r\n",
    );
    graphics_assert_production_paint(window, &view, cx);
    let settled = reading_painted_rows(&view, cx);
    assert_eq!(settled.len(), before.len());
    assert_eq!(
        settled[2], "REWRITTEN",
        "the row that settled into history shows its final frame"
    );
    for (row, (now, then)) in settled.iter().zip(&before).enumerate() {
        if row != 2 {
            assert_eq!(
                now, then,
                "history rows outside the settled frame stay immutable"
            );
        }
    }
    assert!(
        view.read_with(cx, |view, _| view.scrollback_text().contains("REWRITTEN")),
        "authoritative reader keeps parsing"
    );
    view.update(cx, |view, _| view.jump_to_prompt());
    graphics_assert_production_paint(window, &view, cx);
    let fresh = reading_painted_rows(&view, cx);
    assert_ne!(fresh, before);
    assert!(fresh.iter().any(|row| row.contains("ADVANCED")));
    view.update(cx, |view, cx| {
        view.smooth_scroll(1., cx);
    });
    graphics_assert_production_paint(window, &view, cx);
    let second = reading_painted_rows(&view, cx);
    assert_ne!(second, before, "re-entry captures fresh cells");
    graphics_send_fixture(&view, cx, 3, b"\x1b[3;1H\x1b[2KFRESH-REWRITE");
    graphics_assert_production_paint(window, &view, cx);
    assert_eq!(reading_painted_rows(&view, cx), second);
    view.update(cx, |view, _| view.jump_to_prompt());
    view.update(cx, |view, cx| {
        view.smooth_scroll(24., cx);
    });
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
    window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("terminal-return-live").is_none());
        })
        .unwrap();
    view.update(cx, |view, cx| {
        view.smooth_scroll(1., cx);
    });
    graphics_assert_production_paint(window, &view, cx);
    window
        .update(cx, |_, window, cx| {
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
        })
        .unwrap();
    graphics_send_fixture(&view, cx, 11, b"\x1b[24;1H\r\n\r\n\r\n");
    graphics_assert_production_paint(window, &view, cx);
    window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(
                window.find("terminal-return-live").label(),
                Some("↓ 4 lines · Return to live")
            );
            window.press("enter", cx);
            assert!(window.try_find("terminal-return-live").is_none());
            assert_eq!(window.find("terminal-surface").focused(), Some(true));
        })
        .unwrap();
    assert_eq!(
        view.read_with(cx, |view, _| view.terminal.term.lock().display_offset()),
        0
    );
    // A leaked Enter would terminate the real sender's next protocol request.
    graphics_send_fixture(&view, cx, 12, b"\x1b[0m");
    crate::ui::i18n::set_locale("zh-CN");
    view.update(cx, |view, cx| {
        view.smooth_scroll(1., cx);
    });
    graphics_assert_production_paint(window, &view, cx);
    window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(
                window.find("terminal-return-live").label(),
                Some("↓ 1 行 · 回到实时输出")
            );
            window.click("terminal-return-live", cx);
            assert!(window.try_find("terminal-return-live").is_none());
            assert_eq!(window.find("terminal-surface").focused(), Some(true));
        })
        .unwrap();
    crate::ui::i18n::set_locale("en");
    // Commit through the production input path, preserving input-driven return.
    view.update(cx, |view, cx| {
        view.smooth_scroll(1., cx);
    });
    graphics_assert_production_paint(window, &view, cx);
    view.update(cx, |view, cx| {
        view.commit_text("SEND 13 0 1b5b306d 0\n", cx)
    });
    graphics_assert_production_paint(window, &view, cx);
    window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("terminal-return-live").is_none());
        })
        .unwrap();
    graphics_wait_fixture(&view, cx, 13, b"SEND 13 0 1b5b306d 0\n".len());
    graphics_send_fixture(&view, cx, 14, b"\x1b[0m");
    daemon.panes().kill(pane).unwrap();
    graphics_recovery_destroy(view, window, cx);
}

#[gpui_kit::test]
fn reading_pill_stays_inside_narrow_zoomed_panes_without_resizing_terminal_text(
    cx: &mut TestAppContext,
) {
    use gpui_kit::test::TestWindowExt as _;
    let (daemon, pane, view, window) = graphics_layout_harness(cx);
    graphics_send_fixture(&view, cx, 20, &b"ROW\r\n".repeat(72));
    let font = view.read_with(cx, |view, _| (view.font_size, view.line_height_mul));
    graphics_assert_production_paint_at(window, &view, cx, TermSize::new(24, 24));
    graphics_send_fixture(&view, cx, 21, b"\x1b[0m");
    view.update(cx, |view, cx| {
        view.smooth_scroll(1., cx);
    });
    for locale in ["en", "zh-CN"] {
        crate::ui::i18n::set_locale(locale);
        cx.update(|cx| gpui_kit::component::Theme::global_mut(cx).font_size = px(24.));
        graphics_assert_production_paint(window, &view, cx);
        window
            .update(cx, |_, window, cx| {
                window.render_frame(cx);
                let pill = window.find("terminal-return-live");
                let surface = window.find("terminal-surface").bounds();
                assert!(pill.bounds().left() >= surface.left());
                assert!(pill.bounds().right() <= surface.right());
                assert!(pill.bounds().bottom() < surface.bottom());
                assert!(pill.visible());
                assert_eq!(
                    (view.read(cx).font_size, view.read(cx).line_height_mul),
                    font
                );
            })
            .unwrap();
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
        view.scroll_handle
            .set_offset(point(px(0.), -(view.line_height * (history - 24) as f32)));
    });
    graphics_assert_production_paint(window, &view, cx);
    let before = reading_painted_rows(&view, cx);
    assert_ne!(
        before, live,
        "the scrollbar must leave live before output can test capture stability"
    );
    window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("terminal-return-live").is_some());
        })
        .unwrap();
    graphics_send_fixture(
        &view,
        cx,
        31,
        b"\x1b[H\x1b[2J\x1b[24;1H\r\nSCROLLBAR-LIVE\r\n",
    );
    graphics_assert_production_paint(window, &view, cx);
    assert_eq!(reading_painted_rows(&view, cx), before);
    let pasted = "SEND 32 0 1b5b306d 0";
    cx.update(|cx| cx.write_to_clipboard(ClipboardItem::new_string(pasted.into())));
    window
        .update(cx, |_, window, cx| {
            window.dispatch_action(Box::new(PasteText), cx);
        })
        .unwrap();
    cx.run_until_parked();
    view.read_with(cx, |view, _| view.terminal.write(b"\n".to_vec()));
    // The pasted request has its own parser-order reply barrier. Sending the
    // next request before its DONE marker races that request against the reply.
    graphics_wait_fixture(&view, cx, 32, pasted.len() + 1);
    graphics_send_fixture(&view, cx, 33, b"\x1b[0m");
    graphics_assert_production_paint(window, &view, cx);
    assert_ne!(reading_painted_rows(&view, cx), before);
    window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("terminal-return-live").is_none());
        })
        .unwrap();
    daemon.panes().kill(pane).unwrap();
    graphics_recovery_destroy(view, window, cx);
}

// RenderImage stores native BGRA; compare the independent source RGBA identity.
fn reading_rgba_pixel(image: &crate::terminal::images::RenderPlacement) -> [u8; 4] {
    let bgra = image.data.as_ref().unwrap().as_bytes(0).unwrap();
    [bgra[2], bgra[1], bgra[0], bgra[3]]
}

#[gpui_kit::test]
fn reading_placeholders_paint_from_captured_rows_after_live_cells_are_erased(
    cx: &mut TestAppContext,
) {
    use std::sync::Arc;
    use std::sync::atomic::Ordering;
    // A 1x1 source aspect-fits, centered, inside this two-column, one-row
    // placeholder. Its pixel crop need not start at the cell's top edge.
    let square_bounds = |geom: crate::terminal::element::CellGeom, row: f32| {
        let side = (geom.cell_width * 2.).min(geom.line_height);
        Bounds::new(
            point(
                geom.origin.x + geom.cell_width * 4. + (geom.cell_width * 2. - side) / 2.,
                geom.origin.y + geom.line_height * row + (geom.line_height - side) / 2.,
            ),
            gpui_kit::size(side, side),
        )
    };
    let (daemon, pane, view, window) = graphics_layout_harness(cx);
    graphics_send_fixture(&view, cx, 40, &b"PLACEHOLDER\r\n".repeat(72));
    graphics_send_fixture(&view, cx, 41, "\x1b[8;5H\x1b_Ga=T,i=8,p=2,U=1,c=2,r=1,f=32,s=1,v=1,q=2;AAD//w==\x1b\\\x1b[38;5;8;58;5;2m\u{10eeee}\u{305}\u{305}\u{10eeee}\x1b[0m".as_bytes());
    graphics_layout_wait_image(&view, cx, 8);
    view.update(cx, |view, cx| {
        view.smooth_scroll(1., cx);
    });
    graphics_assert_production_paint(window, &view, cx);
    let before = reading_painted_rows(&view, cx);
    let (painted, bounds, frozen) = view.read_with(cx, |view, _| {
        let image = view
            .grid_snap
            .as_ref()
            .unwrap()
            .images
            .iter()
            .find(|image| image.placement.image_id == 8)
            .expect("captured placeholder must enter the production display list");
        assert_eq!(reading_rgba_pixel(image), [0, 0, 255, 255]);
        assert!(image.painted.load(Ordering::Acquire));
        let geom = view.cell_geom.unwrap();
        let offset = view.terminal.term.lock().display_offset() as i32;
        let bounds = image.paint_bounds(
            geom.origin,
            geom.cell_width,
            geom.line_height,
            0,
            offset,
            1.,
        );
        let expected = square_bounds(geom, 8.);
        assert_eq!(
            bounds,
            (expected, expected),
            "full row uses the centered image crop, not the cell box"
        );
        (
            image.painted.clone(),
            bounds,
            Arc::downgrade(image.data.as_ref().unwrap()),
        )
    });
    painted.store(false, Ordering::Release);
    let mut advance = b"\x1b[8;1H\x1b[2KERASED\x1b[24;1H".to_vec();
    advance.extend_from_slice(&b"\r\nADVANCED".repeat(12));
    // Replace the same image ID with red pixels, retiring the old resource.
    // Frozen rows must retain the original decoded identity and atlas lease.
    advance.extend_from_slice(b"\x1b_Ga=t,i=8,f=32,s=1,v=1,q=2;/wAA/w==\x1b\\");
    graphics_send_fixture(&view, cx, 42, &advance);
    graphics_assert_production_paint(window, &view, cx);
    assert_eq!(reading_painted_rows(&view, cx), before);
    view.read_with(cx, |view, _| {
        let image = view
            .grid_snap
            .as_ref()
            .unwrap()
            .images
            .iter()
            .find(|image| image.placement.image_id == 8)
            .expect("erasing live cells must not erase captured placeholder runs");
        assert_eq!(reading_rgba_pixel(image), [0, 0, 255, 255]);
        assert!(Arc::ptr_eq(
            image.data.as_ref().unwrap(),
            &frozen.upgrade().unwrap()
        ));
        assert!(
            painted.load(Ordering::Acquire),
            "a new production frame must paint the captured image"
        );
        let geom = view.cell_geom.unwrap();
        let offset = view.terminal.term.lock().display_offset() as i32;
        assert_eq!(
            image.paint_bounds(
                geom.origin,
                geom.cell_width,
                geom.line_height,
                0,
                offset,
                1.
            ),
            bounds
        );
    });
    // The captured eighth screen row is now above live history. Move it into
    // the preceding half-row sliver, whose live cells no longer contain an image.
    view.update(cx, |view, cx| {
        view.smooth_scroll(-8.5, cx);
    });
    painted.store(false, Ordering::Release);
    graphics_assert_production_paint(window, &view, cx);
    let sliver_bounds = view.read_with(cx, |view, _| {
        let image = view
            .grid_snap
            .as_ref()
            .unwrap()
            .images
            .iter()
            .find(|image| image.placement.image_id == 8)
            .expect("captured placeholder must also enter the fractional sliver display list");
        assert_eq!(reading_rgba_pixel(image), [0, 0, 255, 255]);
        assert!(Arc::ptr_eq(
            image.data.as_ref().unwrap(),
            &frozen.upgrade().unwrap()
        ));
        let geom = view.cell_geom.unwrap();
        let offset = view.terminal.term.lock().display_offset() as i32;
        let bounds = image.paint_bounds(
            geom.origin,
            geom.cell_width,
            geom.line_height,
            0,
            offset,
            1.,
        );
        let expected = square_bounds(geom, -1.);
        assert_eq!(
            bounds,
            (expected, expected),
            "sliver keeps the same centered crop at viewport row -1"
        );
        assert!(
            painted.load(Ordering::Acquire),
            "the visible half-row must actually paint"
        );
        bounds
    });
    painted.store(false, Ordering::Release);
    graphics_send_fixture(&view, cx, 43, b"\x1b[H\x1b[2J\x1b[24;1H\r\nNEW-LIVE\r\n");
    graphics_assert_production_paint(window, &view, cx);
    view.read_with(cx, |view, _| {
        let image = view
            .grid_snap
            .as_ref()
            .unwrap()
            .images
            .iter()
            .find(|image| image.placement.image_id == 8)
            .expect("live redraw must preserve the frozen placeholder sliver");
        assert_eq!(reading_rgba_pixel(image), [0, 0, 255, 255]);
        assert!(Arc::ptr_eq(
            image.data.as_ref().unwrap(),
            &frozen.upgrade().unwrap()
        ));
        let geom = view.cell_geom.unwrap();
        let offset = view.terminal.term.lock().display_offset() as i32;
        assert_eq!(
            image.paint_bounds(
                geom.origin,
                geom.cell_width,
                geom.line_height,
                0,
                offset,
                1.
            ),
            sliver_bounds
        );
        assert!(painted.load(Ordering::Acquire));
    });
    view.update(cx, |view, _| view.jump_to_prompt());
    graphics_assert_production_paint(window, &view, cx);
    window
        .update(cx, |_, window, cx| {
            view.read(cx).terminal.images().release_retired(window, cx);
        })
        .unwrap();
    assert!(
        frozen.upgrade().is_none(),
        "returning live must release the retired frozen pixels"
    );
    daemon.panes().kill(pane).unwrap();
    graphics_recovery_destroy(view, window, cx);
}

#[gpui_kit::test]
fn reading_search_changes_query_without_annotating_live_replacements(cx: &mut TestAppContext) {
    let (daemon, pane, view, window) = graphics_layout_harness(cx);
    view.update(cx, |view, cx| {
        let mut config = cx.global::<Config>().clone();
        config.scrollback_limit = 64;
        view.terminal.apply_user_config(&config);
    });
    graphics_send_fixture(&view, cx, 70, &b"HISTORY\r\n".repeat(200));
    graphics_send_fixture(
        &view,
        cx,
        71,
        b"\x1b[8;1H\x1b[2Kold-needle old-other\x1b[10;1H\x1b[2Kold-needle",
    );
    view.update(cx, |view, cx| {
        view.smooth_scroll(1., cx);
    });
    graphics_assert_production_paint(window, &view, cx);
    graphics_send_fixture(
        &view,
        cx,
        72,
        b"\x1b[8;1H\x1b[2Knew-needle\x1b[10;1H\x1b[2Knew-needle\x1b[24;1H\r\nADVANCE\r\n",
    );
    for (query, expected) in [("old-needle", 2), ("new-needle", 0), ("old-other", 1)] {
        window
            .update(cx, |_, window, cx| {
                view.update(cx, |view, cx| {
                    view.open_search(window, cx);
                    let input = view.search.as_ref().unwrap().input.clone();
                    input.update(cx, |input, cx| input.set_value(query, window, cx));
                    view.recompute_matches(cx);
                });
            })
            .unwrap();
        graphics_assert_production_paint(window, &view, cx);
        view.read_with(cx, |view, _| {
            assert_eq!(
                view.search.as_ref().unwrap().matches.len(),
                expected,
                "query {query} describes semantic reading rows, not replacements"
            );
        });
        let painted = reading_painted_search_runs(window, &view, cx);
        assert_eq!(
            painted
                .iter()
                .map(|(text, _)| text.as_str())
                .collect::<String>(),
            query.repeat(expected),
            "production search fills cover the displayed characters"
        );
        if expected > 1 {
            assert_ne!(
                painted[0].1, painted[1].1,
                "the current result paints a distinct wash"
            );
            view.update(cx, |view, cx| {
                view.step_match(rio_vt::crosswords::pos::Direction::Right, cx)
            });
            graphics_assert_production_paint(window, &view, cx);
            assert_eq!(
                view.read_with(cx, |view, _| view.search.as_ref().unwrap().current_index),
                Some(0)
            );
            let stepped = reading_painted_search_runs(window, &view, cx);
            assert_eq!(
                stepped
                    .iter()
                    .map(|(text, _)| text.as_str())
                    .collect::<String>(),
                "old-needleold-needle"
            );
            assert_eq!(
                stepped.iter().map(|(_, color)| *color).collect::<Vec<_>>(),
                vec![painted[1].1, painted[0].1],
                "next result moves the current wash between retained rows"
            );
            graphics_send_fixture(&view, cx, 73, &b"\x1b[24;1H\r\nSATURATED-LIVE".repeat(8));
            graphics_assert_production_paint(window, &view, cx);
            let saturated = reading_painted_search_runs(window, &view, cx);
            assert_eq!(
                saturated
                    .iter()
                    .map(|(text, _)| text.as_str())
                    .collect::<String>(),
                "old-needleold-needle",
                "eviction cannot move search fills off the retained characters"
            );
        }
    }
    view.update(cx, |view, _| view.jump_to_prompt());
    window
        .update(cx, |_, window, cx| {
            view.update(cx, |view, cx| view.close_search(window, cx))
        })
        .unwrap();
    graphics_assert_production_paint(window, &view, cx);
    daemon.panes().kill(pane).unwrap();
    graphics_recovery_destroy(view, window, cx);
}

#[gpui_kit::test]
fn reading_mixed_images_keep_identity_crop_z_and_release_unneeded_history(cx: &mut TestAppContext) {
    use std::sync::Arc;
    use std::sync::atomic::Ordering;
    let (daemon, pane, view, window) = graphics_layout_harness(cx);
    graphics_send_fixture(
        &view,
        cx,
        80,
        b"\x1b[2;3H\x1b_Ga=T,i=9,p=1,c=2,r=1,C=1,f=32,s=1,v=1,q=2;AP8A/w==\x1b\\",
    );
    graphics_layout_wait_image(&view, cx, 9);
    let history = view.read_with(cx, |view, _| {
        Arc::downgrade(
            view.terminal
                .images()
                .snapshot()
                .iter()
                .find(|image| image.placement.image_id == 9)
                .unwrap()
                .data
                .as_ref()
                .unwrap(),
        )
    });
    graphics_send_fixture(&view, cx, 81, &b"HISTORY\r\n".repeat(96));
    graphics_send_fixture(&view, cx, 82, "\x1b[8;5Hdirect\x1b[8;5H\x1b_Ga=T,i=7,p=2,c=3,r=2,C=1,z=-3,f=32,s=2,v=1,x=1,y=0,w=1,h=1,q=2;AAD//wAA//8=\x1b\\\x1b[11;5H\x1b_Ga=T,i=8,p=3,U=1,c=2,r=1,z=4,f=32,s=1,v=1,q=2;AAD//w==\x1b\\\x1b[38;5;8;58;5;3m\u{10eeee}\u{305}\u{305}\u{10eeee}\x1b[0m".as_bytes());
    graphics_layout_wait_image(&view, cx, 7);
    graphics_layout_wait_image(&view, cx, 8);
    view.update(cx, |view, cx| {
        view.smooth_scroll(1., cx);
    });
    graphics_assert_production_paint(window, &view, cx);
    let geometry = |view: &TerminalView| {
        let geom = view.cell_geom.unwrap();
        let snap = view.grid_snap.as_ref().unwrap();
        snap.images
            .iter()
            .map(|image| {
                assert!(
                    image.painted.load(Ordering::Acquire),
                    "production paint must consume every visible slice"
                );
                (
                    image.placement.image_id,
                    image.placement.resource,
                    image.placement.key,
                    image.placement.z,
                    image.placement.x,
                    image.placement.y,
                    image.placement.w,
                    image.placement.h,
                    image.placement.clip_top,
                    image.placement.clip_bottom,
                    image.paint_bounds(
                        geom.origin,
                        geom.cell_width,
                        geom.line_height,
                        0,
                        snap.display_offset,
                        1.,
                    ),
                )
            })
            .collect::<Vec<_>>()
    };
    let before = view.read_with(cx, |view, _| geometry(view));
    assert_eq!(
        before.iter().map(|image| image.0).collect::<Vec<_>>(),
        vec![7, 7, 8]
    );
    assert_eq!(before[0].3, -3);
    assert_eq!(
        (before[0].4, before[0].5, before[0].6, before[0].7),
        (1, 0, 1, 1)
    );
    assert_eq!(before[2].3, 4);
    let frozen: Vec<_> = view.read_with(cx, |view, _| {
        view.grid_snap
            .as_ref()
            .unwrap()
            .images
            .iter()
            .map(|image| {
                assert_eq!(reading_rgba_pixel(image), [0, 0, 255, 255]);
                Arc::downgrade(image.data.as_ref().unwrap())
            })
            .collect()
    });
    let text = reading_painted_rows(&view, cx);
    // Delete the actual owners, including the offscreen historical image and
    // virtual prototype. d=A only removes visible non-virtual placements.
    graphics_send_fixture(&view, cx, 83, b"\x1b_Ga=d,d=I,i=9\x1b\\\x1b_Ga=d,d=I,i=7\x1b\\\x1b_Ga=d,d=I,i=8\x1b\\\x1b[8;1H\x1b[2KREPLACED\x1b[11;1H\x1b[2KERASED\x1b[24;1H\r\nADVANCE\r\n\x1b[4;9H\x1b_Ga=T,i=7,p=2,c=2,r=1,C=1,z=9,f=32,s=1,v=1,q=2;/wAA/w==\x1b\\");
    graphics_layout_wait_image(&view, cx, 7);
    graphics_assert_production_paint(window, &view, cx);
    assert_eq!(reading_painted_rows(&view, cx), text);
    assert_eq!(view.read_with(cx, |view, _| geometry(view)), before);
    window
        .update(cx, |_, window, cx| {
            view.read(cx).terminal.images().release_retired(window, cx)
        })
        .unwrap();
    assert!(
        history.upgrade().is_none(),
        "reading cannot pin unrelated historical placements"
    );
    assert!(frozen.iter().all(|image| image.upgrade().is_some()));
    view.update(cx, |view, _| view.jump_to_prompt());
    graphics_assert_production_paint(window, &view, cx);
    view.read_with(cx, |view, _| {
        let images = &view.grid_snap.as_ref().unwrap().images;
        assert_eq!(images.len(), 1);
        assert_eq!(images[0].placement.z, 9);
        assert_eq!(reading_rgba_pixel(&images[0]), [255, 0, 0, 255]);
    });
    window
        .update(cx, |_, window, cx| {
            view.read(cx).terminal.images().release_retired(window, cx)
        })
        .unwrap();
    assert!(
        frozen.iter().all(|image| image.upgrade().is_none()),
        "return live retires every frozen resource version"
    );
    daemon.panes().kill(pane).unwrap();
    graphics_recovery_destroy(view, window, cx);
}

#[gpui_kit::test]
fn reading_selection_keeps_absolute_rows_across_viewport_scroll(cx: &mut TestAppContext) {
    let (daemon, pane, view, window) = graphics_layout_harness(cx);
    let mut scene = b"\x1b[2J\x1b[H".to_vec();
    for row in 0..72 {
        scene.extend_from_slice(format!("READ-{row:03}\r\n").as_bytes());
    }
    graphics_send_fixture(&view, cx, 90, &scene);
    view.update(cx, |view, cx| {
        view.smooth_scroll(1., cx);
    });
    graphics_assert_production_paint(window, &view, cx);
    view.update(cx, |view, cx| {
        view.on_select_start(7, 0, false, 1, false, cx);
        view.smooth_scroll(24., cx);
    });
    graphics_assert_production_paint(window, &view, cx);
    view.update(cx, |view, cx| view.on_select_update(0, 0, true, cx));
    graphics_assert_production_paint(window, &view, cx);
    view.update(cx, |view, cx| view.copy_selection(cx));
    let expected = (24..=48)
        .map(|row| format!("READ-{row:03}"))
        .collect::<Vec<_>>()
        .join("\n");
    assert_eq!(
        cx.update(|cx| cx.read_from_clipboard().and_then(|item| item.text())),
        Some(expected)
    );
    graphics_send_fixture(
        &view,
        cx,
        91,
        b"\x1b[H\x1b[2KREPLACED\x1b[24;1H\r\nADVANCE\r\n",
    );
    graphics_assert_production_paint(window, &view, cx);
    assert!(
        view.read_with(cx, |view, _| view.grid_buf.iter().any(|cell| cell.selected)),
        "selection survives unrelated output paint"
    );
    daemon.panes().kill(pane).unwrap();
    graphics_recovery_destroy(view, window, cx);
}

#[gpui_kit::test]
fn reading_retained_allocations_follow_needed_rows_not_visited_history(cx: &mut TestAppContext) {
    let (daemon, pane, view, window) = graphics_layout_harness(cx);
    graphics_send_fixture(&view, cx, 100, &b"BOUNDED-HISTORY\r\n".repeat(600));
    for _ in 0..20 {
        view.update(cx, |view, cx| {
            view.smooth_scroll(24., cx);
        });
        graphics_assert_production_paint(window, &view, cx);
        let observation = view.read_with(cx, |view, _| presentation_allocation_observation(view));
        assert!(
            observation["reading"]["rows"].as_u64().unwrap() <= 49,
            "one mutable screen plus viewport/sliver: {observation}"
        );
        assert!(
            observation["reading"]["cells"].as_u64().unwrap() <= 49 * 80,
            "visiting history cannot accumulate semantic rows: {observation}"
        );
    }
    view.update(cx, |view, _| view.jump_to_prompt());
    graphics_assert_production_paint(window, &view, cx);
    assert!(
        view.read_with(cx, |view, _| presentation_allocation_observation(view))["reading"]
            .is_null()
    );
    daemon.panes().kill(pane).unwrap();
    graphics_recovery_destroy(view, window, cx);
}

#[gpui_kit::test]
fn reading_and_synchronized_output_keep_separate_presentation_lifetimes(cx: &mut TestAppContext) {
    crate::core::config::pin_test_config_dir();
    cx.executor().allow_parking();
    cx.update(|cx| {
        gpui_kit::component::init(cx);
        cx.set_global(Config::default());
    });
    // Observe the daemon/RemoteTerminal seam directly: a real PTY round-trip
    // barrier can outlast the 150 ms fail-safe (and ConPTY may emit ESU).
    // The other reading cases retain the full real-PTY path.
    let (client, mut sender) = test_stream_pair();
    let (view, window) = graphics_build_rooted_consumer_at(
        cx,
        252,
        client,
        Vec::new(),
        TermSize::new(80, 24),
        PtySource::Raw,
    );
    DaemonMsg::Output(b"READING-BASE\r\n".repeat(72))
        .encode(&mut sender)
        .unwrap();
    graphics_recovery_wait(cx, "reading baseline was not parsed", |cx| {
        view.read_with(cx, |view, _| {
            view.scrollback_text().contains("READING-BASE")
        })
    });
    view.update(cx, |view, cx| {
        view.smooth_scroll(1., cx);
    });
    graphics_assert_production_paint(window, &view, cx);
    let frozen = reading_painted_rows(&view, cx);
    DaemonMsg::Output(b"\x1b[?2026h\x1b[8;1H\x1b[2KHIDDEN-SYNC-CONTENT".to_vec())
        .encode(&mut sender)
        .unwrap();
    // Flush this Output before waiting, then observe its parsed cells without
    // advancing GPUI/PTY timers past the production atomic deadline.
    DaemonMsg::Cwd(std::env::temp_dir())
        .encode(&mut sender)
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        if view.read_with(cx, |view, _| {
            let mut term = view.terminal.term.lock();
            if term.grid[Line(7)][Column(0)].c() != 'H' {
                return false;
            }
            assert!(
                term.synchronized_output(),
                "fixture must observe the in-progress atomic hold"
            );
            // This case tests lifetime separation; the quiet-stream tests
            // independently exercise the unmodified 150 ms timeout.
            term.sync_update_started =
                Some(rio_vt::time::Instant::now() + std::time::Duration::from_secs(30));
            true
        }) {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "hidden output was not parsed"
        );
        std::thread::yield_now();
    }
    view.update(cx, |_, cx| cx.notify());
    graphics_assert_production_paint(window, &view, cx);
    assert_eq!(reading_painted_rows(&view, cx), frozen);
    assert!(view.read_with(cx, |view, _| view.grid_snap.as_ref().unwrap().held));
    DaemonMsg::Output(b"\x1b[?2026l".to_vec())
        .encode(&mut sender)
        .unwrap();
    DaemonMsg::Cwd(std::env::temp_dir())
        .encode(&mut sender)
        .unwrap();
    graphics_recovery_wait(cx, "ESU did not release the atomic hold", |cx| {
        view.read_with(cx, |view, _| {
            !view.terminal.term.lock().synchronized_output()
        })
    });
    graphics_assert_production_paint(window, &view, cx);
    assert_eq!(
        reading_painted_rows(&view, cx),
        frozen,
        "ESU releases atomic acquisition, not the reading session"
    );
    assert!(!view.read_with(cx, |view, _| view.grid_snap.as_ref().unwrap().held));
    view.update(cx, |view, _| view.jump_to_prompt());
    graphics_assert_production_paint(window, &view, cx);
    assert!(
        reading_painted_rows(&view, cx)
            .iter()
            .any(|row| row.contains("HIDDEN-SYNC-CONTENT"))
    );
    graphics_recovery_destroy(view, window, cx);
}

#[gpui_kit::test]
fn reading_adopts_a_live_drag_before_autoscrolling_into_frozen_rows(cx: &mut TestAppContext) {
    let (daemon, pane, view, window) = graphics_layout_harness(cx);
    let mut scene = b"\x1b[2J\x1b[H".to_vec();
    for row in 0..72 {
        scene.extend_from_slice(format!("READ-{row:03}\r\n").as_bytes());
    }
    graphics_send_fixture(&view, cx, 120, &scene);
    graphics_assert_production_paint(window, &view, cx);
    view.update(cx, |view, cx| {
        view.on_select_start(7, 0, false, 1, false, cx);
        view.select_autoscroll(1., 0, true, cx);
        view.on_select_end(cx);
    });
    graphics_assert_production_paint(window, &view, cx);
    view.update(cx, |view, cx| view.copy_selection(cx));
    assert_eq!(
        cx.update(|cx| cx.read_from_clipboard().and_then(|item| item.text()))
            .as_deref(),
        Some("READ-048\nREAD-049")
    );
    assert!(view.read_with(cx, |view, _| view.grid_snap.as_ref().unwrap().any_selected));
    daemon.panes().kill(pane).unwrap();
    graphics_recovery_destroy(view, window, cx);
}

#[gpui_kit::test]
fn reading_wrapped_link_uses_retained_prefix_and_hover_survives_live_output(
    cx: &mut TestAppContext,
) {
    let (daemon, pane, view, window) = graphics_layout_harness(cx);
    graphics_send_fixture(&view, cx, 130, &b"HISTORY\r\n".repeat(72));
    let url = format!("https://old.example/{}", "a".repeat(150));
    // The sender's completion barrier writes its marker on screen row two.
    // Keep the wrapped prefix below that row rather than overwriting https://.
    // Windows raw console output starts without wrap-at-EOL. Request DECAWM
    // through the real PTY before relying on physical continuation rows.
    graphics_send_fixture(&view, cx, 131, format!("\x1b[?7h\x1b[3;1H{url}").as_bytes());
    graphics_assert_production_paint(window, &view, cx);
    let live_wrapped = reading_painted_rows(&view, cx);
    assert_eq!(live_wrapped[2], &url[..80]);
    assert_eq!(
        live_wrapped[3],
        "a".repeat(80),
        "the real PTY must wrap the full middle row before capture"
    );
    assert_eq!(
        live_wrapped[4],
        "a".repeat(10),
        "the final continuation must exist before entering reading mode"
    );
    view.update(cx, |view, cx| {
        view.smooth_scroll(1., cx);
    });
    graphics_assert_production_paint(window, &view, cx);
    let wrapped = reading_painted_rows(&view, cx);
    assert_eq!(wrapped[3], &url[..80]);
    assert_eq!(
        wrapped[4],
        "a".repeat(80),
        "the full middle continuation must paint before live output"
    );
    assert_eq!(
        wrapped[5],
        "a".repeat(10),
        "the final continuation must exist before scrolling its prefix offscreen"
    );
    graphics_send_fixture(&view, cx, 132, &b"\x1b[24;1H\r\nADVANCE".repeat(20));
    view.update(cx, |view, cx| {
        view.smooth_scroll(-5., cx);
    });
    graphics_assert_production_paint(window, &view, cx);
    assert_eq!(
        reading_painted_rows(&view, cx)[0],
        "a".repeat(10),
        "the viewport starts on the frozen continuation, with its prefix retained offscreen"
    );
    view.update(cx, |view, cx| assert!(view.hover_link_at(1, 0, true, cx)));
    graphics_send_fixture(&view, cx, 133, b"\x1b[24;1H\r\nMORE-OUTPUT");
    graphics_assert_production_paint(window, &view, cx);
    assert!(
        view.read_with(cx, |view, _| view.links.hovered().is_some()),
        "compensating Rio offset cannot cancel a frozen hover or its pending Host probes"
    );
    window
        .update(cx, |_, window, cx| {
            view.update(cx, |view, cx| assert!(view.open_link_at(1, 0, window, cx)));
        })
        .unwrap();
    assert_eq!(
        cx.opened_url().as_deref(),
        Some(url.as_str()),
        "a viewport-edge continuation keeps its offscreen frozen prefix"
    );
    daemon.panes().kill(pane).unwrap();
    graphics_recovery_destroy(view, window, cx);
}

#[gpui_kit::test]
fn reading_fractional_sliver_keeps_selection_geometry_and_copy(cx: &mut TestAppContext) {
    let (daemon, pane, view, window) = graphics_layout_harness(cx);
    graphics_send_fixture(&view, cx, 140, &b"HISTORY\r\n".repeat(72));
    graphics_send_fixture(&view, cx, 141, b"\x1b[5;1H\x1b[2Kselected-sliver");
    view.update(cx, |view, cx| {
        view.smooth_scroll(1., cx);
    });
    graphics_assert_production_paint(window, &view, cx);
    view.update(cx, |view, cx| {
        view.on_select_start(0, 5, true, 1, false, cx);
        view.on_select_update(14, 5, false, cx);
        view.on_select_end(cx);
    });
    graphics_assert_production_paint(window, &view, cx);
    let selection_fill = window
        .update(cx, |_, window, cx| {
            let geom = view.read(cx).cell_geom.expect("production frame");
            let expected = Bounds::new(
                point(geom.origin.x, geom.origin.y + geom.line_height * 5.),
                gpui_kit::size(geom.cell_width * 15., geom.line_height),
            )
            .map(|pixel| gpui_kit::ScaledPixels(pixel.as_f32() * window.scale_factor()));
            window
                .painted_quads()
                .iter()
                .find(|quad| quad.bounds == expected)
                .expect("the selected retained row must paint before scrolling")
                .background
                .clone()
        })
        .unwrap();
    graphics_send_fixture(&view, cx, 142, &b"\x1b[24;1H\r\nADVANCE".repeat(20));
    view.update(cx, |view, cx| {
        view.smooth_scroll(-5.5, cx);
    });
    graphics_assert_production_paint(window, &view, cx);
    window
        .update(cx, |_, window, cx| {
            let geom = view.read(cx).cell_geom.expect("production frame");
            let scale = window.scale_factor();
            let expected = Bounds::new(
                point(geom.origin.x, geom.origin.y - geom.line_height),
                gpui_kit::size(geom.cell_width * 15., geom.line_height),
            )
            .map(|pixel| gpui_kit::ScaledPixels(pixel.as_f32() * scale));
            assert!(
                window
                    .painted_quads()
                    .iter()
                    .any(|quad| quad.bounds == expected && quad.background == selection_fill),
                "the selected fractional row must paint its full retained text range above row zero"
            );
        })
        .unwrap();
    view.update(cx, |view, cx| view.copy_selection(cx));
    assert_eq!(
        cx.update(|cx| cx.read_from_clipboard().and_then(|item| item.text()))
            .as_deref(),
        Some("selected-sliver")
    );
    daemon.panes().kill(pane).unwrap();
    graphics_recovery_destroy(view, window, cx);
}

#[gpui_kit::test]
fn reading_resize_repeated_narrow_wide_keeps_the_logical_character_offset(cx: &mut TestAppContext) {
    let (daemon, pane, view, window) = graphics_layout_harness(cx);
    let mut scene = b"\x1b[?7h\x1b[2J\x1b[H".to_vec();
    for row in 0..20 {
        scene.extend_from_slice(format!("BEFORE-{row:03}\r\n").as_bytes());
    }
    let response = format!(
        "{}位置e\u{301}{}",
        "A".repeat(160),
        "界e\u{301}".repeat(200)
    );
    scene.extend_from_slice(response.as_bytes());
    scene.extend_from_slice(b"\r\n");
    for row in 0..100 {
        scene.extend_from_slice(format!("AFTER-{row:03}\r\n").as_bytes());
    }
    graphics_send_fixture(&view, cx, 120, &scene);
    reading_scroll_to_live_row(&view, cx, "位置", 0);
    graphics_assert_production_paint(window, &view, cx);
    assert!(reading_painted_rows(&view, cx)[0].starts_with("位置e\u{301}"));
    // No PTY output follows any resize. Ordered Size/graphics alone must
    // publish the relocated frame, including marks and wide-cell mapping.
    for cols in [40, 160, 80, 40, 80] {
        graphics_assert_production_paint_at(window, &view, cx, TermSize::new(cols, 24));
        assert!(
            reading_painted_rows(&view, cx)[0].starts_with("位置e\u{301}"),
            "{cols} columns must retain character 160 of the same hard line"
        );
    }
    daemon.panes().kill(pane).unwrap();
    graphics_recovery_destroy(view, window, cx);
}

// Controlled ordered pane stream, real RemoteTerminal and rooted production
// paint. Unlike the tests above this seam is not a real PTY/daemon scene: it
// deliberately separates Size from its graphics completion to expose races.
fn reading_ordered_resize_harness(
    cx: &mut TestAppContext,
) -> (
    Entity<TerminalView>,
    gpui_kit::AnyWindowHandle,
    crate::daemon::transport::Stream,
) {
    crate::core::config::pin_test_config_dir();
    cx.executor().allow_parking();
    cx.update(|cx| {
        gpui_kit::component::init(cx);
        cx.set_global(Config::default());
    });
    let (client, _initial_sender) = test_stream_pair();
    let (view, window) = graphics_build_rooted_consumer_at(
        cx,
        253,
        client,
        Vec::new(),
        TermSize::new(80, 24),
        PtySource::Raw,
    );
    let (client, sender) = test_stream_pair();
    view.update(cx, |view, cx| {
        view.adopt_relink(
            client,
            Vec::new(),
            &crate::terminal::PaneRoute::Remote {
                header: Box::new(crate::daemon::router::RouteHeader::wsl(
                    "reading-resize-fixture",
                )),
                resize_echo: true,
            },
            TermSize::new(80, 24),
            10,
            20,
            cx,
        )
        .unwrap();
    });
    (view, window, sender)
}

fn reading_request_window_size(
    window: gpui_kit::AnyWindowHandle,
    view: &Entity<TerminalView>,
    cx: &mut TestAppContext,
    size: TermSize,
) {
    let target = graphics_window_geometry(window, view, cx, size).0;
    cx.simulate_window_resize(window, target);
    window
        .update(cx, |_, window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
        })
        .unwrap();
}

fn reading_ordered_size(sender: &mut crate::daemon::transport::Stream, cols: u16, rows: u16) {
    DaemonMsg::Size(crate::daemon::protocol::WinSize {
        cols,
        rows,
        cell_w: 10,
        cell_h: 20,
    })
    .encode(sender)
    .unwrap();
}

fn reading_ordered_screen(sender: &mut crate::daemon::transport::Stream, origin: i64, epoch: u64) {
    use lyrel_core::core::kitty_graphics::GraphicsUpdate;

    send_graphics(
        sender,
        GraphicsUpdate::Screen {
            screen: 0,
            history_origin: origin,
            epoch,
        },
    );
}

#[gpui_kit::test]
fn reading_resize_orders_old_width_scroll_and_latest_geometry_without_more_output(
    cx: &mut TestAppContext,
) {
    let (view, window, mut sender) = reading_ordered_resize_harness(cx);
    let mut scene = b"\x1b[?7h\x1b[2J\x1b[H".to_vec();
    for row in 0..120 {
        scene.extend_from_slice(format!("LINE-{row:03}\r\n").as_bytes());
    }
    DaemonMsg::Output(scene).encode(&mut sender).unwrap();
    reading_ordered_screen(&mut sender, 97, 0);
    graphics_recovery_wait(cx, "ordered reading baseline did not paint", |cx| {
        view.read_with(cx, |view, _| {
            view.grid_snap.is_some() && sync_presented_row(view, 0).starts_with("LINE-097")
        })
    });
    view.update(cx, |view, cx| {
        view.smooth_scroll(47., cx);
    });
    graphics_assert_production_paint(window, &view, cx);
    let before = reading_painted_rows(&view, cx);
    assert_eq!(before[0], "LINE-050");
    // Equal-area dimensions must not reinterpret the old 80-column picture.
    reading_request_window_size(window, &view, cx, TermSize::new(96, 20));
    assert_eq!(reading_painted_rows(&view, cx), before);
    view.update(cx, |view, cx| {
        view.smooth_scroll(3., cx);
    });
    // Supersede before the first echo. Neither that Size nor its Screen can
    // publish an intermediate-width frame over the latest requested intent.
    reading_request_window_size(window, &view, cx, TermSize::new(40, 24));
    reading_ordered_size(&mut sender, 96, 20);
    reading_ordered_screen(&mut sender, 101, 1);
    reading_ordered_size(&mut sender, 40, 24);
    graphics_recovery_wait(cx, "latest Size did not reach the mirror", |cx| {
        view.read_with(cx, |view, _| view.terminal.term.lock().columns() == 40)
    });
    window
        .update(cx, |_, window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
        })
        .unwrap();
    assert_eq!(
        reading_painted_rows(&view, cx),
        before,
        "Size alone cannot pair new text with old graphics geometry"
    );
    reading_ordered_screen(&mut sender, 97, 2);
    graphics_assert_production_paint_at(window, &view, cx, TermSize::new(40, 24));
    assert_eq!(
        reading_painted_rows(&view, cx)[0],
        "LINE-047",
        "three rows of user travel survive both pending resizes"
    );
    graphics_recovery_destroy(view, window, cx);
}

#[gpui_kit::test]
fn reading_resize_relocates_direct_and_placeholder_images_with_their_text(cx: &mut TestAppContext) {
    use std::sync::atomic::Ordering;
    let (daemon, pane, view, window) = graphics_layout_harness(cx);
    let mut scene = b"\x1b[?7h\x1b[2J\x1b[H".to_vec();
    for row in 0..20 {
        scene.extend_from_slice(format!("BEFORE-{row:03}\r\n").as_bytes());
    }
    scene.extend_from_slice("A".repeat(640).as_bytes());
    scene.extend_from_slice(b"\r\nDIRECT-MARK\r\x1b[6G\x1b_Ga=T,i=71,p=1,c=2,r=1,C=1,f=32,s=1,v=1,q=2;AAD//w==\x1b\\\r\nPLACE-MARK  \x1b_Ga=T,i=72,p=2,U=1,c=2,r=1,f=32,s=1,v=1,q=2;AP8A/w==\x1b\\");
    scene.extend_from_slice(
        "\x1b[38;5;72;58;5;2m\u{10eeee}\u{305}\u{305}\u{10eeee}\x1b[0m\r\n".as_bytes(),
    );
    graphics_send_fixture(&view, cx, 121, &scene);
    graphics_layout_wait_image(&view, cx, 71);
    graphics_layout_wait_image(&view, cx, 72);
    let mut tail = Vec::new();
    for row in 0..100 {
        tail.extend_from_slice(format!("AFTER-{row:03}\r\n").as_bytes());
    }
    graphics_send_fixture(&view, cx, 124, &tail);
    view.update(cx, |view, cx| {
        let history = view.terminal.term.lock().grid.history_size();
        view.smooth_scroll((history - 27) as f32, cx);
    });
    for cols in [80, 40, 160, 80, 40] {
        graphics_assert_production_paint_at(window, &view, cx, TermSize::new(cols, 24));
        let rows = reading_painted_rows(&view, cx);
        let direct_row = rows
            .iter()
            .position(|row| row.starts_with("DIRECT-MARK"))
            .expect("direct text is visible");
        let placeholder_row = rows
            .iter()
            .position(|row| row.starts_with("PLACE-MARK"))
            .expect("placeholder text is visible");
        view.read_with(cx, |view, _| {
            let geom = view.cell_geom.unwrap();
            let snap = view.grid_snap.as_ref().unwrap();
            let direct = snap
                .images
                .iter()
                .find(|image| image.placement.image_id == 71)
                .expect("direct placement is painted");
            let placeholder = snap
                .images
                .iter()
                .find(|image| image.placement.image_id == 72)
                .expect("placeholder is painted");
            assert!(direct.painted.load(Ordering::Acquire));
            assert!(placeholder.painted.load(Ordering::Acquire));
            assert_eq!(reading_rgba_pixel(direct), [0, 0, 255, 255]);
            assert_eq!(reading_rgba_pixel(placeholder), [0, 255, 0, 255]);
            let (direct_crop, _) = direct.paint_bounds(
                geom.origin,
                geom.cell_width,
                geom.line_height,
                0,
                snap.display_offset,
                1.,
            );
            assert_eq!(
                direct_crop.origin,
                geom.origin + point(geom.cell_width * 5., geom.line_height * direct_row as f32),
                "{cols} columns: direct image follows its displayed text row {direct_row}"
            );
            let side = (geom.cell_width * 2.).min(geom.line_height);
            let (placeholder_crop, _) = placeholder.paint_bounds(
                geom.origin,
                geom.cell_width,
                geom.line_height,
                0,
                snap.display_offset,
                1.,
            );
            assert_eq!(
                placeholder_crop,
                Bounds::new(
                    geom.origin
                        + point(
                            geom.cell_width * 12. + (geom.cell_width * 2. - side) / 2.,
                            geom.line_height * placeholder_row as f32
                                + (geom.line_height - side) / 2.
                        ),
                    gpui_kit::size(side, side),
                ),
                "{cols} columns: both image kinds remain on their own displayed text rows"
            );
        });
    }
    daemon.panes().kill(pane).unwrap();
    graphics_recovery_destroy(view, window, cx);
}

#[gpui_kit::test]
fn reading_resize_chooses_the_nearest_repeated_256_character_prefix(cx: &mut TestAppContext) {
    let (daemon, pane, view, window) = graphics_layout_harness(cx);
    let mut scene = b"\x1b[?7h\x1b[2J\x1b[H".to_vec();
    for row in 0..20 {
        scene.extend_from_slice(format!("BEFORE-{row:03}\r\n").as_bytes());
    }
    let mut middle = String::new();
    for label in ["FIRST", "MIDDLE", "LAST"] {
        let response = format!(
            "{}{}{}",
            "D".repeat(320),
            label,
            "X".repeat(80 - label.len())
        );
        if label == "MIDDLE" {
            middle = response.chars().skip(160).collect();
        }
        scene.extend_from_slice(response.as_bytes());
        scene.extend_from_slice(b"\r\n");
    }
    for row in 0..100 {
        scene.extend_from_slice(format!("AFTER-{row:03}\r\n").as_bytes());
    }
    graphics_send_fixture(&view, cx, 122, &scene);
    // MIDDLE's label is at character 320; character 160 is two rows above it
    // in the initial 80-column hard line.
    reading_scroll_to_live_row(&view, cx, "MIDDLE", -2);
    for cols in [80, 40, 80] {
        graphics_assert_production_paint_at(window, &view, cx, TermSize::new(cols, 24));
        let tail = view.read_with(cx, |view, _| {
            let geom = view.cell_geom.unwrap();
            let mut text = String::new();
            for row in 0..geom.rows {
                let wrapped = view.grid_buf[(row + 1) * geom.cols - 1].wrapped;
                let displayed = sync_presented_row(view, row);
                text.push_str(if wrapped {
                    &displayed
                } else {
                    displayed.trim_end()
                });
                if !wrapped {
                    break;
                }
            }
            text
        });
        assert_eq!(
            tail, middle,
            "{cols} columns must choose the nearby middle hard line, not the first prefix match"
        );
    }
    daemon.panes().kill(pane).unwrap();
    graphics_recovery_destroy(view, window, cx);
}

#[gpui_kit::test]
fn reading_resize_preserves_a_blank_rows_context_between_reflowed_lines(cx: &mut TestAppContext) {
    let (daemon, pane, view, window) = graphics_layout_harness(cx);
    let mut scene = b"\x1b[?7h\x1b[2J\x1b[H".to_vec();
    for row in 0..20 {
        scene.extend_from_slice(format!("BEFORE-{row:03}{}\r\n", "B".repeat(150)).as_bytes());
    }
    // Each preceding logical line is exactly 160 cells. The second blank of
    // the three-row run is old row 41, independent of the emulator algorithm.
    scene.extend_from_slice(b"\r\n\r\n\r\nBLANK-CONTEXT-TARGET\r\n\r\nOTHER-GAP\r\n");
    for row in 0..100 {
        scene.extend_from_slice(format!("AFTER-{row:03}\r\n").as_bytes());
    }
    graphics_send_fixture(&view, cx, 123, &scene);
    reading_scroll_to_live_row(&view, cx, "BLANK-CONTEXT-TARGET", -2);
    for cols in [80, 40, 160, 80] {
        graphics_assert_production_paint_at(window, &view, cx, TermSize::new(cols, 24));
        let rows = reading_painted_rows(&view, cx);
        assert_eq!(
            &rows[..3],
            &["", "", "BLANK-CONTEXT-TARGET"],
            "{cols} columns preserve the blank within its run, not another empty row"
        );
    }
    daemon.panes().kill(pane).unwrap();
    graphics_recovery_destroy(view, window, cx);
}

#[gpui_kit::test]
fn reading_resize_missing_evicted_anchor_uses_a_retained_geometry_estimate(
    cx: &mut TestAppContext,
) {
    let (view, window, mut sender) = reading_ordered_resize_harness(cx);
    let mut scene = b"\x1b[?7h\x1b[2J\x1b[H".to_vec();
    for row in 0..120 {
        scene.extend_from_slice(format!("LINE-{row:03}\r\n").as_bytes());
    }
    DaemonMsg::Output(scene).encode(&mut sender).unwrap();
    reading_ordered_screen(&mut sender, 97, 0);
    graphics_recovery_wait(cx, "fallback baseline did not paint", |cx| {
        view.read_with(cx, |view, _| {
            view.grid_snap.is_some() && sync_presented_row(view, 0).starts_with("LINE-097")
        })
    });
    view.update(cx, |view, cx| {
        view.smooth_scroll(47., cx);
    });
    graphics_assert_production_paint(window, &view, cx);
    assert_eq!(reading_painted_rows(&view, cx)[0], "LINE-050");
    reading_request_window_size(window, &view, cx, TermSize::new(40, 24));
    view.update(cx, |view, cx| {
        let mut config = cx.global::<Config>().clone();
        config.scrollback_limit = 16;
        view.terminal.apply_user_config(&config);
    });
    reading_ordered_size(&mut sender, 40, 24);
    reading_ordered_screen(&mut sender, 97, 1);
    graphics_assert_production_paint_at(window, &view, cx, TermSize::new(40, 24));
    // The original LINE-050 is gone. Retained history is LINE-081..096;
    // its midpoint estimate is LINE-089, not the old absolute row or live 097.
    assert_eq!(reading_painted_rows(&view, cx)[0], "LINE-089");
    view.update(cx, |view, cx| {
        view.smooth_scroll(10_000., cx);
    });
    reading_request_window_size(window, &view, cx, TermSize::new(80, 24));
    // Complete the new echo after the request made by prepaint above.
    reading_ordered_size(&mut sender, 80, 24);
    reading_ordered_screen(&mut sender, 97, 2);
    graphics_assert_production_paint_at(window, &view, cx, TermSize::new(80, 24));
    assert_eq!(
        reading_painted_rows(&view, cx)[0],
        "LINE-081",
        "oldest clamp never resurrects the expired anchor"
    );
    graphics_recovery_destroy(view, window, cx);
}

/// A reading entered after the resize request but before the ordered `Size`
/// still has to relocate. The request is not tied to a `ReadingView` that
/// happened to exist when the window moved.
#[gpui_kit::test]
fn reading_entered_after_a_resize_request_relocates_when_ordered_size_arrives(
    cx: &mut TestAppContext,
) {
    let (view, window, mut sender) = reading_ordered_resize_harness(cx);
    let mut scene = b"\x1b[?7h\x1b[2J\x1b[H".to_vec();
    for row in 0..120 {
        scene.extend_from_slice(format!("LINE-{row:03}\r\n").as_bytes());
    }
    scene.extend_from_slice(b"LONGSTART");
    scene.extend_from_slice("A".repeat(81).as_bytes());
    scene.extend_from_slice(b"MARKER");
    scene.extend_from_slice("B".repeat(68).as_bytes());
    scene.extend_from_slice(b"\r\n");
    for row in 0..40 {
        scene.extend_from_slice(format!("TAIL-{row:03}\r\n").as_bytes());
    }
    DaemonMsg::Output(scene).encode(&mut sender).unwrap();
    reading_ordered_screen(&mut sender, 97, 0);
    graphics_recovery_wait(cx, "ordered baseline did not paint", |cx| {
        view.read_with(cx, |view, _| view.scrollback_text().contains("LONGSTART"))
    });
    // No reading exists yet, so the request has to be remembered by the
    // viewport owner rather than by a `ReadingView`.
    reading_request_window_size(window, &view, cx, TermSize::new(40, 24));
    reading_scroll_to_live_row(&view, cx, "LONGSTART", 0);
    assert!(
        view.read_with(cx, |view, _| view.selection_scroll.reading.is_some()),
        "the fixture entered reading before the ordered Size"
    );
    reading_ordered_size(&mut sender, 40, 24);
    reading_ordered_screen(&mut sender, 97, 1);
    graphics_assert_production_paint_at(window, &view, cx, TermSize::new(40, 24));
    let rows = reading_painted_rows(&view, cx);
    assert_eq!(rows[0][..9], *"LONGSTART");
    assert!(
        !rows[1].contains("MARKER"),
        "40 columns wrap before the marker; an old 80-column frame would still show it on row 1: {rows:?}"
    );
    assert!(
        rows[2].contains("MARKER"),
        "the relocated hard line keeps the marker on its new wrapped row: {rows:?}"
    );
    graphics_recovery_destroy(view, window, cx);
}

/// HTS/TBC only change the tab map. They must not advance the resize
/// generation that pairs an ordered graphics `Screen` with its `Size`; doing
/// so makes `images_revision == revision` unsatisfiable and blocks relocation.
#[gpui_kit::test]
fn tab_map_changes_do_not_block_a_pending_reading_relocation(cx: &mut TestAppContext) {
    let (view, window, mut sender) = reading_ordered_resize_harness(cx);
    let mut scene = b"\x1b[?7h\x1b[2J\x1b[H".to_vec();
    for row in 0..120 {
        scene.extend_from_slice(format!("LINE-{row:03}\r\n").as_bytes());
    }
    scene.extend_from_slice(b"LONGSTART");
    scene.extend_from_slice("A".repeat(81).as_bytes());
    scene.extend_from_slice(b"MARKER");
    scene.extend_from_slice("B".repeat(68).as_bytes());
    scene.extend_from_slice(b"\r\n");
    for row in 0..40 {
        scene.extend_from_slice(format!("TAIL-{row:03}\r\n").as_bytes());
    }
    DaemonMsg::Output(scene).encode(&mut sender).unwrap();
    reading_ordered_screen(&mut sender, 97, 0);
    graphics_recovery_wait(cx, "ordered baseline did not paint", |cx| {
        view.read_with(cx, |view, _| view.scrollback_text().contains("LONGSTART"))
    });
    reading_scroll_to_live_row(&view, cx, "LONGSTART", 0);
    reading_request_window_size(window, &view, cx, TermSize::new(40, 24));
    reading_ordered_size(&mut sender, 40, 24);
    reading_ordered_screen(&mut sender, 97, 1);
    // A tab-map mutation after the ordered Screen is unrelated to the resize.
    DaemonMsg::Output(b"\x1b[5G\x1bH".to_vec())
        .encode(&mut sender)
        .unwrap();
    graphics_recovery_wait(cx, "HTS did not reach the tab map", |cx| {
        view.read_with(cx, |view, _| {
            view.terminal.presentation_facts().is_tab_stop(4)
        })
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while view.read_with(cx, |view, _| {
        view.selection_scroll
            .reading
            .as_ref()
            .is_some_and(|reading| reading.relocating())
    }) {
        assert!(
            std::time::Instant::now() < deadline,
            "a tab-map change advanced the resize generation and blocked relocation"
        );
        graphics_consumer_progress(cx);
    }
    graphics_assert_production_paint_at(window, &view, cx, TermSize::new(40, 24));
    let rows = reading_painted_rows(&view, cx);
    assert!(!rows[1].contains("MARKER"), "{rows:?}");
    assert!(rows[2].contains("MARKER"), "{rows:?}");
    graphics_recovery_destroy(view, window, cx);
}

/// Scrolling back to live while a relocation is pending is a return to live,
/// not a reading state with a zero-row pill.
#[gpui_kit::test]
fn returning_to_live_during_relocation_drops_the_reading_and_pill(cx: &mut TestAppContext) {
    use gpui_kit::test::TestWindowExt as _;
    let (view, window, mut sender) = reading_ordered_resize_harness(cx);
    let mut scene = b"\x1b[?7h\x1b[2J\x1b[H".to_vec();
    for row in 0..120 {
        scene.extend_from_slice(format!("LINE-{row:03}\r\n").as_bytes());
    }
    DaemonMsg::Output(scene).encode(&mut sender).unwrap();
    reading_ordered_screen(&mut sender, 97, 0);
    graphics_recovery_wait(cx, "ordered baseline did not paint", |cx| {
        view.read_with(cx, |view, _| view.scrollback_text().contains("LINE-119"))
    });
    view.update(cx, |view, cx| {
        view.smooth_scroll(47., cx);
    });
    graphics_assert_production_paint(window, &view, cx);
    reading_request_window_size(window, &view, cx, TermSize::new(40, 24));
    assert!(
        view.read_with(cx, |view, _| {
            view.selection_scroll
                .reading
                .as_ref()
                .is_some_and(|reading| reading.relocating())
        }),
        "the resize request left a pending relocation"
    );
    view.update(cx, |view, cx| {
        view.smooth_scroll(-10_000., cx);
    });
    assert!(
        view.read_with(cx, |view, _| view.selection_scroll.reading.is_none()),
        "returning to live abandons the pending reading"
    );
    window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            assert!(
                window.try_find("terminal-return-live").is_none(),
                "a live pane has no return-to-live pill, even with a zero-row count"
            );
        })
        .unwrap();
    // Complete the ordered resize so no pending relocation leaks out.
    reading_ordered_size(&mut sender, 40, 24);
    reading_ordered_screen(&mut sender, 97, 1);
    graphics_assert_production_paint_at(window, &view, cx, TermSize::new(40, 24));
    graphics_recovery_destroy(view, window, cx);
}
