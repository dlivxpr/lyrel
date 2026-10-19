//! 原生历史的结构化统计 seam；扫描只缓存计量元数据，不保留对话正文。
pub mod analysis;
mod omp;
mod time_range;
use crate::host::{Host, Meta};
pub use omp::discover_omp;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::io;
use std::path::PathBuf;
pub use time_range::{DateWindow, Period};

const MAX_FILES: usize = 5_000;
const MAX_DIRS: usize = 20_000;
const MAX_FILE_BYTES: u64 = 16 * 1024 * 1024;
const MAX_SCAN_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Source {
    root: PathBuf,
    scope: String,
    agent: String,
}
impl Source {
    pub fn omp(root: PathBuf, scope: impl Into<String>) -> Self {
        Self {
            root,
            scope: scope.into(),
            agent: "omp".into(),
        }
    }
    pub fn root(&self) -> &std::path::Path {
        &self.root
    }
    pub fn scope(&self) -> &str {
        &self.scope
    }
    pub fn agent(&self) -> &str {
        &self.agent
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Tokens {
    input: Option<u64>,
    output: Option<u64>,
    cache_read: Option<u64>,
    cache_write: Option<u64>,
    total: Option<u64>,
}
impl Tokens {
    pub fn input(&self) -> Option<u64> {
        self.input
    }
    pub fn output(&self) -> Option<u64> {
        self.output
    }
    pub fn cache_read(&self) -> Option<u64> {
        self.cache_read
    }
    pub fn cache_write(&self) -> Option<u64> {
        self.cache_write
    }
    pub fn total(&self) -> Option<u64> {
        self.total
    }
    fn add(&mut self, other: &Self, first: bool) {
        fn sum(a: Option<u64>, b: Option<u64>, first: bool) -> Option<u64> {
            if first {
                b
            } else {
                a.zip(b).and_then(|(a, b)| a.checked_add(b))
            }
        }
        self.input = sum(self.input, other.input, first);
        self.output = sum(self.output, other.output, first);
        self.cache_read = sum(self.cache_read, other.cache_read, first);
        self.cache_write = sum(self.cache_write, other.cache_write, first);
        self.total = sum(self.total, other.total, first);
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UsageRecord {
    id: String,
    timestamp: i64,
    provider: String,
    model: String,
    tokens: Tokens,
    recorded_usd: Option<f64>,
    #[serde(default)]
    coverage_start: Option<i64>,
    #[serde(default)]
    estimated_usd: Option<f64>,
}
impl UsageRecord {
    pub fn timestamp(&self) -> i64 {
        self.timestamp
    }
    pub fn provider(&self) -> &str {
        &self.provider
    }
    pub fn model(&self) -> &str {
        &self.model
    }
    pub fn tokens(&self) -> &Tokens {
        &self.tokens
    }
    pub fn recorded_usd(&self) -> Option<f64> {
        self.recorded_usd
    }
    pub fn coverage_start(&self) -> Option<i64> {
        self.coverage_start
    }
    pub fn estimated_usd(&self) -> Option<f64> {
        self.estimated_usd
    }
    fn overlaps(&self, window: &DateWindow) -> bool {
        window.start().is_none_or(|s| self.timestamp >= s)
            && window.end().is_none_or(|e| self.coverage_start.unwrap_or(self.timestamp) < e)
    }
    fn covered_by(&self, window: &DateWindow) -> bool {
        window.contains(self.timestamp)
            && window.contains(self.coverage_start.unwrap_or(self.timestamp))
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NativeSession {
    id: String,
    parent: Option<String>,
    timestamp: i64,
    project: Option<String>,
    records: Vec<UsageRecord>,
    #[serde(default)]
    activity: Vec<i64>,
}
impl NativeSession {
    pub fn id(&self) -> &str {
        &self.id
    }
    pub fn project(&self) -> Option<&str> {
        self.project.as_deref()
    }
    pub fn records(&self) -> &[UsageRecord] {
        &self.records
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct FileFacts {
    source: Source,
    path: PathBuf,
    child: bool,
    observed: i64,
    session: NativeSession,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ScanIssue {
    path: PathBuf,
    reason: String,
}
impl ScanIssue {
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }
    pub fn reason(&self) -> &str {
        &self.reason
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Snapshot {
    host: u64,
    sources: Vec<Source>,
    updated: i64,
    files: Vec<FileFacts>,
    issues: Vec<ScanIssue>,
}
impl Snapshot {
    pub fn host(&self) -> u64 {
        self.host
    }
    pub fn sources(&self) -> &[Source] {
        &self.sources
    }
    pub fn updated(&self) -> i64 {
        self.updated
    }
    pub fn issues(&self) -> &[ScanIssue] {
        &self.issues
    }
    pub fn is_complete(&self) -> bool {
        self.issues.is_empty()
    }
    pub fn aggregate(&self, window: &DateWindow) -> Totals {
        let mut out = Totals::default();
        let mut main_sessions = 0;
        let ambiguous = self.visit_usage(None, |file| {
            if file.active_in(window) {
                main_sessions += 1;
            }
        }, |_, record| {
            if record.covered_by(window) {
                out.add_record(record);
            } else if record.overlaps(window) {
                out.complete = false;
            }
        });
        out.complete &= self.is_complete() && ambiguous == 0;
        out.ambiguous_records = ambiguous;
        out.main_sessions = main_sessions;
        out
    }

    // One canonical stream for overview and all analysis. Copies and inherited records
    // are eliminated before consumers filter by date/model/project. Host is snapshot-wide.
    fn visit_usage<'a>(
        &'a self,
        agent: Option<&str>,
        mut session: impl FnMut(&'a FileFacts),
        mut usage: impl FnMut(&'a FileFacts, &'a UsageRecord),
    ) -> u64 {
        let mut identities = HashMap::new();
        for file in &self.files {
            let source = (file.source.agent.as_str(), file.source.scope.as_str());
            identities.insert((source, file.session.id.as_str()), file.session.id.as_str());
            if let Some(path) = file.path.to_str() {
                identities.insert((source, path), file.session.id.as_str());
            }
        }
        let mut parents = HashMap::new();
        for file in &self.files {
            let source = (file.source.agent.as_str(), file.source.scope.as_str());
            if let Some(parent) = file.session.parent.as_deref().and_then(|p| identities.get(&(source, p))) {
                parents.insert((source, file.session.id.as_str()), *parent);
            }
        }
        let mut sessions = HashSet::new();
        let mut records: HashMap<_, (&UsageRecord, Vec<&UsageRecord>)> = HashMap::new();
        let mut ambiguous = 0;
        // Prefer original roots for inherited record attribution, independently of directory order.
        let mut files: Vec<_> = self.files.iter().collect();
        files.sort_by(|a, b| (a.session.parent.is_some(), a.child, &a.path).cmp(&(b.session.parent.is_some(), b.child, &b.path)));
        for file in files {
            if agent.is_some_and(|a| a != file.source.agent) {
                continue;
            }
            let source = (file.source.agent.as_str(), file.source.scope.as_str());
            if !file.child && sessions.insert((source, file.session.id.as_str())) {
                session(file);
            }
            let mut lineage = file.session.id.as_str();
            let mut visited = HashSet::new();
            while let Some(parent) = parents.get(&(source, lineage)) {
                if !visited.insert(lineage) { break; }
                lineage = parent;
            }
            for record in &file.session.records {
                match records.entry((source, lineage, record.id.as_str())) {
                    std::collections::hash_map::Entry::Vacant(entry) => {
                        entry.insert((record, Vec::new()));
                    }
                    std::collections::hash_map::Entry::Occupied(mut entry) => {
                        let (first, variants) = entry.get_mut();
                        if *first == record || variants.contains(&record) { continue; }
                        ambiguous += 1;
                        variants.push(record);
                    }
                }
                usage(file, record);
            }
        }
        ambiguous
    }
}
impl FileFacts {
    fn activity_timestamps(&self) -> impl Iterator<Item = i64> + '_ {
        std::iter::once(self.session.timestamp)
            .filter(|_| self.source.agent != "opencode")
            .chain(self.session.activity.iter().copied())
            .chain(self.session.records.iter().filter(|r| r.timestamp >= self.session.timestamp).map(|r| r.timestamp))
    }
    fn active_in(&self, window: &DateWindow) -> bool {
        self.activity_timestamps().any(|t| window.contains(t))
    }
}
#[derive(Clone, Debug)]
pub struct Totals {
    tokens: Tokens,
    recorded_usd: Option<f64>,
    estimated_usd: Option<f64>,
    unpriced_calls: u64,
    main_sessions: u64,
    calls: u64,
    ambiguous_records: u64,
    complete: bool,
}
impl Totals {
    fn add_record(&mut self, record: &UsageRecord) {
        self.tokens.add(&record.tokens, self.calls == 0);
        self.calls += 1;
        if let Some(cost) = record.recorded_usd {
            self.recorded_usd = Some(self.recorded_usd.unwrap_or(0.) + cost);
        } else if let Some(cost) = record.estimated_usd {
            self.estimated_usd = Some(self.estimated_usd.unwrap_or(0.) + cost);
        } else {
            self.unpriced_calls += 1;
        }
    }
    pub fn estimated_usd(&self) -> Option<f64> {
        self.estimated_usd
    }
    pub fn tokens(&self) -> &Tokens {
        &self.tokens
    }
    pub fn recorded_usd(&self) -> Option<f64> {
        self.recorded_usd
    }
    pub fn unpriced_calls(&self) -> u64 {
        self.unpriced_calls
    }
    pub fn main_sessions(&self) -> u64 {
        self.main_sessions
    }
    pub fn calls(&self) -> u64 {
        self.calls
    }
    pub fn is_complete(&self) -> bool {
        self.complete
    }
    pub fn average_tokens(&self) -> Option<f64> {
        (self.complete && self.main_sessions > 0)
            .then(|| {
                self.tokens
                    .total
                    .map(|t| t as f64 / self.main_sessions as f64)
            })
            .flatten()
    }
    pub fn token_change(&self, previous: &Self) -> Option<f64> {
        if !self.complete || !previous.complete {
            return None;
        }
        self.tokens
            .total
            .zip(previous.tokens.total)
            .filter(|(_, p)| *p > 0)
            .map(|(n, p)| (n as f64 / p as f64 - 1.) * 100.)
    }
}
impl Default for Totals {
    fn default() -> Self {
        Self { tokens: Tokens::default(), recorded_usd: None, estimated_usd: None,
            unpriced_calls: 0, main_sessions: 0, calls: 0, ambiguous_records: 0, complete: true }
    }
}
#[derive(Default)]
pub struct StatisticsCache {
    files: HashMap<(u64, String, PathBuf), (Meta, FileFacts)>,
}

/// Blocking Host operation. GUI callers dispatch using HostOps. No network pricing.
pub fn scan(
    host: &dyn Host,
    sources: &[Source],
    cache: &mut StatisticsCache,
) -> io::Result<Snapshot> {
    crate::host::guard_off_ui();
    if !host.is_connected() {
        return Err(io::Error::new(
            io::ErrorKind::NotConnected,
            "statistics Host is disconnected",
        ));
    }
    let mut snapshot = Snapshot {
        host: host.id().0,
        sources: sources.to_vec(),
        updated: chrono::Utc::now().timestamp(),
        files: Vec::new(),
        issues: Vec::new(),
    };
    let mut seen = HashSet::new();
    let mut dirs = 0;
    let mut files = 0;
    let mut bytes = 0;
    for source in sources {
        let exact_file = host.stat(&source.root).is_ok_and(|m| !m.is_dir);
        let initial = if exact_file {
            let value = source.root.to_string_lossy();
            let split = value.rfind(['/', '\\']).ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "native source has no Host parent",
                )
            })?;
            PathBuf::from(&value[..split.max(1)])
        } else {
            source.root.clone()
        };
        let child_root = if exact_file { &initial } else { &source.root };
        let root_is_child = host
            .stat(&PathBuf::from(format!(
                "{}.jsonl",
                child_root.to_string_lossy()
            )))
            .is_ok_and(|m| !m.is_dir);
        let mut pending = vec![(initial, root_is_child)];
        while let Some((path, child)) = pending.pop() {
            dirs += 1;
            if dirs > MAX_DIRS {
                snapshot.issues.push(ScanIssue {
                    path,
                    reason: "directory scan limit reached".into(),
                });
                break;
            }
            let entries = match host.read_dir(&path, None) {
                Ok(entries) => entries,
                Err(error)
                    if error.kind() == io::ErrorKind::NotFound
                        && path == source.root
                        && !cache
                            .files
                            .keys()
                            .any(|(id, scope, _)| *id == host.id().0 && *scope == source.scope) =>
                {
                    continue;
                }
                Err(error) => {
                    snapshot.issues.push(ScanIssue {
                        path,
                        reason: error.to_string(),
                    });
                    continue;
                }
            };
            for entry in entries {
                if entry.is_symlink {
                    snapshot.issues.push(ScanIssue {
                        path: host.join(&path, &entry.name),
                        reason: "symbolic link omitted from native statistics scope".into(),
                    });
                    continue;
                }
                let path = host.join(&path, &entry.name);
                if exact_file && path != source.root {
                    continue;
                }
                if entry.is_dir {
                    let sibling = PathBuf::from(format!("{}.jsonl", path.to_string_lossy()));
                    let nested = child || host.stat(&sibling).is_ok_and(|m| !m.is_dir);
                    pending.push((path, nested));
                    continue;
                }
                if !entry.name.ends_with(".jsonl") {
                    continue;
                }
                let path = match host.canonicalize(&path) {
                    Ok(path) => path,
                    Err(error) => {
                        snapshot.issues.push(ScanIssue {
                            path,
                            reason: error.to_string(),
                        });
                        continue;
                    }
                };
                if !seen.insert((source.scope.clone(), path.clone())) {
                    continue;
                }
                files += 1;
                if files > MAX_FILES {
                    snapshot.issues.push(ScanIssue {
                        path,
                        reason: "file scan limit reached".into(),
                    });
                    pending.clear();
                    break;
                }
                let key = (host.id().0, source.scope.clone(), path.clone());
                let result = (|| {
                    let meta = host.stat(&path)?;
                    if let Some((old, facts)) = cache.files.get(&key) {
                        if old == &meta {
                            return Ok((meta, facts.clone()));
                        }
                    }
                    if meta.len > MAX_FILE_BYTES || bytes + meta.len > MAX_SCAN_BYTES {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "statistics byte scan limit reached",
                        ));
                    }
                    bytes += meta.len;
                    let data = host.read_file(&path, MAX_FILE_BYTES)?;
                    let session = omp::parse(&data)?;
                    Ok((
                        meta,
                        FileFacts {
                            source: source.clone(),
                            path: path.clone(),
                            child,
                            observed: snapshot.updated,
                            session,
                        },
                    ))
                })();
                match result {
                    Ok((meta, facts)) => {
                        cache.files.insert(key, (meta, facts.clone()));
                        snapshot.files.push(facts);
                    }
                    Err(error) => {
                        snapshot.issues.push(ScanIssue {
                            path,
                            reason: error.to_string(),
                        });
                        if let Some((_, facts)) = cache.files.get(&key) {
                            snapshot.files.push(facts.clone());
                        }
                    }
                }
            }
        }
    }
    if !snapshot.issues.is_empty() {
        for ((host_id, scope, path), (_, facts)) in &cache.files {
            if *host_id == host.id().0
                && sources.iter().any(|s| s.scope == *scope)
                && !snapshot
                    .files
                    .iter()
                    .any(|f| f.path == *path && f.source.scope == *scope)
            {
                snapshot.files.push(facts.clone());
            }
        }
    }
    // Restrict lineage claims rather than silently treating an unresolved parent as reliable.
    for file in &snapshot.files {
        if let Some(parent) = &file.session.parent {
            if !snapshot.files.iter().any(|f| {
                f.source.scope == file.source.scope
                    && (f.session.id == *parent || f.path.to_string_lossy() == parent.as_str())
            }) {
                snapshot.issues.push(ScanIssue {
                    path: file.path.clone(),
                    reason:
                        "parentSession lineage is unavailable; inherited usage cannot be verified"
                            .into(),
                });
            }
        }
    }
    if !snapshot.issues.is_empty() {
        if let Some(oldest) = snapshot.files.iter().map(|f| f.observed).min() {
            snapshot.updated = oldest;
        }
    }
    if snapshot.issues.is_empty() {
        cache.files.retain(|(id, scope, path), _| {
            *id != host.id().0
                || !sources.iter().any(|s| s.scope == *scope)
                || seen.contains(&(scope.clone(), path.clone()))
        });
    }
    Ok(snapshot)
}
#[cfg(test)]
mod tests;
