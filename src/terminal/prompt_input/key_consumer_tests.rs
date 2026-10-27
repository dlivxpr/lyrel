use super::*;

#[gpui_kit::test]
fn insert_newline_action_extends_the_line_and_enter_submits_it(cx: &mut TestAppContext) {
    let dir = std::env::temp_dir().join(format!("lyrel-covtest-{}", std::process::id()));
    std::fs::create_dir_all(&dir).ok();
    crate::core::config::set_config_dir(dir);

    let (window, mut daemon) = harness(cx);
    prompt_ready(&window, cx, &mut daemon);

    window
        .update(cx, |view, _, cx| {
            assert!(view.input_active(), "the editor owns an idle prompt");
            view.commit_text("echo a", cx);
            view.insert_newline_action(cx);
            view.commit_text("echo b", cx);
            assert_eq!(view.input.editor().text(), "echo a\necho b");

            view.handle_editor_key(&key("enter"), cx);
            assert!(view.input.editor().is_empty(), "Enter submits the whole buffer");
        })
        .unwrap();
    assert_eq!(
        next_input_until_timeout(&mut daemon),
        Some(b"echo a\recho b\r".to_vec()),
        "the multi-line command reaches the PTY in one submit"
    );
}

#[gpui_kit::test]
fn shift_enter_reaches_a_foreground_tui_with_kitty_encoding(cx: &mut TestAppContext) {
    crate::core::config::pin_test_config_dir();
    let (window, mut daemon) = harness_on(cx, PtySource::LocalConpty);
    cx.update(|cx| crate::ui::keymap::init(cx));
    DaemonMsg::Output(b"\x1b[>1u".to_vec())
        .encode(&mut daemon)
        .unwrap();
    for _ in 0..200 {
        cx.run_until_parked();
        if window
            .update(cx, |view, _, _| view.key_flags().kitty_active())
            .unwrap()
        {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    window
        .update(cx, |view, window, cx| {
            assert!(!view.input_active(), "the foreground TUI owns input");
            window.activate_window();
            view.focus_handle.focus(window, cx);
        })
        .unwrap();

    let mut vcx = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    vcx.simulate_keystrokes("shift-enter");

    assert_eq!(
        next_input_until_timeout(&mut daemon),
        Some(b"\x1b[13;2u".to_vec())
    );

    vcx.simulate_keystrokes("ctrl-j");
    assert_eq!(
        next_input_until_timeout(&mut daemon),
        Some(b"\x1b[106;5u".to_vec())
    );
}

#[gpui_kit::test]
fn shift_enter_reaches_a_foreground_tui_as_lf_without_kitty(cx: &mut TestAppContext) {
    crate::core::config::pin_test_config_dir();
    let (window, mut daemon) = harness_on(cx, PtySource::Raw);
    cx.update(|cx| crate::ui::keymap::init(cx));
    window
        .update(cx, |view, window, cx| {
            assert!(!view.input_active(), "the foreground TUI owns input");
            window.activate_window();
            view.focus_handle.focus(window, cx);
        })
        .unwrap();

    let mut vcx = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    vcx.simulate_keystrokes("shift-enter");

    assert_eq!(next_input_until_timeout(&mut daemon), Some(b"\n".to_vec()));

    vcx.simulate_keystrokes("ctrl-j");
    assert_eq!(next_input_until_timeout(&mut daemon), Some(b"\n".to_vec()));
}

#[gpui_kit::test]
fn newline_chords_reach_conpty_as_ctrl_j_without_kitty(cx: &mut TestAppContext) {
    let (window, mut daemon) = harness_on(cx, PtySource::LocalConpty);
    cx.update(|cx| crate::ui::keymap::init(cx));
    window
        .update(cx, |view, window, cx| {
            assert!(!view.input_active());
            window.activate_window();
            view.focus_handle.focus(window, cx);
        })
        .unwrap();

    let mut vcx = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    for chord in ["shift-enter", "ctrl-j"] {
        vcx.simulate_keystrokes(chord);
        assert_eq!(
            next_input_until_timeout(&mut daemon),
            Some(b"\x1b[74;36;10;1;8;1_\x1b[74;36;10;0;8;1_".to_vec()),
            "{chord} must preserve Ctrl+J for native console readers"
        );
    }
}

#[gpui_kit::test]
fn alt_enter_keeps_its_legacy_encoding_in_a_foreground_tui(cx: &mut TestAppContext) {
    crate::core::config::pin_test_config_dir();
    let (window, mut daemon) = harness(cx);
    cx.update(|cx| crate::ui::keymap::init(cx));
    window
        .update(cx, |view, window, cx| {
            assert!(!view.input_active(), "the foreground TUI owns input");
            window.activate_window();
            view.focus_handle.focus(window, cx);
        })
        .unwrap();

    let mut vcx = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    vcx.simulate_keystrokes("alt-enter");

    assert_eq!(
        next_input_until_timeout(&mut daemon),
        Some(b"\x1b\r".to_vec())
    );
}

#[gpui_kit::test]
fn insert_newline_action_declines_when_the_editor_is_not_live(cx: &mut TestAppContext) {
    let (window, _daemon) = harness(cx);
    window
        .update(cx, |view, _, cx| {
            view.input.editor_mut().set("keep me");
            view.terminal.exited = true;
            assert!(!view.input_active());
            view.insert_newline_action(cx);
            assert_eq!(view.input.editor().text(), "keep me", "no newline inserted");
        })
        .unwrap();
}

#[gpui_kit::test]
fn the_keymap_routes_both_newline_chords_to_the_action(cx: &mut TestAppContext) {
    let (window, mut daemon) = harness(cx);
    cx.update(|cx| crate::ui::keymap::init(cx));
    prompt_ready(&window, cx, &mut daemon);
    window
        .update(cx, |view, window, cx| {
            window.activate_window();
            view.focus_handle.focus(window, cx);
            view.commit_text("echo a", cx);
        })
        .unwrap();

    let mut vcx = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    vcx.simulate_keystrokes("shift-enter");
    vcx.simulate_keystrokes("alt-enter");
    window
        .update(cx, |view, _, _| {
            assert_eq!(
                view.input.editor().text(),
                "echo a\n\n",
                "both chords dispatched InsertNewline instead of submitting"
            );
        })
        .unwrap();

    cx.update(|cx| crate::ui::keymap::rebind(cx));
    vcx.simulate_keystrokes("shift-enter");
    window
        .update(cx, |view, _, _| {
            assert_eq!(
                view.input.editor().text(),
                "echo a\n\n\n",
                "the chord survives a rebind"
            );
        })
        .unwrap();
}

/// The whole chain for #834, through the real dispatch tree: F3 is bound
/// to Find Next off macOS, and gpui matches bindings before the pane's key
/// handler. With no find bar open the action gives the key back, the pane
/// encodes it, and PSReadLine's CharacterSearch gets its `\EOR`.
///
/// F7 has no binding at all and is the control: it takes the same route
/// with nothing to fall through.
#[cfg(not(target_os = "macos"))]
#[gpui_kit::test]
fn an_unused_find_binding_gives_f3_back_to_the_shell(cx: &mut TestAppContext) {
    let (window, mut daemon) = harness(cx);
    cx.update(|cx| crate::ui::keymap::init(cx));
    prompt_ready(&window, cx, &mut daemon);
    window
        .update(cx, |view, window, cx| {
            window.activate_window();
            view.focus_handle.focus(window, cx);
            view.commit_text("echo a", cx);
        })
        .unwrap();

    let mut vcx = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    for (chord, seq) in [("f3", b"\x1bOR".to_vec()), ("f7", b"\x1b[18~".to_vec())] {
        window
            .update(cx, |view, _, _| {
                // The previous handoff gave this prompt to the shell for
                // good; take it back so both keys are tested from the
                // same starting state.
                view.input.reset_handoff();
                view.input.editor_mut().set("echo a");
                assert!(view.search.is_none(), "no find bar is open");
            })
            .unwrap();
        vcx.simulate_keystrokes(chord);
        window
            .update(cx, |view, _, _| {
                assert!(view.search.is_none(), "{chord} did not open the find bar");
                assert_eq!(view.input.editor().text(), "", "{chord} handed the line over");
            })
            .unwrap();
        assert_eq!(
            next_input_until_timeout(&mut daemon),
            Some(b"echo a".to_vec()),
            "{chord} puts the line on the shell's prompt first"
        );
        assert_eq!(
            next_input_until_timeout(&mut daemon),
            Some(seq),
            "{chord} reaches the PTY"
        );
    }
}

/// The second half of #834. Even once the encoder knew the F keys, the
/// inline editor still ate them: `handle_editor_key` had no arm for a
/// named key it does not bind, so F8 fell out of the bottom of the match
/// and died on a `cx.notify()`. PSReadLine's HistorySearchBackward acts on
/// the line that is on the prompt, so the fix is the unknown-chord route —
/// the line goes over first, then the key.
#[gpui_kit::test]
fn function_keys_hand_the_line_to_the_shell(cx: &mut TestAppContext) {
    let (window, mut daemon) = harness(cx);
    for (chord, seq) in [
        ("f8", b"\x1b[19~".to_vec()),
        ("shift-f8", b"\x1b[19;2~".to_vec()),
        ("alt-f7", b"\x1b[18;3~".to_vec()),
        ("f3", b"\x1bOR".to_vec()),
    ] {
        window
            .update(cx, |view, _, cx| {
                view.input.editor_mut().set("git st");
                view.handle_editor_key(&key(chord), cx);
                assert_eq!(view.input.editor().text(), "", "{chord} handed the line over");
            })
            .unwrap();
        assert_eq!(
            next_input_until_timeout(&mut daemon),
            Some(b"git st".to_vec()),
            "{chord} puts the line on the shell's prompt first"
        );
        assert_eq!(
            next_input_until_timeout(&mut daemon),
            Some(seq),
            "{chord} follows the line"
        );
    }
}

#[gpui_kit::test]
fn ctrl_j_and_ctrl_m_submit_the_line_like_enter(cx: &mut TestAppContext) {
    crate::core::config::pin_test_config_dir();

    let (window, mut daemon) = harness(cx);
    for (chord, line) in [("ctrl-j", "echo j"), ("ctrl-m", "echo m")] {
        window
            .update(cx, |view, _, cx| {
                view.input.editor_mut().set(line);
                view.handle_editor_key(&key(chord), cx);
                assert!(view.input.editor().is_empty(), "{chord} clears the editor");
            })
            .unwrap();
        assert_eq!(
            next_input_until_timeout(&mut daemon),
            Some(format!("{line}\r").into_bytes()),
            "{chord} ships the line to the PTY"
        );
    }
}

#[gpui_kit::test]
fn history_search_off_sends_ctrl_r_to_the_shell(cx: &mut TestAppContext) {
    let (window, mut daemon) = harness(cx);
    cx.update(|cx| {
        let mut cfg = cx.global::<Config>().clone();
        cfg.history_search = false;
        cx.set_global(cfg);
    });
    window
        .update(cx, |view, _, cx| {
            seed_history(view, ["git status"].into_iter().map(String::from).collect());
            
            view.input.editor_mut().set("gi");
            view.handle_editor_key(&key("ctrl-r"), cx);
            assert!(
                view.history.search().is_none(),
                "no lyrel menu while opted out"
            );
            assert_eq!(view.input.editor().text(), "", "the line went to the shell");
        })
        .unwrap();
    assert_eq!(next_input_until_timeout(&mut daemon), Some(b"gi".to_vec()));
    assert_eq!(
        next_input_until_timeout(&mut daemon),
        Some(vec![0x12]),
        "the raw ^R follows the handed-over line"
    );
}

/// The whole point of `prompt_editor: false`: a prefix and an arrow key
/// reach the PTY at a live OSC 133 prompt, so the shell's own line editor
/// (zsh's ZLE, readline) runs the widget bound there — including a history
/// widget reading the shell's own, shared history.
#[gpui_kit::test]
fn prompt_editor_off_hands_typing_and_arrows_to_the_shell(cx: &mut TestAppContext) {
    let (window, mut daemon) = harness(cx);
    DaemonMsg::Prompt {
        active: true,
        at_prompt: true,
        last_exit: None,
    }
    .encode(&mut daemon)
    .unwrap();
    // The editor engages first: this is a pane with working shell
    // integration, which the mode has to keep working.
    wait_for_input_active(&window, cx);

    window
        .update(cx, |view, window, cx| {
            seed_history(view, ["quit"].into_iter().map(String::from).collect());
            
            view.set_prompt_editor(false, cx);
            assert!(
                !view.input_active(),
                "the shell owns the prompt in native input mode"
            );
            assert!(
                view.terminal.at_prompt(),
                "OSC 133 prompt tracking stays live"
            );

            type_char(view, "h", window, cx);
            let up = KeyDownEvent {
                keystroke: key("up"),
                is_held: false,
                prefer_character_input: false,
            };
            view.on_key_down(&up, window, cx);

            assert_eq!(view.input.editor().text(), "", "nothing was typed into lyrel's editor");
            assert!(
                !view.history.is_navigating(),
                "Up never touched lyrel's own history"
            );
        })
        .unwrap();

    assert_eq!(next_input_until_timeout(&mut daemon), Some(b"h".to_vec()));
    assert_eq!(
        next_input_until_timeout(&mut daemon),
        Some(b"\x1b[A".to_vec()),
        "the physical Up key reaches the PTY for ZLE to bind"
    );

    window
        .update(cx, |view, _, cx| {
            view.set_prompt_editor(true, cx);
            assert!(
                view.input_active(),
                "turning it back on re-arms the editor at the same prompt"
            );
        })
        .unwrap();
}

/// Tab and Ctrl-R are the two keys with their own opt-outs; turning the
/// editor off has to hand them over too, without the missing-integration
/// notice that Ctrl-R raises when lyrel *wanted* the key and could not have
/// it.
#[gpui_kit::test]
fn prompt_editor_off_hands_over_tab_and_ctrl_r(cx: &mut TestAppContext) {
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
            view.set_prompt_editor(false, cx);
            view.created_at = std::time::Instant::now() - INTEGRATION_GRACE * 2;
            view.tab_pressed(true, cx);

            let ctrl_r = KeyDownEvent {
                keystroke: key("ctrl-r"),
                is_held: false,
                prefer_character_input: false,
            };
            view.on_key_down(&ctrl_r, window, cx);
            assert!(!view.completion.is_open(), "no lyrel completion menu");
            assert!(view.history.search().is_none(), "no lyrel history menu");
            assert!(
                view.integration_notice.is_none(),
                "the shell owning ^R is what was asked for, not a gap to report"
            );
        })
        .unwrap();

    assert_eq!(next_input_until_timeout(&mut daemon), Some(b"\t".to_vec()));
    assert_eq!(next_input_until_timeout(&mut daemon), Some(vec![0x12]));
}

