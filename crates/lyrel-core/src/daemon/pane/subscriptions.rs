//! 同一 PaneState 锁内拥有订阅身份、顺序回放与输出背压；不创建额外锁或消费者。
pub(crate) mod capacity;
use std::path::PathBuf;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use super::graphics_admission::GraphicsAdmissionBroker;
use super::replay::{RING_CAP, ReplayRing};
use super::{GraphicsFrame, ShellState};
use crate::core::kitty_graphics::{GraphicsEffect, GraphicsState};
use crate::core::term_modes::TerminalModes;
use crate::daemon::protocol::{DaemonMsg, RemoteContext, SshPhase, WinSize};

pub struct GraphicsReplayAdmission {
    endpoint: u64,
    offers: Vec<(u64, u32, u32, usize, usize)>,
    grants: Vec<super::graphics_admission::GraphicsGrant>,
    _work: Option<crate::core::graphics_state::retention::Work>,
}

impl GraphicsReplayAdmission {
    /// Offer waits run after releasing PaneState; the callback only checks currency.
    pub(super) fn prepare(
        endpoint: u64,
        broker: Arc<GraphicsAdmissionBroker>,
        resources: Vec<(u64, u32, u32, usize, usize)>,
        work: Option<crate::core::graphics_state::retention::Work>,
        current: impl Fn(&[(u64, u32, u32, usize, usize)]) -> bool,
    ) -> Result<Option<Self>, String> {
        let mut grants = Vec::with_capacity(resources.len());
        for &(key, width, height, encoded, metadata) in &resources {
            let Some(grant) = broker.request_after_release(
                Some(endpoint), key, width, height, encoded, metadata,
                || current(&resources),
            ) else {
                if !current(&resources) {
                    return Ok(None);
                }
                return Err("Desktop graphics budget cannot admit retained session content".into());
            };
            grants.push(grant);
        }
        Ok(Some(Self { endpoint, offers: resources, grants, _work: work }))
    }

    fn activate(&self, broker: &GraphicsAdmissionBroker, graphics: &Option<GraphicsState>) -> Result<bool, String> {
        if !graphics.as_ref().map_or(self.offers.is_empty(), |graphics| graphics.matches_offers(&self.offers))
            || !self.grants.iter().all(|grant| grant.valid())
        {
            return Ok(false);
        }
        if !broker.activate(self.endpoint) {
            return Err("Graphics replay connection closed during admission".into());
        }
        Ok(true)
    }

    fn commit(self) {
        for grant in self.grants {
            grant.commit();
        }
    }
}

pub struct OutputGate {
    queued: AtomicI64,
    park: Mutex<()>,
    drained: Condvar,
}

impl OutputGate {
    const HIGH_WATER: i64 = 16 * 1024 * 1024;
    const MAX_WAIT: Duration = Duration::from_secs(2);

    pub(crate) fn new() -> Self {
        Self {
            queued: AtomicI64::new(0),
            park: Mutex::new(()),
            drained: Condvar::new(),
        }
    }

    pub(super) fn add(&self, n: usize) {
        self.queued.fetch_add(n as i64, Ordering::Relaxed);
    }

    pub fn sub(&self, n: usize) {
        let prev = self.queued.fetch_sub(n as i64, Ordering::Relaxed);
        if prev >= Self::HIGH_WATER && prev - (n as i64) < Self::HIGH_WATER {
            let _park = self.park.lock().unwrap();
            self.drained.notify_all();
        }
    }

    pub(super) fn reset(&self) {
        self.queued.store(0, Ordering::Relaxed);
        let _park = self.park.lock().unwrap();
        self.drained.notify_all();
    }

    pub(crate) fn queued_bytes(&self) -> i64 {
        self.queued.load(Ordering::Relaxed)
    }

    pub(super) fn wait_below_high_water(&self) {
        if self.queued.load(Ordering::Relaxed) < Self::HIGH_WATER {
            return;
        }
        let deadline = std::time::Instant::now() + Self::MAX_WAIT;
        let mut park = self.park.lock().unwrap();
        while self.queued.load(Ordering::Relaxed) >= Self::HIGH_WATER {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() {
                return;
            }
            let (guard, _) = self.drained.wait_timeout(park, left).unwrap();
            park = guard;
        }
    }
}

