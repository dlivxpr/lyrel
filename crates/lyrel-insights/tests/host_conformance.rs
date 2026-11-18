//! Insights-owned `Host` conformance cases: statistics and skills workflows
//! that used to live in `lyrel-core`'s shared suite.
//!
//! Every case runs against a real [`LocalHost`] and — when
//! `LYREL_TEST_LYREL_SERVER` points at a built `lyrel-server` binary — against
//! a real `lyrel-server --stdio` child through [`RemoteHost`], the same
//! factories the GUI drives. Without the env var the stdio half skips loudly
//! instead of pretending it ran; protocol-shape coverage over a synthetic
//! TCP peer lives in `remote_statistics.rs`, which always runs.
//!
//! The completeness guard below is the same contract core's suite keeps: a
//! case defined in this file must be registered in `CASES`, and the count
//! assertion stops the suite from silently shrinking.

use std::io;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};

use lyrel_core::daemon::control::{ControlHello, LinkShutdown};
use lyrel_core::host::SharedHost;
use lyrel_core::host::local::LocalHost;
use lyrel_core::host::remote::RemoteHost;
use lyrel_insights::skills;

pub type Case = fn(h: &dyn lyrel_core::host::Host, sandbox: &Path);

/// Name and function for every insights-owned conformance case. The
/// `every_case_is_registered` guard compares this table against the `pub fn`
/// definitions in this file, exactly like core's `for_each_host_case!` table.
pub const CASES: &[(&str, Case)] = &[
    ("statistics_native_overview", statistics_native_overview),
    (
        "statistics_kimi_native_overview",
        statistics_kimi_native_overview,
    ),
    (
        "skills_browse_is_readonly_and_host_bound",
        skills_browse_is_readonly_and_host_bound,
    ),
];

fn write(h: &dyn lyrel_core::host::Host, p: &Path, body: &str) {
    h.write_file(p, body.as_bytes())
        .unwrap_or_else(|e| panic!("write {}: {e}", p.display()));
}

fn mkdir(h: &dyn lyrel_core::host::Host, p: &Path) {
    h.create_dir(p, true)
        .unwrap_or_else(|e| panic!("mkdir {}: {e}", p.display()));
}

