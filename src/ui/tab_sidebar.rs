use gpui_kit::TestSupportExt as _;
use gpui_kit::base::animation::ease_out_cubic;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent};
use gpui_kit::component::menu::{ContextMenuExt as _, PopupMenuItem};
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::{
    Animation, AnimationExt as _, AnyElement, Axis, Bounds, Context, ExternalPaths, FontWeight,
    KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Orientation, Pixels,
    Role, SharedString, Window, canvas, deferred, div, ease_out_quint, linear_color_stop,
    linear_gradient, prelude::*, px,
};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::rc::Rc;
use std::time::Instant;

use std::path::{Path, PathBuf};

use lyrel_core::core::machine::TabId;

use crate::core::group_key::{
    AutoKey, GroupId, GroupKey, PinnedGroup, WorkspaceGroups, auto_key, auto_names, pinned_names,
    place,
};
use crate::ui::app::LyrelApp;
use crate::ui::hints::tab_badge_label;
use crate::ui::reorder::{self, Reorder, Surface};
use crate::ui::right_panel::RESIZE_HANDLE_WIDTH;
use crate::ui::scm::status_cache::GitStatusCache;
use crate::ui::tab::Tab;
use crate::ui::tab_strip::{
    DragTab, REORDER_SLIDE_MS, abbreviate_home, elide_end_clusters, elide_label,
    elide_path_keep_tail, measure_text,
};
use lyrel_ui_base::config::Config;
use lyrel_ui_base::i18n::{L10nKey, t, t_fmt};

pub(crate) const MIN_SIDEBAR_WIDTH: f32 = 180.;

const ROW_GAP: f32 = 1.;

/// What marks a kept group, and what the header button that keeps one reads.
///
/// A pin, drawn: it names what was done to the group, where the `◆` it
/// replaced only said "different". With no line between kept and derived
/// groups it is also the only thing that tells the two apart, so every kept
/// group carries it.
const PIN_MARK: &str = "icons/pin.svg";

/// The mark's size: the header's 11px text, like a row's branch icon.
const PIN_MARK_SIZE: f32 = 11.;

/// A single-line tab row: one line of `text_sm` and a little air.
const ROW_HEIGHT: f32 = 30.;

/// A row with a branch line under its title.
const ROW_HEIGHT_TWO_LINE: f32 = 42.;

/// The air between two groups — what makes a group a group without a box.
const GROUP_GAP: f32 = 16.;

/// A group heading's line.
const HEADER_HEIGHT: f32 = 22.;

/// The group heading and the row's branch line: 11.5px at the default size.
const META_REM: f32 = 11.5 / 16.;

/// What sits between a group's branch and its diff counts.
const META_SEP_TRIMMED: &str = "·";

/// A keyboard stop in the terminal-tab sidebar: one visible tab row, or the
/// disclosure header of one visible group. The header is included so a folded
/// group stays keyboard-operable through the control that folds it; its rows
/// are not, because they are not drawn.
///
/// Identity is the domain object — a tab's machine id or a group's key (and
/// `None` for Ungrouped) — never a screen index. Insertion, close, regrouping
/// and reorder move slots without renaming what focus is standing on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SidebarNavTarget {
    Tab(lyrel_core::core::machine::TabId),
    Group(Option<GroupKey>),
}

/// Tabular numerals, so a column of diff counts lines up digit for digit.
fn tabular() -> gpui_kit::FontFeatures {
    gpui_kit::FontFeatures(std::sync::Arc::new(vec![("tnum".to_string(), 1)]))
}

/// The row chrome the text budget has to be measured around. These are the
/// numbers the layout below is built from, not a second guess at it — a row
/// that elides against a budget wider than it really has falls back to CSS
/// truncation, which drops the tail this whole module exists to keep.
mod row_metrics {
    /// `border_r_1` on the sidebar itself.
    pub(super) const BORDER: f32 = 1.;
    /// `px_3` on the scrolling list that holds the rows.
    pub(super) const LIST_PAD: f32 = 12.;
    /// `pl_2` + `pr_2` on the row.
    pub(super) const ROW_PAD: f32 = 8.;
    /// The avatar handed to `tab_avatar_plain`.
    pub(super) const AVATAR: f32 = 16.;
    /// `gap_2p5` between the row's children.
    pub(super) const GAP: f32 = 10.;
    /// The trailing status mark, when the row has one.
    pub(super) const STATUS: f32 = crate::ui::tab_strip::ROW_STATUS_SLOT;
    /// The ⌘N badge, when one is shown.
    pub(super) const BADGE: f32 = 20.;
    /// The zoom mark, when the tab has a pane zoomed over the others.
    pub(super) const ZOOM: f32 = 16.;
    /// `gap_1p5`, between a branch and the counts after it.
    pub(super) const META_GAP: f32 = 6.;
    /// `px_2` on a group header.
    pub(super) const HEADER_PAD: f32 = 8. + 8.;
    /// The chevron a header opens with, and the pin that marks a folder
    /// group: both `xsmall` icons, which resolve to 12px.
    pub(super) const HEADER_ICON: f32 = 12.;

    /// What a row can spend on text, before the badge is taken out.
    pub(super) const fn text_budget(width: f32) -> f32 {
        width - BORDER - 2. * LIST_PAD - 2. * ROW_PAD - AVATAR - GAP
    }

    /// What a group header can spend on its name and the branch beside it.
    /// The chevron, the pin and the folded row count come off at the call
    /// site, which knows whether they are drawn — an open group draws no
    /// chevron, and reserving one anyway elided its branch with 18px to spare.
    pub(super) const fn header_budget(width: f32) -> f32 {
        width - BORDER - 2. * LIST_PAD - HEADER_PAD
    }
}

/// The narrowest a group's heading is allowed to get: three or four capitals
/// and an ellipsis, which is still a name and not a stub.
const HEADER_NAME_FLOOR: f32 = 40.;

/// The narrowest a row's title is allowed to get before the working directory
/// beside it stops taking room, and the narrowest that path may be drawn at:
/// below this it is an ellipsis and a slash, which names no directory.
const ROW_TITLE_FLOOR: f32 = 48.;
const ROW_CWD_FLOOR: f32 = 24.;

/// How a group header divides its line between the heading and the branch its
/// rows share. The branch takes what it wants up to half the line, and the
/// heading keeps the rest — so a long branch can no longer crush the name
/// (flex used to hand the overflow to them in proportion to what each asked
/// for, which gave the longer string the smaller cut), and a long group name
/// cannot crush the branch in return. `git_want` is `None` for a header with
/// no shared branch on it, which then owns the whole line.
fn header_name_avail(avail: f32, git_want: Option<f32>) -> f32 {
    match git_want {
        Some(want) => (avail - want.min(avail * 0.5)).max(HEADER_NAME_FLOOR),
        None => avail,
    }
}

/// What a diff's counts occupy on a line, measured against real glyphs: the
/// two numbers, the gap between them when both are drawn, and the gap that
/// separates them from the branch. They never wrap and never shrink, so this
/// is the width a branch has to be elided around — on a row and on the group
/// header that lifts the branch off its rows alike.
fn counts_width(
    ts: &gpui_kit::WindowTextSystem,
    font: &gpui_kit::Font,
    size: f32,
    status: &crate::ui::scm::status_cache::GitStatus,
) -> f32 {
    let mut w = 0.;
    if status.added > 0 {
        w += measure_text(ts, font, size, &format!("+{}", status.added));
    }
    if status.removed > 0 {
        w += measure_text(ts, font, size, &format!("−{}", status.removed));
    }
    if status.added > 0 && status.removed > 0 {
        w += row_metrics::META_GAP;
    }
    if w > 0. {
        w += row_metrics::META_GAP;
    }
    w
}

/// The branch a whole group shares, lifted off its rows and onto its header.
struct SharedGit {
    status: crate::ui::scm::status_cache::GitStatus,
    /// Where a click on the counts opens the diff overlay, if the setting
    /// allows one.
    click: Option<(lyrel_ui_base::host_ops::HostId, PathBuf)>,
    /// Every row the group counts, drawn or folded away.
    rows: Vec<usize>,
}

/// What a sidebar row rendered, next to what it had to leave out, so the
/// hover card can be built by comparison instead of deriving the same strings
/// a second time — the two derivations have to agree, and the shortest way to
/// guarantee that is to only ever have one.
struct SidebarRowShown {
    /// The elided title, and the full string it came from. `None` when the
    /// row is showing a placeholder (`Shell 3`) rather than a real title,
    /// which nothing can expand.
    title: Option<(SharedString, SharedString)>,
    branch: Option<(SharedString, SharedString, u32, u32)>,
    cwd: Option<(SharedString, SharedString)>,
}

#[derive(Clone)]
pub(crate) struct DragGroup;

impl Render for DragGroup {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

// ── Session-row and group disclosure motion ─────────────────────────────
//
// Behavioral reference: Diri's `sidebar/row_motion.rs` and
// `sidebar/disclosure.rs` at the pinned commit recorded in
// docs/research/diri-right-panel-motion.md, independently implemented on
// GPUI Kit. The model is pure: render feeds it the executor's clock and
// paints what it returns, and a short task keeps frames coming only while
// something owes one.

/// A row arriving opens its slot by height before its content fades in, so
/// text never appears squashed into a sliver; a row leaving fades its
/// content first and reclaims the slot right behind it.
const ROW_ENTER_MS: u64 = 220;
const ROW_EXIT_MS: u64 = 200;
/// More rows than this in motion at once is a bulk change — a project
/// closed, an import, a resync — and everything lands at once instead.
const ROW_MOTION_BULK: usize = 4;
/// Share of the arrival spent opening the gap before content starts to show.
const ENTER_CONTENT_DELAY: f32 = 0.35;
/// Share of the departure spent fading content out, and where the gap starts
/// to close. They overlap a little so the exit reads as one gesture.
const EXIT_CONTENT: f32 = 0.4;
const EXIT_COLLAPSE_DELAY: f32 = 0.2;

/// A group opening reveals its rows with a growing clip while their short,
/// staggered arrivals settle; shutting fades every row together, sooner.
const DISCLOSE_OPEN_MS: u64 = 300;
const DISCLOSE_CLOSE_MS: u64 = 180;
/// 22 ms between arrivals on open, capped so a large group still finishes
/// inside the one 300 ms gesture.
const DISCLOSE_STAGGER: f32 = 0.073;
const DISCLOSE_STAGGER_CAP: f32 = 0.36;

/// How often the frame task below re-renders while a gesture is owed.
const SIDEBAR_MOTION_TICK_MS: u64 = 16;

/// What one row paints at one instant: the share of its natural height its
/// slot takes, and how loudly its content speaks inside that slot.
#[derive(Clone, Copy, Debug, PartialEq)]
struct RowPresence {
    height: f32,
    opacity: f32,
}

impl RowPresence {
    const FULL: RowPresence = RowPresence {
        height: 1.,
        opacity: 1.,
    };
    const GONE: RowPresence = RowPresence {
        height: 0.,
        opacity: 0.,
    };
}

/// A row's membership change: it was not in the sidebar and now is, or the
/// reverse. Folding, regrouping and reordering only hide and show rows the
/// list already knows, so they never animate here.
#[derive(Clone, Copy, Debug, PartialEq)]
enum RowMotionKind {
    Enter,
    Exit,
}

#[derive(Clone, Copy, Debug)]
struct RowMotionEntry {
    kind: RowMotionKind,
    started: Instant,
    /// What was on screen when this motion began, so a reversal turns around
    /// where the row stands instead of jumping to an old endpoint.
    from: RowPresence,
}

impl RowMotionEntry {
    fn duration(&self) -> std::time::Duration {
        std::time::Duration::from_millis(match self.kind {
            RowMotionKind::Enter => ROW_ENTER_MS,
            RowMotionKind::Exit => ROW_EXIT_MS,
        })
    }

    fn done(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.started) >= self.duration()
    }

    fn sample(&self, now: Instant) -> RowPresence {
        let progress = (now.saturating_duration_since(self.started).as_secs_f32()
            / self.duration().as_secs_f32())
        .clamp(0., 1.);
        // The shared curve never overshoots: a row pushing its neighbours
        // never drags them past their resting place.
        let curve = ease_out_quint();
        let from = self.from;
        match self.kind {
            RowMotionKind::Enter => {
                let phase =
                    ((progress - ENTER_CONTENT_DELAY) / (1. - ENTER_CONTENT_DELAY)).clamp(0., 1.);
                RowPresence {
                    height: from.height + (1. - from.height) * curve(progress),
                    opacity: from.opacity + (1. - from.opacity) * curve(phase),
                }
            }
            RowMotionKind::Exit => {
                let fade = curve((progress / EXIT_CONTENT).clamp(0., 1.));
                let collapse = curve(
                    ((progress - EXIT_COLLAPSE_DELAY) / (1. - EXIT_COLLAPSE_DELAY)).clamp(0., 1.),
                );
                RowPresence {
                    height: from.height * (1. - collapse),
                    opacity: from.opacity * (1. - fade),
                }
            }
        }
    }
}

/// One slot of a group's rows while something in it moves: a row that is
/// still there, or a ghost of one that left, woven back in where it last
/// stood.
#[derive(Clone, Copy, Debug, PartialEq)]
enum RowSlot {
    Real(usize, RowPresence),
    Ghost(TabId, RowPresence),
}

#[derive(Default)]
struct SidebarRowMotion {
    /// Every tab the sidebar held at the last observation, in tab order.
    /// `None` until the first observation, which is a baseline and never
    /// animates.
    known: Option<Vec<TabId>>,
    entries: HashMap<TabId, RowMotionEntry>,
    /// What each group's rows last laid out, ghosts included, so a leaving
    /// row has a place to fade from. A container no frame laid out — one
    /// folded away — is dropped with it.
    laid_out: HashMap<u64, (u64, Vec<TabId>)>,
    pass: u64,
}

impl SidebarRowMotion {
    /// Records which tabs exist now. `animate` is false under Reduce Motion
    /// and on the baseline observation: the change lands and nothing
    /// replays later. Most renders replay the same membership, which costs
    /// one compare, not a walk.
    fn observe(&mut self, current: &[TabId], animate: bool, now: Instant) {
        if !animate {
            self.entries.clear();
        }
        if let Some(known) = &self.known
            && known == current
        {
            return;
        }
        let known = self.known.replace(current.to_vec());
        let Some(known) = known else {
            return;
        };
        if !animate {
            return;
        }
        self.entries.retain(|_, entry| !entry.done(now));
        for &id in current.iter().filter(|id| !known.contains(id)) {
            let from = self
                .entries
                .get(&id)
                .map_or(RowPresence::GONE, |entry| entry.sample(now));
            self.entries.insert(
                id,
                RowMotionEntry {
                    kind: RowMotionKind::Enter,
                    started: now,
                    from,
                },
            );
        }
        for id in known.iter().filter(|id| !current.contains(id)) {
            // A row that was not on screen — folded away — has nothing to
            // collapse.
            let on_screen = self.laid_out.values().any(|(_, rows)| rows.contains(id));
            if !on_screen {
                self.entries.remove(id);
                continue;
            }
            let from = self
                .entries
                .get(id)
                .map_or(RowPresence::FULL, |entry| entry.sample(now));
            self.entries.insert(
                *id,
                RowMotionEntry {
                    kind: RowMotionKind::Exit,
                    started: now,
                    from,
                },
            );
        }
        if self.entries.len() > ROW_MOTION_BULK {
            self.entries.clear();
        }
    }

    /// Starts a frame: finished motions are forgotten, so a list at rest
    /// allocates nothing for them.
    fn begin_frame(&mut self, now: Instant) {
        self.entries.retain(|_, entry| !entry.done(now));
        self.pass += 1;
    }

    /// Ends a frame: a container nothing laid out is off screen, and its
    /// rows have nothing left to collapse.
    fn end_frame(&mut self) {
        let pass = self.pass;
        self.laid_out.retain(|_, (p, _)| *p == pass);
    }

    /// The presence of a row still in the list; rows with no entry are
    /// simply full.
    fn presence(&self, id: TabId, now: Instant) -> RowPresence {
        match self.entries.get(&id) {
            Some(entry) if entry.kind == RowMotionKind::Enter => entry.sample(now),
            _ => RowPresence::FULL,
        }
    }

    /// A leaving row's presence, while its exit is still running.
    fn ghost(&self, id: TabId, now: Instant) -> Option<RowPresence> {
        match self.entries.get(&id) {
            Some(entry) if entry.kind == RowMotionKind::Exit && !entry.done(now) => {
                Some(entry.sample(now))
            }
            _ => None,
        }
    }

    /// Lays out one group's rows at `now`: the current rows with their
    /// presences, and every leaving row of this container woven back in
    /// after the row it last followed. `ids` is the display order, which
    /// the identity unless a drag preview has the section — previews own
    /// their section's geometry and motion yields to them.
    fn slots(&mut self, container: u64, ids: &[TabId], now: Instant) -> Vec<RowSlot> {
        let pass = self.pass;
        let mut out: Vec<RowSlot> = ids
            .iter()
            .enumerate()
            .map(|(ix, id)| RowSlot::Real(ix, self.presence(*id, now)))
            .collect();
        if !self.entries.is_empty() {
            let previous = self
                .laid_out
                .get(&container)
                .map(|(_, rows)| rows.clone())
                .unwrap_or_default();
            let mut placed: Vec<TabId> = ids.to_vec();
            for (position, id) in previous.iter().enumerate() {
                let Some(presence) = self.ghost(*id, now) else {
                    continue;
                };
                if placed.contains(id) {
                    continue;
                }
                let at = previous[..position]
                    .iter()
                    .rev()
                    .find_map(|before| placed.iter().position(|p| p == before))
                    .map_or(0, |ix| ix + 1);
                placed.insert(at, *id);
                out.insert(at, RowSlot::Ghost(*id, presence));
            }
        }
        let record: Vec<TabId> = out
            .iter()
            .map(|slot| match slot {
                RowSlot::Real(ix, _) => ids[*ix],
                RowSlot::Ghost(id, _) => *id,
            })
            .collect();
        let entry = self.laid_out.entry(container).or_insert((pass, Vec::new()));
        entry.0 = pass;
        entry.1 = record;
        out
    }
}

/// One group disclosure's sampled instant: how much of the rows' column the
/// clip reveals, and how loudly each row speaks. Rows stay at their natural
/// size throughout — the clip, not a height change, shows and hides them.
#[derive(Clone, Debug, PartialEq)]
struct DisclosureFrame {
    reveal: f32,
    rows: Vec<f32>,
    animating: bool,
}

#[derive(Clone, Debug)]
struct DisclosureMotion {
    expanded: bool,
    started: Instant,
    from: DisclosureFrame,
}

impl DisclosureMotion {
    fn new(expanded: bool, count: usize, now: Instant) -> Self {
        Self {
            expanded,
            started: now,
            from: Self::settled(expanded, count),
        }
    }

    fn settled(expanded: bool, count: usize) -> DisclosureFrame {
        let value = if expanded { 1. } else { 0. };
        DisclosureFrame {
            reveal: value,
            rows: vec![value; count],
            animating: false,
        }
    }

    /// `expanded` is whether the group draws its rows, `count` how many it
    /// has — data changes resize the gesture's rows without replaying it.
    fn update(
        &mut self,
        expanded: bool,
        count: usize,
        now: Instant,
        reduce_motion: bool,
    ) -> DisclosureFrame {
        if reduce_motion {
            self.expanded = expanded;
            self.from = Self::settled(expanded, count);
        } else if expanded != self.expanded {
            self.from = self.sample(now);
            self.from.rows.resize(count, self.from.reveal);
            self.from.animating = true;
            self.expanded = expanded;
            self.started = now;
        } else if self.from.rows.len() != count {
            // Data changes are not disclosure gestures: keep new and removed
            // rows current without replaying an arrival on every update.
            self.from = Self::settled(expanded, count);
        }
        let frame = self.sample(now);
        if !frame.animating {
            self.from = frame.clone();
        }
        frame
    }

    fn sample(&self, now: Instant) -> DisclosureFrame {
        if !self.from.animating {
            return self.from.clone();
        }
        let duration = if self.expanded {
            DISCLOSE_OPEN_MS
        } else {
            DISCLOSE_CLOSE_MS
        };
        let progress = (now.saturating_duration_since(self.started).as_secs_f32()
            / std::time::Duration::from_millis(duration).as_secs_f32())
        .clamp(0., 1.);
        let target = if self.expanded { 1. } else { 0. };
        let interpolate = |from: f32, t: f32| from + (target - from) * ease_out_cubic(t);
        DisclosureFrame {
            reveal: interpolate(self.from.reveal, progress),
            rows: self
                .from
                .rows
                .iter()
                .enumerate()
                .map(|(index, from)| {
                    let delay = if self.expanded {
                        (index as f32 * DISCLOSE_STAGGER).min(DISCLOSE_STAGGER_CAP)
                    } else {
                        0.
                    };
                    let local = ((progress - delay) / (1. - delay)).clamp(0., 1.);
                    interpolate(*from, local)
                })
                .collect(),
            animating: progress < 1.,
        }
    }

    fn animating(&self, now: Instant) -> bool {
        self.from.animating && self.sample(now).animating
    }
}

/// What a leaving row last showed: the exact strings it drew, enough to
/// repaint it fading out without re-deriving anything from a tab that no
/// longer exists.
#[derive(Clone)]
struct RowGhost {
    /// The title exactly as drawn — placeholder included.
    title: SharedString,
    /// The elided working directory beside the title, when there was one.
    cwd: Option<SharedString>,
    /// The elided branch and its counts, when the row carried a second line.
    branch: Option<(SharedString, u32, u32)>,
    two_line: bool,
    active: bool,
    asleep: bool,
}

/// The sidebar's motion, owned by the app and read only by its renderer:
/// which rows are arriving or leaving, which groups are opening or
/// shutting, and what the leaving rows last showed.
#[derive(Default)]
pub(crate) struct SidebarMotion {
    rows: SidebarRowMotion,
    disclosures: HashMap<u64, DisclosureMotion>,
    disclosure_pass: HashMap<u64, u64>,
    last_shown: HashMap<TabId, RowGhost>,
    /// The frame counter `begin_frame` bumps, which `disclose` stamps so
    /// `end_frame` can drop groups no render asked about (a deleted group).
    pass: u64,
    /// Whether the frame-pumping task below is live. Render arms it the
    /// first frame a gesture is owed; the task clears it when the model
    /// settles, and the next gesture arms it again.
    ticking: bool,
}

impl SidebarMotion {
    fn observe(&mut self, current: &[TabId], animate: bool, now: Instant) {
        self.rows.observe(current, animate, now);
        self.last_shown
            .retain(|id, _| current.contains(id) || self.rows.entries.contains_key(id));
    }

    fn begin_frame(&mut self, now: Instant) {
        self.rows.begin_frame(now);
        self.pass += 1;
    }

    fn end_frame(&mut self) {
        self.rows.end_frame();
        let pass = self.pass;
        self.disclosure_pass
            .retain(|container, p| *p == pass && self.disclosures.contains_key(container));
        self.disclosures
            .retain(|container, _| self.disclosure_pass.get(container) == Some(&pass));
    }

    fn is_animating(&self, now: Instant) -> bool {
        self.rows.entries.values().any(|entry| !entry.done(now))
            || self
                .disclosures
                .values()
                .any(|motion| motion.animating(now))
    }

    /// The disclosure frame for one group this render, creating the motion
    /// settled when the group is first seen.
    fn disclose(
        &mut self,
        container: u64,
        expanded: bool,
        count: usize,
        now: Instant,
        reduce_motion: bool,
    ) -> DisclosureFrame {
        self.disclosure_pass.insert(container, self.pass);
        self.disclosures
            .entry(container)
            .or_insert_with(|| DisclosureMotion::new(expanded, count, now))
            .update(expanded, count, now, reduce_motion)
    }
}

impl LyrelApp {
    /// Records the sidebar's membership for this frame. Called from the
    /// work-area render whether or not the sidebar is on screen: a change
    /// that lands while the panel is shut, or under Reduce Motion, is a
    /// landing — nothing replays when the panel opens or motion returns.
    pub(crate) fn observe_sidebar_motion(&self, cx: &gpui_kit::App) {
        let now = cx.background_executor().now();
        let animate = self.sidebar_open(cx) && !cx.reduce_motion();
        let tab_ids: Vec<TabId> = self.tabs.iter().map(|tab| tab.id()).collect();
        self.sidebar_motion
            .borrow_mut()
            .observe(&tab_ids, animate, now);
    }

    /// Keeps frames coming while a sidebar gesture owes one, on the same
    /// timer-and-notify footing as the home cursor's blink. Spawned lazily
    /// by the render that starts a gesture; settles itself when the model
    /// does, so an idle sidebar costs nothing.
    fn ensure_sidebar_motion_tick(motion: &RefCell<SidebarMotion>, cx: &mut Context<Self>) {
        if motion.borrow().ticking {
            return;
        }
        motion.borrow_mut().ticking = true;
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(SIDEBAR_MOTION_TICK_MS))
                    .await;
                let owed = this.update(cx, |this, cx| {
                    let now = cx.background_executor().now();
                    let owed = this.sidebar_motion.borrow().is_animating(now);
                    if !owed {
                        this.sidebar_motion.borrow_mut().ticking = false;
                    }
                    // Present the terminal frame as well: it removes ghosts
                    // and makes settled rows eligible drop targets again.
                    cx.notify();
                    owed
                });
                match owed {
                    Ok(true) => {}
                    _ => break,
                }
            }
        })
        .detach();
    }
}

/// A stable table key for one group's rows.
fn sidebar_container(key: &Option<GroupKey>) -> u64 {
    let mut hasher = DefaultHasher::new();
    key.hash(&mut hasher);
    hasher.finish()
}

/// A leaving row's fading echo, riding in the clipped slot the exit keeps
/// open: same shape and the strings it last drew, and inert — the overlay
/// swallows every press, so nothing a ghost covers answers to a row that is
/// already gone.
fn sidebar_ghost_row(
    id: TabId,
    ghost: &RowGhost,
    presence: RowPresence,
    natural: f32,
    sf: &lyrel_ui_base::presets::Surface,
    cx: &gpui_kit::App,
) -> AnyElement {
    let height = natural * presence.height;
    let theme = cx.theme();
    let title_line = h_flex()
        .w_full()
        .items_center()
        .gap_1p5()
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_sm()
                .when(ghost.active, |d| d.font_weight(FontWeight::MEDIUM))
                .child(ghost.title.clone()),
        )
        .when_some(ghost.cwd.clone(), |line, cwd| {
            line.child(
                div()
                    .flex_shrink_0()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(cwd),
            )
        });
    let git_line = ghost.branch.clone().map(|(branch, added, removed)| {
        h_flex()
            .w_full()
            .items_center()
            .gap_1p5()
            .text_xs()
            .text_color(theme.muted_foreground)
            .child(
                gpui_kit::svg()
                    .path("icons/git-branch.svg")
                    .flex_shrink_0()
                    .size(px(11.))
                    .text_color(theme.muted_foreground),
            )
            .child(div().min_w_0().truncate().child(branch))
            .when(added > 0, |line| {
                line.child(
                    div()
                        .text_color(theme.muted_foreground)
                        .child(format!("+{added}")),
                )
            })
            .when(removed > 0, |line| {
                line.child(
                    div()
                        .text_color(theme.muted_foreground)
                        .child(format!("−{removed}")),
                )
            })
    });
    div()
        .id(SharedString::from(format!("sidebar-ghost-{id}")))
        .test_support()
        .relative()
        .flex_none()
        .w_full()
        .h(px(height))
        .overflow_hidden()
        .child(
            div()
                .absolute()
                .left_0()
                .right_0()
                .top(px((height - natural) / 2.))
                .h(px(natural))
                .opacity(presence.opacity)
                .child(
                    h_flex()
                        .w_full()
                        .h(px(natural))
                        .flex_shrink_0()
                        .items_center()
                        .gap(px(row_metrics::GAP))
                        .px_2()
                        .rounded(lyrel_ui_base::rounding::ROW_RADIUS)
                        .when(ghost.active, |row| {
                            row.bg(gpui_kit::rgb(sf.selected))
                                .text_color(gpui_kit::rgb(sf.text_selected))
                        })
                        .when(!ghost.active, |row| {
                            row.text_color(gpui_kit::rgb(sf.text_resting))
                        })
                        .when(ghost.asleep, |row| row.opacity(0.6))
                        // A quiet circle where the avatar stood: the shape
                        // reads as the row without borrowing its marks.
                        .child(
                            div()
                                .flex_shrink_0()
                                .size(px(row_metrics::AVATAR))
                                .rounded_full()
                                .bg(gpui_kit::rgb(sf.base)),
                        )
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .justify_center()
                                .child(title_line)
                                .children(git_line),
                        ),
                ),
        )
        .child(
            div()
                .absolute()
                .inset_0()
                .occlude()
                .capture_any_mouse_down(|_, _, cx| cx.stop_propagation())
                .capture_any_mouse_up(|_, _, cx| cx.stop_propagation()),
        )
        .into_any_element()
}

/// A row mid-gesture rides in a clipped slot that takes only the share of
/// its height the presence allows, the row itself laid out at full size and
/// centred inside — so nothing paints outside the gap it is opening or
/// closing. `opacity` folds the disclosure fade into the content's own.
fn sidebar_motion_row(
    i: usize,
    row: AnyElement,
    canvas: AnyElement,
    natural: f32,
    presence: RowPresence,
    opacity: f32,
) -> AnyElement {
    let height = natural * presence.height;
    div()
        .id(("sidebar-row-slot", i))
        .test_support()
        .relative()
        .flex_none()
        .w_full()
        .h(px(height))
        .overflow_hidden()
        .child(
            div()
                .absolute()
                .left_0()
                .right_0()
                .top(px((height - natural) / 2.))
                .h(px(natural))
                .opacity(opacity)
                .child(row),
        )
        .child(canvas)
        .into_any_element()
}

/// Every detail a sidebar row could not fit, collected so the hover card can
/// be rendered from cloneable data (an `AnyElement` cannot be cloned, but the
/// tooltip closure has to rebuild its content on every hover).
#[derive(Clone)]
struct SidebarInfo {
    /// Full path, when the row's title was elided.
    title: Option<SharedString>,
    /// Full branch plus diff counts, when the row's branch was elided.
    branch: Option<(SharedString, u32, u32)>,
    /// Full working directory, when the row's second line was elided.
    cwd: Option<SharedString>,
    /// Remote host, when the avatar only shows a dot for it.
    host: Option<SharedString>,
}

impl LyrelApp {
    /// 左侧是唯一标签导航；收起时不绘制迷你轨道。
    pub(crate) fn sidebar_open(&self, cx: &gpui_kit::App) -> bool {
        self.left_panel_open(cx)
    }

