//! 单一有序 pane 消费者：流位置、解析器与 decoder 代次属于同一个 reader。
//! 连接编排只启动或退休它，不能单独推进 replay、文字或图形状态。

use super::{
    EventProxy, GraphicsViewportReporter, LinkSender, PaneEvent, ReaderSignals, ShellState,
    TermSize,
};
use std::collections::VecDeque;
use std::io::Read as _;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread::JoinHandle;

use crate::core::osc::Note;
use crate::daemon::protocol::{ClientMsg, DaemonMsg, RemoteKind};
use crate::daemon::transport::Stream;
use crate::terminal::bridge::ParseBridge;
use lyrel_core::core::term_modes::KeyboardStacks;
use parking_lot::FairMutex;
use rio_vt::crosswords::Crosswords;
use rio_vt::crosswords::Mode;
use rio_vt::crosswords::grid::Dimensions as _;
use rio_vt::event::EventListener;

#[cfg(test)]
use super::handshake::attach_reply_prefix;
#[cfg(test)]
use super::{
    NativeSshSpec, PaneRoute, PtySource, RemoteContext, RemoteTerminal, WinSize, test_term,
};
use crate::core::osc::{OscTokenizer, TitleEffect, TitleLifetime};
use crate::terminal::command_cursor::{CommandCursorStyle, CommandMark};
use crate::terminal::parked_cursor::{CursorCut, ParkedCursorRepair, ParkedCursorScanner};
use crate::terminal::prompt_reflow::{PromptBreak, PromptMark};
use lyrel_core::core::term_modes::{COLOR_SCHEME_UPDATES, TerminalModes};
use rio_vt::performer::handler::{Handler as _, Processor};

/// One reader lifetime, retired without joining a potentially silent socket.
pub(super) struct ReaderTask {
    quit: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl ReaderTask {
    pub(super) fn spawn(
        term: Arc<FairMutex<Crosswords<EventProxy>>>,
        proxy: EventProxy,
        read_half: Stream,
        buffered: Vec<u8>,
        quit: Arc<AtomicBool>,
        signals: ReaderSignals,
        writer: LinkSender,
        viewport_reporter: GraphicsViewportReporter,
    ) -> Self {
        let pressure_displayed = viewport_reporter.displayed();
        let pressure_term = Arc::downgrade(&term);
        let pressure_writer = writer.clone();
        signals.images.register_pressure(
            move |store| {
                use lyrel_core::core::kitty_graphics::placeholder_row;
                let term = pressure_term.upgrade()?;
                let term = term.lock();
                let offset = term.display_offset() as i32;
                let rows: Vec<_> = (0..term.screen_lines())
                    .filter_map(|row| {
                        let slices = placeholder_row(
                            &term.grid,
                            rio_vt::crosswords::pos::Line(row as i32 - offset),
                        );
                        (!slices.is_empty()).then_some((row as i32, slices))
                    })
                    .collect();
                let displayed = pressure_displayed.load(Ordering::Relaxed);
                let origin = term.grid.lines_evicted() as i64 + term.grid.history_size() as i64;
                store.oldest_reclaimable(
                    term.screen_lines(),
                    term.columns(),
                    origin,
                    offset as usize,
                    displayed,
                    &rows,
                )
            },
            move |key| pressure_writer.send(ClientMsg::GraphicsPressure { key }),
        );
        // Establish the decoder generation before scheduling the reader. An old
        // thread starting late must not become the successor's image authority.
        let image_decoder = {
            let wake = proxy.clone();
            crate::terminal::images::DecodeWorker::spawn(signals.images.clone(), move || {
                wake.send(PaneEvent::Wakeup);
            })
        };
        let reader_quit = quit.clone();
        let thread = std::thread::Builder::new()
            .name("lyrel-remote-reader".to_string())
            .spawn(move || {
                crate::core::threads::promote_to_user_interactive();
                FrameReader::new(buffered).consume(
                    term,
                    proxy,
                    read_half,
                    quit,
                    signals,
                    writer,
                    viewport_reporter,
                    image_decoder,
                );
            })
            .expect("spawn remote reader thread");
        Self {
            quit: reader_quit,
            thread: Some(thread),
        }
    }

    /// Fence shared mutations and old decoder completions before replacing a link.
    /// Socket shutdown remains the writer's responsibility; this never waits.
    pub(super) fn retire(&mut self, images: &crate::terminal::images::ImageStore) {
        self.quit.store(true, Ordering::SeqCst);
        images.retire_reader();
        drop(self.thread.take());
    }

    #[cfg(all(test, windows))]
    pub(super) fn take_thread(&mut self) -> JoinHandle<()> {
        self.thread
            .take()
            .expect("the reader thread is still owned")
    }
}

/// Private stream-position state. Only consume advances it, on one reader thread.
struct FrameReader {
    processor: Processor,
    keyboard: KeyboardStacks,
    osc: OscNotifyScanner,
    mode_tok: OscTokenizer,
    zle_tok: OscTokenizer,
    title_tok: OscTokenizer,
    title_life: TitleLifetime,
    replaying_state: bool,
    cursor_scan: ParkedCursorScanner,
    parked_cursor: ParkedCursorRepair,
    command_tok: OscTokenizer,
    command_cursor: CommandCursorStyle,
    prompt_break: PromptBreak,
    modes: TerminalModes,
    report_scheme: bool,
    pending: Vec<u8>,
    scratch: Vec<u8>,
    out_batch: Vec<u8>,
}

impl FrameReader {
    fn new(buffered: Vec<u8>) -> Self {
        Self {
            processor: Processor::default(),
            keyboard: KeyboardStacks::default(),
            osc: OscNotifyScanner::default(),
            mode_tok: OscTokenizer::new(&[b"133"]),
            zle_tok: OscTokenizer::new(&[b"133"]),
            // Same title retirement rule as the daemon, cut at command boundaries.
            title_tok: OscTokenizer::new(&[b"0", b"2", b"133"]),
            title_life: TitleLifetime::default(),
            // No live Output yet: initial Snapshots and stored state are replay.
            replaying_state: true,
            cursor_scan: ParkedCursorScanner::new(),
            parked_cursor: ParkedCursorRepair::default(),
            command_tok: OscTokenizer::new(&[b"133"]),
            command_cursor: CommandCursorStyle::default(),
            prompt_break: PromptBreak::default(),
            modes: TerminalModes::new(),
            report_scheme: false,
            pending: buffered,
            scratch: vec![0u8; 256 * 1024],
            out_batch: Vec::new(),
        }
    }

