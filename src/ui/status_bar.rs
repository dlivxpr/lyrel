//! 全局状态栏；工作区上下文由窗口投影，时钟独立更新，不触发终端重绘。

use chrono::Timelike as _;
use gpui_kit::TestSupportExt as _;
use gpui_kit::component::button::ButtonVariants as _;
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, button::Button, h_flex};
use gpui_kit::{AnyElement, Context, SharedString, Window, div, prelude::*, rems, svg};

use crate::ui::app::LyrelApp;
use lyrel_ui_base::actions::{OpenSettings, ToggleSwitcher};
use lyrel_ui_base::i18n::{L10nKey, t};

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
            .id("status-clock")
            .test_support()
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
            if let Some(tab) = this.tabs.get(this.tabs.active_index()) {
                this.status_document_focus = Some((tab.id(), true));
                cx.notify();
            }
        })
        .detach();
    }

    pub(crate) fn status_document_context(
        &self,
        window: &Window,
        cx: &Context<Self>,
    ) -> Option<crate::ui::tab::OverlayTop> {
        let front = self.document_front()?;
        if self.document_dock_px(window, cx).is_none() {
            return Some(front);
        }
        let tab = self.tabs.get(self.tabs.active_index())?;
        if tab
            .pane
            .leaves()
            .iter()
            .any(|leaf| leaf.contains_focused(window, cx))
        {
            return None;
        }
        let document = self.editor_panel_has_focus(window, cx)
            || tab
                .diff_overlay
                .as_ref()
                .is_some_and(|diff| diff.focus_handle.contains_focused(window, cx))
            || tab
                .github_detail
                .as_ref()
                .is_some_and(|doc| doc.focus_handle.contains_focused(window, cx))
            || self
                .status_document_focus
                .filter(|(id, _)| *id == tab.id())
                .is_none_or(|(_, document)| document);
        document.then_some(front)
    }

    pub(crate) fn render_status_bar(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let document_context = self.status_document_context(window, cx);
        let document = document_context.is_some();
        let location = match document_context {
            Some(crate::ui::tab::OverlayTop::Code) => self.editor_active_location(cx),
            Some(crate::ui::tab::OverlayTop::Diff) => self
                .tabs
                .get(self.tabs.active_index())
                .and_then(|tab| tab.diff_overlay.as_ref())
                .map(|diff| (diff.host_id, diff.cwd.as_path())),
            Some(crate::ui::tab::OverlayTop::Github) => self
                .tabs
                .get(self.tabs.active_index())
                .and_then(|tab| tab.github_detail.as_ref())
                .map(|doc| (doc.host_id, doc.cwd.as_path())),
            None => None,
        };
        let host = location.map_or_else(
            || self.remote_machine_label(cx),
            |(id, _)| {
                if id.is_local() {
                    return t(L10nKey::RemoteThisComputer).to_string();
                }
                lyrel_ui_base::session::WorkspaceStore::all(cx)
                    .views
                    .iter()
                    .filter_map(|workspace| workspace.host.as_ref())
                    .find(|host| host.host_id() == id)
                    .map(|host| crate::ui::desktop_lookups::route_label(cx, host))
                    .or_else(|| {
                        self.host_snapshots
                            .get(&id)
                            .map(|snapshot| snapshot.target.to_string())
                    })
                    .unwrap_or_else(|| {
                        lyrel_ui_base::i18n::t_fmt(
                            L10nKey::StatusRemoteHost,
                            &[("id", &format!("{:016x}", id.0))],
                        )
                    })
            },
        );
        let path = if document {
            location.map(|(host, path)| {
                if host.is_local() {
                    lyrel_ui_base::path_display::native_separators(path)
                        .display()
                        .to_string()
                } else {
                    path.display().to_string()
                }
            })
        } else {
            self.tab_cwd_text(self.tabs.active_index(), window, cx)
        };
        let leaf = self
            .tabs
            .get(self.tabs.active_index())
            .and_then(|tab| tab.pane.focused_or_first(window, cx));
        let branch = (!document)
            .then(|| {
                leaf.as_ref()
                    .and_then(|leaf| leaf.read(cx).git_status(cx))
                    .map(|git| git.branch)
            })
            .flatten();
        let program = (!document)
            .then(|| {
                leaf.as_ref().and_then(|leaf| {
                    leaf.read(cx).with_program_status(|status| {
                        status
                            .representative()
                            .map(|record| crate::ui::program_status::current_status(record, cx))
                    })
                })
            })
            .flatten();
        let context = h_flex()
            .flex_1()
            .min_w_0()
            .gap_1()
            .child(
                Button::new("status-host")
                    .ghost()
                    .xsmall()
                    // Button defaults to shrink=0; instance styles are applied
                    // last, and its native label already owns ellipsis.
                    .flex_shrink_1()
                    .min_w(rems(1.5))
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
                        .flex_shrink_1()
                        .min_w(rems(1.5))
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
            .children(program)
            // Document state lives in the editor panel's own bottom bar now;
            // the global bar only carries window context.
            .child(
                h_flex()
                    .flex_none()
                    .gap_1()
                    .when(!document, |controls| {
                        controls.child(self.render_host_metrics(cx))
                    })
                    .child(self.status_clock.clone())
                    .child(self.notification_bell.clone()),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use crate::ui::app::test_window::harness_with_pane;
    use gpui_kit::test::TestWindowExt as _;
    use gpui_kit::{TestAppContext, VisualTestContext, px, size};
    use lyrel_ui_base::actions::{EditorNewFile, ToggleCodePanel, ToggleDocumentFill};
    use lyrel_ui_base::config::Config;

    fn settle(cx: &mut VisualTestContext) {
        cx.run_until_parked();
        cx.update(|window, cx| window.render_frame(cx));
        cx.run_until_parked();
    }

    fn assert_global_controls_in_bounds(window: &gpui_kit::Window) {
        let bar = window.find("workbench-status").bounds();
        assert!(bar.right() <= window.viewport_size().width);
        for id in ["status-settings", "status-clock", "status-notifications"] {
            let control = window.find(id);
            let bounds = control.bounds();
            assert!(control.visible(), "{id} must remain visible");
            assert!(
                bounds.size.width >= window.rem_size() * 1.25,
                "{id} must retain its standard compact target width"
            );
            assert!(
                bounds.left() >= bar.left() && bounds.right() <= bar.right(),
                "{id} must remain horizontally inside the status bar"
            );
            assert!(
                bounds.top() >= bar.top() && bounds.bottom() <= bar.bottom(),
                "{id} must remain vertically inside the status bar"
            );
        }
    }

    #[gpui_kit::test]
    fn long_host_and_cjk_path_yield_to_global_controls_at_narrow_zoom(cx: &mut TestAppContext) {
        use lyrel_ui_base::session::{
            RemoteRef, RemoteTarget, WindowView, WindowViews, WorkspaceId, WorkspaceStore,
        };
        let (app, mut vcx, _stream) = harness_with_pane(cx);
        let hostname = "构建主机很长的机器名称-build-host.example.internal";
        let path =
            "/srv/项目目录/具有很长名称的工作区/多语言测试与生产环境/源码目录/工作台状态栏回归";
        let remote = WindowView::on_remote(RemoteRef::new(
            RemoteTarget::direct("developer", hostname, 22),
            WorkspaceId::new(),
        ));
        app.update_in(&mut vcx, |app, _, cx| {
            app.workspace = remote.id;
            let mut views = WorkspaceStore::all(cx).clone();
            views.views.push(remote);
            WorkspaceStore::install_for_test(
                cx,
                WindowViews {
                    views: views.views,
                    active: None,
                },
            );
            let workspace = app.window_workspace(cx);
            app.tabs[app.tabs.active_index()].pane.terminals()[0].update(cx, |pane, _| {
                pane.set_workspace(workspace);
                pane.terminal.seed_cwd(Some(path.into()));
            });
            app.sidebar_collapsed = true;
            app.right_panel_visible = false;
            cx.notify();
        });
        let mut wide_path = None;
        for (width, font) in [(1440., 16.), (1440., 24.), (720., 20.), (720., 24.)] {
            vcx.simulate_resize(size(px(width), px(720.)));
            app.update_in(&mut vcx, |_, _, cx| {
                cx.global_mut::<Config>().ui_font_size = font;
                gpui_kit::component::Theme::update(cx, |theme| theme.font_size = px(font));
                cx.notify();
            });
            settle(&mut vcx);
            vcx.update(|window, cx| {
                assert_global_controls_in_bounds(window);
                let bar = window.find("workbench-status").bounds();
                let metrics = window.find("status-host-metrics").bounds();
                let clock = window.find("status-clock").bounds();
                for id in ["status-host-metrics", "status-cpu", "status-memory"] {
                    let control = window.find(id);
                    assert!(control.visible(), "{id} must remain visible");
                    assert!(control.bounds().left() >= bar.left());
                    assert!(control.bounds().right() <= clock.left());
                }
                let host = window.find("status-host");
                assert!(host.label().unwrap().contains(hostname));
                assert!(host.visible());
                let path_bounds = window.find("status-path").bounds();
                assert!(path_bounds.left() >= host.bounds().right());
                assert!(path_bounds.right() <= metrics.left());
                assert!(path_bounds.size.width >= window.rem_size() * 1.25);
                if font == 24. {
                    if width == 1440. {
                        wide_path = Some(path_bounds.size.width);
                    } else {
                        assert!(
                            path_bounds.size.width < wide_path.unwrap(),
                            "the path must truncate before global controls leave the window"
                        );
                    }
                }
                window.click("status-path", cx);
                assert_eq!(
                    cx.read_from_clipboard().unwrap().text().as_deref(),
                    Some(path)
                );
                window.click("status-host", cx);
            });
            settle(&mut vcx);
            assert!(app.read_with(&vcx, |app, _| app.switcher.is_some()));
            vcx.simulate_keystrokes("escape");
            settle(&mut vcx);
            vcx.update(|window, cx| window.click("status-notifications", cx));
            settle(&mut vcx);
            vcx.update(|window, _| assert!(window.find("notification-inbox").visible()));
            vcx.simulate_keystrokes("escape");
            settle(&mut vcx);
            vcx.update(|window, cx| window.click("status-settings", cx));
            settle(&mut vcx);
            assert!(app.read_with(&vcx, |app, _| app.settings.is_some()));
            vcx.simulate_keystrokes("escape");
            settle(&mut vcx);
        }
    }

    #[gpui_kit::test]
    fn settings_remains_reachable_below_terminal_and_filled_document(cx: &mut TestAppContext) {
        let (app, mut vcx, _stream) = harness_with_pane(cx);
        for (width, font) in [(1440., 16.), (720., 20.), (720., 24.)] {
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
                        let pane = app.tabs[app.tabs.active_index()].pane.terminals()[0].read(cx);
                        assert!(pane.terminal.term.lock().screen_lines() >= 24,
                            "the terminal must use the content height, not collapse to its intrinsic size");
                    });
                }
                let line_ending = vcx.update(|window, cx| {
                    let status = window.find("workbench-status").bounds();
                    let content = window.find("workbench-content").bounds();
                    assert_global_controls_in_bounds(window);
                    let mut line_ending = None;
                    if filled && width == 720. {
                        let local = window.find("code-status-bar").bounds();
                        let document = window.find("document-status").bounds();
                        assert!(
                            local.contains(&document.center()),
                            "document status must sit inside the editor's local bar"
                        );
                        assert!(
                            local.bottom() <= status.top(),
                            "the local bar must sit above the global status bar"
                        );
                        assert!(document.right() <= local.right());
                        // At the largest UI scale the local bar's items
                        // overflow; scroll its own strip to reach the tail.
                        window.scroll(
                            "document-status",
                            gpui_kit::ScrollDelta::Pixels(gpui_kit::point(px(-4000.), px(0.))),
                            cx,
                        );
                        let eol = window.find("status-eol");
                        assert!(eol.visible());
                        assert!(eol.bounds().right() <= local.right());
                        assert_global_controls_in_bounds(window);
                        let label = eol.label().unwrap().to_owned();
                        window.click("status-eol", cx);
                        line_ending = Some(label);
                    }
                    assert!(content.bottom() <= status.top());
                    assert!(status.bottom() <= window.viewport_size().height);
                    assert!(window.find("status-settings").visible());
                    line_ending
                });
                if let Some(label) = line_ending {
                    // The editor's click notifies the window through a GPUI event
                    // after the input callback returns.
                    settle(&mut vcx);
                    vcx.update(|window, _| {
                        assert_ne!(window.find("status-eol").label(), Some(label.as_str()));
                    });
                }
                vcx.update(|window, cx| window.click("status-settings", cx));
                settle(&mut vcx);
                assert!(app.read_with(&vcx, |app, _| app.settings.is_some()));
                vcx.simulate_keystrokes("escape");
                settle(&mut vcx);
                assert!(app.read_with(&vcx, |app, _| app.settings.is_none()));
                if filled {
                    vcx.dispatch_action(ToggleCodePanel);
                    settle(&mut vcx);
                }
            }
        }
    }

    #[gpui_kit::test]
    fn markdown_status_belongs_to_the_document_panel(cx: &mut TestAppContext) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("status.md");
        std::fs::write(&path, "# Status\n").unwrap();
        let (app, mut vcx, _stream) = crate::ui::app::test_window::harness_with_tabs(cx, 1);
        app.update_in(&mut vcx, |app, window, cx| {
            app.open_file_in_editor(&path, window, cx)
        });
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
        while app.read_with(&vcx, |app, cx| !app.markdown_document_active(cx)) {
            assert!(
                std::time::Instant::now() < deadline,
                "Markdown fixture loads"
            );
            settle(&mut vcx);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        settle(&mut vcx);
        vcx.update(|window, _| {
            let document = window.find("document-status").bounds();
            let local = window.find("markdown-status-bar").bounds();
            let global = window.find("workbench-status").bounds();
            assert!(
                local.contains(&document.center()),
                "Markdown status must be inside its editor panel: document={document:?}, local={local:?}"
            );
            assert!(
                document.bottom() <= global.top(),
                "Markdown status sits above, not inside, the global status bar"
            );
        });
    }

    #[gpui_kit::test]
    fn status_bar_shows_only_osc7501_program_status_without_duplicate_agent_slot(
        cx: &mut TestAppContext,
    ) {
        use crate::core::cli_agent::{AgentSessionState, AgentStatus, CLIAgent};
        use crate::daemon::protocol::DaemonMsg;
        use lyrel_core::core::program_status::{
            ProgramRecord, ProgramSnapshot, ProgramState, ProgramStatusFrame,
        };

        lyrel_ui_base::i18n::set_locale("en");
        let (app, mut vcx, mut stream) = harness_with_pane(cx);
        let leaf = app.read_with(&vcx, |app, _| {
            app.tabs[0]
                .pane
                .first_leaf()
                .and_then(|slot| slot.terminal().cloned())
                .expect("test pane")
        });

        DaemonMsg::Agent(Some(CLIAgent::OhMyPi))
            .encode(&mut stream)
            .unwrap();
        DaemonMsg::AgentStatus(Some(Box::new(AgentSessionState {
            status: AgentStatus::Working,
            rich: true,
            turns: 1,
            ..Default::default()
        })))
        .encode(&mut stream)
        .unwrap();
        DaemonMsg::ProgramStatus(ProgramStatusFrame::Baseline(ProgramSnapshot::new(
            "sess-1".into(),
            1,
            0,
            vec![ProgramRecord::new(
                String::new(),
                ProgramState::Working,
                None,
                Some("omp".into()),
                None,
                None,
                None,
                1,
                1,
                0,
            )],
        )))
        .encode(&mut stream)
        .unwrap();

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            vcx.run_until_parked();
            let ready = leaf.read_with(&vcx, |view, _| {
                view.agent() == Some(CLIAgent::OhMyPi)
                    && view.with_program_status(|status| status.records().count() == 1)
            });
            if ready {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "timed out waiting for agent and program status"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        settle(&mut vcx);
        vcx.update(|window, _| {
            let program = window.find("status-program");
            assert!(program.visible());
            assert_eq!(program.label(), Some("Program · omp · Working"));
            assert!(
                window.try_find("status-agent").is_none(),
                "status bar must not render a duplicate agent status slot"
            );
        });
    }
}
