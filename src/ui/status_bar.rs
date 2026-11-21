//! 全局状态栏；工作区上下文由窗口投影，时钟独立更新，不触发终端重绘。

use chrono::Timelike as _;
use gpui_kit::TestSupportExt as _;
use gpui_kit::component::button::ButtonVariants as _;
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, button::Button, h_flex};
use gpui_kit::{AnyElement, Context, SharedString, Window, div, prelude::*, rems};

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
            self.context_cwd_text(window, cx)
        };
        // The Git context reads the Context Pane — the last valid terminal
        // pane of the active Workspace — like every other surface that names
        // a repository (#349). A document, a menu or the panel holding focus
        // must not hand the subject back to the tab's first leaf.
        let leaf = self.context_pane(window, cx);
        let git = leaf.as_ref().and_then(|leaf| leaf.read(cx).git_status(cx));
        let branch = git.as_ref().map(|git| git.branch.clone());
        let counts = git
            .filter(|git| git.added > 0 || git.removed > 0)
            .map(|git| (git.added, git.removed));
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
                // The branch names the repository; clicking it opens the same
                // searchable worktree picker the Changes header offers (#351).
                row.child(
                    Button::new("status-git-branch")
                        .ghost()
                        .xsmall()
                        .flex_shrink_1()
                        .min_w(rems(1.5))
                        .max_w(rems(12.))
                        .icon(Icon::empty().path("icons/git-branch.svg"))
                        .label(branch)
                        .tooltip(t(L10nKey::StatusGitWorktree))
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.open_worktree_picker(window, cx);
                        })),
                )
            })
            .when_some(counts, |row, (added, removed)| {
                // The real +added −removed line counts are a different action
                // from the worktree list: they open Changes, never the picker
                // or a branch checkout (#351). Kept distinct from the tool
                // tab's changed-item count, which counts entries.
                row.child(
                    Button::new("status-git-counts")
                        .ghost()
                        .xsmall()
                        .flex_none()
                        .label(format!("+{added} −{removed}"))
                        .tooltip(t(L10nKey::PanelScmTitle))
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.set_right_panel_tab(
                                lyrel_core::core::config::RightPanelTab::Scm,
                                window,
                                cx,
                            );
                        })),
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
    fn status_git_and_info_follow_the_context_pane(cx: &mut TestAppContext) {
        use crate::ui::app::{LyrelApp, test_window::harness};
        use crate::ui::pane::{Pane, PaneSlot};
        use crate::ui::scm::status_cache::{GitStatusCache, RepoSnapshot};
        use lyrel_core::core::config::RightPanelTab;
        use lyrel_terminal::view::quiet_test_pane;
        use lyrel_ui_base::actions::FocusRightPanelNavigation;
        use lyrel_ui_base::host_ops::HostId;

        lyrel_ui_base::i18n::set_locale("en");
        let (app, mut vcx) = harness(cx);
        vcx.update(|_, cx| {
            let mut cfg = cx.global::<Config>().clone();
            cfg.cursor_blink = false;
            cx.set_global(cfg);
        });
        let (left, right, _streams) = app.update_in(&mut vcx, |app, window, cx| {
            let (left, left_stream) = quiet_test_pane(1, window, cx);
            let (right, right_stream) = quiet_test_pane(2, window, cx);
            LyrelApp::watch_terminal(&left, window, cx);
            LyrelApp::watch_terminal(&right, window, cx);
            app.tabs.push(crate::ui::tab::Tab::new(Pane::split_node(
                gpui_kit::Axis::Horizontal,
                0.5,
                Pane::leaf(PaneSlot::Ready(left.clone())),
                Pane::leaf(PaneSlot::Ready(right.clone())),
            )));
            app.tabs.select(0);
            app.right_panel_visible = true;
            app.right_panel_tab = RightPanelTab::Info;
            // Two panes in two repositories: the cache knows each cwd's root
            // and branch, the way the tab badge probe would have reported it.
            for (view, cwd, branch) in [
                (&left, "/repo-left", "ctx-left"),
                (&right, "/repo-right", "ctx-right"),
            ] {
                let cwd = std::path::PathBuf::from(cwd);
                let root = cwd.clone();
                let home = cwd.clone();
                view.update(cx, |view, _| {
                    view.set_git_status_cwd_for_test(Some(cwd.clone()));
                    view.terminal.seed_cwd(Some(cwd.clone()));
                });
                cx.default_global::<GitStatusCache>().finish_probe(
                    HostId::LOCAL,
                    &cwd,
                    Some(RepoSnapshot {
                        root,
                        home,
                        branch: branch.into(),
                        counts: Some((0, 0)),
                    }),
                );
            }
            cx.notify();
            (left, right, (left_stream, right_stream))
        });
        vcx.background_executor.run_until_parked();

        let branch_text = |vcx: &mut VisualTestContext| -> Option<String> {
            vcx.update(|window, _| {
                window
                    .try_find("status-git-branch")
                    .filter(|element| element.visible())
                    .and_then(|element| element.label().map(str::to_owned))
            })
        };
        let info_cwd_shows = |vcx: &mut VisualTestContext, cwd: &str| -> bool {
            vcx.update(|window, _| {
                window
                    .try_find("panel-info-row-0")
                    .filter(|row| row.visible())
                    .and_then(|row| row.label().map(str::to_owned))
                    .is_some_and(|label| label.contains(cwd.trim_start_matches('/')))
            })
        };

        // Nothing has focus yet: the first visible leaf is the subject.
        settle(&mut vcx);
        vcx.update(|window, _| assert!(window.find("workbench-status").visible()));
        assert_eq!(branch_text(&mut vcx).as_deref(), Some("ctx-left"));
        assert!(info_cwd_shows(&mut vcx, "/repo-left"));

        // Focusing a different terminal pane moves the bottom Git indicator
        // and the Info session together.
        app.update_in(&mut vcx, |_, window, cx| {
            right.read(cx).focus_handle.clone().focus(window, cx);
        });
        settle(&mut vcx);
        assert_eq!(branch_text(&mut vcx).as_deref(), Some("ctx-right"));
        assert!(info_cwd_shows(&mut vcx, "/repo-right"));

        // Moving focus into the panel does not change which pane supplies the
        // data: the indicator stays on the last focused pane, not the tab's
        // first leaf. The panel's tab list settles its roving stop over a few
        // frames, the way the navigation consumer test settles it.
        vcx.dispatch_action(FocusRightPanelNavigation);
        for _ in 0..4 {
            vcx.run_until_parked();
            vcx.update(|window, cx| {
                window.simulate_next_frame(cx);
                window.render_frame(cx);
            });
        }
        vcx.run_until_parked();
        vcx.update(|window, _| {
            assert_eq!(
                window.find("tool-tab-info").focused(),
                Some(true),
                "the panel's tab list must own keyboard focus for this check"
            );
        });
        assert_eq!(branch_text(&mut vcx).as_deref(), Some("ctx-right"));
        assert!(info_cwd_shows(&mut vcx, "/repo-right"));

        // Opening a document changes its status controls, not the Git subject.
        vcx.dispatch_action(EditorNewFile);
        settle(&mut vcx);
        assert!(app.read_with(&vcx, |app, cx| app.active_buffer(cx).is_some()));
        assert_eq!(branch_text(&mut vcx).as_deref(), Some("ctx-right"));

        // Back in the terminal, then on to the other pane.
        vcx.simulate_keystrokes("escape");
        settle(&mut vcx);
        app.update_in(&mut vcx, |_, window, cx| {
            left.read(cx).focus_handle.clone().focus(window, cx);
        });
        settle(&mut vcx);
        assert_eq!(branch_text(&mut vcx).as_deref(), Some("ctx-left"));
        assert!(info_cwd_shows(&mut vcx, "/repo-left"));

        // There is no remembered repository pick to outrank the pane: the
        // panel's subject is the Context Pane itself, so focusing a different
        // terminal pane is the only thing that moves it (#349, #345).
        assert!(
            app.read_with(&vcx, |app, _| app.scm.active_repo().is_some()),
            "the Context Pane's repository is the panel's subject"
        );
        app.update_in(&mut vcx, |_, window, cx| {
            right.read(cx).focus_handle.clone().focus(window, cx);
        });
        settle(&mut vcx);
        assert_eq!(branch_text(&mut vcx).as_deref(), Some("ctx-right"));
        assert!(info_cwd_shows(&mut vcx, "/repo-right"));
    }

    #[gpui_kit::test]
    fn status_git_counts_open_changes_and_the_branch_opens_the_picker(cx: &mut TestAppContext) {
        use crate::ui::scm::status_cache::{GitStatusCache, RepoSnapshot};
        use lyrel_ui_base::host_ops::HostId;

        lyrel_ui_base::i18n::set_locale("en");
        let (app, mut vcx, _stream) = harness_with_pane(cx);
        let pane = app.read_with(&vcx, |app, _| {
            app.tabs[0]
                .pane
                .first_leaf()
                .and_then(|slot| slot.terminal().cloned())
                .expect("test pane")
        });
        let cwd = std::path::PathBuf::from("/repo-left");
        app.update_in(&mut vcx, |_, _, cx| {
            pane.update(cx, |view, _| {
                view.set_git_status_cwd_for_test(Some(cwd.clone()));
            });
            cx.default_global::<GitStatusCache>().finish_probe(
                HostId::LOCAL,
                &cwd,
                Some(RepoSnapshot {
                    root: cwd.clone(),
                    home: cwd.clone(),
                    branch: "ctx-left".into(),
                    counts: Some((5, 3)),
                }),
            );
            cx.notify();
        });
        settle(&mut vcx);

        // Real +added −removed counts sit beside the branch…
        vcx.update(|window, _| {
            let counts = window.find("status-git-counts");
            assert!(counts.visible());
            assert_eq!(counts.label(), Some("+5 −3"));
        });
        // …and open Changes — not the picker.
        vcx.update(|window, cx| window.click("status-git-counts", cx));
        settle(&mut vcx);
        app.read_with(&vcx, |app, _| {
            assert!(app.right_panel_visible, "counts open the Changes tool");
            assert_eq!(
                app.right_panel_tab,
                lyrel_core::core::config::RightPanelTab::Scm
            );
            assert!(
                app.worktree_picker.is_none(),
                "the counts action never opens the worktree picker"
            );
        });

        // The branch opens the shared picker instead.
        vcx.update(|window, cx| window.click("status-git-branch", cx));
        settle(&mut vcx);
        assert!(app.read_with(&vcx, |app, _| app.worktree_picker.is_some()));
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
