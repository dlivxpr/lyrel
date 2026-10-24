use super::*;
use crate::client::{ControlClient, PaneClient, PaneSession};
use crate::daemon::control::ControlHello;
use crate::daemon::protocol::{ClientMsg, DaemonMsg, ShellSpec};
use std::path::PathBuf;
use std::time::{Duration, Instant};

mod fixture {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/support/kitty_pty.rs"
    ));
}

#[test]
#[ignore = "private production daemon child with deterministic retention limits"]
fn retention_fixture_daemon() {
    let Some(dir) = std::env::var_os("LYREL_RETENTION_TEST_DAEMON") else {
        return;
    };
    let metadata = std::env::var("LYREL_RETENTION_TEST_METADATA")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    retention::inject_limits(retention::Limits {
        pane_encoded: 1024,
        pane_decoded: 8,
        daemon_encoded: 4096,
        daemon_decoded: 12,
        pane_work: 256 << 10,
        daemon_work: 384 << 10,
        pane_metadata: if metadata != 0 {
            metadata
        } else {
            retention::MAX_PANE_METADATA_BYTES
        },
        daemon_metadata: if metadata != 0 {
            metadata
        } else {
            retention::MAX_DAEMON_METADATA_BYTES
        },
    });
    crate::core::config::set_config_dir(dir.into());
    crate::daemon::server::run_daemon().unwrap();
}

#[test]
#[ignore = "real PTY sender child for daemon retention regressions"]
fn retention_fixture_sender() {
    fixture::pty_sender();
}

struct Daemon {
    child: std::process::Child,
    dir: tempfile::TempDir,
}
impl Daemon {
    fn start() -> Self {
        Self::with_metadata_pressure(false)
    }
    fn with_metadata_pressure(metadata: bool) -> Self {
        Self::with_metadata_limit(if metadata { 4096 } else { 0 })
    }
    fn with_metadata_limit(metadata: usize) -> Self {
        let dir = tempfile::TempDir::new().unwrap();
        let child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--ignored",
                "--exact",
                "core::graphics_state::retention_tests::retention_fixture_daemon",
                "--nocapture",
                "--test-threads=1",
            ])
            // macOS re-exec appends its responsibility flag after libtest options.
            .arg("--")
            .env("LYREL_RETENTION_TEST_DAEMON", dir.path())
            .env("LYREL_RETENTION_TEST_METADATA", metadata.to_string())
            .env("LYREL_CONFIG_DIR", dir.path())
            .env("LYREL_DATA_DIR", dir.path())
            .env("LYREL_CONTROL_SOCK", dir.path().join("control.sock"))
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let daemon = Self { child, dir };
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if ControlClient::connect_at(
                &daemon.endpoint("control"),
                &ControlHello::host_rpc("retention-test", "retention-test"),
            )
            .is_ok()
                && daemon.panes().version().is_ok()
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "production daemon readiness deadline"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        daemon
    }
    fn endpoint(&self, name: &str) -> PathBuf {
        self.dir.path().join(format!(
            "{name}.{}",
            if cfg!(windows) { "port" } else { "sock" }
        ))
    }
    fn panes(&self) -> PaneClient {
        PaneClient::at(self.endpoint("daemon"))
    }
    fn spawn(&self) -> Consumer {
        let shell = ShellSpec {
            program: std::env::current_exe().unwrap().to_str().unwrap().into(),
            args: [
                "--ignored",
                "--exact",
                "core::graphics_state::retention_tests::retention_fixture_sender",
                "--nocapture",
                "--color=never",
                "--test-threads=1",
            ]
            .into_iter()
            .map(String::from)
            .collect(),
            args_are_lyrel_defaults: false,
        };
        let mut session = self
            .panes()
            .spawn(None, size(), Some(shell), None, None)
            .unwrap();
        session
            .set_recv_timeout(Some(Duration::from_secs(30)))
            .unwrap();
        let mut text = Vec::new();
        while !text.windows(11).any(|part| part == b"KITTY_READY") {
            if let DaemonMsg::Output(bytes) = session.recv().unwrap() {
                text.extend(bytes);
            }
        }
        Consumer {
            session,
            serial: 0,
            updates: Vec::new(),
            text: Vec::new(),
        }
    }
    fn capacity_attach(
        &self,
        pane: u64,
        decoded: u64,
        observe: bool,
    ) -> crate::daemon::transport::Stream {
        let mut stream =
            crate::daemon::transport::connect_endpoint_at(&self.endpoint("daemon")).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(30)))
            .unwrap();
        crate::daemon::protocol::negotiate_pane(&mut stream).unwrap();
        ClientMsg::GraphicsCapacity {
            encoded_bytes: 1024,
            decoded_bytes: decoded,
            offers: false,
        }
        .encode(&mut stream)
        .unwrap();
        let request = if observe {
            ClientMsg::Observe {
                pane_id: pane,
                size: size(),
            }
        } else {
            ClientMsg::Attach {
                pane_id: pane,
                size: size(),
                allow_remote_clipboard_write: false,
            }
        };
        request.encode(&mut stream).unwrap();
        stream
    }
}
impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
fn size() -> WinSize {
    WinSize {
        cols: 80,
        rows: 24,
        cell_w: 10,
        cell_h: 20,
    }
}

