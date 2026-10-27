use super::*;

    /// Which machine's spelling a pane's paths are read in. Ungated on
    /// purpose: the bug this settles was a Windows-only one that hid behind a
    /// `#[cfg(unix)]` on the test that covered it.
    #[test]
    fn a_panes_paths_are_read_in_its_own_hosts_spelling() {
        use crate::terminal::search::PathStyle;
        use std::path::Path;

        assert_eq!(
            link_path_style(true, Some(Path::new("/home/u/proj"))),
            PathStyle::NATIVE,
            "a pane on this machine reads its own output this OS's way, \
             whatever its shell spells the cwd like"
        );
        assert_eq!(
            link_path_style(false, Some(Path::new("/home/u/proj"))),
            PathStyle::Posix,
            "an SSH host, a remote workspace or a WSL distro reporting a \
             /-rooted cwd is a POSIX one on every client"
        );
        assert_eq!(
            link_path_style(false, Some(Path::new(r"C:\Users\u\proj"))),
            PathStyle::Windows,
            "and a remote Windows host is not"
        );
        assert_eq!(
            link_path_style(false, None),
            PathStyle::Posix,
            "a remote pane that has not said where it is still has no local \
             drive its paths could hang off"
        );
    }

    #[test]
    fn right_click_opens_the_host_menu_when_the_app_is_not_reporting_mouse() {
        assert!(should_show_context_menu(false, false));
        assert!(should_show_context_menu(false, true));
    }

    #[test]
    fn right_click_belongs_to_the_app_while_mouse_reporting_is_active() {
        assert!(
            !should_show_context_menu(true, false),
            "an unmodified right-click in mouse mode is the application's event, \
             so lyrel must not also pop its own menu over it"
        );
    }

    #[test]
    fn shift_right_click_stays_the_escape_hatch_in_mouse_mode() {
        assert!(
            should_show_context_menu(true, true),
            "Shift is the documented way to reach lyrel's own menu inside a \
             mouse-reporting TUI"
        );
    }

    #[test]
    fn context_menu_and_mouse_report_never_fire_for_the_same_click() {
        // The render-time menu gate and the mouse-down forwarder in
        // `TerminalElement` read the same predicate from opposite sides, so
        // every (mouse_mode, shift) pair must land in exactly one of them.
        for mouse_mode in [false, true] {
            for shift in [false, true] {
                let menu = should_show_context_menu(mouse_mode, shift);
                let reported = mouse_mode && !shift;
                assert_ne!(
                    menu, reported,
                    "mouse_mode={mouse_mode} shift={shift} must route to exactly one consumer"
                );
            }
        }
    }

