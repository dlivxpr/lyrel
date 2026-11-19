//! Contract tests at the shared public seam: everything here drives only
//! `lyrel_updater`'s public interface — the same surface the GUI update
//! coordinator builds plans through and the `lyrel-updater` executable
//! parses them with. Round trips go through `argv()` in one direction and
//! `parse()` in the other, so the two sides of the wire protocol cannot
//! drift apart.

#[cfg(target_os = "macos")]
mod macos_contract {
    use lyrel_updater::macos::{InstallPlan, VerifyPlan};
    use std::ffi::OsString;
    use std::path::PathBuf;

    fn install_plan() -> InstallPlan {
        InstallPlan {
            parent_pid: 4242,
            current: PathBuf::from("/Applications/lyrel.app"),
            archive: PathBuf::from("/stage/lyrel-27.1.0-macos-arm64.zip"),
            checksums: PathBuf::from("/stage/checksums.txt"),
            asset_name: "lyrel-27.1.0-macos-arm64.zip".to_string(),
            stage: PathBuf::from("/stage/lyrel-update-x"),
            expected_version: "27.1.0".to_string(),
            log: PathBuf::from("/cfg/update.log"),
            config_dir: Some(PathBuf::from("/cfg")),
            result_file: Some(PathBuf::from("/cfg/update-outcome.json")),
        }
    }

    #[test]
    fn verify_plan_round_trips_through_argv_and_parse() {
        let plan = install_plan().verify_plan();
        let argv = plan.argv();
        assert_eq!(argv[0], OsString::from("verify"));
        let parsed = VerifyPlan::parse(&mut argv.into_iter().skip(1)).unwrap();
        assert_eq!(parsed, plan);
    }

    #[test]
    fn install_plan_round_trips_through_argv_and_parse() {
        let plan = install_plan();
        let argv = plan.argv();
        assert_eq!(argv[0], OsString::from("install"));
        assert_eq!(argv[1], OsString::from("4242"));
        let parsed = InstallPlan::parse(argv.into_iter().skip(1)).unwrap();
        assert_eq!(parsed, plan);
    }

    #[test]
    fn install_plan_rest_is_the_persisted_shape() {
        let plan = install_plan();
        // The seven positionals after the verb and the parent pid, in the
        // order `update.json` persists them.
        assert_eq!(
            plan.rest(),
            [
                PathBuf::from("/Applications/lyrel.app"),
                PathBuf::from("/stage/lyrel-27.1.0-macos-arm64.zip"),
                PathBuf::from("/stage/checksums.txt"),
                PathBuf::from("lyrel-27.1.0-macos-arm64.zip"),
                PathBuf::from("/stage/lyrel-update-x"),
                PathBuf::from("27.1.0"),
                PathBuf::from("/cfg/update.log"),
            ]
        );
    }
}

#[cfg(target_os = "windows")]
mod windows_contract {
    use lyrel_updater::windows::{
        ElevatedStagePlan, InstallPlan, PortableInstallPlan, TailOptions, VerifyPlan, WatcherPlan,
    };
    use std::ffi::OsString;
    use std::path::PathBuf;

    fn install_plan() -> InstallPlan {
        InstallPlan {
            parent_pid: 4242,
            installer: PathBuf::from(r"C:\stage\lyrel-27.1.0-windows-x86_64-setup.exe"),
            checksums: PathBuf::from(r"C:\stage\checksums.txt"),
            asset_name: "lyrel-27.1.0-windows-x86_64-setup.exe".to_string(),
            install_dir: PathBuf::from(r"C:\Users\someone\AppData\Local\Programs\lyrel"),
            expected_version: "27.1.0".to_string(),
            log: PathBuf::from(r"C:\cfg\update.log"),
            stage: PathBuf::from(r"C:\stage\lyrel-update-x"),
            config_dir: Some(PathBuf::from(r"C:\cfg")),
            result_file: Some(PathBuf::from(r"C:\cfg\update-outcome.json")),
            expected_sha256: None,
        }
    }