struct Consumer {
    session: PaneSession,
    serial: usize,
    updates: Vec<GraphicsUpdate>,
    text: Vec<u8>,
}
impl Consumer {
    fn exchange(&mut self, bytes: &[u8], replies: usize) -> Vec<Vec<u8>> {
        self.serial += 1;
        self.updates.clear();
        self.text.clear();
        self.session
            .input(format!("SEND {} {replies} {} 0\n", self.serial, fixture::hex(bytes)).as_bytes())
            .unwrap();
        self.finish_exchange(bytes)
    }

    fn finish_exchange(&mut self, bytes: &[u8]) -> Vec<Vec<u8>> {
        let marker = format!("KITTY_DONE_{}\r", self.serial);
        loop {
            match self.session.recv().unwrap_or_else(|error| {
                panic!(
                    "pane {} exchange {} failed for {:?}: {error}",
                    self.session.pane_id(),
                    self.serial,
                    String::from_utf8_lossy(&bytes[..bytes.len().min(160)])
                )
            }) {
                DaemonMsg::Graphics(frame) => self
                    .updates
                    .push(GraphicsUpdate::decode_frame_owned(frame.into_vec()).unwrap()),
                DaemonMsg::Snapshot(bytes) | DaemonMsg::Output(bytes) => {
                    self.text.extend(bytes);
                    if self
                        .text
                        .windows(marker.len())
                        .any(|part| part == marker.as_bytes())
                    {
                        break;
                    }
                }
                DaemonMsg::Exited { code } => panic!("sender exited {code:?}"),
                _ => {}
            }
        }
        // Snapshot is real terminal history, including older request logs.
        // Only this request's tagged transcript is a fresh protocol response.
        let prefix = format!("KITTY_REPLY {} ", self.serial);
        std::str::from_utf8(&self.text)
            .unwrap()
            .lines()
            .filter_map(|line| {
                line.trim()
                    .strip_prefix(prefix.as_str())
                    .map(fixture::unhex)
            })
            .collect()
    }
    fn exit(&mut self) {
        self.serial += 1;
        self.session
            .input(format!("EXIT {} 0 00 0\n", self.serial).as_bytes())
            .unwrap();
        loop {
            if matches!(
                self.session
                    .recv()
                    .expect("the real child must report its ordered exit"),
                DaemonMsg::Exited { .. }
            ) {
                break;
            }
        }
    }

    fn red(&mut self, control: &str) -> u64 {
        assert_eq!(
            self.exchange(
                format!("\x1b_G{control},f=32,s=1,v=1,q=0;/wAA/w==\x1b\\").as_bytes(),
                1
            )
            .len(),
            1
        );
        self.updates
            .iter()
            .find_map(|update| match update {
                GraphicsUpdate::Resource { key, .. } => Some(*key),
                _ => None,
            })
            .expect("accepted source")
    }
    fn idle(&mut self, bytes: &[u8]) {
        let replies = self.exchange(bytes, 0);
        assert!(
            replies.is_empty(),
            "quiet request {} received unexpected replies: {replies:?}",
            self.serial
        );
    }
    fn eviction(&mut self) -> GraphicsUpdate {
        loop {
            if let DaemonMsg::Graphics(frame) = self.session.recv().unwrap() {
                let update = GraphicsUpdate::decode_frame_owned(frame.into_vec()).unwrap();
                if matches!(update, GraphicsUpdate::Evicted { .. }) {
                    return update;
                }
            }
        }
    }
}

