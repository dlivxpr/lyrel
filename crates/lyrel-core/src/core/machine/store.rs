//! The sole transaction owner: lock order, acceptance, flush lifecycle, and publication.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::core::session::WorkspaceId;

use super::operations::not_found;
use super::persistence::{self, load_machine};
use super::{Attachment, LayoutDelta, Machine, PaneRecord, Workspace, default_machine_path};
#[cfg(test)]
use super::MACHINE_FILE;

mod facts;
mod panes;
mod sidecar;
mod tabs;
mod workspaces;

pub use facts::{observe_pane, observed_store, publish_observations};
#[cfg(test)]
pub(crate) use facts::{OBSERVE_SLOT, withdraw_observations};

#[cfg(not(test))]
pub const FACT_FLUSH_INTERVAL: Duration = Duration::from_secs(2);

#[cfg(test)]
pub const FACT_FLUSH_INTERVAL: Duration = Duration::from_secs(600);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SubscriberId(pub u64);

pub type Notify = Arc<dyn Fn(&str, &LayoutDelta) + Send + Sync>;

pub struct Subscription {
    store: Arc<MachineStore>,
    id: SubscriberId,
}

impl Subscription {
    pub fn id(&self) -> SubscriberId {
        self.id
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        self.store.unsubscribe(self.id);
    }
}

pub type LivenessProbe = Arc<dyn Fn(u64) -> bool + Send + Sync>;

/// One authority for the tree and its existing mutation/notification locks.
/// Child modules implement semantic transactions without owning separate state.
pub struct MachineStore {
    path: PathBuf,
    state: Mutex<Machine>,
    liveness: Mutex<Option<LivenessProbe>>,
    notify_order: Mutex<()>,
    subscribers: Mutex<Vec<(SubscriberId, Notify)>>,
    next_subscriber: AtomicU64,
    unwritten: AtomicBool,
    flushing: AtomicBool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Persist {
    Now,
    Soon,
}

impl MachineStore {
    pub fn open(path: impl Into<PathBuf>) -> Arc<MachineStore> {
        let path = path.into();
        let machine = load_machine(&path);
        Arc::new(MachineStore {
            path,
            state: Mutex::new(machine),
            liveness: Mutex::new(None),
            notify_order: Mutex::new(()),
            subscribers: Mutex::new(Vec::new()),
            next_subscriber: AtomicU64::new(1),
            unwritten: AtomicBool::new(false),
            flushing: AtomicBool::new(false),
        })
    }

    pub fn set_liveness_probe(&self, probe: LivenessProbe) {
        *self.liveness.lock().unwrap_or_else(|e| e.into_inner()) = Some(probe);
    }

    fn seed_is_live(&self, pane: u64) -> bool {
        let probe = self
            .liveness
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        match probe {
            Some(probe) => probe(pane),
            None => true,
        }
    }

    pub fn shared() -> io::Result<Arc<MachineStore>> {
        Ok(MachineStore::open(default_machine_path()?))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn machine(&self) -> Machine {
        self.locked().clone()
    }

    pub fn workspace(&self, id: WorkspaceId) -> io::Result<Workspace> {
        self.locked()
            .workspaces
            .iter()
            .find(|w| w.id == id)
            .cloned()
            .ok_or_else(|| not_found(format!("no workspace {id} on this machine")))
    }

    pub fn pane(&self, id: u64) -> Option<PaneRecord> {
        self.locked().panes.iter().find(|p| p.id == id).cloned()
    }

    pub fn attach(&self, workspace: WorkspaceId, who: Attachment) -> Option<Attachment> {
        let mut m = self.locked();
        let ws = m.workspaces.iter_mut().find(|w| w.id == workspace)?;
        ws.attachment.replace(who)
    }

    pub fn attachment(&self, workspace: WorkspaceId) -> Option<Attachment> {
        self.locked()
            .workspaces
            .iter()
            .find(|w| w.id == workspace)
            .and_then(|w| w.attachment.clone())
    }

    pub fn detach(&self, workspace: WorkspaceId, token: &str) -> bool {
        let mut m = self.locked();
        let Some(ws) = m.workspaces.iter_mut().find(|w| w.id == workspace) else {
            return false;
        };
        if ws.attachment.as_ref().is_some_and(|a| a.token == token) {
            ws.attachment = None;
            true
        } else {
            false
        }
    }

    pub fn subscribe(self: &Arc<Self>, f: Notify) -> Subscription {
        let id = SubscriberId(self.next_subscriber.fetch_add(1, Ordering::Relaxed));
        self.subscribers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push((id, f));
        Subscription {
            store: Arc::clone(self),
            id,
        }
    }

    fn unsubscribe(&self, id: SubscriberId) {
        self.subscribers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|(sid, _)| *sid != id);
    }

