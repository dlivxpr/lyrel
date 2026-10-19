use super::*;
use crate::host::local::LocalHost;
use crate::statistics::{DateWindow, StatisticsCache, scan};

fn isolated_launch(root: &Path) -> HashMap<String, String> {
    let omp = root.join("native-omp");
    let pi = root.join("native-pi");
    let kimi = root.join("native-kimi");
    let hermes = root.join("native-hermes");
    for directory in [&omp, &pi, &kimi, &hermes] {
        std::fs::create_dir_all(directory).unwrap();
    }
    HashMap::from([
        (
            "omp".into(),
            format!(
                "OMP_PROFILE=default PI_CONFIG_DIR='{}' PI_CODING_AGENT_DIR='{}' XDG_DATA_HOME='{}' XDG_STATE_HOME='{}' omp",
                omp.display(),
                omp.join("agent").display(),
                omp.display(),
                omp.display()
            ),
        ),
        (
            "pi".into(),
            format!(
                "PI_CODING_AGENT_DIR='{}' PI_CODING_AGENT_SESSION_DIR='{}' pi",
                pi.display(),
                pi.join("sessions").display()
            ),
        ),
        (
            "kimi".into(),
            format!("KIMI_CODE_HOME='{}' kimi", kimi.display()),
        ),
        (
            "hermes".into(),
            format!(
                "HOME='{}' USERPROFILE='{}' HERMES_HOME='{}' '{}'",
                root.display(),
                root.display(),
                hermes.display(),
                root.join("missing-hermes/hermes").display()
            ),
        ),
        (
            "opencode".into(),
            format!(
                "HOME='{}' USERPROFILE='{}' '{}'",
                root.display(),
                root.display(),
                root.join("missing-opencode/opencode").display()
            ),
        ),
    ])
}

fn native_history(id: &str) -> String {
    format!(
        "{{\"type\":\"session\",\"version\":3,\"id\":\"{id}\",\"timestamp\":\"2026-10-01T00:00:00Z\"}}\n{{\"type\":\"message\",\"id\":\"usage\",\"timestamp\":\"2026-10-01T00:00:01Z\",\"message\":{{\"role\":\"assistant\",\"provider\":\"test\",\"model\":\"test\",\"usage\":{{\"input\":7,\"output\":3,\"totalTokens\":10}}}}}}\n"
    )
}

#[test]
fn statistics_source_association_changes_scan_and_removal_preserves_native_history() {
    let temp = tempfile::tempdir().unwrap();
    let launch = isolated_launch(temp.path());
    let path = temp.path().join("extra.jsonl");
    let history = native_history("extra");
    std::fs::write(&path, &history).unwrap();
    let host = LocalHost::new();
    let mut bindings = SourceBindings::default();
    let binding = bindings
        .add(&*host, "omp", path.to_str().unwrap(), &launch)
        .unwrap();
    let mut cache = StatisticsCache::default();
    let resolved = bindings.resolve_for_host(&*host, Vec::new(), &HashMap::new());
    assert!(resolved.issues.is_empty(), "{:?}", resolved.issues);
    let snapshot = scan(&*host, &resolved.sources, &mut cache).unwrap();
    assert_eq!(
        snapshot.aggregate(&DateWindow::all("UTC")).tokens().total(),
        Some(10)
    );
    let serialized = serde_json::to_string(&bindings).unwrap();
    let mut restored: SourceBindings = serde_json::from_str(&serialized).unwrap();
    assert_eq!(
        restored
            .check(&*host, &binding, &HashMap::new())
            .unwrap()
            .root(),
        host.canonicalize(&path).unwrap()
    );
    restored.remove(&binding);
    let removed = restored.resolve_for_host(&*host, Vec::new(), &HashMap::new());
    let snapshot = scan(&*host, &removed.sources, &mut cache).unwrap();
    assert_eq!(
        snapshot.aggregate(&DateWindow::all("UTC")).main_sessions(),
        0
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), history);
}