    /// Every keyboard stop the visible sidebar draws, top to bottom: each
    /// named group's disclosure header, then that group's rows unless it is
    /// folded. Empty pinned groups keep their headers; calculated groups with
    /// no tabs do not exist; Ungrouped reaches here as `Group(None)`.
    ///
    /// This is the same shape the renderer builds from, so focus cannot land
    /// on a logical leaf folded out of sight or on a slot that only exists
    /// while a drag preview is in the air.
    pub(crate) fn sidebar_nav_targets(&self, cx: &gpui_kit::App) -> Vec<SidebarNavTarget> {
        let keys = self.sidebar_group_keys(cx);
        let groups = &self.sidebar_groups;
        let mut targets = Vec::new();
        for section in sidebar_sections(&keys, groups) {
            if section.tabs.is_empty() && section.pinned().is_none() {
                continue;
            }
            if section.name.is_some() {
                targets.push(SidebarNavTarget::Group(section.key.clone()));
            }
            if groups.is_folded(section.key.as_ref()) {
                continue;
            }
            targets.extend(
                section
                    .tabs
                    .iter()
                    .filter_map(|&i| self.tabs.get(i).map(|tab| SidebarNavTarget::Tab(tab.id()))),
            );
        }
        targets
    }

    /// Enter the visible sidebar's single roving Tab stop. The selected
    /// visible tab is focused without switching sessions; a folded group that
    /// hides the active tab offers its own disclosure header instead, so the
    /// remaining sessions stay one Enter away.
    pub(crate) fn focus_terminal_tab_navigation(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.tabs.is_empty() || self.sidebar_nav_overlay_open(window, cx) {
            return;
        }
        let targets = self.sidebar_nav_targets(cx);
        if targets.is_empty() {
            return;
        }
        if self.sidebar_collapsed {
            self.sidebar_collapsed = false;
            self.update_config(cx, |cfg| cfg.sidebar_collapsed = false);
        }
        let active = self.tabs.get(self.tabs.active_index()).map(Tab::id);
        let active_header = active.and_then(|id| {
            let at = self.tabs.index_of(id)?;
            let key = self.sidebar_group_keys(cx).get(at).cloned().flatten();
            let header = SidebarNavTarget::Group(key);
            targets.contains(&header).then_some(header)
        });
        // Entry follows the active selection, not where a previous visit left
        // its roving target: sessions may have switched behind the sidebar,
        // and Enter must activate what is current. The remembered target only
        // covers an active tab nothing visible can stand in for.
        let target = active
            .map(SidebarNavTarget::Tab)
            .filter(|target| targets.contains(target))
            .or(active_header)
            .or_else(|| {
                self.sidebar_nav_target
                    .clone()
                    .filter(|target| targets.contains(target))
            })
            .or_else(|| {
                targets
                    .iter()
                    .find(|target| matches!(target, SidebarNavTarget::Tab(_)))
                    .cloned()
            })
            .or_else(|| targets.first().cloned());
        let Some(target) = target else {
            return;
        };
        *self.sidebar_reveal.borrow_mut() = Some(target.clone());
        self.sidebar_nav_target = Some(target);
        window.focus(&self.sidebar_nav_focus, cx);
        cx.notify();
    }

    /// Drop or repair the roving target after the rows moved under it: a tab
    /// closed, its cwd regrouped it, or its group was folded from somewhere
    /// else. A hidden tab hands focus to its group's header; a vanished one
    /// hands it to the first available row.
    pub(crate) fn sidebar_nav_settle(&mut self, cx: &mut Context<Self>) {
        let Some(current) = self.sidebar_nav_target.clone() else {
            return;
        };
        let targets = self.sidebar_nav_targets(cx);
        if targets.contains(&current) {
            return;
        }
        let header = match &current {
            SidebarNavTarget::Tab(id) => self.tabs.index_of(*id).and_then(|at| {
                let key = self.sidebar_group_keys(cx).get(at).cloned().flatten();
                let header = SidebarNavTarget::Group(key);
                targets.contains(&header).then_some(header)
            }),
            SidebarNavTarget::Group(_) => None,
        };
        self.sidebar_nav_target = header
            .or_else(|| {
                targets
                    .iter()
                    .find(|target| matches!(target, SidebarNavTarget::Tab(_)))
                    .cloned()
            })
            .or_else(|| targets.first().cloned());
    }

    /// The visible stop that should inherit navigation when `closing` goes:
    /// the next row, then the previous one, then the closing group's own
    /// disclosure header. Computed before the tab leaves the list so its place
    /// is still exact; [`Self::sidebar_nav_settle`] repairs a header that the
    /// close also removed.
    pub(crate) fn sidebar_nav_successor_after_close(
        &self,
        closing: lyrel_core::core::machine::TabId,
        cx: &gpui_kit::App,
    ) -> Option<SidebarNavTarget> {
        let targets = self.sidebar_nav_targets(cx);
        let tabs: Vec<lyrel_core::core::machine::TabId> = targets
            .iter()
            .filter_map(|target| match target {
                SidebarNavTarget::Tab(id) => Some(*id),
                SidebarNavTarget::Group(_) => None,
            })
            .collect();
        if let Some(at) = tabs.iter().position(|id| *id == closing)
            && let Some(next) = tabs
                .get(at + 1)
                .or_else(|| at.checked_sub(1).and_then(|previous| tabs.get(previous)))
        {
            return Some(SidebarNavTarget::Tab(*next));
        }
        let at = self.tabs.index_of(closing)?;
        let key = self.sidebar_group_keys(cx).get(at).cloned().flatten();
        Some(SidebarNavTarget::Group(key))
    }

    fn set_sidebar_nav_target(&mut self, target: SidebarNavTarget, cx: &mut Context<Self>) {
        if self.sidebar_nav_target.as_ref() == Some(&target) {
            return;
        }
        *self.sidebar_reveal.borrow_mut() = Some(target.clone());
        self.sidebar_nav_target = Some(target);
        cx.notify();
    }

    fn sidebar_nav_step(&mut self, forward: bool, cx: &mut Context<Self>) {
        self.sidebar_nav_settle(cx);
        let targets = self.sidebar_nav_targets(cx);
        let Some(current) = self.sidebar_nav_target.clone() else {
            if let Some(first) = targets.first().cloned() {
                self.set_sidebar_nav_target(first, cx);
            }
            return;
        };
        if targets.is_empty() {
            return;
        }
        let at = targets
            .iter()
            .position(|target| *target == current)
            .unwrap_or(0);
        let next = if forward {
            (at + 1) % targets.len()
        } else {
            (at + targets.len() - 1) % targets.len()
        };
        self.set_sidebar_nav_target(targets[next].clone(), cx);
    }

    fn sidebar_nav_endpoint(&mut self, last: bool, cx: &mut Context<Self>) {
        self.sidebar_nav_settle(cx);
        let targets = self.sidebar_nav_targets(cx);
        let target = match last {
            true => targets.last().cloned(),
            false => targets.first().cloned(),
        };
        if let Some(target) = target {
            self.set_sidebar_nav_target(target, cx);
        }
    }

    /// Enter/Space on the roving target. A tab switches sessions but leaves
    /// the sidebar owning the keyboard; a group header folds or unfolds that
    /// group through the same control a pointer uses.
    fn sidebar_nav_activate(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.sidebar_nav_settle(cx);
        match self.sidebar_nav_target.clone() {
            Some(SidebarNavTarget::Tab(id)) => {
                if let Some(index) = self.tabs.index_of(id) {
                    self.activate_from_sidebar_navigation(index, window, cx);
                } else {
                    self.sidebar_nav_settle(cx);
                }
            }
            Some(SidebarNavTarget::Group(key)) => {
                self.toggle_sidebar_group(key.as_ref(), cx);
                self.sidebar_nav_settle(cx);
            }
            None => {}
        }
    }

    /// Escape leaves the sidebar for the content it stands beside. Overlays
    /// above the sidebar keep their dismissal priority; this is only reached
    /// while the sidebar itself holds focus and no such surface is open.
    fn sidebar_nav_leave(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.focus_previous_content(window, cx);
        cx.notify();
    }

    /// Modal surfaces above workbench navigation. A visible code panel is
    /// ordinary content; a diff, picker, or document prompt is not.
    pub(crate) fn sidebar_nav_overlay_open(&self, window: &Window, cx: &gpui_kit::App) -> bool {
        let settings_here = self.settings.as_ref().is_some_and(|_| {
            self.settings_window
                .is_none_or(|(settings, _)| settings == window.window_handle())
        });
        settings_here
            || self.search.is_open()
            || self.close_prompt_open
            || self.switcher.is_some()
            || self.worktree_prompt.is_some()
            || self.document_navigation_overlay_open(cx)
            || self
                .tabs
                .get(self.tabs.active_index())
                .is_some_and(|tab| tab.document_front() == Some(crate::ui::tab::OverlayTop::Diff))
    }