    #[test]
    fn inno_verify_plan_round_trips_through_argv_and_parse() {
        let plan = VerifyPlan::Installer {
            installer: PathBuf::from(r"C:\stage\lyrel-27.1.0-windows-x86_64-setup.exe"),
            checksums: PathBuf::from(r"C:\stage\checksums.txt"),
            asset_name: "lyrel-27.1.0-windows-x86_64-setup.exe".to_string(),
            expected_version: "27.1.0".to_string(),
        };
        let argv = plan.argv();
        assert_eq!(argv[0], OsString::from("verify"));
        let parsed = VerifyPlan::parse("verify", &mut argv.into_iter().skip(1)).unwrap();
        assert_eq!(parsed, plan);
    }

    #[test]
    fn portable_verify_plan_round_trips_through_argv_and_parse() {
        let plan = VerifyPlan::Portable {
            archive: PathBuf::from(r"C:\stage\lyrel-27.1.0-windows-x86_64.zip"),
            checksums: PathBuf::from(r"C:\stage\checksums.txt"),
            asset_name: "lyrel-27.1.0-windows-x86_64.zip".to_string(),
            expected_version: "27.1.0".to_string(),
            stage: PathBuf::from(r"C:\stage\lyrel-update-x"),
        };
        let argv = plan.argv();
        assert_eq!(argv[0], OsString::from("verify-portable"));
        let parsed = VerifyPlan::parse("verify-portable", &mut argv.into_iter().skip(1)).unwrap();
        assert_eq!(parsed, plan);
    }

    #[test]
    fn install_plan_round_trips_through_argv_and_parse() {
        let plan = install_plan();
        let argv = plan.argv();
        assert_eq!(argv[0], OsString::from("install"));
        assert_eq!(argv[1], OsString::from("4242"));
        let parsed = InstallPlan::parse(argv.into_iter().skip(1)).unwrap();
        assert_eq!(parsed, plan);
    }

    #[test]
    fn install_plan_carries_the_elevation_digest_when_set() {
        let mut plan = install_plan();
        plan.expected_sha256 = Some("ab".repeat(32));
        let argv = plan.argv();
        let parsed = InstallPlan::parse(argv.into_iter().skip(1)).unwrap();
        assert_eq!(parsed, plan);
    }

    #[test]
    fn install_plan_rest_reads_back_through_parse_rest() {
        let plan = install_plan();
        let parsed = InstallPlan::parse_rest(&plan.rest(), plan.parent_pid).unwrap();
        assert_eq!(parsed.installer, plan.installer);
        assert_eq!(parsed.checksums, plan.checksums);
        assert_eq!(parsed.asset_name, plan.asset_name);
        assert_eq!(parsed.install_dir, plan.install_dir);
        assert_eq!(parsed.expected_version, plan.expected_version);
        assert_eq!(parsed.log, plan.log);
        assert_eq!(parsed.stage, plan.stage);
        assert_eq!(parsed.parent_pid, plan.parent_pid);
        // A plan missing any piece is not an elevated plan at all.
        let mut short = plan.rest();
        short.truncate(6);
        assert!(InstallPlan::parse_rest(&short, 1).is_err());
    }

    #[test]
    fn portable_install_plan_round_trips_through_argv_and_parse() {
        let plan = PortableInstallPlan {
            parent_pid: 4242,
            archive: PathBuf::from(r"C:\stage\lyrel-27.1.0-windows-x86_64.zip"),
            checksums: PathBuf::from(r"C:\stage\checksums.txt"),
            asset_name: "lyrel-27.1.0-windows-x86_64.zip".to_string(),
            install_dir: PathBuf::from(r"C:\Users\someone\lyrel"),
            expected_version: "27.1.0".to_string(),
            log: PathBuf::from(r"C:\cfg\update.log"),
            stage: PathBuf::from(r"C:\stage\lyrel-update-x"),
            config_dir: Some(PathBuf::from(r"C:\cfg")),
            result_file: None,
        };
        let argv = plan.argv();
        assert_eq!(argv[0], OsString::from("install-portable"));
        let parsed = PortableInstallPlan::parse(argv.into_iter().skip(1)).unwrap();
        assert_eq!(parsed, plan);
    }

