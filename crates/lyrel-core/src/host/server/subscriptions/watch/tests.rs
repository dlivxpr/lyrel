use super::*;
use super::super::super::{serve, test_support::*};
use crate::daemon::control::{ControlClientMsg, ControlHello, ControlReply, ControlRequest, ReplyOk};
use crate::host::{Host, local::LocalHost, remote::RemoteHost};
use std::io::Write;
use std::os::unix::net::UnixStream;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

#[test]
fn watch_batches_reach_the_client() {
    let p = pair();
    let tmp = tempfile::TempDir::new().unwrap();
    let sub = p.host.watch(&[tmp.path().to_path_buf()]).unwrap();
    while sub.events().try_recv().is_ok() {}

    let deadline = Instant::now() + Duration::from_secs(8);
    let mut seen = false;
    while Instant::now() < deadline && !seen {
        std::fs::write(tmp.path().join("hello.txt"), b"x").unwrap();
        std::thread::sleep(Duration::from_millis(200));
        while let Ok(batch) = sub.events().try_recv() {
            if batch
                .iter()
                .any(|q| q.file_name().is_some_and(|n| n == "hello.txt"))
            {
                seen = true;
            }
        }
    }
    assert!(seen, "no watch event crossed the connection");
}

#[test]
fn a_watch_id_reaches_the_client_before_any_batch_for_it() {
    let (mut client, _ok) = raw();
    let tmp = tempfile::TempDir::new().unwrap();

    ControlClientMsg::Request {
        req_id: 1,
        req: ControlRequest::WatchOpen {
            dirs: vec![tmp.path().to_string_lossy().into_owned()],
        },
    }
    .encode(&mut client)
    .unwrap();
    client.flush().unwrap();

    let churn = tmp.path().to_path_buf();
    let stop = Arc::new(AtomicBool::new(false));
    let churning = Arc::clone(&stop);
    let churner = std::thread::spawn(move || {
        let mut n = 0u32;
        while !churning.load(Ordering::SeqCst) {
            let _ = std::fs::write(churn.join(format!("f{n}")), b"x");
            n = n.wrapping_add(1);
            std::thread::sleep(Duration::from_millis(1));
        }
    });

    let first = ControlServerMsg::read(&mut client).unwrap();
    stop.store(true, Ordering::SeqCst);
    churner.join().unwrap();

    match first {
        ControlServerMsg::Response {
            req_id: 1,
            reply: ControlReply::Ok(ReplyOk::WatchId(_)),
        } => {}
        other => panic!("a batch overtook the WatchId reply: {other:?}"),
    }
}

#[test]
fn closing_a_watch_releases_it_on_the_server() {
    let (server, client) = UnixStream::pair().unwrap();
    let host = LocalHost::new();
    std::thread::spawn(move || {
        let _ = serve(server, host);
    });
    let hello = ControlHello::host_rpc("t", "h");
    let remote = RemoteHost::over_unix(client, "test:watch", &hello).unwrap();

    let tmp = tempfile::TempDir::new().unwrap();
    let sub = remote.watch(&[tmp.path().to_path_buf()]).unwrap();
    let rx = sub.events().clone();
    drop(sub);

    std::thread::sleep(Duration::from_millis(300));
    std::fs::write(tmp.path().join("after.txt"), b"x").unwrap();
    std::thread::sleep(Duration::from_millis(800));
    while let Ok(batch) = rx.try_recv() {
        assert!(
            !batch
                .iter()
                .any(|q| q.file_name().is_some_and(|n| n == "after.txt")),
            "the server kept watching after the subscription closed"
        );
    }
}

#[test]
fn a_gitignore_edit_reaches_a_remote_listing() {
    let p = pair();
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path().to_path_buf();
    p.host
        .write_file(&root.join(".gitignore"), b"*.log\n")
        .unwrap();
    p.host.write_file(&root.join("a.log"), b"").unwrap();

    let listed = p.host.read_dir(&root, Some(&root)).unwrap();
    assert!(
        listed.iter().any(|e| e.name == "a.log" && e.ignored),
        "the fixture should start out ignored"
    );

    let sub = p.host.watch(std::slice::from_ref(&root)).unwrap();

    let deadline = Instant::now() + Duration::from_secs(15);
    let mut cleared = false;
    while Instant::now() < deadline {
        p.host
            .write_file(&root.join(".gitignore"), b"# nothing\n")
            .unwrap();
        std::thread::sleep(Duration::from_millis(250));
        while sub.events().try_recv().is_ok() {}
        if p.host
            .read_dir(&root, Some(&root))
            .unwrap()
            .iter()
            .any(|e| e.name == "a.log" && !e.ignored)
        {
            cleared = true;
            break;
        }
    }
    assert!(
        cleared,
        "a `.gitignore` change on the server must reach the remote client's listing"
    );
}
