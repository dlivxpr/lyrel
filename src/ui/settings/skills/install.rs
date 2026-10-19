//! Discovery and installation belong to the settings session. Network
//! bytes stay on the desktop; the confirmed destination is a frozen Host plan.
use super::*;
use gpui_kit::component::{
    checkbox::Checkbox,
    select::{Select, SelectEvent, SelectItem, SelectState},
};
use lyrel_core::{
    core::github::{ApiError, Transport},
    skills::{
        catalog::{self, GitHubSource, SkillCatalog, SkillSearchResult},
        install::{
            self, ExistingContent, InstallDecision, InstallPreview, InstallStatus, InstallTarget,
        },
    },
};
use std::collections::BTreeSet;

#[cfg(test)]
#[path = "install_tests.rs"]
mod tests;

#[derive(Clone)]
struct ScopeChoice(bool);
impl SelectItem for ScopeChoice {
    type Value = bool;
    fn title(&self) -> SharedString {
        t(if self.0 {
            L10nKey::SkillsProjectShared
        } else {
            L10nKey::SkillsUserShared
        })
        .into()
    }
    fn value(&self) -> &bool {
        &self.0
    }
}
pub(super) struct InstallState {
    pub(super) source: Entity<InputState>,
    reference: Entity<InputState>,
    subpath: Entity<InputState>,
    search: Entity<InputState>,
    generation: u64,
    pub(super) busy: bool,
    committing: bool,
    review_host: Option<lyrel_core::host::SharedHost>,
    pub(super) open: bool,
    project_scope: bool,
    scope: Entity<SelectState<Vec<ScopeChoice>>>,
    inputs: [String; 4],
    catalogue: Option<Arc<SkillCatalog>>,
    download: Option<Arc<dyn Transport>>,
    selected: BTreeSet<String>,
    hits: Vec<SkillSearchResult>,
    searched: bool,
    discovered: bool,
    preview: Option<InstallPreview>,
    decisions: Vec<InstallDecision>,
    messages: Vec<String>,
    error: Option<String>,
    github: Option<Arc<dyn Transport>>,
    registry: Option<Arc<dyn Transport>>,
}
impl InstallState {
    pub(super) fn new(
        subs: &mut Vec<Subscription>,
        window: &mut Window,
        cx: &mut Context<LyrelApp>,
    ) -> Self {
        let mut input = |key| {
            let entity = cx.new(|cx| InputState::new(window, cx).placeholder(t(key)));
            subs.push(cx.subscribe_in(&entity, window, |app, _, event, _, cx| {
                if matches!(event, InputEvent::Change) {
                    if let Some(s) = app.active_settings_mut() {
                        let state = &mut s.skills.install;
                        let inputs = state.input_values(cx);
                        if inputs != state.inputs {
                            state.inputs = inputs;
                            state.invalidate();
                            state.catalogue = None;
                            state.selected.clear();
                            state.hits.clear();
                            state.discovered = false;
                            state.searched = false;
                        }
                    }
                    cx.notify();
                }
            }));
            entity
        };
        let source = input(L10nKey::SkillsGithubHint);
        let reference = input(L10nKey::SkillsRefHint);
        let subpath = input(L10nKey::SkillsSubpathHint);
        let search = input(L10nKey::SkillsRegistryQuery);
        let scope = cx.new(|cx| {
            SelectState::new(
                vec![ScopeChoice(false), ScopeChoice(true)],
                None,
                window,
                cx,
            )
        });
        scope.update(cx, |state, cx| state.set_selected_value(&false, window, cx));
        subs.push(cx.subscribe_in(&scope, window, |app, _, event, _, cx| {
            if let SelectEvent::Confirm(Some(project)) = event {
                if let Some(s) = app.active_settings_mut() {
                    s.skills.install.project_scope = *project;
                    s.skills.install.invalidate();
                }
                cx.notify();
            }
        }));
        Self {
            source,
            reference,
            subpath,
            search,
            scope,
            inputs: Default::default(),
            generation: 0,
            busy: false,
            committing: false,
            review_host: None,
            open: false,
            project_scope: false,
            catalogue: None,
            download: None,
            selected: BTreeSet::new(),
            hits: Vec::new(),
            searched: false,
            discovered: false,
            preview: None,
            decisions: Vec::new(),
            messages: Vec::new(),
            error: None,
            github: None,
            registry: None,
        }
    }
    fn input_values(&self, cx: &App) -> [String; 4] {
        [&self.source, &self.reference, &self.subpath, &self.search]
            .map(|input| input.read(cx).value().to_string())
    }
    pub(super) fn invalidate(&mut self) {
        if self.committing {
            return;
        }
        self.generation += 1;
        self.busy = false;
        self.review_host = None;
        self.preview = None;
        self.decisions.clear();
        self.error = None;
        self.messages.clear();
    }
    pub(super) fn target_locked(&self) -> bool {
        self.committing || self.preview.is_some()
    }
    pub(super) fn cancel_review(&mut self) -> bool {
        if self.preview.take().is_none() {
            return false;
        }
        self.decisions.clear();
        self.review_host = None;
        true
    }
    pub(super) fn refresh_locale(&self, window: &mut Window, cx: &mut Context<LyrelApp>) {
        for (input, key) in [
            (&self.source, L10nKey::SkillsGithubHint),
            (&self.reference, L10nKey::SkillsRefHint),
            (&self.subpath, L10nKey::SkillsSubpathHint),
            (&self.search, L10nKey::SkillsRegistryQuery),
        ] {
            input.update(cx, |input, cx| input.set_placeholder(t(key), window, cx));
        }
    }
}
fn github_transport(
    injected: Option<Arc<dyn Transport>>,
    proxy: Option<&str>,
) -> Result<Arc<dyn Transport>, String> {
    if let Some(transport) = injected {
        return Ok(transport);
    }
    Ok(catalog::desktop_github_transport(proxy))
}
fn catalog_error(error: catalog::CatalogError) -> String {
    let key = match &error {
        catalog::CatalogError::Http(ApiError::Unauthorized) => {
            Some(L10nKey::SkillsGithubUnauthorized)
        }
        catalog::CatalogError::Http(ApiError::Forbidden(_)) => Some(L10nKey::SkillsGithubForbidden),
        catalog::CatalogError::Http(ApiError::NotFound) => Some(L10nKey::SkillsGithubInvisible),
        catalog::CatalogError::Http(ApiError::RateLimited { .. }) => {
            Some(L10nKey::SkillsGithubRateLimited)
        }
        catalog::CatalogError::Http(ApiError::Network(_)) => Some(L10nKey::SkillsGithubNetwork),
        _ => None,
    };
    key.map_or_else(|| error.to_string(), |key| t(key).to_string())
}
impl LyrelApp {
    pub(crate) fn cancel_skills_install(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(s) = self.active_settings_mut().filter(|s| {
            s.section == SettingsSection::Skills
                && s.menu.is_none()
                && s.skills.install.open
                && !s.skills.install.busy
        }) else {
            return false;
        };
        if s.skills.install.cancel_review() {
            window.focus(&s.skills.install.source.read(cx).focus_handle(cx), cx);
        } else {
            s.skills.install.open = false;
            s.skills.install.invalidate();
            window.focus(&s.skills.query.read(cx).focus_handle(cx), cx);
        }
        cx.notify();
        true
    }
    fn skills_install_current(
        &mut self,
        session: u64,
        generation: u64,
    ) -> Option<&mut InstallState> {
        self.active_settings_mut()
            .filter(|s| s.skills.session == session && s.skills.install.generation == generation)
            .map(|s| &mut s.skills.install)
    }
    fn discover_public_skills(&mut self, cx: &mut Context<Self>) {
        let proxy = cx.global::<Config>().http_proxy.clone();
        let Some(s) = self.active_settings_mut() else {
            return;
        };
        let state = &mut s.skills.install;
        if state.busy {
            return;
        }
        state.invalidate();
        state.inputs = state.input_values(cx);
        let reference = state.reference.read(cx).value().trim().to_string();
        let subpath = state.subpath.read(cx).value().trim().to_string();
        let source = GitHubSource::parse(&state.source.read(cx).value()).and_then(|source| {
            source.with_selection(
                (!reference.is_empty()).then_some(reference.as_str()),
                (!subpath.is_empty()).then_some(subpath.as_str()),
            )
        });
        let source = match source {
            Ok(source) => source,
            Err(error) => {
                state.error = Some(error.to_string());
                cx.notify();
                return;
            }
        };
        state.catalogue = None;
        state.download = None;
        state.selected.clear();
        state.discovered = false;
        state.busy = true;
        let (session, generation, injected) =
            (s.skills.session, state.generation, state.github.clone());
        let Some(host) = HostRegistry::get(cx, HostId::LOCAL) else {
            return;
        };
        HostOps::run(
            host,
            cx,
            move |_| {
                let transport = github_transport(injected, proxy.as_deref())?;
                catalog::discover(transport.as_ref(), &source)
                    .map(|catalogue| (Arc::new(catalogue), transport))
                    .map_err(catalog_error)
            },
            move |app, result, cx| {
                if let Some(state) = app.skills_install_current(session, generation) {
                    state.busy = false;
                    state.discovered = true;
                    match result {
                        Ok((catalogue, transport)) => {
                            state.catalogue = Some(catalogue);
                            state.download = Some(transport);
                        }
                        Err(error) => state.error = Some(error),
                    }
                    cx.notify();
                }
            },
        );
        cx.notify();
    }
    fn search_public_skills(&mut self, cx: &mut Context<Self>) {
        let proxy = cx.global::<Config>().http_proxy.clone();
        let Some(s) = self.active_settings_mut() else {
            return;
        };
        let state = &mut s.skills.install;
        if state.busy {
            return;
        }
        state.invalidate();
        state.hits.clear();
        state.searched = false;
        state.busy = true;
        state.inputs = state.input_values(cx);
        let (session, generation, query, injected) = (
            s.skills.session,
            state.generation,
            state.search.read(cx).value().to_string(),
            state.registry.clone(),
        );
        let Some(host) = HostRegistry::get(cx, HostId::LOCAL) else {
            return;
        };
        HostOps::run(
            host,
            cx,
            move |_| {
                let transport: Arc<dyn Transport> = match injected {
                    Some(t) => t,
                    None => Arc::new(catalog::SkillsSearchHttp::new(proxy.as_deref())),
                };
                catalog::search(transport.as_ref(), &query).map_err(|error| error.to_string())
            },
            move |app, result, cx| {
                if let Some(state) = app.skills_install_current(session, generation) {
                    state.busy = false;
                    state.searched = true;
                    match result {
                        Ok(hits) => state.hits = hits,
                        Err(error) => state.error = Some(error),
                    }
                    cx.notify();
                }
            },
        );
        cx.notify();
    }
    fn review_public_install(&mut self, cx: &mut Context<Self>) {
        let proxy = cx.global::<Config>().http_proxy.clone();
        let Some(s) = self.active_settings_mut() else {
            return;
        };
        let state = &mut s.skills.install;
        if state.busy || state.selected.is_empty() {
            return;
        }
        let Some(catalogue) = state.catalogue.clone() else {
            return;
        };
        let project = if state.project_scope {
            match s.skills.project.clone() {
                Some(p) => Some(p),
                None => return,
            }
        } else {
            None
        };
        let selected = state.selected.clone();
        let host_id = s.skills.host;
        state.invalidate();
        state.busy = true;
        let (session, generation, injected) = (
            s.skills.session,
            state.generation,
            state.download.clone().or_else(|| state.github.clone()),
        );
        let Some(host) = HostRegistry::get(cx, host_id).filter(|host| host.is_connected()) else {
            state.busy = false;
            state.error = Some(t(L10nKey::SkillsNoMachine).to_string());
            cx.notify();
            return;
        };
        let bound_host = host.clone();
        HostOps::run(
            host,
            cx,
            move |host| {
                let transport = github_transport(injected, proxy.as_deref())?;
                let indices = catalogue
                    .candidates()
                    .iter()
                    .enumerate()
                    .filter_map(|(ix, candidate)| {
                        selected.contains(&candidate.skill_path).then_some(ix)
                    })
                    .collect::<Vec<_>>();
                let skills = catalogue
                    .prepare_many(transport.as_ref(), &indices)
                    .map_err(catalog_error)?;
                let target = if let Some(project) = project {
                    InstallTarget::project(host, project)
                } else {
                    let environment = host.agent_environment().map_err(|e| e.to_string())?;
                    let keys = if host.separator() == '\\' {
                        ["USERPROFILE", "HOME"]
                    } else {
                        ["HOME", "USERPROFILE"]
                    };
                    let home = keys
                        .into_iter()
                        .find_map(|key| environment.get(key).filter(|s| !s.is_empty()))
                        .ok_or_else(|| {
                            "selected Host did not report HOME/USERPROFILE".to_string()
                        })?;
                    let state = environment
                        .get("XDG_STATE_HOME")
                        .filter(|s| !s.is_empty())
                        .map(PathBuf::from);
                    let cwd = environment
                        .get("PWD")
                        .filter(|cwd| !cwd.is_empty())
                        .map(PathBuf::from)
                        .or_else(|| {
                            (host.id() == HostId::LOCAL)
                                .then(|| std::env::current_dir().ok())
                                .flatten()
                        })
                        .unwrap_or_default();
                    InstallTarget::user(host, PathBuf::from(home), state, cwd)
                }
                .map_err(|e| e.to_string())?;
                install::preview_install(host, target, skills).map_err(|e| e.to_string())
            },
            move |app, result, cx| {
                if let Some(state) = app.skills_install_current(session, generation) {
                    state.busy = false;
                    match result {
                        Ok(preview) => {
                            state.decisions = preview
                                .items()
                                .iter()
                                .map(|item| match item.existing() {
                                    ExistingContent::Absent => InstallDecision::Install,
                                    _ => InstallDecision::Keep,
                                })
                                .collect();
                            state.preview = Some(preview);
                            state.review_host = Some(bound_host);
                        }
                        Err(error) => state.error = Some(error),
                    }
                    cx.notify();
                }
            },
        );
        cx.notify();
    }
    fn confirm_public_install(&mut self, cx: &mut Context<Self>) {
        let Some(s) = self.active_settings_mut() else {
            return;
        };
        let state = &mut s.skills.install;
        if state.busy {
            return;
        }
        let Some(preview) = state.preview.take() else {
            return;
        };
        let Some(host) = state.review_host.take() else {
            state.error = Some(t(L10nKey::SkillsNoMachine).to_string());
            cx.notify();
            return;
        };
        state.committing = true;
        state.busy = true;
        state.messages.clear();
        state.error = None;
        let (session, generation, decisions) = (
            s.skills.session,
            state.generation,
            std::mem::take(&mut state.decisions),
        );
        HostOps::run(
            host,
            cx,
            move |host| {
                install::apply_install(host, preview, &decisions, &chrono::Utc::now().to_rfc3339())
            },
            move |app, report, cx| {
                if let Some(state) = app.skills_install_current(session, generation) {
                    state.busy = false;
                    state.committing = false;
                    for item in report.items {
                        let path = item.destination.to_string_lossy();
                        state.messages.push(match item.status {
                            InstallStatus::Installed | InstallStatus::Replaced => t_fmt(
                                L10nKey::SkillsInstallDone,
                                &[("name", &item.name), ("path", &path)],
                            ),
                            InstallStatus::Kept => t_fmt(
                                L10nKey::SkillsInstallRetained,
                                &[("name", &item.name), ("path", &path)],
                            ),
                            InstallStatus::Failed(error) => t_fmt(
                                L10nKey::SkillsInstallError,
                                &[("name", &item.name), ("path", &path), ("error", &error)],
                            ),
                        });
                    }
                    state.messages.extend(report.warnings);
                    app.refresh_skills(cx);
                    cx.notify();
                }
            },
        );
        cx.notify();
    }
    pub(super) fn render_skills_install(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(s) = self.active_settings() else {
            return div().into_any_element();
        };
        let state = &s.skills.install;
        if !state.open {
            return Button::new("skills-install-open")
                .label(format!("{}…", t(L10nKey::SkillsInstallTitle)))
                .on_click(cx.listener(|app, _, window, cx| {
                    if let Some(s) = app.active_settings_mut() {
                        s.skills.install.open = true;
                        window.focus(&s.skills.install.source.read(cx).focus_handle(cx), cx);
                    }
                    cx.notify();
                }))
                .into_any_element();
        }
        let mut body = vec![
            div()
                .child(t(L10nKey::SkillsInstallTitle))
                .into_any_element(),
            div()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(t(L10nKey::SkillsInstallSafety))
                .into_any_element(),
            div()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(t(L10nKey::SkillsGithubAuthorization))
                .into_any_element(),
        ];
        body.push(
            Button::new("skills-install-browse")
                .label(t(L10nKey::SkillsInstallBrowse))
                .disabled(state.busy)
                .on_click(cx.listener(|app, _, window, cx| {
                    if let Some(s) = app.active_settings_mut() {
                        s.skills.install.open = false;
                        s.skills.install.invalidate();
                        window.focus(&s.skills.query.read(cx).focus_handle(cx), cx);
                    }
                    cx.notify();
                }))
                .into_any_element(),
        );
        if let Some(preview) = &state.preview {
            body.push(
                div()
                    .child(t(L10nKey::SkillsInstallDestination))
                    .into_any_element(),
            );
            body.push(
                div()
                    .text_sm()
                    .child(
                        preview
                            .target()
                            .shared_root()
                            .to_string_lossy()
                            .into_owned(),
                    )
                    .into_any_element(),
            );
            body.push(
                div()
                    .text_sm()
                    .child(preview.target().lock_path().to_string_lossy().into_owned())
                    .into_any_element(),
            );
            if let Some(catalogue) = &state.catalogue {
                body.push(
                    div()
                        .text_sm()
                        .child(t_fmt(
                            L10nKey::SkillsInstallRefResolved,
                            &[("source", catalogue.tree_sha())],
                        ))
                        .into_any_element(),
                );
            }
            for (ix, item) in preview.items().iter().enumerate() {
                let mut row = v_flex()
                    .gap_1()
                    .min_w_0()
                    .child(item.name().to_string())
                    .child(item.destination().to_string_lossy().into_owned())
                    .child(item.source().url());
                row = row.child(t_fmt(
                    L10nKey::SkillsInstallRefResolved,
                    &[(
                        "source",
                        &format!(
                            "{} · {}",
                            item.source().reference.as_deref().unwrap_or("HEAD"),
                            item.skill_path()
                        ),
                    )],
                ));
                if let ExistingContent::Directory { files, bytes } = item.existing() {
                    row = row.child(t_fmt(
                        L10nKey::SkillsCompanions,
                        &[
                            ("files", &files.to_string()),
                            ("bytes", &bytes.to_string()),
                            ("limited", ""),
                        ],
                    ));
                }
                if !matches!(item.existing(), ExistingContent::Absent) {
                    row = row.child(t(L10nKey::SkillsInstallKeep)).child(
                        Checkbox::new(SharedString::from(format!(
                            "skills-replace-{}",
                            item.name()
                        )))
                        .label(t(L10nKey::SkillsInstallReplace))
                        .checked(matches!(
                            state.decisions.get(ix),
                            Some(InstallDecision::Replace)
                        ))
                        .on_click(cx.listener(
                            move |app, checked, _, cx| {
                                if let Some(s) = app.active_settings_mut() {
                                    s.skills.install.decisions[ix] = if *checked {
                                        InstallDecision::Replace
                                    } else {
                                        InstallDecision::Keep
                                    };
                                }
                                cx.notify();
                            },
                        )),
                    );
                }
                body.push(row.into_any_element());
            }
            body.push(
                Button::new("skills-install-confirm")
                    .label(t(L10nKey::SkillsInstallConfirm))
                    .custom(crate::ui::theme::inverted_button(cx.theme().background, cx))
                    .on_click(cx.listener(|app, _, _, cx| app.confirm_public_install(cx)))
                    .into_any_element(),
            );
            body.push(
                Button::new("skills-install-cancel")
                    .label(t(L10nKey::SkillsInstallCancel))
                    .on_click(cx.listener(|app, _, window, cx| {
                        if let Some(s) = app.active_settings_mut() {
                            s.skills.install.cancel_review();
                            window.focus(&s.skills.install.source.read(cx).focus_handle(cx), cx);
                        }
                        cx.notify();
                    }))
                    .into_any_element(),
            );
        } else {
            for (label, input, id) in [
                (
                    L10nKey::SkillsGithubSource,
                    &state.source,
                    "skills-install-source",
                ),
                (L10nKey::SkillsRef, &state.reference, "skills-install-ref"),
                (
                    L10nKey::SkillsSubpath,
                    &state.subpath,
                    "skills-install-subpath",
                ),
            ] {
                body.push(
                    v_flex()
                        .gap_1()
                        .child(t(label))
                        .child(Input::new(input).id(id).disabled(state.busy))
                        .into_any_element(),
                );
            }
            body.push(
                Button::new("skills-discover")
                    .label(t(L10nKey::SkillsDiscover))
                    .disabled(state.busy || state.source.read(cx).value().trim().is_empty())
                    .on_click(cx.listener(|app, _, _, cx| app.discover_public_skills(cx)))
                    .into_any_element(),
            );
            body.push(
                v_flex()
                    .gap_1()
                    .child(t(L10nKey::SkillsInstallScope))
                    .child(
                        Select::new(&state.scope)
                            .id("skills-install-scope")
                            .disabled(state.busy)
                            .accessibility_label(t(L10nKey::SkillsInstallScope)),
                    )
                    .into_any_element(),
            );
            if state.project_scope && s.skills.project.is_none() {
                body.push(
                    div()
                        .text_sm()
                        .text_color(cx.theme().warning)
                        .child(t(L10nKey::SkillsProjectHint))
                        .into_any_element(),
                );
            }
            if let Some(catalogue) = &state.catalogue {
                body.push(
                    div()
                        .text_sm()
                        .child(t_fmt(
                            L10nKey::SkillsInstallRefResolved,
                            &[("source", catalogue.tree_sha())],
                        ))
                        .into_any_element(),
                );
                body.extend(catalogue.problems().iter().map(|problem| {
                    div()
                        .text_sm()
                        .text_color(cx.theme().warning)
                        .child(problem.clone())
                        .into_any_element()
                }));
                body.push(
                    div()
                        .child(t_fmt(
                            L10nKey::SkillsInstallCandidates,
                            &[("count", &catalogue.candidates().len().to_string())],
                        ))
                        .into_any_element(),
                );
                body.push(
                    div()
                        .text_sm()
                        .child(t(L10nKey::SkillsInstallSelect))
                        .into_any_element(),
                );
                for candidate in catalogue.candidates() {
                    let path = candidate.skill_path.clone();
                    body.push(
                        v_flex()
                            .gap_1()
                            .child(
                                Checkbox::new(SharedString::from(format!(
                                    "skills-candidate-{path}"
                                )))
                                .label(candidate.name.clone())
                                .checked(state.selected.contains(&path))
                                .disabled(state.busy)
                                .on_click(cx.listener(
                                    move |app, checked, _, cx| {
                                        if let Some(s) = app.active_settings_mut() {
                                            if *checked {
                                                s.skills.install.selected.insert(path.clone());
                                            } else {
                                                s.skills.install.selected.remove(&path);
                                            }
                                        }
                                        cx.notify();
                                    },
                                )),
                            )
                            .child(candidate.description.clone())
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(candidate.skill_path.clone()),
                            )
                            .into_any_element(),
                    );
                }
                body.push(
                    Button::new("skills-install-review")
                        .label(t(L10nKey::SkillsInstallReview))
                        .disabled(
                            state.busy
                                || state.selected.is_empty()
                                || (state.project_scope && s.skills.project.is_none()),
                        )
                        .on_click(cx.listener(|app, _, _, cx| app.review_public_install(cx)))
                        .into_any_element(),
                );
            }
            body.push(
                v_flex()
                    .gap_1()
                    .child(t(L10nKey::SkillsRegistryQuery))
                    .child(
                        Input::new(&state.search)
                            .id("skills-registry-query")
                            .disabled(state.busy),
                    )
                    .child(
                        Button::new("skills-registry-search")
                            .label(t(L10nKey::SkillsRegistrySearch))
                            .disabled(
                                state.busy
                                    || state.search.read(cx).value().trim().chars().count() < 2,
                            )
                            .on_click(cx.listener(|app, _, _, cx| app.search_public_skills(cx))),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(t(L10nKey::SkillsRegistryNote)),
                    )
                    .into_any_element(),
            );
            for hit in &state.hits {
                let source = format!("{}@{}", hit.source.repository(), hit.skill_id);
                body.push(
                    Button::new(SharedString::from(format!("skills-registry-{}", hit.id)))
                        .ghost()
                        .label(format!("{} — {}", hit.name, hit.source.repository()))
                        .disabled(state.busy)
                        .on_click(cx.listener(move |app, _, window, cx| {
                            if let Some(s) = app.active_settings_mut() {
                                let state = &mut s.skills.install;
                                state.source.update(cx, |input, cx| {
                                    input.set_value(source.clone(), window, cx)
                                });
                                state
                                    .reference
                                    .update(cx, |input, cx| input.set_value("", window, cx));
                                state
                                    .subpath
                                    .update(cx, |input, cx| input.set_value("", window, cx));
                            }
                            cx.notify();
                        }))
                        .into_any_element(),
                );
            }
            if (state.searched && state.hits.is_empty()
                || state.discovered
                    && state
                        .catalogue
                        .as_ref()
                        .is_some_and(|c| c.candidates().is_empty()))
                && state.error.is_none()
            {
                body.push(
                    div()
                        .child(t(L10nKey::SkillsInstallEmpty))
                        .into_any_element(),
                );
            }
        }
        if state.busy {
            body.push(
                div()
                    .child(t(L10nKey::SkillsInstallWorking))
                    .into_any_element(),
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
        v_flex().gap_3().min_w_0().children(body).into_any_element()
    }
}
