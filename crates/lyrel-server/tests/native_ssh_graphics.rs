//! Explicit live-endpoint coverage: production local router -> pinned native SSH
//! -> Linux daemon -> real Linux PTY. This is not native-window/GPU evidence.
#[path = "../../../tests/support/native_ssh_graphics.rs"]
pub mod fixture;

use std::collections::{BTreeMap, VecDeque};
use std::io::Write;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use fixture::{NativeSshFixture, hex, status, win_size};
use lyrel_core::core::kitty_graphics::{GraphicsUpdate, Image, Placement};
use lyrel_core::core::pane_colors::{CURSOR, ColorFrame, DEFAULT_BG, DEFAULT_FG, PaneColors};
use lyrel_core::daemon::protocol::{
    self, ClientMsg, DaemonMsg, FEATURE_PANE_COLORS, FEATURE_STATIC_SIXEL, ShellSpec,
};
use lyrel_core::daemon::transport::Stream;
use lyrel_core::host::Host;

fn unhex(text: &str) -> Vec<u8> {
    assert_eq!(text.len() % 2, 0);
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
        .collect()
}

fn next_serial() -> u64 {
    // A fresh consumer receives the sender's old DONE/REPLY lines in Snapshot.
    // Request IDs belong to the living sender, not the replaceable consumer.
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

fn contains(bytes: &[u8], needle: &[u8]) -> bool {
    bytes.windows(needle.len()).any(|part| part == needle)
}

fn trace_event(trace: &mut VecDeque<String>, event: String) {
    if trace.len() == 96 {
        trace.pop_front();
    }
    trace.push_back(event);
}

#[derive(Default)]
struct State {
    resources: BTreeMap<u64, Arc<Image>>,
    placements: BTreeMap<u64, Placement>,
    evicted: BTreeMap<u64, (u32, u32)>,
    frames: BTreeMap<u64, u64>,
    selected: BTreeMap<u64, (u64, u64)>,
    screen: u64,
    epoch: u64,
    history_origin: i64,
    playback: BTreeMap<u64, lyrel_core::core::graphics_state::Playback>,
    /// The decoded, daemon-authoritative override layer. A fresh consumer
    /// starts from the theme and applies the attach snapshot, never a local
    /// copy of the original OSC bytes.
    colors: PaneColors,
    color_frames: u64,
    color_snapshots: u64,
}

impl State {
    fn apply(&mut self, update: GraphicsUpdate) {
        match update {
            GraphicsUpdate::Resource { key, image } => {
                assert!(
                    self.resources.insert(key, image).is_none(),
                    "recovery/live resource duplicated"
                );
            }
            GraphicsUpdate::Frame { key, root, number } => {
                assert!(number >= 2 && key != root);
                assert!(self.resources.contains_key(&key) && self.resources.contains_key(&root));
                assert!(self.frames.insert(key, root).is_none());
            }
            GraphicsUpdate::Select {
                key,
                frame,
                revision,
            } => {
                assert!(self.resources.contains_key(&key) || self.evicted.contains_key(&key));
                assert!(self.resources.contains_key(&frame) || self.evicted.contains_key(&frame));
                if let Some((_, previous)) = self.selected.insert(key, (frame, revision)) {
                    assert!(revision > previous, "selection revisions must advance");
                }
            }
            GraphicsUpdate::Place(p) => {
                assert!(
                    self.resources.contains_key(&p.resource)
                        || self.evicted.contains_key(&p.resource)
                );
                self.placements.insert(p.key, p);
            }
            GraphicsUpdate::Evicted { key, width, height } => {
                self.resources.remove(&key);
                self.evicted.insert(key, (width, height));
            }
            GraphicsUpdate::Remove {
                placements,
                resources,
            } => {
                for key in placements {
                    self.placements.remove(&key);
                }
                for key in resources {
                    self.resources.remove(&key);
                    self.evicted.remove(&key);
                    self.frames.remove(&key);
                    self.selected.remove(&key);
                }
            }
            GraphicsUpdate::Screen {
                screen,
                epoch,
                history_origin,
            } => {
                self.screen = screen;
                self.epoch = epoch;
                self.history_origin = history_origin;
            }
            GraphicsUpdate::Playback(playback) => {
                assert!(
                    self.resources.contains_key(&playback.key)
                        || self.evicted.contains_key(&playback.key)
                );
                self.playback.insert(playback.key, playback);
            }
        }
    }

    fn apply_colors(&mut self, frame: ColorFrame) {
        if frame.is_snapshot() {
            self.color_snapshots += 1;
        }
        frame.apply(&mut self.colors);
        self.color_frames += 1;
    }

    fn placement(&self, image: u32, placement: u32) -> &Placement {
        self.placements
            .values()
            .find(|p| p.image_id == image && p.placement_id == placement)
            .expect("retained placement")
    }

    fn pixels(&self, p: &Placement) -> Vec<u8> {
        let frame = self
            .selected
            .get(&p.resource)
            .map_or(p.resource, |&(frame, _)| frame);
        self.resources[&frame].to_rgba8().unwrap()
    }
}

struct Consumer {
    stream: Stream,
    pane: u64,
    state: State,
    serial: u64,
    text: Vec<u8>,
    /// Text bytes read by the last `collect_until`, so a fresh attach can prove
    /// its replay is the real 8 MiB ring tail rather than a small snapshot.
    text_bytes: u64,
    needs_replay: bool,
    resource_pixel_bytes: u64,
    graphics_wire_bytes: u64,
    select_packets: u64,
}

impl Consumer {
    fn spawn(fixture: &NativeSshFixture, shell: ShellSpec, encoded: u64, decoded: u64) -> Self {
        let (mut stream, _) = fixture.connect_pane_with_capacity(encoded, decoded);
        ClientMsg::Spawn {
            cwd: Some(fixture.remote_home().into()),
            size: win_size(),
            shell: Some(shell),
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
            other => panic!("expected real remote Spawned, got {other:?}"),
        };
        fixture.track_pane(pane);
        let mut consumer = Self::new(stream, pane, false);
        consumer.until(b"KITTY_READY");
        consumer
    }

    fn new(stream: Stream, pane: u64, needs_replay: bool) -> Self {
        Self {
            stream,
            pane,
            state: State::default(),
            serial: 0,
            text: Vec::new(),
            text_bytes: 0,
            needs_replay,
            resource_pixel_bytes: 0,
            graphics_wire_bytes: 0,
            select_packets: 0,
        }
    }

    fn until(&mut self, marker: &[u8]) {
        self.collect_until(marker);
    }

    fn collect_until(&mut self, marker: &[u8]) -> VecDeque<String> {
        self.text.clear();
        self.text_bytes = 0;
        let mut trace = VecDeque::new();
        let mut replayed = !self.needs_replay;
        let mut colors_replayed = !self.needs_replay;
        loop {
            let message = DaemonMsg::read(&mut self.stream).unwrap_or_else(|error| {
                panic!("bounded real SSH consumer stream: {error}; events={trace:#?}")
            });
            let event = match &message {
                DaemonMsg::Graphics(frame) => format!("Graphics bytes={}", frame.len()),
                DaemonMsg::Snapshot(bytes) => format!(
                    "Snapshot bytes={} marker={} tail(hex)={}",
                    bytes.len(),
                    contains(bytes, marker),
                    hex(&bytes[bytes.len().saturating_sub(256)..])
                ),
                DaemonMsg::Output(bytes) => format!(
                    "Output bytes={} marker={} tail(hex)={}",
                    bytes.len(),
                    contains(bytes, marker),
                    hex(&bytes[bytes.len().saturating_sub(256)..])
                ),
                other => format!("{other:?}"),
            };
            trace_event(&mut trace, event);
            match message {
                DaemonMsg::GraphicsOffer {
                    token,
                    encoded_bytes,
                    width,
                    height,
                    ..
                } => {
                    assert!(encoded_bytes <= protocol::MAX_FRAME as u64);
                    assert!(u64::from(width) * u64::from(height) * 4 <= 128 << 20);
                    ClientMsg::GraphicsGrant {
                        token,
                        accepted: true,
                    }
                    .encode(&mut self.stream)
                    .unwrap();
                }
                DaemonMsg::Graphics(frame) => {
                    assert!(frame.len() <= protocol::MAX_FRAME);
                    self.graphics_wire_bytes += frame.len() as u64;
                    let update = GraphicsUpdate::decode_frame_owned(frame.into_vec())
                        .expect("valid production graphics packet");
                    match &update {
                        GraphicsUpdate::Resource { image, .. } => {
                            self.resource_pixel_bytes += image.data.len() as u64
                        }
                        GraphicsUpdate::Select { .. } => self.select_packets += 1,
                        _ => {}
                    }
                    let event = match &update {
                        GraphicsUpdate::Resource { key, image } => {
                            format!("Resource key={key} size={}x{}", image.width, image.height)
                        }
                        other => format!("{other:?}"),
                    };
                    trace_event(&mut trace, event);
                    // The ring's Snapshot may already contain the new DONE
                    // while replay admission is being retried. GraphicsState's
                    // final Screen follows its Resource/Place replay packets.
                    replayed |= matches!(&update, GraphicsUpdate::Screen { .. });
                    self.state.apply(update);
                }
                DaemonMsg::Snapshot(bytes) | DaemonMsg::Output(bytes) => {
                    assert!(
                        !contains(&bytes, b"\x1b_G"),
                        "pixel APC entered text replay"
                    );
                    self.text_bytes += bytes.len() as u64;
                    self.text.extend(bytes);
                }
                DaemonMsg::Colors(bytes) => {
                    assert!(
                        bytes.len() <= protocol::MAX_FRAME,
                        "bounded authoritative colour frame"
                    );
                    let frame = ColorFrame::decode(&bytes)
                        .expect("production pane-colour frame must decode");
                    // `replay_state` sends the full snapshot after the ring and
                    // graphics replay, so it also ends a replay with no graphics
                    // resources at all. A delta before that snapshot would mean
                    // a fresh consumer lost the authority's earlier overrides.
                    if self.needs_replay {
                        assert!(
                            colors_replayed || frame.is_snapshot(),
                            "fresh consumer needs the authority snapshot before live deltas"
                        );
                    }
                    if frame.is_snapshot() {
                        colors_replayed = true;
                        replayed = true;
                    }
                    self.state.apply_colors(frame);
                }
                DaemonMsg::Error(error) => panic!("remote consumer refused: {error}"),
                DaemonMsg::Exited { code } => panic!("real Linux sender exited: {code:?}"),
                _ => {}
            }
            if replayed && colors_replayed && contains(&self.text, marker) {
                self.needs_replay = false;
                return trace;
            }
        }
    }

    fn exchange(
        &mut self,
        operation: &str,
        bytes: &[u8],
        replies: usize,
        fragment: usize,
    ) -> Vec<Vec<u8>> {
        self.serial = next_serial();
        let serial = self.serial;
        let request = format!("{operation} {serial} {replies} {} {fragment}\n", hex(bytes));
        ClientMsg::Input(request.into_bytes())
            .encode(&mut self.stream)
            .unwrap();
        self.until(format!("KITTY_DONE_{serial};\r").as_bytes());
        let prefix = format!("KITTY_REPLY {serial} ");
        String::from_utf8_lossy(&self.text)
            .lines()
            .filter_map(|line| line.trim().strip_prefix(&prefix).map(unhex))
            .collect()
    }

    fn send(&mut self, bytes: &[u8], replies: usize) -> Vec<Vec<u8>> {
        self.exchange("SEND", bytes, replies, 0)
    }

    fn quiet(&mut self, bytes: &[u8]) {
        assert!(self.send(bytes, 0).is_empty());
    }

    fn stats(&mut self) -> String {
        let replies = self.exchange("STAT", b"\0", 0, 0);
        assert!(
            replies.is_empty(),
            "STAT serial={} unexpectedly matched replies(hex)={:?}; output_tail(hex)={}",
            self.serial,
            replies.iter().map(|reply| hex(reply)).collect::<Vec<_>>(),
            hex(&self.text[self.text.len().saturating_sub(4096)..])
        );
        let prefix = format!("KITTY_STATS_{} ", self.serial);
        String::from_utf8_lossy(&self.text)
            .lines()
            .find_map(|line| line.trim().strip_prefix(&prefix).map(str::to_owned))
            .unwrap()
    }

    fn detach(mut self) {
        ClientMsg::Detach.encode(&mut self.stream).unwrap();
        while DaemonMsg::read(&mut self.stream).is_ok() {}
    }
}

fn attach(fixture: &NativeSshFixture, pane: u64) -> Consumer {
    let (mut stream, _) = fixture.connect_pane();
    ClientMsg::Attach {
        pane_id: pane,
        size: win_size(),
        allow_remote_clipboard_write: false,
    }
    .encode(&mut stream)
    .unwrap();
    Consumer::new(stream, pane, true)
}

fn base64(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut encoded = String::new();
    for chunk in bytes.chunks(3) {
        let bits = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        encoded.push(DIGITS[((bits >> 18) & 63) as usize] as char);
        encoded.push(DIGITS[((bits >> 12) & 63) as usize] as char);
        encoded.push(if chunk.len() > 1 {
            DIGITS[((bits >> 6) & 63) as usize] as char
        } else {
            '='
        });
        encoded.push(if chunk.len() > 2 {
            DIGITS[(bits & 63) as usize] as char
        } else {
            '='
        });
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
    let processes = host
        .pane_procs(pane_id)
        .expect("remote production PaneProcs");
    let context = processes.context.as_ref().expect("remote pane context");
    assert!(
        context.local_pty && context.remote.is_none(),
        "not a remote-daemon-owned Linux PTY: {processes:?}"
    );

    assert_eq!(
        pane.exchange(
            "SEND",
            b"\x1b_Ga=t,i=1,f=32,s=2,v=1;/wAA/wD/AP8=\x1b\\",
            1,
            1
        ),
        [b"\x1b_Gi=1;OK\x1b\\".to_vec()]
    );
    assert!(
        pane.state.placements.is_empty(),
        "transmit-only cannot display"
    );
    assert_eq!(
        pane.send(
            b"\x1b[6;3H\x1b_Ga=p,i=1,p=1,x=1,w=1,h=1,c=3,r=2,C=1\x1b\\",
            1
        ),
        [b"\x1b_Gi=1,p=1;OK\x1b\\".to_vec()]
    );
    let crop = pane.state.placement(1, 1).clone();
    assert_eq!(
        (crop.x, crop.w, crop.h, crop.cols, crop.rows),
        (1, 1, 1, 3, 2)
    );
    assert_eq!(pane.state.pixels(&crop), [255, 0, 0, 255, 0, 255, 0, 255]);
    pane.quiet(b"\x1b_Ga=p,i=1,p=2,U=1,c=1,r=1,q=2\x1b\\\x1b[9;4H\x1b[38;5;1;58;5;2m\xf4\x8e\xbb\xae\xcc\x85\xcc\x85\x1b[0m");
    assert!(pane.state.placement(1, 2).virtual_placement);
    pane.quiet(b"\x1b_Ga=p,i=1,p=1,c=4,r=1,C=1,q=2\x1b\\");
    assert_eq!(
        pane.state
            .placements
            .values()
            .filter(|p| p.image_id == 1 && p.placement_id == 1)
            .count(),
        1
    );
    assert_eq!(pane.state.placement(1, 1).cols, 4);
    pane.quiet(b"\x1b_Ga=d,d=i,i=1,p=1,q=2\x1b\\");
    assert!(pane.state.placements.values().all(|p| p.placement_id != 1));
    assert!(
        pane.state.resources.contains_key(&crop.resource),
        "placement delete retains pixels"
    );
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
    assert_eq!(
        pane.send(
            format!("\x1b_Ga=q,i=40,t=f,f=32,s=1,v=1;{encoded_remote}\x1b\\").as_bytes(),
            1
        ),
        [b"\x1b_Gi=40;OK\x1b\\".to_vec()]
    );
    host.remove(&remote_pixel, false).unwrap();
    let missing = pane.send(
        format!("\x1b_Ga=q,i=40,t=f,f=32,s=1,v=1;{encoded_remote}\x1b\\").as_bytes(),
        1,
    );
    assert!(missing[0].starts_with(b"\x1b_Gi=40;EBADF:"));
    let desktop = tempfile::TempDir::new().unwrap();
    let desktop_pixel = desktop.path().join("desktop-only.rgba");
    std::fs::write(&desktop_pixel, [255, 0, 0, 255]).unwrap();
    let encoded_desktop = base64(desktop_pixel.to_string_lossy().as_bytes());
    let refused = pane.send(
        format!("\x1b_Ga=q,i=41,t=f,f=32,s=1,v=1;{encoded_desktop}\x1b\\").as_bytes(),
        1,
    );
    assert!(
        refused[0].starts_with(b"\x1b_Gi=41;EBADF:"),
        "remote pane resolved desktop path"
    );
    for medium in ["t", "s"] {
        let reply = pane.send(
            format!("\x1b_Ga=q,i=42,t={medium},f=32,s=1,v=1;{encoded_remote}\x1b\\").as_bytes(),
            1,
        );
        assert!(reply[0].starts_with(b"\x1b_Gi=42;EBADF:"));
    }
    let invalid = pane.exchange(
        "SEND",
        b"\x1b_Ga=T,i=43,f=32,s=1,v=1;!\x1b\\after_invalid",
        1,
        1,
    );
    assert!(invalid[0].starts_with(b"\x1b_Gi=43;EINVAL:"));
    assert!(contains(&pane.text, b"after_invalid"));
    assert!(
        pane.send(b"\x1b_Ga=q,i=44,q=2,f=32,s=1,v=1;!\x1b\\", 0)
            .is_empty()
    );
    assert_eq!(
        pane.send(b"\x1b[14t\x1b[16t", 2),
        [b"\x1b[4;480;800t".to_vec(), b"\x1b[6;20;10t".to_vec()]
    );

    // A stopped transmitter still lives in the original Linux PTY. A fresh
    // consumer has no old pixel/placement cache to make this pass accidentally.
    let expected = pane.state.placements.clone();
    let identity = pane.stats();
    let sender_pid: u32 = identity
        .split_whitespace()
        .find_map(|field| field.strip_prefix("pid="))
        .unwrap()
        .parse()
        .unwrap();
    assert!(
        processes.procs.iter().any(|p| p.pid == sender_pid),
        "sender must belong to remote pane process tree"
    );
    pane.detach();
    let mut soft = attach(&fixture, pane_id);
    assert_eq!(
        soft.stats(),
        identity,
        "soft reconnect caused image retransmission or replaced sender"
    );
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
    assert_eq!(
        hard.stats(),
        identity,
        "router restart cannot replace/retransmit remote sender"
    );
    assert_eq!(hard.state.placements, expected);
    let p = hard.state.placement(7, 7);
    assert_eq!(hard.state.pixels(p), [255, 0, 0, 255]);

    // Hold the first real replay offer, then mutate through the active writer.
    // The fresh observer may not resurrect the deleted image at replay cutover.
    let (mut observer, _) = fixture.connect_pane();
    ClientMsg::Observe {
        pane_id,
        size: win_size(),
    }
    .encode(&mut observer)
    .unwrap();
    let pending = match DaemonMsg::read(&mut observer).unwrap() {
        DaemonMsg::GraphicsOffer { token, .. } => token,
        other => panic!("expected held production replay offer, got {other:?}"),
    };
    hard.quiet(b"\x1b_Ga=d,d=I,i=7,q=2\x1b\\\x1b[8;4H\x1b_Ga=T,i=8,p=8,C=1,f=32,s=1,v=1,q=2;AP8A/w==\x1b\\LIVE_NATIVE_SSH_ORDER");
    let current = hard.state.placements.clone();
    let counters = hard.stats();
    ClientMsg::GraphicsGrant {
        token: pending,
        accepted: true,
    }
    .encode(&mut observer)
    .unwrap();
    // STAT arrives from a separate production input command while recovery is
    // pending. It changes no upload/placement/byte counters.
    let serial = next_serial();
    let (mut input, _) = fixture.connect_pane();
    ClientMsg::SendInput {
        pane_id,
        bytes: format!("STAT {serial} 0 00 0\n").into_bytes(),
    }
    .encode(&mut input)
    .unwrap();
    assert!(
        matches!(DaemonMsg::read(&mut input).unwrap(), DaemonMsg::InputAck { pane_id: id } if id == pane_id)
    );
    let mut fresh = Consumer::new(observer, pane_id, true);
    let trace = fresh.collect_until(format!("KITTY_DONE_{serial};\r").as_bytes());
    assert_eq!(
        fresh.state.placements, current,
        "ordered replay events={trace:#?}"
    );
    assert_eq!(
        fresh.state.pixels(fresh.state.placement(8, 8)),
        [0, 255, 0, 255]
    );
    assert!(fresh.state.placements.values().all(|p| p.image_id != 7));
    assert!(contains(&fresh.text, b"LIVE_NATIVE_SSH_ORDER"));
    assert!(contains(
        &fresh.text,
        format!("KITTY_STATS_{serial} {counters}").as_bytes()
    ));
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
    let refused = pressure.send(
        b"\x1b_Ga=T,i=3,p=3,C=1,f=32,s=1,v=1;/wAA/w==\x1b\\after_pressure",
        1,
    );
    assert!(refused[0].starts_with(b"\x1b_Gi=3,p=3;ENOSPC:"));
    assert!(contains(&pressure.text, b"after_pressure"));
    assert_eq!(pressure.state.pixels(&protected), [0, 255, 0, 255]);
    pressure.quiet(&b"\r\n".repeat(40));
    let historical = pressure.state.placement(1, 1).resource;
    ClientMsg::GraphicsPressure { key: historical }
        .encode(&mut pressure.stream)
        .unwrap();
    pressure.stats();
    assert!(
        pressure.state.evicted.contains_key(&historical),
        "eligible oldest history must be reclaimed"
    );
    pressure.quiet(b"\x1b_Ga=T,i=3,p=3,C=1,f=32,s=1,v=1,q=2;/wAA/w==\x1b\\");
    assert_eq!(
        pressure.state.pixels(pressure.state.placement(3, 3)),
        [255, 0, 0, 255]
    );
    pressure.quiet(b"\x1b[3J");
    assert!(
        !pressure.state.evicted.contains_key(&historical),
        "expired history cannot keep an eviction notice"
    );
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
    malformed
        .write_all(&((protocol::MAX_FRAME + 1) as u32).to_le_bytes())
        .unwrap();
    malformed.write_all(&[0]).unwrap();
    let _ = malformed.shutdown(std::net::Shutdown::Write);
    match DaemonMsg::read(&mut malformed) {
        Ok(DaemonMsg::Error(error)) => assert!(!error.is_empty()),
        Err(error) => assert!(matches!(
            error.kind(),
            std::io::ErrorKind::UnexpectedEof
                | std::io::ErrorKind::ConnectionReset
                | std::io::ErrorKind::ConnectionAborted
        )),
        other => panic!("oversized frame manufactured success: {other:?}"),
    }
    assert_eq!(status(&host).pid, remote_status.pid);
    assert_eq!(host.peer().instance, control_instance);
}

#[test]
#[ignore = "requires isolated real Linux native SSH; records initial vs steady protocol bytes, not GPU FPS"]
fn native_ssh_graphics_loaded_frames_select_without_pixel_retransmission() {
    let fixture = NativeSshFixture::start(Path::new(env!("CARGO_BIN_EXE_lyrel-server")));
    let host = fixture.connect_control();
    let mut pane = Consumer::spawn(&fixture, fixture.install_sender(&host), 64 << 20, 128 << 20);
    pane.quiet(b"\x1b[6;5H\x1b_Ga=T,i=147,p=1,C=1,c=4,r=1,f=32,s=2,v=1,q=2;/wAA/wD/AP8=\x1b\\\x1b_Ga=f,i=147,f=32,s=2,v=1,z=20,q=2;AAD///////8=\x1b\\\x1b_Ga=p,i=147,p=2,P=147,Q=1,H=3,V=2,c=4,r=1,q=2\x1b\\");
    let root = pane.state.placement(147, 1).resource;
    ClientMsg::GraphicsViewport(protocol::GraphicsViewport {
        screen: pane.state.screen,
        epoch: pane.state.epoch,
        top: None,
        rows: 24,
        cols: 80,
    })
    .encode(&mut pane.stream)
    .unwrap();
    pane.quiet(b"\x1b_Ga=a,i=147,s=3,r=1,z=20,c=1,q=2\x1b\\");
    let loaded = (
        pane.resource_pixel_bytes,
        pane.graphics_wire_bytes,
        pane.select_packets,
    );
    assert!(
        loaded.0 >= 16,
        "initial loading must transfer the actual distinct root and frame pixels"
    );
    let paused_sender = pane.stats();
    let serial = next_serial();
    let gate = fixture.remote_path("steady-gate");
    fixture.input(
        pane.pane,
        format!(
            "WAIT {serial} 0 00 0 {}\n",
            hex(gate.to_str().unwrap().as_bytes())
        )
        .into_bytes(),
    );
    pane.until(format!("KITTY_WAITING_{serial}").as_bytes());
    // The application is blocked in a remote file wait and emits zero pixels
    // while the daemon loops already-loaded frames. Release the marker only
    // after a real wall interval, not after a synthetic local clock tick.
    let release = gate.clone();
    let host_for_release = host.clone();
    let worker = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(350));
        host_for_release.write_file(&release, b"release").unwrap();
    });
    pane.until(format!("KITTY_DONE_{serial};\r").as_bytes());
    worker.join().unwrap();
    assert!(
        pane.select_packets >= loaded.2 + 4,
        "the real daemon must advance and loop loaded frames while the sender is silent"
    );
    let old_screen = pane.state.screen;
    let old_epoch = pane.state.epoch;
    pane.quiet(b"\x1b[?1049h\x1b[2JALT-SSH-ANIMATION-HIDDEN");
    assert!(
        pane.state.playback[&root].paused,
        "inactive main-screen relative graph pauses while alternate text remains live"
    );
    let inactive = pane.state.playback[&root].clone();
    ClientMsg::GraphicsViewport(protocol::GraphicsViewport {
        screen: old_screen,
        epoch: old_epoch,
        top: None,
        rows: 24,
        cols: 80,
    })
    .encode(&mut pane.stream)
    .unwrap();
    pane.stats();
    assert_eq!(
        pane.state.playback[&root], inactive,
        "a stale main-screen viewport cannot resume an inactive screen"
    );
    pane.quiet(b"\x1b[?1049l");
    ClientMsg::GraphicsViewport(protocol::GraphicsViewport {
        screen: pane.state.screen,
        epoch: pane.state.epoch,
        top: None,
        rows: 0,
        cols: 80,
    })
    .encode(&mut pane.stream)
    .unwrap();
    pane.stats();
    let hidden = pane.state.playback[&root].clone();
    assert!(
        hidden.paused && hidden.remaining_ms > 0,
        "zero-row real SSH consumer suspends its exact remaining gap"
    );
    ClientMsg::GraphicsViewport(protocol::GraphicsViewport {
        screen: pane.state.screen,
        epoch: pane.state.epoch.wrapping_sub(1),
        top: None,
        rows: 24,
        cols: 80,
    })
    .encode(&mut pane.stream)
    .unwrap();
    pane.stats();
    assert_eq!(
        pane.state.playback[&root], hidden,
        "a stale viewport generation cannot resume hidden playback or reset loop progress"
    );
    pane.quiet(b"\x1b_Ga=a,i=147,s=1,c=1,q=2\x1b\\\x1b_Ga=a,i=147,s=1,c=2,q=2\x1b\\\x1b_Ga=a,i=147,s=1,c=1,q=2\x1b\\");
    assert_eq!(
        pane.resource_pixel_bytes, loaded.0,
        "loaded ticks and selections cannot carry Resource pixel payloads"
    );
    assert!(pane.select_packets >= loaded.2 + 2);
    assert!(
        pane.graphics_wire_bytes > loaded.1,
        "the real SSH graphics channel must carry pixel-free selection/playback packets"
    );
    assert_eq!(
        pane.state.placement(147, 2).parent,
        Some(pane.state.placement(147, 1).key)
    );
    assert!(pane.state.resources.contains_key(&root));
    let final_stats = pane.stats();
    for name in ["pid=", "uploads="] {
        let field = |stats: &str| {
            stats
                .split_whitespace()
                .find(|field| field.starts_with(name))
                .unwrap()
                .to_owned()
        };
        assert_eq!(field(&final_stats), field(&paused_sender));
    }
    eprintln!(
        "native SSH initial_resource_pixel_bytes={} initial_graphics_wire_bytes={} steady_resource_pixel_bytes={} steady_graphics_wire_bytes={} steady_select_packets={}",
        loaded.0,
        loaded.1,
        pane.resource_pixel_bytes - loaded.0,
        pane.graphics_wire_bytes - loaded.1,
        pane.select_packets - loaded.2
    );
    fixture.kill_pane(pane.pane);
}

