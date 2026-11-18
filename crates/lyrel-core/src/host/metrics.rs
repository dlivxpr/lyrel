//! Whole-host sampling owned by the daemon's native Host, not a terminal/GUI.
//!
//! The wire values live in `lyrel-protocol`; this crate owns the sampler and
//! injects it into the full local Host at construction. Requests share one
//! sampler per server process. Only the CPU and RAM APIs are refreshed;
//! process enumeration is never enabled. A request worker performs the
//! blocking platform work at most once a second, without a sampling thread
//! or polling work when no client wants metrics.
use std::io;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use sysinfo::{MemoryRefreshKind, System};

pub use lyrel_protocol::host::metrics::*;

const SAMPLE_INTERVAL: Duration = Duration::from_secs(1);
const IDLE_RESET: Duration = Duration::from_secs(10);

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
        let mut system = System::new().map_err(system_error)?;
        let hostname = System::host_name().map_err(system_error)?;
        if hostname.is_empty() {
            return Err(io::Error::other("host name is unavailable"));
        }
        // The first CPU refresh is a baseline, not a measurement of current use.
        system.refresh_cpu_usage();
        *shared = Some(Sampler {
            system,
            snapshot: HostMetrics::new(
                hostname,
                lyrel_protocol::daemon::control::server_instance(),
                MetricsState::Waiting,
            ),
            refreshed: now,
        });
    }
    let sampler = shared
        .as_mut()
        .ok_or_else(|| io::Error::other("whole-host sampler is unavailable"))?;
    if now.duration_since(sampler.refreshed) >= SAMPLE_INTERVAL {
        sampler.system.refresh_cpu_usage();
        sampler
            .system
            .refresh_memory_specifics(MemoryRefreshKind::nothing().with_ram());
        let cpu = sampler.system.global_cpu_usage();
        let total = sampler.system.total_memory();
        let used = sampler.system.used_memory();
        // sysinfo 0.40.1 refresh methods return (), so validate the exposed data.
        // Unsupported platforms already fail System::new() instead of yielding zeros.
        sampler.snapshot.state = if sampler.system.cpus().is_empty()
            || total == 0
            || used > total
            || !cpu.is_finite()
            || !(0.0..=100.0).contains(&cpu)
        {
            MetricsState::Unavailable(
                "the host OS did not return valid whole-machine CPU/RAM data".into(),
            )
        } else {
            MetricsState::Sample(HostSample::new(
                cpu,
                used,
                total,
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis()
                    .min(u128::from(u64::MAX)) as u64,
            ))
        };
        sampler.refreshed = now;
    }
    Ok(sampler.snapshot.clone())
}

fn system_error(error: sysinfo::Error) -> io::Error {
    let kind = match &error {
        sysinfo::Error::Unsupported => io::ErrorKind::Unsupported,
        sysinfo::Error::Io(error) => error.kind(),
        sysinfo::Error::Other(_) => io::ErrorKind::Other,
    };
    io::Error::new(kind, error)
}
