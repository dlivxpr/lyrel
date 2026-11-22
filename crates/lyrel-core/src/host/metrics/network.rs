//! Network receive/transmit throughput behind the whole-host sample's
//! optional network field (#375).
//!
//! The rate is the delta of per-interface byte counters summed over every
//! interface, divided by the real elapsed time between the two reads — never
//! a nominal interval. Three outcomes stay distinct on the wire:
//!
//! - a genuine idle interval is `Some` zero rates: the counters advanced not
//!   at all over a real elapsed time;
//! - the first read after the sampler (re)built its baseline, and any tick
//!   where a counter moved backwards (interface reset or reboot), are `None`:
//!   there is no trustworthy delta, so no reading is reported;
//! - a platform the counter backend cannot open reports nothing at all, and
//!   the daemon withholds the `host-network-metrics-v1` feature.
//!
//! Loopback is excluded by name and never enters the sums: `lo` on Linux,
//! `lo0` on macOS and the BSDs, and the `Loopback Pseudo-Interface …` entries
//! on Windows all match [`is_loopback`]. Local-only traffic therefore never
//! masquerades as host throughput, and the daemon's own control traffic on
//! `127.0.0.1` cannot inflate the reading.
use std::ffi::OsStr;
use std::time::Instant;

use sysinfo::Networks;

pub use lyrel_protocol::host::metrics::NetworkThroughput;

/// Whether this platform's counter backend can be opened at all. The daemon
/// advertises the network feature only when this holds; on the supported
/// desktops the call is a cheap empty-map allocation, on the `unknown`
/// backend it is the one `Unsupported` error.
pub(crate) fn supported() -> bool {
    Networks::new().is_ok()
}

/// Byte-counter totals across the counted interfaces at one instant.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Counters {
    rx_bytes: u64,
    tx_bytes: u64,
    at: Instant,
}

/// Demand-driven network sampler owned by the whole-host sampler. Holds the
/// counter backend and the previous totals; there is no sampling thread.
pub(crate) struct Sampler {
    networks: Option<Networks>,
    previous: Option<Counters>,
}

impl Sampler {
    /// Open the counter backend and prime the baseline, so the first real
    /// tick measures a genuine interval instead of reporting nothing. A
    /// backend that cannot open samples as permanently missing.
    pub(crate) fn new(now: Instant) -> Self {
        let mut networks = Networks::new().ok();
        if let Some(networks) = &mut networks {
            networks.refresh(true);
        }
        let mut sampler = Self {
            networks,
            previous: None,
        };
        sampler.previous = sampler.totals(now);
        sampler
    }

    /// Refresh the counters and finish one rate pair against the previous
    /// totals. `None` is a missing reading — no baseline yet, a backwards
    /// counter, or no backend — never a quiet interface.
    pub(crate) fn sample(&mut self, now: Instant) -> Option<NetworkThroughput> {
        let networks = self.networks.as_mut()?;
        networks.refresh(true);
        let current = self.totals(now)?;
        let previous = self.previous.replace(current)?;
        rate(&previous, &current)
    }

    /// Sum the counted interfaces' totals. A backend that opened but lists no
    /// interfaces has no counters to delta against and is not a zero reading;
    /// an all-zero sum is genuine (a fresh or loopback-only host is idle).
    fn totals(&self, at: Instant) -> Option<Counters> {
        let networks = self.networks.as_ref()?;
        (!networks.is_empty()).then(|| {
            sum_interfaces(
                networks.iter().map(|(name, data)| {
                    (
                        name.as_os_str(),
                        data.total_received(),
                        data.total_transmitted(),
                    )
                }),
                at,
            )
        })
    }
}

/// Fold a counter listing into summed totals; loopback interfaces never
/// enter the sum. `at` stamps the read for the delta that follows.
fn sum_interfaces<'a>(
    interfaces: impl Iterator<Item = (&'a OsStr, u64, u64)>,
    at: Instant,
) -> Counters {
    let mut counters = Counters {
        rx_bytes: 0,
        tx_bytes: 0,
        at,
    };
    for (name, rx_bytes, tx_bytes) in interfaces {
        if is_loopback(name) {
            continue;
        }
        counters.rx_bytes += rx_bytes;
        counters.tx_bytes += tx_bytes;
    }
    counters
}

/// Finish one rate pair from two counter totals. A backwards counter (reset
/// or reboot) or a non-positive elapsed time has no trustworthy delta and
/// reports nothing; a zero delta over real time is a genuine idle `Some(0)`.
fn rate(previous: &Counters, current: &Counters) -> Option<NetworkThroughput> {
    let elapsed = current
        .at
        .saturating_duration_since(previous.at)
        .as_secs_f64();
    if elapsed <= 0.0
        || current.rx_bytes < previous.rx_bytes
        || current.tx_bytes < previous.tx_bytes
    {
        return None;
    }
    let rx_bytes_per_sec = (current.rx_bytes - previous.rx_bytes) as f64 / elapsed;
    let tx_bytes_per_sec = (current.tx_bytes - previous.tx_bytes) as f64 / elapsed;
    if !rx_bytes_per_sec.is_finite() || !tx_bytes_per_sec.is_finite() {
        return None;
    }
    Some(NetworkThroughput::new(rx_bytes_per_sec, tx_bytes_per_sec))
}