    fn consume(
        self,
        term: Arc<FairMutex<Crosswords<EventProxy>>>,
        proxy: EventProxy,
        mut stream: Stream,
        quit: Arc<AtomicBool>,
        signals: ReaderSignals,
        writer: LinkSender,
        viewport_reporter: GraphicsViewportReporter,
        image_decoder: crate::terminal::images::DecodeWorker,
    ) {
        let Self {
            mut processor,
            mut keyboard,
            mut osc,
            mut mode_tok,
            mut zle_tok,
            mut title_tok,
            mut title_life,
            mut replaying_state,
            mut cursor_scan,
            mut parked_cursor,
            mut command_tok,
            mut command_cursor,
            mut prompt_break,
            mut modes,
            mut report_scheme,
            mut pending,
            mut scratch,
            mut out_batch,
        } = self;
        let ReaderSignals {
            cwd,
            shell,
            remote,
            agent,
            agent_session,
            awaiting_replay,
            exited: exited_flag,
            child_exited,
            zle_reading,
            shell_vi_mode,
            running_command,
            auth,
            phase,
            images: _,
            clipboard_writes,
            osc_notes,
            clipboard_write_busy,
            local_conpty,
            color_scheme_updates,
        } = signals;
        let mut awaiting_replay = awaiting_replay;
        *proxy.parse_state.paint_cursor.lock().unwrap() = None;
        color_scheme_updates.store(false, Ordering::Relaxed);
        let trace = std::env::var("LYREL_TRACE").is_ok_and(|v| !v.is_empty() && v != "0");
        let mut tr_last = std::time::Instant::now();
        let mut tr_bytes: u64 = 0;
        let mut tr_reads: u32 = 0;
        let mut tr_read_t = std::time::Duration::ZERO;
        let mut tr_lock_t = std::time::Duration::ZERO;
        let mut tr_adv_t = std::time::Duration::ZERO;
        let mut tr_frames: u32 = 0;

        let teardown = |reason: Option<&str>| {
            // A retired reader exits silently: the pane is being
            // relinked or released, not dying — marking it exited
            // would wrongly kill the freshly adopted link.
            if quit.load(Ordering::SeqCst) {
                return;
            }
            // Said out loud because every way a link dies looks the
            // same from the window — a pane suddenly "disconnected" —
            // and this line is the only record of which way it was.
            if let Some(reason) = reason {
                log::warn!("a pane's link died: {reason}");
            }
            term.lock().exit();
            exited_flag.store(true, Ordering::SeqCst);
            proxy.send(PaneEvent::Wakeup);
            proxy.send(PaneEvent::Exit);
        };

        'main: loop {
            macro_rules! flush_batch {
                () => {
                    if !out_batch.is_empty() {
                        // The scanner reports an offset one past each
                        // sequence it matched, in ascending order, so
                        // the batch splits at each of them: advance the
                        // emulator to the cut, act on the state that
                        // sequence left behind, carry on.
                        let mut cuts: Vec<(usize, ReaderCut)> = Vec::new();
                        if local_conpty.load(Ordering::Relaxed) {
                            cursor_scan
                                .feed(&out_batch, |off, c| cuts.push((off, ReaderCut::Parked(c))));
                        }
                        command_cuts(&mut command_tok, &out_batch, &mut cuts);
                        title_cuts(&mut title_tok, &mut title_life, &out_batch, &mut cuts);
                        let cursor_at_end = cuts.last().is_some_and(|(off, cut)| {
                            *off == out_batch.len() && matches!(cut, ReaderCut::Parked(_))
                        });
                        let mut cuts_left = cuts.len();
                        if local_conpty.load(Ordering::Relaxed) {
                            *proxy.parse_state.paint_cursor.lock().unwrap() = None;
                        }
                        {
                            let t0 = trace.then(std::time::Instant::now);
                            let Some(waited) = feed_grid(
                                &term,
                                &mut processor,
                                proxy.clone(),
                                &mut keyboard,
                                &out_batch,
                                cuts,
                                Some(&proxy.parse_state.history_limit),
                                &quit,
                                |term, cut| {
                                    cuts_left -= 1;
                                    match cut {
                                        ReaderCut::Parked(cut) => {
                                            let position = parked_cursor.apply(term, cut);
                                            if cursor_at_end && cuts_left == 0 {
                                                *proxy.parse_state.paint_cursor.lock().unwrap() =
                                                    position;
                                            }
                                        }
                                        ReaderCut::Command(mark) => {
                                            command_cursor.apply(term, mark)
                                        }
                                        ReaderCut::Prompt(mark) => prompt_break.apply(term, mark),
                                        ReaderCut::RetireTitle => term.set_title(None),
                                    }
                                },
                            ) else {
                                return;
                            };
                            if let Some(t0) = t0 {
                                tr_lock_t += waited;
                                tr_adv_t += t0.elapsed() - waited;
                            }
                        }
                        if cursor_at_end && processor.sync_timeout().sync_timeout().is_some() {
                            *proxy.parse_state.paint_cursor.lock().unwrap() = None;
                        }
                        if let Ok(mut notes) = osc_notes.lock() {
                            osc.feed(&out_batch, &mut *notes);
                        }
                        mode_tok.feed(&out_batch, |payload| {
                            if let Some(mode) = payload.strip_prefix(b"133;V;") {
                                shell_vi_mode.store(mode.first() == Some(&b'1'), Ordering::Relaxed);
                            }
                        });
                        zle_tok.feed(&out_batch, |payload| {
                            if let Some(mark) = payload.strip_prefix(b"133;") {
                                match mark.first() {
                                    Some(b'B') => {
                                        zle_reading.store(true, Ordering::Relaxed);
                                        // Back at a prompt: whatever
                                        // ran is over, so the name of
                                        // it must not outlive it into
                                        // the next close question.
                                        if let Ok(mut cmd) = running_command.lock() {
                                            cmd.clear();
                                        }
                                    }
                                    Some(b'V') => {
                                        shell_vi_mode.store(
                                            mark.strip_prefix(b"V;")
                                                .is_some_and(|v| v.first() == Some(&b'1')),
                                            Ordering::Relaxed,
                                        );
                                    }
                                    Some(b'C') => {
                                        zle_reading.store(false, Ordering::Relaxed);
                                        // `C` alone is a command start
                                        // with no line to report — the
                                        // PowerShell path sends that —
                                        // and leaves the field empty
                                        // rather than holding a stale
                                        // one.
                                        let line = mark
                                            .strip_suffix(b";aid=lyrel")
                                            .unwrap_or(mark)
                                            .strip_prefix(b"C;")
                                            .map(|c| String::from_utf8_lossy(c).into_owned())
                                            .unwrap_or_default();
                                        if let Ok(mut cmd) = running_command.lock() {
                                            *cmd = line;
                                        }
                                    }
                                    _ => zle_reading.store(false, Ordering::Relaxed),
                                }
                            }
                        });
                        proxy.send(PaneEvent::Wakeup);
                        out_batch.clear();
                    }
                };
            }

            loop {
                // Checked per frame, not just per read: a retired
                // reader may still hold complete frames in `pending`,
                // and every arm below writes shared state that
                // outlives a relink (cwd, prompt, agent — and
                // `Exited`, which would close the freshly adopted
                // pane via `child_exited`).
                if quit.load(Ordering::SeqCst) {
                    return;
                }
                let frame = match crate::daemon::protocol::take_frame(&mut pending) {
                    Ok(Some(frame)) => frame,
                    Ok(None) => break,
                    Err(e) => {
                        teardown(Some(&format!("unframeable bytes on the link: {e}")));
                        break 'main;
                    }
                };
                let msg = match DaemonMsg::from_frame(frame.0, frame.1) {
                    Ok(msg) => msg,
                    Err(e) => {
                        teardown(Some(&format!("undecodable frame on the link: {e}")));
                        break 'main;
                    }
                };
                // Once per replay (a modes Snapshot, then one per
                // ring segment), at the first frame past the ring —
                // not when what was read runs out: an 8 MiB ring
                // takes many reads, and a read that ends between the
                // segment holding a dead program's `?2031h` and the
                // one holding the prompt that ended it would report
                // into the shell. A live Output reports once folded.
                if !matches!(
                    msg,
                    DaemonMsg::Size(_) | DaemonMsg::Snapshot(_) | DaemonMsg::Output(_)
                ) && std::mem::take(&mut report_scheme)
                    && modes.is_on(COLOR_SCHEME_UPDATES)
                {
                    proxy.send(crate::terminal::color_scheme::query_reply());
                }
                match msg {
                    // Geometry, applied at this exact stream position.
                    // During replay each ring segment is preceded by
                    // its Size; live, the daemon echoes one when it
                    // applies our Resize (FEATURE_RESIZE_ECHO), which
                    // is the point in the stream where the bytes stop
                    // being old-width — everything still queued before
                    // this frame must be parsed into the old grid.
                    DaemonMsg::Size(ws) => {
                        image_decoder.store_metrics(ws.cell_w, ws.cell_h);
                        flush_batch!();
                        *proxy.parse_state.paint_cursor.lock().unwrap() = None;
                        // A live echo lands just before the shell's
                        // SIGWINCH redraw; a replayed Size heads old
                        // bytes that nothing is going to repaint.
                        let shell_redraws = !replaying_state
                            && !local_conpty.load(Ordering::Relaxed)
                            && shell.lock().is_ok_and(|s| s.active && s.at_prompt);
                        let conpty = local_conpty.load(Ordering::Relaxed);
                        {
                            let mut term = term.lock();
                            if quit.load(Ordering::SeqCst) {
                                return;
                            }
                            crate::terminal::prompt_reflow::resize(
                                &mut term,
                                TermSize::new(ws.cols as usize, ws.rows as usize),
                                shell_redraws,
                                conpty,
                            );
                        }
                        proxy.send(PaneEvent::Wakeup);
                    }
                    DaemonMsg::Snapshot(bytes) => {
                        flush_batch!();
                        cursor_scan.reset();
                        parked_cursor.reset();
                        *proxy.parse_state.paint_cursor.lock().unwrap() = None;
                        proxy.parse_state.replaying.store(true, Ordering::Relaxed);
                        // The replay carries the command marks too, so
                        // a pane reattached after `nvim` exited in it
                        // comes back with the prompt's cursor.
                        let mut cuts: Vec<(usize, ReaderCut)> = Vec::new();
                        command_cuts(&mut command_tok, &bytes, &mut cuts);
                        title_cuts(&mut title_tok, &mut title_life, &bytes, &mut cuts);
                        let fed = feed_grid(
                            &term,
                            &mut processor,
                            proxy.clone(),
                            &mut keyboard,
                            &bytes,
                            cuts,
                            Some(&proxy.parse_state.history_limit),
                            &quit,
                            |term, cut| match cut {
                                ReaderCut::Command(mark) => command_cursor.apply(term, mark),
                                ReaderCut::Prompt(mark) => prompt_break.apply(term, mark),
                                ReaderCut::RetireTitle => term.set_title(None),
                                ReaderCut::Parked(_) => {}
                            },
                        );
                        if fed.is_none() {
                            return;
                        }
                        if processor.sync_timeout().sync_timeout().is_some() {
                            let mut term = term.lock();
                            if quit.load(Ordering::SeqCst) {
                                return;
                            }
                            processor.stop_sync(&mut ParseBridge::new(
                                &mut term,
                                proxy.clone(),
                                &mut keyboard,
                                Some(proxy.parse_state.history_limit.load(Ordering::Relaxed)),
                            ));
                        }
                        mode_tok.feed(&bytes, |payload| {
                            if let Some(mode) = payload.strip_prefix(b"133;V;") {
                                shell_vi_mode.store(mode.first() == Some(&b'1'), Ordering::Relaxed);
                            }
                        });
                        proxy.parse_state.replaying.store(false, Ordering::Relaxed);
                        // A replayed `?996n` was answered long ago,
                        // but the theme may have flipped while the pane
                        // was detached, so say what it is now.
                        crate::terminal::color_scheme::fold(
                            &mut modes,
                            &bytes,
                            &color_scheme_updates,
                        );
                        modes.take_color_scheme_queries();
                        report_scheme = true;
                        proxy.send(PaneEvent::Wakeup);
                    }
                    DaemonMsg::Output(bytes) => {
                        // Live output only ever follows the whole
                        // replay (the daemon sends the stored status
                        // last, and the stream keeps that order), so
                        // the first frame here ends the window in which
                        // a status can still be a replayed one. Without
                        // this, a pane that had no agent session to
                        // replay would keep the window open until some
                        // agent it ran *later* reported for the first
                        // time, and that report would be discounted.
                        awaiting_replay = false;
                        replaying_state = false;
                        crate::terminal::color_scheme::fold(
                            &mut modes,
                            &bytes,
                            &color_scheme_updates,
                        );
                        if std::mem::take(&mut report_scheme) && modes.is_on(COLOR_SCHEME_UPDATES) {
                            proxy.send(crate::terminal::color_scheme::query_reply());
                        }
                        for _ in 0..modes.take_color_scheme_queries() {
                            proxy.send(crate::terminal::color_scheme::query_reply());
                        }
                        out_batch.extend_from_slice(&bytes);
                        tr_frames += 1;
                    }
                    DaemonMsg::Graphics(frame) => {
                        flush_batch!();
                        if let Some(update) =
                            lyrel_core::core::kitty_graphics::GraphicsUpdate::decode_frame_owned(
                                frame.into_vec(),
                            )
                        {
                            if !image_decoder.has_resource_grant(&update) {
                                teardown(Some(
                                    "graphics update arrived without its matching admission grant",
                                ));
                                let _ = stream.shutdown(std::net::Shutdown::Both);
                                return;
                            }
                            let screen = matches!(
                                &update,
                                lyrel_core::core::kitty_graphics::GraphicsUpdate::Screen { .. }
                            );
                            if screen {
                                let term = term.lock();
                                if quit.load(Ordering::SeqCst) {
                                    return;
                                }
                                let changed = image_decoder.apply(update);
                                image_decoder.sync_local_origin(
                                    term.grid.lines_evicted() as i64
                                        + term.grid.history_size() as i64,
                                );
                                if changed {
                                    proxy.send(PaneEvent::Wakeup);
                                }
                                drop(term);
                                // A hidden/startup report may have preceded the
                                // first Screen that establishes its epoch. Retry
                                // that desired viewport without waiting for paint.
                                viewport_reporter.report();
                            } else if image_decoder.apply(update) {
                                proxy.send(PaneEvent::Wakeup);
                            }
                        }
                    }
                    DaemonMsg::GraphicsOffer {
                        token,
                        key,
                        width,
                        height,
                        encoded_bytes,
                        metadata_bytes,
                    } => {
                        flush_batch!();
                        let accepted = image_decoder.offer(
                            token,
                            key,
                            width,
                            height,
                            encoded_bytes,
                            metadata_bytes,
                        );
                        writer.send(ClientMsg::GraphicsGrant { token, accepted });
                    }
                    DaemonMsg::GraphicsCancel { token } => image_decoder.cancel_offer(token),
                    DaemonMsg::GraphicsPressureResult { key, .. } => {
                        image_decoder.complete_pressure(key)
                    }
                    DaemonMsg::ClipboardWrite(frame) => {
                        flush_batch!();
                        if let Some(write) =
                            lyrel_core::core::clipboard::ClipboardWrite::decode_frame(frame)
                        {
                            if clipboard_write_busy
                                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                                .is_ok()
                            {
                                if let Ok(mut pending) = clipboard_writes.lock() {
                                    pending.push_back(write);
                                    proxy.send(PaneEvent::Wakeup);
                                } else {
                                    clipboard_write_busy.store(false, Ordering::Release);
                                }
                            } else {
                                let reply = lyrel_core::core::clipboard::response(
                                    write.id.as_deref(),
                                    "EBUSY",
                                );
                                proxy.send(PaneEvent::PtyWrite(
                                    String::from_utf8(reply).expect("OSC 5522 replies are ASCII"),
                                ));
                            }
                        } else {
                            clipboard_write_busy.store(false, Ordering::Release);
                        }
                    }
                    DaemonMsg::Cwd(path) => {
                        flush_batch!();
                        if let Ok(mut guard) = cwd.lock() {
                            *guard = Some(path);
                        }
                    }
                    DaemonMsg::Prompt {
                        active,
                        at_prompt,
                        last_exit,
                    } => {
                        flush_batch!();
                        // The replay says a command owns the pane
                        // right now. If the ring it replayed had
                        // already rolled past that command's `C`, the
                        // title just adopted from it is the command's
                        // and its `D` has to retire it (#889).
                        if replaying_state && active && !at_prompt {
                            title_life.joined_mid_command();
                        }
                        if let Ok(mut guard) = shell.lock() {
                            *guard = ShellState {
                                active,
                                at_prompt,
                                last_exit,
                                seq: guard.seq + 1,
                                cycle: guard.cycle + u64::from(at_prompt && !guard.at_prompt),
                            };
                        }
                        if active && at_prompt {
                            let mut term = term.lock();
                            if quit.load(Ordering::SeqCst) {
                                return;
                            }
                            let resets = stale_mode_resets(term.mode());
                            if !resets.is_empty() {
                                let mut bridge = ParseBridge::new(
                                    &mut term,
                                    proxy.clone(),
                                    &mut keyboard,
                                    Some(proxy.parse_state.history_limit.load(Ordering::Relaxed)),
                                );
                                processor.advance(&mut bridge, &resets);
                                drop(bridge);
                                drop(term);
                                proxy.send(PaneEvent::Wakeup);
                            }
                        }
                    }
                    DaemonMsg::RemoteContext(ctx) => {
                        flush_batch!();
                        if let Ok(mut guard) = cwd.lock() {
                            *guard = None;
                        }
                        // A native-SSH pane's pty is the far host's,
                        // however local the daemon that dialled it: the
                        // daemon bridges an ssh channel straight through
                        // and opens no ConPTY of its own. The route
                        // cannot say so — such a pane is spawned and
                        // attached through the local daemon like any
                        // other — so this frame is where a client that
                        // reopened onto an existing one finds out. A
                        // one-way latch: the far end of an ssh channel
                        // never becomes a local pty later, while an
                        // `ssh` *command* (RemoteKind::Ssh) is a program
                        // inside a ConPTY and keeps the repair.
                        if ctx
                            .as_ref()
                            .is_some_and(|c| c.kind == RemoteKind::NativeSsh)
                        {
                            local_conpty.store(false, Ordering::Relaxed);
                        }
                        if let Ok(mut guard) = remote.lock() {
                            *guard = ctx;
                        }
                    }
                    DaemonMsg::AuthPrompt { request_id, prompt } => {
                        flush_batch!();
                        if let Ok(mut guard) = auth.lock() {
                            guard.push_back((request_id, prompt));
                        }
                        proxy.send(PaneEvent::Wakeup);
                    }
                    DaemonMsg::SshStatus { phase: p } => {
                        flush_batch!();
                        if let Ok(mut guard) = phase.lock() {
                            *guard = Some(p);
                        }
                        proxy.send(PaneEvent::Wakeup);
                    }
                    DaemonMsg::Agent(a) => {
                        flush_batch!();
                        if let Ok(mut guard) = agent.lock() {
                            *guard = a;
                        }
                    }
                    DaemonMsg::AgentStatus(state) => {
                        flush_batch!();
                        if let Ok(mut guard) = agent_session.lock() {
                            guard.state = state;
                            // The first such frame on an attached link
                            // is the pane's stored status being
                            // replayed, not a turn changing state now.
                            guard.replayed = std::mem::take(&mut awaiting_replay);
                        }
                        proxy.send(PaneEvent::Wakeup);
                    }
                    DaemonMsg::Exited { .. } => {
                        flush_batch!();
                        child_exited.store(true, Ordering::SeqCst);
                        // Routine — the child ended — so no log line.
                        teardown(None);
                        break 'main;
                    }
                    _ => {}
                }
            }
            flush_batch!();

            // The read always times out within QUIT_POLL so a reader
            // parked on a silent link still notices `quit` promptly —
            // on Windows, `shutdown()` alone would never wake it.
            const QUIT_POLL: std::time::Duration = std::time::Duration::from_millis(500);
            let timeout = match processor.sync_timeout().sync_timeout() {
                Some(deadline) => {
                    let left = deadline.saturating_duration_since(std::time::Instant::now());
                    if left.is_zero() {
                        let mut term = term.lock();
                        if quit.load(Ordering::SeqCst) {
                            return;
                        }
                        processor.stop_sync(&mut ParseBridge::new(
                            &mut term,
                            proxy.clone(),
                            &mut keyboard,
                            Some(proxy.parse_state.history_limit.load(Ordering::Relaxed)),
                        ));
                        drop(term);
                        proxy.send(PaneEvent::Wakeup);
                        continue;
                    }
                    left.min(QUIT_POLL)
                }
                None => QUIT_POLL,
            };
            let _ = stream.set_read_timeout(Some(timeout));
            if trace && tr_last.elapsed() >= std::time::Duration::from_secs(1) {
                eprintln!(
                    "[trace client] {:.1} MB/s | {} reads ({} B/read) {} frames | read wait {:?} lock wait {:?} advance {:?}",
                    tr_bytes as f64 / tr_last.elapsed().as_secs_f64() / 1e6,
                    tr_reads,
                    if tr_reads > 0 {
                        tr_bytes / tr_reads as u64
                    } else {
                        0
                    },
                    tr_frames,
                    tr_read_t,
                    tr_lock_t,
                    tr_adv_t,
                );
                tr_last = std::time::Instant::now();
                tr_bytes = 0;
                tr_reads = 0;
                tr_frames = 0;
                tr_read_t = std::time::Duration::ZERO;
                tr_lock_t = std::time::Duration::ZERO;
                tr_adv_t = std::time::Duration::ZERO;
            }
            let tr0 = trace.then(std::time::Instant::now);
            match stream.read(&mut scratch) {
                Ok(0) => {
                    teardown(Some("the far end closed it (EOF)"));
                    break;
                }
                Ok(n) => {
                    if let Some(tr0) = tr0 {
                        tr_read_t += tr0.elapsed();
                        tr_reads += 1;
                        tr_bytes += n as u64;
                    }
                    let header = 4usize.saturating_sub(pending.len()).min(n);
                    pending.extend_from_slice(&scratch[..header]);
                    if pending.len() >= 4 {
                        let payload = u32::from_le_bytes(pending[..4].try_into().unwrap()) as usize;
                        if payload <= crate::daemon::protocol::MAX_FRAME {
                            let target = (5 + payload).max(pending.len() + n - header);
                            pending.reserve_exact(target.saturating_sub(pending.len()));
                        }
                    }
                    pending.extend_from_slice(&scratch[header..n]);
                }
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) => {}
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => {
                    teardown(Some(&format!("read failed: {e}")));
                    break;
                }
            }
        }
    }
}

