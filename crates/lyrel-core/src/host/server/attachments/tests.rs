use std::io::Write;
use std::os::unix::net::UnixStream;
use std::time::Instant;

use crate::daemon::control::CONTROL_VERSION;

use super::super::test_support::*;
use super::super::*;
use super::*;

#[test]
fn a_second_client_takes_the_workspace_and_the_first_is_told() {
    let (services, _dir) = workspace_services();
    let registry = Arc::clone(&services.attachments);
    let w = tree_workspace(&services);

    let ((mut laptop, _), _laptop_served) =
        raw_hello(services.clone(), hello_for(&w, "tok-laptop", "laptop"));
    await_holder(&registry, &w, "laptop");

    let ((mut desktop, _), _desktop_served) =
        raw_hello(services.clone(), hello_for(&w, "tok-desktop", "desktop"));

    assert_eq!(
        await_preempted(&mut laptop),
        Some(ControlEvent::Preempted {
            workspace: w.clone(),
            by: "desktop".to_string(),
        })
    );
    assert_eq!(registry.holder(&w).map(|(_, h)| h), Some("desktop".into()));
    assert_eq!(
        services
            .machine
            .as_ref()
            .unwrap()
            .attachment(w.parse().unwrap())
            .unwrap()
            .hostname,
        "desktop",
        "the tree's record moves with the live handles"
    );

    let (reply, _) = round_trip(
        &mut desktop,
        1,
        ControlRequest::WorkspaceAttach { id: w.clone() },
    );
    assert_eq!(
        reply,
        ControlReply::Ok(ReplyOk::Attached {
            took_over_from: None
        }),
        "re-attaching from the connection that already holds it is not a takeover"
    );
}

#[test]
fn a_dedicated_connection_is_closed_when_its_workspace_is_taken() {
    let (services, _dir) = workspace_services();
    let w = tree_workspace(&services);
    let ((mut laptop, _), _l) = raw_hello(services.clone(), hello_for(&w, "tok-laptop", "laptop"));
    await_holder(&services.attachments, &w, "laptop");
    let ((_desktop, _), _d) = raw_hello(services.clone(), hello_for(&w, "tok-desktop", "desktop"));

    assert!(await_preempted(&mut laptop).is_some());
    let mut sink = Vec::new();
    let _ = std::io::Read::read_to_end(&mut laptop, &mut sink);
    assert!(
        ControlServerMsg::read(&mut laptop).is_err(),
        "a dedicated connection is closed once its workspace is gone"
    );
}

#[test]
fn removing_a_workspace_clears_both_attachment_tables() {
    let (services, _dir) = workspace_services();
    let registry = Arc::clone(&services.attachments);
    let machine = services.machine.clone().unwrap();
    let w = tree_workspace(&services);
    let id: crate::core::session::WorkspaceId = w.parse().unwrap();

    let ((mut laptop, _), _l) = raw_hello(services.clone(), hello_for(&w, "tok-laptop", "laptop"));
    await_holder(&registry, &w, "laptop");
    assert!(machine.attachment(id).is_some());

    let reply = ask(
        &mut laptop,
        1,
        ControlRequest::WorkspaceRemove { workspace: id },
    );
    assert!(
        matches!(reply, ControlReply::Ok(ReplyOk::Panes(_))),
        "{reply:?}"
    );

    assert!(
        machine.attachment(id).is_none(),
        "the tree still names a holder for a workspace that is gone"
    );
    assert!(
        registry.holder(&w).is_none(),
        "the registry still holds a workspace that is gone"
    );
}

#[test]
fn an_attach_moves_both_tables_under_one_lock() {
    let (services, _dir) = workspace_services();
    let registry = Arc::clone(&services.attachments);
    let machine = services.machine.clone().unwrap();
    let w = tree_workspace(&services);
    let id: crate::core::session::WorkspaceId = w.parse().unwrap();

    let held = registry.handover();
    let ((_laptop, _ok), _served) =
        raw_hello(services.clone(), hello_for(&w, "tok-laptop", "laptop"));

    std::thread::sleep(Duration::from_millis(150));
    assert!(
        registry.holder(&w).is_none(),
        "the registry was moved while a handover was in flight"
    );
    assert!(
        machine.attachment(id).is_none(),
        "the tree was moved while a handover was in flight"
    );

    drop(held);
    await_holder(&registry, &w, "laptop");
    assert_eq!(
        machine.attachment(id).map(|a| a.token).as_deref(),
        Some("tok-laptop"),
        "both tables have to name the same session once the handover is done"
    );
}

