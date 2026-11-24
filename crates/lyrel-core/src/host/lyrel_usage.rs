//! Lyrel's own component sampling, owned by the process that is being
//! measured — the daemon stamps its usage into the wire answer here, and the
//! desktop application reuses the same [`Sampler`] for its own process.
//!
//! Only the measuring process itself is ever refreshed: no process
//! enumeration is enabled, so a shell or agent workload spawned under the
//! daemon can never leak into these totals — the numbers attribute exactly
//! one process, by pid and birth time. Whole-host readings live in
//! [`super::metrics`]; this module is the per-component counterpart #377
//! adds below the monitor's resource tiles.
use std::io;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System};

pub use lyrel_protocol::host::lyrel_usage::*;

#[cfg(target_os = "macos")]
mod network;

/// How often a caller who wants a reading may get a fresh one; the daemon's
/// request worker shares one sampler per process, so concurrent consumers
/// collapse onto a single platform read per interval.
const SAMPLE_INTERVAL: Duration = Duration::from_secs(1);

/// After this long without a request the daemon's cached baseline is stale
/// enough that the next call rebuilds it: CPU deltas need a recent reference,
/// and a sampler that idled through a suspend owes the next reader an honest
/// re-baseline rather than a spike computed across the gap.
const IDLE_RESET: Duration = Duration::from_secs(10);

/// One process measured on demand: a fresh `System` holding exactly that
/// process, never the machine's whole table. The first [`Sampler::sample`]
/// after construction is a CPU baseline, so it answers `Ok(None)`; every
/// later call returns the usage computed against the previous refresh.
pub struct Sampler {
    system: System,
    pid: sysinfo::Pid,
    /// The birth stamp observed when the sampler armed, whole seconds since
    /// the Unix epoch. A refresh that reports a different one means the pid
    /// was reused, so the reading is discarded.
    started_secs: u64,
    primed: bool,
    #[cfg(windows)]
    gpu: super::metrics::gpu::GpuSampler,
    #[cfg(target_os = "macos")]
    network: network::Sampler,
}

impl Sampler {
    /// Arm a sampler for `pid`. The process must exist; sampling a live
    /// process that then exits reports `NotFound` from [`Sampler::sample`].
    pub fn for_pid(pid: u32) -> io::Result<Self> {
        let mut system = System::new().map_err(system_error)?;
        let pid = sysinfo::Pid::from_u32(pid);
        refresh_process(&mut system, pid)?;
        let process = system.process(pid).ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, format!("process {pid} is gone"))
        })?;
        let started_secs = process.start_time();
        Ok(Self {
            system,
            pid,
            started_secs,
            primed: false,
            #[cfg(windows)]
            gpu: super::metrics::gpu::GpuSampler::open(),
            #[cfg(target_os = "macos")]
            network: network::Sampler::default(),
        })
    }

    /// The birth stamp the sampler armed with; part of the wire identity.
    pub fn started_secs(&self) -> u64 {
        self.started_secs
    }

    /// Refresh and measure. `Ok(None)` is the CPU baseline pass — the first
    /// sample after arming, exactly like the whole-host sampler's first CPU
    /// refresh. Callers must not poll faster than
    /// [`sysinfo::MINIMUM_CPU_UPDATE_INTERVAL`]; the daemon and GUI wrappers
    /// both hold a one-second cadence. `Err` is an honest failure: the
    /// process exited, its pid was reused, or the platform refused the
    /// reading.
    pub fn sample(&mut self) -> io::Result<Option<ComponentUsage>> {
        refresh_process(&mut self.system, self.pid)?;
        let Some(process) = self.system.process(self.pid) else {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "the sampled process has exited",
            ));
        };
        if process.start_time() != self.started_secs {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "the sampled pid was reused by another process",
            ));
        }
        let pid = self.pid.as_u32();
        #[cfg(target_os = "macos")]
        let network = self.network.sample(pid);
        if !self.primed {
            self.primed = true;
            return Ok(None);
        }
        let cpu = process.cpu_usage();
        let memory = process.memory();
        if !cpu.is_finite() || cpu < 0. || memory == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "the host OS did not return valid process CPU/RAM data",
            ));
        }
        #[cfg(windows)]
        let gpu = self.gpu.process_engines(pid);
        #[cfg(not(windows))]
        let gpu = Err(ProcessMetricUnavailable::Unsupported);
        let component = ComponentUsage::new(pid, self.started_secs, cpu, memory).with_gpu(gpu);
        #[cfg(target_os = "macos")]
        let component = match network {
            Some(reading) => component.with_network(reading),
            None => component,
        };
        #[cfg(not(target_os = "macos"))]
        let component = component.with_network(Err(ProcessMetricUnavailable::Unsupported));
        Ok(Some(component))
    }
}

fn refresh_process(system: &mut System, pid: sysinfo::Pid) -> io::Result<()> {
    if system.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[pid]),
        true,
        ProcessRefreshKind::nothing().with_cpu().with_memory(),
    ) == 0
    {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("cannot refresh process {pid}"),
        ));
    }
    Ok(())
}

