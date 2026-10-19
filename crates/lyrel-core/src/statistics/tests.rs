use super::*;
use crate::host::local::LocalHost;
use serde_json::json;

fn header(id: &str, parent: Option<&str>) -> String {
    format!(
        "{}\n",
        json!({"type":"session","version":3,"id":id,"timestamp":"2026-03-08T07:00:00Z","cwd":"/work","parentSession":parent})
    )
}
fn usage(id: &str, input: u64, cost: Option<f64>) -> String {
    let mut u = json!({"input":input,"output":2,"cacheRead":0,"totalTokens":input+2});
    if let Some(cost) = cost {
        u["cost"] = json!({"total":cost});
    }
    format!(
        "{}\n",
        json!({"type":"message","id":id,"parentId":null,"timestamp":"2026-03-08T08:00:00Z","message":{"role":"assistant","provider":"anthropic","model":"test","usage":u,"content":[{"type":"text","text":"PRIVATE BODY"}]}})
    )
}

#[test]
fn statistics_native_copies_forks_and_children_count_only_new_usage() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::write(
        root.join("a.jsonl"),
        header("main", None) + &usage("u1", 10, Some(0.0)),
    )
    .unwrap();
    std::fs::write(
        root.join("copy.jsonl"),
        header("main", None) + &usage("u1", 10, Some(0.0)),
    )
    .unwrap();
    std::fs::write(
        root.join("fork.jsonl"),
        header("fork", Some("main")) + &usage("u1", 10, Some(0.0)) + &usage("u2", 20, None),
    )
    .unwrap();
    std::fs::create_dir(root.join("a")).unwrap();
    std::fs::write(
        root.join("a/worker.jsonl"),
        header("worker", Some("main")) + &usage("u3", 30, Some(1.5)),
    )
    .unwrap();
    let host = LocalHost::new();
    let source = Source::omp(root.to_path_buf(), "scope-a");
    let mut cache = StatisticsCache::default();
    let snapshot = scan(&*host, &[source], &mut cache).unwrap();
    let totals = snapshot.aggregate(&DateWindow::all("UTC"));
    assert_eq!(totals.tokens().total(), Some(66));
    assert_eq!(totals.tokens().cache_read(), Some(0));
    assert_eq!(totals.tokens().cache_write(), None);
    assert_eq!(totals.recorded_usd(), Some(1.5));
    assert_eq!(totals.unpriced_calls(), 1);
    assert_eq!(totals.main_sessions(), 2);
    assert!(
        !serde_json::to_string(&snapshot)
            .unwrap()
            .contains("PRIVATE BODY")
    );
    std::fs::write(root.join("fork.jsonl"), "broken").unwrap();
    let stale = scan(&*host, snapshot.sources(), &mut cache).unwrap();
    assert!(!stale.is_complete());
    assert_eq!(
        stale.aggregate(&DateWindow::all("UTC")).tokens().total(),
        Some(66)
    );
}

#[test]
fn statistics_same_native_ids_in_different_profiles_are_not_merged() {
    let tmp = tempfile::tempdir().unwrap();
    for scope in ["a", "b"] {
        std::fs::create_dir(tmp.path().join(scope)).unwrap();
        std::fs::write(
            tmp.path().join(scope).join("s.jsonl"),
            header("same", None) + &usage("u1", 10, Some(0.0)),
        )
        .unwrap();
    }
    let host = LocalHost::new();
    let sources = [
        Source::omp(tmp.path().join("a"), "profile-a"),
        Source::omp(tmp.path().join("b"), "profile-b"),
    ];
    let snapshot = scan(&*host, &sources, &mut StatisticsCache::default()).unwrap();
    let totals = snapshot.aggregate(&DateWindow::all("UTC"));
    assert_eq!(totals.tokens().total(), Some(24));
    assert_eq!(totals.main_sessions(), 2);
    assert_eq!(totals.recorded_usd(), Some(0.0));
}

