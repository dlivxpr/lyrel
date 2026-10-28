//! Window composition: one mount per capability, with shared chrome and overlay ordering.
use super::*;

impl LyrelApp {
    fn render_remote_workspace_strip(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<gpui_kit::AnyElement> {
        if self.tabs.is_empty() {
            return None;
        }
        let status = self.remote_status(cx)?;
        let machine = self.remote_machine_label(cx);
        let message = status.strip_message(&machine)?;
        // While an install runs, the strip is that install's progress: the
        // refusal it is answering is no longer the news, and the button that
        // started it would only start a second one.
        let installing = self.remote_strip_progress(cx);
        let action = installing
            .is_none()
            .then(|| self.remote_strip_action(&status, cx))
            .flatten();
        let theme = cx.theme();
        let message = match installing {
            Some(phase) => format!(
                "{machine} — {}",
                crate::ui::remote_workspace::install_phase_caption(phase)
            ),
            None => message,
        };
        let bar = crate::ui::remote_workspace::status_card(cx)
            .occlude()
            .shadow_md()
            .child(
                crate::ui::remote_workspace::status_row()
                    .child(gpui_kit::component::Icon::new(
                        gpui_kit::component::IconName::Globe,
                    ))
                    .child(
                        crate::ui::remote_workspace::status_message(message)
                            .font_weight(gpui_kit::FontWeight::MEDIUM)
                            .text_color(theme.foreground),
                    )
                    .when_some(action, |this, (label, action)| {
                        use gpui_kit::component::Sizable as _;
                        use gpui_kit::component::button::ButtonVariants as _;
                        this.child(
                            gpui_kit::component::button::Button::new("remote-status-action")
                                .flex_shrink_0()
                                .label(label)
                                .primary()
                                .small()
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.run_strip_action(action.clone(), window, cx);
                                })),
                        )
                    }),
            )
            .when_some(installing, |this, phase| {
                this.child(crate::ui::remote_workspace::install_progress_bar(phase, cx))
            });
        Some(
            div()
                .absolute()
                .top_2()
                .left_0()
                .right_0()
                .flex()
                .justify_center()
                .child(bar)
                .into_any_element(),
        )
    }

    fn render_remote_input_notice(&self, cx: &mut Context<Self>) -> Option<gpui_kit::AnyElement> {
        if self.tabs.is_empty() {
            return None;
        }
        let notice = self.remote_status(cx)?.input_notice()?;
        // The pill only. `body_area` anchors it, together with whatever else
        // is floating down there — see `ui::notice`.
        Some(
            crate::ui::notice::pill(cx.theme().warning, cx)
                .child(notice)
                .into_any_element(),
        )
    }
}