#[test]
fn statistics_sources_reject_overlap_and_can_exclude_automatic_discovery_per_host() {
    let temp = tempfile::tempdir().unwrap();
    let launch = isolated_launch(temp.path());
    let directory = temp.path().join("history");
    std::fs::create_dir(&directory).unwrap();
    let file = directory.join("session.jsonl");
    std::fs::write(&file, native_history("same")).unwrap();
    let host = LocalHost::new();
    let mut bindings = SourceBindings::default();
    let binding = bindings
        .add(&*host, "omp", directory.to_str().unwrap(), &launch)
        .unwrap();
    let error = bindings
        .add(&*host, "omp", file.to_str().unwrap(), &launch)
        .unwrap_err();
    assert!(error.to_string().contains("overlap"));
    let error = bindings
        .add(&*host, "omp", directory.to_str().unwrap(), &launch)
        .unwrap_err();
    assert!(error.to_string().contains("overlap"));
    bindings.remove(&binding);
    let automatic = Source::omp(directory.clone(), "native-default");
    let resolved = bindings.resolve_for_host(&*host, vec![automatic.clone()], &HashMap::new());
    let discovered = resolved.bindings.into_iter().next().unwrap();
    assert!(bindings.is_discovered(&discovered));
    bindings.remove(&discovered);
    let persisted: SourceBindings =
        serde_json::from_str(&serde_json::to_string(&bindings).unwrap()).unwrap();
    let excluded = persisted.resolve_for_host(&*host, vec![automatic], &HashMap::new());
    let snapshot = scan(&*host, &excluded.sources, &mut StatisticsCache::default()).unwrap();
    assert_eq!(
        snapshot.aggregate(&DateWindow::all("UTC")).main_sessions(),
        0
    );
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        native_history("same")
    );
    let mut foreign = discovered.clone();
    foreign.host = 99;
    assert!(
        persisted
            .check(&*host, &foreign, &HashMap::new())
            .unwrap_err()
            .to_string()
            .contains("another Host")
    );
    let mut host_isolated = SourceBindings::default();
    host_isolated.remove(&foreign);
    let resolved = host_isolated.resolve_for_host(
        &*host,
        vec![Source::omp(directory, "native-default")],
        &HashMap::new(),
    );
    let snapshot = scan(&*host, &resolved.sources, &mut StatisticsCache::default()).unwrap();
    assert_eq!(
        snapshot.aggregate(&DateWindow::all("UTC")).tokens().total(),
        Some(10)
    );
}

#[test]
fn statistics_sources_reject_invalid_and_unreadable_sources_without_default_fallback() {
    let host = LocalHost::new();
    let temp = tempfile::tempdir().unwrap();
    let broken = temp.path().join("broken.jsonl");
    std::fs::write(&broken, "not native history").unwrap();
    let mut bindings = SourceBindings::default();
    for (agent, selector) in [
        ("omp", "relative/history"),
        ("pi", ""),
        ("omp", broken.to_str().unwrap()),
        ("kimi", broken.to_str().unwrap()),
        ("opencode", broken.to_str().unwrap()),
        ("opencode", "https://user:secret@example.test/"),
        ("opencode", "https://example.test/?token=secret"),
        ("hermes", broken.to_str().unwrap()),
        ("unknown", temp.path().to_str().unwrap()),
    ] {
        assert!(
            bindings
                .add(&*host, agent, selector, &HashMap::new())
                .is_err(),
            "{agent}"
        );
    }
    let missing = temp.path().join("missing");
    assert_eq!(
        bindings
            .add(&*host, "omp", missing.to_str().unwrap(), &HashMap::new())
            .unwrap_err()
            .kind(),
        io::ErrorKind::NotFound
    );
    let resolved = bindings.resolve_for_host(&*host, Vec::new(), &HashMap::new());
    let snapshot = scan(&*host, &resolved.sources, &mut StatisticsCache::default()).unwrap();
    assert_eq!(
        snapshot.aggregate(&DateWindow::all("UTC")).main_sessions(),
        0
    );
}

