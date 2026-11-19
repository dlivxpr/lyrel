//! In-flight diff probe tracking, extracted from `LyrelApp` (#336).
//!
//! A diff probe is one background read of one patch — `(host, cwd, source)` —
//! dispatched through `HostOps::run`. This module owns the policy that used
//! to live in two raw `HashSet`s on the app: which probes are in flight,
//! which keys were invalidated while their probe was out, and which landings
//! may install at all. The app keeps the two seams the tracker cannot hold —
//! the registry lookup and the spawn itself — and hands it their results.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use lyrel_ui_base::host_ops::{HostId, SharedHost};

use crate::ui::scm::diff::DiffSource;

/// The structured identity of one diff probe.
///
/// Replaces the string-mangled `PathBuf` key the de-duplication sets used to
/// share, with the source smuggled inside the path behind a `\u{1}`
/// separator: a real path containing that byte — legal on every host we run
/// on — would have collided with another key. `DiffSource` already hashes and
/// compares by `tag()`, so the three parts split apart here with nothing
/// encoded through strings, and the labelled and unlabelled spelling of one
/// commit stay one probe, the way `DiffSource`'s own `PartialEq` intends.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct DiffProbeKey {
    pub(crate) host_id: HostId,
    pub(crate) cwd: PathBuf,
    pub(crate) source: DiffSource,
}

impl DiffProbeKey {
    pub(crate) fn new(host_id: HostId, cwd: &Path, source: &DiffSource) -> Self {
        DiffProbeKey {
            host_id,
            cwd: cwd.to_path_buf(),
            source: source.clone(),
        }
    }
}

/// What [`DiffProbeTracker::request`] decided about one refresh request.
pub(crate) enum ProbeRequest {
    /// No probe for this key is out on this host: spawn one.
    Spawn,
    /// The same `SharedHost` instance already has a probe out for this key.
    /// The request was folded into the single pending follow-up (`restale`);
    /// an invalidation arriving mid-flight costs no git call of its own.
    Coalesce,
}

/// What [`DiffProbeTracker::complete`] decided about one landing probe.
pub(crate) enum ProbeCompletion {
    /// The probe is still the registry's live host and its key's active
    /// entry: deliver its snapshot. `follow_up` is the live host the one
    /// pending invalidation must be re-probed on, if there is one.
    Deliver { follow_up: Option<SharedHost> },
    /// A retired `SharedHost` or a superseded probe: drop the result entirely
    /// — no install, no in-flight state touched, no retry on the retired
    /// host.
    Reject,
}

/// Which diff probes are out and which of them owe a follow-up refresh.
///
/// The entry a key maps to is the exact `SharedHost` instance the probe is
/// running on, not just the `HostId`: a reconnect hands the same `HostId` to
/// a new host while the old probe may still be out, and only
/// [`Arc::ptr_eq`] against the live registry instance separates "same probe
/// asked for again" from "a new host that must probe now and a late landing
/// it must never see overwritten by".
#[derive(Default)]
pub(crate) struct DiffProbeTracker {
    /// In-flight probes by key.
    inflight: HashMap<DiffProbeKey, SharedHost>,
    /// Keys that were invalidated while their probe was in flight. Set by
    /// every coalesced request, cleared as the follow-up probe spawns: N
    /// invalidations during one flight collapse into exactly one refresh.
    restale: HashSet<DiffProbeKey>,
}

impl DiffProbeTracker {
    /// Decide what a refresh request for `(host, cwd, source)` should do.
    /// The caller spawns only on [`ProbeRequest::Spawn`], and only after
    /// verifying `host` is still the live instance in the registry — this
    /// tracker deliberately knows nothing about either seam.
    pub(crate) fn request(
        &mut self,
        host: &SharedHost,
        cwd: &Path,
        source: &DiffSource,
    ) -> ProbeRequest {
        let key = DiffProbeKey::new(host.id(), cwd, source);
        match self.inflight.get(&key) {
            Some(active) if Arc::ptr_eq(active, host) => {
                // The same probe is still out. Fold this invalidation into
                // the one pending follow-up instead of overlapping git calls.
                self.restale.insert(key);
                ProbeRequest::Coalesce
            }
            Some(_) => {
                // The key is in flight on a different `SharedHost` instance: a
                // reconnect took the HostId over while the old probe was out.
                // Supersede the old entry so the new host probes immediately;
                // the retired probe's late landing is rejected by `complete`.
                // A pending refresh dies with the retired probe — this probe
                // answers everything the old one could have, and anything that
                // changes while it is out marks `restale` again.
                self.inflight.insert(key.clone(), Arc::clone(host));
                self.restale.remove(&key);
                ProbeRequest::Spawn
            }
            None => {
                self.inflight.insert(key, Arc::clone(host));
                ProbeRequest::Spawn
            }
        }
    }