#[test]
fn retention_daemon_protects_live_direct_and_placeholder_evicts_global_oldest_and_releases() {
    let daemon = Daemon::start();
    let (mut a, mut b) = (daemon.spawn(), daemon.spawn());
    let oldest = a.red("a=T,i=1,c=1,r=1,C=1");
    a.idle(&b"\r\n".repeat(40));
    let placeholder = b.red("a=T,i=2,p=1,U=1,c=1,r=1");
    b.idle("\x1b[6;5H\x1b[38;5;2;58;5;1m\u{10eeee}\u{305}\u{305}\x1b[0m".as_bytes());
    let newer = a.red("a=T,i=3,c=1,r=1,C=1");
    a.idle(&b"\r\n".repeat(40));
    b.red("a=T,i=4,c=1,r=1,C=1");
    assert_eq!(
        a.eviction(),
        GraphicsUpdate::Evicted {
            key: oldest,
            width: 1,
            height: 1
        }
    );
    a.red("a=T,i=5,c=1,r=1,C=1");
    assert!(a.updates.iter().any(|update| *update
        == GraphicsUpdate::Evicted {
            key: newer,
            width: 1,
            height: 1
        }));
    let pane = a.session.pane_id();
    a.session.detach().unwrap();
    let detached = fixture::hex(b"\x1b_Ga=T,i=99,f=32,s=1,v=1,C=1;/wAA/w==\x1b\\detached_tail");
    daemon
        .panes()
        .send_input(pane, format!("SEND 1000 1 {detached} 0\n").as_bytes())
        .unwrap();
    a.session = daemon.panes().attach(pane, size()).unwrap();
    a.session
        .set_recv_timeout(Some(Duration::from_secs(30)))
        .unwrap();
    a.updates.clear();
    a.text.clear();
    a.serial = 1000;
    let detached_reply = a.finish_exchange(b"detached i=99 transfer");
    // The old response may be in Snapshot or still arriving live. Once its
    // PTY barrier completes, new input cannot be mistaken for that response.
    // Retain both phases so the fresh marker also fences all graphics replay.
    a.serial += 1;
    a.session
        .input(
            format!(
                "SEND {} 0 {} 0\n",
                a.serial,
                fixture::hex(b"reconnected_shell")
            )
            .as_bytes(),
        )
        .unwrap();
    assert!(a.finish_exchange(b"reconnected_shell").is_empty());
    assert!(
        detached_reply
            .iter()
            .any(|reply| reply.starts_with(b"\x1b_Gi=99;ENOSPC:"))
    );
    assert!(
        a.text
            .windows(b"detached_tail".len())
            .any(|part| part == b"detached_tail")
    );
    assert!(
        a.updates
            .iter()
            .any(|u| matches!(u, GraphicsUpdate::Evicted { key, .. } if *key == oldest))
    );
    let evicted_index = a
        .updates
        .iter()
        .position(|u| matches!(u, GraphicsUpdate::Evicted { key, .. } if *key == oldest))
        .unwrap();
    let placement_index = a
        .updates
        .iter()
        .position(|u| matches!(u, GraphicsUpdate::Place(p) if p.resource == oldest))
        .unwrap();
    assert!(
        evicted_index < placement_index,
        "replay seeds pixel-free identity before any retained position"
    );
    let refused = b.exchange(
        b"\x1b_Ga=T,i=6,f=32,s=1,v=1,C=1;/wAA/w==\x1b\\usable_shell_text",
        1,
    );
    assert!(refused[0].starts_with(b"\x1b_Gi=6;ENOSPC:"));
    assert!(!b.updates.iter().any(|u| matches!(
        u,
        GraphicsUpdate::Resource { .. } | GraphicsUpdate::Evicted { .. }
    )));
    assert!(b.text.windows(17).any(|part| part == b"usable_shell_text"));
    b.idle(b"\x1b[6;1H\x1b[2K");
    b.red("a=T,i=6,c=1,r=1,C=1");
    assert!(b.updates.iter().any(|u| matches!(u, GraphicsUpdate::Remove { resources, .. } if resources.contains(&placeholder))));
    assert!(
        !b.updates
            .iter()
            .any(|u| matches!(u, GraphicsUpdate::Evicted { key, .. } if *key == placeholder))
    );
    a.idle(b"\x1b_Ga=d,d=I,i=1\x1b\\");
    assert!(a.updates.iter().any(
        |u| matches!(u, GraphicsUpdate::Remove { resources, .. } if resources.contains(&oldest))
    ));
    assert!(
        !a.updates
            .iter()
            .any(|u| matches!(u, GraphicsUpdate::Evicted { .. }))
    );
    b.idle(b"\x1b[?1049h");
    let inactive = b.exchange(b"\x1b_Ga=T,i=9,f=32,s=1,v=1;/wAA/w==\x1b\\", 1);
    assert!(
        inactive[0].starts_with(b"\x1b_Gi=9;ENOSPC:"),
        "inactive main screen remains protected"
    );
    b.exit();
    daemon.panes().kill(b.session.pane_id()).unwrap();
    a.red("a=T,i=10,c=1,r=1,C=1");
    daemon.panes().kill(a.session.pane_id()).unwrap();
}

