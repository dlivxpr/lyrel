use super::*;
use crate::daemon::protocol::{ClientMsg, DaemonMsg};
use crate::daemon::transport::Stream;
use gpui_kit::TestAppContext;

fn harness(cx: &mut TestAppContext) -> (gpui_kit::WindowHandle<TerminalView>, Stream) {
    crate::core::config::pin_test_config_dir();
    cx.executor().allow_parking();
    let (client_side, daemon_side) = test_stream_pair();
    cx.update(|cx| {
        gpui_kit::component::init(cx);
        cx.set_global(Config::default());
    });
    let window = cx.add_window(|window, cx| {
        let terminal = RemoteTerminal::from_stream(client_side, TermSize::new(80, 24))
            .expect("link-backed terminal");
        TerminalView::with_terminal(terminal, 1, window, cx)
    });
    (window, daemon_side)
}

/// Everything the pane has written to the PTY, in order.
fn drain(daemon: &mut Stream) -> Vec<u8> {
    daemon
        .set_read_timeout(Some(std::time::Duration::from_millis(150)))
        .unwrap();
    let mut out = Vec::new();
    loop {
        match ClientMsg::read(daemon) {
            Ok(ClientMsg::Input(bytes)) => out.extend_from_slice(&bytes),
            Ok(_) => continue,
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                break;
            }
            Err(e) => panic!("pane link failed: {e}"),
        }
    }
    out
}

