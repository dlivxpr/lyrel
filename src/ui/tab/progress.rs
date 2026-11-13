use super::Tab;
use crate::daemon::protocol::{PaneProgress, ProgressKind};
use crate::ui::i18n::{L10nKey, t, t_fmt};
use gpui_kit::{App, SharedString};

impl Tab {
    /// The tab's program-reported progress, summarized from every terminal
    /// leaf it owns — focused or not, painted or parked.
    ///
    /// Reading the leaves themselves rather than the pane on screen is what
    /// makes a background split report its work, and it also means membership
    /// changes need no second state store: a closed, moved, parked or restored
    /// pane is simply a different set of leaves on the next read. A sleeping
    /// tab owns no leaves and has no live progress to show; waking it reads the
    /// daemon baseline each new attachment replays.
    pub(crate) fn progress_label(&self, cx: &App) -> Option<(ProgressKind, SharedString)> {
        let (kind, percent) = summarize_progress(
            self.pane
                .terminals()
                .into_iter()
                .filter_map(|leaf| leaf.read(cx).progress()),
        )?;
        let key = match (kind, percent.is_some()) {
            (ProgressKind::Running, false) => L10nKey::TabProgressRunning,
            (ProgressKind::Running, true) => L10nKey::TabProgressRunningPercent,
            (ProgressKind::Indeterminate, false) => L10nKey::TabProgressIndeterminate,
            (ProgressKind::Indeterminate, true) => L10nKey::TabProgressIndeterminatePercent,
            (ProgressKind::Paused, false) => L10nKey::TabProgressPaused,
            (ProgressKind::Paused, true) => L10nKey::TabProgressPausedPercent,
            (ProgressKind::Error, false) => L10nKey::TabProgressError,
            (ProgressKind::Error, true) => L10nKey::TabProgressErrorPercent,
        };
        let label = match percent {
            Some(percent) => t_fmt(key, &[("percent", &percent.to_string())]).into(),
            None => t(key).into(),
        };
        Some((kind, label))
    }
}

/// Fold the leaves' current reports into the one line a tab can honestly show.
///
/// A single task keeps its exact state and the percentage its sender supplied.
/// Several tasks are never averaged into a number no sender reported: an error
/// outranks everything, running work outranks indeterminate work, and a tab
/// whose only tasks are paused stays paused. Percentages disappear as soon as
/// more than one task is represented, because no leaf owns the aggregate.
pub(super) fn summarize_progress(
    states: impl IntoIterator<Item = PaneProgress>,
) -> Option<(ProgressKind, Option<u8>)> {
    let mut count = 0usize;
    let mut single = None;
    let mut any_error = false;
    let mut any_running = false;
    let mut any_indeterminate = false;
    for state in states {
        count += 1;
        if count == 1 {
            single = Some(state);
        }
        match state.kind {
            ProgressKind::Error => any_error = true,
            ProgressKind::Running => any_running = true,
            ProgressKind::Indeterminate => any_indeterminate = true,
            ProgressKind::Paused => {}
        }
    }
    if count == 0 {
        return None;
    }
    if count == 1 {
        let state = single?;
        return Some((state.kind, state.percent));
    }
    let kind = if any_error {
        ProgressKind::Error
    } else if any_running {
        ProgressKind::Running
    } else if any_indeterminate {
        ProgressKind::Indeterminate
    } else {
        ProgressKind::Paused
    };
    Some((kind, None))
}
