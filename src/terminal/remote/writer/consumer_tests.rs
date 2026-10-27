use super::MAX_BACKLOG;
use crate::terminal::remote::{RemoteTerminal, PaneEvent};
use crate::terminal::size::TermSize;
use crate::daemon::protocol::ClientMsg;
use std::sync::atomic::Ordering;
use std::os::unix::net::UnixStream;

#[test]
fn write_sends_input_frames() {
    let (client_side, mut daemon_side) = UnixStream::pair().unwrap();
    let term = RemoteTerminal::from_stream(client_side, TermSize::new(80, 24)).unwrap();

    term.write(Vec::<u8>::new());
    term.write(b"echo hi\r".to_vec());

    match ClientMsg::read(&mut daemon_side).unwrap() {
        ClientMsg::Input(bytes) => assert_eq!(bytes, b"echo hi\r"),
        other => panic!("expected Input, got {other:?}"),
    }
}

/// A peer that holds the link open and stops reading is what the router
/// looks like from here when the far end is congested: `copy_bidirectional`
/// stops draining our half and the send buffer fills. The socket is
/// blocking and has no write timeout, so `write(2)` parks — and it used to
/// park on the UI thread, which draws every window. macOS gives a unix
/// stream 8K, so it took about 1400 keystrokes, or one paste.
///
/// Typing into such a pane must now cost the window nothing at all.
#[test]
fn a_link_that_stopped_reading_does_not_park_the_writing_thread() {
    crate::core::config::pin_test_config_dir();
    let (client_side, daemon_side) = UnixStream::pair().unwrap();
    let term = RemoteTerminal::from_stream(client_side, TermSize::new(80, 24)).unwrap();

    // Far past the 8K that used to be fatal, one keystroke at a time, on
    // this very thread: a park anywhere in here is a frozen window.
    let started = std::time::Instant::now();
    for _ in 0..64 * 1024 {
        term.write(vec![b'x']);
    }
    let spent = started.elapsed();

    drop(daemon_side);
    assert!(
        spent < std::time::Duration::from_secs(1),
        "64K keystrokes into a link nobody is draining took {spent:?} — \
         the caller is a gpui event handler, so this is the UI thread"
    );
}

/// The backlog is bounded, and reaching the bound is not a slow link but a
/// dead one: nothing has taken a byte for four megabytes of typing. Saying
/// so is what stops the pane quietly swallowing input forever, and it is
/// said in the same words — and once — as an outright refused write.
#[test]
fn a_backlog_nothing_drains_is_reported_as_a_lost_link() {
    crate::core::config::pin_test_config_dir();
    let (client_side, daemon_side) = UnixStream::pair().unwrap();
    let term = RemoteTerminal::from_stream(client_side, TermSize::new(80, 24)).unwrap();

    // 8K a shot rather than a byte, so this is a few hundred frames and not
    // a few million: the bound is on bytes queued, not on frames.
    //
    // Twice the bound rather than a frame or two past it. The sender does
    // get some of this onto the wire before it parks — as much as the send
    // buffer holds, which is 8K on macOS but a couple hundred K on Linux —
    // and that much is discounted from the backlog. A margin narrower than
    // the widest of those buffers is a test that passes on one platform and
    // not the other.
    let chunk = vec![b'x'; 8 << 10];
    for _ in 0..2 * MAX_BACKLOG / chunk.len() {
        term.write(chunk.clone());
    }

    assert!(
        term.exited_flag.load(Ordering::SeqCst),
        "a link that has taken nothing for {MAX_BACKLOG} bytes is gone, and the pane must say so"
    );
    let mut exits = 0;
    while let Ok(ev) = term.events.try_recv() {
        exits += usize::from(matches!(ev, PaneEvent::Exit));
    }
    assert_eq!(exits, 1, "said once, not once per keystroke");
    drop(daemon_side);
}

/// One frame can be larger than the whole backlog bound: a paste is
/// whatever the clipboard holds. That is not a link nothing is draining,
/// and a pane must not die of being pasted into — nor may the keystrokes
/// that follow read as a backlog merely because the paste is still going.
#[test]
fn a_paste_larger_than_the_backlog_bound_is_not_a_dead_link() {
    crate::core::config::pin_test_config_dir();
    let (client_side, mut daemon_side) = UnixStream::pair().unwrap();
    let term = RemoteTerminal::from_stream(client_side, TermSize::new(80, 24)).unwrap();

    // A peer that keeps reading: the link is healthy, just carrying a lot.
    let draining = std::thread::spawn(move || {
        let _ = std::io::copy(&mut daemon_side, &mut std::io::sink());
    });

    term.write(vec![b'x'; MAX_BACKLOG + (1 << 20)]);
    for _ in 0..1024 {
        term.write(vec![b'y']);
    }

    assert!(
        !term.exited_flag.load(Ordering::SeqCst),
        "a five megabyte paste is a paste, not a link that has stopped taking input"
    );
    drop(term);
    draining.join().unwrap();
}

/// Input the link refuses used to vanish: `write` threw the error away, so a
/// pane whose daemon had stopped reading kept taking keystrokes into
/// nothing. The refusal now marks the pane exited by the reader's own signal
/// — only our sending half is shut here, so the reader is still parked on
/// an open receiving half and the writing side is the one that finds out.
/// (Shutting the peer's receiving half instead is not portable: Linux
/// answers the next write with EPIPE, macOS buffers it.)
///
/// The refusal is met on the sender thread now, so the pane learns of it a
/// moment after the keystroke rather than during it. That is the trade the
/// queue buys: the window never waits on the socket to find out.
#[test]
fn a_write_the_link_refuses_marks_the_pane_gone_once() {
    crate::core::config::pin_test_config_dir();
    let (client_side, _daemon_side) = UnixStream::pair().unwrap();
    let sending_socket = client_side.try_clone().unwrap();
    let term = RemoteTerminal::from_stream(client_side, TermSize::new(80, 24)).unwrap();
    sending_socket.shutdown(std::net::Shutdown::Write).unwrap();

    term.write(b"echo hi\r".to_vec());
    let noticed = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while !term.exited_flag.load(Ordering::SeqCst) && std::time::Instant::now() < noticed {
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(
        term.exited_flag.load(Ordering::SeqCst),
        "a refused Input is the link gone, and the pane has to say so"
    );
    let mut exits = 0;
    while let Ok(ev) = term.events.try_recv() {
        exits += usize::from(matches!(ev, PaneEvent::Exit));
    }
    assert_eq!(
        exits, 1,
        "reported through the same event the reader raises on EOF"
    );

    term.write(b"echo again\r".to_vec());
    while let Ok(ev) = term.events.try_recv() {
        exits += usize::from(matches!(ev, PaneEvent::Exit));
    }
    assert_eq!(exits, 1, "said once, not once per keystroke");
}

/// A link retired for a relink takes no more input — `stop_reader` closes
/// the queue and shuts the socket down — and that must not read as the pane
/// dying under the swap, for the same reason the retired reader exits
/// silently. The frame is now turned away at the queue rather than by the
/// socket, but the pane has to stay alive either way.
#[test]
fn a_write_on_a_retired_link_does_not_mark_the_pane_gone() {
    crate::core::config::pin_test_config_dir();
    let (client_side, _daemon_side) = UnixStream::pair().unwrap();
    let mut term = RemoteTerminal::from_stream(client_side, TermSize::new(80, 24)).unwrap();
    term.stop_reader();

    term.write(b"echo hi\r".to_vec());
    assert!(
        !term.exited_flag.load(Ordering::SeqCst),
        "the pane is being relinked or released, not dying"
    );
}
