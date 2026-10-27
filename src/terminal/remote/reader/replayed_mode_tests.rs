use super::super::test_support::socket_pair;
use super::*;
use lyrel_core::core::term_modes::TerminalModes;
use std::io::Write as _;

#[test]
fn a_replayed_mode_frame_puts_the_client_back_on_the_alternate_screen() {
    crate::core::config::pin_test_config_dir();
    // `btop`'s startup prefix, folded the way the daemon folds it out of
    // the pty — and then re-sent from the fold, the ring having dropped
    // the bytes themselves hours ago.
    let mut modes = TerminalModes::new();
    modes.feed(b"\x1b[?1049h\x1b[?1002h\x1b[?1006h");

    let (client_side, mut daemon_side) = socket_pair();
    let term = RemoteTerminal::from_stream(client_side, TermSize::new(80, 24)).unwrap();
    DaemonMsg::Snapshot(modes.restore_bytes().expect("a fold with modes in it"))
        .encode(&mut daemon_side)
        .unwrap();
    daemon_side.flush().unwrap();

    for _ in 0..600 {
        if term.term.lock().mode().contains(Mode::ALT_SCREEN) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }

    let mode = term.term.lock().mode();
    assert!(
        mode.contains(Mode::ALT_SCREEN),
        "the pane belongs on the alternate screen it never left: {mode:?}"
    );
    // Reporting first, SGR encoding with it: `wheel_route` sends the wheel
    // to the application on the first and encodes the report with the
    // second — see `wheel_routes_by_negotiated_mode_with_reporting_first`.
    assert!(
        mode.intersects(Mode::MOUSE_MODE),
        "mouse reporting is what keeps the wheel off the scrollback: {mode:?}"
    );
    assert!(mode.contains(Mode::SGR_MOUSE), "{mode:?}");
}

/// #1074, checked against the emulator itself rather than against our
/// reading of it: wherever the ring's front falls, the prelude plus what
/// the ring still holds has to leave the client's kitty keyboard flags
/// where the whole stream leaves them — and its stacks too, which only
/// show once the program pops or switches screens, so those are probed
/// after.
#[test]
fn a_replay_prelude_lands_the_kitty_keyboard_flags_where_the_stream_does() {
    use rio_vt::event::VoidListener;

    fn kitty_after(chunks: &[&[u8]], probes: &[&[u8]]) -> Vec<Mode> {
        let mut term = test_term(TermSize::new(40, 10));
        let mut processor = Processor::default();
        let mut keyboard = KeyboardStacks::default();
        for chunk in chunks {
            processor.advance(
                &mut ParseBridge::new(&mut term, VoidListener, &mut keyboard, None),
                chunk,
            );
        }
        let mut seen = vec![term.mode() & Mode::KITTY_KEYBOARD_PROTOCOL];
        for probe in probes {
            processor.advance(
                &mut ParseBridge::new(&mut term, VoidListener, &mut keyboard, None),
                probe,
            );
            seen.push(term.mode() & Mode::KITTY_KEYBOARD_PROTOCOL);
        }
        seen
    }

    let streams: [&[u8]; 5] = [
        // An agent CLI: one push at startup, a pop and push around a
        // subprocess later on.
        b"\x1b[>1uhello\r\n\x1b[<u$ ls\r\n\x1b[>1uback\r\n",
        b"\x1b[>1u\x1b[?1049h\x1b[>7uframe\x1b[=3uframe\x1b[>5u",
        b"\x1b[>1uidle\x1b[=0;1u\x1b[=1;2utyping",
        b"\x1b[>1u\x1b[>3u\x1b[?1049h\x1b[>7u\x1b[<u\x1b[?1049l\x1b[<u\x1b[>2u",
        b"\x1b[>1u\x1bc\x1b[>4uafter a reset",
    ];
    let probes: [&[u8]; 7] = [
        b"\x1b[<u",
        b"\x1b[?1049h",
        b"\x1b[<u",
        b"\x1b[?1049l",
        b"\x1b[<u",
        b"\x1b[<u",
        b"\x1b[>8u",
    ];
    for stream in streams {
        let whole = {
            let mut m = TerminalModes::new();
            m.feed(stream);
            m
        };
        let expected = kitty_after(&[stream], &probes);
        for cut in (0..=stream.len()).filter(|&i| i == stream.len() || stream[i] == 0x1b) {
            let (dropped, kept) = stream.split_at(cut);
            let (mut head, mut ring) = (TerminalModes::new(), TerminalModes::new());
            head.feed(dropped);
            ring.feed(kept);
            // A ring that starts inside an alternate screen it later leaves
            // is replayed onto the primary one — the DEC restore's own
            // limit, not the keyboard's; there is no stack to push it to.
            let restores_alt = whole.is_on(1049) && !ring.is_on(1049);
            if head.is_on(1049) && !restores_alt {
                continue;
            }
            let prelude = whole.replay_prelude(&head, &ring).unwrap_or_default();
            assert_eq!(
                kitty_after(&[&prelude, kept], &probes)
                    .iter()
                    .map(|mode| mode.bits())
                    .collect::<Vec<_>>(),
                expected.iter().map(|mode| mode.bits()).collect::<Vec<_>>(),
                "cut at {cut} of {:?}, prelude {:?}",
                String::from_utf8_lossy(stream),
                String::from_utf8_lossy(&prelude),
            );
        }
    }
}
