//! 与总览共用唯一去重流的日历及来源分解；时间边界由桌面调用方提供。
use super::*;
use chrono::{DateTime, Days, NaiveDate, TimeZone};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Metric {
    #[default]
    Tokens,
    Cost,
    Sessions,
}
impl Metric {
    /// Unknown and partial values never participate in intensity or ranking.
    pub fn value(self, totals: &Totals) -> Option<f64> {
        if !totals.is_complete() {
            return None;
        }
        match self {
            Self::Tokens => {
                if totals.calls() == 0 {
                    Some(0.)
                } else {
                    totals.tokens().total().map(|v| v as f64)
                }
            }
            Self::Cost => {
                if totals.unpriced_calls() > 0 || totals.estimated_usd().is_some() {
                    None
                } else {
                    Some(totals.recorded_usd().unwrap_or(0.))
                }
            }
            Self::Sessions => Some(totals.main_sessions() as f64),
        }
    }
}
#[derive(Clone, Debug)]
pub struct Day {
    date: NaiveDate,
    in_range: bool,
    totals: Totals,
}
impl Day {
    pub fn date(&self) -> NaiveDate {
        self.date
    }
    pub fn is_in_range(&self) -> bool {
        self.in_range
    }
    pub fn totals(&self) -> &Totals {
        &self.totals
    }
    pub fn value(&self, metric: Metric) -> Option<f64> {
        self.in_range.then(|| metric.value(&self.totals)).flatten()
    }
}
#[derive(Clone, Debug)]
pub struct Breakdown {
    label: Option<String>,
    agent: String,
    scope: String,
    provider: Option<String>,
    totals: Totals,
}
impl Breakdown {
    pub fn label(&self) -> Option<&str> {
        self.label.as_deref()
    }
    pub fn agent(&self) -> &str {
        &self.agent
    }
    pub fn scope(&self) -> &str {
        &self.scope
    }
    pub fn provider(&self) -> Option<&str> {
        self.provider.as_deref()
    }
    pub fn totals(&self) -> &Totals {
        &self.totals
    }
}
#[derive(Clone, Debug, Default)]
pub struct Streaks {
    current: Option<u32>,
    longest: Option<u32>,
}
impl Streaks {
    pub fn current(&self) -> Option<u32> {
        self.current
    }
    pub fn longest(&self) -> Option<u32> {
        self.longest
    }
}
#[derive(Clone, Debug)]
pub struct Analysis {
    year: i32,
    agent_filter: Option<String>,
    has_sources: bool,
    days: Vec<Day>,
    totals: Totals,
    agents: Vec<Breakdown>,
    models: Vec<Breakdown>,
    projects: Vec<Breakdown>,
    streaks: Streaks,
}
impl Analysis {
    pub fn agent_filter(&self) -> Option<&str> {
        self.agent_filter.as_deref()
    }
    pub fn has_sources(&self) -> bool {
        self.has_sources
    }
    pub fn year(&self) -> i32 {
        self.year
    }
    pub fn days(&self) -> &[Day] {
        &self.days
    }
    pub fn totals(&self) -> &Totals {
        &self.totals
    }
    pub fn agents(&self) -> &[Breakdown] {
        &self.agents
    }
    pub fn models(&self) -> &[Breakdown] {
        &self.models
    }
    pub fn projects(&self) -> &[Breakdown] {
        &self.projects
    }
    pub fn streaks(&self) -> &Streaks {
        &self.streaks
    }
    pub fn maximum(&self, metric: Metric) -> f64 {
        self.days
            .iter()
            .filter_map(|d| d.value(metric))
            .fold(0., f64::max)
    }
}