    #[test]
    fn watcher_plan_round_trips_through_argv_and_parse() {
        let plan = WatcherPlan {
            status_file: PathBuf::from(r"C:\cfg\update-elevation.status"),
            result_file: PathBuf::from(r"C:\cfg\update-outcome.json"),
            app: PathBuf::from(r"C:\Users\someone\lyrel\lyrel-app.exe"),
            log: PathBuf::from(r"C:\cfg\update.log"),
            version: "27.1.0".to_string(),
            gui_pid: Some(99),
        };
        let argv = plan.argv();
        assert_eq!(argv[0], OsString::from("relaunch-watcher"));
        let parsed = WatcherPlan::parse(argv.into_iter().skip(1)).unwrap();
        assert_eq!(parsed, plan);
    }

    fn elevated_stage_plan() -> ElevatedStagePlan {
        ElevatedStagePlan {
            gui_pid: 77,
            installer: PathBuf::from(r"C:\stage\lyrel-27.1.0-windows-x86_64-setup.exe"),
            asset_name: "lyrel-27.1.0-windows-x86_64-setup.exe".to_string(),
            install_dir: PathBuf::from(r"C:\Program Files\lyrel"),
            expected_version: "27.1.0".to_string(),
            log: PathBuf::from(r"C:\stage\update.log"),
            stage: PathBuf::from(r"C:\stage\lyrel-update-x"),
            expected_sha256: "ab".repeat(32),
            status_file: PathBuf::from(r"C:\cfg\update-elevation.status"),
            result_file: Some(PathBuf::from(r"C:\cfg\update-outcome.json")),
            config_dir: Some(PathBuf::from(r"C:\cfg")),
        }
    }

    #[test]
    fn elevated_stage_plan_round_trips_through_parse() {
        let plan = elevated_stage_plan();
        // command_line is quoted for the CRT splitter; the parse side takes
        // the raw pieces, so feed it the unquoted shape directly.
        let raw = vec![
            OsString::from(plan.gui_pid.to_string()),
            plan.installer.clone().into_os_string(),
            OsString::from(plan.asset_name.clone()),
            plan.install_dir.clone().into_os_string(),
            OsString::from(plan.expected_version.clone()),
            plan.log.clone().into_os_string(),
            plan.stage.clone().into_os_string(),
            OsString::from("--expected-sha256"),
            OsString::from(plan.expected_sha256.clone()),
            OsString::from("--status-file"),
            plan.status_file.clone().into_os_string(),
            OsString::from("--config-dir"),
            plan.config_dir.clone().unwrap().into_os_string(),
            OsString::from("--result-file"),
            plan.result_file.clone().unwrap().into_os_string(),
        ];
        let parsed = ElevatedStagePlan::parse(raw.into_iter()).unwrap();
        assert_eq!(parsed, plan);
    }

