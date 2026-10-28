use super::super::test_support::*;
use super::test_support::SlowGit;
use super::*;
use crate::daemon::control::{ControlServerMsg, MTime};
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

#[test]
fn a_slow_request_does_not_block_a_fast_one() {
    let running = Arc::new(AtomicBool::new(false));
    let p = pair_with(SlowGit::delayed(
        Duration::from_millis(1500),
        Arc::clone(&running),
    ));

    let tmp = tempfile::TempDir::new().unwrap();
    let slow_host = Arc::clone(&p.host);
    let dir = tmp.path().to_path_buf();
    let slow = std::thread::spawn(move || slow_host.git(&dir, &["status"]));

    let deadline = Instant::now() + Duration::from_secs(5);
    while !running.load(Ordering::SeqCst) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(running.load(Ordering::SeqCst), "the slow request never ran");

    let started = Instant::now();
    p.host.stat(tmp.path()).unwrap();
    let elapsed = started.elapsed();
    assert!(
        elapsed < Duration::from_millis(600),
        "a stat behind an in-flight git took {elapsed:?} — the server is serializing"
    );

    let out = slow.join().unwrap().unwrap();
    assert_eq!(out.stdout, b"slow");
}

#[test]
fn many_slow_requests_still_leave_the_server_answering() {
    let running = Arc::new(AtomicBool::new(false));
    let p = pair_with(SlowGit::delayed(
        Duration::from_millis(1200),
        Arc::clone(&running),
    ));
    let tmp = tempfile::TempDir::new().unwrap();

    let mut slow = Vec::new();
    for _ in 0..24 {
        let host = Arc::clone(&p.host);
        let dir = tmp.path().to_path_buf();
        slow.push(std::thread::spawn(move || {
            let _ = host.git(&dir, &["status"]);
        }));
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    while !running.load(Ordering::SeqCst) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }

    let started = Instant::now();
    assert!(p.host.exists(tmp.path()));
    assert!(
        started.elapsed() < Duration::from_millis(800),
        "24 in-flight gits stalled a trivial request"
    );
    for t in slow {
        let _ = t.join();
    }
}

#[test]
fn a_short_deadline_gives_up_on_a_slow_git() {
    let p = pair_with(SlowGit::delayed(
        Duration::from_millis(300),
        Arc::new(AtomicBool::new(false)),
    ));
    let tmp = tempfile::TempDir::new().unwrap();

    let err = p
        .host
        .git_with_deadline(tmp.path(), &["status"], Duration::from_millis(100))
        .unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::TimedOut, "{err}");
}

#[test]
fn a_long_deadline_outlasts_the_one_a_git_request_would_get() {
    let p = pair_with(SlowGit::delayed(
        Duration::from_millis(300),
        Arc::new(AtomicBool::new(false)),
    ));
    let tmp = tempfile::TempDir::new().unwrap();

    // The first call abandons its request mid-flight. The server has no
    // timer of its own — it runs the job to completion and answers into a
    // slot nobody is waiting on — so the point of doing it twice is that
    // the link is still usable afterwards. A `push` behind a cancelled
    // probe depends on exactly that.
    let _ = p
        .host
        .git_with_deadline(tmp.path(), &["status"], Duration::from_millis(100));
    let out = p
        .host
        .git_with_deadline(tmp.path(), &["status"], Duration::from_secs(5))
        .unwrap();
    assert_eq!(out.stdout, b"slow");
}