    /// Decide what a probe that finished on the background worker may do.
    /// `live` is the registry's current instance for the key's `HostId`
    /// (`HostRegistry::lookup` at the landing site), passed in so tests can
    /// exercise every branch without a GPUI context.
    pub(crate) fn complete(
        &mut self,
        key: &DiffProbeKey,
        probe: &SharedHost,
        live: Option<&SharedHost>,
    ) -> ProbeCompletion {
        if !self
            .inflight
            .get(key)
            .is_some_and(|active| Arc::ptr_eq(active, probe))
        {
            // Superseded: the entry now belongs to a newer host's probe (or is
            // gone because that probe already landed). Leave the entry alone —
            // clearing it would strand the new probe — and drop the result.
            // No retry either: the registry no longer holds this host.
            return ProbeCompletion::Reject;
        }
        self.inflight.remove(key);
        let Some(current) = live.filter(|current| Arc::ptr_eq(current, probe)) else {
            // The probe's host retired while it was out: its snapshot answers
            // a connection nobody is on anymore. Reject the result and drop
            // the pending refresh with it — the live host's next request
            // spawns its own probe.
            self.restale.remove(key);
            return ProbeCompletion::Reject;
        };
        let follow_up = self.restale.remove(key).then(|| Arc::clone(current));
        ProbeCompletion::Deliver { follow_up }
    }

