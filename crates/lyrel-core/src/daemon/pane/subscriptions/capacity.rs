//! 连接容量租约；全局容量锁仍先于 PaneState，且不从 pane 内反向获取。
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::daemon::pane::DaemonPane;
use crate::daemon::protocol::DaemonMsg;
use crate::daemon::transport::Stream;

/// A connection's declared capacity is a lifetime lease, not a mutable hint.
/// This lock is acquired before a pane lock and is never acquired from a pane.
pub(crate) struct GraphicsCapacityLease {
    pane: Arc<DaemonPane>,
    token: u64,
}

type GraphicsCapacities = HashMap<(u64, u64), (u64, u64)>;

fn graphics_capacities() -> &'static Mutex<GraphicsCapacities> {
    static CAPACITIES: std::sync::LazyLock<Mutex<GraphicsCapacities>> =
        std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));
    &CAPACITIES
}

impl GraphicsCapacityLease {
    pub(crate) fn endpoint(&self) -> u64 {
        self.token
    }

    pub(crate) fn reserve(
        pane: &Arc<DaemonPane>,
        capacity: (u64, u64),
        mut reply: &Stream,
    ) -> anyhow::Result<Option<Self>> {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let token = NEXT.fetch_add(1, Ordering::Relaxed);
        let mut capacities = graphics_capacities().lock().unwrap();
        let effective = capacities
            .iter()
            .filter(|((id, _), _)| *id == pane.id)
            .fold(capacity, |limit, (_, other)| {
                (limit.0.min(other.0), limit.1.min(other.1))
            });
        if !pane.set_graphics_capacity(effective.0, effective.1) {
            DaemonMsg::Error("graphics capacity cannot hold this pane's retained images; existing session content was preserved".into()).encode(&mut reply)?;
            return Ok(None);
        }
        capacities.insert((pane.id, token), capacity);
        Ok(Some(Self {
            pane: pane.clone(),
            token,
        }))
    }
}

impl Drop for GraphicsCapacityLease {
    fn drop(&mut self) {
        let mut capacities = graphics_capacities().lock().unwrap();
        capacities.remove(&(self.pane.id, self.token));
        let effective = capacities
            .iter()
            .filter(|((id, _), _)| *id == self.pane.id)
            .fold((u64::MAX, u64::MAX), |limit, (_, other)| {
                (limit.0.min(other.0), limit.1.min(other.1))
            });
        // The authority clamps to its finite pane defaults after the last
        // consumer leaves. Releasing a lease never revives evicted content.
        let _ = self.pane.set_graphics_capacity(effective.0, effective.1);
    }
}