#[test]
fn a_shared_connection_survives_losing_one_of_its_workspaces() {
    let (services, _dir) = workspace_services();
    let registry = Arc::clone(&services.attachments);
    let w1 = tree_workspace(&services);
    let w2 = tree_workspace(&services);

    let ((mut laptop, _), _l) = raw_hello(
        services.clone(),
        ControlHello::host_rpc("tok-laptop", "laptop"),
    );
    for (i, id) in [&w1, &w2].iter().enumerate() {
        let (reply, _) = round_trip(
            &mut laptop,
            i as u64 + 1,
            ControlRequest::WorkspaceAttach { id: (*id).clone() },
        );
        assert_eq!(
            reply,
            ControlReply::Ok(ReplyOk::Attached {
                took_over_from: None
            })
        );
    }
    assert_eq!(registry.len(), 2);

    let ((_desktop, _), _d) = raw_hello(services.clone(), hello_for(&w1, "tok-desktop", "desktop"));
    assert_eq!(
        await_preempted(&mut laptop),
        Some(ControlEvent::Preempted {
            workspace: w1.clone(),
            by: "desktop".to_string(),
        })
    );

    let (reply, _) = round_trip(&mut laptop, 9, ControlRequest::Ping);
    assert_eq!(reply, ControlReply::Ok(ReplyOk::Pong));
    assert_eq!(
        registry.holder(&w2).map(|(t, _)| t),
        Some("tok-laptop".into())
    );
    assert_eq!(
        registry.holder(&w1).map(|(t, _)| t),
        Some("tok-desktop".into())
    );
}

/// The machine's own GUI is one more client of its daemon: it claims the
/// workspaces its windows show, is taken over like anyone else, and takes
/// them back the same way — one window drives a workspace at a time.
/// Neither side is hung up on: both are shared (non-dedicated) links.
#[test]
fn the_machines_own_gui_and_a_remote_client_take_a_workspace_from_each_other() {
    let (services, _dir) = workspace_services();
    let registry = Arc::clone(&services.attachments);
    let w = tree_workspace(&services);
    let id: crate::core::session::WorkspaceId = w.parse().unwrap();

    let ((mut gui, _), _g) = raw_hello(services.clone(), ControlHello::gui("tok-gui", "studio"));
    let (reply, _) = round_trip(
        &mut gui,
        1,
        ControlRequest::WorkspaceAttach { id: w.clone() },
    );
    assert_eq!(
        reply,
        ControlReply::Ok(ReplyOk::Attached {
            took_over_from: None
        })
    );

    // A remote client opens the same workspace.
    let ((mut laptop, _), _l) = raw_hello(
        services.clone(),
        ControlHello::host_rpc("tok-laptop", "laptop"),
    );
    let (reply, _) = round_trip(
        &mut laptop,
        1,
        ControlRequest::WorkspaceAttach { id: w.clone() },
    );
    assert_eq!(
        reply,
        ControlReply::Ok(ReplyOk::Attached {
            took_over_from: Some("studio".into())
        }),
        "the remote is told it took the workspace from the machine itself, by name"
    );
    assert_eq!(
        await_preempted(&mut gui),
        Some(ControlEvent::Preempted {
            workspace: w.clone(),
            by: "laptop".into(),
        })
    );

    // What a restored or reconnected local window reads before claiming:
    // the holder, by name, so it can open taken over instead of grabbing.
    let (reply, _) = round_trip(&mut gui, 2, ControlRequest::WorkspaceTree { workspace: id });
    match reply {
        ControlReply::Ok(ReplyOk::WorkspaceTree(ws)) => {
            assert_eq!(ws.attachment.map(|a| a.hostname).as_deref(), Some("laptop"));
        }
        other => panic!("{other:?}"),
    }
    let (reply, _) = round_trip(&mut gui, 3, ControlRequest::Ping);
    assert_eq!(
        reply,
        ControlReply::Ok(ReplyOk::Pong),
        "a GUI that was taken over keeps its link"
    );

    // Take Back from the machine's own window.
    let (reply, _) = round_trip(
        &mut gui,
        4,
        ControlRequest::WorkspaceAttach { id: w.clone() },
    );
    assert_eq!(
        reply,
        ControlReply::Ok(ReplyOk::Attached {
            took_over_from: Some("laptop".into())
        })
    );
    assert_eq!(
        await_preempted(&mut laptop),
        Some(ControlEvent::Preempted {
            workspace: w.clone(),
            by: "studio".into(),
        })
    );
    assert_eq!(registry.holder(&w).map(|(t, _)| t), Some("tok-gui".into()));
    let (reply, _) = round_trip(&mut laptop, 2, ControlRequest::Ping);
    assert_eq!(reply, ControlReply::Ok(ReplyOk::Pong));
}

