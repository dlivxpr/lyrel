//! 单个 pane 链接的安装、替换、resize 同步与退休。
//! successor 安装前先退休 reader；任何关闭路径均不 join socket 线程。
use super::reader::ReaderTask;
use super::writer::{InputLoss, LinkWriter};
use super::{
    EventProxy, GraphicsViewportReporter, PaneEvent, PaneRoute, ReaderSignals, ShellState,
    TermSize, win_size,
};
use crate::daemon::protocol::{ClientMsg, WinSize};
use crate::daemon::transport::Stream;
use crate::terminal::bridge::ParseBridge;
use lyrel_core::core::term_modes::KeyboardStacks;
use parking_lot::FairMutex;
use rio_vt::crosswords::Crosswords;
use rio_vt::performer::handler::Handler as _;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// How long a resize the daemon promised to echo is taken to still be on its
/// way. Past this, a grid that has not reached the requested geometry is not
/// waiting on anything: the request or its echo went missing, and the size is
/// sent again. The local daemon echoes in well under a millisecond and a
/// routed one within a round trip; a resend that beats a slow echo costs one
/// `TIOCSWINSZ` of the size the pty already has, which signals nobody.
const RESIZE_ECHO_GRACE: std::time::Duration = std::time::Duration::from_secs(1);

/// Whether `term`'s grid is `size`, as the emulator would size it — it never
/// goes below `MIN_COLUMNS` × `MIN_SCREEN_LINES`, so a sliver of a pane asking
/// for one column is already as narrow as it can get.
fn grid_holds(term: &Crosswords<EventProxy>, size: TermSize) -> bool {
    use crate::terminal::size::{MIN_COLUMNS, MIN_SCREEN_LINES};
    term.columns() == size.cols.max(MIN_COLUMNS)
        && term.screen_lines() == size.rows.max(MIN_SCREEN_LINES)
}

pub(super) struct PaneConnection {
    link: LinkWriter,
    reader: ReaderTask,
    route: PaneRoute,
    size: TermSize,
    synced_size: bool,
    synced_cell: (u16, u16),
    resize_sent_at: Option<std::time::Instant>,
    spawn_claim: Option<(u64, WinSize)>,
    viewport_reporter: GraphicsViewportReporter,
}

impl PaneConnection {
    pub(super) fn start(
        stream: Stream,
        buffered: Vec<u8>,
        size: TermSize,
        term: &Arc<FairMutex<Crosswords<EventProxy>>>,
        proxy: &EventProxy,
        signals: ReaderSignals,
    ) -> anyhow::Result<Self> {
        let read_half = stream.try_clone()?;
        let quit = Arc::new(AtomicBool::new(false));
        let link = LinkWriter::new(
            stream,
            InputLoss::new(quit.clone(), signals.exited.clone(), proxy.clone()),
        )?;
        let viewport_reporter =
            GraphicsViewportReporter::new(term, signals.images.clone(), link.sender())?;
        let reader = ReaderTask::spawn(
            term.clone(),
            proxy.clone(),
            read_half,
            buffered,
            quit,
            signals,
            link.sender(),
            viewport_reporter.clone(),
        );
        Ok(Self {
            link,
            reader,
            viewport_reporter,
            route: PaneRoute::Local,
            size,
            synced_size: false,
            synced_cell: (0, 0),
            resize_sent_at: None,
            spawn_claim: None,
        })
    }

    pub(super) fn finish_spawn(&mut self, route: PaneRoute, claim: Option<(u64, WinSize)>) {
        self.route = route;
        self.spawn_claim = claim;
    }

    pub(super) fn claim_spawn(&mut self) {
        if let Some((pane_id, size)) = self.spawn_claim.take() {
            self.link.send(ClientMsg::Attach {
                pane_id,
                size,
                allow_remote_clipboard_write: self.route.allow_remote_clipboard_write(),
            });
        }
    }

    pub(super) fn send(&self, message: ClientMsg) {
        self.link.send(message);
    }

    pub(super) fn retire(&mut self, images: &crate::terminal::images::ImageStore) {
        self.reader.retire(images);
        self.link.close();
    }

    pub(super) fn detach(&mut self, images: &crate::terminal::images::ImageStore) {
        self.link.send(ClientMsg::Detach);
        self.retire(images);
    }

    pub(super) fn size(&self) -> TermSize {
        self.size
    }

    pub(super) fn viewport_reporter(&self) -> GraphicsViewportReporter {
        self.viewport_reporter.clone()
    }