#[test]
fn replies_come_back_out_of_order() {
    let running = Arc::new(AtomicBool::new(false));
    let p = pair_with(SlowGit::delayed(
        Duration::from_millis(800),
        Arc::clone(&running),
    ));
    let tmp = tempfile::TempDir::new().unwrap();

    let (tx, rx) = std::sync::mpsc::channel();
    let host = Arc::clone(&p.host);
    let dir = tmp.path().to_path_buf();
    let slow_tx = tx.clone();
    let slow = std::thread::spawn(move || {
        let _ = host.git(&dir, &["status"]);
        let _ = slow_tx.send("git");
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    while !running.load(Ordering::SeqCst) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    p.host.stat(tmp.path()).unwrap();
    tx.send("stat").unwrap();

    assert_eq!(
        rx.recv().unwrap(),
        "stat",
        "the later request answered first"
    );
    assert_eq!(rx.recv().unwrap(), "git");
    slow.join().unwrap();
}

#[test]
fn an_oversized_read_ships_no_bytes() {
    let (mut client, _) = raw();
    let tmp = tempfile::TempDir::new().unwrap();
    let fat = tmp.path().join("fat.bin");
    std::fs::write(&fat, vec![b'x'; 4096]).unwrap();

    ControlClientMsg::Request {
        req_id: 1,
        req: ControlRequest::ReadFile {
            path: fat.to_string_lossy().into_owned(),
            max_bytes: 16,
        },
    }
    .encode(&mut client)
    .unwrap();
    client.flush().unwrap();

    match ControlServerMsg::read(&mut client).unwrap() {
        ControlServerMsg::Response { req_id, reply } => {
            assert_eq!(req_id, 1);
            match reply {
                ControlReply::Err(e) => assert_eq!(e.kind, WireErrorKind::FileTooLarge),
                other => panic!("{other:?}"),
            }
        }
        ControlServerMsg::ResponseBlob { blob, .. } => {
            panic!("the refusal carried {} bytes of the file", blob.len())
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn an_empty_read_still_uses_the_blob_kind() {
    let (mut client, _) = raw();
    let tmp = tempfile::TempDir::new().unwrap();
    let empty = tmp.path().join("empty.txt");
    std::fs::write(&empty, b"").unwrap();

    ControlClientMsg::Request {
        req_id: 1,
        req: ControlRequest::ReadFile {
            path: empty.to_string_lossy().into_owned(),
            max_bytes: 1024,
        },
    }
    .encode(&mut client)
    .unwrap();
    client.flush().unwrap();

    match ControlServerMsg::read(&mut client).unwrap() {
        ControlServerMsg::ResponseBlob { reply, blob, .. } => {
            assert!(blob.is_empty());
            assert!(matches!(reply, ControlReply::Ok(ReplyOk::FileMeta { .. })));
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn error_kinds_survive_the_round_trip() {
    let p = pair();
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path();

    let cases: Vec<(io::ErrorKind, io::Error)> = vec![
        (
            io::ErrorKind::NotFound,
            p.host.stat(&root.join("nope")).unwrap_err(),
        ),
        (io::ErrorKind::AlreadyExists, {
            let f = root.join("taken.txt");
            p.host.write_file(&f, b"x").unwrap();
            p.host.create_file_new(&f).unwrap_err()
        }),
        (io::ErrorKind::FileTooLarge, {
            let f = root.join("fat.txt");
            p.host.write_file(&f, &vec![b'x'; 512]).unwrap();
            p.host.read_file(&f, 16).unwrap_err()
        }),
        (io::ErrorKind::IsADirectory, {
            let d = root.join("adir");
            p.host.create_dir(&d, false).unwrap();
            p.host.read_file(&d, 1024).unwrap_err()
        }),
        (io::ErrorKind::DirectoryNotEmpty, {
            let d = root.join("full");
            p.host.create_dir(&d, false).unwrap();
            p.host.write_file(&d.join("f"), b"x").unwrap();
            p.host.remove(&d, false).unwrap_err()
        }),
    ];
    for (want, got) in cases {
        assert_eq!(got.kind(), want, "{got}");
        assert!(!got.to_string().is_empty());
    }

    for kind in [
        WireErrorKind::NotFound,
        WireErrorKind::PermissionDenied,
        WireErrorKind::AlreadyExists,
        WireErrorKind::InvalidInput,
        WireErrorKind::NotADirectory,
        WireErrorKind::IsADirectory,
        WireErrorKind::DirectoryNotEmpty,
        WireErrorKind::FileTooLarge,
        WireErrorKind::TimedOut,
        WireErrorKind::ConnectionReset,
        WireErrorKind::Other,
    ] {
        let io_err = io::Error::new(kind.to_io_kind(), "x");
        assert_eq!(
            WireError::from_io(&io_err).kind,
            kind,
            "{kind:?} is not its own round trip"
        );
    }
}

#[test]
fn a_failing_git_is_ok_across_the_wire() {
    let p = pair();
    let tmp = tempfile::TempDir::new().unwrap();
    let Ok(out) = p.host.git(tmp.path(), &["rev-parse", "--show-toplevel"]) else {
        return;
    };
    if out.success() {
        return;
    }
    assert!(out.status.is_some());
    assert!(
        !out.stderr.is_empty(),
        "stderr crosses the wire, not dropped"
    );
}

#[test]
fn cancelling_an_unknown_request_does_not_emit_a_reply() {
    let (mut client, _) = raw();
    let tmp = tempfile::TempDir::new().unwrap();

    ControlClientMsg::Cancel { req_id: 1 }
        .encode(&mut client)
        .unwrap();
    ControlClientMsg::Request {
        req_id: 2,
        req: ControlRequest::Stat {
            path: tmp.path().to_string_lossy().into_owned(),
        },
    }
    .encode(&mut client)
    .unwrap();
    client.flush().unwrap();

    match ControlServerMsg::read(&mut client).unwrap() {
        ControlServerMsg::Response { req_id, reply } => {
            assert_eq!(req_id, 2);
            assert!(matches!(reply, ControlReply::Ok(ReplyOk::Meta(_))));
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn the_machine_tree_is_declined_not_faked() {
    let p = pair();
    let err = p
        .host
        .client()
        .call(ControlRequest::MachineGet)
        .unwrap_err();
    assert!(err.to_string().contains("machine tree"), "{err}");
}
#[test]
fn a_write_answers_with_the_new_metadata() {
    let (mut client, _) = raw();
    let tmp = tempfile::TempDir::new().unwrap();
    let f = tmp.path().join("w.txt");

    ControlClientMsg::RequestBlob {
        req_id: 1,
        req: ControlRequest::WriteFile {
            path: f.to_string_lossy().into_owned(),
        },
        blob: b"hello".to_vec(),
    }
    .encode(&mut client)
    .unwrap();
    client.flush().unwrap();

    match ControlServerMsg::read(&mut client).unwrap() {
        ControlServerMsg::Response {
            reply: ControlReply::Ok(ReplyOk::Meta(m)),
            ..
        } => {
            assert_eq!(m.len, 5);
            assert!(
                m.mtime.is_some(),
                "the editor keys its echo detection on this"
            );
            let on_disk = std::fs::metadata(&f).unwrap();
            assert_eq!(
                MTime::from_system_time(on_disk.modified().unwrap()),
                m.mtime.unwrap()
            );
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_filename_that_is_not_utf8_costs_only_its_own_hit() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt as _;

    let hit = |name: &str, path: &Path| SearchHit {
        name: name.to_string(),
        path: path.to_path_buf(),
        is_dir: false,
        ignored: false,
    };
    let mut hits = vec![
        hit("plain-needle.rs", Path::new("/home/me/plain-needle.rs")),
        hit(
            "caf\u{fffd}-needle.rs",
            Path::new(OsStr::from_bytes(b"/home/me/caf\xe9-needle.rs")),
        ),
        hit("other-needle.rs", Path::new("/home/me/other-needle.rs")),
    ];

    drop_unsendable_hits(&mut hits);
    let names: Vec<&str> = hits.iter().map(|h| h.name.as_str()).collect();
    assert_eq!(
        names,
        ["plain-needle.rs", "other-needle.rs"],
        "the representable hits must survive their neighbour"
    );
}
#[test]
fn concurrent_connections_can_all_write_the_tree() {
    let (services, _dir) = workspace_services();
    let store = Arc::clone(services.machine.as_ref().unwrap());

    let writers: Vec<_> = (0..6)
        .map(|_| {
            let services = services.clone();
            std::thread::spawn(move || {
                let (mut client, _) = raw_with(services);
                for i in 0..10 {
                    let reply = ask(
                        &mut client,
                        i as u64 + 1,
                        ControlRequest::WorkspaceCreate {
                            name: Some(format!("w-{i}")),
                            workspace: None,
                        },
                    );
                    assert!(
                        matches!(reply, ControlReply::Ok(ReplyOk::WorkspaceTree(_))),
                        "{reply:?}"
                    );
                }
            })
        })
        .collect();
    for w in writers {
        w.join().unwrap();
    }
    assert_eq!(
        store.machine().workspaces.len(),
        60,
        "every operation landed exactly once"
    );
}
