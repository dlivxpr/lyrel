use super::*;

/// `worktree new` end to end against a real server: the checkout, its
/// `.worktreeinclude` copy, the tab, and `.lyrel/setup` actually running in
/// that tab with its variables; then `ls` sees the pane in it, and `rm` will
/// not take the checkout from under the pane unless told to.
pub(super) fn worktree_new_sets_up_the_checkout_and_rm_takes_it_down(daemon: &Daemon) {
    // The setup script is a POSIX executable; Windows has no `env` to run it.
    if cfg!(windows) {
        return;
    }
    let repo_dir = tempfile::tempdir().expect("temp repo");
    let repo = repo_dir.path();
    let git = |args: &[&str]| {
        let out = Command::new("git")
            .args(["-c", "user.email=t@t", "-c", "user.name=t"])
            .args(args)
            .current_dir(repo)
            .output()
            .expect("run git");
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    git(&["init", "-q"]);
    std::fs::write(repo.join(".gitignore"), ".env\nsetup-ran\n").unwrap();
    std::fs::write(repo.join(".worktreeinclude"), ".env\n").unwrap();
    std::fs::create_dir(repo.join(".lyrel")).unwrap();
    let script = repo.join(".lyrel/setup");
    std::fs::write(
        &script,
        "#!/bin/sh\necho \"$LYREL_PORT $LYREL_WORKTREE_NAME\" > setup-ran\n",
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    git(&["add", "."]);
    git(&["commit", "-q", "-m", "init"]);
    std::fs::write(repo.join(".env"), "SECRET=1\n").unwrap();

    let ws = daemon.run_json(&["new", &workdir()]);
    let ws = ws["id"].as_str().expect("new prints the workspace id");
    let repo_arg = repo.to_str().unwrap();
    let made = daemon.run_json(&["worktree", "new", "e2e-wt", "--ws", ws, "--repo", repo_arg]);
    let path = std::path::PathBuf::from(made["path"].as_str().expect("new prints the path"));
    let pane = made["pane"].as_u64().expect("worktree new prints the pane");
    let port = made["port"].as_u64().expect("worktree new prints the port");
    assert_eq!(made["setup"], true, "{made}");
    assert_eq!(
        std::fs::read_to_string(path.join(".env")).unwrap(),
        "SECRET=1\n"
    );

    let ran = path.join("setup-ran");
    let deadline = Instant::now() + SETTLE_WITHIN;
    let written = loop {
        let text = std::fs::read_to_string(&ran).unwrap_or_default();
        if text.ends_with('\n') {
            break text;
        }
        assert!(
            Instant::now() < deadline,
            "setup never ran; the pane shows:\n{}",
            daemon.run_ok(&["capture", &format!("%{pane}"), "--scrollback"])
        );
        std::thread::sleep(Duration::from_millis(200));
    };
    assert_eq!(written, format!("{port} e2e-wt\n"));

    let listed = daemon.run_json(&["worktree", "ls", "--repo", repo_arg]);
    let rows = listed["worktrees"].as_array().expect("ls lists worktrees");
    assert_eq!(rows.len(), 2, "{listed}");
    assert_eq!(rows[1]["managed"], true, "{listed}");
    assert_eq!(rows[1]["panes"][0].as_u64(), Some(pane), "{listed}");

    let refused = daemon.run(&["worktree", "rm", "e2e-wt", "--repo", repo_arg]);
    assert!(
        !refused.status.success(),
        "rm took the checkout from under its pane"
    );
    assert!(path.exists());
    let removed = daemon.run_json(&[
        "worktree",
        "rm",
        "e2e-wt",
        "--repo",
        repo_arg,
        "--close-panes",
    ]);
    assert_eq!(removed["snapshot"], "refs/lyrel/trash/e2e-wt", "{removed}");
    assert!(!path.exists());
}
