//! Git stream capacity and synchronous chunk backpressure through the one Sink.
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::daemon::control::{
    ControlEvent, ControlServerMsg, GIT_STREAM_CHUNK, GIT_STREAM_CHUNK_MAX,
    MAX_CONCURRENT_GIT_STREAMS,
};
use crate::host::SharedHost;

use super::super::output::Sink;

pub(in super::super) struct GitStreams {
    active: Arc<AtomicUsize>,
}

impl GitStreams {
    pub(in super::super) fn new() -> Self {
        Self {
            active: Arc::new(AtomicUsize::new(0)),
        }
    }

    pub(in super::super) fn start(
        &self,
        host: &SharedHost,
        output: &Arc<Sink>,
        id: u64,
        cwd: PathBuf,
        args: Vec<String>,
    ) {
        let host = Arc::clone(host);
        let sink = Arc::clone(output);
        let slot = StreamSlot(Arc::clone(&self.active));
        if slot.0.fetch_add(1, Ordering::AcqRel) >= MAX_CONCURRENT_GIT_STREAMS {
            drop(slot);
            let _ = sink.send(&ControlServerMsg::Event(ControlEvent::GitEnd {
                id,
                code: None,
                failed: true,
            }));
            return;
        }
        let spawned = std::thread::Builder::new()
            .name("lyrel-control-git-stream".into())
            .spawn(move || {
                let _slot = slot;
                let borrowed: Vec<&str> = args.iter().map(String::as_str).collect();
                let mut batch: Vec<u8> = Vec::with_capacity(GIT_STREAM_CHUNK);
                let mut stopped: Option<StreamStop> = None;
                let flush = |batch: &mut Vec<u8>, stopped: &mut Option<StreamStop>| {
                    if stopped.is_some() {
                        batch.clear();
                        return;
                    }
                    if batch.is_empty() {
                        return;
                    }
                    let bytes = std::mem::take(batch);
                    for piece in bytes.chunks(GIT_STREAM_CHUNK_MAX) {
                        let event = ControlServerMsg::Event(ControlEvent::GitChunk {
                            id,
                            bytes: piece.to_vec(),
                        });
                        let Ok((kind, payload)) = event.to_frame() else {
                            *stopped = Some(StreamStop::Unencodable);
                            return;
                        };
                        if sink.send_frame(kind, &payload).is_err() {
                            *stopped = Some(StreamStop::LinkGone);
                            return;
                        }
                    }
                };
                let result = host.git_lines(&cwd, &borrowed, &mut |line| {
                    if stopped.is_some() {
                        return;
                    }
                    batch.extend_from_slice(line.as_bytes());
                    batch.push(b'\n');
                    if batch.len() >= GIT_STREAM_CHUNK {
                        flush(&mut batch, &mut stopped);
                    }
                });
                flush(&mut batch, &mut stopped);
                if matches!(stopped, Some(StreamStop::LinkGone)) {
                    return;
                }
                let (code, failed) = match (stopped, result) {
                    (Some(StreamStop::Unencodable), _) => (None, true),
                    (_, Ok(code)) => (code, false),
                    (_, Err(_)) => (None, true),
                };
                let _ = sink.send(&ControlServerMsg::Event(ControlEvent::GitEnd {
                    id,
                    code,
                    failed,
                }));
            });
        if spawned.is_err() {
            let _ = output.send(&ControlServerMsg::Event(ControlEvent::GitEnd {
                id,
                code: None,
                failed: true,
            }));
        }
    }
}

struct StreamSlot(Arc<AtomicUsize>);

impl Drop for StreamSlot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum StreamStop {
    LinkGone,
    Unencodable,
}

#[cfg(all(test, unix))]
mod tests;
