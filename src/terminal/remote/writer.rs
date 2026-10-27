//! Pane 写队列、背压和有界非阻塞关闭；reader 只持有可克隆发送端。
use super::{EventProxy, PaneEvent};
use crate::daemon::protocol::ClientMsg;
use crate::daemon::transport::Stream;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

/// How much unsent input a pane holds before it calls the link lost. A link
/// that is draining never gets near this: the sender thread is parked waiting
/// for work, so the queue holds at most the frames of one burst. Reaching it
/// means nothing on the far side has taken a byte for as long as it takes to
/// type — or paste — four megabytes, which is a dead link, not a slow one.
const MAX_BACKLOG: usize = 4 << 20;

/// How long a teardown lets the sender finish what is already queued before it
/// cuts the socket out from under it. `Detach` is the last frame a pane sends
/// and it is worth a moment: on a draining link the sender is idle, so it goes
/// out in microseconds and this returns at once. On a link that has stopped
/// draining it will never go out at all, and closing the pane must not wait
/// around to discover that — the daemon reads the closed socket as a detach
/// anyway.
const DRAIN_GRACE: std::time::Duration = std::time::Duration::from_millis(50);

/// What the sender thread needs to report a link that stopped taking input.
/// The signals the pane holds, cloned out so the failure can be raised from the
/// thread that actually meets it.
#[derive(Clone)]
pub(super) struct InputLoss {
    /// Set by the first frame *this* link refused, so the loss is said once
    /// rather than once per keystroke. One per link rather than one per pane: a
    /// relink hands the retired sender's last, doomed write and the new
    /// sender's first real one two different flags, so the dying link cannot
    /// spend the new link's one chance to speak.
    said: Arc<AtomicBool>,
    reader_quit: Arc<AtomicBool>,
    exited: Arc<AtomicBool>,
    proxy: EventProxy,
}

impl InputLoss {
    pub(super) fn new(
        reader_quit: Arc<AtomicBool>,
        exited: Arc<AtomicBool>,
        proxy: EventProxy,
    ) -> InputLoss {
        InputLoss {
            said: Arc::new(AtomicBool::new(false)),
            reader_quit,
            exited,
            proxy,
        }
    }

    /// The link refused a frame. Every keystroke after the first would say the
    /// same thing, so this side says it once; and unless the reader has been
    /// retired for a relink or a release, the pane is marked exited the way the
    /// reader marks it on EOF — it is the same socket, noticed from the writing
    /// side first — so the window shows the pane as gone instead of taking
    /// input into it that nothing will ever read. The reader still raises its
    /// own `Exit` when it finds the same socket closed; the handler is
    /// idempotent, so a link that is genuinely gone may be reported twice.
    ///
    /// This is hardening for a *closed* link, not the cure for #673: a socket
    /// some process holds open and never reads accepts writes into its send
    /// buffer, so they succeed and vanish until the buffer fills, and nothing
    /// here fires until the backlog bound does. What stops that pane existing
    /// at all is `attach_on` refusing to call a silent `Attach` attached.
    fn note(&self, err: &std::io::Error) {
        if self.said.swap(true, Ordering::SeqCst) {
            return;
        }
        log::warn!("the daemon link stopped taking this pane's input: {err}");
        if self.reader_quit.load(Ordering::SeqCst) {
            return;
        }
        self.exited.store(true, Ordering::SeqCst);
        self.proxy.send(PaneEvent::Wakeup);
        self.proxy.send(PaneEvent::Exit);
    }
}

