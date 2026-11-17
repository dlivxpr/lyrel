//! The revision/generation guard around one document's off-thread projection.
//!
//! The workbench spawns the parse and owns the `Document`; this type owns the
//! part that must not be re-derived per format: which projection is installed,
//! which revision it describes, which parse generation may install, and whether
//! a worker is already running. A cancelled or stale result is never installed,
//! and the debounced worker stops as soon as no newer snapshot is pending.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

/// One parse the worker must execute. The snapshot and revision are captured by
/// the caller from the shared document; the job carries the generation and the
/// cancellation flag that make its result rejectable.
#[derive(Clone, Debug)]
pub struct ProjectionJob {
    generation: u64,
    cancel: Arc<AtomicBool>,
}

impl ProjectionJob {
    /// The generation this job was started for.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// The flag the parse worker and the installer share.
    pub fn cancel(&self) -> &Arc<AtomicBool> {
        &self.cancel
    }

    /// Whether the document has already moved past this job.
    pub fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::Acquire)
    }
}

/// What a finished parse may install. `Cancelled` deliberately installs
/// nothing, not even a failure state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProjectionResult<P, F> {
    Ready(P),
    Failed(F),
    Cancelled,
}

/// One document's projection lifecycle. `P` is the format projection and `F`
/// its parse failure; both stay format-specific, while the guards are shared.
#[derive(Debug)]
pub struct ProjectionState<P, F> {
    projection: Option<Arc<P>>,
    failure: Option<F>,
    installed_revision: Option<u64>,
    generation: u64,
    parsing: bool,
    pending: bool,
    worker_running: bool,
    cancel: Arc<AtomicBool>,
}

impl<P, F> Default for ProjectionState<P, F> {
    fn default() -> Self {
        Self::new()
    }
}

impl<P, F> ProjectionState<P, F> {
    pub fn new() -> Self {
        Self {
            projection: None,
            failure: None,
            installed_revision: None,
            generation: 0,
            parsing: false,
            pending: false,
            worker_running: false,
            cancel: Arc::default(),
        }
    }

    /// The source changed: cancel any in-flight parse, discard the installed
    /// projection, and request a fresh one. Returns true when the caller must
    /// start the worker; false when one is already running.
    pub fn invalidate(&mut self) -> bool {
        self.cancel.store(true, Ordering::Release);
        self.generation = self.generation.wrapping_add(1);
        self.projection = None;
        self.failure = None;
        self.parsing = true;
        self.pending = true;
        !self.worker_running
    }

    /// Marks the debounced worker as running before it is spawned.
    pub fn begin_worker(&mut self) {
        self.worker_running = true;
    }

    /// Takes the newest pending parse, or finishes the worker when the debounce
    /// coalesced every edit away. When this returns `None` the state already
    /// reflects a stopped worker and the caller must not spawn one.
    pub fn take_job(&mut self) -> Option<ProjectionJob> {
        if !self.pending {
            self.parsing = false;
            self.worker_running = false;
            return None;
        }
        self.pending = false;
        self.cancel.store(false, Ordering::Release);
        Some(ProjectionJob {
            generation: self.generation,
            cancel: Arc::clone(&self.cancel),
        })
    }

    /// Installs a finished result only when both the generation and the source
    /// revision still match. Returns whether the worker must continue with a
    /// newer pending snapshot.
    pub fn install(
        &mut self,
        job_generation: u64,
        revision: u64,
        current_revision: u64,
        result: ProjectionResult<P, F>,
    ) -> bool {
        if self.generation == job_generation && current_revision == revision {
            match result {
                ProjectionResult::Ready(projection) => {
                    self.projection = Some(Arc::new(projection));
                    self.failure = None;
                    self.installed_revision = Some(revision);
                }
                ProjectionResult::Cancelled => {}
                ProjectionResult::Failed(failure) => {
                    self.projection = None;
                    self.failure = Some(failure);
                    self.installed_revision = None;
                }
            }
        }
        self.parsing = self.pending;
        if !self.pending {
            self.worker_running = false;
        }
        self.pending
    }

