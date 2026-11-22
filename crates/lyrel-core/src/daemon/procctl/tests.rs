//! Verified End Process regressions against real, disposable processes
//! (#372). The fixture re-runs this test binary in a child role that sleeps —
//! the self-re-exec seam `pane::lifecycle::tests` already uses — so building
//! a parent-with-children tree needs no shell and no platform binary, on any
//! OS. Every child here is one of these fixtures and nothing else on the
//! machine is touched: the tests only ever signal pids that descend from
//! this process, which stands in for the pane's shell the way the daemon's
//! registry does.

use super::*;
use std::io::{Read as _, Write as _};
#[cfg(any(windows, target_os = "linux"))]
use std::net::TcpListener;
use std::net::{SocketAddr, TcpStream};
use std::process::{Child, Command};
use std::time::Duration;
#[cfg(any(windows, target_os = "linux"))]
use std::time::Instant;

/// Role markers in the child environment. `shell` spawns `sleepers` and
/// then sleeps; `sleep` just sleeps.
const ROLE: &str = "LYREL_PROCCTL_FIXTURE";
const SLEEPERS: &str = "LYREL_PROCCTL_SLEEPERS";
const LIFETIME: &str = "LYREL_PROCCTL_LIFETIME";

#[cfg(windows)]
#[test]
fn a_reused_parent_pid_does_not_own_the_older_child() {
    let parent_of = |pid| match pid {
        23 => Some(1),
        24 => Some(23),
        25 => Some(24),
        _ => None,
    };

    let started = |pid| match pid {
        23 => Some(20),
        24 => Some(10),
        _ => None,
    };
    assert_eq!(
        windows_ownership(23, 25, 15, parent_of, started),
        Some(false)
    );
    assert_eq!(
        windows_ownership(23, 25, 15, parent_of, |pid| if pid == 23 {
            Some(5)
        } else {
            started(pid)
        }),
        Some(true)
    );
    assert_eq!(
        windows_ownership(23, 25, 15, parent_of, |pid| (pid == 24).then_some(10)),
        None
    );
    assert_eq!(
        windows_ownership(99, 25, 15, parent_of, |_| None),
        Some(false),
        "unreadable birth times outside the pane tree do not change the refusal"
    );
}

/// When re-run as a fixture child, live in that role and never return.
fn fixture_role() -> bool {
    let Some(role) = std::env::var_os(ROLE) else {
        return false;
    };
    if role == "shell" {
        let sleepers = std::env::var(SLEEPERS)
            .ok()
            .and_then(|n| n.parse::<usize>().ok())
            .unwrap_or(0);
        for _ in 0..sleepers {
            let _ = fixture_child("sleep", 0, fixture_lifetime());
        }
    }
    if role == "sleep"
        && let Some(address) = fixture_lifetime()
    {
        // The connection keeps this sleeper alive even when the test ends
        // its immediate parent. An unwind or process exit closes the other
        // end, including when setup failed before accept.
        if let Ok(mut connection) = TcpStream::connect_timeout(&address, Duration::from_secs(5))
            && connection
                .write_all(&std::process::id().to_le_bytes())
                .is_ok()
        {
            let _ = connection.read(&mut [0]);
        }
        std::process::exit(0);
    }
    loop {
        std::thread::sleep(Duration::from_secs(3600));
    }
}

fn fixture_lifetime() -> Option<SocketAddr> {
    std::env::var(LIFETIME)
        .ok()
        .map(|value| value.parse().unwrap())
}

fn fixture_child(role: &str, sleepers: usize, lifetime: Option<SocketAddr>) -> Child {
    let mut command = Command::new(std::env::current_exe().expect("the test binary"));
    if let Some(lifetime) = lifetime {
        command.env(LIFETIME, lifetime.to_string());
    }
    command
        .arg("--exact")
        .arg(
            std::thread::current()
                .name()
                .unwrap_or("procctl fixture")
                .to_string(),
        )
        .env(ROLE, role)
        .env(SLEEPERS, sleepers.to_string())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn a fixture child")
}

/// This process's own id stands in for the pane's shell — exactly the
/// relationship the daemon has with its pane: the processes being verified
/// and signaled all descend from the root it vouches for.
fn shell_pid() -> u32 {
    std::process::id()
}

