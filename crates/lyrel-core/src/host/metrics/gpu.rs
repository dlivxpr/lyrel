//! Whole-host GPU utilization from the OS's GPU engine performance counters (#374).
//!
//! Windows exposes per-process, per-engine GPU utilization through the
//! documented `GPU Engine` counter set — the same counters Task Manager
//! graphs (Microsoft documents the aggregation at
//! <https://devblogs.microsoft.com/directx/gpus-in-the-task-manager/>). Each
//! counter instance names one process on one physical engine, e.g.
//! `pid_12676_luid_0x00000000_0x00016ef1_phys_0_eng_4_engtype_copy`; the
//! per-process values of one physical engine sum to that engine's
//! utilization, so the whole host folds instances per physical engine and
//! reports the busiest engine. Taking the maximum single per-process sample
//! instead would report one process's share as the host's utilization.
//! Engine instances appear and vanish with the processes using them, so an
//! idle GPU reads as zero rather than as an error.
//!
//! Lyrel's per-process detail (#378) uses those same counters.

/// The GPU part of one whole-host sample: a busiest-engine utilization
/// percentage, or the reason the platform could not supply one.
#[derive(Debug)]
pub(crate) enum GpuReading {
    #[cfg(windows)]
    Busy(f32),
    Unavailable(String),
}

#[cfg(not(windows))]
pub(crate) struct GpuSampler {
    _private: (),
}

#[cfg(not(windows))]
impl GpuSampler {
    pub(crate) fn open() -> Self {
        Self { _private: () }
    }

    pub(crate) fn sample(&mut self) -> GpuReading {
        GpuReading::Unavailable(
            "GPU engine performance counters are not available on this platform".into(),
        )
    }
}

#[cfg(windows)]
mod windows {
    use std::collections::HashMap;

    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::System::Performance::{
        PDH_CSTATUS_NEW_DATA, PDH_CSTATUS_NO_COUNTER, PDH_CSTATUS_NO_OBJECT,
        PDH_CSTATUS_VALID_DATA, PDH_FMT_COUNTERVALUE_ITEM_W, PDH_FMT_DOUBLE, PDH_HCOUNTER,
        PDH_HQUERY, PDH_MORE_DATA, PDH_NO_DATA, PdhAddEnglishCounterW, PdhCloseQuery,
        PdhCollectQueryData, PdhGetFormattedCounterArrayW, PdhOpenQueryW,
    };

    use super::GpuReading;
    use lyrel_protocol::host::lyrel_usage::ProcessMetricUnavailable;

    /// `GPU Engine(*)\Utilization Percentage` in the English counter
    /// database, so the sampler works on localized Windows too.
    const GPU_ENGINE_COUNTER: &str = "\\GPU Engine(*)\\Utilization Percentage";

    pub(crate) struct GpuSampler {
        /// `None` once opening proved the host has no GPU engine counters.
        query: Option<Query>,
        unsupported: Option<String>,
    }

    impl GpuSampler {
        pub(crate) fn open() -> Self {
            match Query::open() {
                Ok(query) => Self {
                    query: Some(query),
                    unsupported: None,
                },
                Err(error) => {
                    let reason = match error {
                        OpenError::Unsupported(reason) | OpenError::Failed(reason) => reason,
                    };
                    Self {
                        query: None,
                        unsupported: Some(reason),
                    }
                }
            }
        }

        pub(crate) fn sample(&self) -> GpuReading {
            match self.rows() {
                Ok(rows) => GpuReading::Busy(busiest_engine(&rows)),
                Err(reason) => GpuReading::Unavailable(reason),
            }
        }

        fn rows(&self) -> Result<Vec<EngineRow>, String> {
            let query = self.query.as_ref().ok_or_else(|| {
                self.unsupported
                    .clone()
                    .unwrap_or_else(|| "GPU sampling is unavailable".into())
            })?;
            query.collect()?;
            query.rows()
        }

