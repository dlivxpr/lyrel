//! 可取消读取的共享令牌。
//!
//! 保留 Glim `crates/glim-services/src/work.rs`（MIT，
//! `fc2183f2c5f27ddfcdc4badbab6d83d50616831b`）的 `Cancellation` /
//! `CancelOnDrop` 链式取消语义：令牌可以由多个 owner 共享，任何一方触发都
//! 让所有持有者看到取消；`CancelOnDrop` 在作用域退出时置位。Lyrel 适配去掉
//! `anyhow`，改为本 crate 的 [`Cancelled`]，并增加显式 [`CancelToken::cancel`]
//! 供关闭/替换目标的消费者调用。原始摘要与保留范围见
//! `crates/lyrel-protocol/UPSTREAM.toml`。

use std::fmt;
use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// 一次读取被取消。
#[derive(Debug)]
pub struct Cancelled;

impl Cancelled {
    /// 取消在 `io` 边界上的表示：调用方据此区分“用户/生命周期取消”和失败。
    pub fn into_io(self) -> io::Error {
        io::Error::new(io::ErrorKind::Interrupted, self.to_string())
    }
}

impl fmt::Display for Cancelled {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("reading cancelled")
    }
}

impl std::error::Error for Cancelled {}

/// A cancellation signal shared by every owner of one read.
///
/// Cloning links the same flags; [`CancelToken::linked`] combines two token
/// sets so a detached stage can be cancelled by either its own lifecycle or the
/// reader that started it.
#[derive(Clone, Debug)]
pub struct CancelToken(Arc<Vec<Arc<AtomicBool>>>);

impl Default for CancelToken {
    fn default() -> Self {
        Self(Arc::new(vec![Arc::new(AtomicBool::new(false))]))
    }
}

/// 作用域结束时取消对应令牌。
pub struct CancelOnDrop(CancelToken);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

impl CancelToken {
    pub fn from_flag(flag: Arc<AtomicBool>) -> Self {
        Self(Arc::new(vec![flag]))
    }

    /// A fresh token plus a guard that trips it when the caller's scope ends.
    pub fn task() -> (Self, CancelOnDrop) {
        let token = Self::default();
        (token.clone(), CancelOnDrop(token))
    }

    /// A token that is cancelled when either source is.
    pub fn linked(&self, other: &Self) -> Self {
        Self(Arc::new(
            self.0.iter().chain(other.0.iter()).cloned().collect(),
        ))
    }

    /// Trip every flag this token shares with its clones.
    pub fn cancel(&self) {
        for flag in self.0.iter() {
            flag.store(true, Ordering::Release);
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.iter().any(|flag| flag.load(Ordering::Acquire))
    }

    pub fn check(&self) -> Result<(), Cancelled> {
        if self.is_cancelled() {
            return Err(Cancelled);
        }
        Ok(())
    }
}
