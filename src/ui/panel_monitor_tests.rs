//! Activity Monitor regressions through the production workbench and the
//! real per-pane procs query path: the pane's Host is asked for the tree
//! exactly the way a managed remote's daemon is (#371), with no network.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{TestAppContext, VisualTestContext};

use crate::daemon::protocol::{
    DaemonMsg, PaneContext, PaneProcs, ProcEntry, RemoteContext, RemoteKind,
};
use crate::ui::app::test_window;
use lyrel_core::core::config::RightPanelTab;
use lyrel_ui_base::host_ops::{Entry, Host, HostId, Meta, Output, SearchHit, WatchSub};
use lyrel_ui_base::route::PaneWorkspace;
use lyrel_ui_base::session::{RemoteTarget, WorkspaceId};

/// Per-pane canned answers; `None` is the host-could-not-answer state.
#[derive(Default, Clone)]
struct Fixture {
    procs: Arc<Mutex<HashMap<u64, Option<PaneProcs>>>>,
}

/// A Host on the managed-remote path: the same `Host::pane_procs` seam the
/// real peer answers over, backed by canned trees instead of a network.
struct MonitorHost {
    id: HostId,
    inner: lyrel_ui_base::host_ops::SharedHost,
    fixture: Fixture,
}

impl Host for MonitorHost {
    fn id(&self) -> HostId {
        self.id
    }
    fn separator(&self) -> char {
        self.inner.separator()
    }
    fn is_absolute(&self, p: &Path) -> bool {
        self.inner.is_absolute(p)
    }
    fn read_dir(&self, dir: &Path, root: Option<&Path>) -> std::io::Result<Vec<Entry>> {
        self.inner.read_dir(dir, root)
    }
    fn stat(&self, p: &Path) -> std::io::Result<Meta> {
        self.inner.stat(p)
    }
    fn read_file(&self, p: &Path, max_bytes: u64) -> std::io::Result<Vec<u8>> {
        self.inner.read_file(p, max_bytes)
    }
    fn canonicalize(&self, p: &Path) -> std::io::Result<PathBuf> {
        self.inner.canonicalize(p)
    }
    fn search(
        &self,
        roots: &[PathBuf],
        query: &str,
        limit: usize,
        max_dirs: usize,
        show_hidden: bool,
    ) -> std::io::Result<Vec<SearchHit>> {
        self.inner
            .search(roots, query, limit, max_dirs, show_hidden)
    }
    fn search_content(
        &self,
        roots: &[PathBuf],
        query: &lyrel_core::host::ContentQuery,
        limits: &lyrel_core::host::ContentLimits,
    ) -> std::io::Result<lyrel_core::host::ContentResults> {
        self.inner.search_content(roots, query, limits)
    }
    fn write_file(&self, p: &Path, bytes: &[u8]) -> std::io::Result<Meta> {
        self.inner.write_file(p, bytes)
    }
    fn create_file_new(&self, p: &Path) -> std::io::Result<()> {
        self.inner.create_file_new(p)
    }
    fn create_dir(&self, p: &Path, recursive: bool) -> std::io::Result<()> {
        self.inner.create_dir(p, recursive)
    }
    fn rename(&self, from: &Path, to: &Path) -> std::io::Result<()> {
        self.inner.rename(from, to)
    }
    fn remove(&self, p: &Path, recursive: bool) -> std::io::Result<()> {
        self.inner.remove(p, recursive)
    }
    fn repo_root(&self, p: &Path) -> std::io::Result<Option<PathBuf>> {
        self.inner.repo_root(p)
    }
    fn git(&self, cwd: &Path, args: &[&str]) -> std::io::Result<Output> {
        self.inner.git(cwd, args)
    }
    fn shells(&self) -> std::io::Result<lyrel_core::host::ShellInventory> {
        self.inner.shells()
    }
    fn watch(&self, dirs: &[PathBuf]) -> std::io::Result<WatchSub> {
        self.inner.watch(dirs)
    }
    fn pane_procs(&self, pane_id: u64) -> Option<PaneProcs> {
        self.fixture.procs.lock().unwrap().get(&pane_id).cloned()?
    }
}

fn entry(pid: u32, name: &str, depth: u8, started: u64, rss: u64, cpu_ns: u64) -> ProcEntry {
    ProcEntry {
        pid,
        name: name.into(),
        depth,
        foreground: pid == 102,
        rss: Some(rss),
        cpu_ns: Some(cpu_ns),
        started: Some(started),
    }
}

