use super::*;
use gpui_kit::VisualTestContext;
use gpui_kit::test::TestWindowExt;
use lyrel_core::core::github::{ApiError, Reply, Transport};

struct UpdatedRepository;
impl Transport for UpdatedRepository {
    fn get(&self, path: &str) -> Result<Reply, ApiError> {
        let body = if path.contains("/git/trees/") {
            serde_json::json!({"sha":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","truncated":false,"tree":[
                {"path":"skills","mode":"040000","type":"tree","sha":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"},
                {"path":"skills/needle","mode":"040000","type":"tree","sha":"cccccccccccccccccccccccccccccccccccccccc"},
                {"path":"skills/needle/SKILL.md","mode":"100644","type":"blob","size":90,"sha":"dddddddddddddddddddddddddddddddddddddddd"}
            ]})
        } else if path.contains("/git/blobs/") {
            let text = "---\nname: needle\ndescription: Updated repository reference\n---\n# Updated upstream content\n";
            serde_json::json!({"sha":"dddddddddddddddddddddddddddddddddddddddd","size":text.len(),"encoding":"base64","content":"LS0tCm5hbWU6IG5lZWRsZQpkZXNjcmlwdGlvbjogVXBkYXRlZCByZXBvc2l0b3J5IHJlZmVyZW5jZQotLS0KIyBVcGRhdGVkIHVwc3RyZWFtIGNvbnRlbnQK"})
        } else {
            return Err(ApiError::NotFound);
        };
        Ok(Reply {
            body: serde_json::to_vec(&body).unwrap(),
            has_next: false,
        })
    }
    fn authenticated(&self) -> bool {
        false
    }
}
fn settle(app: &Entity<LyrelApp>, cx: &mut VisualTestContext) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    loop {
        cx.background_executor.run_until_parked();
        if app.update_in(cx, |app, _, _| {
            app.active_settings()
                .is_some_and(|s| !s.skills.maintenance.busy && !s.skills.loading)
        }) {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "maintenance did not settle"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}
/// Continues the real install workflow with actual user edits and cancellation.
pub(crate) fn lifecycle(app: &Entity<LyrelApp>, cx: &mut VisualTestContext, project: &Path) {
    let installation = project.join(".agents/skills/needle");
    let document = installation.join("SKILL.md");
    let lock = project.join("skills-lock.json");
    let original_lock = std::fs::read(&lock).unwrap();
    let local = "---\nname: needle\ndescription: My local edit\n---\n# Never silently replace\n";
    std::fs::write(&document, local).unwrap();
    let unrelated = project.join("unrelated.txt");
    std::fs::write(&unrelated, "keep unrelated").unwrap();
    cx.update(|window, cx| {
        window.render_frame(cx);
        window.click("skills-install-browse", cx);
    });
    app.update_in(cx, |app, _, cx| {
        app.active_settings_mut().unwrap().skills.maintenance.github =
            Some(Arc::new(UpdatedRepository));
        app.refresh_skills(cx);
    });
    settle(app, cx);
    let selected = app.update_in(cx, |app, _, _| {
        app.active_settings()
            .unwrap()
            .skills
            .snapshot
            .as_ref()
            .unwrap()
            .filtered("needle", Some(SourceKind::ProjectShared))
            .next()
            .unwrap()
            .path()
            .to_path_buf()
    });
    cx.update(|window, cx| {
        window.render_frame(cx);
        let top = window
            .find(SharedString::from(row_id(&selected)))
            .bounds()
            .top();
        let scroll = &app.read(cx).active_settings().unwrap().content_scroll;
        let offset = scroll.offset();
        scroll.set_offset(gpui_kit::point(offset.x, offset.y - top + px(240.)));
        window.render_frame(cx);
        window.click(SharedString::from(row_id(&selected)), cx);
    });
    cx.update(|window, cx| {
        window.render_frame(cx);
        window.click("skills-check-update", cx);
    });
    settle(app, cx);
    cx.update(|window, cx| {
        window.render_frame(cx);
        window.click("skills-update-confirm", cx);
    });
    settle(app, cx);
    assert_eq!(std::fs::read_to_string(&document).unwrap(), local);
    assert_eq!(std::fs::read(&lock).unwrap(), original_lock);
    cx.update(|window, cx| {
        window.render_frame(cx);
        window.click("skills-update-replace", cx);
        window.press("escape", cx);
    });
    assert_eq!(std::fs::read_to_string(&document).unwrap(), local);
    assert_eq!(std::fs::read(&lock).unwrap(), original_lock);
    cx.update(|window, cx| {
        window.render_frame(cx);
        window.click("skills-check-update", cx);
    });
    settle(app, cx);
    cx.update(|window, cx| {
        window.render_frame(cx);
        window.click("skills-update-replace", cx);
        window.click("skills-update-confirm", cx);
    });
    settle(app, cx);
    assert_eq!(
        std::fs::read_to_string(&document).unwrap(),
        "---\nname: needle\ndescription: Updated repository reference\n---\n# Updated upstream content\n"
    );
    let updated_lock: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&lock).unwrap()).unwrap();
    assert_eq!(updated_lock["skills"]["needle"]["source"], "example/skills");
    cx.update(|window, cx| {
        window.render_frame(cx);
        window.click("skills-remove-review", cx);
    });
    settle(app, cx);
    let lock_before_cancel = std::fs::read(&lock).unwrap();
    cx.update(|window, cx| {
        window.render_frame(cx);
        window.press("escape", cx);
    });
    assert!(document.exists());
    assert_eq!(std::fs::read(&lock).unwrap(), lock_before_cancel);
    cx.update(|window, cx| {
        window.render_frame(cx);
        window.click("skills-remove-review", cx);
    });
    settle(app, cx);
    cx.update(|window, cx| {
        window.render_frame(cx);
        window.click("skills-remove-confirm", cx);
    });
    settle(app, cx);
    assert!(!installation.exists());
    let removed: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&lock).unwrap()).unwrap();
    assert!(removed["skills"].get("needle").is_none());
    assert_eq!(
        std::fs::read_to_string(unrelated).unwrap(),
        "keep unrelated"
    );
}

