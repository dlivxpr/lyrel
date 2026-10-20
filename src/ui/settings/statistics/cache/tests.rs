use super::*;
use lyrel_core::host::local::LocalHost;

const AGENT: &str = "cache-test-no-native-sources";

fn request(host: &SharedHost, stored: &Arc<StoredStatistics>) -> Request {
    Request {
        host: host.clone(),
        agent: Some(AGENT),
        launch: HashMap::new(),
        stored: stored.clone(),
        path: None,
        opencode_sources: Vec::new(),
        reuse_snapshot: false,
    }
}
fn receive(subscription: &Subscription) -> Update {
    smol::block_on(subscription.recv()).expect("shared request must terminate")
}
fn finish(subscription: &Subscription) -> Update {
    loop {
        let update = receive(subscription);
        if update.finished {
            assert!(update.error.is_none(), "{:?}", update.error);
            return update;
        }
    }
}
fn entry(service: &Service) -> (Scope, Arc<Mutex<StatisticsCache>>, Option<Uuid>) {
    let state = service.state.lock().unwrap();
    let (scope, entry) = state.entries.iter().next().unwrap();
    (scope.clone(), entry.parser.clone(), entry.flight)
}

#[test]
fn reopening_reuses_snapshot_and_parser_without_another_scan() {
    let service = Arc::new(Service::default());
    let host = LocalHost::new();
    let stored = Arc::new(StoredStatistics::default());
    let first = finish(&service.subscribe(request(&host, &stored), false).unwrap());
    let (_, parser, _) = entry(&service);
    let reopened = receive(&service.subscribe(request(&host, &stored), false).unwrap());
    assert!(reopened.finished);
    assert!(Arc::ptr_eq(
        first.value.as_ref().unwrap(),
        reopened.value.as_ref().unwrap()
    ));
    assert!(Arc::ptr_eq(&parser, &entry(&service).1));
    assert!(entry(&service).2.is_none());
}

#[test]
fn expired_snapshot_is_served_before_background_refresh() {
    let service = Arc::new(Service::default());
    let host = LocalHost::new();
    let stored = Arc::new(StoredStatistics::default());
    let first = finish(&service.subscribe(request(&host, &stored), false).unwrap());
    let (scope, parser, _) = entry(&service);
    service
        .state
        .lock()
        .unwrap()
        .entries
        .get_mut(&scope)
        .unwrap()
        .completed = Some(Instant::now() - TTL);
    let hold_scan = parser.lock().unwrap();
    let subscription = service.subscribe(request(&host, &stored), false).unwrap();
    let stale = receive(&subscription);
    assert!(!stale.finished);
    assert!(Arc::ptr_eq(
        first.value.as_ref().unwrap(),
        stale.value.as_ref().unwrap()
    ));
    drop(hold_scan);
    let refreshed = finish(&subscription);
    assert!(!Arc::ptr_eq(
        first.value.as_ref().unwrap(),
        refreshed.value.as_ref().unwrap()
    ));
}

#[test]
fn concurrent_windows_and_reopen_subscribe_to_the_same_flight() {
    let service = Arc::new(Service::default());
    let host = LocalHost::new();
    let stored = Arc::new(StoredStatistics::default());
    finish(&service.subscribe(request(&host, &stored), false).unwrap());
    let (scope, parser, _) = entry(&service);
    let hold_scan = parser.lock().unwrap();
    let closed_page = service.subscribe(request(&host, &stored), true).unwrap();
    let flight = service.state.lock().unwrap().entries[&scope].flight;
    drop(closed_page);
    let reopened = service.subscribe(request(&host, &stored), false).unwrap();
    let another_window = service.subscribe(request(&host, &stored), false).unwrap();
    assert_eq!(service.state.lock().unwrap().entries[&scope].flight, flight);
    assert_eq!(service.state.lock().unwrap().entries.len(), 1);
    drop(hold_scan);
    let first = finish(&reopened);
    let second = finish(&another_window);
    assert!(Arc::ptr_eq(
        first.value.as_ref().unwrap(),
        second.value.as_ref().unwrap()
    ));
    let cached = receive(&service.subscribe(request(&host, &stored), false).unwrap());
    assert!(cached.finished);
    assert!(Arc::ptr_eq(
        first.value.as_ref().unwrap(),
        cached.value.as_ref().unwrap()
    ));
}