    fn sidebar_nav_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Focus must be on the list itself. A rename field, a menu or anything
        // else living inside the sidebar keeps its own keys.
        if !self.sidebar_nav_focus.is_focused(window) {
            return;
        }
        let modifiers = &event.keystroke.modifiers;
        if modifiers.control || modifiers.platform || modifiers.alt || modifiers.shift {
            return;
        }
        let handled = match event.keystroke.key.as_str() {
            "up" => {
                self.sidebar_nav_step(false, cx);
                true
            }
            "down" => {
                self.sidebar_nav_step(true, cx);
                true
            }
            "home" => {
                self.sidebar_nav_endpoint(false, cx);
                true
            }
            "end" => {
                self.sidebar_nav_endpoint(true, cx);
                true
            }
            "enter" | "space" => {
                self.sidebar_nav_activate(window, cx);
                true
            }
            "escape" => {
                if self.sidebar_nav_overlay_open(window, cx) {
                    false
                } else {
                    self.sidebar_nav_leave(window, cx);
                    true
                }
            }
            _ => false,
        };
        if handled {
            cx.stop_propagation();
        }
    }

    /// What the right panel has reserved, from the sidebar's point of view.
    pub(crate) fn right_panel_floor(&self, window: &Window, cx: &gpui_kit::App) -> f32 {
        if self.right_panel_open(cx) {
            self.right_panel_min_px(window, cx)
        } else {
            0.
        }
    }

    pub(crate) fn sidebar_max_px(&self, window: &Window, cx: &gpui_kit::App) -> f32 {
        crate::ui::app::side_panel_max(
            window.viewport_size().width.as_f32(),
            MIN_SIDEBAR_WIDTH,
            self.right_panel_floor(window, cx) + self.document_floor(cx),
        )
    }

    /// How wide the sidebar is drawn, given the live cell and the cap the rest
    /// of the window leaves it. Read here rather than clamped at each caller so
    /// the document column's budget and the sidebar itself can never disagree
    /// about how much width is already spoken for.
    pub(crate) fn sidebar_px(&self, window: &Window, cx: &gpui_kit::App) -> f32 {
        self.sidebar_width
            .get()
            .clamp(MIN_SIDEBAR_WIDTH, self.sidebar_max_px(window, cx))
    }

    pub(crate) fn tab_sidebar(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let active = self.tabs.active_index();
        let nav_focused = self.sidebar_nav_focus.is_focused(window);
        let nav_target = self.sidebar_nav_target.clone();
        let sf = cx.global::<lyrel_ui_base::presets::Surfaces>().rail;
        let show_badges = self.mod_hint_badges;
        // The badges stay mounted while they fade out after the hold ends, so
        // the row budget and the close button's slot agree with what renders.
        let badges_mounted =
            show_badges || (self.mod_hint_fade_generation.is_some() && !cx.reduce_motion());
        let width = self.sidebar_px(window, cx);
        *self.sidebar_slots.borrow_mut() = vec![Bounds::default(); self.tabs.len()];
        // Read before it is blanked: the rectangles a tab held over the
        // sidebar is measured against are the ones drawn last frame, the
        // same way a pane dropped here is. Blanked and written again below
        // so a group that folds or filters away stops accepting drops.
        let over_group = self.sidebar_regroup_target(window);
        // Taken rather than read, for the same reason: last frame's divider is
        // what a drag is measured against, and the one drawn below replaces it.
        let divider_last = self.sidebar_divider.take();
        let lifting_row = crate::ui::reorder::dragged_sidebar_tab(&self.reorder).is_some();
        // Offered every frame the pointer is over a group, and cleared with
        // the rest of the drag's pending state on the frames it is not — so
        // letting go anywhere else drops on nothing.
        if let Some(target) = over_group {
            crate::ui::reorder::set_regroup(&self.reorder, target);
        }
        self.sidebar_group_slots.borrow_mut().clear();

        let mut list = v_flex()
            .id("tab-sidebar-list")
            // The whole visible column is one roving Tab stop: focus stays on
            // this container and Up/Down move an active descendant keyed by
            // tab/group identity. That keeps the sidebar out of the terminal's
            // Tab and arrow input until the entry action asks for it.
            .track_focus(&self.sidebar_nav_focus)
            .role(Role::TabList)
            .aria_orientation(Orientation::Vertical)
            .aria_label(t(L10nKey::SidebarNavigation))
            .on_key_down(cx.listener(Self::sidebar_nav_key_down))
            // A folder dragged in from Finder is pinned as a group of its own.
            .drag_over::<ExternalPaths>(|s, _, _, cx| s.bg(cx.theme().drag_border.opacity(0.06)))
            .on_drop(cx.listener(|this, paths: &ExternalPaths, _window, cx| {
                this.pin_dropped_folders(paths.paths(), cx)
            }))
            .track_scroll(&self.sidebar_scroll)
            .flex_1()
            .min_h_0()
            .w_full()
            .overflow_y_scroll()
            .px_3()
            .pb(px(20.))
            .gap(px(GROUP_GAP));

        let keys: Rc<Vec<Option<GroupKey>>> = Rc::new(self.sidebar_group_keys(cx));
        let groups = &self.sidebar_groups;
        let sections = sidebar_sections(&keys, groups);

        // ⌘N runs ActivateTabN, which goes through `activate_visual` — the
        // Nth row as the sidebar lays it out, not the Nth tab in `self.tabs`.
        // The badge has to be read off the same order or it names a chord that
        // opens a different tab, so take it from `visual_tab_order` rather than
        // flattening `sections` a second time here.
        let badge_pos: Vec<usize> = {
            let mut pos = vec![0usize; self.tabs.len()];
            for (n, i) in self.visual_tab_order(cx).into_iter().enumerate() {
                pos[i] = n;
            }
            pos
        };

        let visible_by_section: Vec<Vec<usize>> = sections
            .iter()
            .map(|section| section.tabs.clone())
            .collect();

        let pointer = window.mouse_position();
        // The row text is measured against real glyphs before it is elided:
        // `text_sm` is 0.875rem and `text_xs` 0.75rem, resolved here so the
        // measurement and the render use the same sizes and family.
        let font = gpui_kit::Font {
            family: cx.theme().font_family.clone(),
            features: Default::default(),
            fallbacks: None,
            weight: Default::default(),
            style: Default::default(),
        };
        // The active row renders its title at `FontWeight::MEDIUM`, which is
        // wider than the regular weight in any proportional face. Measuring
        // it as regular would let the one row the user is looking at overflow
        // into the truncation this is here to avoid.
        let title_font_active = gpui_kit::Font {
            weight: FontWeight::MEDIUM,
            ..font.clone()
        };
        let rem = window.rem_size().as_f32();
        // Measure with the same interface scale used to paint the header.
        let header_size = rem * META_REM;
        // The git lines and the group header paint with `tabular()` figures,
        // which set a `1` as wide as an `8`; measuring them proportionally
        // came out a few pixels short on counts like `+1565 −124`, and the
        // front-elided branch then lost its tail to CSS truncation as well.
        let meta_font = gpui_kit::Font {
            features: tabular(),
            ..font.clone()
        };
        let header_font = gpui_kit::Font {
            weight: FontWeight::MEDIUM,
            ..meta_font.clone()
        };
        // On the rail the diff counts are metadata like the branch beside
        // them, so they take its caption ink; the hover card, which is where
        // someone goes to read them, keeps the resting green and red.
        let rail_fill: gpui_kit::Hsla = gpui_kit::rgb(sf.base).into();
        let count_ink = cx.theme().muted_foreground;
        let added_ink = lyrel_ui_base::presets::resting_ink(
            cx.theme().success,
            cx.theme().muted_foreground,
            rail_fill,
        );
        let removed_ink = lyrel_ui_base::presets::resting_ink(
            cx.theme().danger,
            cx.theme().muted_foreground,
            rail_fill,
        );
        let rendered =
            |ix: &usize| !visible_by_section[*ix].is_empty() || sections[*ix].pinned().is_some();
        // Pinned headers reorder among themselves, by hand; that order is the
        // workspace's and is written back whole when a drag lands.
        let pinned_slots: Vec<usize> = (0..sections.len())
            .filter(|&ix| sections[ix].pinned().is_some())
            .filter(rendered)
            .collect();
        let pinned_rects: Rc<RefCell<Vec<Bounds<Pixels>>>> =
            Rc::new(RefCell::new(vec![Bounds::default(); pinned_slots.len()]));
        let pinned_preview = reorder::preview(
            &self.reorder,
            &Surface::PinnedGroups,
            pinned_slots.len(),
            pointer,
        );
        let pinned_display: Vec<usize> = match &pinned_preview {
            Some(p) => {
                let all: Vec<GroupId> = groups.pinned.iter().map(|g| g.id).collect();
                let shown: Vec<GroupId> = pinned_slots
                    .iter()
                    .filter_map(|&ix| sections[ix].pinned())
                    .collect();
                if let Some(order) = reordered_pinned(&all, &shown, p.from, p.target) {
                    reorder::set_pending(&self.reorder, &Surface::PinnedGroups, order);
                }
                p.order.clone()
            }
            None => (0..pinned_slots.len()).collect(),
        };
        // Every auto group draws a header, and a header is what there is to
        // grab, so these are the slots the auto-group surface runs over.
        // Ungrouped is excluded: it is where the keyless tabs fall, and it
        // always sits last.
        let keyed_slots: Vec<usize> = (0..sections.len())
            .filter(|&ix| {
                sections[ix]
                    .key
                    .as_ref()
                    .is_some_and(|k| k.auto().is_some())
            })
            .filter(rendered)
            .collect();
        let keyed_groups = keyed_slots.len();
        let group_slots: Rc<RefCell<Vec<Bounds<Pixels>>>> =
            Rc::new(RefCell::new(vec![Bounds::default(); keyed_groups]));
        let group_preview = reorder::preview(
            &self.reorder,
            &Surface::SidebarGroups,
            keyed_groups,
            pointer,
        );
        let keyed_keys: Vec<AutoKey> = keyed_slots
            .iter()
            .filter_map(|&ix| sections[ix].key.as_ref()?.auto().cloned())
            .collect();
        let slot_display: Vec<usize> = match &group_preview {
            Some(p) => {
                if let (Some(from), Some(to)) = (keyed_keys.get(p.from), keyed_keys.get(p.target))
                    && let Some(order) = regrouped_order(&keys, from, to)
                {
                    reorder::set_pending(&self.reorder, &Surface::SidebarGroups, order);
                }
                p.order.clone()
            }
            None => (0..keyed_groups).collect(),
        };
        let mut blocks: Vec<(Option<HeaderSlot>, usize)> = pinned_display
            .into_iter()
            .map(|slot| (Some(HeaderSlot::Pinned(slot)), pinned_slots[slot]))
            .collect();
        let first_unpinned = blocks.len();
        blocks.extend(
            slot_display
                .into_iter()
                .map(|slot| (Some(HeaderSlot::Auto(slot)), keyed_slots[slot])),
        );
        blocks.extend(
            (0..sections.len())
                .filter(|&ix| sections[ix].key.is_none())
                .filter(rendered)
                .map(|ix| (None, ix)),
        );
        // The divider: kept groups above it, derived ones below. Drawn
        // whenever something is kept, and while an auto header is in the air
        // even when nothing is yet — carrying it above the divider is how it
        // gets pinned, so there has to be a divider to carry it above.
        let lifted_auto = reorder::dragged_auto_group(&self.reorder);
        let show_divider = !groups.pinned.is_empty() || lifted_auto.is_some();
        let pin_hover = lifted_auto.is_some()
            && divider_last.is_some_and(|d| pointer.y < d.origin.y + d.size.height);
        if pin_hover {
            reorder::set_pin(&self.reorder);
        }
        let divider_lit = pin_hover || over_group == Some(reorder::Regroup::ToAuto);
        let divider_zone = groups.pinned.is_empty();
        let mut divider_drawn = false;
        // Whether the tab in the air is kept in a pinned group — then
        // everything below the divider is somewhere it can go.
        let lifted_is_kept = reorder::dragged_sidebar_tab(&self.reorder).is_some_and(|id| {
            self.tabs
                .iter()
                .find(|t| t.id() == id)
                .and_then(|t| t.group.get())
                .is_some_and(|g| groups.contains(g))
        });
        let workspace_is_local = !lyrel_ui_base::session::WorkspaceStore::all(cx)
            .get(self.workspace)
            .is_some_and(|w| w.is_remote());

        // Session-row and disclosure motion. Membership was observed by
        // `observe_sidebar_motion` in the work-area render — whether or not
        // the panel is on screen — so every way a tab can appear or vanish
        // lands in the same next-frame diff and no event path has to know
        // about motion. Here the frame just begins.
        let now = cx.background_executor().now();
        let reduce_motion = cx.reduce_motion();
        self.sidebar_motion.borrow_mut().begin_frame(now);

        // A reveal whose row is not drawn this frame (folded away, or the tab
        // gone) is dropped, so it cannot fire when the row turns up later.
        let reveal = self.sidebar_reveal.borrow().clone();
        let mut reveal_drawn = false;
        for (n, (group_slot, group_ix)) in blocks.into_iter().enumerate() {
            if n == first_unpinned && show_divider {
                list = list.child(self.sidebar_divider(divider_lit, divider_zone, cx));
                divider_drawn = true;
            }
            let section = &sections[group_ix];
            let group_key = section.key.clone();
            // A named section is also a keyboard stop: its header is the
            // disclosure control that keeps a folded group's sessions one
            // Enter away instead of unreachable.
            let header_target = section
                .name
                .is_some()
                .then(|| SidebarNavTarget::Group(group_key.clone()));
            if let Some(target) = header_target.as_ref() {
                reveal_drawn |= reveal.as_ref() == Some(target);
            }
            // Only a group that draws a header can be folded — there is
            // nothing to click otherwise, and the one headerless section (the
            // list below the pinned groups, when nothing else is there) must
            // never answer to Ungrouped's fold.
            let folded = section.name.is_some() && groups.is_folded(group_key.as_ref());
            let container = sidebar_container(&group_key);
            // The header keeps counting every row the group has; folding only
            // stops them being drawn. Nothing downstream then registers a
            // rectangle for them, which is what keeps a pane from being
            // dropped into a group that is shut.
            //
            // No exception for the active tab. A fold that leaves one row
            // hanging under a shut chevron, with the header counting rows
            // that are not there, reads as a list that failed to load. The
            // cost is that ⌘T inside a folded group — `spawn_group` seeds
            // the new tab with the group it came from — puts the new tab
            // behind the chevron: the pane area shows the fresh shell and the
            // header count goes up, but the row waits for the group to open.
            let row_count = visible_by_section[group_ix].len();
            let visible_tabs: Vec<usize> = visible_by_section[group_ix].clone();
            let visible_ids: Vec<SharedString> = visible_tabs
                .iter()
                .map(|&i| SharedString::from(format!("tab-{}", self.tabs[i].id())))
                .collect();
            // The disclosure gesture for this group's rows this frame: a
            // growing clip while it opens, a shortening one while it shuts.
            // Under Reduce Motion every frame is the settled frame, and a
            // group that has never been seen starts settled.
            let disclosure_frame = self.sidebar_motion.borrow_mut().disclose(
                container,
                !folded,
                row_count,
                now,
                reduce_motion,
            );
            let rows_revealed =
                disclosure_frame.reveal > 0. || disclosure_frame.rows.iter().any(|&o| o > 0.);
            let visible: Vec<usize> = match rows_revealed {
                true => visible_tabs.clone(),
                false => Vec::new(),
            };
            let row_slots: Rc<RefCell<Vec<Bounds<Pixels>>>> =
                Rc::new(RefCell::new(vec![Bounds::default(); visible.len()]));
            let row_preview = reorder::preview(
                &self.reorder,
                &Surface::SidebarRows(group_key.clone()),
                visible.len(),
                pointer,
            );
            // While a drag preview owns this section it owns the geometry
            // too: motion yields, and the section paints exactly as it did
            // before motion existed.
            let slot_plan: Vec<RowSlot> = match &row_preview {
                None => self.sidebar_motion.borrow_mut().rows.slots(
                    container,
                    &visible
                        .iter()
                        .map(|&i| self.tabs[i].id())
                        .collect::<Vec<_>>(),
                    now,
                ),
                Some(_) => Vec::new(),
            };
            let mut rows: Vec<Option<AnyElement>> = Vec::new();
            // The natural heights of the rows as they would rest, for the
            // disclosure clip's budget; leaving rows weave in with theirs.
            let mut row_heights: Vec<f32> = Vec::new();
            // A group whose rows all sit on the same branch with the same
            // diff says so once, on its header, instead of once per row.
            // Four copies of `pr-818 +94 −26` under one heading describe the
            // repo, not the tabs, and being the only coloured text in the
            // column they were also the loudest thing in it. Read off every
            // row the group counts rather than the ones it draws, so a folded
            // group still names its branch.
            //
            // A lone row is no exception. Its branch describes the same repo
            // the heading above it names, and leaving it down there gave a
            // one-tab group a shape no other group in the column has: a
            // bare heading over a two-line row. It lifts like any other.
            let shared_git: Option<SharedGit> = section.name.as_ref().and_then(|_| {
                let rows = &visible_by_section[group_ix];
                if rows.is_empty() {
                    return None;
                }
                // Only rows that *have* a status get a vote. A tab that was
                // just opened has none until its shell reports a directory
                // and the poll comes back; counting it as a disagreement
                // pulled the branch off the header and grew a branch line
                // under every sibling for the half second it took, then
                // folded them all back — the column jumped twice for every
                // ⌘T. Unknown is not different; it is not yet known.
                let mut known = rows
                    .iter()
                    .filter_map(|&i| Some((i, self.tabs[i].git_status(Some(window), cx)?)));
                let (first, status) = known.next()?;
                let same = known.all(|(_, other)| other == status);
                same.then(|| SharedGit {
                    status,
                    click: git_click(&self.tabs[first], window, cx),
                    rows: rows.clone(),
                })
            });
            for (slot, i) in visible.into_iter().enumerate() {
                let badge_pos = badge_pos[i];
                let tab = &self.tabs[i];
                let tab_id = tab.id();
                // Motion for this row: its own arrival or departure, and the
                // disclosure fade riding the group it is in. A row the plan
                // does not mention (a drag preview owns the section) is full.
                let presence = match slot_plan.get(slot) {
                    Some(RowSlot::Real(_, presence)) => *presence,
                    _ => RowPresence::FULL,
                };
                let disclose_opacity = disclosure_frame.rows.get(slot).copied().unwrap_or(1.);
                // Only a fully settled row registers its rectangle: a row in
                // any part of a gesture leaves the default box behind, which
                // is what tells everything that reads these — drop targets,
                // hit tests — that the row is not really there yet, or not
                // any more. A ghost never registers: it is leaving.
                let register = presence == RowPresence::FULL && disclose_opacity >= 1.;
                let nav_row =
                    nav_focused && nav_target.as_ref() == Some(&SidebarNavTarget::Tab(tab_id));
                reveal_drawn |= reveal.as_ref() == Some(&SidebarNavTarget::Tab(tab_id));
                let is_active = i == active;
                let ssh_dot = self.tab_ssh_dot(tab, cx);
                let asleep = tab.is_asleep();
                let agent = tab.agent(cx).or_else(|| tab.asleep_agent());
                // The indicator is what the activity matrix's tab channel
                // shows: agent_status stays the truth for menus and logic.
                let agent_status = tab.agent_indicator_status(cx);
                let agent_unread = tab.agent_unread_count(cx);
                let git_cwd = git_click(tab, window, cx);
                let badge_extra = if badges_mounted && badge_pos < 9 {
                    row_metrics::BADGE + row_metrics::GAP
                } else {
                    0.
                };
                let zoomed = self.tab_is_zoomed(i);
                let zoom_extra = if zoomed {
                    row_metrics::ZOOM + row_metrics::GAP
                } else {
                    0.
                };
                // The sleep mark takes the zoom mark's size; a sleeping tab
                // has no panes on screen to zoom, so the two never share a row.
                let zoom_extra = if asleep {
                    zoom_extra + row_metrics::ZOOM + row_metrics::GAP
                } else {
                    zoom_extra
                };
                // Status rides at the row's trailing end, where the badge
                // would be; the badge wins while the modifier is held.
                let status_dot = match badges_mounted && badge_pos < 9 {
                    true => None,
                    false => self.row_status_dot(agent_status, agent_unread, rail_fill),
                };
                let status_extra = match status_dot.is_some() {
                    true => row_metrics::STATUS + row_metrics::GAP,
                    false => 0.,
                };
                // Elision is measured against this budget so the label and
                // branch never wrap or overflow into CSS truncation.
                let label_avail =
                    (row_metrics::text_budget(width) - badge_extra - zoom_extra - status_extra)
                        .max(48.);
                let title_size = 0.875 * rem;
                let meta_size = META_REM * rem;
                let title_font = if is_active { &title_font_active } else { &font };
                // Title: the *full* label, elided further down once the
                // working directory beside it has said how much of the line
                // it wants. A wide sidebar shows the whole thing and a narrow
                // one keeps whichever end identifies it — the tail for a
                // path, both edges for anything else. A fixed segment cap
                // (`short_title`) would elide even when the row has room, so
                // only the width may decide here.
                //
                // `full_title` is the unelided string the card can expand
                // back to; `None` means the row is showing a placeholder that
                // no card can improve on.
                //
                // The ladder the strip and the switcher climb, read here for
                // the name and not for the shortening: this column measures in
                // pixels and lets a card expand the row back to the whole
                // string, so it wants what `label_of` would have cut down
                // rather than the cut. A renamed tab is elided like anything
                // else — and so the card has to be able to spell it back out.
                let (title_text, full_title) = match self.full_tab_label(tab, Some(window), cx) {
                    Some(raw) => {
                        let full = SharedString::from(raw);
                        (full.clone(), Some(full))
                    }
                    // Nothing to expand: the row is naming an unnamed shell,
                    // not hiding a title behind an ellipsis.
                    None => {
                        let placeholder = SharedString::from(t_fmt(
                            L10nKey::TabUnnamedShell,
                            &[("n", &((i + 1).to_string()))],
                        ));
                        (placeholder, None)
                    }
                };
                let mut branch_shown: Option<(SharedString, SharedString, u32, u32)> = None;
                let mut cwd_shown: Option<(SharedString, SharedString)> = None;
                let git_line = match shared_git.is_some() {
                    true => None,
                    false => tab.git_status(Some(window), cx),
                }
                .map(|g| {
                    let mut line = h_flex()
                        .id(("sidebar-git", i))
                        .w_full()
                        .items_center()
                        .gap_1p5()
                        .text_size(px(meta_size))
                        .text_color(cx.theme().muted_foreground)
                        .font_features(tabular());
                    let counts_w = counts_width(window.text_system(), &meta_font, meta_size, &g);
                    // Branch: cut from the end, the way any other label is.
                    let branch_avail = (label_avail - counts_w).max(0.);
                    let shown = elide_end_clusters(
                        window.text_system(),
                        &meta_font,
                        meta_size,
                        &g.branch,
                        branch_avail,
                    );
                    branch_shown = Some((
                        shown.clone(),
                        SharedString::from(g.branch.clone()),
                        g.added,
                        g.removed,
                    ));
                    line = line.child(div().flex_1().min_w_0().truncate().child(shown));
                    if g.added > 0 || g.removed > 0 {
                        let mut counts = h_flex()
                            .id(("sidebar-diff", i))
                            .flex_shrink_0()
                            .items_center()
                            .gap_1p5()
                            .when_some(git_cwd, |counts, (host, cwd)| {
                                // A click target inside a click target: the row
                                // highlights as a whole, which says nothing
                                // about the counts being their own button. The
                                // underline the SFTP breadcrumb uses for
                                // clickable text says where this one starts.
                                counts
                                    .cursor_pointer()
                                    .hover(|s| s.underline())
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                                            cx.stop_propagation();
                                            // Swallowing the press also swallows the
                                            // row's click, the only thing that
                                            // activates a tab — so this row has to
                                            // activate itself, or the overlay lands
                                            // in whichever tab was already on
                                            // screen, carrying this row's repo (#706).
                                            this.activate(i, window, cx);
                                            this.toggle_diff_overlay(host, cwd.clone(), window, cx);
                                        }),
                                    )
                            });
                        if g.added > 0 {
                            counts = counts
                                .child(div().text_color(count_ink).child(format!("+{}", g.added)));
                        }
                        if g.removed > 0 {
                            counts = counts.child(
                                div().text_color(count_ink).child(format!("−{}", g.removed)),
                            );
                        }
                        line = line.child(counts);
                    }
                    line
                });
                // Progress is local to the sole terminal leaf. It temporarily
                // owns the existing metadata line, not an agent busy/finish dot.
                let git_line = tab
                    .progress_label(cx)
                    .filter(|(kind, _)| {
                        *kind != crate::daemon::protocol::ProgressKind::Indeterminate
                    })
                    .map(|(kind, label)| {
                        use crate::daemon::protocol::ProgressKind;
                        let color = match kind {
                            ProgressKind::Error => cx.theme().danger,
                            ProgressKind::Paused => cx.theme().warning,
                            _ => cx.theme().muted_foreground,
                        };
                        h_flex()
                            .id(("sidebar-progress", i))
                            .w_full()
                            .min_w_0()
                            .text_xs()
                            .text_color(color)
                            .child(
                                div()
                                    .id(("sidebar-progress-status", i))
                                    .test_support()
                                    .aria_label(label.clone())
                                    .truncate()
                                    .child(label),
                            )
                    })
                    .or(git_line);
                // Outside a repo there is no branch line, and the working
                // directory rides on the title's own line rather than growing
                // a second one under it: a group of plain shells was a column
                // of two-line rows describing paths that mostly agree, which
                // is twice the height for a line of small grey text nobody
                // was reading. A row keeps its second line only for a branch.
                let cwd_full: Option<SharedString> = match git_line.is_none()
                    && shared_git.is_none()
                {
                    false => None,
                    true => tab
                        .pane
                        .focused_or_first(window, cx)
                        .and_then(|leaf| {
                            let leaf = leaf.read(cx);
                            Some((leaf.effective_cwd()?, leaf.display_home(cx)))
                        })
                        .map(|(cwd, home)| {
                            let text = cwd.display().to_string();
                            SharedString::from(abbreviate_home(&text, home.as_deref()).into_owned())
                        })
                        // The title already carries the whole path; a second
                        // copy adds noise, not information.
                        .filter(|full| full.as_ref() != title_text.as_ref()),
                };
                // The path takes what it needs up to half the line and the
                // title keeps the rest — the same split a group header makes
                // with the branch beside its heading. Flex would hand the
                // overflow to the two of them in proportion to what each
                // asked for, which cuts the longer string hardest.
                let cwd_want = cwd_full.as_ref().map(|full| {
                    row_metrics::META_GAP
                        + measure_text(window.text_system(), &font, meta_size, full)
                });
                let title_avail = match cwd_want {
                    Some(want) => (label_avail - want.min(label_avail * 0.5)).max(ROW_TITLE_FLOOR),
                    None => label_avail,
                };
                let shown_title = elide_label(
                    window.text_system(),
                    title_font,
                    title_size,
                    &title_text,
                    title_avail,
                );
                if let Some(full) = cwd_full {
                    // Measured against what the title actually took, not what
                    // it was allowed to: a short title hands the slack back
                    // instead of leaving the path elided around a gap.
                    let avail = (label_avail
                        - measure_text(window.text_system(), title_font, title_size, &shown_title)
                        - row_metrics::META_GAP)
                        .max(0.);
                    if avail >= ROW_CWD_FLOOR {
                        let shown = elide_path_keep_tail(
                            window.text_system(),
                            &font,
                            meta_size,
                            &full,
                            avail,
                        );
                        cwd_shown = Some((shown, full));
                    }
                }
                let rename_input = self
                    .renaming
                    .as_ref()
                    .filter(|r| r.tab == tab.id())
                    .map(|r| r.input.clone());

                let shown = SidebarRowShown {
                    title: full_title.map(|full| (shown_title.clone(), full)),
                    branch: branch_shown.clone(),
                    cwd: cwd_shown.clone(),
                };
                let info = self.sidebar_info(tab, window, cx, &shown);
                // Colors are captured by value so the tooltip builder (which
                // borrows no app state) can style the card on its own.
                let muted = cx.theme().muted_foreground;
                let success = added_ink;
                let danger = removed_ink;

                // A row that grows a branch line under its title stands at
                // `ROW_HEIGHT_TWO_LINE`; a one-line row at `ROW_HEIGHT`.
                let two_line = git_line.is_some();
                let row_h = match two_line {
                    true => ROW_HEIGHT_TWO_LINE,
                    false => ROW_HEIGHT,
                };
                // Remember what this row showed — the exact strings, not a
                // derivation — so a close that lands any frame later can
                // paint the ghost from them instead of from a tab that no
                // longer exists.
                self.sidebar_motion.borrow_mut().last_shown.insert(
                    tab_id,
                    RowGhost {
                        title: shown
                            .title
                            .as_ref()
                            .map(|(shown, _)| shown.clone())
                            .unwrap_or_else(|| title_text.clone()),
                        cwd: shown.cwd.as_ref().map(|(cwd, _)| cwd.clone()),
                        branch: shown
                            .branch
                            .as_ref()
                            .map(|(branch, _, added, removed)| (branch.clone(), *added, *removed)),
                        two_line,
                        active: is_active,
                        asleep,
                    },
                );
                let label_region = match rename_input {
                    Some(input) => div()
                        .id(("sidebar-rename", i))
                        .flex_1()
                        .min_w_0()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        // The row switches tabs on the *release* now, so
                        // holding the press back is no longer enough: a click
                        // landing in the field would reach the row behind it
                        // and switch away from the name being typed, taking
                        // the focus with it.
                        .on_click(|_, _, cx| cx.stop_propagation())
                        // No inset of its own: the label it replaces starts
                        // flush, and the field's 12px padding jumped the name
                        // sideways the moment rename began.
                        .child(Input::new(&input).appearance(false).px_0())
                        .into_any_element(),
                    None => v_flex()
                        .id(("sidebar-label", i))
                        .flex_1()
                        .min_w_0()
                        .justify_center()
                        .when_some(info, |col, info| {
                            col.tooltip(move |window, cx| {
                                // `Tooltip::element` rebuilds its content on
                                // every hover, so the captured info is cloned
                                // per call instead of being moved out.
                                let info = info.clone();
                                gpui_kit::component::tooltip::Tooltip::element(
                                    move |_window, _cx| {
                                        v_flex()
                                            .gap_1()
                                            // The card is the one place that
                                            // promised the whole string, so a long
                                            // path wraps here rather than being
                                            // truncated a second time.
                                            .when_some(info.title.clone(), |c, title| {
                                                c.child(
                                                    div()
                                                        .max_w(px(420.))
                                                        .text_sm()
                                                        .font_weight(FontWeight::MEDIUM)
                                                        .child(title),
                                                )
                                            })
                                            .when_some(
                                                info.branch.clone(),
                                                |c, (branch, added, removed)| {
                                                    let mut line = h_flex()
                                                        .items_center()
                                                        .gap_1p5()
                                                        .text_xs()
                                                        .text_color(muted)
                                                        .child(
                                                            gpui_kit::svg()
                                                                .path("icons/git-branch.svg")
                                                                .flex_shrink_0()
                                                                .size(px(11.))
                                                                .text_color(muted),
                                                        )
                                                        .child(div().child(branch));
                                                    if added > 0 {
                                                        line = line.child(
                                                            div()
                                                                .text_color(success)
                                                                .child(format!("+{added}")),
                                                        );
                                                    }
                                                    if removed > 0 {
                                                        line = line.child(
                                                            div()
                                                                .text_color(danger)
                                                                .child(format!("−{removed}")),
                                                        );
                                                    }
                                                    c.child(line)
                                                },
                                            )
                                            .when_some(info.cwd.clone(), |c, cwd| {
                                                c.child(
                                                    div()
                                                        .max_w(px(420.))
                                                        .text_xs()
                                                        .text_color(muted)
                                                        .child(cwd),
                                                )
                                            })
                                            .when_some(info.host.clone(), |c, host| {
                                                c.child(
                                                    h_flex()
                                                        .items_center()
                                                        .gap_1p5()
                                                        .text_xs()
                                                        .text_color(muted)
                                                        .child(
                                                            gpui_kit::svg()
                                                                .path("icons/machine-remote.svg")
                                                                .flex_shrink_0()
                                                                .size(px(11.))
                                                                .text_color(muted),
                                                        )
                                                        .child(div().truncate().child(host)),
                                                )
                                            })
                                    },
                                )
                                .build(window, cx)
                            })
                        })
                        .child(
                            h_flex()
                                .w_full()
                                .items_center()
                                .gap_1p5()
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .truncate()
                                        .text_sm()
                                        .when(is_active, |d| d.font_weight(FontWeight::MEDIUM))
                                        .child(shown_title),
                                )
                                // The path is elided to the room the title
                                // left, so it may not shrink again here — a
                                // second cut would come out of its tail, the
                                // half that says which directory this is.
                                .when_some(cwd_shown.map(|(cwd, _)| cwd), |line, cwd| {
                                    line.child(
                                        div()
                                            .flex_shrink_0()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(cwd),
                                    )
                                }),
                        )
                        .children(git_line)
                        .into_any_element(),
                };

                // The row's paint-rectangle canvas, built before the row so it
                // can ride either the row itself (at rest) or the clipped
                // slot wrapper (mid-gesture), whose bounds are the slot's
                // true height.
                let row_canvas = canvas(
                    {
                        let slots = row_slots.clone();
                        // The row by tab as well as by slot: reordering
                        // reads the slots of one group, a pane dropped
                        // on the sidebar reads every row there is.
                        let by_tab = self.sidebar_slots.clone();
                        let reveal = self.sidebar_reveal.clone();
                        let scroll = self.sidebar_scroll.clone();
                        let id = tab.id();
                        let reveal_target = SidebarNavTarget::Tab(id);
                        // The header sits directly above a group's
                        // first row, one gap away.
                        let lead = match slot == 0 && section.name.is_some() {
                            true => px(HEADER_HEIGHT + ROW_GAP),
                            false => px(0.),
                        };
                        move |bounds, window, _cx| {
                            if register {
                                if let Some(s) = slots.borrow_mut().get_mut(slot) {
                                    *s = bounds;
                                }
                                if let Some(s) = by_tab.borrow_mut().get_mut(i) {
                                    *s = bounds;
                                }
                            }
                            if reveal.borrow().as_ref() == Some(&reveal_target) {
                                *reveal.borrow_mut() = None;
                                let view = scroll.bounds();
                                let shift = reveal_shift(
                                    (bounds.top(), bounds.bottom()),
                                    lead,
                                    (view.top(), view.bottom()),
                                );
                                if shift != px(0.) {
                                    let mut offset = scroll.offset();
                                    offset.y += shift;
                                    scroll.set_offset(offset);
                                    window.refresh();
                                }
                            }
                        }
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .inset_0();
                let row = h_flex()
                    .id(("tab-row", i))
                    .test_support()
                    .relative()
                    // What a screen reader announces and presses: the full
                    // title, not the elided one drawn.
                    .role(gpui_kit::Role::Tab)
                    .aria_label(SharedString::from(title_text.to_string()))
                    .aria_selected(is_active)
                    .when(nav_row, |row| {
                        row.child(
                            div()
                                .id(("sidebar-nav-ring", i))
                                .absolute()
                                .inset_0()
                                .rounded(lyrel_ui_base::rounding::ROW_RADIUS)
                                .border_1()
                                .border_color(cx.theme().primary),
                        )
                        // The list holds real focus; the row the ring marks is
                        // what assistive technology should announce as current.
                        .aria_active_descendant()
                    })
                    .group(SharedString::from(format!("tab-row-{i}")))
                    .cursor_pointer()
                    .on_drag(DragTab, {
                        let state = self.reorder.clone();
                        let slots = row_slots.clone();
                        let group_key = group_key.clone();
                        let id = tab.id();
                        move |_drag, grab, _window, cx| {
                            cx.stop_propagation();
                            *state.borrow_mut() = Some(
                                Reorder::new(
                                    Surface::SidebarRows(group_key.clone()),
                                    slot,
                                    slots.borrow().clone(),
                                    Axis::Vertical,
                                    px(ROW_GAP),
                                    grab,
                                )
                                .of_tab(id),
                            );
                            cx.new(|_| DragTab)
                        }
                    })
                    .w_full()
                    .h(px(row_h))
                    .flex_shrink_0()
                    .items_center()
                    .justify_between()
                    .gap(px(row_metrics::GAP))
                    .px_2()
                    .rounded(lyrel_ui_base::rounding::ROW_RADIUS)
                    .when(is_active, |s| {
                        s.bg(gpui_kit::rgb(sf.selected))
                            .text_color(gpui_kit::rgb(sf.text_selected))
                    })
                    .when(!is_active, |s| {
                        s.text_color(gpui_kit::rgb(sf.text_resting))
                            .hover(|s| s.bg(gpui_kit::rgb(sf.hover)))
                    })
                    // Faded as well as marked, the way the strip's chip is: the
                    // tabs holding nothing should be the quiet ones in the column.
                    .when(asleep, |s| s.opacity(0.6))
                    .when(row_preview.as_ref().is_some_and(|p| p.from == slot), |s| {
                        s.opacity(0.75)
                    })
                    // Switched on the release, not the press: a press that turns
                    // into a drag is the tab being picked up, and a tab on its
                    // way into another tab's layout must not put itself on
                    // screen on the way there.
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        if this.sidebar_nav_focus.is_focused(window) {
                            this.sidebar_nav_target = Some(SidebarNavTarget::Tab(tab_id));
                        }
                        this.activate(i, window, cx);
                    }))
                    .child(self.tab_avatar_plain(
                        ("sidebar-avatar", i),
                        agent,
                        agent_status,
                        ssh_dot,
                        row_metrics::AVATAR,
                        cx,
                    ))
                    // Leading, like the chip's: the trailing end of a row is
                    // the badge's, and the close button fades in over it.
                    .when(zoomed, |row| {
                        row.child(self.zoom_mark(("sidebar-zoom", i), cx))
                    })
                    .when(asleep, |row| {
                        row.child(self.sleep_mark(("sidebar-asleep", i), cx))
                    })
                    .child(label_region)
                    .children(status_dot)
                    .when(badges_mounted && badge_pos < 9, |row| {
                        row.child(
                            div()
                                .id(("tab-badge", badge_pos))
                                .test_support()
                                .flex_shrink_0()
                                .flex()
                                .items_center()
                                .justify_center()
                                .size(px(20.))
                                .text_xs()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(if is_active {
                                    gpui_kit::rgb(sf.text_selected).into()
                                } else {
                                    cx.theme().muted_foreground
                                })
                                // The spring, not a one-shot entrance: the same
                                // element fades in on the hold and back out on
                                // the release, and a re-hold mid-fade reverses
                                // from the sampled value. Under Reduce the
                                // spring renders at its target with no frames.
                                .with_spring(
                                    ("tab-badge-fade", badge_pos),
                                    gpui_kit::SpringAnimation::new(crate::ui::motion::HINT_FADE)
                                        .to(if show_badges { 1.0f32 } else { 0.0f32 })
                                        .from(0.0f32),
                                    |badge, opacity| badge.opacity(opacity),
                                )
                                .child(tab_badge_label(badge_pos)),
                        )
                    })
                    .when(!(badges_mounted && badge_pos < 9), |row| {
                        let backing: gpui_kit::Hsla = if is_active {
                            gpui_kit::rgb(sf.selected).into()
                        } else {
                            gpui_kit::rgb(sf.hover).into()
                        };
                        let mut fade_from = backing;
                        fade_from.a = 0.;
                        row.child(
                            // The row's full height, not the button's: on a
                            // two-line row the diff count sits under the
                            // button's band, and a band-high backing left the
                            // bottom of "+2" showing beneath the ×.
                            h_flex()
                                .absolute()
                                .top_0()
                                .bottom_0()
                                .right(px(6.))
                                .opacity(0.)
                                .group_hover(SharedString::from(format!("tab-row-{i}")), |s| {
                                    s.opacity(1.)
                                })
                                .child(div().w(px(10.)).h_full().bg(linear_gradient(
                                    90.,
                                    linear_color_stop(fade_from, 0.),
                                    linear_color_stop(backing, 1.),
                                )))
                                .child(
                                    div().h_full().flex().items_center().bg(backing).child(
                                        crate::ui::tab_strip::hit_target(
                                            Button::new(("sidebar-close", i))
                                                .icon(IconName::Close)
                                                .ghost()
                                                .xsmall()
                                                // The sidebar is one roving Tab
                                                // stop; the close affordance is
                                                // pointer/AX only so Tab does not
                                                // walk every row's ×.
                                                .tab_stop(false),
                                        )
                                        .tooltip(t(L10nKey::TabContextCloseTab))
                                        // Held here, because the row behind it
                                        // switches tabs on the release too:
                                        // without this the same click closes
                                        // tab `i` and then activates whichever
                                        // tab slid into its place.
                                        .on_click(
                                            cx.listener(move |this, _, window, cx| {
                                                cx.stop_propagation();
                                                this.close_tab(i, window, cx);
                                            }),
                                        ),
                                    ),
                                ),
                        )
                    });

                let menu_app = cx.entity().downgrade();
                let row = row.context_menu(move |menu, window, cx| {
                    LyrelApp::tab_context_menu(menu, i, true, &menu_app, window, cx)
                });
                // A row mid-gesture rides in a clipped slot that takes only
                // the share of its height the presence allows; the disclosure
                // fade folds into the content's own opacity.
                let moving = presence != RowPresence::FULL || disclose_opacity < 1.;
                rows.push(Some(match moving {
                    false => row.child(row_canvas).into_any_element(),
                    true => sidebar_motion_row(
                        i,
                        row.into_any_element(),
                        row_canvas.into_any_element(),
                        row_h,
                        presence,
                        presence.opacity * disclose_opacity,
                    ),
                }));
                row_heights.push(row_h);
            }

            // An empty pinned group is still drawn: it is kept until deleted.
            if row_count == 0 && section.pinned().is_none() {
                continue;
            }

            let row_display: Vec<usize> = match &row_preview {
                Some(p) => {
                    if let Some(order) =
                        reordered_rows(&keys, groups, &group_key, &visible_tabs, p.from, p.target)
                    {
                        reorder::set_pending(
                            &self.reorder,
                            &Surface::SidebarRows(group_key.clone()),
                            order,
                        );
                    }
                    p.order.clone()
                }
                None => (0..rows.len()).collect(),
            };
            let mut rows: Vec<Option<AnyElement>> = rows;
            let mut rows: Vec<AnyElement> = row_display
                .into_iter()
                .map(|slot| match &row_preview {
                    Some(p) if p.from == slot => deferred(
                        div()
                            .child(rows[slot].take().expect("each slot emitted once"))
                            .relative()
                            .top(p.held),
                    )
                    .into_any_element(),
                    Some(p) => {
                        let offset = p.offsets[slot].as_f32();
                        div()
                            .child(rows[slot].take().expect("each slot emitted once"))
                            .relative()
                            .with_animation(
                                (
                                    SharedString::from(format!(
                                        "row-slide-{}-{}",
                                        visible_ids[slot], p.generation
                                    )),
                                    slot,
                                ),
                                Animation::new(std::time::Duration::from_millis(REORDER_SLIDE_MS))
                                    .with_easing(ease_out_quint()),
                                move |el, delta| el.top(px(offset * (1. - delta))),
                            )
                            .into_any_element()
                    }
                    None => rows[slot]
                        .take()
                        .expect("each slot emitted once")
                        .into_any_element(),
                })
                .collect();
            // Leaving rows weave back in where they last stood, before the
            // column hands itself to the block. This only runs when no drag
            // preview owns the section, so the plan's order and the paint
            // order are the same.
            if row_preview.is_none() && slot_plan.iter().any(|s| matches!(s, RowSlot::Ghost(..))) {
                let mut built: Vec<Option<AnyElement>> = rows.into_iter().map(Some).collect();
                let mut ghost_heights: Vec<f32> = Vec::new();
                rows = slot_plan
                    .iter()
                    .map(|slot| match slot {
                        RowSlot::Real(ix, _) => built[*ix].take().expect("each row emitted once"),
                        RowSlot::Ghost(id, presence) => {
                            let ghost = self
                                .sidebar_motion
                                .borrow()
                                .last_shown
                                .get(id)
                                .expect("a leaving row was drawn before it left")
                                .clone();
                            let natural = match ghost.two_line {
                                true => ROW_HEIGHT_TWO_LINE,
                                false => ROW_HEIGHT,
                            };
                            ghost_heights.push(natural);
                            sidebar_ghost_row(*id, &ghost, *presence, natural, &sf, cx)
                        }
                    })
                    .collect();
                row_heights.extend(ghost_heights);
            }
            // A pinned group carries a mark. It is the only thing separating
            // it on sight from a derived one — they behave differently (a
            // `cd` moves a tab out of a repo group and never out of this
            // one), and a group pinned on a real repo would otherwise print a
            // header identical to that repo's.
            let pinned_id = section.pinned();
            let pinned = pinned_id.is_some();
            let pinned_folder: Option<SharedString> = pinned_id
                .and_then(|id| groups.get(id))
                .and_then(|g| g.folder.clone())
                .map(SharedString::from);
            let auto_key_here = group_key.as_ref().and_then(GroupKey::auto).cloned();
            let renaming_group = self
                .group_rename
                .as_ref()
                .filter(|r| Some(r.group) == pinned_id)
                .map(|r| r.input.clone());
            let header = section.name.clone().map(|name| {
                // The header packs a heading and the branch its whole group
                // shares onto one 11px line, and the branch is the unbounded
                // half of it: beside `fix/rpc-proxy-and-error-classification`,
                // `DELTA-NEUTRAL-BOT` came out as `DEL…`. Flex splits an
                // overflow between the two in proportion to how much room each
                // asked for, which is backwards here — the name is what the
                // group *is*, the branch only what it happens to be sitting
                // on. So both are measured against the header's real chrome:
                // the branch gets what it needs up to half the line, the name
                // keeps the rest, and each is elided into its share the way a
                // row already elides its own.
                let ts = window.text_system();
                let mut avail = row_metrics::header_budget(width);
                if folded {
                    avail -= row_metrics::HEADER_ICON + row_metrics::META_GAP;
                }
                if pinned {
                    avail -= PIN_MARK_SIZE + row_metrics::META_GAP;
                }
                let count_label = row_count.to_string();
                if folded {
                    avail -= measure_text(ts, &meta_font, header_size, &count_label)
                        + row_metrics::META_GAP;
                }
                let avail = avail.max(HEADER_NAME_FLOOR);
                // What the shared branch would take if nothing were in its
                // way: the branch itself, the `·` before its counts, the
                // counts, and the gaps the spacer between the name and the
                // branch sits in.
                // Drawn as a bare `·` with a gap on either side; the gap before
                // it is the one `counts_width` already reserves.
                let sep_w = measure_text(ts, &meta_font, header_size, META_SEP_TRIMMED)
                    + row_metrics::META_GAP;
                let git_want = shared_git.as_ref().map(|shared| {
                    let counts = counts_width(ts, &meta_font, header_size, &shared.status);
                    let sep = if counts > 0. { sep_w } else { 0. };
                    2. * row_metrics::META_GAP
                        + measure_text(ts, &meta_font, header_size, &shared.status.branch)
                        + sep
                        + counts
                });
                let name_avail = header_name_avail(avail, git_want);
                let label = elide_label(ts, &header_font, header_size, &name, name_avail);
                let name_w = measure_text(ts, &header_font, header_size, &label);
                let hover_group = SharedString::from(format!("sidebar-group-{group_ix}"));
                let header_nav = nav_focused && header_target.as_ref() == nav_target.as_ref();
                let bar = h_flex()
                    .id(("sidebar-group", group_ix))
                    .group(hover_group.clone())
                    .relative()
                    .role(Role::Button)
                    .aria_label(SharedString::from(name.clone()))
                    .aria_expanded(!folded)
                    .when(header_nav, |bar| {
                        bar.child(
                            div()
                                .id(("sidebar-group-nav-ring", group_ix))
                                .absolute()
                                .inset_0()
                                .border_1()
                                .border_color(cx.theme().primary),
                        )
                        .aria_active_descendant()
                    })
                    .w_full()
                    .items_center()
                    .gap_1p5()
                    .px_2()
                    // The air above a heading is the list's `GROUP_GAP`;
                    // the heading itself is one short caption line.
                    .h(px(HEADER_HEIGHT))
                    .flex_shrink_0()
                    .text_size(px(header_size))
                    .text_color(cx.theme().muted_foreground)
                    .font_features(tabular())
                    .hover(|s| s.text_color(cx.theme().foreground))
                    .on_click(cx.listener({
                        let key = group_key.clone();
                        let target = SidebarNavTarget::Group(key.clone());
                        move |this, _, window, cx| {
                            if this.sidebar_nav_focus.is_focused(window) {
                                this.sidebar_nav_target = Some(target.clone());
                            }
                            this.toggle_sidebar_group(key.as_ref(), cx)
                        }
                    }))
                    .when_some(group_slot, |header, slot| {
                        let (surface, slot, slots) = match slot {
                            HeaderSlot::Pinned(s) => {
                                (Surface::PinnedGroups, s, pinned_rects.clone())
                            }
                            HeaderSlot::Auto(s) => (Surface::SidebarGroups, s, group_slots.clone()),
                        };
                        let auto = auto_key_here.clone();
                        crate::ui::reorder::cursor_grab(header).on_drag(DragGroup, {
                            let state = self.reorder.clone();
                            move |_drag, grab, _window, cx| {
                                cx.stop_propagation();
                                let reorder = Reorder::new(
                                    surface.clone(),
                                    slot,
                                    slots.borrow().clone(),
                                    Axis::Vertical,
                                    px(GROUP_GAP),
                                    grab,
                                );
                                // An auto header says which group it is, so
                                // letting it go above the divider can pin it.
                                *state.borrow_mut() = Some(match auto.clone() {
                                    Some(key) => reorder.of_auto(key),
                                    None => reorder,
                                });
                                cx.new(|_| DragGroup)
                            }
                        })
                    })
                    // The heading names the group; the chevron only says
                    // something when there is something behind it. An open
                    // group shows its rows, which is its own answer.
                    .when(folded, |header| {
                        header.child(
                            div()
                                .flex_shrink_0()
                                .child(Icon::new(IconName::ChevronRight).xsmall()),
                        )
                    })
                    // Every kept group carries the mark; it is what sets them
                    // apart from the derived groups below. On a folder group it
                    // is also the way to stop keeping it — a click unpins, and
                    // the folder's tabs fall back to the groups their cwds
                    // resolve to. A label group's mark is only a mark: without a
                    // folder there is nothing to fall back on, so letting go of
                    // it is Delete Group, in its menu, not a stray click.
                    .when_some(pinned_id, |header, id| {
                        let mark = div()
                            .flex_shrink_0()
                            .child(Icon::empty().path(PIN_MARK).size(px(PIN_MARK_SIZE)));
                        header.child(match pinned_folder.is_some() {
                            false => mark.into_any_element(),
                            true => mark
                                .id(("sidebar-group-unpin", group_ix))
                                .cursor_pointer()
                                .hover(|s| s.text_color(cx.theme().foreground))
                                .tooltip(|window, cx| {
                                    gpui_kit::component::tooltip::Tooltip::new(t(
                                        L10nKey::SidebarUnpinGroup,
                                    ))
                                    .build(window, cx)
                                })
                                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                .on_click(cx.listener(move |this, _, _window, cx| {
                                    cx.stop_propagation();
                                    this.delete_group(id, cx);
                                }))
                                .into_any_element(),
                        })
                    })
                    .child(match renaming_group {
                        Some(input) => div()
                            .id(("sidebar-group-rename", group_ix))
                            .flex_1()
                            .min_w_0()
                            // The header folds the group on click, so a click
                            // landing in the field would shut the very group
                            // whose name is being typed — and take the focus
                            // with it.
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(|_, _, cx| cx.stop_propagation())
                            // No inset of its own: the label it replaces starts
                            // flush, and the field's 12px padding jumped the name
                            // sideways the moment rename began.
                            .child(Input::new(&input).appearance(false).px_0())
                            .into_any_element(),
                        // Elided above, so the truncation here is only the
                        // backstop for a face that measures wider than it
                        // paints; the name no longer gives room to the branch.
                        None => div()
                            .id(("sidebar-group-name", group_ix))
                            .flex_shrink_0()
                            .min_w_0()
                            .truncate()
                            // A caption, not a title: the rows under it are
                            // what the column is for, so the heading keeps
                            // the metadata's grey and only a medium weight
                            // to stand apart from the branch beside it.
                            .font_weight(FontWeight::MEDIUM)
                            .child(label)
                            // A folder group may have been renamed to anything;
                            // the folder it keeps is what the name stands for.
                            .when_some(pinned_folder.clone(), |name, folder| {
                                name.tooltip(move |window, cx| {
                                    gpui_kit::component::tooltip::Tooltip::new(folder.clone())
                                        .build(window, cx)
                                })
                            })
                            .into_any_element(),
                    })
                    .when_some(shared_git, |bar, shared| {
                        let SharedGit {
                            status,
                            click,
                            rows,
                        } = shared;
                        let counts_w = counts_width(ts, &meta_font, header_size, &status);
                        let sep = if counts_w > 0. { sep_w } else { 0. };
                        let branch_avail =
                            (avail - name_w - 2. * row_metrics::META_GAP - sep - counts_w).max(0.);
                        // Cut from the end, like a row's.
                        let branch = elide_end_clusters(
                            ts,
                            &meta_font,
                            header_size,
                            &status.branch,
                            branch_avail,
                        );
                        let mut line = h_flex()
                            .id(("sidebar-group-git", group_ix))
                            .flex_shrink(1.)
                            .min_w_0()
                            .items_center()
                            .gap_1p5()
                            .child(div().min_w_0().truncate().child(branch));
                        if status.added > 0 || status.removed > 0 {
                            line = line.child(
                                div()
                                    .flex_shrink_0()
                                    .whitespace_nowrap()
                                    .child(META_SEP_TRIMMED),
                            );
                            let mut counts = h_flex()
                                .id(("sidebar-group-diff", group_ix))
                                .flex_shrink_0()
                                .items_center()
                                .gap_1p5()
                                .when_some(click, |counts, (host, cwd)| {
                                    counts
                                        .cursor_pointer()
                                        .hover(|s| s.underline())
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(
                                                move |this, _: &MouseDownEvent, window, cx| {
                                                    cx.stop_propagation();
                                                    // The overlay opens over the
                                                    // active tab; make sure that
                                                    // is one of this group's,
                                                    // the same way a row's counts
                                                    // activate their row first.
                                                    if !rows.contains(&this.tabs.active_index())
                                                        && let Some(&first) = rows.first()
                                                    {
                                                        this.activate(first, window, cx);
                                                    }
                                                    this.toggle_diff_overlay(
                                                        host,
                                                        cwd.clone(),
                                                        window,
                                                        cx,
                                                    );
                                                },
                                            ),
                                        )
                                });
                            if status.added > 0 {
                                counts = counts.child(
                                    div()
                                        .text_color(count_ink)
                                        .child(format!("+{}", status.added)),
                                );
                            }
                            if status.removed > 0 {
                                counts = counts.child(
                                    div()
                                        .text_color(count_ink)
                                        .child(format!("−{}", status.removed)),
                                );
                            }
                            line = line.child(counts);
                        }
                        bar.child(div().flex_1()).child(line)
                    })
                    // The count is redundant while the rows are on screen; it
                    // is what a shut group has instead of them.
                    .when(folded, |bar| {
                        bar.child(div().flex_shrink_0().child(count_label))
                    })
                    .child(
                        // Keyboard navigation scrolls a folded group's header
                        // into view the same way a row's canvas does.
                        canvas(
                            {
                                let reveal = self.sidebar_reveal.clone();
                                let scroll = self.sidebar_scroll.clone();
                                let target = header_target
                                    .clone()
                                    .expect("a drawn header has a navigation target");
                                move |bounds, window, _cx| {
                                    if reveal.borrow().as_ref() == Some(&target) {
                                        *reveal.borrow_mut() = None;
                                        let view = scroll.bounds();
                                        let shift = reveal_shift(
                                            (bounds.top(), bounds.bottom()),
                                            px(0.),
                                            (view.top(), view.bottom()),
                                        );
                                        if shift != px(0.) {
                                            let mut offset = scroll.offset();
                                            offset.y += shift;
                                            scroll.set_offset(offset);
                                            window.refresh();
                                        }
                                    }
                                }
                            },
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .inset_0(),
                    )
                    .children(self.header_actions(
                        group_ix,
                        group_key.clone(),
                        hover_group,
                        rail_fill,
                        cx,
                    ));
                // Renaming is offered on a menu rather than a double click:
                // the first click of a double would fold the group, so the
                // name would be edited on a box that just shut. An auto group
                // offers no rename — it is named after its repo or host, and a
                // rename there could only lie about where its tabs are; pin it
                // first, and it is the user's to name.
                //
                // Attached last and erased to `AnyElement`, because the menu
                // wrapper changes the element's type and the arms have to
                // agree.
                let app = cx.entity().downgrade();
                match (pinned_id, auto_key_here.clone()) {
                    (Some(id), _) => {
                        let has_folder = pinned_folder.is_some();
                        bar.context_menu(move |menu, window, cx| {
                            pinned_header_menu(
                                menu,
                                id,
                                has_folder,
                                workspace_is_local,
                                &app,
                                window,
                                cx,
                            )
                        })
                        .into_any_element()
                    }
                    (None, Some(key)) => bar
                        .context_menu(move |menu, _window, _cx| {
                            auto_header_menu(menu, key.clone(), &app)
                        })
                        .into_any_element(),
                    (None, None) => bar.into_any_element(),
                }
            });

            // A tab in the air makes the difference between the two kinds of
            // group visible: the ones that can take it stay lit, the ones
            // that cannot fade back. Until a drag is under way they look
            // alike, and this is where a user finds out which is which
            // without being told.
            let takes_drops = pinned || (lifted_is_kept && n >= first_unpinned);
            let (preview, slot) = match group_slot {
                Some(HeaderSlot::Pinned(s)) => (pinned_preview.as_ref(), Some(s)),
                Some(HeaderSlot::Auto(s)) => (group_preview.as_ref(), Some(s)),
                None => (None, None),
            };
            let group_identity = match &group_key {
                Some(GroupKey::Pinned(id)) => format!("pinned-{id}"),
                Some(GroupKey::Auto(AutoKey::Repo(path))) => {
                    format!("repo-{}", path.display())
                }
                Some(GroupKey::Auto(AutoKey::SshHost(host))) => format!("ssh-{host}"),
                None => format!("ungrouped-{group_ix}"),
            };
            // An empty pinned group says how to fill it, in the place its
            // rows would be: a tab can be dragged onto it, or opened here.
            let empty_row = (row_count == 0 && !folded)
                .then_some(pinned_id)
                .flatten()
                .map(|id| {
                    h_flex()
                        .id(("sidebar-group-empty", group_ix))
                        .w_full()
                        .h(px(ROW_HEIGHT))
                        .items_center()
                        .gap_2()
                        .pl_2()
                        .rounded(lyrel_ui_base::rounding::CARD_RADIUS)
                        .cursor_pointer()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .hover(|s| {
                            s.bg(gpui_kit::rgb(sf.hover))
                                .text_color(cx.theme().sidebar_foreground)
                        })
                        .child(
                            div()
                                .flex()
                                .flex_shrink_0()
                                .items_center()
                                .justify_center()
                                .size(px(row_metrics::AVATAR))
                                .child(Icon::empty().path("icons/plus.svg").xsmall()),
                        )
                        .child(t(L10nKey::SidebarGroupNewTab))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.new_tab_in_group(GroupKey::Pinned(id), window, cx)
                        }))
                });
            // The rows as one column: at rest they are the block's own
            // children, exactly as they always were; while a disclosure
            // gesture runs, a growing or shrinking clip shows and hides them
            // — the rows themselves never change height.
            let rows_area: AnyElement = match &disclosure_frame {
                frame if frame.reveal < 1. && !rows.is_empty() => {
                    let natural = row_heights.iter().sum::<f32>()
                        + ROW_GAP * row_heights.len().saturating_sub(1) as f32;
                    div()
                        .id(("sidebar-rows", group_ix))
                        .test_support()
                        .w_full()
                        .h(px(natural * frame.reveal))
                        .overflow_hidden()
                        .child(v_flex().w_full().gap(px(ROW_GAP)).children(rows))
                        .into_any_element()
                }
                _ => v_flex()
                    .w_full()
                    .gap(px(ROW_GAP))
                    .children(rows)
                    .into_any_element(),
            };
            let block = v_flex()
                .w_full()
                .gap(px(ROW_GAP))
                .when(preview.is_some_and(|p| Some(p.from) == slot), |b| {
                    b.opacity(0.75)
                })
                .when(lifting_row && !takes_drops, |b| b.opacity(0.4))
                .when(
                    pinned_id.is_some_and(|id| over_group == Some(reorder::Regroup::Into(id))),
                    |b| b.rounded_md().bg(cx.theme().drag_border.opacity(0.15)),
                )
                .children(header)
                .child(rows_area)
                .children(empty_row)
                .child(
                    canvas(
                        {
                            let slots = match group_slot {
                                Some(HeaderSlot::Pinned(_)) => pinned_rects.clone(),
                                _ => group_slots.clone(),
                            };
                            let landing = self.sidebar_group_slots.clone();
                            // Only a pinned group is recorded, so a drag
                            // looking for somewhere to land finds nothing over
                            // an auto group or over Ungrouped.
                            move |bounds, _window, _cx| {
                                if let Some(slot) = slot
                                    && let Some(s) = slots.borrow_mut().get_mut(slot)
                                {
                                    *s = bounds;
                                }
                                if let Some(id) = pinned_id {
                                    landing.borrow_mut().push((id, bounds));
                                }
                            }
                        },
                        |_, _, _, _| {},
                    )
                    .absolute()
                    .inset_0(),
                );

            list = list.child(match (preview, slot) {
                (Some(p), Some(slot)) if p.from == slot => {
                    deferred(block.relative().top(p.held)).into_any_element()
                }
                (Some(p), Some(slot)) => {
                    let offset = p.offsets[slot].as_f32();
                    block
                        .with_animation(
                            (
                                SharedString::from(format!(
                                    "group-slide-{}-{}",
                                    group_identity, p.generation
                                )),
                                slot,
                            ),
                            Animation::new(std::time::Duration::from_millis(REORDER_SLIDE_MS))
                                .with_easing(ease_out_quint()),
                            move |el, delta| el.top(px(offset * (1. - delta))),
                        )
                        .into_any_element()
                }
                _ => block.into_any_element(),
            });
        }
        {
            let mut motion = self.sidebar_motion.borrow_mut();
            motion.end_frame();
            if motion.is_animating(now) {
                drop(motion);
                Self::ensure_sidebar_motion_tick(&self.sidebar_motion, cx);
            }
        }
        if !reveal_drawn {
            *self.sidebar_reveal.borrow_mut() = None;
        }
        if show_divider && !divider_drawn {
            list = list.child(self.sidebar_divider(divider_lit, divider_zone, cx));
        }

        // The room under the last row. Right-clicking a row opens that tab's
        // menu, so the list's own — new tab, new group — lives here rather
        // than on the list, where it would open over every row's as well.
        // It keeps a row's height when the list scrolls, so there is always
        // somewhere to click.
        list = list.child(
            div()
                .id("tab-sidebar-empty")
                .flex_1()
                .min_h(px(ROW_HEIGHT))
                .context_menu({
                    let app = cx.entity().downgrade();
                    move |menu, _window, _cx| {
                        let new_tab = app.clone();
                        let new_group = app.clone();
                        menu.min_w(px(200.))
                            .item(PopupMenuItem::new(t(L10nKey::AppMenuNewTab)).on_click(
                                move |_, window, cx| {
                                    let _ = new_tab.update(cx, |this, cx| this.new_tab(window, cx));
                                },
                            ))
                            .item(PopupMenuItem::new(t(L10nKey::SidebarNewGroup)).on_click(
                                move |_, window, cx| {
                                    let _ = new_group
                                        .update(cx, |this, cx| this.new_empty_group(window, cx));
                                },
                            ))
                    }
                }),
        );

        let workspace_head = v_flex()
            .w(px(width - 1.))
            .flex_shrink_0()
            .px_3()
            .pt_1()
            .pb_4()
            .child(self.workspace_head(cx));

        let container: Rc<Cell<Option<Bounds<Pixels>>>> = Rc::new(Cell::new(None));
        // Read while there is still a `cx` to read it from: the drag handler
        // below only ever sees a `Window`, and the cap it clamps against has to
        // be the same one the layout applies or the sidebar springs back from
        // wherever it was dropped.
        let others_floor = self.right_panel_floor(window, cx) + self.document_floor(cx);
        let backing = canvas(
            {
                let container = container.clone();
                move |bounds, _window, _cx| container.set(Some(bounds))
            },
            {
                let container = container.clone();
                let width_cell = self.sidebar_width.clone();
                let dragging = self.sidebar_dragging.clone();
                move |_bounds, _state, window, _cx| {
                    window.on_mouse_event({
                        let container = container.clone();
                        let width_cell = width_cell.clone();
                        let dragging = dragging.clone();
                        move |ev: &MouseMoveEvent, _phase, window, _cx| {
                            if !dragging.get() {
                                return;
                            }
                            let Some(b) = container.get() else {
                                return;
                            };
                            let raw = (ev.position.x - b.origin.x).as_f32();
                            let max = crate::ui::app::side_panel_max(
                                window.viewport_size().width.as_f32(),
                                MIN_SIDEBAR_WIDTH,
                                others_floor,
                            );
                            width_cell.set(raw.clamp(MIN_SIDEBAR_WIDTH, max));
                            window.refresh();
                        }
                    });
                    window.on_mouse_event({
                        let width_cell = width_cell.clone();
                        let dragging = dragging.clone();
                        move |_ev: &MouseUpEvent, _phase, window, cx| {
                            if !dragging.get() {
                                return;
                            }
                            dragging.set(false);
                            let w = width_cell.get();
                            let cfg = cx.global_mut::<Config>();
                            if cfg.sidebar_width != w {
                                cfg.sidebar_width = w;
                                cfg.save();
                            }
                            window.refresh();
                        }
                    });
                }
            },
        )
        .absolute()
        .size_full();

        let handle_active = self.sidebar_dragging.get();
        let handle = div()
            .id("sidebar-resize")
            .test_support()
            .occlude()
            .absolute()
            .top_0()
            .right(px(-(RESIZE_HANDLE_WIDTH / 2.)))
            .w(px(RESIZE_HANDLE_WIDTH))
            .h_full()
            .flex()
            .items_center()
            .justify_center()
            .cursor_col_resize()
            .child(
                div()
                    .w(px(1.))
                    .h_full()
                    .when(handle_active, |d| d.bg(cx.theme().drag_border)),
            )
            .on_mouse_down(MouseButton::Left, {
                let dragging = self.sidebar_dragging.clone();
                move |_ev, window, _cx| {
                    dragging.set(true);
                    window.refresh();
                }
            });

        div()
            .relative()
            .flex_shrink_0()
            .w(px(width))
            .h_full()
            .bg(crate::ui::theme::rail_surface_color(cx))
            .border_r(crate::ui::theme::hairline(window))
            .border_color(cx.theme().sidebar_border)
            .child(backing)
            .child(
                // Real pixels, not `size_full`: the rail's own width is a
                // definite `px`, but a percentage off it is still a percentage,
                // and on the passes that size this column from its content it
                // resolves against nothing. Everything below asks for `w_full`
                // — the tab rows, their group blocks, the scroll area — so one
                // unresolved link here collapsed the whole chain and every row
                // fell back to hugging the longest tab name. Border-box takes
                // the rail's 1px right border off the content width.
                v_flex()
                    .w(px(width - 1.))
                    .h_full()
                    .child(workspace_head)
                    .child(lyrel_ui_base::scrollbar::with_vertical_scrollbar(
                        "tab-sidebar-scrollbar",
                        list,
                        &self.sidebar_scroll,
                    )),
            )
            .child(handle)
    }

    /// The boundary between the kept groups and the derived ones, recording
    /// where it lies so a drag next frame can tell which side it is on.
    ///
    /// Not drawn at rest — the mark on each kept header already says which
    /// side a group is on, and a hairline across the list was one more rule
    /// for the eye to cross. It keeps its place in the layout all the same, so
    /// nothing jumps when a drag starts, and shows as a line only while it is
    /// the target: when letting go would pin a header or hand a tab back.
    ///
    /// With nothing pinned yet it only appears while an auto header is being
    /// carried, and then as a drop zone rather than a hairline: a one-pixel
    /// line at the very top of the list is not something anyone could aim
    /// above. `lit` while letting go would pin the header, or hand a kept tab
    /// back to auto grouping.
    fn sidebar_divider(&self, lit: bool, zone: bool, cx: &Context<Self>) -> AnyElement {
        let bounds = self.sidebar_divider.clone();
        let ink = match (lit, zone) {
            (true, _) => cx.theme().drag_border,
            (false, true) => cx.theme().sidebar_border,
            (false, false) => gpui_kit::transparent_black(),
        };
        let body = match zone {
            true => h_flex()
                .h(px(ROW_HEIGHT))
                .w_full()
                .items_center()
                .justify_center()
                .rounded(lyrel_ui_base::rounding::CARD_RADIUS)
                .border_1()
                .border_color(ink)
                .when(lit, |d| d.bg(cx.theme().drag_border.opacity(0.15)))
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(t(L10nKey::SidebarDropToPin))
                .into_any_element(),
            false => div()
                .w_full()
                .h(px(if lit { 2. } else { 1. }))
                .bg(ink)
                .into_any_element(),
        };
        v_flex()
            .id("sidebar-divider")
            .relative()
            .w_full()
            .px_2()
            .map(|d| match zone {
                true => d.pt(px(4.)).pb(px(2.)),
                // The rule takes up the gap between two groups instead of
                // adding one of its own: as a child of the list it would
                // otherwise sit between two `GROUP_GAP`s, and kept groups
                // stood more than twice as far from the rest as groups stand
                // from each other.
                false => d.h(px(GROUP_GAP)).my(px(-GROUP_GAP)).justify_center(),
            })
            .child(
                canvas(move |b, _window, _cx| bounds.set(Some(b)), |_, _, _, _| {})
                    .absolute()
                    .inset_0(),
            )
            .child(body)
            .into_any_element()
    }

    /// The buttons a header shows on hover, laid over its right end: pin for
    /// an auto group, and "+" to open a tab in the group. `None` for
    /// Ungrouped, which has neither.
    ///
    /// Laid over the branch rather than beside it, the way a row's close
    /// button covers its badge, so a header does not give up width it would
    /// only use while the pointer is on it. Acted on at the press, as a row's
    /// diff counts are: the header folds on click and picks itself up on a
    /// drag, and swallowing the press is what keeps both from happening too.
    fn header_actions(
        &self,
        group_ix: usize,
        key: Option<GroupKey>,
        hover: SharedString,
        // The rail's own fill, so the buttons cover what they sit over
        // without a patch of another grey around them.
        backing: gpui_kit::Hsla,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let key = key?;
        let mut fade = backing;
        fade.a = 0.;
        let muted = cx.theme().muted_foreground;
        let ink = cx.theme().foreground;
        let button = |id: &'static str, glyph: AnyElement, tip: L10nKey| {
            div()
                .id((id, group_ix))
                .flex_shrink_0()
                .p(px(2.))
                .rounded_sm()
                .cursor_pointer()
                .text_color(muted)
                .hover(move |s| s.text_color(ink))
                .child(glyph)
                .tooltip(move |window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(t(tip)).build(window, cx)
                })
        };
        let mut buttons = h_flex().gap_0p5().bg(backing);
        if let Some(auto) = key.auto().cloned() {
            buttons = buttons.child(
                button(
                    "sidebar-group-pin",
                    // The mark the group will carry once kept, the size of
                    // the icon beside it so the two buttons line up.
                    Icon::empty().path(PIN_MARK).xsmall().into_any_element(),
                    L10nKey::SidebarPinGroup,
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _: &MouseDownEvent, _window, cx| {
                        cx.stop_propagation();
                        this.pin_auto_group(auto.clone(), cx);
                    }),
                ),
            );
        }
        if !matches!(key, GroupKey::Auto(AutoKey::SshHost(_))) {
            buttons = buttons.child(
                button(
                    "sidebar-group-add",
                    Icon::empty()
                        .path("icons/plus.svg")
                        .xsmall()
                        .into_any_element(),
                    L10nKey::SidebarGroupNewTab,
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                        cx.stop_propagation();
                        this.new_tab_in_group(key.clone(), window, cx);
                    }),
                ),
            );
        }
        Some(
            h_flex()
                .absolute()
                .right(px(4.))
                // Centred on the heading's line: 16px of buttons in `HEADER_HEIGHT`.
                .top(px((HEADER_HEIGHT - 16.) / 2.))
                .opacity(0.)
                .group_hover(hover, |s| s.opacity(1.))
                .child(div().w(px(10.)).h(px(16.)).bg(linear_gradient(
                    90.,
                    linear_color_stop(fade, 0.),
                    linear_color_stop(backing, 1.),
                )))
                .child(buttons)
                .into_any_element(),
        )
    }

    /// What the sidebar row hid: the full title, the full branch and diff
    /// counts, the working directory, and the remote host the avatar only
    /// dots. `None` when the row showed everything — a card would add noise,
    /// not information. The host is included even for an untruncated row,
    /// because the title strips the `user@host:` prefix the avatar cannot
    /// spell out.
    ///
    /// Every line is decided by comparing what the row rendered against the
    /// string it was elided from. Both come from the row itself: deriving
    /// them here a second time is how a renamed tab ended up with a name the
    /// row shortened and the card refused to expand.
    fn sidebar_info(
        &self,
        tab: &crate::ui::tab::Tab,
        window: &mut Window,
        cx: &gpui_kit::App,
        shown: &SidebarRowShown,
    ) -> Option<SidebarInfo> {
        let elided = |pair: &Option<(SharedString, SharedString)>| {
            pair.as_ref()
                .filter(|(shown, full)| shown != full)
                .map(|(_, full)| full.clone())
        };
        let mut info = SidebarInfo {
            title: elided(&shown.title),
            branch: shown
                .branch
                .as_ref()
                .filter(|(shown, full, _, _)| shown != full)
                .map(|(_, full, added, removed)| (full.clone(), *added, *removed)),
            // The cwd only earns a card line when it was rendered *and*
            // elided: a repo row already shows the full path as its title, so
            // repeating the cwd under it would be noise, not information.
            cwd: elided(&shown.cwd),
            host: None,
        };
        // The host is read off the same leaf the title and cwd came from; a
        // split tab whose panes sit on different machines would otherwise
        // name whichever one happens to be first.
        if let Some(target) = tab.pane.focused_or_first(window, cx).and_then(|leaf| {
            leaf.read(cx)
                .remote_context()
                .map(|r| SharedString::from(r.target.clone()))
        }) {
            info.host = Some(target);
        }
        (info.title.is_some() || info.branch.is_some() || info.cwd.is_some() || info.host.is_some())
            .then_some(info)
    }

    /// Where a tab being dragged in the sidebar would land if let go now,
    /// when that is somewhere other than a new place in its own group.
    ///
    /// Over a pinned group it did not come from, into that group; anywhere
    /// below the divider, back to auto grouping — but only for a tab that is
    /// in a pinned group, since one that is not already is auto grouped and
    /// a drag that would change nothing offers nothing, falling back to plain
    /// reordering.
    ///
    /// No auto group is a target. Its membership is decided by its tabs'
    /// cwds, so "put this tab in lyrel" is a request the sidebar has no honest
    /// way to honour; the only thing a drop below the divider can mean is
    /// "stop keeping this tab by hand".
    fn sidebar_regroup_target(&self, window: &Window) -> Option<reorder::Regroup> {
        let dragged = crate::ui::reorder::dragged_sidebar_tab(&self.reorder)?;
        let here = self
            .tabs
            .iter()
            .find(|t| t.id() == dragged)
            .and_then(|t| t.group.get())
            .filter(|g| self.sidebar_groups.contains(*g));
        let pointer = window.mouse_position();
        if let Some((id, _)) = self
            .sidebar_group_slots
            .borrow()
            .iter()
            .find(|(_, bounds)| bounds.contains(&pointer))
        {
            return (Some(*id) != here).then_some(reorder::Regroup::Into(*id));
        }
        let below = self
            .sidebar_divider
            .get()
            .is_some_and(|d| pointer.y >= d.origin.y);
        (below && here.is_some()).then_some(reorder::Regroup::ToAuto)
    }

    /// The pinned groups there are, in sidebar order, with the name each
    /// header reads — for menus that offer them.
    pub(crate) fn pinned_group_names(&self) -> Vec<(GroupId, String)> {
        let names = pinned_names(&self.sidebar_groups.pinned);
        self.sidebar_groups
            .pinned
            .iter()
            .map(|g| g.id)
            .zip(names)
            .collect()
    }

    /// Change this workspace's groups, and send the change to the machine so
    /// every other window onto the workspace draws it too.
    pub(crate) fn edit_groups(
        &mut self,
        cx: &mut Context<Self>,
        edit: impl FnOnce(&mut WorkspaceGroups),
    ) {
        edit(&mut self.sidebar_groups);
        // Up as one `WorkspaceSetGroups`, queued ahead of any `TabSetGroup`
        // the same edit made — which the save below sends.
        crate::ui::tree_sync::push_groups(cx, self.workspace, self.sidebar_groups.clone());
        self.save_session(cx);
        cx.notify();
    }

    /// Take the workspace's groups as another window, or the machine, has them.
    ///
    /// Every tab's [`EntryWatch`](crate::core::group_key::EntryWatch) starts
    /// over from where it is. A folder that arrives from elsewhere is not one
    /// this window's tabs walked into — and in particular, a window that drew
    /// a frame before its copy of the groups landed saw no folders at all, so
    /// without this every tab sitting in a pinned folder at launch would read
    /// the groups landing as an entry, and a tab dragged out of its folder
    /// group would be pulled back in on every start. The window that pinned
    /// the folder gathers the tabs itself and says so tab by tab.
    pub(crate) fn adopt_sidebar_groups(&mut self, groups: WorkspaceGroups, cx: &mut Context<Self>) {
        if self.sidebar_groups == groups {
            return;
        }
        self.sidebar_groups = groups;
        for tab in &self.tabs {
            tab.folder_watch
                .set(crate::core::group_key::EntryWatch::baseline());
        }
        cx.notify();
    }

    /// Put the dragged tab where it was dropped.
    ///
    /// By id rather than index: a drag is several frames long, and a tab
    /// closing anywhere else in that time would shift every index after it.
    pub(crate) fn regroup_tab(
        &mut self,
        tab: lyrel_core::core::machine::TabId,
        target: reorder::Regroup,
        cx: &mut Context<Self>,
    ) {
        let Some(index) = self.tabs.iter().position(|t| t.id() == tab) else {
            return;
        };
        let group = match target {
            reorder::Regroup::Into(id) => Some(id),
            reorder::Regroup::ToAuto => None,
        };
        self.set_tab_group(index, group, cx);
    }

    /// Put tab `index` in pinned group `group`, or hand it back to auto
    /// grouping when `group` is `None`.
    pub(crate) fn set_tab_group(
        &mut self,
        index: usize,
        group: Option<GroupId>,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.tabs.get(index) else {
            return;
        };
        tab.group.set(group);
        // Carries the move to the daemon as a `TabSetGroup`, so another
        // window on the same workspace sees it too.
        self.save_session(cx);
        cx.notify();
    }

    /// Put tab `index` in group `key`. An auto group's members are decided by
    /// their cwds, so a tab goes into one by pinning the group, as its
    /// header's pin does — the tab joins in the same edit.
    pub(crate) fn move_tab_to(&mut self, index: usize, key: GroupKey, cx: &mut Context<Self>) {
        match key {
            GroupKey::Pinned(id) => self.set_tab_group(index, Some(id), cx),
            GroupKey::Auto(auto) => self.pin_auto_group_with(auto, Some(index), cx),
        }
    }

    /// A name no pinned group is using yet, for a group about to be made.
    ///
    /// The placeholder only has to be unique — the rename box opens on it
    /// selected, so the first keystroke replaces it. Unique still matters:
    /// two headers reading the same name are two groups nobody can tell
    /// apart.
    fn fresh_group_name(&self) -> String {
        let taken: Vec<String> = self
            .pinned_group_names()
            .into_iter()
            .map(|(_, n)| n)
            .collect();
        let base = t(L10nKey::SidebarNewGroupName).to_string();
        (1..)
            .map(|n| match n {
                1 => base.clone(),
                n => format!("{base} {n}"),
            })
            .find(|candidate| !taken.contains(candidate))
            .expect("an unbounded range always reaches an untaken name")
    }

    /// Make a new label group, put tab `index` in it, and open its header for
    /// renaming.
    ///
    /// No dialog: the tab is in the group before a character is typed, so
    /// what the name is being given to is on screen while it is chosen.
    pub(crate) fn new_tab_group(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let group = PinnedGroup::label(self.fresh_group_name());
        let id = group.id;
        if let Some(tab) = self.tabs.get(index) {
            tab.group.set(Some(id));
        }
        self.edit_groups(cx, |groups| groups.pinned.push(group));
        self.start_group_rename(id, window, cx);
    }

    /// Make a new, empty label group and open its header for renaming — the
    /// palette's "New Group". It stays on screen empty until a tab is dragged
    /// into it or opened from its header.
    pub(crate) fn new_empty_group(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let group = PinnedGroup::label(self.fresh_group_name());
        let id = group.id;
        self.edit_groups(cx, |groups| groups.pinned.push(group));
        self.start_group_rename(id, window, cx);
    }

    /// Keep the auto group `key`: pin it, with the tabs in it now.
    ///
    /// A repo group becomes a folder group on the repo's home, so tabs that
    /// walk into the repo later join it too, and it reads the repo's name
    /// until renamed. A host group has no folder on this workspace's machine
    /// to keep, so it becomes a label group named after the host.
    ///
    /// The fold comes along: a group that was shut stays shut, rather than
    /// springing open in its new place.
    pub(crate) fn pin_auto_group(&mut self, key: AutoKey, cx: &mut Context<Self>) {
        self.pin_auto_group_with(key, None, cx);
    }

    /// [`pin_auto_group`](Self::pin_auto_group), taking tab `also` into the
    /// new group in the same edit.
    fn pin_auto_group_with(&mut self, key: AutoKey, also: Option<usize>, cx: &mut Context<Self>) {
        let mut group = match &key {
            AutoKey::Repo(root) => PinnedGroup::folder(root),
            AutoKey::SshHost(host) => PinnedGroup::label(host.clone()),
        };
        group.collapsed = self.sidebar_groups.auto_collapsed.contains(&key);
        // A repo whose folder is already kept — a tab dragged out of that
        // group while still in the repo is drawn under the repo again — joins
        // the kept group rather than making a second: two groups keeping one
        // folder split its tabs between them by nothing but list order.
        let kept = group.folder.as_deref().and_then(|folder| {
            self.sidebar_groups
                .pinned
                .iter()
                .find(|g| g.folder.as_deref() == Some(folder))
                .map(|g| g.id)
        });
        let id = kept.unwrap_or(group.id);
        let wanted = Some(GroupKey::Auto(key.clone()));
        for (i, (tab, place)) in self
            .tabs
            .iter()
            .zip(self.sidebar_group_keys(cx))
            .enumerate()
        {
            if place == wanted || also == Some(i) {
                tab.group.set(Some(id));
            }
        }
        self.edit_groups(cx, |groups| {
            groups.auto_collapsed.retain(|k| *k != key);
            if kept.is_none() {
                groups.pinned.push(group);
            }
        });
    }

    /// Pin `folder` as a group of its own — a folder dropped from Finder, or
    /// "Pin as Group" in the file tree. Pinning one that is already pinned
    /// does nothing: two groups keeping one folder would split its tabs
    /// between them by nothing but list order.
    ///
    /// Tabs already sitting in the folder are gathered in on the next frame,
    /// the way a tab walking in would be: the folder is new, so each of them
    /// has just "entered" it as far as [`EntryWatch`](crate::core::group_key::EntryWatch) can tell.
    pub(crate) fn pin_folder(&mut self, folder: PathBuf, cx: &mut Context<Self>) -> GroupId {
        let spelled = folder.to_string_lossy();
        if let Some(kept) = self
            .sidebar_groups
            .pinned
            .iter()
            .find(|g| g.folder.as_deref() == Some(&*spelled))
        {
            return kept.id;
        }
        let group = PinnedGroup::folder(&folder);
        let id = group.id;
        self.edit_groups(cx, |groups| groups.pinned.push(group));
        id
    }

    /// The palette's "Open Folder as Group…": pick a folder on this computer,
    /// pin it, and open a tab in it — "open" is the promise, and an empty group
    /// with nothing in it would not keep it.
    pub(crate) fn open_folder_as_group(&mut self, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(gpui_kit::PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: None,
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await
                && let Some(path) = paths.into_iter().next()
            {
                let _ = this.update_in(cx, |this, window, cx| {
                    let id = this.pin_folder(path, cx);
                    this.new_tab_in_group(GroupKey::Pinned(id), window, cx);
                });
            }
        })
        .detach();
    }

    /// Folders dropped on the sidebar from Finder or Explorer, pinned as
    /// groups. Anything that is not a directory is let fall: a file names no
    /// group, and guessing its parent would pin something nobody pointed at.
    ///
    /// Only on a workspace on this computer. A dropped path is this
    /// machine's, and a folder group keeps a directory on the workspace's
    /// host — on a remote one the same spelling names something else, or
    /// nothing at all.
    pub(crate) fn pin_dropped_folders(&mut self, paths: &[PathBuf], cx: &mut Context<Self>) {
        if lyrel_ui_base::session::WorkspaceStore::all(cx)
            .get(self.workspace)
            .is_some_and(|w| w.is_remote())
        {
            return;
        }
        for path in paths.iter().filter(|p| p.is_dir()) {
            self.pin_folder(path.clone(), cx);
        }
    }

    /// Delete pinned group `id`. Its tabs are not closed — they go back to
    /// auto grouping, which is where a tab nobody filed by hand belongs.
    ///
    /// Unpinning a folder group is the same act: the group stops being kept,
    /// and its tabs fall back to the auto groups their cwds resolve to.
    pub(crate) fn delete_group(&mut self, id: GroupId, cx: &mut Context<Self>) {
        for tab in &self.tabs {
            if tab.group.get() == Some(id) {
                tab.group.set(None);
            }
        }
        if self.group_rename.as_ref().is_some_and(|r| r.group == id) {
            self.group_rename = None;
        }
        self.edit_groups(cx, |groups| groups.pinned.retain(|g| g.id != id));
    }

    /// Point pinned group `id` at `folder`, or make it a label group when
    /// `folder` is `None`.
    ///
    /// A folder group nobody renamed reads its folder's name, and would read
    /// nothing once the folder is gone — so clearing the folder keeps the
    /// name it was showing.
    pub(crate) fn set_group_folder(
        &mut self,
        id: GroupId,
        folder: Option<PathBuf>,
        cx: &mut Context<Self>,
    ) {
        let shown = self
            .pinned_group_names()
            .into_iter()
            .find(|(g, _)| *g == id)
            .map(|(_, n)| n);
        self.edit_groups(cx, |groups| {
            let Some(group) = groups.get_mut(id) else {
                return;
            };
            if folder.is_none() && group.given_name().is_none() {
                group.name = shown;
            }
            group.folder = folder.map(|f| f.to_string_lossy().into_owned());
        });
    }

    /// Ask for a folder for pinned group `id` with the system picker. Only
    /// for a workspace on this machine: the picker browses this computer, and
    /// a path picked here means nothing to a remote one.
    pub(crate) fn pick_group_folder(&mut self, id: GroupId, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(gpui_kit::PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: None,
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await
                && let Some(path) = paths.into_iter().next()
            {
                let _ = this.update(cx, |this, cx| this.set_group_folder(id, Some(path), cx));
            }
        })
        .detach();
    }

    /// The directory the active tab is working in, on the workspace's host —
    /// what "Use Current Tab's Folder" pins a group to.
    pub(crate) fn active_tab_folder(&self, window: &Window, cx: &gpui_kit::App) -> Option<PathBuf> {
        let leaf = self
            .tabs
            .get(self.tabs.active_index())?
            .pane
            .focused_or_first(window, cx)?;
        leaf.read(cx).effective_host_cwd()
    }

    /// Open a tab in the group drawn under `key` — a header's "+" and its
    /// menu's "New Tab".
    ///
    /// A folder group opens in its folder, a repo group in its repo; a label
    /// group has no directory of its own and opens where ⌘T would. The tab
    /// joins a pinned group outright rather than waiting for its cwd to walk
    /// in: a label group has nothing to walk into, and "new tab here" should
    /// not depend on a probe.
    pub(crate) fn new_tab_in_group(
        &mut self,
        key: GroupKey,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let before = self.tabs.len();
        let folder = match &key {
            GroupKey::Pinned(id) => self
                .sidebar_groups
                .get(*id)
                .and_then(|g| g.folder_path().map(Path::to_path_buf)),
            GroupKey::Auto(AutoKey::Repo(root)) => Some(root.clone()),
            GroupKey::Auto(AutoKey::SshHost(_)) => None,
        };
        match folder {
            Some(folder) => self.new_tab_at(folder, window, cx),
            None => self.new_tab(window, cx),
        }
        if self.tabs.len() == before {
            return;
        }
        if let GroupKey::Pinned(id) = key
            && let Some(tab) = self.tabs.get(self.tabs.active_index())
        {
            tab.group.set(Some(id));
            self.save_session(cx);
        }
    }

    /// Put the pinned groups in `order` — indices into the list as it stands.
    pub(crate) fn apply_pinned_order(&mut self, order: &[usize], cx: &mut Context<Self>) {
        let pinned = &self.sidebar_groups.pinned;
        if order.len() != pinned.len() || order.iter().enumerate().all(|(i, &o)| i == o) {
            return;
        }
        let reordered: Vec<PinnedGroup> = order
            .iter()
            .filter_map(|&i| pinned.get(i).cloned())
            .collect();
        if reordered.len() != pinned.len() {
            return;
        }
        self.edit_groups(cx, |groups| groups.pinned = reordered);
    }

    /// Open the header of pinned group `id` for renaming.
    pub(crate) fn start_group_rename(
        &mut self,
        id: GroupId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(current) = self
            .pinned_group_names()
            .into_iter()
            .find(|(g, _)| *g == id)
            .map(|(_, n)| n)
        else {
            return;
        };
        let input = Self::rename_box(current, window, cx);
        let subs = vec![cx.subscribe_in(
            &input,
            window,
            |this, _input, ev: &InputEvent, window, cx| match ev {
                InputEvent::PressEnter { .. } | InputEvent::Blur => {
                    this.commit_group_rename(window, cx)
                }
                _ => {}
            },
        )];
        self.group_rename = Some(crate::ui::app::GroupRename {
            group: id,
            input,
            _subs: subs,
        });
        cx.notify();
    }

    /// Write the typed name onto the group being renamed.
    pub(crate) fn commit_group_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(rename) = self.group_rename.take() else {
            return;
        };
        let value = rename.input.read(cx).value().trim().to_string();
        self.edit_groups(cx, |groups| {
            let Some(group) = groups.get_mut(rename.group) else {
                return;
            };
            match (value.is_empty(), group.folder.is_some()) {
                // A folder group handed a blank name goes back to reading
                // its folder's — which is how a rename is undone.
                (true, true) => group.name = None,
                // A label group has nothing to fall back on: a blank name
                // would draw an unlabelled header, so it keeps the one it had.
                (true, false) => {}
                (false, _) => group.name = Some(value),
            }
        });
        self.focus_active(window, cx);
        cx.notify();
    }

    /// Fold the group drawn under `key` (`None`: Ungrouped), or unfold it if
    /// it is already shut. Stored with the workspace: a group folded away is a
    /// statement about work you are done with for now, and every window onto
    /// the workspace — and the next launch — should still show it shut.
    pub(crate) fn toggle_sidebar_group(&mut self, key: Option<&GroupKey>, cx: &mut Context<Self>) {
        self.edit_groups(cx, |groups| groups.toggle_folded(key));
    }

    /// Rule 2: a tab whose cwd walks into a pinned folder joins that group.
    ///
    /// Run once per frame, before anything asks where a tab is drawn. Every
    /// tab's [`EntryWatch`](crate::core::group_key::EntryWatch) is fed whether or not the tab is in a pinned group
    /// — that is what lets a tab dragged out of a folder group while it is
    /// still inside the folder stay out: the watch already knows it is
    /// inside, so staying there is no entry. Only a tab not already kept in a
    /// pinned group acts on an entry; a kept tab never leaves on its own.
    ///
    /// A tab is only looked at once its repo probe has answered. Before that
    /// the question "is it inside" has half an answer — the cwd without the
    /// repo home a worktree is matched by — and recording that half as where
    /// the tab *was* would read the other half landing a frame later as an
    /// entry, pulling every restored worktree tab into its group at launch.
    ///
    /// SSH panes are never looked at: their cwd is on another machine, and a
    /// folder pinned in this workspace names a directory on this one.
    ///
    /// Also where a tab's auto-group hint is refreshed from the live answer
    /// and, when that answer moved, sent up with the tab — so the next launch
    /// draws the tab where it last was instead of in Ungrouped.
    pub(crate) fn settle_sidebar_groups(&mut self, cx: &mut Context<Self>) {
        let mut changed = false;
        for tab in &self.tabs {
            let before = tab.auto_group.borrow().clone();
            changed |= self.tab_auto_group(tab, cx) != before;
        }
        let Some(cache) = cx.try_global::<GitStatusCache>() else {
            if changed {
                self.save_session(cx);
            }
            return;
        };
        let mut joined = changed;
        for tab in &self.tabs {
            let Some(leaf) = tab.pane.first_leaf() else {
                continue;
            };
            let Some(view) = leaf.terminal() else {
                continue;
            };
            let view = view.read(cx);
            if view.remote_context().is_some() {
                continue;
            }
            let Some(cwd) = view.git_status_cwd() else {
                continue;
            };
            let Some(home) = cache.known_repo_for(view.host_id(), cwd) else {
                continue;
            };
            let inside = self.sidebar_groups.folder_for(Some(cwd), home.as_deref());
            let mut watch = tab.folder_watch.get();
            let entered = watch.observe(inside);
            tab.folder_watch.set(watch);
            let kept = tab
                .group
                .get()
                .is_some_and(|g| self.sidebar_groups.contains(g));
            if let Some(id) = entered
                && !kept
            {
                tab.group.set(Some(id));
                joined = true;
            }
        }
        if joined {
            self.save_session(cx);
        }
    }

    /// The auto group tab `tab` resolves to (rule 3): the host for an SSH
    /// pane, otherwise the repo its cwd is in, otherwise none (Ungrouped).
    ///
    /// Remembered on the tab, and the memory is what answers while there is
    /// nothing to go on — a probe in flight, or a pane (a WSL one, say) whose
    /// paths no probe reaches — so a tab does not bounce through Ungrouped
    /// between one answer and the next.
    fn tab_auto_group(&self, tab: &Tab, cx: &gpui_kit::App) -> Option<AutoKey> {
        let resolved = tab.pane.first_leaf().and_then(|leaf| {
            let view = leaf.terminal()?.read(cx);
            let host = ssh_host(view.remote_context().as_ref());
            let known = view.git_status_cwd().and_then(|cwd| {
                cx.try_global::<GitStatusCache>()?
                    .known_repo_for(view.host_id(), cwd)
            });
            auto_key(host.as_deref(), known)
        });
        if let Some(key) = resolved {
            *tab.auto_group.borrow_mut() = key;
        }
        tab.auto_group.borrow().clone()
    }

    /// Where each tab is drawn, in `self.tabs` order: its pinned group, its
    /// auto group, or `None` for Ungrouped (or for the flat list below the
    /// pinned groups, with auto grouping off).
    pub(crate) fn sidebar_group_keys(&self, cx: &gpui_kit::App) -> Vec<Option<GroupKey>> {
        let auto_grouping = cx.global::<Config>().sidebar_auto_grouping;
        self.tabs
            .iter()
            .map(|tab| {
                // Resolved even with auto grouping off, so the remembered
                // answer is warm the moment it is turned back on.
                let auto = self.tab_auto_group(tab, cx);
                place(tab.group.get(), &self.sidebar_groups, auto_grouping, auto)
            })
            .collect()
    }

    pub(crate) fn visual_tab_order(&self, cx: &gpui_kit::App) -> Vec<usize> {
        let keys = self.sidebar_group_keys(cx);
        sidebar_sections(&keys, &self.sidebar_groups)
            .into_iter()
            .flat_map(|s| s.tabs)
            .collect()
    }

    /// The tabs drawn in the same run as `index`, in `self.tabs` order: its
    /// sidebar section on a left tab bar, every tab on a top one. Empty when
    /// no run holds `index`.
    pub(crate) fn tab_run(&self, index: usize, cx: &gpui_kit::App) -> Vec<usize> {
        let keys = self.sidebar_group_keys(cx);
        sidebar_sections(&keys, &self.sidebar_groups)
            .into_iter()
            .map(|s| s.tabs)
            .find(|tabs| tabs.contains(&index))
            .unwrap_or_default()
    }

    pub(crate) fn activate_visual(
        &mut self,
        n: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(&i) = self.visual_tab_order(cx).get(n) {
            self.activate(i, window, cx);
        }
    }

    /// Where a tab about to be spawned in `cwd` goes.
    ///
    /// A tab spawned from one kept in a pinned group joins it, before the cwd
    /// is consulted at all — the cwd says nothing about a group the user
    /// stated by hand. Every caller here spawns from the active tab (the ones
    /// that start from a named tab activate it first), so that is the one to
    /// inherit from. Without this, ⌘T inside a folded group would draw
    /// nothing but the header's count going up by one.
    ///
    /// Otherwise a cwd inside a pinned folder joins that folder's group, and
    /// the auto group is seeded from the repo cache when its probe for that
    /// directory has already landed. A tab's auto group otherwise starts
    /// empty and only fills in once its shell has reported a cwd, which parks
    /// every new tab in Ungrouped at the bottom of the sidebar until then; a
    /// tab spawned from one already in a repo inherits a warm cache, so this
    /// lands it in its group on the first frame.
    pub(crate) fn spawn_group(&self, cwd: Option<&Path>, cx: &gpui_kit::App) -> SpawnPlace {
        let kept = self
            .tabs
            .get(self.tabs.active_index())
            .and_then(|t| t.group.get())
            .filter(|g| self.sidebar_groups.contains(*g));
        let known = cwd.and_then(|cwd| {
            let host = self
                .window_workspace(cx)
                .as_ref()
                .map_or(lyrel_ui_base::host_ops::HostId::LOCAL, |ws| {
                    ws.target.host_id()
                });
            cx.try_global::<GitStatusCache>()?.known_repo_for(host, cwd)
        });
        let entered = cwd.and_then(|cwd| {
            let home = known.clone().flatten();
            self.sidebar_groups.folder_for(Some(cwd), home.as_deref())
        });
        SpawnPlace {
            group: kept.or(entered),
            auto: auto_key(None, known),
        }
    }
}