#[test]
#[ignore = "requires isolated native SSH Linux daemon; explicit refusal/cancel/observer/incompatible contracts"]
fn native_ssh_graphics_failed_admission_and_peers_preserve_authority_and_controller() {
    let fixture = NativeSshFixture::start(Path::new(env!("CARGO_BIN_EXE_lyrel-server")));
    let host = fixture.connect_control();
    let daemon = status(&host);
    let instance = host.peer().instance.clone();
    let mut original =
        Consumer::spawn(&fixture, fixture.install_sender(&host), 64 << 20, 128 << 20);
    original.quiet(b"\x1b[6;5H\x1b_Ga=T,i=1,p=1,C=1,f=32,s=1,v=1,q=2;/wAA/w==\x1b\\\x1b[8;5H\x1b_Ga=T,i=2,p=1,C=1,f=32,s=1,v=1,q=2;AP8A/w==\x1b\\");
    let expected = original.state.placements.clone();
    let identity = original.stats();
    for cancelled in [false, true] {
        let (mut pending, _) = fixture.connect_pane();
        ClientMsg::Attach {
            pane_id: original.pane,
            size: win_size(),
            allow_remote_clipboard_write: false,
        }
        .encode(&mut pending)
        .unwrap();
        let first = match DaemonMsg::read(&mut pending).unwrap() {
            DaemonMsg::GraphicsOffer { token, .. } => token,
            other => panic!("expected real pending offer: {other:?}"),
        };
        ClientMsg::GraphicsGrant {
            token: first,
            accepted: true,
        }
        .encode(&mut pending)
        .unwrap();
        if cancelled {
            // Dropping the actual SSH-backed channel abandons a partially
            // admitted attach, not a local synthetic reservation.
            drop(pending);
        } else {
            let mut released = false;
            loop {
                match DaemonMsg::read(&mut pending).expect("bounded explicit replay refusal") {
                    DaemonMsg::GraphicsOffer { token, .. } => ClientMsg::GraphicsGrant {
                        token,
                        accepted: false,
                    }
                    .encode(&mut pending)
                    .unwrap(),
                    DaemonMsg::GraphicsCancel { token } => released |= token == first,
                    DaemonMsg::Error(error) => {
                        assert!(
                            error.contains("cannot admit retained session content"),
                            "{error}"
                        );
                        break;
                    }
                    DaemonMsg::Graphics(_) | DaemonMsg::Snapshot(_) => {
                        panic!("partially granted attach leaked retained content")
                    }
                    other => panic!("unexpected refusal event {other:?}"),
                }
            }
            assert!(
                released,
                "accepted early grant must be cancelled on later refusal"
            );
        }
        assert_eq!(
            original.stats(),
            identity,
            "failed attach cannot steal control or request new pixels"
        );
        assert_eq!(original.state.placements, expected);
    }
    let (mut observer, _) = fixture.connect_pane();
    ClientMsg::GraphicsCapacity {
        encoded_bytes: 64 << 20,
        decoded_bytes: 128 << 20,
        offers: false,
    }
    .encode(&mut observer)
    .unwrap();
    ClientMsg::Observe {
        pane_id: original.pane,
        size: win_size(),
    }
    .encode(&mut observer)
    .unwrap();
    // Ordered replay's Screen is the subscription barrier. Observers cannot
    // mutate the authoritative pane through input, clear, resize or pressure.
    loop {
        if let DaemonMsg::Graphics(frame) = DaemonMsg::read(&mut observer).unwrap()
            && matches!(
                GraphicsUpdate::decode_frame_owned(frame.into_vec()),
                Some(GraphicsUpdate::Screen { .. })
            )
        {
            break;
        }
    }
    ClientMsg::Input(b"OBSERVER_MUST_NOT_CONTROL\n".to_vec())
        .encode(&mut observer)
        .unwrap();
    loop {
        match DaemonMsg::read(&mut observer).unwrap() {
            DaemonMsg::Error(error) => {
                assert!(error.contains("read-only observer"), "{error}");
                break;
            }
            DaemonMsg::Exited { .. } => panic!("observer error killed Linux sender"),
            _ => {}
        }
    }
    assert_eq!(original.stats(), identity);
    for peer in [
        0,
        protocol::PROTOCOL_VERSION - 1,
        protocol::PROTOCOL_VERSION + 1,
    ] {
        let mut invalid = fixture.connect_unnegotiated_pane();
        if peer == 0 {
            ClientMsg::Observe {
                pane_id: original.pane,
                size: win_size(),
            }
            .encode(&mut invalid)
            .unwrap();
            assert!(
                matches!(DaemonMsg::read(&mut invalid).unwrap(), DaemonMsg::Error(error) if error.contains("handshake required"))
            );
        } else {
            ClientMsg::Negotiate { protocol: peer }
                .encode(&mut invalid)
                .unwrap();
            assert!(
                matches!(DaemonMsg::read(&mut invalid).unwrap(), DaemonMsg::Version(version) if version.protocol == protocol::PROTOCOL_VERSION)
            );
            assert!(
                DaemonMsg::read(&mut invalid).is_err(),
                "incompatible remote peer remained subscribed"
            );
        }
    }
    let (mut missing, _) = fixture.connect_pane();
    ClientMsg::Attach {
        pane_id: u64::MAX,
        size: win_size(),
        allow_remote_clipboard_write: false,
    }
    .encode(&mut missing)
    .unwrap();
    assert!(
        matches!(DaemonMsg::read(&mut missing).unwrap(), DaemonMsg::Error(error) if !error.is_empty())
    );
    assert_eq!(original.stats(), identity);
    let pane = original.pane;
    original.detach();
    let mut fresh = attach(&fixture, pane);
    assert_eq!(
        fresh.stats(),
        identity,
        "failed/refused/cancelled consumers cannot leak capacity or replace the living sender"
    );
    assert_eq!(fresh.state.placements, expected);
    assert_eq!(
        fresh.state.pixels(fresh.state.placement(1, 1)),
        [255, 0, 0, 255]
    );
    assert_eq!(
        fresh.state.pixels(fresh.state.placement(2, 1)),
        [0, 255, 0, 255]
    );
    drop(fresh);
    assert_eq!(status(&host).pid, daemon.pid);
    assert_eq!(host.peer().instance, instance);
    fixture.kill_pane(pane);
}

