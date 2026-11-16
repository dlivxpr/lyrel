//! 传输工作内存的先记账后分配准入。
//!
//! 保留 Glim `crates/glim-core/src/resources.rs`（MIT，
//! `fc2183f2c5f27ddfcdc4badbab6d83d50616831b`）`reserve_working(minimum,
//! desired)` 的两段式语义和 `WorkingPressure` 显式失败：先记账再分配，
//! 竞争的读取不能同时认领同一份容量；`WorkingReservation` 在 Drop 时归还。
//!
//! Lyrel 适配：去掉了缓存/回收、UI 资源类目和 macOS 内存余量回调，因为传输
//! 预算是一本独立、固定上界的账；图片解码后的像素缓存仍由现成的
//! `markdown::images` lease 记账，两者不混用。原始摘要与保留范围见
//! `crates/lyrel-core/UPSTREAM.toml`。

use std::fmt;
use std::sync::{Arc, LazyLock, Mutex};

/// 所有活动预览读取合计可认领的传输工作内存。
pub const TRANSFER_WORKING_BYTES: u64 = 256 * 1024 * 1024;

/// 传输工作内存不足以满足一次读取的最小需求。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WorkingPressure {
    pub minimum: u64,
    pub available: u64,
}

impl fmt::Display for WorkingPressure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "preview transfer needs {} bytes but only {} are free; close another preview",
            self.minimum, self.available
        )
    }
}

impl std::error::Error for WorkingPressure {}

#[derive(Debug, Default)]
struct State {
    reserved: u64,
    budget: u64,
}

/// 一本传输预算账。克隆共享同一份额度。
#[derive(Clone, Default)]
pub struct TransferAdmission {
    inner: Arc<Mutex<State>>,
}

#[derive(Debug)]
struct Reservation {
    inner: Arc<Mutex<State>>,
    bytes: u64,
}

impl Drop for Reservation {
    fn drop(&mut self) {
        let mut state = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        state.reserved = state.reserved.saturating_sub(self.bytes);
    }
}

/// 已认领的传输工作内存；Drop 时归还。
#[derive(Debug)]
pub struct WorkingReservation {
    reservation: Reservation,
}

impl WorkingReservation {
    pub fn bytes(&self) -> u64 {
        self.reservation.bytes
    }
}

impl TransferAdmission {
    pub fn new(budget: u64) -> Self {
        Self {
            inner: Arc::new(Mutex::new(State {
                reserved: 0,
                budget,
            })),
        }
    }

    pub fn budget(&self) -> u64 {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).budget
    }

    pub fn reserved(&self) -> u64 {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .reserved
    }

    pub fn available(&self) -> u64 {
        let state = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        state.budget.saturating_sub(state.reserved)
    }

    /// Reserve before allocating. `desired` is capped by what is free; a
    /// request that cannot get at least `minimum` fails instead of shrinking
    /// the read silently.
    pub fn reserve(
        &self,
        minimum: u64,
        desired: u64,
    ) -> Result<WorkingReservation, WorkingPressure> {
        assert!(
            minimum <= desired,
            "minimum {minimum} exceeds desired {desired}"
        );
        let mut state = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let available = state.budget.saturating_sub(state.reserved);
        let bytes = desired.min(available);
        if bytes < minimum {
            return Err(WorkingPressure { minimum, available });
        }
        state.reserved = state.reserved.saturating_add(bytes);
        Ok(WorkingReservation {
            reservation: Reservation {
                inner: Arc::clone(&self.inner),
                bytes,
            },
        })
    }
}

/// 本进程所有预览传输共用的账。
pub fn transfer_admission() -> &'static TransferAdmission {
    static POOL: LazyLock<TransferAdmission> =
        LazyLock::new(|| TransferAdmission::new(TRANSFER_WORKING_BYTES));
    &POOL
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reservation_holds_its_bytes_until_dropped() {
        let pool = TransferAdmission::new(16);
        let held = pool.reserve(8, 8).unwrap();
        assert_eq!(held.bytes(), 8);
        assert_eq!(pool.reserved(), 8);
        assert_eq!(pool.available(), 8);
        drop(held);
        assert_eq!(pool.reserved(), 0);
        assert_eq!(pool.available(), 16);
    }

    #[test]
    fn competing_reads_cannot_claim_the_same_capacity() {
        let pool = TransferAdmission::new(10);
        let _first = pool.reserve(10, 10).unwrap();
        assert_eq!(
            pool.reserve(1, 10).unwrap_err(),
            WorkingPressure {
                minimum: 1,
                available: 0
            }
        );
    }

    #[test]
    fn a_larger_request_is_capped_by_what_is_free() {
        let pool = TransferAdmission::new(10);
        let _held = pool.reserve(6, 6).unwrap();
        let capped = pool.reserve(2, 10).unwrap();
        assert_eq!(capped.bytes(), 4);
        assert_eq!(pool.available(), 0);
    }

    #[test]
    fn a_reservation_released_by_its_owner_frees_the_next_read() {
        let pool = TransferAdmission::new(4);
        let first = pool.reserve(4, 4).unwrap();
        assert!(pool.reserve(1, 1).is_err());
        drop(first);
        assert!(pool.reserve(1, 1).is_ok());
    }
}