fn settle(
    cx: &mut TestAppContext,
    window: &gpui_kit::WindowHandle<TerminalView>,
    what: &str,
    f: impl Fn(&TerminalView) -> bool,
) {
    for _ in 0..300 {
        cx.run_until_parked();
        if window.update(cx, |view, _, _| f(view)).unwrap() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    panic!("never settled: {what}");
}

/// Bytes from the pane's pty, the way the daemon forwards them.
fn output(daemon: &mut Stream, bytes: &[u8]) {
    DaemonMsg::Output(bytes.to_vec()).encode(daemon).unwrap();
}

/// Waits for the tab's reading of what the pane calls itself to become
/// `expect`, running out the wait a new title is held for on each pass.
fn titled(
    cx: &mut TestAppContext,
    window: &gpui_kit::WindowHandle<TerminalView>,
    expect: Option<&str>,
) {
    for _ in 0..300 {
        cx.run_until_parked();
        cx.executor().advance_clock(TITLE_SETTLE * 2);
        cx.run_until_parked();
        let showing = window
            .update(cx, |view, _, _| view.stated_title().map(str::to_string))
            .unwrap();
        if showing.as_deref() == expect {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    panic!("the tab never came to read {expect:?}");
}

/// #889: a tab went on reading the name Claude Code had left on it long
/// after Claude exited and the pane was back at its own prompt in a real
/// directory. An OSC 0/2 had no end — only another OSC 0/2 replaced it —
/// so the last title any program wrote in a pane outlived it forever.
#[gpui_kit::test]
fn a_title_a_command_set_is_retired_when_that_command_finishes(cx: &mut TestAppContext) {
    let (window, mut daemon) = harness(cx);
    output(&mut daemon, b"\x1b]133;A\x07\x1b]133;B\x07");
    output(
        &mut daemon,
        b"\x1b]133;C;claude\x07\x1b]2;\xe2\x9c\xb3 fixing the switcher\x1b\\",
    );
    titled(cx, &window, Some("✳ fixing the switcher"));

    // Claude exits and the shell reports the command finished. Nothing
    // titles the pane after it, so the tab has to fall back down the
    // label ladder — `stated_title` saying nothing is how it does that.
    output(&mut daemon, b"\x1b]133;D;0\x07");
    titled(cx, &window, None);
}

/// The two cases a title is *supposed* to outlive: a program that is still
/// running, and a shell that titles its own prompt (which it does between
/// the `D` and the `A`, so it is the last word rather than a thing undone).
#[gpui_kit::test]
fn a_running_program_and_a_shells_own_prompt_title_both_keep_theirs(cx: &mut TestAppContext) {
    let (window, mut daemon) = harness(cx);
    output(
        &mut daemon,
        b"\x1b]133;C;vim\x07\x1b]2;vim \xe2\x80\x94 main.rs\x1b\\",
    );
    titled(cx, &window, Some("vim — main.rs"));

    output(
        &mut daemon,
        b"\x1b]133;D;0\x07\x1b]0;me@box:~/dev\x07\x1b]133;A\x07\x1b]133;B\x07",
    );
    titled(cx, &window, Some("me@box:~/dev"));
    settle(cx, &window, "the prompt is reading", |view| {
        view.terminal.zle_reading()
    });

    // And that prompt title is nobody's command to retire. Both marks are
    // chased by an edge the pane reports, so the assertion below cannot
    // pass on bytes that have not landed yet.
    output(&mut daemon, b"\x1b]133;C;ls\x07");
    settle(cx, &window, "a command takes the pane", |view| {
        !view.terminal.zle_reading()
    });
    output(&mut daemon, b"\x1b]133;D;0\x07\x1b]133;B\x07");
    settle(cx, &window, "the prompt comes back", |view| {
        view.terminal.zle_reading()
    });
    titled(cx, &window, Some("me@box:~/dev"));
}

/// #889 on a reattached window: a long session outran the daemon's replay
/// ring, so the replay carries the program's title but not the `C` that
/// started it. The replayed prompt state says a command owns the pane,
/// and that is enough to know the title is the command's to lose at `D`.
#[gpui_kit::test]
fn a_reattached_window_retires_a_title_whose_c_rolled_out_of_the_replay(
    cx: &mut TestAppContext,
) {
    crate::core::config::pin_test_config_dir();
    cx.executor().allow_parking();
    let (client_side, mut daemon) = test_stream_pair();
    cx.update(|cx| {
        gpui_kit::component::init(cx);
        cx.set_global(Config::default());
    });
    let window = cx.add_window(|window, cx| {
        let terminal =
            RemoteTerminal::from_stream_reattached(client_side, TermSize::new(80, 24))
                .expect("reattached link-backed terminal");
        TerminalView::with_terminal(terminal, 1, window, cx)
    });

    DaemonMsg::Snapshot(b"\x1b]2;\xe2\x9c\xb3 fixing the switcher\x1b\\redraw".to_vec())
        .encode(&mut daemon)
        .unwrap();
    DaemonMsg::Prompt {
        active: true,
        at_prompt: false,
        last_exit: None,
    }
    .encode(&mut daemon)
    .unwrap();
    titled(cx, &window, Some("✳ fixing the switcher"));

    output(&mut daemon, b"\x1b]133;D;0\x07");
    titled(cx, &window, None);
}

/// Printable text arrives the way the platform delivers it — through the
/// text-input path, which is what the gap hold and the typeahead record see.
fn type_text(
    window: &gpui_kit::WindowHandle<TerminalView>,
    cx: &mut TestAppContext,
    text: &str,
) {
    for ch in text.chars() {
        window
            .update(cx, |view, _, cx| view.commit_text(&ch.to_string(), cx))
            .unwrap();
    }
}

fn press(window: &gpui_kit::WindowHandle<TerminalView>, cx: &mut TestAppContext, key: &str) {
    window
        .update(cx, |view, window, cx| {
            view.on_key_down(
                &KeyDownEvent {
                    keystroke: gpui_kit::Keystroke::parse(key).unwrap(),
                    is_held: false,
                    prefer_character_input: false,
                },
                window,
                cx,
            );
        })
        .unwrap();
}

fn prompt(daemon: &mut Stream, at_prompt: bool) {
    DaemonMsg::Prompt {
        active: true,
        at_prompt,
        last_exit: None,
    }
    .encode(daemon)
    .unwrap();
}

/// Types `text` into the gap of a running command and lets the hold window
/// expire, so the bytes go to the PTY and are recorded for replay. Then
/// puts the pane back at a prompt the way the `D` mark does — before the
/// prompt is drawn, so the shell's line editor is not reading yet.
fn typed_into_the_gap_then_handed_back(
    cx: &mut TestAppContext,
    window: &gpui_kit::WindowHandle<TerminalView>,
    daemon: &mut Stream,
    text: &str,
) {
    prompt(daemon, true);
    DaemonMsg::Output(b"\x1b]133;B\x07".to_vec())
        .encode(daemon)
        .unwrap();
    settle(cx, window, "the editor takes the first prompt", |view| {
        view.input_active() && view.terminal.zle_reading()
    });

    prompt(daemon, false);
    DaemonMsg::Output(b"\x1b]133;C\x07".to_vec())
        .encode(daemon)
        .unwrap();
    settle(cx, window, "a command takes the pane", |view| {
        !view.input_active()
    });

    type_text(window, cx, text);
    cx.executor().advance_clock(HOLD_WINDOW * 2);
    cx.run_until_parked();
    assert_eq!(
        drain(daemon),
        text.as_bytes(),
        "the hold window gives up and dumps what it held"
    );

    prompt(daemon, true);
    settle(cx, window, "the editor takes the prompt back", |view| {
        view.input_active()
    });
    assert!(
        !window
            .update(cx, |view, _, _| view.terminal.zle_reading())
            .unwrap(),
        "this is the D-to-B window: the shell is not reading its line yet"
    );
}

#[gpui_kit::test]
fn a_line_typed_into_the_gap_survives_a_prompt_that_is_not_reading_yet(
    cx: &mut TestAppContext,
) {
    let (window, mut daemon) = harness(cx);
    typed_into_the_gap_then_handed_back(cx, &window, &mut daemon, "echo hi");

    press(&window, cx, "enter");
    cx.run_until_parked();
    assert_eq!(
        drain(&mut daemon),
        b"\x15echo hi\r".to_vec(),
        "the line the shell is holding must be erased and submitted whole, \
         not erased and replaced by an empty command"
    );
}

#[gpui_kit::test]
fn a_prompt_handover_keeps_the_held_text_in_front_of_what_follows_it(cx: &mut TestAppContext) {
    let (window, mut daemon) = harness(cx);
    typed_into_the_gap_then_handed_back(cx, &window, &mut daemon, "echo");

    // Typing carries straight on into the editor that just took the prompt.
    type_text(&window, cx, " hi");
    cx.run_until_parked();
    assert_eq!(
        drain(&mut daemon),
        Vec::<u8>::new(),
        "the editor owns these keys, so none of them reach the PTY"
    );

    press(&window, cx, "enter");
    cx.run_until_parked();
    assert_eq!(
        drain(&mut daemon),
        b"\x15echo hi\r".to_vec(),
        "what the shell was holding leads the line, not the tail alone"
    );
}

/// The editor takes the held line over the moment it is touched, before it
/// edits anything — and takes it over without putting the wipe on the wire,
/// which is still the shell's line editor's to receive when it starts
/// reading.
#[gpui_kit::test]
fn the_editor_takes_the_held_line_over_before_it_edits_it(cx: &mut TestAppContext) {
    let (window, mut daemon) = harness(cx);
    typed_into_the_gap_then_handed_back(cx, &window, &mut daemon, "echo");

    // `home` moves the caret and nothing else: any editor key is enough.
    press(&window, cx, "home");
    cx.run_until_parked();
    window
        .update(cx, |view, _, _| {
            assert_eq!(
                view.input.editor().text(),
                "echo",
                "the line the shell is sitting on is the editor's line now"
            );
        })
        .unwrap();
    assert_eq!(
        drain(&mut daemon),
        Vec::<u8>::new(),
        "taking the line over owes the wipe, it does not send it early"
    );

    press(&window, cx, "enter");
    cx.run_until_parked();
    assert_eq!(
        drain(&mut daemon),
        b"\x15echo\r".to_vec(),
        "the owed wipe is paid on submit, still in front of the line"
    );
}

/// The half of the window the seed alone does not cover: the editor is
/// live, so the user can *replace* the line before submitting it. Recalling
/// history and pressing Enter has to run the entry recalled — not that
/// entry with the text the shell was holding glued to its front.
#[gpui_kit::test]
fn recalling_history_in_the_gap_window_replaces_the_held_line(cx: &mut TestAppContext) {
    let (window, mut daemon) = harness(cx);
    typed_into_the_gap_then_handed_back(cx, &window, &mut daemon, "echo");
    window
        .update(cx, |view, _, _| {
            seed_history(view, vec!["echo from history".to_string()]);
        })
        .unwrap();

    press(&window, cx, "up");
    cx.run_until_parked();
    window
        .update(cx, |view, _, _| {
            assert_eq!(
                view.input.editor().text(),
                "echo from history",
                "the recall searches on the whole line, held text included"
            );
        })
        .unwrap();

    press(&window, cx, "enter");
    cx.run_until_parked();
    assert_eq!(
        drain(&mut daemon),
        b"\x15echo from history\r".to_vec(),
        "the recalled entry runs on its own, with the held text replaced \
         rather than prefixed to it"
    );
}

/// A paste that landed in the gap is still a paste after the handover. The
/// record replays it into the editor, and a line that arrives there looking
/// typed is submitted raw through the shell's binding table (#660) — the
/// hole `GapHold::pasted` closed for the hold's own route.
#[gpui_kit::test]
fn a_paste_held_in_the_gap_is_still_a_paste_after_the_handover(cx: &mut TestAppContext) {
    let (window, mut daemon) = harness(cx);
    // Bracketed paste is what a live prompt advertises; without it there is
    // no framing to lose in the first place.
    DaemonMsg::Output(b"\x1b[?2004h".to_vec())
        .encode(&mut daemon)
        .unwrap();
    settle(cx, &window, "the shell turns bracketed paste on", |view| {
        view.terminal
            .term
            .lock()
            .mode()
            .contains(Mode::BRACKETED_PASTE)
    });

    prompt(&mut daemon, true);
    DaemonMsg::Output(b"\x1b]133;B\x07".to_vec())
        .encode(&mut daemon)
        .unwrap();
    settle(cx, &window, "the editor takes the first prompt", |view| {
        view.input_active() && view.terminal.zle_reading()
    });
    prompt(&mut daemon, false);
    DaemonMsg::Output(b"\x1b]133;C\x07".to_vec())
        .encode(&mut daemon)
        .unwrap();
    settle(cx, &window, "a command takes the pane", |view| {
        !view.input_active()
    });

    window
        .update(cx, |view, _, cx| view.paste("echo hi".to_string(), cx))
        .unwrap();
    cx.executor().advance_clock(HOLD_WINDOW * 2);
    cx.run_until_parked();
    assert_eq!(
        drain(&mut daemon),
        b"\x1b[200~echo hi\x1b[201~".to_vec(),
        "the hold window gives up and dumps the paste as a paste"
    );

    prompt(&mut daemon, true);
    settle(cx, &window, "the editor takes the prompt back", |view| {
        view.input_active()
    });

    press(&window, cx, "enter");
    cx.run_until_parked();
    assert_eq!(
        drain(&mut daemon),
        b"\x15\x1b[200~echo hi\x1b[201~\r".to_vec(),
        "the replayed line keeps its framing instead of being typed at the \
         shell's binding table"
    );
}

/// The same for an emptied line: ⌃U clears what the editor is holding, and
/// the shell's copy of it goes too instead of coming back at submit.
#[gpui_kit::test]
fn clearing_the_line_in_the_gap_window_clears_the_held_text_too(cx: &mut TestAppContext) {
    let (window, mut daemon) = harness(cx);
    typed_into_the_gap_then_handed_back(cx, &window, &mut daemon, "echo");

    press(&window, cx, "ctrl-u");
    cx.run_until_parked();
    window
        .update(cx, |view, _, _| assert_eq!(view.input.editor().text(), ""))
        .unwrap();

    press(&window, cx, "enter");
    cx.run_until_parked();
    assert_eq!(
        drain(&mut daemon),
        b"\x15\r".to_vec(),
        "an emptied line submits empty: the wipe is still owed, the seed is not"
    );
}

/// #1131: a wheel turned over one part of a TUI scrolled whichever part
/// was last clicked. The wheel report carried the cell of the last button
/// press, and only a click — or motion, for the few programs that ask for
/// every move — ever updated it. tmux asks for button events only (1002),
/// so after clicking its status line every wheel notch over the panes
/// above was reported as being over the status line.
#[gpui_kit::test]
fn a_wheel_report_names_the_cell_under_the_pointer_not_the_last_click(cx: &mut TestAppContext) {
    let (window, mut daemon) = harness(cx);
    output(&mut daemon, b"\x1b[?1002h\x1b[?1006h");
    settle(cx, &window, "the program asked for the mouse", |view| {
        view.mouse_mode()
    });

    window
        .update(cx, |view, w, cx| {
            view.cell_geom = Some(super::super::element::CellGeom {
                origin: gpui_kit::point(px(0.), px(0.)),
                cell_width: px(10.),
                line_height: px(20.),
                cols: 80,
                rows: 24,
            });
            // A click on the bottom row, where tmux keeps its window list.
            view.mouse_press(MouseButton::Left, 3, 23, &Modifiers::default());
            view.mouse_release(MouseButton::Left, 3, 23, &Modifiers::default());
            // Then one notch up, with the pointer up in the pane content.
            let ev = ScrollWheelEvent {
                position: gpui_kit::point(px(105.), px(110.)),
                delta: gpui_kit::ScrollDelta::Lines(gpui_kit::point(0., 1.)),
                ..Default::default()
            };
            view.on_scroll(&ev, w, cx);
        })
        .unwrap();
    cx.run_until_parked();
    assert_eq!(
        String::from_utf8(drain(&mut daemon)).unwrap(),
        "\x1b[<0;4;24M\x1b[<0;4;24m\x1b[<64;11;6M",
        "the notch is reported at the cell the pointer is over"
    );
}

/// #1131: on Linux the candidate window only follows the caret if the
/// rectangle is pushed, so every move has to be pushed — once — and a
/// pane coming back into focus has to push again even if the caret never
/// moved, because another pane's anchor was the last one sent.
#[gpui_kit::test]
fn the_ime_anchor_is_pushed_when_it_moves_and_again_after_a_refocus(cx: &mut TestAppContext) {
    let (window, _daemon) = harness(cx);
    let cell = |col: f32, row: f32| {
        Some(gpui_kit::Bounds::new(
            gpui_kit::point(px(col * 10.), px(row * 20.)),
            gpui_kit::size(px(10.), px(20.)),
        ))
    };
    window
        .update(cx, |view, _, _| {
            assert!(view.ime_anchor_moved(cell(2., 0.)), "the first frame");
            assert!(!view.ime_anchor_moved(cell(2., 0.)), "nothing moved");
            assert!(view.ime_anchor_moved(cell(9., 4.)), "the TUI moved it");
            assert!(!view.ime_anchor_moved(None), "focus left: nothing to push");
            assert!(
                view.ime_anchor_moved(cell(9., 4.)),
                "focus came back to a caret that never moved"
            );
        })
        .unwrap();
}