/// Pane 1's tree: bash → (make → cc), node.
fn tree_one() -> PaneProcs {
    PaneProcs {
        procs: vec![
            entry(100, "bash", 0, 1000, 80_000_000, 1_000_000_000),
            entry(101, "make", 1, 1001, 40_000_000, 9_000_000_000),
            entry(102, "cc", 2, 1002, 30_000_000, 3_000_000_000),
            entry(103, "node", 1, 1003, 90_000_000, 5_000_000_000),
        ],
        context: Some(PaneContext {
            local_pty: true,
            ..Default::default()
        }),
        ..Default::default()
    }
}

/// Pane 2's tree: a different shell with one child — same directory as pane
/// 1, a different process tree.
fn tree_two() -> PaneProcs {
    PaneProcs {
        procs: vec![
            entry(200, "zsh", 0, 2000, 60_000_000, 2_000_000_000),
            entry(201, "python", 1, 2001, 120_000_000, 7_000_000_000),
        ],
        context: Some(PaneContext {
            local_pty: true,
            ..Default::default()
        }),
        ..Default::default()
    }
}

fn settle(vcx: &mut VisualTestContext) {
    vcx.run_until_parked();
    vcx.update(|window, cx| window.render_frame(cx));
    vcx.run_until_parked();
    // The poll lands after the frame that armed it; one more frame paints
    // the landed snapshot before assertions read the tree.
    vcx.update(|window, cx| window.render_frame(cx));
    vcx.run_until_parked();
}

/// Install the fixture host and point every harness pane at it as their
/// workspace, so `pane_forward_ctx` takes the `Host::pane_procs` path.
fn install_monitor_host(
    app: &gpui_kit::Entity<crate::ui::app::LyrelApp>,
    vcx: &mut VisualTestContext,
    fixture: &Fixture,
) {
    let target = RemoteTarget::Alias {
        alias: "monitor-fixture".into(),
    };
    let host = Arc::new(MonitorHost {
        id: target.host_id(),
        inner: lyrel_core::host::local::shared(),
        fixture: fixture.clone(),
    });
    vcx.update(|_, cx| {
        lyrel_ui_base::host_registry::HostRegistry::insert(cx, host);
    });
    app.update_in(vcx, |app, window, cx| {
        for i in 0..app.tabs.len() {
            let Some(leaf) = app.tabs[i].detail_pane(window, cx) else {
                continue;
            };
            leaf.update(cx, |view, _| {
                view.set_workspace(Some(PaneWorkspace {
                    workspace: WorkspaceId::new(),
                    target: target.clone(),
                    spec: None,
                    label: Some("monitor-fixture".into()),
                    resize_echo: false,
                }));
            });
        }
    });
}

fn open_monitor(app: &gpui_kit::Entity<crate::ui::app::LyrelApp>, vcx: &mut VisualTestContext) {
    app.update_in(vcx, |app, window, cx| {
        app.set_right_panel_tab(RightPanelTab::Monitor, window, cx);
    });
    settle(vcx);
}

fn set_filter(
    app: &gpui_kit::Entity<crate::ui::app::LyrelApp>,
    vcx: &mut VisualTestContext,
    value: &str,
) {
    app.update_in(vcx, |app, window, cx| {
        app.monitor.input.update(cx, |input, cx| {
            input.set_value(value, window, cx);
        });
    });
    settle(vcx);
}

fn repoll(vcx: &mut VisualTestContext) {
    // The on-screen poll re-arms on a two-second timer; land the next
    // snapshot the way a later round would.
    vcx.executor()
        .advance_clock(std::time::Duration::from_secs(2));
    settle(vcx);
}

fn row_id(pane: u64, pid: u32, started: u64) -> String {
    format!("monitor-proc-{pane}-{pid}-s{started}")
}

