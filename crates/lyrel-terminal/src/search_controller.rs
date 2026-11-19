//! The floating find bar's state and the debounced rescan lifecycle that keeps
//! its matches honest under output — one private deep module instead of eight
//! flat flags on [`crate::view::TerminalView`].
//!
//! Every transition that can invalidate a pending rescan (opening, closing,
//! retyping the query) funnels through [`TerminalSearch::invalidate_scan`],
//! and the debounce task in `crate::search` never touches the raw counters: it
//! reads a [`ScanTicket`] before each wait and asks
//! [`TerminalSearch::scan_outcome`] after it. A timer armed under a closed or
//! superseded bar therefore discards its completion instead of refreshing
//! matches the current session never asked for.

use super::search::SearchState;

/// Everything the find bar knows, and everything a pending rescan needs to
/// know about it.
///
/// The fields this type replaces lived flat on `TerminalView`, where any
/// handler could bump the scan epoch without disarming the latch, or drop the
/// session without outdating the debounce task that still meant to refresh it.
/// Behind one type each transition picks up the others' invariants for free:
/// closing the bar cannot forget to disarm the rescan, because disarming is
/// part of closing.
#[derive(Default)]
pub(crate) struct TerminalSearch {
    /// The live bar: its input field, the matches the last scan measured, and
    /// the scan's frame of reference. `None` while the bar is closed.
    session: Option<SearchState>,
    /// Whether the bar's input field currently holds the keyboard. Cleared
    /// when the bar closes; set from the input's focus/blur events.
    focused: bool,
    /// The `Aa` toggle. Survives close/reopen on purpose: a case decision is
    /// about what the user is looking for, not about the bar.
    case_sensitive: bool,
    /// The `.*` toggle: read the query as a regex instead of literal text.
    /// Survives close/reopen the way `case_sensitive` does.
    regex: bool,
    /// Whether the last scan's pattern failed to compile. Feedback for the
    /// bar's danger border; cleared on close and overwritten by the next scan.
    regex_error: bool,
    /// The query the bar closed on, so reopening lands back on the word the
    /// user was just looking for.
    last_query: String,
    /// Bumped by every wakeup that reaches an open bar, so the pending rescan
    /// can tell "the pane went quiet" from "more output landed".
    scan_epoch: u64,
    /// Whether a rescan is already waiting out the debounce. One task at a
    /// time, however fast the pane is printing.
    scan_armed: bool,
    /// Bumped whenever the session's identity changes — open, close, or a
    /// retype — so a rescan task armed under an older bar discards its
    /// completion outright instead of refreshing the new session's matches.
    scan_generation: u64,
}

impl TerminalSearch {
    /// The bar is open: a session owns its input and its matches.
    pub(crate) fn is_open(&self) -> bool {
        self.session.is_some()
    }

    pub(crate) fn session(&self) -> Option<&SearchState> {
        self.session.as_ref()
    }

    pub(crate) fn session_mut(&mut self) -> Option<&mut SearchState> {
        self.session.as_mut()
    }

    /// Whether the bar's input field holds the keyboard.
    pub(crate) fn focused(&self) -> bool {
        self.focused
    }