    fn locked(&self) -> std::sync::MutexGuard<'_, Machine> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn mutate<T>(
        &self,
        origin: Option<SubscriberId>,
        op: impl FnOnce(&mut Machine) -> io::Result<(T, Vec<(WorkspaceId, LayoutDelta)>)>,
    ) -> io::Result<T> {
        self.mutate_with(origin, Persist::Now, op)
    }

    fn mutate_with<T>(
        &self,
        origin: Option<SubscriberId>,
        persist: Persist,
        op: impl FnOnce(&mut Machine) -> io::Result<(T, Vec<(WorkspaceId, LayoutDelta)>)>,
    ) -> io::Result<T> {
        // Hold notification order through publication, but release state before
        // calling observers. A synchronous write failure restores state here.
        let _order = self.notify_order.lock().unwrap_or_else(|e| e.into_inner());
        let deltas;
        let value;
        {
            let mut m = self.locked();
            let before = m.clone();
            match op(&mut m).and_then(|out| {
                if *m != before {
                    match persist {
                        Persist::Now => persistence::persist(&self.path, &m)?,
                        Persist::Soon => self.unwritten.store(true, Ordering::Release),
                    }
                }
                Ok(out)
            }) {
                Ok((v, d)) => {
                    value = v;
                    deltas = d;
                }
                Err(e) => {
                    *m = before;
                    return Err(e);
                }
            }
        }
        if !deltas.is_empty() {
            self.notify_all(&deltas, origin);
        }
        Ok(value)
    }

    pub fn flush(&self) {
        if !self.unwritten.load(Ordering::Acquire) {
            return;
        }
        let _order = self.notify_order.lock().unwrap_or_else(|e| e.into_inner());
        let m = self.locked();
        self.unwritten.store(false, Ordering::Release);
        if let Err(e) = persistence::persist(&self.path, &m) {
            log::warn!("could not write {}: {e}", self.path.display());
            self.unwritten.store(true, Ordering::Release);
        }
    }

    fn ensure_flusher(self: &Arc<Self>) {
        if self.flushing.swap(true, Ordering::AcqRel) {
            return;
        }
        let weak = Arc::downgrade(self);
        let spawned = std::thread::Builder::new()
            .name("lyrel-machine-flush".into())
            .spawn(move || {
                loop {
                    std::thread::sleep(FACT_FLUSH_INTERVAL);
                    let Some(store) = weak.upgrade() else { return };
                    store.flush();
                }
            });
        if let Err(e) = spawned {
            log::warn!("could not start the machine-tree flusher ({e}); writing facts inline");
            self.flushing.store(false, Ordering::Release);
            self.flush();
        }
    }

    fn notify_all(&self, deltas: &[(WorkspaceId, LayoutDelta)], origin: Option<SubscriberId>) {
        let subscribers: Vec<(SubscriberId, Notify)> = self
            .subscribers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        for (workspace, delta) in deltas {
            let key = workspace.to_string();
            for (sid, f) in &subscribers {
                if Some(*sid) != origin {
                    f(&key, delta);
                }
            }
        }
    }
}

#[cfg(test)]
mod facts_tests;
#[cfg(test)]
mod pane_tests;
#[cfg(test)]
mod tab_tests;
#[cfg(test)]
mod transaction_tests;
#[cfg(test)]
mod workspace_tests;