/// A header that can be picked up, and which of the two orders it belongs to.
#[derive(Clone, Copy)]
enum HeaderSlot {
    /// Slot `n` among the pinned headers, above the divider.
    Pinned(usize),
    /// Slot `n` among the auto headers, below it.
    Auto(usize),
}

/// One item of a group header's menu, acting on the app when it is chosen.
fn menu_item(
    label: L10nKey,
    app: &gpui_kit::WeakEntity<LyrelApp>,
    act: impl Fn(&mut LyrelApp, &mut Window, &mut Context<LyrelApp>) + 'static,
) -> PopupMenuItem {
    let app = app.clone();
    PopupMenuItem::new(t(label)).on_click(move |_, window, cx| {
        let _ = app.update(cx, |this, cx| act(this, window, cx));
    })
}

/// A pinned group's header menu. Everything about a kept group is the
/// user's to change: its name, the folder it keeps, and whether it is kept at
/// all. Deleting closes nothing — its tabs go back to auto grouping.
///
/// "Set Folder…" asks the system picker, which browses this computer, so it
/// is only offered for a workspace on it; "Use Current Tab's Folder" works
/// anywhere, since the tab's cwd is on the workspace's own host.
fn pinned_header_menu(
    menu: gpui_kit::component::menu::PopupMenu,
    id: GroupId,
    has_folder: bool,
    local: bool,
    app: &gpui_kit::WeakEntity<LyrelApp>,
    window: &Window,
    cx: &gpui_kit::App,
) -> gpui_kit::component::menu::PopupMenu {
    let current = app
        .upgrade()
        .and_then(|a| a.read(cx).active_tab_folder(window, cx));
    let mut menu = menu.min_w(px(200.)).item(menu_item(
        L10nKey::SidebarRenameGroup,
        app,
        move |this, w, cx| this.start_group_rename(id, w, cx),
    ));
    if local {
        menu = menu.item(menu_item(
            L10nKey::SidebarSetGroupFolder,
            app,
            move |this, _, cx| this.pick_group_folder(id, cx),
        ));
    }
    menu = menu.item(
        menu_item(
            L10nKey::SidebarUseCurrentTabFolder,
            app,
            move |this, w, cx| {
                if let Some(folder) = this.active_tab_folder(w, cx) {
                    this.set_group_folder(id, Some(folder), cx);
                }
            },
        )
        .disabled(current.is_none()),
    );
    if has_folder {
        menu = menu.item(menu_item(
            L10nKey::SidebarClearGroupFolder,
            app,
            move |this, _, cx| this.set_group_folder(id, None, cx),
        ));
    }
    menu = menu
        .separator()
        .item(menu_item(
            L10nKey::SidebarGroupNewTab,
            app,
            move |this, w, cx| this.new_tab_in_group(GroupKey::Pinned(id), w, cx),
        ))
        .separator();
    if has_folder {
        menu = menu.item(menu_item(
            L10nKey::SidebarUnpinGroup,
            app,
            move |this, _, cx| this.delete_group(id, cx),
        ));
    }
    menu.item(menu_item(
        L10nKey::SidebarDeleteGroup,
        app,
        move |this, _, cx| this.delete_group(id, cx),
    ))
}