/// What an observer may have queued and not yet written: the replay it is
/// charged for on subscribing, which can be a whole ring, and as much again
/// of live output. Were it only a ring, a full one would leave no room while
/// its replay is still going out, and the first byte the pane printed in that
/// time would drop the observer.
pub(crate) const OBSERVER_BUDGET: i64 = 2 * RING_CAP as i64;

/// What an observer dropped at its budget is told, last on its stream.
pub(crate) const OBSERVER_BEHIND: &str =
    "fell behind this pane's output; open it again to catch up";

/// The bytes a message counts against its stream's [`OutputGate`]: charged
/// when it is queued, credited back by the writer once it is on the wire. The
/// two sides must use this one measure, or a gate drifts until it throttles a
/// stream or drops an observer that has nothing queued.
pub(crate) fn gated_len(msg: &DaemonMsg) -> usize {
    match msg {
        DaemonMsg::Output(b) | DaemonMsg::Snapshot(b) => b.len(),
        // Lifted from the same PTY read as the output around them.
        DaemonMsg::Graphics(b) => b.len(),
        DaemonMsg::ClipboardWrite(b) => b.len(),
        _ => 0,
    }
}

struct Observer {
    id: u64,
    tx: Arc<Sender<DaemonMsg>>,
    gate: Arc<OutputGate>,
}


pub(super) struct Subscriptions {
    subscriber: Option<Arc<Sender<DaemonMsg>>>,
    subscriber_epoch: u64,
    observers: Vec<Observer>,
    observer_seq: u64,
    allow_remote_clipboard_write: bool,
    // Native SSH keeps its profile permission across controller replacement.
    clipboard_write_from_spec: Option<bool>,
    admission: Arc<GraphicsAdmissionBroker>,
}

/// Borrowed replay inputs, not a second owner of pane or foreground state.
pub(super) struct ReplayState<'a> {
    pub ring: &'a ReplayRing,
    pub modes: &'a TerminalModes,
    pub shell: &'a ShellState,
    pub remote: &'a Option<RemoteContext>,
    pub cwd: &'a Option<PathBuf>,
    pub ssh_phase: &'a Option<SshPhase>,
    pub agent: Option<crate::core::cli_agent::CLIAgent>,
    pub agent_session: &'a Option<crate::core::cli_agent::AgentSessionState>,
    pub alive: bool,
    pub exit_code: Option<i32>,
}

impl Subscriptions {
    pub(super) fn new(allow_clipboard: bool, spec_permission: Option<bool>) -> Self {
        Self {
            subscriber: None,
            subscriber_epoch: 0,
            observers: Vec::new(),
            observer_seq: 0,
            allow_remote_clipboard_write: allow_clipboard,
            clipboard_write_from_spec: spec_permission,
            admission: Arc::new(GraphicsAdmissionBroker::default()),
        }
    }

    pub(super) fn admission(&self) -> &Arc<GraphicsAdmissionBroker> {
        &self.admission
    }

    pub(super) fn set_graphics_capacity(&self, graphics: &mut Option<GraphicsState>, encoded: u64, decoded: u64) -> bool {
        graphics.as_mut().is_none_or(|graphics| graphics.set_capacity(
            usize::try_from(encoded).unwrap_or(usize::MAX),
            usize::try_from(decoded).unwrap_or(usize::MAX),
        ))
    }

    pub(super) fn replay_offers(&mut self, graphics: &mut Option<GraphicsState>, gate: &OutputGate) -> Result<(Vec<(u64, u32, u32, usize, usize)>, Option<crate::core::graphics_state::retention::Work>), String> {
        self.publish_evictions(graphics, gate);
        match graphics {
            Some(graphics) => {
                let (resources, work) = graphics.offer_resources()
                    .ok_or("Graphics replay metadata work budget exhausted")?;
                Ok((resources, Some(work)))
            }
            None => Ok((Vec::new(), None)),
        }
    }

    fn publish_evictions(&mut self, graphics: &mut Option<GraphicsState>, gate: &OutputGate) {
        let effects = graphics.as_mut().map(GraphicsState::drain_evictions).unwrap_or_default();
        self.publish_graphics(effects, gate, graphics);
    }

