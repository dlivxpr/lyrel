//! Request correlation, cancellation eligibility, and reply completion.
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::daemon::control::{ControlReply, ControlServerMsg, WireError};

use super::output::Sink;
use super::subscriptions::Watches;

/// One in-flight request.
///
/// `cancelled` is the correlation the reply path uses. `interrupt` is the
/// operation's own stop button: a SQLite statement that is already running
/// cannot see a flag, so the request registers `sqlite3_interrupt` (through
/// its page token) under its own id. The hook is called at most once, and only
/// for this request — an old `CANCEL` that arrives after a newer query on the
/// same view started finds the old entry gone and does nothing.
#[derive(Default)]
struct Inflight {
    cancelled: bool,
    interrupt: Option<Arc<dyn Fn() + Send + Sync>>,
}

pub(super) struct Requests {
    inflight: Mutex<HashMap<u64, Inflight>>,
}

impl Requests {
    pub(super) fn new() -> Self {
        Self {
            inflight: Mutex::new(HashMap::new()),
        }
    }

    pub(super) fn register(&self, req_id: u64) {
        self.inflight
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(req_id, Inflight::default());
    }

    pub(super) fn is_cancelled(&self, req_id: u64) -> bool {
        self.inflight
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&req_id)
            .is_some_and(|entry| entry.cancelled)
    }

    /// Give one in-flight request its stop button.
    ///
    /// A cancellation that arrived before the job reached this point calls the
    /// hook immediately, so the statement never starts. A request that has
    /// already finished has no entry; its hook is not installed and never
    /// fires, because nothing is left to stop.
    pub(super) fn set_interrupt(&self, req_id: u64, interrupt: Arc<dyn Fn() + Send + Sync>) {
        let already_cancelled = {
            let mut inflight = self.inflight.lock().unwrap_or_else(|e| e.into_inner());
            match inflight.get_mut(&req_id) {
                Some(entry) if entry.cancelled => true,
                Some(entry) => {
                    entry.interrupt = Some(interrupt);
                    false
                }
                None => return,
            }
        };
        if already_cancelled {
            interrupt();
        }
    }

    pub(super) fn forget(&self, req_id: u64) {
        self.inflight
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&req_id);
    }

    pub(super) fn cancel(&self, req_id: u64) {
        let hook = {
            let mut inflight = self.inflight.lock().unwrap_or_else(|e| e.into_inner());
            let Some(entry) = inflight.get_mut(&req_id) else {
                return;
            };
            entry.cancelled = true;
            entry.interrupt.take()
        };
        // Outside the lock: the hook interrupts a statement (or posts a
        // control CANCEL) and must not run while request correlation is
        // unavailable to another thread.
        if let Some(hook) = hook {
            hook();
        }
    }

    // Remove correlation before taking the output lock; never retain inflight while
    // sending or activating a watch. Cancellation drops only the reply, not host work.
    pub(super) fn finish(
        &self,
        sink: &Arc<Sink>,
        watches: &Watches,
        req_id: u64,
        reply: ControlReply,
        blob: Vec<u8>,
        wants_blob: bool,
    ) {
        let cancelled = self
            .inflight
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&req_id)
            .unwrap_or_default()
            .cancelled;
        if cancelled {
            log::trace!("control request {req_id} was cancelled; dropping its reply");
            watches.after_reply(sink, req_id, false);
            return;
        }

        let msg = if wants_blob && matches!(reply, ControlReply::Ok(_)) {
            ControlServerMsg::ResponseBlob {
                req_id,
                reply,
                blob,
            }
        } else {
            ControlServerMsg::Response { req_id, reply }
        };
        let delivered = match msg.to_frame() {
            Ok((k, payload)) => match sink.send_frame(k, &payload) {
                Ok(()) => true,
                Err(e) => {
                    log::debug!("control reply {req_id} could not be written: {e}");
                    false
                }
            },
            Err(e) => {
                log::warn!("control reply {req_id} could not be encoded: {e}");
                let excuse = ControlServerMsg::Response {
                    req_id,
                    reply: ControlReply::Err(WireError::from_io(&e)),
                };
                if let Err(e) = sink.send(&excuse) {
                    log::debug!("control error reply {req_id} could not be written: {e}");
                }
                false
            }
        };

        watches.after_reply(sink, req_id, delivered);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn a_cancel_runs_only_the_requests_own_interrupt_once() {
        let requests = Requests::new();
        requests.register(7);
        requests.register(8);
        let seven = Arc::new(AtomicUsize::new(0));
        let eight = Arc::new(AtomicUsize::new(0));
        requests.set_interrupt(7, {
            let seven = Arc::clone(&seven);
            Arc::new(move || {
                seven.fetch_add(1, Ordering::SeqCst);
            })
        });
        requests.set_interrupt(8, {
            let eight = Arc::clone(&eight);
            Arc::new(move || {
                eight.fetch_add(1, Ordering::SeqCst);
            })
        });
        requests.cancel(7);
        requests.cancel(7);
        assert_eq!(seven.load(Ordering::SeqCst), 1);
        assert_eq!(eight.load(Ordering::SeqCst), 0);
        assert!(requests.is_cancelled(7));
        assert!(!requests.is_cancelled(8));
    }

    #[test]
    fn an_old_cancel_cannot_reach_a_newer_request() {
        let requests = Requests::new();
        let old = Arc::new(AtomicUsize::new(0));
        requests.register(1);
        requests.set_interrupt(1, {
            let old = Arc::clone(&old);
            Arc::new(move || {
                old.fetch_add(1, Ordering::SeqCst);
            })
        });
        requests.forget(1);
        let new = Arc::new(AtomicUsize::new(0));
        requests.register(2);
        requests.set_interrupt(2, {
            let new = Arc::clone(&new);
            Arc::new(move || {
                new.fetch_add(1, Ordering::SeqCst);
            })
        });
        requests.cancel(1);
        assert_eq!(
            old.load(Ordering::SeqCst),
            0,
            "the finished request is gone"
        );
        assert_eq!(new.load(Ordering::SeqCst), 0);
        assert!(!requests.is_cancelled(2));
    }

    #[test]
    fn a_cancel_before_the_job_installs_its_interrupt_still_stops_it() {
        let requests = Requests::new();
        requests.register(3);
        requests.cancel(3);
        let fired = Arc::new(AtomicUsize::new(0));
        requests.set_interrupt(3, {
            let fired = Arc::clone(&fired);
            Arc::new(move || {
                fired.fetch_add(1, Ordering::SeqCst);
            })
        });
        assert_eq!(fired.load(Ordering::SeqCst), 1);
    }
}
