use crate::address::Context;
use crate::commands::execute;
use crate::commands::test_support::cli;
use crate::commands::test_support::mock;

#[test]
fn worktree_kimi_task_fails_before_creating_a_checkout_or_touching_any_pane() {
    let repo = tempfile::tempdir().unwrap();
    lyrel_core::core::config::set_config_dir(repo.path().join("config"));
    let repo_arg = repo.path().to_str().unwrap();
    for task in ["修复问题\n保留交互", "--model other", "fork source", "/new"] {
        let mut backend = mock();
        let error = execute(
            cli(&[
                "lyrel",
                "worktree",
                "new",
                "--repo",
                repo_arg,
                "--agent",
                "kimi",
                &format!("--task={task}"),
            ]),
            &Context::default(),
            &mut backend,
        )
        .expect_err("Kimi's unavailable interactive task delivery must be explicit");
        assert!(error.to_string().contains("native TUI"), "{error:#}");
        assert!(!error.to_string().contains(task));
        assert!(backend.spawned.is_empty(), "no new pane may be started");
        assert!(
            backend.sent.is_empty(),
            "no existing pane may receive input"
        );
        assert!(
            backend.control_calls.is_empty(),
            "the machine tree must stay untouched"
        );
        assert!(std::fs::read_dir(repo.path()).unwrap().next().is_none());
    }
}