fn identity_of(pid: u32) -> ProcIdentity {
    ProcIdentity {
        pid,
        started: crate::daemon::procstat::read(pid).started,
    }
}

/// Whether `pid` names a live process this user can inspect. Opens the same
/// binding `execute` uses and asks whether the process object already
/// exited: a killed-but-unreaped process stays openable, and its creation
/// time stays readable, on both Windows and Linux — "opens" is not "lives".
#[cfg(any(windows, target_os = "linux"))]
fn alive(pid: u32) -> bool {
    bind::Bound::open(pid).is_some_and(|bound| !bound.exited())
}

/// A fixture shell with `sleepers` children, killed and reaped on drop so a
/// failing assertion cannot leak sleepers into the next test.
#[cfg(any(windows, target_os = "linux"))]
struct Tree {
    shell: Child,
    children: Vec<u32>,
    lifetimes: Vec<(TcpStream, bind::Bound)>,
}

#[cfg(any(windows, target_os = "linux"))]
impl Tree {
    fn spawn(sleepers: usize) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let mut tree = Self {
            shell: fixture_child("shell", sleepers, Some(listener.local_addr().unwrap())),
            children: Vec::with_capacity(sleepers),
            lifetimes: Vec::with_capacity(sleepers),
        };
        let deadline = Instant::now() + Duration::from_secs(10);
        while tree.children.len() < sleepers {
            let remaining = deadline.saturating_duration_since(Instant::now());
            assert!(!remaining.is_zero(), "fixture sleepers did not register");
            match listener.accept() {
                Ok((mut stream, _)) => {
                    stream.set_read_timeout(Some(remaining)).unwrap();
                    let mut bytes = [0; 4];
                    stream.read_exact(&mut bytes).unwrap();
                    let pid = u32::from_le_bytes(bytes);
                    let bound = bind::Bound::open(pid).expect("the registered sleeper is live");
                    tree.children.push(pid);
                    tree.lifetimes.push((stream, bound));
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("cannot accept a fixture sleeper: {error}"),
            }
        }
        tree
    }

    fn shell_pid(&self) -> u32 {
        self.shell.id()
    }

    fn identity(&self, pid: u32) -> ProcIdentity {
        assert!(alive(pid), "fixture member {pid} must be live");
        identity_of(pid)
    }

    fn plan(&self) -> EndProcessPlan {
        let plan = plan(
            shell_pid(),
            self.shell_pid(),
            Some(
                identity_of(self.shell_pid())
                    .started
                    .expect("the shell has a birth stamp"),
            ),
        )
        .expect("the fixture tree must plan");
        assert!(
            plan.complete,
            "three processes are nowhere near the walk cap"
        );
        plan
    }
}

#[cfg(any(windows, target_os = "linux"))]
impl Drop for Tree {
    fn drop(&mut self) {
        // Cleanup uses only handles held by the test, including sleepers
        // orphaned by the single-process termination case.
        let _ = self.shell.kill();
        let _ = self.shell.wait();
        for (stream, bound) in self.lifetimes.drain(..) {
            drop(stream);
            let _ = bound.wait_gone(Duration::from_secs(2));
        }
    }
}

#[cfg(any(windows, target_os = "linux"))]
#[test]
fn plan_refuses_an_identity_without_a_birth_stamp() {
    if fixture_role() {
        return;
    }
    let tree = Tree::spawn(1);
    assert_eq!(
        plan(shell_pid(), tree.shell_pid(), None),
        Err(PlanRefusal::Unverifiable),
        "a bare pid names whatever the kernel reused the number for last"
    );
}

#[cfg(any(windows, target_os = "linux"))]
#[test]
fn plan_refuses_a_stale_birth_stamp_and_leaves_the_process_alive() {
    if fixture_role() {
        return;
    }
    let tree = Tree::spawn(0);
    let stale = ProcIdentity {
        pid: tree.shell_pid(),
        started: Some(identity_of(tree.shell_pid()).started.unwrap() ^ 1),
    };
    assert_eq!(
        plan(shell_pid(), stale.pid, stale.started),
        Err(PlanRefusal::Stale),
        "a live process whose stamp does not match is a reused number, not a target"
    );
    assert!(alive(tree.shell_pid()), "refusing must not signal anything");
}

