#[cfg(unix)]
use super::gui_wire_path;
use crate::address::Context;
use crate::commands::Outcome;
use crate::commands::execute;
use crate::commands::test_support::cli;
use crate::commands::test_support::mock;
use crate::commands::test_support::run_cli;
use lyrel_core::daemon::control::ControlRequest;
use lyrel_core::daemon::control::ReplyOk;

#[test]
fn a_mistyped_subcommand_is_named_as_one_not_offered_to_the_gui() {
    for typo in ["tree", "statu", "pnae", "workspace"] {
        let err = execute(cli(&["lyrel", typo]), &Context::default(), &mut mock())
            .expect_err("a bare word is not a path");
        let msg = err.to_string();
        assert!(msg.contains("unknown subcommand"), "{typo}: {msg}");
        assert!(msg.contains(typo), "{typo}: {msg}");
    }
}

#[test]
fn a_path_asks_the_running_gui_to_open_an_absolute_directory() {
    let mut backend = mock();
    backend.replies.push_back(ReplyOk::Bool(true));
    let out = run_cli(&["lyrel", "."], &Context::default(), &mut backend);
    let expected = std::env::current_dir()
        .unwrap()
        .join(".")
        .to_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        backend.control_calls,
        vec![ControlRequest::GuiOpen {
            path: Some(expected.clone()),
            workspace: None,
        }]
    );
    let Outcome::Report(report) = out else {
        panic!("GUI open is a regular report");
    };
    assert_eq!(report.json["path"], expected);
    assert_eq!(report.json["delivered"], true);
    assert_eq!(report.json["launched"], false);
}

#[test]
fn an_invalid_gui_path_fails_before_touching_the_wire() {
    let mut backend = mock();
    let missing =
        std::env::temp_dir().join(format!("lyrel-cli-missing-path-{}", std::process::id()));
    let arg = missing.to_str().unwrap().to_owned();
    let err = execute(cli(&["lyrel", &arg]), &Context::default(), &mut backend)
        .expect_err("a missing directory must be rejected");
    assert!(err.to_string().contains("opening"), "{err:#}");
    assert!(backend.control_calls.is_empty());
}

#[cfg(unix)]
#[test]
fn a_non_utf8_gui_path_stays_off_the_string_protocol() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt as _;

    let path = std::env::temp_dir().join(OsString::from_vec(b"lyrel-\xff".to_vec()));
    assert_eq!(gui_wire_path(&path), None);
}

#[test]
fn the_gui_launcher_is_local_only() {
    let mut backend = mock();
    let err = execute(
        cli(&["lyrel", "-m", "devbox", "."]),
        &Context::default(),
        &mut backend,
    )
    .expect_err("a local GUI request cannot be routed to another machine");
    assert!(err.to_string().contains("cannot be combined"), "{err:#}");
    assert!(backend.control_calls.is_empty());
}
