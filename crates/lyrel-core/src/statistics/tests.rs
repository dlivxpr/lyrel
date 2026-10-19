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
    let analysis = snapshot.analyze(&DateWindow::all("UTC"), chrono::Utc::now(), None, 2026).unwrap();
    assert_eq!(analysis.totals().tokens().total(), Some(66));
    assert_eq!(analysis.totals().main_sessions(), 2);
    assert_eq!(analysis.models().iter().map(|m| m.totals().tokens().total().unwrap()).sum::<u64>(), 66);
    assert_eq!(analysis.projects().iter().map(|p| p.totals().tokens().total().unwrap()).sum::<u64>(), 66);
    assert_eq!(analysis.models()[0].totals().main_sessions(), 2);
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
fn statistics_analysis_reconciles_calendar_copies_models_and_unknown_projects() {
    use chrono::{NaiveDate, TimeZone};
    let tmp = tempfile::tempdir().unwrap();
    let sample = concat!(
        "{\"type\":\"session\",\"version\":3,\"id\":\"cross-day\",\"timestamp\":\"2026-03-08T04:30:00Z\"}\n",
        "{\"type\":\"model_usage\",\"id\":\"a\",\"timestamp\":\"2026-03-08T04:30:00Z\",\"provider\":\"one\",\"model\":\"shared\",\"usage\":{\"totalTokens\":10,\"cost\":{\"total\":0}}}\n",
        "{\"type\":\"model_usage\",\"id\":\"b\",\"timestamp\":\"2026-03-08T07:30:00Z\",\"provider\":\"two\",\"model\":\"shared\",\"usage\":{\"totalTokens\":20}}\n"
    );
    std::fs::write(tmp.path().join("a.jsonl"), sample).unwrap();
    std::fs::write(tmp.path().join("copy.jsonl"), sample).unwrap();
    let snapshot = scan(&*LocalHost::new(), &[Source::omp(tmp.path().into(), "test")], &mut StatisticsCache::default()).unwrap();
    let now = chrono_tz::America::New_York.with_ymd_and_hms(2026, 3, 8, 12, 0, 0).unwrap();
    let range = DateWindow::all("America/New_York");
    let analysis = snapshot.analyze(&range, now, None, 2026).unwrap();
    let seventh = analysis.days().iter().find(|d| d.date() == NaiveDate::from_ymd_opt(2026, 3, 7).unwrap()).unwrap();
    let eighth = analysis.days().iter().find(|d| d.date() == NaiveDate::from_ymd_opt(2026, 3, 8).unwrap()).unwrap();
    assert_eq!(seventh.totals().tokens().total(), Some(10));
    assert_eq!(eighth.totals().tokens().total(), Some(20));
    assert_eq!(seventh.totals().main_sessions(), 1);
    assert_eq!(eighth.totals().main_sessions(), 1);
    assert_eq!(analysis.totals().main_sessions(), 1); // daily session counts are not additive
    assert_eq!(analysis.totals().tokens().total(), Some(30));
    assert_eq!(analysis.models().len(), 2); // provider is part of model identity
    assert_eq!(analysis.projects().len(), 1);
    assert!(analysis.projects()[0].label().is_none());
    assert_eq!(analysis.projects()[0].totals().tokens().total(), Some(30));
    assert_eq!(analysis.streaks().current(), Some(2));
    assert_eq!(analysis.streaks().longest(), Some(2));
    assert_eq!(eighth.value(analysis::Metric::Cost), None); // unknown is not zero cost
    assert_eq!(analysis.totals().tokens().total(), snapshot.aggregate(&range).tokens().total());
    let today = DateWindow::calendar(Period::Today, now, None).unwrap();
    let filtered = snapshot.analyze(&today, now, Some("omp"), 2026).unwrap();
    assert!(!filtered.days().iter().find(|d| d.date() == seventh.date()).unwrap().is_in_range());
    assert_eq!(filtered.totals().tokens().total(), Some(20));
    assert_eq!(snapshot.analyze(&range, now, Some("not-registered"), 2026).unwrap().totals().calls(), 0);
}

