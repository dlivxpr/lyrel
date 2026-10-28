use super::*;
use std::path::Path;

    #[test]
    fn file_command_template_substitutes_path_line_and_column() {
        let argv = expand_file_command_template(
            "herdr edit {path} --line={line} --column={column}",
            Path::new("/tmp/foo.rs"),
            Some(42),
            Some(7),
        );
        assert_eq!(
            argv,
            vec!["herdr", "edit", "/tmp/foo.rs", "--line=42", "--column=7",]
        );
    }

    #[test]
    fn file_command_template_drops_tokens_for_absent_values() {
        let argv = expand_file_command_template(
            "herdr edit {path} --line={line} --column={column}",
            Path::new("/tmp/foo.rs"),
            None,
            None,
        );
        assert_eq!(argv, vec!["herdr", "edit", "/tmp/foo.rs"]);

        let argv = expand_file_command_template(
            "herdr edit {path} --line={line} --column={column}",
            Path::new("/tmp/foo.rs"),
            Some(42),
            None,
        );
        assert_eq!(argv, vec!["herdr", "edit", "/tmp/foo.rs", "--line=42"]);
    }

    #[test]
    fn file_command_template_keeps_path_only_token_and_unknown_placeholder() {
        let argv = expand_file_command_template(
            "code --goto {path}:{line} {other}",
            Path::new("/tmp/foo.rs"),
            None,
            None,
        );
        assert_eq!(argv, vec!["code", "--goto", "{other}"]);
    }

    /// #542's contract: a failed open is an `Err` the click site can toast,
    /// not a line in a logfile nobody is watching.
    #[test]
    fn a_file_command_that_cannot_spawn_comes_back_as_an_error() {
        let path = Path::new("/tmp/wherever.rs");
        // The binary does not exist, so the spawn itself fails.
        assert!(super::run_file_command("lyrel-no-such-binary {path}", path, None, None).is_err());
        // A template whose tokens all expand to nothing is a config error,
        // not a silent no-op.
        assert!(super::run_file_command("{line}", path, None, None).is_err());
    }

    /// "Open with Default App" sits beside "Open" only where it adds
    /// something: on a local file, and when "Open" is not already the OS
    /// association.
    #[test]
    fn the_default_app_row_is_offered_only_where_it_differs_from_open() {
        use super::{LinkFileOpen, offers_default_app_open};
        assert!(offers_default_app_open(true, LinkFileOpen::Internal));
        assert!(offers_default_app_open(true, LinkFileOpen::Command));
        assert!(
            !offers_default_app_open(true, LinkFileOpen::System),
            "it would repeat Open"
        );
        for mode in [
            LinkFileOpen::Internal,
            LinkFileOpen::System,
            LinkFileOpen::Command,
        ] {
            assert!(
                !offers_default_app_open(false, mode),
                "a remote file has nothing here for the OS to open ({mode:?})"
            );
        }
    }
