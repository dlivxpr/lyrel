//! Master–detail presentation; Host operations remain owned by the settings session.
use super::*;
use gpui_kit::component::FocusTrapElement as _;
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::{TestSupportExt as _, rems};

impl LyrelApp {
    pub(crate) fn render_settings_skills(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(s) = self.active_settings() else {
            return div().into_any_element();
        };
        let state = &s.skills;
        let locked = state.install.target_locked() || state.maintenance.target_locked();
        let host_id = state.host;
        let machines = self.agent_hooks_machines(cx);
        let machine = machines
            .iter()
            .find(|m| m.host == host_id)
            .map(|m| m.label.clone())
            .unwrap_or_else(|| t(L10nKey::SkillsNoMachine).into());
        let entries = machines
            .into_iter()
            .map(|m| {
                MenuEntry::item(m.label, m.host == host_id, move |this, window, cx| {
                    this.select_skills_host(m.host, window, cx)
                })
            })
            .collect();
        let host_picker = if locked {
            div().text_sm().child(machine).into_any_element()
        } else {
            self.analysis_menu("skills-machine", machine, entries, cx)
        };
        let narrow = self.settings_viewport_w.get() < 1000. * ui_scale(cx);
        let selected = state.snapshot.as_ref().and_then(|snapshot| {
            snapshot
                .installations()
                .iter()
                .find(|skill| Some(skill.path()) == state.selected.as_deref())
        });
        let detail = selected
            .map(|skill| self.render_skill_detail(skill, narrow, cx))
            .unwrap_or_else(|| {
                v_flex()
                    .size_full()
                    .items_center()
                    .justify_center()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(t(L10nKey::SkillsSelectSkill))
                    .into_any_element()
            });
        v_flex()
            .size_full()
            .min_h_0()
            .min_w_0()
            .relative()
            .track_focus(&state.focus)
            .child(
                h_flex()
                    .flex_none()
                    .flex_wrap()
                    .gap_2()
                    .pb_3()
                    .child(host_picker)
                    .child(
                        div().flex_1().min_w_0().child(
                            Input::new(&state.project_input)
                                .id("skills-project-input")
                                .small()
                                .aria_label(t(L10nKey::SkillsProject))
                                .disabled(locked),
                        ),
                    )
                    .child(
                        Button::new("skills-apply-project")
                            .small()
                            .ghost()
                            .label(t(L10nKey::SkillsApplyProject))
                            .disabled(locked)
                            .on_click(cx.listener(|this, _, _, cx| this.apply_skills_project(cx))),
                    ),
            )
            .child(
                h_flex()
                    .items_stretch()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .when(!narrow || selected.is_none(), |pane| {
                        pane.child(self.render_skills_list(narrow, cx))
                    })
                    .when(!narrow || selected.is_some(), |pane| {
                        pane.child(
                            v_flex().flex_1().min_h_0().min_w_0().child(
                                crate::ui::scrollbar::with_vertical_scrollbar(
                                    "skills-detail-scrollbar",
                                    div()
                                        .id("skills-detail-scroll")
                                        .test_support()
                                        .size_full()
                                        .overflow_y_scroll()
                                        .track_scroll(&state.detail_scroll)
                                        .child(div().p_4().min_w_0().child(detail)),
                                    &state.detail_scroll,
                                ),
                            ),
                        )
                    }),
            )
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key != "escape" {
                    return;
                }
                if this.cancel_skills_maintenance(window, cx)
                    || this.cancel_skills_install(window, cx)
                {
                    cx.stop_propagation();
                    return;
                }
                if let Some(s) = this.active_settings_mut().filter(|s| s.menu.is_none()) {
                    if s.skills.browser.take().is_some() {
                        s.skills.browser_generation += 1;
                    } else if s.skills.selected.take().is_some() {
                        s.skills.browser_generation += 1;
                        window.focus(&s.skills.query.read(cx).focus_handle(cx), cx);
                    } else if !s.skills.query.read(cx).value().is_empty() {
                        s.skills
                            .query
                            .clone()
                            .update(cx, |input, cx| input.set_value("", window, cx));
                    } else {
                        return;
                    }
                    cx.stop_propagation();
                    cx.notify();
                }
            }))
            .into_any_element()
    }

    pub(in crate::ui::settings) fn skills_modal_open(&self) -> bool {
        self.active_settings().is_some_and(|s| {
            s.section == SettingsSection::Skills
                && (s.skills.install.open || s.skills.maintenance.is_open())
        })
    }

    pub(crate) fn render_skills_overlay(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let s = self.active_settings()?;
        if s.section != SettingsSection::Skills || s.search_active {
            return None;
        }
        if s.skills.install.open {
            Some(self.render_skills_install(cx))
        } else if s.skills.maintenance.is_open() {
            Some(self.render_skills_maintenance_dialog(cx))
        } else {
            None
        }
    }

    fn render_skills_list(&self, narrow: bool, cx: &mut Context<Self>) -> AnyElement {
        let state = &self.active_settings().unwrap().skills;
        let filter = state.filter;
        let filters = [
            None,
            Some(SourceKind::UserShared),
            Some(SourceKind::ProjectShared),
            Some(SourceKind::AgentPrivate),
        ]
        .into_iter()
        .map(|kind| {
            MenuEntry::item(
                t(kind.map(source_key).unwrap_or(L10nKey::SkillsAllSources)),
                kind == filter,
                move |this, _, cx| {
                    if let Some(s) = this.active_settings_mut() {
                        if s.skills.maintenance.target_locked() {
                            return;
                        }
                        s.skills.filter = kind;
                        s.skills.page = 0;
                        s.skills
                            .list_scroll
                            .set_offset(gpui_kit::point(px(0.), px(0.)));
                    }
                    cx.notify();
                },
            )
        })
        .collect::<Vec<_>>();
        let query = state.query.read(cx).value();
        let total = state
            .snapshot
            .as_ref()
            .map_or(0, |snapshot| snapshot.installations().len());
        let mut matches = state
            .snapshot
            .as_ref()
            .map(|snapshot| snapshot.filtered(&query, filter).collect::<Vec<_>>())
            .unwrap_or_default();
        matches.sort_unstable_by_key(|skill| {
            let scope = match skill_scope(skill) {
                SourceKind::UserShared => 0,
                SourceKind::ProjectShared => 1,
                SourceKind::AgentPrivate => 2,
            };
            (scope, skill.name(), skill.path())
        });
        let page = state.page.min(matches.len().saturating_sub(1) / 50);
        let mut rows = v_flex().gap_1().p_2().min_w_0();
        if state.loading {
            rows = rows.child(
                div()
                    .p_2()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(t(L10nKey::SkillsLoading)),
            );
        }
        if !machines_connected(cx, state.host)
            || state.error.is_some()
            || state
                .snapshot
                .as_ref()
                .is_some_and(|snapshot| !snapshot.problems().is_empty())
        {
            rows = rows.child(
                div()
                    .p_2()
                    .text_sm()
                    .text_color(cx.theme().warning)
                    .child(t(L10nKey::SkillsIncomplete)),
            );
        }
        if let Some(error) = &state.error {
            rows = rows.child(
                div()
                    .p_2()
                    .text_sm()
                    .text_color(cx.theme().danger)
                    .child(error.clone()),
            );
        }
        if let Some(snapshot) = &state.snapshot {
            for problem in snapshot.problems() {
                rows = rows.child(
                    div()
                        .p_2()
                        .text_sm()
                        .text_color(cx.theme().warning)
                        .child(problem.clone()),
                );
            }
        }
        if matches.is_empty() && !state.loading {
            rows = rows.child(
                div()
                    .p_2()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(t(L10nKey::SkillsEmpty)),
            );
        }
        let mut previous_scope = None;
        for skill in matches.iter().skip(page * 50).take(50) {
            let scope = skill_scope(skill);
            if previous_scope != Some(scope) {
                let count = matches.iter().filter(|skill| skill_scope(skill) == scope).count();
                rows = rows.child(
                    h_flex()
                        .gap_2()
                        .px_2()
                        .pt_2()
                        .pb_1()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(div().font_weight(FontWeight::MEDIUM).child(t(source_key(scope))))
                        .child(count.to_string()),
                );
                previous_scope = Some(scope);
            }
            let path = skill.path().to_path_buf();
            let selected = state.selected.as_deref() == Some(path.as_path());
            let scopes = skill
                .sources()
                .iter()
                .map(|source| {
                    let scope = t(source_key(source.kind()));
                    if source.kind() == SourceKind::ProjectShared {
                        let owner = skill
                            .path()
                            .parent()
                            .and_then(Path::parent)
                            .and_then(Path::parent)
                            .and_then(Path::file_name)
                            .map(|name| name.to_string_lossy())
                            .unwrap_or_default();
                        format!("{} · {scope} · {owner}", source.agent().display_name())
                    } else {
                        format!("{} · {scope}", source.agent().display_name())
                    }
                })
                .collect::<Vec<_>>()
                .join(" · ");
            rows = rows.child(
                Button::new(SharedString::from(row_id(&path)))
                    .ghost()
                    .w_full()
                    .h_auto()
                    .px_2()
                    .py_2()
                    .justify_start()
                    .accessibility_label(format!(
                        "{} · {} · {}",
                        skill.name(),
                        scopes,
                        path.display()
                    ))
                    .when(selected, |button| button.bg(cx.theme().secondary))
                    .child(
                        v_flex()
                            .min_w_0()
                            .flex_1()
                            .gap_1()
                            .child(
                                h_flex()
                                    .gap_2()
                                    .min_w_0()
                                    .child(skill_icon(cx))
                                    .child(
                                        div()
                                            .text_sm()
                                            .truncate()
                                            .font_weight(if selected {
                                                FontWeight::MEDIUM
                                            } else {
                                                FontWeight::NORMAL
                                            })
                                            .child(skill.name().to_string()),
                                    ),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .truncate()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(
                                        skill
                                            .description()
                                            .unwrap_or(t(L10nKey::SkillsUnknown))
                                            .to_string(),
                                    ),
                            ),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if let Some(s) = this.active_settings_mut() {
                            s.skills.selected = Some(path.clone());
                            s.skills.browser = None;
                            s.skills.browser_generation += 1;
                            s.skills
                                .detail_scroll
                                .set_offset(gpui_kit::point(px(0.), px(0.)));
                            window.focus(&s.skills.focus, cx);
                        }
                        cx.notify();
                    })),
            );
        }
        if matches.len() > 50 {
            let last = matches.len().saturating_sub(1) / 50;
            rows = rows.child(
                h_flex()
                    .justify_between()
                    .gap_2()
                    .child(
                        Button::new("skills-prev")
                            .small()
                            .ghost()
                            .label("←")
                            .accessibility_label(t(L10nKey::SkillsBack))
                            .disabled(page == 0)
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some(s) = this.active_settings_mut() {
                                    s.skills.page = s.skills.page.saturating_sub(1);
                                    s.skills
                                        .list_scroll
                                        .set_offset(gpui_kit::point(px(0.), px(0.)));
                                }
                                cx.notify();
                            })),
                    )
                    .child(format!("{} / {}", page + 1, last + 1))
                    .child(
                        Button::new("skills-next")
                            .small()
                            .ghost()
                            .label("→")
                            .accessibility_label(t(L10nKey::SkillsNextPage))
                            .disabled(page == last)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(s) = this.active_settings_mut() {
                                    s.skills.page = (page + 1).min(last);
                                    s.skills
                                        .list_scroll
                                        .set_offset(gpui_kit::point(px(0.), px(0.)));
                                }
                                cx.notify();
                            })),
                    ),
            );
        }
        v_flex()
            .h_full()
            .min_h_0()
            .min_w_0()
            .when(narrow, |pane| pane.flex_1())
            .when(!narrow, |pane| {
                pane.w(rems(236. / 16.))
                    .flex_none()
            })
            .child(
                v_flex()
                    .flex_none()
                    .p_2()
                    .gap_2()
                    .child(
                        Input::new(&state.query)
                            .id("skills-query")
                            .small()
                            .w_full()
                            .aria_label(t(L10nKey::SkillsSearch))
                            .disabled(state.maintenance.target_locked()),
                    )
                    .child(
                        Button::new("skills-source")
                            .small()
                            .w_full()
                            .child(
                                h_flex()
                                    .w_full()
                                    .min_w_0()
                                    .justify_between()
                                    .gap_2()
                                    .child(
                                        div().truncate().child(t(
                                            filter.map(source_key).unwrap_or(L10nKey::SkillsAllSources),
                                        )),
                                    )
                                    .child(
                                        Icon::new(gpui_kit::component::IconName::ChevronDown).small(),
                                    ),
                            )
                            .accessibility_label(format!(
                                "{} · {}",
                                t(L10nKey::SkillsSource),
                                t(filter.map(source_key).unwrap_or(L10nKey::SkillsAllSources))
                            ))
                            .disabled(state.maintenance.target_locked())
                            .dropdown_menu_with_anchor(gpui_kit::Anchor::TopLeft, {
                                let app = cx.entity().downgrade();
                                move |mut menu, _, _| {
                                    for entry in &filters {
                                        if let MenuEntry::Item { label, checked, on_pick, .. } = entry {
                                            let app = app.clone();
                                            let on_pick = on_pick.clone();
                                            menu = menu.item(
                                                PopupMenuItem::new(label.clone())
                                                    .checked(*checked)
                                                    .on_click(move |_, window, cx| {
                                                        let _ = app.update(cx, |app, cx| on_pick(app, window, cx));
                                                    }),
                                            );
                                        }
                                    }
                                    menu
                                }
                            }),
                    ),
            )
            .child(crate::ui::scrollbar::with_vertical_scrollbar(
                "skills-list-scrollbar",
                div()
                    .id("skills-list-scroll")
                    .test_support()
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&state.list_scroll)
                    .child(rows),
                &state.list_scroll,
            ))
            .child(
                h_flex()
                    .flex_none()
                    .p_2()
                    .gap_1()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(t_fmt(
                                L10nKey::SkillsCount,
                                &[
                                    ("shown", &matches.len().to_string()),
                                    ("total", &total.to_string()),
                                ],
                            )),
                    )
                    .child(self.render_skills_install_trigger(cx))
                    .child(
                        Button::new("skills-refresh")
                            .small()
                            .ghost()
                            .icon(gpui_kit::component::IconName::RefreshCw)
                            .accessibility_label(t(L10nKey::SkillsRefresh))
                            .tooltip(t(L10nKey::SkillsRefresh))
                            .disabled(
                                state.loading
                                    || state.install.target_locked()
                                    || state.maintenance.target_locked(),
                            )
                            .on_click(cx.listener(|this, _, _, cx| this.refresh_skills(cx))),
                    ),
            )
            .into_any_element()
    }

    pub(super) fn skills_restore_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(s) = self.active_settings_mut() {
            let focus = s
                .skills
                .return_focus
                .take()
                .unwrap_or_else(|| s.skills.focus.clone());
            window.focus(&focus, cx);
        }
    }

    pub(super) fn render_skills_modal(
        &self,
        id: &'static str,
        title: SharedString,
        body: AnyElement,
        footer: AnyElement,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let state = &self.active_settings().unwrap().skills;
        div()
            .absolute()
            .inset_0()
            .occlude()
            .bg(crate::ui::presets::scrim_fill(cx))
            .flex()
            .items_center()
            .justify_center()
            .p_3()
            .child(
                v_flex()
                    .id(id)
                    .role(gpui_kit::Role::Dialog)
                    .aria_label(title.clone())
                    .w_full()
                    .max_w(rems(38.))
                    .h_full()
                    .max_h(rems(42.))
                    .min_h_0()
                    .min_w_0()
                    .map(|panel| crate::ui::theme::floating_surface(panel, cx))
                    .rounded_lg()
                    .overflow_hidden()
                    .child(crate::ui::dialog::header(title, cx))
                    .child(crate::ui::scrollbar::with_vertical_scrollbar(
                        "skills-modal-scrollbar",
                        div()
                            .id("skills-modal-scroll")
                            .test_support()
                            .size_full()
                            .overflow_y_scroll()
                            .track_scroll(&state.modal_scroll)
                            .child(v_flex().p_4().gap_3().min_w_0().child(body)),
                        &state.modal_scroll,
                    ))
                    .child(crate::ui::dialog::footer(cx).child(footer))
                    .focus_trap("skills-modal-focus-trap", &state.modal_focus),
            )
            .into_any_element()
    }
    pub(super) fn skills_modal_target(&self, cx: &mut App) -> AnyElement {
        let state = &self.active_settings().unwrap().skills;
        let machine = self
            .agent_hooks_machines(cx)
            .into_iter()
            .find(|machine| machine.host == state.host)
            .map(|machine| machine.label)
            .unwrap_or_else(|| t(L10nKey::SkillsNoMachine).into());
        v_flex()
            .gap_1()
            .text_sm()
            .child(skill_metadata_row(
                L10nKey::SkillsMachine,
                format!("{machine} · {:?}", state.host),
                cx,
            ))
            .child(skill_metadata_row(
                L10nKey::SkillsProject,
                state
                    .project
                    .as_ref()
                    .map(|project| project.display().to_string())
                    .unwrap_or_else(|| t(L10nKey::SkillsUserShared).to_string()),
                cx,
            ))
            .into_any_element()
    }
}
