use super::*;

#[gpui_kit::test]
fn ctrl_c_copy_consumes_the_selection_so_the_next_press_is_sigint(cx: &mut TestAppContext) {
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

    window
        .update(cx, |view, window, cx| {
            view.on_select_start(0, 0, true, 1, false, cx);
            view.on_select_update(4, 0, false, cx);
            view.on_select_end(cx);
            assert!(view.has_selection(), "the drag must leave a selection");

            let consumed = view.handle_cmd_shortcut(&key("ctrl-c"), window, cx);
            assert!(matches!(consumed, CmdKey::Consumed));
            assert!(
                !view.has_selection(),
                "the Ctrl+C copy must consume the selection"
            );

            let fell_through = view.handle_cmd_shortcut(&key("ctrl-c"), window, cx);
            assert!(matches!(fell_through, CmdKey::FallThrough));
        })
        .unwrap();
    let text = cx.update(|cx| cx.read_from_clipboard().and_then(|item| item.text()));
    assert_eq!(text.as_deref(), Some("hello"));
}

#[gpui_kit::test]
fn cmd_v_pastes_on_the_alternate_screen(cx: &mut TestAppContext) {
    let (window, mut daemon) = harness(cx);
    cx.update(|cx| cx.write_to_clipboard(ClipboardItem::new_string("echo hi".into())));
    alt_screen_ready(&window, cx, &mut daemon);

    window
        .update(cx, |view, window, cx| {
            let pasted = view.handle_cmd_shortcut(&key("cmd-v"), window, cx);
            assert!(
                matches!(pasted, CmdKey::Consumed),
                "Cmd+V carries no control code and pastes on every screen"
            );
        })
        .unwrap();
    assert_eq!(next_input(&mut daemon), b"echo hi".to_vec());
    assert_eq!(next_input_until_timeout(&mut daemon), None);
}

/// The Ctrl+V half of the same key, through the real keymap: whether it
/// pastes is the `AlternatePaste` binding's decision, not this view's, so
/// these two drive it the way a user does rather than calling in.
#[cfg(not(target_os = "macos"))]
#[gpui_kit::test]
fn ctrl_v_pastes_at_a_prompt(cx: &mut TestAppContext) {
    crate::core::config::pin_test_config_dir();
    let (window, mut daemon) = harness(cx);
    cx.update(|cx| crate::ui::keymap::init(cx));
    cx.update(|cx| cx.write_to_clipboard(ClipboardItem::new_string("echo hi".into())));
    window
        .update(cx, |view, window, cx| {
            assert!(!view.on_alt_screen());
            window.activate_window();
            view.focus_handle.focus(window, cx);
        })
        .unwrap();

    let mut vcx = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    vcx.simulate_keystrokes("ctrl-v");

    assert_eq!(
        next_input_until_timeout(&mut daemon),
        Some(b"echo hi".to_vec())
    );
}

#[cfg(not(target_os = "macos"))]
#[gpui_kit::test]
fn ctrl_v_reaches_a_full_screen_program_as_syn(cx: &mut TestAppContext) {
    crate::core::config::pin_test_config_dir();
    let (window, mut daemon) = harness(cx);
    cx.update(|cx| crate::ui::keymap::init(cx));
    cx.update(|cx| cx.write_to_clipboard(ClipboardItem::new_string("echo hi".into())));
    alt_screen_ready(&window, cx, &mut daemon);
    window
        .update(cx, |view, window, cx| {
            window.activate_window();
            view.focus_handle.focus(window, cx);
        })
        .unwrap();

    let mut vcx = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    vcx.simulate_keystrokes("ctrl-v");

    assert_eq!(next_input_until_timeout(&mut daemon), Some(vec![0x16]));
}

/// The other half of that rule, which the binding's context cannot state.
///
/// gpui matches a keystroke against the frame it last *painted*, so
/// `!alt_screen` outlives the switch by a frame: launch a full-screen
/// program and hit Ctrl+V before the next paint and the keymap still
/// believes the pane is at a prompt. The action asks the grid itself, so
/// the clipboard never lands in a program that would run it as commands.
#[gpui_kit::test]
fn alternate_paste_asks_the_grid_and_not_the_last_frame(cx: &mut TestAppContext) {
    let (window, mut daemon) = harness(cx);
    cx.update(|cx| cx.write_to_clipboard(ClipboardItem::new_string("echo hi".into())));

    window
        .update(cx, |view, _window, cx| {
            assert!(!view.on_alt_screen());
            view.alternate_paste(cx);
        })
        .unwrap();
    assert_eq!(next_input(&mut daemon), b"echo hi".to_vec());

    alt_screen_ready(&window, cx, &mut daemon);
    window
        .update(cx, |view, _window, cx| view.alternate_paste(cx))
        .unwrap();
    assert_eq!(
        next_input_until_timeout(&mut daemon),
        None,
        "the paste is withheld even when the frame that matched said otherwise"
    );
}

/// `Ctrl-^`, which used to die in a hardcoded block that swallowed every
/// Ctrl+digit off macOS — nothing has claimed those chords since tabs
/// moved to Alt+1..9.
#[gpui_kit::test]
fn ctrl_6_reaches_the_pty_as_rs(cx: &mut TestAppContext) {
    crate::core::config::pin_test_config_dir();
    let (window, mut daemon) = harness(cx);
    cx.update(|cx| crate::ui::keymap::init(cx));
    window
        .update(cx, |view, window, cx| {
            window.activate_window();
            view.focus_handle.focus(window, cx);
        })
        .unwrap();

    let mut vcx = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    vcx.simulate_keystrokes("ctrl-6");

    assert_eq!(next_input_until_timeout(&mut daemon), Some(vec![0x1e]));
}

