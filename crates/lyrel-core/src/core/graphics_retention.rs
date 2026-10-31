//! Shared daemon retention. The registry never takes a pane lock: payloads are
//! released under this lock and a coalesced wake asks the pane to publish the
//! ordered mutation under its own output lock.
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, OnceLock, Weak, mpsc::SyncSender};

use super::{MAX_PANE_DECODED_BYTES, MAX_PANE_ENCODED_BYTES};

pub const MAX_DAEMON_ENCODED_BYTES: usize = 128 << 20;
pub const MAX_DAEMON_DECODED_BYTES: usize = 256 << 20;
pub const MAX_PANE_WORK_BYTES: usize = 128 << 20;
pub const MAX_DAEMON_WORK_BYTES: usize = 256 << 20;
pub use crate::daemon::protocol::{
    GRAPHICS_METADATA_ENTRY_BYTES as GRAPHICS_METADATA_BYTES,
    MAX_AGGREGATE_GRAPHICS_METADATA_BYTES as MAX_DAEMON_METADATA_BYTES,
    MAX_PANE_GRAPHICS_METADATA_BYTES as MAX_PANE_METADATA_BYTES,
};

#[derive(Clone, Copy)]
pub(crate) struct Limits {
    pub pane_encoded: usize,
    pub pane_decoded: usize,
    pub daemon_encoded: usize,
    pub daemon_decoded: usize,
    pub pane_work: usize,
    pub daemon_work: usize,
    pub pane_metadata: usize,
    pub daemon_metadata: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            pane_encoded: MAX_PANE_ENCODED_BYTES,
            pane_decoded: MAX_PANE_DECODED_BYTES,
            daemon_encoded: MAX_DAEMON_ENCODED_BYTES,
            daemon_decoded: MAX_DAEMON_DECODED_BYTES,
            pane_work: MAX_PANE_WORK_BYTES,
            daemon_work: MAX_DAEMON_WORK_BYTES,
            pane_metadata: MAX_PANE_METADATA_BYTES,
            daemon_metadata: MAX_DAEMON_METADATA_BYTES,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Priority {
    Unreferenced,
    History,
    Protected,
}

pub(super) struct Resident {
    pub packet: Option<crate::daemon::protocol::GraphicsPacket>,
    pub decoded: usize,
    pub priority: Priority,
    pub announced: bool,
    pub wake: Option<SyncSender<()>>,
}
struct Entry {
    owner: u64,
    group: u64,
    resident: Weak<Mutex<Resident>>,
}
#[derive(Default)]
struct Registry {
    sequence: u64,
    entries: BTreeMap<u64, Entry>,
    work: BTreeMap<u64, usize>,
    metadata: BTreeMap<u64, usize>,
}
pub(crate) struct Retention {
    pub limits: Limits,
    registry: Mutex<Registry>,
}
static RETENTION: OnceLock<Arc<Retention>> = OnceLock::new();
impl Retention {
    pub(crate) fn shared() -> Arc<Self> {
        RETENTION
            .get_or_init(|| {
                Arc::new(Self {
                    limits: Limits::default(),
                    registry: Mutex::new(Registry::default()),
                })
            })
            .clone()
    }
    pub(super) fn protect<'a>(&self, residents: impl Iterator<Item = &'a Arc<Mutex<Resident>>>) {
        // Preserve registry -> resident lock order and the admission barrier,
        // but visit only this pane's existing owners, not every daemon entry.
        let _registry = self.registry.lock().unwrap();
        for resident in residents {
            resident.lock().unwrap().priority = Priority::Protected;
        }
    }
    pub(crate) fn owner(&self) -> u64 {
        let mut registry = self.registry.lock().unwrap();
        registry.sequence = registry
            .sequence
            .checked_add(1)
            .expect("graphics sequence exhausted");
        registry.sequence
    }
    pub(super) fn reclaim_specific(&self, slot: &Arc<Mutex<Resident>>) -> bool {
        let registry = self.registry.lock().unwrap();
        let Some(group) = registry.entries.values().find_map(|entry|
            entry.resident.upgrade().filter(|resident| Arc::ptr_eq(resident, slot))
                .map(|_| (entry.owner, entry.group))) else { return false; };
        let slots: Vec<_> = registry.entries.values()
            .filter(|entry| (entry.owner, entry.group) == group)
            .filter_map(|entry| entry.resident.upgrade()).collect();
        let absent = { slot.lock().unwrap().packet.is_none() };
        if absent || slots.iter().any(|slot|
            slot.lock().unwrap().priority == Priority::Protected) { return false; }
        for slot in slots {
            let mut resident = slot.lock().unwrap();
            resident.packet = None;
            if let Some(wake) = &resident.wake { let _ = wake.try_send(()); }
        }
        true
    }
    pub(super) fn admit(
        &self,
        owner: u64,
        group: u64,
        decoded: usize,
        replacement: Option<&Arc<Mutex<Resident>>>,
        wake: Option<SyncSender<()>>,
        capacity: (usize, usize),
        packet: crate::daemon::protocol::GraphicsPacket,
    ) -> Option<Arc<Mutex<Resident>>> {
        let mut registry = self.registry.lock().unwrap();
        registry
            .entries
            .retain(|_, entry| entry.resident.strong_count() != 0);
        let mut total = (packet.len(), decoded);
        let mut pane = total;
        let replaced_group = replacement.and_then(|old| registry.entries.values().find_map(|entry|
            entry.resident.upgrade().filter(|slot| Arc::ptr_eq(old, slot))
                .map(|_| (entry.owner, entry.group))));
        let mut groups = BTreeMap::new();
        for (&sequence, entry) in &registry.entries {
            if replaced_group == Some((entry.owner, entry.group)) { continue; }
            let Some(slot) = entry.resident.upgrade() else { continue; };
            let resident = slot.lock().unwrap();
            if let Some(packet) = &resident.packet {
                total.0 += packet.len();
                total.1 += resident.decoded;
                if entry.owner == owner {
                    pane.0 += packet.len();
                    pane.1 += resident.decoded;
                }
                let candidate = groups.entry((entry.owner, entry.group)).or_insert_with(||
                    (Priority::Unreferenced, sequence, entry.owner, 0usize, 0usize, Vec::new()));
                candidate.0 = candidate.0.max(resident.priority);
                candidate.3 += packet.len();
                candidate.4 += resident.decoded;
                candidate.5.push(slot.clone());
            }
        }
        // The image being extended is never its own admission victim.
        let mut candidates: Vec<_> = groups.into_iter()
            .filter(|((candidate_owner, candidate_group), candidate)|
                candidate.0 != Priority::Protected && (*candidate_owner != owner || *candidate_group != group))
            .map(|(_, candidate)| candidate).collect();
        candidates.sort_by_key(|c| (c.0, c.1));
        let fits = |pane: (usize, usize), total: (usize, usize)| {
            pane.0 <= capacity.0.min(self.limits.pane_encoded)
                && pane.1 <= capacity.1.min(self.limits.pane_decoded)
                && total.0 <= self.limits.daemon_encoded
                && total.1 <= self.limits.daemon_decoded
        };
        let mut victims = Vec::new();
        let mut selected = std::collections::BTreeSet::new();
        for (_, sequence, candidate_owner, encoded, decoded, slot) in &candidates {
            if pane.0 <= capacity.0.min(self.limits.pane_encoded)
                && pane.1 <= capacity.1.min(self.limits.pane_decoded)
            {
                break;
            }
            if *candidate_owner != owner {
                continue;
            }
            pane.0 -= *encoded;
            pane.1 -= *decoded;
            total.0 -= *encoded;
            total.1 -= *decoded;
            selected.insert(*sequence);
            victims.extend(slot.iter().cloned());
        }
        for (_, sequence, candidate_owner, encoded, decoded, slot) in candidates {
            if fits(pane, total) {
                break;
            }
            if selected.contains(&sequence) {
                continue;
            }
            total.0 -= encoded;
            total.1 -= decoded;
            if candidate_owner == owner {
                pane.0 -= encoded;
                pane.1 -= decoded;
            }
            victims.extend(slot);
        }
        if !fits(pane, total) {
            return None;
        }
        for slot in victims {
            let mut resident = slot.lock().unwrap();
            resident.packet = None;
            if let Some(wake) = &resident.wake {
                let _ = wake.try_send(());
            }
        }
        registry.sequence = registry
            .sequence
            .checked_add(1)
            .expect("graphics sequence exhausted");
        let sequence = registry.sequence;
        let slot = Arc::new(Mutex::new(Resident {
            packet: Some(packet),
            decoded,
            priority: Priority::Protected,
            announced: false,
            wake,
        }));
        registry.entries.insert(
            sequence,
            Entry {
                owner,
                group,
                resident: Arc::downgrade(&slot),
            },
        );
        Some(slot)
    }
    pub(super) fn metadata(self: &Arc<Self>, owner: u64) -> Option<MetadataTicket> {
        let mut registry = self.registry.lock().unwrap();
        let pane = registry.metadata.get(&owner).copied().unwrap_or(0);
        let total: usize = registry.metadata.values().sum();
        if pane + GRAPHICS_METADATA_BYTES > self.limits.pane_metadata
            || total + GRAPHICS_METADATA_BYTES > self.limits.daemon_metadata
        {
            return None;
        }
        registry
            .metadata
            .insert(owner, pane + GRAPHICS_METADATA_BYTES);
        Some(Arc::new(MetadataLease {
            retention: self.clone(),
            owner,
        }))
    }
    pub(crate) fn work(self: &Arc<Self>, owner: u64) -> Work {
        Work {
            retention: self.clone(),
            owner,
            bytes: 0,
        }
    }
}