#[derive(Default)]
struct SendQueue {
    frames: VecDeque<Vec<u8>>,
    /// Bytes queued but not yet on the wire — including the batch the sender
    /// currently holds, which is why this is not just `frames`' total. That
    /// batch is exactly what a stalled link parks in, so leaving it out would
    /// mean the backlog bound could never be reached.
    bytes: usize,
    /// How much of `bytes` belongs to frames that were over the whole bound on
    /// their own, and so were let through on their own terms. The bound is
    /// raised by exactly this while they are outstanding, and put back the
    /// moment the queue empties — otherwise one big paste would leave every
    /// keystroke behind it looking like a dead link.
    oversize: usize,
    /// Set by teardown, or by a sender that has given up: either way the queue
    /// takes nothing more. Teardown's sender writes what is left first.
    closing: bool,
    /// Set by the sender once there is nothing left to write. Teardown waits
    /// on this for `DRAIN_GRACE`, no longer.
    drained: bool,
}

/// A pane's writing half, moved onto a thread of its own.
///
/// The socket underneath is blocking and has no write timeout, so a peer that
/// stops reading parks `write(2)` in the kernel until it starts again. Every
/// caller of `RemoteTerminal::write` is a gpui event handler — a keystroke, a
/// paste, a mouse report, a focus change — and one UI thread draws every
/// window, so a park there is every window frozen. On macOS a unix stream gives
/// up after 8K of send buffer, which is about 1400 keystrokes: a single paste.
///
/// So nothing on the UI thread touches the socket. Frames are encoded, queued,
/// and handed to a sender thread that is welcome to park for as long as the far
/// end makes it.
pub(super) struct LinkWriter {
    /// A second handle on the same socket, kept for `shutdown` alone.
    /// `shutdown(2)` returns at once even while another thread is parked in
    /// `write(2)` on that socket — which is exactly the state teardown has to
    /// be able to break, and exactly what a handle behind the sender's own lock
    /// could not do.
    closer: Stream,
    queue: Arc<(Mutex<SendQueue>, std::sync::Condvar)>,
    loss: InputLoss,
    thread: Option<JoinHandle<()>>,
}

#[derive(Clone)]
pub(super) struct LinkSender {
    queue: Arc<(Mutex<SendQueue>, std::sync::Condvar)>,
    loss: InputLoss,
}

impl LinkSender {
    pub(super) fn send(&self, msg: ClientMsg) {
        Self::enqueue(&self.queue, &self.loss, msg);
    }

    fn enqueue(
        queue: &Arc<(Mutex<SendQueue>, std::sync::Condvar)>,
        loss: &InputLoss,
        msg: ClientMsg,
    ) {
        let mut frame = Vec::new();
        if let Err(error) = msg.encode(&mut frame) {
            log::warn!("could not encode a frame for this pane's link: {error}");
            return;
        }
        let (lock, wake) = &**queue;
        let Ok(mut q) = lock.lock() else {
            return;
        };
        if q.closing {
            return;
        }
        let oversize = frame.len() > MAX_BACKLOG && q.bytes == 0;
        if !oversize && q.bytes + frame.len() > MAX_BACKLOG + q.oversize {
            drop(q);
            loss.note(&std::io::Error::new(
                std::io::ErrorKind::WouldBlock,
                format!("nothing has drained this pane's link for {MAX_BACKLOG} bytes of input"),
            ));
            return;
        }
        if oversize {
            q.oversize = frame.len();
        }
        q.bytes += frame.len();
        q.frames.push_back(frame);
        q.drained = false;
        drop(q);
        wake.notify_one();
    }
}

impl LinkWriter {
    pub(super) fn new(stream: Stream, loss: InputLoss) -> std::io::Result<LinkWriter> {
        let closer = stream.try_clone()?;
        let queue = Arc::new((Mutex::new(SendQueue::default()), std::sync::Condvar::new()));
        let sending = Arc::clone(&queue);
        let sender_loss = loss.clone();
        let thread = std::thread::Builder::new()
            .name("lyrel-pane-writer".into())
            .spawn(move || send_loop(stream, sending, sender_loss))?;
        Ok(LinkWriter {
            closer,
            queue,
            loss,
            thread: Some(thread),
        })
    }