    pub(crate) fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
    }

    /// The query the bar closed on; what a reopening seeds its input with.
    pub(crate) fn last_query(&self) -> &str {
        &self.last_query
    }

    /// Whether the last scan's pattern failed to compile.
    pub(crate) fn has_regex_error(&self) -> bool {
        self.regex_error
    }

    pub(crate) fn set_regex_error(&mut self, failed: bool) {
        self.regex_error = failed;
    }

    /// The `Aa` toggle's position.
    pub(crate) fn is_case_sensitive(&self) -> bool {
        self.case_sensitive
    }

    pub(crate) fn toggle_case(&mut self) {
        self.case_sensitive = !self.case_sensitive;
    }

    /// The `.*` toggle's position.
    pub(crate) fn uses_regex(&self) -> bool {
        self.regex
    }

    pub(crate) fn toggle_regex(&mut self) {
        self.regex = !self.regex;
    }

    /// Install the bar's fresh session. A rescan task still waiting from a
    /// previous bar must not land on this one, so opening outdates it exactly
    /// as a close does.
    pub(crate) fn open(&mut self, session: SearchState) {
        self.invalidate_scan();
        self.session = Some(session);
    }

    /// Take the bar down. `query` is the session's closing input value, kept
    /// as the seed for the next opening; `None` only when the caller is
    /// closing a bar that is not there. The toggles and the persisted query
    /// survive on purpose — only the error feedback and the pending rescan
    /// are torn down with the session.
    pub(crate) fn close(&mut self, query: Option<String>) {
        if let Some(query) = query {
            self.last_query = query;
        }
        self.session = None;
        self.focused = false;
        self.regex_error = false;
        self.invalidate_scan();
    }

    /// Output landed under the open bar. Bumps the scan epoch; returns
    /// `true` only when the caller must arm a new debounce task, which is the
    /// moment the latch flips — a task is already waiting otherwise.
    pub(crate) fn note_output(&mut self) -> bool {
        self.scan_epoch = self.scan_epoch.wrapping_add(1);
        if self.scan_armed {
            return false;
        }
        self.scan_armed = true;
        true
    }

    /// What a pending debounced rescan reads before each of its waits: the
    /// two counters its completion is valid for.
    pub(crate) fn scan_ticket(&self) -> ScanTicket {
        ScanTicket {
            epoch: self.scan_epoch,
            generation: self.scan_generation,
        }
    }

    /// Decide what a debounce task whose wait just ended should do with its
    /// completion, given the counters it captured and how many windows it has
    /// already waited out.
    pub(crate) fn scan_outcome(
        &self,
        ticket: ScanTicket,
        waited: u32,
        lag_limit: u32,
    ) -> ScanOutcome {
        if ticket.generation != self.scan_generation {
            // The bar closed (and maybe reopened), or the user retyped, after
            // this task armed. The rescan it is holding would re-derive the
            // match position against a session that never measured this grid,
            // which can knock the user off the match they just stepped to.
            // Drop it silently.
            return ScanOutcome::Discard;
        }
        if ticket.epoch != self.scan_epoch && waited < lag_limit {
            // More output landed while the timer slept. Wait again, up to
            // `lag_limit` windows, so a pane that never pauses still gets one
            // rescan per lag limit instead of none.
            return ScanOutcome::Wait;
        }
        ScanOutcome::Settle
    }

    /// The debounce settled: drop the latch so the next wakeup arms a fresh
    /// task, then let the caller run the rescan.
    pub(crate) fn scan_settle(&mut self) {
        self.scan_armed = false;
    }

    /// Whether a rescan is waiting out the debounce.
    #[cfg(test)]
    pub(crate) fn scan_pending(&self) -> bool {
        self.scan_armed
    }

    /// Whatever the matches describe changed faster than output scrolls it:
    /// the bar opened or closed, or the user retyped or flipped a toggle. The
    /// pending debounce's completion is stale in a way more output cannot
    /// repair — outdate it and drop the latch.
    pub(crate) fn invalidate_scan(&mut self) {
        self.scan_epoch = self.scan_epoch.wrapping_add(1);
        self.scan_generation = self.scan_generation.wrapping_add(1);
        self.scan_armed = false;
    }
}

/// The two counters a pending debounced rescan captured, and against which
/// its completion is judged. While both still match, the bar this task armed
/// for is still the bar on screen and the pane has been quiet since.
#[derive(Clone, Copy)]
pub(crate) struct ScanTicket {
    epoch: u64,
    generation: u64,
}

/// What a debounce task whose wait just ended should do with its completion.
pub(crate) enum ScanOutcome {
    /// The pane stayed quiet and the bar is unchanged: run the rescan.
    Settle,
    /// More output landed, and the lag limit has not been reached: wait one
    /// more window.
    Wait,
    /// The bar's session outlived the task: drop the completion silently and
    /// stop looping.
    Discard,
}