    #[test]
    fn the_elevated_command_line_quotes_every_argument() {
        let plan = elevated_stage_plan();
        let line = plan
            .command_line()
            .into_string()
            .expect("the test paths are valid Unicode");

        assert!(line.starts_with("\"elevated-stage\" "), "{line}");
        assert!(line.contains("\"77\""), "{line}");
        // A space in a path must never reach the CRT splitter unquoted.
        assert!(line.contains(r#""C:\Program Files\lyrel""#), "{line}");
        assert!(
            line.contains(&format!(
                "\"--expected-sha256\" \"{}\"",
                plan.expected_sha256
            )),
            "{line}"
        );
        assert!(
            line.contains(r#""--status-file" "C:\cfg\update-elevation.status""#),
            "{line}"
        );
        assert!(line.contains(r#""--config-dir" "C:\cfg""#), "{line}");
        assert!(
            line.contains(r#""--result-file" "C:\cfg\update-outcome.json""#),
            "{line}"
        );
    }

    /// The CRT splitter the elevated helper is parsed by treats a backslash
    /// as an escape only in front of a quote — so a config directory ending
    /// in one would otherwise escape its own closing quote and eat every
    /// argument after it, including `--result-file`, which is the file the
    /// watcher waits on.
    #[test]
    fn a_trailing_backslash_does_not_escape_its_own_closing_quote() {
        let mut plan = elevated_stage_plan();
        plan.config_dir = Some(PathBuf::from(r"D:\lyrel\"));
        let line = plan
            .command_line()
            .into_string()
            .expect("the test paths are valid Unicode");
        // The doubled backslash keeps the closing quote inside the argument.
        assert!(line.contains(r#""--config-dir" "D:\lyrel\\""#), "{line}");
        // The flags after the config dir survive: the quote did not escape.
        assert!(
            line.contains(r#""--result-file" "C:\cfg\update-outcome.json""#),
            "{line}"
        );

        // Interior backslashes stay literal: doubling them would rename paths.
        let mut plan = elevated_stage_plan();
        plan.config_dir = Some(PathBuf::from(r"C:\Program Files\lyrel"));
        let line = plan
            .command_line()
            .into_string()
            .expect("the test paths are valid Unicode");
        assert!(
            line.contains(r#""--config-dir" "C:\Program Files\lyrel""#),
            "{line}"
        );

        // An embedded quote is escaped, and the run in front of it doubled.
        let mut plan = elevated_stage_plan();
        plan.config_dir = Some(PathBuf::from(r#"D:\ly"rel"#));
        let line = plan
            .command_line()
            .into_string()
            .expect("the test paths are valid Unicode");
        assert!(line.contains(r#""--config-dir" "D:\ly\"rel""#), "{line}");
    }

    #[test]
    fn tail_options_round_trip_through_argv_and_parse() {
        let options = TailOptions {
            config_dir: Some(PathBuf::from(r"C:\cfg")),
            result_file: Some(PathBuf::from(r"C:\cfg\update-outcome.json")),
            expected_sha256: Some("ab".repeat(32)),
            status_file: Some(PathBuf::from(r"C:\cfg\update-elevation.status")),
            app_path: Some(PathBuf::from(r"C:\lyrel\lyrel-app.exe")),
            log: Some(PathBuf::from(r"C:\cfg\update.log")),
            expected_version: Some("27.1.0".to_string()),
            gui_pid: Some(7),
        };
        let parsed = TailOptions::parse(options.argv().into_iter()).unwrap();
        assert_eq!(parsed, options);
    }

    #[test]
    fn capabilities_cover_elevation_matches_what_the_helper_prints() {
        let answer = lyrel_updater::ELEVATION_CAPABILITIES.join("\n");
        assert!(lyrel_updater::capabilities_cover_elevation(
            answer.as_bytes()
        ));
        // Order and unrelated tokens are the helper's business.
        assert!(lyrel_updater::capabilities_cover_elevation(
            b"install\nrelaunch-watcher\nelevated-stage\ninstall-elevated\n"
        ));
        // An old updater exits with a usage error: no tokens, no chain.
        assert!(!lyrel_updater::capabilities_cover_elevation(
            b"usage: lyrel-updater <command> [args]"
        ));
        assert!(!lyrel_updater::capabilities_cover_elevation(b""));
        // Two of the three verbs is not the chain.
        assert!(!lyrel_updater::capabilities_cover_elevation(
            b"elevated-stage\ninstall-elevated\n"
        ));
    }
}

/// Fail-closed ConPTY manifest verification at the public seam: the staged
/// portable payload carries the release-authenticated target manifest, and
/// every deviation — missing, malformed, future schema, or digest-mismatched
/// pair — is refused without falling back to anything.
#[cfg(target_os = "windows")]
mod conpty_contract {
    use std::fs;
    use std::path::Path;

    /// The pinned manifest bytes double as a valid target manifest: the
    /// staged pair beside this test executable is the pinned pair.
    const PINNED_MANIFEST: &[u8] = include_bytes!("../../../assets/windows/conpty/manifest.json");

    fn staged(name: &str) -> Vec<u8> {
        let directory = std::env::current_exe()
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf();
        fs::read(directory.join(name)).expect(
            "lyrel-core's build script stages the pinned ConPTY files beside every test executable",
        )
    }

    fn payload(root: &Path, manifest: &[u8]) {
        fs::create_dir_all(root).unwrap();
        fs::write(root.join("ConPTY-manifest.json"), manifest).unwrap();
        fs::write(root.join("conpty.dll"), staged("conpty.dll")).unwrap();
        fs::write(root.join("OpenConsole.exe"), staged("OpenConsole.exe")).unwrap();
        fs::write(
            root.join("LICENSE-ConPTY.txt"),
            staged("LICENSE-ConPTY.txt"),
        )
        .unwrap();
    }

    #[test]
    fn accepts_the_unchanged_pinned_manifest() {
        let root = tempfile::tempdir().unwrap();
        payload(root.path(), PINNED_MANIFEST);
        lyrel_updater::windows::verify_portable_conpty(root.path()).unwrap();
    }

    #[test]
    fn rejects_a_missing_target_manifest() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path()).unwrap();
        fs::write(root.path().join("conpty.dll"), staged("conpty.dll")).unwrap();
        fs::write(
            root.path().join("OpenConsole.exe"),
            staged("OpenConsole.exe"),
        )
        .unwrap();
        fs::write(
            root.path().join("LICENSE-ConPTY.txt"),
            staged("LICENSE-ConPTY.txt"),
        )
        .unwrap();
        let error = lyrel_updater::windows::verify_portable_conpty(root.path()).unwrap_err();
        assert!(error.contains("ConPTY-manifest.json"), "{error}");
    }

    #[test]
    fn rejects_a_corrupt_target_manifest() {
        let root = tempfile::tempdir().unwrap();
        payload(root.path(), b"not json");
        let error = lyrel_updater::windows::verify_portable_conpty(root.path()).unwrap_err();
        assert!(error.contains("invalid target ConPTY manifest"), "{error}");
    }

    #[test]
    fn rejects_a_future_manifest_schema() {
        let mut manifest: serde_json::Value = serde_json::from_slice(PINNED_MANIFEST).unwrap();
        manifest["schemaVersion"] = 2.into();
        let root = tempfile::tempdir().unwrap();
        payload(root.path(), &serde_json::to_vec(&manifest).unwrap());
        let error = lyrel_updater::windows::verify_portable_conpty(root.path()).unwrap_err();
        assert!(
            error.contains("unsupported target ConPTY manifest schema"),
            "{error}"
        );
    }

    #[test]
    fn rejects_a_pair_member_that_does_not_match_the_manifest_digest() {
        let mut manifest: serde_json::Value = serde_json::from_slice(PINNED_MANIFEST).unwrap();
        let digest = manifest["files"]["x64"]["conpty.dll"].as_str().unwrap();
        let mut tampered = digest.to_string();
        tampered.replace_range(0..2, if &digest[..2] == "AA" { "BB" } else { "AA" });
        manifest["files"]["x64"]["conpty.dll"] = tampered.into();
        let root = tempfile::tempdir().unwrap();
        payload(root.path(), &serde_json::to_vec(&manifest).unwrap());
        let error = lyrel_updater::windows::verify_portable_conpty(root.path()).unwrap_err();
        assert!(error.contains("SHA-256 mismatch"), "{error}");
    }

    #[test]
    fn rejects_a_missing_pair_member() {
        let root = tempfile::tempdir().unwrap();
        payload(root.path(), PINNED_MANIFEST);
        fs::remove_file(root.path().join("OpenConsole.exe")).unwrap();
        let error = lyrel_updater::windows::verify_portable_conpty(root.path()).unwrap_err();
        assert!(error.contains("OpenConsole.exe"), "{error}");
    }

    #[test]
    fn rejects_a_damaged_pair_member() {
        let root = tempfile::tempdir().unwrap();
        payload(root.path(), PINNED_MANIFEST);
        let mut damaged = staged("conpty.dll");
        let last = damaged.len() - 1;
        damaged[last] ^= 0xff;
        fs::write(root.path().join("conpty.dll"), damaged).unwrap();
        let error = lyrel_updater::windows::verify_portable_conpty(root.path()).unwrap_err();
        assert!(error.contains("SHA-256 mismatch"), "{error}");
    }

    #[test]
    fn rejects_a_manifest_without_a_pair_digest() {
        let mut manifest: serde_json::Value = serde_json::from_slice(PINNED_MANIFEST).unwrap();
        manifest["files"]["x64"]["conpty.dll"] = serde_json::Value::Null;
        let root = tempfile::tempdir().unwrap();
        payload(root.path(), &serde_json::to_vec(&manifest).unwrap());
        let error = lyrel_updater::windows::verify_portable_conpty(root.path()).unwrap_err();
        assert!(error.contains("missing a digest"), "{error}");
    }
}

/// `Path` is only referenced by the shared helpers above; keep the import
/// unified across platform cfgs.
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
#[test]
fn the_contract_is_desktop_only() {
    // On other platforms the library still compiles (outcome and shared
    // types only); there is no updater surface to exercise.
    assert!(true);
}
