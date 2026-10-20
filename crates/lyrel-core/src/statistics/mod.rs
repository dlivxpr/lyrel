//! 原生历史的结构化统计 seam；扫描只缓存计量元数据，不保留对话正文。
mod hermes;
pub use hermes::discover_hermes;
pub mod analysis;
mod kimi;
#[cfg(test)]
mod kimi_tests;
pub use kimi::discover_kimi;
mod omp;
mod opencode;
pub use opencode::discover_opencode;
mod pi;
pub mod pricing;
mod progress;
pub mod sources;
pub use progress::scan_with_progress;
mod time_range;
use crate::host::{Host, Meta};
pub use omp::discover_omp;
pub use pi::discover_pi;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::io;
use std::path::PathBuf;
pub use time_range::{DateWindow, Period};

const MAX_FILES: usize = 5_000;
const MAX_DIRS: usize = 20_000;
const MAX_FILE_BYTES: u64 = 16 * 1024 * 1024;
const MAX_SCAN_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Source {
    root: PathBuf,
    scope: String,
    agent: String,
    #[serde(skip)]
    opencode_request: Option<Box<crate::host::session_operations::SessionOperation>>,
    #[serde(skip)]
    hermes_request: Option<std::sync::Arc<crate::host::session_operations::SessionOperation>>,
    #[serde(default)]
    max_depth: Option<u8>,
    #[serde(default)]
    discovery_issue: Option<String>,
}
impl std::fmt::Debug for Source {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Source")
            .field("root", &self.root)
            .field("scope", &self.scope)
            .field("agent", &self.agent)
            .finish()
    }
}
impl Source {
    pub fn omp(root: PathBuf, scope: impl Into<String>) -> Self {
        Self {
            root,
            scope: scope.into(),
            agent: "omp".into(),
            opencode_request: None,
            hermes_request: None,
            max_depth: None,
            discovery_issue: None,
        }
    }
    pub fn kimi(root: PathBuf, scope: impl Into<String>) -> Self {
        Self {
            root,
            scope: scope.into(),
            agent: "kimi".into(),
            opencode_request: None,
            hermes_request: None,
            max_depth: None,
            discovery_issue: None,
        }
    }
    pub fn pi(root: PathBuf, scope: impl Into<String>) -> Self {
        Self {
            root,
            scope: scope.into(),
            agent: "pi".into(),
            opencode_request: None,
            hermes_request: None,
            max_depth: Some(0),
            discovery_issue: None,
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
    #[serde(default)]
    reasoning: Option<u64>,
    cache_write: Option<u64>,
    total: Option<u64>,
}
impl Tokens {
    pub fn reasoning(&self) -> Option<u64> {
        self.reasoning
    }
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
        self.reasoning = sum(self.reasoning, other.reasoning, first);
        self.cache_write = sum(self.cache_write, other.cache_write, first);
        self.total = sum(self.total, other.total, first);
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UsageRecord {
    id: String,
    #[serde(default)]
    fingerprint: Option<String>,
    timestamp: i64,
    provider: String,
    model: String,
    tokens: Tokens,
    recorded_usd: Option<f64>,
    #[serde(default)]
    coverage_start: Option<i64>,
    #[serde(default)]
    estimated_usd: Option<f64>,
    #[serde(default)]
    time_unknown: bool,
}
impl UsageRecord {
    pub fn time_unknown(&self) -> bool {
        self.time_unknown
    }
    pub fn coverage_start(&self) -> Option<i64> {
        self.coverage_start
    }
    pub fn estimated_usd(&self) -> Option<f64> {
        self.estimated_usd
    }
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
    fn overlaps(&self, window: &DateWindow) -> bool {
        if self.time_unknown {
            return true;
        }
        window.start().is_none_or(|s| self.timestamp >= s)
            && window
                .end()
                .is_none_or(|e| self.coverage_start.unwrap_or(self.timestamp) < e)
    }
    fn covered_by(&self, window: &DateWindow) -> bool {
        if self.time_unknown {
            return window.start().is_none() && window.end().is_none();
        }
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
    #[serde(default)]
    metadata_stamp: Option<u64>,
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
    #[serde(default)]
    pricing: pricing::Pricing,
}
impl Snapshot {
    pub fn with_pricing(mut self, pricing: pricing::Pricing) -> Self {
        self.apply_pricing(pricing);
        self
    }
    pub fn apply_pricing(&mut self, pricing: pricing::Pricing) {
        self.pricing = pricing;
    }
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
    /// Attach a failed source discovery without discarding readable agents.
    pub fn with_issue(mut self, path: PathBuf, reason: impl Into<String>) -> Self {
        self.issues.push(ScanIssue {
            path,
            reason: reason.into(),
        });
        self
    }
    pub fn aggregate(&self, window: &DateWindow) -> Totals {
        let mut out = Totals::default();
        let mut main_sessions = 0;
        let ambiguous = self.visit_usage(
            None,
            |file| {
                if file.active_in(window) {
                    main_sessions += 1;
                }
            },
            |_, record, cost| {
                if record.covered_by(window) {
                    out.add_record(record, cost);
                } else if record.overlaps(window) {
                    out.complete = false;
                }
            },
        );
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
        mut usage: impl FnMut(&'a FileFacts, &'a UsageRecord, &pricing::RecordCost<'a>),
    ) -> u64 {
        let prices = pricing::PriceIndex::new(&self.pricing);
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
            if let Some(parent) = file
                .session
                .parent
                .as_deref()
                .and_then(|p| identities.get(&(source, p)))
            {
                parents.insert((source, file.session.id.as_str()), *parent);
            }
        }
        let mut sessions = HashSet::new();
        let mut records: HashMap<_, (&UsageRecord, Vec<&UsageRecord>)> = HashMap::new();
        let mut ambiguous = 0;
        // Prefer original roots for inherited record attribution, independently of directory order.
        let mut files: Vec<_> = self.files.iter().collect();
        files.sort_by(|a, b| {
            (a.session.parent.is_some(), a.child, &a.path).cmp(&(
                b.session.parent.is_some(),
                b.child,
                &b.path,
            ))
        });
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
                if !visited.insert(lineage) {
                    break;
                }
                lineage = parent;
            }
            for record in &file.session.records {
                match records.entry((source, lineage, record.id.as_str())) {
                    std::collections::hash_map::Entry::Vacant(entry) => {
                        entry.insert((record, Vec::new()));
                    }
                    std::collections::hash_map::Entry::Occupied(mut entry) => {
                        let (first, variants) = entry.get_mut();
                        if *first == record || variants.contains(&record) {
                            continue;
                        }
                        ambiguous += 1;
                        variants.push(record);
                    }
                }
                let cost = prices.resolve(record);
                usage(file, record, &cost);
            }
        }
        ambiguous
    }
}
impl FileFacts {
    fn activity_timestamps(&self) -> impl Iterator<Item = i64> + '_ {
        std::iter::once(self.session.timestamp)
            .filter(|timestamp| self.source.agent != "opencode" && *timestamp != i64::MIN)
            .chain(self.session.activity.iter().copied())
            .chain(
                self.session
                    .records
                    .iter()
                    .filter(|r| {
                        self.source.agent != "opencode"
                            && !r.time_unknown
                            && r.timestamp >= self.session.timestamp
                    })
                    .map(|r| r.timestamp),
            )
    }
    fn active_in(&self, window: &DateWindow) -> bool {
        (window.start().is_none()
            && window.end().is_none()
            && self.session.records.iter().any(|r| r.time_unknown))
            || self.activity_timestamps().any(|t| window.contains(t))
    }
}
#[derive(Clone, Debug)]
pub struct Totals {
    tokens: Tokens,
    known_tokens: Option<u64>,
    has_known_tokens: bool,
    recorded_usd: Option<f64>,
    estimated_usd: Option<f64>,
    estimated_costs: std::collections::BTreeMap<String, pricing::CurrencyEstimate>,
    recorded_calls: u64,
    unpriced_calls: u64,
    main_sessions: u64,
    calls: u64,
    cumulative_records: u64,
    ambiguous_records: u64,
    complete: bool,
}
impl Totals {
    fn add_record(&mut self, record: &UsageRecord, cost: &pricing::RecordCost<'_>) {
        self.tokens.add(&record.tokens, self.calls == 0);
        if let Some(tokens) = record.tokens.total {
            self.known_tokens = self
                .known_tokens
                .and_then(|total| total.checked_add(tokens));
            self.has_known_tokens = true;
        }
        self.calls += 1;
        self.cumulative_records +=
            u64::from(record.coverage_start.is_some() || record.time_unknown);
        if let Some(cost) = record.estimated_usd {
            self.estimated_usd = Some(self.estimated_usd.unwrap_or(0.) + cost);
        }
        if let Some(cost) = record.recorded_usd {
            self.recorded_usd = Some(self.recorded_usd.unwrap_or(0.) + cost);
            self.recorded_calls += 1;
        } else if cost.unpriced {
            self.unpriced_calls += 1;
        }
        pricing::add_estimates(&mut self.estimated_costs, cost);
    }
    pub fn estimated_costs(
        &self,
    ) -> &std::collections::BTreeMap<String, pricing::CurrencyEstimate> {
        &self.estimated_costs
    }
    /// Returns a complete comparable amount in one explicit currency only.
    /// Native estimated_usd remains a separate informational estimate.
    pub fn cost_in(&self, currency: &str) -> Option<f64> {
        if !self.complete
            || self.unpriced_calls > 0
            || (currency != "USD" && self.recorded_calls > 0)
        {
            return None;
        }
        let estimated_calls = self.calls - self.recorded_calls;
        let estimate = self.estimated_costs.get(currency);
        if estimated_calls > 0
            && !estimate.is_some_and(|value| value.is_complete() && value.calls == estimated_calls)
        {
            return None;
        }
        Some(
            if currency == "USD" {
                self.recorded_usd.unwrap_or(0.0)
            } else {
                0.0
            } + estimate
                .and_then(pricing::CurrencyEstimate::total)
                .unwrap_or(0.0),
        )
    }
    pub fn estimated_usd(&self) -> Option<f64> {
        self.estimated_usd
    }
    pub fn tokens(&self) -> &Tokens {
        &self.tokens
    }
    /// Known per-record token subtotal; never a replacement for the complete total.
    pub fn known_tokens(&self) -> Option<u64> {
        (self.has_known_tokens && self.ambiguous_records == 0)
            .then_some(self.known_tokens)
            .flatten()
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
    pub fn cumulative_records(&self) -> u64 {
        self.cumulative_records
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
        Self {
            tokens: Tokens::default(),
            known_tokens: Some(0),
            has_known_tokens: false,
            recorded_usd: None,
            estimated_usd: None,
            estimated_costs: std::collections::BTreeMap::new(),
            recorded_calls: 0,
            unpriced_calls: 0,
            main_sessions: 0,
            calls: 0,
            cumulative_records: 0,
            ambiguous_records: 0,
            complete: true,
        }
    }
}
#[derive(Default)]
pub struct StatisticsCache {
    files: HashMap<(u64, String, String, PathBuf), (Meta, FileFacts)>,
    opencode: HashMap<(u64, String), (Source, Vec<FileFacts>, i64)>,
    native: HashMap<(u64, String, String), Vec<FileFacts>>,
    discovery: HashMap<(u64, String), DiscoveredSources>,
    source_roots: HashMap<(u64, String, String, PathBuf), PathBuf>,
}

struct DiscoveredSources {
    environment: Vec<(String, String)>,
    launcher: Option<String>,
    sources: Vec<Source>,
}
impl StatisticsCache {
    /// Resolve defaults on this Host, retaining failed observations only while
    /// both its environment and this agent's explicit launcher remain the same.
    /// SourceBindings still applies removals after discovery, before scan.
    pub fn discover(
        &mut self,
        host: &dyn Host,
        agent: &str,
        launch: &HashMap<String, String>,
    ) -> io::Result<Vec<Source>> {
        let mut environment: Vec<_> = host.agent_environment()?.into_iter().collect();
        environment.sort();
        let launcher = launch.get(agent).cloned();
        let key = (host.id().0, agent.to_owned());
        let previous = self
            .discovery
            .get(&key)
            .filter(|old| old.environment == environment && old.launcher == launcher);
        let discovered = match agent {
            "omp" => discover_omp(host, launch),
            "pi" => discover_pi(host, launch),
            "kimi" => discover_kimi(host, launch),
            "hermes" => discover_hermes(host, launch),
            "opencode" => discover_opencode(host, launch),
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "unsupported statistics agent",
                ));
            }
        };
        let mut found = match discovered {
            Ok(found) => found,
            Err(error) => {
                let Some(previous) = previous else {
                    return Err(error);
                };
                return Ok(previous
                    .sources
                    .iter()
                    .cloned()
                    .map(|mut source| {
                        source.discovery_issue =
                            Some(format!("{agent} source discovery failed: {error}"));
                        source
                    })
                    .collect());
            }
        };
        if let Some(issue) = found
            .iter()
            .find_map(|source| source.discovery_issue.as_ref())
            .cloned()
        {
            if let Some(previous) = previous {
                for old in &previous.sources {
                    if found.iter().any(|source| source.scope == old.scope)
                        && !found
                            .iter()
                            .any(|source| source.root == old.root && source.scope == old.scope)
                    {
                        let mut failed = old.clone();
                        failed.discovery_issue = Some(issue.clone());
                        found.push(failed);
                    }
                }
            }
        }
        self.discovery.insert(
            key,
            DiscoveredSources {
                environment,
                launcher,
                sources: found.clone(),
            },
        );
        Ok(found)
    }
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
    // Resolve selected filesystem roots, never paths supplied by history records.
    // Retain only identities observed for this same Host, scope and input root.
    let original_sources = sources;
    let host_id = host.id().0;
    cache.source_roots.retain(|(id, agent, scope, root), _| {
        *id != host_id
            || !sources
                .iter()
                .any(|source| source.agent == *agent && source.scope == *scope)
            || sources.iter().any(|source| {
                source.agent == *agent && source.scope == *scope && source.root == *root
            })
    });
    let mut normalized_sources: Option<Vec<Source>> = None;
    for (ix, source) in sources.iter().enumerate() {
        if !matches!(source.agent.as_str(), "omp" | "pi" | "kimi") {
            continue;
        }
        let (root, issue) = match host.canonicalize(&source.root) {
            Ok(root) if root.as_os_str() == source.root.as_os_str() => {
                cache.source_roots.retain(|(id, agent, scope, input), _| {
                    *id != host_id
                        || *agent != source.agent
                        || *scope != source.scope
                        || *input != source.root
                });
                continue;
            }
            Ok(root) => {
                cache.source_roots.insert(
                    (
                        host_id,
                        source.agent.clone(),
                        source.scope.clone(),
                        source.root.clone(),
                    ),
                    root.clone(),
                );
                (root, None)
            }
            Err(error) => {
                let Some(root) =
                    cache
                        .source_roots
                        .iter()
                        .find_map(|((id, agent, scope, input), root)| {
                            (*id == host_id
                                && *agent == source.agent
                                && *scope == source.scope
                                && *input == source.root)
                                .then(|| root.clone())
                        })
                else {
                    continue;
                };
                (
                    root,
                    Some(format!(
                        "previously readable statistics source is unavailable: {error}; retaining stale facts"
                    )),
                )
            }
        };
        let normalized = normalized_sources.get_or_insert_with(|| sources.to_vec());
        normalized[ix].root = root;
        if let Some(issue) = issue {
            normalized[ix].discovery_issue.get_or_insert(issue);
        }
    }
    let sources = normalized_sources.as_deref().unwrap_or(sources);
    let mut snapshot = Snapshot {
        host: host_id,
        sources: sources
            .iter()
            .cloned()
            .map(|mut source| {
                source.opencode_request = None;
                source
            })
            .collect(),
        updated: chrono::Utc::now().timestamp(),
        files: Vec::new(),
        issues: Vec::new(),
        pricing: pricing::Pricing::default(),
    };
    let mut seen = HashSet::new();
    let mut dirs = 0;
    let mut files = 0;
    let mut bytes = 0;
    for source in sources {
        if let Some(reason) = &source.discovery_issue {
            snapshot = snapshot.with_issue(source.root.clone(), reason.clone());
            if let Some(facts) =
                cache
                    .native
                    .get(&(host.id().0, source.agent.clone(), source.scope.clone()))
            {
                snapshot.files.extend(
                    facts
                        .iter()
                        .filter(|fact| fact.source.root == source.root)
                        .cloned(),
                );
            }
            if source.agent == "opencode" {
                if let Some((old, facts, _)) =
                    cache.opencode.get(&(host.id().0, source.scope.clone()))
                {
                    if old.root == source.root {
                        snapshot.files.extend(facts.iter().cloned());
                    }
                }
            }
            continue;
        }
        if source.agent == "hermes" {
            hermes::scan_source(host, source, cache, &mut snapshot);
            continue;
        }
        if source.agent == "opencode" {
            opencode::scan_source(host, source, &mut snapshot, cache);
            continue;
        }
        if source.agent == "kimi" {
            kimi::scan_source(
                host,
                source,
                cache,
                &mut snapshot,
                &mut seen,
                &mut dirs,
                &mut files,
                &mut bytes,
            );
            continue;
        }
        if source.agent != "omp" && source.agent != "pi" {
            snapshot.issues.push(ScanIssue {
                path: source.root.clone(),
                reason: format!("unsupported native statistics agent: {}", source.agent),
            });
            continue;
        }
        let metadata = host.stat(&source.root);
        if metadata
            .as_ref()
            .is_err_and(|error| error.kind() == io::ErrorKind::NotFound)
            && cache
                .files
                .iter()
                .any(|((id, agent, scope, _), (_, facts))| {
                    *id == host.id().0
                        && *agent == source.agent
                        && *scope == source.scope
                        && facts.source.root == source.root
                })
        {
            snapshot.issues.push(ScanIssue {
                path: source.root.clone(),
                reason: "previously readable statistics source is missing; retaining stale facts"
                    .into(),
            });
            continue;
        }
        let exact_file = metadata.is_ok_and(|m| !m.is_dir);
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
        let root_is_child = source.agent == "omp"
            && host
                .stat(&PathBuf::from(format!(
                    "{}.jsonl",
                    child_root.to_string_lossy()
                )))
                .is_ok_and(|m| !m.is_dir);
        let mut pending = vec![(initial, root_is_child, 0u8)];
        while let Some((path, child, depth)) = pending.pop() {
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
                            .iter()
                            .any(|((id, agent, scope, _), (_, facts))| {
                                *id == host.id().0
                                    && *agent == source.agent
                                    && *scope == source.scope
                                    && facts.source.root == source.root
                            }) =>
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
                    if source.max_depth.is_some_and(|limit| depth >= limit) {
                        continue;
                    }
                    let sibling = PathBuf::from(format!("{}.jsonl", path.to_string_lossy()));
                    let nested = source.agent == "omp"
                        && (child || host.stat(&sibling).is_ok_and(|m| !m.is_dir));
                    pending.push((path, nested, depth.saturating_add(1)));
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
                if !seen.insert((source.agent.clone(), source.scope.clone(), path.clone())) {
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
                let key = (
                    host.id().0,
                    source.agent.clone(),
                    source.scope.clone(),
                    path.clone(),
                );
                let result = (|| {
                    let meta = host.stat(&path)?;
                    if let Some((old, facts)) = cache.files.get_mut(&key) {
                        if *old == meta {
                            // Directory lineage can change without touching this history file.
                            facts.child = child;
                            snapshot.files.push(facts.clone());
                            return Ok(None);
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
                    let session = match source.agent.as_str() {
                        "omp" => omp::parse(&data)?,
                        "pi" => pi::parse(&data)?,
                        _ => {
                            return Err(io::Error::new(
                                io::ErrorKind::Unsupported,
                                "unsupported native statistics agent",
                            ));
                        }
                    };
                    Ok(Some((
                        meta,
                        FileFacts {
                            source: source.clone(),
                            path: path.clone(),
                            child,
                            observed: snapshot.updated,
                            session,
                            metadata_stamp: None,
                        },
                    )))
                })();
                match result {
                    Ok(Some((meta, facts))) => {
                        cache.files.insert(key, (meta, facts.clone()));
                        snapshot.files.push(facts);
                    }
                    Ok(None) => {}
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
        for ((host_id, agent, scope, path), (_, facts)) in &cache.files {
            if *host_id == host.id().0
                && sources
                    .iter()
                    .any(|s| s.agent == *agent && s.scope == *scope && s.root == facts.source.root)
                && snapshot
                    .issues
                    .iter()
                    .any(|issue| path.starts_with(&issue.path) || issue.path == facts.source.root)
                && !snapshot.files.iter().any(|f| {
                    f.path == *path && f.source.agent == *agent && f.source.scope == *scope
                })
            {
                snapshot.files.push(facts.clone());
            }
        }
    }
    pi::resolve_inheritance(&mut snapshot, host, original_sources);
    // Restrict lineage claims rather than silently treating an unresolved parent as reliable.
    for file in &snapshot.files {
        if let Some(parent) = &file.session.parent {
            if !snapshot.files.iter().any(|f| {
                f.source.agent == file.source.agent
                    && f.source.scope == file.source.scope
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
        cache.files.retain(|(id, agent, scope, path), _| {
            *id != host.id().0
                || !sources
                    .iter()
                    .any(|s| s.agent == *agent && s.scope == *scope)
                || seen.contains(&(agent.clone(), scope.clone(), path.clone()))
        });
    }
    Ok(snapshot)
}
#[cfg(test)]
mod tests;
