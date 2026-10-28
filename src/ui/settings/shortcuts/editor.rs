//! Shortcut editing owns capture, conflict decisions and delayed-result freshness.
//! The window only installs GPUI subscriptions and applies accepted configuration edits.

use super::super::*;
use crate::core::config::KeybindingOverride;
use crate::ui::keymap::{effective_chords, extra_bindings, same_chord};

pub(in crate::ui) struct ShortcutEditor {
    search: Entity<InputState>,
    _search_subscription: Subscription,
    recording: Option<Recording>,
    conflict: Option<KeyConflict>,
    note: Option<String>,
    generation: u64,
}

struct Recording {
    action: String,
    chords: Vec<String>,
    _intercept: Subscription,
}

struct KeyConflict {
    action: String,
    spec: String,
    other: String,
}

/// Reuse the retained input's identity: a rebuilt settings session cannot accept
/// an old timer even if its local generation happens to have the same value.
#[derive(Clone, Copy)]
pub(super) struct RecordCommit {
    session: gpui_kit::EntityId,
    generation: u64,
}

pub(super) enum Capture {
    Ignored,
    Changed,
    Schedule(RecordCommit),
    Edit(ShortcutChange),
}

pub(super) enum Commit {
    Ignored,
    Conflict,
    Edit(ShortcutChange),
}

/// An accepted edit; persistence and keymap installation remain on the existing
/// settings configuration path, including its save-error/dirty handling.
pub(super) enum ShortcutChange {
    Assign {
        action: String,
        spec: String,
        displaced: Option<(String, Vec<String>)>,
    },
    Unbind(String),
    Reset(String),
    RestoreDefaults,
    Preset(String),
    Prefix(String),
}

impl ShortcutChange {
    pub(super) fn apply(self, cfg: &mut Config) {
        match self {
            Self::Assign { action, spec, displaced } => {
                if let Some((other, rest)) = displaced {
                    cfg.keybindings.insert(other, KeybindingOverride::Exact(rest));
                }
                // Recording replaces, whereas a bare string adds to defaults.
                cfg.keybindings.insert(action, KeybindingOverride::Exact(vec![spec]));
            }
            Self::Unbind(action) => {
                cfg.keybindings.insert(action, KeybindingOverride::Exact(Vec::new()));
            }
            Self::Reset(action) => { cfg.keybindings.remove(&action); }
            Self::RestoreDefaults => cfg.keybindings.clear(),
            Self::Preset(preset) => cfg.keybinding_preset = preset,
            Self::Prefix(prefix) => cfg.prefix = prefix,
        }
    }
}

