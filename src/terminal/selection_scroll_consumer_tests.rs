use super::*;
/// The whole point of the animation: a detent must not land in one go.
#[gpui_kit::test]
fn a_wheel_notch_is_spread_over_frames(cx: &mut TestAppContext) {
    let (window, _daemon) = harness(cx);
    window
        .update(cx, |view, w, cx| {
            scroll_into_history(view, 10);
            let ev = notch(view, -4.9);
            view.on_scroll(&ev, w, cx);
            assert_eq!(
                display_offset(view),
                10,
                "the detent was applied whole, before a single frame ran"
            );
            assert_eq!(
                view.selection_scroll.fraction(),
                0.,
                "and not even a sliver of it"
            );
            assert!(
                view.selection_scroll.is_animating(),
                "nothing was left to animate"
            );
        })
        .unwrap();
}

/// Zooming has to take the wheel away from the buffer entirely, or the
/// grid would slide under the pointer while the font changed size.
#[gpui_kit::test]
fn the_zoom_modifier_takes_the_wheel_off_the_scrollback(cx: &mut TestAppContext) {
    let (window, _daemon) = harness(cx);
    cx.update(|cx| cx.global_mut::<Config>().mouse_zoom_modifier = MouseZoomModifier::Platform);
    window
        .update(cx, |view, w, cx| {
            scroll_into_history(view, 10);
            let mut ev = notch(view, -4.9);
            ev.modifiers = Modifiers::secondary_key();
            view.on_scroll(&ev, w, cx);
            assert_eq!(display_offset(view), 10, "the wheel reached the grid");
            assert!(
                !view.selection_scroll.is_animating(),
                "and queued more of it"
            );
        })
        .unwrap();
}

/// The point of turning it off: the modifier goes back to being an
/// ordinary scroll, rather than eating the wheel.
#[gpui_kit::test]
fn zooming_off_hands_the_wheel_back_to_the_scrollback(cx: &mut TestAppContext) {
    let (window, _daemon) = harness(cx);
    cx.update(|cx| {
        cx.global_mut::<Config>().mouse_zoom_modifier = MouseZoomModifier::None;
    });
    window
        .update(cx, |view, w, cx| {
            scroll_into_history(view, 10);
            let mut ev = notch(view, -4.9);
            ev.modifiers = Modifiers::secondary_key();
            view.on_scroll(&ev, w, cx);
            assert!(
                view.selection_scroll.is_animating(),
                "the wheel never reached the scrollback"
            );
        })
        .unwrap();
}

/// The bug this guards: a two-finger flick coasts long after the fingers
/// are gone, and every coasting event carries whatever modifiers are down
/// when it lands. Grabbing ⌘ for something else mid-coast used to read as
/// a zoom and run the font down to its minimum in a blink (#912).
#[gpui_kit::test]
fn a_modifier_pressed_mid_flick_does_not_hijack_it_into_a_zoom(cx: &mut TestAppContext) {
    let (window, _daemon) = harness(cx);
    window
        .update(cx, |view, w, cx| {
            scroll_into_history(view, 10);
            view.on_scroll(&wheel(view, -0.5, gpui_kit::TouchPhase::Started), w, cx);
            let before = display_offset(view);
            // The tail, now stamped with ⌘ the hand reached for.
            let mut tail = wheel(view, -3., gpui_kit::TouchPhase::Moved);
            tail.modifiers = Modifiers::secondary_key();
            view.on_scroll(&tail, w, cx);
            assert!(
                display_offset(view) != before || view.selection_scroll.is_animating(),
                "the tail stayed a scroll, the way the gesture started"
            );
        })
        .unwrap();
}

