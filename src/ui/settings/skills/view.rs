//! Master–detail presentation; Host operations remain owned by the settings session.
use super::*;
use gpui_kit::component::FocusTrapElement as _;
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::text::TextView;
use gpui_kit::rems;
use std::time::UNIX_EPOCH;

impl LyrelApp {
    pub(crate) fn render_settings_skills(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(s) = self.active_settings() else {
            return div().into_any_element();
        };
        let state = &s.skills;
        let locked = state.install.target_locked() || state.maintenance.target_locked();
        let host_picker = self.render_skills_host_picker(locked, cx);
        let scale = ui_scale(cx);
        let viewport = self.settings_viewport_w.get();
        // The data-page inset is px_5 on both sides, not the prose-page inset.
        // Keep both panes while a 17rem list and a 20rem detail still fit.
        let content_width = viewport - super::super::nav_width(viewport, scale) - 40. * scale;
        let narrow = content_width < (17. + 20.) * 16. * scale;
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
                div()
                    .flex_none()
                    .pb_2()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(t(L10nKey::SkillsBrowserDescription)),
            )
            .child(
                h_flex()
                    .flex_none()
                    .flex_wrap()
                    .gap_2()
                    .pb_2()
                    .child(
                        h_flex()
                            .flex_none()
                            .gap_2()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(t(L10nKey::SkillsMachine)),
                            )
                            .child(host_picker),
                    )
                    .child(
                        h_flex()
                            .flex_1()
                            .min_w(rems(16.))
                            .gap_2()
                            .child(
                                div()
                                    .flex_none()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(t(L10nKey::SkillsProject)),
                            )
                            .child(
                                div().flex_1().min_w_0().child(
                                    Input::new(&state.project_input)
                                        .id("skills-project-input")
                                        .small()
                                        .aria_label(t(L10nKey::SkillsProject))
                                        .disabled(locked)
                                        .w_full(),
                                ),
                            )
                            .child(
                                Button::new("skills-apply-project")
                                    .small()
                                    .ghost()
                                    .label(t(L10nKey::SkillsApplyProject))
                                    .disabled(locked)
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.apply_skills_project(cx)),
                                    ),
                            ),
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

    fn render_skills_host_picker(&self, locked: bool, cx: &mut Context<Self>) -> AnyElement {
        let state = &self.active_settings().unwrap().skills;
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
            .collect::<Vec<_>>();
        let host_picker = Button::new("skills-machine")
            .small()
            .ghost()
            .w(rems(11.))
            .min_w_0()
            .disabled(locked)
            .accessibility_label(format!("{} · {machine}", t(L10nKey::SkillsMachine)))
            .tooltip(machine.clone())
            .child(
                h_flex()
                    .w_full()
                    .min_w_0()
                    .gap_2()
                    .child(div().flex_1().truncate().child(machine))
                    .child(Icon::new(gpui_kit::component::IconName::ChevronDown).small()),
            )
            .dropdown_menu_with_anchor(gpui_kit::Anchor::TopLeft, {
                let app = cx.entity().downgrade();
                move |mut menu, _, _| {
                    for entry in &entries {
                        if let MenuEntry::Item {
                            label,
                            checked,
                            on_pick,
                            ..
                        } = entry
                        {
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
            })
            .into_any_element();
        host_picker
    }
    fn render_skill_detail(
        &self,
        skill: &SkillInstallation,
        narrow: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let host_id = skill.host();
        let path = skill.path().to_path_buf();
        let markdown_path = skill.document().to_path_buf();
        let browse_path = if skill.document() == skill.path() {
            skill
                .document()
                .parent()
                .unwrap_or(skill.path())
                .to_path_buf()
        } else {
            path.clone()
        };
        let copy_path = path.to_string_lossy().into_owned();
        let mut content = Vec::new();
        if narrow {
            content.push(
                Button::new("skills-back")
                    .small()
                    .ghost()
                    .self_start()
                    .disabled(
                        self.active_settings()
                            .is_some_and(|s| s.skills.maintenance.target_locked()),
                    )
                    .label(t(L10nKey::SkillsBack))
                    .on_click(cx.listener(|this, _, window, cx| {
                        if let Some(s) = this.active_settings_mut() {
                            s.skills.selected = None;
                            s.skills.browser = None;
                            s.skills.browser_generation += 1;
                            window.focus(&s.skills.query.read(cx).focus_handle(cx), cx);
                        }
                        cx.notify();
                    }))
                    .into_any_element(),
            );
        }
        let scope = skill_scope(skill);
        let discovery = skill.sources().iter().map(|source| source.state()).next();
        let uniform_state = discovery.filter(|state| {
            skill
                .sources()
                .iter()
                .all(|source| source.state() == *state)
        });
        content.push(
            h_flex()
                .gap_3()
                .min_w_0()
                .child(
                    div()
                        .size_8()
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_md()
                        .bg(cx.theme().muted)
                        .child(skill_icon(cx)),
                )
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .gap_1()
                        .child(
                            div()
                                .id("skills-detail-name")
                                .test_support()
                                .truncate()
                                .text_base()
                                .font_weight(FontWeight::MEDIUM)
                                .child(skill.name().to_string()),
                        )
                        .child(
                            div()
                                .truncate()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(format!(
                                    "{} · {}",
                                    t(source_key(scope)),
                                    t(uniform_state
                                        .map(state_key)
                                        .unwrap_or(L10nKey::SkillsMixedState))
                                )),
                        ),
                )
                .into_any_element(),
        );
        content.push(
            div()
                .text_sm()
                .child(
                    skill
                        .description()
                        .unwrap_or(t(L10nKey::SkillsNoDescription))
                        .to_string(),
                )
                .into_any_element(),
        );
        let companions = skill.companions();
        let mut metadata = v_flex()
            .id("skills-detail-metadata")
            .test_support()
            .min_w_0();
        let mut agents = std::collections::HashSet::new();
        let invocation = skill
            .sources()
            .iter()
            .filter(|source| agents.insert(source.agent()))
            .map(|source| {
                use crate::core::agent_hooks::HookAgent;
                let name = skill.name();
                let command = match source.agent() {
                    HookAgent::OhMyPi | HookAgent::Pi => format!("/skill:{name}"),
                    HookAgent::Kimi => format!("/skill:{name} · /{name}"),
                    HookAgent::OpenCode => format!("@{name}"),
                    HookAgent::Hermes => format!("/{name} · /skill {name}"),
                };
                format!("{} · {command}", source.agent().display_name())
            })
            .collect::<Vec<_>>()
            .join("; ");
        metadata = metadata.child(skill_detail_row(
            "skills-invoke".into(),
            t(L10nKey::SkillsInvoke).into(),
            invocation,
            false,
            cx,
        ));
        if let Some(hint) = skill.invocation() {
            metadata = metadata.child(skill_detail_row(
                "skills-argument-hint".into(),
                t(L10nKey::SkillsInvocation).into(),
                hint.to_string(),
                false,
                cx,
            ));
        }
        metadata = metadata.child(skill_detail_row(
            "skills-location".into(),
            t(L10nKey::SkillsLocation).into(),
            path.to_string_lossy().into_owned(),
            true,
            cx,
        ));
        let mut locations = std::collections::HashSet::new();
        for source in skill.sources() {
            if !locations.insert(source.entry()) {
                continue;
            }
            let label = skill
                .sources()
                .iter()
                .filter(|other| other.entry() == source.entry())
                .map(|source| {
                    format!(
                        "{} · {} · {}",
                        source.agent().display_name(),
                        t(source_key(source.kind())),
                        t(state_key(source.state())),
                    )
                })
                .collect::<Vec<_>>()
                .join("; ");
            metadata = metadata.child(skill_detail_row(
                format!("skill-source-{}", row_id(source.entry())).into(),
                label.into(),
                source.entry().to_string_lossy().into_owned(),
                true,
                cx,
            ));
        }
        for (id, key, value) in [
            (
                "skills-contents",
                L10nKey::SkillsContents,
                t_fmt(
                    L10nKey::SkillsCompanions,
                    &[
                        ("files", &companions.files().to_string()),
                        ("bytes", &skill_size(companions.bytes())),
                        (
                            "limited",
                            if companions.is_incomplete() {
                                t(L10nKey::SkillsFileLimit)
                            } else {
                                ""
                            },
                        ),
                    ],
                ),
            ),
            (
                "skills-updated",
                L10nKey::SkillsUpdated,
                skill
                    .updated()
                    .map(|time| skill_timestamp(time.secs))
                    .unwrap_or_else(|| t(L10nKey::SkillsUnknown).to_string()),
            ),
            (
                "skills-origin",
                L10nKey::SkillsOrigin,
                skill
                    .origin()
                    .unwrap_or(t(L10nKey::SkillsNoSource))
                    .to_string(),
            ),
        ] {
            metadata = metadata.child(skill_detail_row(id.into(), t(key).into(), value, false, cx));
        }
        if let Some(record) = skill.source_record() {
            metadata = metadata
                .child(skill_detail_row(
                    "skills-source-reference".into(),
                    t(L10nKey::SkillsSourceReference).into(),
                    format!(
                        "{} · {} · {}",
                        record.source_type(),
                        record.reference().unwrap_or(t(L10nKey::SkillsUnknown)),
                        record.skill_path().unwrap_or(t(L10nKey::SkillsUnknown)),
                    ),
                    false,
                    cx,
                ))
                .child(skill_detail_row(
                    "skills-source-record".into(),
                    t(L10nKey::SkillsSourceRecord).into(),
                    record.lock_path().to_string_lossy().into_owned(),
                    true,
                    cx,
                ));
        }
        content.push(metadata.into_any_element());
        for problem in skill.problems() {
            content.push(
                div()
                    .text_color(cx.theme().warning)
                    .text_sm()
                    .child(problem.clone())
                    .into_any_element(),
            );
        }
        content.push(
            h_flex()
                .id("skills-detail-actions")
                .test_support()
                .flex_wrap()
                .gap_2()
                .child(
                    Button::new("skills-open-file")
                        .small()
                        .label(t(L10nKey::SkillsOpenFile))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.skills_open_file(host_id, markdown_path.clone(), window, cx)
                        })),
                )
                .child(
                    Button::new("skills-browse-files")
                        .small()
                        .label(t(L10nKey::SkillsBrowseFiles))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if host_id.is_local() {
                                cx.reveal_path(&browse_path);
                            } else {
                                this.skills_browse(host_id, browse_path.clone(), cx);
                            }
                        })),
                )
                .child(
                    Button::new("skills-copy-path")
                        .small()
                        .label(t(L10nKey::SkillsCopyPath))
                        .on_click(move |_, _, cx| {
                            cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(
                                copy_path.clone(),
                            ))
                        }),
                )
                .child(self.render_skill_maintenance(skill, cx))
                .into_any_element(),
        );
        if let Some((directory, entries)) = self
            .active_settings()
            .and_then(|s| s.skills.browser.as_ref())
        {
            content.push(
                div()
                    .child(directory.to_string_lossy().into_owned())
                    .into_any_element(),
            );
            for entry in entries {
                let entry_path =
                    HostRegistry::get(cx, host_id).map(|h| h.join(directory, &entry.name));
                if let Some(path) = entry_path {
                    let is_dir = entry.is_dir;
                    content.push(
                        Button::new(SharedString::from(format!("skill-file-{}", row_id(&path))))
                            .ghost()
                            .label(format!("{}{}", entry.name, if is_dir { "/" } else { "" }))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                if is_dir {
                                    this.skills_browse(host_id, path.clone(), cx)
                                } else {
                                    this.skills_open_file(host_id, path.clone(), window, cx)
                                }
                            }))
                            .into_any_element(),
                    );
                }
            }
        }
        if scope == SourceKind::AgentPrivate {
            content.push(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(t(L10nKey::SkillsReadonly))
                    .into_any_element(),
            );
        }
        if let Some(markdown) = skill.markdown() {
            let safe =
                lyrel_core::core::github::markdown::sanitize(markdown, t(L10nKey::GitHubImage));
            content.push(
                div()
                    .pt_2()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child("SKILL.md")
                    .into_any_element(),
            );
            content.push(
                TextView::markdown(
                    SharedString::from(format!("skill-markdown-{}", row_id(skill.path()))),
                    safe,
                )
                .selectable(true)
                .markdown_extensions(
                    gpui_kit::component::text::MarkdownExtensions::default().frontmatter(),
                )
                .markdown_block_parser(|node, parse| {
                    // Frontmatter 已用于上方元数据；正文预览不再重复一份 YAML。
                    if matches!(node, gpui_kit::component::text::markdown_ast::Node::Yaml(_)) {
                        return Some(
                            gpui_kit::component::text::MarkdownNode::new("skill-frontmatter", ())
                                .text(""),
                        );
                    }
                    (!safe_node(node)).then(|| {
                        let source = parse.node_source(node).unwrap_or_default().to_string();
                        gpui_kit::component::text::MarkdownNode::new("skill-unsafe", source.clone())
                            .text(source)
                    })
                })
                .markdown_block_renderer("skill-frontmatter", |_, _, _| div().h_0())
                .markdown_block_renderer("skill-unsafe", |node, _, _| {
                    div().child(node.data::<String>().cloned().unwrap_or_default())
                })
                .into_any_element(),
            );
        }
        content.push(
            div()
                .pt_3()
                .border_t_1()
                .border_color(cx.theme().border)
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(t(L10nKey::SkillsScope))
                .into_any_element(),
        );
        let mut agents = std::collections::HashSet::new();
        for source in skill
            .sources()
            .iter()
            .filter(|source| agents.insert(source.agent()))
        {
            content.push(
                Link::new(SharedString::from(format!(
                    "skill-guide-{}-{}",
                    source.agent().slug(),
                    row_id(source.entry())
                )))
                .child(format!(
                    "{} · {}",
                    source.agent().display_name(),
                    t(L10nKey::SkillsNativeGuide)
                ))
                .href(native_guide(source.agent()))
                .into_any_element(),
            );
            content.push(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(t(native_help(source.agent())))
                    .into_any_element(),
            );
        }
        if let Some(snapshot) = self
            .active_settings()
            .and_then(|s| s.skills.snapshot.as_ref())
        {
            let time = snapshot
                .scanned_at()
                .duration_since(UNIX_EPOCH)
                .ok()
                .and_then(|time| i64::try_from(time.as_secs()).ok())
                .map(skill_timestamp)
                .unwrap_or_else(|| t(L10nKey::SkillsUnknown).to_string());
            content.push(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(t_fmt(L10nKey::SkillsScannedAt, &[("time", &time)]))
                    .into_any_element(),
            );
        }
        v_flex()
            .gap_3()
            .min_w_0()
            .children(content)
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
                let count = matches
                    .iter()
                    .filter(|skill| skill_scope(skill) == scope)
                    .count();
                rows = rows.child(
                    h_flex()
                        .gap_2()
                        .px_2()
                        .pt_2()
                        .pb_1()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(
                            div()
                                .font_weight(FontWeight::MEDIUM)
                                .child(t(source_key(scope))),
                        )
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
                        let owner = source
                            .project()
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
                    .py_1()
                    .justify_start()
                    .accessibility_label(format!(
                        "{} · {} · {}",
                        skill.name(),
                        scopes,
                        path.display()
                    ))
                    .when(selected, |button| button.bg(cx.theme().secondary))
                    .child(
                        h_flex()
                            .w_full()
                            .min_w_0()
                            .gap_2()
                            .child(skill_icon(cx))
                            .child(
                                v_flex()
                                    .min_w_0()
                                    .flex_1()
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
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .truncate()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(
                                                skill.description().unwrap_or(&scopes).to_string(),
                                            ),
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
        let pagination = (matches.len() > 50).then(|| {
            let last = matches.len().saturating_sub(1) / 50;
            h_flex()
                .flex_none()
                .px_2()
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
                )
        });
        v_flex()
            .h_full()
            .min_h_0()
            .min_w_0()
            .when(narrow, |pane| pane.flex_1())
            .when(!narrow, |pane| {
                pane.w(rems(17.))
                    .flex_none()
                    .border_r_1()
                    .border_color(cx.theme().border)
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
                            .prefix(Icon::new(gpui_kit::component::IconName::Search).small())
                            .w_full()
                            .aria_label(t(L10nKey::SkillsSearch))
                            .disabled(state.maintenance.target_locked()),
                    )
                    .child(
                        Button::new("skills-source")
                            .small()
                            .ghost()
                            .bg(cx.theme().muted)
                            .w_full()
                            .child(
                                h_flex()
                                    .w_full()
                                    .min_w_0()
                                    .justify_between()
                                    .gap_2()
                                    .child(div().truncate().child(t(
                                        filter.map(source_key).unwrap_or(L10nKey::SkillsAllSources),
                                    )))
                                    .child(
                                        Icon::new(gpui_kit::component::IconName::ChevronDown)
                                            .small(),
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
                                        if let MenuEntry::Item {
                                            label,
                                            checked,
                                            on_pick,
                                            ..
                                        } = entry
                                        {
                                            let app = app.clone();
                                            let on_pick = on_pick.clone();
                                            menu = menu.item(
                                                PopupMenuItem::new(label.clone())
                                                    .checked(*checked)
                                                    .on_click(move |_, window, cx| {
                                                        let _ = app.update(cx, |app, cx| {
                                                            on_pick(app, window, cx)
                                                        });
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
            .children(pagination)
            .child(
                h_flex()
                    .flex_none()
                    .border_t_1()
                    .border_color(cx.theme().border)
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
                            .loading(state.loading)
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

/// Truncate long technical values without hiding their full copy/view path.
/// The detail owns this table; modal target rows retain their wrapping layout.
fn skill_detail_row(
    id: SharedString,
    label: SharedString,
    value: String,
    copy_path: bool,
    cx: &App,
) -> AnyElement {
    let value_element = if copy_path {
        let copied = value.clone();
        Button::new("value")
            .small()
            .ghost()
            .w_full()
            .min_w_0()
            .px_1()
            .justify_start()
            .accessibility_label(format!("{} · {label}", t(L10nKey::SkillsCopyPath)))
            .tooltip(value.clone())
            .child(div().min_w_0().truncate().text_xs().child(value))
            .on_click(move |_, _, cx| {
                cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(copied.clone()));
            })
            .into_any_element()
    } else {
        let full = value.clone();
        div()
            .id("value")
            .min_w_0()
            .py_1()
            .text_xs()
            .truncate()
            .tooltip(move |window, cx| {
                gpui_kit::component::tooltip::Tooltip::new(full.clone()).build(window, cx)
            })
            .child(value)
            .into_any_element()
    };
    let full_label = label.clone();
    h_flex()
        .id(id)
        .min_w_0()
        .gap_3()
        .border_b_1()
        .border_color(cx.theme().border)
        .child(
            div()
                .id("label")
                .w(rems(6.5))
                .flex_none()
                .truncate()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .tooltip(move |window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(full_label.clone()).build(window, cx)
                })
                .child(label),
        )
        .child(div().min_w_0().flex_1().child(value_element))
        .into_any_element()
}