fn stale_mode_resets(mode: Mode) -> Vec<u8> {
    let mut seq = Vec::new();
    if mode.contains(Mode::ALT_SCREEN) {
        seq.extend_from_slice(b"\x1b[?1049l");
    }
    if !mode.contains(Mode::SHOW_CURSOR) {
        seq.extend_from_slice(b"\x1b[?25h");
    }
    if mode.intersects(Mode::MOUSE_MODE) {
        seq.extend_from_slice(b"\x1b[?1000l\x1b[?1002l\x1b[?1003l");
    }
    if mode.contains(Mode::SGR_MOUSE) {
        seq.extend_from_slice(b"\x1b[?1006l");
    }
    if mode.contains(Mode::UTF8_MOUSE) {
        seq.extend_from_slice(b"\x1b[?1005l");
    }
    if mode.contains(Mode::FOCUS_IN_OUT) {
        seq.extend_from_slice(b"\x1b[?1004l");
    }
    if mode.intersects(Mode::KITTY_KEYBOARD_PROTOCOL) || mode.contains(Mode::ALT_SCREEN) {
        seq.extend_from_slice(b"\x1b[=0;1u");
    }
    seq
}

/// Notes queued for a view that has not polled: a burst keeps only its
/// newest few, so it shows as a few rather than a spray.
const MAX_OSC_NOTES: usize = 3;

