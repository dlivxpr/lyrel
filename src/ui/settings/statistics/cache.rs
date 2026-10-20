//! 进程内统计服务：元数据锁不跨 Host I/O，扫描由两个专用 worker 执行。
use super::{Snapshot, StatisticsCache, statistics, storage::StoredStatistics};
use crate::ui::host_registry::{HostId, SharedHost};
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, LazyLock, Mutex, mpsc};
use std::time::{Duration, Instant};
use uuid::Uuid;

const TTL: Duration = Duration::from_secs(60);
const RETAINED_REQUESTS: usize = 16;
type Scope = (HostId, Option<&'static str>, Arc<[u8]>);
type Job = Box<dyn FnOnce() + Send>;

pub(super) struct CachedSnapshot {
    pub snapshot: Arc<Snapshot>,
    pub bindings: Arc<[statistics::sources::SourceBinding]>,
}
#[derive(Clone)]
pub(super) struct Update {
    pub value: Option<Arc<CachedSnapshot>>,
    pub error: Option<String>,
    pub finished: bool,
    pub stored: Arc<StoredStatistics>,
    pub revision: Uuid,
}
pub(super) struct Subscription {
    wake: smol::channel::Receiver<()>,
    latest: Arc<Mutex<Option<Update>>>,
}
#[derive(Clone)]
struct Subscriber {
    wake: smol::channel::Sender<()>,
    latest: Arc<Mutex<Option<Update>>>,
}
impl Subscription {
    pub async fn recv(&self) -> Result<Update, smol::channel::RecvError> {
        loop {
            self.wake.recv().await?;
            if let Some(update) = self.latest.lock().unwrap_or_else(|e| e.into_inner()).take() {
                return Ok(update);
            }
        }
    }
}
impl Subscriber {
    fn publish(&self, update: Update) -> bool {
        if self.wake.is_closed() {
            return false;
        }
        let previous = self
            .latest
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .replace(update);
        drop(previous);
        let _ = self.wake.try_send(());
        true
    }
}
pub(super) struct Request {
    pub host: SharedHost,
    pub agent: Option<&'static str>,
    pub launch: HashMap<String, String>,
    pub stored: Arc<StoredStatistics>,
    pub path: Option<PathBuf>,
    pub opencode_sources: Vec<(
        String,
        lyrel_core::host::session_operations::SessionOperation,
    )>,
    pub reuse_snapshot: bool,
}
struct Entry {
    _host: SharedHost,
    _stored: Arc<StoredStatistics>,
    path: Option<PathBuf>,
    flight: Option<Uuid>,
    revision: Uuid,
    value: Option<Arc<CachedSnapshot>>,
    completed: Option<Instant>,
    accessed: Instant,
    subscribers: Vec<Subscriber>,
    parser: Arc<Mutex<StatisticsCache>>,
}
struct CachedSettings {
    stamp: super::storage::SettingsStamp,
    value: Arc<StoredStatistics>,
}

#[derive(Default)]
struct State {
    entries: HashMap<Scope, Entry>,
    settings: HashMap<PathBuf, CachedSettings>,
}
pub(super) struct Service {
    state: Mutex<State>,
    settings_write: Mutex<()>,
    workers: LazyLock<Result<mpsc::Sender<Job>, String>>,
}
impl Default for Service {
    fn default() -> Self {
        Self {
            state: Mutex::new(State::default()),
            settings_write: Mutex::new(()),
            workers: LazyLock::new(|| worker_pool("lyrel-statistics-scan")),
        }
    }
}
pub(super) fn shared() -> &'static Arc<Service> {
    static SERVICE: LazyLock<Arc<Service>> = LazyLock::new(|| Arc::new(Service::default()));
    &SERVICE
}
pub(super) async fn compute<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, String> {
    static WORKERS: LazyLock<Result<mpsc::Sender<Job>, String>> =
        LazyLock::new(|| worker_pool("lyrel-statistics-aggregate"));
    let sender = WORKERS.as_ref().map_err(Clone::clone)?;
    let (reply, result) = smol::channel::bounded(1);
    sender
        .send(Box::new(move || {
            let value = std::panic::catch_unwind(std::panic::AssertUnwindSafe(work))
                .map_err(|_| "Statistics aggregation failed".to_string());
            let _ = reply.send_blocking(value);
        }))
        .map_err(|_| "Statistics aggregation worker unavailable".to_string())?;
    result
        .recv()
        .await
        .map_err(|_| "Statistics aggregation worker unavailable".to_string())?
}
impl Service {
    // Called only on a worker. The settings payload is never serialized or cloned on GPUI.
    pub fn load_settings(&self, path: &PathBuf) -> std::io::Result<Arc<StoredStatistics>> {
        let _write = self
            .settings_write
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        self.load_settings_locked(path)
    }