#[cfg(any(windows, target_os = "linux"))]
#[test]
fn plan_refuses_a_process_outside_the_shell_tree() {
    if fixture_role() {
        return;
    }
    // Two sibling fixture shells: neither descends from the other, so each
    // is outside the other's tree — the shape of a pid that wandered between
    // panes, or never belonged to one.
    let a = Tree::spawn(0);
    let b = Tree::spawn(0);
    assert_eq!(
        plan(
            a.shell_pid(),
            b.shell_pid(),
            Some(identity_of(b.shell_pid()).started.unwrap())
        ),
        Err(PlanRefusal::NotOwned)
    );
    assert!(alive(b.shell_pid()));
}

#[cfg(any(windows, target_os = "linux"))]
#[test]
fn plan_refuses_a_process_that_already_exited() {
    if fixture_role() {
        return;
    }
    let mut child = fixture_child("sleep", 0, None);
    let pid = child.id();
    let started = identity_of(pid).started;
    child.kill().expect("kill the fixture child");
    child.wait().expect("reap the fixture child");
    assert_eq!(
        plan(shell_pid(), pid, started),
        Err(PlanRefusal::Gone),
        "a number whose process is gone names nothing to end"
    );
}

#[cfg(any(windows, target_os = "linux"))]
#[test]
fn plan_names_the_complete_subtree_deepest_first() {
    if fixture_role() {
        return;
    }
    let tree = Tree::spawn(2);
    let plan = tree.plan();
    let pids: Vec<u32> = plan.targets.iter().map(|target| target.pid).collect();
    assert_eq!(
        pids.len(),
        3,
        "the scope covers the shell and both sleepers, not just what a row cap would show"
    );
    assert!(
        pids[..2].iter().all(|pid| tree.children.contains(pid)),
        "children come first: {pids:?}"
    );
    assert_eq!(pids[2], tree.shell_pid(), "the target itself is last");
    assert!(
        plan.targets.iter().all(|target| target.started.is_some()),
        "every planned row carries its identity into the confirmation"
    );
}

#[cfg(any(windows, target_os = "linux"))]
#[test]
fn ending_one_process_leaves_its_children_running() {
    if fixture_role() {
        return;
    }
    let mut tree = Tree::spawn(2);
    let id = tree.identity(tree.shell_pid());
    let outcomes = execute(shell_pid(), std::slice::from_ref(&id));
    assert_eq!(outcomes.len(), 1);
    assert_eq!(outcomes[0].state, EndProcessState::Ended);
    tree.shell.wait().expect("the ended shell reaps");
    for &child in &tree.children {
        assert!(
            alive(child),
            "single-target termination must not touch the children: {child}"
        );
    }
    // The tree's Drop now disposes of the orphaned sleepers.
}

#[cfg(any(windows, target_os = "linux"))]
#[test]
fn ending_the_tree_ends_every_confirmed_identity() {
    if fixture_role() {
        return;
    }
    let mut tree = Tree::spawn(2);
    let plan = tree.plan();
    let scope: Vec<ProcIdentity> = plan
        .targets
        .iter()
        .map(|target| ProcIdentity {
            pid: target.pid,
            started: target.started,
        })
        .collect();
    let outcomes = execute(shell_pid(), &scope);
    assert_eq!(outcomes.len(), scope.len());
    assert!(
        outcomes
            .iter()
            .all(|outcome| outcome.state == EndProcessState::Ended),
        "every confirmed identity ends and reports itself: {outcomes:?}"
    );
    tree.shell.wait().expect("the ended shell reaps");
    for &child in &tree.children {
        assert!(
            !alive(child),
            "tree termination covers the children: {child}"
        );
    }
}

#[cfg(any(windows, target_os = "linux"))]
#[test]
fn a_drifted_identity_is_refused_and_never_signaled() {
    if fixture_role() {
        return;
    }
    let tree = Tree::spawn(1);
    let child = tree.children[0];
    let mut drifted = tree.identity(child);
    drifted.started = Some(drifted.started.unwrap() ^ 1);
    let outcomes = execute(shell_pid(), &[drifted]);
    assert_eq!(outcomes.len(), 1);
    assert_eq!(outcomes[0].state, EndProcessState::Stale);
    assert!(
        alive(child),
        "a refused identity must survive: this is the reused-pid guarantee"
    );
}

