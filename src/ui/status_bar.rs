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
    /// Only content focus changes this choice; opening a status control or a
    /// window overlay must not silently switch a docked editor back to a shell.
    pub(crate) fn watch_document_status_focus(
        &self,
        handle: &gpui_kit::FocusHandle,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.on_focus_in(handle, window, |this, _, cx| {
            if let Some(tab) = this.tabs.get(this.active) {
                this.status_document_focus = Some((tab.tree_id.get(), true));
                cx.notify();
            }
        }).detach();
    }

    pub(crate) fn status_document_context(
        &self,
        window: &Window,
        cx: &Context<Self>,
    ) -> Option<crate::ui::app::OverlayTop> {
        let front = self.document_front()?;
        if self.document_dock_px(window, cx).is_none() {
            return Some(front);
        }
        let tab = self.tabs.get(self.active)?;
        if tab.pane.leaves().iter().any(|leaf| leaf.contains_focused(window, cx)) {
            return None;
        }
        let document = self.editor_panel_has_focus(window, cx)
            || tab.diff_overlay.as_ref().is_some_and(|diff| diff.focus_handle.contains_focused(window, cx))
            || self.status_document_focus.filter(|(id, _)| *id == tab.tree_id.get()).is_none_or(|(_, document)| document);
        document.then_some(front)
    }

    pub(crate) fn render_status_bar(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let document_context = self.status_document_context(window, cx);
        let document = document_context.is_some();
        let location = match document_context {
            Some(crate::ui::app::OverlayTop::Code) => self.editor_active_location(),
            Some(crate::ui::app::OverlayTop::Diff) => self.tabs.get(self.active)
                .and_then(|tab| tab.diff_overlay.as_ref())
                .map(|diff| (diff.host_id, diff.cwd.as_path())),
            None => None,
        };
        let host = location.map_or_else(|| self.remote_machine_label(cx), |(id, _)| {
            if id.is_local() {
                return t(L10nKey::RemoteThisComputer).to_string();
            }
            crate::core::session::WorkspaceStore::all(cx).views.iter()
                .filter_map(|workspace| workspace.host.as_ref())
                .find(|host| host.host_id() == id)
                .map(|host| crate::ui::remote_connect::route_label(cx, host))
                .or_else(|| self.host_snapshots.get(&id).map(|snapshot| snapshot.target.to_string()))
                .unwrap_or_else(|| crate::ui::i18n::t_fmt(L10nKey::StatusRemoteHost, &[("id", &format!("{:016x}", id.0))]))
        });
        let path = if document {
            location.map(|(host, path)| {
                if host.is_local() {
                    crate::ui::path_display::native_separators(path).display().to_string()
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
            .when(!document, |bar| bar.child(self.render_host_metrics(cx)))
            .when(document_context == Some(crate::ui::app::OverlayTop::Code), |bar| {
                bar.child(self.render_document_status(cx))
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