    fn load_settings_locked(&self, path: &PathBuf) -> std::io::Result<Arc<StoredStatistics>> {
        let stamp = super::storage::settings_stamp(path)?;
        if let Some(value) = self
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .settings
            .get(path)
            .filter(|cached| cached.stamp == stamp)
            .map(|cached| cached.value.clone())
        {
            return Ok(value);
        }
        let value = Arc::new(StoredStatistics::load(path)?);
        self.replace_settings(path, value.clone(), stamp);
        Ok(value)
    }
    pub fn change_settings(
        &self,
        path: &PathBuf,
        mutation: impl FnOnce(&mut StoredStatistics) -> Result<(), String>,
    ) -> Result<Arc<StoredStatistics>, String> {
        let _write = self
            .settings_write
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let mut stored = (*self
            .load_settings_locked(path)
            .map_err(|error| error.to_string())?)
        .clone();
        mutation(&mut stored)?;
        stored.save(path).map_err(|error| error.to_string())?;
        let stored = Arc::new(stored);
        self.save_settings(Some(path), stored.clone())
            .map_err(|error| error.to_string())?;
        Ok(stored)
    }
    pub fn save_settings(
        &self,
        path: Option<&PathBuf>,
        stored: Arc<StoredStatistics>,
    ) -> std::io::Result<()> {
        let Some(path) = path else { return Ok(()) };
        let stamp = super::storage::settings_stamp(path)?;
        self.replace_settings(path, stored, stamp);
        Ok(())
    }

