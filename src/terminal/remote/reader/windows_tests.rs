use super::*;

use super::super::test_support::socket_pair as tcp_pair;

/// On Windows, `shutdown()` does not wake a thread parked in a blocking
/// `read` on the same socket (it does on unix). `detach_link`,
/// `adopt_relink`, and `Drop` all shut the writer down and then `join()`
/// the reader thread, counting on that wake-up — so when the peer stays
/// silent (a routed pane whose SSH leg went zombie: nothing arrives, no
/// FIN ever comes), the join blocks its caller, which in production is
/// the UI thread. This is the whole-window "not responding" hang right
/// after a remote workspace reconnects.
#[test]
fn detach_link_returns_promptly_when_the_peer_stays_silent() {
    crate::core::config::pin_test_config_dir();
    let (client_side, daemon_side) = tcp_pair();
    let mut term = RemoteTerminal::from_stream(client_side, TermSize::new(80, 24)).unwrap();
    // Let the reader thread park in read() before tearing down.
    std::thread::sleep(std::time::Duration::from_millis(200));

    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        term.detach_link();
        let _ = tx.send(());
    });
    let outcome = rx.recv_timeout(std::time::Duration::from_secs(3));
    drop(daemon_side);
    assert!(
        outcome.is_ok(),
        "detach_link blocked for over 3s against a silent peer: shutdown() \
         did not unblock the reader, so join() hangs the calling thread"
    );
}

