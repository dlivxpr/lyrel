use super::*;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{TestAppContext, VisualTestContext};
use lyrel_core::core::github::{ApiError, Reply, Transport};

struct PublicRepository;
impl Transport for PublicRepository {
    fn get(&self, path: &str) -> Result<Reply, ApiError> {
        let value = if path.contains("/git/trees/") {
            serde_json::json!({"sha":"1111111111111111111111111111111111111111","truncated":false,"tree":[
                {"path":"skills","mode":"040000","type":"tree","sha":"2222222222222222222222222222222222222222"},
                {"path":"skills/needle","mode":"040000","type":"tree","sha":"3333333333333333333333333333333333333333"},
                {"path":"skills/needle/SKILL.md","mode":"100644","type":"blob","size":82,"sha":"4444444444444444444444444444444444444444"}
            ]})
        } else if path.contains("/git/blobs/") {
            serde_json::json!({"sha":"4444444444444444444444444444444444444444","size":82,"encoding":"base64","content":"LS0tCm5hbWU6IG5lZWRsZQpkZXNjcmlwdGlvbjogUHVibGljIHJlcG9zaXRvcnkgcmVmZXJlbmNlCi0tLQojIEluc3RhbGxlZCBjb250ZW50Cg=="})
        } else if path.starts_with("/api/search?") {
            serde_json::json!({"skills":[{"id":"example/skills/needle","source":"example/skills","name":"needle","skillId":"needle","installs":20}]})
        } else {
            return Err(ApiError::NotFound);
        };
        Ok(Reply {
            body: serde_json::to_vec(&value).unwrap(),
            has_next: false,
        })
    }
    fn authenticated(&self) -> bool {
        false
    }
}
fn settle_install(app: &Entity<LyrelApp>, cx: &mut VisualTestContext) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    loop {
        cx.run_until_parked();
        if app.update_in(cx, |app, _, _| {
            app.active_settings()
                .is_some_and(|s| !s.skills.install.busy && !s.skills.loading)
        }) {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "installation did not settle"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}
fn click_form(window: &mut Window, cx: &mut App, id: &'static str) {
    super::super::tests::reveal(window, cx, id, "skills-modal-scroll");
    window.click(id, cx);
}
#[gpui_kit::test]
fn public_install_review_cancel_and_confirm_write_only_explicit_project(cx: &mut TestAppContext) {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let (app, mut vcx) = crate::ui::app::test_window::harness(cx);
    vcx.simulate_resize(gpui_kit::size(px(1200.), px(1800.)));
    app.update_in(&mut vcx, |app, window, cx| {
        super::super::tests::isolate_native_roots(&temp.path().join("home"), cx);
        app.open_settings_section(SettingsSection::Skills, window, cx);
        let state = &mut app.active_settings_mut().unwrap().skills.install;
        state.github = Some(Arc::new(PublicRepository));
        state.registry = Some(Arc::new(PublicRepository));
    });
    settle_install(&app, &mut vcx);
    vcx.update(|window, cx| {
        window.render_frame(cx);
        window.click("skills-project-input", cx);
        window.input(project.to_string_lossy().as_ref(), cx);
        window.click("skills-apply-project", cx);
    });
    settle_install(&app, &mut vcx);
    vcx.update(|window, cx| {
        window.render_frame(cx);
        window.click("skills-install-open", cx);
    });
    vcx.update(|window, cx| {
        window.render_frame(cx);
        for id in [
            "skills-install-source",
            "skills-install-ref",
            "skills-install-subpath",
            "skills-registry-query",
        ] {
            assert!(window.find(id).label().is_some_and(|name| !name.is_empty()));
        }
        click_form(window, cx, "skills-install-scope");
        window.press("down", cx);
        window.press("enter", cx);
    });
    vcx.update(|window, cx| {
        window.render_frame(cx);
        click_form(window, cx, "skills-install-source");
        window.input("example/skills", cx);
        click_form(window, cx, "skills-discover");
    });
    settle_install(&app, &mut vcx);
    vcx.update(|window, cx| {
        window.render_frame(cx);
        click_form(window, cx, "skills-candidate-skills/needle/SKILL.md");
    });
    vcx.run_until_parked();
    vcx.update(|window, cx| {
        window.click("skills-install-review", cx);
    });
    settle_install(&app, &mut vcx);
    vcx.update(|window, cx| {
        window.render_frame(cx);
        window.click("skills-install-cancel", cx);
    });
    vcx.run_until_parked();
    assert!(!project.join(".agents").exists());
    assert!(!project.join("skills-lock.json").exists());
    vcx.update(|window, cx| {
        window.render_frame(cx);
        window.click("skills-install-review", cx);
    });
    settle_install(&app, &mut vcx);
    vcx.update(|window, cx| {
        window.render_frame(cx);
        window.click("skills-install-confirm", cx);
    });
    settle_install(&app, &mut vcx);
    let document = project.join(".agents/skills/needle/SKILL.md");
    assert_eq!(
        std::fs::read_to_string(&document).unwrap(),
        "---\nname: needle\ndescription: Public repository reference\n---\n# Installed content\n"
    );
    let lock: serde_json::Value =
        serde_json::from_slice(&std::fs::read(project.join("skills-lock.json")).unwrap()).unwrap();
    assert_eq!(lock["version"], 1);
    assert_eq!(lock["skills"]["needle"]["source"], "example/skills");
    app.update_in(&mut vcx, |app, _, _| {
        let skills = &app.active_settings().unwrap().skills;
        let installed = skills
            .snapshot
            .as_ref()
            .unwrap()
            .installations()
            .iter()
            .find(|s| s.name() == "needle")
            .unwrap();
        assert_eq!(
            installed.source_record().unwrap().source(),
            "example/skills"
        );
    });
    let local_edit =
        "---\nname: needle\ndescription: User edited content\n---\n# Keep my changes\n";
    std::fs::write(&document, local_edit).unwrap();
    let lock_before = std::fs::read(project.join("skills-lock.json")).unwrap();
    vcx.update(|window, cx| {
        window.render_frame(cx);
        window.click("skills-install-review", cx);
    });
    settle_install(&app, &mut vcx);
    vcx.update(|window, cx| {
        window.render_frame(cx);
        window.click("skills-install-confirm", cx);
    });
    settle_install(&app, &mut vcx);
    assert_eq!(std::fs::read_to_string(&document).unwrap(), local_edit);
    assert_eq!(
        std::fs::read(project.join("skills-lock.json")).unwrap(),
        lock_before
    );
    vcx.update(|window, cx| {
        window.render_frame(cx);
        window.click("skills-install-review", cx);
    });
    settle_install(&app, &mut vcx);
    vcx.update(|window, cx| {
        window.render_frame(cx);
        click_form(window, cx, "skills-replace-needle");
        window.press("escape", cx);
    });
    assert_eq!(std::fs::read_to_string(&document).unwrap(), local_edit);
    assert_eq!(
        std::fs::read(project.join("skills-lock.json")).unwrap(),
        lock_before
    );
    vcx.update(|window, cx| {
        window.render_frame(cx);
        window.click("skills-install-review", cx);
    });
    settle_install(&app, &mut vcx);
    vcx.update(|window, cx| {
        window.render_frame(cx);
        click_form(window, cx, "skills-replace-needle");
        window.click("skills-install-confirm", cx);
    });
    settle_install(&app, &mut vcx);
    assert_eq!(
        std::fs::read_to_string(&document).unwrap(),
        "---\nname: needle\ndescription: Public repository reference\n---\n# Installed content\n"
    );
    super::super::maintenance::tests::lifecycle(&app, &mut vcx, &project);
}

#[gpui_kit::test]
fn search_results_require_discovery_and_never_install_on_selection(cx: &mut TestAppContext) {
    let temp = tempfile::tempdir().unwrap();
    let (app, mut vcx) = crate::ui::app::test_window::harness(cx);
    vcx.simulate_resize(gpui_kit::size(px(1200.), px(1800.)));
    app.update_in(&mut vcx, |app, window, cx| {
        super::super::tests::isolate_native_roots(&temp.path().join("home"), cx);
        app.open_settings_section(SettingsSection::Skills, window, cx);
        let state = &mut app.active_settings_mut().unwrap().skills.install;
        state.github = Some(Arc::new(PublicRepository));
        state.registry = Some(Arc::new(PublicRepository));
    });
    settle_install(&app, &mut vcx);
    vcx.update(|window, cx| {
        window.render_frame(cx);
        window.click("skills-install-open", cx);
        click_form(window, cx, "skills-registry-query");
        window.input("needle", cx);
        click_form(window, cx, "skills-registry-search");
    });
    settle_install(&app, &mut vcx);
    vcx.update(|window, cx| {
        window.render_frame(cx);
        click_form(window, cx, "skills-registry-example/skills/needle");
    });
    app.update_in(&mut vcx, |app, _, cx| {
        let state = &app.active_settings().unwrap().skills.install;
        assert_eq!(
            state.source.read(cx).value().as_ref(),
            "example/skills@needle"
        );
        assert!(state.preview.is_none());
        assert!(state.catalogue.is_none());
        assert!(!state.busy);
    });
    assert!(!temp.path().join("home/.agents").exists());
    vcx.update(|window, cx| {
        window.render_frame(cx);
        click_form(window, cx, "skills-discover");
    });
    settle_install(&app, &mut vcx);
    app.update_in(&mut vcx, |app, _, _| {
        let state = &app.active_settings().unwrap().skills.install;
        assert_eq!(
            state.catalogue.as_ref().unwrap().candidates()[0].name,
            "needle"
        );
        assert!(state.selected.is_empty());
        assert!(state.preview.is_none());
    });
}

struct SearchUnavailable;
impl Transport for SearchUnavailable {
    fn get(&self, _: &str) -> Result<Reply, ApiError> {
        Err(ApiError::RateLimited { reset: Some(42) })
    }
    fn authenticated(&self) -> bool {
        false
    }
}
#[gpui_kit::test]
fn registry_failure_stays_visible_and_does_not_create_candidates(cx: &mut TestAppContext) {
    let temp = tempfile::tempdir().unwrap();
    let (app, mut vcx) = crate::ui::app::test_window::harness(cx);
    vcx.simulate_resize(gpui_kit::size(px(1200.), px(1800.)));
    app.update_in(&mut vcx, |app, window, cx| {
        super::super::tests::isolate_native_roots(&temp.path().join("home"), cx);
        app.open_settings_section(SettingsSection::Skills, window, cx);
        app.active_settings_mut().unwrap().skills.install.registry =
            Some(Arc::new(SearchUnavailable));
    });
    settle_install(&app, &mut vcx);
    vcx.update(|window, cx| {
        window.render_frame(cx);
        window.click("skills-install-open", cx);
        click_form(window, cx, "skills-registry-query");
        window.input("pdf", cx);
        click_form(window, cx, "skills-registry-search");
    });
    settle_install(&app, &mut vcx);
    app.update_in(&mut vcx, |app, _, _| {
        let state = &app.active_settings().unwrap().skills.install;
        assert!(state.error.as_ref().unwrap().contains("rate limit"));
        assert!(state.hits.is_empty());
        assert!(state.preview.is_none());
    });
    assert!(!temp.path().join("home/.agents").exists());
}

#[gpui_kit::test]
fn installer_escape_returns_to_browsing_without_writes_in_a_narrow_window(cx: &mut TestAppContext) {
    let temp = tempfile::tempdir().unwrap();
    let (app, mut vcx) = crate::ui::app::test_window::harness(cx);
    app.update_in(&mut vcx, |app, window, cx| {
        super::super::tests::isolate_native_roots(&temp.path().join("home"), cx);
        app.open_settings_section(SettingsSection::Skills, window, cx);
    });
    settle_install(&app, &mut vcx);
    vcx.simulate_resize(gpui_kit::size(px(720.), px(560.)));
    vcx.update(|window, cx| {
        window.render_frame(cx);
        for _ in 0..20 {
            if window.find("skills-install-open").focused() == Some(true) {
                break;
            }
            window.press("tab", cx);
        }
        assert_eq!(window.find("skills-install-open").focused(), Some(true));
        window.press("enter", cx);
        for _ in 0..24 {
            window.press("tab", cx);
            assert!(
                app.read(cx)
                    .active_settings()
                    .unwrap()
                    .skills
                    .modal_focus
                    .contains_focused(window, cx)
            );
        }
        window.press("escape", cx);
        assert_eq!(window.find("skills-install-open").focused(), Some(true));
    });
    app.update_in(&mut vcx, |app, _, _| {
        assert!(!app.active_settings().unwrap().skills.install.open);
        assert!(app.has_settings());
    });
    assert!(!temp.path().join("home/.agents").exists());
}

struct DelayedRepository {
    first: std::sync::atomic::AtomicBool,
    entered: std::sync::mpsc::Sender<()>,
    release: std::sync::Mutex<std::sync::mpsc::Receiver<()>>,
}
impl Transport for DelayedRepository {
    fn get(&self, path: &str) -> Result<Reply, ApiError> {
        if self.first.swap(false, std::sync::atomic::Ordering::SeqCst) {
            self.entered.send(()).unwrap();
            self.release
                .lock()
                .unwrap()
                .recv_timeout(std::time::Duration::from_secs(10))
                .map_err(|e| ApiError::Network(e.to_string()))?;
        }
        PublicRepository.get(path)
    }
    fn authenticated(&self) -> bool {
        false
    }
}
#[gpui_kit::test]
fn late_repository_response_cannot_replace_new_source_or_explicit_project(cx: &mut TestAppContext) {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("new-project");
    std::fs::create_dir(&project).unwrap();
    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let (app, mut vcx) = crate::ui::app::test_window::harness(cx);
    vcx.simulate_resize(gpui_kit::size(px(1200.), px(1800.)));
    app.update_in(&mut vcx, |app, window, cx| {
        super::super::tests::isolate_native_roots(&temp.path().join("home"), cx);
        app.open_settings_section(SettingsSection::Skills, window, cx);
        app.active_settings_mut().unwrap().skills.install.github =
            Some(Arc::new(DelayedRepository {
                first: std::sync::atomic::AtomicBool::new(true),
                entered: entered_tx,
                release: std::sync::Mutex::new(release_rx),
            }));
    });
    settle_install(&app, &mut vcx);
    vcx.update(|window, cx| {
        window.render_frame(cx);
        window.click("skills-install-open", cx);
        click_form(window, cx, "skills-install-source");
        window.input("old/skills", cx);
        click_form(window, cx, "skills-discover");
    });
    vcx.run_until_parked();
    entered_rx
        .recv_timeout(std::time::Duration::from_secs(5))
        .unwrap();
    vcx.update(|window, cx| {
        window.render_frame(cx);
        window.press("escape", cx);
        window.click("skills-project-input", cx);
        window.input(project.to_string_lossy().as_ref(), cx);
        window.click("skills-apply-project", cx);
        window.click("skills-install-open", cx);
        click_form(window, cx, "skills-install-source");
        window.press("secondary-a", cx);
        window.input("example/skills", cx);
        click_form(window, cx, "skills-discover");
    });
    release_tx.send(()).unwrap();
    settle_install(&app, &mut vcx);
    app.update_in(&mut vcx, |app, _, _| {
        let skills = &app.active_settings().unwrap().skills;
        assert_eq!(skills.project.as_deref(), Some(project.as_path()));
        assert_eq!(
            skills.install.catalogue.as_ref().unwrap().candidates()[0]
                .source
                .repository(),
            "example/skills"
        );
        assert!(skills.install.preview.is_none());
    });
    assert!(!project.join(".agents").exists());
}

#[gpui_kit::test]
fn remote_install_review_freezes_host_home_and_source_without_touching_local_twin(
    cx: &mut TestAppContext,
) {
    let temp = tempfile::tempdir().unwrap();
    let local = temp.path().join("local/home");
    let remote = temp.path().join("remote/home");
    std::fs::create_dir_all(&remote).unwrap();
    let remote_id = HostId(404);
    let (app, mut vcx) = crate::ui::app::test_window::harness(cx);
    vcx.simulate_resize(gpui_kit::size(px(1200.), px(1800.)));
    app.update_in(&mut vcx, |app, window, cx| {
        super::super::tests::isolate_native_roots(&local, cx);
        std::fs::create_dir(remote.join("cwd")).unwrap();
        super::super::test_host::install_on(&remote, remote_id, true, cx);
        app.open_settings_section(SettingsSection::Skills, window, cx);
        app.select_skills_host(remote_id, window, cx);
        app.active_settings_mut().unwrap().skills.install.github = Some(Arc::new(PublicRepository));
    });
    settle_install(&app, &mut vcx);
    vcx.update(|window, cx| {
        window.render_frame(cx);
        window.click("skills-install-open", cx);
        click_form(window, cx, "skills-install-source");
        window.input("example/skills", cx);
        click_form(window, cx, "skills-discover");
    });
    settle_install(&app, &mut vcx);
    vcx.update(|window, cx| {
        window.render_frame(cx);
        click_form(window, cx, "skills-candidate-skills/needle/SKILL.md");
    });
    vcx.run_until_parked();
    vcx.update(|window, cx| {
        window.click("skills-install-review", cx);
    });
    settle_install(&app, &mut vcx);
    vcx.update(|window, cx| {
        window.render_frame(cx);
        window.press("escape", cx);
    });
    vcx.run_until_parked();
    app.read_with(&vcx, |app, _| {
        let state = &app.active_settings().unwrap().skills.install;
        assert!(
            state.open,
            "Escape from review must keep the installer open"
        );
        assert!(state.preview.is_none(), "Escape must cancel the review");
    });
    assert!(!remote.join(".agents").exists());
    assert!(!local.join(".agents").exists());
    vcx.update(|window, cx| {
        window.render_frame(cx);
        window.click("skills-install-review", cx);
    });
    settle_install(&app, &mut vcx);
    app.update_in(&mut vcx, |app, window, cx| {
        app.select_skills_host(HostId::LOCAL, window, cx);
        let state = &app.active_settings().unwrap().skills;
        assert_eq!(state.host, remote_id);
        assert_eq!(
            state.install.preview.as_ref().unwrap().target().anchor(),
            remote.as_path()
        );
    });
    vcx.update(|window, cx| {
        window.render_frame(cx);
        window.click("skills-install-confirm", cx);
    });
    settle_install(&app, &mut vcx);
    assert_eq!(
        std::fs::read_to_string(remote.join(".agents/skills/needle/SKILL.md")).unwrap(),
        "---\nname: needle\ndescription: Public repository reference\n---\n# Installed content\n"
    );
    let lock: serde_json::Value = serde_json::from_slice(
        &std::fs::read(remote.join("cwd/.state/skills/.skill-lock.json")).unwrap(),
    )
    .unwrap();
    // 此 fixture 由 LocalHost 承载；安装身份使用它规范化后的真实路径，
    // 不能与临时目录的 Windows 短路径或 macOS 符号链接拼写直接比较。
    let installed_path = lyrel_core::core::path_spelling::local_spelling_buf(
        std::fs::canonicalize(remote.join(".agents/skills/needle")).unwrap(),
    );
    app.update_in(&mut vcx, |app, _, _| {
        let snapshot = app
            .active_settings()
            .unwrap()
            .skills
            .snapshot
            .as_ref()
            .unwrap();
        assert_eq!(snapshot.host(), remote_id);
        let installed = snapshot
            .installations()
            .iter()
            .find(|skill| skill.path() == installed_path)
            .expect("the remote snapshot must contain the canonical shared installation");
        assert_eq!(installed.host(), remote_id);
        assert_eq!(
            installed.source_record().unwrap().source(),
            "example/skills"
        );
    });
    assert_eq!(lock["skills"]["needle"]["source"], "example/skills");
    assert_eq!(
        lock["skills"]["needle"]["skillFolderHash"],
        "3333333333333333333333333333333333333333"
    );
    assert!(!local.join(".agents").exists());
    assert!(!local.join(".state").exists());
}

struct RepositoryFailure(ApiError);
impl Transport for RepositoryFailure {
    fn get(&self, _: &str) -> Result<Reply, ApiError> {
        Err(self.0.clone())
    }
    fn authenticated(&self) -> bool {
        true
    }
}
#[gpui_kit::test]
fn github_failures_explain_authorization_visibility_limits_and_network_without_writes(
    cx: &mut TestAppContext,
) {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let (app, mut vcx) = crate::ui::app::test_window::harness(cx);
    vcx.simulate_resize(gpui_kit::size(px(1200.), px(1800.)));
    app.update_in(&mut vcx, |app, window, cx| {
        super::super::tests::isolate_native_roots(&home, cx);
        app.open_settings_section(SettingsSection::Skills, window, cx);
    });
    settle_install(&app, &mut vcx);
    vcx.update(|window, cx| {
        window.render_frame(cx);
        window.click("skills-install-open", cx);
        click_form(window, cx, "skills-install-source");
        window.input("example/private-skills", cx);
    });
    for (error, key) in [
        (ApiError::Unauthorized, L10nKey::SkillsGithubUnauthorized),
        (
            ApiError::Forbidden("SSO required".into()),
            L10nKey::SkillsGithubForbidden,
        ),
        (ApiError::NotFound, L10nKey::SkillsGithubInvisible),
        (
            ApiError::RateLimited { reset: Some(42) },
            L10nKey::SkillsGithubRateLimited,
        ),
        (
            ApiError::Network("offline".into()),
            L10nKey::SkillsGithubNetwork,
        ),
    ] {
        app.update_in(&mut vcx, |app, _, _| {
            app.active_settings_mut().unwrap().skills.install.github =
                Some(Arc::new(RepositoryFailure(error)));
        });
        vcx.update(|window, cx| {
            window.render_frame(cx);
            click_form(window, cx, "skills-discover");
        });
        settle_install(&app, &mut vcx);
        app.update_in(&mut vcx, |app, _, _| {
            let state = &app.active_settings().unwrap().skills.install;
            assert_eq!(state.error.as_deref(), Some(t(key)));
            assert!(state.preview.is_none());
            assert!(state.catalogue.is_none());
        });
        assert!(!home.join(".agents").exists());
    }
}
