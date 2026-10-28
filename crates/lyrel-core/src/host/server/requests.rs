//! Request correlation, cancellation eligibility, and reply completion.
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::daemon::control::{ControlReply, ControlServerMsg, WireError};

use super::output::Sink;
use super::subscriptions::Watches;

pub(super) struct Requests {
    inflight: Mutex<HashMap<u64, bool>>,
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
            .insert(req_id, false);
    }

    pub(super) fn is_cancelled(&self, req_id: u64) -> bool {
        self.inflight
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&req_id)
            .copied()
            .unwrap_or(false)
    }

    pub(super) fn forget(&self, req_id: u64) {
        self.inflight
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&req_id);
    }

    pub(super) fn cancel(&self, req_id: u64) {
        let mut f = self.inflight.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(flag) = f.get_mut(&req_id) {
            *flag = true;
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
            .unwrap_or(false);
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