fn native_ssh_media_case(nested: bool) {
    let fixture = NativeSshFixture::start(Path::new(env!("CARGO_BIN_EXE_lyrel-server")));
    let host = fixture.connect_control();
    let desktop = tempfile::tempdir().unwrap();
    let same_named = desktop.path().join(format!(
        "tty-graphics-protocol-{}.rgba",
        desktop.path().file_name().unwrap().to_string_lossy()
    ));
    let sentinel = [91, 72, 53, 255];
    std::fs::write(&same_named, sentinel).unwrap();
    let mut args = vec![
        "--desktop-path".into(),
        same_named.to_string_lossy().into_owned(),
    ];
    if nested {
        args.push("--nested".into());
    }
    let shell = fixture.install_script(
        &host,
        "kitty_ssh_media.mjs",
        include_bytes!("../../../tests/support/kitty_ssh_media.mjs"),
        args,
    );
    let shell = if nested {
        fixture.nested_shell(shell)
    } else {
        shell
    };
    let mut pane = Consumer::spawn(&fixture, shell, 64 << 20, 128 << 20);
    ClientMsg::Input(b"RUN\n".to_vec())
        .encode(&mut pane.stream)
        .unwrap();
    pane.until(b"KITTY_REMOTE_MEDIA_OK");
    assert_eq!(
        std::fs::read(&same_named).unwrap(),
        sentinel,
        "Linux file/temp/shm lookup must never read or delete the desktop object"
    );
    for id in 1471..=1473 {
        if nested {
            assert!(
                pane.state
                    .placements
                    .values()
                    .all(|placement| placement.image_id != id),
                "nested indirect locality must fail closed"
            );
        } else {
            assert_eq!(
                pane.state.pixels(pane.state.placement(id, id)),
                [12, 34, 56, 255]
            );
        }
    }
    assert_eq!(
        pane.state.pixels(pane.state.placement(1474, 1474)),
        [255, 0, 255, 255],
        "direct RGBA remains usable after every indirect refusal"
    );
    ClientMsg::Input(b"QUIT\n".to_vec())
        .encode(&mut pane.stream)
        .unwrap();
    loop {
        if let DaemonMsg::Exited { code } = DaemonMsg::read(&mut pane.stream).unwrap() {
            assert_eq!(code, Some(0), "remote media lifecycle/assertion failed");
            break;
        }
    }
    assert_eq!(std::fs::read(&same_named).unwrap(), sentinel);
}

