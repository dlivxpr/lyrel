use std::collections::HashMap;
use std::sync::{Arc, Mutex, atomic::{AtomicBool, Ordering}};
use std::time::Duration;

use gpui_kit::{App, Context, EntityId, FocusHandle, Subscription, Task, Window, prelude::*};

use super::images::ImageStore;
use super::remote::{GraphicsViewportReporter, PaneEvent, RemoteTerminal};
use crate::core::config::Config;

/// Out-of-band visibility: reading every pane for chrome must not enroll hidden
/// output in the window's frame loop. Unknown panes deliberately stay visible.
/// App-local ids cannot safely be kept in a process-wide static registry.
#[derive(Default)]
struct DisplayedRegistry(Mutex<HashMap<EntityId, GraphicsViewportReporter>>);
impl gpui_kit::Global for DisplayedRegistry {}

pub fn declare_displayed(cx: &App, panes: impl IntoIterator<Item = (EntityId, bool)>) {
    let Some(registry) = cx.try_global::<DisplayedRegistry>() else {
        return;
    };
    let map = registry.0.lock().unwrap();
    for (id, on) in panes {
        if let Some(reporter) = map.get(&id) {
            reporter.set_displayed(on);
        }
    }
}

#[cfg(test)]
pub(crate) fn displayed_for_test(cx: &App, id: EntityId) -> Option<bool> {
    cx.try_global::<DisplayedRegistry>()?
        .0.lock().unwrap().get(&id)
        .map(|reporter| reporter.displayed().load(Ordering::Relaxed))
}

/// GPUI's typed entity context requires adapters, but this owner never knows
/// the retained view type or reaches into its input, agent, or shell state.
#[non_exhaustive]
pub(super) struct LifecycleCallbacks<T: 'static> {
    pub state: fn(&mut T) -> &mut TerminalLifecycle,
    pub event: fn(&mut T, PaneEvent, &mut Context<T>),
    pub focus: fn(&mut T, bool, &mut Context<T>),
    pub foreground: fn(&mut T, &mut Window, &mut Context<T>),
    pub detach: fn(&mut T),
}

/// Owns exactly one retained terminal's event receiver, clocks, subscriptions,
/// visibility registration and teardown. Tasks retain weak entity handles only;
/// keeping their cancellation handles here also stops idle work on release.
pub(super) struct TerminalLifecycle {
    displayed: Arc<AtomicBool>,
    cursor_visible: bool,
    id: EntityId,
    window: gpui_kit::AnyWindowHandle,
    images: ImageStore,
    released: bool,
    _tasks: [Task<()>; 3],
    _subscriptions: [Subscription; 3],
}

impl TerminalLifecycle {
    pub(super) fn new<T: 'static>(
        terminal: &RemoteTerminal,
        focus: &FocusHandle,
        window: &mut Window,
        cx: &mut Context<T>,
        callbacks: LifecycleCallbacks<T>,
    ) -> Self {
        let images = terminal.images();
        let events = terminal.events.clone();
        let on_event = callbacks.event;
        let event_images = images.clone();
        let state = callbacks.state;
        let on_detach = callbacks.detach;
        let pump = cx.spawn(async move |this, cx| {
            let mut batch = Vec::new();
            while let Ok(event) = events.recv().await {
                batch.push(event);
                while let Ok(event) = events.try_recv() {
                    batch.push(event);
                }
                if this.update_in(cx, |view, window, cx| {
                    let mut woke = false;
                    for event in batch.drain(..) {
                        if matches!(event, PaneEvent::Wakeup) && std::mem::replace(&mut woke, true) {
                            continue;
                        }
                        on_event(view, event, cx);
                    }
                    // Admission can be waiting for this exact GPU lease. Even
                    // hidden panes return retired pages after this batch, not
                    // after some future visible frame or periodic cleanup.
                    event_images.release_retired(window, cx);
                }).is_err() {
                    this.update(cx, |view, cx| Self::release_view(view, cx, state, on_detach)).ok();
                    break;
                }
            }
        });

        let on_focus = callbacks.focus;
        let focus_in = cx.on_focus_in(focus, window, move |view, _, cx| {
            state(view).show_cursor();
            on_focus(view, true, cx);
            cx.notify();
        });
        let blur = cx.on_blur(focus, window, move |view, _, cx| {
            on_focus(view, false, cx);
            cx.notify();
        });

        let blink_focus = focus.clone();
        let blink = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(530)).await;
                if this.update_in(cx, |view, window, cx| {
                    // A handle may receive focus before its leaf is attached,
                    // and then miss blur. The window, not a cached flag, wins.
                    let focused = blink_focus.is_focused(window);
                    let enabled = cx.global::<Config>().cursor_blink;
                    if state(view).tick_cursor(focused, enabled) {
                        cx.notify();
                    }
                }).is_err() {
                    this.update(cx, |view, cx| Self::release_view(view, cx, state, on_detach)).ok();
                    break;
                }
            }
        });

        let poll_images = images.clone();
        let on_foreground = callbacks.foreground;
        let foreground = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(300)).await;
                if this.update_in(cx, |view, window, cx| {
                    poll_images.release_retired(window, cx);
                    // Shell and agent facts must still advance while hidden.
                    on_foreground(view, window, cx);
                }).is_err() {
                    this.update(cx, |view, cx| Self::release_view(view, cx, state, on_detach)).ok();
                    break;
                }
            }
        });

        let reporter = terminal.viewport_reporter();
        let displayed = reporter.displayed();
        let id = cx.entity().entity_id();
        cx.default_global::<DisplayedRegistry>().0.lock().unwrap().insert(id, reporter);
        let release = cx.on_release(move |view, cx| {
            Self::release_view(view, cx, state, on_detach);
        });

        Self {
            displayed,
            cursor_visible: true,
            id,
            window: window.window_handle(),
            images,
            released: false,
            _tasks: [pump, blink, foreground],
            _subscriptions: [focus_in, blur, release],
        }
    }

    fn release_view<T: 'static>(
        view: &mut T,
        cx: &mut App,
        state: fn(&mut T) -> &mut Self,
        detach: fn(&mut T),
    ) {
        if state(view).released {
            return;
        }
        // Fence the reader before clearing images. No late decode can restore
        // the released consumer, and repeated task failures are harmless.
        detach(view);
        let lifecycle = state(view);
        lifecycle.released = true;
        if let Some(registry) = cx.try_global::<DisplayedRegistry>() {
            registry.0.lock().unwrap().remove(&lifecycle.id);
        }
        if lifecycle.window.update(cx, |_, window, cx| lifecycle.images.release_all(window, cx)).is_err() {
            lifecycle.images.release_after_window_close(lifecycle.window.window_id(), cx);
        }
    }

    pub(super) fn is_displayed(&self) -> bool {
        self.displayed.load(Ordering::Relaxed)
    }

    pub(super) fn paint_visibility(&self) -> &Arc<AtomicBool> {
        &self.displayed
    }

    pub(super) fn notify_output<T: 'static>(&self, cx: &mut Context<T>) {
        if self.is_displayed() {
            cx.notify();
        }
    }

    pub(super) fn cursor_visible(&self) -> bool {
        self.cursor_visible
    }

    pub(super) fn show_cursor(&mut self) {
        self.cursor_visible = true;
    }

    fn tick_cursor(&mut self, focused: bool, enabled: bool) -> bool {
        if !focused {
            return false;
        }
        let visible = !enabled || !self.cursor_visible;
        std::mem::replace(&mut self.cursor_visible, visible) != visible
    }
}
