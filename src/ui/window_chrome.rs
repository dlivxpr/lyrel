//! 窗口级标题栏与常驻六按钮；独立于标签导航和文档布局。

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Icon, TitleBar, h_flex};
use gpui_kit::{AnyElement, Context, Window, prelude::*, px, rems};

use crate::core::actions::{
    SplitDown, SplitRight, ToggleLeftPanel, TogglePalette, ToggleRightPanel,
};
use crate::ui::app::{LyrelApp, window_controls_drawn};
use crate::ui::i18n::{L10nKey, t};
use crate::ui::search::SearchTab;
use crate::ui::tab_strip::chrome_tile_variant;

/// 原生窗口按钮仍由 TitleBar 留出空间，产品控件尺寸跟随 rem。
impl LyrelApp {
    pub(crate) fn render_window_chrome(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let macos_fullscreen = cfg!(target_os = "macos") && window.is_fullscreen();
        let left_open = self.left_panel_open(cx);
        let right_open = self.right_panel_open(cx);
        let can_split = self
            .tabs
            .get(self.tabs.active_index())
            .and_then(|tab| tab.detail_pane(window, cx))
            .is_some();
        let tile = |button: Button| button.custom(chrome_tile_variant(cx)).size_8().rounded_lg();
        let group = || {
            h_flex()
                // 按钮区不能穿透到 TitleBar 的原生命中与按下拖动处理。
                .occlude()
                .flex_none()
                .p_0p5()
                .gap_0p5()
                .rounded_lg()
                .bg(cx.theme().muted.opacity(0.5))
        };
        let left = group()
            .child(
                tile(
                    Button::new("titlebar-left-panel").icon(
                        Icon::empty()
                            .path(if left_open {
                                "icons/panel-left.svg"
                            } else {
                                "icons/panel-left-collapsed.svg"
                            })
                            .size_4(),
                    ),
                )
                .accessibility_label(t(if left_open {
                    L10nKey::TabTooltipHideSidebar
                } else {
                    L10nKey::TabTooltipShowSidebar
                }))
                .tooltip_with_action(
                    t(if left_open {
                        L10nKey::TabTooltipHideSidebar
                    } else {
                        L10nKey::TabTooltipShowSidebar
                    }),
                    &ToggleLeftPanel,
                    None,
                )
                .on_click(cx.listener(|this, _, window, cx| {
                    this.toggle_left_panel(cx);
                    this.focus_previous_content(window, cx);
                })),
            )
            .child(
                tile(
                    Button::new("titlebar-search")
                        .icon(Icon::empty().path("icons/command.svg").size_4()),
                )
                .accessibility_label(t(L10nKey::AppMenuSearchEverywhere))
                .tooltip_with_action(t(L10nKey::AppMenuSearchEverywhere), &TogglePalette, None)
                .on_click(
                    cx.listener(|this, _, window, cx| match this.search.is_some() {
                        true => this.close_search(window, cx),
                        false => this.open_search(SearchTab::All, "", window, cx),
                    }),
                ),
            )
            .child(self.new_tab_button("titlebar-add", cx));
        let right = group()
            .child(
                tile(
                    Button::new("titlebar-split-right")
                        .icon(Icon::empty().path("icons/split-right.svg").size_4()),
                )
                .disabled(!can_split)
                .accessibility_label(t(L10nKey::AppMenuSplitRight))
                .tooltip_with_action(t(L10nKey::AppMenuSplitRight), &SplitRight, None)
                .on_click(cx.listener(|this, _, window, cx| {
                    this.split(gpui_kit::Axis::Horizontal, window, cx)
                })),
            )
            .child(
                tile(
                    Button::new("titlebar-split-down")
                        .icon(Icon::empty().path("icons/split-down.svg").size_4()),
                )
                .disabled(!can_split)
                .accessibility_label(t(L10nKey::AppMenuSplitDown))
                .tooltip_with_action(t(L10nKey::AppMenuSplitDown), &SplitDown, None)
                .on_click(cx.listener(|this, _, window, cx| {
                    this.split(gpui_kit::Axis::Vertical, window, cx)
                })),
            )
            .child(
                tile(
                    Button::new("titlebar-right-panel").icon(
                        Icon::empty()
                            .path(if right_open {
                                "icons/panel-right.svg"
                            } else {
                                "icons/panel-right-collapsed.svg"
                            })
                            .size_4(),
                    ),
                )
                .accessibility_label(t(if right_open {
                    L10nKey::TabTooltipHideDetailPanel
                } else {
                    L10nKey::TabTooltipShowDetailPanel
                }))
                .tooltip_with_action(
                    t(if right_open {
                        L10nKey::TabTooltipHideDetailPanel
                    } else {
                        L10nKey::TabTooltipShowDetailPanel
                    }),
                    &ToggleRightPanel,
                    None,
                )
                .on_click(cx.listener(|this, _, window, cx| {
                    this.toggle_right_panel(cx);
                    this.focus_previous_content(window, cx);
                })),
            );
        let left_row = h_flex()
            .id("workbench-left-drag")
            .w_full()
            .min_w_0()
            .h(rems(3.))
            .flex_none()
            .border_b_1()
            .border_color(cx.theme().transparent)
            // 仅窗口模式保留原生交通灯几何；全屏恢复产品控件的边距。
            .pl(px(crate::ui::app::TITLE_BAR_LEAD))
            .when(macos_fullscreen, |row| row.pl_3())
            .pr_1()
            .gap_2()
            .child(crate::ui::app::window_mark())
            .child(left);
        let left = crate::ui::app::title_bar_drag(left_row, "workbench-left-drag", window, cx)
            .into_any_element();
        let trailing_inset = if macos_fullscreen { 0.75 } else { 0.25 };
        let strip = h_flex()
            .w_full()
            .min_w_0()
            .h_full()
            .justify_end()
            .pr(rems(trailing_inset))
            .child(right);
        let controls_drawn = window_controls_drawn(window.is_fullscreen());
        let native_width = if controls_drawn {
            let controls = window.window_controls();
            let count = 1 + u8::from(controls.minimize) + u8::from(controls.maximize);
            gpui_kit::component::TITLE_BAR_HEIGHT.as_f32() * f32::from(count)
        } else {
            0.
        };
        // 三按钮组与外边距共用宽度预算；原生控件不随界面缩放。
        let right_width = window.rem_size().as_f32() * (6.5 + trailing_inset) + native_width;
        let right = if !macos_fullscreen && (controls_drawn || cfg!(target_os = "macos")) {
            TitleBar::new()
                .h(rems(3.))
                .pl_0()
                .bg(cx.theme().transparent)
                .border_color(cx.theme().transparent)
                .child(strip)
                .into_any_element()
        } else {
            // 全屏无原生控件；避免 TitleBar 内部的全屏占位挤出按钮组。
            let row = h_flex()
                .id("workbench-right-drag")
                .h(rems(3.))
                .flex_none()
                .w_full()
                .child(strip);
            crate::ui::app::title_bar_drag(row, "workbench-right-drag", window, cx)
                .into_any_element()
        };
        h_flex()
            .flex_none()
            .h(rems(3.))
            .w_full()
            .child(gpui_kit::div().flex_1().min_w_0().child(left))
            .child(gpui_kit::div().flex_none().w(px(right_width)).child(right))
            .into_any_element()
    }
}

