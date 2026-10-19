use lyrel_core::host::local::LocalHost;
use lyrel_core::statistics::{DateWindow, Snapshot, Source, StatisticsCache, scan};
use lyrel_core::statistics::pricing::{Pricing, UnitPrice};
use serde_json::json;

fn price(currency: &str, input: Option<f64>, output: Option<f64>) -> UnitPrice {
    UnitPrice {
        provider: "provider".into(),
        model: "model".into(),
        currency: currency.into(),
        input,
        output,
        ..UnitPrice::default()
    }
}

fn snapshot(records: &[serde_json::Value]) -> Snapshot {
    let directory = tempfile::tempdir().unwrap();
    let mut lines = vec![json!({"type":"session","version":3,"id":"main","timestamp":"2026-03-08T07:00:00Z","cwd":"/work"}).to_string()];
    for (index, usage) in records.iter().enumerate() {
        lines.push(json!({"type":"message","id":format!("u{index}"),"timestamp":"2026-03-08T08:00:00Z","message":{"role":"assistant","provider":"provider","model":"model","usage":usage}}).to_string());
    }
    std::fs::write(directory.path().join("history.jsonl"), lines.join("\n")).unwrap();
    let host = LocalHost::new();
    scan(&*host, &[Source::omp(directory.path().to_owned(), "isolated")], &mut StatisticsCache::default()).unwrap()
}

#[test]
fn statistics_pricing_recorded_zero_wins_and_custom_prices_only_missing_cost() {
    let snapshot = snapshot(&[
        json!({"input":1_000_000,"output":0,"totalTokens":1_000_000,"cost":{"total":0}}),
        json!({"input":2_000_000,"output":0,"totalTokens":2_000_000}),
    ]);
    let mut pricing = Pricing::default();
    pricing.upsert_custom(price("USD", Some(3.0), None)).unwrap();
    let totals = snapshot.with_pricing(pricing).aggregate(&DateWindow::all("UTC"));
    assert_eq!(totals.recorded_usd(), Some(0.0));
    assert_eq!(totals.estimated_usd(), None);
    assert_eq!(totals.estimated_costs()["USD"].custom, Some(6.0));
    assert_eq!(totals.unpriced_calls(), 0);
    assert_eq!(totals.cost_in("USD"), Some(6.0));
}

fn catalog() -> Vec<u8> {
    serde_json::to_vec(&json!({
        "provider": {"id":"provider", "models": {
            "model": {"id":"model", "canonical_model_id":"other/canonical", "cost":{"input":7,"output":11,"cache_read":2}},
            "no-price": {"id":"no-price"}
        }}
    })).unwrap()
}

