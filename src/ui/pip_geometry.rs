use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::component::{ActiveTheme as _, InteractiveElementExt as _};
use gpui_kit::{App, Bounds, EntityId, KeyDownEvent, MouseButton, MouseMoveEvent,
    MouseUpEvent, Pixels, Point, TestSupportExt as _, WeakEntity, Window,
    canvas, div, point, prelude::*, px, size};
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
    pub(crate) fn cancel(&self) { self.gesture.set(None); }
}

#[derive(Clone, Copy)]
struct Gesture {
    pane: EntityId,
    start: Point<Pixels>,
    initial: Bounds<Pixels>,
    geometry: PipGeometry,
    resize: bool,
}

fn resolve(intent: PipGeometry, viewport: Bounds<Pixels>, rem: f32) -> Bounds<Pixels> {
    // Reserve the entire shadow and the handle inside the content viewport.
    let inset = (rem * 2.).min(viewport.size.width.as_f32() / 4.).min(viewport.size.height.as_f32() / 4.);
    let width = (viewport.size.width.as_f32() - inset * 2.).max(1.);
    let height = (viewport.size.height.as_f32() - inset * 2.).max(1.);
    let w = (width * intent.width()).clamp((20. * rem).min(width), width);
    let h = (height * intent.height()).clamp((12. * rem).min(height), height);
    Bounds::new(viewport.origin + point(px(inset + (width - w) * intent.x()), px(inset + (height - h) * intent.y())), size(px(w), px(h)))
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
    let inset = (rem * 2.).min(viewport.size.width.as_f32() / 4.).min(viewport.size.height.as_f32() / 4.);
    let width = (viewport.size.width.as_f32() - inset * 2.).max(1.);
    let height = (viewport.size.height.as_f32() - inset * 2.).max(1.);
    let w = bounds.size.width.as_f32().clamp((20. * rem).min(width), width);
    let h = bounds.size.height.as_f32().clamp((12. * rem).min(height), height);
    let x = ((bounds.origin.x - viewport.origin.x).as_f32() - inset) / (width - w).max(1.);
    let y = ((bounds.origin.y - viewport.origin.y).as_f32() - inset) / (height - h).max(1.);
    PipGeometry::new(x.clamp(0., 1.), y.clamp(0., 1.), w / width, h / height).unwrap_or_default()
}

