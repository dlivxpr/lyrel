//! Activity Monitor regressions through the production workbench and the
//! real per-pane procs query path: the pane's Host is asked for the tree
//! exactly the way a managed remote's daemon is (#371), with no network.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{TestAppContext, VisualTestContext};

use crate::daemon::protocol::{
    DaemonMsg, EndProcessOutcome, EndProcessPlan, PaneContext, PaneProcs, ProcEntry, ProcIdentity,
    RemoteContext, RemoteKind,
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
    /// The end-process seam: what was asked, and what to answer.
    plans: Arc<Mutex<Vec<(u64, u32, Option<u64>)>>>,
    plan_answer: Arc<Mutex<Option<Option<Result<EndProcessPlan, String>>>>>,
    executed: Arc<Mutex<Vec<(u64, Vec<ProcIdentity>)>>>,
    outcomes: Arc<Mutex<Option<Vec<EndProcessOutcome>>>>,
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

    fn pane_end_process_plan(
        &self,
        pane_id: u64,
        pid: u32,
        started: Option<u64>,
    ) -> Option<Result<EndProcessPlan, String>> {
        self.fixture
            .plans
            .lock()
            .unwrap()
            .push((pane_id, pid, started));
        self.fixture.plan_answer.lock().unwrap().clone()?
    }

    fn pane_end_process(
        &self,
        pane_id: u64,
        scope: &[ProcIdentity],
    ) -> Option<Vec<EndProcessOutcome>> {
        self.fixture
            .executed
            .lock()
            .unwrap()
            .push((pane_id, scope.to_vec()));
        self.fixture.outcomes.lock().unwrap().clone()
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

fn end_target(pid: u32, name: &str, started: u64) -> crate::daemon::protocol::EndProcessTarget {
    crate::daemon::protocol::EndProcessTarget {
        pid,
        name: name.into(),
        started: Some(started),
    }
}

/// The verified scope for ending `make` (101) in tree_one: its child `cc`
/// (102) first, the target itself last — the order the daemon's complete
/// walk produces, and the order execute must carry.
fn make_plan() -> EndProcessPlan {
    EndProcessPlan {
        targets: vec![end_target(102, "cc", 1002), end_target(101, "make", 1001)],
        complete: true,
    }
}

fn request_end(
    app: &gpui_kit::Entity<crate::ui::app::LyrelApp>,
    vcx: &mut VisualTestContext,
    key: super::panel_monitor::ProcKey,
    tree: bool,
) {
    app.update_in(vcx, |app, window, cx| {
        app.monitor_request_end(key, tree, window, cx);
    });
    settle(vcx);
}

#[gpui_kit::test]
fn ending_a_process_confirms_the_verified_scope_then_executes_it(cx: &mut TestAppContext) {
    let fixture = Fixture::default();
    fixture.procs.lock().unwrap().insert(1, Some(tree_one()));
    *fixture.plan_answer.lock().unwrap() = Some(Some(Ok(make_plan())));
    *fixture.outcomes.lock().unwrap() = Some(vec![
        crate::daemon::protocol::EndProcessOutcome {
            pid: 102,
            name: "cc".into(),
            state: crate::daemon::protocol::EndProcessState::Ended,
        },
        crate::daemon::protocol::EndProcessOutcome {
            pid: 101,
            name: "make".into(),
            state: crate::daemon::protocol::EndProcessState::Ended,
        },
    ]);
    let (app, mut vcx, _streams) = test_window::harness_with_tabs(cx, 1);
    install_monitor_host(&app, &mut vcx, &fixture);
    open_monitor(&app, &mut vcx);

    // Expand the root, select `make`, and ask to end the tree.
    let root_id = row_id(1, 100, 1000);
    vcx.update(|window, cx| window.click(format!("{root_id}-chevron"), cx));
    settle(&mut vcx);
    let make_id = row_id(1, 101, 1001);
    vcx.update(|window, cx| window.click(make_id.clone(), cx));
    settle(&mut vcx);

    request_end(
        &app,
        &mut vcx,
        super::panel_monitor::ProcKey {
            pid: 101,
            started: Some(1001),
        },
        true,
    );

    // The plan went to the owning host with the row's exact identity.
    assert_eq!(
        fixture.plans.lock().unwrap().as_slice(),
        [(1, 101, Some(1001))],
        "the plan request names the pane, the pid, and the birth stamp"
    );
    // The confirmation names the target and the scope the daemon verified.
    let (msg, detail) = vcx
        .pending_prompt()
        .expect("the confirm dialog must be up before anything is signaled");
    assert!(msg.contains("make"), "the title names the target: {msg}");
    assert!(
        detail.contains("cc (102)") && detail.contains("make (101)"),
        "the body lists the scope the daemon verified: {detail}"
    );
    vcx.simulate_prompt_answer("End Process");
    settle(&mut vcx);

    let executed = fixture.executed.lock().unwrap();
    assert_eq!(executed.len(), 1, "exactly one execute request");
    let (pane, scope) = &executed[0];
    assert_eq!(*pane, 1);
    assert_eq!(
        scope.as_slice(),
        [
            ProcIdentity {
                pid: 102,
                started: Some(1002)
            },
            ProcIdentity {
                pid: 101,
                started: Some(1001)
            },
        ],
        "the tree action carries the whole confirmed scope, deepest first"
    );
}

#[gpui_kit::test]
fn ending_a_single_process_executes_only_the_target(cx: &mut TestAppContext) {
    let fixture = Fixture::default();
    fixture.procs.lock().unwrap().insert(1, Some(tree_one()));
    // The plan for `make` names its child `cc` first — the deepest-first
    // order a live walk produces. The single action must still confirm and
    // execute only `make`'s own identity: a scope built from the plan's
    // front row would name the child.
    *fixture.plan_answer.lock().unwrap() = Some(Some(Ok(make_plan())));
    let (app, mut vcx, _streams) = test_window::harness_with_tabs(cx, 1);
    install_monitor_host(&app, &mut vcx, &fixture);
    open_monitor(&app, &mut vcx);

    let root_id = row_id(1, 100, 1000);
    vcx.update(|window, cx| window.click(format!("{root_id}-chevron"), cx));
    settle(&mut vcx);
    let make_id = row_id(1, 101, 1001);
    vcx.update(|window, cx| window.click(make_id.clone(), cx));
    settle(&mut vcx);

    request_end(
        &app,
        &mut vcx,
        super::panel_monitor::ProcKey {
            pid: 101,
            started: Some(1001),
        },
        false,
    );
    assert!(vcx.has_pending_prompt(), "the single action confirms too");
    vcx.simulate_prompt_answer("End Process");
    settle(&mut vcx);

    let executed = fixture.executed.lock().unwrap();
    assert_eq!(executed.len(), 1);
    assert_eq!(
        executed[0].1.as_slice(),
        [ProcIdentity {
            pid: 101,
            started: Some(1001)
        }],
        "the single action carries the target's own confirmed identity"
    );
    assert!(
        executed[0].1.iter().all(|id| id.pid != 102),
        "a child named by the deepest-first plan must never be executed by the single action"
    );
}

#[gpui_kit::test]
fn a_refused_or_incomplete_plan_executes_nothing(cx: &mut TestAppContext) {
    // Refused: the daemon answered, and its answer was "this row is stale".
    let fixture = Fixture::default();
    fixture.procs.lock().unwrap().insert(1, Some(tree_one()));
    *fixture.plan_answer.lock().unwrap() = Some(Some(Err(
        "process 101 no longer matches the confirmed identity".into(),
    )));
    let (app, mut vcx, _streams) = test_window::harness_with_tabs(cx, 1);
    install_monitor_host(&app, &mut vcx, &fixture);
    open_monitor(&app, &mut vcx);
    request_end(
        &app,
        &mut vcx,
        super::panel_monitor::ProcKey {
            pid: 101,
            started: Some(1001),
        },
        false,
    );
    assert_eq!(fixture.executed.lock().unwrap().len(), 0);
    drop(app);

    // Incomplete, tree: the walk never finished, so the tree scope cannot be
    // confirmed — refused, nothing executes.
    let fixture = Fixture::default();
    fixture.procs.lock().unwrap().insert(1, Some(tree_one()));
    *fixture.plan_answer.lock().unwrap() = Some(Some(Ok(EndProcessPlan {
        complete: false,
        ..make_plan()
    })));
    let (app, mut vcx, _streams) = test_window::harness_with_tabs(cx, 1);
    install_monitor_host(&app, &mut vcx, &fixture);
    open_monitor(&app, &mut vcx);
    request_end(
        &app,
        &mut vcx,
        super::panel_monitor::ProcKey {
            pid: 101,
            started: Some(1001),
        },
        true,
    );
    assert_eq!(fixture.executed.lock().unwrap().len(), 0);
    drop(app);

    // Incomplete, single: the walk never finished, but the single action
    // covers only the target's own identity, which the daemon did stamp —
    // it stays available and executes exactly that identity.
    let fixture = Fixture::default();
    fixture.procs.lock().unwrap().insert(1, Some(tree_one()));
    *fixture.plan_answer.lock().unwrap() = Some(Some(Ok(EndProcessPlan {
        complete: false,
        ..make_plan()
    })));
    let (app, mut vcx, _streams) = test_window::harness_with_tabs(cx, 1);
    install_monitor_host(&app, &mut vcx, &fixture);
    open_monitor(&app, &mut vcx);
    request_end(
        &app,
        &mut vcx,
        super::panel_monitor::ProcKey {
            pid: 101,
            started: Some(1001),
        },
        false,
    );
    assert!(vcx.has_pending_prompt(), "the single action still confirms");
    vcx.simulate_prompt_answer("End Process");
    settle(&mut vcx);
    assert_eq!(
        fixture.executed.lock().unwrap().as_slice(),
        [(
            1,
            vec![ProcIdentity {
                pid: 101,
                started: Some(1001)
            }]
        )],
        "the single action stays available on an incomplete subtree"
    );
    drop(app);

    // A child row the daemon could not stamp: the tree action still cannot
    // name its whole extent, while the single action on the stamped target
    // remains available.
    let fixture = Fixture::default();
    fixture.procs.lock().unwrap().insert(1, Some(tree_one()));
    *fixture.plan_answer.lock().unwrap() = Some(Some(Ok(EndProcessPlan {
        targets: vec![
            crate::daemon::protocol::EndProcessTarget {
                pid: 102,
                name: "cc".into(),
                started: None,
            },
            end_target(101, "make", 1001),
        ],
        ..make_plan()
    })));
    let (app, mut vcx, _streams) = test_window::harness_with_tabs(cx, 1);
    install_monitor_host(&app, &mut vcx, &fixture);
    open_monitor(&app, &mut vcx);
    request_end(
        &app,
        &mut vcx,
        super::panel_monitor::ProcKey {
            pid: 101,
            started: Some(1001),
        },
        true,
    );
    assert_eq!(fixture.executed.lock().unwrap().len(), 0);
    request_end(
        &app,
        &mut vcx,
        super::panel_monitor::ProcKey {
            pid: 101,
            started: Some(1001),
        },
        false,
    );
    assert!(vcx.has_pending_prompt(), "the single action still confirms");
    vcx.simulate_prompt_answer("End Process");
    settle(&mut vcx);
    assert_eq!(
        fixture.executed.lock().unwrap().as_slice(),
        [(
            1,
            vec![ProcIdentity {
                pid: 101,
                started: Some(1001)
            }]
        )],
        "an unstamped child never blocks the single action on its stamped target"
    );
    drop(app);

    // Unsupported: the host seam answers `None` — a peer too old for the
    // request. The UI asked; the answer produced no execute and no crash.
    let fixture = Fixture::default();
    fixture.procs.lock().unwrap().insert(1, Some(tree_one()));
    let (app, mut vcx, _streams) = test_window::harness_with_tabs(cx, 1);
    install_monitor_host(&app, &mut vcx, &fixture);
    open_monitor(&app, &mut vcx);
    request_end(
        &app,
        &mut vcx,
        super::panel_monitor::ProcKey {
            pid: 101,
            started: Some(1001),
        },
        false,
    );
    assert_eq!(fixture.executed.lock().unwrap().len(), 0);
}

#[gpui_kit::test]
fn read_only_sources_send_no_end_process_request(cx: &mut TestAppContext) {
    // The pane's shell is running ssh: real rows, no vouch, no action.
    let fixture = Fixture::default();
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
    *fixture.plan_answer.lock().unwrap() = Some(Some(Ok(make_plan())));
    let (app, mut vcx, _streams) = test_window::harness_with_tabs(cx, 1);
    install_monitor_host(&app, &mut vcx, &fixture);
    open_monitor(&app, &mut vcx);
    request_end(
        &app,
        &mut vcx,
        super::panel_monitor::ProcKey {
            pid: 101,
            started: Some(1001),
        },
        false,
    );
    assert!(fixture.plans.lock().unwrap().is_empty());
    assert!(fixture.executed.lock().unwrap().is_empty());
    drop(app);

    // A row without a birth stamp names a pid, not a process: no action
    // either, whatever the source says.
    let fixture = Fixture::default();
    let mut unstamped = tree_one();
    unstamped.procs[1].started = None;
    fixture.procs.lock().unwrap().insert(1, Some(unstamped));
    *fixture.plan_answer.lock().unwrap() = Some(Some(Ok(make_plan())));
    let (app, mut vcx, _streams) = test_window::harness_with_tabs(cx, 1);
    install_monitor_host(&app, &mut vcx, &fixture);
    open_monitor(&app, &mut vcx);
    request_end(
        &app,
        &mut vcx,
        super::panel_monitor::ProcKey {
            pid: 101,
            started: None,
        },
        false,
    );
    assert!(fixture.plans.lock().unwrap().is_empty());
    assert!(fixture.executed.lock().unwrap().is_empty());
}

#[gpui_kit::test]
fn two_panes_sharing_a_directory_scope_the_request_to_the_context_pane(cx: &mut TestAppContext) {
    let fixture = Fixture::default();
    fixture.procs.lock().unwrap().insert(1, Some(tree_one()));
    fixture.procs.lock().unwrap().insert(2, Some(tree_two()));
    *fixture.plan_answer.lock().unwrap() = Some(Some(Ok(EndProcessPlan {
        targets: vec![end_target(201, "python", 2001)],
        complete: true,
    })));
    let (app, mut vcx, mut streams) = test_window::harness_with_tabs(cx, 2);
    let shared = PathBuf::from("/tmp/monitor-shared");
    for stream in streams.iter_mut() {
        DaemonMsg::Cwd(shared.clone()).encode(stream).unwrap();
    }
    install_monitor_host(&app, &mut vcx, &fixture);
    open_monitor(&app, &mut vcx);

    // Pane 2 shows its own tree; ending its process asks about pane 2, and
    // pane 1's fixture answer is never touched.
    vcx.dispatch_action(lyrel_ui_base::actions::SelectNextTab);
    settle(&mut vcx);
    // The root renders folded: expand it so the `python` row exists to click.
    let root_id = row_id(2, 200, 2000);
    vcx.update(|window, cx| window.click(format!("{root_id}-chevron"), cx));
    settle(&mut vcx);
    let py_id = row_id(2, 201, 2001);
    vcx.update(|window, cx| window.click(py_id.clone(), cx));
    settle(&mut vcx);
    request_end(
        &app,
        &mut vcx,
        super::panel_monitor::ProcKey {
            pid: 201,
            started: Some(2001),
        },
        false,
    );
    assert!(vcx.has_pending_prompt());
    vcx.simulate_prompt_answer("End Process");
    settle(&mut vcx);
    assert_eq!(
        fixture.plans.lock().unwrap().as_slice(),
        [(2, 201, Some(2001))],
        "the request names the context pane, never a same-directory neighbor"
    );
    assert_eq!(
        fixture.executed.lock().unwrap().as_slice(),
        [(
            2,
            vec![ProcIdentity {
                pid: 201,
                started: Some(2001)
            }]
        )],
    );
}

#[gpui_kit::test]
fn process_confirmation_cannot_move_to_a_replacement_host(cx: &mut TestAppContext) {
    let original = Fixture::default();
    original.procs.lock().unwrap().insert(1, Some(tree_one()));
    *original.plan_answer.lock().unwrap() = Some(Some(Ok(make_plan())));
    let (app, mut vcx, _streams) = test_window::harness_with_tabs(cx, 1);
    install_monitor_host(&app, &mut vcx, &original);
    open_monitor(&app, &mut vcx);
    request_end(
        &app,
        &mut vcx,
        super::panel_monitor::ProcKey {
            pid: 101,
            started: Some(1001),
        },
        false,
    );
    assert!(vcx.has_pending_prompt());

    let replacement = Fixture::default();
    replacement
        .procs
        .lock()
        .unwrap()
        .insert(1, Some(tree_one()));
    install_monitor_host(&app, &mut vcx, &replacement);
    settle(&mut vcx);
    vcx.simulate_prompt_answer("End Process");
    settle(&mut vcx);

    assert!(original.executed.lock().unwrap().is_empty());
    assert!(replacement.executed.lock().unwrap().is_empty());
}
