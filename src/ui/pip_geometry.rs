use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::component::{ActiveTheme as _, tooltip::Tooltip};
use gpui_kit::{
    App, Bounds, EntityId, KeyDownEvent, MouseButton, MouseMoveEvent, MouseUpEvent, Pixels, Point,
    TestSupportExt as _, WeakEntity, Window, canvas, div, point, prelude::*, px, size,
};
use lyrel_core::core::machine::{PipGeometry, TabId};

use crate::ui::app::LyrelApp;
use crate::ui::i18n::{L10nKey, t};
use crate::ui::pane::{Pane, PaneChrome, PaneSlot};

/// Only the committed intent belongs to the daemon. Capture, measurements and
/// in-progress movement stay with this tab's client and never enter a snapshot.
#[derive(Default)]
pub(crate) struct PipInteraction {
    viewport: Cell<Option<Bounds<Pixels>>>,
    gesture: Cell<Option<Gesture>>,
}

impl PipInteraction {
    pub(crate) fn cancel(&self) {
        self.gesture.set(None);
    }

    pub(crate) fn dragged_pane(&self) -> Option<EntityId> {
        self.gesture.get().filter(|gesture| !gesture.resize && gesture.dragged).map(|gesture| gesture.pane)
    }

    pub(crate) fn has_gesture(&self) -> bool { self.gesture.get().is_some() }
}

#[derive(Clone, Copy)]
struct Gesture {
    pane: EntityId,
    start: Point<Pixels>,
    initial: Bounds<Pixels>,
    geometry: PipGeometry,
    resize: bool,
    dragged: bool,
}

fn resolve(intent: PipGeometry, viewport: Bounds<Pixels>, rem: f32) -> Bounds<Pixels> {
    // Reserve the entire shadow and the handle inside the content viewport.
    let inset = (rem * 2.)
        .min(viewport.size.width.as_f32() / 4.)
        .min(viewport.size.height.as_f32() / 4.);
    let width = (viewport.size.width.as_f32() - inset * 2.).max(1.);
    let height = (viewport.size.height.as_f32() - inset * 2.).max(1.);
    let w = (width * intent.width()).clamp((20. * rem).min(width), width);
    let h = (height * intent.height()).clamp((12. * rem).min(height), height);
    Bounds::new(
        viewport.origin
            + point(
                px(inset + (width - w) * intent.x()),
                px(inset + (height - h) * intent.y()),
            ),
        size(px(w), px(h)),
    )
}

fn changed(gesture: Gesture, at: Point<Pixels>, viewport: Bounds<Pixels>, rem: f32) -> PipGeometry {
    let delta = at - gesture.start;
    let mut bounds = gesture.initial;
    if gesture.resize {
        bounds.size.width += delta.x;
        bounds.size.height += delta.y;
    } else {
        bounds.origin += delta;
    }
    let inset = (rem * 2.)
        .min(viewport.size.width.as_f32() / 4.)
        .min(viewport.size.height.as_f32() / 4.);
    let width = (viewport.size.width.as_f32() - inset * 2.).max(1.);
    let height = (viewport.size.height.as_f32() - inset * 2.).max(1.);
    let w = bounds
        .size
        .width
        .as_f32()
        .clamp((20. * rem).min(width), width);
    let h = bounds
        .size
        .height
        .as_f32()
        .clamp((12. * rem).min(height), height);
    let x = ((bounds.origin.x - viewport.origin.x).as_f32() - inset) / (width - w).max(1.);
    let y = ((bounds.origin.y - viewport.origin.y).as_f32() - inset) / (height - h).max(1.);
    PipGeometry::new(x.clamp(0., 1.), y.clamp(0., 1.), w / width, h / height).unwrap_or_default()
}

pub(crate) fn commit(
    app: &WeakEntity<LyrelApp>,
    tab_id: TabId,
    pane: EntityId,
    geometry: PipGeometry,
    cx: &mut App,
) {
    let _ = app.update(cx, |app, cx| {
        let Some(tab) = app.tabs.iter_mut().find(|tab| tab.tree_id.get() == tab_id) else {
            return;
        };
        if !tab
            .pip_slot(cx)
            .is_some_and(|slot| slot.entity_id() == pane)
        {
            return;
        }
        if let Some(presentation) = &mut tab.foreign_presentation {
            presentation.set_pip_geometry(Some(geometry));
        }
        if tab.presentation.set_pip_geometry(Some(geometry)) {
            app.save_session(cx);
            cx.notify();
        }
    });
}

