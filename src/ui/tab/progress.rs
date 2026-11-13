use super::Tab;
use crate::daemon::protocol::ProgressKind;
use crate::ui::i18n::{L10nKey, t, t_fmt};
use crate::ui::pane::Pane;
use gpui_kit::{App, SharedString};

impl Tab {
    /// This slice deliberately has no split-tab aggregation policy.
    pub(crate) fn progress_label(&self, cx: &App) -> Option<(ProgressKind, SharedString)> {
        let Pane::Leaf(slot) = &self.pane else {
            return None;
        };
        let state = slot.terminal()?.read(cx).progress()?;
        let key = match (state.kind, state.percent.is_some()) {
            (ProgressKind::Running, false) => L10nKey::TabProgressRunning,
            (ProgressKind::Running, true) => L10nKey::TabProgressRunningPercent,
            (ProgressKind::Indeterminate, false) => L10nKey::TabProgressIndeterminate,
            (ProgressKind::Indeterminate, true) => L10nKey::TabProgressIndeterminatePercent,
            (ProgressKind::Paused, false) => L10nKey::TabProgressPaused,
            (ProgressKind::Paused, true) => L10nKey::TabProgressPausedPercent,
            (ProgressKind::Error, false) => L10nKey::TabProgressError,
            (ProgressKind::Error, true) => L10nKey::TabProgressErrorPercent,
        };
        let label = match state.percent {
            Some(percent) => t_fmt(key, &[("percent", &percent.to_string())]).into(),
            None => t(key).into(),
        };
        Some((state.kind, label))
    }
}
