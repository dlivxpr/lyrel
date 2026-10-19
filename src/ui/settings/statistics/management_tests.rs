//! 统计设置的持久化与外部公共目录请求行为。
use super::storage::*;

#[test]
fn statistics_settings_round_trip_and_corrupt_store_is_not_overwritten() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("statistics-settings.json");
    let mut stored = StoredStatistics::load(&path).unwrap();
    stored.pricing.public_enabled = true;
    stored.save(&path).unwrap();
    assert!(StoredStatistics::load(&path).unwrap().pricing.public_enabled);
    std::fs::write(&path, b"not-json").unwrap();
    assert!(StoredStatistics::load(&path).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), b"not-json");
}

#[test]
fn statistics_public_catalog_is_opt_in_and_failure_keeps_last_prices() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct ControlledHttp(AtomicUsize);
    impl CatalogHttp for ControlledHttp {
        fn get(&self, _: Option<&str>) -> Result<Vec<u8>, String> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Err("controlled offline".into())
        }
    }
    let http = ControlledHttp(AtomicUsize::new(0));
    let mut pricing = lyrel_core::statistics::pricing::Pricing::default();
    assert!(check_catalog(&mut pricing, &http, None, 10).is_err());
    assert_eq!(http.0.load(Ordering::SeqCst), 0);
    pricing.public_enabled = true;
    pricing.update_catalog(br#"{"fixture-provider":{"id":"fixture-provider","models":{"fixture-model":{"id":"fixture-model","cost":{"input":2,"output":3}}}}}"#, 12).unwrap();
    assert!(check_catalog(&mut pricing, &http, None, 20).is_err());
    assert_eq!(http.0.load(Ordering::SeqCst), 1);
    assert_eq!(pricing.checked_at, Some(20));
    assert_eq!(pricing.fetched_at, Some(12));
    assert_eq!(pricing.catalog[0].provider, "fixture-provider");
    assert_eq!(pricing.catalog[0].model, "fixture-model");
    assert_eq!(pricing.catalog[0].input, Some(2.));
    assert!(pricing.last_error.is_some());
}

#[test]
#[ignore = "explicit opt-in public HTTPS smoke; downloads only public prices"]
fn statistics_public_catalog_explicit_opt_in_smoke() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("statistics-settings.json");
    let mut stored = StoredStatistics::default();
    stored.pricing.public_enabled = true;
    let now = chrono::Utc::now().timestamp();
    check_catalog(&mut stored.pricing, &PublicCatalogHttp, None, now).expect("fixed public HTTPS catalog download");
    stored.save(&path).unwrap();
    let restored = StoredStatistics::load(&path).unwrap();
    assert_eq!(restored.pricing.fetched_at, Some(now));
    assert_eq!(restored.pricing.checked_at, Some(now));
    assert!(restored.pricing.catalog.iter().any(|p| p.currency == "USD" && p.input.is_some_and(|v| v.is_finite() && v >= 0.)));
    println!("models.dev public catalog: {} exact provider/model prices; fetched_at={now}; no history/paths/prompts/credentials sent", restored.pricing.catalog.len());
}