#[gpui_kit::test]
fn monitor_opens_folded_and_info_keeps_session_and_ports(cx: &mut TestAppContext) {
    let fixture = Fixture::default();
    fixture.procs.lock().unwrap().insert(1, Some(tree_one()));
    let (app, mut vcx, _streams) = test_window::harness_with_tabs(cx, 1);
    install_monitor_host(&app, &mut vcx, &fixture);
    app.update_in(&mut vcx, |app, _, _| app.right_panel_width.set(276.));
    open_monitor(&app, &mut vcx);

    let root_id = row_id(1, 100, 1000);
    let child_id = row_id(1, 101, 1001);
    vcx.update(|window, _| {
        // The tool is one of the host's tools.
        assert!(window.try_find("tool-tab-monitor").is_some());
        // Folded at the Shell root: the root is the whole first paint and
        // carries the descendant count of its three children.
        let root = window.find(root_id.clone());
        assert_eq!(root.expanded(), Some(false));
        let count_id = format!("{root_id}-count");
        assert_eq!(window.find(count_id.clone()).label(), Some("3"));
        let name = window.find(format!("{root_id}-name"));
        assert!(name.bounds().size.width >= gpui_kit::px(36.));
        assert!(window.try_find(child_id.clone()).is_none());
    });

    // Info still answers for the same pane: its rows render there and the
    // process tree does not.
    app.update_in(&mut vcx, |app, window, cx| {
        app.set_right_panel_tab(RightPanelTab::Info, window, cx);
    });
    settle(&mut vcx);
    vcx.update(|window, _| {
        assert!(window.try_find("panel-info-row-0").is_some());
        assert!(window.try_find(root_id.clone()).is_none());
    });
}

#[gpui_kit::test]
fn expanding_filtering_and_sorting_drive_the_tree(cx: &mut TestAppContext) {
    let fixture = Fixture::default();
    fixture.procs.lock().unwrap().insert(1, Some(tree_one()));
    let (app, mut vcx, _streams) = test_window::harness_with_tabs(cx, 1);
    install_monitor_host(&app, &mut vcx, &fixture);
    open_monitor(&app, &mut vcx);

    let root_id = row_id(1, 100, 1000);
    let make_id = row_id(1, 101, 1001);
    let cc_id = row_id(1, 102, 1002);
    let node_id = row_id(1, 103, 1003);

    // Expand the root: both children appear; the grandchild stays folded.
    let chevron_id = format!("{root_id}-chevron");
    vcx.update(|window, cx| window.click(chevron_id.clone(), cx));
    settle(&mut vcx);
    vcx.update(|window, _| {
        assert_eq!(window.find(root_id.clone()).expanded(), Some(true));
        assert!(window.try_find(make_id.clone()).is_some());
        assert!(window.try_find(node_id.clone()).is_some());
        assert!(window.try_find(cc_id.clone()).is_none());
    });

    // Filter by name: only the matching path shows, ancestors revealed.
    set_filter(&app, &mut vcx, "cc");
    vcx.update(|window, _| {
        assert!(window.try_find(cc_id.clone()).is_some());
        assert!(window.try_find(make_id.clone()).is_some());
        assert!(window.try_find(node_id.clone()).is_none());
    });

    // Clear: the expansion stored before the filter is back.
    set_filter(&app, &mut vcx, "");
    vcx.update(|window, _| {
        assert!(window.try_find(node_id.clone()).is_some());
        assert!(
            window.try_find(cc_id.clone()).is_none(),
            "make still folded"
        );
    });

    // Sort by CPU: the header marks it; the second sample's percentages are
    // what the rows order by.
    vcx.update(|window, cx| window.click("monitor-sort-cpu", cx));
    settle(&mut vcx);
    vcx.update(|window, _| {
        assert_eq!(window.find("monitor-sort-cpu").label(), Some("CPU ↓"));
    });
    repoll(&mut vcx);
    app.update_in(&mut vcx, |app, _window, _cx| {
        assert_eq!(app.monitor.sort, super::panel_monitor::MonitorSort::Cpu);
    });
}