#[test]
fn statistics_sources_use_distinct_pi_and_kimi_native_storage_shapes() {
    let temp = tempfile::tempdir().unwrap();
    let launch = isolated_launch(temp.path());
    let pi = temp.path().join("extra-pi");
    std::fs::create_dir(&pi).unwrap();
    std::fs::write(
        pi.join("pi.jsonl"),
        native_history("pi").replace(
            "\"totalTokens\":10",
            "\"cacheRead\":0,\"cacheWrite\":0,\"totalTokens\":10",
        ),
    )
    .unwrap();
    let kimi = temp.path().join("extra-kimi");
    let session = kimi.join("wd_work_0123456789ab/session_main");
    std::fs::create_dir_all(session.join("agents/main")).unwrap();
    std::fs::write(
        session.join("state.json"),
        r#"{"id":"session_main","cwd":"/work","createdAt":1792320000000}"#,
    )
    .unwrap();
    std::fs::write(session.join("agents/main/wire.jsonl"), concat!(
        "{\"type\":\"metadata\",\"protocol_version\":\"1.5\",\"created_at\":1792320000000}\n",
        "{\"type\":\"usage.record\",\"agentId\":\"main\",\"model\":\"kimi-code/kimi-for-coding\",\"usageScope\":\"turn\",\"time\":1792320001000,\"usage\":{\"inputOther\":17,\"output\":2,\"inputCacheRead\":1,\"inputCacheCreation\":0}}\n"
    )).unwrap();
    let host = LocalHost::new();
    let mut bindings = SourceBindings::default();
    bindings
        .add(&*host, "pi", pi.to_str().unwrap(), &launch)
        .unwrap();
    bindings
        .add(&*host, "kimi", kimi.to_str().unwrap(), &launch)
        .unwrap();
    let resolved = bindings.resolve_for_host(&*host, Vec::new(), &launch);
    let snapshot = scan(&*host, &resolved.sources, &mut StatisticsCache::default()).unwrap();
    assert!(snapshot.is_complete(), "{:?}", snapshot.issues());
    assert_eq!(
        snapshot.aggregate(&DateWindow::all("UTC")).tokens().total(),
        Some(30)
    );
    assert_eq!(
        snapshot.aggregate(&DateWindow::all("UTC")).main_sessions(),
        2
    );
}

#[test]
fn statistics_sources_rebuild_native_requests_from_nonsecret_bindings_after_restart() {
    let temp = tempfile::tempdir().unwrap();
    let mut launch = isolated_launch(temp.path());
    launch
        .get_mut("opencode")
        .unwrap()
        .push_str(" --server http://127.0.0.1:43219/ --prompt PRIVATE-LAUNCH-TEXT");
    let profile = temp.path().join("native-hermes/profiles/work");
    std::fs::create_dir_all(&profile).unwrap();
    std::fs::write(profile.join("config.yaml"), "").unwrap();
    launch
        .get_mut("hermes")
        .unwrap()
        .push_str(" --profile work");
    let host = LocalHost::new();
    let discovered = [
        discover_opencode(&*host, &launch).unwrap(),
        discover_hermes(&*host, &launch).unwrap(),
    ]
    .concat();
    let resolved = SourceBindings::default().resolve_for_host(&*host, discovered, &launch);
    let serialized = serde_json::to_string(&resolved.bindings).unwrap();
    assert!(!serialized.contains("PRIVATE-LAUNCH-TEXT"));
    assert!(!serialized.contains("launch_argv"));
    let stored = serde_json::json!({"added": resolved.bindings});
    let restored: SourceBindings = serde_json::from_value(stored).unwrap();
    // Current launch defaults may change, but the selected service/profile cannot.
    let launch = isolated_launch(temp.path());
    let rebuilt = restored.resolve_for_host(&*host, Vec::new(), &launch);
    assert!(rebuilt.issues.is_empty(), "{:?}", rebuilt.issues);
    assert_eq!(
        rebuilt
            .bindings
            .iter()
            .find(|binding| binding.agent == "opencode")
            .unwrap()
            .selector,
        "http://127.0.0.1:43219/"
    );
    let hermes = rebuilt
        .bindings
        .iter()
        .find(|binding| binding.agent == "hermes")
        .unwrap();
    assert!(Path::new(&hermes.selector).ends_with(Path::new("profiles").join("work")));
    assert!(
        rebuilt
            .sources
            .iter()
            .find(|source| source.agent() == "hermes")
            .unwrap()
            .root()
            .ends_with(Path::new("profiles").join("work"))
    );
    let snapshot = scan(&*host, &rebuilt.sources, &mut StatisticsCache::default()).unwrap();
    // Deliberately absent native launchers exercise real Host failures, not a fake Host.
    assert!(!snapshot.is_complete());
    assert_eq!(snapshot.issues().len(), 2);
    assert!(
        snapshot
            .issues()
            .iter()
            .all(|issue| !issue.reason().contains("selector is unavailable")
                && !issue.reason().contains("no launcher scope"))
    );
    let mut foreign = serde_json::to_value(&restored).unwrap();
    for binding in foreign["added"].as_array_mut().unwrap() {
        binding["host"] = serde_json::json!(999);
    }
    let foreign: SourceBindings = serde_json::from_value(foreign).unwrap();
    let resolved = foreign.resolve_for_host(&*host, Vec::new(), &launch);
    let snapshot = scan(&*host, &resolved.sources, &mut StatisticsCache::default()).unwrap();
    assert!(snapshot.is_complete());
    assert_eq!(
        snapshot.aggregate(&DateWindow::all("UTC")).main_sessions(),
        0
    );
}