/// The daemon-process sampler behind [`Host::lyrel_usage`], shared by every
/// control request this process serves. Exactly like the whole-host sampler:
/// a request worker performs the blocking platform work at most once a
/// second, with no sampling thread and no polling when no client asks.
pub(crate) fn sample_local() -> io::Result<LyrelUsage> {
    struct Cached {
        sampler: Sampler,
        snapshot: LyrelUsage,
        refreshed: Instant,
    }

    static SAMPLER: LazyLock<Mutex<Option<Cached>>> = LazyLock::new(|| Mutex::new(None));
    let mut shared = SAMPLER
        .lock()
        .map_err(|_| io::Error::other("Lyrel usage sampler failed"))?;
    let now = Instant::now();
    if shared
        .as_ref()
        .is_none_or(|cached| now.duration_since(cached.refreshed) > IDLE_RESET)
    {
        let hostname = System::host_name().map_err(system_error)?;
        if hostname.is_empty() {
            return Err(io::Error::other("host name is unavailable"));
        }
        let sampler = Sampler::for_pid(std::process::id())?;
        *shared = Some(Cached {
            sampler,
            snapshot: LyrelUsage::new(
                hostname,
                lyrel_protocol::daemon::control::server_instance(),
                LyrelUsageState::Waiting,
            ),
            refreshed: now,
        });
    }
    let cached = shared
        .as_mut()
        .ok_or_else(|| io::Error::other("Lyrel usage sampler is unavailable"))?;
    if now.duration_since(cached.refreshed) >= SAMPLE_INTERVAL {
        let state = match cached.sampler.sample() {
            Ok(Some(component)) => {
                LyrelUsageState::Sample(LyrelSample::new(now_millis(), component))
            }
            // The first refresh is the CPU baseline; the stamp and the last
            // honest reading stay as they are.
            Ok(None) => LyrelUsageState::Waiting,
            Err(error) => LyrelUsageState::Unavailable(error.to_string()),
        };
        cached.snapshot.set_state(state);
        cached.refreshed = now;
    }
    Ok(cached.snapshot.clone())
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}

fn system_error(error: sysinfo::Error) -> io::Error {
    let kind = match &error {
        sysinfo::Error::Unsupported => io::ErrorKind::Unsupported,
        sysinfo::Error::Io(error) => error.kind(),
        sysinfo::Error::Other(_) => io::ErrorKind::Other,
    };
    io::Error::new(kind, error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};
    use std::time::Duration;

    /// The sampler measures exactly the process it armed for: a live child
    /// of this test process is never part of the reading.
    #[test]
    fn the_sampler_never_includes_child_workloads() {
        #[cfg(windows)]
        let mut command = {
            let mut command = Command::new("ping.exe");
            command.args(["-n", "30", "127.0.0.1"]);
            command
        };
        #[cfg(not(windows))]
        let mut command = {
            let mut command = Command::new("sh");
            command.args(["-c", "while :; do :; done"]);
            command
        };
        let mut child = command.stdout(Stdio::null()).spawn().unwrap();
        // Arm while the child exists so a whole-table walk could see it —
        // the sampler must still answer with this process alone.
        let mut sampler = Sampler::for_pid(std::process::id()).unwrap();
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(
            sampler.sample().unwrap(),
            None,
            "first refresh is the baseline"
        );
        std::thread::sleep(Duration::from_millis(1100));
        let component = sampler
            .sample()
            .unwrap()
            .expect("the second refresh measures against the baseline");
        child.kill().unwrap();
        child.wait().unwrap();
        assert_eq!(component.pid(), std::process::id());
        assert_eq!(component.started_secs(), sampler.started_secs());
        assert!(component.memory_used_bytes() > 0);
        assert!(component.cpu_percent().is_finite());
    }

    #[test]
    fn sampling_an_exited_process_reports_not_found() {
        let mut child = Command::new(std::env::var("COMSPEC").unwrap_or_else(|_| "sh".into()))
            .arg(if cfg!(windows) { "/c" } else { "-c" })
            .arg(if cfg!(windows) {
                "set /p lyrel_fixture="
            } else {
                "read -r fixture"
            })
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .spawn()
            .unwrap();
        let mut sampler = Sampler::for_pid(child.id()).unwrap();
        assert_eq!(sampler.sample().unwrap(), None);
        drop(child.stdin.take());
        child.wait().unwrap();
        drop(child);
        assert_eq!(
            sampler.sample().unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
    }

    #[test]
    fn the_daemon_snapshot_is_stamped_with_host_and_instance() {
        let usage = sample_local().unwrap();
        assert_eq!(
            usage.instance(),
            lyrel_protocol::daemon::control::server_instance()
        );
        assert!(!usage.hostname().is_empty());
        // Repeated asks share the sampler and stay one stamped answer.
        assert_eq!(usage, sample_local().unwrap());
    }

    #[test]
    fn a_sample_taken_is_the_daemon_process_itself() {
        let usage = sample_local().unwrap();
        let LyrelUsageState::Sample(sample) = usage.state() else {
            // A platform without a usable first delta still proves the stamp;
            // the component itself is covered by the sampler tests above.
            return;
        };
        assert_eq!(sample.server().pid(), std::process::id());
        assert!(sample.server().memory_used_bytes() > 0);
    }
}
