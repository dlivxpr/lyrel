//! A pane's screen survives the daemon being stopped and started.
//!
//! The unit tests under `daemon::scrollback` cover the file: it round-trips, it
//! is trimmed from the front, the sweep keeps what it is told to keep. None of
//! that answers the question the feature exists for, which is whether a window
//! that comes back to a restarted daemon is shown what its panes had on them.
//! That answer involves a real daemon writing a real snapshot, dying, and a
//! second daemon handing the bytes to the client that asks — so this runs all
//! of it and reads the wire.
//!
//! The ordering is the whole difficulty. A client cannot ask for a restore
//! until the daemon is listening, so anything the daemon throws away *while
//! starting up* is thrown away before the only party who knows what is still
//! wanted has been able to say so.

use std::io::Write as _;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use lyrel_core::client::{ControlClient, PaneClient};
use lyrel_core::core::machine::PaneSeed;
use lyrel_core::daemon::control::{ControlHello, ControlRequest, ReplyOk};
use lyrel_core::daemon::protocol::{ClientMsg, DaemonMsg, RestoreFrom, ShellSpec, WinSize};
use lyrel_core::daemon::transport;

const READY_WITHIN: Duration = Duration::from_secs(30);
const STREAM_WITHIN: Duration = Duration::from_secs(30);
const STOP_WITHIN: Duration = Duration::from_secs(30);

const MARKER: &[u8] = b"lyrel_screen_kept";

/// One instance's directories, outliving the daemons that serve them — the
/// point of the test is what is on disk between two of them.
struct Instance {
    dir: tempfile::TempDir,
}

impl Instance {
    fn new() -> Instance {
        // No config: keeping each pane's screen is what the daemon does, not
        // something it is asked to do.
        Instance {
            dir: tempfile::TempDir::new().unwrap(),
        }
    }

    fn path(&self) -> &std::path::Path {
        self.dir.path()
    }

    /// What the daemon writes down for clients to find it by: a socket on unix,
    /// a port-and-token file on Windows.
    fn endpoint(&self) -> PathBuf {
        #[cfg(unix)]
        let name = "daemon.sock";
        #[cfg(windows)]
        let name = "daemon.port";
        self.dir.path().join(name)
    }

    fn panes(&self) -> PaneClient {
        PaneClient::at(self.endpoint())
    }

    fn control_endpoint(&self) -> PathBuf {
        #[cfg(unix)]
        let name = "control.sock";
        #[cfg(windows)]
        let name = "control.port";
        self.dir.path().join(name)
    }

    fn snapshot_of(&self, pane_id: u64) -> Option<Vec<u8>> {
        std::fs::read(
            self.dir
                .path()
                .join("scrollback")
                .join(format!("{pane_id}.bin")),
        )
        .ok()
    }

