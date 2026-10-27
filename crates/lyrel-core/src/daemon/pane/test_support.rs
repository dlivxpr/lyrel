//! pane 消费者测试共享的最小状态与流夹具。
use super::*;

pub(super) fn ws(cols: u16, rows: u16) -> WinSize {
    WinSize {
        cols,
        rows,
        cell_w: 8,
        cell_h: 16,
    }
}

pub(super) fn test_state(alive: bool) -> PaneState {
    PaneState {
        id: 0,
        ring: ReplayRing::new(ws(80, 24)),
        graphics: Some(GraphicsState::new(ws(80, 24))),
        subscriptions: Subscriptions::new(false, None),
        facts: PaneFacts::default(),
        ssh_phase: None,
        modes: TerminalModes::default(),
        alive,
        exit_code: None,
        last_redraw_nudge: None,
    }
}

pub(super) fn drain(rx: &mpsc::Receiver<DaemonMsg>) -> Vec<DaemonMsg> {
    let mut got = Vec::new();
    while let Ok(msg) = rx.try_recv() {
        got.push(msg);
    }
    got
}

/// A discard writer for `spawn_reader` tests that don't inspect the PTY
/// write-back path (graphics query replies).
pub(super) fn null_writer() -> Arc<Mutex<Box<dyn Write + Send>>> {
    Arc::new(Mutex::new(Box::new(std::io::sink())))
}
