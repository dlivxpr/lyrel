//! Explicit-machine skill browsing and reviewed shared installation. The settings
//! session owns target, selection and async generation independently of workspace focus.
use super::{shell::MenuEntry, *};
use crate::ui::{
    host_ops::{Entry, HostOps},
    host_registry::HostRegistry,
};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _,
    button::{Button, ButtonVariants as _},
    text::TextView,
};
use lyrel_core::skills::{
    self, DiscoveryState, ScanRequest, ScanSnapshot, SkillInstallation, SourceKind,
};
use std::{
    io,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::UNIX_EPOCH,
};
mod install;
mod maintenance;
#[cfg(test)]
mod test_host;
mod view;

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::test::TestWindowExt;
    use gpui_kit::{TestAppContext, VisualTestContext};
    fn settle(app: &Entity<LyrelApp>, cx: &mut VisualTestContext) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            cx.background_executor.run_until_parked();
            if app.update_in(cx, |app, _, _| {
                app.active_settings().is_some_and(|s| !s.skills.loading)
            }) {
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "skill scan did not settle"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
    pub(super) fn reveal(
        window: &mut Window,
        cx: &mut App,
        target: impl Into<SharedString>,
        region: &'static str,
    ) {
        let target = target.into();
        for _ in 0..50 {
            window.render_frame(cx);
            let bounds = window.find(region).bounds();
            let item = window.find(target.clone());
            let center = item.bounds().center();
            if item.visible() && bounds.contains(&center) {
                return;
            }
            let delta = if center.y < bounds.top() { 160. } else { -160. };
            window.scroll(
                region,
                gpui_kit::ScrollDelta::Pixels(gpui_kit::point(px(0.), px(delta))),
                cx,
            );
        }
        panic!("{target} is not reachable through {region}");
    }
    #[gpui_kit::test]
    fn skills_workflow_searches_actual_project_files_and_keeps_scope_on_navigation(
        cx: &mut TestAppContext,
    ) {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join(".git")).unwrap();
        let install = temp.path().join(".agents/skills/needle");
        std::fs::create_dir_all(&install).unwrap();
        let original = "---\nname: needle\ndescription: A uniquely searchable reference\n---\n# Actual project skill\n";
        std::fs::write(install.join("SKILL.md"), original).unwrap();
        let (app, mut vcx) = crate::ui::app::test_window::harness(cx);
        vcx.simulate_resize(gpui_kit::size(px(1200.), px(800.)));
        app.update_in(&mut vcx, |app, window, cx| {
            isolate_native_roots(&temp.path().join("native-home"), cx);
            app.open_settings_section(SettingsSection::Skills, window, cx)
        });
        settle(&app, &mut vcx);
        vcx.update(|window, cx| {
            window.render_frame(cx);
            for id in ["skills-project-input", "skills-query"] {
                assert!(window.find(id).label().is_some_and(|name| !name.is_empty()));
            }
            window.click("skills-project-input", cx);
            window.input(temp.path().to_string_lossy().as_ref(), cx);
            window.click("skills-apply-project", cx);
        });
        settle(&app, &mut vcx);
        vcx.update(|window, cx| {
            window.render_frame(cx);
            window.click("skills-query", cx);
            window.input("uniquely searchable", cx);
        });
        let path = app.update_in(&mut vcx, |app, _, cx| {
            let state = &app.active_settings().unwrap().skills;
            let query = state.query.read(cx).value();
            let result = state
                .snapshot
                .as_ref()
                .unwrap()
                .filtered(&query, state.filter)
                .collect::<Vec<_>>();
            assert_eq!(result.len(), 1);
            assert_eq!(result[0].markdown(), Some(original));
            result[0].path().to_path_buf()
        });
        vcx.update(|window, cx| {
            window.render_frame(cx);
            window.click(SharedString::from(row_id(&path)), cx);
        });
        app.update_in(&mut vcx, |app, _, cx| {
            assert_eq!(
                app.active_settings().unwrap().skills.selected.as_deref(),
                Some(path.as_path())
            );
            app.select_settings_section(SettingsSection::General, cx);
            app.select_settings_section(SettingsSection::Skills, cx);
            assert_eq!(
                app.active_settings().unwrap().skills.project.as_deref(),
                Some(temp.path())
            );
        });
        vcx.update(|window, cx| {
            window.render_frame(cx);
            assert!(window.find(SharedString::from(row_id(&path))).visible());
            window.click("skills-copy-path", cx);
            assert_eq!(
                cx.read_from_clipboard().unwrap().text().as_deref(),
                Some(path.to_string_lossy().as_ref())
            );
        });
        vcx.simulate_resize(gpui_kit::size(px(680.), px(560.)));
        vcx.update(|window, cx| {
            window.render_frame(cx);
            assert!(window.find("skills-back").visible());
            window.click("skills-back", cx);
            assert!(window.find(SharedString::from(row_id(&path))).visible());
            window.click(SharedString::from(row_id(&path)), cx);
            window.press("escape", cx);
        });
        app.update_in(&mut vcx, |app, _, _| {
            let state = &app.active_settings().unwrap().skills;
            assert!(state.selected.is_none());
            assert_eq!(state.project.as_deref(), Some(temp.path()));
        });
        assert_eq!(
            std::fs::read_to_string(install.join("SKILL.md")).unwrap(),
            original
        );
    }
    pub(super) fn isolate_native_roots(home: &Path, cx: &mut Context<LyrelApp>) {
        std::fs::create_dir_all(home).unwrap();
        super::test_host::install(home, cx);
        let home = home.to_string_lossy();
        cx.global_mut::<Config>().agent_launch=["omp","pi","kimi","opencode","hermes"].into_iter().map(|agent| {
            let dir=if agent=="omp"{format!("{home}/.omp/agent")}else{format!("{home}/.pi/agent")};
            (agent.into(),format!("HOME='{home}' USERPROFILE='{home}' PROGRAMDATA='{home}/programdata' CLAUDE_CONFIG_DIR='{home}/.claude' XDG_DATA_HOME='{home}/.data' XDG_STATE_HOME='{home}/.state' PI_CONFIG_DIR=.omp OMP_PROFILE=default PI_PROFILE=default PI_CODING_AGENT_DIR='{dir}' KIMI_CODE_HOME='{home}/.kimi-code' HERMES_HOME='{home}/.hermes' XDG_CONFIG_HOME='{home}/.config' OPENCODE_CONFIG_DIR='{home}/.config/opencode' {agent}"))
        }).collect();
    }

    #[gpui_kit::test]
    fn late_scans_never_replace_a_new_project_or_reopened_settings_session(
        cx: &mut TestAppContext,
    ) {
        let temp = tempfile::tempdir().unwrap();
        let old = temp.path().join("old");
        let new = temp.path().join("new");
        for (project, name) in [(&old, "old-installation"), (&new, "new-installation")] {
            std::fs::create_dir_all(project.join(".git")).unwrap();
            let dir = project.join(".agents/skills/reference");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(
                dir.join("SKILL.md"),
                format!("---\nname: {name}\ndescription: Reference\n---\n# {name}\n"),
            )
            .unwrap();
        }
        let (app, mut vcx) = crate::ui::app::test_window::harness(cx);
        app.update_in(&mut vcx, |app, window, cx| {
            isolate_native_roots(&temp.path().join("native-home"), cx);
            app.open_settings_section(SettingsSection::Skills, window, cx);
            for project in [&old, &new] {
                let input = app.active_settings().unwrap().skills.project_input.clone();
                input.update(cx, |input, cx| {
                    input.set_value(project.to_string_lossy().to_string(), window, cx)
                });
                app.apply_skills_project(cx);
            }
        });
        settle(&app, &mut vcx);
        app.update_in(&mut vcx, |app, window, cx| {
            let state = &app.active_settings().unwrap().skills;
            let snapshot = state.snapshot.as_ref().unwrap();
            assert_eq!(snapshot.project(), Some(new.as_path()));
            assert_eq!(snapshot.filtered("old-installation", None).count(), 0);
            assert_eq!(snapshot.filtered("new-installation", None).count(), 1);
            app.refresh_skills(cx);
            app.close_settings(window, cx);
            app.open_settings_section(SettingsSection::Skills, window, cx);
        });
        settle(&app, &mut vcx);
        app.update_in(&mut vcx, |app, _, _| {
            let state = &app.active_settings().unwrap().skills;
            assert_eq!(state.project, None);
            assert_eq!(state.snapshot.as_ref().unwrap().project(), None);
            assert_eq!(
                state
                    .snapshot
                    .as_ref()
                    .unwrap()
                    .filtered("new-installation", None)
                    .count(),
                0
            );
        });
    }
}

