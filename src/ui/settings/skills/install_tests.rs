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
        } else { return Err(ApiError::NotFound); };
        Ok(Reply { body: serde_json::to_vec(&value).unwrap(), has_next: false })
    }
    fn authenticated(&self) -> bool { false }
}
fn settle_install(app: &Entity<LyrelApp>, cx: &mut VisualTestContext) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    loop {
        cx.background_executor.run_until_parked();
        if app.update_in(cx, |app, _, _| app.active_settings().is_some_and(|s| !s.skills.install.busy && !s.skills.loading)) { return; }
        assert!(std::time::Instant::now() < deadline, "installation did not settle");
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
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
        window.click("skills-install-scope-trigger", cx);
        window.render_frame(cx);
        window.click("skills-install-scope-item-1", cx);
    });
    vcx.update(|window, cx| {
        window.render_frame(cx);
        window.click("skills-install-source", cx);
        window.input("example/skills", cx);
        window.click("skills-discover", cx);
    });
    settle_install(&app, &mut vcx);
    vcx.update(|window, cx| {
        window.render_frame(cx);
        window.click("skills-candidate-skills/needle/SKILL.md", cx);
        window.click("skills-install-review", cx);
    });
    settle_install(&app, &mut vcx);
    vcx.update(|window, cx| {
        window.render_frame(cx);
        window.click("skills-install-cancel", cx);
    });
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
    assert_eq!(std::fs::read_to_string(&document).unwrap(), "---\nname: needle\ndescription: Public repository reference\n---\n# Installed content\n");
    let lock: serde_json::Value = serde_json::from_slice(&std::fs::read(project.join("skills-lock.json")).unwrap()).unwrap();
    assert_eq!(lock["version"], 1);
    assert_eq!(lock["skills"]["needle"]["source"], "example/skills");
    app.update_in(&mut vcx, |app, _, _| {
        let skills = &app.active_settings().unwrap().skills;
        let installed = skills.snapshot.as_ref().unwrap().installations().iter().find(|s| s.name() == "needle").unwrap();
        assert_eq!(installed.source_record().unwrap().source(), "example/skills");
    });
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
    vcx.update(|window,cx| {
        window.render_frame(cx);
        window.click("skills-install-open",cx);
        window.click("skills-registry-query",cx);
        window.input("needle",cx);
        window.click("skills-registry-search",cx);
    });
    settle_install(&app, &mut vcx);
    vcx.update(|window,cx| {
        window.render_frame(cx);
        window.click("skills-registry-example/skills/needle",cx);
    });
    app.update_in(&mut vcx, |app,_,cx| {
        let state=&app.active_settings().unwrap().skills.install;
        assert_eq!(state.source.read(cx).value().as_ref(),"example/skills@needle");
        assert!(state.preview.is_none());
        assert!(state.catalogue.is_none());
        assert!(!state.busy);
    });
    assert!(!temp.path().join("home/.agents").exists());
    vcx.update(|window,cx| {window.render_frame(cx);window.click("skills-discover",cx);});
    settle_install(&app,&mut vcx);
    app.update_in(&mut vcx,|app,_,_| {
        let state=&app.active_settings().unwrap().skills.install;
        assert_eq!(state.catalogue.as_ref().unwrap().candidates()[0].name,"needle");
        assert!(state.selected.is_empty());
        assert!(state.preview.is_none());
    });
}

struct SearchUnavailable;
impl Transport for SearchUnavailable {
    fn get(&self,_:&str)->Result<Reply,ApiError>{Err(ApiError::RateLimited{reset:Some(42)})}
    fn authenticated(&self)->bool{false}
}
#[gpui_kit::test]
fn registry_failure_stays_visible_and_does_not_create_candidates(cx:&mut TestAppContext) {
    let temp=tempfile::tempdir().unwrap();
    let (app,mut vcx)=crate::ui::app::test_window::harness(cx);
    vcx.simulate_resize(gpui_kit::size(px(1200.), px(1800.)));
    app.update_in(&mut vcx,|app,window,cx|{
        super::super::tests::isolate_native_roots(&temp.path().join("home"),cx);
        app.open_settings_section(SettingsSection::Skills,window,cx);
        app.active_settings_mut().unwrap().skills.install.registry=Some(Arc::new(SearchUnavailable));
    });
    settle_install(&app,&mut vcx);
    vcx.update(|window,cx|{window.render_frame(cx);window.click("skills-install-open",cx);window.click("skills-registry-query",cx);window.input("pdf",cx);window.click("skills-registry-search",cx);});
    settle_install(&app,&mut vcx);
    app.update_in(&mut vcx,|app,_,_|{
        let state=&app.active_settings().unwrap().skills.install;
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
            if window.find("skills-install-open").focused() == Some(true) { break; }
            window.press("tab", cx);
        }
        assert_eq!(window.find("skills-install-open").focused(), Some(true));
        window.press("enter", cx);
        window.press("escape", cx);
    });
    app.update_in(&mut vcx, |app, _, _| {
        assert!(!app.active_settings().unwrap().skills.install.open);
        assert!(app.has_settings());
    });
    assert!(!temp.path().join("home/.agents").exists());
}