impl ShortcutEditor {
    pub(in crate::ui::settings) fn new(window: &mut Window, cx: &mut Context<SettingsWindow>) -> Self {
        let search = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t(L10nKey::SettingsSearchShortcuts))
        });
        let subscription = cx.subscribe_in(&search, window, |_, _, ev, _, cx| {
            if matches!(ev, InputEvent::Change) {
                cx.notify();
            }
        });
        Self {
            search,
            _search_subscription: subscription,
            recording: None,
            conflict: None,
            note: None,
            generation: 0,
        }
    }

    pub(in crate::ui) fn search(&self) -> &Entity<InputState> { &self.search }

    pub(in crate::ui) fn is_recording(&self) -> bool { self.recording.is_some() }

    pub(super) fn recording(&self) -> Option<(&str, &[String])> {
        self.recording.as_ref().map(|r| (r.action.as_str(), r.chords.as_slice()))
    }

    pub(super) fn conflict(&self) -> Option<(&str, &str, &str)> {
        self.conflict.as_ref().map(|c| (c.action.as_str(), c.spec.as_str(), c.other.as_str()))
    }

    pub(super) fn note(&self) -> Option<&str> { self.note.as_deref() }

    pub(super) fn begin(&mut self, action: String, intercept: Subscription) {
        self.cancel_recording();
        self.note = None;
        self.conflict = None;
        self.recording = Some(Recording { action, chords: Vec::new(), _intercept: intercept });
    }

    pub(in crate::ui) fn cancel_recording(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.recording = None;
    }

    pub(in crate::ui) fn cancel_conflict(&mut self) -> bool { self.conflict.take().is_some() }

    pub(in crate::ui) fn retire(&mut self) {
        self.cancel_recording();
        self.conflict = None;
    }

    fn pending_commit(&mut self) -> RecordCommit {
        self.generation = self.generation.wrapping_add(1);
        RecordCommit { session: self.search.entity_id(), generation: self.generation }
    }

    pub(super) fn capture(&mut self, key: &gpui_kit::Keystroke) -> Capture {
        let Some(recording) = self.recording.as_mut() else { return Capture::Ignored; };
        match key.key.as_str() {
            "escape" => {
                self.cancel_recording();
                Capture::Changed
            }
            "backspace" | "delete" => {
                if recording.chords.pop().is_some() {
                    if recording.chords.is_empty() {
                        self.generation = self.generation.wrapping_add(1);
                        Capture::Changed
                    } else {
                        Capture::Schedule(self.pending_commit())
                    }
                } else {
                    let action = recording.action.clone();
                    Capture::Edit(self.unbind(action))
                }
            }
            _ => {
                let Some(spec) = crate::ui::keymap::spec_from_keystroke(key) else {
                    return Capture::Ignored;
                };
                recording.chords.push(spec);
                Capture::Schedule(self.pending_commit())
            }
        }
    }

    pub(super) fn commit(&mut self, token: RecordCommit, cx: &App) -> Commit {
        if token.session != self.search.entity_id() || token.generation != self.generation {
            return Commit::Ignored;
        }
        let Some(recording) = self.recording.as_ref().filter(|r| !r.chords.is_empty()) else {
            return Commit::Ignored;
        };
        let action = recording.action.clone();
        let spec = recording.chords.join(" ");
        self.cancel_recording();
        if let Some(other) = effective_chords(cx).into_iter()
            .find(|(a, chords)| *a != action && chords.iter().any(|k| same_chord(k, &spec)))
            .map(|(a, _)| a)
            .or_else(|| extra_bindings(cx).into_iter()
                .find(|(a, k)| *a != action && same_chord(k, &spec)).map(|(a, _)| a))
        {
            self.conflict = Some(KeyConflict { action, spec, other });
            Commit::Conflict
        } else {
            Commit::Edit(self.assign(action, spec, cx))
        }
    }

    pub(super) fn resolve_conflict(&mut self, replace: bool, cx: &App) -> Option<ShortcutChange> {
        let conflict = self.conflict.take()?;
        if !replace { return None; }
        let edit = self.assign(conflict.action, conflict.spec, cx);
        // Replace already explained the displacement in the row.
        self.note = None;
        Some(edit)
    }

    fn assign(&mut self, action: String, spec: String, cx: &App) -> ShortcutChange {
        // Compare interpreted chords, not spelling (secondary-} vs secondary-shift-]).
        // Displace only this chord; an extra default follows its owner's first chord.
        let displaced = effective_chords(cx).into_iter()
            .find(|(a, chords)| *a != action && chords.iter().any(|k| same_chord(k, &spec)))
            .map(|(a, chords)| {
                (a, chords.into_iter().filter(|k| !same_chord(k, &spec)).collect())
            })
            .or_else(|| extra_bindings(cx).into_iter()
                .find(|(a, k)| *a != action && same_chord(k, &spec))
                .map(|(a, _)| (a, Vec::new())));
        let in_prose = |name: &str| name.trim_end_matches('…').to_string();
        self.note = displaced.as_ref().map(|(other, _)| {
            t_fmt(L10nKey::AppKeybindingDisplacedNote, &[
                ("action", &in_prose(&crate::ui::keymap::action_entry(&action).1)),
                ("previous", &in_prose(&crate::ui::keymap::action_entry(other).1)),
            ])
        });
        ShortcutChange::Assign { action, spec, displaced }
    }

    fn clear_recording_note(&mut self) {
        self.cancel_recording();
        self.note = None;
    }

    pub(super) fn unbind(&mut self, action: String) -> ShortcutChange {
        self.clear_recording_note();
        ShortcutChange::Unbind(action)
    }

    pub(super) fn reset(&mut self, action: String) -> ShortcutChange {
        self.clear_recording_note();
        ShortcutChange::Reset(action)
    }

    pub(super) fn restore_defaults(&mut self) -> ShortcutChange {
        self.clear_recording_note();
        ShortcutChange::RestoreDefaults
    }

    pub(super) fn set_preset(&mut self, preset: &str) -> ShortcutChange {
        self.clear_recording_note();
        ShortcutChange::Preset(preset.to_string())
    }

    pub(super) fn set_prefix(&mut self, prefix: &str) -> ShortcutChange {
        ShortcutChange::Prefix(prefix.to_string())
    }
}
