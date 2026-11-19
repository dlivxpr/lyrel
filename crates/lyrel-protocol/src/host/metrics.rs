//! Whole-host metrics wire values.
//!
//! The sampler itself — the `sysinfo`-backed platform work — lives in
//! `lyrel-core`, which injects it into the local Host at construction; this
//! module owns only the values that cross the wire, so a peer and its client
//! read one schema. Requests share one sampler per server process: only the
//! CPU and RAM APIs are refreshed; process enumeration is never enabled. A
//! request worker performs the blocking platform work at most once a second,
//! without a sampling thread or polling work when no client wants metrics.
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HostMetrics {
    hostname: String,
    instance: String,
    state: MetricsState,
}

impl HostMetrics {
    /// One snapshot stamped by the owning sampler (`lyrel-core` injects it
    /// into the full local Host). Fields stay private so the only way to make
    /// a sample is to actually sample.
    pub fn new(
        hostname: impl Into<String>,
        instance: impl Into<String>,
        state: MetricsState,
    ) -> Self {
        Self {
            hostname: hostname.into(),
            instance: instance.into(),
            state,
        }
    }

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

    /// Replace the reading without changing its host or server instance stamp.
    pub fn set_state(&mut self, state: MetricsState) {
        self.state = state;
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
    /// One validated sample from the owning sampler.
    pub fn new(
        cpu_percent: f32,
        memory_used_bytes: u64,
        memory_total_bytes: u64,
        sampled_at_millis: u64,
    ) -> Self {
        Self {
            cpu_percent,
            memory_used_bytes,
            memory_total_bytes,
            sampled_at_millis,
        }
    }

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