type GroupKey<'a> = (&'a str, &'a str, Option<&'a str>, Option<&'a str>);
#[derive(Default)]
struct Group<'a> {
    totals: Totals,
    sessions: HashSet<&'a str>,
}
fn group_key<'a>(
    file: &'a FileFacts,
    label: Option<&'a str>,
    provider: Option<&'a str>,
) -> GroupKey<'a> {
    (
        file.source.agent.as_str(),
        file.source.scope.as_str(),
        provider.filter(|s| !s.is_empty()),
        label.filter(|s| !s.is_empty()),
    )
}
fn main_identity(file: &FileFacts) -> &str {
    &file.session.id
}
fn finish(groups: BTreeMap<GroupKey<'_>, Group<'_>>, complete: bool) -> Vec<Breakdown> {
    let mut rows: Vec<_> = groups
        .into_iter()
        .map(|((agent, scope, provider, label), mut group)| {
            group.totals.complete &= complete;
            group.totals.main_sessions = group.sessions.len() as u64;
            Breakdown {
                label: label.map(str::to_owned),
                agent: agent.to_owned(),
                scope: scope.to_owned(),
                provider: provider.map(str::to_owned),
                totals: group.totals,
            }
        })
        .collect();
    rows.sort_by(|a, b| {
        let value = |row: &Breakdown| Metric::Tokens.value(&row.totals);
        match (value(a), value(b)) {
            (Some(a), Some(b)) => b.total_cmp(&a),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => std::cmp::Ordering::Equal,
        }
    });
    rows
}
impl Snapshot {
    /// `now` and its timezone belong to the desktop, never to the scanned Host.
    /// The calendar covers `year`; totals/breakdowns cover the full selected window.
    /// Daily sessions and model/project session participation are not additive.
    pub fn analyze<T: TimeZone>(
        &self,
        window: &DateWindow,
        now: DateTime<T>,
        agent: Option<&str>,
        year: i32,
    ) -> Result<Analysis, String>
    where
        T::Offset: std::fmt::Display,
    {
        let first = NaiveDate::from_ymd_opt(year, 1, 1).ok_or("invalid calendar year")?;
        let next =
            NaiveDate::from_ymd_opt(year.checked_add(1).ok_or("invalid calendar year")?, 1, 1)
                .ok_or("invalid calendar year")?;
        let count = (next - first).num_days() as usize;
        let mut boundaries = Vec::with_capacity(count);
        let mut days = Vec::with_capacity(count);
        for ix in 0..count {
            let date = first
                .checked_add_days(Days::new(ix as u64))
                .ok_or("invalid date")?;
            let day_window = DateWindow::calendar(Period::Custom, now.clone(), Some((date, date)))?;
            let in_range = window.start().is_none_or(|s| day_window.end().unwrap() > s)
                && window.end().is_none_or(|e| day_window.start().unwrap() < e);
            days.push(Day {
                date,
                in_range,
                totals: Totals::default(),
            });
            boundaries.push(day_window);
        }
        let mut totals = Totals::default();
        let mut agents = BTreeMap::<GroupKey, Group>::new();
        let mut models = BTreeMap::<GroupKey, Group>::new();
        let mut projects = BTreeMap::<GroupKey, Group>::new();
        let mut sessions = Vec::new();
        let ambiguous = self.visit_usage(
            agent,
            |file| sessions.push(file),
            |file, record| {
                let keys = [
                    group_key(file, Some(&file.source.agent), None),
                    group_key(
                        file,
                        (record.coverage_start.is_none() && !record.time_unknown)
                            .then_some(record.model.as_str()),
                        Some(&record.provider),
                    ),
                    group_key(file, file.session.project.as_deref(), None),
                ];
                if record.covered_by(window) {
                    totals.add_record(record);
                    for (groups, key) in [&mut agents, &mut models, &mut projects]
                        .into_iter()
                        .zip(keys)
                    {
                        let group = groups.entry(key).or_default();
                        group.totals.add_record(record);
                        if !file.child {
                            group.sessions.insert(main_identity(file));
                        }
                    }
                } else if record.overlaps(window) {
                    totals.complete = false;
                    for (groups, key) in [&mut agents, &mut models, &mut projects]
                        .into_iter()
                        .zip(keys)
                    {
                        groups.entry(key).or_default().totals.complete = false;
                    }
                }
                // Cumulative intervals are never assigned wholly to their last-active day.
                if record.time_unknown {
                    for day in days.iter_mut().filter(|d| d.in_range) {
                        day.totals.complete = false;
                    }
                    return;
                }
                let start = boundaries.partition_point(|w| {
                    w.end().unwrap() <= record.coverage_start.unwrap_or(record.timestamp)
                });
                let end = boundaries.partition_point(|w| w.start().unwrap() <= record.timestamp);
                for (day, boundary) in days[start..end].iter_mut().zip(&boundaries[start..end]) {
                    if !day.in_range || !record.overlaps(window) {
                        continue;
                    }
                    if record.covered_by(boundary) && record.covered_by(window) {
                        day.totals.add_record(record);
                    } else {
                        day.totals.complete = false;
                    }
                }
            },
        );
        for file in sessions {
            if !file.active_in(window) {
                continue;
            }
            totals.main_sessions += 1;
            let identity = main_identity(file);
            agents
                .entry(group_key(file, Some(&file.source.agent), None))
                .or_default()
                .sessions
                .insert(identity);
            projects
                .entry(group_key(file, file.session.project.as_deref(), None))
                .or_default()
                .sessions
                .insert(identity);
            let mut active_days = HashSet::new();
            for timestamp in file.activity_timestamps() {
                if !window.contains(timestamp) {
                    continue;
                }
                let ix = boundaries.partition_point(|w| w.end().unwrap() <= timestamp);
                if ix < days.len() && boundaries[ix].contains(timestamp) {
                    active_days.insert(ix);
                }
            }
            for ix in active_days {
                days[ix].totals.main_sessions += 1;
            }
        }
        let complete = self.is_complete()
            && ambiguous == 0
            && self
                .sources
                .iter()
                .any(|source| agent.is_none_or(|agent| agent == source.agent));
        totals.complete &= complete;
        totals.ambiguous_records = ambiguous;
        for day in &mut days {
            day.totals.complete &= complete;
        }
        let mut streaks = Streaks::default();
        if days
            .iter()
            .any(|d| d.in_range && d.date <= now.date_naive())
            && days
                .iter()
                .filter(|d| d.in_range && d.date <= now.date_naive())
                .all(|d| d.totals.is_complete())
        {
            let active =
                |d: &Day| d.in_range && (d.totals.main_sessions() > 0 || d.totals.calls() > 0);
            let mut run = 0;
            let mut longest = 0;
            for day in days.iter().filter(|d| d.date <= now.date_naive()) {
                run = if active(day) { run + 1 } else { 0 };
                longest = longest.max(run);
            }
            streaks.longest = Some(longest);
            if let Some(ix) = days
                .iter()
                .position(|d| d.date == now.date_naive() && d.in_range)
            {
                let end = if active(&days[ix]) { ix + 1 } else { ix };
                streaks.current =
                    Some(days[..end].iter().rev().take_while(|d| active(d)).count() as u32);
            }
        }
        Ok(Analysis {
            agent_filter: agent.map(str::to_owned),
            has_sources: self
                .sources
                .iter()
                .any(|source| agent.is_none_or(|agent| agent == source.agent)),
            year,
            days,
            totals,
            agents: finish(agents, complete),
            models: finish(models, complete),
            projects: finish(projects, complete),
            streaks,
        })
    }
}
