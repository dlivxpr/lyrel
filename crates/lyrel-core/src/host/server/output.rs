//! One synchronous output lock for replies, events, and retirement.
use std::io::{self, Write};
use std::sync::Mutex;

use crate::daemon::control::ControlServerMsg;

pub(super) struct Sink {
    out: Mutex<Option<Box<dyn Write + Send>>>,
}

impl Sink {
    pub(super) fn new<W: Write + Send + 'static>(w: W) -> Sink {
        Sink {
            out: Mutex::new(Some(Box::new(w))),
        }
    }

    pub(super) fn send(&self, msg: &ControlServerMsg) -> io::Result<()> {
        let (k, payload) = msg.to_frame()?;
        self.send_frame(k, &payload)
    }

    pub(super) fn send_frame(&self, k: u8, payload: &[u8]) -> io::Result<()> {
        let mut slot = self.out.lock().unwrap_or_else(|e| e.into_inner());
        let w = slot.as_mut().ok_or_else(|| {
            io::Error::new(io::ErrorKind::BrokenPipe, "control connection is closed")
        })?;
        crate::daemon::protocol::write_frame(&mut *w, k, payload).and_then(|()| w.flush())
    }

    pub(super) fn retire(&self) {
        *self.out.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
}
