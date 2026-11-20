//! Insights statistics/skills behavior over a real `RemoteHost`, against a
//! synthetic TCP control peer. These cases moved out of
//! `lyrel-core`'s `host::remote` tests with their owner: they consume
//! `lyrel_insights::statistics`/`skills` types across the wire, so they live
//! where those types live. The fake peer speaks the public control protocol
//! (`lyrel_protocol::daemon::control`) over a real socket — the same mechanism
//! core's remote tests use.

use std::io::Write as _;
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, mpsc};
use std::thread;

use lyrel_core::host::remote::RemoteHost;
use lyrel_insights::statistics::{self, DateWindow, Source, StatisticsCache};
use lyrel_protocol::core::cli_agent::CLIAgent;
use lyrel_protocol::daemon::control::{
    CONTROL_VERSION, ControlClientMsg, ControlHello, ControlHelloOk, ControlReply, ControlRequest,
    ControlServerMsg, ReplyOk, feature,
};
use lyrel_protocol::daemon::protocol::PROTOCOL_VERSION;
use lyrel_protocol::host::Host as _;
use lyrel_protocol::host::session_operations::{
    SessionOperation, SessionOperationKind, SessionScope,
};

fn hello_ok(separator: char) -> ControlHelloOk {
    ControlHelloOk {
        control_version: CONTROL_VERSION,
        protocol_version: PROTOCOL_VERSION,
        build: "test".into(),
        separator,
        home: "/home/me".into(),
        agent_environment: None,
        features: vec![feature::CONTROL.into(), feature::HOST_RPC.into()],
        instance: "test-instance".into(),
    }
}

fn host_with_peer_announcing<F>(
    separator: char,
    extra: &[&str],
    answer: F,
) -> (Arc<RemoteHost>, mpsc::Receiver<ControlRequest>)
where
    F: Fn(&ControlRequest) -> Option<(ControlReply, Vec<u8>)> + Send + 'static,
{
    let mut hello = hello_ok(separator);
    hello.features.extend(extra.iter().map(|f| f.to_string()));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let (seen_tx, seen_rx) = mpsc::channel();
    thread::spawn(move || {
        let (mut sock, _) = listener.accept().unwrap();
        match ControlClientMsg::read(&mut sock).unwrap() {
            ControlClientMsg::Hello(_) => {}
            other => panic!("expected Hello, got {other:?}"),
        }
        ControlServerMsg::HelloOk(hello).encode(&mut sock).unwrap();
        sock.flush().unwrap();
        loop {
            let (req_id, req) = match ControlClientMsg::read(&mut sock) {
                Ok(ControlClientMsg::Request { req_id, req }) => (req_id, req),
                Ok(ControlClientMsg::RequestBlob { req_id, req, blob }) => {
                    let _ = blob;
                    (req_id, req)
                }
                _ => return,
            };
            let reply = answer(&req);
            let _ = seen_tx.send(req);
            match reply {
                Some((reply, blob)) if blob.is_empty() => {
                    ControlServerMsg::Response { req_id, reply }
                        .encode(&mut sock)
                        .unwrap();
                }
                Some((reply, blob)) => {
                    ControlServerMsg::ResponseBlob {
                        req_id,
                        reply,
                        blob,
                    }
                    .encode(&mut sock)
                    .unwrap();
                }
                None => return,
            }
            sock.flush().unwrap();
        }
    });

    let sock = TcpStream::connect(addr).unwrap();
    let host = RemoteHost::over_tcp(
        sock,
        "ssh-alias:testbox",
        &ControlHello::host_rpc("tok", "laptop"),
    )
    .unwrap();
    (host, seen_rx)
}

fn session_request(host: lyrel_protocol::host::HostId) -> SessionOperation {
    SessionOperation {
        expected_host: host.0,
        agent: CLIAgent::OhMyPi,
        kind: SessionOperationKind::Fork,
        source_id: "same-id-on-two-hosts".into(),
        cwd: std::path::PathBuf::from("/remote/project"),
        launch_argv: vec!["omp".into()],
        scope: SessionScope::default(),
    }
}

