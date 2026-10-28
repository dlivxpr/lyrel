use super::*;

#[test]
fn the_default_path_ends_at_the_documented_file() {
    match default_machine_path() {
        Ok(path) => assert_eq!(
            path.file_name().and_then(|n| n.to_str()),
            Some(MACHINE_FILE)
        ),
        Err(e) => assert!(e.to_string().contains(DATA_DIR_ENV)),
    }
}

/// The invariant the whole file rests on: one config directory is one
/// instance, tree included.
///
/// While the tree resolved from `HOME` alone, `--config-dir` moved the
/// lock, both sockets, `views.json` and the scrollback but left the tree
/// behind, so two lyrels that each believed they were the only server on
/// this machine wrote one `machine.json` between them — whole-document
/// writes, last one wins, and the workspaces the loser knew about were
/// gone.
#[test]
fn the_tree_lives_in_the_config_directory() {
    if std::env::var_os(DATA_DIR_ENV).is_some() {
        return;
    }
    // Against whatever the config directory resolves to, not against the
    // path this call passed in: `set_config_dir` is first-wins and
    // process-wide, so the pin only guarantees *a* scratch directory is in
    // force — which test put it there depends on the order they ran in.
    let _ = crate::core::session::test_support::pin_config_dir();
    let config_dir = crate::core::config::config_dir_path();
    assert!(config_dir.is_some(), "the pin puts one in force");
    assert_eq!(
        default_machine_path().unwrap().parent(),
        config_dir.as_deref(),
        "a config directory nobody else names must not share anybody's tree"
    );
}

/// The other half of "one config directory is one instance": there is one
/// legacy tree, so exactly one instance may inherit it, and start order
/// must not be what picks. A second lyrel renaming the tree into its own
/// directory leaves the primary owning nothing — the same bug, one upgrade
/// later.
#[test]
fn only_the_machines_own_instance_inherits_the_legacy_tree() {
    let machines = PathBuf::from("/home/u/.config/lyrel");
    assert!(is_the_machines_instance(
        Some(&machines),
        Some(&machines.clone())
    ));
    assert!(
        !is_the_machines_instance(Some(Path::new("/tmp/scratch")), Some(&machines)),
        "an instance pointed somewhere of its own starts on an empty tree"
    );
    assert!(
        !is_the_machines_instance(None, Some(&machines)),
        "nowhere to put it is not a licence to move it"
    );
    assert!(!is_the_machines_instance(Some(&machines), None));
}

