//! An uncommitted composition belongs to the input that started it, not the
//! next control focused inside the same native window.

use std::collections::HashMap;

use gpui_kit::base::{Root, RootPlugin};
use gpui_kit::component::input::{EditorState, InputState, TextareaState};
use gpui_kit::{
    App, Context, ElementId, EntityInputHandler, FocusHandle, Focusable, Global, IntoElement,
    Render, WeakEntity, Window,
};

#[derive(Clone)]
enum CompositionOwner {
    Input(WeakEntity<InputState>),
    Textarea(WeakEntity<TextareaState>),
    Editor(WeakEntity<EditorState>),
}

#[derive(Default)]
struct CompositionOwners(HashMap<ElementId, CompositionOwner>);
impl Global for CompositionOwners {}

pub(crate) fn install(cx: &mut App) {
    cx.set_global(CompositionOwners::default());
    observe_inputs(cx, CompositionOwner::Input);
    observe_inputs(cx, CompositionOwner::Textarea);
    observe_inputs(cx, CompositionOwner::Editor);
    Root::register_plugin(cx, |window, cx| CompositionFocus {
        focus: window.focused(cx),
    });
}

fn observe_inputs<T: EntityInputHandler + Focusable + 'static>(
    cx: &mut App,
    owner: fn(WeakEntity<T>) -> CompositionOwner,
) {
    cx.observe_new::<T>(move |input, _, cx| {
        // A window's initial inputs can be created before observe_new has a
        // window context. Their stable focus identity is already available.
        let key = ElementId::from(&input.focus_handle(cx));
        let input = cx.entity().downgrade();
        cx.global_mut::<CompositionOwners>()
            .0
            .insert(key.clone(), owner(input));
        cx.on_release(move |_, cx| {
            if cx.has_global::<CompositionOwners>() {
                cx.global_mut::<CompositionOwners>().0.remove(&key);
            }
        })
        .detach();
    })
    .detach();
}

struct CompositionFocus {
    focus: Option<FocusHandle>,
}

impl RootPlugin for CompositionFocus {
    fn prepare(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let focus = window.focused(cx);
        if self.focus != focus {
            // The old platform handler has been taken, but the new one has
            // not been installed. Native cancellation cannot edit the new
            // input, and the retained component still owns its marked text.
            #[cfg(windows)]
            windows::cancel_native_composition(window);
            let owner = self.focus.as_ref().and_then(|focus| {
                cx.global::<CompositionOwners>()
                    .0
                    .get(&ElementId::from(focus))
                    .cloned()
            });
            match owner {
                Some(CompositionOwner::Input(input)) => discard_composition(input, window, cx),
                Some(CompositionOwner::Textarea(input)) => discard_composition(input, window, cx),
                Some(CompositionOwner::Editor(input)) => discard_composition(input, window, cx),
                None => {}
            }
            self.focus = focus;
        }
    }
}

impl Render for CompositionFocus {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        gpui_kit::Empty
    }
}

fn discard_composition<T: EntityInputHandler + 'static>(
    input: WeakEntity<T>,
    window: &mut Window,
    cx: &mut App,
) {
    let Some(input) = input.upgrade() else {
        return;
    };
    input.update(cx, |input, cx| {
        if let Some(range) = input.marked_text_range(window, cx) {
            input.replace_text_in_range(Some(range), "", window, cx);
            input.unmark_text(window, cx);
        }
    });
}

#[cfg(windows)]
mod windows {
    pub(super) fn cancel_native_composition(window: &gpui_kit::Window) {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        use windows_sys::Win32::Foundation::HWND;
        use windows_sys::Win32::UI::Input::Ime::{
            CPS_CANCEL, GCS_COMPSTR, ImmGetCompositionStringW, ImmGetContext, ImmNotifyIME,
            ImmReleaseContext, NI_COMPOSITIONSTR,
        };
        use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetFocus;

        let Ok(handle) = HasWindowHandle::window_handle(window) else {
            return;
        };
        let RawWindowHandle::Win32(handle) = handle.as_raw() else {
            return;
        };
        let hwnd = handle.hwnd.get() as HWND;
        // SAFETY: this callback runs on the live window's UI thread. The input
        // context is used only here and released on every acquired path. It is
        // shared by this thread's windows: never cancel another window's IME.
        unsafe {
            if GetFocus() != hwnd {
                return;
            }
            let context = ImmGetContext(hwnd);
            if context.is_null() {
                return;
            }
            if ImmGetCompositionStringW(context, GCS_COMPSTR, std::ptr::null_mut(), 0) > 0
                && ImmNotifyIME(context, NI_COMPOSITIONSTR, CPS_CANCEL, 0) == 0
            {
                log::warn!("ImmNotifyIME(CPS_CANCEL) failed while input focus changed");
            }
            ImmReleaseContext(hwnd, context);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::component::input::{Input, InputState};
    use gpui_kit::{AppContext as _, Entity, ParentElement as _, TestAppContext, div};

    struct Inputs {
        first: Entity<InputState>,
        second: Entity<InputState>,
    }

    impl Render for Inputs {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .child(Input::new(&self.first))
                .child(Input::new(&self.second))
        }
    }

    #[gpui_kit::test]
    fn focus_change_discards_only_the_previous_inputs_preedit(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            install(cx);
        });
        let window = cx.add_window(|window, cx| {
            let view = cx.new(|cx| Inputs {
                first: cx.new(|cx| InputState::new(window, cx)),
                second: cx.new(|cx| InputState::new(window, cx)),
            });
            gpui_kit::component::Root::new(view, window, cx)
        });
        let (first, second) = window
            .update(cx, |root, window, cx| {
                window.activate_window();
                let view = root.view().clone().downcast::<Inputs>().unwrap();
                let inputs = view.read(cx);
                (inputs.first.clone(), inputs.second.clone())
            })
            .unwrap();
        window
            .update(cx, |_, window, cx| {
                second.update(cx, |input, cx| input.set_value("新值", window, cx));
                first.update(cx, |input, cx| {
                    input.focus_handle(cx).focus(window, cx);
                    input.set_value("保留 ", window, cx);
                });
            })
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |_, window, cx| {
                first.update(cx, |input, cx| {
                    input.replace_and_mark_text_in_range(None, "ce shi", Some(6..6), window, cx);
                    assert_eq!(input.value(), "保留 ce shi");
                });
            })
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |_, window, cx| {
                second.read(cx).focus_handle(cx).focus(window, cx);
            })
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |_, window, cx| {
                first.update(cx, |input, cx| {
                    assert_eq!(input.value(), "保留 ");
                    assert_eq!(input.marked_text_range(window, cx), None);
                });
                assert_eq!(second.read(cx).value(), "新值");
                assert!(second.read(cx).focus_handle(cx).is_focused(window));
            })
            .unwrap();
    }
}