#[test]
fn skills_require_selected_peer_environment_before_reading_any_files() {
    let (host, seen) = host_with_peer_announcing('/', &[], |_| {
        panic!("old skill peer must not receive filesystem or command requests")
    });
    let error = lyrel_insights::skills::scan(
        &*host,
        &lyrel_insights::skills::ScanRequest::new("/peer/home"),
    )
    .unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::Unsupported);
    assert!(seen.try_recv().is_err());
}

#[test]
fn hermes_statistics_preserves_cost_kinds_and_window_completeness() {
    let (host, _) = host_with_peer_announcing('/', &[feature::HERMES_STATISTICS], |request| {
        let ControlRequest::HermesStatistics { request } = request else {
            panic!("expected scoped statistics")
        };
        assert_eq!(request.expected_host, 0);
        assert_eq!(request.scope.profile.as_deref(), Some("work"));
        let rows = serde_json::json!({"projects":[{"path":"/work", "sessionCount":3,"sessionIds":["main","branch","continuation"],"previewSessions":[
                {"id":"main","source":"cli","started_at":100,"last_active":90000,"input_tokens":10,"output_tokens":5,"actual_cost_usd":0,"estimated_cost_usd":0.8,"model":"native"},
                {"id":"branch","source":"cli","parent_session_id":"main","_lineage_root_id":"main","_lineage_ids":["branch"],"started_at":300,"last_active":400,"input_tokens":7,"output_tokens":3,"estimated_cost_usd":0.2},
                {"id":"continuation","source":"cli","_lineage_ids":["main","continuation"],"started_at":100,"last_active":90000,"input_tokens":100,"output_tokens":50,"actual_cost_usd":99}
            ]}],"scoped_session_ids":["main","branch","continuation"]});
        Some((
            ControlReply::Ok(ReplyOk::HermesStatistics(Box::new(Ok(rows)))),
            vec![],
        ))
    });
    let mut request = session_request(host.id());
    request.agent = CLIAgent::Hermes;
    request.scope.profile = Some("work".into());
    let source = Source::hermes(
        std::path::PathBuf::from("/remote/hermes/profiles/work"),
        "hermes:work",
        request,
    );
    let snapshot = statistics::scan(&*host, &[source], &mut StatisticsCache::default()).unwrap();
    let totals = snapshot.aggregate(&DateWindow::all("UTC"));
    assert_eq!(totals.main_sessions(), 2);
    assert_eq!(totals.tokens().total(), Some(25));
    assert_eq!(totals.recorded_usd(), Some(0.0));
    assert_eq!(totals.estimated_usd(), Some(1.0));
    assert!(
        !snapshot.is_complete(),
        "public gateway cannot include child usage"
    );
    assert!(
        snapshot
            .issues()
            .iter()
            .any(|i| i.reason().contains("children"))
    );
    use chrono::TimeZone;
    let day = DateWindow::calendar(
        statistics::Period::Today,
        chrono::Utc.with_ymd_and_hms(1970, 1, 2, 12, 0, 0).unwrap(),
        None,
    )
    .unwrap();
    let partial = snapshot.aggregate(&day);
    assert_eq!(
        partial.tokens().total(),
        None,
        "session cumulative totals must not be attributed to its last day"
    );
    assert!(!partial.is_complete());
}