#[test]
fn statistics_pricing_catalog_is_opt_in_and_failed_refresh_retains_timestamped_prices() {
    let mut pricing = Pricing::default();
    assert!(pricing.update_catalog(&catalog(), 100).is_err());
    assert_eq!(pricing.fetched_at, None);
    pricing.public_enabled = true;
    pricing.update_catalog(&catalog(), 101).unwrap();
    assert_eq!(pricing.checked_at, Some(101));
    assert_eq!(pricing.fetched_at, Some(101));
    assert_eq!(pricing.catalog[0].currency, "USD");
    assert_eq!(pricing.catalog[0].reasoning, None);
    pricing.upsert_custom(price("USD", Some(3.0), None)).unwrap();
    let record = json!({"input":1_000_000,"output":2_000_000,"totalTokens":3_000_000});
    let totals = snapshot(&[record.clone()]).with_pricing(pricing.clone()).aggregate(&DateWindow::all("UTC"));
    assert_eq!(totals.estimated_costs()["USD"].custom, Some(3.0));
    assert_eq!(totals.estimated_costs()["USD"].catalog, Some(22.0));
    assert_eq!(totals.cost_in("USD"), Some(25.0));
    pricing.update_catalog(br#"{"provider":{"id":"provider","models":{"model":{"id":"model","cost":{"input":-1}}}}}"#, 102).unwrap_err();
    assert_eq!(pricing.checked_at, Some(102));
    assert_eq!(pricing.fetched_at, Some(101));
    assert!(pricing.last_error.is_some());
    pricing.catalog_failed(103, "offline");
    assert_eq!(pricing.last_error.as_deref(), Some("offline"));
    let restored: Pricing = serde_json::from_slice(&serde_json::to_vec(&pricing).unwrap()).unwrap();
    assert_eq!(snapshot(&[record.clone()]).with_pricing(restored).aggregate(&DateWindow::all("UTC")).cost_in("USD"), Some(25.0));
    pricing.public_enabled = false;
    let totals = snapshot(&[record]).with_pricing(pricing).aggregate(&DateWindow::all("UTC"));
    assert_eq!(totals.estimated_costs()["USD"].custom, Some(3.0));
    assert_eq!(totals.estimated_costs()["USD"].catalog, None);
    assert_eq!(totals.unpriced_calls(), 1);
}

#[test]
fn statistics_pricing_missing_categories_and_unknown_tokens_remain_unpriced() {
    let mut pricing = Pricing::default();
    pricing.upsert_custom(price("USD", Some(2.0), Some(9.0))).unwrap();
    let source = snapshot(&[json!({"input":1_000_000,"output":0,"cacheRead":2_000_000,"cacheWrite":3_000_000,"totalTokens":6_000_000})]);
    let totals = source.with_pricing(pricing.clone()).aggregate(&DateWindow::all("UTC"));
    let usd = &totals.estimated_costs()["USD"];
    assert_eq!(usd.custom, Some(2.0));
    assert_eq!(usd.priced_tokens, 1_000_000);
    assert_eq!(usd.unpriced_tokens, 5_000_000);
    assert_eq!(usd.partial_calls, 1);
    assert_eq!(totals.cost_in("USD"), None);
    let totals = snapshot(&[json!({"input":1_000_000,"output":0,"totalTokens":4_000_000})])
        .with_pricing(pricing).aggregate(&DateWindow::all("UTC"));
    assert_eq!(totals.estimated_costs()["USD"].custom, Some(2.0));
    assert!(totals.estimated_costs()["USD"].unknown_categories);
    assert_eq!(totals.estimated_costs()["USD"].unpriced_tokens, 3_000_000);
    assert_eq!(totals.unpriced_calls(), 1);
}

#[test]
fn statistics_pricing_exact_provider_model_and_currency_never_borrow_prices() {
    let source = snapshot(&[json!({"input":1_000_000,"output":1_000_000,"totalTokens":2_000_000})]);
    let mut pricing = Pricing::default();
    let mut other = price("USD", Some(2.0), Some(5.0));
    other.provider = "other-provider".into();
    pricing.upsert_custom(other).unwrap();
    let mut other = price("USD", Some(2.0), Some(5.0));
    other.model = "canonical-model".into();
    pricing.upsert_custom(other).unwrap();
    let totals = source.clone().with_pricing(pricing.clone()).aggregate(&DateWindow::all("UTC"));
    assert_eq!(totals.cost_in("USD"), None);
    assert_eq!(totals.unpriced_calls(), 1);
    pricing.upsert_custom(price("EUR", Some(3.0), None)).unwrap();
    pricing.public_enabled = true;
    pricing.update_catalog(&catalog(), 100).unwrap();
    let totals = source.with_pricing(pricing).aggregate(&DateWindow::all("UTC"));
    assert_eq!(totals.estimated_costs()["EUR"].custom, Some(3.0));
    assert_eq!(totals.estimated_costs()["EUR"].catalog, None);
    assert_eq!(totals.estimated_costs()["EUR"].unpriced_tokens, 1_000_000);
    assert_eq!(totals.estimated_costs()["USD"].catalog, Some(18.0));
    assert_eq!(totals.cost_in("EUR"), None);
    assert_eq!(totals.cost_in("USD"), Some(18.0));
    assert_eq!(lyrel_core::statistics::analysis::Metric::Cost.value(&totals), None);
}

#[test]
fn statistics_pricing_rejects_invalid_prices_and_removes_only_exact_currency() {
    let mut pricing = Pricing::default();
    for invalid in [f64::NAN, f64::INFINITY, -1.0] {
        assert!(pricing.upsert_custom(price("USD", Some(invalid), None)).is_err());
    }
    assert!(pricing.upsert_custom(price("usd", Some(1.0), None)).is_err());
    pricing.upsert_custom(price("USD", Some(1.0), None)).unwrap();
    pricing.upsert_custom(price("EUR", Some(5.0), None)).unwrap();
    pricing.upsert_custom(price("USD", Some(0.0), None)).unwrap();
    assert!(!pricing.remove_custom("provider", "other-model", "USD"));
    assert!(pricing.remove_custom("provider", "model", "EUR"));
    pricing.validate().unwrap();
    let totals = snapshot(&[json!({"input":1_000_000,"output":0,"totalTokens":1_000_000})]).with_pricing(pricing).aggregate(&DateWindow::all("UTC"));
    assert_eq!(totals.cost_in("USD"), Some(0.0));
}

/// Exercise the persisted Snapshot contract used by the desktop cache, including
/// native adapter fields not represented by OMP's message format.
fn cached_record(source: &Snapshot, fields: serde_json::Value) -> Snapshot {
    let mut cached = serde_json::to_value(source).unwrap();
    let record = cached["files"][0]["session"]["records"][0].as_object_mut().unwrap();
    record.extend(fields.as_object().unwrap().clone());
    serde_json::from_value(cached).unwrap()
}

#[test]
fn statistics_pricing_reasoning_and_native_estimates_keep_independent_provenance() {
    let source = snapshot(&[json!({"input":1_000_000,"output":0,"totalTokens":3_000_000})]);
    let source = cached_record(&source, json!({"estimated_usd":0.75,"tokens":{"input":1_000_000,"output":0,"reasoning":2_000_000,"cache_read":0,"cache_write":0,"total":3_000_000}}));
    let mut pricing = Pricing::default();
    pricing.upsert_custom(price("USD", Some(2.0), Some(9.0))).unwrap();
    let totals = source.clone().with_pricing(pricing.clone()).aggregate(&DateWindow::all("UTC"));
    assert_eq!(totals.estimated_usd(), Some(0.75));
    assert_eq!(totals.estimated_costs()["USD"].custom, Some(2.0));
    assert_eq!(totals.estimated_costs()["USD"].unpriced_tokens, 2_000_000);
    assert_eq!(totals.cost_in("USD"), None);
    let mut complete = price("USD", Some(2.0), Some(9.0));
    complete.reasoning = Some(4.0);
    pricing.upsert_custom(complete).unwrap();
    let totals = source.clone().with_pricing(pricing.clone()).aggregate(&DateWindow::all("UTC"));
    assert_eq!(totals.estimated_usd(), Some(0.75));
    assert_eq!(totals.cost_in("USD"), Some(10.0));
    let recorded = cached_record(&source, json!({"recorded_usd":0.0})).with_pricing(pricing);
    let totals = recorded.aggregate(&DateWindow::all("UTC"));
    assert_eq!(totals.recorded_usd(), Some(0.0));
    assert_eq!(totals.estimated_usd(), Some(0.75));
    assert_eq!(totals.cost_in("USD"), Some(0.0));
    assert!(totals.estimated_costs().is_empty());
}

#[test]
fn statistics_pricing_overview_heatmap_and_breakdowns_share_one_priced_result() {
    use chrono::TimeZone;
    use lyrel_core::statistics::analysis::Metric;
    let mut pricing = Pricing::default();
    pricing.upsert_custom(price("USD", Some(3.0), Some(5.0))).unwrap();
    let mut source = snapshot(&[json!({"input":1_000_000,"output":1_000_000,"totalTokens":2_000_000})]);
    source.apply_pricing(pricing);
    let window = DateWindow::all("UTC");
    let now = chrono::Utc.with_ymd_and_hms(2026, 3, 8, 12, 0, 0).unwrap();
    let analysis = source.analyze(&window, now, None, 2026).unwrap();
    assert_eq!(source.aggregate(&window).cost_in("USD"), Some(8.0));
    assert_eq!(analysis.totals().cost_in("USD"), Some(8.0));
    assert_eq!(analysis.agents()[0].totals().cost_in("USD"), Some(8.0));
    assert_eq!(analysis.models()[0].totals().cost_in("USD"), Some(8.0));
    assert_eq!(analysis.projects()[0].totals().cost_in("USD"), Some(8.0));
    assert_eq!(analysis.days().iter().find(|day| day.date() == now.date_naive()).unwrap().value(Metric::Cost), Some(8.0));
}

#[test]
fn statistics_pricing_cumulative_and_unknown_times_are_not_distributed_to_days() {
    use chrono::TimeZone;
    use lyrel_core::statistics::analysis::Metric;
    let source = snapshot(&[json!({"input":1_000_000,"output":0,"totalTokens":1_000_000})]);
    let mut pricing = Pricing::default();
    pricing.upsert_custom(price("USD", Some(3.0), None)).unwrap();
    let now = chrono::Utc.with_ymd_and_hms(2026, 3, 8, 12, 0, 0).unwrap();
    let earlier = chrono::Utc.with_ymd_and_hms(2026, 3, 7, 8, 0, 0).unwrap();
    for fields in [json!({"coverage_start":earlier.timestamp()}), json!({"time_unknown":true})] {
        let source = cached_record(&source, fields).with_pricing(pricing.clone());
        let analysis = source.analyze(&DateWindow::all("UTC"), now, None, 2026).unwrap();
        assert_eq!(analysis.totals().cost_in("USD"), Some(3.0));
        assert_eq!(analysis.totals().cumulative_records(), 1);
        assert_eq!(analysis.days().iter().find(|day| day.date() == now.date_naive()).unwrap().value(Metric::Cost), None);
        let today = DateWindow::calendar(lyrel_core::statistics::Period::Today, now, None).unwrap();
        assert_eq!(source.aggregate(&today).cost_in("USD"), None);
    }
}

#[test]
fn statistics_failed_source_keeps_old_facts_but_changed_root_cannot_revive_them() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("old");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("a.jsonl"), concat!(
        "{\"type\":\"session\",\"version\":3,\"id\":\"main\",\"timestamp\":\"2026-03-08T07:00:00Z\"}\n",
        "{\"type\":\"message\",\"id\":\"u\",\"timestamp\":\"2026-03-08T08:00:00Z\",\"message\":{\"role\":\"assistant\",\"usage\":{\"input\":11,\"output\":0,\"totalTokens\":11}}}"
    )).unwrap();
    let host = LocalHost::new();
    let mut cache = StatisticsCache::default();
    let source = Source::omp(root.clone(), "same-scope");
    assert_eq!(scan(&*host, &[source.clone()], &mut cache).unwrap().aggregate(&DateWindow::all("UTC")).tokens().total(), Some(11));
    std::fs::remove_dir_all(root).unwrap();
    let stale = scan(&*host, &[source], &mut cache).unwrap();
    assert!(!stale.is_complete());
    assert_eq!(stale.aggregate(&DateWindow::all("UTC")).tokens().total(), Some(11));
    let changed = scan(&*host, &[Source::omp(directory.path().join("other"), "same-scope")], &mut cache).unwrap();
    assert_eq!(changed.aggregate(&DateWindow::all("UTC")).calls(), 0);
    let removed = scan(&*host, &[], &mut cache).unwrap();
    assert_eq!(removed.aggregate(&DateWindow::all("UTC")).calls(), 0);
}