#[test]
fn retention_daemon_capacity_rejects_inadequate_attach_and_refuses_new_input() {
    let daemon = Daemon::start();
    let mut a = daemon.spawn();
    a.red("a=T,i=1,c=1,r=1,C=1");
    let mut too_small = daemon.capacity_attach(a.session.pane_id(), 0, false);
    assert!(matches!(
        DaemonMsg::read(&mut too_small).unwrap(),
        DaemonMsg::Error(_)
    ));
    let mut observer = daemon.capacity_attach(a.session.pane_id(), 4, true);
    let mut observed_source = false;
    while !observed_source {
        match DaemonMsg::read(&mut observer).unwrap() {
            DaemonMsg::Graphics(frame) => {
                observed_source = matches!(
                    GraphicsUpdate::decode_frame_owned(frame.into_vec()),
                    Some(GraphicsUpdate::Resource { .. })
                )
            }
            DaemonMsg::Error(error) => panic!("observer capacity: {error}"),
            _ => {}
        }
    }
    let reply = a.exchange(b"\x1b_Ga=T,i=2,f=32,s=1,v=1,C=1;/wAA/w==\x1b\\", 1);
    assert!(reply[0].starts_with(b"\x1b_Gi=2;ENOSPC:"));
    drop(observer);
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let reply = a.exchange(b"\x1b_Ga=T,i=2,f=32,s=1,v=1,C=1;/wAA/w==\x1b\\", 1);
        if reply[0] == b"\x1b_Gi=2;OK\x1b\\" {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "released observer credit must no longer constrain controller"
        );
    }
    daemon.panes().kill(a.session.pane_id()).unwrap();
}

#[test]
fn retention_daemon_partial_pressure_cancel_quiet_and_reset_preserve_shell() {
    let daemon = Daemon::start();
    let mut consumer = daemon.spawn();
    let accepted = consumer.red("a=T,i=1,c=1,r=1,C=1");
    consumer.red("a=T,i=2,c=1,r=1,C=1");
    consumer.idle(b"\x1b_Ga=T,i=3,f=32,s=1,v=1,q=2;/wAA/w==\x1b\\quiet_tail");
    assert!(
        !consumer
            .updates
            .iter()
            .any(|u| matches!(u, GraphicsUpdate::Resource { .. }))
    );
    let quiet_error = consumer.exchange(b"\x1b_Ga=T,i=3,f=32,s=1,v=1,q=1;/wAA/w==\x1b\\", 1);
    assert!(quiet_error[0].starts_with(b"\x1b_Gi=3;ENOSPC:"));
    let replacement = consumer.exchange(b"\x1b_Ga=T,i=1,f=32,s=2,v=1,C=1;/wAA//8AAP8=\x1b\\", 1);
    assert!(replacement[0].starts_with(b"\x1b_Gi=1;ENOSPC:"));
    assert!(!consumer.updates.iter().any(
        |u| matches!(u, GraphicsUpdate::Remove { resources, .. } if resources.contains(&accepted))
    ));
    let mut oversized = b"\x1b_Ga=t,i=7,f=32,s=8192,v=1;".to_vec();
    oversized.extend(std::iter::repeat_n(b'A', 350_000));
    oversized.extend_from_slice(b"\x1b\\after_work_refusal");
    let reply = consumer.exchange(&oversized, 1);
    assert!(reply[0].starts_with(b"\x1b_Gi=7;ENOSPC:"));
    assert!(
        consumer
            .text
            .windows(18)
            .any(|part| part == b"after_work_refusal")
    );
    consumer.idle(b"\x1b_Ga=t,i=8,f=32,s=1,v=1,m=1;/wAA\x1b\\");
    consumer.idle(b"\x1b_Ga=d,d=I,i=8\x1b\\");
    let continuation = consumer.exchange(b"\x1b_Gm=0,q=0;/w==\x1b\\", 0);
    assert!(continuation.is_empty());
    assert!(
        !consumer
            .updates
            .iter()
            .any(|u| matches!(u, GraphicsUpdate::Resource { .. }))
    );
    consumer.idle(b"\x1bc");
    assert!(
        !consumer
            .updates
            .iter()
            .any(|u| matches!(u, GraphicsUpdate::Evicted { .. }))
    );
    assert!(consumer.updates.iter().any(
        |u| matches!(u, GraphicsUpdate::Remove { resources, .. } if resources.contains(&accepted))
    ));
    consumer.red("a=T,i=9,c=1,r=1,C=1");
    daemon.panes().kill(consumer.session.pane_id()).unwrap();
}