        pub(crate) fn process_engines(
            &self,
            pid: u32,
        ) -> Result<Vec<(String, f32)>, ProcessMetricUnavailable> {
            if self.query.is_none() {
                return Err(ProcessMetricUnavailable::Unsupported);
            }
            let rows = self
                .rows()
                .map_err(|_| ProcessMetricUnavailable::CounterReadFailed)?;
            Ok(rows
                .into_iter()
                .filter_map(|row| {
                    (row.pid == pid && row.percent > 0.)
                        .then(|| (row.engine, row.percent.clamp(0., 100.) as f32))
                })
                .collect())
        }
    }

    /// One parsed GPU engine counter row: the process that owns the share,
    /// the physical engine it belongs to (everything after `pid_<n>_`), and
    /// that process's percentage of the engine.
    #[derive(Clone, Debug, PartialEq)]
    struct EngineRow {
        pid: u32,
        engine: String,
        percent: f64,
    }

    /// Split `pid_<pid>_<engine…>` into the process id and the physical
    /// engine key. Instances that do not follow the documented naming (none
    /// observed on real hardware, which reports every instance per-process)
    /// are skipped rather than misparsed.
    fn split_instance(name: &str) -> Option<(u32, &str)> {
        let rest = name.strip_prefix("pid_")?;
        let split = rest.find('_')?;
        let pid = rest[..split].parse().ok()?;
        Some((pid, &rest[split + 1..]))
    }

    /// Fold per-process rows the way Task Manager's headline GPU number does:
    /// per-process values of one physical engine sum to the engine's
    /// utilization, and the whole host reads as the busiest engine.
    fn busiest_engine(rows: &[EngineRow]) -> f32 {
        let mut per_engine: HashMap<&str, f64> = HashMap::new();
        for row in rows {
            *per_engine.entry(row.engine.as_str()).or_default() += row.percent;
        }
        per_engine
            .values()
            .fold(0.0_f64, |busiest, &utilization| busiest.max(utilization))
            .clamp(0.0, 100.0) as f32
    }

    /// A primed PDH query over the wildcard GPU engine counter. Primed means
    /// one collection already happened at open time: the utilization counter
    /// is a delta, and without a baseline the first read would be garbage —
    /// exactly like the CPU sampler's first refresh.
    struct Query {
        query: PDH_HQUERY,
        counter: PDH_HCOUNTER,
    }

    // SAFETY: the process-local query is owned by one sampler and every use
    // is serialized by the owning mutex. Drop closes the handle once.
    unsafe impl Send for Query {}

    impl Query {
        fn open() -> Result<Self, OpenError> {
            let path: Vec<u16> = GPU_ENGINE_COUNTER
                .encode_utf16()
                .chain(std::iter::once(0))
                .collect();
            unsafe {
                // The query reads the live performance data (no log file),
                // so the data source is NULL; the counter path goes to the
                // counter-add call below.
                let mut query = std::ptr::null_mut();
                let status = PdhOpenQueryW(std::ptr::null(), 0, &mut query);
                if status != ERROR_SUCCESS {
                    return Err(OpenError::Failed(format!(
                        "opening the GPU counter query failed (PDH status {status:#010X})"
                    )));
                }
                let mut this = Self {
                    query,
                    counter: std::ptr::null_mut(),
                };
                let status = PdhAddEnglishCounterW(query, path.as_ptr(), 0, &mut this.counter);
                if status != ERROR_SUCCESS {
                    return Err(missing_counter_reason(status));
                }
                let status = PdhCollectQueryData(this.query);
                if status != ERROR_SUCCESS && status != PDH_NO_DATA {
                    let reason =
                        format!("priming the GPU counter query failed (PDH status {status:#010X})");
                    return Err(OpenError::Failed(reason));
                }
                Ok(this)
            }
        }

        fn collect(&self) -> Result<(), String> {
            // SAFETY: `self.query` is a live PDH query handle opened in
            // `open` and closed only in `drop`.
            let status = unsafe { PdhCollectQueryData(self.query) };
            match status {
                ERROR_SUCCESS => Ok(()),
                // No engine instances exist right now — every engine is idle.
                PDH_NO_DATA => Ok(()),
                _ => Err(format!(
                    "collecting GPU engine counters failed (PDH status {status:#010X})"
                )),
            }
        }