impl LyrelApp {
    /// 唯一工作区组合入口；终端、文档、面板与内容浮层在此各挂载一次。
    /// 窗口级提示仍由 render 挂载，后续全局状态栏应与本区域并列。
    fn render_work_area(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        self.sync_parked_deck(window, cx);
        let pointer = window.mouse_position();
        let show_park_target = self.dragged_parking_pane(cx).is_some()
            && self.pane_area.get().is_some_and(|area| {
                area.contains(&pointer)
                    && pointer.y < area.bottom()
                    && pointer.y
                        >= area.bottom() - (window.rem_size() * (170. / 16.)).min(area.size.height)
            });
        self.parked_deck.update(cx, |deck, cx| {
            deck.set_drop_target(show_park_target, cx);
        });
        let tab_landing = self.tab_landing(window, cx);
        let sidebar = self
            .sidebar_open(cx)
            .then(|| self.tab_sidebar(window, cx).into_any_element());
        let ssh_status = self
            .tabs
            .get(self.tabs.active_index())
            .and_then(|t| t.pane.focused_or_first(window, cx))
            .and_then(|leaf| self.render_ssh_status_strip(&leaf, cx));
        let body = match self.tabs.get(self.tabs.active_index()) {
            None => self.render_home(cx).into_any_element(),
            Some(active_tab) => {
                let maximized = self.maximized.as_ref().filter(|leaf| {
                    active_tab.pane.leaves().iter().any(|slot| {
                        slot.entity_id() == leaf.entity_id() && !active_tab.is_parked_slot(slot, cx)
                    })
                });
                match maximized {
                    Some(leaf) => Pane::leaf(PaneSlot::Ready(leaf.clone())).render(
                        &crate::ui::pane::PaneChrome {
                            dim_inactive: false,
                            app: cx.entity().downgrade(),
                            lifted: None,
                            drag: self.pane_drag.clone(),
                        },
                        window,
                        cx,
                    ),
                    None => {
                        let several = active_tab.pane.leaves().len() > 1;
                        let chrome = crate::ui::pane::PaneChrome {
                            dim_inactive: several && cx.global::<Config>().dim_inactive_panes,
                            app: cx.entity().downgrade(),
                            lifted: crate::ui::pane_drag::lifted(&self.pane_drag),
                            drag: self.pane_drag.clone(),
                        };
                        let panes = active_tab.render_panes(&chrome, window, cx);
                        let all_parked = active_tab.pip_slot(cx).is_none()
                            && active_tab.focus_target(cx).is_none()
                            && active_tab.scoped_presentation().map_or_else(
                                || !active_tab.presentation().parked().is_empty(),
                                |presentation| !presentation.parked().is_empty(),
                            );
                        div()
                            .size_full()
                            .relative()
                            .child(panes)
                            .when(all_parked, |body| {
                                body.child(
                                    div()
                                        .id("parked-empty")
                                        .test_support()
                                        .absolute()
                                        .inset_0()
                                        .flex()
                                        .flex_col()
                                        .items_center()
                                        .justify_center()
                                        .gap_3()
                                        .text_sm()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(t(L10nKey::ParkedEmpty))
                                        .child(
                                            gpui_kit::component::button::Button::new(
                                                "parked-new-pane",
                                            )
                                            .label(t(L10nKey::AppMenuSplitRight))
                                            .on_click(
                                                |_, window, cx| {
                                                    window.dispatch_action(Box::new(SplitRight), cx)
                                                },
                                            ),
                                        ),
                                )
                            })
                            .into_any_element()
                    }
                }
            }
        };

        let body_area = div()
            .flex_1()
            .relative()
            .overflow_hidden()
            .child(
                gpui_kit::canvas(
                    {
                        let area = self.pane_area.clone();
                        move |bounds, _window, _cx| area.set(Some(bounds))
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .inset_0(),
            )
            .child(body)
            .when_some(self.pane_landing(window, cx), |this, el| this.child(el))
            .when_some(tab_landing, |this, el| this.child(el))
            .when_some(self.render_remote_workspace_strip(cx), |this, el| {
                this.child(el)
            })
            // Both of these used to anchor themselves at `bottom_4` and centre
            // themselves, as siblings here — so a remote workspace whose ssh
            // link had also dropped drew them one on top of the other. One
            // anchor now, and it stacks. The ssh strip goes last because it is
            // the one carrying buttons.
            .when_some(
                crate::ui::notice::anchor(
                    [self.render_remote_input_notice(cx), ssh_status]
                        .into_iter()
                        .flatten()
                        .collect(),
                ),
                |this, el| this.child(el),
            );

        // One decision for the whole document surface. Docked, exactly one of
        // the two surfaces is drawn — a column has one child, and two `flex_1`
        // siblings would split it and fight — so `overlay_top` stops ordering a
        // pair and starts choosing between them. Filling, nothing changes: both
        // are rendered, ordered by `overlay_top`, and the front one wins on
        // paint order as it always has.
        let document_dock_px = self.document_dock_px(window, cx);
        let document_chrome = crate::ui::document_column::DocumentChrome::Dock;
        let (overlays, document_column) = match document_dock_px {
            Some(w) => (
                Vec::new(),
                self.render_document_column(w, document_chrome, window, cx),
            ),
            None => {
                let diff_overlay = self.render_diff_overlay(
                    crate::ui::document_column::DocumentChrome::Fill,
                    window,
                    cx,
                );
                let code_overlay = self.render_code_overlay(
                    crate::ui::document_column::DocumentChrome::Fill,
                    window,
                    cx,
                );
                let mut pair = vec![
                    (OverlayTop::Diff, diff_overlay),
                    (OverlayTop::Code, code_overlay),
                ];
                if self
                    .tabs
                    .get(self.tabs.active_index())
                    .is_some_and(Tab::is_diff_on_top)
                {
                    pair.reverse();
                }
                (
                    pair.into_iter()
                        .filter_map(|(_, el)| el)
                        .collect::<Vec<gpui_kit::AnyElement>>(),
                    None,
                )
            }
        };

        let right_panel = self.render_right_panel(window, cx);
        let can_split = self
            .tabs
            .get(self.tabs.active_index())
            .and_then(|tab| tab.detail_pane(window, cx))
            .is_some();
        let top_chrome = WindowChrome::new(
            self.left_panel_open(cx),
            self.right_panel_open(cx),
            can_split,
            self.new_tab_button("titlebar-add", cx),
            cx,
            |command| {
                cx.listener(move |this, _, window, cx| match command {
                    ChromeCommand::ToggleLeftPanel => {
                        this.toggle_left_panel(cx);
                        this.focus_previous_content(window, cx);
                    }
                    ChromeCommand::ToggleSearch => match this.search.is_open() {
                        true => this.close_search(window, cx),
                        false => this.open_search(SearchTab::All, "", window, cx),
                    },
                    ChromeCommand::Split(axis) => this.split(axis, window, cx),
                    ChromeCommand::ToggleRightPanel => {
                        this.toggle_right_panel(cx);
                        this.focus_previous_content(window, cx);
                    }
                })
            },
        )
        .render(window, cx);
        let terminal_column = div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .relative()
            .child(body_area)
            .children(overlays)
            .child(div().absolute().inset_0().child(self.parked_deck.clone()));
        let panel_row = div()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .flex()
            .flex_row()
            .child(terminal_column)
            .when_some(document_column, |this, column| this.child(column))
            .when_some(right_panel, |this, panel| this.child(panel));
        div()
            .flex_1()
            .min_h_0()
            .w_full()
            .flex()
            .flex_col()
            // Keyboard capture follows focus ancestry, unlike the mouse capture
            // canvas: it must enclose both terminal and handle focus targets.
            .capture_key_down(
                cx.listener(|this, event: &gpui_kit::KeyDownEvent, window, cx| {
                    if event.keystroke.key == "escape"
                        && (crate::ui::pane_drag::lifted(&this.pane_drag).is_some()
                            || this
                                .tabs
                                .get(this.tabs.active_index())
                                .is_some_and(|tab| tab.pip_interaction.has_gesture()))
                    {
                        this.cancel_pane_drag(window, cx);
                        cx.stop_propagation();
                    }
                }),
            )
            .child(top_chrome)
            .child(self.pane_drag_capture(cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .flex()
                    .flex_row()
                    .when_some(sidebar, |this, sidebar| this.child(sidebar))
                    .child(panel_row),
            )
            .into_any_element()
    }
}

impl LyrelApp {
    pub(super) fn render_window(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(test)]
        render_probe::record();
        let prof = crate::ui::perf::enabled().then(std::time::Instant::now);
        // Every window's root is this view, so setting the rem here is what
        // makes `ui_font_size` reach the whole interface — the rem ladder
        // (`text_sm`, `text_xs`, `rems(..)`) resolves against it, and the
        // terminal grid, sized in absolute px from `font_size`, does not move.
        window.set_rem_size(px(cx.global::<Config>().ui_font_size));
        self.claim_pending_tab(window, cx);
        self.claim_notification_source(window, cx);
        // Before anything asks where a tab is drawn: a tab that walked into a
        // pinned folder since the last frame is filed there on this one.
        self.settle_sidebar_groups(cx);
        self.tabs.touch_active();
        self.declare_displayed_panes(cx);
        self.scm_sync_watchers(window, cx);
        self.editor_sync(window, cx);
        // Keeps looking for new listening ports on the pane in front, panel
        // open or not — a port that appears while the panel is shut is exactly
        // the one worth forwarding unasked.
        self.sync_port_watch(window, cx);
        self.sync_status_metrics(window, cx);
        if !window.is_window_active()
            || (crate::ui::pane_drag::lifted(&self.pane_drag).is_some()
                && (!cx.has_active_drag() || self.dragged_parking_pane(cx).is_none()))
        {
            if crate::ui::pane_drag::lifted(&self.pane_drag).is_some()
                || self
                    .tabs
                    .get(self.tabs.active_index())
                    .is_some_and(|tab| tab.pip_interaction.has_gesture())
            {
                self.cancel_pane_drag(window, cx);
            }
        }
        if cx.has_active_drag() {
            crate::ui::reorder::clear_pending(&self.reorder);
            crate::ui::pane_drag::clear_landing(&self.pane_drag);
        } else {
            // Taken first either way: this is what ends the drag, and the merge
            // below must not find the tab it just moved still in the air.
            let landed = crate::ui::reorder::take_landed(&self.reorder);
            if let Some((tab, zone)) = self.tab_merge.take() {
                self.merge_tab(tab, zone, window, cx);
            } else if let Some((tab, target)) = landed.regroup {
                // A drop into another group outranks the reordering the drag
                // did on its way out of the one it came from. The pointer
                // left that group; the shuffle it caused before leaving is
                // not what was being asked for.
                self.regroup_tab(tab, target, cx);
            } else if let Some(key) = landed.pin {
                // The same for a header let go above the divider: it was
                // carried there to be pinned, not to be reordered on the way.
                self.pin_auto_group(key, cx);
            } else if let Some(order) = landed.order {
                match landed.surface {
                    Some(crate::ui::reorder::Surface::PinnedGroups) => {
                        self.apply_pinned_order(&order, cx)
                    }
                    _ => self.apply_tab_order(&order, cx),
                }
            }
        }
        // Windows has no closed-hand cursor and gpui answers `ClosedHand` with
        // the plain arrow there, which would drop the grip's pointing hand the
        // instant the drag it advertised began.
        let held = if cfg!(target_os = "windows") {
            gpui_kit::CursorStyle::PointingHand
        } else {
            gpui_kit::CursorStyle::ClosedHand
        };
        if (self.reorder.borrow().is_some() || self.pane_drag.borrow().is_some())
            && cx.active_drag_cursor_style() != Some(held)
        {
            cx.set_active_drag_cursor_style(held, window);
        }
        let detach_caret = self.detach_caret(window, cx);
        let main_layout = self.render_work_area(window, cx);

        let window_bg = crate::ui::theme::workspace_background(cx);
        let bg_image = window_background_image_layer(cx);
        let settings_bg = crate::ui::theme::overlay_background(cx);

        let settings_overlay =
            (self.settings.is_some() && self.settings_window.is_none()).then(|| {
                div()
                    .absolute()
                    .inset_0()
                    .occlude()
                    // Opaque on purpose: the settings panel must never let the
                    // workspace translucency (window opacity / backdrop material)
                    // show through, even at window edges during a resize. The
                    // preset's gradient fill is preserved, just with alpha 1;
                    // `render_settings` repaints the theme image over it.
                    .bg(settings_bg)
                    .child(self.settings.as_ref().unwrap().clone())
            });

        let root =
            div()
                .id("lyrel-root")
                .size_full()
                .flex()
                .flex_col()
                .bg(window_bg)
                .text_color(cx.theme().foreground)
                .on_modifiers_changed(cx.listener(Self::on_modifiers_changed));
        let root = Self::bind_window_actions(root, cx);
        let root = root
                .children(bg_image)
                .child(
                    div()
                        .id("workbench-content")
                        .test_support()
                        .flex()
                        .flex_col()
                        .relative()
                        .flex_1()
                        .min_h_0()
                        .w_full()
                        .overflow_hidden()
                        .child(main_layout)
                        .when_some(detach_caret, |this, caret| this.child(caret))
                        .when_some(settings_overlay, |this, overlay| this.child(overlay))
                        .when_some(self.render_worktree_prompt_overlay(cx), |this, el| {
                            this.child(el)
                        })
                        .when_some(self.render_ssh_prompt_overlay(window, cx), |this, el| {
                            this.child(el)
                        })
                        .children(self.render_switcher(window, cx))
                        .when_some(self.search.view(), |this, search| this.child(search)),
                )
                .child(self.render_status_bar(window, cx));
        let root = Self::with_editor_text_commands(root, cx);

        if let Some(start) = prof {
            crate::ui::perf::record("window", start.elapsed());
        }
        root
    }
}

// The output gate: a pane repaints on PTY output only while it is on screen.
// `LyrelApp::render` declares the active tab's panes displayed each frame and
// everything else hidden; a pane nobody has declared — or whose id nobody
// registered — must err toward displayed, because the failure direction that
// matters is a visible pane that stops repainting.
#[cfg(test)]
mod displayed_gpui_tests {
    use gpui_kit::TestAppContext;

    use crate::terminal::lifecycle::{declare_displayed, displayed_for_test};
    use crate::terminal::view::quiet_test_pane;
    use crate::ui::app::test_window::harness_with_tabs;

    fn pane_id(app: &super::LyrelApp, tab: usize) -> gpui_kit::EntityId {
        app.tabs[tab]
            .pane
            .first_leaf()
            .expect("tab has a pane")
            .entity_id()
    }

    #[gpui_kit::test]
    fn the_output_gate_follows_the_active_tab(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 2);

        app.update_in(&mut vcx, |app, window, cx| {
            let (front, back) = (pane_id(app, 0), pane_id(app, 1));

            app.declare_displayed_panes(cx);
            assert_eq!(
                displayed_for_test(cx, front),
                Some(true),
                "the active tab's pane repaints on output"
            );
            assert_eq!(
                displayed_for_test(cx, back),
                Some(false),
                "a background tab's pane stays out of the frame loop"
            );

            app.activate(1, window, cx);
            app.declare_displayed_panes(cx);
            assert_eq!(displayed_for_test(cx, front), Some(false));
            assert_eq!(
                displayed_for_test(cx, back),
                Some(true),
                "switching tabs hands the frame loop to the new tab"
            );
        });
    }

    #[gpui_kit::test]
    fn a_pane_nobody_declared_counts_as_displayed(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);

        let _orphan = app.update_in(&mut vcx, |app, window, cx| {
            // A pane that exists but sits in no tab — the shape of every
            // "path that never declares": it registers as displayed and a
            // declaration pass over the app's own tabs leaves it alone.
            let (view, stream) = quiet_test_pane(99, window, cx);
            let orphan = view.entity_id();
            assert_eq!(
                displayed_for_test(cx, orphan),
                Some(true),
                "a fresh pane defaults to displayed"
            );

            app.declare_displayed_panes(cx);
            assert_eq!(
                displayed_for_test(cx, orphan),
                Some(true),
                "declaring only speaks about panes the app holds"
            );

            // An id nobody registered is declared into the void, not
            // inserted: the registry only ever holds live panes' flags.
            let ghost = gpui_kit::EntityId::from(u64::MAX);
            declare_displayed(cx, [(ghost, false)]);
            assert_eq!(displayed_for_test(cx, ghost), None);

            (view, stream)
        });
    }

    #[gpui_kit::test]
    fn a_released_pane_leaves_the_registry(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 2);

        let back = app.update_in(&mut vcx, |app, _window, _cx| {
            let back = pane_id(app, 1);
            app.tabs.remove(1);
            back
        });
        vcx.background_executor.run_until_parked();

        app.update_in(&mut vcx, |_, _, cx| {
            assert_eq!(
                displayed_for_test(cx, back),
                None,
                "a closed pane's flag does not outlive it"
            );
        });
    }
}