pub(crate) fn render(
    slot: PaneSlot,
    tab_id: TabId,
    intent: Option<PipGeometry>,
    state: Rc<PipInteraction>,
    chrome: &PaneChrome,
    _window: &mut Window,
    _cx: &mut App,
) -> gpui_kit::AnyElement {
    PipViewport {
        slot,
        tab_id,
        intent,
        state,
        chrome: PaneChrome {
            dim_inactive: false,
            app: chrome.app.clone(),
            lifted: None,
            drag: chrome.drag.clone(),
        },
    }
    .into_any_element()
}

/// Resolve against this frame's actual viewport before laying out the terminal.
/// A cached previous-frame size would briefly resize the PTY against stale
/// sidebar/window bounds and clip the shadow on the first frame after resize.
struct PipViewport {
    slot: PaneSlot,
    tab_id: TabId,
    intent: Option<PipGeometry>,
    state: Rc<PipInteraction>,
    chrome: PaneChrome,
}

impl IntoElement for PipViewport {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl gpui_kit::Element for PipViewport {
    type RequestLayoutState = gpui_kit::AnyElement;
    type PrepaintState = gpui_kit::AnyElement;

    fn id(&self) -> Option<gpui_kit::ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&gpui_kit::GlobalElementId>,
        _inspector_id: Option<&gpui_kit::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (gpui_kit::LayoutId, Self::RequestLayoutState) {
        let mut container = div().absolute().top_0().left_0().size_full().into_any_element();
        let layout = container.request_layout(window, cx);
        (layout, container)
    }

    fn prepaint(
        &mut self,
        _id: Option<&gpui_kit::GlobalElementId>,
        _inspector_id: Option<&gpui_kit::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        self.state.viewport.set(Some(bounds));
        let mut content = render_in_viewport(
            self.slot.clone(),
            self.tab_id,
            self.intent,
            self.state.clone(),
            &self.chrome,
            bounds,
            window,
            cx,
        );
        content.prepaint_as_root(
            bounds.origin,
            size(
                gpui_kit::AvailableSpace::Definite(bounds.size.width),
                gpui_kit::AvailableSpace::Definite(bounds.size.height),
            ),
            window,
            cx,
        );
        content
    }

