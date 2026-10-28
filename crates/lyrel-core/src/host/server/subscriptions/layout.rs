//! Machine subscription forwarding and bounded backlog resynchronization.
use std::sync::Arc;
use std::sync::atomic::Ordering;

use crate::core::machine;
use crate::daemon::control::{ControlEvent, ControlServerMsg};

use super::super::output::Sink;
use super::super::{LAYOUT_EVENT_QUEUE, Services};

pub(in super::super) fn subscribe_machine(
    services: &Services,
    sink: &Arc<Sink>,
) -> Option<machine::Subscription> {
    let store = services.machine.as_ref()?;
    let (tx, rx) = smol::channel::bounded::<(String, machine::LayoutDelta)>(LAYOUT_EVENT_QUEUE);
    let lagged = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let saw_drop = Arc::clone(&lagged);
    let subscription = store.subscribe(Arc::new(
        move |workspace: &str, delta: &machine::LayoutDelta| {
            if tx.try_send((workspace.to_string(), delta.clone())).is_err() {
                saw_drop.store(true, Ordering::Release);
                log::warn!(
                    "dropping a layout delta for a peer {LAYOUT_EVENT_QUEUE} deltas behind; \
                     it will be told to resync"
                );
            }
        },
    ));
    spawn_layout_forwarder(rx, Arc::clone(sink), lagged);
    Some(subscription)
}

fn spawn_layout_forwarder(
    rx: smol::channel::Receiver<(String, machine::LayoutDelta)>,
    sink: Arc<Sink>,
    lagged: Arc<std::sync::atomic::AtomicBool>,
) {
    let spawned = std::thread::Builder::new()
        .name("lyrel-control-layout".into())
        .spawn(move || {
            while let Ok((workspace, delta)) = rx.recv_blocking() {
                if lagged.swap(false, Ordering::AcqRel) {
                    let mut superseded = 1;
                    while rx.try_recv().is_ok() {
                        superseded += 1;
                    }
                    log::info!(
                        "dropping {superseded} superseded layout delta(s) and asking the peer \
                         to resync"
                    );
                    if sink
                        .send(&ControlServerMsg::Event(ControlEvent::LayoutResync))
                        .is_err()
                    {
                        return;
                    }
                    continue;
                }
                let event = ControlEvent::Layout { workspace, delta };
                if sink.send(&ControlServerMsg::Event(event)).is_err() {
                    return;
                }
            }
        });
    if let Err(e) = spawned {
        log::warn!("could not start the layout forwarder: {e}");
    }
}

#[cfg(all(test, unix))]
mod tests;