#[test]
fn statistics_calendar_windows_follow_dst_and_previous_calendar_period() {
    use chrono::TimeZone;
    let zone = chrono_tz::America::New_York;
    let now = zone.with_ymd_and_hms(2026, 3, 8, 12, 0, 0).unwrap();
    let window = DateWindow::calendar(Period::Today, now, None).unwrap();
    assert_eq!(window.end().unwrap() - window.start().unwrap(), 23 * 3600);
    assert_eq!(
        window.previous().unwrap().end().unwrap() - window.previous().unwrap().start().unwrap(),
        24 * 3600
    );
    let custom = DateWindow::calendar(
        Period::Custom,
        now,
        Some((
            chrono::NaiveDate::from_ymd_opt(2026, 3, 8).unwrap(),
            chrono::NaiveDate::from_ymd_opt(2026, 3, 8).unwrap(),
        )),
    )
    .unwrap();
    assert_eq!(custom.start(), window.start());
    assert_eq!(custom.end(), window.end());
}

#[test]
fn statistics_week_compares_same_weekdays_not_adjacent_days() {
    use chrono::TimeZone;
    let now = chrono::Utc
        .with_ymd_and_hms(2026, 10, 21, 12, 0, 0)
        .unwrap();
    let window = DateWindow::calendar(Period::Week, now, None).unwrap();
    let previous = window.previous().unwrap();
    assert_eq!(
        previous.start(),
        Some(
            chrono::Utc
                .with_ymd_and_hms(2026, 10, 12, 0, 0, 0)
                .unwrap()
                .timestamp()
        )
    );
    assert_eq!(
        previous.end(),
        Some(
            chrono::Utc
                .with_ymd_and_hms(2026, 10, 15, 0, 0, 0)
                .unwrap()
                .timestamp()
        )
    );
}

#[test]
fn statistics_discovers_registered_custom_files_without_reading_another_profile() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("agent");
    std::fs::create_dir_all(root.join("custom-session-files")).unwrap();
    let custom = temp.path().join("custom.jsonl");
    std::fs::write(
        &custom,
        header("external", None) + &usage("u", 40, Some(0.25)),
    )
    .unwrap();
    std::fs::write(
        root.join("custom-session-files/marker"),
        custom.to_string_lossy().as_bytes(),
    )
    .unwrap();
    let launch = HashMap::from([(
        "omp".into(),
        format!(
            "OMP_PROFILE=default PI_CODING_AGENT_DIR='{}' omp",
            root.display()
        ),
    )]);
    let host = LocalHost::new();
    let sources = discover_omp(&*host, &launch).unwrap();
    let snapshot = scan(&*host, &sources, &mut StatisticsCache::default()).unwrap();
    let totals = snapshot.aggregate(&DateWindow::all("UTC"));
    assert_eq!(totals.tokens().total(), Some(42));
    assert_eq!(totals.recorded_usd(), Some(0.25));
    assert_eq!(totals.main_sessions(), 1);
}

#[test]
fn statistics_discovers_explicit_native_session_directory_without_registry_markers() {
    let temp = tempfile::tempdir().unwrap();
    let custom = temp.path().join("custom");
    std::fs::create_dir(&custom).unwrap();
    std::fs::write(
        custom.join("external.jsonl"),
        header("explicit", None) + &usage("u", 50, Some(0.5)),
    )
    .unwrap();
    let launch = HashMap::from([(
        "omp".into(),
        format!(
            "OMP_PROFILE=default PI_CODING_AGENT_DIR='{}' omp --session-dir '{}'",
            temp.path().join("agent").display(),
            custom.display()
        ),
    )]);
    let host = LocalHost::new();
    let sources = discover_omp(&*host, &launch).unwrap();
    let snapshot = scan(&*host, &sources, &mut StatisticsCache::default()).unwrap();
    let totals = snapshot.aggregate(&DateWindow::all("UTC"));
    assert_eq!(totals.tokens().total(), Some(52));
    assert_eq!(totals.recorded_usd(), Some(0.5));
    assert_eq!(totals.main_sessions(), 1);
}

