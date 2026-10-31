//! Opt-in native acceptance driver. The GUI is the unmodified production app:
//! standard config -> remote workspace -> machine tree -> rooted TerminalViews.
#[path = "../../../tests/support/native_ssh_graphics.rs"]
mod fixture;

use std::path::{Path, PathBuf};
use std::process::Child;
use std::time::{Duration, Instant};

use fixture::{NativeSshFixture, win_size};
use lyrel_core::daemon::control::{ControlRequest, ReplyOk};
use lyrel_core::daemon::protocol::{ClientMsg, DaemonMsg, ShellSpec};

fn required(name: &str) -> String {
    std::env::var(name).ok().filter(|value| !value.is_empty()).unwrap_or_else(|| panic!("native SSH surface prerequisite missing: {name}"))
}

fn quote(text: &str) -> String { format!("'{}'", text.replace('\'', "'\\''")) }

fn spawn(fixture: &NativeSshFixture, shell: ShellSpec) -> u64 {
    let (mut stream, _) = fixture.connect_pane();
    ClientMsg::Spawn { cwd: Some(fixture.remote_home().into()), size: win_size(), shell: Some(shell), owner: None, workspace: None, restore: None, allow_remote_clipboard_write: false, await_claim: false }.encode(&mut stream).unwrap();
    let pane = match DaemonMsg::read(&mut stream).unwrap() {
        DaemonMsg::Spawned { pane_id } => pane_id,
        other => panic!("real Linux native surface spawn: {other:?}"),
    };
    fixture.track_pane(pane);
    let mut text = Vec::new();
    loop {
        match DaemonMsg::read(&mut stream).unwrap() {
            DaemonMsg::Output(bytes) => text.extend(bytes),
            DaemonMsg::Exited { code } => panic!("native scene exited before its real PTY startup: {code:?}"),
            DaemonMsg::Error(error) => panic!("native scene failed: {error}"),
            _ => {}
        }
        if text.windows(b"KITTY_NATIVE_READY".len()).any(|part| part == b"KITTY_NATIVE_READY") { break; }
        assert!(text.len() < 1 << 20, "bounded native startup capture");
    }
    ClientMsg::Detach.encode(&mut stream).unwrap();
    pane
}

struct OwnedGui(Option<Child>);
impl Drop for OwnedGui {
    fn drop(&mut self) {
        if let Some(child) = &mut self.0 { let _ = child.kill(); let _ = child.wait(); }
    }
}

#[derive(Default, Clone, serde::Serialize)]
struct WireCounts {
    graphics_bytes: u64,
    resource_pixel_bytes: u64,
    resource_packets: u64,
    select_packets: u64,
    text_bytes: u64,
    error: Option<String>,
}

