//! Activation of a saved history session lands in the right pane (#365).
//!
//! Driven through the production workbench window and the real
//! `resume_session` path, with the owning Host behind a decorator that
//! records the operations asked of it and answers a canned prepared session —
//! so the seam under test is the destination decision: focus an open exact
//! session, reuse a verified idle Shell, or split right, and surface launch
//! and identity failures. Panes are quiet stream-backed fixtures whose prompt,
//! cwd, program and input states are driven the way the daemon drives them.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gpui_kit::{App, Entity, Focusable as _, TestAppContext, VisualTestContext};
use lyrel_core::core::cli_agent::{AgentReadout, AgentSessionState, CLIAgent};
use lyrel_core::daemon::protocol::{ClientMsg, DaemonMsg};
use lyrel_core::daemon::transport::Stream;
use lyrel_core::host::session_operations::{
    PreparedSession, SessionErrorKind, SessionOperation, SessionOperationError,
    SessionOperationKind,
};
use lyrel_core::host::{Entry, Host, Meta, Output, SharedHost};
use lyrel_terminal::view::TerminalView;
use lyrel_ui_base::host_ops::HostId;

use crate::ui::agent_launch::ResumeSessionRequest;
use crate::ui::app::LyrelApp;

const SESSION_ID: &str = "hist365";
const COMMAND: &str = "pi --resume hist365 --session-dir /store";

/// The owning Host behind the resume: records what was asked, answers what
/// the test queued, and can hold the prepare open until the test changes the
/// pane underneath it — the readiness-changed-before-dispatch case.
struct HistoryHost {
    inner: SharedHost,
    seen: Mutex<Vec<(SessionOperationKind, String)>>,
    queue: Mutex<Vec<Result<PreparedSession, SessionOperationError>>>,
    gate: Mutex<Option<std::sync::mpsc::Receiver<()>>>,
    /// Bumped when the request reaches the Host, before a held gate delays
    /// its answer, so a test can act while the prepare is genuinely in
    /// flight.
    asked: std::sync::atomic::AtomicUsize,
}

impl HistoryHost {
    fn new(inner: SharedHost) -> Arc<Self> {
        Arc::new(Self {
            inner,
            seen: Mutex::new(Vec::new()),
            queue: Mutex::new(Vec::new()),
            gate: Mutex::new(None),
            asked: std::sync::atomic::AtomicUsize::new(0),
        })
    }

    fn asked(&self) -> usize {
        self.asked.load(std::sync::atomic::Ordering::SeqCst)
    }

    fn push(&self, answer: Result<PreparedSession, SessionOperationError>) {
        self.queue.lock().unwrap().push(answer);
    }

    fn hold_until(&self, release: std::sync::mpsc::Receiver<()>) {
        *self.gate.lock().unwrap() = Some(release);
    }

    fn seen(&self) -> Vec<(SessionOperationKind, String)> {
        self.seen.lock().unwrap().clone()
    }
}

impl Host for HistoryHost {
    fn id(&self) -> HostId {
        self.inner.id()
    }
    fn separator(&self) -> char {
        self.inner.separator()
    }
    fn is_absolute(&self, p: &Path) -> bool {
        self.inner.is_absolute(p)
    }
    fn read_dir(&self, dir: &Path, root: Option<&Path>) -> io::Result<Vec<Entry>> {
        self.inner.read_dir(dir, root)
    }
    fn stat(&self, p: &Path) -> io::Result<Meta> {
        self.inner.stat(p)
    }
    fn read_file(&self, p: &Path, max: u64) -> io::Result<Vec<u8>> {
        self.inner.read_file(p, max)
    }
    fn canonicalize(&self, p: &Path) -> io::Result<PathBuf> {
        self.inner.canonicalize(p)
    }
    fn search(
        &self,
        roots: &[PathBuf],
        query: &str,
        limit: usize,
        dirs: usize,
        hidden: bool,
    ) -> io::Result<Vec<lyrel_core::host::SearchHit>> {
        self.inner.search(roots, query, limit, dirs, hidden)
    }
    fn search_content(
        &self,
        roots: &[PathBuf],
        query: &lyrel_core::host::ContentQuery,
        limits: &lyrel_core::host::ContentLimits,
    ) -> io::Result<lyrel_core::host::ContentResults> {
        self.inner.search_content(roots, query, limits)
    }
    fn write_file(&self, p: &Path, bytes: &[u8]) -> io::Result<Meta> {
        self.inner.write_file(p, bytes)
    }
    fn create_file_new(&self, p: &Path) -> io::Result<()> {
        self.inner.create_file_new(p)
    }
    fn create_dir(&self, p: &Path, recursive: bool) -> io::Result<()> {
        self.inner.create_dir(p, recursive)
    }
    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        self.inner.rename(from, to)
    }
    fn remove(&self, p: &Path, recursive: bool) -> io::Result<()> {
        self.inner.remove(p, recursive)
    }
    fn repo_root(&self, p: &Path) -> io::Result<Option<PathBuf>> {
        self.inner.repo_root(p)
    }
    fn git(&self, cwd: &Path, args: &[&str]) -> io::Result<Output> {
        self.inner.git(cwd, args)
    }
    fn shells(&self) -> io::Result<lyrel_core::core::shells::ShellInventory> {
        self.inner.shells()
    }
    fn watch(&self, dirs: &[PathBuf]) -> io::Result<lyrel_core::host::WatchSub> {
        self.inner.watch(dirs)
    }
    fn session_operation(
        &self,
        request: &SessionOperation,
    ) -> Result<PreparedSession, SessionOperationError> {
        self.asked.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if let Some(gate) = self.gate.lock().unwrap().take()
            && gate.recv().is_err()
        {
            return Err(SessionOperationError {
                kind: SessionErrorKind::Connection,
                message: "test gate closed".into(),
                retained: None,
            });
        }
        self.seen
            .lock()
            .unwrap()
            .push((request.kind, request.source_id.clone()));
        let mut queue = self.queue.lock().unwrap();
        match queue.is_empty() {
            true => Err(SessionOperationError {
                kind: SessionErrorKind::Unsupported,
                message: "no canned answer".into(),
                retained: None,
            }),
            false => queue.remove(0),
        }
    }
}

