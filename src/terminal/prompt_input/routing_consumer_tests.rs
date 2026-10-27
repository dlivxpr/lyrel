use super::*;

#[gpui_kit::test]
fn ctrl_l_at_prompt_reaches_the_shell(cx: &mut TestAppContext) {
    let (window, mut daemon) = harness(cx);
    window
        .update(cx, |view, _, cx| {
            let ctrl_l = gpui_kit::Keystroke {
                modifiers: gpui_kit::Modifiers {
                    control: true,
                    ..Default::default()
                },
                key: "l".to_string(),
                key_char: None,
            };
            view.handle_editor_key(&ctrl_l, cx);
        })
        .unwrap();

    assert_eq!(next_input_until_timeout(&mut daemon), Some(vec![0x0c]));
}

#[gpui_kit::test]
fn passthrough_ctrl_c_discards_typeahead_before_the_shell_can_resume(cx: &mut TestAppContext) {
    assert_foreground_interrupt_does_not_wipe_prompt(cx, "agent input", "ctrl-c", 0x03);
}

#[gpui_kit::test]
fn passthrough_ctrl_d_discards_typeahead_before_the_shell_can_resume(cx: &mut TestAppContext) {
    // Ctrl-D only ends input on an empty line; with text it is an edit.
    assert_foreground_interrupt_does_not_wipe_prompt(cx, "", "ctrl-d", 0x04);
}

fn assert_foreground_interrupt_does_not_wipe_prompt(
    cx: &mut TestAppContext,
    pending: &str,
    chord: &str,
    byte: u8,
) {
    let (window, mut daemon) = harness(cx);
    window
        .update(cx, |view, window, cx| {
            assert!(!view.input_active(), "the foreground process owns input");
            view.input.record_for_test(RawInput::Text(pending), false);
            view.input.record_for_test(RawInput::Key {
                key: "up",
                plain: true,
            }, false);

            view.on_key_down(
                &KeyDownEvent {
                    keystroke: key(chord),
                    is_held: false,
                    prefer_character_input: false,
                },
                window,
                cx,
            );
            // Exercise the consumers without draining their input first.
            view.input.adopt_typeahead();
            view.input.flush_typeahead(&view.terminal);
            assert!(view.input.editor().text().is_empty());
        })
        .unwrap();

    assert_eq!(next_input_until_timeout(&mut daemon), Some(vec![byte]));
    assert_eq!(
        next_input_until_timeout(&mut daemon),
        None,
        "resuming the shell must not synthesize Ctrl-U after {chord}"
    );
}

#[gpui_kit::test]
fn submitted_exit_typeahead_does_not_wipe_the_returned_prompt(cx: &mut TestAppContext) {
    let (window, mut daemon) = harness(cx);
    window
        .update(cx, |view, window, cx| {
            // Ordinary SSH need not take the alternate screen or identify
            // as an agent. Its input reaches the passthrough recorder.
            assert!(!view.input_active());
            for ch in ["e", "x", "i", "t"] {
                type_char(view, ch, window, cx);
            }
            view.on_key_down(
                &KeyDownEvent {
                    keystroke: key("enter"),
                    is_held: false,
                    prefer_character_input: false,
                },
                window,
                cx,
            );
        })
        .unwrap();
    for bytes in [b"e", b"x", b"i", b"t", b"\r"] {
        assert_eq!(next_input_until_timeout(&mut daemon), Some(bytes.to_vec()));
    }

    prompt_ready(&window, cx, &mut daemon);
    window
        .update(cx, |view, _, _| {
            assert!(view.input_active());
            view.input.adopt_typeahead();
            view.input.flush_typeahead(&view.terminal);
            assert!(
                view.input.editor().text().is_empty(),
                "exit belongs to the finished session"
            );
        })
        .unwrap();
    assert_eq!(
        next_input_until_timeout(&mut daemon),
        None,
        "returning from exit must not inject Ctrl-U into the local prompt"
    );
}

