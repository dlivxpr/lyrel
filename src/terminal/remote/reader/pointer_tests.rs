use super::super::test_support::socket_pair;
use super::*;
use crate::daemon::protocol::DaemonMsg;
use gpui_kit::CursorStyle;

fn terminal() -> (RemoteTerminal, Stream) {
    crate::core::config::pin_test_config_dir();
    let (client_side, daemon_side) = socket_pair();
    let term = RemoteTerminal::from_stream_with(
        client_side,
        TermSize::new(80, 24),
        Vec::new(),
        PtySource::Raw,
    )
    .expect("a terminal over a socket pair");
    (term, daemon_side)
}

fn pointer(term: &RemoteTerminal) -> Option<CursorStyle> {
    term.presentation_facts().pointer
}

/// Waits for `ready` to describe this pane's pointer claim, then returns it.
/// The daemon-side stream is real transport, so the check must poll.
fn settled_pointer(term: &RemoteTerminal, want: Option<CursorStyle>) -> Option<CursorStyle> {
    for _ in 0..600 {
        if pointer(term) == want {
            return pointer(term);
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    pointer(term)
}

fn send(term_daemon: &mut Stream, bytes: &[u8]) {
    DaemonMsg::Output(bytes.to_vec())
        .encode(term_daemon)
        .unwrap();
}

/// Waits for a mark cell the batch wrote, so the parse in question is known
/// to have completed before the claim is read.
fn wait_for_cell(term: &RemoteTerminal, row: usize, col: usize, c: char) {
    use rio_vt::crosswords::pos::{Column, Line};
    for _ in 0..600 {
        if term.term.lock().grid[Line(row as i32)][Column(col)].c() == c {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!("the marker cell {c:?} never painted at ({row},{col})");
}

/// A named OSC 22 set lands on the pane's own paint-side facts through the
/// real parser; the platform shape for `pointer` is the pointing hand, and
/// an unsupported name still counts as a claim (its in-band fallback is the
/// arrow — not silence, and not the previous shape).
#[test]
fn a_named_shape_claims_the_pointer_and_an_unknown_name_degrades() {
    let (term, mut daemon) = terminal();
    send(&mut daemon, b"\x1b]22;pointer\x1b\\");
    assert_eq!(
        settled_pointer(&term, Some(CursorStyle::PointingHand)),
        Some(CursorStyle::PointingHand),
        "OSC 22 ; pointer must reach the pane's pointer claim"
    );
    send(&mut daemon, b"\x1b]22;help\x1b\\");
    assert_eq!(
        settled_pointer(&term, Some(CursorStyle::Arrow)),
        Some(CursorStyle::Arrow),
        "a name with no platform shape degrades to the arrow claim, not to silence"
    );
}

/// `OSC 22 ;` never reaches the parser's `set_mouse_cursor_icon` upstream —
/// it is the application's own release and the pane must honor it, at the
/// offset it occupied in the stream.
#[test]
fn the_empty_reset_releases_the_claim_at_stream_position() {
    let (term, mut daemon) = terminal();
    send(
        &mut daemon,
        b"\x1b]22;pointer\x1b\\\x1b]22;\x1b\\\x1b]22;ew-resize\x1b\\SETTLED",
    );
    assert_eq!(
        settled_pointer(&term, Some(CursorStyle::ResizeLeftRight)),
        Some(CursorStyle::ResizeLeftRight),
        "release-then-set in one batch leaves the later claim, not the reset"
    );
    wait_for_cell(&term, 0, 0, 'S');

    send(&mut daemon, b"\x1b]22;\x1b\\DONE");
    assert_eq!(
        settled_pointer(&term, None),
        None,
        "the empty reset releases the claim"
    );
}

/// A claim split across socket batches — terminator and all — still parses
/// and still releases; bytes that follow it are ordinary text.
#[test]
fn a_reset_split_across_batches_still_releases() {
    let (term, mut daemon) = terminal();
    send(&mut daemon, b"\x1b]22;pointer\x1b\\");
    assert_eq!(
        settled_pointer(&term, Some(CursorStyle::PointingHand)),
        Some(CursorStyle::PointingHand)
    );
    // Split the ST terminator between its ESC and backslash.
    send(&mut daemon, b"\x1b]22;\x1b");
    send(&mut daemon, b"\\AFTER");
    assert_eq!(
        settled_pointer(&term, None),
        None,
        "a split empty reset still releases"
    );
    wait_for_cell(&term, 0, 0, 'A');
    assert_eq!(
        term.term.lock().grid[rio_vt::crosswords::pos::Line(0)][rio_vt::crosswords::pos::Column(4)]
            .c(),
        'R',
        "AFTER parses as text, not as part of the sequence"
    );
}

/// Malformed and unsupported spellings do not touch the claim: a bare
/// `OSC 22` is not the reset (upstream needs a second param), and a
/// whitespace payload is an unrecognized name — both leave the existing
/// claim standing, and the text after them parses normally.
#[test]
fn malformed_and_unrecognized_payloads_leave_the_claim_alone() {
    let (term, mut daemon) = terminal();
    send(&mut daemon, b"\x1b]22;pointer\x1b\\");
    assert_eq!(
        settled_pointer(&term, Some(CursorStyle::PointingHand)),
        Some(CursorStyle::PointingHand)
    );
    send(&mut daemon, b"\x1b]22\x1b\\"); // no `;` at all: not the reset
    send(&mut daemon, b"\x1b]22; \x1b\\"); // unrecognized name stays a no-op
    // Basic set/reset deliberately does not interpret stack or query verbs.
    send(&mut daemon, b"\x1b]22;>text\x07\x1b]22;<\x07\x1b]22;?\x07");
    send(&mut daemon, b"\x1b]22;text;pointer\x07");
    send(&mut daemon, b"OK");
    wait_for_cell(&term, 0, 0, 'O');
    assert_eq!(
        pointer(&term),
        Some(CursorStyle::PointingHand),
        "bare and whitespace payloads change nothing"
    );
}

/// The shell's own mark ends the claim: `OSC 133;D` (command finished) is
/// the stream-position boundary the command's un-released claim dies at.
#[test]
fn a_finished_command_releases_the_claim_it_left() {
    let (term, mut daemon) = terminal();
    send(&mut daemon, b"\x1b]133;C\x07\x1b]22;pointer\x1b\\");
    assert_eq!(
        settled_pointer(&term, Some(CursorStyle::PointingHand)),
        Some(CursorStyle::PointingHand)
    );
    send(&mut daemon, b"\x1b]133;D\x07");
    assert_eq!(
        settled_pointer(&term, None),
        None,
        "133;D releases whatever the command claimed"
    );
}

/// The daemon's prompt frame is the shell-return release for panes whose
/// shell emits no 133;D — the pane's own `Prompt` bookkeeping already clears
/// a dead program's modes there, and the pointer claim goes with it.
#[test]
fn a_prompt_return_releases_the_claim() {
    let (term, mut daemon) = terminal();
    send(&mut daemon, b"\x1b]22;pointer\x1b\\");
    assert_eq!(
        settled_pointer(&term, Some(CursorStyle::PointingHand)),
        Some(CursorStyle::PointingHand)
    );
    DaemonMsg::Prompt {
        active: true,
        at_prompt: true,
        last_exit: Some(0),
    }
    .encode(&mut daemon)
    .unwrap();
    assert_eq!(
        settled_pointer(&term, None),
        None,
        "shell back at a prompt owns no pointer claim"
    );
}

/// A full reset releases the claim with the rest of the emulator state; the
/// bytes after RIS parse as ordinary text.
#[test]
fn ris_releases_the_claim() {
    let (term, mut daemon) = terminal();
    send(&mut daemon, b"\x1b]22;pointer\x1b\\");
    assert_eq!(
        settled_pointer(&term, Some(CursorStyle::PointingHand)),
        Some(CursorStyle::PointingHand)
    );
    send(&mut daemon, b"\x1bcRESETTING");
    assert_eq!(settled_pointer(&term, None), None, "RIS releases");
    wait_for_cell(&term, 0, 0, 'R');
}

/// A pane whose link dies keeps no claim: teardown is the disconnect
/// release, so the next owner does not inherit a dead application's shape.
#[test]
fn a_dead_link_releases_the_claim() {
    let (term, mut daemon) = terminal();
    send(&mut daemon, b"\x1b]22;pointer\x1b\\");
    assert_eq!(
        settled_pointer(&term, Some(CursorStyle::PointingHand)),
        Some(CursorStyle::PointingHand)
    );
    drop(daemon);
    assert_eq!(
        settled_pointer(&term, None),
        None,
        "the link's end releases what arrived over it"
    );
}

/// Replay restores state, not side effects: a claim the ring still holds is
/// the pane's current claim again on a fresh consumer, and a replayed
/// command boundary still ends it. Same bytes, same stream order.
#[test]
fn replay_restores_and_retires_the_claim_in_order() {
    let (term, mut daemon) = terminal();
    DaemonMsg::Snapshot(b"\x1b]22;pointer\x1b\\A".to_vec())
        .encode(&mut daemon)
        .unwrap();
    wait_for_cell(&term, 0, 0, 'A');
    assert_eq!(pointer(&term), Some(CursorStyle::PointingHand));
    DaemonMsg::Snapshot(b"\x1b]22;ew-resize\x1b\\\x1b]133;D\x07B".to_vec())
        .encode(&mut daemon)
        .unwrap();
    wait_for_cell(&term, 0, 1, 'B');
    assert_eq!(pointer(&term), None, "replay retires the claim in order");
}

#[test]
fn detaching_a_live_link_releases_its_pointer_before_returning() {
    let (mut term, mut daemon) = terminal();
    send(&mut daemon, b"\x1b]22;pointer\x1b\\");
    assert_eq!(
        settled_pointer(&term, Some(CursorStyle::PointingHand)),
        Some(CursorStyle::PointingHand)
    );
    term.detach_link();
    assert_eq!(pointer(&term), None, "a retired link owns no pointer");
}