#[test]
fn statistics_sources_reject_native_profile_and_unbound_service_instead_of_defaulting() {
    let temp = tempfile::tempdir().unwrap();
    let launch = isolated_launch(temp.path());
    let invalid_profile = temp.path().join("profiles/bad profile");
    std::fs::create_dir_all(&invalid_profile).unwrap();
    let host = LocalHost::new();
    let mut bindings = SourceBindings::default();
    assert!(
        bindings
            .add(&*host, "hermes", invalid_profile.to_str().unwrap(), &launch)
            .unwrap_err()
            .to_string()
            .contains("profile name")
    );
    let discovered = discover_opencode(&*host, &launch).unwrap();
    let serialized = serde_json::to_string(&discovered).unwrap();
    let lost_requests: Vec<Source> = serde_json::from_str(&serialized).unwrap();
    let failed = bindings.resolve_for_host(&*host, lost_requests, &launch);
    assert!(
        failed
            .issues
            .iter()
            .any(|issue| issue.reason.contains("durable service selector"))
    );
    let snapshot = scan(&*host, &failed.sources, &mut StatisticsCache::default()).unwrap();
    assert_eq!(
        snapshot.aggregate(&DateWindow::all("UTC")).main_sessions(),
        0
    );
}

#[test]
fn statistics_sources_reject_overlap_with_real_host_discovery_and_can_reassociate_excluded_history()
{
    let temp = tempfile::tempdir().unwrap();
    let launch = isolated_launch(temp.path());
    let root = temp.path().join("native-omp/agent/sessions");
    std::fs::create_dir_all(&root).unwrap();
    let file = root.join("native.jsonl");
    std::fs::write(&file, native_history("native")).unwrap();
    let host = LocalHost::new();
    let mut bindings = SourceBindings::default();
    assert!(
        bindings
            .add(&*host, "omp", file.to_str().unwrap(), &launch)
            .unwrap_err()
            .to_string()
            .contains("automatically discovered")
    );
    let discovered = discover_omp(&*host, &launch).unwrap();
    let resolved = bindings.resolve_for_host(&*host, discovered, &launch);
    let automatic = resolved
        .bindings
        .into_iter()
        .find(|binding| {
            binding.agent == "omp" && Path::new(&binding.selector).ends_with("sessions")
        })
        .unwrap();
    bindings.remove(&automatic);
    let added = bindings
        .add(&*host, "omp", file.to_str().unwrap(), &launch)
        .unwrap();
    let resolved =
        bindings.resolve_for_host(&*host, discover_omp(&*host, &launch).unwrap(), &launch);
    assert!(resolved.issues.is_empty(), "{:?}", resolved.issues);
    let snapshot = scan(&*host, &resolved.sources, &mut StatisticsCache::default()).unwrap();
    assert_eq!(
        snapshot.aggregate(&DateWindow::all("UTC")).tokens().total(),
        Some(10)
    );
    bindings.remove(&added);
    let resolved =
        bindings.resolve_for_host(&*host, discover_omp(&*host, &launch).unwrap(), &launch);
    let snapshot = scan(&*host, &resolved.sources, &mut StatisticsCache::default()).unwrap();
    assert_eq!(
        snapshot.aggregate(&DateWindow::all("UTC")).main_sessions(),
        0
    );
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        native_history("native")
    );
}