/// An auto group's header menu: pin it, or open a tab in it. Nothing else —
/// an auto group is derived, so there is nothing about it to edit until it
/// is pinned, and pinning is always an explicit act.
fn auto_header_menu(
    menu: gpui_kit::component::menu::PopupMenu,
    key: AutoKey,
    app: &gpui_kit::WeakEntity<LyrelApp>,
) -> gpui_kit::component::menu::PopupMenu {
    let pin = key.clone();
    let mut menu = menu.min_w(px(200.)).item(menu_item(
        L10nKey::SidebarPinGroup,
        app,
        move |this, _, cx| this.pin_auto_group(pin.clone(), cx),
    ));
    // A host group has no directory to open a tab in, and no way to open a
    // shell on the host that would not guess at how the others got there.
    if let AutoKey::Repo(_) = &key {
        menu = menu.item(menu_item(
            L10nKey::SidebarGroupNewTab,
            app,
            move |this, w, cx| this.new_tab_in_group(GroupKey::Auto(key.clone()), w, cx),
        ));
    }
    menu
}

/// Where a tab about to be spawned goes, worked out before it exists — see
/// [`LyrelApp::spawn_group`].
pub(crate) struct SpawnPlace {
    group: Option<GroupId>,
    auto: Option<Option<AutoKey>>,
}

impl SpawnPlace {
    /// Seed the auto group with SSH host `host`, for a tab dialling a machine
    /// whose pane has not reported its remote context yet.
    pub(crate) fn on_host(mut self, host: Option<String>) -> Self {
        if let Some(auto) = auto_key(host.as_deref(), None) {
            self.auto = Some(auto);
        }
        self
    }

    /// Put `tab` where this says.
    pub(crate) fn seat(&self, tab: &Tab) {
        if let Some(id) = self.group {
            tab.group.set(Some(id));
        }
        if let Some(auto) = &self.auto {
            *tab.auto_group.borrow_mut() = auto.clone();
        }
    }
}

/// The host an SSH pane is on, when it is one.
///
/// A native SSH pane and a shell that has ssh'd onward from a local prompt
/// are grouped alike, by the target they name: in neither case does a `Host`
/// of ours reach the far side, so neither has a repo to probe — the first
/// reports a remote cwd that names nothing here, the second only the local
/// directory the `ssh` was typed in, which says nothing about where its shell
/// now is. The host is the one thing about either that is certain. A WSL pane
/// is not an SSH one: its distro is on this machine, and it keeps the group it
/// had.
fn ssh_host(remote: Option<&crate::daemon::protocol::RemoteContext>) -> Option<String> {
    use crate::daemon::protocol::RemoteKind;
    remote
        .filter(|r| matches!(r.kind, RemoteKind::Ssh | RemoteKind::NativeSsh))
        .map(|r| r.target.clone())
}

#[derive(Debug, PartialEq)]
struct Section {
    key: Option<GroupKey>,
    name: Option<String>,
    tabs: Vec<usize>,
}

impl Section {
    fn pinned(&self) -> Option<GroupId> {
        self.key.as_ref().and_then(GroupKey::pinned)
    }
}

/// How far to move the sidebar's scroll offset so a newly active row shows.
/// A row with any part in view stays put — clicking a row must not move the
/// list under the pointer; one out of view comes in at the nearest edge.
/// `lead` is what sits on top of the row and belongs with it — its group's
/// header, for the first row — so a row coming in from above brings the
/// name of the group it is in, not just itself.
fn reveal_shift(row: (Pixels, Pixels), lead: Pixels, view: (Pixels, Pixels)) -> Pixels {
    let ((top, bottom), (view_top, view_bottom)) = (row, view);
    if bottom <= view_top {
        view_top - (top - lead)
    } else if top >= view_bottom {
        view_bottom - bottom
    } else {
        px(0.)
    }
}

/// The sidebar's sections, top to bottom: every pinned group in the user's
/// order — an empty one too, since a kept group stays until it is deleted —
/// then the auto groups in the order their first tab appears, then
/// Ungrouped.
///
/// Ungrouped only draws a header beside other groups below the divider. With
/// no auto group to set it apart from — grouping off, or nothing resolved
/// yet — it is just the list, and a header over the whole of it would be a
/// label on nothing. The same goes for a sidebar with no groups at all, which
/// comes out as one headerless section holding every tab.
fn sidebar_sections(keys: &[Option<GroupKey>], groups: &WorkspaceGroups) -> Vec<Section> {
    let members = |key: &GroupKey| -> Vec<usize> {
        (0..keys.len())
            .filter(|&i| keys[i].as_ref() == Some(key))
            .collect()
    };
    let mut auto_order: Vec<&AutoKey> = Vec::new();
    for k in keys.iter().flatten().filter_map(GroupKey::auto) {
        if !auto_order.contains(&k) {
            auto_order.push(k);
        }
    }
    let rest: Vec<usize> = (0..keys.len()).filter(|&i| keys[i].is_none()).collect();
    if groups.pinned.is_empty() && auto_order.is_empty() {
        return vec![Section {
            key: None,
            name: None,
            tabs: rest,
        }];
    }
    let pinned = pinned_names(&groups.pinned);
    let auto = auto_names(&auto_order);
    let mut sections: Vec<Section> = groups
        .pinned
        .iter()
        .zip(pinned)
        .map(|(g, name)| {
            let key = GroupKey::Pinned(g.id);
            Section {
                tabs: members(&key),
                key: Some(key),
                name: Some(name),
            }
        })
        .collect();
    sections.extend(auto_order.iter().zip(auto).map(|(k, name)| {
        let key = GroupKey::Auto((*k).clone());
        Section {
            tabs: members(&key),
            key: Some(key),
            name: Some(name),
        }
    }));
    if !rest.is_empty() {
        sections.push(Section {
            key: None,
            name: (!auto_order.is_empty()).then(|| t(L10nKey::SidebarUngroupedGroup).to_string()),
            tabs: rest,
        });
    }
    sections
}

/// A row of the tab menu's "Move to Group".
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct MoveTarget {
    pub key: GroupKey,
    pub name: String,
    pub checked: bool,
}

/// Every group the sidebar draws, in its order — pinned and auto alike —
/// with the one tab `index` is in checked.
pub(crate) fn move_targets(
    keys: &[Option<GroupKey>],
    groups: &WorkspaceGroups,
    index: usize,
) -> Vec<MoveTarget> {
    let here = keys.get(index).cloned().flatten();
    sidebar_sections(keys, groups)
        .into_iter()
        .filter_map(|s| {
            let key = s.key?;
            Some(MoveTarget {
                checked: here.as_ref() == Some(&key),
                name: s.name.expect("a section with a key has a header"),
                key,
            })
        })
        .collect()
}

fn reordered_rows(
    keys: &[Option<GroupKey>],
    groups: &WorkspaceGroups,
    group: &Option<GroupKey>,
    visible: &[usize],
    from: usize,
    to: usize,
) -> Option<Vec<usize>> {
    let (&moved, &anchor) = (visible.get(from)?, visible.get(to)?);
    if moved == anchor {
        return None;
    }
    let mut members: Vec<usize> = (0..keys.len()).filter(|&i| keys[i] == *group).collect();
    members.retain(|&i| i != moved);
    let at = members.iter().position(|&i| i == anchor)? + usize::from(to > from);
    members.insert(at, moved);

    let mut out: Vec<usize> = Vec::with_capacity(keys.len());
    for g in sidebar_sections(keys, groups).iter().map(|s| &s.key) {
        if g == group {
            out.extend_from_slice(&members);
        } else {
            out.extend((0..keys.len()).filter(|&i| keys[i] == *g));
        }
    }
    Some(out)
}

/// The tab order that moves auto group `from` to where auto group `to` is.
///
/// Auto groups are ordered by where their first tab sits, so moving one is
/// moving its tabs. Pinned groups keep an order of their own and take no part:
/// their tabs follow the auto groups' here, where they draw no differently.
fn regrouped_order(keys: &[Option<GroupKey>], from: &AutoKey, to: &AutoKey) -> Option<Vec<usize>> {
    if from == to {
        return None;
    }
    let mut order: Vec<&AutoKey> = Vec::new();
    for k in keys.iter().flatten().filter_map(GroupKey::auto) {
        if !order.contains(&k) {
            order.push(k);
        }
    }
    let fi = order.iter().position(|g| *g == from)?;
    let ti = order.iter().position(|g| *g == to)?;
    let moved = order.remove(fi);
    order.insert(ti, moved);

    let auto_of = |i: usize| keys[i].as_ref().and_then(GroupKey::auto);
    let mut out: Vec<usize> = Vec::with_capacity(keys.len());
    for g in &order {
        out.extend((0..keys.len()).filter(|&i| auto_of(i) == Some(*g)));
    }
    out.extend((0..keys.len()).filter(|&i| auto_of(i).is_none()));
    Some(out)
}

/// The order of all pinned groups after the header in visible slot `from` is
/// dropped on slot `to`, as indices into `all`. `shown` is the pinned groups
/// on screen, in order — a search can hide some, and those keep their places.
fn reordered_pinned(
    all: &[GroupId],
    shown: &[GroupId],
    from: usize,
    to: usize,
) -> Option<Vec<usize>> {
    let (&moved, &anchor) = (shown.get(from)?, shown.get(to)?);
    if moved == anchor {
        return None;
    }
    let mut order: Vec<GroupId> = all.to_vec();
    order.retain(|g| *g != moved);
    let at = order.iter().position(|g| *g == anchor)? + usize::from(to > from);
    order.insert(at, moved);
    order
        .iter()
        .map(|g| all.iter().position(|a| a == g))
        .collect()
}

/// Where a click on a tab's diff counts opens the overlay: the focused pane's
/// repo.
fn git_click(
    tab: &Tab,
    window: &Window,
    cx: &gpui_kit::App,
) -> Option<(lyrel_ui_base::host_ops::HostId, PathBuf)> {
    tab.pane.focused_or_first(window, cx).and_then(|leaf| {
        let view = leaf.read(cx);
        let cwd = view.git_status_cwd()?.to_path_buf();
        Some((view.host_id(), cwd))
    })
}

#[cfg(test)]
mod fold_tests {
    use super::*;
    use crate::ui::app::test_window::harness_with_tabs;
    use gpui_kit::TestAppContext;
    use gpui_kit::test::TestWindowExt as _;

    /// Bounds a row registered for itself while it was on screen. A folded
    /// row leaves the default rectangle behind, and that is what stops a pane
    /// being dropped into a group that is shut.
    fn drawn(app: &LyrelApp, i: usize) -> bool {
        app.sidebar_slots.borrow()[i].size.height > px(0.)
    }

    /// Put tab `i` in a directory and tell the cache that directory is in the
    /// repo `root`, whose home is `home` — the same as `root` unless the
    /// checkout is a linked worktree. Both halves are needed: the sidebar
    /// reads the tab's cwd and looks it up in the cache, and either one
    /// missing makes it return "no decision", which would leave every group
    /// below untouched and every assertion about moving vacuous.
    fn plant(app: &LyrelApp, i: usize, cwd: &str, root: &str, home: &str, cx: &mut gpui_kit::App) {
        use crate::ui::scm::status_cache::{GitStatusCache, RepoSnapshot};
        use lyrel_ui_base::host_ops::HostId;

        let cwd = PathBuf::from(cwd);
        let leaf = app.tabs[i].pane.first_leaf().expect("test tab has a pane");
        leaf.terminal()
            .expect("test pane is a terminal")
            .update(cx, |view, _| {
                view.set_git_status_cwd_for_test(Some(cwd.clone()))
            });
        cx.update_global::<GitStatusCache, _>(|cache, _| {
            cache.finish_probe(
                HostId::LOCAL,
                &cwd,
                Some(RepoSnapshot {
                    root: PathBuf::from(root),
                    home: PathBuf::from(home),
                    branch: "main".into(),
                    counts: Some((0, 0)),
                }),
            );
        });
    }

    fn plant_repo(app: &LyrelApp, i: usize, cwd: &str, root: &str, cx: &mut gpui_kit::App) {
        plant(app, i, cwd, root, root, cx);
    }

    /// Put tab `i` somewhere the cache knows is in no repo at all.
    fn plant_plain(app: &LyrelApp, i: usize, cwd: &str, cx: &mut gpui_kit::App) {
        use crate::ui::scm::status_cache::GitStatusCache;
        use lyrel_ui_base::host_ops::HostId;

        let cwd = PathBuf::from(cwd);
        let leaf = app.tabs[i].pane.first_leaf().expect("test tab has a pane");
        leaf.terminal()
            .expect("test pane is a terminal")
            .update(cx, |view, _| {
                view.set_git_status_cwd_for_test(Some(cwd.clone()))
            });
        cx.update_global::<GitStatusCache, _>(|cache, _| {
            cache.finish_probe(HostId::LOCAL, &cwd, None);
        });
    }

    fn repo(s: &str) -> Option<GroupKey> {
        Some(GroupKey::Auto(AutoKey::Repo(PathBuf::from(s))))
    }

    /// Pin a label group called `name`, answering its id.
    fn label(app: &mut LyrelApp, name: &str, cx: &mut Context<LyrelApp>) -> GroupId {
        let group = PinnedGroup::label(name);
        let id = group.id;
        app.edit_groups(cx, |groups| groups.pinned.push(group));
        id
    }

    /// Pin `folder`, answering the group's id.
    fn folder(app: &mut LyrelApp, folder: &str, cx: &mut Context<LyrelApp>) -> GroupId {
        app.pin_folder(PathBuf::from(folder), cx);
        app.sidebar_groups
            .pinned
            .iter()
            .find(|g| g.folder.as_deref() == Some(folder))
            .expect("just pinned")
            .id
    }