    #[cfg(test)]
    fn is_restale(&self, key: &DiffProbeKey) -> bool {
        self.restale.contains(key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lyrel_core::host::local::LocalHost;

    #[test]
    fn the_probe_key_separates_the_sources_over_one_directory() {
        let host = HostId::LOCAL;
        let cwd = Path::new("/repo");
        let worktree = DiffProbeKey::new(host, cwd, &DiffSource::Worktree);
        assert_ne!(worktree, DiffProbeKey::new(host, cwd, &DiffSource::Staged));
        assert_ne!(worktree, DiffProbeKey::new(host, cwd, &DiffSource::Head));
        assert_ne!(
            DiffProbeKey::new(host, cwd, &DiffSource::commit("a")),
            DiffProbeKey::new(host, cwd, &DiffSource::commit("b")),
            "two commits are two probes"
        );
        assert_eq!(
            DiffProbeKey::new(host, cwd, &DiffSource::commit("a")),
            DiffProbeKey::new(host, cwd, &DiffSource::commit("a"))
        );
        assert_ne!(
            worktree,
            DiffProbeKey::new(host, Path::new("/other"), &DiffSource::Worktree)
        );
        assert_ne!(
            worktree,
            DiffProbeKey::new(
                HostId::from_connection_key("ssh-direct:me@box:22"),
                cwd,
                &DiffSource::Worktree
            ),
            "two hosts over one directory are two probes"
        );
        // One commit is one probe however much is known about it: the label
        // is how the patch is introduced, not which patch it is. The old
        // string-mangled key was built from `Debug` once, which split the
        // labelled and unlabelled spellings into two in-flight probes for one
        // patch; `DiffSource::tag()` is what `PartialEq` itself compares by.
        assert_eq!(
            DiffProbeKey::new(host, cwd, &DiffSource::commit("a")),
            DiffProbeKey::new(
                host,
                cwd,
                &DiffSource::Commit {
                    rev: "a".into(),
                    label: Some(crate::ui::scm::diff::CommitLabel {
                        subject: "s".into(),
                        author: "Ada".into(),
                        at: 1,
                    }),
                }
            )
        );
    }

    #[test]
    fn invalidations_during_one_flight_coalesce_into_a_single_follow_up() {
        let mut probes = DiffProbeTracker::default();
        let host = LocalHost::new();
        let cwd = Path::new("/repo");
        let source = DiffSource::Worktree;
        let key = DiffProbeKey::new(host.id(), cwd, &source);

        assert!(matches!(
            probes.request(&host, cwd, &source),
            ProbeRequest::Spawn
        ));
        for nth in 2..=4 {
            assert!(
                matches!(probes.request(&host, cwd, &source), ProbeRequest::Coalesce),
                "invalidation #{nth} during the flight must coalesce"
            );
        }
        assert!(probes.is_restale(&key));

        match probes.complete(&key, &host, Some(&host)) {
            ProbeCompletion::Deliver { follow_up } => {
                let next = follow_up.expect("one pending follow-up, however many arrived");
                assert!(
                    Arc::ptr_eq(&next, &host),
                    "the follow-up probes on the same live host"
                );
            }
            ProbeCompletion::Reject => panic!("the live probe's landing must deliver"),
        }
        assert!(probes.inflight.is_empty());
        assert!(!probes.is_restale(&key));

        // The follow-up is the second and last probe: it spawns once and its
        // own landing owes nothing.
        assert!(matches!(
            probes.request(&host, cwd, &source),
            ProbeRequest::Spawn
        ));
        match probes.complete(&key, &host, Some(&host)) {
            ProbeCompletion::Deliver { follow_up } => {
                assert!(follow_up.is_none())
            }
            ProbeCompletion::Reject => panic!("the follow-up's landing must deliver"),
        }
        assert!(probes.inflight.is_empty());
    }

    #[test]
    fn a_reconnect_supersedes_the_retired_probe_and_rejects_its_late_landing() {
        let mut probes = DiffProbeTracker::default();
        // Two distinct `SharedHost` instances under one `HostId`: the shape a
        // reconnect leaves behind.
        let retired = LocalHost::new();
        let live = LocalHost::new();
        assert_eq!(retired.id(), live.id());
        assert!(!Arc::ptr_eq(&retired, &live));
        let cwd = Path::new("/repo");
        let source = DiffSource::Worktree;
        let key = DiffProbeKey::new(retired.id(), cwd, &source);

        assert!(matches!(
            probes.request(&retired, cwd, &source),
            ProbeRequest::Spawn
        ));
        // An invalidation arrives while the retired host's probe is out.
        assert!(matches!(
            probes.request(&retired, cwd, &source),
            ProbeRequest::Coalesce
        ));
        // The reconnect's first refresh supersedes: the new host probes now,
        // and the stale pending refresh dies with the probe it was booked on.
        assert!(matches!(
            probes.request(&live, cwd, &source),
            ProbeRequest::Spawn
        ));
        assert!(!probes.is_restale(&key));

        // The retired probe lands late. It must not deliver, must not clear
        // the new host's in-flight entry, and must not schedule a retry.
        assert!(
            matches!(
                probes.complete(&key, &retired, Some(&live)),
                ProbeCompletion::Reject
            ),
            "a retired host's late landing is rejected"
        );
        assert!(
            probes
                .inflight
                .get(&key)
                .is_some_and(|active| Arc::ptr_eq(active, &live)),
            "the new host's in-flight entry survives the retired probe"
        );

        // The live probe's landing delivers — with no follow-up, the
        // superseded invalidation having been answered twice over.
        match probes.complete(&key, &live, Some(&live)) {
            ProbeCompletion::Deliver { follow_up } => assert!(follow_up.is_none()),
            ProbeCompletion::Reject => panic!("the live probe's landing must deliver"),
        }
        assert!(probes.inflight.is_empty());
    }

    #[test]
    fn a_probe_whose_host_left_the_registry_entirely_is_rejected() {
        let mut probes = DiffProbeTracker::default();
        let host = LocalHost::new();
        let cwd = Path::new("/repo");
        let source = DiffSource::Staged;
        let key = DiffProbeKey::new(host.id(), cwd, &source);

        assert!(matches!(
            probes.request(&host, cwd, &source),
            ProbeRequest::Spawn
        ));
        assert!(matches!(
            probes.request(&host, cwd, &source),
            ProbeRequest::Coalesce
        ));
        assert!(matches!(
            probes.complete(&key, &host, None),
            ProbeCompletion::Reject
        ));
        assert!(
            probes.inflight.is_empty(),
            "the retired entry is cleared, not left to block a future reconnect"
        );
        assert!(!probes.is_restale(&key));
    }

    #[test]
    fn independent_sources_are_keyed_independently_and_never_cancel_one_another() {
        let mut probes = DiffProbeTracker::default();
        let host = LocalHost::new();
        let cwd = Path::new("/repo");
        let sources = [
            DiffSource::Worktree,
            DiffSource::Staged,
            DiffSource::Head,
            DiffSource::commit("a1b2c3"),
            DiffSource::Range {
                base: "main".into(),
                head: "feature".into(),
            },
        ];
        for source in &sources {
            assert!(
                matches!(probes.request(&host, cwd, source), ProbeRequest::Spawn),
                "{source:?} must get its own probe"
            );
        }
        assert_eq!(
            probes.inflight.len(),
            sources.len(),
            "five sources, five coexisting probes"
        );
        // A second request for any one of them coalesces on its own probe
        // without touching the other four.
        assert!(matches!(
            probes.request(&host, cwd, &DiffSource::Staged),
            ProbeRequest::Coalesce
        ));
        assert_eq!(probes.inflight.len(), sources.len());
        for source in &sources {
            let key = DiffProbeKey::new(host.id(), cwd, source);
            match probes.complete(&key, &host, Some(&host)) {
                ProbeCompletion::Deliver { follow_up } => {
                    // Only the staged probe carried a coalesced invalidation.
                    let want_follow_up = (*source == DiffSource::Staged).then(|| Arc::clone(&host));
                    assert_eq!(follow_up.is_some(), want_follow_up.is_some());
                }
                ProbeCompletion::Reject => panic!("{source:?} must deliver"),
            }
        }
        assert!(probes.inflight.is_empty());
    }
}

#[cfg(test)]
mod gpui_tests {
    use super::*;
    use crate::ui::app::LyrelApp;
    use crate::ui::app::test_window;
    use crate::ui::diff_overlay::DiffLoad;
    use gpui_kit::{Entity, TestAppContext, VisualTestContext};
    use lyrel_core::host::local::LocalHost;
    use lyrel_core::host::{
        ContentLimits, ContentQuery, ContentResults, Entry, Host, Meta, Output, SearchHit,
    };
    use lyrel_ui_base::host_registry::HostRegistry;
    use std::io;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    /// A one-shot hold plus the channels a test waits on and releases
    /// through, handed out by [`ProbeGateHost::arm`].
    struct ReadGate {
        entered: mpsc::Receiver<()>,
        release: mpsc::Sender<()>,
        finished: mpsc::Receiver<()>,
    }

    impl ReadGate {
        /// Pump the window until the gated probe is blocked inside its
        /// `git diff` call.
        fn wait_for_probe(&self, vcx: &mut VisualTestContext) {
            pump_until_channel_sends(vcx, &self.entered, "the probe never reached the gate");
        }

        /// Pump the window until the gated probe's worker-side read finished.
        /// The landing task itself is waited on with `settle`, since a
        /// rejected landing changes nothing a condition could observe.
        fn wait_for_result(&self, vcx: &mut VisualTestContext) {
            pump_until_channel_sends(vcx, &self.finished, "the gated probe never finished");
        }

        fn release(&self) {
            let _ = self.release.send(());
        }
    }

    fn pump_until_channel_sends(
        vcx: &mut VisualTestContext,
        rx: &mpsc::Receiver<()>,
        timeout_message: &str,
    ) {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            vcx.executor().advance_clock(Duration::from_millis(300));
            vcx.run_until_parked();
            match rx.recv_timeout(Duration::from_millis(50)) {
                Ok(()) => return,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    assert!(Instant::now() < deadline, "{timeout_message}")
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    panic!("the gate's probe thread died: {timeout_message}")
                }
            }
        }
    }

