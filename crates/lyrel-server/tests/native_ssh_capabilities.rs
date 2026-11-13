//! The checked-in terminal producer runs on a Linux PTY reached through the
//! production native SSH router, not a desktop-local protocol simulation.
#[path = "../../../tests/support/native_ssh_graphics.rs"]
pub mod fixture;

use std::path::Path;

use fixture::{NativeSshFixture, win_size};
use lyrel_core::daemon::protocol::{ClientMsg, DaemonMsg, FEATURE_XTGETTCAP_OWNER};

#[test]
#[ignore = "requires an explicit isolated Linux SSH endpoint, pinned public host key, identity and compatible candidate server; run alone"]
fn native_ssh_xtgettcap_runs_the_checked_in_terminal_producer() {
    let fixture = NativeSshFixture::start(Path::new(env!("CARGO_BIN_EXE_lyrel-server")));
    let host = fixture.connect_control();
    let sender = fixture.install_script(
        &host,
        "terminal-capabilities.mjs",
        include_bytes!("../../../scripts/terminal-capabilities.mjs"),
        Vec::new(),
    );
    let (mut stream, endpoint) = fixture.connect_pane();
    assert!(
        endpoint.has_feature(FEATURE_XTGETTCAP_OWNER),
        "the actual remote endpoint must own capability replies"
    );
    eprintln!(
        "ACTUAL_SSH_ENDPOINT protocol={} build={} instance={}",
        endpoint.protocol, endpoint.build, endpoint.instance
    );
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
        other => panic!("remote producer did not spawn: {other:?}"),
    };
    fixture.track_pane(pane);
    let mut output = Vec::new();
    loop {
        match DaemonMsg::read(&mut stream).expect("bounded actual SSH producer read") {
            DaemonMsg::Output(bytes) | DaemonMsg::Snapshot(bytes) => {
                assert!(
                    output.len() + bytes.len() <= 2 << 20,
                    "bounded producer output"
                );
                output.extend_from_slice(&bytes);
            }
            DaemonMsg::Exited { code } => {
                assert_eq!(
                    code,
                    Some(0),
                    "producer assertions failed: {}",
                    String::from_utf8_lossy(&output)
                );
                break;
            }
            DaemonMsg::Error(error) => panic!("actual SSH producer failed: {error}"),
            _ => {}
        }
    }
    // Each marker follows the producer's exact-byte reply assertion; successful
    // process exit also proves its no-stray-input checks ran through completion.
    let output = String::from_utf8_lossy(&output);
    for outcome in [
        "PASS allowed + denied ordered batch",
        "PASS fragmented request and ST",
        "PASS malformed hex",
        "PASS CAN cancellation",
        "PASS SUB cancellation",
        "PASS interrupted DCS",
        "PASS no historical/duplicate replies",
        "PASS oversized request followed by ordinary text",
        "PASS legal query after oversize",
    ] {
        assert!(
            output.contains(outcome),
            "missing protocol outcome {outcome}: {output}"
        );
    }
}