#[cfg(any(windows, target_os = "linux"))]
#[test]
fn an_identity_that_left_the_tree_is_refused_and_never_signaled() {
    if fixture_role() {
        return;
    }
    let a = Tree::spawn(0);
    let b = Tree::spawn(1);
    let stranger = b.identity(b.children[0]);
    let outcomes = execute(a.shell_pid(), std::slice::from_ref(&stranger));
    assert_eq!(outcomes.len(), 1);
    assert_eq!(outcomes[0].state, EndProcessState::NotOwned);
    assert!(alive(stranger.pid), "someone else's process must survive");
}

#[cfg(any(windows, target_os = "linux"))]
#[test]
fn a_scope_is_a_ceiling_reexecution_reports_instead_of_expanding() {
    if fixture_role() {
        return;
    }
    let mut tree = Tree::spawn(2);
    let plan = tree.plan();
    let scope: Vec<ProcIdentity> = plan
        .targets
        .iter()
        .map(|target| ProcIdentity {
            pid: target.pid,
            started: target.started,
        })
        .collect();
    // End one child directly, the way a user ending a single row would.
    let removed = scope[1];
    let single = execute(shell_pid(), std::slice::from_ref(&removed));
    assert_eq!(single[0].state, EndProcessState::Ended);

    // Re-executing the confirmed scope afterwards must not re-signal the
    // gone child into existence or reach for anything new: it reports the
    // exited member as exited and ends the rest.
    let outcomes = execute(shell_pid(), &scope);
    assert_eq!(
        outcomes.len(),
        scope.len(),
        "one outcome per confirmed identity"
    );
    assert_eq!(outcomes[0].state, EndProcessState::Ended);
    assert_eq!(outcomes[1].state, EndProcessState::Exited);
    assert_eq!(outcomes[2].state, EndProcessState::Ended);
    tree.shell.wait().expect("the ended shell reaps");
}

#[cfg(any(windows, target_os = "linux"))]
#[test]
fn an_unverifiable_identity_in_the_scope_is_refused() {
    if fixture_role() {
        return;
    }
    let tree = Tree::spawn(0);
    let bare = ProcIdentity {
        pid: tree.shell_pid(),
        started: None,
    };
    let outcomes = execute(shell_pid(), std::slice::from_ref(&bare));
    assert_eq!(outcomes.len(), 1);
    assert_eq!(outcomes[0].state, EndProcessState::Unverifiable);
    assert!(alive(tree.shell_pid()));
}

/// Platforms whose native API cannot atomically bind a verified identity to
/// the termination handle or token fail closed: no scope is ever planned and
/// no identity is ever signaled, however trustworthy the row looks — and the
/// refusal is `Unverifiable`, never the "already exited" that a bare `open`
/// failure would report.
#[cfg(not(any(windows, target_os = "linux")))]
#[test]
fn platforms_without_an_atomic_bind_fail_closed() {
    if fixture_role() {
        return;
    }
    let mut child = fixture_child("sleep", 0, None);
    let identity = identity_of(child.id());
    let plan_result = plan(shell_pid(), identity.pid, identity.started);
    let outcomes = execute(shell_pid(), std::slice::from_ref(&identity));
    let status = child.try_wait();
    // The test owns this child handle. Reap through it before asserting so
    // cleanup never depends on the process control feature under test.
    let _ = child.kill();
    child.wait().expect("reap the fixture child");
    #[cfg(target_os = "macos")]
    assert!(identity.started.is_some(), "the fixture has a birth stamp");

    assert!(
        !bind::ATOMIC_BIND,
        "this platform is the fail-closed arm under test"
    );
    assert_eq!(
        plan_result,
        Err(PlanRefusal::Unverifiable),
        "no scope is planned on a platform that cannot vouch for one"
    );
    assert_eq!(outcomes.len(), 1);
    assert_eq!(
        outcomes[0].state,
        EndProcessState::Unverifiable,
        "each identity reports why it was not signaled"
    );
    assert!(
        status.expect("read the fixture child status").is_none(),
        "fail closed leaves the fixture child running"
    );
}