#[cfg(target_os = "macos")]
#[gpui_kit::test]
fn cmd_backspace_reaches_a_foreground_tui_as_ctrl_u(cx: &mut TestAppContext) {
    crate::core::config::pin_test_config_dir();
    let (window, mut daemon) = harness(cx);
    window
        .update(cx, |view, window, cx| {
            assert!(
                !view.input_active(),
                "the foreground process, not lyrel's editor, owns input"
            );
            view.on_key_down(
                &KeyDownEvent {
                    keystroke: gpui_kit::Keystroke {
                        modifiers: Modifiers {
                            platform: true,
                            ..Modifiers::default()
                        },
                        key: "backspace".into(),
                        key_char: None,
                    },
                    is_held: false,
                    prefer_character_input: false,
                },
                window,
                cx,
            );
        })
        .unwrap();

    assert_eq!(next_input_until_timeout(&mut daemon), Some(vec![0x15]));
}

#[cfg(target_os = "macos")]
#[gpui_kit::test]
fn cmd_navigation_reaches_a_foreground_tui_as_readline_controls(cx: &mut TestAppContext) {
    crate::core::config::pin_test_config_dir();
    let (window, mut daemon) = harness(cx);

    for (key, expected) in [("left", 0x01), ("right", 0x05), ("delete", 0x0b)] {
        window
            .update(cx, |view, window, cx| {
                assert!(!view.input_active(), "the foreground TUI owns input");
                view.on_key_down(
                    &KeyDownEvent {
                        keystroke: gpui_kit::Keystroke {
                            modifiers: Modifiers {
                                platform: true,
                                ..Modifiers::default()
                            },
                            key: key.into(),
                            key_char: None,
                        },
                        is_held: false,
                        prefer_character_input: false,
                    },
                    window,
                    cx,
                );
            })
            .unwrap();
        assert_eq!(next_input_until_timeout(&mut daemon), Some(vec![expected]));
    }
}

#[cfg(target_os = "macos")]
#[gpui_kit::test]
fn cmd_backspace_releases_held_input_before_ctrl_u(cx: &mut TestAppContext) {
    crate::core::config::pin_test_config_dir();
    let (window, mut daemon) = harness(cx);
    DaemonMsg::Prompt {
        active: true,
        at_prompt: false,
        last_exit: None,
    }
    .encode(&mut daemon)
    .unwrap();
    for _ in 0..200 {
        cx.run_until_parked();
        let gap = window
            .update(cx, |view, _, _| {
                view.terminal.shell_active() && !view.terminal.at_prompt()
            })
            .unwrap();
        if gap {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }

    window
        .update(cx, |view, window, cx| {
            view.commit_text("ls", cx);
            view.on_key_down(
                &KeyDownEvent {
                    keystroke: gpui_kit::Keystroke {
                        modifiers: Modifiers {
                            platform: true,
                            ..Modifiers::default()
                        },
                        key: "backspace".into(),
                        key_char: None,
                    },
                    is_held: false,
                    prefer_character_input: false,
                },
                window,
                cx,
            );
        })
        .unwrap();

    assert_eq!(
        next_input_until_timeout(&mut daemon),
        Some(b"ls".to_vec()),
        "held text must reach the PTY before the line-kill"
    );
    assert_eq!(
        next_input_until_timeout(&mut daemon),
        Some(vec![0x15]),
        "Ctrl-U must follow the text it clears"
    );
}

#[gpui_kit::test]
fn paste_to_the_pty_consumes_the_selection(cx: &mut TestAppContext) {
    let (window, mut daemon) = harness(cx);
    window
        .update(cx, |view, _, cx| {
            view.select_all(cx);
            assert!(view.has_selection());
            view.paste("echo hi".into(), cx);
            assert!(
                !view.has_selection(),
                "a PTY paste must consume the selection"
            );
        })
        .unwrap();
    assert_eq!(next_input(&mut daemon), b"echo hi".to_vec());
}

#[gpui_kit::test]
fn ctrl_c_copy_consumes_the_editor_selection_at_the_prompt(cx: &mut TestAppContext) {
    let (window, mut daemon) = harness(cx);

    DaemonMsg::Prompt {
        active: true,
        at_prompt: true,
        last_exit: Some(0),
    }
    .encode(&mut daemon)
    .unwrap();
    for _ in 0..400 {
        cx.run_until_parked();
        let active = window.update(cx, |view, _, _| view.input_active()).unwrap();
        if active {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }

    window
        .update(cx, |view, window, cx| {
            assert!(view.input_active(), "the inline editor must be active");
            view.input.editor_mut().insert_str("echo hi");
            view.input.editor_mut().select_all();

            let consumed = view.handle_cmd_shortcut(&key("ctrl-c"), window, cx);
            assert!(matches!(consumed, CmdKey::Consumed));
            assert!(
                view.input.editor().selection().is_none(),
                "the Ctrl+C copy must consume the editor selection"
            );

            let fell_through = view.handle_cmd_shortcut(&key("ctrl-c"), window, cx);
            assert!(matches!(fell_through, CmdKey::FallThrough));
        })
        .unwrap();
    let text = cx.update(|cx| cx.read_from_clipboard().and_then(|item| item.text()));
    assert_eq!(text.as_deref(), Some("echo hi"));
}
