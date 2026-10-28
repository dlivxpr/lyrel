use super::*;
use std::os::unix::net::UnixStream;
use std::sync::atomic::AtomicBool;

#[test]
fn a_lagged_connection_hears_a_resync_instead_of_the_superseded_backlog() {
    let (server_end, mut client_end) = UnixStream::pair().unwrap();
    let sink = Arc::new(Sink::new(server_end));
    let (tx, rx) = smol::channel::bounded::<(String, machine::LayoutDelta)>(8);
    let lagged = Arc::new(AtomicBool::new(false));

    for _ in 0..4 {
        tx.send_blocking(("ws-1".to_string(), machine::LayoutDelta::WorkspaceDeleted))
            .unwrap();
    }
    lagged.store(true, Ordering::Release);
    spawn_layout_forwarder(rx, sink, Arc::clone(&lagged));

    assert_eq!(
        ControlServerMsg::read(&mut client_end).unwrap(),
        ControlServerMsg::Event(ControlEvent::LayoutResync)
    );
    assert!(
        !lagged.load(Ordering::Acquire),
        "the flag is consumed: one gap, one resync"
    );

    tx.send_blocking(("ws-2".to_string(), machine::LayoutDelta::WorkspaceDeleted))
        .unwrap();
    match ControlServerMsg::read(&mut client_end).unwrap() {
        ControlServerMsg::Event(ControlEvent::Layout { workspace, delta }) => {
            assert_eq!(workspace, "ws-2", "the stale backlog was delivered anyway");
            assert_eq!(delta, machine::LayoutDelta::WorkspaceDeleted);
        }
        other => panic!("expected the post-resync delta, got {other:?}"),
    }
}