pub fn statistics_native_overview(h: &dyn lyrel_core::host::Host, sandbox: &Path) {
    use lyrel_insights::statistics::{DateWindow, StatisticsCache, discover_omp, scan};
    let home = sandbox;
    let config = h.join(home, "omp-config");
    let agent = h.join(&h.join(&h.join(&config, "profiles"), "work"), "agent");
    let sessions = h.join(&agent, "sessions");
    h.create_dir(&sessions, true).unwrap();
    let native = b"{\"type\":\"session\",\"version\":3,\"id\":\"stdio-native\",\"timestamp\":\"2026-03-08T08:00:00Z\",\"cwd\":\"/work\"}\n{\"type\":\"model_usage\",\"id\":\"usage\",\"timestamp\":\"2026-03-08T09:00:00Z\",\"provider\":\"anthropic\",\"model\":\"test\",\"usage\":{\"input\":12,\"output\":3,\"cacheRead\":0,\"cacheWrite\":0,\"totalTokens\":15,\"cost\":{\"total\":0}}}\n";
    h.write_file(&h.join(&sessions, "native.jsonl"), native)
        .unwrap();
    let command = format!(
        "HOME='{}' USERPROFILE='{}' PI_CONFIG_DIR='{}' XDG_DATA_HOME='{}' XDG_STATE_HOME='{}' omp --profile work",
        home.display(),
        home.display(),
        config.display(),
        home.display(),
        home.display()
    );
    let launch = std::collections::HashMap::from([("omp".into(), command)]);
    let sources = discover_omp(h, &launch).unwrap();
    let mut cache = StatisticsCache::default();
    let snapshot = scan(h, &sources, &mut cache).unwrap();
    let totals = snapshot.aggregate(&DateWindow::all("UTC"));
    assert_eq!(totals.tokens().total(), Some(15));
    assert_eq!(totals.recorded_usd(), Some(0.));
    assert_eq!(totals.main_sessions(), 1);
    assert!(totals.is_complete());
    let analysis = snapshot
        .analyze(
            &DateWindow::all("UTC"),
            chrono::Utc::now(),
            Some("omp"),
            2026,
        )
        .unwrap();
    let day = analysis
        .days()
        .iter()
        .find(|d| d.date().to_string() == "2026-03-08")
        .unwrap();
    assert_eq!(day.totals().tokens().total(), Some(15));
    assert_eq!(day.totals().main_sessions(), 1);
    assert_eq!(analysis.models()[0].label(), Some("test"));
    assert_eq!(analysis.projects()[0].label(), Some("/work"));
    assert_eq!(analysis.agents()[0].totals().recorded_usd(), Some(0.));
    // A failed refresh retains known values rather than turning history into zero.
    h.write_file(&h.join(&sessions, "native.jsonl"), b"{broken")
        .unwrap();
    let stale = scan(h, &sources, &mut cache).unwrap();
    assert!(!stale.is_complete());
    assert_eq!(
        stale.aggregate(&DateWindow::all("UTC")).tokens().total(),
        Some(15)
    );
    let analysis = stale
        .analyze(&DateWindow::all("UTC"), chrono::Utc::now(), None, 2026)
        .unwrap();
    assert_eq!(analysis.totals().tokens().total(), Some(15));
    assert_eq!(analysis.streaks().current(), None);
    assert!(analysis.days().iter().all(|d| {
        d.value(lyrel_insights::statistics::analysis::Metric::Tokens)
            .is_none()
    }));
    // pi's formal usage entries are not omp model_usage entries. The same Host transport
    // must discover and read its isolated flat store without falling back to the GUI home.
    let pi_sessions = h.join(home, "pi-private");
    h.create_dir(&pi_sessions, true).unwrap();
    h.write_file(&h.join(&pi_sessions, "pi.jsonl"), b"{\"type\":\"session\",\"version\":3,\"id\":\"stdio-native\",\"timestamp\":\"2026-03-08T08:00:00Z\",\"cwd\":\"/work\"}\n{\"type\":\"usage\",\"id\":\"usage\",\"timestamp\":\"2026-03-08T09:00:00Z\",\"kind\":\"future\",\"provider\":\"anthropic\",\"model\":\"test\",\"usage\":{\"input\":12,\"output\":3,\"cacheRead\":2,\"cacheWrite\":4,\"totalTokens\":999,\"cost\":{\"total\":0}}}\n").unwrap();
    let launch = std::collections::HashMap::from([(
        "pi".into(),
        format!(
            "HOME='{}' USERPROFILE='{}' PI_CODING_AGENT_DIR='{}' pi --session-dir '{}'",
            home.display(),
            home.display(),
            h.join(home, "pi-agent").display(),
            pi_sessions.display()
        ),
    )]);
    let sources = lyrel_insights::statistics::discover_pi(h, &launch).unwrap();
    let pi = scan(h, &sources, &mut cache).unwrap();
    assert_eq!(
        pi.aggregate(&DateWindow::all("UTC")).tokens().total(),
        Some(21)
    );
    assert_eq!(
        pi.aggregate(&DateWindow::all("UTC")).recorded_usd(),
        Some(0.)
    );
    assert_eq!(pi.aggregate(&DateWindow::all("UTC")).main_sessions(), 1);
    assert!(pi.is_complete());
}

