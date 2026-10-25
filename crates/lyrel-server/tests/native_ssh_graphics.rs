//! Explicit live-endpoint coverage: production local router -> pinned native SSH
//! -> Linux daemon -> real Linux PTY. This is not native-window/GPU evidence.
#[path = "../../../tests/support/native_ssh_graphics.rs"]
mod fixture;

use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;
use std::sync::Arc;

use fixture::{NativeSshFixture, hex, status, unhex, win_size};
use lyrel_core::core::kitty_graphics::{GraphicsUpdate, Image, Placement};
use lyrel_core::daemon::protocol::{self, ClientMsg, DaemonMsg, ShellSpec};
use lyrel_core::daemon::transport::Stream;
use lyrel_core::host::Host;

fn contains(bytes: &[u8], needle: &[u8]) -> bool {
    bytes.windows(needle.len()).any(|part| part == needle)
}

#[derive(Default)]
struct State {
    resources: BTreeMap<u64, Arc<Image>>,
    placements: BTreeMap<u64, Placement>,
    evicted: BTreeMap<u64, (u32, u32)>,
    screen: u64,
}

impl State {
    fn apply(&mut self, update: GraphicsUpdate) {
        match update {
            GraphicsUpdate::Resource { key, image } => {
                assert!(self.resources.insert(key, image).is_none(), "recovery/live resource duplicated");
            }
            GraphicsUpdate::Place(p) => {
                assert!(self.resources.contains_key(&p.resource) || self.evicted.contains_key(&p.resource));
                self.placements.insert(p.key, p);
            }
            GraphicsUpdate::Evicted { key, width, height } => {
                self.resources.remove(&key);
                self.evicted.insert(key, (width, height));
            }
            GraphicsUpdate::Remove { placements, resources } => {
                for key in placements { self.placements.remove(&key); }
                for key in resources { self.resources.remove(&key); self.evicted.remove(&key); }
            }
            GraphicsUpdate::Screen { screen, .. } => self.screen = screen,
        }
    }

    fn placement(&self, image: u32, placement: u32) -> &Placement {
        self.placements.values().find(|p| p.image_id == image && p.placement_id == placement).expect("retained placement")
    }

    fn pixels(&self, p: &Placement) -> Vec<u8> {
        self.resources[&p.resource].to_rgba8().unwrap()
    }
}

struct Consumer {
    stream: Stream,
    pane: u64,
    state: State,
    serial: u64,
    text: Vec<u8>,
}

impl Consumer {
    fn spawn(fixture: &NativeSshFixture, shell: ShellSpec, encoded: u64, decoded: u64) -> Self {
        let (mut stream, _) = fixture.connect_pane_with_capacity(encoded, decoded);
        ClientMsg::Spawn {
            cwd: Some(fixture.remote_home().into()), size: win_size(), shell: Some(shell),
            owner: None, workspace: None, restore: None, allow_remote_clipboard_write: false, await_claim: false,
        }.encode(&mut stream).unwrap();
        let pane = match DaemonMsg::read(&mut stream).unwrap() {
            DaemonMsg::Spawned { pane_id } => pane_id,
            other => panic!("expected real remote Spawned, got {other:?}"),
        };
        fixture.track_pane(pane);
        let mut consumer = Self::new(stream, pane);
        consumer.until(b"KITTY_READY");
        consumer
    }

    fn new(stream: Stream, pane: u64) -> Self {
        Self { stream, pane, state: State::default(), serial: 0, text: Vec::new() }
    }