#[test]
fn retention_daemon_historical_placeholder_notice_expires_without_touching_text() {
    let daemon = Daemon::start();
    let (mut a, mut b) = (daemon.spawn(), daemon.spawn());
    let old = a.red("a=T,i=7,p=1,U=1,c=1,r=1");
    a.idle("\x1b[6;5H\x1b[38;5;7;58;5;1m\u{10eeee}\u{305}\u{305}\x1b[0m".as_bytes());
    a.idle(&b"\r\n".repeat(40));
    b.red("a=T,i=2,c=1,r=1,C=1");
    a.red("a=T,i=3,c=1,r=1,C=1");
    b.red("a=T,i=4,c=1,r=1,C=1");
    assert_eq!(
        a.eviction(),
        GraphicsUpdate::Evicted {
            key: old,
            width: 1,
            height: 1
        }
    );
    a.idle(b"\x1b[3J");
    assert!(a.updates.iter().any(
        |u| matches!(u, GraphicsUpdate::Remove { resources, .. } if resources.contains(&old))
    ));
    assert!(
        !a.updates
            .iter()
            .any(|u| matches!(u, GraphicsUpdate::Evicted { .. }))
    );
    let pane = a.session.pane_id();
    a.session.detach().unwrap();
    a.session = daemon.panes().attach(pane, size()).unwrap();
    a.session
        .set_recv_timeout(Some(Duration::from_secs(30)))
        .unwrap();
    a.idle(b"history_clear_shell");
    assert!(
        !a.updates
            .iter()
            .any(|u| matches!(u, GraphicsUpdate::Evicted { key, .. } if *key == old))
    );
    assert!(
        !a.updates
            .iter()
            .any(|u| matches!(u, GraphicsUpdate::Place(p) if p.resource == old))
    );
    daemon.panes().kill(pane).unwrap();
    daemon.panes().kill(b.session.pane_id()).unwrap();
}

fn offer_observer(daemon: &Daemon, pane: u64) -> crate::daemon::transport::Stream {
    let mut stream =
        crate::daemon::transport::connect_endpoint_at(&daemon.endpoint("daemon")).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(30)))
        .unwrap();
    crate::daemon::protocol::negotiate_pane(&mut stream).unwrap();
    ClientMsg::GraphicsCapacity {
        encoded_bytes: 1024,
        decoded_bytes: 8,
        offers: true,
    }
    .encode(&mut stream)
    .unwrap();
    ClientMsg::Observe {
        pane_id: pane,
        size: size(),
    }
    .encode(&mut stream)
    .unwrap();
    stream
}