#[gpui_kit::test]
fn ctrl_v_on_the_alternate_screen_reaches_the_pty_as_syn(cx: &mut TestAppContext) {
    let (window, mut daemon) = harness(cx);
    cx.update(|cx| cx.write_to_clipboard(ClipboardItem::new_string("echo hi".into())));
    alt_screen_ready(&window, cx, &mut daemon);
    window
        .update(cx, |view, window, cx| {
            assert!(!view.input_active(), "the full-screen program owns input");
            view.on_key_down(
                &KeyDownEvent {
                    keystroke: key("ctrl-v"),
                    is_held: false,
                    prefer_character_input: false,
                },
                window,
                cx,
            );
        })
        .unwrap();

    assert_eq!(next_input_until_timeout(&mut daemon), Some(vec![0x16]));
    assert_eq!(
        next_input_until_timeout(&mut daemon),
        None,
        "the clipboard must stay where it is: vim's Ctrl+V is blockwise select, not paste"
    );
}

#[gpui_kit::test]
fn agent_question_arrows_bypass_prompt_history_after_a_stale_prompt(cx: &mut TestAppContext) {
    assert_agent_question_owns_arrows(cx, crate::core::cli_agent::CLIAgent::OhMyPi, false);
}

#[gpui_kit::test]
fn agent_question_arrows_bypass_prompt_history_when_detection_arrives_last(
    cx: &mut TestAppContext,
) {
    assert_agent_question_owns_arrows(cx, crate::core::cli_agent::CLIAgent::Pi, true);
}

