//! Per-connection open read sessions: one bound file version, bounded handles.
//!
//! The far side of a segmented read is a real open file, not a path re-resolved
//! per segment. A session lives at most until the connection ends, until its
//! client closes it, or until it has been idle for
//! [`transfer::READ_SESSION_IDLE`]; the map is bounded by
//! [`transfer::MAX_READ_SESSIONS`] so a peer cannot accumulate handles.

use std::collections::HashMap;
use std::io;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::host::HostRead;
use crate::host::Meta;
use crate::host::transfer::{
    MAX_READ_SESSIONS, READ_SESSION_IDLE, SEGMENT_BYTES_MAX,
};

struct Slot {
    reader: Mutex<Box<dyn HostRead>>,
    last_used: Mutex<Instant>,
}

#[derive(Default)]
struct Inner {
    slots: HashMap<u64, Arc<Slot>>,
}

#[derive(Default)]
pub(super) struct ReadSessions {
    inner: Mutex<Inner>,
    next: AtomicU64,
}

impl ReadSessions {
    pub(super) fn open(&self, reader: Box<dyn HostRead>) -> io::Result<(u64, Meta)> {
        let meta = reader.meta();
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        Self::reclaim_idle(&mut inner);
        if inner.slots.len() >= MAX_READ_SESSIONS {
            return Err(io::Error::other(format!(
                "the control server already holds {MAX_READ_SESSIONS} read sessions"
            )));
        }
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        inner.slots.insert(
            id,
            Arc::new(Slot {
                reader: Mutex::new(reader),
                last_used: Mutex::new(Instant::now()),
            }),
        );
        Ok((id, meta))
    }

    pub(super) fn read(&self, session: u64, max_bytes: u64) -> io::Result<(Meta, Vec<u8>)> {
        if max_bytes == 0 || max_bytes > SEGMENT_BYTES_MAX {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("a read segment must be 1..={SEGMENT_BYTES_MAX} bytes, not {max_bytes}"),
            ));
        }
        let slot = {
            let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            inner.slots.get(&session).cloned()
        };
        let Some(slot) = slot else {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "the read session closed or expired before the next segment",
            ));
        };
        let mut reader = slot.reader.lock().unwrap_or_else(|e| e.into_inner());
        let meta = reader.meta();
        let bytes = match reader.read_segment(max_bytes) {
            Ok(bytes) => bytes,
            Err(error) => {
                // A session whose version was invalidated, or that failed on the
                // host, must not keep its handle: the client will reopen.
                drop(reader);
                self.close(session);
                return Err(error);
            }
        };
        *slot.last_used.lock().unwrap_or_else(|e| e.into_inner()) = Instant::now();
        Ok((meta, bytes))
    }

    pub(super) fn close(&self, session: u64) -> bool {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .slots
            .remove(&session)
            .is_some()
    }

    /// Drop every session and its handle. Called when the connection ends.
    pub(super) fn clear(&self) {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .slots
            .clear();
    }

    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .slots
            .len()
    }

    fn reclaim_idle(inner: &mut Inner) {
        let now = Instant::now();
        inner.slots.retain(|_, slot| {
            let idle = now.duration_since(*slot.last_used.lock().unwrap_or_else(|e| e.into_inner()))
                >= READ_SESSION_IDLE;
            // A segment in progress owns its handle until it returns; never
            // reclaim a session mid-read just because the clock passed.
            !(idle && slot.reader.try_lock().is_ok())
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::read;
    use std::time::Duration;

    struct StalledRead {
        meta: Meta,
        segments: usize,
    }

    impl HostRead for StalledRead {
        fn meta(&self) -> Meta {
            self.meta
        }

        fn read_segment(&mut self, _max_bytes: u64) -> io::Result<Vec<u8>> {
            if self.segments == 0 {
                return Ok(Vec::new());
            }
            self.segments -= 1;
            Ok(vec![1])
        }
    }

    fn meta() -> Meta {
        Meta {
            is_dir: false,
            is_symlink: false,
            len: 2,
            mtime: None,
            readonly: false,
        }
    }

    #[test]
    fn an_unknown_session_reports_expiry_not_empty_content() {
        let sessions = ReadSessions::default();
        let error = sessions.read(9, 1).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert!(!read::is_file_changed(&error));
    }

    #[test]
    fn closing_a_session_releases_it() {
        let sessions = ReadSessions::default();
        let (id, opened) = sessions
            .open(Box::new(StalledRead {
                meta: meta(),
                segments: 2,
            }))
            .unwrap();
        assert_eq!(opened, meta());
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions.read(id, 1).unwrap().1, vec![1]);
        assert!(sessions.close(id));
        assert_eq!(sessions.len(), 0);
        assert_eq!(sessions.read(id, 1).unwrap_err().kind(), io::ErrorKind::TimedOut);
    }

    #[test]
    fn a_segment_size_outside_the_wire_bound_is_refused() {
        let sessions = ReadSessions::default();
        let (id, _) = sessions
            .open(Box::new(StalledRead {
                meta: meta(),
                segments: 1,
            }))
            .unwrap();
        assert_eq!(sessions.read(id, 0).unwrap_err().kind(), io::ErrorKind::InvalidInput);
        assert_eq!(
            sessions.read(id, SEGMENT_BYTES_MAX + 1).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
        assert_eq!(sessions.len(), 1, "a refused size does not drop the session");
    }

    #[test]
    fn idle_sessions_are_reclaimed_before_the_ceiling_is_hit() {
        let sessions = ReadSessions::default();
        let (first, _) = sessions
            .open(Box::new(StalledRead {
                meta: meta(),
                segments: 0,
            }))
            .unwrap();
        // Backdate the session instead of waiting out the real timeout.
        {
            let inner = sessions.inner.lock().unwrap();
            let slot = inner.slots.get(&first).unwrap();
            *slot.last_used.lock().unwrap() =
                Instant::now() - (READ_SESSION_IDLE + Duration::from_secs(1));
        }
        for _ in 0..MAX_READ_SESSIONS {
            sessions
                .open(Box::new(StalledRead {
                    meta: meta(),
                    segments: 0,
                }))
                .unwrap();
        }
        assert_eq!(sessions.len(), MAX_READ_SESSIONS);
        assert_eq!(
            sessions.read(first, 1).unwrap_err().kind(),
            io::ErrorKind::TimedOut,
            "the idle session was reclaimed"
        );
    }

    struct ChangedRead;
    impl HostRead for ChangedRead {
        fn meta(&self) -> Meta {
            meta()
        }
        fn read_segment(&mut self, _max_bytes: u64) -> io::Result<Vec<u8>> {
            Err(read::file_changed("changed under the read"))
        }
    }

    #[test]
    fn a_changed_file_invalidates_its_session() {
        let sessions = ReadSessions::default();
        let (id, _) = sessions.open(Box::new(ChangedRead)).unwrap();
        let error = sessions.read(id, 1).unwrap_err();
        assert!(read::is_file_changed(&error));
        assert_eq!(sessions.len(), 0);
    }
}
