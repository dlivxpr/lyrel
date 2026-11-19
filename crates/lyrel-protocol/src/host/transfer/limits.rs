//! 命名预算与失败效果清单。
//!
//! 保留 Glim `crates/glim-services/src/limits.rs`（MIT，
//! `fc2183f2c5f27ddfcdc4badbab6d83d50616831b`）的
//! `PreviewBudgetExceeded` + `preview_budget(within, message)` 机制：每个预算
//! 有名字、单位与明确的失败类别，超限是一个可命名的错误而不是静默截断。
//!
//! Glim 的 `assets/reader-limits.json` → Rust 生成管线不导入（Lyrel 没有
//! build-time 生成需求）；本模块的数值来自 Lyrel 自己的消费者设计，不复制
//! Glim 以 macOS ImageIO 为前提的常量。原始摘要见
//! `crates/lyrel-protocol/UPSTREAM.toml`。

use std::fmt;

/// 一次预览请求超出了具名预算。
#[derive(Debug, PartialEq, Eq)]
pub struct PreviewBudgetExceeded(pub &'static str);

impl fmt::Display for PreviewBudgetExceeded {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

impl std::error::Error for PreviewBudgetExceeded {}

/// `within` 为假时返回命名的超限错误。
pub fn preview_budget(within: bool, message: &'static str) -> Result<(), PreviewBudgetExceeded> {
    if within {
        Ok(())
    } else {
        Err(PreviewBudgetExceeded(message))
    }
}