    fn until(&mut self, marker: &[u8]) {
        self.text.clear();
        loop {
            match DaemonMsg::read(&mut self.stream).expect("bounded real SSH consumer stream") {
                DaemonMsg::GraphicsOffer { token, encoded_bytes, width, height, .. } => {
                    assert!(encoded_bytes <= protocol::MAX_FRAME as u64);
                    assert!(u64::from(width) * u64::from(height) * 4 <= 128 << 20);
                    ClientMsg::GraphicsGrant { token, accepted: true }.encode(&mut self.stream).unwrap();
                }
                DaemonMsg::Graphics(frame) => {
                    assert!(frame.len() <= protocol::MAX_FRAME);
                    self.state.apply(GraphicsUpdate::decode_frame_owned(frame.into_vec()).expect("valid production graphics packet"));
                }
                DaemonMsg::Snapshot(bytes) | DaemonMsg::Output(bytes) => {
                    assert!(!contains(&bytes, b"\x1b_G"), "pixel APC entered text replay");
                    self.text.extend(bytes);
                    if contains(&self.text, marker) { return; }
                }
                DaemonMsg::Error(error) => panic!("remote consumer refused: {error}"),
                DaemonMsg::Exited { code } => panic!("real Linux sender exited: {code:?}"),
                _ => {}
            }
        }
    }

    fn exchange(&mut self, operation: &str, bytes: &[u8], replies: usize, fragment: usize) -> Vec<Vec<u8>> {
        self.serial += 1;
        let serial = self.serial;
        let request = format!("{operation} {serial} {replies} {} {fragment}\n", hex(bytes));
        ClientMsg::Input(request.into_bytes()).encode(&mut self.stream).unwrap();
        self.until(format!("KITTY_DONE_{serial}\r").as_bytes());
        let prefix = format!("KITTY_REPLY {serial} ");
        String::from_utf8_lossy(&self.text).lines()
            .filter_map(|line| line.trim().strip_prefix(&prefix).map(unhex)).collect()
    }

    fn send(&mut self, bytes: &[u8], replies: usize) -> Vec<Vec<u8>> {
        self.exchange("SEND", bytes, replies, 0)
    }

    fn quiet(&mut self, bytes: &[u8]) { assert!(self.send(bytes, 0).is_empty()); }

    fn stats(&mut self) -> String {
        assert!(self.exchange("STAT", b"\0", 0, 0).is_empty());
        let prefix = format!("KITTY_STATS_{} ", self.serial);
        String::from_utf8_lossy(&self.text).lines()
            .find_map(|line| line.trim().strip_prefix(&prefix).map(str::to_owned)).unwrap()
    }

    fn detach(mut self) {
        ClientMsg::Detach.encode(&mut self.stream).unwrap();
        while DaemonMsg::read(&mut self.stream).is_ok() {}
    }
}

fn attach(fixture: &NativeSshFixture, pane: u64) -> Consumer {
    let (mut stream, _) = fixture.connect_pane();
    ClientMsg::Attach { pane_id: pane, size: win_size(), allow_remote_clipboard_write: false }.encode(&mut stream).unwrap();
    Consumer::new(stream, pane)
}

fn base64(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut encoded = String::new();
    for chunk in bytes.chunks(3) {
        let bits = (u32::from(chunk[0]) << 16) | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8) | u32::from(*chunk.get(2).unwrap_or(&0));
        encoded.push(DIGITS[((bits >> 18) & 63) as usize] as char);
        encoded.push(DIGITS[((bits >> 12) & 63) as usize] as char);
        encoded.push(if chunk.len() > 1 { DIGITS[((bits >> 6) & 63) as usize] as char } else { '=' });
        encoded.push(if chunk.len() > 2 { DIGITS[(bits & 63) as usize] as char } else { '=' });
    }
    encoded
}

