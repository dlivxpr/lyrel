use crate::address::Context;
use crate::commands::execute;
use crate::commands::test_support::cli;
use crate::commands::test_support::human;
use crate::commands::test_support::mock;
use crate::commands::test_support::run_cli;
use lyrel_core::daemon::control::CONTROL_VERSION;
use lyrel_core::daemon::control::ControlRequest;
use lyrel_core::daemon::control::ReplyOk;
use lyrel_core::daemon::protocol::PROTOCOL_VERSION;

#[test]
fn a_machine_flag_refuses_the_local_server_verbs() {
    for verb in ["start", "stop", "restart", "logs"] {
        let mut backend = mock();
        let err = execute(
            cli(&["lyrel", "-m", "devbox", "server", verb]),
            &Context::default(),
            &mut backend,
        )
        .expect_err("server lifecycle verbs are local-only");
        let msg = err.to_string();
        assert!(msg.contains("LOCAL"), "{msg}");
        assert!(msg.contains(verb), "{msg}");
        assert!(
            backend.control_calls.is_empty(),
            "the refusal must come before any dial"
        );
    }
}

#[test]
fn status_and_machine_ls_are_single_aggregate_requests() {
    use lyrel_core::daemon::control::{RouteInfo, ServerStatus};

    let mut backend = mock();
    backend.replies.push_back(ReplyOk::Status(ServerStatus {
        pid: 4242,
        uptime_secs: 61,
        panes: 3,
        control_version: CONTROL_VERSION,
        protocol_version: PROTOCOL_VERSION,
        build: "26.7.5".into(),
        socket: "127.0.0.1:5555".into(),
    }));
    let out = run_cli(&["lyrel", "status"], &Context::default(), &mut backend);
    assert_eq!(backend.control_calls, vec![ControlRequest::Status]);
    let rendered = human(out);
    assert!(rendered.contains("4242"), "{rendered}");
    assert!(rendered.contains("61s"), "{rendered}");

    let mut backend = mock();
    backend.replies.push_back(ReplyOk::Routes(vec![RouteInfo {
        key: "me@build-box:22".into(),
        kind: "ssh".into(),
        connected: true,
    }]));
    let out = run_cli(
        &["lyrel", "machine", "ls"],
        &Context::default(),
        &mut backend,
    );
    assert_eq!(backend.control_calls, vec![ControlRequest::Routes]);
    let rendered = human(out);
    assert!(
        rendered.contains("local"),
        "machine 0 is always listed: {rendered}"
    );
    assert!(rendered.contains("me@build-box:22"), "{rendered}");
}