#[test]
fn a_displaced_session_tidying_up_does_not_evict_the_new_owner() {
    let (services, _dir) = workspace_services();
    let registry = Arc::clone(&services.attachments);
    let machine = Arc::clone(services.machine.as_ref().unwrap());
    let w = tree_workspace(&services);
    let other = tree_workspace(&services);

    let ((mut laptop, _), _l) = raw_hello(
        services.clone(),
        ControlHello::host_rpc("tok-laptop", "laptop"),
    );
    for (i, id) in [&w, &other].iter().enumerate() {
        round_trip(
            &mut laptop,
            i as u64 + 1,
            ControlRequest::WorkspaceAttach { id: (*id).clone() },
        );
    }
    let ((_desktop, _), _d) = raw_hello(services.clone(), hello_for(&w, "tok-desktop", "desktop"));
    assert!(await_preempted(&mut laptop).is_some());

    let (reply, _) = round_trip(
        &mut laptop,
        3,
        ControlRequest::WorkspaceDetach { id: w.clone() },
    );
    assert_eq!(reply, ControlReply::Ok(ReplyOk::Unit));
    assert_eq!(
        registry.holder(&w).map(|(t, _)| t),
        Some("tok-desktop".into()),
        "the displaced session must not release what it no longer holds"
    );
    assert_eq!(
        machine.attachment(w.parse().unwrap()).unwrap().token,
        "tok-desktop"
    );
}

#[test]
fn a_closed_connection_releases_what_it_held() {
    let (services, _dir) = workspace_services();
    let registry = Arc::clone(&services.attachments);
    let machine = Arc::clone(services.machine.as_ref().unwrap());
    let w = tree_workspace(&services);
    {
        let ((client, _), served) = raw_hello(services.clone(), hello_for(&w, "tok", "laptop"));
        await_holder(&registry, &w, "laptop");
        drop(client);
        served.join().unwrap();
    }
    assert!(registry.is_empty(), "the registry outlived the connection");
    assert_eq!(machine.attachment(w.parse().unwrap()), None);
}

#[test]
fn attaching_to_a_server_without_a_tree_is_an_error() {
    let (mut client, _) = raw_with(Services::none());
    let (reply, _) = round_trip(
        &mut client,
        1,
        ControlRequest::WorkspaceAttach { id: "w".into() },
    );
    assert!(matches!(reply, ControlReply::Err(_)), "{reply:?}");
}
pub(super) fn hello_for(workspace: &str, token: &str, hostname: &str) -> ControlHello {
    ControlHello {
        control_version: CONTROL_VERSION,
        workspace: Some(workspace.to_string()),
        client_token: token.to_string(),
        client_hostname: hostname.to_string(),
        gui: false,
    }
}

pub(super) fn round_trip(
    sock: &mut UnixStream,
    req_id: u64,
    req: ControlRequest,
) -> (ControlReply, Vec<ControlEvent>) {
    ControlClientMsg::Request { req_id, req }
        .encode(sock)
        .unwrap();
    sock.flush().unwrap();
    let mut events = Vec::new();
    loop {
        match ControlServerMsg::read(sock).unwrap() {
            ControlServerMsg::Response { req_id: got, reply } if got == req_id => {
                return (reply, events);
            }
            ControlServerMsg::Event(e) => events.push(e),
            other => panic!("unexpected frame {other:?}"),
        }
    }
}

pub(super) fn await_preempted(sock: &mut UnixStream) -> Option<ControlEvent> {
    loop {
        match ControlServerMsg::read(sock) {
            Ok(ControlServerMsg::Event(e @ ControlEvent::Preempted { .. })) => return Some(e),
            Ok(_) => continue,
            Err(_) => return None,
        }
    }
}

pub(super) fn await_holder(registry: &AttachRegistry, workspace: &str, hostname: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while registry.holder(workspace).map(|(_, h)| h).as_deref() != Some(hostname) {
        assert!(
            Instant::now() < deadline,
            "{workspace} is held by {:?}, not {hostname}",
            registry.holder(workspace)
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}
pub(super) fn tree_workspace(services: &Services) -> String {
    services
        .machine
        .as_ref()
        .expect("workspace_services always carries a tree")
        .workspace_create(None, None, None)
        .expect("an empty tree accepts a workspace")
        .id
        .to_string()
}
