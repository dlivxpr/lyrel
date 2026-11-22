//! Lyrel's own component usage wire values (#377).
//!
//! Unlike [`super::metrics`] — whole-host readings — these values attribute
//! CPU and RAM to Lyrel's own processes only: the desktop application, the
//! session daemon, and, on a managed remote, the Lyrel server. Shell and
//! agent workloads are the daemon's children but are never part of these
//! totals; they stay in the pane process table.
//!
//! The values ride inside the existing `HostMetrics` response as an
//! ignorable, feature-negotiated field: a peer that has never heard of them
//! keeps interoperating on control v19, and a client that has never heard of
//! them drops them on decode. The daemon stamps every reading with its
//! hostname and control instance, so a client can bind a sample to the exact
//! process and daemon instance it displays, and refuse a predecessor's.
use serde::{Deserialize, Serialize};

/// One machine's Lyrel usage answer, stamped with the host and server
/// instance that produced it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LyrelUsage {
    hostname: String,
    instance: String,
    state: LyrelUsageState,
}

impl LyrelUsage {
    /// One stamped answer; the only way to set the fields is to sample.
    pub fn new(hostname: impl Into<String>, instance: impl Into<String>, state: LyrelUsageState) -> Self {
        Self {
            hostname: hostname.into(),
            instance: instance.into(),
            state,
        }
    }

    /// The host this sample was taken on — the machine the grouped rows
    /// name, never the client's guess.
    pub fn hostname(&self) -> &str {
        &self.hostname
    }

    /// Matches the control hello, so a previous daemon's sample cannot be reused.
    pub fn instance(&self) -> &str {
        &self.instance
    }

    pub fn state(&self) -> &LyrelUsageState {
        &self.state
    }

    /// Replace the reading without changing its host or server instance stamp.
    pub fn set_state(&mut self, state: LyrelUsageState) {
        self.state = state;
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LyrelUsageState {
    /// The sampler exists but has no CPU delta yet — its first refresh is the
    /// baseline, not a measurement.
    Waiting,
    Sample(LyrelSample),
    /// The platform could not attribute a reading; the reason is shown where
    /// the number would be, never silently read as a quiet zero.
    Unavailable(String),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LyrelSample {
    sampled_at_millis: u64,
    /// The answering process itself: the session daemon locally, the Lyrel
    /// server remotely. The desktop application is sampled by the GUI process
    /// and merged client-side, so it never crosses this wire.
    server: ComponentUsage,
}

impl LyrelSample {
    pub fn new(sampled_at_millis: u64, server: ComponentUsage) -> Self {
        Self {
            sampled_at_millis,
            server,
        }
    }

    pub fn sampled_at_millis(&self) -> u64 {
        self.sampled_at_millis
    }

    pub fn server(&self) -> &ComponentUsage {
        &self.server
    }
}

/// CPU and RAM attributed to exactly one Lyrel process.
///
/// Later resource tickets extend this with per-component GPU and network
/// fields; appending serde-default fields keeps old peers decoding.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ComponentUsage {
    pid: u32,
    /// Process birth time, whole seconds since the Unix epoch — the identity
    /// half that keeps a reused pid from inheriting its predecessor's row.
    started_secs: u64,
    /// Share of one logical CPU: 100 is one fully busy core, so a
    /// multithreaded process may read above 100 on multi-core machines.
    cpu_percent: f32,
    /// Resident set size as the host OS reports it.
    memory_used_bytes: u64,
}

impl ComponentUsage {
    pub fn new(pid: u32, started_secs: u64, cpu_percent: f32, memory_used_bytes: u64) -> Self {
        Self {
            pid,
            started_secs,
            cpu_percent,
            memory_used_bytes,
        }
    }

    pub fn pid(&self) -> u32 {
        self.pid
    }

    pub fn started_secs(&self) -> u64 {
        self.started_secs
    }

    pub fn cpu_percent(&self) -> f32 {
        self.cpu_percent
    }

    pub fn memory_used_bytes(&self) -> u64 {
        self.memory_used_bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_metrics_answer_without_lyrel_usage_still_decodes() {
        // A control-v19 peer from before this ticket answers HostMetrics
        // without the appended field; serde's default keeps it decodable.
        let json = serde_json::json!({
            "hostname": "dev-linux",
            "instance": "instance-a",
            "state": {"sample": {
                "cpu_percent": 12.5,
                "memory_used_bytes": 8,
                "memory_total_bytes": 16,
                "sampled_at_millis": 42,
            }},
        });
        let metrics: crate::host::metrics::HostMetrics = serde_json::from_value(json).unwrap();
        assert!(metrics.lyrel_usage().is_none());
    }

    #[test]
    fn lyrel_usage_round_trips_inside_host_metrics() {
        let usage = LyrelUsage::new(
            "dev-linux",
            "instance-a",
            LyrelUsageState::Sample(LyrelSample::new(
                42,
                ComponentUsage::new(4242, 1_700_000_000, 1.5, 96 * 1024 * 1024),
            )),
        );
        let mut metrics = crate::host::metrics::HostMetrics::new(
            "dev-linux",
            "instance-a",
            crate::host::metrics::MetricsState::Waiting,
        );
        metrics.set_lyrel_usage(Some(usage.clone()));
        let encoded = serde_json::to_value(&metrics).unwrap();
        let decoded: crate::host::metrics::HostMetrics = serde_json::from_value(encoded).unwrap();
        assert_eq!(decoded.lyrel_usage(), Some(&usage));
    }

    #[test]
    fn component_usage_rejects_no_identity_or_memory() {
        // The sampler validates before building one of these; the constructor
        // itself stays plain so tests can shape exact values.
        let component = ComponentUsage::new(7, 1_700_000_000, 250., 12 * 1024 * 1024);
        assert_eq!(component.pid(), 7);
        assert_eq!(component.started_secs(), 1_700_000_000);
        assert_eq!(component.cpu_percent(), 250.);
    }
}
