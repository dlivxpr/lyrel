use std::time::{Duration, Instant};

use gpui_kit::{
    App, Context, Entity, EntityId, FocusHandle, Focusable as _, KeyDownEvent, KeyUpEvent,
    PlatformInput, Window,
};
use lyrel_core::core::machine::TabId;
use lyrel_terminal::view::TerminalView;
use lyrel_ui_base::session::WorkspaceId;

use super::{LyrelApp, PaneSlot};

// Retain only the quick burst after an explicit workspace switch. A slow or
// failed connection must not type into a shell later on the user's behalf.
const HANDOFF_WINDOW: Duration = Duration::from_millis(500);
const MAX_EVENTS: usize = 64;

pub(super) struct WorkspaceInput {
    workspace: WorkspaceId,
    requested_tab: Option<TabId>,
    deadline: Instant,
    target: Option<PendingTarget>,
    events: Vec<PlatformInput>,
}

struct PendingTarget {
    slot: EntityId,
    pane: u64,
    focus: FocusHandle,
}

impl WorkspaceInput {
    pub(super) fn begin(workspace: WorkspaceId) -> Self {
        Self {
            workspace,
            requested_tab: None,
            deadline: Instant::now() + HANDOFF_WINDOW,
            target: None,
            events: Vec::new(),
        }
    }

    pub(super) fn requested_tab(&self) -> Option<TabId> {
        self.requested_tab
    }

    pub(super) fn request_tab(&mut self, tab: TabId) {
        self.requested_tab = Some(tab);
    }

    fn expired(&self) -> bool {
        Instant::now() >= self.deadline
    }
}

impl LyrelApp {
    pub(super) fn bind_workspace_input(&mut self, tab: Option<TabId>, window: &Window, cx: &App) {
        let Some(mut handoff) = self.workspace_input.take() else {
            return;
        };
        if handoff.workspace != self.workspace || handoff.expired() || handoff.target.is_some() {
            return;
        }
        let Some(PaneSlot::Connecting(pending)) = self
            .tabs
            .get(self.tabs.active_index())
            .filter(|active| Some(active.id()) == tab)
            .and_then(|tab| tab.focus_target(cx))
        else {
            return;
        };
        let pending_view = pending.read(cx);
        let Some(pane) = pending_view.restore_pane() else {
            return;
        };
        let focus = pending_view.focus_handle(cx);
        if focus.is_focused(window) {
            handoff.target = Some(PendingTarget {
                slot: pending.entity_id(),
                pane,
                focus,
            });
            self.workspace_input = Some(handoff);
        }
    }

    fn waiting_for_workspace_input(&mut self, window: &Window) -> Option<&mut WorkspaceInput> {
        let handoff = self.workspace_input.as_ref()?;
        let focus = handoff
            .target
            .as_ref()
            .map_or(&self.home_focus, |target| &target.focus);
        if handoff.expired() || self.workspace != handoff.workspace || !focus.is_focused(window) {
            self.workspace_input = None;
            return None;
        }
        self.workspace_input.as_mut()
    }

    pub(super) fn capture_workspace_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(handoff) = self.waiting_for_workspace_input(window) else {
            return;
        };
        let key = &event.keystroke;
        let modifiers = key.modifiers;
        if event.prefer_character_input
            || modifiers.control
            || modifiers.alt
            || modifiers.platform
            || modifiers.function
            || (key.key_char.is_none() && !matches!(key.key.as_str(), "enter" | "backspace"))
        {
            self.workspace_input = None;
            return;
        }
        if handoff.events.len() == MAX_EVENTS {
            self.workspace_input = None;
        } else {
            handoff.events.push(PlatformInput::KeyDown(event.clone()));
        }
        cx.stop_propagation();
    }

    pub(super) fn capture_workspace_key_up(
        &mut self,
        event: &KeyUpEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(handoff) = self.waiting_for_workspace_input(window) else {
            return;
        };
        // The key that confirmed the switch was pressed in the switcher.
        // Its release belongs there, not to the terminal.
        let queued_press = handoff.events.iter().rev().find_map(|queued| match queued {
            PlatformInput::KeyDown(down) if down.keystroke.key == event.keystroke.key => Some(true),
            PlatformInput::KeyUp(up) if up.keystroke.key == event.keystroke.key => Some(false),
            _ => None,
        });
        if queued_press != Some(true) {
            return;
        }
        if handoff.events.len() == MAX_EVENTS {
            self.workspace_input = None;
        } else {
            handoff.events.push(PlatformInput::KeyUp(event.clone()));
        }
        cx.stop_propagation();
    }

    pub(super) fn deliver_workspace_input(
        &mut self,
        slot: EntityId,
        view: &Entity<TerminalView>,
        restored: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(target) = self
            .workspace_input
            .as_ref()
            .and_then(|handoff| handoff.target.as_ref())
            .filter(|target| target.slot == slot)
        else {
            return;
        };
        let expected_pane = target.pane;
        let Some(handoff) = self.workspace_input.take() else {
            return;
        };
        if !restored
            || handoff.expired()
            || handoff.events.is_empty()
            || view.read(cx).pane_id != expected_pane
        {
            return;
        }
        let workspace = handoff.workspace;
        let focus = view.read(cx).focus_handle.clone();
        let app = cx.weak_entity();
        #[cfg(target_os = "macos")]
        let view = view.downgrade();
        window.defer(cx, move |window, cx| {
            let Some(app) = app.upgrade() else {
                return;
            };
            for event in handoff.events {
                if !window.is_window_active()
                    || !focus.is_focused(window)
                    || app.read(cx).workspace != workspace
                {
                    break;
                }
                // Re-run the normal keymap and terminal encoding against the
                // restored pane's current state. A physical printable event
                // may fall through to platform text delivery on macOS.
                #[cfg(target_os = "macos")]
                let text = match &event {
                    PlatformInput::KeyDown(down) => down.keystroke.key_char.clone(),
                    _ => None,
                };
                let _result = window.dispatch_event(event, cx);
                #[cfg(target_os = "macos")]
                if let Some(text) = text
                    && _result.propagate
                    && !_result.default_prevented
                    && focus.is_focused(window)
                    && let Some(view) = view.upgrade()
                {
                    view.update(cx, |terminal, cx| terminal.input_text(&text, cx));
                }
            }
        });
    }
}
