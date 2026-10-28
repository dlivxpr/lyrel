use super::*;
use crate::daemon::control::ControlHello;
use crate::host::local::LocalHost;
use crate::host::remote::RemoteHost;
use crate::host::Host;
use std::time::{Duration, Instant};

#[test]
fn the_socket_path_honours_its_override() {
    let dir = tempfile::TempDir::new().unwrap();
    let sock = dir.path().join("explicit.sock");
    let listener = bind_control_socket(&sock).unwrap();
    assert!(sock.exists());
    use std::os::unix::fs::PermissionsExt as _;
    assert_eq!(
        std::fs::metadata(&sock).unwrap().permissions().mode() & 0o777,
        0o600,
        "a control socket is the access boundary; it must not be group- or world-reachable"
    );
    drop(listener);
}

#[test]
fn binding_does_not_re_permission_a_directory_it_did_not_create() {
    use std::os::unix::fs::PermissionsExt as _;
    let dir = tempfile::TempDir::new().unwrap();
    let shared = dir.path().join("shared");
    std::fs::create_dir(&shared).unwrap();
    std::fs::set_permissions(&shared, std::fs::Permissions::from_mode(0o1777)).unwrap();

    let listener = bind_control_socket(&shared.join("s.sock")).unwrap();
    assert_eq!(
        std::fs::metadata(&shared).unwrap().permissions().mode() & 0o7777,
        0o1777,
        "a directory lyrel did not create must keep its own permissions"
    );
    drop(listener);
}

#[test]
fn a_bind_is_owner_only_under_a_permissive_umask() {
    use std::os::unix::fs::PermissionsExt as _;
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("permissive.sock");

    let previous = unsafe { libc::umask(0) };
    let bound = bind_private(&path);
    unsafe { libc::umask(previous) };

    let listener = bound.unwrap();
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o077,
        0,
        "bind left a window in which another user could connect"
    );
    drop(listener);
}

#[test]
fn a_socket_connection_serves_requests() {
    let dir = tempfile::TempDir::new().unwrap();
    let sock = dir.path().join("s.sock");
    let listener = bind_control_socket(&sock).unwrap();
    std::thread::spawn(move || serve_listener(listener, LocalHost::new()));

    let stream = UnixStream::connect(&sock).unwrap();
    let hello = ControlHello::host_rpc("t", "h");
    let host = RemoteHost::over_unix(stream, "test:sock", &hello).unwrap();

    let tmp = tempfile::TempDir::new().unwrap();
    host.write_file(&tmp.path().join("x.txt"), b"content")
        .unwrap();
    assert_eq!(
        host.read_file(&tmp.path().join("x.txt"), 1024).unwrap(),
        b"content"
    );
    let entries = host.read_dir(tmp.path(), None).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].name, "x.txt");
}

#[test]
fn binding_refuses_a_live_server() {
    let dir = tempfile::TempDir::new().unwrap();
    let sock = dir.path().join("s.sock");
    let _first = bind_control_socket(&sock).unwrap();
    assert_eq!(
        bind_control_socket(&sock).unwrap_err().kind(),
        io::ErrorKind::AddrInUse
    );
}

#[test]
fn a_too_long_socket_path_falls_back_to_one_that_fits() {
    let short = PathBuf::from("/tmp/rt");
    let long = PathBuf::from(format!("/tmp/{}", "d".repeat(120)));

    assert_eq!(
        socket_path_in(&short, &[]).unwrap(),
        PathBuf::from("/tmp/rt/control.sock")
    );

    let picked = socket_path_in(&long, &[long.clone(), short.clone()]).unwrap();
    assert!(picked.starts_with(&short), "{}", picked.display());
    assert!(picked.to_string_lossy().ends_with(".sock"));

    let other = PathBuf::from(format!("/tmp/{}", "e".repeat(120)));
    assert_ne!(
        picked,
        socket_path_in(&other, std::slice::from_ref(&short)).unwrap()
    );

    assert_eq!(
        picked,
        socket_path_in(&long, &[long.clone(), short.clone()]).unwrap()
    );

    let err = socket_path_in(&long, std::slice::from_ref(&long)).unwrap_err();
    assert!(err.to_string().contains(CONTROL_SOCK_ENV), "{err}");
}

/// `run_daemon` keeps serving panes past a control listener it could not
/// open only when something else is answering there, and the errno cannot
/// make that call. `bind_control_socket` clears the leftovers it can, but a
/// path it cannot clear still comes back `AddrInUse` — indistinguishable
/// from a live server, with nothing listening behind it. A daemon that read
/// the error as "someone else is serving" stayed alive holding the
/// single-server lock, answering nothing, forever.
#[cfg(unix)]
#[test]
fn only_a_connect_tells_a_live_server_from_a_blocked_path() {
    let dir = tempfile::TempDir::new().unwrap();

    let sock = dir.path().join("s.sock");
    let listener = bind_control_socket(&sock).unwrap();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            drop(stream);
        }
    });
    assert_eq!(
        bind_control_socket(&sock).unwrap_err().kind(),
        io::ErrorKind::AddrInUse,
        "a live server is in use"
    );
    assert!(
        control_socket_answers(&sock),
        "and it answers, which is what makes it live"
    );

    // A directory where the socket goes. Nothing is listening, the connect
    // fails so there is nothing to unlink, and `bind` refuses it in the
    // same words it used for the live server above.
    let blocked = dir.path().join("blocked.sock");
    std::fs::create_dir(&blocked).unwrap();
    assert_eq!(
        bind_control_socket(&blocked).unwrap_err().kind(),
        io::ErrorKind::AddrInUse,
        "same error, and no server anywhere near it"
    );
    assert!(
        !control_socket_answers(&blocked),
        "which is the difference the daemon has to act on"
    );
}

#[test]
fn binding_clears_a_socket_a_crash_left_behind() {
    let dir = tempfile::TempDir::new().unwrap();
    let sock = dir.path().join("s.sock");
    let first = bind_control_socket(&sock).unwrap();
    drop(first);
    assert!(sock.exists());

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match bind_control_socket(&sock) {
            Ok(listener) => {
                drop(listener);
                return;
            }
            Err(e) if Instant::now() >= deadline => {
                panic!("a stale control socket was never cleared: {e}")
            }
            Err(_) => std::thread::sleep(Duration::from_millis(20)),
        }
    }
}