#[test]
#[ignore = "requires isolated native SSH Linux with writable /dev/shm; real file/temp/POSIX-shm positive lifecycle"]
fn native_ssh_graphics_remote_file_temp_posix_shm_preserve_desktop_names() {
    native_ssh_media_case(false);
}

#[test]
#[ignore = "also requires explicit Linux-side nested SSH key/pin/endpoint; never copies desktop credentials"]
fn native_ssh_graphics_nested_ssh_refuses_indirect_and_preserves_direct() {
    native_ssh_media_case(true);
}

// The exact Ubuntu `libsixel-bin 1.10.5-1build1` output of
// `img2sixel -w 40 -h 40 -p 4 -d none colors.ppm` for the 2x2 red/green over
// blue/yellow PPM. The daemon's decoder yields `[184, 51, 61]` for the first
// pixel (the production GUI sees it as BGRA). A recorded real-producer payload
// is used because the fixed Linux fixture need not install img2sixel.
const SIXEL_PRODUCER_OUTPUT: &[u8] = b"\x1bPq\"1;1;40;40#0;2;72;20;24#1;2;24;74;11#2;2;74;74;24#3;2;24;20;66#0!19~#1!21~-#0!19~_$#1!19?^!20~-#0!20~#1^!19~$#2!20?_-#0!13BFFN^~~~#2~}!6{!12w$#3!13{wwo_#1!4?@!6B!12F-#3!18~}w#2!20~$#0!18?@F-#3!20~#2!20~-#3!20N#2!20N\x1b\\";