/// Turning the setting off mid-line must not eat what is already typed —
/// the editor hands the line over the way an unknown chord does, so it is
/// still on the prompt for the shell to finish.
#[gpui_kit::test]
fn turning_the_prompt_editor_off_hands_the_typed_line_over(cx: &mut TestAppContext) {
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
        .update(cx, |view, _, cx| {
            view.input.editor_mut().set("git status");
            view.set_prompt_editor(false, cx);
            assert_eq!(view.input.editor().text(), "", "the editor let the line go");
        })
        .unwrap();

    assert_eq!(
        next_input_until_timeout(&mut daemon),
        Some(b"git status".to_vec()),
        "the half-typed line is on the shell's prompt now"
    );
}

#[gpui_kit::test]
fn meta_word_chords_edit_the_prompt_line(cx: &mut TestAppContext) {
    let (window, _daemon) = harness(cx);
    window
        .update(cx, |view, _, cx| {
            let meta = |key: &str| gpui_kit::Keystroke {
                modifiers: gpui_kit::Modifiers {
                    alt: true,
                    ..Default::default()
                },
                key: key.to_string(),
                key_char: None,
            };
            view.input.editor_mut().set("echo hello");
            view.handle_editor_key(&meta("b"), cx);
            assert_eq!(view.input.editor().cursor(), 5);
            view.handle_editor_key(&meta("d"), cx);
            assert_eq!(view.input.editor().text(), "echo ");
            view.handle_editor_key(&meta("b"), cx);
            assert_eq!(view.input.editor().cursor(), 0);
            view.handle_editor_key(&meta("f"), cx);
            assert_eq!(view.input.editor().cursor(), 4);
            view.handle_editor_key(&meta("z"), cx);
            assert_eq!(view.input.editor().text(), "");
        })
        .unwrap();
}