#[test]
#[ignore = "requires explicit fixture SSH endpoint, fixed public host pin, fixture identity, private Linux home and Node; never silently skips"]
fn native_ssh_graphics_replies_recovery_limits_and_host_local_media() {
    let mut fixture = NativeSshFixture::start(Path::new(env!("CARGO_BIN_EXE_lyrel-server")));
    let host = fixture.connect_control();
    let remote_status = status(&host);
    let control_instance = host.peer().instance.clone();
    let (version_stream, pane_version) = fixture.connect_pane();
    drop(version_stream);
    let shell = fixture.install_sender(&host);
    let mut pane = Consumer::spawn(&fixture, shell.clone(), 64 << 20, 128 << 20);
    let pane_id = pane.pane;
    let processes = host.pane_procs(pane_id).expect("remote production PaneProcs");
    let context = processes.context.as_ref().expect("remote pane context");
    assert!(context.local_pty && context.remote.is_none(), "not a remote-daemon-owned Linux PTY: {processes:?}");

    assert_eq!(pane.exchange("SEND", b"\x1b_Ga=t,i=1,f=32,s=2,v=1;/wAA/wD/AP8=\x1b\\", 1, 1), [b"\x1b_Gi=1;OK\x1b\\".to_vec()]);
    assert!(pane.state.placements.is_empty(), "transmit-only cannot display");
    assert_eq!(pane.send(b"\x1b[6;3H\x1b_Ga=p,i=1,p=1,x=1,w=1,h=1,c=3,r=2,C=1\x1b\\", 1), [b"\x1b_Gi=1,p=1;OK\x1b\\".to_vec()]);
    let crop = pane.state.placement(1, 1).clone();
    assert_eq!((crop.x, crop.w, crop.h, crop.cols, crop.rows), (1, 1, 1, 3, 2));
    assert_eq!(pane.state.pixels(&crop), [255, 0, 0, 255, 0, 255, 0, 255]);
    pane.quiet(b"\x1b_Ga=p,i=1,p=2,U=1,c=1,r=1,q=2\x1b\\\x1b[9;4H\x1b[38;5;1;58;5;2m\xf4\x8e\xbb\xae\xcc\x85\xcc\x85\x1b[0m");
    assert!(pane.state.placement(1, 2).virtual_placement);
    pane.quiet(b"\x1b_Ga=p,i=1,p=1,c=4,r=1,C=1,q=2\x1b\\");
    assert_eq!(pane.state.placements.values().filter(|p| p.image_id == 1 && p.placement_id == 1).count(), 1);
    assert_eq!(pane.state.placement(1, 1).cols, 4);
    pane.quiet(b"\x1b_Ga=d,d=i,i=1,p=1,q=2\x1b\\");
    assert!(pane.state.placements.values().all(|p| p.placement_id != 1));
    assert!(pane.state.resources.contains_key(&crop.resource), "placement delete retains pixels");
    pane.quiet(b"\x1b[?1049h\x1b_Ga=T,i=9,p=9,f=32,s=1,v=1,C=1,q=2;AAD//w==\x1b\\");
    assert_eq!(pane.state.screen, 1);
    let alt = pane.state.placement(9, 9).clone();
    pane.quiet(b"\x1b[?1049l");
    assert_eq!(pane.state.screen, 0);
    assert!(!pane.state.resources.contains_key(&alt.resource));
    assert!(pane.state.placements.values().all(|p| p.image_id != 9));
    pane.quiet(b"\x1b_Ga=d,d=I,i=1,q=2\x1b\\\x1b[3;2H\x1b_Ga=T,i=7,p=7,C=1,f=32,s=1,v=1,q=2;/wAA/w==\x1b\\");

    // Linux file transport is evaluated by the Linux pane owner, never by the
    // Windows/macOS desktop. Our file contains only this fixture's raw pixel.
    let remote_pixel = fixture.remote_path("remote-pixel.rgba");
    host.write_file(&remote_pixel, &[12, 34, 56, 255]).unwrap();
    let encoded_remote = base64(remote_pixel.to_string_lossy().as_bytes());
    assert_eq!(pane.send(format!("\x1b_Ga=q,i=40,t=f,f=32,s=1,v=1;{encoded_remote}\x1b\\").as_bytes(), 1), [b"\x1b_Gi=40;OK\x1b\\".to_vec()]);
    host.remove(&remote_pixel, false).unwrap();
    let missing = pane.send(format!("\x1b_Ga=q,i=40,t=f,f=32,s=1,v=1;{encoded_remote}\x1b\\").as_bytes(), 1);
    assert!(missing[0].starts_with(b"\x1b_Gi=40;EBADF:"));
    let desktop = tempfile::TempDir::new().unwrap();
    let desktop_pixel = desktop.path().join("desktop-only.rgba");
    std::fs::write(&desktop_pixel, [255, 0, 0, 255]).unwrap();
    let encoded_desktop = base64(desktop_pixel.to_string_lossy().as_bytes());
    let refused = pane.send(format!("\x1b_Ga=q,i=41,t=f,f=32,s=1,v=1;{encoded_desktop}\x1b\\").as_bytes(), 1);
    assert!(refused[0].starts_with(b"\x1b_Gi=41;EBADF:"), "remote pane resolved desktop path");
    for medium in ["t", "s"] {
        let reply = pane.send(format!("\x1b_Ga=q,i=42,t={medium},f=32,s=1,v=1;{encoded_remote}\x1b\\").as_bytes(), 1);
        assert!(reply[0].starts_with(b"\x1b_Gi=42;EBADF:"));
    }
    let invalid = pane.exchange("SEND", b"\x1b_Ga=T,i=43,f=32,s=1,v=1;!\x1b\\after_invalid", 1, 1);
    assert!(invalid[0].starts_with(b"\x1b_Gi=43;EINVAL:"));
    assert!(contains(&pane.text, b"after_invalid"));
    assert!(pane.send(b"\x1b_Ga=q,i=44,q=2,f=32,s=1,v=1;!\x1b\\", 0).is_empty());
    assert_eq!(pane.send(b"\x1b[14t\x1b[16t", 2), [b"\x1b[4;480;800t".to_vec(), b"\x1b[6;20;10t".to_vec()]);

    // A stopped transmitter still lives in the original Linux PTY. A fresh
    // consumer has no old pixel/placement cache to make this pass accidentally.
    let expected = pane.state.placements.clone();
    let identity = pane.stats();
    let sender_pid: u32 = identity.split_whitespace().find_map(|field| field.strip_prefix("pid=")).unwrap().parse().unwrap();
    assert!(processes.procs.iter().any(|p| p.pid == sender_pid), "sender must belong to remote pane process tree");
    pane.detach();
    let mut soft = attach(&fixture, pane_id);
    assert_eq!(soft.stats(), identity, "soft reconnect caused image retransmission or replaced sender");
    assert_eq!(soft.state.placements, expected);
    soft.detach();
    drop(host);
    fixture.restart_router();
    let host = fixture.connect_control();
    assert_eq!(host.peer().instance, control_instance);
    assert_eq!(status(&host).pid, remote_status.pid);
    let (stream, version) = fixture.connect_pane();
    assert_eq!(version.instance, pane_version.instance);
    drop(stream);
    let mut hard = attach(&fixture, pane_id);
    assert_eq!(hard.stats(), identity, "router restart cannot replace/retransmit remote sender");
    assert_eq!(hard.state.placements, expected);
    let p = hard.state.placement(7, 7);
    assert_eq!(hard.state.pixels(p), [255, 0, 0, 255]);

    // Hold the first real replay offer, then mutate through the active writer.
    // The fresh observer may not resurrect the deleted image at replay cutover.
    let (mut observer, _) = fixture.connect_pane();
    ClientMsg::Observe { pane_id, size: win_size() }.encode(&mut observer).unwrap();
    let pending = match DaemonMsg::read(&mut observer).unwrap() {
        DaemonMsg::GraphicsOffer { token, .. } => token,
        other => panic!("expected held production replay offer, got {other:?}"),
    };
    hard.quiet(b"\x1b_Ga=d,d=I,i=7,q=2\x1b\\\x1b[8;4H\x1b_Ga=T,i=8,p=8,C=1,f=32,s=1,v=1,q=2;AP8A/w==\x1b\\LIVE_NATIVE_SSH_ORDER");
    let current = hard.state.placements.clone();
    let counters = hard.stats();
    ClientMsg::GraphicsGrant { token: pending, accepted: true }.encode(&mut observer).unwrap();
    // STAT arrives from a separate production input command while recovery is
    // pending. It changes no upload/placement/byte counters.
    let serial = 900_001;
    let (mut input, _) = fixture.connect_pane();
    ClientMsg::SendInput { pane_id, bytes: format!("STAT {serial} 0 00 0\n").into_bytes() }.encode(&mut input).unwrap();
    assert!(matches!(DaemonMsg::read(&mut input).unwrap(), DaemonMsg::InputAck { pane_id: id } if id == pane_id));
    let mut fresh = Consumer::new(observer, pane_id);
    fresh.until(format!("KITTY_DONE_{serial}\r").as_bytes());
    assert_eq!(fresh.state.placements, current);
    assert!(fresh.state.placements.values().all(|p| p.image_id != 7));
    assert!(contains(&fresh.text, b"LIVE_NATIVE_SSH_ORDER"));
    assert!(contains(&fresh.text, format!("KITTY_STATS_{serial} {counters}").as_bytes()));
    assert_eq!(hard.stats(), counters);
    drop(fresh);
    hard.quiet(b"\x1b[2J");
    assert!(hard.state.placements.is_empty());
    hard.quiet(b"\x1bc");
    assert!(hard.state.resources.is_empty());
    hard.detach();
    fixture.kill_pane(pane_id);

    // Real finite capacities, not a 64MiB/queue-limit override for this route.
    let mut pressure = Consumer::spawn(&fixture, shell, 1024, 8);
    pressure.quiet(b"\x1b[3;2H\x1b_Ga=T,i=1,p=1,C=1,f=32,s=1,v=1,q=2;/wAA/w==\x1b\\\x1b[5;2H\x1b_Ga=T,i=2,p=2,C=1,f=32,s=1,v=1,q=2;AP8A/w==\x1b\\");
    let protected = pressure.state.placement(2, 2).clone();
    let refused = pressure.send(b"\x1b_Ga=T,i=3,p=3,C=1,f=32,s=1,v=1;/wAA/w==\x1b\\after_pressure", 1);
    assert!(refused[0].starts_with(b"\x1b_Gi=3,p=3;ENOSPC:"));
    assert!(contains(&pressure.text, b"after_pressure"));
    assert_eq!(pressure.state.pixels(&protected), [0, 255, 0, 255]);
    pressure.quiet(&b"\r\n".repeat(40));
    let historical = pressure.state.placement(1, 1).resource;
    ClientMsg::GraphicsPressure { key: historical }.encode(&mut pressure.stream).unwrap();
    pressure.stats();
    assert!(pressure.state.evicted.contains_key(&historical), "eligible oldest history must be reclaimed");
    pressure.quiet(b"\x1b_Ga=T,i=3,p=3,C=1,f=32,s=1,v=1,q=2;/wAA/w==\x1b\\");
    assert_eq!(pressure.state.pixels(pressure.state.placement(3, 3)), [255, 0, 0, 255]);
    pressure.quiet(b"\x1b[3J");
    assert!(!pressure.state.evicted.contains_key(&historical), "expired history cannot keep an eviction notice");
    let flood = pressure.exchange("FLOOD", b"\0", 1, 0);
    assert!(flood[0].starts_with(b"\x1b_Gi=720;ENOSPC:"));
    assert!(contains(&pressure.text, b"after_oversize"));
    pressure.quiet(b"\x1bcinput_still_usable");
    assert!(contains(&pressure.text, b"input_still_usable"));
    fixture.kill_pane(pressure.pane);
    drop(pressure);

    // Refuse a >64MiB framed request before allocating its payload on the real
    // remote channel; this isolated failed route cannot kill the Linux daemon.
    let (mut malformed, _) = fixture.connect_pane();
    malformed.write_all(&((protocol::MAX_FRAME + 1) as u32).to_le_bytes()).unwrap();
    malformed.write_all(&[0]).unwrap();
    let _ = malformed.shutdown(std::net::Shutdown::Write);
    match DaemonMsg::read(&mut malformed) {
        Ok(DaemonMsg::Error(error)) => assert!(!error.is_empty()),
        Err(error) => assert!(matches!(error.kind(), std::io::ErrorKind::UnexpectedEof | std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::ConnectionAborted)),
        other => panic!("oversized frame manufactured success: {other:?}"),
    }
    assert_eq!(status(&host).pid, remote_status.pid);
    assert_eq!(host.peer().instance, control_instance);
}