    /// Rule 1. A tab kept in a pinned group is not the probe's to move:
    /// without this the cwd would drag a hand-placed tab back into its repo
    /// on the very next frame, and no amount of clicking would keep it where
    /// it was put.
    ///
    /// Tab 1 is the control. It is kept nowhere, so the same probe that must
    /// leave tab 0 alone has to file tab 1 — otherwise this test would pass
    /// just as well with the probe switched off entirely.
    #[gpui_kit::test]
    fn a_probe_files_an_auto_tab_and_never_a_pinned_one(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 2);

        let work = app.update(&mut vcx, |app, cx| {
            let work = label(app, "work", cx);
            app.set_tab_group(0, Some(work), cx);
            for i in 0..2 {
                plant_repo(app, i, "/w/probed/sub", "/w/probed", cx);
            }
            cx.notify();
            work
        });
        vcx.run_until_parked();

        app.update(&mut vcx, |app, cx| {
            let keys = app.sidebar_group_keys(cx);
            assert_eq!(
                keys[0],
                Some(GroupKey::Pinned(work)),
                "the pinned group survived a probe that had a real answer"
            );
            assert_eq!(
                keys[1],
                repo("/w/probed"),
                "and that same probe did file the tab that was kept nowhere"
            );
        });
    }

    /// Dropping a tab below the divider is the way out of a pinned group: the
    /// cwd decides again.
    #[gpui_kit::test]
    fn a_tab_handed_back_to_auto_follows_its_cwd_again(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);

        let (work, dragged) = app.update(&mut vcx, |app, cx| {
            plant_repo(app, 0, "/w/probed/sub", "/w/probed", cx);
            let work = label(app, "work", cx);
            app.set_tab_group(0, Some(work), cx);
            (work, app.tabs[0].id())
        });
        vcx.run_until_parked();
        app.update(&mut vcx, |app, cx| {
            assert_eq!(app.sidebar_group_keys(cx)[0], Some(GroupKey::Pinned(work)));
        });

        app.update(&mut vcx, |app, cx| {
            app.regroup_tab(dragged, reorder::Regroup::ToAuto, cx)
        });
        vcx.run_until_parked();

        app.update(&mut vcx, |app, cx| {
            assert_eq!(
                app.sidebar_group_keys(cx)[0],
                repo("/w/probed"),
                "dropped below the divider, so the probe takes the tab back"
            );
        });
    }

    /// A drag is several frames long, so the tab it is carrying is named by
    /// id. Were it an index, any tab closing before the drop — in another
    /// window on the same workspace, or by a shell exiting — would shift it,
    /// and the drop would land on whichever tab slid into that slot.
    #[gpui_kit::test]
    fn a_drop_finds_its_tab_after_the_indexes_shift(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 3);

        let dragged = app.update(&mut vcx, |app, _| app.tabs[2].id());
        let work = app.update_in(&mut vcx, |app, window, cx| {
            let work = label(app, "work", cx);
            app.close_tab(0, window, cx);
            app.regroup_tab(dragged, reorder::Regroup::Into(work), cx);
            work
        });
        vcx.run_until_parked();

        app.update(&mut vcx, |app, _| {
            let moved = app
                .tabs
                .iter()
                .find(|t| t.id() == dragged)
                .expect("the dragged tab is still open");
            assert_eq!(
                moved.group.get(),
                Some(work),
                "the tab that was picked up is the tab that moved"
            );
            assert!(
                app.tabs
                    .iter()
                    .filter(|t| t.id() != dragged)
                    .all(|t| t.group.get().is_none()),
                "and no bystander was regrouped in its place"
            );
        });
    }

    /// ⌘T inside a pinned group has to land in it. Otherwise the new tab
    /// goes wherever its cwd says, and if the group it was opened from is
    /// folded, the only thing that happens on screen is the header's count
    /// going up by one — the symptom #804 fixed for repo groups.
    #[gpui_kit::test]
    fn a_tab_spawned_inside_a_pinned_group_joins_it(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);

        let work = app.update(&mut vcx, |app, cx| {
            // A probe that would send the tab somewhere else if it were
            // consulted, so this cannot pass by there being no answer.
            plant_repo(app, 0, "/w/probed/sub", "/w/probed", cx);
            let work = label(app, "work", cx);
            app.set_tab_group(0, Some(work), cx);
            app.tabs.select(0);
            work
        });
        vcx.run_until_parked();

        app.update(&mut vcx, |app, cx| {
            let place = app.spawn_group(Some(&PathBuf::from("/w/probed/sub")), cx);
            assert_eq!(
                place.group,
                Some(work),
                "the pinned group is inherited ahead of anything the cwd says"
            );
        });

        app.update(&mut vcx, |app, cx| app.set_tab_group(0, None, cx));
        vcx.run_until_parked();

        app.update(&mut vcx, |app, cx| {
            let place = app.spawn_group(Some(&PathBuf::from("/w/probed/sub")), cx);
            assert_eq!(place.group, None, "with nothing kept, nothing inherited");
            assert_eq!(
                place.auto,
                Some(Some(AutoKey::Repo(PathBuf::from("/w/probed")))),
                "and the cwd seeds the auto group from the warm cache"
            );
        });
    }

    /// ⌘T from an SSH tab dials the same host, and the new tab is filed
    /// under that host before its pane has reported where it is — not in
    /// Ungrouped, which is where the local shell it used to open landed.
    #[gpui_kit::test]
    fn a_tab_dialling_a_host_is_filed_under_it(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);

        app.update(&mut vcx, |app, cx| {
            let place = app.spawn_group(None, cx).on_host(Some("hermes_ali".into()));
            assert_eq!(
                place.auto,
                Some(Some(AutoKey::SshHost("hermes_ali".into())))
            );
            let place = app.spawn_group(None, cx).on_host(None);
            assert_eq!(place.auto, None, "no host, no opinion");
        });
    }

    /// A tab opened in a pinned folder is filed there before its shell has
    /// said a word — from the cwd it was opened in.
    #[gpui_kit::test]
    fn a_tab_spawned_in_a_pinned_folder_joins_it(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);

        app.update(&mut vcx, |app, cx| {
            let lyrel = folder(app, "/w/lyrel", cx);
            let place = app.spawn_group(Some(&PathBuf::from("/w/lyrel/src")), cx);
            assert_eq!(place.group, Some(lyrel));
            let place = app.spawn_group(Some(&PathBuf::from("/w/else")), cx);
            assert_eq!(place.group, None);
        });
    }

    /// A rename lands on the group itself: its tabs point at it by id, so
    /// nothing about them has to change.
    #[gpui_kit::test]
    fn renaming_a_group_keeps_its_tabs_in_it(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 2);

        let work = app.update_in(&mut vcx, |app, window, cx| {
            let work = label(app, "work", cx);
            app.set_tab_group(0, Some(work), cx);
            app.set_tab_group(1, Some(work), cx);
            app.start_group_rename(work, window, cx);
            app.group_rename
                .as_ref()
                .expect("the box is up")
                .input
                .update(cx, |state, cx| state.set_value("urgent", window, cx));
            app.commit_group_rename(window, cx);
            work
        });
        vcx.run_until_parked();

        app.update(&mut vcx, |app, _| {
            assert_eq!(app.pinned_group_names(), vec![(work, "urgent".to_string())]);
            assert!(app.tabs.iter().all(|t| t.group.get() == Some(work)));
        });
    }

    /// Clearing the box and dismissing it must not leave a label group with
    /// no name to draw; a folder group, which has its folder's name to fall
    /// back on, goes back to reading it.
    #[gpui_kit::test]
    fn a_blank_rename_keeps_a_label_and_resets_a_folder(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);

        app.update_in(&mut vcx, |app, window, cx| {
            let work = label(app, "work", cx);
            let lyrel = folder(app, "/w/lyrel", cx);
            app.edit_groups(cx, |groups| {
                groups.get_mut(lyrel).expect("pinned").name = Some("mine".into())
            });
            for id in [work, lyrel] {
                app.start_group_rename(id, window, cx);
                app.group_rename
                    .as_ref()
                    .expect("the box is up")
                    .input
                    .update(cx, |state, cx| state.set_value("   ", window, cx));
                app.commit_group_rename(window, cx);
            }
            assert_eq!(
                app.pinned_group_names(),
                vec![(work, "work".to_string()), (lyrel, "lyrel".to_string())],
                "the label kept its name; the folder went back to its own"
            );
        });
    }

    /// The placeholder only has to be unique — the box opens selected, so
    /// the first keystroke replaces it. But two headers with one name are two
    /// groups nobody can tell apart.
    #[gpui_kit::test]
    fn a_second_new_group_does_not_land_on_the_first(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 2);

        app.update_in(&mut vcx, |app, window, cx| {
            app.new_tab_group(0, window, cx);
            app.commit_group_rename(window, cx);
            app.new_tab_group(1, window, cx);
            app.commit_group_rename(window, cx);
        });
        vcx.run_until_parked();

        app.update(&mut vcx, |app, _| {
            let names = app.pinned_group_names();
            assert_eq!(names.len(), 2, "two groups, not one shared by both tabs");
            assert_ne!(names[0].1, names[1].1);
            assert_ne!(app.tabs[0].group.get(), app.tabs[1].group.get());
        });
    }

    /// "Auto grouping off" hides the groups the sidebar works out, and
    /// Ungrouped's header with them — but a pinned group is the user's, and
    /// still stands.
    #[gpui_kit::test]
    fn auto_grouping_off_hides_auto_groups_and_keeps_pinned_ones(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 3);

        let work = app.update(&mut vcx, |app, cx| {
            let work = label(app, "work", cx);
            app.set_tab_group(0, Some(work), cx);
            plant_repo(app, 1, "/w/alpha", "/w/alpha", cx);
            plant_plain(app, 2, "/tmp", cx);
            let mut cfg = cx.global::<Config>().clone();
            cfg.sidebar_auto_grouping = false;
            cx.set_global(cfg);
            cx.notify();
            work
        });
        vcx.run_until_parked();

        app.update(&mut vcx, |app, cx| {
            let keys = app.sidebar_group_keys(cx);
            assert_eq!(keys, vec![Some(GroupKey::Pinned(work)), None, None]);
            let sections = sidebar_sections(&keys, &app.sidebar_groups);
            assert_eq!(
                sections.iter().map(|s| s.name.clone()).collect::<Vec<_>>(),
                vec![Some("work".into()), None],
                "the pinned header, then a flat list with no header over it"
            );
        });

        app.update(&mut vcx, |_, cx| {
            let mut cfg = cx.global::<Config>().clone();
            cfg.sidebar_auto_grouping = true;
            cx.set_global(cfg);
            cx.notify();
        });
        vcx.run_until_parked();

        app.update(&mut vcx, |app, cx| {
            let keys = app.sidebar_group_keys(cx);
            assert_eq!(
                keys,
                vec![Some(GroupKey::Pinned(work)), repo("/w/alpha"), None]
            );
            let sections = sidebar_sections(&keys, &app.sidebar_groups);
            assert_eq!(
                sections.last().and_then(|s| s.name.clone()),
                Some("Ungrouped".into()),
                "beside an auto group, the rest is Ungrouped again"
            );
        });
    }

    /// Rule 2: a tab walking into a pinned folder joins it.
    #[gpui_kit::test]
    fn a_tab_walking_into_a_pinned_folder_joins_it(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 2);

        let probed = app.update(&mut vcx, |app, cx| {
            plant_plain(app, 0, "/w/elsewhere", cx);
            plant_plain(app, 1, "/w/elsewhere", cx);
            let probed = folder(app, "/w/probed", cx);
            cx.notify();
            probed
        });
        vcx.run_until_parked();
        app.update(&mut vcx, |app, cx| {
            plant_repo(app, 0, "/w/probed/sub", "/w/probed", cx);
            cx.notify();
        });
        vcx.run_until_parked();

        app.update(&mut vcx, |app, cx| {
            assert_eq!(app.tabs[0].group.get(), Some(probed), "walked in: joined");
            assert_eq!(app.tabs[1].group.get(), None, "stayed out: did not");
            assert_eq!(
                app.sidebar_group_keys(cx)[0],
                Some(GroupKey::Pinned(probed)),
                "and the pinned folder beats the repo root it sits in"
            );
        });
    }

    /// Nested folders: the deepest one a tab is inside wins, so pinning a
    /// monorepo and a package in it files the package's tabs under the
    /// package.
    #[gpui_kit::test]
    fn nested_pinned_folders_file_a_tab_under_the_deepest(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 2);

        let (mono, pkg) = app.update(&mut vcx, |app, cx| {
            let mono = folder(app, "/w/mono", cx);
            let pkg = folder(app, "/w/mono/pkg", cx);
            plant_repo(app, 0, "/w/mono/pkg/src", "/w/mono", cx);
            plant_repo(app, 1, "/w/mono/docs", "/w/mono", cx);
            cx.notify();
            (mono, pkg)
        });
        vcx.run_until_parked();

        app.update(&mut vcx, |app, _| {
            assert_eq!(app.tabs[0].group.get(), Some(pkg));
            assert_eq!(app.tabs[1].group.get(), Some(mono));
        });
    }

    /// A linked worktree lives outside its main checkout, but its repo home
    /// is that checkout — so pinning the repo keeps its worktrees too.
    #[gpui_kit::test]
    fn a_worktree_joins_the_folder_its_repo_home_is_pinned_as(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);

        let lyrel = app.update(&mut vcx, |app, cx| {
            let lyrel = folder(app, "/w/lyrel", cx);
            plant(
                app,
                0,
                "/tmp/wt/feature/src",
                "/tmp/wt/feature",
                "/w/lyrel",
                cx,
            );
            cx.notify();
            lyrel
        });
        vcx.run_until_parked();

        app.update(&mut vcx, |app, _| {
            assert_eq!(app.tabs[0].group.get(), Some(lyrel));
        });
    }

    /// The edge the whole rule turns on. A tab dragged out of a folder group
    /// while it is still in the folder has been told where to go; it must not
    /// be pulled straight back, and only walking out and in again rejoins it.
    #[gpui_kit::test]
    fn a_tab_dragged_out_is_not_pulled_back_until_it_re_enters(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);

        let probed = app.update(&mut vcx, |app, cx| {
            let probed = folder(app, "/w/probed", cx);
            plant_repo(app, 0, "/w/probed/sub", "/w/probed", cx);
            cx.notify();
            probed
        });
        vcx.run_until_parked();
        app.update(&mut vcx, |app, cx| {
            assert_eq!(
                app.tabs[0].group.get(),
                Some(probed),
                "joined on the way in"
            );
            app.set_tab_group(0, None, cx);
        });
        for _ in 0..3 {
            app.update(&mut vcx, |_, cx| cx.notify());
            vcx.run_until_parked();
        }
        app.update(&mut vcx, |app, cx| {
            assert_eq!(app.tabs[0].group.get(), None, "still inside: stays out");
            plant_plain(app, 0, "/w/elsewhere", cx);
            cx.notify();
        });
        vcx.run_until_parked();
        app.update(&mut vcx, |app, cx| {
            assert_eq!(app.tabs[0].group.get(), None, "left: nothing to join");
            plant_repo(app, 0, "/w/probed/sub", "/w/probed", cx);
            cx.notify();
        });
        vcx.run_until_parked();
        app.update(&mut vcx, |app, _| {
            assert_eq!(app.tabs[0].group.get(), Some(probed), "re-entered: joined");
        });
    }

    /// A tab restored from the machine tree was filed by whoever had it
    /// last. Sitting in a folder at launch is not walking into it.
    #[gpui_kit::test]
    fn a_restored_tab_is_not_pulled_in_by_where_it_already_is(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);

        app.update(&mut vcx, |app, cx| {
            app.tabs[0]
                .folder_watch
                .set(crate::core::group_key::EntryWatch::baseline());
            folder(app, "/w/probed", cx);
            plant_repo(app, 0, "/w/probed/sub", "/w/probed", cx);
            cx.notify();
        });
        vcx.run_until_parked();

        app.update(&mut vcx, |app, _| {
            assert_eq!(app.tabs[0].group.get(), None);
        });
    }

    /// Deleting a group closes nothing: its tabs go back to auto grouping.
    #[gpui_kit::test]
    fn deleting_a_group_returns_its_tabs_to_auto_grouping(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 2);

        let work = app.update(&mut vcx, |app, cx| {
            let work = label(app, "work", cx);
            plant_repo(app, 0, "/w/alpha", "/w/alpha", cx);
            app.set_tab_group(0, Some(work), cx);
            app.set_tab_group(1, Some(work), cx);
            work
        });
        vcx.run_until_parked();

        app.update(&mut vcx, |app, cx| app.delete_group(work, cx));
        vcx.run_until_parked();

        app.update(&mut vcx, |app, cx| {
            assert_eq!(app.tabs.len(), 2, "no tab was closed");
            assert!(app.sidebar_groups.pinned.is_empty());
            assert!(app.tabs.iter().all(|t| t.group.get().is_none()));
            assert_eq!(app.sidebar_group_keys(cx)[0], repo("/w/alpha"));
        });
    }

    /// Pinning an auto group keeps the tabs in it now, keeps its fold, and —
    /// for a repo — keeps its folder, so tabs walking in later join too.
    #[gpui_kit::test]
    fn pinning_an_auto_group_keeps_its_tabs_and_its_folder(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 3);

        app.update(&mut vcx, |app, cx| {
            plant_repo(app, 0, "/w/alpha", "/w/alpha", cx);
            plant_repo(app, 1, "/w/alpha/src", "/w/alpha", cx);
            plant_repo(app, 2, "/w/beta", "/w/beta", cx);
            app.toggle_sidebar_group(repo("/w/alpha").as_ref(), cx);
            cx.notify();
        });
        vcx.run_until_parked();
        app.update(&mut vcx, |app, cx| {
            app.pin_auto_group(AutoKey::Repo(PathBuf::from("/w/alpha")), cx)
        });
        vcx.run_until_parked();

        app.update(&mut vcx, |app, _| {
            let group = app.sidebar_groups.pinned.first().expect("pinned");
            assert_eq!(group.folder.as_deref(), Some("/w/alpha"));
            assert!(group.collapsed, "the fold came along");
            assert!(app.sidebar_groups.auto_collapsed.is_empty());
            assert_eq!(app.tabs[0].group.get(), Some(group.id));
            assert_eq!(app.tabs[1].group.get(), Some(group.id));
            assert_eq!(app.tabs[2].group.get(), None, "beta stays auto");
        });
    }

    /// Moving a tab into an auto group pins that group, in one edit: the
    /// group's own tabs and the moved one end up in a single folder group.
    #[gpui_kit::test]
    fn moving_a_tab_into_an_auto_group_pins_it_with_the_tab(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 3);
        let r = AutoKey::Repo(PathBuf::from("/w/r"));
        app.update(&mut vcx, |app, cx| {
            for i in 0..2 {
                *app.tabs[i].auto_group.borrow_mut() = Some(r.clone());
            }
            *app.tabs[2].auto_group.borrow_mut() = Some(AutoKey::Repo(PathBuf::from("/w/s")));
            cx.notify();
        });
        vcx.run_until_parked();

        app.update(&mut vcx, |app, cx| {
            app.move_tab_to(2, GroupKey::Auto(r.clone()), cx)
        });
        vcx.run_until_parked();

        app.update(&mut vcx, |app, cx| {
            let [group] = app.sidebar_groups.pinned.as_slice() else {
                panic!("exactly one pinned group: {:?}", app.sidebar_groups.pinned);
            };
            assert_eq!(group.folder.as_deref(), Some("/w/r"));
            for i in 0..3 {
                assert_eq!(app.tabs[i].group.get(), Some(group.id), "tab {i}");
            }
            let keys = app.sidebar_group_keys(cx);
            assert!(
                !keys.contains(&Some(GroupKey::Auto(r.clone()))),
                "r is no longer an auto group"
            );
        });
    }

    /// A tab dragged out of a folder group while still in the repo is drawn
    /// under the repo's auto group, beside the folder group keeping the same
    /// directory. Moving a tab into that auto group joins the kept group
    /// rather than pinning the folder a second time.
    #[gpui_kit::test]
    fn moving_into_an_auto_group_whose_folder_is_kept_joins_the_kept_group(
        cx: &mut TestAppContext,
    ) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 3);
        let r = AutoKey::Repo(PathBuf::from("/w/r"));
        let kept = PinnedGroup::folder(Path::new("/w/r"));
        let id = kept.id;
        app.update(&mut vcx, |app, cx| {
            app.sidebar_groups.pinned.push(kept);
            app.tabs[0].group.set(Some(id));
            for i in 0..2 {
                *app.tabs[i].auto_group.borrow_mut() = Some(r.clone());
            }
            *app.tabs[2].auto_group.borrow_mut() = Some(AutoKey::Repo(PathBuf::from("/w/s")));
            cx.notify();
        });
        vcx.run_until_parked();
        app.update(&mut vcx, |app, cx| {
            assert!(
                app.sidebar_group_keys(cx)
                    .contains(&Some(GroupKey::Auto(r.clone()))),
                "tab 1 is drawn under the repo's auto group"
            );
            app.move_tab_to(2, GroupKey::Auto(r.clone()), cx)
        });
        vcx.run_until_parked();

        app.update(&mut vcx, |app, _| {
            let [group] = app.sidebar_groups.pinned.as_slice() else {
                panic!("still one pinned group: {:?}", app.sidebar_groups.pinned);
            };
            assert_eq!(group.id, id);
            for i in 0..3 {
                assert_eq!(app.tabs[i].group.get(), Some(id), "tab {i}");
            }
        });
    }

    /// A pinned group with no tabs is still drawn — it is kept until it is
    /// deleted — while an auto group with none simply is not there.
    #[gpui_kit::test]
    fn an_empty_pinned_group_still_has_a_section(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);

        app.update(&mut vcx, |app, cx| {
            let work = label(app, "work", cx);
            let keys = app.sidebar_group_keys(cx);
            let sections = sidebar_sections(&keys, &app.sidebar_groups);
            assert_eq!(sections[0].key, Some(GroupKey::Pinned(work)));
            assert!(sections[0].tabs.is_empty());
        });
    }

    #[gpui_kit::test]
    fn folding_a_group_takes_its_rows_off_the_sidebar(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 3);
        let alpha = AutoKey::Repo(PathBuf::from("/w/alpha"));
        let beta = AutoKey::Repo(PathBuf::from("/w/beta"));

        app.update(&mut vcx, |app, cx| {
            for (i, root) in [(0, &alpha), (1, &alpha), (2, &beta)] {
                *app.tabs[i].auto_group.borrow_mut() = Some(root.clone());
            }
            app.tabs.select(2);
            cx.notify();
        });
        vcx.run_until_parked();
        vcx.executor()
            .advance_clock(std::time::Duration::from_millis(ROW_ENTER_MS + 50));
        vcx.run_until_parked();
        vcx.update(|window, cx| window.render_frame(cx));

        app.update(&mut vcx, |app, _| {
            assert!(
                (0..3).all(|i| drawn(app, i)),
                "every row is on screen before anything is folded"
            );
        });

        let alpha_key = GroupKey::Auto(alpha.clone());
        app.update(&mut vcx, |app, cx| {
            app.toggle_sidebar_group(Some(&alpha_key), cx)
        });
        vcx.run_until_parked();
        // The fold is a disclosure gesture: the rows fade and clip shut over
        // `DISCLOSE_CLOSE_MS`, so the settled assertion runs past it.
        vcx.executor()
            .advance_clock(std::time::Duration::from_millis(DISCLOSE_CLOSE_MS + 50));
        vcx.run_until_parked();
        vcx.update(|window, cx| window.render_frame(cx));

        app.update(&mut vcx, |app, _| {
            assert!(
                !drawn(app, 0) && !drawn(app, 1),
                "the folded group's rows left no rectangle behind"
            );
            assert!(drawn(app, 2), "the group next to it is untouched");
            assert_eq!(
                app.sidebar_groups.auto_collapsed,
                vec![alpha.clone()],
                "the fold is kept with the workspace, where the next launch reads it"
            );
        });

        app.update(&mut vcx, |app, cx| {
            app.toggle_sidebar_group(Some(&alpha_key), cx)
        });
        vcx.run_until_parked();
        vcx.executor()
            .advance_clock(std::time::Duration::from_millis(DISCLOSE_OPEN_MS + 50));
        vcx.run_until_parked();
        vcx.update(|window, cx| window.render_frame(cx));

        app.update(&mut vcx, |app, _| {
            assert!((0..3).all(|i| drawn(app, i)), "unfolding brings them back");
            assert!(
                app.sidebar_groups.auto_collapsed.is_empty(),
                "and takes the entry back out rather than piling up"
            );
        });
    }

    /// A fold hides every row the group has, the active one included. The
    /// alternative — leaving the active row on screen under a shut chevron,
    /// with the header counting rows that are not drawn — looks like a list
    /// that failed to load, which is what folding a group you are working in
    /// used to produce.
    #[gpui_kit::test]
    fn a_fold_hides_the_active_row_too(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 2);
        let alpha = GroupKey::Auto(AutoKey::Repo(PathBuf::from("/w/alpha")));

        app.update(&mut vcx, |app, cx| {
            for i in 0..2 {
                *app.tabs[i].auto_group.borrow_mut() = alpha.auto().cloned();
            }
            app.tabs.select(0);
            app.toggle_sidebar_group(Some(&alpha), cx);
        });
        vcx.run_until_parked();
        // Past the closing gesture: the fold is drawn, not just decided.
        vcx.executor()
            .advance_clock(std::time::Duration::from_millis(DISCLOSE_CLOSE_MS + 50));
        vcx.run_until_parked();
        vcx.update(|window, cx| window.render_frame(cx));

        app.update(&mut vcx, |app, _| {
            assert!(!drawn(app, 0), "the active row folds away with the rest");
            assert!(!drawn(app, 1), "and so does everything else in the group");
        });

        app.update(&mut vcx, |app, cx| {
            app.toggle_sidebar_group(Some(&alpha), cx)
        });
        vcx.run_until_parked();
        vcx.executor()
            .advance_clock(std::time::Duration::from_millis(DISCLOSE_OPEN_MS + 50));
        vcx.run_until_parked();
        vcx.update(|window, cx| window.render_frame(cx));

        app.update(&mut vcx, |app, _| {
            assert!((0..2).all(|i| drawn(app, i)), "unfolding brings both back");
        });
    }
    /// The divider stands between kept and derived groups, so it is there
    /// exactly when something is kept — and it is recorded, because a tab
    /// dropped below it leaves its pinned group.
    #[gpui_kit::test]
    fn the_divider_is_drawn_once_something_is_pinned(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 2);
        app.update(&mut vcx, |_, cx| cx.notify());
        vcx.run_until_parked();
        app.update(&mut vcx, |app, _| {
            assert!(app.sidebar_divider.get().is_none(), "nothing kept, no line");
        });

        app.update(&mut vcx, |app, cx| {
            label(app, "work", cx);
        });
        vcx.run_until_parked();
        app.update(&mut vcx, |app, _| {
            assert!(app.sidebar_divider.get().is_some(), "kept, so a line");
        });
    }

    /// An empty pinned group stays on screen, and takes drops like any other:
    /// its block is recorded where a dragged tab looks for somewhere to land.
    #[gpui_kit::test]
    fn an_empty_pinned_group_is_drawn_and_takes_drops(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);

        let work = app.update(&mut vcx, |app, cx| label(app, "work", cx));
        vcx.run_until_parked();

        app.update(&mut vcx, |app, _| {
            let landing = app.sidebar_group_slots.borrow();
            let block = landing
                .iter()
                .find(|(id, _)| *id == work)
                .expect("the empty group was drawn");
            assert!(block.1.size.height > px(0.));
        });
    }

    /// Pinned headers are ordered by hand; a drag lands as a new order for
    /// the list, not for the tabs.
    #[gpui_kit::test]
    fn a_pinned_drag_reorders_the_groups_not_the_tabs(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 2);

        app.update(&mut vcx, |app, cx| {
            let (a, b, c) = (
                label(app, "a", cx),
                label(app, "b", cx),
                label(app, "c", cx),
            );
            app.set_tab_group(0, Some(a), cx);
            app.set_tab_group(1, Some(c), cx);
            let tabs: Vec<_> = app.tabs.iter().map(|t| t.id()).collect();
            app.apply_pinned_order(&[2, 0, 1], cx);
            assert_eq!(
                app.pinned_group_names()
                    .into_iter()
                    .map(|(id, _)| id)
                    .collect::<Vec<_>>(),
                vec![c, a, b]
            );
            assert_eq!(
                app.tabs.iter().map(|t| t.id()).collect::<Vec<_>>(),
                tabs,
                "the tabs stayed where they were"
            );
            // An order that is not a permutation of the list changes nothing.
            app.apply_pinned_order(&[0, 1], cx);
            assert_eq!(app.sidebar_groups.pinned.len(), 3);
        });
    }

    /// "New Tab" on a pinned group opens a tab in it — joining outright,
    /// without waiting for a cwd to walk in, since a label group has nothing
    /// to walk into.
    #[gpui_kit::test]
    fn a_label_group_can_be_filled_from_its_header(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);

        let before = app.update(&mut vcx, |app, _| app.tabs.len());
        let work = app.update_in(&mut vcx, |app, window, cx| {
            let work = label(app, "work", cx);
            app.new_tab_in_group(GroupKey::Pinned(work), window, cx);
            work
        });
        vcx.run_until_parked();

        app.update(&mut vcx, |app, _| {
            // A harness without a daemon may refuse to spawn; when it does
            // open one, it has to be in the group.
            if app.tabs.len() > before {
                assert_eq!(app.tabs[app.tabs.active_index()].group.get(), Some(work));
            }
        });
    }

    /// Clearing a folder keeps the name the header was showing: a folder
    /// group nobody renamed reads its folder's name, and would read nothing
    /// once the folder is gone.
    #[gpui_kit::test]
    fn clearing_a_folder_keeps_the_name_it_was_showing(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);

        app.update(&mut vcx, |app, cx| {
            let lyrel = folder(app, "/w/lyrel", cx);
            app.set_group_folder(lyrel, None, cx);
            let group = app.sidebar_groups.get(lyrel).expect("still pinned");
            assert_eq!(group.folder, None);
            assert_eq!(group.given_name(), Some("lyrel"));
            app.set_group_folder(lyrel, Some(PathBuf::from("/w/else")), cx);
            let group = app.sidebar_groups.get(lyrel).expect("still pinned");
            assert_eq!(group.folder.as_deref(), Some("/w/else"));
            assert_eq!(group.given_name(), Some("lyrel"), "a set name stays");
        });
    }
    /// A folder dropped from Finder is pinned; a file is let fall, and a
    /// folder already pinned is not pinned twice.
    #[gpui_kit::test]
    fn a_dropped_folder_is_pinned_and_a_dropped_file_is_not(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("notes.txt");
        std::fs::write(&file, "x").expect("write");

        app.update(&mut vcx, |app, cx| {
            let paths = [dir.path().to_path_buf(), file.clone()];
            app.pin_dropped_folders(&paths, cx);
            app.pin_dropped_folders(&paths, cx);
            let folders: Vec<_> = app
                .sidebar_groups
                .pinned
                .iter()
                .map(|g| g.folder.clone())
                .collect();
            assert_eq!(
                folders,
                vec![Some(dir.path().to_string_lossy().into_owned())]
            );
        });
    }

    /// The palette's "New Group" makes an empty pinned group and opens its
    /// name for typing.
    #[gpui_kit::test]
    fn the_palette_makes_an_empty_group_ready_to_name(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);

        app.update_in(&mut vcx, |app, window, cx| {
            app.new_empty_group(window, cx);
            let id = app.sidebar_groups.pinned[0].id;
            assert_eq!(app.group_rename.as_ref().map(|r| r.group), Some(id));
            assert!(app.tabs.iter().all(|t| t.group.get().is_none()));
            let kinds: Vec<_> = app
                .search_input(window, cx)
                .build(cx)
                .actions
                .into_iter()
                .map(|c| c.kind)
                .collect();
            assert!(kinds.contains(&crate::ui::search::CommandKind::NewGroup));
            assert!(
                kinds.contains(&crate::ui::search::CommandKind::OpenFolderAsGroup),
                "a workspace on this computer can pick a folder"
            );
        });
    }

    /// Groups that arrive from another window, or from the machine at launch,
    /// are not folders this window's tabs walked into. A tab sitting in one —
    /// here, one that was dragged out of it — stays where it is.
    #[gpui_kit::test]
    fn groups_arriving_from_elsewhere_pull_no_tab_in(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);

        app.update(&mut vcx, |app, cx| {
            plant_repo(app, 0, "/w/probed/sub", "/w/probed", cx);
            cx.notify();
        });
        vcx.run_until_parked();
        app.update(&mut vcx, |app, cx| {
            let mut groups = WorkspaceGroups::default();
            groups
                .pinned
                .push(PinnedGroup::folder(Path::new("/w/probed")));
            app.adopt_sidebar_groups(groups, cx);
        });
        vcx.run_until_parked();

        app.update(&mut vcx, |app, _| {
            assert_eq!(app.tabs[0].group.get(), None);
        });
    }

    /// A tab restored from the tree is drawn in the auto group it last had
    /// before any probe has answered — the launch no longer parks every tab
    /// in Ungrouped — and the live answer, once it lands, wins and becomes
    /// the new hint. A hint never outranks a pinned group.
    #[gpui_kit::test]
    fn a_restored_tab_is_drawn_from_its_hint_until_a_probe_answers(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 2);

        app.update(&mut vcx, |app, cx| {
            for i in 0..2 {
                let pane = std::mem::replace(&mut app.tabs[i].pane, crate::ui::pane::Pane::Empty);
                let tree = lyrel_core::core::machine::Tab {
                    id: app.tabs[i].id(),
                    name: None,
                    group: None,
                    last_auto: Some(AutoKey::Repo(PathBuf::from("/w/alpha"))),
                    root: lyrel_core::core::machine::PaneNode::Leaf { pane: 1 },
                    hibernated: false,
                    presentation: Default::default(),
                };
                app.tabs[i] = crate::ui::tab::Tab::from_tree(&tree, pane);
            }
            let work = label(app, "work", cx);
            app.tabs[1].group.set(Some(work));
            let keys = app.sidebar_group_keys(cx);
            assert_eq!(
                keys[0],
                repo("/w/alpha"),
                "drawn from the hint, no probe yet"
            );
            assert_eq!(
                keys[1],
                Some(GroupKey::Pinned(work)),
                "a pinned group outranks any hint"
            );
            plant_repo(app, 0, "/w/beta/src", "/w/beta", cx);
            cx.notify();
        });
        vcx.run_until_parked();

        app.update(&mut vcx, |app, cx| {
            assert_eq!(
                app.sidebar_group_keys(cx)[0],
                repo("/w/beta"),
                "the probe wins"
            );
            let (desired, _, _) = crate::ui::tree_sync::projection::desired_tabs(
                &app.tabs,
                app.tabs.active_index(),
                app.workspace,
                cx,
            );
            assert_eq!(
                desired[0].last_auto,
                Some(AutoKey::Repo(PathBuf::from("/w/beta"))),
                "and is what goes up as the next launch's hint"
            );
        });
    }
}

