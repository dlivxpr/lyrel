use super::*;
use crate::core::machine::{self, MachineStore};
use crate::daemon::control::{
    CONTROL_VERSION, ControlClientMsg, ControlHello, ControlReply, ControlRequest, ControlServerMsg,
    ReplyOk,
};
use crate::host::local::LocalHost;
use std::net::{TcpListener, TcpStream};
use std::time::Duration;

fn tcp_peer(services: Services) -> (TcpStream, std::thread::JoinHandle<io::Result<()>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let served = std::thread::spawn(move || {
        let (server, _) = listener.accept().unwrap();
        serve_with(server, LocalHost::new(), services)
    });
    let peer = TcpStream::connect(address).unwrap();
    peer.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    (peer, served)
}

fn hello(peer: &mut TcpStream, hello: ControlHello) {
    ControlClientMsg::Hello(hello).encode(peer).unwrap();
    peer.flush().unwrap();
    let ControlServerMsg::HelloOk(reply) = ControlServerMsg::read(peer).unwrap() else {
        panic!("expected negotiated HELLO_OK");
    };
    assert_eq!(reply.control_version, CONTROL_VERSION);
}

fn call(peer: &mut TcpStream, req_id: u64, req: ControlRequest) -> ControlReply {
    ControlClientMsg::Request { req_id, req }.encode(peer).unwrap();
    peer.flush().unwrap();
    loop {
        match ControlServerMsg::read(peer).unwrap() {
            ControlServerMsg::Response { req_id: got, reply } if got == req_id => return reply,
            ControlServerMsg::Event(_) => continue,
            other => panic!("unexpected frame {other:?}"),
        }
    }
}

#[test]
fn a_tcp_protocol_error_releases_attachment_and_unregisters_the_gui() {
    let dir = tempfile::TempDir::new().unwrap();
    let store = MachineStore::open(dir.path().join(machine::MACHINE_FILE));
    let workspace = store.workspace_create(None, None, None).unwrap().id;
    let services = Services::with_machine(Arc::clone(&store));
    let (mut gui, served) = tcp_peer(services.clone());
    hello(
        &mut gui,
        ControlHello {
            workspace: Some(workspace.to_string()),
            ..ControlHello::gui("gui-token", "desktop")
        },
    );
    assert_eq!(call(&mut gui, 1, ControlRequest::Ping), ControlReply::Ok(ReplyOk::Pong));
    assert_eq!(store.attachment(workspace).unwrap().token, "gui-token");
    assert_eq!(
        call(&mut gui, 2, ControlRequest::GuiOpen { path: None, workspace: None }),
        ControlReply::Ok(ReplyOk::Bool(true)),
    );

    // A protocol error, not only clean EOF, must converge on explicit teardown.
    ControlClientMsg::Hello(ControlHello::host_rpc("second", "invalid"))
        .encode(&mut gui)
        .unwrap();
    gui.flush().unwrap();
    let error = served.join().unwrap().unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    loop {
        if let Err(error) = ControlServerMsg::read(&mut gui) {
            assert!(is_disconnect(&error), "expected disconnect, got {error}");
            break;
        }
    }
    assert!(services.attachments.is_empty());
    assert!(store.attachment(workspace).is_none());

    // Ask through another real control connection, without registry test access.
    let (mut probe, probe_served) = tcp_peer(services);
    hello(&mut probe, ControlHello::host_rpc("probe", "probe"));
    assert_eq!(
        call(&mut probe, 1, ControlRequest::GuiOpen { path: None, workspace: None }),
        ControlReply::Ok(ReplyOk::Bool(false)),
    );
    drop(probe);
    probe_served.join().unwrap().unwrap();
}