fn assert_agent_question_owns_arrows(
    cx: &mut TestAppContext,
    agent: crate::core::cli_agent::CLIAgent,
    prompt_first: bool,
) {
    let (window, mut daemon) = harness(cx);
    prompt_ready(&window, cx, &mut daemon);
    window
        .update(cx, |view, window, cx| {
            seed_history(view, vec!["echo shell-history".into()]);
            
            for chord in ["up", "down"] {
                view.on_key_down(
                    &KeyDownEvent {
                        keystroke: key(chord),
                        is_held: false,
                        prefer_character_input: false,
                    },
                    window,
                    cx,
                );
                assert_eq!(
                    view.input.editor().text(),
                    if chord == "up" {
                        "echo shell-history"
                    } else {
                        ""
                    },
                    "ordinary shell prompts must retain local history navigation"
                );
            }
            view.input.editor_mut().set("interactive-agent");
            view.on_key_down(
                &KeyDownEvent {
                    keystroke: key("enter"),
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
        Some(b"interactive-agent\r".to_vec())
    );

    // Agent detection and shell integration arrive independently. A stale
    // prompt must not reclaim input from a main-screen question TUI.
    let prompt = DaemonMsg::Prompt {
        active: true,
        at_prompt: true,
        last_exit: None,
    };
    let detected = DaemonMsg::Agent(Some(agent));
    let messages = if prompt_first {
        [prompt, detected]
    } else {
        [detected, prompt]
    };
    for message in messages {
        message.encode(&mut daemon).unwrap();
    }
    DaemonMsg::Output(b"\x1b[?2004hChoose an answer:\r\n> Yes\r\n  No".to_vec())
        .encode(&mut daemon)
        .unwrap();
    for _ in 0..200 {
        cx.run_until_parked();
        let ready = window
            .update(cx, |view, _, _| {
                view.agent() == Some(agent)
                    && view.terminal.prompt_seq() >= 2
                    && view
                        .terminal
                        .term
                        .lock()
                        .mode()
                        .contains(Mode::BRACKETED_PASTE)
            })
            .unwrap();
        if ready {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    window
        .update(cx, |view, window, cx| {
            assert_eq!(view.agent(), Some(agent));
            assert!(view.terminal.prompt_seq() >= 2, "the stale report arrived");
            assert!(
                view.terminal
                    .term
                    .lock()
                    .mode()
                    .contains(Mode::BRACKETED_PASTE)
            );
            assert!(view.terminal.at_prompt(), "retain the stale shell report");
            assert!(!view.on_alt_screen(), "the question uses the main screen");
            for chord in ["up", "down"] {
                view.on_key_down(
                    &KeyDownEvent {
                        keystroke: key(chord),
                        is_held: false,
                        prefer_character_input: false,
                    },
                    window,
                    cx,
                );
                assert_eq!(
                    view.input.editor().text(),
                    "",
                    "{chord} in an agent question must not recall shell history"
                );
                assert!(!view.history.is_navigating());
            }
        })
        .unwrap();
    assert_eq!(
        next_input_until_timeout(&mut daemon),
        Some(b"\x1b[A".to_vec())
    );
    assert_eq!(
        next_input_until_timeout(&mut daemon),
        Some(b"\x1b[B".to_vec())
    );

    let prompt = DaemonMsg::Prompt {
        active: true,
        at_prompt: true,
        last_exit: Some(0),
    };
    let exited = DaemonMsg::Agent(None);
    let messages = if prompt_first {
        [prompt, exited]
    } else {
        [exited, prompt]
    };
    for message in messages {
        message.encode(&mut daemon).unwrap();
    }
    for _ in 0..200 {
        cx.run_until_parked();
        let ready = window
            .update(cx, |view, _, _| {
                view.agent().is_none() && view.terminal.prompt_seq() >= 3
            })
            .unwrap();
        if ready {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    window
        .update(cx, |view, window, cx| {
            assert!(view.agent().is_none());
            assert!(
                view.terminal.prompt_seq() >= 3,
                "the returned prompt arrived"
            );
            assert!(
                view.input_active(),
                "the returned shell prompt owns input again"
            );
            view.on_key_down(
                &KeyDownEvent {
                    keystroke: key("up"),
                    is_held: false,
                    prefer_character_input: false,
                },
                window,
                cx,
            );
            assert_eq!(view.input.editor().text(), "interactive-agent");
        })
        .unwrap();
    assert_eq!(next_input_until_timeout(&mut daemon), None);
}

#[gpui_kit::test]
fn shell_vi_mode_prompt_bypasses_the_local_editor(cx: &mut TestAppContext) {
    let (window, mut daemon) = harness(cx);
    DaemonMsg::Prompt {
        active: true,
        at_prompt: true,
        last_exit: None,
    }
    .encode(&mut daemon)
    .unwrap();
    DaemonMsg::Output(b"\x1b]133;V;1\x07\x1b]133;B\x07".to_vec())
        .encode(&mut daemon)
        .unwrap();

    for _ in 0..200 {
        cx.run_until_parked();
        let ready = window
            .update(cx, |view, _, _| {
                view.terminal.shell_vi_mode() && view.terminal.zle_reading()
            })
            .unwrap();
        if ready {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }

    window
        .update(cx, |view, window, cx| {
            assert!(
                !view.input_active(),
                "shell vi-mode lets the shell line editor own prompt input"
            );
            type_char(view, "a", window, cx);
            assert_eq!(
                view.input.editor().text(),
                "",
                "vi-mode prompt input must not draw through the local overlay"
            );
        })
        .unwrap();
    assert_eq!(
        next_input_until_timeout(&mut daemon),
        Some(b"a".to_vec()),
        "shell vi-mode prompt input must reach the shell directly"
    );

    DaemonMsg::Prompt {
        active: true,
        at_prompt: true,
        last_exit: Some(0),
    }
    .encode(&mut daemon)
    .unwrap();
    DaemonMsg::Output(b"\x1b]133;V;0\x07\x1b]133;B\x07".to_vec())
        .encode(&mut daemon)
        .unwrap();
    for _ in 0..200 {
        cx.run_until_parked();
        let active = window.update(cx, |view, _, _| view.input_active()).unwrap();
        if active {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!("an emacs-mode prompt should re-enable lyrel's local editor");
}

/// Handing the line back to the shell walks the cursor left once per
/// character it sat before. Those are arrow keys like any other, so under
/// DECCKM they have to be SS3 — and zsh's zle does turn DECCKM on, so this
/// is the ordinary case rather than the exotic one.
#[gpui_kit::test]
fn a_handoff_walks_the_cursor_back_in_ss3_under_app_cursor_mode(cx: &mut TestAppContext) {
    crate::core::config::pin_test_config_dir();
    let (window, mut daemon) = harness(cx);
    DaemonMsg::Output(b"\x1b[?1h".to_vec())
        .encode(&mut daemon)
        .unwrap();
    DaemonMsg::Prompt {
        active: true,
        at_prompt: true,
        last_exit: None,
    }
    .encode(&mut daemon)
    .unwrap();
    wait_for_input_active(&window, cx);

    window
        .update(cx, |view, window, cx| {
            assert!(
                view.key_flags().app_cursor(),
                "the shell asked for application cursor keys"
            );
            for ch in ["z", "z", "q", "q", "x"] {
                type_char(view, ch, window, cx);
            }
            view.handle_editor_key(&key("left"), cx);
            view.handle_editor_key(&key("left"), cx);
            view.complete_tab(true, cx);
        })
        .unwrap();
    assert_eq!(
        next_input_until_timeout(&mut daemon),
        Some(b"zzqqx".to_vec())
    );
    assert_eq!(
        next_input_until_timeout(&mut daemon),
        Some(b"\x1bOD\x1bOD".to_vec()),
        "the cursor walks back in SS3, not CSI"
    );
}

#[gpui_kit::test]
fn tab_with_no_candidates_hands_the_line_to_the_shell(cx: &mut TestAppContext) {
    let (window, mut daemon) = harness(cx);
    DaemonMsg::Prompt {
        active: true,
        at_prompt: true,
        last_exit: None,
    }
    .encode(&mut daemon)
    .unwrap();
    wait_for_input_active(&window, cx);

    window
        .update(cx, |view, window, cx| {
            for ch in ["z", "z", "q", "q", "x"] {
                type_char(view, ch, window, cx);
            }
            assert_eq!(view.input.editor().text(), "zzqqx");
            view.complete_tab(true, cx);
            assert_eq!(view.input.editor().text(), "", "the line moved to the shell");
            assert!(
                !view.input_active(),
                "the shell owns the prompt after the handoff"
            );
        })
        .unwrap();
    assert_eq!(
        next_input_until_timeout(&mut daemon),
        Some(b"zzqqx".to_vec()),
        "the edited line ships ahead of the Tab"
    );
    assert_eq!(
        next_input_until_timeout(&mut daemon),
        Some(b"\t".to_vec()),
        "the Tab reaches the PTY instead of being swallowed"
    );

    DaemonMsg::Prompt {
        active: true,
        at_prompt: true,
        last_exit: None,
    }
    .encode(&mut daemon)
    .unwrap();
    for _ in 0..200 {
        cx.run_until_parked();
        let applied = window
            .update(cx, |view, _, _| view.terminal.prompt_seq() >= 2)
            .unwrap();
        if applied {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    window
        .update(cx, |view, _, _| {
            assert!(
                !view.input_active(),
                "a same-prompt redraw must not re-engage the editor"
            );
        })
        .unwrap();

    DaemonMsg::Prompt {
        active: true,
        at_prompt: false,
        last_exit: None,
    }
    .encode(&mut daemon)
    .unwrap();
    DaemonMsg::Prompt {
        active: true,
        at_prompt: true,
        last_exit: Some(0),
    }
    .encode(&mut daemon)
    .unwrap();
    wait_for_input_active(&window, cx);
}

#[gpui_kit::test]
fn ctrl_c_after_tab_handoff_returns_the_fresh_prompt_to_the_editor(cx: &mut TestAppContext) {
    crate::core::config::pin_test_config_dir();
    let (window, mut daemon) = harness(cx);
    DaemonMsg::Prompt {
        active: true,
        at_prompt: true,
        last_exit: None,
    }
    .encode(&mut daemon)
    .unwrap();
    wait_for_input_active(&window, cx);

    window
        .update(cx, |view, window, cx| {
            for ch in ["z", "z", "q", "q", "x"] {
                type_char(view, ch, window, cx);
            }
            view.complete_tab(true, cx);
            assert!(!view.input_active(), "Tab handed this line to the shell");
            view.on_key_down(
                &KeyDownEvent {
                    keystroke: key("ctrl-c"),
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
        Some(b"zzqqx".to_vec())
    );
    assert_eq!(next_input_until_timeout(&mut daemon), Some(b"\t".to_vec()));
    assert_eq!(next_input_until_timeout(&mut daemon), Some(vec![0x03]));

    // Tab handoff never emitted C, so the shell integration reports the
    // interrupted prompt as another A/B while the daemon is still at_prompt.
    DaemonMsg::Prompt {
        active: true,
        at_prompt: true,
        last_exit: Some(130),
    }
    .encode(&mut daemon)
    .unwrap();
    wait_for_input_active(&window, cx);

    window
        .update(cx, |view, window, cx| {
            type_char(view, "n", window, cx);
            assert_eq!(
                view.input.editor().text(),
                "n",
                "the first character on the fresh line belongs to lyrel's editor"
            );
        })
        .unwrap();
    assert_eq!(
        next_input_until_timeout(&mut daemon),
        None,
        "the fresh line must not keep going raw to the shell"
    );
}

#[gpui_kit::test]
fn shell_vi_mode_prompt_input_is_not_typeahead(cx: &mut TestAppContext) {
    let (window, mut daemon) = harness(cx);
    DaemonMsg::Prompt {
        active: true,
        at_prompt: true,
        last_exit: None,
    }
    .encode(&mut daemon)
    .unwrap();
    DaemonMsg::Output(b"\x1b]133;V;1\x07\x1b]133;B\x07".to_vec())
        .encode(&mut daemon)
        .unwrap();

    for _ in 0..200 {
        cx.run_until_parked();
        let ready = window
            .update(cx, |view, _, _| {
                !view.input_active()
                    && view.terminal.shell_vi_mode()
                    && view.terminal.zle_reading()
            })
            .unwrap();
        if ready {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }

    window
        .update(cx, |view, window, cx| {
            type_char(view, "i", window, cx);
        })
        .unwrap();
    assert_eq!(
        next_input_until_timeout(&mut daemon),
        Some(b"i".to_vec()),
        "vi prompt input is normal shell input, not deferred gap typeahead"
    );

    DaemonMsg::Output(b"\x1b]133;V;0\x07\x1b]133;B\x07".to_vec())
        .encode(&mut daemon)
        .unwrap();
    for _ in 0..200 {
        cx.run_until_parked();
        let active = window.update(cx, |view, _, _| view.input_active()).unwrap();
        if active {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    window
        .update(cx, |view, _, _| assert!(view.input_active()))
        .unwrap();
    assert_eq!(
        next_input_until_timeout(&mut daemon),
        None,
        "leaving shell vi-mode must not flush a stale typeahead wipe"
    );
}

#[gpui_kit::test]
fn shell_vi_mode_prompt_releases_gap_hold_without_stale_typeahead(cx: &mut TestAppContext) {
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
        .update(cx, |view, _, cx| view.commit_text("ls", cx))
        .unwrap();

    DaemonMsg::Prompt {
        active: true,
        at_prompt: true,
        last_exit: Some(0),
    }
    .encode(&mut daemon)
    .unwrap();
    DaemonMsg::Output(b"\x1b]133;V;1\x07\x1b]133;B\x07".to_vec())
        .encode(&mut daemon)
        .unwrap();
    for _ in 0..200 {
        cx.run_until_parked();
        let ready = window
            .update(cx, |view, _, _| {
                view.terminal.shell_vi_mode() && view.terminal.zle_reading()
            })
            .unwrap();
        if ready {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    cx.executor().advance_clock(HOLD_WINDOW * 2);
    cx.run_until_parked();
    assert_eq!(
        next_input_until_timeout(&mut daemon),
        Some(b"ls".to_vec()),
        "gap text typed before a vi prompt must reach the shell"
    );

    DaemonMsg::Output(b"\x1b]133;V;0\x07\x1b]133;B\x07".to_vec())
        .encode(&mut daemon)
        .unwrap();
    for _ in 0..200 {
        cx.run_until_parked();
        let active = window.update(cx, |view, _, _| view.input_active()).unwrap();
        if active {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    window
        .update(cx, |view, _, _| {
            assert!(view.input_active());
            assert_eq!(
                view.input.editor().text(),
                "",
                "gap text consumed at the vi prompt must not resurrect in the editor"
            );
        })
        .unwrap();
    assert_eq!(
        next_input_until_timeout(&mut daemon),
        None,
        "no stale ^U wipe once the vi prompt consumed the gap text"
    );
}
