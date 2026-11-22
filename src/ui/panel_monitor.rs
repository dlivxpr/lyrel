//! Activity Monitor — the context pane's process tree (#371).
//!
//! The tree moved out of Info, which keeps its Session and Ports sections.
//! Both readers share the one `RightPanelState::procs` snapshot and its
//! poll; this module only owns what the monitor adds on top of it — the
//! filter field, sibling ordering, and the expanded/selected sets — all
//! keyed by process identity, never by list position.

use std::collections::HashSet;

use gpui_kit::TestSupportExt as _;
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, h_flex, v_flex};
use gpui_kit::{AnyElement, Context, MouseButton, Subscription, Window, div, prelude::*, px, rems};

use crate::daemon::protocol::{PaneContext, PaneProcs, ProcEntry};
use crate::ui::app::LyrelApp;
use crate::ui::proc_usage::{CPU_W, CpuTracker, MEM_W};
use crate::ui::right_panel::{
    HEADING, META, PROC_INDENT, PROC_ROW_H, ROW_FILL_RADIUS, ROW_INSET, SECTION_GAP, TEXT,
    TEXT_INSET,
};
use crate::ui::window_chrome::CONTENT_INSET;
use lyrel_ui_base::i18n::{L10nKey, t, t_fmt};
use lyrel_ui_base::scrollbar::with_vertical_scrollbar;
use lyrel_ui_base::session::WorkspaceStore;

/// The trustworthy identity of one process row across snapshots.
///
/// The pid alone can be reused by whichever process the kernel assigns it
/// next, so the daemon's birth stamp (`ProcEntry::started`) is part of the
/// key: selection and expansion follow the process, never a reused pid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct ProcKey {
    pid: u32,
    started: Option<u64>,
}

impl ProcKey {
    fn of(entry: &ProcEntry) -> Self {
        Self {
            pid: entry.pid,
            started: entry.started,
        }
    }
}

/// Sibling ordering for the tree. Sorting never crosses a parent boundary:
/// it re-orders one generation at a time, so parentage stays readable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum MonitorSort {
    /// The walk's own order — ascending pid within each generation.
    #[default]
    Tree,
    Cpu,
    Memory,
}

impl MonitorSort {
    /// What a click on `column`'s header selects: that column's order, or
    /// back to the tree when the tree already sorts by it.
    fn cycle(self, column: MonitorSort) -> MonitorSort {
        match self == column {
            true => MonitorSort::Tree,
            false => column,
        }
    }

    fn name(self) -> &'static str {
        match self {
            MonitorSort::Tree => "tree",
            MonitorSort::Cpu => "cpu",
            MonitorSort::Memory => "memory",
        }
    }
}

/// What the Activity Monitor adds to the shared process snapshot.
pub(crate) struct MonitorState {
    /// The name/PID filter field.
    pub(crate) input: gpui_kit::Entity<InputState>,
    pub(crate) sort: MonitorSort,
    /// The shared resource-trend window the CPU/RAM tiles chart.
    pub(crate) range: crate::ui::host_metrics::MetricsRange,
    /// Branches the user has opened. A node not in the set renders folded,
    /// with its descendant count — the Shell root included, so the first
    /// paint is one row.
    expanded: HashSet<ProcKey>,
    /// The row last selected, kept across refreshes as long as the pane
    /// stays the same.
    selected: Option<ProcKey>,
    /// The pane `expanded`/`selected` belong to. A pane switch resets both.
    pane: Option<u64>,
    _input_sub: Subscription,
}

