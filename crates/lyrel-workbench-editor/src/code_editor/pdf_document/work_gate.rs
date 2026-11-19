//! Test work gates for the PDF pipeline.
//!
//! Compiled under `cfg(any(test, feature = "test-support"))` so the
//! retained root tests can pause background PDF work (unlock, first-page
//! render, full-text sweep) at deterministic points; production never links
//! them.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Work {
    FirstPage,
    Unlock,
    /// 全文扫掠的后台解释阶段；`block` 在进入后台任务后等待，`finish`
    /// 在收集完（或取消返回）时置位。
    Search,
}

pub(super) struct WorkGate {
    receiver: smol::channel::Receiver<()>,
    entered: Arc<AtomicBool>,
    finished: Arc<AtomicBool>,
}

impl WorkGate {
    pub async fn block(&self) {
        self.entered
            .store(true, std::sync::atomic::Ordering::Release);
        self.receiver.recv().await.expect("release owned PDF work");
    }

    pub fn finish(self) {
        self.finished
            .store(true, std::sync::atomic::Ordering::Release);
    }
}

thread_local! {
    static WORK_GATE: std::cell::RefCell<Option<(Work, WorkGate)>> = const { std::cell::RefCell::new(None) };
}

pub fn pause_work(work: Work) -> (smol::channel::Sender<()>, Arc<AtomicBool>, Arc<AtomicBool>) {
    let (sender, receiver) = smol::channel::bounded(1);
    let entered = Arc::new(AtomicBool::new(false));
    let finished = Arc::new(AtomicBool::new(false));
    WORK_GATE.with_borrow_mut(|slot| {
        *slot = Some((
            work,
            WorkGate {
                receiver,
                entered: entered.clone(),
                finished: finished.clone(),
            },
        ))
    });
    (sender, entered, finished)
}

pub(super) fn take_work(work: Work) -> Option<WorkGate> {
    WORK_GATE.with_borrow_mut(|slot| {
        if slot.as_ref().is_some_and(|(pending, _)| *pending == work) {
            slot.take().map(|(_, gate)| gate)
        } else {
            None
        }
    })
}