#[test]
fn statistics_analysis_never_spreads_cumulative_or_estimated_cost_across_days() {
    use chrono::TimeZone;
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(temp.path().join("native.jsonl"), header("interval", None) + &usage("u", 10, None)).unwrap();
    let mut snapshot = scan(&*LocalHost::new(), &[Source::omp(temp.path().into(), "profile")], &mut StatisticsCache::default()).unwrap();
    let record = &mut snapshot.files[0].session.records[0];
    record.coverage_start = Some(record.timestamp - 86_400);
    record.estimated_usd = Some(2.5);
    let now = chrono::Utc.with_ymd_and_hms(2026, 3, 8, 12, 0, 0).unwrap();
    let all = DateWindow::all("UTC");
    let analysis = snapshot.analyze(&all, now, None, 2026).unwrap();
    assert_eq!(analysis.totals().tokens().total(), Some(12));
    assert_eq!(analysis.totals().estimated_usd(), Some(2.5));
    assert_eq!(analysis.totals().recorded_usd(), None);
    assert_eq!(analysis.totals().unpriced_calls(), 0);
    assert_eq!(analysis.streaks().longest(), None);
    assert!(analysis.days().iter().filter(|d| d.date().to_string() == "2026-03-07" || d.date().to_string() == "2026-03-08").all(|d| d.value(analysis::Metric::Tokens).is_none()));
    let today = DateWindow::calendar(Period::Today, now, None).unwrap();
    let partial = snapshot.analyze(&today, now, None, 2026).unwrap();
    assert!(!partial.totals().is_complete());
    assert_eq!(partial.totals().calls(), 0);
    assert_eq!(partial.models().len(), 1);
    assert!(!partial.models()[0].totals().is_complete());
    assert!(!snapshot.aggregate(&today).is_complete());
    assert_eq!(snapshot.aggregate(&today).calls(), 0);
}

#[test]
fn statistics_analysis_isolates_agents_and_preserves_conflicting_native_records() {
    use chrono::TimeZone;
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(temp.path().join("native.jsonl"), header("same", None) + &usage("u", 10, Some(0.))).unwrap();
    let mut snapshot = scan(&*LocalHost::new(), &[Source::omp(temp.path().into(), "same-scope")], &mut StatisticsCache::default()).unwrap();
    // Serialized Snapshot is the production Host transport seam. Distinct registered
    // agents may supply identical native IDs; neither may erase the other's usage.
    let mut other = snapshot.files[0].clone();
    other.source.agent = "another-native-agent".into();
    snapshot.sources.push(other.source.clone());
    snapshot.files.push(other);
    let now = chrono::Utc.with_ymd_and_hms(2026, 3, 8, 12, 0, 0).unwrap();
    let all = DateWindow::all("UTC");
    assert_eq!(snapshot.aggregate(&all).tokens().total(), Some(24));
    assert_eq!(snapshot.aggregate(&all).main_sessions(), 2);
    let analysis = snapshot.analyze(&all, now, Some("omp"), 2026).unwrap();
    assert_eq!(analysis.totals().tokens().total(), Some(12));
    assert_eq!(analysis.agents().len(), 1);
    let mut collision = snapshot.files[0].clone();
    collision.path = temp.path().join("collision.jsonl");
    collision.session.records[0].tokens.total = Some(22);
    snapshot.files.push(collision);
    let analysis = snapshot.analyze(&all, now, Some("omp"), 2026).unwrap();
    assert_eq!(analysis.totals().tokens().total(), Some(34));
    assert!(!analysis.totals().is_complete());
    assert_eq!(analysis.streaks().longest(), None);
    assert_eq!(analysis.maximum(analysis::Metric::Tokens), 0.);
}
