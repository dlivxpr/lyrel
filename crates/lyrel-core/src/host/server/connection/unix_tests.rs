use super::super::test_support::*;
use super::super::*;
use super::*;
use crate::daemon::control::ControlHello;
use crate::host::local::LocalHost;
use std::os::unix::net::UnixStream;

#[test]
fn a_closed_connection_stops_being_a_subscriber() {
    let dir = tempfile::TempDir::new().unwrap();
    let store = MachineStore::open(dir.path().join(machine::MACHINE_FILE));
    let served = {
        let store = Arc::clone(&store);
        let (server, client) = UnixStream::pair().unwrap();
        let handle = std::thread::spawn(move || {
            let _ = serve_with(server, LocalHost::new(), Services::with_machine(store));
        });
        let mut client = client;
        ControlClientMsg::Hello(ControlHello::host_rpc("t", "h"))
            .encode(&mut client)
            .unwrap();
        client.flush().unwrap();
        let _ = ControlServerMsg::read(&mut client).unwrap();
        drop(client);
        handle
    };
    served.join().unwrap();

    let ws = store
        .workspace_create(None, Some("api".into()), None)
        .unwrap();
    assert_eq!(store.machine().workspaces.len(), 1);
    assert_eq!(store.workspace(ws.id).unwrap().name.as_deref(), Some("api"));
}

#[test]
fn an_unknown_frame_kind_ends_the_connection() {
    let (mut client, _) = raw();
    crate::daemon::protocol::write_frame(&mut client, 99, b"junk").unwrap();
    client.flush().unwrap();
    let mut buf = [0u8; 1];
    assert_eq!(
        client.read(&mut buf).unwrap(),
        0,
        "the server should hang up"
    );
}