    fn paint(
        &mut self,
        _id: Option<&gpui_kit::GlobalElementId>,
        _inspector_id: Option<&gpui_kit::InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _layout: &mut Self::RequestLayoutState,
        content: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        content.paint(window, cx);
    }
}

fn render_in_viewport(
    slot: PaneSlot,
    tab_id: TabId,
    intent: Option<PipGeometry>,
    state: Rc<PipInteraction>,
    chrome: &PaneChrome,
    viewport: Bounds<Pixels>,
    window: &mut Window,
    cx: &mut App,
) -> gpui_kit::AnyElement {
    let id = slot.entity_id();
    if !window.is_window_active()
        || state
            .gesture
            .get()
            .is_some_and(|gesture| gesture.pane != id)
    {
        state.cancel();
    }
    let app = chrome.app.clone();
    let rem = window.rem_size().as_f32();
    let intent = state
        .gesture
        .get()
        .map(|g| g.geometry)
        .or(intent)
        .filter(|g| g.is_valid())
        .unwrap_or_default();
    let resolved = resolve(intent, viewport, rem);
    let local = resolved.origin - viewport.origin;
    let measurement = canvas(|_, _, _| (), {
        let state = state.clone();
        let app = app.clone();
        move |_, _, window, _| {
            window.on_mouse_event({
                let state = state.clone();
                move |event: &MouseMoveEvent, phase, window, cx| {
                    if !phase.capture() {
                        return;
                    }
                    let Some(mut gesture) = state.gesture.get() else {
                        return;
                    };
                    if event.pressed_button != Some(MouseButton::Left) {
                        state.gesture.set(None);
                        window.refresh();
                        return;
                    }
                    let Some(viewport) = state.viewport.get() else {
                        return;
                    };
                    let delta = event.position - gesture.start;
                    gesture.dragged |= delta.x.abs().max(delta.y.abs()) >= window.rem_size() / 4.;
                    gesture.geometry = changed(
                        gesture,
                        event.position,
                        viewport,
                        window.rem_size().as_f32(),
                    );
                    state.gesture.set(Some(gesture));
                    window.refresh();
                    cx.stop_propagation();
                }
            });
            window.on_mouse_event({
                let state = state.clone();
                let app = app.clone();
                move |event: &MouseUpEvent, phase, window, cx| {
                    if !phase.capture() || event.button != MouseButton::Left {
                        return;
                    }
                    if let Some(gesture) = state.gesture.take() {
                        let geometry = state
                            .viewport
                            .get()
                            .map(|viewport| {
                                changed(
                                    gesture,
                                    event.position,
                                    viewport,
                                    window.rem_size().as_f32(),
                                )
                            })
                            .unwrap_or(gesture.geometry);
                        commit(&app, tab_id, id, geometry, cx);
                        window.refresh();
                        cx.stop_propagation();
                    }
                }
            });
        }
    })
    .absolute()
    .size_full();
    let terminal_chrome = PaneChrome {
        dim_inactive: false,
        app: app.clone(),
        lifted: None,
        drag: chrome.drag.clone(),
    };
    let clearance = (rem * 2.)
        .min(viewport.size.width.as_f32() / 4.)
        .min(viewport.size.height.as_f32() / 4.);
    let shadow = if cx.theme().shadow {
        vec![
            gpui_kit::BoxShadow::new(
                px(0.),
                px(clearance * 0.125),
                cx.theme().transparent.alpha(0.18),
            )
            .blur_radius(px(clearance * 0.5)),
        ]
    } else {
        Vec::new()
    };
    let mut shell = div()
        .id(("pane-pip", id.as_u64() as usize))
        .test_support()
        .aria_label(t(L10nKey::PanePipTitle))
        .absolute()
        .left(local.x)
        .top(local.y)
        .w(resolved.size.width)
        .h(resolved.size.height)
        .rounded_lg()
        .border_1()
        .border_color(cx.theme().border)
        .bg(cx.theme().background)
        .shadow(shadow)
        .overflow_hidden()
        .occlude()
        .flex()
        .flex_col()
        .child(
            div()
                .flex_1()
                .min_h_0()
                .w_full()
                .child(Pane::leaf(slot.clone()).render(&terminal_chrome, window, cx)),
        );
    for resize in [false, true] {
        let focus = window.use_keyed_state(
            (
                if resize {
                    "pip-resize-focus"
                } else {
                    "pip-move-focus"
                },
                id.as_u64() as usize,
            ),
            cx,
            |_, cx| cx.focus_handle().tab_stop(true),
        );
        let focus = focus.read(cx).clone();
        let name = if resize {
            L10nKey::PanePipResize
        } else {
            L10nKey::PanePipMove
        };
        let handle = div()
            .id((
                if resize { "pip-resize" } else { "pip-move" },
                id.as_u64() as usize,
            ))
            .test_support()
            .aria_label(t(name))
            .track_focus(&focus)
            .absolute()
            .bg(cx.theme().muted)
            .text_color(cx.theme().muted_foreground)
            .hover(|s| s.bg(cx.theme().secondary))
            .focus_visible(|s| s.border_1().border_color(cx.theme().primary))
            .tooltip(move |window, cx| Tooltip::new(t(name)).build(window, cx))
            .cursor(if resize {
                gpui_kit::CursorStyle::ResizeUpLeftDownRight
            } else {
                gpui_kit::CursorStyle::OpenHand
            })
            .when(resize, |d| {
                d.right_0().bottom_0().size(gpui_kit::rems(1.5)).child("◢")
            })
            .when(!resize, |d| {
                d.left(gpui_kit::rems(0.5))
                    .top(gpui_kit::rems(0.5))
                    .w(gpui_kit::rems(2.5))
                    .h(gpui_kit::rems(1.5))
                    .rounded_md()
                    .child("⠿")
            })
            .on_mouse_down(MouseButton::Left, {
                let state = state.clone();
                move |event, window, cx| {
                    let Some(viewport) = state.viewport.get() else {
                        return;
                    };
                    state.gesture.set(Some(Gesture {
                        pane: id,
                        start: event.position,
                        initial: resolve(intent, viewport, window.rem_size().as_f32()),
                        geometry: intent,
                        resize,
                        dragged: false,
                    }));
                    window.refresh();
                    cx.stop_propagation();
                }
            })
            .on_key_down({
                let state = state.clone();
                let app = app.clone();
                let slot = slot.clone();
                move |event: &KeyDownEvent, window, cx| {
                    let step = window.rem_size();
                    let delta = match event.keystroke.key.as_str() {
                        "left" => point(-step, px(0.)),
                        "right" => point(step, px(0.)),
                        "up" => point(px(0.), -step),
                        "down" => point(px(0.), step),
                        "escape" | "enter" => {
                            state.cancel();
                            window.focus(&slot.focus_handle(cx), cx);
                            cx.stop_propagation();
                            return;
                        }
                        _ => return,
                    };
                    let Some(viewport) = state.viewport.get() else {
                        return;
                    };
                    let bounds = resolve(intent, viewport, step.as_f32());
                    let geometry = changed(
                        Gesture {
                            pane: id,
                            start: point(px(0.), px(0.)),
                            initial: bounds,
                            geometry: intent,
                            resize,
                            dragged: false,
                        },
                        delta,
                        viewport,
                        step.as_f32(),
                    );
                    commit(&app, tab_id, id, geometry, cx);
                    cx.stop_propagation();
                }
            });
        shell = shell.child(handle);
    }
    div()
        .relative()
        .size_full()
        .child(measurement)
        .child(shell)
        .into_any_element()
}
