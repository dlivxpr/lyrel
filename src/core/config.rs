//! The GUI config global and test guards now live in `lyrel-ui-base`; this
//! module is the root package's explicit facade over them, re-exported by
//! name so nothing here glob-shadows the protocol's `config`.

pub use lyrel_ui_base::config::{Config, LoadOutcome, gpui_font_features};

#[cfg(test)]
pub use lyrel_ui_base::config::CoreConfig;

#[cfg(any(test, feature = "native-markdown-tests"))]
pub(crate) use lyrel_ui_base::config::pin_test_config_dir;

// The pure protocol configuration surface the GUI consumes, named explicitly
// (no glob) so the wrapper `Config` above is the only `Config` in this module.
pub use lyrel_core::core::config::{
    BellMode, ConfirmClose, CursorStyle, DOCUMENT_RATIO_HALF, DOCUMENT_RATIO_MAX,
    DOCUMENT_RATIO_MIN, DOCUMENT_RATIO_STOPS, DOCUMENT_RATIO_THIRD, DOCUMENT_RATIO_TWO_THIRDS,
    DiffViewMode, DocumentLayout, FONT_SIZE_MAX, FONT_SIZE_MIN, FontFeatures, KeybindingOverride,
    LINE_HEIGHT_MAX, LINE_HEIGHT_MIN, LinkFileOpen, MAX_SCROLLBACK, MotionPreference,
    MouseZoomModifier, NewTabPosition, NotifyMode, ProfileUsage, PromptCursorStyle, RightPanelTab,
    SERVERS_FILE, ShellConfig, StartupMode, UI_FONT_SIZE_DEFAULT, UI_FONT_SIZE_MAX,
    UI_FONT_SIZE_MIN, WdStrategy, WindowBackdrop, config_dir_path, config_path, detection_aliases,
    normalize_gui_language, set_config_dir, unix_now, write_atomic, write_atomic_private,
};
