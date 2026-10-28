use super::credentials::{first_readable_key_in, first_readable_key_or};
use super::*;
use crate::ui::settings::test_window::harness;
use gpui_kit::{TestAppContext, size};

/// The password lives in the keychain under the address, not in the
/// profile — so saving has to decide two things the config file cannot
/// record: whether the entry the form read is now stranded, and whether
/// there is anything new to write.
#[test]
fn saving_moves_a_password_with_the_address_it_belongs_to() {
    let plan = |was, typed, moved| password_plan(was, typed, moved);

    // A form nobody typed in writes nothing at all.
    assert_eq!(
        plan("hunter2", "hunter2", false),
        PasswordPlan {
            drop_old: false,
            store: false
        }
    );
    // A new secret replaces the old one in place.
    assert_eq!(
        plan("hunter2", "correct horse", false),
        PasswordPlan {
            drop_old: false,
            store: true
        }
    );
    // Clearing the box is how a saved password is let go of.
    assert_eq!(
        plan("hunter2", "", false),
        PasswordPlan {
            drop_old: true,
            store: false
        }
    );
    // Retargeting the host carries the secret across and leaves nothing
    // behind under the old address — even when the secret itself is
    // untouched, because the account it is filed under is the address.
    assert_eq!(
        plan("hunter2", "hunter2", true),
        PasswordPlan {
            drop_old: true,
            store: true
        }
    );
    // A host that never had one, and still does not.
    assert_eq!(
        plan("", "", true),
        PasswordPlan {
            drop_old: false,
            store: false
        }
    );
    // The first password a host is given.
    assert_eq!(
        plan("", "hunter2", false),
        PasswordPlan {
            drop_old: false,
            store: true
        }
    );
}

