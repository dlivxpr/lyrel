use super::presentation::{OpenedFrom, inherited_start_dir};

#[test]
fn a_saved_host_opened_from_a_local_pane_does_not_start_in_its_local_directory() {
    let local = Some(std::path::PathBuf::from("/Users/alice/dev/lyrel"));
    assert_eq!(
        inherited_start_dir(OpenedFrom::FreshSshHost, local.clone(), false),
        None
    );
    let remote = Some(std::path::PathBuf::from("/home/alice/my_service"));
    assert_eq!(
        inherited_start_dir(OpenedFrom::FreshSshHost, remote, true),
        None,
        "another host's directory means nothing on the one being dialled"
    );
}

#[test]
fn redialling_the_same_ssh_host_starts_in_the_far_directory_it_was_in() {
    let remote = Some(std::path::PathBuf::from("/home/alice/my_service"));
    assert_eq!(
        inherited_start_dir(OpenedFrom::SameSshHost, remote.clone(), true),
        remote
    );
    assert_eq!(
        inherited_start_dir(OpenedFrom::SameSshHost, None, true),
        None
    );
}

#[test]
fn a_local_shell_inherits_only_a_local_directory() {
    let dir = Some(std::path::PathBuf::from("/work/repo"));
    assert_eq!(
        inherited_start_dir(OpenedFrom::LocalShell, dir.clone(), false),
        dir
    );
    assert_eq!(
        inherited_start_dir(OpenedFrom::LocalShell, dir, true),
        None,
        "a remote pane's directory is on the far host"
    );
}
