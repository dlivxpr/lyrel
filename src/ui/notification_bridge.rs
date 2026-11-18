//! The process-wide channel OS notification clicks land on. Platform
//! activation callbacks run on whatever thread the OS hands them — never the
//! UI thread — so all they can do is enqueue a typed event here; the workbench
//! installs the receiver at startup and does the window/pane reveal in its own
//! dispatch loop.
//!
//! This module is the only thing notification delivery may touch upward: no
//! tray type, no window registry, no pane lookup. It is installed exactly once
//! and lives for the whole process, so a click still routes when every window
//! is closed and only the tray remains.

use std::sync::Mutex;

/// What a click on a pane-targeted notification asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaneActivation {
    /// Bring `leaf_id` — a pane's gpui entity id, not a daemon `pane_id` — to
    /// the front: surface its window, activate its tab, focus it.
    Reveal { leaf_id: u64 },
}

/// The sender half of the activation channel, installed by the workbench.
///
/// Kept process-wide for the same reason the tray's channel is: windows come
/// and go (the tray flow retires the last one without exiting), and a click
/// on a notification that outlived its window must still have somewhere to go.
static ACTIVATION: Mutex<Option<smol::channel::Sender<PaneActivation>>> = Mutex::new(None);

/// Install the sender the workbench's receiver loop drains. Called once, at
/// startup, before any notification can carry a pane.
pub fn install(sender: smol::channel::Sender<PaneActivation>) {
    if let Ok(mut slot) = ACTIVATION.lock() {
        *slot = Some(sender);
    }
}

/// A sender for the activation channel, if the workbench has installed one.
/// Cloning only — safe from any thread, including platform callback threads
/// that may not touch GPUI.
pub fn sender() -> Option<smol::channel::Sender<PaneActivation>> {
    ACTIVATION.lock().ok()?.clone()
}