#[test]
fn retention_daemon_offer_replay_refusal_live_cancel_and_pressure_keep_authority() {
    use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
    let daemon = Daemon::start();
    let mut a = daemon.spawn();
    let first = a.red("a=T,i=1,c=1,r=1,C=1");
    // Replay cannot send payloads or install an observer before an actual grant.
    let mut refused = offer_observer(&daemon, a.session.pane_id());
    loop {
        match DaemonMsg::read(&mut refused).unwrap() {
            DaemonMsg::GraphicsOffer {
                token,
                key,
                width,
                height,
                encoded_bytes,
                metadata_bytes,
            } => {
                assert_eq!(metadata_bytes, GRAPHICS_METADATA_BYTES as u64);
                assert_eq!((key, width, height), (first, 1, 1));
                assert!(
                    encoded_bytes > 4,
                    "source lease includes the actual packet allocation"
                );
                ClientMsg::GraphicsGrant {
                    token,
                    accepted: false,
                }
                .encode(&mut refused)
                .unwrap();
            }
            DaemonMsg::Graphics(_) => panic!("ungranted replay leaked pixels"),
            DaemonMsg::Error(_) => break,
            _ => {}
        }
    }
    drop(refused);
    let mut observer = offer_observer(&daemon, a.session.pane_id());
    let (offered, offers) = std::sync::mpsc::channel();
    let (cancelled, cancellations) = std::sync::mpsc::channel();
    let mode = Arc::new(AtomicU8::new(1));
    let mode_reader = mode.clone();
    let ready = Arc::new(AtomicBool::new(false));
    let ready_reader = ready.clone();
    let mut writer = observer.try_clone().unwrap();
    let worker = std::thread::spawn(move || {
        while let Ok(message) = DaemonMsg::read(&mut observer) {
            match message {
                DaemonMsg::GraphicsOffer {
                    token,
                    key,
                    encoded_bytes,
                    ..
                } => {
                    offered.send((token, key)).unwrap();
                    let mode = mode_reader.load(Ordering::Acquire);
                    if mode != 2 {
                        ClientMsg::GraphicsGrant {
                            token,
                            accepted: mode == 1 || (mode == 3 && encoded_bytes != 0),
                        }
                        .encode(&mut observer)
                        .unwrap();
                    }
                }
                DaemonMsg::GraphicsCancel { token } => {
                    let _ = cancelled.send(token);
                }
                DaemonMsg::Graphics(frame) => {
                    if matches!(GraphicsUpdate::decode_frame_owned(frame.into_vec()), Some(GraphicsUpdate::Resource { key, .. }) if key == first)
                    {
                        ready_reader.store(true, Ordering::Release);
                    }
                }
                _ => {}
            }
        }
    });
    assert_eq!(
        offers.recv_timeout(Duration::from_secs(30)).unwrap().1,
        first
    );
    let deadline = Instant::now() + Duration::from_secs(30);
    while !ready.load(Ordering::Acquire) {
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
    // An observer cannot force reclamation of accepted current pixels.
    ClientMsg::GraphicsPressure { key: first }
        .encode(&mut writer)
        .unwrap();
    a.idle(b"visible_pressure_barrier");
    assert!(!a.updates.iter().any(|u| matches!(
        u,
        GraphicsUpdate::Evicted { .. } | GraphicsUpdate::Remove { .. }
    )));
    mode.store(3, Ordering::Release);
    let metadata_only = a.exchange(b"\x1b_Ga=p,i=1,p=9,c=1,r=1,C=1\x1b\\", 1);
    assert!(metadata_only[0].ends_with(b"Desktop graphics metadata budget exhausted\x1b\\"));
    assert!(
        !a.updates
            .iter()
            .any(|u| matches!(u, GraphicsUpdate::Place(_)))
    );
    let atomic = a.exchange(b"\x1b_Ga=T,i=2,f=32,s=1,v=1,C=1;/wAA/w==\x1b\\", 1);
    assert!(atomic[0].starts_with(b"\x1b_Gi=2;ENOSPC:"));
    assert!(!a.updates.iter().any(|u| matches!(
        u,
        GraphicsUpdate::Resource { .. } | GraphicsUpdate::Remove { .. }
    )));
    mode.store(0, Ordering::Release);
    let refusal = a.exchange(b"\x1b_Ga=T,i=2,f=32,s=1,v=1,C=1;/wAA/w==\x1b\\", 1);
    assert!(refusal[0].starts_with(b"\x1b_Gi=2;ENOSPC:"));
    assert!(
        !a.updates
            .iter()
            .any(|u| matches!(u, GraphicsUpdate::Resource { .. }))
    );
    while offers.try_recv().is_ok() {}
    while cancellations.try_recv().is_ok() {}
    mode.store(2, Ordering::Release);
    a.updates.clear();
    a.text.clear();
    a.serial += 1;
    a.session
        .input(
            format!(
                "SEND {} 1 {} 0\n",
                a.serial,
                fixture::hex(b"\x1b_Ga=T,i=3,f=32,s=1,v=1,C=1;/wAA/w==\x1b\\")
            )
            .as_bytes(),
        )
        .unwrap();
    let (pending, _) = offers.recv_timeout(Duration::from_secs(30)).unwrap();
    // Clear executes while the graphics reader is awaiting this observer: no
    // PaneState lock is held, and the late grant cannot revive the request.
    a.session.clear_presentation(true).unwrap();
    assert_eq!(
        cancellations.recv_timeout(Duration::from_secs(30)).unwrap(),
        pending
    );
    ClientMsg::GraphicsGrant {
        token: pending,
        accepted: true,
    }
    .encode(&mut writer)
    .unwrap();
    mode.store(1, Ordering::Release);
    // GraphicsCancel precedes the PTY error response. Do not inject the next
    // SEND while the child still reads that old reply: it would consume the
    // new command as reply bytes. Await this request's existing stream barrier.
    let cancellation_reply = a.finish_exchange(b"cancelled i=3 transfer");
    assert!(
        cancellation_reply
            .iter()
            .any(|reply| reply.starts_with(b"\x1b_Gi=3;EINVAL:"))
    );
    assert!(
        !a.updates
            .iter()
            .any(|u| matches!(u, GraphicsUpdate::Resource { .. }))
    );
    a.idle(b"clear_cancel_shell");
    let history = a.red("a=T,i=4,c=1,r=1,C=1");
    a.idle(&b"\r\n".repeat(40));
    ClientMsg::GraphicsPressure { key: history }
        .encode(&mut writer)
        .unwrap();
    assert_eq!(
        a.eviction(),
        GraphicsUpdate::Evicted {
            key: history,
            width: 1,
            height: 1
        }
    );
    ClientMsg::Detach.encode(&mut writer).unwrap();
    drop(writer);
    worker.join().unwrap();
    daemon.panes().kill(a.session.pane_id()).unwrap();
}

#[test]
fn retention_daemon_metadata_budget_keeps_notice_owners_until_history_or_close_releases() {
    let daemon = Daemon::with_metadata_pressure(true);
    let (mut a, mut b) = (daemon.spawn(), daemon.spawn());
    let history = a.red("a=T,i=1,p=1,c=1,r=1,C=1");
    a.idle(&b"\r\n".repeat(40));
    b.red("a=T,i=2,p=1,c=1,r=1,C=1");
    a.red("a=T,i=3,p=1,c=1,r=1,C=1");
    a.idle(&b"\r\n".repeat(40));
    b.red("a=T,i=4,p=1,c=1,r=1,C=1");
    assert_eq!(
        a.eviction(),
        GraphicsUpdate::Evicted {
            key: history,
            width: 1,
            height: 1
        }
    );
    let refused = b.exchange(b"\x1b_Ga=p,i=2,p=9,c=1,r=1,C=1\x1b\\", 1);
    assert!(refused[0].starts_with(b"\x1b_Gi=2,p=9;ENOSPC:"));
    assert!(
        !b.updates
            .iter()
            .any(|u| matches!(u, GraphicsUpdate::Place(_)))
    );
    // Same-placement updates reuse the existing owner even at the byte limit.
    let updated = b.exchange(b"\x1b_Ga=p,i=2,p=1,c=2,r=1,C=1\x1b\\", 1);
    assert!(updated[0].ends_with(b";OK\x1b\\"));
    a.idle(b"\x1b[3J");
    assert!(a.updates.iter().any(
        |u| matches!(u, GraphicsUpdate::Remove { resources, .. } if resources.contains(&history))
    ));
    let accepted = b.exchange(b"\x1b_Ga=p,i=2,p=9,c=1,r=1,C=1\x1b\\", 1);
    assert!(accepted[0].ends_with(b";OK\x1b\\"));
    a.exit();
    daemon.panes().kill(a.session.pane_id()).unwrap();
    let after_close = b.exchange(b"\x1b_Ga=p,i=2,p=10,c=1,r=1,C=1\x1b\\", 1);
    assert!(after_close[0].ends_with(b";OK\x1b\\"));
    daemon.panes().kill(b.session.pane_id()).unwrap();
}

#[test]
fn retention_daemon_one_kib_metadata_refuses_atomic_image_without_moving_cursor_and_releases() {
    let daemon = Daemon::with_metadata_limit(1024);
    let mut a = daemon.spawn();
    a.red("a=T,i=1,p=1,c=1,r=1,C=1");
    let original = a
        .updates
        .iter()
        .find_map(|update| match update {
            GraphicsUpdate::Place(p) => Some((p.anchor_row, p.anchor_col)),
            _ => None,
        })
        .unwrap();
    let refused = a.exchange(b"\x1b_Ga=T,i=2,f=32,s=1,v=1,c=3,r=2;/wAA/w==\x1b\\", 1);
    assert!(refused[0].starts_with(b"\x1b_Gi=2;ENOSPC:"));
    assert!(!a.updates.iter().any(|u| matches!(
        u,
        GraphicsUpdate::Resource { .. }
            | GraphicsUpdate::Place(_)
            | GraphicsUpdate::Remove { .. }
            | GraphicsUpdate::Evicted { .. }
    )));
    a.idle(b"\x1b_Ga=d,d=I,i=1\x1b\\");
    a.red("a=T,i=3,p=1,c=1,r=1,C=1");
    let after = a
        .updates
        .iter()
        .find_map(|update| match update {
            GraphicsUpdate::Place(p) => Some((p.anchor_row, p.anchor_col)),
            _ => None,
        })
        .unwrap();
    assert_eq!(
        after, original,
        "refused a=T leaves the accepted cursor anchor unchanged"
    );
    a.session.clear_presentation(true).unwrap();
    a.red("a=T,i=4,p=1,c=1,r=1,C=1");
    a.exit();
    daemon.panes().kill(a.session.pane_id()).unwrap();
    let mut b = daemon.spawn();
    b.red("a=T,i=5,p=1,c=1,r=1,C=1");
    daemon.panes().kill(b.session.pane_id()).unwrap();
}

#[test]
fn retention_daemon_recovery_fresh_subscribers_keep_refusal_notices_and_detached_delete_expiry_precedence() {
    fn stats(consumer: &mut Consumer) -> String {
        consumer.serial += 1;
        consumer.text.clear();
        consumer.updates.clear();
        consumer.session.input(format!("STAT {} 0 00 0\n", consumer.serial).as_bytes()).unwrap();
        assert!(consumer.finish_exchange(b"sender identity and no-retransmit counters").is_empty());
        let prefix = format!("KITTY_STATS_{} ", consumer.serial);
        String::from_utf8_lossy(&consumer.text).lines().find_map(|line| line.trim().strip_prefix(&prefix).map(str::to_owned)).unwrap()
    }
    fn detach(consumer: Consumer) {
        let (input, mut output) = consumer.session.split();
        input.detach().unwrap();
        while output.recv().is_ok() {}
    }
    fn fresh(daemon: &Daemon, pane: u64, serial: usize) -> Consumer {
        let session = daemon.panes().attach(pane, size()).unwrap();
        session.set_recv_timeout(Some(Duration::from_secs(30))).unwrap();
        Consumer { session, serial, updates: Vec::new(), text: Vec::new() }
    }
    let daemon = Daemon::start();
    let (mut a, mut b) = (daemon.spawn(), daemon.spawn());
    let deleted_notice = a.red("a=T,i=1,p=1,c=1,r=1,C=1");
    let expired_notice = a.red("a=T,i=2,p=1,c=1,r=1,C=1");
    a.idle(&b"historical explanation\r\n".repeat(40));
    b.red("a=T,i=10,p=1,c=1,r=1,C=1");
    b.red("a=T,i=11,p=1,c=1,r=1,C=1");
    assert_eq!(a.eviction(), GraphicsUpdate::Evicted { key: deleted_notice, width: 1, height: 1 });
    let visible = a.red("a=T,i=3,p=1,c=1,r=1,C=1");
    assert!(a.updates.iter().any(|u| *u == GraphicsUpdate::Evicted { key: expired_notice, width: 1, height: 1 }));
    let refused = a.exchange(b"\x1b_Ga=T,i=4,p=1,f=32,s=1,v=1,C=1;/wAA/w==\x1b\\visible_refusal_keeps_shell", 1);
    assert_eq!(refused.len(), 1);
    assert!(refused[0].starts_with(b"\x1b_Gi=4,p=1;ENOSPC:"));
    assert!(a.text.windows(b"visible_refusal_keeps_shell".len()).any(|part| part == b"visible_refusal_keeps_shell"));
    assert!(!a.updates.iter().any(|u| matches!(u, GraphicsUpdate::Resource { .. } | GraphicsUpdate::Place(_) | GraphicsUpdate::Evicted { .. })));
    let identity = stats(&mut a);
    let (pane, serial) = (a.session.pane_id(), a.serial);
    detach(a);

    let mut recovered = fresh(&daemon, pane, serial);
    assert_eq!(stats(&mut recovered), identity, "fresh cache does not make the child resend evicted or accepted content");
    for old in [deleted_notice, expired_notice] {
        assert!(recovered.updates.iter().any(|u| matches!(u, GraphicsUpdate::Evicted { key, .. } if *key == old)));
        assert!(recovered.updates.iter().any(|u| matches!(u, GraphicsUpdate::Place(p) if p.resource == old)));
        assert!(!recovered.updates.iter().any(|u| matches!(u, GraphicsUpdate::Resource { key, .. } if *key == old)), "evicted historical pixels cannot hide in an old subscriber cache or replay packet");
    }
    assert!(recovered.updates.iter().any(|u| matches!(u, GraphicsUpdate::Resource { key, .. } if *key == visible)));
    let mut detached = b"\x1b_Ga=d,d=I,i=1,q=2\x1b\\\x1b[24;1H".to_vec();
    detached.extend(b"detached expiry\r\n".repeat(10_100));
    detached.extend_from_slice(b"\x1b_Ga=T,i=4,p=1,f=32,s=1,v=1,C=1,q=2;/wAA/w==\x1b\\\x1b_Ga=p,i=4,p=2,U=1,c=1,r=1,q=2\x1b\\\x1b[38;5;4;58;5;2m\xf4\x8e\xbb\xae\xcc\x85\xcc\x85\x1b[0mDETACHED_RETENTION_DONE");
    detach(recovered);
    let dir = tempfile::TempDir::new().unwrap();
    let gate = dir.path().join("release");
    daemon.panes().send_input(pane, format!("WAIT 1000 0 {} 0 {}\n", fixture::hex(&detached), fixture::hex(gate.to_str().unwrap().as_bytes())).as_bytes()).unwrap();
    std::fs::write(&gate, []).unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    while !gate.with_extension("done").exists() {
        assert!(Instant::now() < deadline, "detached sender barrier deadline");
        std::thread::sleep(Duration::from_millis(2));
    }
    let mut final_consumer = fresh(&daemon, pane, 1000);
    let final_identity = stats(&mut final_consumer);
    assert!(final_consumer.text.windows(b"DETACHED_RETENTION_DONE".len()).any(|part| part == b"DETACHED_RETENTION_DONE"));
    assert!(!final_consumer.updates.iter().any(|u| match u {
        GraphicsUpdate::Resource { key, .. } | GraphicsUpdate::Evicted { key, .. } => [deleted_notice, expired_notice, visible].contains(key),
        GraphicsUpdate::Place(p) => [deleted_notice, expired_notice, visible].contains(&p.resource),
        _ => false,
    }), "detached application deletion and history expiration outrank stale replay notices/pixels");
    assert_eq!(final_consumer.updates.iter().filter(|u| matches!(u, GraphicsUpdate::Resource { .. })).count(), 1);
    let placements: Vec<_> = final_consumer.updates.iter().filter_map(|u| if let GraphicsUpdate::Place(p) = u { Some(p) } else { None }).collect();
    assert_eq!(placements.len(), 2);
    assert_eq!(placements[0].resource, placements[1].resource);
    assert!(placements.iter().any(|p| p.virtual_placement));
    let serial = final_consumer.serial;
    detach(final_consumer);
    let mut last = fresh(&daemon, pane, serial);
    assert_eq!(stats(&mut last), final_identity, "a second fresh cache cannot revive discarded history or resend the new retained resource");
    last.exit();
    b.exit();
    daemon.panes().kill(pane).unwrap();
    daemon.panes().kill(b.session.pane_id()).unwrap();
}