impl MonitorState {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<LyrelApp>) -> Self {
        let input = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t(L10nKey::MonitorFilterPlaceholder))
        });
        let sub = cx.subscribe_in(&input, window, |_, _, ev, _, cx| {
            if matches!(ev, InputEvent::Change) {
                cx.notify();
            }
        });
        Self {
            input,
            sort: MonitorSort::Tree,
            range: crate::ui::host_metrics::MetricsRange::default(),
            expanded: HashSet::new(),
            selected: None,
            pane: None,
            _input_sub: sub,
        }
    }

    /// Point the per-pane state at `pane`, resetting expansion and selection
    /// when the pane changed. Called from the procs sync, the owner that
    /// already detects pane changes.
    pub(crate) fn pane_changed(&mut self, pane: u64) {
        if self.pane != Some(pane) {
            self.pane = Some(pane);
            self.expanded.clear();
            self.selected = None;
        }
    }

    /// Drop expansion and selection while staying on the same pane: the tree
    /// changed owner (a different host instance answers for the pane id), so
    /// branch state from the previous owner is not trustworthy.
    pub(crate) fn reset_branches(&mut self) {
        self.expanded.clear();
        self.selected = None;
    }

    /// A fresh snapshot landed; only keys that name a process, not just a
    /// pid, survive it — see [`prune_untrusted`].
    pub(crate) fn snapshot_landed(&mut self) {
        prune_untrusted(&mut self.selected, &mut self.expanded);
    }

    fn toggle(&mut self, key: ProcKey) {
        if !self.expanded.insert(key) {
            self.expanded.remove(&key);
        }
    }

    fn select(&mut self, key: ProcKey) {
        self.selected = Some(key);
    }

    fn filter(&self, cx: &gpui_kit::App) -> String {
        self.input.read(cx).value().to_string()
    }
}

/// One node of the pane's process tree, rebuilt from each snapshot. The
/// daemon sends a pre-order walk with depths, which is exactly what a
/// parent/child rebuild needs.
struct ProcNode {
    entry: ProcEntry,
    key: ProcKey,
    /// The node's position in the walk, so equal-sorting siblings keep their
    /// arrival order instead of flickering with the sort's instability.
    ord: usize,
    children: Vec<ProcNode>,
}

impl ProcNode {
    fn descendants(&self) -> usize {
        self.children
            .iter()
            .map(|child| 1 + child.descendants())
            .sum()
    }
}

fn build_tree(procs: &[ProcEntry]) -> Vec<ProcNode> {
    fn children_at(procs: &[ProcEntry], pos: &mut usize, depth: u8) -> Vec<ProcNode> {
        let mut out = Vec::new();
        while let Some(entry) = procs.get(*pos) {
            if entry.depth < depth {
                break;
            }
            let ord = *pos;
            *pos += 1;
            out.push(ProcNode {
                key: ProcKey::of(entry),
                children: children_at(procs, pos, entry.depth.saturating_add(1)),
                ord,
                entry: entry.clone(),
            });
        }
        out
    }
    let mut pos = 0;
    children_at(procs, &mut pos, 0)
}

/// Re-order siblings within each generation, parents first. `None` metrics
/// sort last — a number nobody reported is not a small number.
fn sort_tree(nodes: &mut [ProcNode], sort: MonitorSort, cpu: &CpuTracker) {
    if sort == MonitorSort::Tree {
        return;
    }
    for node in nodes.iter_mut() {
        sort_tree(&mut node.children, sort, cpu);
    }
    fn metric(node: &ProcNode, sort: MonitorSort, cpu: &CpuTracker) -> Option<f64> {
        match sort {
            MonitorSort::Tree => Some(node.ord as f64),
            MonitorSort::Cpu => cpu.percent(node.entry.pid),
            MonitorSort::Memory => node.entry.rss.map(|rss| rss as f64),
        }
    }
    fn cmp_desc(a: Option<f64>, b: Option<f64>) -> std::cmp::Ordering {
        use std::cmp::Ordering;
        match (a, b) {
            (Some(a), Some(b)) => b.partial_cmp(&a).unwrap_or(Ordering::Equal),
            (None, None) => Ordering::Equal,
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
        }
    }
    nodes.sort_by(|a, b| {
        cmp_desc(metric(a, sort, cpu), metric(b, sort, cpu)).then(a.ord.cmp(&b.ord))
    });
}

/// Whether a row matches the filter: its name contains the text, or the
/// text is all digits and a prefix of its pid.
fn matches_filter(entry: &ProcEntry, filter: &str) -> bool {
    let filter = filter.trim().to_lowercase();
    if filter.is_empty() {
        return true;
    }
    if entry.name.to_lowercase().contains(&filter) {
        return true;
    }
    filter.chars().all(|c| c.is_ascii_digit()) && entry.pid.to_string().starts_with(&filter)
}