    pub(super) fn controller_epoch(&self) -> Option<u64> {
        self.subscriber.as_ref().map(|_| self.subscriber_epoch)
    }

    // Ownership checks intentionally retain the existing epoch-only semantics.
    pub(super) fn controls(&self, epoch: u64) -> bool {
        self.subscriber_epoch == epoch
    }

    pub(super) fn clipboard_allowed(&self) -> bool {
        self.allow_remote_clipboard_write
    }

    pub(super) fn has_observer(&self, id: u64) -> bool {
        self.observers.iter().any(|observer| observer.id == id)
    }

    pub(super) fn contains(&self, sender: &Arc<Sender<DaemonMsg>>) -> bool {
        self.subscriber.as_ref().is_some_and(|s| Arc::ptr_eq(s, sender))
            || self.observers.iter().any(|o| Arc::ptr_eq(&o.tx, sender))
    }

    fn prune_viewports(&self, graphics: &mut Option<GraphicsState>) {
        if let Some(graphics) = graphics {
            graphics.retain_viewports(|owner| {
                self.subscriber.as_ref().is_some_and(|sender| owner.as_ptr() == Arc::as_ptr(sender))
                    || self.observers.iter().any(|observer| owner.as_ptr() == Arc::as_ptr(&observer.tx))
            });
        }
    }

    pub(super) fn tell_controller(&self, msg: DaemonMsg) -> bool {
        self.subscriber.as_ref().is_some_and(|sub| sub.send(msg).is_ok())
    }

    pub(super) fn tell_observer(&self, id: u64, msg: DaemonMsg) -> bool {
        self.observers.iter().find(|observer| observer.id == id)
            .is_some_and(|observer| observer.tx.send(msg).is_ok())
    }

    pub(super) fn notify(&mut self, msg: DaemonMsg, graphics: &mut Option<GraphicsState>) {
        if let Some(sub) = &self.subscriber {
            let _ = sub.send(msg.clone());
        }
        // Status frames retain the existing budget check without a byte charge.
        self.observers.retain(|observer| send_to_observer(observer, &msg, 0));
        self.prune_viewports(graphics);
    }

    pub(super) fn resize(&mut self, size: WinSize) {
        if let Some(sub) = &self.subscriber {
            let _ = sub.send(DaemonMsg::Size(size));
        }
        self.observers.retain(|observer| observer.tx.send(DaemonMsg::Size(size)).is_ok());
    }

    fn transport_failed(&mut self, graphics: &mut Option<GraphicsState>) {
        self.notify(DaemonMsg::Error(
            "Graphics transport budget exhausted; reconnect to recover retained content".into(),
        ), graphics);
        self.subscriber = None;
        self.subscriber_epoch += 1;
        self.observers.clear();
    }

    fn broadcast(&mut self, msg: DaemonMsg, len: usize, gate: &OutputGate, graphics: &mut Option<GraphicsState>) {
        if let Some(sub) = &self.subscriber {
            if sub.send(msg.clone()).is_ok() {
                gate.add(len);
            }
        }
        self.observers.retain(|observer| send_to_observer(observer, &msg, len));
        self.prune_viewports(graphics);
    }

    pub(super) fn publish_graphics(&mut self, effects: Vec<GraphicsEffect>, gate: &OutputGate, graphics: &mut Option<GraphicsState>) {
        for effect in effects {
            let packet = match effect {
                GraphicsEffect::Update(update) => update.encode_packet(),
                GraphicsEffect::Frame(packet) => Some(packet),
                _ => continue,
            };
            let Some(packet) = packet else {
                self.transport_failed(graphics);
                return;
            };
            let len = packet.len();
            self.broadcast(DaemonMsg::Graphics(packet), len, gate, graphics);
        }
    }