#[test]
fn an_instance_of_its_own_leaves_the_machines_tree_where_it_is() {
    let old = tempfile::TempDir::new().unwrap();
    let new = tempfile::TempDir::new().unwrap();
    let legacy = old.path().join(MACHINE_FILE);
    std::fs::write(&legacy, br#"{"workspaces":[],"panes":[]}"#).unwrap();

    adopt_into(Some(new.path()), Some(old.path()), false);

    assert!(
        legacy.exists(),
        "a --config-dir instance that takes the tree hands the primary an \
         empty one, which is the failure this change exists to stop"
    );
    assert!(!new.path().join(MACHINE_FILE).exists());
}

#[test]
fn an_upgrade_carries_the_appearance_hint_along_with_the_tree() {
    let old = tempfile::TempDir::new().unwrap();
    let new = tempfile::TempDir::new().unwrap();
    for file in [MACHINE_FILE, APPEARANCE_FILE] {
        std::fs::write(old.path().join(file), b"{}").unwrap();
    }

    adopt_into(Some(new.path()), Some(old.path()), true);

    for file in [MACHINE_FILE, APPEARANCE_FILE] {
        assert!(new.path().join(file).exists(), "{file} was left behind");
        assert!(!old.path().join(file).exists(), "{file} was not moved");
    }
}

/// `LYREL_DATA_DIR` answers both getters, so the sandboxes every test
/// harness pins with it must come out the far side untouched.
#[test]
fn one_directory_under_two_names_is_not_a_migration() {
    let dir = tempfile::TempDir::new().unwrap();
    let file = dir.path().join(MACHINE_FILE);
    std::fs::write(&file, b"live").unwrap();

    adopt_into(Some(dir.path()), Some(dir.path()), true);

    assert_eq!(std::fs::read(&file).unwrap(), b"live");
}

/// The cross-filesystem path, where `rename` refuses and copying is the
/// only way over. It may add a file and it may not replace one.
#[test]
fn a_copied_tree_never_lands_on_one_already_there() {
    let dir = tempfile::TempDir::new().unwrap();
    let (from, to) = (dir.path().join("from"), dir.path().join("to"));
    std::fs::write(&from, b"carried").unwrap();

    copy_new(&from, &to).expect("nothing is there yet");
    assert_eq!(std::fs::read(&to).unwrap(), b"carried");

    let second = copy_new(&from, &to).expect_err("the second must not replace the first");
    assert_eq!(second.kind(), std::io::ErrorKind::AlreadyExists);
    assert_eq!(std::fs::read(&to).unwrap(), b"carried");
}

/// A rename that failed because somebody else already carried the file
/// over is the one benign race, and it must not turn into a copy — there
/// is nothing left to copy, and the winner's file is what was wanted.
#[test]
fn a_source_already_carried_over_is_not_an_error() {
    let dir = tempfile::TempDir::new().unwrap();
    let (legacy, current) = (dir.path().join("legacy"), dir.path().join("current"));
    std::fs::write(&current, b"the winner's").unwrap();

    adopt_by_copy(&legacy, &current, &io::Error::from(io::ErrorKind::NotFound));

    assert_eq!(std::fs::read(&current).unwrap(), b"the winner's");
}

#[test]
fn an_upgrade_carries_the_legacy_tree_forward() {
    let old = tempfile::TempDir::new().unwrap();
    let new = tempfile::TempDir::new().unwrap();
    let (legacy, current) = (old.path().join(MACHINE_FILE), new.path().join(MACHINE_FILE));
    std::fs::write(&legacy, br#"{"workspaces":[],"panes":[]}"#).unwrap();

    adopt_legacy_file(&legacy, &current);

    assert!(
        current.exists(),
        "moving the path without the tree would lose every workspace at the \
         moment of upgrade, which is the failure this change exists to stop"
    );
    assert!(
        !legacy.exists(),
        "a move leaves nothing to be adopted twice"
    );
}

/// The stale-overwrite case, and the reason the guard is "the destination
/// exists" rather than "the source is newer": a build from before the move,
/// run once since, rewrites the legacy path with a tree that predates
/// everything done since. Adopting that over the live file would hand the
/// old tree back — a slower version of the bug, not a fix for it.
#[test]
fn a_legacy_file_never_overwrites_the_tree_in_use() {
    let old = tempfile::TempDir::new().unwrap();
    let new = tempfile::TempDir::new().unwrap();
    let (legacy, current) = (old.path().join(MACHINE_FILE), new.path().join(MACHINE_FILE));
    std::fs::write(&legacy, b"stale").unwrap();
    std::fs::write(&current, b"live").unwrap();

    adopt_legacy_file(&legacy, &current);

    assert_eq!(std::fs::read(&current).unwrap(), b"live");
    assert!(
        legacy.exists(),
        "the one it did not adopt is not the one it may delete"
    );
}

#[test]
fn nothing_to_adopt_creates_nothing() {
    let old = tempfile::TempDir::new().unwrap();
    let new = tempfile::TempDir::new().unwrap();
    let current = new.path().join(MACHINE_FILE);

    adopt_legacy_file(&old.path().join(MACHINE_FILE), &current);

    assert!(
        !current.exists(),
        "a fresh install has nothing to carry forward and must not be handed an empty tree"
    );
}
