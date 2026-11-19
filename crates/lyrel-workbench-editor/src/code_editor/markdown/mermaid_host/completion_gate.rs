//! Completion-hold gate for the native stale-completion case.
//!
//! Compiled under `cfg(any(test, feature = "native-markdown-tests"))`: the
//! retained root tests arm the gate to hold a real rendered result at the
//! completion boundary so a stale derivation cannot replace it. Production
//! never links this.

/// One armed hold: `arrived` fires when the held job reaches the gate,
/// `release` is the test's permission to continue, `returned` confirms exit.
pub struct CompletionGate {
    pub source: String,
    pub arrived: smol::channel::Sender<()>,
    pub release: smol::channel::Receiver<()>,
    pub returned: smol::channel::Sender<()>,
}

pub static COMPLETION_GATE: std::sync::Mutex<Option<CompletionGate>> = std::sync::Mutex::new(None);

// The real native result is held only after rendering; no output is replaced.
pub(super) fn hold_completion(key: &super::Key) {
    let gate = {
        let mut gate = COMPLETION_GATE.lock().unwrap();
        if gate
            .as_ref()
            .is_some_and(|gate| key.source.contains(&gate.source))
        {
            gate.take()
        } else {
            None
        }
    };
    if let Some(gate) = gate {
        let _ = gate.arrived.send_blocking(());
        let _ = gate.release.recv_blocking();
        let _ = gate.returned.send_blocking(());
    }
}
