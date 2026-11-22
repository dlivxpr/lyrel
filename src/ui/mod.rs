pub mod agent_launch;
pub mod app;
pub mod assets;
#[cfg(target_os = "windows")]
pub mod backdrop;
#[cfg(any(test, feature = "native-markdown-tests"))]
pub mod code_editor;
pub mod desktop_lookups;
pub mod dialog;
pub mod diff_list;
pub mod diff_overlay;
pub(crate) mod diff_probe;
pub mod diff_rows;
pub mod document_column;
pub mod file_copy;
pub mod file_tree;
pub mod forwards;
pub mod github;
pub mod hints;
pub mod home;
pub(crate) mod host_metrics;
pub(crate) mod ime;
pub mod keymap;
pub mod local_link;
pub mod machine_mirror;
pub(crate) mod motion;
#[cfg(test)]
mod navigation_consumer_tests;
pub mod notice;
pub(crate) mod notification_history;
pub(crate) mod palette;
pub mod pane;
pub mod pane_drag;
pub(crate) mod pane_liveness;
pub(crate) mod pane_sidecar;
pub mod panel_github;
pub(crate) mod panel_monitor;
#[cfg(test)]
mod panel_monitor_tests;
pub mod panel_search;
pub(crate) mod panel_slide;
pub(crate) mod parked_deck;
pub mod pending_pane;
pub mod perf;
pub(crate) mod pip_geometry;
#[cfg(target_os = "macos")]
pub mod platform;
pub mod proc_usage;
pub(crate) mod program_attention;
pub(crate) mod program_status;
pub(crate) mod program_summary;
pub mod prompt;
pub mod remote_connect;
pub mod remote_workspace;
pub mod reorder;
pub mod right_panel;
pub mod scm;
pub mod scm_snapshot;
pub mod search;
pub(crate) mod session_history;
pub(crate) mod session_lifecycle;
pub mod settings;
pub mod settings_window;
pub mod sftp;
pub mod sftp_host;
pub mod ssh_connect;
pub mod ssh_prompt;
pub(crate) mod status_bar;
pub mod switcher;
pub(crate) mod tab;
pub mod tab_sidebar;
pub mod tab_strip;
pub mod theme;
pub(crate) mod theme_transition;
pub mod tray;
pub mod tree_sync;
mod window_chrome;
pub mod windows;
pub mod worktree_picker;
pub mod worktree_prompt;

/// The two answers a confirmation dialog gets, arranged the way macOS arranges
/// them: the action on the right, where Return lands and where every other app
/// on the machine puts it, and the safe answer on the left, marked as the
/// cancel so it also answers Escape, Space and the initial keyboard focus.
///
/// gpui renders answer 0 rightmost, so the action goes first — `Ok(0)` is
/// "they meant it" and everything else, including a dropped channel, is "leave
/// it alone". The implementation lives in `lyrel-ui-base`.
pub(crate) use lyrel_ui_base::dialog::confirm_answers;

#[cfg(test)]
mod tests {
    #[test]
    fn the_action_answers_return_and_the_safe_one_answers_escape() {
        // gpui hands answer 0 to the platform first, and both NSAlert and
        // TaskDialog draw that one on the right and give it Return. Reversing
        // these two puts Delete under the mouse where Cancel belongs.
        let [action, keep] = super::confirm_answers("Delete", "Cancel");
        assert_eq!(action.label(), "Delete");
        assert!(!action.is_cancel(), "answer 0 has to keep Return");
        assert_eq!(keep.label(), "Cancel");
        assert!(keep.is_cancel(), "only a cancel answer is given Escape");
    }
}