struct OscNotifyScanner {
    tok: OscTokenizer,
    notes: crate::core::osc::Notifications,
}

impl Default for OscNotifyScanner {
    fn default() -> Self {
        Self {
            tok: OscTokenizer::new(&[b"9", b"99", b"777"]),
            notes: Default::default(),
        }
    }
}

impl OscNotifyScanner {
    fn feed(&mut self, bytes: &[u8], out: &mut VecDeque<Note>) {
        let notes = &mut self.notes;
        self.tok.feed(bytes, |payload| {
            if let Some(note) = parse_osc_notification(notes, payload) {
                if out.len() >= MAX_OSC_NOTES {
                    out.pop_front();
                }
                out.push_back(note);
            }
        });
    }
}

fn parse_osc_notification(
    notes: &mut crate::core::osc::Notifications,
    payload: &[u8],
) -> Option<Note> {
    if crate::core::cli_agent::parse_agent_event(payload).is_some() {
        return None;
    }
    let (title, body) = notes.parse(payload)?;
    if title.as_deref() == Some(crate::core::cli_agent::AGENT_EVENT_SENTINEL) {
        return None;
    }
    Some((title, body))
}

/// A point in a batch of pty output where the reader stops advancing the
/// emulator to act on the state the bytes before it left behind.
enum ReaderCut {
    Parked(CursorCut),
    Command(CommandMark),
    Prompt(PromptMark),
    RetireTitle,
}

