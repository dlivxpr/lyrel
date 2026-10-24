//! Per-pane desktop admission. This mutex never acquires a pane-state lock.
use std::collections::BTreeMap;
use std::sync::{Arc, Condvar, Mutex, Weak, mpsc::Sender};
use std::time::{Duration, Instant};

use crate::daemon::protocol::DaemonMsg;

struct Endpoint { sender: Weak<Sender<DaemonMsg>>, active: bool }
struct Pending { endpoint: u64, accepted: Option<bool> }
#[derive(Default)]
struct State {
    sequence: u64,
    revision: u64,
    endpoints: BTreeMap<u64, Endpoint>,
    pending: BTreeMap<u64, Pending>,
}
#[derive(Default)]
pub struct GraphicsAdmissionBroker { state: Mutex<State>, changed: Condvar }

pub(crate) struct GraphicsGrant {
    broker: Arc<GraphicsAdmissionBroker>,
    revision: u64,
    tokens: Vec<u64>,
    committed: bool,
}
impl GraphicsGrant {
    pub(crate) fn valid(&self) -> bool { self.broker.state.lock().unwrap().revision == self.revision }
    pub(crate) fn commit(mut self) {
        let mut state = self.broker.state.lock().unwrap();
        for token in &self.tokens { state.pending.remove(token); }
        self.committed = true;
    }
}
impl Drop for GraphicsGrant {
    fn drop(&mut self) {
        if self.committed { return; }
        let mut state = self.broker.state.lock().unwrap();
        for token in &self.tokens {
            if let Some(pending) = state.pending.remove(token)
                && let Some(sender) = state.endpoints.get(&pending.endpoint).and_then(|endpoint| endpoint.sender.upgrade()) {
                let _ = sender.send(DaemonMsg::GraphicsCancel { token: *token });
            }
        }
    }
}

impl GraphicsAdmissionBroker {
    /// Register a pending replay endpoint. It does not receive live offers until
    /// its replay and generation have been admitted under the pane output lock.
    pub fn register(&self, endpoint: u64, sender: &Arc<Sender<DaemonMsg>>) {
        let mut state = self.state.lock().unwrap();
        state.revision += 1;
        state.endpoints.insert(endpoint, Endpoint { sender: Arc::downgrade(sender), active: false });
        self.changed.notify_all();
    }
    pub fn unregister(&self, endpoint: u64) {
        let mut state = self.state.lock().unwrap();
        if state.endpoints.remove(&endpoint).is_some() { state.revision += 1; self.changed.notify_all(); }
    }
    pub fn cancel(&self) {
        let mut state = self.state.lock().unwrap();
        state.revision += 1;
        self.changed.notify_all();
    }
    pub fn grant(&self, endpoint: u64, token: u64, accepted: bool) {
        let mut state = self.state.lock().unwrap();
        if let Some(pending) = state.pending.get_mut(&token)
            && pending.endpoint == endpoint && pending.accepted.is_none() {
            pending.accepted = Some(accepted);
            self.changed.notify_all();
        }
    }
    pub(crate) fn activate(&self, endpoint: u64) -> bool {
        let mut state = self.state.lock().unwrap();
        let Some(endpoint) = state.endpoints.get_mut(&endpoint) else { return false; };
        endpoint.active = true;
        true
    }
    pub(crate) fn request_after_release(self: &Arc<Self>, only: Option<u64>, key: u64, width: u32, height: u32, encoded_bytes: usize, metadata_bytes: usize,
        current: impl Fn() -> bool,
    ) -> Option<GraphicsGrant> {
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            if !current() { return None; }
            if let Some(grant) = self.request(only, key, width, height, encoded_bytes, metadata_bytes) { return Some(grant); }
            let remaining = deadline.checked_duration_since(Instant::now())?;
            // The desktop's retirement/atlas compaction owns its actual lease.
            // Never infer release from sending Pressure or Evicted alone.
            std::thread::sleep(remaining.min(Duration::from_millis(350)));
            if Instant::now() >= deadline { return None; }
        }
    }
    pub(crate) fn request(self: &Arc<Self>, only: Option<u64>, key: u64, width: u32, height: u32, encoded_bytes: usize, metadata_bytes: usize) -> Option<GraphicsGrant> {
        let mut state = self.state.lock().unwrap();
        let revision = state.revision;
        let endpoints: Vec<_> = state.endpoints.iter().filter(|(id, endpoint)| only.map_or(endpoint.active, |wanted| **id == wanted))
            .filter_map(|(&id, endpoint)| endpoint.sender.upgrade().map(|sender| (id, sender))).collect();
        if only.is_some() && endpoints.is_empty() { return None; }
        let mut grant = GraphicsGrant { broker: self.clone(), revision, tokens: Vec::with_capacity(endpoints.len()), committed: false };
        for (endpoint, sender) in endpoints {
            state.sequence = state.sequence.checked_add(1).expect("graphics offer identity exhausted");
            let token = state.sequence;
            state.pending.insert(token, Pending { endpoint, accepted: None });
            grant.tokens.push(token);
            if sender.send(DaemonMsg::GraphicsOffer { token, key, width, height, encoded_bytes: encoded_bytes as u64, metadata_bytes: metadata_bytes as u64 }).is_err() {
                drop(state); return None;
            }
        }
        // A desktop that cannot service admission must not stall shell output
        // indefinitely. Cancellation, detach, clear and close wake this wait.
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            if state.revision != revision || grant.tokens.iter().any(|token| state.pending.get(token).is_none_or(|p| p.accepted == Some(false))) {
                drop(state); return None;
            }
            if grant.tokens.iter().all(|token| state.pending.get(token).is_some_and(|p| p.accepted == Some(true))) {
                drop(state); return Some(grant);
            }
            let Some(remaining) = deadline.checked_duration_since(Instant::now()) else { drop(state); return None; };
            let (next, _) = self.changed.wait_timeout(state, remaining).unwrap();
            state = next;
        }
    }
}
