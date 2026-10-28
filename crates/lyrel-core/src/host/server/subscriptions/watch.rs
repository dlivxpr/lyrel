//! Directory subscription ownership and reply-before-events activation.
use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::daemon::control::{ControlEvent, ControlServerMsg, WATCH_BURST_CAP};
use crate::host::{SharedHost, WatchSub};

use super::super::output::Sink;

pub(in super::super) struct Watches {
    watches: Mutex<HashMap<u64, WatchSub>>,
    deferred: Mutex<HashMap<u64, (u64, smol::channel::Receiver<Vec<PathBuf>>)>>,
    next: AtomicU64,
}

impl Watches {
    pub(in super::super) fn new() -> Self {
        Self {
            watches: Mutex::new(HashMap::new()),
            deferred: Mutex::new(HashMap::new()),
            next: AtomicU64::new(1),
        }
    }

    // Park the receiver under the request ID: no forwarder may race the WatchId reply.
    pub(in super::super) fn open(&self, host: &SharedHost, req_id: u64, dirs: &[PathBuf]) -> io::Result<u64> {
        let sub = host.watch(dirs)?;
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let rx = sub.events().clone();
        self.watches
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id, sub);
        self.deferred
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(req_id, (id, rx));
        Ok(id)
    }

    // Called only after reply delivery (or its cancellation/encoding failure) is known.
    pub(in super::super) fn after_reply(&self, sink: &Arc<Sink>, req_id: u64, deliver: bool) {
        let parked = self
            .deferred
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&req_id);
        let Some((id, rx)) = parked else {
            return;
        };
        if !deliver {
            self.watches
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&id);
            return;
        }
        spawn_watch_forwarder(id, rx, Arc::clone(sink));
    }

    pub(in super::super) fn set_dirs(&self, id: u64, dirs: &[PathBuf]) -> io::Result<()> {
        let watches = self.watches.lock().unwrap_or_else(|e| e.into_inner());
        let sub = watches.get(&id).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("no such watch subscription {id}"),
            )
        })?;
        sub.set_dirs(dirs)
    }

    pub(in super::super) fn close(&self, id: u64) {
        self.watches
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&id);
    }

    pub(in super::super) fn clear(&self) {
        self.watches.lock().unwrap_or_else(|e| e.into_inner()).clear();
    }
}

fn spawn_watch_forwarder(id: u64, rx: smol::channel::Receiver<Vec<PathBuf>>, sink: Arc<Sink>) {
    let spawned = std::thread::Builder::new()
        .name("lyrel-control-watch".into())
        .spawn(move || {
            while let Ok(batch) = rx.recv_blocking() {
                let event = if batch.len() > WATCH_BURST_CAP {
                    ControlEvent::WatchOverflow { id }
                } else {
                    ControlEvent::Watch {
                        id,
                        paths: batch
                            .iter()
                            .map(|p| p.to_string_lossy().into_owned())
                            .collect(),
                    }
                };
                if sink.send(&ControlServerMsg::Event(event)).is_err() {
                    return;
                }
            }
        });
    if let Err(e) = spawned {
        log::warn!("could not start the watch forwarder for subscription {id}: {e}");
    }
}

#[cfg(all(test, unix))]
mod tests;