/// 250 SGR resets per line consume the production 8 MiB replay ring without
/// occupying retained cell rows, so the original colour OSC (or the image's
/// precedence rows) leaves the ring while the accepted authority remains.
fn ring_chunk(lines: usize) -> Vec<u8> {
    let mut line = b"\x1b[0m".repeat(250);
    line.extend_from_slice(b"\r\n");
    let mut chunk = Vec::with_capacity(lines * line.len());
    for _ in 0..lines {
        chunk.extend_from_slice(&line);
    }
    chunk
}

/// The production replay ring is exactly 8 MiB, but eviction advances to the
/// next parseable boundary and can drop the remainder of a sequence, so a real
/// ring tail is at most a few bytes short of the cap.
const RING_TAIL_FLOOR: u64 = (8 << 20) - 4096;

fn sender_bytes(stats: &str) -> usize {
    stats
        .split_whitespace()
        .find_map(|field| field.strip_prefix("bytes="))
        .expect("real sender byte counter")
        .parse()
        .expect("real sender byte count")
}

#[test]
#[ignore = "requires the updated fixture SSH endpoint (pane-colors/static-sixel features), pinned host key, identity, private Linux home and Node; never silently skips"]
fn native_ssh_pane_colors_projection_survives_ring_eviction_and_fresh_consumer() {
    let fixture = NativeSshFixture::start(Path::new(env!("CARGO_BIN_EXE_lyrel-server")));
    let host = fixture.connect_control();
    let daemon = status(&host);
    let instance = host.peer().instance.clone();
    let (probe, endpoint) = fixture.connect_pane();
    assert_eq!(endpoint.protocol, protocol::PROTOCOL_VERSION);
    assert!(
        endpoint.has_feature(FEATURE_PANE_COLORS),
        "actual remote endpoint must own authoritative pane colours: {endpoint:?}"
    );
    assert!(
        endpoint.has_feature(FEATURE_STATIC_SIXEL),
        "the same updated candidate must carry the static-sixel feature: {endpoint:?}"
    );
    eprintln!(
        "ACTUAL_SSH_PANE_COLORS_ENDPOINT build={} instance={} features={:?}",
        endpoint.build, endpoint.instance, endpoint.features
    );
    drop(probe);

    // The opt-in is a real handshake command, not a client-side filter: a peer
    // that sends it before negotiating is refused and cannot fake the feature.
    let mut unnegotiated = fixture.connect_unnegotiated_pane();
    ClientMsg::SubscribePaneColors
        .encode(&mut unnegotiated)
        .unwrap();
    match DaemonMsg::read(&mut unnegotiated).unwrap() {
        DaemonMsg::Error(error) => assert!(
            error.contains("handshake required before pane colours"),
            "unexpected refusal: {error}"
        ),
        other => panic!("pane colours accepted before handshake: {other:?}"),
    }
    assert!(
        DaemonMsg::read(&mut unnegotiated).is_err(),
        "the refused stream must close rather than keep a colour subscription"
    );

    let mut pane = Consumer::spawn(&fixture, fixture.install_sender(&host), 64 << 20, 128 << 20);
    let pane_id = pane.pane;

    // A real Linux PTY application sets the indexed palette and all named
    // defaults in one ordered burst. These are live deltas over the theme.
    pane.quiet(b"\x1b]4;1;#112233\x07\x1b]10;#0a0b0c\x07\x1b]11;#101112\x07\x1b]12;#131415\x07");
    assert_eq!(pane.state.colors.get(1), Some([0x11, 0x22, 0x33]));
    assert_eq!(pane.state.colors.get(DEFAULT_FG), Some([0x0a, 0x0b, 0x0c]));
    assert_eq!(pane.state.colors.get(DEFAULT_BG), Some([0x10, 0x11, 0x12]));
    assert_eq!(pane.state.colors.get(CURSOR), Some([0x13, 0x14, 0x15]));
    assert!(
        pane.state.color_frames > 0,
        "the opted-in live consumer must receive real ColorFrame messages"
    );

    // Queries and malformed values publish no override change, and ordinary
    // text after the refused sequence still reaches the ordered stream.
    let frames = pane.state.color_frames;
    pane.quiet(b"\x1b]4;1;?\x07\x1b]10;?\x07\x1b]4;1;rgb:zz/00/00\x07AFTER_BAD_COLOR");
    assert_eq!(
        pane.state.color_frames, frames,
        "a colour probe or malformed value is not a live override change"
    );
    assert_eq!(pane.state.colors.get(1), Some([0x11, 0x22, 0x33]));
    assert!(contains(&pane.text, b"AFTER_BAD_COLOR"));

    // Reset every layer, then accept a smaller final layer. A reset reveals
    // the current defaults (None), never a captured theme snapshot.
    pane.quiet(b"\x1b]104;1\x07\x1b]110\x07\x1b]111\x07\x1b]112\x07");
    for slot in [1, DEFAULT_FG, DEFAULT_BG, CURSOR] {
        assert_eq!(
            pane.state.colors.get(slot),
            None,
            "reset slot {slot} must return to the live theme"
        );
    }
    pane.quiet(b"\x1b]4;1;#445566\x07\x1b]11;#202122\x07");
    let accepted = pane.state.colors;
    assert_eq!(accepted.get(1), Some([0x44, 0x55, 0x66]));
    assert_eq!(accepted.get(DEFAULT_BG), Some([0x20, 0x21, 0x22]));
    assert_eq!(accepted.get(DEFAULT_FG), None);
    assert_eq!(accepted.get(CURSOR), None);

    // The production opt-in command is real for read-only peers too: after
    // `SubscribePaneColors` the observer receives the authoritative snapshot.
    let mut opted_in = fixture.connect_unnegotiated_pane();
    ClientMsg::Negotiate {
        protocol: protocol::PROTOCOL_VERSION,
    }
    .encode(&mut opted_in)
    .unwrap();
    let version = match DaemonMsg::read(&mut opted_in).unwrap() {
        DaemonMsg::Version(version) => version,
        other => panic!("expected negotiated endpoint, got {other:?}"),
    };
    assert!(version.has_feature(FEATURE_PANE_COLORS));
    ClientMsg::SubscribePaneColors
        .encode(&mut opted_in)
        .unwrap();
    ClientMsg::GraphicsCapacity {
        encoded_bytes: 64 << 20,
        decoded_bytes: 128 << 20,
        offers: true,
    }
    .encode(&mut opted_in)
    .unwrap();
    ClientMsg::Observe {
        pane_id,
        size: win_size(),
    }
    .encode(&mut opted_in)
    .unwrap();
    let opted_snapshot;
    loop {
        match DaemonMsg::read(&mut opted_in).unwrap() {
            DaemonMsg::Colors(bytes) => {
                assert!(
                    bytes.len() <= protocol::MAX_FRAME,
                    "bounded authoritative colour frame"
                );
                let frame =
                    ColorFrame::decode(&bytes).expect("production pane-colour frame must decode");
                assert!(frame.is_snapshot(), "an attach carries a full snapshot");
                let mut colors = PaneColors::default();
                frame.apply(&mut colors);
                opted_snapshot = colors;
                break;
            }
            DaemonMsg::Error(error) => panic!("opted-in observer refused: {error}"),
            DaemonMsg::Exited { code } => panic!("opted-in observer pane exited: {code:?}"),
            _ => {}
        }
    }
    assert_eq!(
        opted_snapshot, accepted,
        "the explicit opt-in must deliver the same decoded authority"
    );
    drop(opted_in);

    // A consumer that does not opt in must never receive the frame kind. The
    // replay tail is the last real DONE marker; no Colors frame may follow it.
    let marker = format!("KITTY_DONE_{};\r", pane.serial);
    let mut raw = fixture.connect_unnegotiated_pane();
    ClientMsg::Negotiate {
        protocol: protocol::PROTOCOL_VERSION,
    }
    .encode(&mut raw)
    .unwrap();
    let version = match DaemonMsg::read(&mut raw).unwrap() {
        DaemonMsg::Version(version) => version,
        other => panic!("expected negotiated endpoint, got {other:?}"),
    };
    assert!(version.has_feature(FEATURE_PANE_COLORS));
    ClientMsg::GraphicsCapacity {
        encoded_bytes: 64 << 20,
        decoded_bytes: 128 << 20,
        offers: true,
    }
    .encode(&mut raw)
    .unwrap();
    // Observe, not Attach: attaching would replace the controller under test.
    ClientMsg::Observe {
        pane_id,
        size: win_size(),
    }
    .encode(&mut raw)
    .unwrap();
    let mut replay = Vec::new();
    loop {
        match DaemonMsg::read(&mut raw).unwrap() {
            DaemonMsg::Colors(_) => panic!("a peer that did not opt in received pane colours"),
            DaemonMsg::Output(bytes) | DaemonMsg::Snapshot(bytes) => {
                replay.extend(bytes);
                if contains(&replay, marker.as_bytes()) {
                    break;
                }
            }
            DaemonMsg::Exited { code } => panic!("un-opted-in replay pane exited: {code:?}"),
            _ => {}
        }
    }
    raw.set_read_timeout(Some(Duration::from_millis(1000)))
        .unwrap();
    loop {
        match DaemonMsg::read(&mut raw) {
            Ok(DaemonMsg::Colors(_)) => {
                panic!("a peer that did not opt in received pane colours")
            }
            Ok(_) => {}
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                break;
            }
            Err(error) => panic!("un-opted-in replay drained with {error}"),
        }
    }
    drop(raw);

    // More than the production ring, through the real Linux PTY. Each request
    // stays under the daemon's 1 MiB Input admission cap (the hex payload is
    // twice the emitted bytes); 18 x 500 lines is 9,018,000 bytes, past the
    // 8 MiB ring. The original OSC bytes are gone; the accepted layer is not.
    for _ in 0..18 {
        pane.quiet(&ring_chunk(500));
    }
    let stopped = pane.stats();
    assert!(
        sender_bytes(&stopped) > 8 << 20,
        "the real sender must write past the 8 MiB replay ring: {stopped}"
    );
    pane.detach();
    let mut fresh = attach(&fixture, pane_id);
    let replay_stats = fresh.stats();
    assert_eq!(
        fresh.state.colors, accepted,
        "a fresh consumer restores the decoded layer, not a copy of old OSC bytes"
    );
    assert!(
        fresh.state.color_snapshots >= 1,
        "an attach must carry a full authoritative snapshot"
    );
    assert!(
        fresh.text_bytes >= RING_TAIL_FLOOR,
        "the fresh replay is the real ring tail, not a small snapshot: {} bytes",
        fresh.text_bytes
    );
    for stale in [
        b"\x1b]4;1;#445566".as_slice(),
        b"\x1b]11;#202122".as_slice(),
    ] {
        assert!(
            !contains(&fresh.text, stale),
            "the original colour OSC left the replay ring"
        );
    }
    assert_eq!(
        replay_stats, stopped,
        "a fresh consumer must not make the living sender resend colour state"
    );
    assert_eq!(status(&host).pid, daemon.pid);
    assert_eq!(host.peer().instance, instance);
    fixture.kill_pane(pane_id);
}