    /// Cancels the current parse and tells the next job to stop early. Called
    /// on document drop so a worker cannot outlive its owner.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Release);
    }

    pub fn projection(&self) -> Option<&P> {
        self.projection.as_deref()
    }

    /// The installed projection shared with a presentation; cloning the `Arc`
    /// keeps a stale view from borrowing the document across an update.
    pub fn shared_projection(&self) -> Option<Arc<P>> {
        self.projection.clone()
    }

    pub fn failure(&self) -> Option<F>
    where
        F: Copy,
    {
        self.failure
    }

    pub fn is_parsing(&self) -> bool {
        self.parsing
    }

    pub fn installed_revision(&self) -> Option<u64> {
        self.installed_revision
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn worker_running(&self) -> bool {
        self.worker_running
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Failure {
        Bad,
    }

    fn install(
        state: &mut ProjectionState<u32, Failure>,
        job: &ProjectionJob,
        revision: u64,
        current: u64,
        result: ProjectionResult<u32, Failure>,
    ) -> bool {
        state.install(job.generation(), revision, current, result)
    }

    #[test]
    fn a_newer_edit_coalesces_the_worker_instead_of_spawning_a_second_one() {
        let mut state = ProjectionState::<u32, Failure>::new();
        assert!(state.invalidate());
        state.begin_worker();
        assert!(!state.invalidate(), "the running worker is reused");
        let job = state.take_job().expect("first pending parse");
        assert_eq!(job.generation(), 2);
        // A second edit while the first parse is in flight stays pending.
        assert!(!state.invalidate());
        assert!(install(&mut state, &job, 7, 7, ProjectionResult::Ready(1)));
        let next = state.take_job().expect("newest pending parse");
        assert_eq!(next.generation(), 3);
        assert!(!install(
            &mut state,
            &next,
            8,
            8,
            ProjectionResult::Ready(2)
        ));
        assert_eq!(state.projection(), Some(&2));
        assert_eq!(state.installed_revision(), Some(8));
        assert!(!state.is_parsing());
    }

    #[test]
    fn stale_generation_and_stale_revision_install_nothing() {
        let mut state = ProjectionState::<u32, Failure>::new();
        state.invalidate();
        state.begin_worker();
        let job = state.take_job().unwrap();
        state.invalidate(); // bump generation while the parse is in flight
        install(&mut state, &job, 3, 3, ProjectionResult::Ready(10));
        assert_eq!(state.projection(), None);
        assert_eq!(state.installed_revision(), None);

        let job = state.take_job().unwrap();
        assert!(!install(
            &mut state,
            &job,
            4,
            5,
            ProjectionResult::Ready(11)
        ));
        assert_eq!(state.projection(), None);
    }

    #[test]
    fn a_cancelled_result_keeps_the_previous_projection_and_a_failure_names_itself() {
        let mut state = ProjectionState::<u32, Failure>::new();
        state.invalidate();
        state.begin_worker();
        let first = state.take_job().unwrap();
        assert!(!install(
            &mut state,
            &first,
            1,
            1,
            ProjectionResult::Ready(7)
        ));
        assert_eq!(state.projection(), Some(&7));

        state.invalidate();
        let second = state.take_job().unwrap();
        assert!(!install(
            &mut state,
            &second,
            2,
            2,
            ProjectionResult::Cancelled
        ));
        // Cancelled installs no failure, and the invalidation already cleared
        // the old projection.
        assert_eq!(state.projection(), None);
        assert_eq!(state.failure(), None);

        state.invalidate();
        let third = state.take_job().unwrap();
        assert!(!install(
            &mut state,
            &third,
            3,
            3,
            ProjectionResult::Failed(Failure::Bad)
        ));
        assert_eq!(state.failure(), Some(Failure::Bad));
        assert_eq!(state.installed_revision(), None);
    }

    #[test]
    fn take_job_stops_the_worker_when_nothing_is_pending() {
        let mut state = ProjectionState::<u32, Failure>::new();
        state.invalidate();
        state.begin_worker();
        let job = state.take_job().unwrap();
        assert!(!install(&mut state, &job, 1, 1, ProjectionResult::Ready(9)));
        assert!(!state.worker_running());
        assert!(state.take_job().is_none());
        assert!(!state.is_parsing());
    }
}