        /// Read and validate the current counter array. A hard read error
        /// propagates — a failed sample must not masquerade as an idle GPU.
        fn rows(&self) -> Result<Vec<EngineRow>, String> {
            Ok(read_counter_rows(self.counter)?
                .into_iter()
                .filter_map(|(name, value)| {
                    let (pid, engine) = split_instance(&name)?;
                    Some(EngineRow {
                        pid,
                        engine: engine.to_owned(),
                        percent: value,
                    })
                })
                .collect())
        }
    }

    impl Drop for Query {
        fn drop(&mut self) {
            // SAFETY: the handle is opened exactly once in `open` and this
            // drop runs at most once.
            unsafe {
                PdhCloseQuery(self.query);
            }
        }
    }

    /// Why a query could not be opened: the platform simply has no GPU
    /// engine counters to read, or the counter query itself failed.
    enum OpenError {
        Unsupported(String),
        Failed(String),
    }

    /// Translate a counter-add failure into the kind of open error it was.
    /// Missing object/counter means the host (or its GPU stack) does not
    /// expose the engine counter set; anything else stays a raw PDH status.
    fn missing_counter_reason(status: u32) -> OpenError {
        match status {
            PDH_CSTATUS_NO_OBJECT | PDH_CSTATUS_NO_COUNTER => OpenError::Unsupported(
                "this host does not expose GPU engine performance counters".into(),
            ),
            _ => OpenError::Failed(format!(
                "adding the GPU engine counter failed (PDH status {status:#010X})"
            )),
        }
    }

    /// Read the wildcard counter as (instance name, cooked percent) pairs,
    /// keeping only items whose cooked value is valid, finite and
    /// non-negative. An idle GPU yields an empty vec (either no instances or
    /// `PDH_NO_DATA`), never an error.
    fn read_counter_rows(counter: PDH_HCOUNTER) -> Result<Vec<(String, f64)>, String> {
        let mut size = 0u32;
        let mut count = 0u32;
        // SAFETY: `counter` is a live counter handle; the null buffer is the
        // documented size probe and writes only `size`/`count`.
        let probe = unsafe {
            PdhGetFormattedCounterArrayW(
                counter,
                PDH_FMT_DOUBLE,
                &mut size,
                &mut count,
                std::ptr::null_mut(),
            )
        };
        match probe {
            PDH_NO_DATA => return Ok(Vec::new()),
            ERROR_SUCCESS if size == 0 => return Ok(Vec::new()),
            PDH_MORE_DATA if size > 0 => {}
            status => {
                return Err(format!(
                    "sizing the GPU engine counter array failed (PDH status {status:#010X})"
                ));
            }
        }
        // Item layout is two 8-byte words; a u64 vec keeps the buffer aligned.
        let mut buffer = vec![0u64; (size as usize) / 8 + 2];
        let status = unsafe {
            // SAFETY: the buffer is `size` bytes (rounded up to the item
            // stride) and aligned for the item struct.
            PdhGetFormattedCounterArrayW(
                counter,
                PDH_FMT_DOUBLE,
                &mut size,
                &mut count,
                buffer.as_mut_ptr().cast::<PDH_FMT_COUNTERVALUE_ITEM_W>(),
            )
        };
        if status == PDH_NO_DATA {
            return Ok(Vec::new());
        }
        if status != ERROR_SUCCESS {
            return Err(format!(
                "reading the GPU engine counter values failed (PDH status {status:#010X})"
            ));
        }
        let mut rows = Vec::with_capacity(count as usize);
        // SAFETY: PDH filled `count` consecutive items inside `buffer`.
        let items = unsafe {
            std::slice::from_raw_parts(
                buffer.as_ptr().cast::<PDH_FMT_COUNTERVALUE_ITEM_W>(),
                count as usize,
            )
        };
        for item in items {
            if !matches!(
                item.FmtValue.CStatus,
                PDH_CSTATUS_VALID_DATA | PDH_CSTATUS_NEW_DATA
            ) {
                continue;
            }
            let value = unsafe { item.FmtValue.Anonymous.doubleValue };
            if !value.is_finite() || value < 0.0 {
                continue;
            }
            let name = read_wide(item.szName.cast_const());
            rows.push((name, value));
        }
        Ok(rows)
    }