    /// Counts the `git diff` probes that pass through and can hold the next
    /// one at the diff argv until a test releases it — the controlled-adapter
    /// shape of `scm::data::lifetime_tests::GatedHost`, retargeted at diff
    /// reads so a probe stays genuinely in flight for as long as a test
    /// needs. Only scheduling changes: every read runs on the real local
    /// Host, so the bytes a probe returns are the tree's own.
    struct ProbeGateHost {
        inner: SharedHost,
        id: HostId,
        diff_calls: AtomicUsize,
        gate: std::sync::Mutex<Option<(mpsc::Sender<()>, mpsc::Receiver<()>, mpsc::Sender<()>)>>,
    }

    impl ProbeGateHost {
        fn new(id: HostId) -> Arc<Self> {
            Arc::new(Self {
                inner: LocalHost::new(),
                id,
                diff_calls: AtomicUsize::new(0),
                gate: std::sync::Mutex::new(None),
            })
        }

        /// Arm the one-shot gate the next `git diff` call blocks on.
        fn arm(&self) -> ReadGate {
            let (entered_tx, entered) = mpsc::channel();
            let (release, release_rx) = mpsc::channel();
            let (finished_tx, finished) = mpsc::channel();
            *self.gate.lock().unwrap() = Some((entered_tx, release_rx, finished_tx));
            ReadGate {
                entered,
                release,
                finished,
            }
        }