/// The same latch the other way round: a zoom gesture that outlives the
/// modifier keeps zooming rather than dumping its tail into the buffer.
#[gpui_kit::test]
fn a_zoom_gesture_keeps_zooming_after_the_modifier_is_released(cx: &mut TestAppContext) {
    let (window, _daemon) = harness(cx);
    cx.update(|cx| cx.global_mut::<Config>().mouse_zoom_modifier = MouseZoomModifier::Platform);
    window
        .update(cx, |view, w, cx| {
            scroll_into_history(view, 10);
            let mut start = wheel(view, -0.5, gpui_kit::TouchPhase::Started);
            start.modifiers = Modifiers::secondary_key();
            view.on_scroll(&start, w, cx);
            view.on_scroll(&wheel(view, -0.5, gpui_kit::TouchPhase::Moved), w, cx);
            assert_eq!(display_offset(view), 10, "the grid never moved");
            assert!(
                !view.selection_scroll.is_animating(),
                "and nothing was queued for it"
            );
        })
        .unwrap();
}

/// The latch is per gesture, not per stream: fingers going down again
/// start a fresh question. Carrying the last answer over would mean one
/// ⌘-zoom left every later flick zooming with no modifier held at all.
#[gpui_kit::test]
fn a_new_gesture_is_not_bound_by_what_the_last_one_answered(cx: &mut TestAppContext) {
    let (window, _daemon) = harness(cx);
    cx.update(|cx| cx.global_mut::<Config>().mouse_zoom_modifier = MouseZoomModifier::Platform);
    window
        .update(cx, |view, w, cx| {
            scroll_into_history(view, 10);
            let mut zoom = wheel(view, -0.5, gpui_kit::TouchPhase::Started);
            zoom.modifiers = Modifiers::secondary_key();
            view.on_scroll(&zoom, w, cx);
            assert_eq!(display_offset(view), 10, "that one was a zoom");
            // Fingers down again, nothing held: a plain scroll.
            view.on_scroll(&wheel(view, -3., gpui_kit::TouchPhase::Started), w, cx);
            assert_ne!(
                display_offset(view),
                10,
                "the new gesture asked the question again"
            );
        })
        .unwrap();
}

/// The one thing typing at a prompt must never do is land where the
/// person typing cannot see it.
#[gpui_kit::test]
fn typing_brings_the_view_back_to_the_prompt(cx: &mut TestAppContext) {
    let (window, mut daemon) = harness(cx);
    prompt_ready(&window, cx, &mut daemon);
    window
        .update(cx, |view, _w, cx| {
            assert!(
                view.input_active(),
                "this is the branch that was leaving the view parked"
            );
            scroll_into_history(view, 10);
            view.commit_text("l", cx);
            assert_eq!(
                display_offset(view),
                0,
                "the character went in while the viewport stayed in the scrollback"
            );
            assert_eq!(view.input.editor().text(), "l", "and it did reach the line");
        })
        .unwrap();
}

/// A paste is a larger change than a keystroke to make out of sight, and
/// it went the same way.
#[gpui_kit::test]
fn pasting_brings_the_view_back_to_the_prompt(cx: &mut TestAppContext) {
    let (window, mut daemon) = harness(cx);
    prompt_ready(&window, cx, &mut daemon);
    window
        .update(cx, |view, _w, cx| {
            scroll_into_history(view, 10);
            view.paste("cargo test\n".to_string(), cx);
            assert_eq!(display_offset(view), 0, "the paste landed off screen");
            assert_eq!(
                view.input.editor().text(),
                "cargo test",
                "and the trailing newline is still dropped"
            );
        })
        .unwrap();
}

/// A trackpad is already a continuous stream — animating it would only put
/// lag between the fingers and the grid. It is told apart by its phase, not
/// by its delta type or size: a flick moves further in one event than a
/// slowly inched wheel does.
#[gpui_kit::test]
fn a_trackpad_gesture_scrolls_the_instant_it_is_touched(cx: &mut TestAppContext) {
    let (window, _daemon) = harness(cx);
    window
        .update(cx, |view, w, cx| {
            scroll_into_history(view, 10);
            let ev = wheel(view, -3., gpui_kit::TouchPhase::Started);
            view.on_scroll(&ev, w, cx);
            assert_eq!(display_offset(view), 7, "the gesture was held back");
            assert!(
                !view.selection_scroll.is_animating(),
                "a trackpad started an animation"
            );
        })
        .unwrap();
}

