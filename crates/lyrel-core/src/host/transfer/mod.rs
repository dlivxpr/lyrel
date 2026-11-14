//! 预览大文件的有界分段传输：预算、取消与上游保留记录。
//!
//! 这里只放与 Host 无关的传输策略；打开文件、绑定版本和字节来源仍在
//! [`crate::host::Host::open_read`] 与其消费者。分段是拉式的：客户端每要一段
//! 才发一个请求，交互 control 帧在段与段之间插队，不会被一次大帧独占。
//!
//! 下表是第 #281 票的设计边界，单位与失败效果是契约；具体吞吐/内存占用仍需
//! 原生窗口与真实远端的独立测量，本表不冒充实测结果。解码像素与图片缓存有
//! 各自的预算（`markdown::images` 的 `MAX_PIXELS` / `CACHE_BYTES`），与传输
//! 账分开。

pub mod admission;
pub mod cancellation;
pub mod limits;

pub use admission::{
    TRANSFER_WORKING_BYTES, TransferAdmission, WorkingPressure, WorkingReservation,
    transfer_admission,
};
pub use cancellation::{CancelOnDrop, CancelToken, Cancelled};
pub use limits::{PreviewBudgetExceeded, preview_budget};

use std::time::Duration;

/// 一次 `ReadChunk` 默认携带的字节数。它是排队的粒度：一段写完后交互帧可以
/// 立刻获得输出锁，而不是排在整份文件后面。
pub const SEGMENT_BYTES: u64 = 256 * 1024;

/// 服务端接受的单段上限。超过即拒绝（`InvalidInput`），不做静默截断。
pub const SEGMENT_BYTES_MAX: u64 = 4 * 1024 * 1024;

/// 每条 control 连接同时打开的读取会话上限；超过先回收空闲会话，再拒绝。
pub const MAX_READ_SESSIONS: usize = 64;

/// 读取会话空闲多久后被回收。连接退出会立即清空全部会话，不等这个时限。
pub const READ_SESSION_IDLE: Duration = Duration::from_secs(120);

/// 预算超限时的消费者可见行为。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BudgetEffect {
    /// 拒绝该次读取，保留原文件与既有文档。
    Reject,
    /// 按预算截断（当前没有预算使用它；保留类别以对齐上游清单）。
    Truncate,
    /// 退回到另一种明确能力（例如无分段能力的对端走旧的整文件读取）。
    Fallback,
    /// 单次传输/工作单元的上界。
    Budget,
    /// 超时即结束该请求，不静默继续。
    Timeout,
    /// 单页数量上界。
    Page,
    /// 不设上限。
    Unlimited,
}

impl BudgetEffect {
    pub fn name(self) -> &'static str {
        match self {
            Self::Reject => "reject",
            Self::Truncate => "truncate",
            Self::Fallback => "fallback",
            Self::Budget => "budget",
            Self::Timeout => "timeout",
            Self::Page => "page",
            Self::Unlimited => "unlimited",
        }
    }
}

/// 一条具名预算：数值、单位、失败效果和一句说明。
#[derive(Clone, Copy, Debug)]
pub struct Budget {
    pub name: &'static str,
    pub value: u64,
    pub unit: &'static str,
    pub effect: BudgetEffect,
    pub description: &'static str,
}

/// 传输侧的全部预算。测试会核对每条都有单位、效果和说明。
pub const BUDGETS: &[Budget] = &[
    Budget {
        name: "transfer.segment_bytes",
        value: SEGMENT_BYTES,
        unit: "bytes",
        effect: BudgetEffect::Budget,
        description: "one pulled segment; interactive frames interleave between segments",
    },
    Budget {
        name: "transfer.segment_bytes_max",
        value: SEGMENT_BYTES_MAX,
        unit: "bytes",
        effect: BudgetEffect::Reject,
        description: "server refuses a larger segment request instead of truncating it",
    },
    Budget {
        name: "transfer.working_bytes",
        value: TRANSFER_WORKING_BYTES,
        unit: "bytes",
        effect: BudgetEffect::Reject,
        description: "sum of active preview transfer buffers; refusal is explicit",
    },
    Budget {
        name: "transfer.max_read_sessions",
        value: MAX_READ_SESSIONS as u64,
        unit: "sessions",
        effect: BudgetEffect::Reject,
        description: "open read sessions per control connection; idle ones are reclaimed first",
    },
    Budget {
        name: "transfer.read_session_idle_secs",
        value: READ_SESSION_IDLE.as_secs(),
        unit: "seconds",
        effect: BudgetEffect::Timeout,
        description: "an idle open file is closed and its handle released",
    },
];

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn every_budget_names_its_unit_and_failure_effect() {
        let mut names = HashSet::new();
        for budget in BUDGETS {
            assert!(!budget.name.is_empty());
            assert!(names.insert(budget.name), "duplicate budget {}", budget.name);
            assert!(!budget.unit.is_empty(), "{} has no unit", budget.name);
            assert!(!budget.description.is_empty(), "{} has no statement of effect", budget.name);
            assert!(
                !matches!(budget.effect, BudgetEffect::Unlimited) || budget.value == u64::MAX,
                "{} claims unlimited with a finite value",
                budget.name
            );
        }
    }

    #[test]
    fn a_segment_fits_well_inside_the_wire_frame() {
        assert!(SEGMENT_BYTES <= SEGMENT_BYTES_MAX);
        assert!(SEGMENT_BYTES_MAX < crate::daemon::protocol::MAX_FRAME as u64);
    }

    #[test]
    fn a_budget_refusal_names_what_was_exceeded() {
        let error = preview_budget(false, "the preview input budget").unwrap_err();
        assert_eq!(error.to_string(), "the preview input budget");
        assert!(preview_budget(true, "never used").is_ok());
    }
}