/// Loopback naming is consistent enough across the supported desktops for a
/// name test: exactly `lo` (Linux), `lo` followed by digits (`lo0` on macOS
/// and the BSDs), and anything containing `loopback` (Windows' pseudo
/// interfaces). A longer name merely starting with the letters, like a
/// `london0` uplink, is not loopback.
fn is_loopback(name: &OsStr) -> bool {
    let name = name.to_string_lossy().to_lowercase();
    name == "lo"
        || (name.starts_with("lo") && name[2..].chars().all(|c| c.is_ascii_digit()))
        || name.contains("loopback")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn counters(at: Instant, rx_bytes: u64, tx_bytes: u64) -> Counters {
        Counters {
            rx_bytes,
            tx_bytes,
            at,
        }
    }

    #[test]
    fn loopback_names_are_excluded_and_lookalikes_are_not() {
        for name in [
            "lo",
            "lo0",
            "lo10",
            "Loopback Pseudo-Interface 1",
            "LOOPBACK",
        ] {
            assert!(is_loopback(OsStr::new(name)), "{name} must be loopback");
        }
        for name in ["eth0", "en0", "wlan0", "london0", "l0o"] {
            assert!(
                !is_loopback(OsStr::new(name)),
                "{name} must not read as loopback"
            );
        }
    }

    #[test]
    fn sums_skip_loopback_interfaces() {
        let at = Instant::now();
        let total = sum_interfaces(
            [
                (OsStr::new("lo"), 10_u64, 10_u64),
                (OsStr::new("Loopback Pseudo-Interface 1"), 5, 5),
                (OsStr::new("eth0"), 1_000, 2_000),
                (OsStr::new("en0"), 3_000, 4_000),
            ]
            .into_iter(),
            at,
        );
        assert_eq!(total.rx_bytes, 4_000);
        assert_eq!(total.tx_bytes, 6_000);
        assert_eq!(total.at, at);
    }

    #[test]
    fn rates_use_total_deltas_over_real_elapsed_time() {
        let start = Instant::now();
        let previous = counters(start, 1_000, 2_000);
        // Two real seconds between reads: the delta is spread over the actual
        // gap, not the sampler's nominal one-second cadence.
        let current = counters(start + Duration::from_secs(2), 3_000, 2_500);
        let throughput = rate(&previous, &current).expect("a forward delta is a real reading");
        assert_eq!(throughput.rx_bytes_per_sec(), 1_000.0);
        assert_eq!(throughput.tx_bytes_per_sec(), 250.0);
    }

    #[test]
    fn an_idle_interval_is_a_genuine_zero_reading() {
        let start = Instant::now();
        let previous = counters(start, 9_000, 9_000);
        let current = counters(start + Duration::from_secs(1), 9_000, 9_000);
        let throughput = rate(&previous, &current).expect("no counter movement is a real zero");
        assert_eq!(throughput.rx_bytes_per_sec(), 0.0);
        assert_eq!(throughput.tx_bytes_per_sec(), 0.0);
    }

    #[test]
    fn the_first_read_primes_the_baseline_and_reports_nothing() {
        let Some(networks) = Networks::new().ok() else {
            // The counter backend cannot open on this platform (the `unknown`
            // sysinfo backend); the sampler reports nothing at all, and the
            // daemon withholds the wire feature instead.
            assert!(!supported());
            return;
        };
        let start = Instant::now();
        let mut sampler = Sampler {
            networks: Some(networks),
            previous: None,
        };
        assert!(
            sampler.sample(start).is_none(),
            "the first read after a baseline rebuild only primes it — there is no delta yet"
        );
        if sampler
            .networks
            .as_ref()
            .is_some_and(|networks| !networks.is_empty())
        {
            assert!(
                sampler.previous.is_some(),
                "a listed interface leaves a primed baseline behind"
            );
        }
    }

    #[test]
    fn a_backwards_counter_is_a_reset_not_a_negative_rate() {
        let start = Instant::now();
        let previous = counters(start, 9_000, 9_000);
        // Counters only move backwards across a reset (or reboot); a negative
        // "rate" is not data, and the tick reports nothing.
        let current = counters(start + Duration::from_secs(1), 100, 50);
        assert!(rate(&previous, &current).is_none());
        let tx_reset = counters(start + Duration::from_secs(1), 9_100, 50);
        assert!(rate(&previous, &tx_reset).is_none());
    }

    #[test]
    fn a_non_positive_elapsed_time_reports_nothing() {
        let start = Instant::now();
        let previous = counters(start, 1_000, 1_000);
        let same_instant = counters(start, 2_000, 2_000);
        assert!(rate(&previous, &same_instant).is_none());
    }
}