pub fn statistics_kimi_native_overview(h: &dyn lyrel_core::host::Host, sandbox: &Path) {
    use lyrel_insights::statistics::{DateWindow, StatisticsCache, discover_kimi, scan};
    let home = h.join(sandbox, "kimi-home");
    let session = h.join(&home, "sessions/wd_work_0123456789ab/session_native");
    let main = h.join(&session, "agents/main");
    let child = h.join(&session, "agents/agent-1");
    h.create_dir(&main, true).unwrap();
    h.create_dir(&child, true).unwrap();
    h.write_file(
        &h.join(&session, "state.json"),
        br#"{"id":"session_native","cwd":"/work","createdAt":1792320000000}"#,
    )
    .unwrap();
    let metadata =
        "{\"type\":\"metadata\",\"protocol_version\":\"1.5\",\"created_at\":1792320000000}\n";
    let turn = "{\"type\":\"usage.record\",\"agentId\":\"main\",\"model\":\"kimi-code/kimi-for-coding\",\"time\":1792320001000,\"usageScope\":\"turn\",\"usage\":{\"inputOther\":10,\"output\":2,\"inputCacheRead\":3,\"inputCacheCreation\":0}}\n";
    let operation = turn.replace("\"turn\"", "\"session\"");
    h.write_file(
        &h.join(&main, "wire.jsonl"),
        (metadata.to_owned() + turn + &operation).as_bytes(),
    )
    .unwrap();
    h.write_file(
        &h.join(&child, "wire.jsonl"),
        (metadata.to_owned() + &turn.replace("\"main\"", "\"agent-1\"")).as_bytes(),
    )
    .unwrap();
    let launch = std::collections::HashMap::from([(
        "kimi".into(),
        format!("KIMI_CODE_HOME='{}' kimi", home.display()),
    )]);
    let sources = discover_kimi(h, &launch).unwrap();
    let mut cache = StatisticsCache::default();
    let snapshot = scan(h, &sources, &mut cache).unwrap();
    let totals = snapshot.aggregate(&DateWindow::all("UTC"));
    assert!(totals.is_complete(), "{:?}", snapshot.issues());
    assert_eq!(totals.tokens().total(), Some(45));
    assert_eq!(totals.main_sessions(), 1);
    assert_eq!(totals.unpriced_calls(), 3);
    assert_eq!(totals.recorded_usd(), None);
    h.write_file(&h.join(&child, "wire.jsonl"), b"truncated")
        .unwrap();
    let stale = scan(h, &sources, &mut cache).unwrap();
    assert!(!stale.is_complete());
    assert_eq!(
        stale.aggregate(&DateWindow::all("UTC")).tokens().total(),
        Some(45)
    );
}

/// The same actual-file workflow runs locally and through the real stdio server.
pub fn skills_browse_is_readonly_and_host_bound(h: &dyn lyrel_core::host::Host, sb: &Path) {
    let home = h.join(sb, "skills-home");
    let project = h.join(sb, "skills-project");
    mkdir(h, &h.join(&project, ".git"));
    let user = h.join(&h.join(&h.join(&home, ".agents"), "skills"), "review");
    let project_skill = h.join(&h.join(&h.join(&project, ".agents"), "skills"), "review");
    mkdir(h, &user);
    mkdir(h, &project_skill);
    let document = h.join(&user, "SKILL.md");
    write(
        h,
        &document,
        "---\nname: review\ndescription: Host user review\n---\n# Actual host instructions\n",
    );
    write(
        h,
        &h.join(&project_skill, "SKILL.md"),
        "---\nname: review\ndescription: Host project review\n---\n# Different installation\n",
    );
    let untouched = h.join(&home, "untouched.txt");
    write(h, &untouched, "unchanged");
    let before = h.read_file(&document, 4096).unwrap();
    let before_meta = h.stat(&document).unwrap();
    let spelling = home.to_string_lossy();
    let launches=["omp","pi","kimi","opencode","hermes"].into_iter().map(|agent|{
        let agent_dir=if agent=="omp"{h.join(&h.join(&home,".omp"),"agent")}else{h.join(&h.join(&home,".pi"),"agent")};
        (agent.to_owned(),format!("HOME='{spelling}' USERPROFILE='{spelling}' PROGRAMDATA='{spelling}/programdata' CLAUDE_CONFIG_DIR='{spelling}/.claude' XDG_DATA_HOME='{spelling}/.data' XDG_STATE_HOME='{spelling}/.state' PI_CONFIG_DIR=.omp OMP_PROFILE=default PI_PROFILE=default PI_CODING_AGENT_DIR='{}' KIMI_CODE_HOME='{spelling}/.kimi-code' HERMES_HOME='{spelling}/.hermes' XDG_CONFIG_HOME='{spelling}/.config' OPENCODE_CONFIG_DIR='{spelling}/.config/opencode' {agent}",agent_dir.display()))
    }).collect();
    let result = skills::scan(
        h,
        &skills::ScanRequest::new(&home)
            .with_project(&project)
            .with_launch_commands(launches),
    )
    .unwrap();
    assert_eq!(result.host(), h.id());
    assert_eq!(result.filtered("review", None).count(), 2);
    let user = result
        .filtered("Host user review", Some(skills::SourceKind::UserShared))
        .next()
        .unwrap();
    assert_eq!(user.document(), h.canonicalize(&document).unwrap());
    assert!(
        user.markdown()
            .unwrap()
            .contains("# Actual host instructions")
    );
    assert_eq!(h.read_file(&document, 4096).unwrap(), before);
    assert_eq!(h.stat(&document).unwrap(), before_meta);
    assert_eq!(h.read_file(&untouched, 1024).unwrap(), b"unchanged");
}