#[test]
fn statistics_pi_counts_official_full_session_usage_without_omp_fields() {
    let temp = tempfile::tempdir().unwrap();
    let mut data = header("pi-native", None);
    // pi v1.3.0 session-format.md UsageEntry, including its published literal totals.
    data.push_str(r#"{"type":"usage","id":"f6g7h8i9","parentId":"e5f6g7h8","timestamp":"2024-12-03T14:08:00.000Z","kind":"cache_warm","provider":"anthropic","model":"claude-sonnet-4-5","usage":{"input":0,"output":0,"cacheRead":50000,"cacheWrite":0,"totalTokens":50000,"cost":{"input":0,"output":0,"cacheRead":0.015,"cacheWrite":0,"total":0.015}}}
"#);
    for (id, kind, role, cost) in [
        ("assistant", "message", "assistant", Some(0.0)),
        ("tool", "message", "toolResult", None),
        ("compact", "compaction", "", Some(0.25)),
        ("branch", "branch_summary", "", Some(0.25)),
        ("future", "usage", "", Some(0.0)),
    ] {
        let mut u = json!({"input":10,"output":2,"cacheRead":3,"cacheWrite":4,"reasoning":1,"cacheWrite1h":2,"totalTokens":999});
        if let Some(cost) = cost {
            u["cost"] = json!({"total":cost});
        }
        let mut entry = json!({"type":kind,"id":id,"timestamp":"2026-03-08T08:00:00Z","usage":u,"kind":"future-operation"});
        if kind == "message" {
            entry["message"] = json!({"role":role,"provider":"provider","model":"logical","responseModel":"physical","stopReason":"error","usage":entry["usage"].clone()});
        }
        data.push_str(&format!("{entry}\n"));
    }
    std::fs::write(temp.path().join("native.jsonl"), data).unwrap();
    let host = LocalHost::new();
    let snapshot = scan(
        &*host,
        &[Source::pi(temp.path().into(), "pi-scope")],
        &mut StatisticsCache::default(),
    )
    .unwrap();
    let totals = snapshot.aggregate(&DateWindow::all("UTC"));
    assert_eq!(totals.calls(), 6);
    assert_eq!(totals.tokens().total(), Some(50_095));
    assert_eq!(totals.tokens().output(), Some(10));
    assert_eq!(totals.tokens().cache_write(), Some(20));
    assert_eq!(totals.recorded_usd(), Some(0.515));
    assert_eq!(totals.unpriced_calls(), 1);
    assert_eq!(totals.main_sessions(), 1);
}

#[test]
fn statistics_pi_forks_preserve_new_usage_and_missing_parents_are_restricted() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let parent = root.join("parent.jsonl");
    let inherited = usage("same", 10, Some(0.0));
    std::fs::write(&parent, header("parent", None) + &inherited).unwrap();
    std::fs::write(
        root.join("fork.jsonl"),
        header("fork", Some(&parent.to_string_lossy())) + &inherited + &usage("new", 20, Some(1.0)),
    )
    .unwrap();
    let host = LocalHost::new();
    let source = Source::pi(root.into(), "pi-scope");
    let mut cache = StatisticsCache::default();
    let snapshot = scan(&*host, &[source.clone()], &mut cache).unwrap();
    assert_eq!(snapshot.aggregate(&DateWindow::all("UTC")).calls(), 2);
    assert_eq!(
        snapshot.aggregate(&DateWindow::all("UTC")).recorded_usd(),
        Some(1.0)
    );
    std::fs::remove_file(parent).unwrap();
    let missing = scan(&*host, &[source], &mut cache).unwrap();
    assert!(!missing.is_complete());
    assert_eq!(missing.aggregate(&DateWindow::all("UTC")).calls(), 0);
    assert!(
        missing
            .issues()
            .iter()
            .any(|i| i.reason().contains("2 usage records excluded"))
    );
}