fn prepared(cwd: PathBuf) -> PreparedSession {
    PreparedSession {
        host_id: HostId::LOCAL.0,
        agent: CLIAgent::Pi,
        source_id: SESSION_ID.into(),
        target_id: Some(SESSION_ID.into()),
        session_file: Some(PathBuf::from("/store/hist365.jsonl")),
        title: Some("hist 365".into()),
        cwd,
        scope: Default::default(),
        launch_argv: vec!["pi".into()],
        command: Some(COMMAND.into()),
        created: false,
    }
}

fn antigravity_prepared(cwd: PathBuf, conversation: &str) -> PreparedSession {
    PreparedSession {
        host_id: HostId::LOCAL.0,
        agent: CLIAgent::Antigravity,
        source_id: conversation.into(),
        target_id: Some(conversation.into()),
        session_file: Some(PathBuf::from(format!(
            "/store/.gemini/antigravity-cli/brain/{conversation}/.system_generated/logs/transcript.jsonl"
        ))),
        title: Some("agy fixture".into()),
        cwd,
        scope: Default::default(),
        launch_argv: vec!["agy".into()],
        command: Some(format!("agy --conversation {conversation}")),
        created: false,
    }
}

fn request(cwd: Option<PathBuf>, fork: bool) -> ResumeSessionRequest {
    ResumeSessionRequest {
        agent: CLIAgent::Pi,
        session_id: SESSION_ID.into(),
        cwd,
        host_id: HostId::LOCAL,
        scope: None,
        fork,
    }
}

fn install_host(cx: &mut App, host: Arc<HistoryHost>) {
    lyrel_ui_base::host_registry::HostRegistry::insert(cx, host);
}