pub(crate) struct SkillsState {
    session: u64,
    generation: u64,
    host: HostId,
    project: Option<PathBuf>,
    project_input: Entity<InputState>,
    query: Entity<InputState>,
    filter: Option<SourceKind>,
    focus: gpui_kit::FocusHandle,
    modal_focus: gpui_kit::FocusHandle,
    return_focus: Option<gpui_kit::FocusHandle>,
    list_scroll: gpui_kit::ScrollHandle,
    detail_scroll: gpui_kit::ScrollHandle,
    modal_scroll: gpui_kit::ScrollHandle,
    snapshot: Option<Arc<ScanSnapshot>>,
    loading: bool,
    error: Option<String>,
    selected: Option<PathBuf>,
    page: usize,
    browser: Option<(PathBuf, Vec<Entry>)>,
    browser_generation: u64,
    install: install::InstallState,
    maintenance: maintenance::MaintenanceState,
}
impl SkillsState {
    pub(crate) fn refresh_locale(&self, window: &mut Window, cx: &mut Context<LyrelApp>) {
        self.query.update(cx, |input, cx| {
            input.set_placeholder(t(L10nKey::SkillsSearch), window, cx)
        });
        self.project_input.update(cx, |input, cx| {
            input.set_placeholder(t(L10nKey::SkillsProjectHint), window, cx)
        });
        self.install.refresh_locale(window, cx);
    }
}
fn row_id(path: &Path) -> String {
    format!(
        "skill-installation-{}",
        lyrel_core::host::fnv1a64(path.to_string_lossy().as_bytes())
    )
}
fn source_key(kind: SourceKind) -> L10nKey {
    match kind {
        SourceKind::UserShared => L10nKey::SkillsUserShared,
        SourceKind::ProjectShared => L10nKey::SkillsProjectShared,
        SourceKind::AgentPrivate => L10nKey::SkillsPrivate,
    }
}
fn state_key(state: DiscoveryState) -> L10nKey {
    match state {
        DiscoveryState::Discovered => L10nKey::SkillsDiscovered,
        DiscoveryState::Disabled => L10nKey::SkillsDisabled,
        DiscoveryState::Unknown => L10nKey::SkillsUnknown,
    }
}