/// The passphrase box is about one key at a time: the first one named that
/// is actually on disk, with `%h` and `%r` filled in the way the daemon
/// will fill them. A path that is not there can hold no passphrase.
#[test]
fn the_passphrase_follows_the_first_key_that_is_really_there() {
    let dir = std::env::temp_dir().join(format!("lyrel-keyform-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let real = dir.join("id_example.com");
    std::fs::write(&real, b"key").unwrap();
    let missing = dir.join("absent").to_string_lossy().to_string();
    let pattern = dir.join("id_%h").to_string_lossy().to_string();

    assert_eq!(
        first_readable_key_in(&[missing.clone(), pattern], "example.com", "ada"),
        Some(real.to_string_lossy().to_string())
    );
    assert_eq!(
        first_readable_key_in(&[missing.clone()], "example.com", "ada"),
        None
    );
    // An empty field falls back to the defaults the handshake offers —
    // and a named key, even a missing one, replaces them entirely.
    let defaults = || vec![missing.clone(), real.to_string_lossy().to_string()];
    assert_eq!(
        first_readable_key_or(&[], "example.com", "ada", defaults),
        Some(real.to_string_lossy().to_string())
    );
    assert_eq!(
        first_readable_key_or(&[missing.clone()], "example.com", "ada", defaults),
        None
    );
    assert_eq!(
        first_readable_key_or(&[], "example.com", "ada", Vec::new),
        None
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn the_host_filter_matches_name_address_and_port() {
    let mut p = SshProfile::new("prod-web");
    p.host = "10.0.1.21".to_string();
    p.user = "deploy".to_string();
    p.port = 2222;

    assert!(ssh_row_matches(&p, ""), "an empty query keeps everything");
    assert!(ssh_row_matches(&p, "prod"));
    assert!(ssh_row_matches(&p, "10.0.1"));
    assert!(ssh_row_matches(&p, "deploy"));
    assert!(ssh_row_matches(&p, "2222"));
    assert!(!ssh_row_matches(&p, "staging"));
}

#[test]
fn the_host_filter_ignores_case() {
    let mut p = SshProfile::new("Prod-Web");
    p.host = "Example.COM".to_string();
    assert!(ssh_row_matches(&p, "prod"));
    assert!(ssh_row_matches(&p, "example.com"));
}

#[test]
fn group_buckets_sort_imported_first_and_ungrouped_last() {
    let mut keys = vec!["", "Work", crate::core::ssh_config::IMPORTED_GROUP];
    keys.sort_by_key(|k| ssh_group_rank(k));
    assert_eq!(
        keys,
        vec![crate::core::ssh_config::IMPORTED_GROUP, "Work", ""]
    );
}

#[test]
fn group_labels_name_the_file_and_the_app() {
    assert_eq!(
        ssh_group_label(crate::core::ssh_config::IMPORTED_GROUP),
        "~/.ssh/config"
    );
    assert_eq!(ssh_group_label(""), "lyrel settings");
    assert_eq!(ssh_group_label("Work"), "Work");
}

#[test]
fn group_key_falls_back_to_the_ungrouped_bucket() {
    let mut p = SshProfile::new("a");
    assert_eq!(ssh_group_key(&p), "");
    p.group = Some("Work".to_string());
    assert_eq!(ssh_group_key(&p), "Work");
}

#[test]
fn parse_host_port_handles_blank_and_ports() {
    assert!(
        parse_host_port_checked("  ", DEFAULT_SOCKS_PORT)
            .unwrap()
            .is_none()
    );
    let hp = parse_host_port_checked("example.com:2222", DEFAULT_SOCKS_PORT)
        .unwrap()
        .unwrap();
    assert_eq!(hp.host, "example.com");
    assert_eq!(hp.port, 2222);
    // Used to be port 0, which no proxy answers on.
    assert_eq!(
        parse_host_port_checked("host", DEFAULT_SOCKS_PORT)
            .unwrap()
            .unwrap()
            .port,
        DEFAULT_SOCKS_PORT
    );
}

/// A form with the one field that is genuinely required, and nothing else.
fn draft_with_host() -> SshFormDraft {
    SshFormDraft {
        host: "example.com".to_string(),
        ..Default::default()
    }
}

#[test]
fn a_profile_with_no_host_is_not_saveable() {
    let (_, errors) = validate_ssh_draft(SshFormDraft::default(), &[]);
    assert_eq!(errors.host, Some(SshFieldError::HostMissing));
    assert!(!errors.is_empty());
}

#[test]
fn spaces_are_not_a_host() {
    let draft = SshFormDraft {
        host: "   ".to_string(),
        ..Default::default()
    };
    let (profile, errors) = validate_ssh_draft(draft, &[]);
    assert_eq!(errors.host, Some(SshFieldError::HostMissing));
    assert_eq!(profile.host, "");
}

#[test]
fn a_name_is_not_required() {
    // Every host imported from ~/.ssh/config arrives without one, and the
    // list falls back to the address.
    let (profile, errors) = validate_ssh_draft(draft_with_host(), &[]);
    assert_eq!(profile.name, "");
    assert!(errors.is_empty());
}

#[test]
fn a_blank_port_still_means_22() {
    let (profile, errors) = validate_ssh_draft(draft_with_host(), &[]);
    assert_eq!(profile.port, 22);
    assert_eq!(errors.port, None);
}

#[test]
fn a_port_that_is_not_a_port_is_refused() {
    // "0" parses as a u16 and used to be saved as written; the other two
    // failed to parse and were silently rewritten to 22.
    for text in ["0", "abc", "70000", "-1", "22 "] {
        let draft = SshFormDraft {
            port: text.to_string(),
            ..draft_with_host()
        };
        let (profile, errors) = validate_ssh_draft(draft, &[]);
        match text {
            "22 " => {
                assert_eq!(errors.port, None, "{text:?} is a port with spare space");
                assert_eq!(profile.port, 22);
            }
            _ => {
                assert_eq!(errors.port, Some(SshFieldError::PortRange), "{text:?}");
                assert!(!errors.is_empty());
            }
        }
    }
}

#[test]
fn a_jump_host_that_exists_is_kept_by_id() {
    let bastion = SshProfile::new("bastion");
    let draft = SshFormDraft {
        jump: "bastion".to_string(),
        ..draft_with_host()
    };
    let (profile, errors) = validate_ssh_draft(draft, &[bastion.clone()]);
    assert_eq!(profile.jump_host, Some(bastion.id));
    assert!(errors.is_empty());
}

#[test]
fn a_mistyped_jump_host_says_which_name_it_could_not_find() {
    let draft = SshFormDraft {
        jump: "bastian".to_string(),
        ..draft_with_host()
    };
    let (profile, errors) = validate_ssh_draft(draft, &[SshProfile::new("bastion")]);
    assert_eq!(
        errors.jump,
        Some(SshFieldError::JumpUnknown("bastian".to_string()))
    );
    assert_eq!(
        profile.jump_host, None,
        "a typo never saves as a direct connection"
    );
}

#[test]
fn a_host_cannot_jump_through_itself() {
    let me = SshProfile::new("prod");
    let draft = SshFormDraft {
        id: me.id,
        jump: "prod".to_string(),
        ..draft_with_host()
    };
    let (profile, errors) = validate_ssh_draft(draft, &[me]);
    assert_eq!(errors.jump, Some(SshFieldError::JumpIsSelf));
    assert_eq!(profile.jump_host, None);
}

#[test]
fn a_bare_proxy_host_takes_the_scheme_default_port() {
    let draft = SshFormDraft {
        socks: "socks.example.com".to_string(),
        http: "http.example.com".to_string(),
        ..draft_with_host()
    };
    let (profile, errors) = validate_ssh_draft(draft, &[]);
    assert_eq!(
        profile.socks_proxy,
        Some(HostPort::new("socks.example.com", 1080))
    );
    assert_eq!(
        profile.http_proxy,
        Some(HostPort::new("http.example.com", 8080))
    );
    assert!(errors.is_empty());
}

#[test]
fn a_proxy_address_with_a_colon_and_no_port_is_refused() {
    for text in ["proxy.example.com:", "proxy.example.com:abc", "proxy:0"] {
        let draft = SshFormDraft {
            socks: text.to_string(),
            ..draft_with_host()
        };
        let (profile, errors) = validate_ssh_draft(draft, &[]);
        assert_eq!(
            errors.socks,
            Some(SshFieldError::ProxyPortRange),
            "{text:?}"
        );
        assert_eq!(profile.socks_proxy, None, "{text:?}");
    }
}

#[test]
fn a_form_that_cannot_be_saved_still_reports_what_it_would_save() {
    // The Escape prompt asks whether the form differs from the config, so
    // an invalid form has to hand back a profile to compare — otherwise a
    // half-typed new host looks identical to the nothing on disk and
    // Escape throws it away without asking.
    let draft = SshFormDraft {
        name: "half typed".to_string(),
        ..Default::default()
    };
    let (profile, errors) = validate_ssh_draft(draft, &[]);
    assert!(!errors.is_empty());
    assert_eq!(profile.name, "half typed");
}

fn profile_at(name: &str, user: &str, host: &str, port: u16) -> SshProfile {
    let mut p = SshProfile::new(name);
    p.user = user.to_string();
    p.host = host.to_string();
    p.port = port;
    p
}

/// The saved password belongs to `user@host:port`, so what counts as
/// "shared" is exactly that triple — a different name or a jump host in
/// front of it changes nothing, and a different port makes it a different
/// secret entirely.
#[test]
fn the_same_endpoint_under_two_names_counts_as_shared() {
    let direct = profile_at("direct", "ana", "build.example.com", 22);
    let mut via_jump = profile_at("via bastion", "ana", "build.example.com", 22);
    via_jump.jump_host = Some(direct.id);
    let staging = profile_at("staging", "ana", "build.example.com", 2222);
    let other_user = profile_at("root", "root", "build.example.com", 22);

    let mut cfg = Config::default();
    let (direct_id, jump_id, staging_id) = (direct.id, via_jump.id, staging.id);
    cfg.ssh_profiles = vec![direct, via_jump, staging, other_user];

    // The two that reach the same endpoint see each other, and neither
    // counts itself.
    assert_eq!(profiles_sharing_endpoint(&cfg, direct_id), 1);
    assert_eq!(profiles_sharing_endpoint(&cfg, jump_id), 1);
    // A port apart is a keychain entry apart, so this one is alone even
    // though the user and host match two of the others.
    assert_eq!(profiles_sharing_endpoint(&cfg, staging_id), 0);
    // A profile that is no longer on the list shares with nobody.
    assert_eq!(profiles_sharing_endpoint(&cfg, Uuid::new_v4()), 0);
}

/// The password is the one field on the host editor that never reaches the
/// profile — it goes to the system keychain — so the dirty check that
/// compares profiles is blind to it. Without the secret folded in, Save
/// stays greyed out over a password the user has just typed, and the only
/// way to store one is to connect and wait to be asked.
#[gpui_kit::test]
fn a_typed_password_is_something_the_form_has_to_save(cx: &mut TestAppContext) {
    crate::core::config::pin_test_config_dir();
    let (app, mut vcx) = harness(cx);
    app.update_in(&mut vcx, |app, window, cx| {
        app.open_settings_section(SettingsSection::Ssh, window, cx);
        // A saved host that names no address, so opening its editor asks
        // the keychain nothing and starts out with nothing to save.
        let profile = crate::core::ssh_profile::SshProfile::new("blank");
        app.update_config(cx, |cfg| cfg.ssh_profiles.push(profile.clone()));
        app.ssh_form_load(&profile, window, cx);
    });
    vcx.simulate_resize(size(px(1100.), px(800.)));
    vcx.run_until_parked();

    assert!(
        !vcx.update(|_, cx| app.read(cx).ssh_form_dirty(cx)),
        "a form nobody has typed in has nothing to save"
    );

    let password = vcx.update(|_, cx| {
        app.read(cx)
            .active_settings()
            .and_then(|s| s.ssh.form.as_ref())
            .map(|f| f.password.clone())
            .expect("the host editor is open")
    });
    app.update_in(&mut vcx, |_app, window, cx| {
        password.update(cx, |input, cx| input.set_value("hunter2", window, cx));
    });
    vcx.run_until_parked();

    assert!(
        vcx.update(|_, cx| app.read(cx).ssh_form_dirty(cx)),
        "a password typed into the form is an unsaved change"
    );
}

#[gpui_kit::test]
fn external_ssh_navigation_resolves_the_current_form_once(cx: &mut TestAppContext) {
    crate::core::config::pin_test_config_dir();
    let (workspace, mut vcx) = crate::ui::app::test_window::harness(cx);
    let app = crate::ui::settings::test_window::open(&workspace, &mut vcx);
    let mut original = crate::core::ssh_profile::SshProfile::new("original");
    original.host = "original.example.com".into();
    let id = original.id;
    workspace.update_in(&mut vcx, |workspace, window, cx| {
        cx.global_mut::<Config>()
            .ssh_profiles
            .push(original.clone());
        workspace.open_ssh_profile_in_settings(id, window, cx);
    });
    vcx.run_until_parked();
    app.update_in(&mut vcx, |app, window, cx| {
        let input = app
            .active_settings()
            .unwrap()
            .ssh
            .form
            .as_ref()
            .unwrap()
            .host
            .clone();
        input.update(cx, |input, cx| {
            input.set_value("edited.example.com", window, cx)
        });
    });
    workspace.update_in(&mut vcx, |workspace, window, cx| {
        workspace.open_ssh_profile_new_from_target("new.example.com".into(), window, cx);
    });
    vcx.run_until_parked();
    assert!(vcx.has_pending_prompt());
    vcx.simulate_prompt_answer(crate::ui::i18n::t(
        crate::ui::i18n::L10nKey::SettingsKeepEditing,
    ));
    vcx.run_until_parked();
    app.update_in(&mut vcx, |app, _, cx| {
        assert_eq!(
            app.active_settings()
                .unwrap()
                .ssh
                .form
                .as_ref()
                .unwrap()
                .editing,
            id
        );
        assert!(app.ssh_form_dirty(cx));
    });
    workspace.update_in(&mut vcx, |workspace, window, cx| {
        workspace.open_ssh_profile_new_from_target("new.example.com".into(), window, cx);
    });
    vcx.run_until_parked();
    assert!(vcx.has_pending_prompt());
    vcx.simulate_prompt_answer(crate::ui::i18n::t(crate::ui::i18n::L10nKey::EditorDiscard));
    vcx.run_until_parked();
    assert!(!vcx.has_pending_prompt());
    app.update_in(&mut vcx, |app, _, cx| {
        let form = app.active_settings().unwrap().ssh.form.as_ref().unwrap();
        assert_ne!(form.editing, id);
        assert_eq!(form.host.read(cx).value().as_str(), "new.example.com");
    });
    app.update_in(&mut vcx, |app, _, cx| app.cancel_ssh_form(cx));
    workspace.update_in(&mut vcx, |workspace, window, cx| {
        workspace.open_ssh_profile_in_settings(id, window, cx);
    });
    vcx.run_until_parked();
    app.update_in(&mut vcx, |app, window, cx| {
        let input = app
            .active_settings()
            .unwrap()
            .ssh
            .form
            .as_ref()
            .unwrap()
            .host
            .clone();
        input.update(cx, |s, cx| s.set_value("saved.example.com", window, cx));
    });
    workspace.update_in(&mut vcx, |workspace, window, cx| {
        workspace.open_ssh_profile_in_settings(id, window, cx);
    });
    vcx.run_until_parked();
    vcx.simulate_prompt_answer(crate::ui::i18n::t(
        crate::ui::i18n::L10nKey::SettingsSaveChanges,
    ));
    vcx.run_until_parked();
    assert!(!vcx.has_pending_prompt());
    app.update_in(&mut vcx, |app, _, cx| {
        assert!(!app.ssh_form_dirty(cx));
        assert_eq!(
            app.active_settings()
                .unwrap()
                .ssh
                .form
                .as_ref()
                .unwrap()
                .host
                .read(cx)
                .value()
                .as_str(),
            "saved.example.com"
        );
    });
}

/// The credential rows follow the auth method the same way connecting
/// does: a password only for methods that offer one, a passphrase never
/// for a method that does not try keys. (Whether Auto / PublicKey show the
/// passphrase depends on the default keys in this machine's `~/.ssh`, so
/// that half is not pinned here.)
#[gpui_kit::test]
fn the_credential_rows_follow_the_auth_method(cx: &mut TestAppContext) {
    use crate::core::ssh_profile::{AuthMode, SshProfile};
    crate::core::config::pin_test_config_dir();
    let (workspace, mut vcx) = crate::ui::app::test_window::harness(cx);
    let app = crate::ui::settings::test_window::open(&workspace, &mut vcx);
    for (auth, password, keys) in [
        (AuthMode::Auto, true, true),
        (AuthMode::Password, true, false),
        (AuthMode::PublicKey, false, true),
        (AuthMode::Agent, false, false),
    ] {
        let mut profile = SshProfile::new("creds");
        profile.auth = auth;
        let id = profile.id;
        workspace.update_in(&mut vcx, |workspace, window, cx| {
            cx.global_mut::<Config>().ssh_profiles.push(profile);
            workspace.open_ssh_profile_in_settings(id, window, cx);
        });
        vcx.run_until_parked();
        app.update_in(&mut vcx, |app, _, cx| {
            let form = app.active_settings().unwrap().ssh.form.as_ref().unwrap();
            assert_eq!(form.wants_password(), password, "{auth:?}");
            if !keys {
                assert!(!form.wants_passphrase(), "{auth:?} never tries a key");
            }
            app.cancel_ssh_form(cx);
        });
        vcx.run_until_parked();
    }
}

#[gpui_kit::test]
fn invalid_ssh_edits_stay_dirty_until_explicit_cancel(cx: &mut TestAppContext) {
    crate::core::config::pin_test_config_dir();
    let (app, mut vcx) = harness(cx);
    app.update_in(&mut vcx, |app, window, cx| {
        app.open_settings_section(SettingsSection::Ssh, window, cx);
        app.add_new_profile(window, cx);
        let input = app.ssh_form_mut().unwrap().name.clone();
        input.update(cx, |input, cx| {
            input.set_value("unfinished host", window, cx)
        });
        assert!(app.ssh_form_dirty(cx));
        assert!(app.save_editing_profile(window, cx).is_none());
        app.navigate_settings(SettingsSection::General, None, window, cx);
    });
    vcx.run_until_parked();
    assert!(vcx.has_pending_prompt());
    vcx.simulate_prompt_answer(t(L10nKey::SettingsKeepEditing));
    vcx.run_until_parked();
    app.update_in(&mut vcx, |app, _, cx| {
        let form = app.active_settings().unwrap().ssh.form.as_ref().unwrap();
        assert_eq!(form.name.read(cx).value().as_str(), "unfinished host");
        let id = form.editing;
        assert!(
            !cx.global::<Config>()
                .ssh_profiles
                .iter()
                .any(|p| p.id == id)
        );
        assert!(app.ssh_form_dirty(cx));
        app.cancel_ssh_form(cx);
        assert!(!app.ssh_form_dirty(cx));
        assert!(app.active_settings().unwrap().ssh.form.is_none());
    });
}

#[gpui_kit::test]
fn failed_ssh_config_save_never_advances_credentials_or_secret_baselines(cx: &mut TestAppContext) {
    use crate::core::keychain::InMemoryCredentialStore;
    crate::core::config::pin_test_config_dir();
    let dir = tempfile::tempdir().unwrap();
    let key = dir.path().join("temporary-identity");
    std::fs::write(&key, b"isolated settings key").unwrap();
    let account = key_account_from_contents(b"isolated settings key");
    let store = std::sync::Arc::new(InMemoryCredentialStore::new());
    store
        .set_password("tester", "old.invalid", 22, "old password")
        .unwrap();
    store
        .set_key_passphrase(&account, "old passphrase")
        .unwrap();
    let (app, mut vcx) = harness(cx);
    app.update_in(&mut vcx, |app, window, cx| {
        app.open_settings_section(SettingsSection::Ssh, window, cx);
        app.active_settings_mut().unwrap().ssh.credentials = store.clone();
        let mut profile = profile_at("isolated", "tester", "old.invalid", 22);
        profile.identity_files = vec![key.to_string_lossy().into_owned()];
        cx.global_mut::<Config>().ssh_profiles.push(profile.clone());
        app.ssh_form_load(&profile, window, cx);
        let form = app.active_settings().unwrap().ssh.form.as_ref().unwrap();
        let (host, password, passphrase) = (
            form.host.clone(),
            form.password.clone(),
            form.passphrase.clone(),
        );
        host.update(cx, |i, cx| i.set_value("new.invalid", window, cx));
        password.update(cx, |i, cx| i.set_value("new password ", window, cx));
        passphrase.update(cx, |i, cx| i.set_value("new passphrase ", window, cx));
        cx.global_mut::<Config>().quarantined = true;
        assert!(app.save_editing_profile(window, cx).is_none());
        assert!(app.active_settings().unwrap().save_error.is_some());
        assert_eq!(
            store
                .password_for("tester", "old.invalid", 22)
                .unwrap()
                .as_deref(),
            Some("old password")
        );
        assert_eq!(
            store.password_for("tester", "new.invalid", 22).unwrap(),
            None
        );
        assert_eq!(
            store.passphrase_for_key(&account).unwrap().as_deref(),
            Some("old passphrase")
        );
        let form = app.active_settings().unwrap().ssh.form.as_ref().unwrap();
        assert_eq!(form.loaded_password, "old password");
        assert_eq!(form.loaded_passphrase, "old passphrase");
        assert!(app.ssh_form_dirty(cx));

        cx.global_mut::<Config>().quarantined = false;
        assert_eq!(app.save_editing_profile(window, cx), Some(profile.id));
        assert!(app.active_settings().unwrap().save_error.is_none());
        assert_eq!(
            store.password_for("tester", "old.invalid", 22).unwrap(),
            None
        );
        assert_eq!(
            store
                .password_for("tester", "new.invalid", 22)
                .unwrap()
                .as_deref(),
            Some("new password ")
        );
        assert_eq!(
            store.passphrase_for_key(&account).unwrap().as_deref(),
            Some("new passphrase ")
        );
        assert!(!app.ssh_form_dirty(cx));
    });
}

#[gpui_kit::test]
fn ssh_retargeting_preserves_shared_passwords_and_other_keys_passphrases(cx: &mut TestAppContext) {
    use crate::core::keychain::InMemoryCredentialStore;
    crate::core::config::pin_test_config_dir();
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("key-a");
    let b = dir.path().join("key-b");
    std::fs::write(&a, b"temporary key a").unwrap();
    std::fs::write(&b, b"temporary key b").unwrap();
    let account_a = key_account_from_contents(b"temporary key a");
    let account_b = key_account_from_contents(b"temporary key b");
    let store = std::sync::Arc::new(InMemoryCredentialStore::new());
    store
        .set_password("tester", "shared.invalid", 22, "shared password")
        .unwrap();
    store
        .set_key_passphrase(&account_a, "passphrase a")
        .unwrap();
    store
        .set_key_passphrase(&account_b, "passphrase b")
        .unwrap();
    let (app, mut vcx) = harness(cx);
    app.update_in(&mut vcx, |app, window, cx| {
        app.open_settings_section(SettingsSection::Ssh, window, cx);
        app.active_settings_mut().unwrap().ssh.credentials = store.clone();
        let mut profile = profile_at("edited", "tester", "shared.invalid", 22);
        profile.identity_files = vec![a.to_string_lossy().into_owned()];
        let mut kept = profile.clone();
        kept.id = Uuid::new_v4();
        kept.name = "kept".into();
        cx.global_mut::<Config>()
            .ssh_profiles
            .extend([profile.clone(), kept]);
        app.ssh_form_load(&profile, window, cx);
        let form = app.active_settings().unwrap().ssh.form.as_ref().unwrap();
        let (host, identity, passphrase) = (
            form.host.clone(),
            form.identity_files.clone(),
            form.passphrase.clone(),
        );
        host.update(cx, |i, cx| i.set_value("moved.invalid", window, cx));
        identity.update(cx, |i, cx| {
            i.set_value(b.to_string_lossy().into_owned(), window, cx)
        });
        app.resync_key_passphrase(window, cx);
        assert_eq!(passphrase.read(cx).value().as_str(), "passphrase b");
        passphrase.update(cx, |i, cx| i.set_value("edited b", window, cx));
        assert_eq!(app.save_editing_profile(window, cx), Some(profile.id));
        assert_eq!(
            store
                .password_for("tester", "shared.invalid", 22)
                .unwrap()
                .as_deref(),
            Some("shared password")
        );
        assert_eq!(
            store
                .password_for("tester", "moved.invalid", 22)
                .unwrap()
                .as_deref(),
            Some("shared password")
        );
        assert_eq!(
            store.passphrase_for_key(&account_a).unwrap().as_deref(),
            Some("passphrase a")
        );
        assert_eq!(
            store.passphrase_for_key(&account_b).unwrap().as_deref(),
            Some("edited b")
        );
        app.delete_profile_confirmed(profile.id, cx);
        assert_eq!(
            store.password_for("tester", "moved.invalid", 22).unwrap(),
            None
        );
        assert_eq!(store.passphrase_for_key(&account_b).unwrap(), None);
        assert_eq!(
            store
                .password_for("tester", "shared.invalid", 22)
                .unwrap()
                .as_deref(),
            Some("shared password")
        );
        assert_eq!(
            store.passphrase_for_key(&account_a).unwrap().as_deref(),
            Some("passphrase a")
        );
        assert!(app.active_settings().unwrap().ssh.form.is_none());
    });
}

#[gpui_kit::test]
fn ssh_form_round_trip_preserves_windows_paths_and_raw_command_spelling(cx: &mut TestAppContext) {
    crate::core::config::pin_test_config_dir();
    let dir = tempfile::tempdir().unwrap();
    let (app, mut vcx) = harness(cx);
    app.update_in(&mut vcx, |app, window, cx| {
        app.open_settings_section(SettingsSection::Ssh, window, cx);
        let mut profile = profile_at("round trip", "tester", "example.invalid", 2222);
        profile.identity_files = vec![
            format!(r"{}\temporary identity\id_ed25519", dir.path().display()),
            format!(r"{}\keys\second", dir.path().display()),
        ];
        profile.proxy_command = Some(r#""C:\Tools\proxy.exe" --route "a b" %h %p"#.into());
        profile.login_scripts = vec![r#"printf '%s\n' 'a b'"#.into()];
        profile.group = Some("isolated group".into());
        profile.agent_forward = true;
        profile.keepalive_interval_s = Some(17);
        profile.verify_host_keys = Some(false);
        cx.global_mut::<Config>().ssh_profiles.push(profile.clone());
        app.ssh_form_load(&profile, window, cx);
        let (collected, errors) = app.ssh_form_collect(cx).unwrap();
        assert!(errors.is_empty());
        assert_eq!(collected, profile);
        assert!(!app.ssh_form_dirty(cx));
    });
}