/// A row the tree renders this frame: the node, and — when the row hides a
/// folded branch — how many descendants sit under it.
struct VisibleRow<'a> {
    node: &'a ProcNode,
    folded: Option<usize>,
}

/// The rows to draw. With a filter, every match and every match's ancestor
/// shows, fully expanded — the filter owns expansion while it is set, and
/// the stored branch state is untouched, so clearing restores it. Without
/// one, the stored `expanded` set decides, and a folded branch reports its
/// descendant count.
fn visible_rows<'a>(
    roots: &'a [ProcNode],
    expanded: &HashSet<ProcKey>,
    filter: &str,
) -> Vec<VisibleRow<'a>> {
    let mut out = Vec::new();
    if filter.trim().is_empty() {
        fn walk<'a>(
            nodes: &'a [ProcNode],
            expanded: &HashSet<ProcKey>,
            out: &mut Vec<VisibleRow<'a>>,
        ) {
            for node in nodes {
                if node.children.is_empty() {
                    out.push(VisibleRow { node, folded: None });
                } else if expanded.contains(&node.key) {
                    out.push(VisibleRow { node, folded: None });
                    walk(&node.children, expanded, out);
                } else {
                    out.push(VisibleRow {
                        node,
                        folded: Some(node.descendants()),
                    });
                }
            }
        }
        walk(roots, expanded, &mut out);
        return out;
    }
    fn walk_filtered<'a>(
        nodes: &'a [ProcNode],
        filter: &str,
        out: &mut Vec<VisibleRow<'a>>,
    ) -> bool {
        let mut any = false;
        for node in nodes {
            let mut sub = Vec::new();
            let child_hit = walk_filtered(&node.children, filter, &mut sub);
            if matches_filter(&node.entry, filter) || child_hit {
                out.push(VisibleRow { node, folded: None });
                out.extend(sub);
                any = true;
            }
        }
        any
    }
    walk_filtered(roots, filter, &mut out);
    out
}

/// How much the process answer can be trusted, shown as an explicit marker
/// rather than letting a partial view read as the whole truth.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProcSource {
    /// The answering daemon walked real processes on the pane's own host.
    Verified,
    /// Real processes, but not a verifiable picture of this pane's workload:
    /// either the near side of a connection the shell is running (`ssh` from
    /// a local shell), or the far-side sampling a native-SSH pane reports.
    Heuristic,
    /// The owning host could not be asked at all.
    Unavailable,
}

fn classify_source(unsupported: bool, procs: Option<&PaneProcs>) -> ProcSource {
    if unsupported {
        return ProcSource::Unavailable;
    }
    match procs.and_then(|p| p.context.as_ref()) {
        Some(PaneContext {
            local_pty: false, ..
        })
        | Some(PaneContext {
            remote: Some(_), ..
        }) => ProcSource::Heuristic,
        _ => ProcSource::Verified,
    }
}

/// A key without a birth stamp names a pid, not a process — selection or an
/// open branch kept on one could follow a reused pid onto whatever replaced
/// it, which is exactly what the read-only heuristic sources cannot vouch
/// against. Only keys that carry `started` survive a refresh.
fn prune_untrusted(selected: &mut Option<ProcKey>, expanded: &mut HashSet<ProcKey>) {
    expanded.retain(|key| key.started.is_some());
    if selected.is_some_and(|key| key.started.is_none()) {
        *selected = None;
    }
}

/// Row geometry shared with the fixed header, so the sort controls sit over
/// the cells they order.
const CHEVRON_SLOT: f32 = 10.;
const CHEVRON_GAP: f32 = 6.;
const CELL_GAP: f32 = 6.;
const PID_W: f32 = 48.;