    /// The tree a window would have left behind: one workspace, one tab, and
    /// that tab standing on the pane whose screen we want back.
    fn record_tab_on(&self, pane_id: u64) {
        let tree = format!(
            r#"{{
  "workspaces": [
    {{
      "id": "11111111-2222-3333-4444-555555555555",
      "name": null,
      "last_active": 1786343761,
      "tabs": [
        {{
          "id": "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee",
          "name": null,
          "group": null,
          "root": {{ "Leaf": {{ "pane": {pane_id} }} }}
        }}
      ],
      "active_tab": "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"
    }}
  ],
  "panes": [
    {{ "id": {pane_id}, "cwd": null, "title": "", "ssh_spec": null, "agent": null, "live": false }}
  ]
}}"#
        );
        std::fs::write(self.dir.path().join("machine.json"), tree).unwrap();
    }

    fn start(&self) -> Running {
        let child = Command::new(env!("CARGO_BIN_EXE_lyrel-server"))
            .arg("--daemon")
            .arg("--config-dir")
            .arg(self.path())
            .env("LYREL_DATA_DIR", self.path())
            .env("LYREL_CONTROL_SOCK", self.path().join("control.sock"))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("start lyrel-server --daemon");
        let running = Running {
            child,
            stopped: false,
        };
        let deadline = Instant::now() + READY_WITHIN;
        loop {
            if self.panes().version().is_ok() {
                return running;
            }
            assert!(
                Instant::now() < deadline,
                "the daemon did not open its pane endpoint within {READY_WITHIN:?}"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Ask the daemon to go, the way the restart in Settings asks. This is the
    /// path that takes each pane's last snapshot on the way out, so a test that
    /// killed the process instead would be testing the periodic writer.
    fn stop(&self, mut running: Running) {
        let mut stream =
            transport::connect_endpoint_at(&self.endpoint()).expect("connect to ask for shutdown");
        ClientMsg::Shutdown
            .encode(&mut stream)
            .expect("send Shutdown");
        stream.flush().ok();
        drop(stream);

        let deadline = Instant::now() + STOP_WITHIN;
        loop {
            match running.child.try_wait() {
                Ok(Some(_)) => {
                    running.stopped = true;
                    return;
                }
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(50));
                }
                Ok(None) => panic!("the daemon did not exit within {STOP_WITHIN:?}"),
                Err(e) => panic!("waiting for the daemon failed: {e}"),
            }
        }
    }

    /// The `Spawn` a window sends for a pane whose `Attach` found nothing: a
    /// new shell, asked to open showing what the dead one had.
    fn spawn_restoring(&self, dead: u64) -> (u64, Vec<u8>) {
        let (pane_id, segments) = self.spawn_restoring_segments(dead);
        (
            pane_id,
            segments.into_iter().flat_map(|s| s.bytes).collect(),
        )
    }

    fn spawn_restoring_segments(
        &self,
        dead: u64,
    ) -> (u64, Vec<lyrel_core::daemon::scrollback::Segment>) {
        let mut stream =
            transport::connect_endpoint_at(&self.endpoint()).expect("connect to spawn");
        lyrel_core::daemon::protocol::negotiate_pane(&mut stream).expect("matching pane protocol");
        ClientMsg::Spawn {
            cwd: None,
            size: size(),
            shell: Some(interactive_shell()),
            owner: None,
            workspace: None,
            restore: Some(RestoreFrom {
                pane_id: dead,
                banner: Some("the shell below is new".to_string()),
            }),
            allow_remote_clipboard_write: false,
            await_claim: false,
        }
        .encode(&mut stream)
        .expect("send Spawn");

        let pane_id = match DaemonMsg::read(&mut stream) {
            Ok(DaemonMsg::Spawned { pane_id }) => pane_id,
            other => panic!("expected Spawned, got {other:?}"),
        };

        // Everything the daemon replays sits in the socket ahead of whatever
        // the new shell writes, so a bounded drain is enough: the replay is
        // already queued by the time `Spawned` is read.
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("bound the drain");
        let mut replayed: Vec<lyrel_core::daemon::scrollback::Segment> = Vec::new();
        let mut geometry = size();
        while let Ok(msg) = DaemonMsg::read(&mut stream) {
            match msg {
                DaemonMsg::Size(size) => geometry = size,
                DaemonMsg::Snapshot(bytes) | DaemonMsg::Output(bytes) => {
                    replayed.push(lyrel_core::daemon::scrollback::Segment {
                        size: geometry,
                        bytes,
                    });
                }
                _ => {}
            }
        }
        (pane_id, replayed)
    }
}

struct Running {
    child: Child,
    stopped: bool,
}