#[test]
#[ignore = "requires the updated fixture SSH endpoint (static-sixel feature), pinned host key, identity, private Linux home and Node; never silently skips"]
fn native_ssh_recorded_sixel_survives_ring_eviction_and_fresh_consumer() {
    let fixture = NativeSshFixture::start(Path::new(env!("CARGO_BIN_EXE_lyrel-server")));
    let host = fixture.connect_control();
    let daemon = status(&host);
    let instance = host.peer().instance.clone();
    let (probe, endpoint) = fixture.connect_pane();
    assert!(
        endpoint.has_feature(FEATURE_STATIC_SIXEL),
        "actual remote endpoint must own static Sixel: {endpoint:?}"
    );
    eprintln!(
        "ACTUAL_SSH_SIXEL_ENDPOINT build={} instance={} features={:?}",
        endpoint.build, endpoint.instance, endpoint.features
    );
    drop(probe);

    let mut pane = Consumer::spawn(&fixture, fixture.install_sender(&host), 64 << 20, 128 << 20);
    let pane_id = pane.pane;

    // The daemon owns discovery through the real SSH route: colour registers,
    // the pane's current geometry, refused set/reset and ReGIS, in order.
    assert_eq!(
        pane.exchange("SEND", b"\x1b[?1;1;0S\x1b[?2;1;0S\x1b[?2;4S\x1b[14t", 1, 0),
        [b"\x1b[?1;0;1024S\x1b[?2;0;800;480S\x1b[?2;3S\x1b[4;480;800t".to_vec()],
        "the updated daemon answers Sixel discovery itself; a desktop mirror does not guess"
    );

    // The recorded official producer output lands through the real Linux PTY.
    // Its DCS is extracted before the text ring, so the following text is the
    // only Sixel-adjacent data in the ordered text stream.
    // RIS gives the same zero-based screen the core producer test uses; `2J`
    // would fold the preceding viewport into history before the anchor is taken.
    let mut accepted_bytes = b"\x1bc\x1b[6;1HEARLY_SIXEL\r".to_vec();
    accepted_bytes.extend_from_slice(SIXEL_PRODUCER_OUTPUT);
    accepted_bytes.extend_from_slice(b"AFTER_EARLY");
    pane.quiet(&accepted_bytes);
    assert!(
        !contains(&pane.text, b"\x1bPq"),
        "the Sixel DCS is extracted before the text replay"
    );
    assert!(contains(&pane.text, b"EARLY_SIXEL"));
    assert!(contains(&pane.text, b"AFTER_EARLY"));
    let sixel = pane
        .state
        .placements
        .values()
        .find(|placement| placement.z == -1)
        .expect("accepted static Sixel placement")
        .clone();
    assert_eq!(
        (sixel.anchor_row, sixel.anchor_col),
        (5, 0),
        "Sixel anchors at the preceding text cursor, not at the pixel extent; history_origin={} text_tail={:?}",
        pane.state.history_origin,
        String::from_utf8_lossy(&pane.text[pane.text.len().saturating_sub(512)..])
    );
    assert_eq!(
        (sixel.span_cols, sixel.span_rows),
        (4, 2),
        "40x40 pixels at 10x20 cells occupy four columns and two rows"
    );
    let image = &pane.state.resources[&sixel.resource];
    assert_eq!((image.width, image.height), (40, 40));
    let accepted_pixels = pane.state.pixels(&sixel);
    assert_eq!(&accepted_pixels[..4], [184, 51, 61, 255]);
    assert_eq!(
        &accepted_pixels[(39 * 40 + 39) * 4..][..4],
        [189, 189, 61, 255]
    );

    // Hostile bounded refusals cannot replace the accepted placement and do not
    // swallow the ordinary text that follows them.
    let accepted_placements = pane.state.placements.clone();
    pane.quiet(b"\x1bPq\"1;1;65535;65535~\x1b\\\x1bPq#0;2;101;0;0~\x1b\\AFTER_REFUSED_SIXEL");
    assert_eq!(
        pane.state.placements, accepted_placements,
        "a refused Sixel cannot replace or remove an accepted placement"
    );
    assert_eq!(pane.state.pixels(&sixel), accepted_pixels);
    assert!(contains(&pane.text, b"AFTER_REFUSED_SIXEL"));

    // Push the original precedence rows out of the production 8 MiB ring. Each
    // request stays under the daemon's 1 MiB Input cap; the image authority is
    // independent of those bytes; the sender then waits.
    for _ in 0..18 {
        pane.quiet(&ring_chunk(500));
    }
    let stopped = pane.stats();
    assert!(
        sender_bytes(&stopped) > 8 << 20,
        "the real sender must write past the 8 MiB replay ring: {stopped}"
    );
    let expected = pane.state.placements.clone();
    let expected_pixels = pane.state.pixels(
        pane.state
            .placements
            .values()
            .find(|placement| placement.z == -1)
            .expect("placement survives the ring trim"),
    );
    pane.detach();
    let mut fresh = attach(&fixture, pane_id);
    let replay_stats = fresh.stats();
    assert_eq!(
        fresh.state.placements, expected,
        "the fresh consumer restores accepted placement authority"
    );
    let restored = fresh
        .state
        .placements
        .values()
        .find(|placement| placement.z == -1)
        .expect("restored static Sixel placement");
    assert_eq!(fresh.state.pixels(restored), expected_pixels);
    let restored_image = &fresh.state.resources[&restored.resource];
    assert_eq!((restored_image.width, restored_image.height), (40, 40));
    assert!(
        fresh.text_bytes >= RING_TAIL_FLOOR,
        "the fresh replay is the real ring tail, not a small snapshot: {} bytes",
        fresh.text_bytes
    );
    assert!(
        !contains(&fresh.text, b"\x1bPq"),
        "the original DCS never rides the fresh text replay"
    );
    assert_eq!(
        replay_stats, stopped,
        "the living producer must not retransmit accepted Sixel on attach"
    );
    assert_eq!(status(&host).pid, daemon.pid);
    assert_eq!(host.peer().instance, instance);
    fixture.kill_pane(pane_id);
}