#[gpui_kit::test]
fn an_unknown_ctrl_chord_goes_to_the_shell_with_the_line(cx: &mut TestAppContext) {
    let (window, mut daemon) = harness(cx);
    window
        .update(cx, |view, _, cx| {
            view.input.editor_mut().set("echo hi");
            view.handle_editor_key(&key("ctrl-t"), cx);
            assert_eq!(
                view.input.editor().text(),
                "",
                "the line left for the shell, so the local buffer is empty"
            );
            assert!(
                view.input.has_handoff(),
                "the local editor stands down for the rest of the line"
            );
        })
        .unwrap();
    assert_eq!(next_input(&mut daemon), b"echo hi".to_vec());
    assert_eq!(next_input(&mut daemon), vec![0x14], "⌃T reached the shell");
}

#[gpui_kit::test]
fn an_unknown_meta_chord_goes_to_the_shell_with_the_line(cx: &mut TestAppContext) {
    let (window, mut daemon) = harness(cx);
    window
        .update(cx, |view, _, cx| {
            view.input.editor_mut().set("echo hi");
            view.handle_editor_key(
                &gpui_kit::Keystroke {
                    modifiers: gpui_kit::Modifiers {
                        alt: true,
                        ..Default::default()
                    },
                    key: "u".to_string(),
                    key_char: None,
                },
                cx,
            );
            assert_eq!(view.input.editor().text(), "");
        })
        .unwrap();
    assert_eq!(next_input(&mut daemon), b"echo hi".to_vec());
    assert_eq!(next_input(&mut daemon), b"\x1bu".to_vec());
}