/// The reconnect path: `relink_panes` calls this on the UI thread right
/// after a remote workspace re-attaches, with the old link still parked
/// on a zombie route. It must swap links promptly, and the adopted link
/// must actually work.
#[test]
fn adopt_relink_swaps_links_promptly_when_the_old_peer_stays_silent() {
    crate::core::config::pin_test_config_dir();
    let (old_client, mut _old_daemon) = tcp_pair();
    let (new_client, mut new_daemon) = tcp_pair();
    let term = RemoteTerminal::from_stream(old_client, TermSize::new(80, 24)).unwrap();
    // Let the old reader park in read() on the silent link.
    std::thread::sleep(std::time::Duration::from_millis(200));

    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut term = term;
        term.adopt_relink(
            new_client,
            Vec::new(),
            &PaneRoute::Local,
            TermSize::new(80, 24),
            8,
            16,
        )
        .unwrap();
        let _ = tx.send(term);
    });
    let term = rx
        .recv_timeout(std::time::Duration::from_secs(3))
        .expect("adopt_relink blocked for over 3s against a silent old peer");

    DaemonMsg::Output(b"hi".to_vec())
        .encode(&mut new_daemon)
        .unwrap();
    let mut first = ' ';
    for _ in 0..200 {
        first = term.term.lock().grid[rio_vt::crosswords::pos::Line(0)]
            [rio_vt::crosswords::pos::Column(0)]
        .c();
        if first == 'h' {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(first, 'h', "output on the adopted link must reach the grid");
    assert!(
        !term.exited_flag.load(Ordering::SeqCst),
        "retiring the old reader must not mark the pane as exited"
    );

    // Late traffic on the abandoned link must be ignored wholesale: an
    // Exited frame processed by the retired reader would flip
    // `child_exited` and close the freshly adopted pane from under the
    // user. Give the retired reader a full quit-poll tick to (mis)handle
    // it before checking.
    let _ = DaemonMsg::Exited { code: Some(0) }.encode(&mut _old_daemon);
    std::thread::sleep(std::time::Duration::from_millis(700));
    assert!(
        !term.child_exited(),
        "an Exited frame on the abandoned link must not close the pane"
    );
    assert!(
        !term.exited_flag.load(Ordering::SeqCst),
        "the abandoned link must not tear the adopted pane down"
    );
}

#[test]
fn adopted_reader_still_places_and_deletes_after_retired_reader_finishes() {
    use lyrel_core::core::kitty_graphics::{GraphicsUpdate, Image, Placement, WireFormat};
    crate::core::config::pin_test_config_dir();
    let (old_client, mut old_daemon) = tcp_pair();
    let (new_client, mut new_daemon) = tcp_pair();
    let mut term = RemoteTerminal::from_stream(old_client, TermSize::new(80, 24)).unwrap();
    // Keep the real retired reader handle solely to establish its completed
    // teardown boundary. Decode scheduling itself is synchronized by the
    // focused real-decoder generation regression in images.rs.
    let old_reader = term.reader.take_thread();
    term.adopt_relink(
        new_client,
        Vec::new(),
        &PaneRoute::Local,
        TermSize::new(80, 24),
        8,
        16,
    )
    .unwrap();
    let send_image = |stream: &mut Stream, key, red| {
        let update = GraphicsUpdate::Resource {
            key,
            image: Arc::new(Image {
                width: 1,
                height: 1,
                format: WireFormat::Rgba,
                compressed: false,
                data: vec![red, 20, 30, 128],
            }),
        };
        let frame = update.encode_frame();
        assert!(term.images.admit_test_resource(&update, frame.len()));
        DaemonMsg::Graphics(frame.into()).encode(stream).unwrap();
        let mut placement = Placement::default();
        placement.key = key;
        placement.resource = key;
        placement.w = 1;
        placement.h = 1;
        placement.span_cols = 1;
        placement.span_rows = 1;
        placement.clip_bottom = 1;
        let update = GraphicsUpdate::Place(placement);
        assert!(term.images.admit_test_resource(&update, 0));
        DaemonMsg::Graphics(update.encode_frame().into())
            .encode(stream)
            .unwrap();
    };
    let wait_pixel = |term: &RemoteTerminal, red| {
        for _ in 0..200 {
            let images = term.images.snapshot();
            if images.len() == 1
                && images[0].data.as_ref().unwrap().as_bytes(0).unwrap() == [30, 20, red, 128]
            {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("adopted link did not decode its real image");
    };
    send_image(&mut new_daemon, 11, 90);
    wait_pixel(&term, 90);
    let _ = DaemonMsg::Graphics(
        GraphicsUpdate::Remove {
            placements: vec![11],
            resources: vec![11],
        }
        .encode_frame()
        .into(),
    )
    .encode(&mut old_daemon);
    drop(old_daemon);
    old_reader.join().unwrap();
    wait_pixel(&term, 90);
    send_image(&mut new_daemon, 12, 100);
    DaemonMsg::Graphics(
        GraphicsUpdate::Remove {
            placements: vec![11],
            resources: vec![11],
        }
        .encode_frame()
        .into(),
    )
    .encode(&mut new_daemon)
    .unwrap();
    wait_pixel(&term, 100);
    DaemonMsg::Graphics(
        GraphicsUpdate::Remove {
            placements: vec![12],
            resources: vec![12],
        }
        .encode_frame()
        .into(),
    )
    .encode(&mut new_daemon)
    .unwrap();
    DaemonMsg::Output(b"after_graphics_relink".to_vec())
        .encode(&mut new_daemon)
        .unwrap();
    for _ in 0..200 {
        if term.term.lock().grid.cursor.pos.col.0 == 21 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(
        term.images.snapshot().is_empty(),
        "new link deletion remains active"
    );
    assert_eq!(term.term.lock().grid.cursor.pos.col.0, 21);
}

/// What the daemon replays into a pane restored from a stored screen, in
/// the frames and the order it sends them: the dead pane's screen, then the
/// preamble, then the new shell's own first output.
fn replay_restore_into(daemon: &mut std::net::TcpStream, old_screen: &[u8], shell: &[u8]) {
    let size = WinSize {
        cols: 40,
        rows: 10,
        cell_w: 8,
        cell_h: 17,
    };
    DaemonMsg::Size(size).encode(daemon).unwrap();
    DaemonMsg::Snapshot(old_screen.to_vec())
        .encode(daemon)
        .unwrap();
    DaemonMsg::Size(size).encode(daemon).unwrap();
    DaemonMsg::Snapshot(crate::daemon::pane::restore_preamble(Some(
        "the shell below is new",
    )))
    .encode(daemon)
    .unwrap();
    DaemonMsg::Output(shell.to_vec()).encode(daemon).unwrap();
}

fn row(term: &RemoteTerminal, line: i32) -> String {
    use rio_vt::crosswords::pos::{Column, Line, Pos};
    let term = term.term.lock();
    term.bounds_to_string(
        Pos::new(Line(line), Column(0)),
        Pos::new(Line(line), Column(term.columns() - 1)),
    )
    .trim_end()
    .to_string()
}

/// A restored screen must not be sitting in the viewport when the new
/// shell's ConPTY starts drawing on it.
///
/// conhost addresses its own screen buffer absolutely — PSReadLine repaints
/// the line being typed with `ESC[6;20H` and conhost frames it the same way
/// — and that buffer starts blank with the cursor at the top-left. Restored
/// output is output conhost never produced: left on screen it shifts every
/// row conhost names, so the first keystroke repaints the input line on top
/// of the old text instead of at the prompt. The restored screen belongs in
/// scrollback, where it survives without claiming a row.
#[test]
fn a_restored_screen_leaves_the_new_shell_the_viewport_conpty_thinks_it_has() {
    crate::core::config::pin_test_config_dir();
    let (client_side, mut daemon_side) = tcp_pair();
    let term = RemoteTerminal::from_stream(client_side, TermSize::new(40, 10)).unwrap();

    // Five lines of the dead pane, then the shell painting its prompt the
    // way conhost does: at row 1 of a buffer it believes is blank.
    replay_restore_into(
        &mut daemon_side,
        b"line one\r\nline two\r\nline three\r\nline four\r\nline five\r\n",
        b"\x1b[?25l\x1b[1;1HPS C:\\> \x1b[?25h",
    );

    let mut top = String::new();
    for _ in 0..400 {
        top = row(&term, 0);
        if top.starts_with("PS C:") {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(
        top, "PS C:\\>",
        "the shell's prompt paints where conhost put it"
    );

    for line in 1..10 {
        assert_eq!(
            row(&term, line),
            "",
            "row {line} still holds restored output, so conhost and the client \
             disagree about which row is which: the next repaint of the input \
             line lands on the old screen instead of at the prompt"
        );
    }

    // Kept, not erased: `ESC[2J` on the primary screen scrolls the viewport
    // into history, so the screen the daemon restored is one scroll away.
    let depth = {
        use rio_vt::crosswords::grid::Dimensions as _;
        term.term.lock().grid.history_size() as i32
    };
    let history: Vec<String> = (-depth..0).map(|line| row(&term, line)).collect();
    for wanted in [
        "line one",
        "line two",
        "line three",
        "line four",
        "line five",
    ] {
        assert!(
            history.iter().any(|row| row == wanted),
            "{wanted:?} is not in the scrollback; the restored screen was erased \
             rather than scrolled away. History holds {history:?}"
        );
    }
}
