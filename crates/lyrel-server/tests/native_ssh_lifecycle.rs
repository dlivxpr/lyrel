//! Real native shell-channel lifetime, separate from routed remote-daemon PTYs.
#[path = "../../../tests/support/native_ssh_graphics.rs"]
mod fixture;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use fixture::{NativeSshFixture, STREAM_WITHIN, win_size};
use lyrel_core::client::PaneClient;
use lyrel_core::daemon::protocol::{self, ClientMsg, DaemonMsg, SshPhase};
use lyrel_core::daemon::transport::{self, Stream};

struct NativePane {
    endpoint: PathBuf,
    id: u64,
    stream: Stream,
}

impl NativePane {
    fn spawn(fixture: &NativeSshFixture) -> Self {
        let endpoint = fixture.local_config_dir().join(if cfg!(windows) {
            "daemon.port"
        } else {
            "daemon.sock"
        });
        let mut stream = transport::connect_endpoint_at(&endpoint).unwrap();
        stream.set_read_timeout(Some(STREAM_WITHIN)).unwrap();
        protocol::negotiate_pane(&mut stream).unwrap();
        ClientMsg::GraphicsCapacity {
            encoded_bytes: 64 << 20,
            decoded_bytes: 128 << 20,
            offers: false,
        }
        .encode(&mut stream)
        .unwrap();
        let mut spec = fixture.spec();
        // The literal ready marker never appears in terminal input echo.
        spec.login_script = vec!["stty -echo; printf 'LIFECYCLE_%s\\n' READY".into()];
        ClientMsg::SpawnNativeSsh {
            cwd: Some(fixture.remote_home().into()),
            size: win_size(),
            spec: Box::new(spec),
        }
        .encode(&mut stream)
        .unwrap();
        let id = match DaemonMsg::read(&mut stream).unwrap() {
            DaemonMsg::Spawned { pane_id } => pane_id,
            other => panic!("expected native shell-channel pane, got {other:?}"),
        };
        let mut pane = Self {
            endpoint,
            id,
            stream,
        };
        let mut output = Vec::new();
        loop {
            match pane.recv() {
                DaemonMsg::Output(bytes) | DaemonMsg::Snapshot(bytes) => output.extend(bytes),
                DaemonMsg::Exited { code } => {
                    panic!("native shell exited during startup: {code:?}")
                }
                _ => {}
            }
            if contains(&output, b"LIFECYCLE_READY") {
                return pane;
            }
        }
    }

    fn recv(&mut self) -> DaemonMsg {
        let message = DaemonMsg::read(&mut self.stream).expect("bounded real native SSH stream");
        match &message {
            DaemonMsg::AuthPrompt { .. } => panic!("fixed key/pin fixture must never prompt"),
            DaemonMsg::SshStatus {
                phase: SshPhase::Failed { reason },
            } => panic!("native SSH failed: {reason}"),
            DaemonMsg::Error(reason) => panic!("native pane failed: {reason}"),
            _ => {}
        }
        message
    }

    fn kill(&self) {
        let mut stream = transport::connect_endpoint_at(&self.endpoint).unwrap();
        protocol::negotiate_pane(&mut stream).unwrap();
        ClientMsg::Kill { pane_id: self.id }
            .encode(&mut stream)
            .unwrap();
        let deadline = Instant::now() + STREAM_WITHIN;
        while PaneClient::at(self.endpoint.clone())
            .list()
            .unwrap()
            .iter()
            .any(|pane| pane.pane_id == self.id && pane.alive)
        {
            assert!(
                Instant::now() < deadline,
                "native SSH pane remained alive after Kill"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn assert_no_competing_exit(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(1);
        self.stream
            .set_read_timeout(Some(Duration::from_millis(250)))
            .unwrap();
        while Instant::now() < deadline {
            match DaemonMsg::read(&mut self.stream) {
                Ok(DaemonMsg::Exited { .. }) => {
                    panic!("termination must not publish a competing Exited")
                }
                Ok(_) => {}
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                    ) => {}
                Err(_) => break,
            }
        }
    }
}

impl Drop for NativePane {
    fn drop(&mut self) {
        if let Ok(mut stream) = transport::connect_endpoint_at(&self.endpoint) {
            let _ = protocol::negotiate_pane(&mut stream);
            let _ = ClientMsg::Kill { pane_id: self.id }.encode(&mut stream);
        }
    }
}

fn contains(bytes: &[u8], marker: &[u8]) -> bool {
    bytes.windows(marker.len()).any(|part| part == marker)
}

#[test]
#[ignore = "requires explicit isolated SSH endpoint, public host pin, identity and Linux fixture; run alone"]
fn native_shell_channel_drains_final_output_and_intentional_shutdown_is_silent() {
    let fixture = NativeSshFixture::start(Path::new(env!("CARGO_BIN_EXE_lyrel-server")));
    let mut natural = NativePane::spawn(&fixture);
    ClientMsg::Input(b"printf 'LIFECYCLE_%s\\n' FINAL; exit 7\n".to_vec())
        .encode(&mut natural.stream)
        .unwrap();
    let mut output = Vec::new();
    loop {
        match natural.recv() {
            DaemonMsg::Output(bytes) | DaemonMsg::Snapshot(bytes) => output.extend(bytes),
            DaemonMsg::Exited { code } => {
                assert!(
                    contains(&output, b"LIFECYCLE_FINAL"),
                    "native bridge must drain before Exited"
                );
                // Native SSH has no local child status probe; do not invent one.
                assert_eq!(code, None);
                break;
            }
            _ => {}
        }
    }
    natural.assert_no_competing_exit();

    let mut intentional = NativePane::spawn(&fixture);
    intentional.kill();
    intentional.assert_no_competing_exit();
}
