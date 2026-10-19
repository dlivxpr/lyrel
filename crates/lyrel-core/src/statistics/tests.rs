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
