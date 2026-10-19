//! Frozen, single-installation maintenance plans owned by the settings session.
use super::*;
use gpui_kit::component::checkbox::Checkbox;
use lyrel_core::{
    core::github::Transport,
    host::{Host, SharedHost},
    skills::{
        catalog,
        install::{self, InstallDecision, InstallPreview, InstallStatus, InstallTarget},
        maintenance::{self, LocalChanges, RemovalStatus, RemovePreview, UpdateStatus},
    },
};

#[cfg(test)]
#[path = "maintenance_tests.rs"]
pub(super) mod tests;

#[derive(Default)]
pub(super) struct MaintenanceState {
    generation: u64,
    pub(super) busy: bool,
    committing: bool,
    path: Option<PathBuf>,
    host: Option<SharedHost>,
    update: Option<InstallPreview>,
    remove: Option<RemovePreview>,
    replace: bool,
    status: Option<UpdateStatus>,
    local_changes: Option<LocalChanges>,
    messages: Vec<String>,
    error: Option<String>,
    pub(super) github: Option<Arc<dyn Transport>>,
}
impl MaintenanceState {
    pub(super) fn is_open(&self) -> bool {
        self.path.is_some()
    }
    pub(super) fn target_locked(&self) -> bool {
        self.busy || self.update.is_some() || self.remove.is_some()
    }
    pub(super) fn invalidate(&mut self) {
        if self.committing {
            return;
        }
        self.generation += 1;
        self.busy = false;
        self.path = None;
        self.host = None;
        self.update = None;
        self.remove = None;
        self.replace = false;
        self.status = None;
        self.local_changes = None;
        self.messages.clear();
        self.error = None;
    }
}
fn target_and_request(
    host: &dyn Host,
    skill: &SkillInstallation,
    project: Option<PathBuf>,
    launches: std::collections::HashMap<String, String>,
) -> io::Result<(InstallTarget, ScanRequest)> {
    let environment = host.agent_environment()?;
    let keys = if host.separator() == '\\' {
        ["USERPROFILE", "HOME"]
    } else {
        ["HOME", "USERPROFILE"]
    };
    let home = keys
        .into_iter()
        .find_map(|key| environment.get(key).filter(|v| !v.is_empty()))
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::Unsupported,
                "selected Host did not report HOME/USERPROFILE; upgrade the server",
            )
        })?;
    let state = environment
        .get("XDG_STATE_HOME")
        .filter(|s| !s.is_empty())
        .map(PathBuf::from);
    let cwd = if host.id().is_local() || state.as_ref().is_some_and(|s| !host.is_absolute(s)) {
        host.canonicalize(Path::new("."))?
    } else {
        PathBuf::new()
    };
    let mut request = ScanRequest::new(home).with_launch_commands(launches);
    if !cwd.as_os_str().is_empty() {
        request = request.with_working_directory(cwd.clone());
    }
    if let Some(project) = &project {
        request = request.with_project(project);
    }
    // The core checks that the exact installation lies directly in this target.
    let target = if skill
        .sources()
        .iter()
        .any(|s| s.kind() == SourceKind::ProjectShared)
    {
        let owner = skill
            .sources()
            .iter()
            .filter_map(|source| source.project())
            .next()
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "project installation has no proven owner",
                )
            })?;
        if project
            .as_deref()
            .map(|selected| host.canonicalize(selected))
            .transpose()?
            != Some(host.canonicalize(owner)?)
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                t_fmt(
                    L10nKey::SkillsOwnerProjectRequired,
                    &[("project", &owner.to_string_lossy())],
                ),
            ));
        }
        InstallTarget::project(host, owner.to_path_buf())?
    } else {
        InstallTarget::user(host, home.into(), state, cwd)?
    };
    Ok((target, request))
}
fn removal_message(status: &RemovalStatus) -> String {
    match status {
        RemovalStatus::Removed => t(L10nKey::SkillsRemoved).into(),
        RemovalStatus::Retained(reason) => {
            t_fmt(L10nKey::SkillsMaintenanceRetained, &[("reason", reason)])
        }
        RemovalStatus::Failed(reason) => {
            t_fmt(L10nKey::SkillsMaintenanceFailed, &[("error", reason)])
        }
        RemovalStatus::OutcomeUnknown(reason) => t_fmt(
            L10nKey::SkillsMaintenanceUnknownOutcome,
            &[("error", reason)],
        ),
    }
}
impl LyrelApp {
    fn maintenance_current(
        &mut self,
        session: u64,
        generation: u64,
    ) -> Option<&mut MaintenanceState> {
        self.active_settings_mut()
            .filter(|s| {
                s.skills.session == session && s.skills.maintenance.generation == generation
            })
            .map(|s| &mut s.skills.maintenance)
    }
    pub(crate) fn cancel_skills_maintenance(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(s) = self
            .active_settings_mut()
            .filter(|s| s.section == SettingsSection::Skills && s.menu.is_none())
        else {
            return false;
        };
        let state = &mut s.skills.maintenance;
        if state.committing {
            return true;
        }
        if state.path.is_none() {
            return false;
        }
        state.invalidate();
        let focus = s
            .skills
            .return_focus
            .take()
            .unwrap_or_else(|| s.skills.focus.clone());
        window.focus(&focus, cx);
        cx.notify();
        true
    }
    fn review_skill_maintenance(&mut self, remove: bool, cx: &mut Context<Self>) {
        let proxy = cx.global::<Config>().http_proxy.clone();
        let launches = cx.global::<Config>().agent_launch.clone();
        let Some(s) = self.active_settings_mut() else {
            return;
        };
        if s.skills.maintenance.busy || s.skills.install.target_locked() {
            return;
        }
        let Some(skill) = s
            .skills
            .snapshot
            .as_ref()
            .and_then(|snapshot| {
                snapshot
                    .installations()
                    .iter()
                    .find(|skill| Some(skill.path()) == s.skills.selected.as_deref())
            })
            .cloned()
        else {
            return;
        };
        let project = s.skills.project.clone();
        let state = &mut s.skills.maintenance;
        state.invalidate();
        state.path = Some(skill.path().into());
        state.busy = true;
        let (session, generation, injected) =
            (s.skills.session, state.generation, state.github.clone());
        let Some(host) = HostRegistry::get(cx, skill.host()).filter(|host| host.is_connected())
        else {
            state.busy = false;
            state.error = Some(t(L10nKey::SkillsNoMachine).into());
            cx.notify();
            return;
        };
        let fixed_host = host.clone();
        HostOps::run(
            host,
            cx,
            move |host| {
                let (target, request) = target_and_request(host, &skill, project, launches)
                    .map_err(|e| e.to_string())?;
                if remove {
                    let roots =
                        maintenance::private_roots(host, &request).map_err(|e| e.to_string())?;
                    maintenance::preview_remove(host, target, &skill, &roots)
                        .map(MaintenanceReview::Remove)
                        .map_err(|e| e.to_string())
                } else {
                    let http = injected
                        .unwrap_or_else(|| catalog::desktop_github_transport(proxy.as_deref()));
                    Ok(MaintenanceReview::Update(maintenance::check_update(
                        host,
                        target,
                        &skill,
                        http.as_ref(),
                    )))
                }
            },
            move |app, result: Result<MaintenanceReview, String>, cx| {
                if let Some(state) = app.maintenance_current(session, generation) {
                    state.busy = false;
                    match result {
                        Ok(MaintenanceReview::Update(check)) => {
                            state.status = Some(check.status);
                            state.local_changes = Some(check.local_changes);
                            state.update = check.preview;
                            state.messages = check.warnings;
                            state.host = Some(fixed_host);
                        }
                        Ok(MaintenanceReview::Remove(preview)) => {
                            state.remove = Some(preview);
                            state.host = Some(fixed_host);
                        }
                        Err(error) => state.error = Some(error),
                    }
                    cx.notify();
                }
            },
        );
        cx.notify();
    }
    fn confirm_skill_maintenance(&mut self, remove: bool, cx: &mut Context<Self>) {
        let Some(s) = self.active_settings_mut() else {
            return;
        };
        let state = &mut s.skills.maintenance;
        if state.busy {
            return;
        }
        // Keep is the default. Activating the commit without opt-in is a no-op.
        if !remove && !state.replace {
            return;
        }
        let plan = if remove {
            state.remove.take().map(MaintenancePlan::Remove)
        } else {
            state.update.take().map(MaintenancePlan::Update)
        };
        let Some(plan) = plan else {
            return;
        };
        let Some(host) = state.host.take() else {
            return;
        };
        state.busy = true;
        state.committing = true;
        state.error = None;
        state.messages.clear();
        let (session, generation) = (s.skills.session, state.generation);
        HostOps::run(
            host,
            cx,
            move |host| match plan {
                MaintenancePlan::Update(preview) => {
                    let report = install::apply_install(
                        host,
                        preview,
                        &[InstallDecision::Replace],
                        &chrono::Utc::now().to_rfc3339(),
                    );
                    let mut messages: Vec<String> = report
                        .items
                        .into_iter()
                        .map(|item| match item.status {
                            InstallStatus::Installed | InstallStatus::Replaced => t_fmt(
                                L10nKey::SkillsUpdateApplied,
                                &[("path", &item.destination.to_string_lossy())],
                            ),
                            InstallStatus::Kept => t_fmt(
                                L10nKey::SkillsInstallRetained,
                                &[
                                    ("name", &item.name),
                                    ("path", &item.destination.to_string_lossy()),
                                ],
                            ),
                            InstallStatus::Failed(error) => {
                                t_fmt(L10nKey::SkillsMaintenanceFailed, &[("error", &error)])
                            }
                        })
                        .collect();
                    messages.extend(report.warnings);
                    messages
                }
                MaintenancePlan::Remove(preview) => {
                    let path = preview.destination().to_string_lossy().into_owned();
                    let lock = preview.target().lock_path().to_string_lossy().into_owned();
                    let report = maintenance::apply_remove(host, preview);
                    let mut messages = vec![
                        format!("{path}: {}", removal_message(&report.content)),
                        format!("{lock}: {}", removal_message(&report.source_record)),
                    ];
                    messages.extend(report.links.iter().map(|link| {
                        format!("{}: {}", link.path.display(), removal_message(&link.status))
                    }));
                    messages.extend(report.warnings);
                    messages
                }
            },
            move |app, messages, cx| {
                if let Some(state) = app.maintenance_current(session, generation) {
                    state.busy = false;
                    state.committing = false;
                    state.replace = false;
                    state.messages = messages;
                    state.status = None;
                    state.local_changes = None;
                    app.refresh_skills(cx);
                    cx.notify();
                }
            },
        );
        cx.notify();
    }
    pub(super) fn render_skill_maintenance(
        &self,
        skill: &SkillInstallation,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(s) = self.active_settings() else {
            return div().into_any_element();
        };
        let state = &s.skills.maintenance;
        if !skill.sources().iter().any(|source| {
            matches!(
                source.kind(),
                SourceKind::UserShared | SourceKind::ProjectShared
            )
        }) {
            return div().into_any_element();
        }
        h_flex()
            .flex_wrap()
            .gap_2()
            .child(
                Button::new("skills-check-update")
                    .small()
                    .ghost()
                    .label(format!("{}…", t(L10nKey::SkillsCheckUpdate)))
                    .disabled(state.target_locked() || s.skills.install.open)
                    .on_click(cx.listener(|app, _, window, cx| {
                        if let Some(s) = app.active_settings_mut() {
                            s.skills.return_focus = window.focused(cx);
                            s.skills
                                .modal_scroll
                                .set_offset(gpui_kit::point(px(0.), px(0.)));
                        }
                        app.review_skill_maintenance(false, cx);
                        if let Some(s) = app.active_settings() {
                            window.focus(&s.skills.modal_focus, cx);
                        }
                    })),
            )
            .child(
                Button::new("skills-remove-review")
                    .small()
                    .ghost()
                    .label(t(L10nKey::SkillsRemoveReview))
                    .disabled(state.target_locked() || s.skills.install.open)
                    .on_click(cx.listener(|app, _, window, cx| {
                        if let Some(s) = app.active_settings_mut() {
                            s.skills.return_focus = window.focused(cx);
                            s.skills
                                .modal_scroll
                                .set_offset(gpui_kit::point(px(0.), px(0.)));
                        }
                        app.review_skill_maintenance(true, cx);
                        if let Some(s) = app.active_settings() {
                            window.focus(&s.skills.modal_focus, cx);
                        }
                    })),
            )
            .into_any_element()
    }
    pub(super) fn render_skills_maintenance_dialog(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(s) = self.active_settings() else {
            return div().into_any_element();
        };
        let state = &s.skills.maintenance;
        let mut body = vec![self.skills_modal_target(cx)];
        if let Some(path) = &state.path {
            body.push(skill_metadata_row(
                L10nKey::SkillsLocation,
                path.display().to_string(),
                cx,
            ));
        }
        let mut footer = h_flex().gap_2().child(
            Button::new("skills-maintenance-cancel")
                .small()
                .ghost()
                .label(t(L10nKey::SkillsMaintenanceCancel))
                .disabled(state.committing)
                .on_click(cx.listener(|app, _, window, cx| {
                    app.cancel_skills_maintenance(window, cx);
                })),
        );
        if state.busy {
            body.push(
                div()
                    .child(t(L10nKey::SkillsMaintenanceWorking))
                    .into_any_element(),
            );
        }
        if let Some(status) = &state.status {
            let (key, reason) = match status {
                UpdateStatus::Available => (L10nKey::SkillsUpdateAvailable, None),
                UpdateStatus::UpToDate => (L10nKey::SkillsUpdateCurrent, None),
                UpdateStatus::DeletedUpstream => (L10nKey::SkillsUpdateDeleted, None),
                UpdateStatus::UnknownSource(reason) => (L10nKey::SkillsUpdateUnknown, Some(reason)),
                UpdateStatus::Failed(reason) => (L10nKey::SkillsUpdateFailed, Some(reason)),
            };
            body.push(div().child(t(key)).into_any_element());
            if let Some(reason) = reason {
                body.push(div().text_sm().child(reason.clone()).into_any_element());
            }
        }
        if let Some(preview) = &state.update {
            let changes = match state.local_changes.as_ref() {
                Some(LocalChanges::Unmodified) => L10nKey::SkillsBaselineUnmodified,
                Some(LocalChanges::Modified) => L10nKey::SkillsBaselineModified,
                _ => L10nKey::SkillsBaselineUnknown,
            };
            body.push(div().child(t(changes)).into_any_element());
            body.push(
                div()
                    .text_sm()
                    .child(preview.target().lock_path().to_string_lossy().into_owned())
                    .into_any_element(),
            );
            for item in preview.items() {
                body.push(
                    div()
                        .text_sm()
                        .child(format!(
                            "{}\n{} · {}\n{}",
                            item.destination().display(),
                            item.source().repository(),
                            item.source().reference.as_deref().unwrap_or("HEAD"),
                            item.skill_path()
                        ))
                        .into_any_element(),
                );
            }
            body.push(
                Checkbox::new("skills-update-replace")
                    .checked(state.replace)
                    .label(t(L10nKey::SkillsUpdateReplace))
                    .on_click(cx.listener(|app, checked, _, cx| {
                        if let Some(s) = app.active_settings_mut() {
                            if !s.skills.maintenance.busy {
                                s.skills.maintenance.replace = *checked;
                            }
                        }
                        cx.notify();
                    }))
                    .into_any_element(),
            );
            footer = footer.child(
                Button::new("skills-update-confirm")
                    .small()
                    .label(t(L10nKey::SkillsUpdateConfirm))
                    .disabled(state.busy || !state.replace)
                    .custom(crate::ui::theme::inverted_button(cx.theme().popover, cx))
                    .on_click(cx.listener(|app, _, window, cx| {
                        if let Some(s) = app.active_settings() {
                            s.skills
                                .modal_scroll
                                .set_offset(gpui_kit::point(px(0.), px(0.)));
                            window.focus(&s.skills.modal_focus, cx);
                        }
                        app.confirm_skill_maintenance(false, cx);
                    })),
            );
        }
        if let Some(preview) = &state.remove {
            body.push(
                div()
                    .child(t(L10nKey::SkillsRemoveImpact))
                    .into_any_element(),
            );
            body.push(
                div()
                    .text_sm()
                    .child(preview.destination().to_string_lossy().into_owned())
                    .into_any_element(),
            );
            body.push(
                div()
                    .text_sm()
                    .child(format!(
                        "{}\n{}",
                        preview.target().lock_path().display(),
                        preview.lock_key().unwrap_or(t(L10nKey::SkillsNoSource))
                    ))
                    .into_any_element(),
            );
            body.push(
                div()
                    .child(t(L10nKey::SkillsRemoveLinks))
                    .into_any_element(),
            );
            if preview.links().is_empty() {
                body.push(
                    div()
                        .text_sm()
                        .child(t(L10nKey::SkillsRemoveNoLinks))
                        .into_any_element(),
                );
            }
            for link in preview.links() {
                body.push(
                    div()
                        .text_sm()
                        .child(format!(
                            "{} → {}",
                            link.path().display(),
                            link.target().display()
                        ))
                        .into_any_element(),
                );
            }
            for warning in preview.warnings() {
                body.push(
                    div()
                        .text_sm()
                        .text_color(cx.theme().warning)
                        .child(warning.clone())
                        .into_any_element(),
                );
            }
            footer = footer.child(
                Button::new("skills-remove-confirm")
                    .small()
                    .danger()
                    .label(t(L10nKey::SkillsRemoveConfirm))
                    .disabled(state.busy)
                    .on_click(cx.listener(|app, _, window, cx| {
                        if let Some(s) = app.active_settings() {
                            s.skills
                                .modal_scroll
                                .set_offset(gpui_kit::point(px(0.), px(0.)));
                            window.focus(&s.skills.modal_focus, cx);
                        }
                        app.confirm_skill_maintenance(true, cx);
                    })),
            );
        }
        if let Some(error) = &state.error {
            body.push(
                div()
                    .text_sm()
                    .text_color(cx.theme().danger)
                    .child(error.clone())
                    .into_any_element(),
            );
        }
        body.extend(
            state
                .messages
                .iter()
                .map(|message| div().text_sm().child(message.clone()).into_any_element()),
        );
        self.render_skills_modal(
            "skills-maintenance-dialog",
            t(L10nKey::SkillsMaintenanceTitle).into(),
            v_flex().gap_3().min_w_0().children(body).into_any_element(),
            footer.into_any_element(),
            cx,
        )
    }
}
enum MaintenanceReview {
    Update(maintenance::UpdateCheck),
    Remove(RemovePreview),
}
enum MaintenancePlan {
    Update(InstallPreview),
    Remove(RemovePreview),
}