    pub(super) fn output(&mut self, bytes: &[u8], frames: Vec<GraphicsFrame>, gate: &OutputGate, graphics: &mut Option<GraphicsState>) {
        if frames.is_empty() {
            if !bytes.is_empty() {
                self.broadcast(DaemonMsg::Output(bytes.to_vec()), bytes.len(), gate, graphics);
            }
            return;
        }
        for frame in frames {
            match frame {
                GraphicsFrame::Output(bytes) => {
                    if !bytes.is_empty() {
                        let len = bytes.len();
                        self.broadcast(DaemonMsg::Output(bytes), len, gate, graphics);
                    }
                }
                GraphicsFrame::Graphics(packet) => {
                    let len = packet.len();
                    self.broadcast(DaemonMsg::Graphics(packet), len, gate, graphics);
                }
                GraphicsFrame::TransportFull => {
                    self.transport_failed(graphics);
                    return;
                }
                GraphicsFrame::ClipboardWrite(frame) => {
                    let len = frame.len();
                    if self.tell_controller(DaemonMsg::ClipboardWrite(frame)) {
                        gate.add(len);
                    }
                }
            }
        }
    }

    pub(super) fn attach(&mut self, replay: ReplayState<'_>, graphics: &mut Option<GraphicsState>, subscriber: impl Into<Arc<Sender<DaemonMsg>>>, gate: &OutputGate, allow_clipboard: bool, foreground: bool) -> u64 {
        let subscriber = subscriber.into();
        self.subscriber_epoch += 1;
        self.allow_remote_clipboard_write = self.clipboard_write_from_spec.unwrap_or(allow_clipboard);
        gate.reset();
        let epoch = self.subscriber_epoch;
        let (queued, complete) = replay_state(&replay, graphics.as_ref(), &subscriber, foreground);
        gate.add(queued);
        if complete {
            self.subscriber = Some(subscriber);
        } else {
            self.subscriber = None;
            self.subscriber_epoch += 1;
        }
        self.prune_viewports(graphics);
        epoch
    }

    pub(super) fn observe(&mut self, replay: ReplayState<'_>, graphics: &Option<GraphicsState>, observer: impl Into<Arc<Sender<DaemonMsg>>>, gate: Arc<OutputGate>, foreground: bool) -> u64 {
        let observer = observer.into();
        self.observer_seq += 1;
        // Charge the entire replay before exposing this observer to live output.
        let (queued, complete) = replay_state(&replay, graphics.as_ref(), &observer, foreground);
        gate.add(queued);
        if complete {
            self.observers.push(Observer { id: self.observer_seq, tx: observer, gate });
        }
        self.observer_seq
    }

    pub(super) fn attach_admitted(&mut self, replay: ReplayState<'_>, graphics: &mut Option<GraphicsState>, subscriber: Arc<Sender<DaemonMsg>>, gate: &OutputGate, allow_clipboard: bool, admission: GraphicsReplayAdmission, foreground: bool) -> Result<Option<u64>, String> {
        self.publish_evictions(graphics, gate);
        if !admission.activate(&self.admission, graphics)? {
            return Ok(None);
        }
        let epoch = self.attach(replay, graphics, subscriber, gate, allow_clipboard, foreground);
        if self.controller_epoch() != Some(epoch) {
            return Err("Graphics replay failed".into());
        }
        admission.commit();
        Ok(Some(epoch))
    }

    pub(super) fn observe_admitted(&mut self, replay: ReplayState<'_>, graphics: &mut Option<GraphicsState>, subscriber: Arc<Sender<DaemonMsg>>, gate: Arc<OutputGate>, controller_gate: &OutputGate, admission: GraphicsReplayAdmission, foreground: bool) -> Result<Option<u64>, String> {
        self.publish_evictions(graphics, controller_gate);
        if !admission.activate(&self.admission, graphics)? {
            return Ok(None);
        }
        let id = self.observe(replay, graphics, subscriber, gate, foreground);
        if !self.has_observer(id) {
            return Err("Graphics replay failed".into());
        }
        admission.commit();
        Ok(Some(id))
    }


    pub(super) fn detach(&mut self, epoch: u64, gate: &OutputGate, graphics: &mut Option<GraphicsState>) {
        if self.controls(epoch) {
            self.subscriber = None;
            self.prune_viewports(graphics);
            self.allow_remote_clipboard_write = self.clipboard_write_from_spec.unwrap_or(false);
            gate.reset();
        }
    }

    pub(super) fn unobserve(&mut self, id: u64, graphics: &mut Option<GraphicsState>) {
        self.observers.retain(|observer| observer.id != id);
        self.prune_viewports(graphics);
    }

