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
    /// Lyrel's own component usage, appended as an ignorable field (#377):
    /// absent from a peer that predates the feature, `None` from one that
    /// cannot attribute it. Never required to read the whole-host values.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    lyrel_usage: Option<super::lyrel_usage::LyrelUsage>,
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
            lyrel_usage: None,
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

    /// Lyrel's own usage riding this answer, when the peer sampled it.
    pub fn lyrel_usage(&self) -> Option<&super::lyrel_usage::LyrelUsage> {
        self.lyrel_usage.as_ref()
    }

    /// Attach (or clear) the Lyrel usage without touching the whole-host
    /// reading or the host/instance stamps.
    pub fn set_lyrel_usage(&mut self, lyrel_usage: Option<super::lyrel_usage::LyrelUsage>) {
        self.lyrel_usage = lyrel_usage;
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
    // Optional wire additions (#374): peers from before GPU sampling read
    // them as absent, and peers without a GPU reading send them unset.
    gpu_percent: Option<f32>,
    gpu_unavailable_reason: Option<String>,
}

impl HostSample {
    /// One validated sample from the owning sampler, without a GPU reading.
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
            gpu_percent: None,
            gpu_unavailable_reason: None,
        }
    }

    /// Attach a validated whole-host GPU utilization reading (0–100).
    pub fn with_gpu(mut self, gpu_percent: f32) -> Self {
        self.gpu_percent = Some(gpu_percent);
        self
    }

    /// Attach the reason this host cannot supply a GPU reading right now.
    pub fn with_gpu_unavailable(mut self, reason: impl Into<String>) -> Self {
        self.gpu_unavailable_reason = Some(reason.into());
        self
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
    /// Busiest-GPU-engine utilization for the whole host, 0–100, when the
    /// platform supplied one. `None` either predates GPU sampling or means
    /// the reason below explains the miss.
    pub fn gpu_percent(&self) -> Option<f32> {
        self.gpu_percent
    }
    /// Why this sample carries no GPU reading, straight from the sampler.
    pub fn gpu_unavailable_reason(&self) -> Option<&str> {
        self.gpu_unavailable_reason.as_deref()
    }
    pub fn memory_percent(&self) -> f64 {
        self.memory_used_bytes as f64 * 100.0 / self.memory_total_bytes as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gpu_fields_are_optional_on_the_wire() {
        // A sample from a daemon that predates GPU sampling still parses,
        // with both readings absent instead of failing on missing fields.
        let legacy: HostSample = serde_json::from_value(serde_json::json!({
            "cpu_percent": 12.5,
            "memory_used_bytes": 8,
            "memory_total_bytes": 16,
            "sampled_at_millis": 1_700_000_000_000u64,
        }))
        .unwrap();
        assert_eq!(legacy.gpu_percent(), None);
        assert_eq!(legacy.gpu_unavailable_reason(), None);

        // The GPU-bearing sample round-trips with its reading and reason.
        let sample = HostSample::new(12.5, 8, 16, 1_700_000_000_000).with_gpu(34.25);
        let encoded = serde_json::to_value(&sample).unwrap();
        let decoded: HostSample = serde_json::from_value(encoded).unwrap();
        assert_eq!(decoded.gpu_percent(), Some(34.25));
        assert_eq!(decoded.gpu_unavailable_reason(), None);

        let sample = HostSample::new(12.5, 8, 16, 1_700_000_000_000)
            .with_gpu_unavailable("no GPU engine counters");
        let decoded: HostSample =
            serde_json::from_value(serde_json::to_value(&sample).unwrap()).unwrap();
        assert_eq!(decoded.gpu_percent(), None);
        assert_eq!(
            decoded.gpu_unavailable_reason(),
            Some("no GPU engine counters")
        );
    }
}
