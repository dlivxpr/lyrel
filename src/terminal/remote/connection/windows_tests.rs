use super::*;
use crate::daemon::protocol::DaemonMsg;
use crate::terminal::remote::RemoteTerminal;

use super::super::test_support::socket_pair as tcp_pair;

/// On Windows, `shutdown()` does not wake a thread parked in a blocking
/// `read` on the same socket (it does on unix). `detach_link`,
/// `adopt_relink`, and `Drop` used to shut the writer down and then `join()`
/// the reader thread, counting on that wake-up — so when the peer stayed
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
    let old_reader = term.connection.reader.take_thread();
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
