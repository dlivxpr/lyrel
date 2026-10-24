//! Whole-host sampling owned by the daemon's native Host, not a terminal/GUI.
//!
//! Requests share one sampler per server process. Only the CPU and RAM APIs are
//! refreshed; process enumeration is never enabled. A request worker performs
//! the blocking platform work at most once a second, without a sampling thread
//! or polling work when no client wants metrics.
use std::io;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sysinfo::{MemoryRefreshKind, System};

const SAMPLE_INTERVAL: Duration = Duration::from_secs(1);
const IDLE_RESET: Duration = Duration::from_secs(10);

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HostMetrics {
    hostname: String,
    instance: String,
    state: MetricsState,
}

impl HostMetrics {
    pub fn hostname(&self) -> &str {
        &self.hostname
    }
    /// Matches the control hello, so a previous daemon's sample cannot be reused.
    pub fn instance(&self) -> &str {
        &self.instance
    }
    pub fn state(&self) -> &MetricsState {
        &self.state
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MetricsState {
    Waiting,
    Sample(HostSample),
    Unavailable(String),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HostSample {
    cpu_percent: f32,
    memory_used_bytes: u64,
    memory_total_bytes: u64,
    sampled_at_millis: u64,
}

impl HostSample {
    /// Whole-machine busy time, normalized to 0–100 across all logical CPUs.
    pub fn cpu_percent(&self) -> f32 {
        self.cpu_percent
    }
    /// Physical RAM in use according to the host OS; excludes swap.
    pub fn memory_used_bytes(&self) -> u64 {
        self.memory_used_bytes
    }
    pub fn memory_total_bytes(&self) -> u64 {
        self.memory_total_bytes
    }
    pub fn sampled_at_millis(&self) -> u64 {
        self.sampled_at_millis
    }
    pub fn memory_percent(&self) -> f64 {
        self.memory_used_bytes as f64 * 100.0 / self.memory_total_bytes as f64
    }
}

struct Sampler {
    system: System,
    snapshot: HostMetrics,
    refreshed: Instant,
}

pub(crate) fn sample_local() -> io::Result<HostMetrics> {
    static SAMPLER: LazyLock<Mutex<Option<Sampler>>> = LazyLock::new(|| Mutex::new(None));
    let mut shared = SAMPLER
        .lock()
        .map_err(|_| io::Error::other("whole-host sampler failed"))?;
    let now = Instant::now();
    if shared
        .as_ref()
        .is_none_or(|sampler| now.duration_since(sampler.refreshed) > IDLE_RESET)
    {
        let hostname = System::host_name()
            .filter(|name| !name.is_empty())
            .ok_or_else(|| io::Error::other("host name is unavailable"))?;
        let mut system = System::new();
        // The first CPU refresh is a baseline, not a measurement of current use.
        system.refresh_cpu_usage();
        *shared = Some(Sampler {
            system,
            snapshot: HostMetrics {
                hostname,
                instance: crate::daemon::control::server_instance().to_owned(),
                state: MetricsState::Waiting,
            },
            refreshed: now,
        });
    }
    let sampler = shared.as_mut().expect("initialized above");
    if now.duration_since(sampler.refreshed) >= SAMPLE_INTERVAL {
        sampler.system.refresh_cpu_usage();
        sampler
            .system
            .refresh_memory_specifics(MemoryRefreshKind::new().with_ram());
        let cpu = sampler.system.global_cpu_usage();
        let total = sampler.system.total_memory();
        let used = sampler.system.used_memory();
        sampler.snapshot.state = if !sysinfo::IS_SUPPORTED_SYSTEM
            || sampler.system.cpus().is_empty()
            || total == 0
            || used > total
            || !cpu.is_finite()
            || !(0.0..=100.0).contains(&cpu)
        {
            MetricsState::Unavailable(
                "the host OS did not return valid whole-machine CPU/RAM data".into(),
            )
        } else {
            MetricsState::Sample(HostSample {
                cpu_percent: cpu,
                memory_used_bytes: used,
                memory_total_bytes: total,
                sampled_at_millis: SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis()
                    .min(u128::from(u64::MAX)) as u64,
            })
        };
        sampler.refreshed = now;
    }
    Ok(sampler.snapshot.clone())
}