#[cfg(test)]
mod navigation_tests {
    use super::*;
    use crate::daemon::protocol::ClientMsg;
    use crate::ui::app::test_window::harness_with_tabs;
    use gpui_kit::{Entity, Modifiers, TestAppContext, VisualTestContext};

    fn repo(path: &str) -> AutoKey {
        AutoKey::Repo(PathBuf::from(path))
    }

    fn assert_tab_owns_keyboard(app: &Entity<LyrelApp>, vcx: &mut VisualTestContext, index: usize) {
        let target = app.read_with(vcx, |app, _| SidebarNavTarget::Tab(app.tabs[index].id()));
        assert_navigation_owns_keyboard(app, vcx, target);
    }

    fn focus_pane(app: &Entity<LyrelApp>, vcx: &mut VisualTestContext, index: usize) {
        app.update_in(vcx, |app, window, cx| {
            let leaf = app.tabs[index].pane.first_leaf().expect("a test pane");
            window.focus(&leaf.focus_handle(cx), cx);
        });
    }

    fn assert_navigation_owns_keyboard(
        app: &Entity<LyrelApp>,
        vcx: &mut VisualTestContext,
        expected: SidebarNavTarget,
    ) {
        app.update_in(vcx, |app, window, _| {
            assert!(
                app.sidebar_nav_focus.is_focused(window),
                "the sidebar keeps the roving Tab stop"
            );
            assert_eq!(app.sidebar_nav_target.as_ref(), Some(&expected));
        });
    }

    #[gpui_kit::test]
    fn sidebar_navigation_roves_groups_activates_and_returns_to_content(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 3);
        vcx.update(|_, cx| cx.set_reduce_motion(true));
        let alpha = repo("/w/alpha");
        let beta = repo("/w/beta");
        app.update(&mut vcx, |app, cx| {
            *app.tabs[0].auto_group.borrow_mut() = Some(alpha.clone());
            *app.tabs[1].auto_group.borrow_mut() = Some(alpha.clone());
            *app.tabs[2].auto_group.borrow_mut() = Some(beta.clone());
            app.sidebar_groups.auto_collapsed.push(beta.clone());
            app.tabs.select(0);
            app.sidebar_collapsed = false;
            cx.notify();
        });
        vcx.run_until_parked();
        focus_pane(&app, &mut vcx, 0);

        // Enter through the real action/keymap path, not a private call.
        vcx.simulate_keystrokes("secondary-shift-l");
        vcx.run_until_parked();
        assert_tab_owns_keyboard(&app, &mut vcx, 0);
        app.update_in(&mut vcx, |app, _, _| {
            assert_eq!(
                app.tabs.active_index(),
                0,
                "focusing the sidebar must not switch sessions"
            );
        });

        vcx.simulate_keystrokes("down");
        assert_tab_owns_keyboard(&app, &mut vcx, 1);
        vcx.simulate_keystrokes("enter");
        vcx.run_until_parked();
        app.update_in(&mut vcx, |app, window, _| {
            assert_eq!(app.tabs.active_index(), 1);
            assert!(
                app.sidebar_nav_focus.is_focused(window),
                "activation keeps navigation focus"
            );
            assert_eq!(
                app.sidebar_nav_target,
                Some(SidebarNavTarget::Tab(app.tabs[1].id()))
            );
        });

        // Down from the last visible row of alpha lands on beta's disclosure
        // control, which is folded and therefore has no rows of its own.
        vcx.simulate_keystrokes("down");
        assert_navigation_owns_keyboard(
            &app,
            &mut vcx,
            SidebarNavTarget::Group(Some(GroupKey::Auto(beta.clone()))),
        );
        vcx.simulate_keystrokes("enter");
        vcx.run_until_parked();
        app.update_in(&mut vcx, |app, _, _| {
            assert!(
                !app.sidebar_groups.auto_collapsed.contains(&beta),
                "Enter on a group header uses the same disclosure control"
            );
        });
        vcx.simulate_keystrokes("down");
        assert_tab_owns_keyboard(&app, &mut vcx, 2);
        vcx.simulate_keystrokes("enter");
        vcx.run_until_parked();
        app.update_in(&mut vcx, |app, window, _| {
            assert_eq!(app.tabs.active_index(), 2);
            assert!(app.sidebar_nav_focus.is_focused(window));
        });

        // Home/End are endpoints over the same visible stops, and Escape
        // returns to the intended pane of whichever session was activated.
        vcx.simulate_keystrokes("home");
        assert_navigation_owns_keyboard(
            &app,
            &mut vcx,
            SidebarNavTarget::Group(Some(GroupKey::Auto(alpha.clone()))),
        );
        vcx.simulate_keystrokes("end");
        assert_tab_owns_keyboard(&app, &mut vcx, 2);
        vcx.simulate_keystrokes("escape");
        vcx.run_until_parked();
        app.update_in(&mut vcx, |app, window, cx| {
            let leaf = app.tabs[2].pane.first_leaf().expect("a test pane");
            assert!(
                leaf.contains_focused(window, cx),
                "Escape returns to the active session's pane"
            );
        });
    }

    #[gpui_kit::test]
    fn sidebar_navigation_skips_a_folded_active_row_and_keeps_it_reachable(
        cx: &mut TestAppContext,
    ) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 3);
        vcx.update(|_, cx| cx.set_reduce_motion(true));
        let alpha = repo("/w/alpha");
        let beta = repo("/w/beta");
        app.update(&mut vcx, |app, cx| {
            *app.tabs[0].auto_group.borrow_mut() = Some(alpha.clone());
            *app.tabs[1].auto_group.borrow_mut() = Some(alpha.clone());
            *app.tabs[2].auto_group.borrow_mut() = Some(beta.clone());
            // The selected tab is the one folded out of sight.
            app.sidebar_groups.auto_collapsed.push(alpha.clone());
            app.tabs.select(0);
            app.sidebar_collapsed = false;
            cx.notify();
        });
        vcx.run_until_parked();
        focus_pane(&app, &mut vcx, 0);
        vcx.simulate_keystrokes("secondary-shift-l");
        vcx.run_until_parked();

        assert_navigation_owns_keyboard(
            &app,
            &mut vcx,
            SidebarNavTarget::Group(Some(GroupKey::Auto(alpha.clone()))),
        );
        app.update_in(&mut vcx, |app, _, cx| {
            assert_eq!(app.tabs.active_index(), 0);
            assert!(
                app.sidebar_nav_targets(cx).iter().all(
                    |target| !matches!(target, SidebarNavTarget::Tab(id) if *id == app.tabs[0].id())
                ),
                "a folded row is never a keyboard stop"
            );
        });

        vcx.simulate_keystrokes("enter");
        vcx.run_until_parked();
        app.update_in(&mut vcx, |app, _, _| {
            assert!(!app.sidebar_groups.auto_collapsed.contains(&alpha));
        });
        vcx.simulate_keystrokes("down");
        assert_tab_owns_keyboard(&app, &mut vcx, 0);
        vcx.simulate_keystrokes("down");
        assert_tab_owns_keyboard(&app, &mut vcx, 1);
        vcx.simulate_keystrokes("enter");
        vcx.run_until_parked();
        app.update_in(&mut vcx, |app, window, _| {
            assert_eq!(app.tabs.active_index(), 1);
            assert!(app.sidebar_nav_focus.is_focused(window));
        });
    }

    #[gpui_kit::test]
    fn sidebar_navigation_keeps_domain_identity_through_reorder_and_close(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 3);
        vcx.update(|_, cx| cx.set_reduce_motion(true));
        let alpha = repo("/w/alpha");
        app.update(&mut vcx, |app, cx| {
            for index in 0..3 {
                *app.tabs[index].auto_group.borrow_mut() = Some(alpha.clone());
            }
            app.tabs.select(0);
            app.sidebar_collapsed = false;
            cx.notify();
        });
        vcx.run_until_parked();
        focus_pane(&app, &mut vcx, 0);
        vcx.simulate_keystrokes("secondary-shift-l");
        vcx.simulate_keystrokes("down");
        let reordered = app.update_in(&mut vcx, |app, _, _| {
            let id = app.tabs[1].id();
            app.tabs.move_to(id, 0);
            id
        });
        vcx.run_until_parked();
        app.update_in(&mut vcx, |app, _, _| {
            assert_eq!(
                app.sidebar_nav_target,
                Some(SidebarNavTarget::Tab(reordered)),
                "reorder moves the row, not the identity focus stands on"
            );
            assert_eq!(app.tabs.index_of(reordered), Some(0));
        });
        vcx.simulate_keystrokes("enter");
        vcx.run_until_parked();
        app.update_in(&mut vcx, |app, window, _| {
            assert_eq!(
                app.tabs.get(app.tabs.active_index()).map(Tab::id),
                Some(reordered)
            );
            assert!(app.sidebar_nav_focus.is_focused(window));
        });

        // Close the tab the navigation is standing on. The successor is the
        // next visible row by domain identity, and navigation survives.
        let survivor = app.update_in(&mut vcx, |app, window, cx| {
            let at = app.tabs.index_of(reordered).expect("the focused tab");
            let survivor = app
                .tabs
                .get((at + 1) % app.tabs.len())
                .map(Tab::id)
                .expect("another tab");
            app.close_tab(at, window, cx);
            survivor
        });
        vcx.run_until_parked();
        app.update_in(&mut vcx, |app, window, _| {
            assert!(!app.tabs.iter().any(|tab| tab.id() == reordered));
            assert!(
                app.sidebar_nav_focus.is_focused(window),
                "a keyboard close leaves the sidebar's Tab stop in place"
            );
            assert_eq!(
                app.sidebar_nav_target,
                Some(SidebarNavTarget::Tab(survivor)),
                "focus moves to a surviving domain identity, never a stale slot"
            );
            assert!(app.tabs.index_of(survivor).is_some());
        });
    }

    #[gpui_kit::test]
    fn sidebar_pointer_activation_keeps_content_focus_and_close_affordance_pointer_only(
        cx: &mut TestAppContext,
    ) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 2);
        vcx.update(|_, cx| cx.set_reduce_motion(true));
        app.update(&mut vcx, |app, cx| {
            app.tabs.select(0);
            app.sidebar_collapsed = false;
            cx.notify();
        });
        vcx.run_until_parked();
        focus_pane(&app, &mut vcx, 0);
        let row = app.update_in(&mut vcx, |app, _, _| app.sidebar_slots.borrow()[1]);
        assert!(row.size.height > px(0.), "the second row is drawn");
        vcx.simulate_click(row.center(), Modifiers::default());
        vcx.run_until_parked();
        app.update_in(&mut vcx, |app, window, cx| {
            assert_eq!(app.tabs.active_index(), 1);
            let leaf = app.tabs[1].pane.first_leaf().expect("a test pane");
            assert!(
                leaf.contains_focused(window, cx),
                "pointer activation keeps its established content-focus behavior"
            );
            assert!(!app.sidebar_nav_focus.is_focused(window));
        });
    }

    #[gpui_kit::test]
    fn terminal_tab_and_arrows_stay_with_the_pty_while_content_owns_focus(cx: &mut TestAppContext) {
        let (app, mut vcx, mut streams) = harness_with_tabs(cx, 2);
        vcx.update(|_, cx| cx.set_reduce_motion(true));
        app.update(&mut vcx, |app, cx| {
            app.tabs.select(0);
            app.sidebar_collapsed = false;
            cx.notify();
        });
        vcx.run_until_parked();
        focus_pane(&app, &mut vcx, 0);
        streams[0]
            .set_read_timeout(Some(std::time::Duration::from_secs(1)))
            .unwrap();
        vcx.simulate_keystrokes("tab");
        let mut input = Vec::new();
        while let Ok(message) = ClientMsg::read(&mut streams[0]) {
            if let ClientMsg::Input(bytes) = message {
                input.extend(bytes);
                break;
            }
        }
        assert_eq!(input, b"\t", "Tab reaches the shell, not the sidebar");
        app.update_in(&mut vcx, |app, window, _| {
            assert!(!app.sidebar_nav_focus.is_focused(window));
        });
    }

    #[gpui_kit::test]
    fn sidebar_follows_tty7_without_program_idle_row(cx: &mut TestAppContext) {
        use gpui_kit::test::TestWindowExt as _;
        use lyrel_core::core::program_status::{
            ProgramRecord, ProgramSnapshot, ProgramState, ProgramStatusFrame,
        };
        let (app, mut vcx, mut streams) = harness_with_tabs(cx, 1);
        vcx.update(|_, cx| cx.set_reduce_motion(true));
        app.update(&mut vcx, |app, cx| {
            app.sidebar_collapsed = false;
            cx.notify();
        });
        vcx.run_until_parked();

        let frame = ProgramStatusFrame::Baseline(ProgramSnapshot::new(
            "sess-1".into(),
            1,
            0,
            vec![ProgramRecord::new(
                String::new(),
                ProgramState::Idle,
                None,
                Some("omp".into()),
                None,
                None,
                None,
                1,
                1,
                0,
            )],
        ));
        crate::daemon::protocol::DaemonMsg::ProgramStatus(frame)
            .encode(&mut streams[0])
            .unwrap();
        let leaf = app.read_with(&vcx, |app, _| {
            app.tabs[0]
                .pane
                .first_leaf()
                .and_then(|slot| slot.terminal().cloned())
                .expect("test pane")
        });
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            vcx.run_until_parked();
            let ready = leaf.read_with(&vcx, |view, _| {
                view.with_program_status(|status| status.records().count() == 1)
            });
            if ready {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "timed out waiting for program status frame"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        vcx.run_until_parked();
        vcx.update(|window, cx| {
            window.render_frame(cx);
            assert!(
                window
                    .try_find(("sidebar-program-status", 0usize))
                    .is_none(),
                "left sidebar must not add a program status row like 'Program · Idle'"
            );
        });
        let row_height = app.read_with(&vcx, |app, _| app.sidebar_slots.borrow()[0].size.height);
        assert_eq!(
            row_height,
            px(ROW_HEIGHT),
            "sidebar row must remain single-line height without a git branch"
        );
    }
}

#[cfg(test)]
mod drag_tests {
    use super::*;
    use crate::ui::app::test_window::harness_with_tabs;
    use gpui_kit::test::{TestEventExt as _, TestWindowExt as _};
    use gpui_kit::{TestAppContext, point, size};

    /// Three plain rows in the open sidebar, the first tab active and its
    /// terminal holding the keyboard.
    fn three_rows(
        cx: &mut TestAppContext,
    ) -> (
        gpui_kit::Entity<LyrelApp>,
        gpui_kit::VisualTestContext,
        lyrel_core::core::machine::TabId,
        crate::ui::pane::PaneSlot,
        Vec<crate::daemon::transport::Stream>,
    ) {
        let (app, mut vcx, streams) = harness_with_tabs(cx, 3);
        vcx.update(|_, cx| cx.set_reduce_motion(true));
        let first = app.update(&mut vcx, |app, cx| {
            app.sidebar_collapsed = false;
            app.tabs.select(0);
            cx.notify();
            app.tabs[0].id()
        });
        vcx.run_until_parked();
        let pane = app.update_in(&mut vcx, |app, window, cx| {
            let pane = app.tabs[0].pane.first_leaf().expect("tab holds a pane");
            window.focus(&pane.focus_handle(cx), cx);
            pane
        });
        (app, vcx, first, pane, streams)
    }

    #[gpui_kit::test]
    fn a_dragged_row_follows_the_pointer_and_the_drop_settles_the_order(cx: &mut TestAppContext) {
        let (app, mut vcx, first, pane, _streams) = three_rows(cx);
        let (grab, row_h, top0, top1, title) = vcx.update(|window, cx| {
            window.render_frame(cx);
            let row = window.find(("tab-row", 0usize));
            let row0 = row.bounds();
            let row1 = window.find(("tab-row", 1usize)).bounds();
            (
                point(row0.left() + row0.size.width / 3., row0.center().y),
                row0.size.height,
                row0,
                row1,
                row.label().unwrap().to_owned(),
            )
        });

        let over = point(grab.x, top1.center().y);
        assert!(
            top0.bottom() <= top1.top(),
            "fixture rows must not overlap: {top0:?}, {top1:?}"
        );
        vcx.update(|window, cx| {
            window.pointer_move(grab, None, cx);
            window.pointer_down(grab, MouseButton::Left, cx);
            // A small sideways step crosses gpui's drag threshold without
            // disturbing the vertical grab offset the row was picked up with.
            window.pointer_move(grab + point(px(4.), px(0.)), Some(MouseButton::Left), cx);
            window.pointer_move(over, Some(MouseButton::Left), cx);
            assert!(
                cx.has_active_drag(),
                "a press and move over the visible session row must start a drag"
            );

            // The grabbed row tracks the pointer exactly — it is positioned,
            // never animated, so it cannot lag a frame behind.
            let dragged = window.find(("tab-row", 0usize)).bounds();
            assert!(
                (dragged.origin.y - (over.y - row_h / 2.)).abs() < px(1.5),
                "the grabbed row sits where the pointer holds it, not a slide behind"
            );
            // The neighbour it crossed has taken its old slot already: with
            // motion reduced the slide resolves on the frame it happens.
            let neighbour = window.find(("tab-row", 1usize)).bounds();
            assert!(
                (neighbour.origin.y - top0.origin.y).abs() < px(1.5),
                "the crossed neighbour settles into the row's old slot"
            );
        });

        vcx.update(|window, cx| {
            window.pointer_up(over, MouseButton::Left, cx);
            assert!(!cx.has_active_drag());
        });
        vcx.run_until_parked();
        app.update_in(&mut vcx, |app, window, cx| {
            assert!(app.reorder.borrow().is_none(), "mouse-up ended the drag");
            assert_eq!(
                app.tabs[app.tabs.active_index()].id(),
                first,
                "dropping the row keeps the original terminal selected"
            );
            let landed = app.tabs.iter().position(|tab| tab.id() == first);
            assert_eq!(
                landed,
                Some(1),
                "the dragged session moved below its neighbour"
            );
            assert!(
                pane.contains_focused(window, cx),
                "the keyboard never left the dragged terminal"
            );
        });
        vcx.update(|window, cx| {
            window.render_frame(cx);
            let landed = window.find(("tab-row", 1usize));
            assert_eq!(landed.label(), Some(title.as_str()));
            assert!(
                (landed.bounds().origin.y - top1.origin.y).abs() < px(1.5),
                "after the drop the dragged session occupies its new slot"
            );
        });
    }

    #[gpui_kit::test]
    fn escape_while_dragging_a_row_leaves_the_order_and_active_tab_untouched(
        cx: &mut TestAppContext,
    ) {
        let (app, mut vcx, first, pane, _streams) = three_rows(cx);
        let before: Vec<_> =
            app.read_with(&vcx, |app, _| app.tabs.iter().map(|tab| tab.id()).collect());
        let (grab, row_h, top1) = vcx.update(|window, cx| {
            window.render_frame(cx);
            let row0 = window.find(("tab-row", 0usize)).bounds();
            let row1 = window.find(("tab-row", 1usize)).bounds();
            (row0.center(), row0.size.height, row1)
        });

        vcx.update(|window, cx| {
            window.pointer_down(grab, MouseButton::Left, cx);
            window.pointer_move(grab + point(px(4.), px(0.)), Some(MouseButton::Left), cx);
            let over = point(grab.x, top1.center().y);
            window.pointer_move(over, Some(MouseButton::Left), cx);
            let dragged = window.find(("tab-row", 0usize)).bounds();
            assert!(
                (dragged.origin.y - (over.y - row_h / 2.)).abs() < px(1.5),
                "the row really is in the air before it is cancelled"
            );

            window.press("escape", cx);
            assert!(!cx.has_active_drag(), "escape ends the drag");
            assert!(
                app.read_with(cx, |app, _| app.reorder.borrow().is_none()),
                "and drops the pending order with it"
            );
            // A late release over the crossed row must not land.
            window.pointer_up(over, MouseButton::Left, cx);
            assert!(!cx.has_active_drag());
        });
        vcx.run_until_parked();
        app.update_in(&mut vcx, |app, window, cx| {
            let ids: Vec<_> = app.tabs.iter().map(|tab| tab.id()).collect();
            assert_eq!(ids, before, "cancellation leaves the prior order intact");
            assert_eq!(app.tabs.active_index(), 0);
            assert_eq!(app.tabs[0].id(), first);
            assert!(
                pane.contains_focused(window, cx),
                "the active terminal is the one that was active"
            );
            assert!(app.reorder.borrow().is_none());
        });
    }