pub(crate) fn commit(app: &WeakEntity<LyrelApp>, tab_id: TabId, pane: EntityId, geometry: PipGeometry, cx: &mut App) {
    let _ = app.update(cx, |app, cx| {
        let Some(tab) = app.tabs.iter_mut().find(|tab| tab.tree_id.get() == tab_id) else { return; };
        if !tab.pip_slot(cx).is_some_and(|slot| slot.entity_id() == pane) { return; }
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
    window: &mut Window,
    cx: &mut App,
) -> gpui_kit::AnyElement {
    let id = slot.entity_id();
    if !window.is_window_active() || state.gesture.get().is_some_and(|gesture| gesture.pane != id) {
        state.cancel();
    }
    let app = chrome.app.clone();
    let rem = window.rem_size().as_f32();
    let intent = state.gesture.get().map(|g| g.geometry).or(intent).filter(|g| g.is_valid()).unwrap_or_default();
    let viewport = state.viewport.get().unwrap_or_else(|| Bounds::new(point(px(0.), px(0.)), window.viewport_size()));
    let resolved = resolve(intent, viewport, rem);
    let local = resolved.origin - viewport.origin;
    let measurement = canvas({
        let state = state.clone();
        move |bounds, window, _| {
            if state.viewport.replace(Some(bounds)) != Some(bounds) { window.refresh(); }
        }
    }, {
        let state = state.clone();
        let app = app.clone();
        move |_, _, window, _| {
            window.on_mouse_event({
                let state = state.clone();
                move |event: &MouseMoveEvent, phase, window, cx| {
                    if !phase.capture() { return; }
                    let Some(mut gesture) = state.gesture.get() else { return; };
                    if event.pressed_button != Some(MouseButton::Left) {
                        state.gesture.set(None);
                        window.refresh();
                        return;
                    }
                    let Some(viewport) = state.viewport.get() else { return; };
                    gesture.geometry = changed(gesture, event.position, viewport, window.rem_size().as_f32());
                    state.gesture.set(Some(gesture));
                    window.refresh();
                    cx.stop_propagation();
                }
            });
            window.on_mouse_event({
                let state = state.clone();
                let app = app.clone();
                move |event: &MouseUpEvent, phase, window, cx| {
                    if !phase.capture() || event.button != MouseButton::Left { return; }
                    if let Some(gesture) = state.gesture.take() {
                        let geometry = state.viewport.get().map(|viewport| changed(gesture, event.position, viewport, window.rem_size().as_f32())).unwrap_or(gesture.geometry);
                        commit(&app, tab_id, id, geometry, cx);
                        window.refresh();
                        cx.stop_propagation();
                    }
                }
            });
        }
    }).absolute().size_full();
    let terminal_chrome = PaneChrome { dim_inactive: false, rearrangeable: false,
        app: app.clone(), lifted: None, drag: chrome.drag.clone() };
    let clearance = (rem * 2.).min(viewport.size.width.as_f32() / 4.).min(viewport.size.height.as_f32() / 4.);
    let shadow = if cx.theme().shadow {
        vec![gpui_kit::BoxShadow::new(px(0.), px(clearance * 0.125), cx.theme().transparent.alpha(0.18))
            .blur_radius(px(clearance * 0.5))]
    } else { Vec::new() };
    let mut shell = div().id(("pane-pip", id.as_u64() as usize)).test_support()
        .aria_label(t(L10nKey::PanePipTitle)).absolute()
        .left(local.x).top(local.y).w(resolved.size.width).h(resolved.size.height)
        .rounded_lg().border_1().border_color(cx.theme().border).bg(cx.theme().background)
        .shadow(shadow).overflow_hidden().occlude().flex().flex_col()
        .child(div().flex_1().min_h_0().w_full().child(Pane::leaf(slot.clone()).render(&terminal_chrome, window, cx)));
    for resize in [false, true] {
        let focus = window.use_keyed_state((if resize { "pip-resize-focus" } else { "pip-move-focus" }, id.as_u64() as usize), cx, |_, cx| cx.focus_handle().tab_stop(true));
        let focus = focus.read(cx).clone();
        let name = if resize { L10nKey::PanePipResize } else { L10nKey::PanePipMove };
        let handle = div().id((if resize { "pip-resize" } else { "pip-move" }, id.as_u64() as usize))
            .test_support().aria_label(t(name)).track_focus(&focus).absolute()
            .bg(cx.theme().muted).text_color(cx.theme().muted_foreground)
            .hover(|s| s.bg(cx.theme().secondary)).focus_visible(|s| s.border_1().border_color(cx.theme().primary))
            .tooltip(t(name))
            .cursor(if resize { gpui_kit::CursorStyle::ResizeUpLeftDownRight } else { gpui_kit::CursorStyle::OpenHand })
            .when(resize, |d| d.right_0().bottom_0().size(gpui_kit::rems(1.5)).child("◢"))
            .when(!resize, |d| d.left(gpui_kit::rems(0.5)).top(gpui_kit::rems(0.5)).w(gpui_kit::rems(2.5)).h(gpui_kit::rems(1.5)).rounded_md().child("⠿"))
            .on_mouse_down(MouseButton::Left, {
                let state = state.clone();
                move |event, window, cx| {
                    let Some(viewport) = state.viewport.get() else { return; };
                    state.gesture.set(Some(Gesture { pane: id, start: event.position, initial: resolve(intent, viewport, window.rem_size().as_f32()), geometry: intent, resize }));
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
                        "left" => point(-step, px(0.)), "right" => point(step, px(0.)),
                        "up" => point(px(0.), -step), "down" => point(px(0.), step),
                        "escape" | "enter" => { window.focus(&slot.focus_handle(cx), cx); cx.stop_propagation(); return; }
                        _ => return,
                    };
                    let Some(viewport) = state.viewport.get() else { return; };
                    let bounds = resolve(intent, viewport, step.as_f32());
                    let geometry = changed(Gesture { pane: id, start: point(px(0.), px(0.)), initial: bounds, geometry: intent, resize }, delta, viewport, step.as_f32());
                    commit(&app, tab_id, id, geometry, cx);
                    cx.stop_propagation();
                }
            });
        shell = shell.child(handle);
    }
    div().absolute().size_full().child(measurement).child(shell).into_any_element()
}