/// Lifting the fingers does not end the stream: macOS keeps sending Moved
/// events for the momentum tail, and they are *larger* than the gesture
/// that spawned them. Treating those as a wheel would smooth what the
/// system is already smoothing.
#[gpui_kit::test]
fn a_momentum_tail_is_not_mistaken_for_a_wheel(cx: &mut TestAppContext) {
    let (window, _daemon) = harness(cx);
    window
        .update(cx, |view, w, cx| {
            scroll_into_history(view, 10);
            for (lines, phase) in [
                (-0.5, gpui_kit::TouchPhase::Started),
                (-1.2, gpui_kit::TouchPhase::Moved),
                (0., gpui_kit::TouchPhase::Ended),
                (-2.9, gpui_kit::TouchPhase::Moved),
                (-2.4, gpui_kit::TouchPhase::Moved),
            ] {
                let ev = wheel(view, lines, phase);
                view.on_scroll(&ev, w, cx);
                assert!(
                    !view.selection_scroll.is_animating(),
                    "the momentum tail was animated at {lines} lines"
                );
            }
            assert_eq!(display_offset(view), 3, "the tail did not all land");
        })
        .unwrap();
}

/// Inching the wheel one detent at a time reads as continuous already.
#[gpui_kit::test]
fn a_scroll_too_small_to_see_jump_stays_direct(cx: &mut TestAppContext) {
    let (window, _daemon) = harness(cx);
    window
        .update(cx, |view, w, cx| {
            scroll_into_history(view, 10);
            let ev = notch(view, -0.57);
            view.on_scroll(&ev, w, cx);
            assert!(
                !view.selection_scroll.is_animating(),
                "half a line was animated"
            );
            assert!(
                view.selection_scroll.fraction() > 0.,
                "and it did not move either"
            );
        })
        .unwrap();
}

#[gpui_kit::test]
fn turning_smooth_scrolling_off_restores_the_direct_path(cx: &mut TestAppContext) {
    let (window, _daemon) = harness(cx);
    cx.update(|cx| cx.global_mut::<Config>().smooth_scroll = false);
    window
        .update(cx, |view, w, cx| {
            scroll_into_history(view, 10);
            let ev = notch(view, -3.);
            view.on_scroll(&ev, w, cx);
            assert_eq!(display_offset(view), 7);
            assert!(!view.selection_scroll.is_animating());
        })
        .unwrap();
}

/// An animation that kept walking after the view was moved out from under
/// it would drag the user back off the prompt they just jumped to.
#[gpui_kit::test]
fn moving_the_viewport_cancels_an_animation_in_flight(cx: &mut TestAppContext) {
    let (window, _daemon) = harness(cx);
    window
        .update(cx, |view, w, cx| {
            scroll_into_history(view, 10);
            let ev = notch(view, -4.9);
            view.on_scroll(&ev, w, cx);
            assert!(view.selection_scroll.is_animating());

            view.jump_to_prompt();
            assert!(
                !view.selection_scroll.is_animating(),
                "the animation outlived the jump"
            );
            assert_eq!(display_offset(view), 0);
        })
        .unwrap();
}

/// A frame callback that was already queued when the animation was
/// cancelled must not walk the viewport on its stale epoch.
#[gpui_kit::test]
fn a_stale_frame_after_cancellation_does_not_walk(cx: &mut TestAppContext) {
    let (window, _daemon) = harness(cx);
    window
        .update(cx, |view, w, cx| {
            scroll_into_history(view, 10);
            let ev = notch(view, -4.9);
            view.on_scroll(&ev, w, cx);
            let epoch = view.selection_scroll.animation_epoch();
            view.selection_scroll.cancel_animation();

            assert!(
                !view.scroll_anim_frame(epoch, cx),
                "a stale frame callback kept the animation alive"
            );
            assert_eq!(display_offset(view), 10, "the stale frame moved");
        })
        .unwrap();
}