#[test]
fn statistics_source_read_failure_keeps_association_and_prior_usage_in_existing_cache() {
    let temp = tempfile::tempdir().unwrap();
    let launch = isolated_launch(temp.path());
    let path = temp.path().join("extra.jsonl");
    std::fs::write(&path, native_history("stale")).unwrap();
    let host = LocalHost::new();
    let mut bindings = SourceBindings::default();
    let binding = bindings
        .add(&*host, "omp", path.to_str().unwrap(), &launch)
        .unwrap();
    let mut cache = StatisticsCache::default();
    let resolved = bindings.resolve_for_host(&*host, Vec::new(), &launch);
    let original = scan(&*host, &resolved.sources, &mut cache).unwrap();
    assert_eq!(
        original.aggregate(&DateWindow::all("UTC")).tokens().total(),
        Some(10)
    );
    std::fs::write(&path, "broken native data after successful scan").unwrap();
    assert!(bindings.check(&*host, &binding, &launch).is_err());
    let resolved = bindings.resolve_for_host(&*host, Vec::new(), &launch);
    let stale = scan(&*host, &resolved.sources, &mut cache).unwrap();
    assert!(!stale.is_complete());
    assert_eq!(
        stale.aggregate(&DateWindow::all("UTC")).tokens().total(),
        Some(10)
    );
    assert_eq!(resolved.bindings[0].key(), binding.key());
    assert_eq!(stale.updated(), original.updated());
}

#[test]
fn statistics_failed_automatic_native_source_remains_removable() {
    let temp = tempfile::tempdir().unwrap();
    let launch = isolated_launch(temp.path());
    let host = LocalHost::new();
    let sources = discover_opencode(&*host, &launch).unwrap();
    let sources: Vec<Source> =
        serde_json::from_str(&serde_json::to_string(&sources).unwrap()).unwrap();
    let mut bindings = SourceBindings::default();
    let failed = bindings.resolve_for_host(&*host, sources.clone(), &launch);
    assert_eq!(failed.issues[0].binding.key(), failed.bindings[0].key());
    bindings.remove(&failed.bindings[0]);
    let removed = bindings.resolve_for_host(&*host, sources, &launch);
    assert!(removed.issues.is_empty());
    let snapshot = scan(&*host, &removed.sources, &mut StatisticsCache::default()).unwrap();
    assert_eq!(
        snapshot.aggregate(&DateWindow::all("UTC")).main_sessions(),
        0
    );
}

#[test]
fn statistics_source_persistence_never_replays_launch_material_or_unknown_environment() {
    let host = LocalHost::new();
    let binding = serde_json::json!({
        "host": host.id().0, "agent":"opencode", "selector":"http://127.0.0.1:43219/", "scope":"selected-service",
        "native_scope":{"environment":{},"opencode":{"connection_argv":["--server","http://127.0.0.1:43219/"],"launch_argv":["PRIVATE-LAUNCH"],"bridge":null}}
    });
    let restored: SourceBinding = serde_json::from_value(binding.clone()).unwrap();
    let persisted = serde_json::to_string(&restored).unwrap();
    assert!(!persisted.contains("PRIVATE-LAUNCH"));
    assert!(!persisted.contains("launch_argv"));
    let mut invalid_binding = binding;
    invalid_binding["native_scope"]["environment"] =
        serde_json::json!({"API_TOKEN":"PRIVATE-TOKEN"});
    let restored: SourceBinding = serde_json::from_value(invalid_binding).unwrap();
    assert!(serde_json::to_string(&restored).is_err());
    let temp = tempfile::tempdir().unwrap();
    assert!(
        SourceBindings::default()
            .check(&*host, &restored, &isolated_launch(temp.path()))
            .unwrap_err()
            .to_string()
            .contains("unsupported environment selector")
    );
}