#[test]
fn statistics_pi_custom_store_precedence_and_scope_isolation() {
    let temp = tempfile::tempdir().unwrap();
    let agent = temp.path().join("agent");
    let global = temp.path().join("global");
    let environment = temp.path().join("environment");
    let explicit = temp.path().join("explicit");
    for dir in [&agent, &global, &environment, &explicit] {
        std::fs::create_dir_all(dir).unwrap();
    }
    std::fs::write(
        agent.join("settings.json"),
        json!({"sessionDir":global}).to_string(),
    )
    .unwrap();
    for dir in [&global, &environment, &explicit] {
        std::fs::write(
            dir.join("a.jsonl"),
            header("same", None) + &usage("u", 10, Some(0.0)),
        )
        .unwrap();
    }
    let host = LocalHost::new();
    let launch = HashMap::from([(
        "pi".into(),
        format!(
            "PI_CODING_AGENT_DIR='{}' PI_CODING_AGENT_SESSION_DIR='{}' pi --session-dir '{}'",
            agent.display(),
            environment.display(),
            explicit.display()
        ),
    )]);
    let sources = discover_pi(&*host, &launch).unwrap();
    assert!(
        sources
            .iter()
            .any(|s| s.root() == host.canonicalize(&explicit).unwrap())
    );
    assert!(
        !sources
            .iter()
            .any(|s| s.root() == global || s.root() == environment)
    );
    let snapshot = scan(
        &*host,
        &[
            Source::pi(global, "same"),
            Source::pi(environment, "other"),
            Source::omp(explicit, "same"),
        ],
        &mut StatisticsCache::default(),
    )
    .unwrap();
    assert_eq!(snapshot.aggregate(&DateWindow::all("UTC")).calls(), 3);
}