/// Adds the batch's command marks to `cuts`, keeping them in stream order
/// alongside whatever cuts are already there.
fn command_cuts(tok: &mut OscTokenizer, bytes: &[u8], cuts: &mut Vec<(usize, ReaderCut)>) {
    let before = cuts.len();
    tok.feed_at(bytes, |off, payload| {
        if let Some(mark) = CommandMark::parse(payload) {
            cuts.push((off, ReaderCut::Command(mark)));
        } else if let Some(mark) = PromptMark::parse(payload) {
            cuts.push((off, ReaderCut::Prompt(mark)));
        }
    });
    if before > 0 && cuts.len() > before {
        // Stable, so a mark that ends where a cursor show does keeps its place
        // after it.
        cuts.sort_by_key(|(off, _)| *off);
    }
}

/// Retire at the command's end, before a later OSC 0/2 in the same socket
/// batch reaches Rio's title deduplication. A shell's post-command title then
/// remains the last word instead of being reset at the end of the batch.
fn title_cuts(
    tok: &mut OscTokenizer,
    lifetime: &mut TitleLifetime,
    bytes: &[u8],
    cuts: &mut Vec<(usize, ReaderCut)>,
) {
    let before = cuts.len();
    tok.feed_at(bytes, |off, payload| {
        if lifetime.saw(payload) == TitleEffect::Retire {
            cuts.push((off, ReaderCut::RetireTitle));
        }
    });
    if cuts.len() > before {
        cuts.sort_by_key(|(off, _)| *off);
    }
}