#[test]
fn statistics_sources_detect_equivalent_native_service_urls_without_merging_hosts() {
    let temp = tempfile::tempdir().unwrap();
    let launch = isolated_launch(temp.path());
    let host = LocalHost::new();
    let bindings: SourceBindings = serde_json::from_value(serde_json::json!({"added":[
        {"host":host.id().0,"agent":"opencode","selector":"http://localhost/","scope":"first"},
        {"host":host.id().0,"agent":"opencode","selector":"http://LOCALHOST:80","scope":"duplicate"},
        {"host":999,"agent":"opencode","selector":"http://localhost/","scope":"foreign"}
    ]})).unwrap();
    let resolved = bindings.resolve_for_host(&*host, Vec::new(), &launch);
    assert_eq!(resolved.issues[0].binding.scope, "duplicate");
    assert!(resolved.issues[0].reason.contains("overlaps"));
    assert!(
        resolved
            .bindings
            .iter()
            .all(|binding| binding.host == host.id().0)
    );
}

#[test]
fn statistics_failed_native_rebinding_keeps_original_scope_as_incomplete_not_default_success() {
    let temp = tempfile::tempdir().unwrap();
    let host = LocalHost::new();
    let launch = isolated_launch(temp.path());
    let bindings: SourceBindings = serde_json::from_value(serde_json::json!({"added":[{
        "host":host.id().0,"agent":"opencode","selector":"http://127.0.0.1:43219/","scope":"original-service-scope",
        "cwd":temp.path(),"native_scope":{"environment":{"API_TOKEN":"not-a-valid-selector"}}
    }]})).unwrap();
    let resolved = bindings.resolve_for_host(&*host, Vec::new(), &launch);
    let snapshot = scan(&*host, &resolved.sources, &mut StatisticsCache::default()).unwrap();
    assert!(!snapshot.is_complete());
    assert!(
        snapshot
            .issues()
            .iter()
            .any(|issue| issue.reason().contains("unsupported environment selector"))
    );
    assert_eq!(snapshot.sources()[0].scope(), "original-service-scope");
    assert_eq!(snapshot.sources()[0].root(), temp.path());
    assert_eq!(
        snapshot.aggregate(&DateWindow::all("UTC")).main_sessions(),
        0
    );
}

#[test]
fn statistics_sources_reject_alias_of_a_bound_native_service_even_when_discovered_as_configured() {
    use crate::core::cli_agent::CLIAgent;
    use crate::host::session_operations::{
        SessionOperationKind,
        opencode::{OpenCodeScope, OpenCodeServer},
    };
    let temp = tempfile::tempdir().unwrap();
    let launch = isolated_launch(temp.path());
    let host = LocalHost::new();
    let source = Source::opencode(
        temp.path().into(),
        "configured-service",
        SessionOperation {
            expected_host: host.id().0,
            agent: CLIAgent::OpenCode,
            kind: SessionOperationKind::Resume,
            source_id: String::new(),
            cwd: temp.path().into(),
            launch_argv: vec![
                temp.path()
                    .join("missing/opencode")
                    .to_string_lossy()
                    .into_owned(),
            ],
            scope: SessionScope {
                opencode: Some(OpenCodeScope {
                    server: Some(OpenCodeServer {
                        pid: 123,
                        urls: vec![
                            "http://localhost:43219/".into(),
                            "http://127.0.0.1:43219/".into(),
                        ],
                    }),
                    ..Default::default()
                }),
                ..Default::default()
            },
        },
    );
    let bindings: SourceBindings = serde_json::from_value(serde_json::json!({"added":[{
        "host":host.id().0,"agent":"opencode","selector":"http://127.0.0.1:43219/","scope":"extra-alias","cwd":temp.path()
    }]})).unwrap();
    let resolved = bindings.resolve_for_host(&*host, vec![source], &launch);
    assert_eq!(resolved.issues[0].binding.scope, "extra-alias");
    assert!(resolved.issues[0].reason.contains("overlaps"));
    assert_eq!(resolved.bindings[0].selector, "http://localhost:43219/");
}