        fn diff_count(&self) -> usize {
            self.diff_calls.load(Ordering::SeqCst)
        }
    }

    impl Host for ProbeGateHost {
        fn id(&self) -> HostId {
            self.id
        }
        fn separator(&self) -> char {
            self.inner.separator()
        }
        fn is_absolute(&self, p: &Path) -> bool {
            self.inner.is_absolute(p)
        }
        fn read_dir(&self, p: &Path, root: Option<&Path>) -> io::Result<Vec<Entry>> {
            self.inner.read_dir(p, root)
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
            max_dirs: usize,
            hidden: bool,
        ) -> io::Result<Vec<SearchHit>> {
            self.inner.search(roots, query, limit, max_dirs, hidden)
        }
        fn search_content(
            &self,
            roots: &[PathBuf],
            query: &ContentQuery,
            limits: &ContentLimits,
        ) -> io::Result<ContentResults> {
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
        fn git(&self, p: &Path, args: &[&str]) -> io::Result<Output> {
            if args.contains(&"diff") {
                self.diff_calls.fetch_add(1, Ordering::SeqCst);
                // Hold before the read itself, so what the probe eventually
                // returns is the tree as it stands at release time — that is
                // what makes a late landing observably different from the
                // fresh snapshot it must not overwrite.
                let gate = self.gate.lock().unwrap().take();
                if let Some((entered, release, finished)) = gate {
                    let _ = entered.send(());
                    if release.recv_timeout(Duration::from_secs(30)).is_err() {
                        return Err(io::Error::other("the test gate was never released"));
                    }
                    let out = self.inner.git(p, args);
                    // Worker-side proof the read finished; the landing task
                    // follows on the UI thread and is given pump rounds by
                    // the test's `settle`.
                    let _ = finished.send(());
                    return out;
                }
            }
            self.inner.git(p, args)
        }
        fn shells(&self) -> io::Result<lyrel_core::core::shells::ShellInventory> {
            self.inner.shells()
        }
        fn watch(&self, paths: &[PathBuf]) -> io::Result<lyrel_ui_base::host_ops::WatchSub> {
            self.inner.watch(paths)
        }
    }

    fn repository() -> (tempfile::TempDir, PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let root = lyrel_core::core::path_spelling::local_spelling_buf(
            std::fs::canonicalize(temp.path()).unwrap(),
        );
        let output = std::process::Command::new("git")
            .args(["init", "--quiet"])
            .current_dir(&root)
            .output()
            .unwrap();
        assert!(output.status.success());
        std::fs::write(root.join("before.txt"), "before\n").unwrap();
        (temp, root)
    }