struct WireTap {
    counts: std::sync::Arc<std::sync::Mutex<WireCounts>>,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    socket: lyrel_core::daemon::transport::Stream,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl WireTap {
    fn start(fixture: &NativeSshFixture, pane: u64, log: PathBuf) -> Self {
        use std::io::Write;
        use lyrel_core::core::kitty_graphics::GraphicsUpdate;
        let (mut stream, _) = fixture.connect_pane_with_capacity(512 << 20, 512 << 20);
        ClientMsg::Observe { pane_id: pane, size: win_size() }.encode(&mut stream).unwrap();
        stream.set_read_timeout(None).unwrap();
        let socket = stream.try_clone().unwrap();
        let counts = std::sync::Arc::new(std::sync::Mutex::new(WireCounts::default()));
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let saved = counts.clone();
        let stopped = stop.clone();
        let (ready, subscribed) = std::sync::mpsc::sync_channel(1);
        let worker = std::thread::spawn(move || {
            let mut text = std::fs::File::create(log).unwrap();
            let mut ready = Some(ready);
            loop {
                let result = (|| -> std::io::Result<()> {
                    match DaemonMsg::read(&mut stream)? {
                        DaemonMsg::GraphicsOffer { token, encoded_bytes, width, height, .. } => {
                            assert!(encoded_bytes <= 512 << 20 && u64::from(width) * u64::from(height) * 4 <= 512 << 20);
                            ClientMsg::GraphicsGrant { token, accepted: true }.encode(&mut stream)?;
                        }
                        DaemonMsg::Graphics(frame) => {
                            let bytes = frame.len() as u64;
                            let update = GraphicsUpdate::decode_frame_owned(frame.into_vec()).expect("production graphics packet");
                            let mut counts = saved.lock().unwrap();
                            counts.graphics_bytes += bytes;
                            match update {
                                GraphicsUpdate::Resource { image, .. } => { counts.resource_pixel_bytes += image.data.len() as u64; counts.resource_packets += 1; }
                                GraphicsUpdate::Select { .. } => counts.select_packets += 1,
                                GraphicsUpdate::Screen { screen, epoch, .. } => {
                                    // Sampling is never a visible GUI. A zero-row
                                    // observer cannot keep animation alive when
                                    // all ordinary windows/panes are hidden.
                                    ClientMsg::GraphicsViewport(lyrel_core::daemon::protocol::GraphicsViewport { screen, epoch, top: None, rows: 0, cols: 80 }).encode(&mut stream)?;
                                    if let Some(ready) = ready.take() { let _ = ready.send(()); }
                                }
                                _ => {}
                            }
                        }
                        DaemonMsg::Snapshot(bytes) | DaemonMsg::Output(bytes) => {
                            let mut counts = saved.lock().unwrap();
                            counts.text_bytes += bytes.len() as u64;
                            if counts.text_bytes <= 8 << 20 { text.write_all(&bytes)?; text.flush()?; }
                        }
                        DaemonMsg::Error(error) => return Err(std::io::Error::other(error)),
                        DaemonMsg::Exited { code } => return Err(std::io::Error::other(format!("native pane exited: {code:?}"))),
                        _ => {}
                    }
                    Ok(())
                })();
                if let Err(error) = result {
                    if !stopped.load(std::sync::atomic::Ordering::Acquire) { saved.lock().unwrap().error = Some(error.to_string()); }
                    break;
                }
            }
        });
        subscribed.recv_timeout(fixture::STREAM_WITHIN).expect("real SSH wire sampler Screen subscription barrier");
        Self { counts, stop, socket, worker: Some(worker) }
    }

    fn snapshot(&self) -> WireCounts {
        let counts = self.counts.lock().unwrap().clone();
        assert!(counts.error.is_none(), "native SSH wire sampler failed: {:?}", counts.error);
        counts
    }
}

impl Drop for WireTap {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Release);
        let _ = self.socket.shutdown(std::net::Shutdown::Both);
        if let Some(worker) = self.worker.take() { worker.join().expect("native wire sampler"); }
    }
}