    #[gpui_kit::test]
    fn a_tab_held_over_the_layout_shows_its_landing_and_escape_returns_it(cx: &mut TestAppContext) {
        let (app, mut vcx, _first, _pane, _streams) = three_rows(cx);
        // Two tabs are enough for the merge offer; drop the third.
        app.update(&mut vcx, |app, cx| {
            app.tabs.remove(2);
            cx.notify();
        });
        vcx.run_until_parked();
        let second = app.read_with(&vcx, |app, _| app.tabs[1].id());
        let grab = vcx.update(|window, cx| {
            window.render_frame(cx);
            window.find(("tab-row", 1usize)).bounds().center()
        });
        let area = app
            .read_with(&vcx, |app, _| app.pane_area.get())
            .expect("drawn");
        let over_layout = point(
            area.origin.x + area.size.width / 2.,
            area.origin.y + area.size.height / 2.,
        );
        // Held over the middle of a wide single pane, the arriving tab would
        // split it and take the right half.
        let landing = Bounds {
            origin: point(area.origin.x + area.size.width / 2., area.origin.y),
            size: size(area.size.width / 2., area.size.height),
        };

        vcx.update(|window, cx| {
            window.pointer_down(grab, MouseButton::Left, cx);
            window.pointer_move(grab + point(px(4.), px(0.)), Some(MouseButton::Left), cx);
            window.pointer_move(over_layout, Some(MouseButton::Left), cx);
            let preview = window.find("tab-drop-preview").bounds();
            assert!(
                (preview.origin.x - landing.origin.x).abs() < px(1.5)
                    && (preview.size.width - landing.size.width).abs() < px(1.5),
                "the preview is where the tab's panes would land"
            );

            window.press("escape", cx);
            assert!(!cx.has_active_drag());
            assert!(
                window.try_find("tab-drop-preview").is_none(),
                "no stale landing after the cancel"
            );
            window.pointer_up(over_layout, MouseButton::Left, cx);
        });
        vcx.run_until_parked();
        app.update_in(&mut vcx, |app, _, _| {
            assert_eq!(app.tabs.len(), 2, "the cancelled tab did not merge");
            assert!(
                app.tabs.iter().any(|tab| tab.id() == second),
                "both tabs are still here"
            );
            assert!(app.reorder.borrow().is_none());
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reveal_shift_moves_only_rows_out_of_view() {
        let view = (px(100.), px(400.));
        // On screen, or cut by an edge: a click there scrolls nothing.
        assert_eq!(reveal_shift((px(200.), px(230.)), px(0.), view), px(0.));
        assert_eq!(reveal_shift((px(90.), px(120.)), px(0.), view), px(0.));
        assert_eq!(reveal_shift((px(390.), px(420.)), px(0.), view), px(0.));
        // Above: its top lands on the top edge.
        assert_eq!(reveal_shift((px(10.), px(40.)), px(0.), view), px(90.));
        // Below: its bottom lands on the bottom edge.
        assert_eq!(reveal_shift((px(500.), px(530.)), px(0.), view), px(-130.));
    }

    #[test]
    fn a_first_row_revealed_from_above_brings_its_header() {
        let view = (px(100.), px(400.));
        // The header's top, not the row's, lands on the top edge.
        assert_eq!(reveal_shift((px(10.), px(40.)), px(23.), view), px(113.));
        // A header half under the top edge, row on screen: still nothing.
        assert_eq!(reveal_shift((px(110.), px(140.)), px(23.), view), px(0.));
        // Below: the header is already above the row, only the bottom counts.
        assert_eq!(reveal_shift((px(500.), px(530.)), px(23.), view), px(-130.));
    }

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    /// The auto group keyed on repo root `s`.
    fn g(s: &str) -> GroupKey {
        GroupKey::Auto(AutoKey::Repo(p(s)))
    }

    fn host(s: &str) -> GroupKey {
        GroupKey::Auto(AutoKey::SshHost(s.into()))
    }

    fn none() -> WorkspaceGroups {
        WorkspaceGroups::default()
    }

    /// Two machines' `/home/ubuntu` are two directories: each host gets a
    /// group of its own, named for the host.
    #[test]
    fn ssh_tabs_on_two_hosts_land_in_two_groups() {
        let keys = [
            Some(host("ubuntu@alpha")),
            Some(host("ubuntu@beta")),
            Some(host("ubuntu@alpha")),
        ];
        let sections = sidebar_sections(&keys, &none());
        let shape: Vec<(Option<String>, Vec<usize>)> =
            sections.into_iter().map(|s| (s.name, s.tabs)).collect();
        assert_eq!(
            shape,
            vec![
                (Some("ubuntu@alpha".into()), vec![0, 2]),
                (Some("ubuntu@beta".into()), vec![1]),
            ]
        );
    }

    #[test]
    fn a_shell_that_sshd_onward_groups_by_host_and_wsl_does_not() {
        use crate::daemon::protocol::{RemoteContext, RemoteKind};
        let ctx = |kind| RemoteContext {
            kind,
            argv: vec![],
            target: "u@h".into(),
        };
        assert_eq!(
            ssh_host(Some(&ctx(RemoteKind::Ssh))).as_deref(),
            Some("u@h")
        );
        assert_eq!(
            ssh_host(Some(&ctx(RemoteKind::NativeSsh))).as_deref(),
            Some("u@h")
        );
        assert_eq!(ssh_host(Some(&ctx(RemoteKind::Wsl))), None);
        assert_eq!(ssh_host(None), None);
    }

    #[test]
    fn sections_order_groups_by_first_appearance_ungrouped_last() {
        let keys = vec![
            Some(g("/w/beta")),
            None,
            Some(g("/w/alpha")),
            Some(g("/w/beta")),
        ];
        let sections = sidebar_sections(&keys, &none());
        let shape: Vec<(Option<GroupKey>, Option<String>, Vec<usize>)> = sections
            .into_iter()
            .map(|s| (s.key, s.name, s.tabs))
            .collect();
        assert_eq!(
            shape,
            vec![
                (Some(g("/w/beta")), Some("beta".into()), vec![0, 3]),
                (Some(g("/w/alpha")), Some("alpha".into()), vec![2]),
                (None, Some("Ungrouped".into()), vec![1]),
            ]
        );

        let flat = sidebar_sections(&[None, None], &none());
        assert_eq!(flat.len(), 1);
        assert_eq!(flat[0].name, None);
        assert_eq!(flat[0].tabs, vec![0, 1]);
    }

    /// Pinned groups come first, in the order the user gave them, whatever
    /// order their tabs are in — and an empty one keeps its place.
    #[test]
    fn pinned_groups_lead_in_their_own_order_empty_or_not() {
        let mut groups = none();
        let (a, b, c) = (
            PinnedGroup::label("a"),
            PinnedGroup::folder(Path::new("/w/b")),
            PinnedGroup::label("c"),
        );
        groups.pinned = vec![a.clone(), b.clone(), c.clone()];
        let keys = vec![
            Some(g("/w/r")),
            Some(GroupKey::Pinned(c.id)),
            Some(GroupKey::Pinned(a.id)),
            None,
        ];
        let sections = sidebar_sections(&keys, &groups);
        let shape: Vec<(Option<String>, Vec<usize>)> =
            sections.into_iter().map(|s| (s.name, s.tabs)).collect();
        assert_eq!(
            shape,
            vec![
                (Some("a".into()), vec![2]),
                (Some("b".into()), vec![]),
                (Some("c".into()), vec![1]),
                (Some("r".into()), vec![0]),
                (Some("Ungrouped".into()), vec![3]),
            ]
        );
    }

    /// "Move to Group" offers every group the sidebar draws — auto groups
    /// too, not just the pinned ones — in sidebar order.
    #[test]
    fn move_targets_list_every_sidebar_group_in_order() {
        let mut groups = none();
        let work = PinnedGroup::label("work");
        groups.pinned = vec![work.clone()];
        let keys = vec![
            Some(g("/w/r")),
            Some(GroupKey::Pinned(work.id)),
            Some(host("u@h")),
            None,
        ];
        let shape = |index| -> Vec<(GroupKey, String, bool)> {
            move_targets(&keys, &groups, index)
                .into_iter()
                .map(|m| (m.key, m.name, m.checked))
                .collect()
        };
        assert_eq!(
            shape(0),
            vec![
                (GroupKey::Pinned(work.id), "work".into(), false),
                (g("/w/r"), "r".into(), true),
                (host("u@h"), "u@h".into(), false),
            ]
        );
        let checked = |index| -> Vec<bool> { shape(index).into_iter().map(|m| m.2).collect() };
        assert_eq!(checked(1), vec![true, false, false]);
        assert_eq!(
            checked(3),
            vec![false, false, false],
            "an ungrouped tab is in none"
        );
        assert!(move_targets(&[None], &none(), 0).is_empty());
    }

    /// With pinned groups and nothing auto-grouped below them, the rest is
    /// just the list — a header reading "Ungrouped" over all of it would be a
    /// label on nothing.
    #[test]
    fn below_pinned_groups_the_rest_needs_no_header_on_its_own() {
        let mut groups = none();
        let work = PinnedGroup::label("work");
        groups.pinned = vec![work.clone()];
        let keys = vec![Some(GroupKey::Pinned(work.id)), None];
        let sections = sidebar_sections(&keys, &groups);
        assert_eq!(sections.len(), 2);
        assert_eq!(sections[1].name, None);
        assert_eq!(sections[1].tabs, vec![1]);
    }

    /// The badge on a row and the tab ⌘N opens are two readings of one order,
    /// taken in two places. Grouping makes them diverge from `self.tabs`
    /// order — tab 3 sits in the second row here — so if they are ever read
    /// off different things, the badge names a chord that opens another tab.
    #[test]
    fn a_row_badge_names_the_chord_that_opens_that_row() {
        let keys = vec![
            Some(g("/w/beta")),
            None,
            Some(g("/w/alpha")),
            Some(g("/w/beta")),
        ];
        // What `visual_tab_order` returns for a left tab bar.
        let order: Vec<usize> = sidebar_sections(&keys, &none())
            .into_iter()
            .flat_map(|s| s.tabs)
            .collect();
        assert_eq!(order, vec![0, 3, 2, 1]);

        let mut badge_pos = vec![0usize; keys.len()];
        for (n, i) in order.iter().copied().enumerate() {
            badge_pos[i] = n;
        }
        for (row, tab) in order.iter().copied().enumerate() {
            // ActivateTabN → activate_visual(N - 1) → order[N - 1].
            let chord = tab_badge_label(badge_pos[tab]);
            let opens = order[badge_pos[tab]];
            assert_eq!(
                opens, tab,
                "row {row} badges ⌘{chord}, which opens tab {opens}"
            );
        }
    }

    #[test]
    fn reordered_rows_moves_within_the_group_only() {
        let keys = vec![
            Some(g("/w/alpha")),
            Some(g("/w/beta")),
            Some(g("/w/alpha")),
            None,
        ];
        let alpha = Some(g("/w/alpha"));
        assert_eq!(
            reordered_rows(&keys, &none(), &alpha, &[0, 2], 0, 1),
            Some(vec![2, 0, 1, 3])
        );
        assert_eq!(
            reordered_rows(&keys, &none(), &alpha, &[0, 2], 1, 0),
            Some(vec![2, 0, 1, 3])
        );
        assert_eq!(reordered_rows(&keys, &none(), &alpha, &[0, 2], 1, 1), None);
    }

    #[test]
    fn reordered_rows_leaves_filtered_out_rows_alone() {
        let keys = vec![Some(g("/w/a")), Some(g("/w/a")), Some(g("/w/a"))];
        let a = Some(g("/w/a"));
        assert_eq!(
            reordered_rows(&keys, &none(), &a, &[0, 2], 0, 1),
            Some(vec![1, 2, 0])
        );
    }

    /// A pinned group is where a tab was put, and rows move within it the
    /// same way they move within a repo group.
    #[test]
    fn rows_reorder_inside_a_pinned_group_too() {
        let mut groups = none();
        let work = PinnedGroup::label("work");
        groups.pinned = vec![work.clone()];
        let w = Some(GroupKey::Pinned(work.id));
        let keys = vec![w.clone(), Some(g("/w/beta")), w.clone()];
        assert_eq!(
            reordered_rows(&keys, &groups, &w, &[0, 2], 0, 1),
            Some(vec![2, 0, 1])
        );
    }

    #[test]
    fn regrouped_order_moves_the_group_into_the_target_slot() {
        let keys = vec![
            Some(g("/w/alpha")),
            None,
            Some(g("/w/beta")),
            Some(g("/w/alpha")),
            Some(g("/w/gamma")),
        ];
        let r = |s: &str| AutoKey::Repo(p(s));
        assert_eq!(
            regrouped_order(&keys, &r("/w/gamma"), &r("/w/alpha")),
            Some(vec![4, 0, 3, 2, 1])
        );
        assert_eq!(
            regrouped_order(&keys, &r("/w/alpha"), &r("/w/gamma")),
            Some(vec![2, 4, 0, 3, 1])
        );
    }

    #[test]
    fn regrouped_order_ignores_self_and_unknown_roots() {
        let keys = vec![Some(g("/w/alpha")), Some(g("/w/beta"))];
        let r = |s: &str| AutoKey::Repo(p(s));
        assert_eq!(regrouped_order(&keys, &r("/w/alpha"), &r("/w/alpha")), None);
        assert_eq!(regrouped_order(&keys, &r("/w/gone"), &r("/w/beta")), None);
        assert_eq!(regrouped_order(&keys, &r("/w/alpha"), &r("/w/gone")), None);
    }

    #[test]
    fn reordered_pinned_moves_a_header_among_the_ones_shown() {
        let ids: Vec<GroupId> = (0..4).map(|_| GroupId::new()).collect();
        // All four shown: dragging the first onto the third.
        assert_eq!(reordered_pinned(&ids, &ids, 0, 2), Some(vec![1, 2, 0, 3]));
        assert_eq!(reordered_pinned(&ids, &ids, 3, 0), Some(vec![3, 0, 1, 2]));
        assert_eq!(reordered_pinned(&ids, &ids, 1, 1), None);
        // A search hides the second; it keeps its place.
        let shown = [ids[0], ids[2], ids[3]];
        assert_eq!(reordered_pinned(&ids, &shown, 2, 0), Some(vec![3, 0, 1, 2]));
    }

    /// A header with a long branch on it used to leave the heading as `DEL…`
    /// while the branch kept thirty characters. The name is the group; the
    /// branch is what it happens to be sitting on, and it may take at most
    /// half the line before the heading starts paying for it.
    #[test]
    fn a_long_branch_takes_half_the_header_and_no_more() {
        let avail = 200.;
        assert_eq!(header_name_avail(avail, Some(400.)), 100.);
        assert_eq!(header_name_avail(avail, Some(100.)), 100.);
    }

    #[test]
    fn a_short_branch_leaves_the_heading_the_rest_of_the_header() {
        assert_eq!(header_name_avail(200., Some(40.)), 160.);
        assert_eq!(header_name_avail(200., None), 200.);
    }

    /// Even a header narrow enough that the branch's half swallows the line
    /// keeps a readable stub of the name, rather than eliding it away.
    #[test]
    fn the_heading_keeps_a_floor_on_a_narrow_sidebar() {
        assert_eq!(header_name_avail(60., Some(400.)), HEADER_NAME_FLOOR);
    }
}

#[cfg(test)]
mod motion_model_tests {
    use super::*;
    use std::time::Duration;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    fn at(start: Instant, n: u64) -> Instant {
        start + ms(n)
    }

    /// A model that has seen `ids` once, at rest, laid out in container 1.
    fn settled_model(ids: &[TabId], start: Instant) -> SidebarRowMotion {
        let mut model = SidebarRowMotion::default();
        model.observe(ids, true, start);
        assert!(
            model.entries.is_empty(),
            "the first observation is a baseline"
        );
        model.begin_frame(start);
        let plan = model.slots(1, ids, start);
        assert!(
            plan.iter()
                .all(|slot| matches!(slot, RowSlot::Real(_, p) if *p == RowPresence::FULL)),
            "a settled list paints everything at full presence"
        );
        model.end_frame();
        model
    }

    #[test]
    fn a_replayed_membership_is_a_no_op() {
        let start = Instant::now();
        let mut model = settled_model(&[TabId::new(), TabId::new()], start);
        let ids: Vec<TabId> = model.known.clone().expect("the baseline is recorded");
        model.observe(&ids, true, at(start, 10));
        assert!(model.entries.is_empty());
    }

    #[test]
    fn reduce_motion_lands_the_membership_without_motion() {
        let start = Instant::now();
        let a = TabId::new();
        let mut model = settled_model(&[a], start);
        let b = TabId::new();
        model.observe(&[a, b], false, start);
        assert!(model.entries.is_empty(), "nothing is owed a frame");
        model.begin_frame(start);
        let plan = model.slots(1, &[a, b], start);
        assert!(
            plan.iter()
                .all(|slot| matches!(slot, RowSlot::Real(_, p) if *p == RowPresence::FULL))
        );
    }

    #[test]
    fn an_arrival_opens_its_gap_before_its_content_shows() {
        let start = Instant::now();
        let a = TabId::new();
        let mut model = settled_model(&[a], start);
        let b = TabId::new();
        model.observe(&[a, b], true, start);
        model.begin_frame(start);
        // At the very first frame the slot is a sliver and the content is
        // silent: the room is made before anything moves in.
        let plan = model.slots(1, &[a, b], start);
        match plan[1] {
            RowSlot::Real(_, presence) => {
                assert!(presence.height < 0.05, "the gap has barely opened");
                assert_eq!(presence.opacity, 0., "no content in a sliver");
            }
            slot => panic!("expected the arriving row, got {slot:?}"),
        }
        // Partway in the gap is mostly open while the content has not caught up.
        let mid = at(start, 60);
        model.begin_frame(mid);
        let plan = model.slots(1, &[a, b], mid);
        match plan[1] {
            RowSlot::Real(_, presence) => {
                assert!(presence.height > 0.7, "the room is mostly made");
                assert_eq!(presence.opacity, 0., "the content still waits");
            }
            slot => panic!("expected the arriving row, got {slot:?}"),
        }
        // Past the duration everything is full and nothing is owed.
        let settled = at(start, 250);
        model.begin_frame(settled);
        let plan = model.slots(1, &[a, b], settled);
        assert!(
            plan.iter()
                .all(|slot| matches!(slot, RowSlot::Real(_, p) if *p == RowPresence::FULL))
        );
        assert!(model.entries.is_empty());
    }

    #[test]
    fn a_departure_fades_before_it_reclaims_the_space() {
        let start = Instant::now();
        let (a, b, c) = (TabId::new(), TabId::new(), TabId::new());
        let mut model = settled_model(&[a, b, c], start);
        model.observe(&[a, c], true, start);
        model.begin_frame(start);
        // The leaving row is woven back in where it stood, holding its slot.
        let plan = model.slots(1, &[a, c], start);
        assert!(
            matches!(plan[0], RowSlot::Real(0, RowPresence::FULL)),
            "the row above is untouched"
        );
        assert!(
            matches!(plan[1], RowSlot::Ghost(id, RowPresence::FULL) if id == b),
            "the leaving row holds its place at full height"
        );
        assert!(
            matches!(plan[2], RowSlot::Real(1, RowPresence::FULL)),
            "the row below has not moved"
        );
        // Partway out the content is gone but the gap is only starting to close.
        let mid = at(start, 40);
        model.begin_frame(mid);
        let plan = model.slots(1, &[a, c], mid);
        match plan[1] {
            RowSlot::Ghost(id, presence) => {
                assert_eq!(id, b);
                assert!(presence.opacity < 0.1, "the content has faded");
                assert!(presence.height > 0.95, "the space is not reclaimed yet");
            }
            slot => panic!("expected the leaving row's ghost, got {slot:?}"),
        }
        // Past the duration the ghost is gone: the layout reclaims the slot.
        let settled = at(start, 220);
        model.begin_frame(settled);
        let plan = model.slots(1, &[a, c], settled);
        assert_eq!(plan.len(), 2);
        assert!(
            plan.iter()
                .all(|slot| matches!(slot, RowSlot::Real(_, p) if *p == RowPresence::FULL))
        );
    }

    #[test]
    fn a_reversal_turns_around_from_where_the_row_stands() {
        let start = Instant::now();
        let a = TabId::new();
        let mut model = settled_model(&[a], start);
        let b = TabId::new();
        model.observe(&[a, b], true, start);
        let mid = at(start, 120);
        model.begin_frame(mid);
        model.slots(1, &[a, b], mid);
        model.end_frame();
        let standing = model.presence(b, mid);
        assert!(
            standing.height > 0. && standing.height < 1.,
            "the arrival is partway when the reversal lands"
        );
        // Close it before the arrival settles: the exit starts exactly where
        // the arrival stood, and keeps moving from there.
        model.observe(&[a], true, mid);
        let turned = model.ghost(b, mid).expect("the exit runs");
        assert_eq!(turned, standing, "no jump to an old endpoint");
        let later = model.ghost(b, mid + ms(30)).expect("the exit continues");
        assert!(later.opacity < turned.opacity);
        assert!(later.height <= turned.height);
    }

    #[test]
    fn five_rows_in_motion_lands_everything_at_once() {
        let start = Instant::now();
        let a = TabId::new();
        let mut model = settled_model(&[a], start);
        let mut all = vec![a];
        all.extend((0..5).map(|_| TabId::new()));
        model.observe(&all, true, start);
        assert!(
            model.entries.is_empty(),
            "a bulk change plays no per-row motion"
        );
    }

    #[test]
    fn a_row_never_on_screen_has_nothing_to_collapse() {
        let start = Instant::now();
        let (a, b) = (TabId::new(), TabId::new());
        let mut model = SidebarRowMotion::default();
        model.observe(&[a, b], true, start);
        // b was never laid out — a fold hides a row without unmaking it —
        // so removing it now is not a departure the eye can see.
        model.observe(&[a], true, start);
        assert!(model.entries.is_empty());
    }

    #[test]
    fn leaving_rows_weave_back_where_they_last_stood() {
        let start = Instant::now();
        let (a, b, c) = (TabId::new(), TabId::new(), TabId::new());
        let mut model = settled_model(&[a, b, c], start);
        model.observe(&[b], true, start);
        model.begin_frame(start);
        let plan = model.slots(1, &[b], start);
        assert!(
            matches!(plan[0], RowSlot::Ghost(id, _) if id == a),
            "the first leaver stays at the head"
        );
        assert!(matches!(plan[1], RowSlot::Real(0, _)));
        assert!(
            matches!(plan[2], RowSlot::Ghost(id, _) if id == c),
            "the last leaver stays at the tail"
        );
    }

    #[test]
    fn opening_staggers_row_arrivals_inside_one_gesture() {
        let start = Instant::now();
        let mut motion = DisclosureMotion::new(false, 6, start);
        let first = motion.update(true, 6, start, false);
        assert!(first.animating);
        let mid = motion.update(true, 6, start + ms(100), false);
        assert!(mid.rows[0] > mid.rows[3], "earlier rows arrive sooner");
        assert!(mid.rows[3] > mid.rows[5], "later rows are still waiting");
        let done = motion.update(true, 6, start + ms(300), false);
        assert_eq!(done.reveal, 1.0);
        assert_eq!(done.rows, vec![1.0; 6]);
        assert!(!done.animating);
    }

    #[test]
    fn shutting_fades_every_row_together_and_sooner() {
        let start = Instant::now();
        let mut motion = DisclosureMotion::new(true, 4, start);
        assert!(!motion.update(true, 4, start, false).animating);
        assert!(motion.update(false, 4, start, false).animating);
        let mid = motion.update(false, 4, start + ms(90), false);
        assert!(
            mid.rows
                .iter()
                .all(|&o| (o - mid.rows[0]).abs() < f32::EPSILON),
            "the exit fades the rows as one"
        );
        assert!(mid.reveal > 0. && mid.reveal < 1.);
        let done = motion.update(false, 4, start + ms(180), false);
        assert_eq!(done.reveal, 0.0);
        assert_eq!(done.rows, vec![0.0; 4]);
        assert!(!done.animating);
    }

    #[test]
    fn reversing_a_disclosure_continues_from_the_sampled_frame() {
        let start = Instant::now();
        let mut motion = DisclosureMotion::new(false, 6, start);
        motion.update(true, 6, start, false);
        let halfway = start + ms(120);
        let before = motion.update(true, 6, halfway, false);
        let reversed = motion.update(false, 6, halfway, false);
        assert_eq!(before.reveal, reversed.reveal, "no jump on the reversal");
        assert_eq!(before.rows, reversed.rows);
        let closing = motion.update(false, 6, halfway + ms(50), false);
        assert!(closing.reveal < reversed.reveal);
    }

    #[test]
    fn reduced_motion_reaches_the_settled_frame_immediately() {
        let start = Instant::now();
        let mut motion = DisclosureMotion::new(true, 6, start);
        assert!(motion.update(false, 6, start, false).animating);
        let reduced = motion.update(true, 6, start, true);
        assert_eq!(reduced.reveal, 1.0);
        assert_eq!(reduced.rows, vec![1.0; 6]);
        assert!(!reduced.animating);
    }

    #[test]
    fn a_data_change_mid_gesture_does_not_replay_the_arrival() {
        let start = Instant::now();
        let mut motion = DisclosureMotion::new(false, 6, start);
        motion.update(true, 6, start, false);
        let changed = motion.update(true, 8, start + ms(100), false);
        assert!(
            !changed.animating,
            "rows coming and going land the gesture, not a new one"
        );
        assert_eq!(changed.rows, vec![1.0; 8]);
    }
}

#[cfg(test)]
mod motion_consumer_tests {
    use super::*;
    use crate::ui::app::test_window::harness_with_tabs;
    use gpui_kit::TestAppContext;
    use gpui_kit::point;
    use gpui_kit::test::TestWindowExt as _;
    use std::time::Duration;

    /// Advances the clock, lets the frame task and notifications run, and
    /// completes a frame — the same rhythm a running window would have.
    fn frame(vcx: &mut gpui_kit::VisualTestContext, cx: &mut TestAppContext, advance_ms: u64) {
        if advance_ms > 0 {
            vcx.executor()
                .advance_clock(Duration::from_millis(advance_ms));
        }
        cx.run_until_parked();
        vcx.update(|window, cx| window.render_frame(cx));
    }

    // Seeded tabs are installed after the test window has already drawn.
    // Start each gesture from those rows at rest, not their initial entrance.
    fn settle_seeded_rows(vcx: &mut gpui_kit::VisualTestContext, cx: &mut TestAppContext) {
        frame(vcx, cx, 0);
        frame(vcx, cx, ROW_ENTER_MS + 50);
    }

    /// Put tab `i` in a directory of the repo `root`, as `fold_tests` does.
    fn plant_repo(app: &LyrelApp, i: usize, cwd: &str, root: &str, cx: &mut gpui_kit::App) {
        use crate::ui::scm::status_cache::{GitStatusCache, RepoSnapshot};
        use lyrel_ui_base::host_ops::HostId;

        let cwd = PathBuf::from(cwd);
        let leaf = app.tabs[i].pane.first_leaf().expect("test tab has a pane");
        leaf.terminal()
            .expect("test pane is a terminal")
            .update(cx, |view, _| {
                view.set_git_status_cwd_for_test(Some(cwd.clone()))
            });
        cx.update_global::<GitStatusCache, _>(|cache, _| {
            cache.finish_probe(
                HostId::LOCAL,
                &cwd,
                Some(RepoSnapshot {
                    root: PathBuf::from(root),
                    home: PathBuf::from(root),
                    branch: "main".into(),
                    counts: Some((0, 0)),
                }),
            );
        });
    }

    /// Every quiet tab in one repo group.
    fn grouped_three(app: &LyrelApp, cx: &mut gpui_kit::App) -> GroupKey {
        for i in 0..3 {
            plant_repo(app, i, "/w/probed/sub", "/w/probed", cx);
        }
        app.sidebar_group_keys(cx)[0]
            .clone()
            .expect("the repo group")
    }

    #[gpui_kit::test]
    fn a_new_row_makes_room_before_its_content_arrives(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        settle_seeded_rows(&mut vcx, cx);
        let stream = app.update_in(&mut vcx, |app, window, cx| {
            use crate::ui::pane::{Pane, PaneSlot};
            use lyrel_terminal::view::quiet_test_pane;

            let (view, stream) = quiet_test_pane(2, window, cx);
            LyrelApp::watch_terminal(&view, window, cx);
            app.tabs.push(Tab::new(Pane::leaf(PaneSlot::Ready(view))));
            cx.notify();
            stream
        });
        let _stream = stream;
        // First frame after the tab appears: the slot is a sliver and the
        // row registers nothing — the room is made before the content shows.
        frame(&mut vcx, cx, 0);
        let opening = vcx.update(|window, _| window.find(("sidebar-row-slot", 1usize)).bounds());
        assert!(
            opening.size.height < px(ROW_HEIGHT / 2.),
            "the new row's slot opens before its content arrives"
        );
        // Partway: the slot is nearly full.
        frame(&mut vcx, cx, 100);
        let growing = vcx.update(|window, _| window.find(("sidebar-row-slot", 1usize)).bounds());
        assert!(growing.size.height > px(ROW_HEIGHT / 2.));
        assert!(growing.size.height < px(ROW_HEIGHT));
        assert_eq!(
            app.read_with(&vcx, |app, _| app.sidebar_slots.borrow()[1].size.height),
            px(0.)
        );
        // Settled: the row stands at its full height.
        frame(&mut vcx, cx, 150);
        let settled = app.read_with(&vcx, |app, _| app.sidebar_slots.borrow()[1]);
        assert_eq!(settled.size.height, px(ROW_HEIGHT));
    }

    #[gpui_kit::test]
    fn closing_a_row_keeps_its_ghost_before_the_space_is_reclaimed(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 3);
        settle_seeded_rows(&mut vcx, cx);
        let (closed, last_before) = app.read_with(&vcx, |app, _| {
            (app.tabs[0].id(), app.sidebar_slots.borrow()[2])
        });
        app.update_in(&mut vcx, |app, window, cx| {
            app.close_tab(0, window, cx);
        });
        // First frame: the ghost holds the row's slot at full height, and
        // the row beneath has not moved.
        frame(&mut vcx, cx, 0);
        vcx.update(|window, _| {
            let ghost = window.find(format!("sidebar-ghost-{closed}"));
            assert_eq!(
                ghost.bounds().size.height,
                px(ROW_HEIGHT),
                "the fading row keeps its place"
            );
        });
        let waiting = app.read_with(&vcx, |app, _| app.sidebar_slots.borrow()[1]);
        assert_eq!(
            waiting.origin, last_before.origin,
            "the neighbour waits out the fade where it was"
        );
        // Partway: the content is gone and the gap is closing.
        frame(&mut vcx, cx, 120);
        vcx.update(|window, _| {
            let ghost = window.find(format!("sidebar-ghost-{closed}"));
            assert!(
                ghost.bounds().size.height < px(ROW_HEIGHT / 2.),
                "the space is only now reclaimed"
            );
        });
        // Settled: no ghost, and the neighbour sits in the reclaimed space.
        frame(&mut vcx, cx, 120);
        vcx.update(|window, _| {
            assert!(
                window.try_find(format!("sidebar-ghost-{closed}")).is_none(),
                "the ghost is gone once the exit settles"
            );
        });
        let reclaimed = app.read_with(&vcx, |app, _| app.sidebar_slots.borrow()[0]);
        assert_eq!(reclaimed.size.height, px(ROW_HEIGHT));
        assert!(
            reclaimed.origin.y < last_before.origin.y,
            "the neighbour has slid up into the reclaimed gap"
        );
    }

    #[gpui_kit::test]
    fn a_leaving_row_takes_no_pointer_input(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 3);
        settle_seeded_rows(&mut vcx, cx);
        let closed = app.read_with(&vcx, |app, _| app.tabs[1].id());
        app.update_in(&mut vcx, |app, window, cx| {
            app.close_tab(1, window, cx);
        });
        // The content is already silent but the slot is still held: the
        // exact moment a click would matter most.
        frame(&mut vcx, cx, 30);
        vcx.update(|window, cx| {
            window.click_at(
                format!("sidebar-ghost-{closed}"),
                point(px(20.), px(15.)),
                cx,
            );
        });
        let (active, count) =
            app.read_with(&vcx, |app, _| (app.tabs.active_index(), app.tabs.len()));
        assert_eq!(
            active, 0,
            "the press died on the ghost; the row that slid under it never saw it"
        );
        assert_eq!(count, 2, "and nothing was closed through it");
    }

    #[gpui_kit::test]
    fn folding_a_group_shuts_it_with_a_clip_and_unfolding_reopens_it(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 3);
        let key = app.update(&mut vcx, |app, cx| grouped_three(app, cx));
        settle_seeded_rows(&mut vcx, cx);
        assert!(
            app.read_with(&vcx, |app, _| app
                .sidebar_slots
                .borrow()
                .iter()
                .all(|b| b.size.height == px(ROW_HEIGHT))),
            "all three rows are drawn before the fold"
        );
        let natural = 3. * ROW_HEIGHT + 2. * ROW_GAP;
        app.update(&mut vcx, |app, cx| {
            app.toggle_sidebar_group(Some(&key), cx);
        });
        // The gesture starts on the next render, from the fully open group;
        // a beat later the clip has taken over and rows stop being drop
        // targets while clipped.
        frame(&mut vcx, cx, 0);
        frame(&mut vcx, cx, 90);
        vcx.update(|window, _| {
            let clip = window.find(("sidebar-rows", 0usize));
            assert!(
                clip.bounds().size.height < px(natural) && clip.bounds().size.height > px(0.),
                "a shortening clip is shutting the group"
            );
        });
        assert!(
            app.read_with(&vcx, |app, _| app.sidebar_slots.borrow()[0].size.height) == px(0.),
            "a clipped row registers no rectangle — nothing can land on it"
        );
        // Settled shut: no rows, no clip, and only the header is a stop.
        frame(&mut vcx, cx, 120);
        vcx.update(|window, _| {
            assert!(window.try_find(("sidebar-rows", 0usize)).is_none());
        });
        assert_eq!(
            app.read_with(&vcx, |app, cx| app.sidebar_nav_targets(cx)),
            vec![SidebarNavTarget::Group(Some(key.clone()))],
            "the folded rows are no keyboard stops either"
        );
        // Unfold: the gesture starts from the fully shut group, and a beat
        // later a growing clip is revealing the rows again.
        app.update(&mut vcx, |app, cx| {
            app.toggle_sidebar_group(Some(&key), cx);
        });
        frame(&mut vcx, cx, 0);
        frame(&mut vcx, cx, 150);
        vcx.update(|window, _| {
            let clip = window.find(("sidebar-rows", 0usize));
            assert!(
                clip.bounds().size.height > px(0.) && clip.bounds().size.height < px(natural),
                "a growing clip is opening the group"
            );
        });
        frame(&mut vcx, cx, 200);
        vcx.update(|window, _| {
            assert!(
                window.try_find(("sidebar-rows", 0usize)).is_none(),
                "settled open draws the plain column again"
            );
        });
        assert!(
            app.read_with(&vcx, |app, _| app
                .sidebar_slots
                .borrow()
                .iter()
                .all(|b| b.size.height == px(ROW_HEIGHT))),
            "every row is back at its full height"
        );
    }

    #[gpui_kit::test]
    fn reduced_motion_lands_row_and_group_changes_at_once(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 3);
        vcx.update(|_, cx| {
            let mut cfg = cx.global::<Config>().clone();
            cfg.motion_preference = lyrel_core::core::config::MotionPreference::Reduce;
            cx.set_global(cfg);
            crate::ui::motion::apply(cx);
            assert!(cx.reduce_motion());
        });
        let key = app.update(&mut vcx, |app, cx| grouped_three(app, cx));
        frame(&mut vcx, cx, 0);
        // A close lands whole: no ghost, the space reclaimed in the same frame.
        let closed = app.read_with(&vcx, |app, _| app.tabs[0].id());
        app.update_in(&mut vcx, |app, window, cx| {
            app.close_tab(0, window, cx);
        });
        frame(&mut vcx, cx, 0);
        vcx.update(|window, _| {
            assert!(
                window.try_find(format!("sidebar-ghost-{closed}")).is_none(),
                "Reduce plays no exit"
            );
        });
        let reclaimed = app.read_with(&vcx, |app, _| app.sidebar_slots.borrow()[0]);
        assert_eq!(
            reclaimed.size.height,
            px(ROW_HEIGHT),
            "the neighbour sits in the reclaimed space immediately"
        );
        // A fold lands whole: no clip gesture, the rows simply gone.
        app.update(&mut vcx, |app, cx| {
            app.toggle_sidebar_group(Some(&key), cx);
        });
        frame(&mut vcx, cx, 0);
        vcx.update(|window, _| {
            assert!(
                window.try_find(("sidebar-rows", 0usize)).is_none(),
                "Reduce plays no disclosure gesture"
            );
        });
        assert_eq!(
            app.read_with(&vcx, |app, cx| app.sidebar_nav_targets(cx)),
            vec![SidebarNavTarget::Group(Some(key))],
        );
    }

    #[gpui_kit::test]
    fn five_rows_leaving_together_land_without_motion(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 6);
        frame(&mut vcx, cx, 0);
        app.update_in(&mut vcx, |app, window, cx| {
            for _ in 0..5 {
                app.close_tab(1, window, cx);
            }
        });
        frame(&mut vcx, cx, 0);
        let (row, count) = app.read_with(&vcx, |app, _| {
            (app.sidebar_slots.borrow()[0], app.tabs.len())
        });
        assert_eq!(count, 1, "five closes landed in one observation");
        assert_eq!(
            row.size.height,
            px(ROW_HEIGHT),
            "a bulk close reclaims the space in a single frame"
        );
    }
}