    /// Reader-ordered death releases graphics before canceling admission.
    pub(super) fn release_graphics(&self, graphics: &mut Option<GraphicsState>) {
        *graphics = None;
        self.admission.cancel();
    }

    /// Explicit shutdown cancels pending admission before releasing authority.
    pub(super) fn shutdown_graphics(&self, graphics: &mut Option<GraphicsState>) {
        self.admission.cancel();
        *graphics = None;
    }
}

fn send_to_observer(observer: &Observer, msg: &DaemonMsg, len: usize) -> bool {
    if observer.gate.queued_bytes() + len as i64 > OBSERVER_BUDGET {
        let _ = observer.tx.send(DaemonMsg::Error(OBSERVER_BEHIND.to_string()));
        return false;
    }
    if observer.tx.send(msg.clone()).is_err() {
        return false;
    }
    observer.gate.add(len);
    true
}

fn replay_state(
    st: &ReplayState<'_>,
    graphics: Option<&GraphicsState>,
    subscriber: &Sender<DaemonMsg>,
    foreground_command: bool,
) -> (usize, bool) {
    // What the replay queues is charged to the stream's gate like live output,
    // by the same measure its writer credits back (`gated_len`).
    let mut queued = 0;
    let mut send = |msg: DaemonMsg| {
        let len = gated_len(&msg);
        if subscriber.send(msg).is_ok() {
            queued += len;
        }
    };
    // Ahead of the ring, not after it: a client that is put into the alternate
    // screen first paints the replayed frames into the buffer they belong to.
    //
    // Only the modes the ring cannot switch on itself, though. Re-entering an
    // alternate screen the ring still carries is not a harmless duplicate —
    // the emulator makes `?1049h` a no-op when the mode is already on, so the
    // ring's own copy stops clearing the alternate screen, and everything the
    // ring holds from *before* that sequence (the shell scrollback the user
    // had behind the program) is painted into the alternate buffer, which has
    // no history to keep it and is left behind when the program exits. What
    // the ring carries always wins on its own terms.
    if let Some(modes) = st.ring.replay_prelude(&st.modes) {
        send(DaemonMsg::Snapshot(modes));
    }
    st.ring.replay(&mut send);
    if let Some(graphics) = graphics {
        let Some((effects, _snapshot_work)) = graphics.replay() else {
            send(DaemonMsg::Error(
                "Graphics replay metadata work budget exhausted".into(),
            ));
            return (queued, false);
        };
        for effect in effects {
            let packet = match effect {
                GraphicsEffect::Frame(packet) => Some(packet),
                GraphicsEffect::Update(update) => update.encode_packet(),
                _ => continue,
            };
            let Some(packet) = packet else {
                send(DaemonMsg::Error("Graphics replay transport budget exhausted; reconnect when capacity is available".into()));
                return (queued, false);
            };
            send(DaemonMsg::Graphics(packet));
        }
    }
    if st.shell.active {
        send(DaemonMsg::Prompt {
            active: st.shell.active,
            at_prompt: st.shell.at_prompt && !foreground_command,
            last_exit: st.shell.last_exit_code,
        });
    }
    if st.remote.is_some() {
        send(DaemonMsg::RemoteContext((*st.remote).clone()));
    }
    // After the remote context, never before it. A client drops its cwd on
    // every `RemoteContext`, because live that frame means the pane just hopped
    // and the old directory belongs to the other side. Replayed, it is only
    // the standing context, and `st.cwd` is already the far shell's own report
    // (`apply_remote_context` clears it on every hop). Sent first, it would be
    // wiped, and a native SSH pane reopened by a client would have no remote
    // directory for ⌘T or a split to start the new pane in until its next
    // prompt.
    if let Some(cwd) = &st.cwd {
        send(DaemonMsg::Cwd(cwd.clone()));
    }
    if let Some(phase) = &st.ssh_phase {
        send(DaemonMsg::SshStatus {
            phase: phase.clone(),
        });
    }
    if st.agent.is_some() {
        send(DaemonMsg::Agent(st.agent));
    }
    if st.agent_session.is_some() {
        send(DaemonMsg::AgentStatus((*st.agent_session).clone()));
    }
    if !st.alive {
        send(DaemonMsg::Exited { code: st.exit_code });
    }
    (queued, true)
}


#[cfg(test)]
mod tests;