pub(crate) struct Work {
    retention: Arc<Retention>,
    owner: u64,
    bytes: usize,
}
impl Work {
    pub(crate) fn resize(&mut self, bytes: usize) -> bool {
        let mut registry = self.retention.registry.lock().unwrap();
        let pane = registry.work.get(&self.owner).copied().unwrap_or(0);
        let total: usize = registry.work.values().sum();
        if pane - self.bytes + bytes > self.retention.limits.pane_work
            || total - self.bytes + bytes > self.retention.limits.daemon_work
        {
            return false;
        }
        let pane = pane - self.bytes + bytes;
        if pane == 0 {
            registry.work.remove(&self.owner);
        } else {
            registry.work.insert(self.owner, pane);
        }
        self.bytes = bytes;
        true
    }
}
impl Drop for Work {
    fn drop(&mut self) {
        self.resize(0);
    }
}

pub(super) type MetadataTicket = Arc<MetadataLease>;
pub(super) struct MetadataLease {
    retention: Arc<Retention>,
    owner: u64,
}
impl Drop for MetadataLease {
    fn drop(&mut self) {
        let mut registry = self.retention.registry.lock().unwrap();
        let bytes = registry
            .metadata
            .get_mut(&self.owner)
            .expect("live metadata lease owns its charge");
        *bytes -= GRAPHICS_METADATA_BYTES;
        if *bytes == 0 {
            registry.metadata.remove(&self.owner);
        }
    }
}

#[cfg(test)]
pub(crate) fn inject_limits(limits: Limits) {
    assert!(
        RETENTION
            .set(Arc::new(Retention {
                limits,
                registry: Mutex::new(Registry::default())
            }))
            .is_ok()
    );
}