#[gpui_kit::test]
fn selection_survives_refreshes_but_not_pid_reuse(cx: &mut TestAppContext) {
    let fixture = Fixture::default();
    fixture.procs.lock().unwrap().insert(1, Some(tree_one()));
    let (app, mut vcx, _streams) = test_window::harness_with_tabs(cx, 1);
    install_monitor_host(&app, &mut vcx, &fixture);
    open_monitor(&app, &mut vcx);

    // Expand the root so the child row exists to select.
    let root_id = row_id(1, 100, 1000);
    let chevron_id = format!("{root_id}-chevron");
    vcx.update(|window, cx| window.click(chevron_id, cx));
    settle(&mut vcx);

    let node_id = row_id(1, 103, 1003);
    // Select `node`, then land a fresh snapshot of the same tree: the
    // selection holds by process identity.
    vcx.update(|window, cx| window.click(node_id.clone(), cx));
    settle(&mut vcx);
    vcx.update(|window, _| {
        assert_eq!(window.find(node_id.clone()).selected(), Some(true));
    });
    repoll(&mut vcx);
    vcx.update(|window, _| {
        assert_eq!(window.find(node_id.clone()).selected(), Some(true));
    });

    // The pid is reused by another process (new birth stamp): the selection
    // must not follow it onto the newcomer.
    let mut reused = tree_one();
    for p in &mut reused.procs {
        if p.pid == 103 {
            p.started = Some(9999);
        }
    }
    fixture.procs.lock().unwrap().insert(1, Some(reused));
    repoll(&mut vcx);
    let newcomer_id = row_id(1, 103, 9999);
    vcx.update(|window, _| {
        assert_eq!(
            window.find(newcomer_id.clone()).selected(),
            Some(false),
            "a reused pid does not inherit the selection"
        );
    });
}

#[gpui_kit::test]
fn two_panes_sharing_a_directory_keep_separate_trees(cx: &mut TestAppContext) {
    let fixture = Fixture::default();
    fixture.procs.lock().unwrap().insert(1, Some(tree_one()));
    fixture.procs.lock().unwrap().insert(2, Some(tree_two()));
    let (app, mut vcx, mut streams) = test_window::harness_with_tabs(cx, 2);
    // Both panes report the same directory: the trees must still be per pane.
    let shared = PathBuf::from("/tmp/monitor-shared");
    for stream in streams.iter_mut() {
        DaemonMsg::Cwd(shared.clone()).encode(stream).unwrap();
    }
    install_monitor_host(&app, &mut vcx, &fixture);
    open_monitor(&app, &mut vcx);

    let one_root = row_id(1, 100, 1000);
    let two_root = row_id(2, 200, 2000);
    vcx.update(|window, _| {
        assert!(window.try_find(one_root.clone()).is_some());
        assert!(window.try_find(two_root.clone()).is_none());
    });

    vcx.dispatch_action(lyrel_ui_base::actions::SelectNextTab);
    settle(&mut vcx);
    vcx.update(|window, _| {
        assert!(window.try_find(two_root.clone()).is_some());
        assert!(window.try_find(one_root.clone()).is_none());
        // The host line names the fixture host for this pane too.
        assert!(window.find("monitor-host-line").label().is_some());
    });
}

#[gpui_kit::test]
fn sources_are_marked_heuristic_and_unavailable(cx: &mut TestAppContext) {
    let fixture = Fixture::default();
    fixture.procs.lock().unwrap().insert(1, Some(tree_one()));
    let (app, mut vcx, _streams) = test_window::harness_with_tabs(cx, 1);
    install_monitor_host(&app, &mut vcx, &fixture);
    open_monitor(&app, &mut vcx);

    // A local-pty walk: no marker.
    vcx.update(|window, _| assert!(window.try_find("monitor-source-note").is_none()));

    // An ssh-from-shell pane: the list is near-side only and says so.
    let mut near_side = tree_one();
    near_side.context = Some(PaneContext {
        local_pty: true,
        remote: Some(RemoteContext {
            kind: RemoteKind::Ssh,
            argv: vec!["ssh".into(), "dev".into()],
            target: "dev".into(),
        }),
        ..Default::default()
    });
    fixture.procs.lock().unwrap().insert(1, Some(near_side));
    repoll(&mut vcx);
    vcx.update(|window, _| {
        let note = window.find("monitor-source-note");
        assert_eq!(note.label(), Some(t_heuristic()));
    });

    // The host stops answering: the unavailable marker replaces the list.
    fixture.procs.lock().unwrap().insert(1, None);
    repoll(&mut vcx);
    let root_id = row_id(1, 100, 1000);
    vcx.update(|window, _| {
        let note = window.find("monitor-source-note");
        assert_eq!(note.label(), Some(t_unavailable()));
        assert!(window.try_find(root_id.clone()).is_none());
    });
}

fn t_heuristic() -> &'static str {
    lyrel_ui_base::i18n::t(lyrel_ui_base::i18n::L10nKey::MonitorProcsHeuristic)
}

fn t_unavailable() -> &'static str {
    lyrel_ui_base::i18n::t(lyrel_ui_base::i18n::L10nKey::MonitorProcsUnavailable)
}