impl LyrelApp {
    pub(crate) fn build_skills_state(
        &self,
        subs: &mut Vec<Subscription>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> SkillsState {
        static SESSION: AtomicU64 = AtomicU64::new(1);
        let project_input =
            cx.new(|cx| InputState::new(window, cx).placeholder(t(L10nKey::SkillsProjectHint)));
        subs.push(
            cx.subscribe_in(&project_input, window, |this, _, event, _, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    this.apply_skills_project(cx);
                }
            }),
        );
        let query = cx.new(|cx| InputState::new(window, cx).placeholder(t(L10nKey::SkillsSearch)));
        subs.push(cx.subscribe_in(&query, window, |this, _, event, _, cx| {
            if matches!(event, InputEvent::Change) {
                if let Some(s) = this.active_settings_mut() {
                    s.skills.page = 0;
                    s.skills
                        .list_scroll
                        .set_offset(gpui_kit::point(px(0.), px(0.)));
                }
                cx.notify();
            }
        }));
        SkillsState {
            session: SESSION.fetch_add(1, Ordering::Relaxed),
            generation: 0,
            host: HostId::LOCAL,
            project: None,
            project_input,
            query,
            filter: None,
            focus: cx.focus_handle().tab_stop(true),
            modal_focus: cx.focus_handle(),
            return_focus: None,
            list_scroll: gpui_kit::ScrollHandle::new(),
            detail_scroll: gpui_kit::ScrollHandle::new(),
            modal_scroll: gpui_kit::ScrollHandle::new(),
            snapshot: None,
            loading: false,
            error: None,
            selected: None,
            page: 0,
            browser: None,
            browser_generation: 0,
            install: install::InstallState::new(subs, window, cx),
            maintenance: maintenance::MaintenanceState::default(),
        }
    }
    pub(crate) fn skills_section_changed(
        &mut self,
        target: SettingsSection,
        cx: &mut Context<Self>,
    ) {
        if target == SettingsSection::Skills {
            if self
                .active_settings()
                .is_some_and(|s| s.skills.snapshot.is_none() && !s.skills.loading)
            {
                self.refresh_skills(cx);
            }
        } else if let Some(s) = self.active_settings_mut() {
            s.skills.generation += 1;
            s.skills.browser_generation += 1;
            s.skills.loading = false;
            s.skills.install.invalidate();
            s.skills.maintenance.invalidate();
        }
    }
    fn select_skills_host(&mut self, host: HostId, window: &mut Window, cx: &mut Context<Self>) {
        if self.active_settings().is_some_and(|s| {
            s.skills.install.target_locked() || s.skills.maintenance.target_locked()
        }) {
            return;
        }
        if let Some(s) = self.active_settings_mut() {
            if s.skills.host == host {
                return;
            }
            s.skills.host = host;
            s.skills.install.invalidate();
            s.skills.maintenance.invalidate();
            s.skills.project = None;
            s.skills.snapshot = None;
            s.skills.selected = None;
            s.skills.browser = None;
            s.skills
                .project_input
                .clone()
                .update(cx, |input, cx| input.set_value("", window, cx));
        }
        self.refresh_skills(cx);
    }
    fn apply_skills_project(&mut self, cx: &mut Context<Self>) {
        if self.active_settings().is_some_and(|s| {
            s.skills.install.target_locked() || s.skills.maintenance.target_locked()
        }) {
            return;
        }
        let Some(project) = self
            .active_settings()
            .map(|s| s.skills.project_input.read(cx).value().trim().to_string())
        else {
            return;
        };
        if let Some(s) = self.active_settings_mut() {
            let project = (!project.is_empty()).then(|| PathBuf::from(project));
            if s.skills.project != project {
                s.skills.install.invalidate();
                s.skills.maintenance.invalidate();
                s.skills.snapshot = None;
                s.skills.selected = None;
                s.skills.browser = None;
            }
            s.skills.project = project;
        }
        self.refresh_skills(cx);
    }
    pub(crate) fn refresh_skills(&mut self, cx: &mut Context<Self>) {
        let launches = cx.global::<Config>().agent_launch.clone();
        let Some(s) = self.active_settings_mut() else {
            return;
        };
        let state = &mut s.skills;
        state.generation += 1;
        state.browser_generation += 1;
        let (session, generation, host_id, project) = (
            state.session,
            state.generation,
            state.host,
            state.project.clone(),
        );
        state.error = None;
        state.loading = true;
        let Some(host) = HostRegistry::get(cx, host_id).filter(|host| host.is_connected()) else {
            if let Some(s) = self.active_settings_mut() {
                s.skills.loading = false;
                s.skills.error = Some(t(L10nKey::SkillsNoMachine).to_string());
            }
            cx.notify();
            return;
        };
        HostOps::run(
            host,
            cx,
            move |host| {
                let environment = host.agent_environment()?;
                let keys = if host.separator() == '\\' {
                    ["USERPROFILE", "HOME"]
                } else {
                    ["HOME", "USERPROFILE"]
                };
                let home=keys.into_iter().find_map(|key|environment.get(key).filter(|v|!v.is_empty())).ok_or_else(||io::Error::new(io::ErrorKind::Unsupported,"selected Host did not report HOME/USERPROFILE; upgrade remote capability"))?;
                let mut request = ScanRequest::new(home).with_launch_commands(launches);
                if host.id() == HostId::LOCAL
                    || environment.get("XDG_STATE_HOME").is_some_and(|state| {
                        !state.is_empty() && !host.is_absolute(Path::new(state))
                    })
                {
                    request = request.with_working_directory(host.canonicalize(Path::new("."))?);
                }
                if let Some(project) = project {
                    request = request.with_project(project);
                }
                skills::scan(host, &request)
            },
            move |this, result: io::Result<ScanSnapshot>, cx| {
                let Some(s) = this.active_settings_mut().filter(|s| {
                    s.section == SettingsSection::Skills
                        && s.skills.session == session
                        && s.skills.generation == generation
                        && s.skills.host == host_id
                }) else {
                    return;
                };
                s.skills.loading = false;
                match result {
                    Ok(snapshot) => {
                        s.skills.snapshot = Some(Arc::new(snapshot));
                        s.skills.error = None;
                    }
                    Err(error) => {
                        s.skills.error = Some(t_fmt(
                            L10nKey::SkillsReadError,
                            &[("error", &error.to_string())],
                        ))
                    }
                }
                cx.notify();
            },
        );
        cx.notify();
    }
    fn skills_open_file(
        &mut self,
        host_id: HostId,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(host) = HostRegistry::get(cx, host_id).filter(|h| h.is_connected()) else {
            if let Some(s) = self.active_settings_mut() {
                s.skills.error = Some(t(L10nKey::SkillsNoMachine).to_string());
            }
            cx.notify();
            return;
        };
        self.close_settings_then(window, cx, move |this, window, cx| {
            this.editor_open_on_host(host, &path, window, cx)
        });
    }
    fn skills_browse(&mut self, host_id: HostId, path: PathBuf, cx: &mut Context<Self>) {
        let Some(host) = HostRegistry::get(cx, host_id).filter(|h| h.is_connected()) else {
            if let Some(s) = self.active_settings_mut() {
                s.skills.error = Some(t(L10nKey::SkillsNoMachine).to_string());
            }
            cx.notify();
            return;
        };
        let Some(s) = self.active_settings_mut() else {
            return;
        };
        s.skills.browser_generation += 1;
        let (session, generation) = (s.skills.session, s.skills.browser_generation);
        let requested = path.clone();
        HostOps::run(
            host,
            cx,
            move |host| host.read_dir(&path, None),
            move |this, result, cx| {
                if let Some(s) = this.active_settings_mut().filter(|s| {
                    s.section == SettingsSection::Skills
                        && s.skills.session == session
                        && s.skills.browser_generation == generation
                        && s.skills.host == host_id
                }) {
                    match result {
                        Ok(entries) => s.skills.browser = Some((requested, entries)),
                        Err(error) => s.skills.error = Some(error.to_string()),
                    };
                    cx.notify();
                }
            },
        );
    }
    fn render_skill_detail(&self, skill: &SkillInstallation, cx: &mut Context<Self>) -> AnyElement {
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
        let mut content = vec![
            Button::new("skills-back")
                .small()
                .ghost()
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
            div()
                .text_base()
                .font_weight(FontWeight::MEDIUM)
                .child(skill.name().to_string())
                .into_any_element(),
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(
                    skill
                        .sources()
                        .iter()
                        .map(|source| {
                            format!(
                                "{} · {}",
                                source.agent().display_name(),
                                t(source_key(source.kind()))
                            )
                        })
                        .collect::<Vec<_>>()
                        .join(" · "),
                )
                .into_any_element(),
            div()
                .child(
                    skill
                        .description()
                        .unwrap_or(t(L10nKey::SkillsUnknown))
                        .to_string(),
                )
                .into_any_element(),
            skill_metadata_row(
                L10nKey::SkillsLocation,
                path.to_string_lossy().into_owned(),
                cx,
            ),
            h_flex()
                .flex_wrap()
                .gap_2()
                .child(
                    Button::new("skills-open-file")
                        .small()
                        .ghost()
                        .label(t(L10nKey::SkillsOpenFile))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.skills_open_file(host_id, markdown_path.clone(), window, cx)
                        })),
                )
                .child(
                    Button::new("skills-copy-path")
                        .small()
                        .ghost()
                        .label(t(L10nKey::SkillsCopyPath))
                        .on_click(move |_, _, cx| {
                            cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(
                                copy_path.clone(),
                            ))
                        }),
                )
                .child(
                    Button::new("skills-browse-files")
                        .small()
                        .ghost()
                        .label(t(L10nKey::SkillsBrowseFiles))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if host_id.is_local() {
                                cx.reveal_path(&browse_path);
                            } else {
                                this.skills_browse(host_id, browse_path.clone(), cx);
                            }
                        })),
                )
                .into_any_element(),
        ];
        content.push(self.render_skill_maintenance(skill, cx));
        for (key, value) in [
            (
                L10nKey::SkillsOrigin,
                skill
                    .origin()
                    .unwrap_or(t(L10nKey::SkillsNoSource))
                    .to_string(),
            ),
            (
                L10nKey::SkillsInvocation,
                skill
                    .invocation()
                    .unwrap_or(t(L10nKey::SkillsUnknown))
                    .to_string(),
            ),
            (
                L10nKey::SkillsUpdated,
                skill
                    .updated()
                    .map(|time| time.secs.to_string())
                    .unwrap_or_else(|| t(L10nKey::SkillsUnknown).to_string()),
            ),
        ] {
            content.push(skill_metadata_row(key, value, cx));
        }
        let companions = skill.companions();
        content.push(
            div()
                .text_sm()
                .child(t_fmt(
                    L10nKey::SkillsCompanions,
                    &[
                        ("files", &companions.files().to_string()),
                        ("bytes", &companions.bytes().to_string()),
                        (
                            "limited",
                            if companions.is_incomplete() {
                                t(L10nKey::SkillsFileLimit)
                            } else {
                                ""
                            },
                        ),
                    ],
                ))
                .into_any_element(),
        );
        if !skill.sources().iter().any(|source| {
            matches!(
                source.kind(),
                SourceKind::UserShared | SourceKind::ProjectShared
            )
        }) {
            content.push(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(t(L10nKey::SkillsReadonly))
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
                .unwrap_or_default()
                .as_secs()
                .to_string();
            content.push(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(t_fmt(L10nKey::SkillsScannedAt, &[("time", &time)]))
                    .into_any_element(),
            );
        }
        if let Some(markdown) = skill.markdown() {
            let safe =
                lyrel_core::core::github::markdown::sanitize(markdown, t(L10nKey::GitHubImage));
            content.push(
                TextView::markdown(
                    SharedString::from(format!("skill-markdown-{}", row_id(skill.path()))),
                    safe,
                )
                .selectable(true)
                .markdown_extensions(
                    gpui_kit::component::text::MarkdownExtensions::default()
                        .frontmatter()
                        .plugin(gpui_kit::component::text::FrontmatterPlugin::new()),
                )
                .markdown_block_parser(|node, parse| {
                    (!safe_node(node)).then(|| {
                        let source = parse.node_source(node).unwrap_or_default().to_string();
                        gpui_kit::component::text::MarkdownNode::new("skill-unsafe", source.clone())
                            .text(source)
                    })
                })
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
        for source in skill.sources() {
            content.push(
                div()
                    .text_sm()
                    .child(format!(
                        "{} · {} · {}\n{}",
                        source.agent().display_name(),
                        t(source_key(source.kind())),
                        t(state_key(source.state())),
                        source.entry().display()
                    ))
                    .into_any_element(),
            );
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
        }
        if let Some(record) = skill.source_record() {
            content.push(
                div()
                    .text_sm()
                    .child(format!(
                        "{} · {}\n{}\n{}",
                        record.source_type(),
                        record.reference().unwrap_or(t(L10nKey::SkillsUnknown)),
                        record.skill_path().unwrap_or(t(L10nKey::SkillsUnknown)),
                        record.lock_path().display()
                    ))
                    .into_any_element(),
            );
        }
        let mut agents = std::collections::HashSet::new();
        for source in skill
            .sources()
            .iter()
            .filter(|source| agents.insert(source.agent()))
        {
            content.push(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(t(native_help(source.agent())))
                    .into_any_element(),
            );
        }
        for problem in skill.problems() {
            content.push(
                div()
                    .text_color(cx.theme().warning)
                    .text_sm()
                    .child(problem.clone())
                    .into_any_element(),
            );
        }
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
        v_flex()
            .gap_3()
            .min_w_0()
            .children(content)
            .into_any_element()
    }
}
fn skill_metadata_row(key: L10nKey, value: String, cx: &App) -> AnyElement {
    h_flex()
        .items_start()
        .gap_3()
        .min_w_0()
        .child(
            div()
                .w_24()
                .flex_none()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(t(key)),
        )
        .child(div().flex_1().min_w_0().text_sm().truncate().child(value))
        .into_any_element()
}
fn machines_connected(cx: &App, id: HostId) -> bool {
    HostRegistry::lookup(cx, id).is_some_and(|host| host.is_connected())
}
fn native_guide(agent: crate::core::agent_hooks::HookAgent) -> &'static str {
    use crate::core::agent_hooks::HookAgent::*;
    match agent {
        OhMyPi => {
            "https://github.com/can1357/oh-my-pi/blob/9d5d4ac5b50ac2bb5866e4202efc0cf785dfc57a/docs/skills.md"
        }
        Pi => {
            "https://github.com/earendil-works/pi/blob/1402973a9dbe8c627341e04c9744c54953e285ca/packages/coding-agent/docs/skills.md"
        }
        Kimi => {
            "https://github.com/MoonshotAI/kimi-code/blob/472192c34c1efad89a369870eff0da0b708ce9bb/docs/en/customization/skills.md"
        }
        OpenCode => {
            "https://github.com/anomalyco/opencode/blob/v2.0.32/services/www/src/docs/content/skills.mdx"
        }
        Hermes => {
            "https://github.com/NousResearch/hermes-agent/blob/v0.22.2/website/docs/user-guide/features/skills.md"
        }
    }
}
fn safe_node(node: &gpui_kit::component::text::markdown_ast::Node) -> bool {
    use gpui_kit::component::text::markdown_ast::Node;
    let here = match node {
        Node::Image(_) | Node::ImageReference(_) | Node::Html(_) => false,
        Node::Link(link) => lyrel_core::core::github::markdown::is_safe_target(&link.url),
        Node::Definition(def) => lyrel_core::core::github::markdown::is_safe_target(&def.url),
        _ => true,
    };
    here && node
        .children()
        .is_none_or(|children| children.iter().all(safe_node))
}

fn native_help(agent: crate::core::agent_hooks::HookAgent) -> L10nKey {
    use crate::core::agent_hooks::HookAgent::*;
    match agent {
        OhMyPi => L10nKey::SkillsHelpOmp,
        Pi => L10nKey::SkillsHelpPi,
        Kimi => L10nKey::SkillsHelpKimi,
        OpenCode => L10nKey::SkillsHelpOpenCode,
        Hermes => L10nKey::SkillsHelpHermes,
    }
}