fn pump_until(deadline_msg: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !done() {
        assert!(
            Instant::now() < deadline,
            "{deadline_msg} within the deadline"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Pump the executor each round: state that lands through `HostOps`
/// callbacks and notifications only exists once the executor has run.
fn settle(vcx: &mut VisualTestContext, deadline_msg: &str, mut done: impl FnMut(&mut App) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        vcx.run_until_parked();
        if vcx.update(|_, cx| done(cx)) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "{deadline_msg} within the deadline"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn wait_for_scoped_session(view: &Entity<TerminalView>, vcx: &mut VisualTestContext) {
    // The daemon delivers agent detection and session identity in separate frames.
    pump_until("the scoped session report lands", || {
        view.read_with(vcx, |view, _| {
            view.agent() == Some(CLIAgent::Pi)
                && view.agent_session().is_some_and(|state| {
                    state.session_id.as_deref() == Some(SESSION_ID)
                        && state.readout.session_scope.is_some()
                })
        })
    });
}

fn context_view(app: &Entity<LyrelApp>, vcx: &mut VisualTestContext) -> Entity<TerminalView> {
    app.update(vcx, |app, _| app.tabs[0].pane.terminals()[0].clone())
}

/// Report a prompt state and wait until the pane's link believes it.
fn report_prompt(
    stream: &mut Stream,
    view: &Entity<TerminalView>,
    vcx: &mut VisualTestContext,
    at: bool,
) {
    DaemonMsg::Prompt {
        active: true,
        at_prompt: at,
        last_exit: None,
    }
    .encode(stream)
    .unwrap();

    pump_until("the prompt report lands", || {
        view.read_with(vcx, |view, _| view.terminal.at_prompt() == at)
    });
}

/// The foreground poll that turns the prompt report into the view's running
/// state, the way the production 300 ms timer does.
fn poll(view: &Entity<TerminalView>, vcx: &mut VisualTestContext) {
    view.update_in(vcx, |view, window, cx| view.poll_foreground(window, cx));
}

/// Drive the context pane to a verified idle prompt sitting in `cwd`.
fn make_idle_context(
    app: &Entity<LyrelApp>,
    vcx: &mut VisualTestContext,
    stream: &mut Stream,
    cwd: &Path,
) -> Entity<TerminalView> {
    let view = context_view(app, vcx);
    DaemonMsg::Cwd(cwd.to_path_buf()).encode(stream).unwrap();

    report_prompt(stream, &view, vcx, true);
    pump_until("the cwd report lands", || {
        view.read_with(vcx, |view, _| view.host_cwd().as_deref() == Some(cwd))
    });
    view
}

fn leaves(app: &Entity<LyrelApp>, vcx: &mut VisualTestContext) -> usize {
    app.update(vcx, |app, _| {
        app.tabs[app.tabs.active_index()].pane.leaves().len()
    })
}

fn tab_count(app: &Entity<LyrelApp>, vcx: &mut VisualTestContext) -> usize {
    app.update(vcx, |app, _| app.tabs.len())
}

fn pane_count(app: &Entity<LyrelApp>, vcx: &mut VisualTestContext) -> usize {
    app.update(vcx, |app, _| {
        app.tabs.iter().map(|t| t.pane.terminals().len()).sum()
    })
}

fn run_resume(app: &Entity<LyrelApp>, vcx: &mut VisualTestContext, req: ResumeSessionRequest) {
    app.update_in(vcx, |app, window, cx| {
        app.resume_session(req, window, cx);
    });
}

/// Every input payload the pane wrote, read until the short timeout empties.
fn inputs_now(stream: &mut Stream) -> Vec<Vec<u8>> {
    stream
        .set_read_timeout(Some(Duration::from_millis(200)))
        .unwrap();
    let mut inputs = Vec::new();
    loop {
        match ClientMsg::read(stream) {
            Ok(ClientMsg::Input(bytes)) => inputs.push(bytes),
            Ok(_) => {}
            Err(_) => break,
        }
    }
    inputs
}

fn wait_split(app: &Entity<LyrelApp>, vcx: &mut VisualTestContext, what: &str) {
    vcx.background_executor.run_until_parked();
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if leaves(app, vcx) == 2 {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "{what}: the right split never opened"
        );
        vcx.background_executor.run_until_parked();
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[gpui_kit::test]
fn resume_focuses_an_open_exact_session_without_a_second_writer(cx: &mut TestAppContext) {
    let host = HistoryHost::new(lyrel_core::host::local::shared());
    let (app, mut vcx, mut stream) = crate::ui::app::test_window::harness_with_pane(cx);
    install_host_for(&app, &mut vcx, host.clone());
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().canonicalize().unwrap();
    let first = make_idle_context(&app, &mut vcx, &mut stream, &cwd);
    // The pane runs the exact session: same agent, same native id, same scope.
    let mut scope = lyrel_core::host::session_operations::SessionScope::default();
    scope.session_file = Some(PathBuf::from("/store/hist365.jsonl"));
    DaemonMsg::Agent(Some(CLIAgent::Pi))
        .encode(&mut stream)
        .unwrap();
    DaemonMsg::AgentStatus(Some(Box::new(AgentSessionState {
        session_id: Some(SESSION_ID.into()),
        readout: AgentReadout {
            session_scope: Some(scope),
            ..Default::default()
        },
        ..Default::default()
    })))
    .encode(&mut stream)
    .unwrap();

    wait_for_scoped_session(&first, &mut vcx);
    // Focus sits in a second, non-matching pane so the focus move is visible.
    let (second, mut _second_stream) = app.update_in(&mut vcx, |app, window, cx| {
        let (view, stream) = lyrel_terminal::view::quiet_test_pane(77, window, cx);
        LyrelApp::watch_terminal(&view, window, cx);
        app.tabs
            .push(crate::ui::tab::Tab::new(crate::ui::pane::Pane::leaf(
                crate::ui::pane::PaneSlot::Ready(view.clone()),
            )));
        app.tabs.select(1);
        let focus = view.read(cx).focus_handle(cx).clone();
        window.focus(&focus, cx);
        cx.notify();
        (view, stream)
    });
    let tabs_before = tab_count(&app, &mut vcx);
    let panes_before = pane_count(&app, &mut vcx);
    // The listing row carries the session's durable scope, exactly what the
    // running pane reports; identity matching is exact on both.
    let mut req_scope = lyrel_core::host::session_operations::SessionScope::default();
    req_scope.session_file = Some(PathBuf::from("/store/hist365.jsonl"));
    req_scope
        .environment
        .insert("PI_CODING_AGENT_DIR".into(), "/store".into());
    let mut req = request(Some(cwd), false);
    req.scope = Some(req_scope);
    run_resume(&app, &mut vcx, req);
    vcx.background_executor.run_until_parked();

    assert!(
        host.seen().is_empty(),
        "an open exact session must not reach the Host: {:?}",
        host.seen()
    );
    assert_eq!(tab_count(&app, &mut vcx), tabs_before, "no new tab");
    assert_eq!(pane_count(&app, &mut vcx), panes_before, "no new pane");
    let focused = vcx.update(|window, cx| window.focused(cx));
    let first_focus = first.read_with(&mut vcx, |view, cx| view.focus_handle(cx));
    assert_eq!(
        focused,
        Some(first_focus.clone()),
        "the original pane took focus"
    );
    assert_ne!(
        second.read_with(&mut vcx, |view, cx| view.focus_handle(cx)),
        first_focus
    );
}

/// The live readout need not repeat the row's exact file: the durable store
/// selectors both sides state are enough to confirm the same session, so
/// the open pane is focused instead of a second writer opening beside it.
#[gpui_kit::test]
fn resume_focuses_an_open_session_with_an_equivalent_store(cx: &mut TestAppContext) {
    let host = HistoryHost::new(lyrel_core::host::local::shared());
    let (app, mut vcx, mut stream) = crate::ui::app::test_window::harness_with_pane(cx);
    install_host_for(&app, &mut vcx, host.clone());
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().canonicalize().unwrap();
    let first = make_idle_context(&app, &mut vcx, &mut stream, &cwd);
    let mut live_scope = lyrel_core::host::session_operations::SessionScope::default();
    live_scope
        .environment
        .insert("PI_CODING_AGENT_DIR".into(), "/store".into());
    DaemonMsg::Agent(Some(CLIAgent::Pi))
        .encode(&mut stream)
        .unwrap();
    DaemonMsg::AgentStatus(Some(Box::new(AgentSessionState {
        session_id: Some(SESSION_ID.into()),
        readout: AgentReadout {
            session_scope: Some(live_scope),
            ..Default::default()
        },
        ..Default::default()
    })))
    .encode(&mut stream)
    .unwrap();
    wait_for_scoped_session(&first, &mut vcx);

    // Focus sits in a second, non-matching pane so the focus move is visible.
    let (second, mut _second_stream) = app.update_in(&mut vcx, |app, window, cx| {
        let (view, stream) = lyrel_terminal::view::quiet_test_pane(78, window, cx);
        LyrelApp::watch_terminal(&view, window, cx);
        app.tabs
            .push(crate::ui::tab::Tab::new(crate::ui::pane::Pane::leaf(
                crate::ui::pane::PaneSlot::Ready(view.clone()),
            )));
        app.tabs.select(1);
        let focus = view.read(cx).focus_handle(cx).clone();
        window.focus(&focus, cx);
        cx.notify();
        (view, stream)
    });
    assert_eq!(
        vcx.update(|window, cx| window.focused(cx)),
        Some(second.read_with(&mut vcx, |view, cx| view.focus_handle(cx))),
        "the second pane starts focused"
    );
    let panes_before = pane_count(&app, &mut vcx);
    let mut req_scope = lyrel_core::host::session_operations::SessionScope::default();
    req_scope.session_file = Some(PathBuf::from("/store/hist365.jsonl"));
    req_scope
        .environment
        .insert("PI_CODING_AGENT_DIR".into(), "/store".into());
    let mut req = request(Some(cwd), false);
    req.scope = Some(req_scope);
    run_resume(&app, &mut vcx, req);
    vcx.background_executor.run_until_parked();

    assert!(
        host.seen().is_empty(),
        "an equivalent store must not reach the Host: {:?}",
        host.seen()
    );
    assert_eq!(pane_count(&app, &mut vcx), panes_before, "no new pane");
    let focused = vcx.update(|window, cx| window.focused(cx));
    assert_eq!(
        focused,
        Some(first.read_with(&mut vcx, |view, cx| view.focus_handle(cx))),
        "the original pane took focus"
    );
}

/// A same-ID pane whose store cannot be checked against the selected row is
/// neither focused (it may be another store's record) nor resumed over (it
/// may already be this one): the identity failure is surfaced instead of
/// starting a second writer.
#[gpui_kit::test]
fn resume_refuses_an_unconfirmed_open_session_instead_of_a_second_writer(cx: &mut TestAppContext) {
    let host = HistoryHost::new(lyrel_core::host::local::shared());
    let (app, mut vcx, mut stream) = crate::ui::app::test_window::harness_with_pane(cx);
    install_host_for(&app, &mut vcx, host.clone());
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().canonicalize().unwrap();
    let view = make_idle_context(&app, &mut vcx, &mut stream, &cwd);
    // The pane reports the same agent and native id but no store scope:
    // there is no evidence either way.
    DaemonMsg::Agent(Some(CLIAgent::Pi))
        .encode(&mut stream)
        .unwrap();
    DaemonMsg::AgentStatus(Some(Box::new(AgentSessionState {
        session_id: Some(SESSION_ID.into()),
        readout: AgentReadout::default(),
        ..Default::default()
    })))
    .encode(&mut stream)
    .unwrap();
    pump_until("the native session identity lands", || {
        view.read_with(&mut vcx, |view, _| {
            view.agent() == Some(CLIAgent::Pi)
                && view
                    .agent_session()
                    .and_then(|state| state.session_id)
                    .as_deref()
                    == Some(SESSION_ID)
        })
    });
    let tabs_before = tab_count(&app, &mut vcx);
    let panes_before = pane_count(&app, &mut vcx);

    let mut req_scope = lyrel_core::host::session_operations::SessionScope::default();
    req_scope
        .environment
        .insert("PI_CODING_AGENT_DIR".into(), "/store".into());
    let mut req = request(Some(cwd), false);
    req.scope = Some(req_scope);
    run_resume(&app, &mut vcx, req);
    settle(&mut vcx, "the unconfirmed identity is surfaced", |cx| {
        crate::ui::notification_history::history(cx)
            .read(cx)
            .records()
            .any(|record| record.message().contains(SESSION_ID))
    });

    assert!(
        host.seen().is_empty(),
        "no resume may be prepared while a same-ID writer is unconfirmed: {:?}",
        host.seen()
    );
    assert_eq!(tab_count(&app, &mut vcx), tabs_before, "no new tab");
    assert_eq!(pane_count(&app, &mut vcx), panes_before, "no new pane");
    assert!(inputs_now(&mut stream).is_empty());
}

/// A session that opens while its prepare is in flight is found again at
/// dispatch: the prepared line is not typed into a new split beside the
/// pane that already runs it.
#[gpui_kit::test]
fn dispatch_focuses_a_session_opened_while_preparing(cx: &mut TestAppContext) {
    let host = HistoryHost::new(lyrel_core::host::local::shared());
    let (app, mut vcx, mut stream) = crate::ui::app::test_window::harness_with_pane(cx);
    install_host_for(&app, &mut vcx, host.clone());
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().canonicalize().unwrap();
    let first = make_idle_context(&app, &mut vcx, &mut stream, &cwd);

    // A second pane holds focus, so the dispatch's focus move is observable.
    let (second, _second_stream) = app.update_in(&mut vcx, |app, window, cx| {
        let (view, stream) = lyrel_terminal::view::quiet_test_pane(79, window, cx);
        LyrelApp::watch_terminal(&view, window, cx);
        app.tabs
            .push(crate::ui::tab::Tab::new(crate::ui::pane::Pane::leaf(
                crate::ui::pane::PaneSlot::Ready(view.clone()),
            )));
        app.tabs.select(1);
        let focus = view.read(cx).focus_handle(cx).clone();
        window.focus(&focus, cx);
        cx.notify();
        (view, stream)
    });
    let second_focus = second.read_with(&mut vcx, |view, cx| view.focus_handle(cx));
    assert_eq!(
        vcx.update(|window, cx| window.focused(cx)),
        Some(second_focus)
    );
    let panes_before = pane_count(&app, &mut vcx);

    let (release, gate) = std::sync::mpsc::channel();
    host.hold_until(gate);
    host.push(Ok(prepared(cwd.clone())));
    let mut req_scope = lyrel_core::host::session_operations::SessionScope::default();
    req_scope
        .environment
        .insert("PI_CODING_AGENT_DIR".into(), "/store".into());
    let mut req = request(Some(cwd.clone()), false);
    req.scope = Some(req_scope);
    run_resume(&app, &mut vcx, req);
    let prepare_deadline = Instant::now() + Duration::from_secs(15);
    while host.asked() == 0 {
        vcx.background_executor.run_until_parked();
        assert!(
            Instant::now() < prepare_deadline,
            "the prepare is asked within the deadline"
        );
        std::thread::sleep(Duration::from_millis(5));
    }

    // The session comes up in the original pane while the answer is held.
    let mut live_scope = lyrel_core::host::session_operations::SessionScope::default();
    live_scope
        .environment
        .insert("PI_CODING_AGENT_DIR".into(), "/store".into());
    DaemonMsg::Agent(Some(CLIAgent::Pi))
        .encode(&mut stream)
        .unwrap();
    DaemonMsg::AgentStatus(Some(Box::new(AgentSessionState {
        session_id: Some(SESSION_ID.into()),
        readout: AgentReadout {
            session_scope: Some(live_scope),
            ..Default::default()
        },
        ..Default::default()
    })))
    .encode(&mut stream)
    .unwrap();
    wait_for_scoped_session(&first, &mut vcx);

    release.send(()).unwrap();
    let first_focus = first.read_with(&mut vcx, |view, cx| view.focus_handle(cx));
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        vcx.run_until_parked();
        if vcx.update(|window, cx| window.focused(cx)) == Some(first_focus.clone()) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the dispatch focuses the opened pane within the deadline"
        );
        std::thread::sleep(Duration::from_millis(5));
    }

    assert_eq!(
        pane_count(&app, &mut vcx),
        panes_before,
        "the prepared line did not open a second pane"
    );
    assert!(inputs_now(&mut stream).is_empty());
}

fn install_host_for(app: &Entity<LyrelApp>, vcx: &mut VisualTestContext, host: Arc<HistoryHost>) {
    app.update(vcx, |_, cx| install_host(cx, host));
}

#[gpui_kit::test]
fn resume_reuses_a_verified_idle_shell_in_the_source_directory(cx: &mut TestAppContext) {
    let host = HistoryHost::new(lyrel_core::host::local::shared());
    let (app, mut vcx, mut stream) = crate::ui::app::test_window::harness_with_pane(cx);
    install_host_for(&app, &mut vcx, host.clone());
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().canonicalize().unwrap();
    make_idle_context(&app, &mut vcx, &mut stream, &cwd);
    host.push(Ok(prepared(cwd.clone())));
    assert_eq!(pane_count(&app, &mut vcx), 1);

    run_resume(&app, &mut vcx, request(Some(cwd.clone()), false));
    // Reuse types the prepared line into the very pane that was verified
    // idle: one input payload, submitted exactly the way a typed line is —
    // the command bytes plus the CR that submits them.
    let expected = lyrel_terminal::prompt_input::bytes::submit_bytes(COMMAND, false, false);
    settle(
        &mut vcx,
        "the prepared command reaches the idle shell",
        |_| {
            inputs_now(&mut stream)
                .iter()
                .any(|bytes| *bytes == expected)
        },
    );
    assert_eq!(tab_count(&app, &mut vcx), 1, "reuse opens no tab");
    assert_eq!(pane_count(&app, &mut vcx), 1, "reuse opens no pane");
    assert_eq!(
        host.seen(),
        vec![(SessionOperationKind::Resume, SESSION_ID.into())]
    );
}

#[gpui_kit::test]
fn resume_splits_right_when_the_context_pane_runs_a_program(cx: &mut TestAppContext) {
    let host = HistoryHost::new(lyrel_core::host::local::shared());
    let (app, mut vcx, mut stream) = crate::ui::app::test_window::harness_with_pane(cx);
    install_host_for(&app, &mut vcx, host.clone());
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().canonicalize().unwrap();
    let view = make_idle_context(&app, &mut vcx, &mut stream, &cwd);
    // An ordinary program starts: the prompt leaves and stays away.
    report_prompt(&mut stream, &view, &mut vcx, false);
    poll(&view, &mut vcx);
    assert!(view.read_with(&mut vcx, |view, _| view.busy().is_some()));
    host.push(Ok(prepared(cwd.clone())));

    run_resume(&app, &mut vcx, request(Some(cwd.clone()), false));
    wait_split(&app, &mut vcx, "a busy context pane");
    assert_eq!(tab_count(&app, &mut vcx), 1, "the split stays in the tab");
    let typed = inputs_now(&mut stream);
    assert!(
        !typed.iter().any(|bytes| bytes == COMMAND.as_bytes()),
        "the running program must not receive the resume line: {typed:?}"
    );
}

#[gpui_kit::test]
fn resume_splits_right_when_the_context_pane_sits_on_the_alternate_screen(cx: &mut TestAppContext) {
    let host = HistoryHost::new(lyrel_core::host::local::shared());
    let (app, mut vcx, mut stream) = crate::ui::app::test_window::harness_with_pane(cx);
    install_host_for(&app, &mut vcx, host.clone());
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().canonicalize().unwrap();
    let view = make_idle_context(&app, &mut vcx, &mut stream, &cwd);
    // A full-screen program took over: the alternate screen is never a
    // reusable idle Shell, even if a prompt mark were relayed behind it.
    DaemonMsg::Output(b"\x1b[?1049h".to_vec())
        .encode(&mut stream)
        .unwrap();

    pump_until("the alternate screen reaches the grid", || {
        view.read_with(&mut vcx, |view, _| view.on_alt_screen())
    });
    host.push(Ok(prepared(cwd.clone())));

    run_resume(&app, &mut vcx, request(Some(cwd.clone()), false));
    wait_split(&app, &mut vcx, "an alternate-screen context pane");
    assert!(
        view.read_with(&mut vcx, |view, _| view.on_alt_screen()),
        "the full-screen program is still running where it was"
    );
}

#[gpui_kit::test]
fn held_input_is_preserved_and_the_resume_splits_right(cx: &mut TestAppContext) {
    let host = HistoryHost::new(lyrel_core::host::local::shared());
    let (app, mut vcx, mut stream) = crate::ui::app::test_window::harness_with_pane(cx);
    install_host_for(&app, &mut vcx, host.clone());
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().canonicalize().unwrap();
    let view = make_idle_context(&app, &mut vcx, &mut stream, &cwd);
    // The user typed something and left it unsubmitted.
    view.update(&mut vcx, |view, cx| view.commit_text("git st", cx));
    assert!(view.read_with(&mut vcx, |view, _| view.has_pending_prompt_input()));
    host.push(Ok(prepared(cwd.clone())));

    run_resume(&app, &mut vcx, request(Some(cwd.clone()), false));
    wait_split(&app, &mut vcx, "a line the user is still holding");
    assert!(
        view.read_with(&mut vcx, |view, _| view.has_pending_prompt_input()),
        "the held input survives untouched"
    );
    let typed = inputs_now(&mut stream);
    assert!(
        !typed.iter().any(|bytes| bytes == COMMAND.as_bytes()),
        "the held line must not receive the resume command: {typed:?}"
    );
}

#[gpui_kit::test]
fn resume_splits_right_when_readiness_changed_before_dispatch(cx: &mut TestAppContext) {
    let host = HistoryHost::new(lyrel_core::host::local::shared());
    let (app, mut vcx, mut stream) = crate::ui::app::test_window::harness_with_pane(cx);
    install_host_for(&app, &mut vcx, host.clone());
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().canonicalize().unwrap();
    let view = make_idle_context(&app, &mut vcx, &mut stream, &cwd);
    // The prepare holds until the pane is no longer idle — readiness changed
    // between the request and the dispatch.
    let (release, gate) = std::sync::mpsc::channel();
    host.hold_until(gate);
    host.push(Ok(prepared(cwd.clone())));
    run_resume(&app, &mut vcx, request(Some(cwd.clone()), false));
    report_prompt(&mut stream, &view, &mut vcx, false);
    poll(&view, &mut vcx);
    assert!(view.read_with(&mut vcx, |view, _| view.busy().is_some()));
    release.send(()).unwrap();
    wait_split(&app, &mut vcx, "readiness that changed before dispatch");
}

#[gpui_kit::test]
fn resume_discards_preparation_after_replacing_the_host_connection(cx: &mut TestAppContext) {
    let host = HistoryHost::new(lyrel_core::host::local::shared());
    let (app, mut vcx, mut stream) = crate::ui::app::test_window::harness_with_pane(cx);
    install_host_for(&app, &mut vcx, host.clone());
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().canonicalize().unwrap();
    make_idle_context(&app, &mut vcx, &mut stream, &cwd);
    let (release, gate) = std::sync::mpsc::channel();
    host.hold_until(gate);
    host.push(Ok(prepared(cwd.clone())));
    run_resume(&app, &mut vcx, request(Some(cwd), false));

    let replacement = HistoryHost::new(lyrel_core::host::local::shared());
    install_host_for(&app, &mut vcx, replacement.clone());
    release.send(()).unwrap();
    settle(&mut vcx, "retired host response is refused", |cx| {
        crate::ui::notification_history::history(cx)
            .read(cx)
            .records()
            .any(|note| note.message().contains("host changed while preparing it"))
    });
    assert_eq!(pane_count(&app, &mut vcx), 1);
    assert!(replacement.seen().is_empty());
    assert!(inputs_now(&mut stream).is_empty());
}

#[gpui_kit::test]
fn resume_splits_right_when_readiness_is_unknown(cx: &mut TestAppContext) {
    let host = HistoryHost::new(lyrel_core::host::local::shared());
    let (app, mut vcx, _stream) = crate::ui::app::test_window::harness_with_pane(cx);
    install_host_for(&app, &mut vcx, host.clone());
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().canonicalize().unwrap();
    // No prompt report and no cwd report: the pane cannot prove idle, so the
    // resume cannot trust it.
    host.push(Ok(prepared(cwd.clone())));
    run_resume(&app, &mut vcx, request(Some(cwd.clone()), false));
    wait_split(&app, &mut vcx, "readiness that cannot be verified");
}

#[gpui_kit::test]
fn resume_waits_for_a_slow_new_shell_prompt(cx: &mut TestAppContext) {
    let (app, mut vcx, mut stream) = crate::ui::app::test_window::harness_with_pane(cx);
    let view = context_view(&app, &mut vcx);
    let cwd = tempfile::tempdir().unwrap();
    app.update_in(&mut vcx, |app, window, cx| {
        app.run_history_prepared_when_ready(
            crate::ui::pane::PaneSlot::Ready(view.clone()),
            prepared(cwd.path().to_path_buf()),
            window,
            cx,
        );
    });
    vcx.run_until_parked();

    // A shell profile can take longer than the ordinary quick-launch grace
    // period. Keep the terminal responsive while it has no trusted prompt.
    vcx.executor().advance_clock(Duration::from_millis(3100));
    vcx.run_until_parked();
    assert!(inputs_now(&mut stream).is_empty());

    let elsewhere = tempfile::tempdir().unwrap();
    make_idle_context(&app, &mut vcx, &mut stream, elsewhere.path());
    poll(&view, &mut vcx);
    vcx.executor().advance_clock(Duration::from_millis(100));
    vcx.run_until_parked();
    assert!(inputs_now(&mut stream).is_empty());

    make_idle_context(&app, &mut vcx, &mut stream, cwd.path());
    poll(&view, &mut vcx);
    vcx.executor().advance_clock(Duration::from_millis(100));
    vcx.run_until_parked();
    assert_eq!(
        inputs_now(&mut stream),
        vec![format!("{COMMAND}\r").into_bytes()]
    );
}

#[gpui_kit::test]
fn an_identity_failure_is_surfaced_without_touching_any_pane(cx: &mut TestAppContext) {
    let host = HistoryHost::new(lyrel_core::host::local::shared());
    let (app, mut vcx, _stream) = crate::ui::app::test_window::harness_with_pane(cx);
    install_host_for(&app, &mut vcx, host.clone());
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().canonicalize().unwrap();
    host.push(Err(SessionOperationError {
        kind: SessionErrorKind::Identity,
        message: "Pi source file does not match the selected session identity".into(),
        retained: None,
    }));
    let tabs_before = tab_count(&app, &mut vcx);
    run_resume(&app, &mut vcx, request(Some(cwd), false));
    settle(&mut vcx, "the identity error on screen", |cx| {
        crate::ui::notification_history::history(cx)
            .read(cx)
            .records()
            .any(|record| {
                record
                    .message()
                    .contains("does not match the selected session identity")
            })
    });
    assert_eq!(tab_count(&app, &mut vcx), tabs_before, "no pane opened");
}

#[gpui_kit::test]
fn fork_still_opens_a_new_tab_even_when_the_session_is_open(cx: &mut TestAppContext) {
    let host = HistoryHost::new(lyrel_core::host::local::shared());
    let (app, mut vcx, mut stream) = crate::ui::app::test_window::harness_with_pane(cx);
    install_host_for(&app, &mut vcx, host.clone());
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().canonicalize().unwrap();
    let view = make_idle_context(&app, &mut vcx, &mut stream, &cwd);
    let mut scope = lyrel_core::host::session_operations::SessionScope::default();
    scope.session_file = Some(PathBuf::from("/store/hist365.jsonl"));
    DaemonMsg::Agent(Some(CLIAgent::Pi))
        .encode(&mut stream)
        .unwrap();
    DaemonMsg::AgentStatus(Some(Box::new(AgentSessionState {
        session_id: Some(SESSION_ID.into()),
        readout: AgentReadout {
            session_scope: Some(scope),
            ..Default::default()
        },
        ..Default::default()
    })))
    .encode(&mut stream)
    .unwrap();

    pump_until("the agent report lands", || {
        view.read_with(&mut vcx, |view, _| view.agent() == Some(CLIAgent::Pi))
    });
    let mut forked = prepared(std::env::temp_dir());
    forked.created = true;
    forked.target_id = Some("hist365-fork".into());
    host.push(Ok(forked));
    let tabs_before = tab_count(&app, &mut vcx);

    run_resume(&app, &mut vcx, request(Some(std::env::temp_dir()), true));
    settle(&mut vcx, "the fork opening its new tab", |cx| {
        app.update(cx, |app, _| app.tabs.len()) == tabs_before + 1
    });
    assert!(
        host.seen()
            .iter()
            .any(|(kind, id)| *kind == SessionOperationKind::Fork && id == SESSION_ID),
        "the fork reaches the Host as a fork: {:?}",
        host.seen()
    );
}

/// The three consumers share one plan: an offered Antigravity agent adds one
/// read-only index request, and an unoffered one adds none.
#[gpui_kit::test]
fn plan_scan_offers_antigravity_history_only_for_an_offered_agent(
    cx: &mut gpui_kit::TestAppContext,
) {
    let launches = std::collections::HashMap::new();
    let offered = cx.update(|cx| {
        crate::ui::session_history::plan_scan(
            HostId::LOCAL,
            &[],
            &[CLIAgent::Antigravity],
            Path::new("/project"),
            &launches,
            cx,
        )
    });
    assert_eq!(offered.antigravity.len(), 1);
    assert_eq!(offered.antigravity[0].agent, CLIAgent::Antigravity);
    assert_eq!(offered.antigravity[0].cwd, PathBuf::from("/project"));
    assert_eq!(
        offered.antigravity[0].kind,
        SessionOperationKind::Resume
    );

    let unoffered = cx.update(|cx| {
        crate::ui::session_history::plan_scan(
            HostId::LOCAL,
            &[],
            &[CLIAgent::Pi],
            Path::new("/project"),
            &launches,
            cx,
        )
    });
    assert!(
        unoffered.antigravity.is_empty(),
        "a workspace that never offered agy does not scan its store"
    );
}

/// Antigravity's native conversation id is the exact identity: a pane that
/// reports it on the same Host is the same conversation even when the live
/// hook's transcript path is not the scan's derived path. Focusing it starts
/// no second writer (#385).
#[gpui_kit::test]
fn resume_focuses_an_open_antigravity_conversation_by_exact_id(cx: &mut TestAppContext) {
    let host = HistoryHost::new(lyrel_core::host::local::shared());
    let (app, mut vcx, mut stream) = crate::ui::app::test_window::harness_with_pane(cx);
    install_host_for(&app, &mut vcx, host.clone());
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().canonicalize().unwrap();
    let first = make_idle_context(&app, &mut vcx, &mut stream, &cwd);
    DaemonMsg::Agent(Some(CLIAgent::Antigravity))
        .encode(&mut stream)
        .unwrap();
    DaemonMsg::AgentStatus(Some(Box::new(AgentSessionState {
        session_id: Some(SESSION_ID.into()),
        ..Default::default()
    })))
    .encode(&mut stream)
    .unwrap();
    pump_until("the Antigravity identity lands", || {
        first.read_with(&mut vcx, |view, _| {
            view.agent() == Some(CLIAgent::Antigravity)
                && view
                    .agent_session()
                    .and_then(|state| state.session_id)
                    .as_deref()
                    == Some(SESSION_ID)
        })
    });

    let tabs_before = tab_count(&app, &mut vcx);
    let panes_before = pane_count(&app, &mut vcx);
    let mut req = request(Some(cwd), false);
    req.agent = CLIAgent::Antigravity;
    // The live hook need not repeat the scan's transcript path; the exact
    // conversation id is the identity the CLI itself assigned.
    let mut saved = lyrel_core::host::session_operations::SessionScope::default();
    saved.session_file = Some(PathBuf::from(
        "/some/other/.gemini/antigravity-cli/brain/hist365/.system_generated/logs/transcript.jsonl",
    ));
    req.scope = Some(saved);
    run_resume(&app, &mut vcx, req);
    vcx.background_executor.run_until_parked();

    assert!(
        host.seen().is_empty(),
        "an open exact conversation must not reach the Host: {:?}",
        host.seen()
    );
    assert_eq!(tab_count(&app, &mut vcx), tabs_before, "no new tab");
    assert_eq!(pane_count(&app, &mut vcx), panes_before, "no new pane");
    let focused = vcx.update(|window, cx| window.focused(cx));
    let first_focus = first.read_with(&mut vcx, |view, cx| view.focus_handle(cx));
    assert_eq!(focused, Some(first_focus), "the original pane took focus");
}

/// The observer hook's conversation id is authoritative. If upstream opens a
/// different conversation instead of the requested one, the workbench says
/// the exact resume failed and leaves the newly opened conversation alone.
#[gpui_kit::test]
fn exact_resume_reports_a_different_upstream_conversation(cx: &mut TestAppContext) {
    let (app, mut vcx, mut stream) = crate::ui::app::test_window::harness_with_pane(cx);
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().canonicalize().unwrap();
    let view = make_idle_context(&app, &mut vcx, &mut stream, &cwd);
    let prepared = antigravity_prepared(cwd, "expected-conversation");
    app.update_in(&mut vcx, |app, window, cx| {
        app.run_history_prepared_when_ready(
            crate::ui::pane::PaneSlot::Ready(view.clone()),
            prepared,
            window,
            cx,
        );
    });
    vcx.run_until_parked();

    DaemonMsg::Agent(Some(CLIAgent::Antigravity))
        .encode(&mut stream)
        .unwrap();
    DaemonMsg::AgentStatus(Some(Box::new(AgentSessionState {
        session_id: Some("different-conversation".into()),
        ..Default::default()
    })))
    .encode(&mut stream)
    .unwrap();
    pump_until("the different conversation identity lands", || {
        view.read_with(&mut vcx, |view, _| {
            view.agent() == Some(CLIAgent::Antigravity)
                && view
                    .agent_session()
                    .and_then(|state| state.session_id)
                    .as_deref()
                    == Some("different-conversation")
        })
    });
    view.update_in(&mut vcx, |view, window, cx| {
        view.poll_agent_status(false, window, cx);
    });
    settle(&mut vcx, "the failed exact resume is reported", |cx| {
        crate::ui::notification_history::history(cx)
            .read(cx)
            .records()
            .any(|record| {
                record.message().contains("expected-conversation")
                    && record.message().contains("different-conversation")
            })
    });
}
