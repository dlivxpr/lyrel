use super::*;
use crate::terminal::remote::test_support::socket_pair;
use std::io::{Read as _, Write as _};

#[test]
fn a_slow_split_spawn_reply_keeps_its_real_pane_id() {
    let (mut client, mut daemon) = socket_pair();
    let cancellation = SpawnCancellation::default();
    let worker =
        std::thread::spawn(move || cancellable_spawn_reply(&mut client, &cancellation));
    let mut bytes = Vec::new();
    DaemonMsg::Spawned { pane_id: 42 }
        .encode(&mut bytes)
        .unwrap();
    // Longer than the old local request deadline. A partial header also
    // catches restarting frame decoding after a cancellation poll.
    daemon.write_all(&bytes[..2]).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(2200));
    daemon.write_all(&bytes[2..]).unwrap();
    assert_eq!(
        worker.join().unwrap().unwrap(),
        DaemonMsg::Spawned { pane_id: 42 }
    );
}

#[test]
fn closing_a_pending_slot_cancels_a_silent_spawn_reader() {
    let (mut client, mut daemon) = socket_pair();
    daemon
        .set_read_timeout(Some(std::time::Duration::from_secs(2)))
        .unwrap();
    let cancellation = SpawnCancellation::default();
    let worker_cancellation = cancellation.clone();
    let (started, ready) = std::sync::mpsc::channel();
    let (finished, done) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        started.send(()).unwrap();
        let result = cancellable_spawn_reply(&mut client, &worker_cancellation);
        finished.send(result.map(|_| ())).unwrap();
    });
    ready.recv().unwrap();
    cancellation.cancel();
    let error = done
        .recv_timeout(std::time::Duration::from_secs(2))
        .expect("cancellation must release the worker, not leave a silent read forever")
        .expect_err("cancelled creation is not a successful pane");
    assert!(
        !daemon_disconnected_before_spawn_reply(&error),
        "cancellation must not retry Spawn"
    );
    let mut byte = [0];
    assert_eq!(
        daemon.read(&mut byte).unwrap(),
        0,
        "cancellation closes only its Spawn connection"
    );
    worker.join().unwrap();
}

#[test]
fn an_explicit_spawn_failure_is_delivered_without_recovery() {
    let (mut client, mut daemon) = socket_pair();
    DaemonMsg::Error("configured shell missing".into())
        .encode(&mut daemon)
        .unwrap();
    let reply = cancellable_spawn_reply(&mut client, &SpawnCancellation::default()).unwrap();
    assert_eq!(reply, DaemonMsg::Error("configured shell missing".into()));
}

#[test]
fn only_landing_a_provisional_terminal_sends_its_matching_attach() {
    crate::core::config::pin_test_config_dir();
    let (client, mut daemon) = socket_pair();
    let mut terminal = RemoteTerminal::from_stream(client, TermSize::new(80, 24)).unwrap();
    terminal.connection.finish_spawn(PaneRoute::Local, Some((43, win_size(TermSize::new(80, 24), 8, 17))));
    terminal.claim_spawn();
    daemon
        .set_read_timeout(Some(std::time::Duration::from_secs(2)))
        .unwrap();
    assert!(matches!(
        ClientMsg::read(&mut daemon).unwrap(),
        ClientMsg::Attach { pane_id: 43, .. }
    ));
    terminal.claim_spawn();
    daemon
        .set_read_timeout(Some(std::time::Duration::from_millis(100)))
        .unwrap();
    assert!(
        ClientMsg::read(&mut daemon).is_err(),
        "a second landing cannot claim twice"
    );
}