#[gpui_kit::test]
fn configured_word_selection_preserves_default_drag_reversal_and_shift(cx: &mut TestAppContext) {
    let (window, _daemon) = harness(cx);
    window
        .update(cx, |view, _, cx| {
            {
                let mut term = view.terminal.term.lock();
                rio_vt::performer::handler::Processor::default()
                    .advance(&mut *term, b"zero.foo.bar.baz tail");
            }
            let selected = |view: &TerminalView| view.terminal.term.lock().selection_to_string();
            cx.update_global::<Config, _>(|cfg, _| cfg.smart_select = false);
            view.on_select_start(6, 0, true, 2, false, cx);
            assert_eq!(selected(view).as_deref(), Some("zero.foo.bar.baz"));

            cx.update_global::<Config, _>(|cfg, _| cfg.word_separators.push('.'));
            view.on_select_start(6, 0, true, 2, false, cx);
            assert_eq!(selected(view).as_deref(), Some("foo"));
            view.on_select_update(14, 0, false, cx);
            assert_eq!(selected(view).as_deref(), Some("foo.bar.baz"));
            view.on_select_update(1, 0, true, cx);
            assert_eq!(selected(view).as_deref(), Some("zero.foo"));
            view.on_select_update(6, 0, true, cx);
            assert_eq!(selected(view).as_deref(), Some("foo"));
            view.on_select_end(cx);

            view.on_select_start(14, 0, false, 1, true, cx);
            assert_eq!(selected(view).as_deref(), Some("foo.bar.baz"));
            view.on_select_update(1, 0, true, cx);
            assert_eq!(selected(view).as_deref(), Some("zero.foo"));
            view.on_select_end(cx);

            view.select_all(cx);
            view.on_select_start(2, 0, false, 1, true, cx);
            assert_eq!(selected(view).as_deref(), Some("zer"));
            view.on_select_start(1, 0, true, 1, false, cx);
            view.on_select_update(2, 0, false, cx);
            assert_eq!(selected(view).as_deref(), Some("er"));
        })
        .unwrap();
}

#[gpui_kit::test]
fn configured_word_selection_keeps_smart_exact_nonexact_and_fallback_ranges(
    cx: &mut TestAppContext,
) {
    let (window, _daemon) = harness(cx);
    window
        .update(cx, |view, _, cx| {
            {
                let mut term = view.terminal.term.lock();
                rio_vt::performer::handler::Processor::default()
                    .advance(&mut *term, "left💙(foo)💙right x(foo)y foo.bar".as_bytes());
            }
            let selected = |view: &TerminalView| view.terminal.term.lock().selection_to_string();
            cx.update_global::<Config, _>(|cfg, _| cfg.word_separators.push('💙'));
            view.on_select_start(6, 0, true, 2, false, cx);
            assert_eq!(selected(view).as_deref(), Some("(foo)"));
            view.on_select_start(20, 0, true, 2, false, cx);
            assert_eq!(selected(view).as_deref(), Some("(foo)"));
            view.on_select_start(1, 0, true, 2, false, cx);
            assert_eq!(selected(view).as_deref(), Some("left"));

            cx.update_global::<Config, _>(|cfg, _| cfg.word_separators.push('.'));
            view.on_select_start(28, 0, true, 2, false, cx);
            assert_eq!(selected(view).as_deref(), Some("foo.bar"));
        })
        .unwrap();
}