impl Drop for Running {
    fn drop(&mut self) {
        if !self.stopped {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

fn size() -> WinSize {
    WinSize {
        cols: 100,
        rows: 30,
        cell_w: 8,
        cell_h: 16,
    }
}

fn interactive_shell() -> ShellSpec {
    #[cfg(unix)]
    let program = "/bin/sh";
    #[cfg(windows)]
    let program = "cmd.exe";
    ShellSpec {
        program: program.into(),
        args: Vec::new(),
        args_are_lyrel_defaults: false,
    }
}

fn windows_contain(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

fn collect_until(session: &mut lyrel_core::client::PaneSession, marker: &[u8]) -> Vec<u8> {
    let mut seen: Vec<u8> = Vec::new();
    loop {
        match session.recv() {
            Ok(DaemonMsg::Output(bytes)) | Ok(DaemonMsg::Snapshot(bytes)) => {
                seen.extend_from_slice(&bytes);
                if windows_contain(&seen, marker) {
                    return seen;
                }
            }
            Ok(DaemonMsg::Exited { code }) => panic!(
                "the pane exited ({code:?}) before {:?} appeared; saw {:?}",
                String::from_utf8_lossy(marker),
                String::from_utf8_lossy(&seen)
            ),
            Ok(_) => {}
            Err(e) => panic!(
                "the pane stream ended early: {e}; saw {:?}",
                String::from_utf8_lossy(&seen)
            ),
        }
    }
}

/// Put a marker on a pane's screen and give the daemon back the pane's id.
fn pane_showing_the_marker(instance: &Instance) -> u64 {
    let mut session = instance
        .panes()
        .spawn(None, size(), Some(interactive_shell()), None, None)
        .expect("spawn a pane");
    session
        .set_recv_timeout(Some(STREAM_WITHIN))
        .expect("bound the stream reads");
    let pane_id = session.pane_id();
    session
        .input(format!("echo {}\r", String::from_utf8_lossy(MARKER)).as_bytes())
        .expect("the shell takes input");
    collect_until(&mut session, MARKER);
    // Detach rather than kill: the pane outlives this connection, which is the
    // state a daemon restart finds its panes in.
    session.detach().expect("detach");
    pane_id
}

/// The case the feature is for: nothing has told the new daemon anything yet,
/// because the window that would tell it is still waiting for it to listen.
#[test]
fn a_restarted_daemon_still_has_the_screen_when_the_window_asks() {
    let instance = Instance::new();
    let running = instance.start();
    let dead = pane_showing_the_marker(&instance);
    instance.stop(running);

    let stored = instance
        .snapshot_of(dead)
        .expect("the shutdown wrote the pane's screen");
    assert!(
        windows_contain(&stored, MARKER),
        "the snapshot on disk does not hold the pane's screen"
    );

    let _restarted = instance.start();
    let (_new_pane, replayed) = instance.spawn_restoring(dead);
    assert!(
        windows_contain(&replayed, MARKER),
        "the restarted daemon did not give the pane back its screen; \
         the client received {:?}",
        String::from_utf8_lossy(&replayed)
    );
}

#[cfg(windows)]
#[test]
fn an_empty_native_cmd_pane_does_not_accumulate_restored_screens() {
    let instance = Instance::new();
    let running = instance.start();
    let mut session = instance
        .panes()
        .spawn(None, size(), Some(interactive_shell()), None, None)
        .unwrap();
    session.set_recv_timeout(Some(STREAM_WITHIN)).unwrap();
    let dead = session.pane_id();
    let live = collect_until(&mut session, b"777;lyrel-prompt;B");
    assert!(!live.is_empty(), "the live shell still draws its prompt");
    session.detach().unwrap();
    instance.stop(running);
    let saved = instance.snapshot_of(dead).unwrap();
    let (segments, _) = lyrel_core::daemon::scrollback::decode(&saved).unwrap();
    assert!(
        segments.is_empty(),
        "prompt decoration is not persisted as content"
    );
    instance.record_tab_on(dead);
    let _restarted = instance.start();
    let (_, replayed) = instance.spawn_restoring(dead);
    assert!(!windows_contain(&replayed, b"the shell below is new"));
}

#[test]
fn a_prompt_only_stored_screen_does_not_manufacture_a_restore_banner() {
    let instance = Instance::new();
    let dead = 900_001;
    instance.record_tab_on(dead);
    let directory = instance.path().join("scrollback");
    std::fs::create_dir_all(&directory).unwrap();
    let prompt = b"\x1b]133;A\x07old_empty_prompt\r\n> \x1b]133;B\x07";
    let snapshot = lyrel_core::daemon::scrollback::encode(
        &[lyrel_core::daemon::scrollback::Segment {
            size: size(),
            bytes: prompt.to_vec(),
        }],
        Some("old empty shell"),
    );
    std::fs::write(directory.join(format!("{dead}.bin")), snapshot).unwrap();
    let _running = instance.start();
    let (_, replayed) = instance.spawn_restoring(dead);
    assert!(!windows_contain(&replayed, b"old_empty_prompt"));
    assert!(!windows_contain(&replayed, b"the shell below is new"));
    assert!(!windows_contain(&replayed, b"old empty shell"));
}

/// What a pane is running has to reach the tree, because that is the only
/// place a window rebuilding the pane can read it from. Without it the rebuild
/// falls back to the default shell, which is how a restart turns a bash pane
/// into a PowerShell one.
#[test]
fn the_tree_records_the_shell_a_pane_is_running() {
    let instance = Instance::new();
    let _running = instance.start();
    let pane = pane_showing_the_marker(&instance);

    // A pane reaches the tree by being put in a tab, which is what the window
    // does right after it spawns one — carrying the same seed it spawned with.
    // Until then the daemon has nothing to record its facts against, so this is
    // the pane's first chance to say what it is running, and for a pane that
    // then sits at a prompt it is the only one: the daemon's own observation
    // rides on a fact *changing*, and a pane's shell never does.
    let control = ControlClient::connect_at(
        &instance.control_endpoint(),
        &ControlHello::host_rpc("probe", "probe"),
    )
    .expect("control handshake");
    let ws = match control
        .request(ControlRequest::WorkspaceCreate {
            name: Some("probe".into()),
            workspace: None,
        })
        .expect("create a workspace")
    {
        ReplyOk::WorkspaceTree(ws) => *ws,
        other => panic!("expected WorkspaceTree, got {other:?}"),
    };
    control
        .request(ControlRequest::TabCreate {
            workspace: ws.id,
            at: None,
            pane: PaneSeed {
                shell: Some(interactive_shell()),
                ..PaneSeed::bare(pane)
            },
            tab: None,
        })
        .expect("put the pane in a tab");

    let wanted = interactive_shell().program;
    let deadline = Instant::now() + STREAM_WITHIN;
    loop {
        let tree =
            std::fs::read_to_string(instance.path().join("machine.json")).unwrap_or_default();
        // The record is keyed by pane id and the program is a plain string in
        // it; matching on both together is enough to say this pane's shell —
        // not some other pane's — reached the tree.
        if tree.contains(&format!("\"id\": {pane}")) && tree.contains(&wanted) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "the tree never recorded pane {pane}'s shell ({wanted}); it holds: {tree}"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// The same thing for an instance whose tree still names the pane. This one
/// passes on its own merits today; it is here so a fix for the case above
/// cannot be one that quietly stops honouring the tree.
#[test]
fn a_screen_comes_back_when_the_tree_still_names_its_pane() {
    let instance = Instance::new();
    let running = instance.start();
    let dead = pane_showing_the_marker(&instance);
    instance.stop(running);
    instance.record_tab_on(dead);

    let _restarted = instance.start();
    let (_new_pane, replayed) = instance.spawn_restoring(dead);
    assert!(
        windows_contain(&replayed, MARKER),
        "a pane the tree still names came back blank; the client received {:?}",
        String::from_utf8_lossy(&replayed)
    );
}

/// A real PTY produces more than the storage budget at two geometries. A
/// reconnect sees the full live replay; a restart must see its exact bounded
/// suffix, including cuts through UTF-8/escape bytes, at the original sizes.
#[test]
fn a_bounded_restart_preserves_the_live_replay_suffix_and_geometry() {
    use lyrel_core::daemon::scrollback::{SNAPSHOT_CAP, Segment, decode, trim_to};

    let instance = Instance::new();
    let running = instance.start();
    let chunk = format!("{}\x1b[31m下一步\x1b[0m\r\n", "x".repeat(4070));
    #[cfg(unix)]
    let (script_name, program, args, script) = (
        "output.sh",
        "/bin/sh",
        Vec::<String>::new(),
        format!(
            "printf 'READY_FOR_OUTPUT'; read first\ni=0; while [ $i -lt 70 ]; do printf '%s' '{chunk}'; i=$((i+1)); done\nprintf 'FIRST_OUTPUT_DONE'; read second\ni=0; while [ $i -lt 16 ]; do printf '%s' '{chunk}'; i=$((i+1)); done\nprintf 'SECOND_OUTPUT_DONE'; read final\n"
        ),
    );
    #[cfg(windows)]
    let (script_name, program, args, script) = (
        "output.ps1",
        "powershell.exe",
        vec!["-NoLogo".into(), "-NoProfile".into(), "-Command".into()],
        "$out=[Console]::OpenStandardOutput(); function Emit([string]$s) { $b=[Text.Encoding]::UTF8.GetBytes($s); $out.Write($b,0,$b.Length); $out.Flush() }\nEmit 'READY_FOR_OUTPUT'; $null=[Console]::ReadLine()\n$chunk=[Text.Encoding]::UTF8.GetString([IO.File]::ReadAllBytes((Join-Path $PSScriptRoot 'chunk.bin')))\nfor($i=0;$i -lt 70;$i++) { Emit $chunk }\nEmit 'FIRST_OUTPUT_DONE'; $null=[Console]::ReadLine()\nfor($i=0;$i -lt 16;$i++) { Emit $chunk }\nEmit 'SECOND_OUTPUT_DONE'; $null=[Console]::ReadLine()\n".to_string(),
    );
    // PowerShell 5 defaults to the system script encoding; keep its source
    // ASCII-only and load the terminal's UTF-8 fixture separately.
    #[cfg(windows)]
    std::fs::write(instance.path().join("chunk.bin"), chunk.as_bytes()).unwrap();
    let script_path = instance.path().join(script_name);
    std::fs::write(&script_path, script).unwrap();
    let mut args = args;
    #[cfg(unix)]
    args.push(script_path.to_str().unwrap().into());
    #[cfg(windows)]
    args.push(std::fs::read_to_string(&script_path).unwrap().replace(
        "$PSScriptRoot",
        &format!(
            "'{}'",
            instance.path().display().to_string().replace('\'', "''")
        ),
    ));
    let mut session = instance
        .panes()
        .spawn(
            None,
            size(),
            Some(ShellSpec {
                program: program.into(),
                args,
                args_are_lyrel_defaults: false,
            }),
            None,
            None,
        )
        .unwrap();
    session.set_recv_timeout(Some(STREAM_WITHIN)).unwrap();
    let pane = session.pane_id();
    collect_until(&mut session, b"READY_FOR_OUTPUT");
    session.input(b"1\r").unwrap();
    collect_until(&mut session, b"FIRST_OUTPUT_DONE");
    let narrow = WinSize { cols: 71, ..size() };
    session.resize(narrow).unwrap();
    session.input(b"2\r").unwrap();
    collect_until(&mut session, b"SECOND_OUTPUT_DONE");
    session.detach().unwrap();

    let mut fresh = instance.panes().attach(pane, narrow).unwrap();
    fresh
        .set_recv_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let mut geometry = size();
    let mut expected = Vec::new();
    while let Ok(msg) = fresh.recv() {
        match msg {
            DaemonMsg::Size(size) => geometry = size,
            DaemonMsg::Snapshot(bytes) if !bytes.is_empty() => expected.push(Segment {
                size: geometry,
                bytes,
            }),
            DaemonMsg::Exited { .. } => panic!("the fixture must remain alive"),
            _ => {}
        }
    }
    assert!(expected.iter().map(|s| s.bytes.len()).sum::<usize>() > SNAPSHOT_CAP);
    trim_to(&mut expected, SNAPSHOT_CAP);
    assert!(expected.iter().any(|s| s.size == size()));
    assert!(expected.iter().any(|s| s.size == narrow));
    fresh.detach().unwrap();
    instance.stop(running);

    let stored = instance.snapshot_of(pane).expect("shutdown snapshot");
    let (saved, _) = decode(&stored).expect("the stored snapshot is readable");
    assert_eq!(
        saved, expected,
        "persistence must retain the exact live suffix"
    );
    let _restarted = instance.start();
    let (_, restored) = instance.spawn_restoring_segments(pane);
    assert!(
        restored.starts_with(&expected),
        "restore must preserve bytes and geometry before the new-shell reset"
    );
}

impl Instance {
    /// Like `spawn_restoring_segments`, but keeps the colour projection the
    /// restarted daemon sends for the replacement pane.
    fn spawn_restoring_with_colors(
        &self,
        dead: u64,
    ) -> (
        u64,
        Vec<lyrel_core::daemon::scrollback::Segment>,
        Option<lyrel_core::core::pane_colors::PaneColors>,
    ) {
        let mut stream =
            transport::connect_endpoint_at(&self.endpoint()).expect("connect to spawn");
        lyrel_core::daemon::protocol::negotiate_pane(&mut stream).expect("matching pane protocol");
        ClientMsg::Spawn {
            cwd: None,
            size: size(),
            shell: Some(interactive_shell()),
            owner: None,
            workspace: None,
            restore: Some(RestoreFrom {
                pane_id: dead,
                banner: Some("the shell below is new".to_string()),
            }),
            allow_remote_clipboard_write: false,
            await_claim: false,
        }
        .encode(&mut stream)
        .expect("send Spawn");

        let pane_id = match DaemonMsg::read(&mut stream) {
            Ok(DaemonMsg::Spawned { pane_id }) => pane_id,
            other => panic!("expected Spawned, got {other:?}"),
        };
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("bound the drain");
        let mut replayed: Vec<lyrel_core::daemon::scrollback::Segment> = Vec::new();
        let mut colors = None;
        let mut geometry = size();
        while let Ok(msg) = DaemonMsg::read(&mut stream) {
            match msg {
                DaemonMsg::Size(size) => geometry = size,
                DaemonMsg::Snapshot(bytes) | DaemonMsg::Output(bytes) => {
                    replayed.push(lyrel_core::daemon::scrollback::Segment {
                        size: geometry,
                        bytes,
                    });
                }
                DaemonMsg::Colors(frame) => {
                    let frame = lyrel_core::core::pane_colors::ColorFrame::decode(&frame)
                        .expect("decodable colour projection");
                    let mut state = lyrel_core::core::pane_colors::PaneColors::default();
                    frame.apply(&mut state);
                    colors = Some(state);
                }
                _ => {}
            }
        }
        (pane_id, replayed, colors)
    }

    /// Like `spawn_restoring_with_colors`, but negotiated for the retained
    /// glyph glossary: the caller keeps the live restored-pane stream and the
    /// authoritative frame the restarted daemon projects for it. Used only by
    /// the Unix glyph isolation test — `interactive_shell` elsewhere names
    /// cmd.exe, and the producer/query exchange assumes `/bin/sh` semantics.
    #[cfg(unix)]
    fn spawn_restoring_live(
        &self,
        dead: u64,
    ) -> (
        u64,
        transport::Stream,
        Vec<lyrel_core::daemon::scrollback::Segment>,
        lyrel_core::core::glyph_protocol::GlyphFrame,
    ) {
        let mut stream =
            transport::connect_endpoint_at(&self.endpoint()).expect("connect to spawn");
        let endpoint = lyrel_core::daemon::protocol::negotiate_pane(&mut stream)
            .expect("matching pane protocol");
        assert!(
            endpoint.has_feature(lyrel_core::daemon::protocol::FEATURE_GLYPH_STATE),
            "this path exists only for a daemon that projects the glossary"
        );
        ClientMsg::Spawn {
            cwd: None,
            size: size(),
            shell: Some(interactive_shell()),
            owner: None,
            workspace: None,
            restore: Some(RestoreFrom {
                pane_id: dead,
                banner: Some("the shell below is new".to_string()),
            }),
            allow_remote_clipboard_write: false,
            await_claim: false,
        }
        .encode(&mut stream)
        .expect("send Spawn");

        let pane_id = match DaemonMsg::read(&mut stream) {
            Ok(DaemonMsg::Spawned { pane_id }) => pane_id,
            other => panic!("expected Spawned, got {other:?}"),
        };

        // The glossary snapshot follows the replayed ring on this negotiated
        // stream, so it is the boundary that says the replay is done.
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .expect("bound the replay drain");
        let mut replayed: Vec<lyrel_core::daemon::scrollback::Segment> = Vec::new();
        let mut geometry = size();
        let frame = loop {
            match DaemonMsg::read(&mut stream).expect("the spawned pane answers") {
                DaemonMsg::Size(next) => geometry = next,
                DaemonMsg::Snapshot(bytes) | DaemonMsg::Output(bytes) => {
                    replayed.push(lyrel_core::daemon::scrollback::Segment {
                        size: geometry,
                        bytes,
                    });
                }
                DaemonMsg::GlyphState(frame) => {
                    break lyrel_core::core::glyph_protocol::GlyphFrame::decode(&frame)
                        .expect("decodable glossary snapshot");
                }
                DaemonMsg::Exited { code } => {
                    panic!("the restored pane exited during replay: {code:?}")
                }
                _ => {}
            }
        };
        (pane_id, stream, replayed, frame)
    }
}

#[test]
fn a_replacement_shell_does_not_reactivate_old_temporary_colours() {
    let instance = Instance::new();
    let running = instance.start();
    let mut session = instance
        .panes()
        .spawn(None, size(), Some(interactive_shell()), None, None)
        .expect("spawn a pane");
    session
        .set_recv_timeout(Some(STREAM_WITHIN))
        .expect("bound the stream reads");
    let dead = session.pane_id();
    session
        .input(b"printf '\\033]4;1;#112233\\007\\033]11;#101112\\007'; echo lyrel_colour_restore\r")
        .expect("the shell takes input");
    collect_until(&mut session, b"lyrel_colour_restore");
    session.detach().expect("detach");
    instance.stop(running);

    let _restarted = instance.start();
    let (_new_pane, replayed, colors) = instance.spawn_restoring_with_colors(dead);
    let text: Vec<u8> = replayed
        .into_iter()
        .flat_map(|segment| segment.bytes)
        .collect();
    assert!(
        windows_contain(&text, b"\x1b]4;1;#112233"),
        "the persisted text path still carries the historical bytes; the test must prove they are not decoded"
    );
    let colors = colors.expect("a restored pane still sends its authoritative (empty) layer");
    assert!(
        colors.is_empty(),
        "a replacement shell must start with no inherited temporary colours"
    );
}

/// A small valid `glyf` outline — one closed quad — the only payload format
/// this build accepts. Only the Unix isolation test produces registrations:
/// the shell semantics (`cat`, `%b`-free `printf`, `^U` line kill) have no
/// cmd.exe equivalent here.
#[cfg(unix)]
fn glyph_outline(width: i16, height: i16) -> Vec<u8> {
    let mut bytes = Vec::new();
    for word in [1, 0, 0, width, height, 3, 0] {
        bytes.extend_from_slice(&word.to_be_bytes());
    }
    bytes.extend_from_slice(&[1; 4]);
    for delta in [0, width, 0, -width, 0, 0, height, 0] {
        bytes.extend_from_slice(&delta.to_be_bytes());
    }
    bytes
}

#[cfg(unix)]
fn base64(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
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

/// One complete `r` request — `ESC _ 25a1 ; r ; … ESC \` — as a producer
/// would emit it.
#[cfg(unix)]
fn glyph_registration(cp: u32, glyf: &[u8], width: u8, reply: u8) -> Vec<u8> {
    format!(
        "\x1b_25a1;r;cp={cp:x};fmt=glyf;upm=1000;width={width};reply={reply};{}\x1b\\",
        base64(glyf)
    )
    .into_bytes()
}

/// A daemon's glossary dies with it. The replacement pane a restore seeds
/// must open with an empty authoritative frame, the historical registration
/// must never ride the persisted text, and a live `q` must be answered from
/// the new pane's own (empty) glossary — the strongest external proof that
/// nothing inherited the old registry.
#[cfg(unix)]
#[test]
fn a_replacement_shell_does_not_reactivate_old_glyph_registrations() {
    let instance = Instance::new();
    let running = instance.start();
    let mut session = instance
        .panes()
        .spawn(None, size(), Some(interactive_shell()), None, None)
        .expect("spawn a pane");
    session
        .set_recv_timeout(Some(STREAM_WITHIN))
        .expect("bound the stream reads");
    let dead = session.pane_id();

    // A real producer — `cat` on bytes it never saw before — registers a
    // definition, replaces it, prints ordinary text, and exits. reply=1
    // earns the live ACK echo that proves the daemon accepted both bodies.
    const CP: u32 = 0x100000;
    let mut produced = glyph_registration(CP, &glyph_outline(1000, 1000), 1, 1);
    produced.extend(glyph_registration(CP, &glyph_outline(2000, 500), 2, 1));
    let glyphs_file = instance.path().join("glyphs.bin");
    std::fs::write(&glyphs_file, &produced).unwrap();
    session
        .input(
            format!(
                "cat '{}'; echo lyrel_glyph_text_kept; echo lyrel_glyph_sender_exit_$?\r",
                glyphs_file.display()
            )
            .as_bytes(),
        )
        .expect("the registration producer runs");
    // The ACK echo and the exit marker share no ordering — the tty echoes a
    // reply the moment the daemon writes it, before `cat` has finished — so
    // the drain has to accept either first.
    let mut live = Vec::new();
    let live_deadline = Instant::now() + STREAM_WITHIN;
    while !(windows_contain(&live, b"lyrel_glyph_sender_exit_0")
        && windows_contain(&live, b"cp=100000;status=0"))
    {
        assert!(
            Instant::now() < live_deadline,
            "the producer's output or the daemon's ACK never arrived"
        );
        match session.recv().expect("the pane answers") {
            DaemonMsg::Output(bytes) | DaemonMsg::Snapshot(bytes) => live.extend(bytes),
            DaemonMsg::Exited { code } => panic!("the pane exited during registration: {code:?}"),
            _ => {}
        }
    }
    assert!(
        windows_contain(&live, b"\x1b_25a1;r;cp=100000"),
        "the daemon re-emitted and acknowledged the bodies while it lived"
    );
    session.detach().expect("detach");
    instance.stop(running);

    // The persisted screen keeps the ordinary text but must not carry the
    // control APCs: a replacement shell's stream can never hand them back.
    let stored = instance
        .snapshot_of(dead)
        .expect("the shutdown wrote the pane's screen");
    assert!(
        windows_contain(&stored, b"lyrel_glyph_text_kept"),
        "the persisted screen keeps ordinary text"
    );
    assert!(
        !windows_contain(&stored, b"\x1b_25a1"),
        "a complete glyph APC is stripped before persisted bytes outlive the pane"
    );

    let _restarted = instance.start();
    let (_new_pane, mut stream, segments, frame) = instance.spawn_restoring_live(dead);
    let restored: Vec<u8> = segments.into_iter().flat_map(|s| s.bytes).collect();
    assert!(
        windows_contain(&restored, b"lyrel_glyph_text_kept"),
        "the new shell still reads the old pane's ordinary text"
    );
    assert!(
        !windows_contain(&restored, b"\x1b_25a1"),
        "no historical glyph APC enters the revivable stream"
    );
    assert!(
        frame.glyphs().is_empty(),
        "the replacement pane opens with an empty authoritative glossary"
    );

    // Ask the codepoint the dead pane had registered. The daemon routes the
    // query to this stream as the opted-in controller; answering `system:
    // false` makes the pane's own glossary the only possible source of a
    // non-empty status.
    ClientMsg::Input(b"printf '\\033_25a1;q;cp=100000\\033\\\\'; echo lyrel_query_sent\r".to_vec())
        .encode(&mut stream)
        .expect("send the coverage query");
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("bound the query round trip");
    let deadline = Instant::now() + STREAM_WITHIN;
    let mut token = None;
    let mut projected_frames = 0u64;
    let mut live = Vec::new();
    while token.is_none() {
        assert!(
            Instant::now() < deadline,
            "the query never reached this pane's controller"
        );
        match DaemonMsg::read(&mut stream).expect("the pane answers") {
            DaemonMsg::GlyphQuery { token: t, cp } => {
                assert_eq!(cp, CP, "the query carries the codepoint asked about");
                token = Some(t);
            }
            DaemonMsg::Output(bytes) | DaemonMsg::Snapshot(bytes) => live.extend(bytes),
            DaemonMsg::GlyphState(_) => projected_frames += 1,
            DaemonMsg::Exited { code } => panic!("the restored pane exited: {code:?}"),
            _ => {}
        }
    }
    ClientMsg::GlyphQueryResult {
        token: token.expect("the daemon asked for one codepoint"),
        cp: CP,
        system: false,
    }
    .encode(&mut stream)
    .expect("answer the font coverage query");

    // The daemon writes the single `q` reply to the pane's tty. Its shell
    // echo is the observable status; afterwards a marker proves the shell is
    // still cleanly consuming input (^U clears the pending reply line).
    while !windows_contain(&live, b"25a1;q;cp=100000;status=") {
        assert!(
            Instant::now() < deadline,
            "the coverage answer never reached the new shell"
        );
        match DaemonMsg::read(&mut stream).expect("the pane answers") {
            DaemonMsg::Output(bytes) | DaemonMsg::Snapshot(bytes) => live.extend(bytes),
            DaemonMsg::GlyphState(_) => projected_frames += 1,
            DaemonMsg::Exited { code } => panic!("the restored pane exited: {code:?}"),
            _ => {}
        }
    }
    ClientMsg::Input(b"\x15echo lyrel_glyph_after_query\r".to_vec())
        .encode(&mut stream)
        .expect("the shell still takes input");
    while !windows_contain(&live, b"lyrel_glyph_after_query") {
        assert!(
            Instant::now() < deadline,
            "the new shell stopped answering after the query exchange"
        );
        match DaemonMsg::read(&mut stream).expect("the pane answers") {
            DaemonMsg::Output(bytes) | DaemonMsg::Snapshot(bytes) => live.extend(bytes),
            DaemonMsg::GlyphState(_) => projected_frames += 1,
            DaemonMsg::Exited { code } => panic!("the restored pane exited: {code:?}"),
            _ => {}
        }
    }
    let status_at = live
        .windows(b"25a1;q;cp=100000;status=".len())
        .position(|w| w == b"25a1;q;cp=100000;status=")
        .expect("the echoed reply was collected");
    let after_status = live[status_at + b"25a1;q;cp=100000;status=".len()..]
        .first()
        .copied();
    assert_eq!(
        projected_frames, 0,
        "the restored pane's empty glossary is projected exactly once, at attach"
    );
    assert!(
        matches!(after_status, Some(0x1b) | Some(b'^')),
        "with no glossary and no system coverage the status must be empty; \
         a resurrected registry would answer `glossary` (saw {:?})",
        String::from_utf8_lossy(&live[status_at..(status_at + 60).min(live.len())])
    );
    assert!(
        !windows_contain(&live, b"\x1b_25a1;r"),
        "the historical register ACK is not re-sent to the replacement shell"
    );

    let _ = ClientMsg::Detach.encode(&mut stream);
    drop(stream);
}
