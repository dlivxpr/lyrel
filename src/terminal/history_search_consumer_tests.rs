use super::*;

#[gpui_kit::test]
fn ctrl_r_fuzzy_search_accepts_into_the_editor(cx: &mut TestAppContext) {
    let (window, _daemon) = harness(cx);
    window
        .update(cx, |view, _, cx| {
            seed_history(view, ["git status", "cargo build", "git commit -m x"]
                .into_iter()
                .map(String::from)
                .collect());
            

            view.handle_editor_key(&key("ctrl-r"), cx);
            assert!(view.history.search().is_some(), "Ctrl+R opens the search");
            view.commit_text("gst", cx);
            assert_eq!(
                view.history.search()
                    .as_ref()
                    .and_then(|rs| rs.selected_line(view.history.entries())),
                Some("git status")
            );
            view.handle_editor_key(&key("enter"), cx);
            assert!(view.history.search().is_none(), "Enter closes the search");
            assert_eq!(view.cmd.text(), "git status");
        })
        .unwrap();
}

#[gpui_kit::test]
fn ctrl_r_steps_matches_and_cmd_enter_runs(cx: &mut TestAppContext) {
    crate::core::config::pin_test_config_dir();

    let (window, mut daemon) = harness(cx);
    window
        .update(cx, |view, _, cx| {
            seed_history(view, ["git status", "cargo build", "git commit -m x"]
                .into_iter()
                .map(String::from)
                .collect());
            

            view.handle_editor_key(&key("ctrl-r"), cx);
            view.commit_text("git", cx);
            assert_eq!(
                view.history.search()
                    .as_ref()
                    .and_then(|rs| rs.selected_line(view.history.entries())),
                Some("git commit -m x")
            );
            view.handle_editor_key(&key("ctrl-r"), cx);
            assert_eq!(
                view.history.search()
                    .as_ref()
                    .and_then(|rs| rs.selected_line(view.history.entries())),
                Some("git status")
            );
            view.handle_editor_key(&key("cmd-enter"), cx);
            assert!(view.history.search().is_none());
            assert!(view.cmd.is_empty(), "submit clears the editor");
        })
        .unwrap();
    assert_eq!(
        next_input_until_timeout(&mut daemon),
        Some(b"git status\r".to_vec()),
        "Cmd+Enter ships the selected line to the PTY"
    );
}

#[gpui_kit::test]
fn reverse_search_menu_survives_a_real_render_pass(cx: &mut TestAppContext) {
    let (window, mut daemon) = harness(cx);
    DaemonMsg::Prompt {
        active: true,
        at_prompt: true,
        last_exit: None,
    }
    .encode(&mut daemon)
    .unwrap();
    for _ in 0..200 {
        if window
            .update(cx, |view, _, _| view.terminal.at_prompt())
            .unwrap()
        {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }

    window
        .update(cx, |view, _, cx| {
            assert!(view.input_active(), "prompt report engages the editor");
            let mut history = history_fixture(["git status", "cargo build --release", "echo hello"]
                .into_iter()
                .map(String::from)
                .collect());
            
            history.meta.insert(
                "cargo build --release".into(),
                crate::terminal::history::EntryMeta {
                    ts: Some(unix_now().saturating_sub(7200)),
                    exit: Some(1),
                },
            );
            view.history = TerminalHistory::from_loaded(history);
            view.handle_editor_key(&key("ctrl-r"), cx);
            view.commit_text("c", cx);
            assert!(
                view.history.search()
                    .as_ref()
                    .is_some_and(|rs| !rs.matches().is_empty()),
                "the query has matches for the menu to draw"
            );
            cx.notify();
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |view, _, _| {
            assert!(view.history.search().is_some(), "search survives the frame");
        })
        .unwrap();
}
    /// #541: the history menu floats over live grid cells, and a `div` that
    /// carries no handler of its own inserts no hitbox — so the press went
    /// straight through to the grid behind the menu, which cleared the
    /// selection, dragged out a new one, or (with the link modifier held)
    /// opened whatever link the menu was covering.
    ///
    /// The second half is the other side of the pair `src/ui/app.rs` keeps for
    /// the resize handle: with the menu gone the very same press must still
    /// reach the grid, or this would pass on a pane that never sees a mouse.
    #[gpui_kit::test]
    fn a_press_on_the_history_menu_never_reaches_the_grid(cx: &mut TestAppContext) {
        use gpui_kit::{MouseMoveEvent, PlatformInput};

        crate::core::config::pin_test_config_dir();
        let (window, mut daemon) = harness(cx);
        prompt_ready(&window, cx, &mut daemon);
        wait_for_input_active(&window, cx);

        window
            .update(cx, |view, window, cx| {
                window.activate_window();
                view.focus_handle.focus(window, cx);
                seed_history(view, vec!["echo one".to_string(), "echo two".to_string()]);
                
                view.start_reverse_search();
                cx.notify();
            })
            .unwrap();

        let mut vcx = gpui_kit::VisualTestContext::from_window(window.into(), cx);
        vcx.update(|window, _| window.refresh());
        vcx.run_until_parked();

        // The menu is laid out one row under the cursor, plus the gap
        // `render_reverse_search_menu` leaves; this lands in the middle of its
        // first row, where a candidate is drawn.
        let lh = window.update(cx, |view, _, _| view.line_height).unwrap();
        let at = point(
            px(GRID_PAD_X) + px(10.),
            px(GRID_PAD_Y) + lh * 2.0 + px(10.),
        );

        let press = |vcx: &mut gpui_kit::VisualTestContext| {
            vcx.update(|window, cx| {
                window.dispatch_event(
                    PlatformInput::MouseMove(MouseMoveEvent {
                        position: at,
                        pressed_button: None,
                        modifiers: Modifiers::none(),
                    }),
                    cx,
                );
                window.dispatch_event(
                    PlatformInput::MouseDown(MouseDownEvent {
                        button: MouseButton::Left,
                        position: at,
                        modifiers: Modifiers::none(),
                        click_count: 1,
                        first_mouse: false,
                    }),
                    cx,
                );
            });
            vcx.run_until_parked();
        };

        press(&mut vcx);
        window
            .update(cx, |view, _, _| {
                assert!(
                    view.history.search().is_some(),
                    "the press must not have closed the menu either"
                );
                assert!(
                    !view.selection_scroll.is_selecting(),
                    "the menu swallowed the press, so no selection started under it"
                );
            })
            .unwrap();

        window
            .update(cx, |view, _, cx| {
                view.history.cancel_search();
                cx.notify();
            })
            .unwrap();
        vcx.update(|window, _| window.refresh());
        vcx.run_until_parked();

        press(&mut vcx);
        window
            .update(cx, |view, _, _| {
                assert!(
                    view.selection_scroll.is_selecting(),
                    "with no menu over it the same press is the grid's to take"
                );
            })
            .unwrap();
    }