struct ServerProcess {
    child: Mutex<Option<Child>>,
}

impl LinkShutdown for ServerProcess {
    fn shutdown_link(&self) -> io::Result<()> {
        if let Some(mut child) = self.child.lock().unwrap().take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        Ok(())
    }
}

/// A real `lyrel-server --stdio` child, located through
/// `LYREL_TEST_LYREL_SERVER` (the CI workflow sets it; locally it usually is
/// absent). Returns `None` — and the caller skips — when the binary path is
/// not provided. Dropping the returned host shuts the child down through the
/// `LinkShutdown` closer.
fn stdio_host(sandbox: &Path) -> Option<SharedHost> {
    let executable = std::env::var("LYREL_TEST_LYREL_SERVER").ok()?;
    let mut child = Command::new(executable)
        .args(["--stdio", "--serve"])
        .env("LYREL_DATA_DIR", sandbox)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("could not start lyrel-server --stdio");

    let stdout = child.stdout.take().expect("piped");
    let stdin = child.stdin.take().expect("piped");
    let closer: Arc<dyn LinkShutdown> = Arc::new(ServerProcess {
        child: Mutex::new(Some(child)),
    });

    let hello = ControlHello::host_rpc("insights-conformance", "localhost");
    let host = RemoteHost::connect_with(stdout, stdin, Some(closer), "stdio:insights", &hello)
        .expect("handshake with lyrel-server --stdio");
    Some(host.into_shared())
}

fn case(name: &str) -> Case {
    CASES
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, c)| *c)
        .unwrap_or_else(|| panic!("case {name} missing from CASES"))
}

fn run_local(name: &str) {
    let sandbox = tempfile::tempdir().unwrap();
    let host = LocalHost::new();
    case(name)(&host, sandbox.path());
}

fn run_stdio(name: &str) {
    let sandbox = tempfile::tempdir().unwrap();
    let Some(host) = stdio_host(sandbox.path()) else {
        eprintln!("skip stdio_{name}: set LYREL_TEST_LYREL_SERVER to a built lyrel-server binary");
        return;
    };
    case(name)(&host, sandbox.path());
}

#[test]
fn local_statistics_native_overview() {
    run_local("statistics_native_overview");
}

#[test]
fn stdio_statistics_native_overview() {
    run_stdio("statistics_native_overview");
}

#[test]
fn local_statistics_kimi_native_overview() {
    run_local("statistics_kimi_native_overview");
}

#[test]
fn stdio_statistics_kimi_native_overview() {
    run_stdio("statistics_kimi_native_overview");
}

#[test]
fn local_skills_browse_is_readonly_and_host_bound() {
    run_local("skills_browse_is_readonly_and_host_bound");
}

#[test]
fn stdio_skills_browse_is_readonly_and_host_bound() {
    run_stdio("skills_browse_is_readonly_and_host_bound");
}

#[test]
fn every_case_is_registered() {
    let src = include_str!("host_conformance.rs");
    let defined: Vec<&str> = src
        .lines()
        .filter_map(|l| l.strip_prefix("pub fn "))
        .filter(|l| l.contains("&Path)"))
        .filter_map(|l| l.split('(').next())
        .collect();

    let registered: Vec<&str> = CASES.iter().map(|(n, _)| *n).collect();
    for name in &defined {
        assert!(
            registered.contains(name),
            "case `{name}` is defined but missing from CASES"
        );
    }
    for name in &registered {
        assert!(
            defined.contains(name),
            "case `{name}` is registered but has no `pub fn`"
        );
    }
    assert_eq!(defined.len(), registered.len(), "duplicate registration");
}

#[test]
fn the_whole_suite_has_three_cases() {
    let names: Vec<&str> = CASES.iter().map(|(n, _)| *n).collect();
    assert_eq!(
        names,
        [
            "statistics_native_overview",
            "statistics_kimi_native_overview",
            "skills_browse_is_readonly_and_host_bound",
        ],
        "the insights conformance suite changed shape: {names:?}",
    );
}