#[test]
fn manual_refresh_supersedes_old_flight_and_releases_all_subscribers() {
    let service = Arc::new(Service::default());
    let host = LocalHost::new();
    let stored = Arc::new(StoredStatistics::default());
    let first = finish(&service.subscribe(request(&host, &stored), false).unwrap());
    let (scope, parser, _) = entry(&service);
    let hold_scan = parser.lock().unwrap();
    let older = service.subscribe(request(&host, &stored), true).unwrap();
    let old_flight = service.state.lock().unwrap().entries[&scope]
        .flight
        .unwrap();
    let newer = service.subscribe(request(&host, &stored), true).unwrap();
    let new_flight = service.state.lock().unwrap().entries[&scope]
        .flight
        .unwrap();
    assert_ne!(old_flight, new_flight);
    service.publish(
        &scope,
        old_flight,
        Update {
            value: first.value,
            error: Some("obsolete failure".into()),
            finished: true,
            stored: stored.clone(),
            revision: old_flight,
        },
    );
    assert_eq!(
        service.state.lock().unwrap().entries[&scope].flight,
        Some(new_flight)
    );
    assert!(!service.accepts(host.id(), Some(AGENT), old_flight));
    drop(hold_scan);
    assert_eq!(finish(&older).revision, new_flight);
    assert_eq!(finish(&newer).revision, new_flight);
}

#[test]
fn settings_change_invalidates_old_publication_and_reloads_shared_configuration() {
    let service = Arc::new(Service::default());
    let temp = tempfile::TempDir::new().unwrap();
    let path = temp.path().join("statistics-settings.json");
    let stored = service.load_settings(&path).unwrap();
    let host = LocalHost::new();
    let make_request = || {
        let mut request = request(&host, &stored);
        request.path = Some(path.clone());
        request
    };
    let first = finish(&service.subscribe(make_request(), false).unwrap());
    let (old_scope, parser, _) = entry(&service);
    let hold_scan = parser.lock().unwrap();
    let old_subscriber = service.subscribe(make_request(), true).unwrap();
    let old_revision = receive(&old_subscriber).revision;
    let mut edited = (*stored).clone();
    edited.pricing.public_enabled = true;
    let edited = Arc::new(edited);
    service.save_settings(Some(&path), edited.clone()).unwrap();
    assert!(smol::block_on(old_subscriber.recv()).is_err());
    service.publish(
        &old_scope,
        old_revision,
        Update {
            value: first.value,
            error: None,
            finished: true,
            stored: stored.clone(),
            revision: old_revision,
        },
    );
    assert!(!service.accepts(host.id(), Some(AGENT), old_revision));
    assert!(service.state.lock().unwrap().entries.is_empty());
    drop(hold_scan);
    let updated = finish(&service.subscribe(make_request(), false).unwrap());
    assert!(Arc::ptr_eq(&updated.stored, &edited));
    assert!(!Arc::ptr_eq(&parser, &entry(&service).1));
}