#[test]
fn statistics_checking_automatic_pi_preserves_native_workspace_store_scope() {
    let temp = tempfile::tempdir().unwrap();
    let mut launch = isolated_launch(temp.path());
    let agent = temp.path().join("native-pi");
    launch.insert(
        "pi".into(),
        format!("PI_CODING_AGENT_DIR='{}' pi", agent.display()),
    );
    let directory = agent.join("sessions/project");
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(
        directory.join("session.jsonl"),
        native_history("pi-default").replace(
            "\"totalTokens\":10",
            "\"cacheRead\":0,\"cacheWrite\":0,\"totalTokens\":10",
        ),
    )
    .unwrap();
    let host = LocalHost::new();
    let bindings = SourceBindings::default();
    let resolved =
        bindings.resolve_for_host(&*host, discover_pi(&*host, &launch).unwrap(), &launch);
    let binding = resolved
        .bindings
        .iter()
        .find(|binding| Path::new(&binding.selector).ends_with("sessions"))
        .unwrap();
    let checked = bindings.check(&*host, binding, &launch).unwrap();
    let snapshot = scan(&*host, &[checked], &mut StatisticsCache::default()).unwrap();
    assert_eq!(
        snapshot.aggregate(&DateWindow::all("UTC")).tokens().total(),
        Some(10)
    );
}

#[test]
fn statistics_sources_accept_partial_directories_but_keep_read_failures_visible() {
    let temp = tempfile::tempdir().unwrap();
    let launch = isolated_launch(temp.path());
    let directory = temp.path().join("partial");
    std::fs::create_dir(&directory).unwrap();
    std::fs::write(directory.join("good.jsonl"), native_history("readable")).unwrap();
    std::fs::write(directory.join("bad.jsonl"), "invalid history").unwrap();
    let host = LocalHost::new();
    let mut bindings = SourceBindings::default();
    let binding = bindings
        .add(&*host, "omp", directory.to_str().unwrap(), &launch)
        .unwrap();
    bindings.check(&*host, &binding, &launch).unwrap();
    let resolved = bindings.resolve_for_host(&*host, Vec::new(), &launch);
    let snapshot = scan(&*host, &resolved.sources, &mut StatisticsCache::default()).unwrap();
    assert_eq!(
        snapshot.aggregate(&DateWindow::all("UTC")).tokens().total(),
        Some(10)
    );
    assert!(!snapshot.is_complete());
    assert!(
        snapshot
            .issues()
            .iter()
            .any(|issue| issue.path().ends_with("bad.jsonl"))
    );
}

#[test]
fn statistics_sources_accept_readable_zero_usage_forks_without_claiming_complete_lineage() {
    let temp = tempfile::tempdir().unwrap();
    let launch = isolated_launch(temp.path());
    let path = temp.path().join("fork.jsonl");
    std::fs::write(&path, "{\"type\":\"session\",\"version\":3,\"id\":\"fork\",\"parentSession\":\"missing-parent\",\"timestamp\":\"2026-10-01T00:00:00Z\"}\n").unwrap();
    let host = LocalHost::new();
    let mut bindings = SourceBindings::default();
    bindings
        .add(&*host, "omp", path.to_str().unwrap(), &launch)
        .unwrap();
    let resolved = bindings.resolve_for_host(&*host, Vec::new(), &launch);
    let snapshot = scan(&*host, &resolved.sources, &mut StatisticsCache::default()).unwrap();
    assert!(!snapshot.is_complete());
    assert!(snapshot.issues().iter().any(|issue| {
        issue
            .reason()
            .contains("parentSession lineage is unavailable")
    }));
}