    fn read_wide(ptr: *const u16) -> String {
        if ptr.is_null() {
            return String::new();
        }
        let mut len = 0usize;
        // SAFETY: PDH returns NUL-terminated instance names.
        while unsafe { *ptr.add(len) } != 0 {
            len += 1;
        }
        // SAFETY: `len` covers exactly the name's code units.
        let units = unsafe { std::slice::from_raw_parts(ptr, len) };
        String::from_utf16_lossy(units)
    }

    /// Real-device probe for the GPU fold: prints the busiest-engine host
    /// reading and the hottest per-process rows from the actual counters.
    /// Not a CI case — CI hosts may have no GPU — run it explicitly on
    /// hardware with a GPU.
    #[test]
    #[ignore = "probes the real GPU counters; run explicitly on GPU hardware"]
    fn real_device_probe_prints_host_fold_and_process_rows() {
        // The utilization counter is a delta: like the production sampler,
        // give the priming collect a real second of engine time before
        // reading a value.
        let sampler = GpuSampler::open();
        std::thread::sleep(std::time::Duration::from_millis(1_100));
        let reading = sampler.sample();
        let GpuReading::Busy(host) = reading else {
            panic!("this machine should expose GPU engine counters: {reading:?}");
        };
        let mut rows = sampler.rows().expect("native process GPU rows");
        rows.sort_by(|a, b| b.percent.total_cmp(&a.percent));
        eprintln!("host GPU utilization: {host}%");
        eprintln!("per-process rows: {}", rows.len());
        for row in rows.iter().take(10) {
            eprintln!("pid={} engine={} {:.2}%", row.pid, row.engine, row.percent);
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn row(pid: u32, engine: &str, percent: f64) -> EngineRow {
            EngineRow {
                pid,
                engine: engine.into(),
                percent,
            }
        }

        #[test]
        fn instance_names_split_into_pid_and_physical_engine() {
            let (pid, engine) =
                split_instance("pid_12676_luid_0x00000000_0x00016ef1_phys_0_eng_4_engtype_copy")
                    .expect("a real counter instance splits");
            assert_eq!(pid, 12676);
            assert_eq!(
                engine,
                "luid_0x00000000_0x00016ef1_phys_0_eng_4_engtype_copy"
            );
            // Engine types carry their own suffixes, e.g. `security_1`.
            let (pid, engine) =
                split_instance("pid_4_luid_0x00000000_0x00016ef1_phys_0_eng_15_engtype_security_1")
                    .expect("an underscored engine type still splits");
            assert_eq!(pid, 4);
            assert!(engine.ends_with("engtype_security_1"));
            assert_eq!(split_instance("total"), None);
            assert_eq!(
                split_instance("pid_notanumber_luid_0_phys_0_eng_0_engtype_3d"),
                None
            );
        }

        #[test]
        fn host_utilization_folds_pids_per_engine_and_picks_the_busiest() {
            let rows = [
                row(100, "luid_a_phys_0_eng_0_engtype_3d", 30.0),
                row(200, "luid_a_phys_0_eng_0_engtype_3d", 25.0),
                row(300, "luid_a_phys_0_eng_1_engtype_copy", 10.0),
                row(400, "luid_b_phys_0_eng_0_engtype_3d", 12.5),
            ];
            // eng_0 of luid_a sums its two processes to 55 and is busiest.
            assert_eq!(busiest_engine(&rows), 55.0);
        }

        #[test]
        fn the_host_reading_is_not_a_single_pids_share() {
            // One process dominates one engine while another engine idles at
            // zero: the busiest engine is the dominated engine's full sum,
            // not any per-process maximum from elsewhere.
            let rows = [
                row(900, "luid_a_phys_0_eng_0_engtype_3d", 2.0),
                row(901, "luid_a_phys_0_eng_0_engtype_3d", 3.0),
                row(902, "luid_a_phys_0_eng_1_engtype_copy", 0.0),
            ];
            assert_eq!(busiest_engine(&rows), 5.0);
            // With no rows at all the GPU is idle, not failed.
            assert_eq!(busiest_engine(&[]), 0.0);
        }
    }
}

#[cfg(windows)]
pub(crate) use windows::GpuSampler;