#[test]
fn launch_and_host_reconnection_are_independent_request_identities() {
    let service = Arc::new(Service::default());
    let host = LocalHost::new();
    let stored = Arc::new(StoredStatistics::default());
    let first = finish(&service.subscribe(request(&host, &stored), false).unwrap());
    let mut launcher = request(&host, &stored);
    launcher.launch.insert("omp".into(), "alternate-omp".into());
    let changed = finish(&service.subscribe(launcher, false).unwrap());
    assert!(!Arc::ptr_eq(
        first.value.as_ref().unwrap(),
        changed.value.as_ref().unwrap()
    ));
    let reconnected = LocalHost::new();
    assert_eq!(host.id(), reconnected.id());
    let reconnected = finish(
        &service
            .subscribe(request(&reconnected, &stored), false)
            .unwrap(),
    );
    assert!(!Arc::ptr_eq(
        first.value.as_ref().unwrap(),
        reconnected.value.as_ref().unwrap()
    ));
    // Returning to a distinct, still-valid request does not cancel another window's flight.
    let original = receive(&service.subscribe(request(&host, &stored), false).unwrap());
    assert!(Arc::ptr_eq(
        first.value.as_ref().unwrap(),
        original.value.as_ref().unwrap()
    ));
}

#[test]
fn inactive_configuration_variants_have_a_bounded_retention() {
    let service = Arc::new(Service::default());
    let host = LocalHost::new();
    let stored = Arc::new(StoredStatistics::default());
    for ix in 0..(RETAINED_REQUESTS + 5) {
        let mut request = request(&host, &stored);
        request.launch.insert("omp".into(), format!("omp-{ix}"));
        finish(&service.subscribe(request, false).unwrap());
    }
    assert!(service.state.lock().unwrap().entries.len() <= RETAINED_REQUESTS);
}

#[test]
fn final_publication_survives_without_any_open_page() {
    let service = Arc::new(Service::default());
    let host = LocalHost::new();
    let stored = Arc::new(StoredStatistics::default());
    let initial = finish(&service.subscribe(request(&host, &stored), false).unwrap());
    let (scope, _, _) = entry(&service);
    let revision = Uuid::new_v4();
    {
        let mut state = service.state.lock().unwrap();
        let entry = state.entries.get_mut(&scope).unwrap();
        entry.flight = Some(revision);
        entry.revision = revision;
    }
    let page = service.subscribe(request(&host, &stored), false).unwrap();
    drop(page);
    service.publish(
        &scope,
        revision,
        Update {
            value: initial.value.clone(),
            error: None,
            finished: true,
            stored: stored.clone(),
            revision,
        },
    );
    let reopened = receive(&service.subscribe(request(&host, &stored), false).unwrap());
    assert!(reopened.finished);
    assert_eq!(reopened.revision, revision);
    assert!(Arc::ptr_eq(
        initial.value.as_ref().unwrap(),
        reopened.value.as_ref().unwrap()
    ));
}

#[test]
fn failed_flight_terminates_subscribers_and_is_retryable() {
    let service = Arc::new(Service::default());
    let host = LocalHost::new();
    let stored = Arc::new(StoredStatistics::default());
    finish(&service.subscribe(request(&host, &stored), false).unwrap());
    let (scope, _, _) = entry(&service);
    let revision = Uuid::new_v4();
    {
        let mut state = service.state.lock().unwrap();
        let entry = state.entries.get_mut(&scope).unwrap();
        entry.flight = Some(revision);
        entry.revision = revision;
    }
    let subscription = service.subscribe(request(&host, &stored), false).unwrap();
    let stale = receive(&subscription);
    assert!(!stale.finished);
    service.publish(
        &scope,
        revision,
        Update {
            value: None,
            error: Some("source unavailable".into()),
            finished: true,
            stored: stored.clone(),
            revision,
        },
    );
    let failed = receive(&subscription);
    assert!(failed.finished);
    assert_eq!(failed.error.as_deref(), Some("source unavailable"));
    assert!(entry(&service).2.is_none());
    let retried = finish(&service.subscribe(request(&host, &stored), false).unwrap());
    assert_ne!(retried.revision, revision);
}