impl LyrelApp {
    pub(crate) fn render_panel_monitor(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let title = self.panel_title(t(L10nKey::PanelMonitorTitle), None, None, window, cx);
        let Some(pane_id) = self.right_panel.procs_pane else {
            return self.panel_scroll(
                self.panel_empty(
                    t(L10nKey::MonitorNoPane),
                    Some(t(L10nKey::MonitorNoPaneHint)),
                    cx,
                ),
                title,
            );
        };

        let source = classify_source(
            self.right_panel.procs_unsupported,
            self.right_panel.procs.as_ref(),
        );
        let filter = self.monitor.filter(cx);
        let sort = self.monitor.sort;
        let mono = cx.theme().mono_font_family.clone();
        let muted = cx.theme().muted_foreground;

        // The host label names the machine the snapshot comes from — the
        // pane's owning host when there is one, this machine otherwise.
        let (host_label, host_remote) = match &self.right_panel.procs_host {
            Some(_) => {
                let label = self
                    .context_pane(window, cx)
                    .and_then(|leaf| {
                        let view = leaf.read(cx);
                        view.workspace().map(|w| {
                            WorkspaceStore::remote_ref(cx, w.workspace)
                                .map(|remote| crate::ui::desktop_lookups::route_label(cx, &remote))
                                .or_else(|| w.label.clone())
                                .unwrap_or_else(|| w.target.to_string())
                        })
                    })
                    .unwrap_or_else(|| t(L10nKey::MonitorScopeCurrentPane).to_string());
                (label, true)
            }
            None => (crate::ui::remote_connect::client_hostname(), false),
        };

        let host_line = h_flex()
            .id("monitor-host-line")
            .aria_label(gpui_kit::SharedString::from(host_label.clone()))
            .test_support()
            .px(px(TEXT_INSET))
            .pb(px(6.))
            .items_center()
            .gap(px(6.))
            .text_size(rems(META))
            .text_color(muted)
            .child(
                Icon::empty()
                    .path(match host_remote {
                        true => "icons/machine-remote.svg",
                        false => "icons/machine-local.svg",
                    })
                    .size(px(12.)),
            )
            .child(div().min_w_0().truncate().child(host_label))
            .child("·")
            .child(t(L10nKey::MonitorScopeCurrentPane))
            .into_any_element();

        let header = self.monitor_header(sort, muted, cx);

        // The scrolling part: the tree, or the explicit note about why there
        // is no tree. No snapshot yet (first poll still out) draws nothing —
        // the same silence Info's sections kept before their answer landed.
        let body = match source {
            ProcSource::Unavailable => self.panel_empty(
                t(L10nKey::MonitorProcsUnavailable),
                Some(t(L10nKey::MonitorProcsUnavailableHint)),
                cx,
            ),
            ProcSource::Verified | ProcSource::Heuristic => {
                match self.right_panel.procs.as_ref() {
                    None => v_flex().into_any_element(),
                    Some(procs) if procs.procs.is_empty() => self.panel_empty(
                        t(L10nKey::MonitorProcsEmpty),
                        Some(t(L10nKey::MonitorProcsEmptyHint)),
                        cx,
                    ),
                    Some(procs) => {
                        let mut tree = build_tree(&procs.procs);
                        sort_tree(&mut tree, sort, &self.right_panel.cpu);
                        let rows = visible_rows(&tree, &self.monitor.expanded, &filter);
                        let mut list = v_flex().px(px(CONTENT_INSET)).pb(px(SECTION_GAP));
                        if rows.is_empty() {
                            // A filter nothing matches: say so where the rows
                            // would be, instead of a blank well.
                            list = list.child(self.panel_empty(
                                t(L10nKey::MonitorProcsEmpty),
                                Some(t(L10nKey::MonitorProcsEmptyHint)),
                                cx,
                            ));
                        } else {
                            let name_width = self.right_panel_px(window, cx)
                                - 2. * TEXT_INSET
                                - PID_W
                                - CPU_W
                                - MEM_W
                                - CHEVRON_SLOT
                                - 5. * CELL_GAP;
                            let wide_label = window.rem_size().as_f32() * 10.;
                            for row in rows {
                                let compact_count = name_width
                                    - f32::from(row.node.entry.depth) * PROC_INDENT
                                    < wide_label;
                                list = list.child(self.monitor_row(
                                    pane_id,
                                    row,
                                    mono.clone(),
                                    compact_count,
                                    cx,
                                ));
                            }
                        }
                        list.into_any_element()
                    }
                }
            }
        };

        let body = div()
            .id("right-panel-monitor-body")
            .test_support()
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.right_panel.scroll)
            .child(body);
        v_flex()
            .flex_1()
            .min_h_0()
            .child(title)
            .child(host_line)
            .child(self.render_monitor_resources(window, cx))
            .child(self.panel_search_with(&self.monitor.input, None, cx))
            .child(header)
            .children(self.monitor_source_note(source, muted))
            .child(with_vertical_scrollbar(
                "right-panel-monitor-scrollbar",
                body,
                &self.right_panel.scroll,
            ))
            .into_any_element()
    }

    /// The marker over a partial or missing process answer, kept outside the
    /// scrolling list so it cannot scroll out of sight.
    fn monitor_source_note(&self, source: ProcSource, muted: gpui_kit::Hsla) -> Option<AnyElement> {
        let (text, hint) = match source {
            ProcSource::Verified => return None,
            ProcSource::Heuristic => (
                t(L10nKey::MonitorProcsHeuristic),
                t(L10nKey::MonitorProcsHeuristicHint),
            ),
            ProcSource::Unavailable => (
                t(L10nKey::MonitorProcsUnavailable),
                t(L10nKey::MonitorProcsUnavailableHint),
            ),
        };
        Some(
            v_flex()
                .id("monitor-source-note")
                .aria_label(gpui_kit::SharedString::from(text))
                .test_support()
                .px(px(TEXT_INSET))
                .pb(px(6.))
                .gap(px(3.))
                .text_size(rems(META))
                .text_color(muted)
                .child(text.to_string())
                .child(hint.to_string())
                .into_any_element(),
        )
    }

    /// The fixed table header. CPU and Memory are the sort controls and the
    /// active one carries a descending marker; clicking it again restores
    /// the tree order. Each control's column is the same width as the rows'
    /// cells, so the header and the values stay aligned.
    fn monitor_header(
        &self,
        sort: MonitorSort,
        muted: gpui_kit::Hsla,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let heading = |label: gpui_kit::SharedString| {
            div()
                .text_size(rems(HEADING))
                .font_weight(gpui_kit::FontWeight::MEDIUM)
                .text_color(muted)
                .child(label)
        };
        h_flex()
            .pl(px(TEXT_INSET + CHEVRON_SLOT + CHEVRON_GAP))
            .pr(px(TEXT_INSET))
            .pb(px(4.))
            .items_center()
            .gap(px(CELL_GAP))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(heading(t(L10nKey::MonitorColName).into())),
            )
            .child(
                div()
                    .w(px(PID_W))
                    .flex_none()
                    .flex()
                    .justify_end()
                    .child(heading(t(L10nKey::MonitorColPid).into())),
            )
            .child(self.monitor_sort_header(
                MonitorSort::Cpu,
                t(L10nKey::MonitorColCpu),
                sort,
                muted,
                cx,
            ))
            .child(self.monitor_sort_header(
                MonitorSort::Memory,
                t(L10nKey::MonitorColMemory),
                sort,
                muted,
                cx,
            ))
            .into_any_element()
    }

    fn monitor_sort_header(
        &self,
        column: MonitorSort,
        label: &'static str,
        sort: MonitorSort,
        muted: gpui_kit::Hsla,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let width = match column {
            MonitorSort::Cpu => CPU_W,
            _ => MEM_W,
        };
        let title = match sort == column {
            true => format!("{label} ↓"),
            false => label.to_string(),
        };
        let next = sort.cycle(column);
        h_flex()
            .id(format!("monitor-sort-{}", column.name()))
            .aria_label(gpui_kit::SharedString::from(title.clone()))
            .test_support()
            .w(px(width))
            .flex_none()
            .flex()
            .justify_end()
            .cursor_pointer()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |this, _, _, cx| {
                this.monitor.sort = next;
                cx.notify();
            }))
            .child(
                div()
                    .text_size(rems(HEADING))
                    .font_weight(gpui_kit::FontWeight::MEDIUM)
                    .text_color(muted)
                    .child(gpui_kit::SharedString::from(title)),
            )
            .into_any_element()
    }

    fn monitor_row(
        &self,
        pane_id: u64,
        row: VisibleRow,
        mono: gpui_kit::SharedString,
        compact_count: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let entry = &row.node.entry;
        let key = row.node.key;
        let selected = self.monitor.selected == Some(key);
        let has_children = !row.node.children.is_empty();
        let expanded = self.monitor.expanded.contains(&key);
        let sf = cx.global::<lyrel_ui_base::presets::Surfaces>().sidebar;
        let muted = cx.theme().muted_foreground;
        let id = gpui_kit::SharedString::from(format!(
            "monitor-proc-{pane_id}-{}-{}",
            key.pid,
            key.started
                .map(|s| format!("s{s}"))
                .unwrap_or_else(|| "x".into())
        ));
        let a11y = format!("{} {}", entry.name, entry.pid);

        let chevron = has_children.then(|| {
            Icon::new(match expanded {
                true => IconName::ChevronDown,
                false => IconName::ChevronRight,
            })
            .size(px(CHEVRON_SLOT))
            .text_color(muted)
        });

        h_flex()
            .id(id.clone())
            .test_support()
            .items_center()
            .gap(px(CHEVRON_GAP))
            .h(px(PROC_ROW_H))
            .pl(px(ROW_INSET + f32::from(entry.depth) * PROC_INDENT))
            .pr(px(ROW_INSET))
            .rounded(ROW_FILL_RADIUS)
            .cursor_pointer()
            .role(gpui_kit::Role::TreeItem)
            .aria_label(gpui_kit::SharedString::from(a11y))
            .aria_selected(selected)
            .when(has_children, |r| r.aria_expanded(expanded))
            .when(selected, |r| r.bg(gpui_kit::rgb(sf.selected)))
            .when(!selected, |r| r.hover(|s| s.bg(gpui_kit::rgb(sf.hover))))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.monitor.select(key);
                cx.notify();
            }))
            .child(
                div()
                    .id(format!("{id}-chevron"))
                    .test_support()
                    .flex_none()
                    .w(px(CHEVRON_SLOT))
                    .h_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .when(has_children, |c| c.cursor_pointer())
                    .children(chevron)
                    .when(has_children, |c| {
                        c.role(gpui_kit::Role::Button)
                            .aria_label(t(if expanded {
                                L10nKey::MonitorCollapse
                            } else {
                                L10nKey::MonitorExpand
                            }))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.monitor.toggle(key);
                                cx.notify();
                            }))
                    }),
            )
            .child(
                div()
                    .id(format!("{id}-name"))
                    .test_support()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(rems(TEXT))
                    .text_color(match selected {
                        true => gpui_kit::rgb(sf.text_selected),
                        false => cx.theme().foreground.into(),
                    })
                    .when(entry.foreground, |d| {
                        d.font_weight(gpui_kit::FontWeight::MEDIUM)
                    })
                    .child(entry.name.clone()),
            )
            .when_some(row.folded, |this, count| {
                let count = count.to_string();
                let label = t_fmt(L10nKey::MonitorChildCount, &[("count", &count)]);
                let value = if compact_count {
                    count.clone()
                } else {
                    label.clone()
                };
                this.child(
                    div()
                        .id(format!("{id}-count"))
                        .aria_label(gpui_kit::SharedString::from(count))
                        .test_support()
                        .flex_none()
                        .text_size(rems(META))
                        .text_color(muted)
                        .child(value)
                        .when(compact_count, |badge| {
                            badge.tooltip(move |window, cx| {
                                gpui_kit::component::tooltip::Tooltip::new(label.clone())
                                    .build(window, cx)
                            })
                        }),
                )
            })
            .child(
                div()
                    .id(format!("{id}-pid"))
                    .test_support()
                    .flex_none()
                    .w(px(PID_W))
                    .flex()
                    .justify_end()
                    .text_size(rems(META))
                    .font_family(mono.clone())
                    .text_color(muted)
                    .child(entry.pid.to_string()),
            )
            .child(crate::ui::proc_usage::row_cells(
                entry,
                &self.right_panel.cpu,
                mono,
                cx,
            ))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// A walk the way the daemon sends it: pre-order, depth-first, depths on
    /// the rows. `bash` is the shell root with two children, one of them a
    /// grandparent.
    fn walk() -> Vec<ProcEntry> {
        fn entry(
            pid: u32,
            name: &str,
            depth: u8,
            started: u64,
            rss: u64,
            cpu_ns: u64,
        ) -> ProcEntry {
            ProcEntry {
                pid,
                name: name.into(),
                depth,
                foreground: pid == 3,
                rss: Some(rss),
                cpu_ns: Some(cpu_ns),
                started: Some(started),
            }
        }
        vec![
            entry(10, "bash", 0, 100, 80_000_000, 1_000_000_000),
            entry(11, "make", 1, 101, 40_000_000, 9_000_000_000),
            entry(12, "cc", 2, 102, 30_000_000, 3_000_000_000),
            entry(13, "node", 1, 103, 90_000_000, 5_000_000_000),
        ]
    }

    fn keys(rows: &[VisibleRow]) -> Vec<u32> {
        rows.iter().map(|r| r.node.entry.pid).collect()
    }

    #[test]
    fn the_tree_rebuilds_parentage_from_the_walk() {
        let tree = build_tree(&walk());
        assert_eq!(tree.len(), 1, "one shell root");
        let root = &tree[0];
        assert_eq!(root.entry.pid, 10);
        assert_eq!(root.children.len(), 2);
        assert_eq!(root.children[0].entry.pid, 11);
        assert_eq!(root.children[0].children[0].entry.pid, 12, "grandchild");
        assert_eq!(root.children[1].entry.pid, 13);
    }

    #[test]
    fn the_root_starts_folded_with_its_descendant_count() {
        let tree = build_tree(&walk());
        let rows = visible_rows(&tree, &HashSet::new(), "");
        assert_eq!(
            rows.len(),
            1,
            "the folded shell root is the whole first paint"
        );
        assert_eq!(rows[0].node.entry.pid, 10);
        assert_eq!(rows[0].folded, Some(3), "three descendants hide under it");
    }

    #[test]
    fn expanding_reveals_children_and_refolding_hides_them_again() {
        let tree = build_tree(&walk());
        let root_key = ProcKey::of(&tree[0].entry);
        let mut expanded = HashSet::new();
        expanded.insert(root_key);
        let rows = visible_rows(&tree, &expanded, "");
        assert_eq!(keys(&rows), [10, 11, 13]);
        assert!(
            rows.iter()
                .all(|r| r.folded.is_none() || r.node.entry.pid == 11)
        );
        assert_eq!(rows[1].folded, Some(1), "make still folds cc");
        expanded.remove(&root_key);
        assert_eq!(
            visible_rows(&tree, &expanded, "").len(),
            1,
            "refold restores"
        );
    }

    #[test]
    fn a_filter_reveals_matches_and_their_ancestors() {
        let tree = build_tree(&walk());
        let rows = visible_rows(&tree, &HashSet::new(), "cc");
        assert_eq!(keys(&rows), [10, 11, 12], "ancestor path shows even folded");
        assert!(rows.iter().all(|r| r.folded.is_none()));

        let by_pid = visible_rows(&tree, &HashSet::new(), "13");
        assert_eq!(keys(&by_pid), [10, 13], "a numeric filter prefixes on pid");
    }

    #[test]
    fn clearing_a_filter_restores_the_branch_state() {
        let tree = build_tree(&walk());
        let root_key = ProcKey::of(&tree[0].entry);
        let mut expanded = HashSet::new();
        expanded.insert(root_key);
        // While the filter is set it owns expansion; the stored set is unread.
        assert_eq!(keys(&visible_rows(&tree, &expanded, "node")), [10, 13]);
        // Clearing gives the stored state back, untouched.
        assert_eq!(keys(&visible_rows(&tree, &expanded, "")), [10, 11, 13]);
    }

    /// Two samples one second apart with `deltas_ns` of CPU time gained per
    /// pid, so `CpuTracker` has real percentages to sort by.
    fn tracker(deltas_ns: &[(u32, u64)]) -> CpuTracker {
        let t0 = std::time::Instant::now();
        let mut first = walk();
        for entry in &mut first {
            entry.cpu_ns = Some(0);
        }
        let mut second = first.clone();
        for (pid, delta) in deltas_ns {
            second.iter_mut().find(|e| e.pid == *pid).unwrap().cpu_ns = Some(*delta);
        }
        let mut cpu = CpuTracker::default();
        cpu.sample(&first, t0);
        cpu.sample(&second, t0 + std::time::Duration::from_secs(1));
        assert!(
            cpu.percent(11).is_some(),
            "the fixture must yield percentages"
        );
        cpu
    }

    #[test]
    fn sorting_reorders_siblings_only() {
        // node gains more CPU than make, so the two children swap; cc, the
        // grandchild, stays under make no matter how its generation sorts.
        let cpu = tracker(&[
            (11, 3_000_000_000),
            (12, 9_000_000_000),
            (13, 5_000_000_000),
        ]);
        let mut sorted = build_tree(&walk());
        sort_tree(&mut sorted, MonitorSort::Cpu, &cpu);
        let children: Vec<u32> = sorted[0].children.iter().map(|n| n.entry.pid).collect();
        assert_eq!(children, [13, 11], "cpu desc: node 500% before make 300%");
        let make = sorted[0]
            .children
            .iter()
            .find(|n| n.entry.pid == 11)
            .unwrap();
        assert_eq!(make.children[0].entry.pid, 12, "parentage intact");

        let root_key = ProcKey::of(&walk()[0]);
        let mut expanded = HashSet::new();
        expanded.insert(root_key);
        let rows = visible_rows(&sorted, &expanded, "");
        assert_eq!(
            keys(&rows),
            [10, 13, 11],
            "both children visible, make folded over cc"
        );
        assert_eq!(rows[2].folded, Some(1));
    }

    #[test]
    fn sorting_by_memory_puts_unreported_rows_last() {
        let mut walk = walk();
        walk[1].rss = None;
        let mut tree = build_tree(&walk);
        sort_tree(&mut tree, MonitorSort::Memory, &CpuTracker::default());
        let children: Vec<u32> = tree[0].children.iter().map(|n| n.entry.pid).collect();
        assert_eq!(
            children,
            [13, 11],
            "node 90 MB before make with no reported rss"
        );
    }

    #[test]
    fn a_reused_pid_is_a_different_identity() {
        let first = ProcKey::of(&walk()[0]);
        let mut reused = walk();
        reused[0].started = Some(999);
        let second = ProcKey::of(&reused[0]);
        assert_ne!(first, second, "selection never follows a reused pid");
        assert_eq!(first.pid, second.pid);
    }

    #[test]
    fn sources_are_classified_explicitly() {
        assert_eq!(classify_source(true, None), ProcSource::Unavailable);
        assert_eq!(classify_source(false, None), ProcSource::Verified);

        let mut procs = PaneProcs::default();
        assert_eq!(classify_source(false, Some(&procs)), ProcSource::Verified);
        procs.context = Some(PaneContext {
            local_pty: false,
            ..Default::default()
        });
        assert_eq!(classify_source(false, Some(&procs)), ProcSource::Heuristic);
        procs.context = Some(PaneContext {
            local_pty: true,
            remote: Some(crate::daemon::protocol::RemoteContext {
                kind: crate::daemon::protocol::RemoteKind::Ssh,
                argv: vec!["ssh".into(), "dev".into()],
                target: "dev".into(),
            }),
            ..Default::default()
        });
        assert_eq!(classify_source(false, Some(&procs)), ProcSource::Heuristic);
    }

    #[test]
    fn a_filter_matching_nothing_empties_the_tree() {
        let tree = build_tree(&walk());
        assert!(visible_rows(&tree, &HashSet::new(), "zzz").is_empty());
    }

    #[test]
    fn keys_without_a_birth_stamp_do_not_survive_a_refresh() {
        let mut expanded = HashSet::from([
            ProcKey {
                pid: 1,
                started: None,
            },
            ProcKey {
                pid: 2,
                started: Some(9),
            },
        ]);
        let mut selected = Some(ProcKey {
            pid: 3,
            started: None,
        });
        prune_untrusted(&mut selected, &mut expanded);
        assert_eq!(
            expanded,
            HashSet::from([ProcKey {
                pid: 2,
                started: Some(9)
            }])
        );
        assert_eq!(selected, None);
    }
}
