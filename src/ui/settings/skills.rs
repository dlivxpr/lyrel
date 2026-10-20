//! Explicit-machine skill browsing and reviewed shared installation. The settings
//! session owns target, selection and async generation independently of workspace focus.
use super::{shell::MenuEntry, *};
use crate::ui::{
    host_ops::{Entry, HostOps},
    host_registry::HostRegistry,
};
use gpui_kit::TestSupportExt as _;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _,
    button::{Button, ButtonVariants as _},
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
};
mod install;
mod maintenance;
#[cfg(test)]
pub(crate) mod test_host;
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
            assert!(window.try_find("skills-back").is_none());
            assert!(
                window.find("skills-detail-metadata").bounds().bottom()
                    <= window.find("skills-detail-actions").bounds().top()
            );
            assert!(
                (window.find("skills-query").bounds().size.width
                    - window.find("skills-source").bounds().size.width)
                    .abs()
                    <= px(1.)
            );
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
}
fn skill_metadata_row(key: L10nKey, value: String, cx: &App) -> AnyElement {
    h_flex()
        .items_start()
        .gap_3()
        .min_w_0()
        .child(
            div()
                .w_16()
                .flex_none()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(t(key)),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_sm()
                .whitespace_normal()
                .child(value),
        )
        .into_any_element()
}
fn skill_scope(skill: &SkillInstallation) -> SourceKind {
    [SourceKind::UserShared, SourceKind::ProjectShared]
        .into_iter()
        .find(|kind| skill.sources().iter().any(|source| source.kind() == *kind))
        .unwrap_or(SourceKind::AgentPrivate)
}

fn skill_icon(cx: &App) -> impl IntoElement + use<> {
    gpui_kit::svg()
        .path("icons/settings/integrations.svg")
        .size_4()
        .flex_none()
        .text_color(cx.theme().muted_foreground)
}

fn skill_timestamp(seconds: i64) -> String {
    chrono::DateTime::from_timestamp(seconds, 0)
        .map(|time| {
            time.with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M %:z")
                .to_string()
        })
        .unwrap_or_else(|| t(L10nKey::SkillsUnknown).to_string())
}

fn skill_size(bytes: u64) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1} MiB", bytes as f64 / (1024. * 1024.))
    } else if bytes >= 1024 {
        format!("{:.1} KiB", bytes as f64 / 1024.)
    } else {
        format!("{bytes} B")
    }
}
fn machines_connected(cx: &App, id: HostId) -> bool {
    HostRegistry::lookup(cx, id).is_some_and(|host| host.is_connected())
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