    fn pump_until(
        app: &Entity<LyrelApp>,
        vcx: &mut VisualTestContext,
        mut done: impl FnMut(&LyrelApp) -> bool,
    ) {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            vcx.executor().advance_clock(Duration::from_millis(300));
            vcx.run_until_parked();
            if app.update_in(vcx, |app, _, _| done(app)) {
                return;
            }
            assert!(Instant::now() < deadline, "condition did not settle");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn settle(vcx: &mut VisualTestContext) {
        for _ in 0..5 {
            vcx.executor().advance_clock(Duration::from_millis(300));
            vcx.run_until_parked();
        }
    }

    fn landed_snapshot(
        app: &Entity<LyrelApp>,
        vcx: &mut VisualTestContext,
    ) -> Option<Arc<crate::ui::scm::diff::DiffSnapshot>> {
        app.update_in(vcx, |app, _, _| {
            let overlay = app.tabs[app.tabs.active_index()]
                .diff_overlay
                .as_ref()
                .expect("an overlay is open");
            match &overlay.load {
                DiffLoad::Ready(snap) => Some(Arc::clone(snap)),
                _ => None,
            }
        })
    }

    #[gpui_kit::test]
    fn invalidations_during_one_gated_probe_coalesce_into_one_follow_up(cx: &mut TestAppContext) {
        let (_temp, root) = repository();
        let (app, mut vcx, _streams) = test_window::harness_with_tabs(cx, 1);
        let host = ProbeGateHost::new(HostId::from_connection_key("diff-probe-coalesce"));
        vcx.update(|_, cx| {
            HostRegistry::insert(cx, host.clone());
        });

        let gate = host.arm();
        app.update_in(&mut vcx, |app, window, cx| {
            app.open_diff_overlay(
                host.id(),
                root.clone(),
                DiffSource::Worktree,
                None,
                window,
                cx,
            );
        });
        gate.wait_for_probe(&mut vcx);
        assert_eq!(
            host.diff_count(),
            1,
            "the overlay's first probe is the one held at the gate"
        );

        // Three invalidations arrive while the probe is in flight, through the
        // same `spawn_diff_probe_for` path a refresh trigger uses.
        for nth in 1..=3 {
            app.update_in(&mut vcx, |app, _, cx| {
                app.spawn_diff_probe_for(host.clone(), root.clone(), DiffSource::Worktree, cx);
            });
            assert_eq!(
                host.diff_count(),
                1,
                "invalidation #{nth} must not spawn an overlapping probe"
            );
        }
        gate.release();

        // Both probes have landed — the original and exactly one follow-up —
        // and nothing is left in flight or loading.
        pump_until(&app, &mut vcx, |app| {
            app.diff_probes.inflight.is_empty()
                && app.tabs[app.tabs.active_index()]
                    .diff_overlay
                    .as_ref()
                    .is_some_and(|o| !o.loading && matches!(&o.load, DiffLoad::Ready(_)))
                && host.diff_count() == 2
        });
        assert!(
            landed_snapshot(&app, &mut vcx).is_some(),
            "the follow-up's snapshot is installed"
        );
    }

    #[gpui_kit::test]
    fn a_reconnect_rejects_the_retired_hosts_late_landing(cx: &mut TestAppContext) {
        let (_temp, root) = repository();
        let (app, mut vcx, _streams) = test_window::harness_with_tabs(cx, 1);
        let id = HostId::from_connection_key("diff-probe-reconnect");
        let retired = ProbeGateHost::new(id);
        vcx.update(|_, cx| {
            HostRegistry::insert(cx, retired.clone());
        });

        let gate = retired.arm();
        app.update_in(&mut vcx, |app, window, cx| {
            app.open_diff_overlay(id, root.clone(), DiffSource::Worktree, None, window, cx);
        });
        gate.wait_for_probe(&mut vcx);

        // Reconnect under the same HostId with a fresh host instance: the new
        // host's probe supersedes the retired one's in-flight entry.
        let live = ProbeGateHost::new(id);
        assert!(!Arc::ptr_eq(&live, &retired));
        vcx.update(|_, cx| {
            HostRegistry::insert(cx, live.clone());
        });
        app.update_in(&mut vcx, |app, _, cx| {
            app.spawn_diff_probe_for(live.clone(), root.clone(), DiffSource::Worktree, cx);
        });
        pump_until(&app, &mut vcx, |app| {
            live.diff_count() == 1 && app.diff_probes.inflight.is_empty()
        });
        let fresh = landed_snapshot(&app, &mut vcx).expect("the live probe's snapshot installed");
        assert_eq!(fresh.untracked, vec!["before.txt".to_string()]);

        // The tree changes while the retired probe is still blocked at its
        // diff: the answer it eventually produces is one nobody may install.
        std::fs::write(root.join("late.txt"), "late\n").unwrap();
        gate.release();
        gate.wait_for_result(&mut vcx);
        settle(&mut vcx);

        let after = landed_snapshot(&app, &mut vcx).expect("the fresh snapshot survives");
        assert_eq!(
            after.untracked,
            vec!["before.txt".to_string()],
            "the retired host's late landing must not overwrite the fresh snapshot"
        );
        assert!(
            app.update_in(&mut vcx, |app, _, _| app.diff_probes.inflight.is_empty()),
            "the rejected landing left nothing in flight"
        );
        assert_eq!(
            retired.diff_count(),
            1,
            "a rejected landing never retries on the retired host"
        );
        assert_eq!(live.diff_count(), 1, "the live host probed exactly once");
    }

    #[gpui_kit::test]
    fn independent_sources_over_one_directory_spawn_independent_probes(cx: &mut TestAppContext) {
        let (_temp, root) = repository();
        let (app, mut vcx, _streams) = test_window::harness_with_tabs(cx, 1);
        let host = ProbeGateHost::new(HostId::from_connection_key("diff-probe-sources"));
        vcx.update(|_, cx| {
            HostRegistry::insert(cx, host.clone());
        });

        for source in [DiffSource::Worktree, DiffSource::Staged, DiffSource::Head] {
            app.update_in(&mut vcx, |app, _, cx| {
                app.spawn_diff_probe_for(host.clone(), root.clone(), source, cx);
            });
        }
        pump_until(&app, &mut vcx, |app| {
            host.diff_count() == 3 && app.diff_probes.inflight.is_empty()
        });
    }
}
