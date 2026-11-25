//! Focus-boundary IME cleanup for the app-owned Markdown editor.
//!
//! Framework inputs (`InputState`, `TextareaState`, `EditorState`) rely on
//! `gpui-kit 0.11.0` (#4032), whose `EntityInputHandler::accepts_text_input`
//! returns `false` once blurred so the platform ends composition cleanly.
//! The custom `MarkdownEditor` is not covered by that upstream component state,
//! so this module keeps focus-change unmarking and Windows native cancellation
//! scoped strictly to `MarkdownEditor`.

use std::collections::HashMap;

use lyrel_workbench_editor::code_editor::markdown::EditorState as MarkdownEditor;

use gpui_kit::base::{Root, RootPlugin};
use gpui_kit::{
    App, Context, ElementId, EntityInputHandler, FocusHandle, Focusable, Global, IntoElement,
    Render, WeakEntity, Window,
};

#[derive(Default)]
struct CompositionOwners {
    markdown_editors: HashMap<ElementId, WeakEntity<MarkdownEditor>>,
}
impl Global for CompositionOwners {}

pub(crate) fn install(cx: &mut App) {
    cx.set_global(CompositionOwners::default());
    observe_markdown_editors(cx);
    Root::register_plugin(cx, |window, cx| CompositionFocus {
        focus: window
            .is_window_active()
            .then(|| window.focused(cx))
            .flatten(),
    });
}

fn observe_markdown_editors(cx: &mut App) {
    cx.observe_new::<MarkdownEditor>(move |input, _, cx| {
        // A window's initial editors can be created before observe_new has a
        // window context. Their stable focus identity is already available.
        let key = ElementId::from(&input.focus_handle(cx));
        let input = cx.entity().downgrade();
        cx.global_mut::<CompositionOwners>()
            .markdown_editors
            .insert(key.clone(), input);
        cx.on_release(move |_, cx| {
            if cx.has_global::<CompositionOwners>() {
                let owners = cx.global_mut::<CompositionOwners>();
                owners.markdown_editors.remove(&key);
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
        let focus = window
            .is_window_active()
            .then(|| window.focused(cx))
            .flatten();
        if self.focus != focus {
            let owner = self.focus.as_ref().and_then(|focus| {
                cx.global::<CompositionOwners>()
                    .markdown_editors
                    .get(&ElementId::from(focus))
                    .cloned()
            });
            if let Some(input) = owner.and_then(|input| input.upgrade()) {
                // The old platform handler has been taken, but the new one has
                // not been installed. Native cancellation cannot edit the new
                // input, and the Markdown editor still owns its marked text.
                #[cfg(windows)]
                windows::cancel_native_composition(window);
                // Focus/mode/close handoff keeps visible preedit as one
                // meaningful document edit. Only explicit cancel rolls
                // back its original source; committed prefixes survive.
                input.update(cx, |input, cx| input.unmark_text(window, cx));
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
        markdown: Entity<MarkdownEditor>,
        first: Entity<InputState>,
        second: Entity<InputState>,
    }

    impl Render for Inputs {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .child(self.markdown.clone())
                .child(Input::new(&self.first))
                .child(Input::new(&self.second))
        }
    }

    #[gpui_kit::test]
    fn focus_change_unmarks_only_the_previous_markdown_editors_preedit(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            install(cx);
        });
        let window = cx.add_window(|window, cx| {
            let view = cx.new(|cx| Inputs {
                markdown: cx.new(|cx| MarkdownEditor::new(window, cx)),
                first: cx.new(|cx| InputState::new(window, cx)),
                second: cx.new(|cx| InputState::new(window, cx)),
            });
            gpui_kit::component::Root::new(view, window, cx)
        });
        let (markdown, second) = window
            .update(cx, |root, window, cx| {
                window.activate_window();
                let view = root.view().clone().downcast::<Inputs>().unwrap();
                let inputs = view.read(cx);
                (inputs.markdown.clone(), inputs.second.clone())
            })
            .unwrap();
        window
            .update(cx, |_, window, cx| {
                second.update(cx, |input, cx| input.set_value("新值", window, cx));
                markdown.update(cx, |editor, cx| {
                    editor.focus_handle(cx).focus(window, cx);
                    editor.replace_text_in_range(None, "保留 ", window, cx);
                });
            })
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |_, window, cx| {
                markdown.update(cx, |editor, cx| {
                    editor.replace_and_mark_text_in_range(None, "ce shi", Some(6..6), window, cx);
                    assert_eq!(editor.text(), "保留 ce shi");
                    assert!(editor.marked_text_range(window, cx).is_some());
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
                markdown.update(cx, |editor, cx| {
                    assert_eq!(editor.text(), "保留 ce shi");
                    assert_eq!(editor.marked_text_range(window, cx), None);
                });
                assert_eq!(second.read(cx).value(), "新值");
                assert!(second.read(cx).focus_handle(cx).is_focused(window));
            })
            .unwrap();
    }

    #[gpui_kit::test]
    fn framework_input_accepts_text_input_only_while_focused(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            install(cx);
        });
        let window = cx.add_window(|window, cx| {
            let view = cx.new(|cx| Inputs {
                markdown: cx.new(|cx| MarkdownEditor::new(window, cx)),
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
                first.read(cx).focus_handle(cx).focus(window, cx);
            })
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |_, window, cx| {
                assert!(first.update(cx, |input, cx| input.accepts_text_input(window, cx)));
                assert!(!second.update(cx, |input, cx| input.accepts_text_input(window, cx)));
                second.read(cx).focus_handle(cx).focus(window, cx);
            })
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |_, window, cx| {
                assert!(!first.update(cx, |input, cx| input.accepts_text_input(window, cx)));
                assert!(second.update(cx, |input, cx| input.accepts_text_input(window, cx)));
            })
            .unwrap();
    }
}