    pub(super) fn adopt_relink(
        &mut self,
        stream: Stream,
        term: &Arc<FairMutex<Crosswords<EventProxy>>>,
        proxy: &EventProxy,
        events: &smol::channel::Receiver<PaneEvent>,
        exited: &mut bool,
        signals: ReaderSignals,
        buffered: Vec<u8>,
        route: &PaneRoute,
    ) -> anyhow::Result<()> {
        self.retire(&signals.images);
        while events.try_recv().is_ok() {}
        if let Ok(mut pending) = signals.clipboard_writes.lock() {
            pending.clear();
        }
        signals.clipboard_write_busy.store(false, Ordering::Release);

        let read_half = stream.try_clone()?;

        // A relink keeps the view, and the view keeps the status it last saw
        // before the link dropped. That is already a baseline, and the better
        // one: if the agent finished its turn while the link was down, the
        // daemon's replayed `Done` against the view's `Working` is exactly the
        // edge the reader must be told about. So the relink's replay is read
        // as a live report, and a cold-attach mark nobody took yet is dropped
        // rather than left to swallow that edge.
        if let Ok(mut guard) = signals.agent_session.lock() {
            guard.replayed = false;
        }

        signals.exited.store(false, Ordering::SeqCst);
        *exited = false;
        {
            let mut term = term.lock();
            ParseBridge::new(
                &mut term,
                proxy.clone(),
                &mut KeyboardStacks::default(),
                Some(proxy.parse_state.history_limit.load(Ordering::Relaxed)),
            )
            .reset_state();
        }
        // The grid was just reset, so every image anchor now points nowhere.
        // Drop them; the daemon does not replay out-of-band image frames, so a
        // browser redraws on its next transmit (see issue #213's reattach note).
        signals.images.clear_for_relink();

        let quit = Arc::new(AtomicBool::new(false));
        let link = LinkWriter::new(
            stream,
            InputLoss::new(quit.clone(), signals.exited.clone(), proxy.clone()),
        )?;
        let reader = ReaderTask::spawn(
            term.clone(),
            proxy.clone(),
            read_half,
            buffered,
            quit.clone(),
            signals,
            link.sender(),
            self.viewport_reporter.clone(),
        );
        self.reader = reader;
        // Installed after the reader, so the sender reports a refusal
        // against the reader this link actually has. Assigning retires the old
        // `LinkWriter` through its `Drop`, which is what closes the old socket.
        self.link = link;
        self.viewport_reporter.relink(self.link.sender());
        self.route = route.clone();
        self.synced_size = false;
        Ok(())
    }

    /// Whether the daemon behind this pane echoes a `DaemonMsg::Size` into the
    /// output stream when it applies our `ClientMsg::Resize`. When it does, the
    /// local grid reflow is deferred to that echo on the reader thread: any
    /// backlog still queued between daemon and client was produced at the old
    /// geometry, and reflowing before it drains parses old-width bytes into a
    /// new-width grid (maximize during a burst of output garbled the pane).
    ///
    /// The local daemon is probed directly. A routed pane consults the answer
    /// its route carries, read off the host's control hello when the route was
    /// built — the daemon serves one `ClientMsg` per connection, so asking
    /// `Version` per pane would cost a whole routed connection each time. The
    /// remaining cases never defer: an older server does not advertise the
    /// echo, an unroutable route reaches no daemon at all, and deferral must
    /// not hang the grid on an echo nobody promised.
    fn resize_echoed(&self) -> bool {
        match &self.route {
            PaneRoute::Local => crate::daemon::spawn::local_daemon_supports(
                crate::daemon::protocol::FEATURE_RESIZE_ECHO,
            ),
            PaneRoute::Remote { resize_echo, .. } => *resize_echo,
            PaneRoute::Unroutable(_) => false,
        }
    }

    pub(super) fn resize(
        &mut self,
        term: &Arc<FairMutex<Crosswords<EventProxy>>>,
        proxy: &EventProxy,
        size: TermSize,
        cell_w: u16,
        cell_h: u16,
        local_conpty: &AtomicBool,
        shell: &Mutex<ShellState>,
        images: &crate::terminal::images::ImageStore,
    ) {
        images.set_cell_metrics(cell_w, cell_h);
        let echoed = self.resize_echoed();
        // The cell size has to be part of the early-out, not just cols/rows: it
        // is reported in *device* pixels, so moving the window between a 2x and
        // a 1x display changes `ws_xpixel`/`ws_ypixel` while the grid stays
        // exactly the same. Comparing only `size` there would skip the resize
        // and leave a pixel-aware child rendering for the old framebuffer.
        let cell = (cell_w, cell_h);
        if self.synced_size && size == self.size && cell == self.synced_cell {
            // Already asked for, so the only question is whether the grid got
            // there. It is not enough to trust the request (#893): on an
            // echoing route the grid moves only when the daemon's `Size` comes
            // back, and a request or an echo that went missing left a pane
            // painting a grid shorter than its bounds — the bottom rows blank,
            // the child still drawing for the old size — with every later
            // frame asking for the same size and being skipped here, until a
            // divider drag asked for a different one.
            if echoed {
                // Disagreement while an echo can still be on its way is just
                // the echo (or a replay segment mid-apply) not having landed
                // yet. Asked first because it needs no lock.
                if self
                    .resize_sent_at
                    .is_some_and(|at| at.elapsed() < RESIZE_ECHO_GRACE)
                {
                    return;
                }
                // Tried rather than waited for, as the painter does: this runs
                // every frame, and a held lock is the reader mid-batch — the
                // next frame asks again.
                match term.try_lock() {
                    Some(term) if !grid_holds(&term, size) => {}
                    _ => return,
                }
            } else if grid_holds(&term.lock(), size) {
                return;
            }
        }
        self.synced_size = true;
        self.size = size;
        self.synced_cell = cell;
        *proxy.parse_state.paint_cursor.lock().unwrap() = None;
        if !echoed {
            let local_conpty = local_conpty.load(Ordering::Relaxed);
            let shell_redraws = !local_conpty
                && shell
                    .lock()
                    .map(|s| s.active && s.at_prompt)
                    .unwrap_or(false);
            crate::terminal::prompt_reflow::resize(
                &mut term.lock(),
                size,
                shell_redraws,
                local_conpty,
            );
        }

        let win = win_size(size, cell_w, cell_h);
        self.link.send(ClientMsg::Resize(win));
        self.resize_sent_at = Some(std::time::Instant::now());
    }
}

#[cfg(all(test, unix))]
mod resize_tests;
#[cfg(all(test, windows))]
mod windows_tests;