#[path = "maintenance_keyboard_tests.rs"]
mod keyboard;

#[gpui_kit::test]
fn ancestor_skill_requires_explicit_owner_project_switch_before_removal(
    cx: &mut gpui_kit::TestAppContext,
) {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("repo");
    let child = project.join("crates/app");
    let installation = project.join(".agents/skills/ancestor");
    std::fs::create_dir_all(&child).unwrap();
    std::fs::create_dir_all(&installation).unwrap();
    std::fs::create_dir(project.join(".git")).unwrap();
    std::fs::write(
        installation.join("SKILL.md"),
        "---\nname: ancestor\n---\n# Keep until owner selected\n",
    )
    .unwrap();
    std::fs::write(project.join("skills-lock.json"), br#"{"version":1,"skills":{"ancestor":{"source":"example/skills","sourceType":"github","skillPath":"ancestor/SKILL.md"}}}"#).unwrap();
    let (app, mut vcx) = crate::ui::app::test_window::harness(cx);
    app.update_in(&mut vcx, |app, window, cx| {
        super::super::tests::isolate_native_roots(&temp.path().join("home"), cx);
        app.open_settings_section(SettingsSection::Skills, window, cx);
        app.active_settings()
            .unwrap()
            .skills
            .project_input
            .clone()
            .update(cx, |input, cx| {
                input.set_value(child.to_string_lossy().into_owned(), window, cx)
            });
        app.apply_skills_project(cx);
    });
    settle(&app, &mut vcx);
    vcx.simulate_resize(gpui_kit::size(px(1200.), px(1800.)));
    let selected = app.read_with(&vcx, |app, _| {
        app.active_settings()
            .unwrap()
            .skills
            .snapshot
            .as_ref()
            .unwrap()
            .filtered("ancestor", Some(SourceKind::ProjectShared))
            .next()
            .unwrap()
            .path()
            .to_owned()
    });
    vcx.update(|window, cx| {
        window.render_frame(cx);
        window.click(SharedString::from(row_id(&selected)), cx);
    });
    vcx.update(|window, cx| {
        window.render_frame(cx);
        window.click("skills-remove-review", cx);
    });
    settle(&app, &mut vcx);
    assert!(app.read_with(&vcx, |app, _| {
        app.active_settings()
            .unwrap()
            .skills
            .maintenance
            .error
            .as_ref()
            .is_some_and(|error| error.contains(project.to_string_lossy().as_ref()))
    }));
    assert!(installation.join("SKILL.md").exists());
    vcx.simulate_keystrokes("escape");
    app.update_in(&mut vcx, |app, window, cx| {
        app.active_settings()
            .unwrap()
            .skills
            .project_input
            .clone()
            .update(cx, |input, cx| {
                input.set_value(project.to_string_lossy().into_owned(), window, cx)
            });
        app.apply_skills_project(cx);
    });
    settle(&app, &mut vcx);
    vcx.update(|window, cx| {
        window.render_frame(cx);
        window.click(SharedString::from(row_id(&selected)), cx);
    });
    vcx.update(|window, cx| {
        window.render_frame(cx);
        window.click("skills-remove-review", cx);
    });
    settle(&app, &mut vcx);
    vcx.update(|window, cx| {
        window.render_frame(cx);
        window.click("skills-remove-confirm", cx);
    });
    settle(&app, &mut vcx);
    assert!(!installation.exists());
    let lock: serde_json::Value =
        serde_json::from_slice(&std::fs::read(project.join("skills-lock.json")).unwrap()).unwrap();
    assert!(lock["skills"].get("ancestor").is_none());
}
