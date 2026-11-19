//! Terminal consumer suites that stay in the root package because they need
//! the workbench shell: the whole-window `LyrelApp` harness, the real keymap,
//! and the tab/pane fixtures. Everything here drives the terminal client
//! through `lyrel_terminal`'s public and `test-support` surfaces.

/// The native-font terminal consumer, run by the `native_terminal` test
/// binary on the process main thread (see `lib.rs::run_native_terminal_tests`).
#[cfg(feature = "native-markdown-tests")]
pub(crate) mod native_terminal_tests;

/// Suites that attach a production `TerminalView` to the whole-window
/// `LyrelApp` harness, or drive real keystrokes through the root keymap.
#[cfg(test)]
mod whole_window {
    // The terminal-client surface these suites were written against: the
    // view and its events, the remote connection, and the rooted-consumer
    // graphics helpers.
    use lyrel_terminal::test_support::*;
    use lyrel_terminal::view::*;
    use lyrel_terminal::{RemoteTerminal, TermSize};
    use lyrel_terminal::remote::PtySource;
    // The workbench fixtures they additionally need.
    use crate::ui::app::LyrelApp;
    use crate::ui::app::test_window;
    use crate::ui::keymap;
    use crate::ui::pane::{self, Pane, PaneSlot};
    use crate::ui::tab::Tab;
    use gpui_kit::{ClipboardItem, TestAppContext, VisualTestContext};
    use lyrel_core::core::config::Config;
    use lyrel_ui_base::config as ui_config;
    use lyrel_ui_base::i18n;
    use lyrel_ui_base::notification_history;

    include!("whole_window/command_notification_consumer_tests.rs");
    include!("whole_window/program_status_consumer_tests.rs");
    include!("whole_window/program_summary_consumer_tests.rs");
    include!("whole_window/progress_consumer_tests.rs");
    include!("whole_window/title_consumer_tests.rs");
    include!("whole_window/program_attention_consumer_tests.rs");
    include!("whole_window/history_search_consumer_tests.rs");
    include!("whole_window/history_search_pty_consumer_tests.rs");
    include!("whole_window/keyboard_boundary_consumer_tests.rs");
    include!("keystroke_tests.rs");
    include!("snapshot_harness_tests.rs");
}