    /// Queues a frame. Never blocks: the socket belongs to the sender thread,
    /// and the only thing that happens here is a push onto a `VecDeque`.
    pub(super) fn send(&self, msg: ClientMsg) {
        LinkSender::enqueue(&self.queue, &self.loss, msg);
    }

    pub(super) fn sender(&self) -> LinkSender {
        LinkSender {
            queue: self.queue.clone(),
            loss: self.loss.clone(),
        }
    }

    /// Retires the sender and cuts the link. Gives what is queued `DRAIN_GRACE`
    /// to go out — see the constant — and then shuts the socket down whether it
    /// went or not. Never joins: a sender parked on a dead link would take the
    /// UI thread down with it, the same trap `stop_reader` documents.
    ///
    /// Called twice on the way out — `stop_reader` closes the link, then the
    /// field drop closes it again — so the second call has to be free rather
    /// than another `DRAIN_GRACE` spent waiting for a sender that is already
    /// gone. A retired handle is one whose thread has been let go.
    pub(super) fn close(&mut self) {
        if self.thread.is_none() {
            return;
        }
        let (lock, wake) = &*self.queue;
        if let Ok(mut q) = lock.lock() {
            q.closing = true;
            wake.notify_one();
            if let Ok((waited, _)) = wake.wait_timeout_while(q, DRAIN_GRACE, |q| !q.drained) {
                drop(waited);
            }
        }
        let _ = self.closer.shutdown(std::net::Shutdown::Both);
        drop(self.thread.take());
    }
}

impl Drop for LinkWriter {
    fn drop(&mut self) {
        self.close();
    }
}

fn send_loop(
    mut stream: Stream,
    queue: Arc<(Mutex<SendQueue>, std::sync::Condvar)>,
    loss: InputLoss,
) {
    use std::io::Write as _;
    let (lock, wake) = &*queue;

    // Abandons whatever is still queued and reports the link settled. Wakes a
    // teardown that is inside `DRAIN_GRACE` waiting to hear it: what is left
    // here is never going out, and there is nothing to be gained by making the
    // window sit out the rest of the grace period to find that out. Closes the
    // queue on the way, too — with no thread left to drain it, anything queued
    // after this is just a keystroke held onto until the pane drops.
    let give_up = || {
        if let Ok(mut q) = lock.lock() {
            q.frames.clear();
            q.bytes = 0;
            q.oversize = 0;
            q.closing = true;
            q.drained = true;
            wake.notify_all();
        }
    };

    loop {
        let batch = {
            let Ok(mut q) = lock.lock() else { return };
            loop {
                if !q.frames.is_empty() {
                    break;
                }
                // Nothing left to write, so the link is as flushed as this
                // thread can make it. Said before the `closing` check, so a
                // teardown racing a sender that has already finished hears it
                // rather than waiting out `DRAIN_GRACE` for nothing.
                q.drained = true;
                wake.notify_all();
                if q.closing {
                    return;
                }
                let Ok(next) = wake.wait(q) else { return };
                q = next;
            }
            std::mem::take(&mut q.frames)
        };
        // Counted against the backlog until it is actually out. Discounting it
        // at the moment it left the `VecDeque` would let a sender parked on the
        // first frame of a huge batch hold the whole thing off the books, and
        // the bound the batch is meant to enforce would never be reached.
        let taken: usize = batch.iter().map(Vec::len).sum();
        // Written with the lock released: parking here is the whole point, and
        // a sender holding the queue lock while it parked would put every
        // `send` — every keystroke — behind the same wait it exists to absorb.
        for frame in batch {
            if let Err(e) = stream.write_all(&frame) {
                loss.note(&e);
                give_up();
                return;
            }
        }
        if let Err(e) = stream.flush() {
            loss.note(&e);
            give_up();
            return;
        }
        if let Ok(mut q) = lock.lock() {
            q.bytes = q.bytes.saturating_sub(taken);
            if q.bytes == 0 {
                q.oversize = 0;
            }
        }
    }
}

#[cfg(all(test, unix))]
mod consumer_tests;
