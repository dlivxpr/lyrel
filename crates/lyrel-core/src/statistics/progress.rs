use super::*;

/// Scan independent native services concurrently and publish readable local facts
/// before those services finish. Pending sources remain explicitly incomplete.
/// The callback runs on the calling worker, never on the GUI thread.
pub fn scan_with_progress(
    host: &dyn Host,
    sources: &[Source],
    cache: &mut StatisticsCache,
    mut progress: impl FnMut(&Snapshot),
) -> io::Result<Snapshot> {
    crate::host::guard_off_ui();
    if !host.is_connected() {
        return Err(io::Error::new(
            io::ErrorKind::NotConnected,
            "statistics Host is disconnected",
        ));
    }
    let mut services = Vec::with_capacity(2);
    for agent in ["hermes", "opencode"] {
        let selected: Vec<_> = sources
            .iter()
            .filter(|s| s.agent == agent)
            .cloned()
            .collect();
        if selected.is_empty() {
            continue;
        }
        let mut service_cache = StatisticsCache::default();
        if agent == "hermes" {
            service_cache.native = std::mem::take(&mut cache.native);
        } else {
            service_cache.opencode = std::mem::take(&mut cache.opencode);
        }
        services.push((agent, selected, service_cache));
    }
    if services.is_empty() {
        return scan(host, sources, cache);
    }
    let local: Vec<_> = sources
        .iter()
        .filter(|s| s.agent != "hermes" && s.agent != "opencode")
        .cloned()
        .collect();
    let mut pending: Vec<_> = services.iter().map(|(agent, _, _)| *agent).collect();
    std::thread::scope(|threads| {
        let (send, receive) = std::sync::mpsc::channel();
        for (agent, selected, mut service_cache) in services {
            let send = send.clone();
            threads.spawn(move || {
                let result = scan(host, &selected, &mut service_cache);
                let _ = send.send((agent, result, service_cache));
            });
        }
        drop(send);
        let mut result = scan(host, &local, cache);
        let observed = chrono::Utc::now().timestamp();
        if let Ok(snapshot) = &mut result {
            snapshot.sources = sources
                .iter()
                .cloned()
                .map(|mut s| {
                    s.opencode_request = None;
                    s.hermes_request = None;
                    s
                })
                .collect();
            publish(snapshot, &pending, observed, &mut progress);
        }
        for (agent, partial, service_cache) in receive {
            if agent == "hermes" {
                cache.native = service_cache.native;
            } else {
                cache.opencode = service_cache.opencode;
            }
            pending.retain(|pending| *pending != agent);
            if let Ok(snapshot) = &mut result {
                match partial {
                    Ok(partial) => {
                        for (target, verified) in snapshot
                            .sources
                            .iter_mut()
                            .filter(|s| s.agent == agent)
                            .zip(partial.sources)
                        {
                            *target = verified;
                        }
                        snapshot.files.extend(partial.files);
                        snapshot.issues.extend(partial.issues);
                    }
                    Err(error) => snapshot.issues.push(ScanIssue {
                        path: PathBuf::from(agent),
                        reason: error.to_string(),
                    }),
                }
                if !pending.is_empty() {
                    publish(snapshot, &pending, observed, &mut progress);
                }
            }
        }
        if let Ok(snapshot) = &mut result {
            update_observed(snapshot, observed);
        }
        result
    })
}

fn publish(
    snapshot: &mut Snapshot,
    pending: &[&str],
    observed: i64,
    progress: &mut impl FnMut(&Snapshot),
) {
    let issues = snapshot.issues.len();
    for source in snapshot
        .sources
        .iter()
        .filter(|s| pending.contains(&s.agent.as_str()))
    {
        snapshot.issues.push(ScanIssue {
            path: source.root.clone(),
            reason: format!("{} source is still scanning", source.agent),
        });
    }
    update_observed(snapshot, observed);
    progress(snapshot);
    snapshot.issues.truncate(issues);
}

fn update_observed(snapshot: &mut Snapshot, observed: i64) {
    snapshot.updated = if snapshot.issues.is_empty() {
        observed
    } else {
        snapshot
            .files
            .iter()
            .map(|f| f.observed)
            .min()
            .unwrap_or(observed)
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::cli_agent::CLIAgent;
    use crate::host::local::LocalHost;
    use crate::host::session_operations::{SessionOperation, SessionOperationKind, SessionScope};

    #[test]
    fn local_usage_is_published_before_native_failures_without_claiming_complete_totals() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("local.jsonl"), concat!(
            "{\"type\":\"session\",\"version\":3,\"id\":\"local\",\"timestamp\":\"2026-10-01T00:00:00Z\",\"cwd\":\"/work\"}\n",
            "{\"type\":\"model_usage\",\"id\":\"usage\",\"timestamp\":\"2026-10-01T00:00:00Z\",\"provider\":\"test\",\"model\":\"fixture\",\"usage\":{\"input\":8,\"output\":2,\"cacheRead\":0,\"cacheWrite\":0,\"totalTokens\":10}}\n"
        )).unwrap();
        let host = LocalHost::new();
        let missing = directory
            .path()
            .join("missing-native-cli")
            .to_string_lossy()
            .into_owned();
        let sources = vec![
            Source::omp(directory.path().into(), "local"),
            Source::opencode(
                directory.path().into(),
                "service",
                SessionOperation {
                    expected_host: host.id().0,
                    agent: CLIAgent::OpenCode,
                    kind: SessionOperationKind::Resume,
                    source_id: String::new(),
                    cwd: directory.path().into(),
                    launch_argv: vec![missing],
                    scope: SessionScope::default(),
                },
            ),
        ];
        let mut published = Vec::new();
        let mut cache = StatisticsCache::default();
        let snapshot = scan_with_progress(&*host, &sources, &mut cache, |snapshot| {
            let totals = snapshot.aggregate(&DateWindow::all("UTC"));
            published.push((
                totals.known_tokens(),
                totals.is_complete(),
                snapshot.sources().len(),
            ));
        })
        .unwrap();
        assert_eq!(published, [(Some(10), false, 2)]);
        assert!(!snapshot.is_complete());
        assert_eq!(
            snapshot.aggregate(&DateWindow::all("UTC")).known_tokens(),
            Some(10)
        );
        assert!(
            snapshot
                .issues()
                .iter()
                .all(|issue| !issue.reason().contains("still scanning"))
        );
        // A later local-only selection neither inherits pending errors nor rereads another scope's service.
        let local =
            scan_with_progress(&*host, &sources[..1], &mut cache, |_| unreachable!()).unwrap();
        assert!(local.is_complete());
        assert_eq!(
            local.aggregate(&DateWindow::all("UTC")).tokens().total(),
            Some(10)
        );
    }
}