#[test]
fn hermes_statistics_marks_global_preview_and_ambiguous_empty_results() {
    for (value, expected) in [
        (
            serde_json::json!({"projects":[],"active_id":null,"scoped_session_ids":[]}),
            "empty response",
        ),
        (
            serde_json::json!({"projects":[{"sessionCount":502,"previewSessions":[],"sessionIds":[]}],"scoped_session_ids":vec!["stored";501]}),
            "global 501",
        ),
        (
            serde_json::json!({"projects":[{"sessionCount":2,"previewSessions":[],"sessionIds":["a","b"]}],"scoped_session_ids":[]}),
            "preview window",
        ),
    ] {
        let (host, _) = host_with_peer_announcing('/', &[feature::HERMES_STATISTICS], move |_| {
            Some((
                ControlReply::Ok(ReplyOk::HermesStatistics(Box::new(Ok(value.clone())))),
                vec![],
            ))
        });
        let mut request = session_request(host.id());
        request.agent = CLIAgent::Hermes;
        let source = Source::hermes("/isolated".into(), "scope", request);
        let snapshot =
            statistics::scan(&*host, &[source], &mut StatisticsCache::default()).unwrap();
        assert!(!snapshot.is_complete());
        assert!(
            snapshot
                .issues()
                .iter()
                .any(|i| i.reason().contains(expected)),
            "{:?}",
            snapshot.issues()
        );
    }
}

#[test]
fn hermes_statistics_keeps_known_undated_fields_and_cached_values_on_failure() {
    let calls = std::sync::atomic::AtomicUsize::new(0);
    let (host, _) = host_with_peer_announcing('/', &[feature::HERMES_STATISTICS], move |_| {
        let attempt = calls.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let result = if attempt == 0 {
            Ok(
                serde_json::json!({"projects":[{"previewSessions":[{"id":"known","source":"cli","input_tokens":9,"actual_cost_usd":1.25}]}],"scoped_session_ids":["known"]}),
            )
        } else if attempt == 1 {
            Err(
                lyrel_protocol::host::session_operations::SessionOperationError {
                    kind: lyrel_protocol::host::session_operations::SessionErrorKind::Command,
                    message: "runtime failed".into(),
                    retained: None,
                },
            )
        } else {
            Ok(serde_json::json!({"projects":[],"active_id":null,"scoped_session_ids":[]}))
        };
        Some((
            ControlReply::Ok(ReplyOk::HermesStatistics(Box::new(result))),
            vec![],
        ))
    });
    let mut request = session_request(host.id());
    request.agent = CLIAgent::Hermes;
    let source = Source::hermes("/isolated".into(), "scope", request);
    let mut cache = StatisticsCache::default();
    let first = statistics::scan(&*host, std::slice::from_ref(&source), &mut cache).unwrap();
    let totals = first.aggregate(&DateWindow::all("UTC"));
    assert_eq!(totals.main_sessions(), 1);
    assert_eq!(totals.tokens().input(), Some(9));
    assert_eq!(totals.tokens().output(), None);
    assert_eq!(totals.recorded_usd(), Some(1.25));
    use chrono::TimeZone;
    let window = DateWindow::calendar(
        statistics::Period::Today,
        chrono::Utc.with_ymd_and_hms(1970, 1, 1, 12, 0, 0).unwrap(),
        None,
    )
    .unwrap();
    assert_eq!(first.aggregate(&window).tokens().input(), None);
    assert_eq!(
        first.aggregate(&window).main_sessions(),
        0,
        "missing timestamps must not fabricate activity on the Unix epoch day"
    );
    let failed = statistics::scan(&*host, std::slice::from_ref(&source), &mut cache).unwrap();
    assert_eq!(
        failed.aggregate(&DateWindow::all("UTC")).recorded_usd(),
        Some(1.25)
    );
    assert_eq!(failed.updated(), first.updated());
    assert!(
        failed
            .issues()
            .iter()
            .any(|i| i.reason().contains("runtime failed"))
    );
    let empty = statistics::scan(&*host, &[source], &mut cache).unwrap();
    assert_eq!(
        empty.aggregate(&DateWindow::all("UTC")).recorded_usd(),
        Some(1.25),
        "ambiguous empty responses must retain dated cached usage, not claim a new zero history"
    );
    assert_eq!(empty.updated(), first.updated());
    assert!(
        empty
            .issues()
            .iter()
            .any(|i| i.reason().contains("empty response"))
    );
}
