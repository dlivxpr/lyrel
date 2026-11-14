//! The checked-in terminal producer runs on a Linux PTY reached through the
//! production native SSH router, not a desktop-local protocol simulation.
#[path = "../../../tests/support/native_ssh_graphics.rs"]
pub mod fixture;

use std::path::Path;

use fixture::{NativeSshFixture, win_size};
use lyrel_core::daemon::protocol::{ClientMsg, DaemonMsg};

#[test]
#[ignore = "requires an explicit isolated Linux SSH endpoint, pinned public host key, identity and compatible candidate server; run alone"]
fn native_ssh_progress_survives_eviction_and_a_fresh_consumer_without_resending() {
    use lyrel_core::daemon::protocol::{PaneProgress, ProgressKind};

    let fixture = NativeSshFixture::start(Path::new(env!("CARGO_BIN_EXE_lyrel-server")));
    let host = fixture.connect_control();
    let sender = fixture.install_script(
        &host,
        "terminal-progress.mjs",
        include_bytes!("../../../scripts/terminal-progress.mjs"),
        Vec::new(),
    );
    let (mut stream, endpoint) = fixture.connect_pane();
    eprintln!("ACTUAL_PROGRESS_SSH_ENDPOINT {endpoint:?}");
    ClientMsg::Spawn {
        cwd: Some(fixture.remote_home().into()),
        size: win_size(),
        shell: Some(sender),
        owner: None,
        workspace: None,
        restore: None,
        allow_remote_clipboard_write: false,
        await_claim: false,
    }
    .encode(&mut stream)
    .unwrap();
    let pane = match DaemonMsg::read(&mut stream).unwrap() {
        DaemonMsg::Spawned { pane_id } => pane_id,
        other => panic!("remote progress producer did not spawn: {other:?}"),
    };
    fixture.track_pane(pane);
    let mut startup = Vec::new();
    while !startup
        .windows(b"q=quit".len())
        .any(|part| part == b"q=quit")
    {
        match DaemonMsg::read(&mut stream).unwrap() {
            DaemonMsg::Output(bytes) | DaemonMsg::Snapshot(bytes) => {
                assert!(startup.len() + bytes.len() < 64 << 10);
                startup.extend_from_slice(&bytes);
            }
            DaemonMsg::Error(error) => panic!("remote startup: {error}"),
            DaemonMsg::Exited { code } => panic!("remote startup exited: {code:?}"),
            _ => {}
        }
    }
    ClientMsg::Input(b"1".to_vec()).encode(&mut stream).unwrap();
    let expected = Some(PaneProgress::new(ProgressKind::Running, Some(42)));
    loop {
        match DaemonMsg::read(&mut stream).unwrap() {
            DaemonMsg::Progress(state) if state.is_some() => {
                assert_eq!(state, expected);
                break;
            }
            DaemonMsg::Exited { code } => panic!("progress producer exited: {code:?}"),
            DaemonMsg::Error(error) => panic!("remote progress: {error}"),
            _ => {}
        }
    }
    ClientMsg::Input(b"f".to_vec()).encode(&mut stream).unwrap();
    let marker = b"Progress bytes evicted; detach and reattach this live pane.";
    let mut tail = Vec::new();
    let mut produced = 0usize;
    while !tail.windows(marker.len()).any(|part| part == marker) {
        match DaemonMsg::read(&mut stream).unwrap() {
            DaemonMsg::Output(bytes) | DaemonMsg::Snapshot(bytes) => {
                produced += bytes.len();
                assert!(produced < 12 << 20, "finite producer flood");
                tail.extend_from_slice(&bytes);
                if tail.len() > 64 << 10 {
                    tail.drain(..tail.len() - 1024);
                }
            }
            DaemonMsg::Exited { code } => panic!("flood producer exited: {code:?}"),
            DaemonMsg::Error(error) => panic!("remote flood: {error}"),
            _ => {}
        }
    }
    assert!(
        produced > 8 << 20,
        "original progress bytes leave the real ring"
    );
    ClientMsg::Detach.encode(&mut stream).unwrap();
    drop(stream);
    let (mut stream, restored_endpoint) = fixture.connect_pane();
    assert_eq!(restored_endpoint.instance, endpoint.instance);
    ClientMsg::Attach {
        pane_id: pane,
        size: win_size(),
        allow_remote_clipboard_write: false,
    }
    .encode(&mut stream)
    .unwrap();
    let mut replay = Vec::new();
    loop {
        match DaemonMsg::read(&mut stream).unwrap() {
            DaemonMsg::Output(bytes) | DaemonMsg::Snapshot(bytes) => {
                assert!(
                    replay.len() + bytes.len() <= 9 << 20,
                    "bounded fresh replay"
                );
                replay.extend_from_slice(&bytes);
            }
            DaemonMsg::Progress(state) => {
                assert_eq!(state, expected, "restore current authority, not old OSC");
                break;
            }
            DaemonMsg::Exited { code } => panic!("same pane exited: {code:?}"),
            DaemonMsg::Error(error) => panic!("remote attach: {error}"),
            _ => {}
        }
    }
    assert!(replay.windows(marker.len()).any(|part| part == marker));
    assert!(
        !replay
            .windows(b"\x1b]9;4;1;42\x07".len())
            .any(|part| part == b"\x1b]9;4;1;42\x07")
    );
    ClientMsg::Input(b"q".to_vec()).encode(&mut stream).unwrap();
    let mut cleared = false;
    loop {
        match DaemonMsg::read(&mut stream).unwrap() {
            DaemonMsg::Progress(state) => {
                assert_eq!(state, None);
                cleared = true;
            }
            DaemonMsg::Exited { code } => {
                assert_eq!(code, Some(0));
                assert!(cleared, "explicit clear reaches the consumer before exit");
                break;
            }
            DaemonMsg::Error(error) => panic!("remote finish: {error}"),
            _ => {}
        }
    }
}
