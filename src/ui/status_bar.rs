//! 全局状态栏；工作区上下文由窗口投影，时钟独立更新，不触发终端重绘。

use chrono::Timelike as _;
use gpui_kit::TestSupportExt as _;
use gpui_kit::component::button::ButtonVariants as _;
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, button::Button, h_flex};
use gpui_kit::{AnyElement, Context, SharedString, Window, div, prelude::*, rems, svg};

use crate::core::actions::{OpenSettings, ShowRightPanelInfo, ToggleSwitcher};
use crate::ui::app::LyrelApp;
use crate::ui::i18n::{L10nKey, t};

pub(crate) struct Clock {
    text: SharedString,
}

impl Clock {
    pub(crate) fn new(cx: &mut Context<Self>) -> Self {
        cx.spawn(async move |this, cx| {
            loop {
                let now = chrono::Local::now();
                let delay = 60_000
                    - u64::from(now.second()) * 1_000
                    - u64::from(now.nanosecond()) / 1_000_000;
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(delay))
                    .await;
                if this
                    .update(cx, |this, cx| {
                        let next: SharedString =
                            chrono::Local::now().format("%H:%M").to_string().into();
                        if this.text != next {
                            this.text = next;
                            cx.notify();
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        Self {
            text: chrono::Local::now().format("%H:%M").to_string().into(),
        }
    }
}

impl Render for Clock {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex_none()
            .px_2()
            .text_xs()
            .text_color(cx.theme().muted_foreground)
            .child(self.text.clone())
    }
}

impl LyrelApp {
    pub(crate) fn render_status_bar(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let host = self.remote_machine_label(cx);
        let document = self.code_panel_visible();
        let path = if document {
            self.editor_active_location().map(|(host, path)| {
                if host.is_local() {
                    crate::ui::path_display::native_separators(path)
                        .display()
                        .to_string()
                } else {
                    path.display().to_string()
                }
            })
        } else {
            self.tab_cwd_text(self.active, window, cx)
        };
        let leaf = self
            .tabs
            .get(self.active)
            .and_then(|tab| tab.pane.focused_or_first(window, cx));
        let branch = (!document)
            .then(|| {
                leaf.as_ref()
                    .and_then(|leaf| leaf.read(cx).git_status(cx))
                    .map(|git| git.branch)
            })
            .flatten();
        let agent = (!document)
            .then(|| self.tabs.get(self.active).and_then(|tab| tab.agent_row(cx)))
            .flatten();
        let context = h_flex()
            .min_w_0()
            .gap_1()
            .child(
                Button::new("status-host")
                    .ghost()
                    .xsmall()
                    .max_w(rems(10.))
                    .label(host.clone())
                    .tooltip(host)
                    .on_click(|_, window, cx| window.dispatch_action(Box::new(ToggleSwitcher), cx)),
            )
            .when_some(path, |row, path| {
                let copied = path.clone();
                row.child(
                    Button::new("status-path")
                        .ghost()
                        .xsmall()
                        .min_w_0()
                        .max_w(rems(26.))
                        .label(path.clone())
                        .tooltip(path)
                        .accessibility_label(t(if document {
                            L10nKey::FileTreeContextCopyPath
                        } else {
                            L10nKey::CmdCopyWorkingDirectory
                        }))
                        .on_click(move |_, _, cx| {
                            cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(
                                copied.clone(),
                            ));
                        }),
                )
            })
            .when_some(branch, |row, branch| {
                row.child(
                    h_flex()
                        .min_w_0()
                        .max_w(rems(12.))
                        .gap_1()
                        .child(svg().path("icons/git-branch.svg").size_3().flex_none())
                        .child(div().min_w_0().text_ellipsis().child(branch)),
                )
            });
        h_flex()
            .id("workbench-status")
            .test_support()
            .flex_none()
            .w_full()
            .h(rems(1.625))
            .px_1()
            .gap_1()
            .border_t_1()
            .border_color(cx.theme().border)
            .bg(crate::ui::theme::workspace_surface_color(cx))
            .text_xs()
            .text_color(cx.theme().muted_foreground)
            .child(
                Button::new("status-settings")
                    .ghost()
                    .xsmall()
                    .icon(Icon::empty().path("icons/settings/general.svg"))
                    .accessibility_label(t(L10nKey::CmdSettings))
                    .tooltip(t(L10nKey::CmdSettings))
                    .on_click(|_, window, cx| window.dispatch_action(Box::new(OpenSettings), cx)),
            )
            .child(context)
            .child(div().flex_1().min_w_0())
            .when_some(agent, |bar, (agent, status)| {
                let label = match crate::ui::tab_strip::agent_status_label(Some(status)) {
                    Some(state) => format!("{} · {state}", agent.display_name()),
                    None => agent.display_name().to_string(),
                };
                bar.child(
                    Button::new("status-agent")
                        .ghost()
                        .xsmall()
                        .max_w(rems(12.))
                        .label(label.clone())
                        .tooltip(label)
                        .on_click(|_, window, cx| {
                            window.dispatch_action(Box::new(ShowRightPanelInfo), cx)
                        }),
                )
            })
            .child(self.status_clock.clone())
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use crate::core::actions::{EditorNewFile, ToggleCodePanel, ToggleDocumentFill};
    use crate::core::config::Config;
    use crate::ui::app::test_window::harness_with_pane;
    use gpui_kit::test::TestWindowExt as _;
    use gpui_kit::{TestAppContext, VisualTestContext, px, size};
    use rio_vt::crosswords::grid::Dimensions as _;

    fn settle(cx: &mut VisualTestContext) {
        cx.run_until_parked();
        cx.update(|window, cx| window.render_frame(cx));
        cx.run_until_parked();
    }

    #[gpui_kit::test]
    fn settings_remains_reachable_below_terminal_and_filled_document(cx: &mut TestAppContext) {
        let (app, mut vcx, _stream) = harness_with_pane(cx);
        for (width, font) in [(1440., 16.), (720., 20.)] {
            vcx.simulate_resize(size(px(width), px(720.)));
            app.update_in(&mut vcx, |app, _, cx| {
                cx.global_mut::<Config>().ui_font_size = font;
                gpui_kit::component::Theme::update(cx, |theme| theme.font_size = px(font));
                app.sidebar_collapsed = true;
                app.right_panel_visible = false;
                cx.notify();
            });
            for filled in [false, true] {
                if filled {
                    vcx.dispatch_action(EditorNewFile);
                    vcx.dispatch_action(ToggleDocumentFill);
                }
                settle(&mut vcx);
                if !filled {
                    app.read_with(&vcx, |app, cx| {
                        let pane = app.tabs[app.active].pane.terminals()[0].read(cx);
                        assert!(pane.terminal.term.lock().screen_lines() >= 24,
                            "the terminal must use the content height, not collapse to its intrinsic size");
                    });
                }
                vcx.update(|window, cx| {
                    let status = window.find("workbench-status").bounds();
                    let content = window.find("workbench-content").bounds();
                    assert!(content.bottom() <= status.top());
                    assert!(status.bottom() <= window.viewport_size().height);
                    assert!(window.find("status-settings").visible());
                    window.click("status-settings", cx);
                });
                settle(&mut vcx);
                assert!(app.read_with(&vcx, |app, _| app.active_settings().is_some()));
                vcx.simulate_keystrokes("escape");
                settle(&mut vcx);
                assert!(app.read_with(&vcx, |app, _| app.active_settings().is_none()));
                if filled {
                    vcx.dispatch_action(ToggleCodePanel);
                    settle(&mut vcx);
                }
            }
        }
    }
}