#[gpui_kit::test]
fn configured_word_selection_autoscroll_extends_by_whole_words(cx: &mut TestAppContext) {
    let (window, _daemon) = harness(cx);
    window
        .update(cx, |view, _, cx| {
            {
                let mut term = view.terminal.term.lock();
                let output = b"zero.foo.bar.baz\r\n".repeat(term.screen_lines() + 2);
                rio_vt::performer::handler::Processor::default().advance(&mut *term, &output);
            }
            cx.update_global::<Config, _>(|cfg, _| {
                cfg.smart_select = false;
                cfg.word_separators.push('.');
            });
            view.on_select_start(6, 0, true, 2, false, cx);
            view.select_autoscroll(1., 6, true, cx);
            let term = view.terminal.term.lock();
            assert!(term.grid.display_offset() > 0);
            let selected = term.selection_to_string().expect("word drag selection");
            assert!(selected.starts_with("foo.bar.baz"), "{selected:?}");
            assert!(selected.ends_with("zero.foo"), "{selected:?}");
        })
        .unwrap();
}

#[gpui_kit::test]
fn copy_on_select_writes_the_clipboard_at_mouse_up(cx: &mut TestAppContext) {
    let (window, mut daemon) = harness(cx);

    DaemonMsg::Output(b"hello world".to_vec())
        .encode(&mut daemon)
        .unwrap();
    for _ in 0..400 {
        cx.run_until_parked();
        let row: String = window
            .update(cx, |view, _, _| {
                let term = view.terminal.term.clone();
                let term = term.lock();
                (0..11).map(|c| term.grid[Line(0)][Column(c)].c()).collect()
            })
            .unwrap();
        if row == "hello world" {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }

    let drag_hello = |cx: &mut TestAppContext| {
        window
            .update(cx, |view, _, cx| {
                view.on_select_start(0, 0, true, 1, false, cx);
                view.on_select_update(4, 0, false, cx);
                view.on_select_end(cx);
            })
            .unwrap();
    };
    drag_hello(cx);
    assert_eq!(
        cx.update(|cx| cx.read_from_clipboard()),
        None,
        "default-off must never write the clipboard"
    );

    cx.update(|cx| cx.update_global::<Config, _>(|cfg, _| cfg.copy_on_select = true));
    drag_hello(cx);
    let text = cx.update(|cx| cx.read_from_clipboard().and_then(|item| item.text()));
    assert_eq!(text.as_deref(), Some("hello"));

    let selected = window
        .update(cx, |view, _, _| {
            view.terminal.term.lock().selection.is_some()
        })
        .unwrap();
    assert!(
        selected,
        "copy-on-select must keep the selection highlighted"
    );
}

#[gpui_kit::test]
fn the_scrollbar_moves_the_viewport_and_follows_it_back(cx: &mut TestAppContext) {
    use gpui_kit::component::scroll::ScrollbarHandle as _;

    let (window, mut daemon) = harness(cx);

    // Overflow the 24-row viewport so there is a scrollback to scroll.
    let mut out = Vec::new();
    for i in 0..60 {
        out.extend_from_slice(format!("line {i}\r\n").as_bytes());
    }
    DaemonMsg::Output(out).encode(&mut daemon).unwrap();
    // Wait for the reader to go quiet, not just to start: a scrollback
    // still filling underneath would move every row this test names.
    let mut settled = 0;
    for _ in 0..200 {
        let now = window
            .update(cx, |view, _, _| {
                view.terminal.term.lock().grid.history_size()
            })
            .unwrap();
        if now > 0 && now == settled {
            break;
        }
        settled = now;
        std::thread::sleep(std::time::Duration::from_millis(5));
    }

    window
        .update(cx, |view, _, _| {
            view.sync_scrollbar();
            let history = view.terminal.term.lock().grid.history_size();
            assert!(history > 0, "the test needs a scrollback to scroll");
            let row = view.line_height.as_f32();
            assert_eq!(
                view.scroll_handle.offset().y,
                px(-(history as f32) * row),
                "at the live edge the whole scrollback sits above the viewport"
            );

            // Drag the thumb a third of the way up its track. The bar only
            // records the row; the pane applies it on its next render.
            view.selection_scroll
                .smooth_scroll(&mut view.terminal.term.lock(), 0.5);
            view.scroll_handle
                .set_offset(point(px(0.), px(-(history as f32) * row / 3.)));
            assert_eq!(
                view.terminal.term.lock().grid.display_offset(),
                0,
                "the bar does not reach into the terminal itself"
            );

            view.sync_scrollbar();
            let offset = view.terminal.term.lock().grid.display_offset();
            assert_eq!(
                offset,
                history - (history as f32 / 3.).round() as usize,
                "the viewport lands on the row the thumb was dropped on"
            );
            assert_eq!(
                view.selection_scroll.fraction(),
                0.,
                "a sub-line remainder left over from the wheel would paint \
                     the grid off the row the thumb picked"
            );
            assert_eq!(
                view.scroll_handle.offset().y,
                px(-((history - offset) as f32) * row),
                "and the thumb reports the row the grid actually reached"
            );
        })
        .unwrap();
}

#[gpui_kit::test]
fn a_word_drag_keeps_its_live_anchor_when_output_scrolls(cx: &mut TestAppContext) {
    let (window, _daemon) = harness(cx);
    window
        .update(cx, |view, _, cx| {
            cx.update_global::<Config, _>(|cfg, _| {
                cfg.smart_select = false;
                cfg.word_separators.push('.');
            });
            let bottom = view.terminal.size().rows - 1;
            let mut parser = rio_vt::performer::handler::Processor::default();
            {
                let mut term = view.terminal.term.lock();
                parser.advance(&mut *term, &b"\r\n".repeat(bottom));
                parser.advance(&mut *term, b"zero.foo.bar.baz");
            }
            view.on_select_start(6, bottom, true, 2, false, cx);
            assert_eq!(view.selection_text().as_deref(), Some("foo"));
            parser.advance(&mut *view.terminal.term.lock(), b"\r\nnext");
            view.on_select_update(14, bottom - 1, false, cx);
            assert_eq!(view.selection_text().as_deref(), Some("foo.bar.baz"));
            view.on_select_update(1, bottom - 1, true, cx);
            assert_eq!(view.selection_text().as_deref(), Some("zero.foo"));
            view.on_select_end(cx);
            assert_eq!(view.selection_text().as_deref(), Some("zero.foo"));
        })
        .unwrap();
}

#[gpui_kit::test]
fn selection_uses_resized_grid_cells_at_scaled_display_metrics(cx: &mut TestAppContext) {
    let (window, mut daemon) = harness(cx);
    window
        .update(cx, |view, _, cx| {
            view.set_grid_size(100, 30, px(10.), px(24.), 2., cx);
        })
        .unwrap();
    DaemonMsg::Size(crate::daemon::protocol::WinSize {
        cols: 100,
        rows: 30,
        cell_w: 20,
        cell_h: 48,
    })
    .encode(&mut daemon)
    .unwrap();
    DaemonMsg::Output(b"\x1b[29;90Hresize".to_vec())
        .encode(&mut daemon)
        .unwrap();
    let mut ready = false;
    for _ in 0..200 {
        // The reader owns the ordered Size/Output frames; painting the default
        // test window here would issue an unrelated viewport resize over them.
        ready = window
            .update(cx, |view, _, _| {
                let term = view.terminal.term.lock();
                term.columns() == 100
                    && term.screen_lines() == 30
                    && term.grid[Line(28)][Column(94)].c() == 'e'
            })
            .unwrap();
        if ready {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(
        ready,
        "ordered resize and output must reach the consumer grid: {:?}",
        window
            .update(cx, |view, _, _| {
                let term = view.terminal.term.lock();
                (term.columns(), term.screen_lines(), view.terminal.size())
            })
            .unwrap()
    );
    window
        .update(cx, |view, _, cx| {
            view.on_select_start(89, 28, true, 1, false, cx);
            view.on_select_update(94, 28, false, cx);
            view.on_select_end(cx);
            assert_eq!(view.selection_text().as_deref(), Some("resize"));
        })
        .unwrap();
}