#[test]
fn edits_from_two_windows_mutate_the_latest_shared_settings() {
    let service = Arc::new(Service::default());
    let temp = tempfile::TempDir::new().unwrap();
    let path = temp.path().join("statistics-settings.json");
    service.load_settings(&path).unwrap();
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let workers: Vec<_> = [true, false]
        .into_iter()
        .map(|enable| {
            let service = service.clone();
            let path = path.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                service
                    .change_settings(&path, |stored| {
                        if enable {
                            stored.pricing.public_enabled = true;
                        } else {
                            stored.pricing.last_error = Some("retained catalog failure".into());
                        }
                        Ok(())
                    })
                    .unwrap();
            })
        })
        .collect();
    for worker in workers {
        worker.join().unwrap();
    }
    let saved = StoredStatistics::load(&path).unwrap();
    assert!(saved.pricing.public_enabled);
    assert_eq!(
        saved.pricing.last_error.as_deref(),
        Some("retained catalog failure")
    );
}

#[test]
fn live_opencode_context_changes_do_not_reuse_a_serialized_source_shell() {
    use lyrel_core::host::session_operations::{
        SessionOperation, SessionOperationKind, SessionScope,
    };
    let service = Arc::new(Service::default());
    let host = LocalHost::new();
    let stored = Arc::new(StoredStatistics::default());
    let scoped = |argument: &str| {
        let mut request = request(&host, &stored);
        request.opencode_sources.push((
            "opencode:client:stable-client".into(),
            SessionOperation {
                expected_host: host.id().0,
                agent: lyrel_core::core::cli_agent::CLIAgent::OpenCode,
                kind: SessionOperationKind::Resume,
                source_id: String::new(),
                cwd: PathBuf::from("same-directory"),
                launch_argv: vec!["opencode".into(), argument.into()],
                scope: SessionScope::default(),
            },
        ));
        request
    };
    // The dummy selected agent keeps this a metadata-only test: no service request occurs.
    let first = finish(&service.subscribe(scoped("--first"), false).unwrap());
    let changed = finish(&service.subscribe(scoped("--second"), false).unwrap());
    assert!(!Arc::ptr_eq(
        first.value.as_ref().unwrap(),
        changed.value.as_ref().unwrap()
    ));
    let original = receive(&service.subscribe(scoped("--first"), false).unwrap());
    assert!(Arc::ptr_eq(
        first.value.as_ref().unwrap(),
        original.value.as_ref().unwrap()
    ));
}

#[test]
fn changing_view_filters_never_restarts_an_expired_snapshot_scan() {
    let service = Arc::new(Service::default());
    let host = LocalHost::new();
    let stored = Arc::new(StoredStatistics::default());
    let first = finish(&service.subscribe(request(&host, &stored), false).unwrap());
    let (scope, parser, _) = entry(&service);
    service
        .state
        .lock()
        .unwrap()
        .entries
        .get_mut(&scope)
        .unwrap()
        .completed = Some(Instant::now() - TTL);
    let hold_scan = parser.lock().unwrap();
    let mut filter = request(&host, &stored);
    filter.reuse_snapshot = true;
    let filtered = receive(&service.subscribe(filter, false).unwrap());
    assert!(
        filtered.finished,
        "date/year filters must not restart native scanning after TTL"
    );
    assert_eq!(filtered.revision, first.revision);
    assert!(
        service.state.lock().unwrap().entries[&scope]
            .flight
            .is_none()
    );
    drop(hold_scan);
}

#[test]
fn reopening_detects_an_externally_changed_statistics_configuration() {
    let service = Arc::new(Service::default());
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("statistics-settings.json");
    let stored = service.load_settings(&path).unwrap();
    let host = LocalHost::new();
    let make_request = || {
        let mut request = request(&host, &stored);
        request.path = Some(path.clone());
        request
    };
    let first = finish(&service.subscribe(make_request(), false).unwrap());
    let mut changed = StoredStatistics::default();
    changed.pricing.public_enabled = true;
    changed.save(&path).unwrap();
    let reopened = finish(&service.subscribe(make_request(), false).unwrap());
    assert!(reopened.stored.pricing.public_enabled);
    assert_ne!(reopened.revision, first.revision);
    assert!(!service.accepts(host.id(), Some(AGENT), first.revision));
}