#[cfg(test)]
mod gpui_tests {
    use std::io::ErrorKind;

    use gpui_kit::test::{TestEventExt as _, TestWindowExt as _};
    use gpui_kit::{Entity, TestAppContext, VisualTestContext, px, size};

    use crate::core::actions::{ActivateTab1, ActivateTab2};
    use crate::core::config::RightPanelTab;
    use crate::daemon::protocol::ClientMsg;
    use crate::daemon::transport::Stream;
    use crate::ui::app::{LyrelApp, test_window::harness_with_tabs};

    fn settle(vcx: &mut VisualTestContext) {
        vcx.run_until_parked();
        vcx.update(|window, cx| window.render_frame(cx));
        vcx.run_until_parked();
    }

    fn wait_for_editor(app: &Entity<LyrelApp>, vcx: &mut VisualTestContext) {
        // HostOps 的真实线程池不随 GPUI executor 一起排空。
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            settle(vcx);
            if app.read_with(vcx, |app, _| app.editor_active_local_file().is_some()) {
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the document did not finish opening"
            );
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }

    fn terminal_focused(app: &Entity<LyrelApp>, vcx: &mut VisualTestContext) -> bool {
        app.update_in(vcx, |app, window, cx| {
            app.tabs[app.tabs.active_index()]
                .pane
                .leaves()
                .iter()
                .any(|leaf| leaf.contains_focused(window, cx))
        })
    }

