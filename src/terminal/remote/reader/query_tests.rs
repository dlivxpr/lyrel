use super::*;
use std::io::Write as _;
use std::os::unix::net::UnixStream;

#[test]
fn snapshot_replay_suppresses_query_replies_and_side_effects() {
    crate::core::config::pin_test_config_dir();
    let (client_side, mut daemon_side) = UnixStream::pair().unwrap();
    let term = RemoteTerminal::from_stream(client_side, TermSize::new(80, 24)).unwrap();

    // Replaying queries or side effects must never send fresh input to a
    // later shell. Device-pixel queries are now owned solely by the daemon;
    // live cursor reports below still belong to the desktop terminal grid.
    DaemonMsg::Snapshot(
        b"\x1b[6n\x1b]11;?\x07\x1b[14t\x1b[16t\x1b[18t\x1b]52;c;aGk=\x07\x07replayed".to_vec(),
    )
    .encode(&mut daemon_side)
    .unwrap();
    daemon_side.flush().unwrap();

    let mut events = Vec::new();
    for _ in 0..200 {
        while let Ok(ev) = term.events.try_recv() {
            events.push(ev);
        }
        if events.iter().any(|e| matches!(e, PaneEvent::Wakeup)) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(
        events.iter().any(|e| matches!(e, PaneEvent::Wakeup)),
        "the replay's Wakeup should still arrive"
    );
    assert!(
        !events.iter().any(|e| matches!(
            e,
            PaneEvent::PtyWrite(_)
                | PaneEvent::ColorRequest(..)
                | PaneEvent::TextAreaSizeRequest(_)
                | PaneEvent::ClipboardStore(..)
                | PaneEvent::ClipboardLoad(..)
                | PaneEvent::Bell
        )),
        "replayed history must not re-answer queries or replay side effects"
    );

    DaemonMsg::Output(b"\x1b[6n".to_vec())
        .encode(&mut daemon_side)
        .unwrap();
    daemon_side.flush().unwrap();
    let mut got_reply = false;
    for _ in 0..200 {
        while let Ok(ev) = term.events.try_recv() {
            if matches!(ev, PaneEvent::PtyWrite(_)) {
                got_reply = true;
            }
        }
        if got_reply {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(got_reply, "live queries must still be answered");
}

#[test]
fn decrqm_probe_reports_sync_update_supported() {
    let (client_side, mut daemon_side) = UnixStream::pair().unwrap();
    let term = RemoteTerminal::from_stream(client_side, TermSize::new(80, 24)).unwrap();

    DaemonMsg::Output(b"\x1b[?2026$p".to_vec())
        .encode(&mut daemon_side)
        .unwrap();
    daemon_side.flush().unwrap();

    let mut reply = None;
    for _ in 0..200 {
        while let Ok(ev) = term.events.try_recv() {
            if let PaneEvent::PtyWrite(text) = ev {
                reply = Some(text);
            }
        }
        if reply.is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(
        reply.as_deref(),
        Some("\x1b[?2026;2$y"),
        "DECRQM ?2026 must be answered as supported (2 = reset)"
    );
}

#[test]
fn sync_update_without_esu_flushes_after_the_deadline() {
    let (client_side, mut daemon_side) = UnixStream::pair().unwrap();
    let term = RemoteTerminal::from_stream(client_side, TermSize::new(80, 24)).unwrap();

    DaemonMsg::Output(b"\x1b[?2026habc".to_vec())
        .encode(&mut daemon_side)
        .unwrap();
    daemon_side.flush().unwrap();

    let mut got = String::new();
    for _ in 0..600 {
        {
            let t = term.term.lock();
            let grid = &t.grid;
            got.clear();
            for col in 0..3usize {
                got.push(
                    grid[rio_vt::crosswords::pos::Line(0)]
                        [rio_vt::crosswords::pos::Column(col)]
                    .c(),
                );
            }
        }
        if got == "abc" {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(got, "abc", "dangling BSU must flush on the sync deadline");
}

#[test]
fn snapshot_replay_flushes_a_dangling_sync_frame_suppressed() {
    let (client_side, mut daemon_side) = UnixStream::pair().unwrap();
    let term = RemoteTerminal::from_stream(client_side, TermSize::new(80, 24)).unwrap();

    DaemonMsg::Snapshot(b"\x1b[?2026h\x1b[6nhi".to_vec())
        .encode(&mut daemon_side)
        .unwrap();
    daemon_side.flush().unwrap();

    let mut got = String::new();
    for _ in 0..200 {
        {
            let t = term.term.lock();
            let grid = &t.grid;
            got.clear();
            for col in 0..2usize {
                got.push(
                    grid[rio_vt::crosswords::pos::Line(0)]
                        [rio_vt::crosswords::pos::Column(col)]
                    .c(),
                );
            }
        }
        if got == "hi" {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(
        got, "hi",
        "the trapped replay tail must flush with the snapshot"
    );

    let mut events = Vec::new();
    while let Ok(ev) = term.events.try_recv() {
        events.push(ev);
    }
    assert!(
        !events.iter().any(|e| matches!(e, PaneEvent::PtyWrite(_))),
        "a query inside the replayed sync tail must stay suppressed"
    );
}