#[test]
fn statistics_pi_legacy_and_missing_fields_remain_unknown_not_free() {
    let temp = tempfile::tempdir().unwrap();
    // Versioned upstream migration.test.ts sample: no entry IDs and no recorded cost.
    std::fs::write(temp.path().join("legacy.jsonl"), r#"{"type":"session","id":"sess-1","timestamp":"2025-01-01T00:00:00Z","cwd":"/tmp"}
{"type":"message","timestamp":"2025-01-01T00:00:02Z","message":{"role":"assistant","content":[{"type":"text","text":"hello"}],"api":"test","provider":"test","model":"test","usage":{"input":1,"output":1,"cacheRead":0,"cacheWrite":0},"stopReason":"stop","timestamp":2}}
"#).unwrap();
    let host = LocalHost::new();
    let snapshot = scan(
        &*host,
        &[Source::pi(temp.path().into(), "legacy")],
        &mut StatisticsCache::default(),
    )
    .unwrap();
    let totals = snapshot.aggregate(&DateWindow::all("UTC"));
    assert_eq!(totals.tokens().total(), Some(2));
    assert_eq!(totals.recorded_usd(), None);
    assert_eq!(totals.unpriced_calls(), 1);
}

#[test]
#[ignore = "requires official pi 1.3.0 package dist/core in LYREL_PI_NATIVE_CORE and Node 22.19+"]
fn statistics_pi_official_sdk_fork_clone_and_usage_oracle() {
    let temp = tempfile::tempdir().unwrap();
    let core = std::env::var("LYREL_PI_NATIVE_CORE")
        .expect("isolated official pi core directory required");
    let script = r#"
import { pathToFileURL } from 'node:url';
import { join } from 'node:path';
import { readFileSync } from 'node:fs';
const core = process.argv[1], store = process.argv[2];
const metadata = JSON.parse(readFileSync(join(core, '../../package.json'), 'utf8'));
if (metadata.name !== '@earendil-works/pi-coding-agent' || metadata.version !== '1.3.0') throw Error('official pi 1.3.0 required');
const { SessionManager } = await import(pathToFileURL(join(core, 'session-manager.js')));
const { getUsageCostBreakdown } = await import(pathToFileURL(join(core, 'usage-totals.js')));
const usage = n => ({input:n,output:2,cacheRead:3,cacheWrite:4,reasoning:1,cacheWrite1h:2,totalTokens:999,cost:{input:0,output:0,cacheRead:0,cacheWrite:0,total:n/100}});
const total = s => getUsageCostBreakdown(s.getEntries()).reduce((a,b) => ({tokens:a.tokens+b.tokens,cost:a.cost+b.cost}), {tokens:0,cost:0});
const source = SessionManager.create(store, store);
const user = source.appendMessage({role:'user',content:'isolated official persistence probe',timestamp:Date.now()});
source.appendMessage({role:'assistant',content:[{type:'text',text:'not retained'}],api:'test',provider:'test',model:'logical',responseModel:'physical',usage:usage(10),stopReason:'error',timestamp:Date.now()});
source.appendMessage({role:'toolResult',toolName:'nested',toolCallId:'call',content:[],isError:false,usage:usage(11),timestamp:Date.now()});
source.appendUsage('future-operation','test','cache',usage(12));
source.appendCompaction('not retained',user,100,undefined,false,usage(13));
source.branchWithSummary(user,'not retained',undefined,false,usage(14));
source.appendMessage({role:'assistant',content:[],api:'test',provider:'test',model:'second',usage:usage(15),stopReason:'aborted',timestamp:Date.now()});
const parentFile = source.getSessionFile(), baseline = total(source);
const fork = SessionManager.forkFrom(parentFile,store,store);
const forkBefore = total(fork);
fork.appendUsage('new-fork','test','fork',usage(17));
const forkAfter = total(fork);
const clone = SessionManager.open(parentFile,store);
clone.createBranchedSession(clone.getLeafId());
const cloneBefore = total(clone);
clone.appendUsage('new-clone','test','clone',usage(19));
const cloneAfter = total(clone);
console.log(JSON.stringify({tokens:baseline.tokens+forkAfter.tokens-forkBefore.tokens+cloneAfter.tokens-cloneBefore.tokens,cost:baseline.cost+forkAfter.cost-forkBefore.cost+cloneAfter.cost-cloneBefore.cost}));
"#;
    let output = std::process::Command::new("node")
        .args(["--input-type=module", "--eval", script, &core])
        .arg(temp.path())
        .env("PI_CODING_AGENT_DIR", temp.path().join("config"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let oracle: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let host = LocalHost::new();
    let snapshot = scan(
        &*host,
        &[Source::pi(temp.path().into(), "native-official")],
        &mut StatisticsCache::default(),
    )
    .unwrap();
    assert!(snapshot.is_complete(), "{:?}", snapshot.issues());
    let totals = snapshot.aggregate(&DateWindow::all("UTC"));
    assert_eq!(totals.tokens().total(), oracle["tokens"].as_u64());
    assert!((totals.recorded_usd().unwrap() - oracle["cost"].as_f64().unwrap()).abs() < 1e-9);
    assert_eq!(totals.main_sessions(), 3);
    assert!(
        !serde_json::to_string(&snapshot)
            .unwrap()
            .contains("not retained")
    );
}

#[test]
#[ignore = "requires isolated SSH server argv, native pi store and oracle in LYREL_PI_SSH_* (no production home)"]
fn statistics_pi_real_ssh_host_reads_official_persisted_history() {
    use crate::daemon::control::{ControlHello, LinkShutdown};
    use crate::host::remote::RemoteHost;
    use std::process::{Command, Stdio};
    use std::sync::{Arc, Mutex};
    struct Process(Mutex<Option<std::process::Child>>);
    impl LinkShutdown for Process {
        fn shutdown_link(&self) -> io::Result<()> {
            if let Some(mut child) = self.0.lock().unwrap().take() {
                let _ = child.kill();
                let _ = child.wait();
            }
            Ok(())
        }
    }
    let argv: Vec<String> = serde_json::from_str(
        &std::env::var("LYREL_PI_SSH_ARGV").expect("isolated SSH argv required"),
    )
    .unwrap();
    let store = std::env::var("LYREL_PI_SSH_STORE").expect("isolated native pi store required");
    let oracle: serde_json::Value = serde_json::from_str(
        &std::env::var("LYREL_PI_SSH_EXPECTED").expect("official usage-totals oracle required"),
    )
    .unwrap();
    let mut child = Command::new(&argv[0])
        .args(&argv[1..])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let stdin = child.stdin.take().unwrap();
    let closer: Arc<dyn LinkShutdown> = Arc::new(Process(Mutex::new(Some(child))));
    let host = RemoteHost::connect_with(
        stdout,
        stdin,
        Some(closer),
        "pi-isolated-ssh",
        &ControlHello::host_rpc("pi-statistics-smoke", "isolated-ssh"),
    )
    .unwrap()
    .into_shared();
    let launch = HashMap::from([(
        "pi".into(),
        format!(
            "HOME='{store}' USERPROFILE='{store}' PI_CODING_AGENT_DIR='{store}/config' pi --session-dir '{store}'"
        ),
    )]);
    let sources = discover_pi(&*host, &launch).unwrap();
    let snapshot = scan(&*host, &sources, &mut StatisticsCache::default()).unwrap();
    assert!(snapshot.is_complete(), "{:?}", snapshot.issues());
    let totals = snapshot.aggregate(&DateWindow::all("desktop-calendar"));
    assert_eq!(totals.tokens().total(), oracle["tokens"].as_u64());
    assert!((totals.recorded_usd().unwrap() - oracle["cost"].as_f64().unwrap()).abs() < 1e-9);
    assert_eq!(totals.main_sessions(), 3);
}

#[test]
fn statistics_pi_default_project_layout_is_not_an_omp_child_layout() {
    let temp = tempfile::tempdir().unwrap();
    let agent = temp.path().join("agent");
    let project = agent.join("sessions/--work--");
    std::fs::create_dir_all(project.join("artifact")).unwrap();
    std::fs::write(
        project.join("session.jsonl"),
        header("main", None) + &usage("entry", 10, Some(0.0)),
    )
    .unwrap();
    // Arbitrary extension/artifact output is not a native pi project store or a child identity.
    std::fs::write(project.join("artifact/not-a-session.jsonl"), "unrelated").unwrap();
    let launch = HashMap::from([(
        "pi".into(),
        format!(
            "PI_CODING_AGENT_DIR='{}' PI_CODING_AGENT_SESSION_DIR='' pi",
            agent.display()
        ),
    )]);
    let host = LocalHost::new();
    let sources = discover_pi(&*host, &launch).unwrap();
    let snapshot = scan(&*host, &sources, &mut StatisticsCache::default()).unwrap();
    assert!(snapshot.is_complete(), "{:?}", snapshot.issues());
    assert_eq!(snapshot.aggregate(&DateWindow::all("UTC")).calls(), 1);
    assert_eq!(
        snapshot.aggregate(&DateWindow::all("UTC")).main_sessions(),
        1
    );
}

#[test]
fn statistics_pi_identity_collision_does_not_drop_new_fork_usage() {
    let temp = tempfile::tempdir().unwrap();
    let parent = temp.path().join("parent.jsonl");
    std::fs::write(
        &parent,
        header("parent", None) + &usage("short-id", 10, Some(1.0)),
    )
    .unwrap();
    std::fs::write(
        temp.path().join("fork.jsonl"),
        header("fork", Some(&parent.to_string_lossy())) + &usage("short-id", 20, Some(2.0)),
    )
    .unwrap();
    let host = LocalHost::new();
    let snapshot = scan(
        &*host,
        &[Source::pi(temp.path().into(), "pi")],
        &mut StatisticsCache::default(),
    )
    .unwrap();
    assert!(!snapshot.is_complete());
    assert_eq!(snapshot.aggregate(&DateWindow::all("UTC")).calls(), 2);
    assert_eq!(
        snapshot.aggregate(&DateWindow::all("UTC")).recorded_usd(),
        Some(3.0)
    );
    assert!(
        snapshot
            .issues()
            .iter()
            .any(|i| i.reason().contains("identity conflict"))
    );
}

#[test]
fn statistics_pi_duplicate_identity_variants_are_counted_once_each() {
    let temp = tempfile::tempdir().unwrap();
    let first = header("same-session", None) + &usage("same-entry", 10, Some(0.0));
    let second = first.replace("PRIVATE BODY", "DIFFERENT PRIVATE BODY");
    for (name, bytes) in [
        ("a.jsonl", &first),
        ("b.jsonl", &second),
        ("c.jsonl", &first),
    ] {
        std::fs::write(temp.path().join(name), bytes).unwrap();
    }
    let host = LocalHost::new();
    let snapshot = scan(
        &*host,
        &[Source::pi(temp.path().into(), "pi")],
        &mut StatisticsCache::default(),
    )
    .unwrap();
    let totals = snapshot.aggregate(&DateWindow::all("UTC"));
    assert_eq!(totals.calls(), 2);
    assert_eq!(totals.main_sessions(), 1);
    assert!(!totals.is_complete());
    assert!(
        snapshot
            .issues()
            .iter()
            .any(|i| i.reason().contains("identity conflict"))
    );
    assert!(
        !serde_json::to_string(&snapshot)
            .unwrap()
            .contains("PRIVATE BODY")
    );
}