#[test]
#[ignore = "opens an isolated real native desktop; requires explicit same-commit app/Linux daemon and human desktop approval"]
fn native_ssh_graphics_standard_app_surface() {
    let app = PathBuf::from(required("LYREL_TEST_SSH_NATIVE_APP"));
    assert!(app.is_absolute() && app.is_file());
    let output = PathBuf::from(required("LYREL_TEST_SSH_NATIVE_OUTPUT"));
    assert!(output.is_absolute() && output.is_dir(), "explicit owned evidence parent directory required");
    let evidence = tempfile::Builder::new().prefix("native-ssh147-").tempdir_in(output).unwrap().keep();
    let scene = required("LYREL_TEST_SSH_NATIVE_SCENE");
    let script = match scene.as_str() {
        "frames" => "kitty-frames.mjs",
        "partial" => "kitty-frames.mjs",
        "playback" => "kitty-playback.mjs",
        "mutations" => "kitty-frame-mutations.mjs",
        "relative" => "kitty-relative.mjs",
        "capacity" => "kitty-capacity.mjs",
        _ => panic!("native scene must be frames/partial/playback/mutations/relative/capacity"),
    };
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts").join(script);
    let body = std::fs::read(&source).unwrap_or_else(|error| panic!("same-integration workload {} unavailable: {error}", source.display()));
    let mut fixture = NativeSshFixture::start(Path::new(env!("CARGO_BIN_EXE_lyrel-server")));
    let mut host = fixture.connect_control();
    let before = fixture::status(&host);
    let instance = host.peer().instance.clone();
    let mut panes = [0; 2];
    for (index, pane) in panes.iter_mut().enumerate() {
        let args = if scene == "capacity" { vec!["--pane".into(), index.to_string()] } else { Vec::new() };
        let sender = fixture.install_script(&host, script, &body, args);
        let command = std::iter::once(sender.program.as_str()).chain(sender.args.iter().map(String::as_str)).map(quote).collect::<Vec<_>>().join(" ");
        // Keep the same real PTY/shell when the sender exits: fresh GUIs must
        // restore retained daemon state without rerunning the producer.
        let shell = ShellSpec { program: "/bin/sh".into(), args: vec!["-c".into(), format!("printf '\\r\\nKITTY_NATIVE_READY\\r\\n'; uname -sr; {} --version; {command}; printf '\\r\\nKITTY_NATIVE_SENDER_EXIT=%s\\r\\n' \"$?\"; exec /bin/sh", quote(&sender.program))], args_are_lyrel_defaults: false };
        *pane = spawn(&fixture, shell);
    }
    let (config, workspace) = fixture.prepare_native_workspace(&host, panes);
    let mut taps: Vec<_> = panes.iter().enumerate().map(|(index, pane)| WireTap::start(&fixture, *pane, evidence.join(format!("pane-{index}.pty.log")))).collect();
    let manifest = serde_json::json!({
        "scope": "ordinary production app / native router / real SSH / Linux daemon / actual Linux PTY / remote machine tree / two TerminalViews",
        "scene": scene, "source": source, "localBuild": env!("CARGO_PKG_VERSION"), "remoteBuild": host.peer().build,
        "protocol": before.protocol_version, "control": before.control_version, "remotePid": before.pid, "remoteInstance": instance,
        "config": config, "home": fixture.native_home(), "app": app, "workspace": workspace, "panes": panes,
        "host": fixture.spec().host, "port": fixture.spec().port, "remoteHome": fixture.remote_home(),
        "conditions": { "fixedEd25519Pin": true, "strictHostKeyVerification": true, "agentForwarding": false, "clipboardWrite": false },
        "wireCounting": "actual decoded native SSH pane Graphics bytes; Resource image payload bytes separate from Select packets; not encrypted TCP byte totals",
        "requestFile": evidence.join("request.json"), "holdSeconds": 1800,
        "acceptance": "not yet observed; process startup and contracts do not prove native pixels, cadence or performance"
    });
    std::fs::write(evidence.join("session.json"), serde_json::to_vec_pretty(&manifest).unwrap()).unwrap();
    let mut gui = OwnedGui(Some(fixture.native_app_command(&app).spawn().expect("launch only the isolated production GUI")));
    eprintln!("NATIVE_SSH_SURFACE {}", evidence.display());
    let deadline = Instant::now() + Duration::from_secs(1800);
    let mut serial = 0;
    loop {
        assert!(Instant::now() < deadline, "native acceptance deadline expired; no success is inferred");
        let request = evidence.join("request.json");
        if let Ok(bytes) = std::fs::read(&request) {
            assert!(bytes.len() < 64 << 10, "bounded native control request");
            // The operator writes a temp file and atomically renames it. No
            // partially written JSON is treated as a native action.
            let request: serde_json::Value = serde_json::from_slice(&bytes).expect("atomic request.json");
            let next = request["serial"].as_u64().expect("monotonic request serial");
            if next > serial {
                match request["action"].as_str().expect("native request action") {
                    "input" => {
                        let index = request["pane"].as_u64().filter(|index| *index < 2).expect("pane index 0/1") as usize;
                        let input = request["input"].as_str().expect("exact synthetic sender command/input");
                        fixture.input(panes[index], input.as_bytes().to_vec());
                    }
                    "reopen" => {
                        if let Some(child) = &mut gui.0 { assert!(child.try_wait().unwrap().is_some(), "close the previous owned GUI before fresh-process recovery"); }
                        gui.0 = Some(fixture.native_app_command(&app).spawn().unwrap());
                    }
                    "sample" => {
                        let label = request["label"].as_str().filter(|label| !label.is_empty() && label.len() <= 48 && label.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')).expect("ASCII sample label");
                        let counts: Vec<_> = taps.iter().map(WireTap::snapshot).collect();
                        let sample = serde_json::json!({ "serial": next, "label": label, "elapsedMs": 1800000 - deadline.saturating_duration_since(Instant::now()).as_millis(), "panes": counts });
                        std::fs::write(evidence.join(format!("sample-{next}-{label}.json")), serde_json::to_vec_pretty(&sample).unwrap()).unwrap();
                    }
                    "restart-router" => { taps.clear(); fixture.restart_router(); host = fixture.connect_control(); taps = panes.iter().enumerate().map(|(index, pane)| WireTap::start(&fixture, *pane, evidence.join(format!("pane-{index}-reconnected-{next}.pty.log")))).collect(); }
                    "stop" => break,
                    other => panic!("unknown native request action {other}"),
                }
                serial = next;
                std::fs::write(evidence.join("ack.json"), serde_json::to_vec(&serde_json::json!({ "serial": serial, "remotePid": before.pid, "panes": panes })).unwrap()).unwrap();
                assert_eq!(fixture::status(&host).pid, before.pid, "native actions cannot silently replace the authoritative daemon");
                assert_eq!(host.peer().instance, instance);
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    drop(gui);
    drop(taps);
    let workspace = workspace.parse().unwrap();
    assert!(matches!(host.client().call(ControlRequest::WorkspaceRemove { workspace }).unwrap(), ReplyOk::Panes(_)));
    // Fixture Drop retires only its own panes/directory/router. session.json
    // remains an execution pointer, not a fabricated successful evidence log.
}