#[gpui_kit::test]
fn ctrl_y_yanks_back_what_the_kill_chords_cut(cx: &mut TestAppContext) {
    let (window, _daemon) = harness(cx);
    window
        .update(cx, |view, _, cx| {
            view.input.editor_mut().set("echo hello world");
            view.handle_editor_key(&key("ctrl-w"), cx);
            assert_eq!(view.input.editor().text(), "echo hello ");
            view.handle_editor_key(&key("ctrl-y"), cx);
            assert_eq!(view.input.editor().text(), "echo hello world");
            assert!(
                !view.input.has_handoff(),
                "the line never left for the shell"
            );
        })
        .unwrap();
}

#[gpui_kit::test]
fn a_shifted_meta_chord_hands_off_the_shifted_character(cx: &mut TestAppContext) {
    let (window, mut daemon) = harness(cx);
    window
        .update(cx, |view, _, cx| {
            view.input.editor_mut().set("echo hi");
            view.handle_editor_key(
                &gpui_kit::Keystroke {
                    modifiers: gpui_kit::Modifiers {
                        alt: true,
                        shift: true,
                        ..Default::default()
                    },
                    key: "u".to_string(),
                    key_char: None,
                },
                cx,
            );
            assert_eq!(view.input.editor().text(), "");
        })
        .unwrap();
    assert_eq!(next_input(&mut daemon), b"echo hi".to_vec());
    assert_eq!(next_input(&mut daemon), b"\x1bU".to_vec());
}

#[gpui_kit::test]
fn a_known_ctrl_chord_stays_in_the_local_editor(cx: &mut TestAppContext) {
    let (window, _daemon) = harness(cx);
    window
        .update(cx, |view, _, cx| {
            view.input.editor_mut().set("echo hi");
            view.handle_editor_key(&key("ctrl-w"), cx);
            assert_eq!(view.input.editor().text(), "echo ", "⌃W cut the word locally");
            assert!(!view.input.has_handoff());
        })
        .unwrap();
}