/// How much output the reader parses per hold of the grid lock. Every UI-thread
/// handler that touches the grid — a scroll, a selection drag, a keystroke —
/// queues on this lock, and one UI thread serves every window, so a hold is how
/// long all of them can stall. Alacritty's own reader lets go at the same size
/// (`MAX_LOCKED_READ`).
const MAX_LOCKED_FEED: usize = 64 * 1024;

/// Parses `bytes` into the emulator, stopping at each of `cuts` for `on_cut` to
/// act on the state the bytes before it left behind — and letting go of the
/// grid every [`MAX_LOCKED_FEED`] bytes, so a waiting handler gets its turn
/// between slices rather than after the whole batch. A relink replays up to
/// 8 MB in one frame; parsed in one hold, that froze every window for as long
/// as it took.
///
/// The lock is fair, so re-taking it queues behind whoever arrived meanwhile.
/// The parser keeps its own state across `advance` calls, so where a slice
/// ends — mid escape sequence, mid UTF-8 — makes no difference to what is
/// parsed. `proxy` is the listener the parse bridge answers device queries
/// through; `Crosswords` keeps its own private.
///
/// Returns how long it spent waiting for the lock, or `None` if `quit` was
/// raised while it waited: a retired reader must not write into a grid its
/// successor now owns.
fn feed_grid<T: EventListener + Clone>(
    term: &FairMutex<Crosswords<T>>,
    processor: &mut Processor,
    proxy: T,
    keyboard: &mut KeyboardStacks,
    bytes: &[u8],
    cuts: Vec<(usize, ReaderCut)>,
    history_limit: Option<&AtomicUsize>,
    quit: &AtomicBool,
    mut on_cut: impl FnMut(&mut Crosswords<T>, ReaderCut),
) -> Option<std::time::Duration> {
    let mut waited = std::time::Duration::ZERO;
    let mut lock = || {
        let t = std::time::Instant::now();
        let grid = term.lock();
        waited += t.elapsed();
        (!quit.load(Ordering::SeqCst)).then_some(grid)
    };
    let mut grid = lock()?;
    let mut held = 0usize;
    let mut at = 0usize;
    let mut cuts = cuts.into_iter();
    let mut next = cuts.next();
    loop {
        let stop = next.as_ref().map_or(bytes.len(), |(off, _)| *off);
        while at < stop {
            if held == MAX_LOCKED_FEED {
                drop(grid);
                grid = lock()?;
                held = 0;
            }
            let end = stop.min(at + MAX_LOCKED_FEED - held);
            {
                let mut bridge = ParseBridge::new(
                    &mut grid,
                    proxy.clone(),
                    keyboard,
                    history_limit.map(|limit| limit.load(Ordering::Relaxed)),
                );
                processor.advance(&mut bridge, &bytes[at..end]);
            }
            held += end - at;
            at = end;
        }
        let Some((_, cut)) = next else { break };
        if matches!(cut, ReaderCut::RetireTitle)
            && processor.sync_timeout().sync_timeout().is_some()
        {
            // A command boundary also closes any buffered part of that
            // command before its title is retired. Otherwise delayed sync
            // playback could set the finished title after the reset.
            processor.stop_sync(&mut ParseBridge::new(
                &mut grid,
                proxy.clone(),
                keyboard,
                history_limit.map(|limit| limit.load(Ordering::Relaxed)),
            ));
        }
        on_cut(&mut grid, cut);
        next = cuts.next();
    }
    drop(grid);
    Some(waited)
}

#[cfg(test)]
mod chunking_tests;
#[cfg(test)]
mod live_history_config_tests;
#[cfg(test)]
mod osc_tests;
#[cfg(test)]
mod parked_cursor_tests;
#[cfg(all(test, unix))]
mod query_tests;
#[cfg(test)]
mod replay_tests;
#[cfg(test)]
mod replayed_mode_tests;
#[cfg(all(test, unix))]
mod stale_mode_tests;
#[cfg(all(test, windows))]
mod windows_tests;