    fn replace_settings(
        &self,
        path: &PathBuf,
        stored: Arc<StoredStatistics>,
        stamp: super::storage::SettingsStamp,
    ) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let previous = state.settings.insert(
            path.clone(),
            CachedSettings {
                stamp,
                value: stored,
            },
        );
        let removed: Vec<_> = state
            .entries
            .iter()
            .filter(|(_, entry)| entry.path.as_ref() == Some(path))
            .map(|(scope, _)| scope.clone())
            .collect();
        let entries: Vec<_> = removed
            .iter()
            .filter_map(|scope| state.entries.remove(scope))
            .collect();
        // Dropping parser facts and waking displaced subscribers stay off the metadata lock.
        drop(state);
        drop(entries);
        drop(previous);
    }
    fn enqueue(&self, job: Job) -> Result<(), String> {
        let sender = self.workers.as_ref().map_err(Clone::clone)?;
        sender
            .send(job)
            .map_err(|_| "Statistics worker unavailable".into())
    }
    fn prune(&self, incoming: &Scope) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let target = RETAINED_REQUESTS - usize::from(!state.entries.contains_key(incoming));
        let mut removed = Vec::new();
        while state.entries.len() > target {
            let oldest = state
                .entries
                .iter()
                .filter(|(scope, entry)| {
                    *scope != incoming
                        && entry.flight.is_none()
                        && entry
                            .subscribers
                            .iter()
                            .all(|subscriber| subscriber.wake.is_closed())
                })
                .min_by_key(|(_, entry)| entry.accessed)
                .map(|(scope, _)| scope.clone());
            let Some(oldest) = oldest else { break };
            removed.extend(state.entries.remove(&oldest));
        }
        drop(state);
        drop(removed);
    }
    pub fn subscribe(
        self: &Arc<Self>,
        mut request: Request,
        refresh: bool,
    ) -> Result<Subscription, String> {
        if let Some(path) = &request.path {
            request.stored = self
                .load_settings(path)
                .map_err(|error| error.to_string())?;
        }
        request
            .opencode_sources
            .sort_by(|(left, _), (right, _)| left.cmp(right));
        let launch: BTreeMap<_, _> = request.launch.iter().collect();
        let identity = Arc::as_ptr(&request.host) as *const () as usize;
        let settings_identity = Arc::as_ptr(&request.stored) as usize;
        let key = serde_json::to_vec(&(
            identity,
            settings_identity,
            launch,
            &request.path,
            &request.opencode_sources,
        ))
        .map_err(|error| error.to_string())?;
        let scope = (request.host.id(), request.agent, Arc::from(key));
        self.prune(&scope);
        // A capacity-one mailbox coalesces partial snapshots; final always replaces partial.
        let (wake, receiver) = smol::channel::bounded(1);
        let latest = Arc::new(Mutex::new(None));
        let sender = Subscriber {
            wake,
            latest: latest.clone(),
        };
        let subscription = Subscription {
            wake: receiver,
            latest,
        };
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if request
            .path
            .as_ref()
            .and_then(|path| state.settings.get(path))
            .is_some_and(|stored| !Arc::ptr_eq(&stored.value, &request.stored))
        {
            drop(state);
            return self.subscribe(request, refresh);
        }
        let entry = state.entries.entry(scope.clone()).or_insert_with(|| Entry {
            _host: request.host.clone(),
            _stored: request.stored.clone(),
            path: request.path.clone(),
            flight: None,
            revision: Uuid::new_v4(),
            value: None,
            completed: None,
            accessed: Instant::now(),
            subscribers: Vec::new(),
            parser: Arc::new(Mutex::new(StatisticsCache::default())),
        });
        entry.accessed = Instant::now();
        let fresh = (request.reuse_snapshot && entry.value.is_some())
            || entry.completed.is_some_and(|time| time.elapsed() < TTL);
        let start = refresh || (!fresh && entry.flight.is_none());
        if start {
            entry.revision = Uuid::new_v4();
            entry.flight = Some(entry.revision);
        }
        if let Some(value) = &entry.value {
            sender.publish(Update {
                value: Some(value.clone()),
                error: None,
                finished: !refresh && fresh && entry.flight.is_none(),
                stored: request.stored.clone(),
                revision: entry.revision,
            });
        }
        if !refresh && fresh && entry.flight.is_none() {
            return Ok(subscription);
        }
        entry.subscribers.push(sender);
        if !start {
            return Ok(subscription);
        }
        let flight = entry.revision;
        let parser = entry.parser.clone();
        drop(state);
        let service = self.clone();
        let error_stored = request.stored.clone();
        let worker_scope = scope.clone();
        let scheduled = self.enqueue(Box::new(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                if !service.is_current(&worker_scope, flight) {
                    return Ok(());
                }
                let mut parser = parser.lock().unwrap_or_else(|e| e.into_inner());
                if !service.is_current(&worker_scope, flight) {
                    return Ok(());
                }
                scan(&request, &mut parser, |value, finished| {
                    service.publish(
                        &worker_scope,
                        flight,
                        Update {
                            value: Some(value),
                            error: None,
                            finished,
                            stored: request.stored.clone(),
                            revision: flight,
                        },
                    );
                })
            }))
            .unwrap_or_else(|_| Err("Statistics worker failed".into()));
            if let Err(error) = result {
                service.publish(
                    &worker_scope,
                    flight,
                    Update {
                        value: None,
                        error: Some(error),
                        finished: true,
                        stored: request.stored,
                        revision: flight,
                    },
                );
            }
        }));
        if let Err(error) = scheduled {
            self.publish(
                &scope,
                flight,
                Update {
                    value: None,
                    error: Some(error),
                    finished: true,
                    stored: error_stored,
                    revision: flight,
                },
            );
        }
        Ok(subscription)
    }
    pub fn accepts(&self, host: HostId, agent: Option<&'static str>, revision: Uuid) -> bool {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entries
            .iter()
            .any(|((id, selected, _), entry)| {
                *id == host && *selected == agent && entry.revision == revision
            })
    }
    fn is_current(&self, scope: &Scope, flight: Uuid) -> bool {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entries
            .get(scope)
            .is_some_and(|entry| entry.flight == Some(flight))
    }
    fn publish(&self, scope: &Scope, flight: Uuid, update: Update) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let Some(entry) = state
            .entries
            .get_mut(scope)
            .filter(|entry| entry.flight == Some(flight))
        else {
            return;
        };
        let previous = update
            .value
            .as_ref()
            .and_then(|value| entry.value.replace(value.clone()));
        if update.finished {
            entry.flight = None;
            entry.completed = update.error.is_none().then(Instant::now);
        }
        entry
            .subscribers
            .retain(|subscriber| !subscriber.wake.is_closed());
        let subscribers = if update.finished {
            std::mem::take(&mut entry.subscribers)
        } else {
            entry.subscribers.clone()
        };
        drop(state);
        drop(previous);
        for subscriber in subscribers {
            subscriber.publish(update.clone());
        }
        if update.finished {
            self.prune(scope);
        }
    }
}
fn worker_pool(name: &str) -> Result<mpsc::Sender<Job>, String> {
    let (sender, receiver) = mpsc::channel::<Job>();
    let receiver = Arc::new(Mutex::new(receiver));
    for ix in 0..2 {
        let receiver = receiver.clone();
        std::thread::Builder::new()
            .name(format!("{name}-{ix}"))
            .spawn(move || {
                loop {
                    let job = receiver.lock().unwrap_or_else(|e| e.into_inner()).recv();
                    let Ok(job) = job else { break };
                    job();
                }
            })
            .map_err(|error| error.to_string())?;
    }
    Ok(sender)
}
fn scan(
    request: &Request,
    parser: &mut StatisticsCache,
    mut publish: impl FnMut(Arc<CachedSnapshot>, bool),
) -> Result<(), String> {
    let host = &*request.host;
    let mut sources = Vec::new();
    let mut issues = Vec::new();
    for name in ["omp", "pi", "kimi", "hermes", "opencode"] {
        if request.agent.is_some_and(|selected| selected != name) {
            continue;
        }
        if name == "opencode" && !request.opencode_sources.is_empty() {
            sources.extend(request.opencode_sources.iter().map(|(scope, operation)| {
                statistics::Source::opencode(
                    operation.cwd.clone(),
                    scope.clone(),
                    operation.clone(),
                )
            }));
            continue;
        }
        match parser.discover(host, name, &request.launch) {
            Ok(found) => sources.extend(found),
            Err(error) => issues.push((name.to_owned(), error.to_string())),
        }
    }
    let resolved = request
        .stored
        .sources
        .resolve_for_host(host, sources, &request.launch);
    for failure in resolved.issues {
        issues.push((
            "source".into(),
            format!("{}: {}", failure.binding.selector, failure.reason),
        ));
    }
    let bindings: Arc<[_]> = resolved.bindings.into();
    let sources: Vec<_> = resolved
        .sources
        .into_iter()
        .filter(|source| request.agent.is_none_or(|agent| agent == source.agent()))
        .collect();
    let prepare = |mut snapshot: Snapshot| {
        for (name, error) in &issues {
            snapshot = snapshot.with_issue(name.into(), error.clone());
        }
        Arc::new(CachedSnapshot {
            snapshot: Arc::new(snapshot.with_pricing(request.stored.pricing.clone())),
            bindings: bindings.clone(),
        })
    };
    let snapshot = statistics::scan_with_progress(host, &sources, parser, |snapshot| {
        publish(prepare(snapshot.clone()), false)
    })
    .map_err(|error| error.to_string())?;
    publish(prepare(snapshot), true);
    Ok(())
}

#[cfg(test)]
mod tests;