    fn input(stream: &mut Stream) -> Option<Vec<u8>> {
        stream
            .set_read_timeout(Some(std::time::Duration::from_millis(100)))
            .unwrap();
        loop {
            match ClientMsg::read(stream) {
                Ok(ClientMsg::Input(bytes)) => return Some(bytes),
                Ok(_) => continue,
                Err(error)
                    if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) =>
                {
                    return None;
                }
                Err(error) => panic!("pane connection failed: {error}"),
            }
        }
    }
    #[cfg(target_os = "macos")]
    #[gpui_kit::test]
    fn macos_fullscreen_chrome_uses_compact_balanced_edge_insets(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        let edge_controls = ["titlebar-left-panel", "titlebar-right-panel"];
        for (width, font_size) in [(1440., 16.), (760., 16.), (1440., 20.), (760., 20.)] {
            app.update_in(&mut vcx, |_, _, cx| {
                cx.global_mut::<crate::core::config::Config>().ui_font_size = font_size;
                cx.notify();
            });
            vcx.simulate_resize(size(px(width), px(900.)));
            settle(&mut vcx);
            let windowed = vcx.update(|window, _| {
                assert!(!window.is_fullscreen());
                edge_controls.map(|id| window.find(id).bounds())
            });
            vcx.dispatch_action(crate::core::actions::ToggleFullscreen);
            settle(&mut vcx);
            let fullscreen = vcx.update(|window, _| {
                assert!(window.is_fullscreen());
                edge_controls.map(|id| window.find(id).bounds())
            });
            for (left_open, right_open) in
                [(true, true), (false, true), (false, false), (true, false)]
            {
                app.update_in(&mut vcx, |app, _, cx| {
                    app.sidebar_collapsed = !left_open;
                    app.right_panel_visible = right_open;
                    cx.notify();
                });
                settle(&mut vcx);
                vcx.update(|window, _| {
                    let [left, right] = edge_controls.map(|id| window.find(id).bounds());
                    let right_gap = window.viewport_size().width - right.right();
                    assert_eq!([left, right], fullscreen, "sidebars cannot move fullscreen controls");
                    assert!(
                        left.left() < window.rem_size() * 4.,
                        "fullscreen must not reserve the hidden traffic lights' leading space: {left:?}"
                    );
                    assert!(left.left() < windowed[0].left());
                    assert!(
                        right_gap >= window.rem_size() * 0.75 && right_gap <= window.rem_size(),
                        "fullscreen controls need a small trailing inset rather than hugging the window edge: {right_gap}"
                    );
                });
            }
            vcx.dispatch_action(crate::core::actions::ToggleFullscreen);
            settle(&mut vcx);
            vcx.update(|window, _| {
                assert!(!window.is_fullscreen());
                assert_eq!(edge_controls.map(|id| window.find(id).bounds()), windowed);
            });
        }
    }

    #[gpui_kit::test]
    fn sidebar_toggles_preserve_titlebar_and_terminal_height(cx: &mut TestAppContext) {
        let (app, mut vcx, mut streams) = harness_with_tabs(cx, 1);
        vcx.simulate_resize(size(px(1440.), px(900.)));
        app.update_in(&mut vcx, |app, window, cx| {
            app.sidebar_collapsed = false;
            app.right_panel_visible = true;
            app.focus_active(window, cx);
            cx.notify();
        });
        settle(&mut vcx);
        let area = app.read_with(&vcx, |app, _| app.pane_area.get().unwrap());
        let title_height = vcx.update(|window, _| window.rem_size() * 3.);
        assert_eq!(
            area.top(),
            title_height,
            "the terminal starts below the persistent titlebar even with both sidebars open"
        );
        let rows = app.read_with(&vcx, |app, cx| {
            app.tabs[0].pane.terminals()[0]
                .read(cx)
                .terminal
                .size()
                .rows
        });
        let controls = [
            "titlebar-left-panel",
            "titlebar-search",
            "titlebar-add",
            "titlebar-split-right",
            "titlebar-split-down",
            "titlebar-right-panel",
        ];
        let control_bounds =
            vcx.update(|window, _| controls.map(|control| window.find(control).bounds()));
        for (button, left_open, right_open) in [
            ("titlebar-left-panel", false, true),
            ("titlebar-right-panel", false, false),
            ("titlebar-left-panel", true, false),
            ("titlebar-right-panel", true, true),
        ] {
            vcx.update(|window, cx| window.click(button, cx));
            settle(&mut vcx);
            app.update_in(&mut vcx, |app, _, cx| {
                assert_eq!(app.left_panel_open(cx), left_open);
                assert_eq!(app.right_panel_open(cx), right_open);
                let current = app.pane_area.get().unwrap();
                assert_eq!(
                    current.top(),
                    area.top(),
                    "sidebar toggles cannot move the terminal top"
                );
                assert_eq!(current.bottom(), area.bottom());
                assert_eq!(
                    app.tabs[0].pane.terminals()[0]
                        .read(cx)
                        .terminal
                        .size()
                        .rows,
                    rows,
                    "sidebar toggles change PTY columns, not its row budget"
                );
            });
            vcx.update(|window, cx| {
                for (control, expected) in controls.into_iter().zip(control_bounds) {
                    let bounds = window.find(control).bounds();
                    assert_eq!(bounds, expected, "{control} stays anchored to the window");
                    assert!(
                        bounds.bottom() <= area.top(),
                        "{control} cannot cover terminal rows"
                    );
                }
                let terminal = window.find("terminal-surface").bounds();
                assert_eq!(terminal.top(), area.top());
                // 首行中央保留窗格抓手；在抓手旁验证终端输入。
                window.click_at(
                    "terminal-surface",
                    gpui_kit::point(terminal.size.width / 4., px(12.)),
                    cx,
                );
            });
            settle(&mut vcx);
            assert!(terminal_focused(&app, &mut vcx));
            vcx.simulate_keystrokes("x");
            settle(&mut vcx);
            assert_eq!(input(&mut streams[0]).as_deref(), Some(&b"x"[..]));
        }
    }

    #[gpui_kit::test]
    fn titlebar_buttons_accept_clicks_with_pointer_drift(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        vcx.simulate_resize(size(px(1440.), px(900.)));
        settle(&mut vcx);

        for button in [
            "titlebar-left-panel",
            "titlebar-right-panel",
            "titlebar-search",
        ] {
            let before = app.read_with(&vcx, |app, cx| {
                (
                    app.left_panel_open(cx),
                    app.right_panel_open(cx),
                    app.search.is_some(),
                )
            });
            vcx.update(|window, cx| {
                let start = window.find(button).bounds().center();
                let end = start + gpui_kit::point(px(1.), px(1.));
                window.pointer_down(start, gpui_kit::MouseButton::Left, cx);
                window.pointer_move(end, Some(gpui_kit::MouseButton::Left), cx);
                window.pointer_up(end, gpui_kit::MouseButton::Left, cx);
            });
            settle(&mut vcx);
            let after = app.read_with(&vcx, |app, cx| {
                (
                    app.left_panel_open(cx),
                    app.right_panel_open(cx),
                    app.search.is_some(),
                )
            });
            let expected = match button {
                "titlebar-left-panel" => (!before.0, before.1, before.2),
                "titlebar-right-panel" => (before.0, !before.1, before.2),
                _ => (before.0, before.1, !before.2),
            };
            assert_eq!(
                after, expected,
                "{button} activates despite a 1px pointer drift"
            );
        }
    }

    #[gpui_kit::test]
    fn sidebar_chrome_preserves_document_and_terminal_input(cx: &mut TestAppContext) {
        let (app, mut vcx, mut streams) = harness_with_tabs(cx, 2);
        vcx.simulate_resize(size(px(1600.), px(900.)));
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("sidebar-focus.txt");
        std::fs::write(&path, "").unwrap();
        app.update_in(&mut vcx, |app, window, cx| {
            app.open_file_in_editor(&path, window, cx)
        });
        wait_for_editor(&app, &mut vcx);
        let editor = app.read_with(&vcx, |app, _| app.editor_active_local_file().unwrap().input);
        assert!(app.update_in(&mut vcx, |app, window, cx| {
            app.document_dock_px(window, cx).is_some()
        }));

        for button in ["titlebar-left-panel", "titlebar-right-panel"] {
            for _ in 0..2 {
                let before = editor.read_with(&vcx, |editor, _| editor.value().to_string());
                vcx.update(|window, cx| window.click(button, cx));
                settle(&mut vcx);
                vcx.simulate_keystrokes("d");
                settle(&mut vcx);
                assert_eq!(
                    editor.read_with(&vcx, |editor, _| editor.value().to_string()),
                    format!("{before}d"),
                    "{button} preserves the docked document's input on hide and show"
                );
                assert_eq!(input(&mut streams[0]), None);
                assert_eq!(input(&mut streams[1]), None);
            }
        }

        app.update_in(&mut vcx, |app, window, cx| app.focus_active(window, cx));
        settle(&mut vcx);
        let before = editor.read_with(&vcx, |editor, _| editor.value().to_string());
        for button in ["titlebar-left-panel", "titlebar-right-panel"] {
            for _ in 0..2 {
                vcx.update(|window, cx| window.click(button, cx));
                settle(&mut vcx);
                assert!(terminal_focused(&app, &mut vcx));
                vcx.simulate_keystrokes("t");
                settle(&mut vcx);
                assert_eq!(input(&mut streams[0]).as_deref(), Some(&b"t"[..]));
                assert_eq!(input(&mut streams[0]), None, "one key is delivered once");
                assert_eq!(input(&mut streams[1]), None);
                assert_eq!(
                    editor.read_with(&vcx, |editor, _| editor.value().to_string()),
                    before
                );
            }
        }
    }

    #[gpui_kit::test]
    fn changes_tab_count_tracks_real_scm_status_on_one_surface(cx: &mut TestAppContext) {
        use crate::daemon::protocol::DaemonMsg;
        let dir = tempfile::TempDir::new().unwrap();
        let root = lyrel_core::core::path_spelling::local_spelling_buf(
            &std::fs::canonicalize(dir.path()).unwrap(),
        );
        let init = std::process::Command::new("git")
            .args(["init", "--quiet"])
            .current_dir(&root)
            .output()
            .unwrap();
        assert!(init.status.success());
        let (app, mut vcx, mut streams) = harness_with_tabs(cx, 1);
        vcx.simulate_resize(size(px(1440.), px(900.)));
        DaemonMsg::Cwd(root.clone())
            .encode(&mut streams[0])
            .unwrap();
        app.update_in(&mut vcx, |app, _, cx| {
            app.right_panel_visible = true;
            app.right_panel_tab = RightPanelTab::Scm;
            cx.notify();
        });
        for expected in [0, 1, 3, 0] {
            for name in ["a.txt", "b.txt", "c.txt"] {
                let path = root.join(name);
                if path.exists() {
                    std::fs::remove_file(path).unwrap();
                }
            }
            for name in ["a.txt", "b.txt", "c.txt"].into_iter().take(expected) {
                std::fs::write(root.join(name), "changed\n").unwrap();
            }
            for at_prompt in [false, true] {
                DaemonMsg::Prompt {
                    active: true,
                    at_prompt,
                    last_exit: Some(0),
                }
                .encode(&mut streams[0])
                .unwrap();
            }
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
            loop {
                vcx.executor()
                    .advance_clock(std::time::Duration::from_millis(300));
                settle(&mut vcx);
                let ready = app.read_with(&vcx, |app, cx| {
                    app.scm.repo.as_ref().is_some_and(|repo| {
                        crate::terminal::git_data::status_of(cx, repo.host, &repo.root)
                            .is_some_and(|status| status.total_entries == expected)
                    })
                });
                if ready {
                    break;
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "SCM count did not reach {expected}"
                );
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            vcx.update(|window, cx| {
                window.render_frame(cx);
                let counts = window.find_all("right-panel-change-count");
                if expected == 0 {
                    assert!(counts.is_empty(), "a clean working tree clears the count");
                } else {
                    assert_eq!(counts.len(), 1, "Changes owns the only count surface");
                    let count = window
                        .within("panel-title")
                        .find("right-panel-change-count");
                    assert!(count.visible());
                    assert_eq!(count.label(), Some(expected.to_string().as_str()));
                }
            });
        }
    }

    #[gpui_kit::test]
    fn closing_panel_chrome_restores_terminal_input_with_either_sidebar_state(
        cx: &mut TestAppContext,
    ) {
        let (app, mut vcx, mut streams) = harness_with_tabs(cx, 2);
        vcx.simulate_resize(size(px(1440.), px(900.)));
        let original = app.read_with(&vcx, |app, cx| {
            (
                app.workspace,
                app.tabs
                    .iter()
                    .map(|tab| tab.id())
                    .collect::<Vec<_>>(),
                app.tabs
                    .iter()
                    .map(|tab| {
                        let terminal = tab.pane.terminals().pop().unwrap();
                        (terminal.entity_id(), terminal.read(cx).pane_id)
                    })
                    .collect::<Vec<_>>(),
            )
        });

        for collapsed in [false, true] {
            app.update_in(&mut vcx, |app, window, cx| {
                app.sidebar_collapsed = collapsed;
                app.right_panel_visible = false;
                app.right_panel_tab = RightPanelTab::Files;
                app.focus_active(window, cx);
                cx.notify();
            });
            settle(&mut vcx);
            vcx.update(|window, cx| {
                assert_eq!(window.find_all("titlebar-right-panel").len(), 1);
                window.click("titlebar-right-panel", cx);
            });
            settle(&mut vcx);
            app.update_in(&mut vcx, |app, window, cx| {
                assert!(app.right_panel_open(cx));
                app.file_search
                    .update(cx, |search, cx| search.focus(window, cx));
            });
            settle(&mut vcx);
            vcx.update(|window, cx| {
                assert_eq!(window.find_all("titlebar-right-panel").len(), 1);
                window.click("titlebar-right-panel", cx);
            });
            settle(&mut vcx);
            assert!(!app.read_with(&vcx, |app, cx| app.right_panel_open(cx)));
            assert!(terminal_focused(&app, &mut vcx));

            vcx.simulate_keystrokes("x");
            settle(&mut vcx);
            assert_eq!(input(&mut streams[0]).as_deref(), Some(&b"x"[..]));
            assert_eq!(input(&mut streams[0]), None, "one key is delivered once");
            assert_eq!(
                input(&mut streams[1]),
                None,
                "the hidden tab receives no key"
            );
            vcx.dispatch_action(ActivateTab2);
            settle(&mut vcx);
            assert_eq!(app.read_with(&vcx, |app, _| app.tabs.active_index()), 1);
            assert!(terminal_focused(&app, &mut vcx));
            vcx.dispatch_action(ActivateTab1);
            settle(&mut vcx);
        }
        app.read_with(&vcx, |app, cx| {
            assert_eq!(app.workspace, original.0);
            assert_eq!(
                app.tabs
                    .iter()
                    .map(|tab| tab.id())
                    .collect::<Vec<_>>(),
                original.1
            );
            assert_eq!(
                app.tabs
                    .iter()
                    .map(|tab| {
                        let terminal = tab.pane.terminals().pop().unwrap();
                        (terminal.entity_id(), terminal.read(cx).pane_id)
                    })
                    .collect::<Vec<_>>(),
                original.2
            );
        });
    }

    #[gpui_kit::test]
    fn navigation_clicks_and_modal_keys_reach_only_the_selected_session(cx: &mut TestAppContext) {
        let (app, mut vcx, mut streams) = harness_with_tabs(cx, 2);
        vcx.simulate_resize(size(px(1440.), px(900.)));
        let first_tab_key =
            vcx.update(|_, cx| crate::ui::keymap::effective_key("ActivateTab1", cx).unwrap());
        for collapsed in [false, true] {
            app.update_in(&mut vcx, |app, window, cx| {
                app.sidebar_collapsed = collapsed;
                app.right_panel_visible = false;
                app.focus_active(window, cx);
                cx.notify();
            });
            settle(&mut vcx);
            if collapsed {
                vcx.update(|window, cx| window.click("titlebar-left-panel", cx));
                settle(&mut vcx);
            }
            let second_tab = app.read_with(&vcx, |app, _| app.sidebar_slots.borrow()[1]);
            assert!(second_tab.size.width > px(0.));
            vcx.simulate_click(second_tab.center(), gpui_kit::Modifiers::none());
            settle(&mut vcx);
            assert_eq!(app.read_with(&vcx, |app, _| app.tabs.active_index()), 1);
            assert!(terminal_focused(&app, &mut vcx));

            vcx.update(|window, cx| window.click("titlebar-search", cx));
            settle(&mut vcx);
            assert!(app.read_with(&vcx, |app, _| app.search.is_some()));
            vcx.simulate_keystrokes(&first_tab_key);
            vcx.simulate_keystrokes("q");
            settle(&mut vcx);
            assert_eq!(app.read_with(&vcx, |app, _| app.tabs.active_index()), 1);
            assert_eq!(input(&mut streams[0]), None);
            assert_eq!(input(&mut streams[1]), None);
            vcx.simulate_keystrokes("escape");
            settle(&mut vcx);
            assert!(app.read_with(&vcx, |app, _| app.search.is_none()));
            assert!(terminal_focused(&app, &mut vcx));
            vcx.simulate_keystrokes("y");
            settle(&mut vcx);
            assert_eq!(input(&mut streams[1]).as_deref(), Some(&b"y"[..]));
            assert_eq!(input(&mut streams[0]), None);
            vcx.update(|window, _| {
                assert_eq!(window.viewport_size(), size(px(1440.), px(900.)));
            });
            vcx.dispatch_action(ActivateTab1);
            settle(&mut vcx);
        }
    }
    #[gpui_kit::test]
    fn file_fill_keeps_search_available_and_returns_input_to_the_document(cx: &mut TestAppContext) {
        use crate::core::actions::ToggleDocumentFill;
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("navigation.txt");
        std::fs::write(&path, "").unwrap();
        for collapsed in [false, true] {
            let (app, mut vcx, mut streams) = harness_with_tabs(cx, 1);
            vcx.simulate_resize(size(px(1000.), px(800.)));
            app.update_in(&mut vcx, |app, window, cx| {
                app.sidebar_collapsed = collapsed;
                app.right_panel_visible = false;
                app.focus_active(window, cx);
                cx.notify();
            });
            app.update_in(&mut vcx, |app, window, cx| {
                app.open_file_in_editor(&path, window, cx)
            });
            wait_for_editor(&app, &mut vcx);
            vcx.dispatch_action(ToggleDocumentFill);
            settle(&mut vcx);
            let editor =
                app.read_with(&vcx, |app, _| app.editor_active_local_file().unwrap().input);
            vcx.simulate_keystrokes("a");
            settle(&mut vcx);
            let before = editor.read_with(&vcx, |editor, _| editor.value().to_string());
            vcx.update(|window, cx| window.click("titlebar-search", cx));
            settle(&mut vcx);
            assert!(app.read_with(&vcx, |app, _| app.search.is_some()));
            vcx.simulate_keystrokes("q");
            vcx.simulate_keystrokes("escape");
            settle(&mut vcx);
            assert!(app.read_with(&vcx, |app, _| app.search.is_none()));
            assert_eq!(
                editor.read_with(&vcx, |editor, _| editor.value().to_string()),
                before
            );
            assert!(app.update_in(&mut vcx, |app, window, cx| {
                app.document_dock_px(window, cx).is_none()
            }));
            vcx.simulate_keystrokes("b");
            settle(&mut vcx);
            assert_eq!(
                editor.read_with(&vcx, |editor, _| editor.value().to_string()),
                format!("{before}b")
            );
            assert_eq!(
                input(&mut streams[0]),
                None,
                "search and editor input never leak into the terminal"
            );
        }
    }

    #[gpui_kit::test]
    fn persistent_controls_keep_navigation_and_real_sidebar_resizing_available(
        cx: &mut TestAppContext,
    ) {
        let (app, mut vcx, mut streams) = harness_with_tabs(cx, 1);
        vcx.simulate_resize(size(px(1440.), px(900.)));
        app.update_in(&mut vcx, |app, window, cx| {
            app.sidebar_collapsed = false;
            app.right_panel_visible = true;
            app.focus_active(window, cx);
            cx.notify();
        });
        settle(&mut vcx);
        let initial = app.read_with(&vcx, |app, _| {
            (app.sidebar_width.get(), app.right_panel_width.get())
        });
        vcx.update(|window, cx| {
            let left = window.find("sidebar-resize").bounds().center();
            window.drag(left, left + gpui_kit::point(px(70.), px(0.)), cx);
        });
        settle(&mut vcx);
        vcx.update(|window, cx| {
            let right = window.find("right-panel-resize").bounds().center();
            window.drag(right, right - gpui_kit::point(px(60.), px(0.)), cx);
        });
        settle(&mut vcx);
        app.read_with(&vcx, |app, cx| {
            assert!(app.sidebar_width.get() > initial.0 + 40.);
            assert!(app.right_panel_width.get() > initial.1 + 40.);
            assert_eq!(
                cx.global::<crate::core::config::Config>().sidebar_width,
                app.sidebar_width.get()
            );
            assert_eq!(
                cx.global::<crate::core::config::Config>().right_panel_width,
                app.right_panel_width.get()
            );
        });
        vcx.update(|window, cx| window.click("rail-workspace-head", cx));
        settle(&mut vcx);
        assert!(app.read_with(&vcx, |app, _| app.switcher.is_some()));
        vcx.simulate_keystrokes("escape");
        settle(&mut vcx);
        for width in [760., 1440.] {
            vcx.simulate_resize(size(px(width), px(900.)));
            vcx.update(|window, cx| {
                window.click("titlebar-left-panel", cx);
                window.click("titlebar-right-panel", cx);
            });
            settle(&mut vcx);
            vcx.update(|window, cx| window.click("titlebar-add", cx));
            settle(&mut vcx);
            vcx.simulate_keystrokes("q");
            settle(&mut vcx);
            assert_eq!(
                input(&mut streams[0]),
                None,
                "new-tab choices own keyboard input until dismissed"
            );
            vcx.simulate_keystrokes("escape");
            settle(&mut vcx);
            assert!(terminal_focused(&app, &mut vcx));
            vcx.simulate_keystrokes("n");
            settle(&mut vcx);
            assert_eq!(input(&mut streams[0]).as_deref(), Some(&b"n"[..]));
        }
    }
}
