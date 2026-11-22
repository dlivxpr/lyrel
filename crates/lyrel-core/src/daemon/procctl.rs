//! Verified process termination for the Activity Monitor's End Process
//! actions (#372).
//!
//! Two entry points, one safety model. `plan` answers "what would this
//! action cover": it binds the live process, checks the client's claimed
//! birth stamp against it, checks the process still belongs to the pane's
//! tree, and walks the *complete* subtree from the daemon's own process
//! table — never from the UI's capped snapshot, which must never be what a
//! confirmation dialog is built from. `execute` acts on exactly the
//! identities the user confirmed: each one is re-verified against the live
//! process and the pane's tree immediately before it is signaled, drifted
//! ones are dropped, and the scope is never expanded to processes the user
//! did not confirm.
//!
//! The signal itself goes through the same OS object that carried the
//! identity check — a Windows process handle opened with terminate rights,
//! a Linux pidfd — so the process that is signaled is provably the process
//! that was verified, and a pid the kernel reused in between cannot receive
//! the signal meant for its predecessor.

use std::time::{Duration, Instant};

use crate::daemon::protocol::{
    EndProcessOutcome, EndProcessPlan, EndProcessState, EndProcessTarget, ProcIdentity,
};

/// The one budget the whole termination batch waits on, never a fresh
/// timeout per process: `terminate_and_wait_all` sets the same precedent for
/// installation teardown. Sits inside the control request's own deadline.
const EXEC_BUDGET: Duration = Duration::from_secs(8);

/// Why a scope request was refused before any signal. Each variant is a
/// distinct user-facing reason: the caller asked to end a process the daemon
/// can no longer vouch for, and "no" is the only safe answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PlanRefusal {
    /// The pid names no live process this user can reach.
    Gone,
    /// The pid names a live process, but not one that belongs to the pane's
    /// tree — the pane moved on, or the number was reused outside the pane.
    NotOwned,
    /// The live process's birth stamp does not match the claimed one: the pid
    /// was reused, or the confirmed snapshot was stale.
    Stale,
    /// The request carried no birth stamp to check against. A bare pid names
    /// whatever process the kernel most recently assigned the number to, and
    /// signaling that is never allowed.
    Unverifiable,
}

impl PlanRefusal {
    /// The daemon's wording of the refusal, for logs and for the wire. The
    /// daemon has no locale; the UI localizes on top of the refusal kind.
    pub(crate) fn message(self, pid: u32, pane_id: u64) -> String {
        match self {
            PlanRefusal::Gone => format!("process {pid} has already exited"),
            PlanRefusal::NotOwned => {
                format!("process {pid} no longer belongs to pane {pane_id}")
            }
            PlanRefusal::Stale => {
                format!("process {pid} no longer matches the confirmed identity")
            }
            PlanRefusal::Unverifiable => {
                format!("process {pid} carries no verifiable identity")
            }
        }
    }
}

/// What the daemon found when it tried to act on one confirmed identity and
/// chose not to signal it. Signaled targets are not an error and are
/// reported by `execute` directly.
enum SignalRefusal {
    Gone,
    NotOwned,
    Stale,
    Unverifiable,
}

impl SignalRefusal {
    fn state(self) -> EndProcessState {
        match self {
            SignalRefusal::Gone => EndProcessState::Exited,
            SignalRefusal::NotOwned => EndProcessState::NotOwned,
            SignalRefusal::Stale => EndProcessState::Stale,
            SignalRefusal::Unverifiable => EndProcessState::Unverifiable,
        }
    }
}

/// The owning daemon's scope answer for ending `pid` in the pane rooted at
/// `shell_pid`. `started` is the birth stamp the client's selected row
/// carried; any refusal is returned before a scope is built, so a
/// confirmation dialog can never form around a process the daemon cannot
/// vouch for.
pub(crate) fn plan(
    shell_pid: u32,
    pid: u32,
    started: Option<u64>,
) -> Result<EndProcessPlan, PlanRefusal> {
    // Platforms whose native API cannot atomically bind a verified identity
    // to the termination handle or token fail closed before any scope is
    // built: the caller's monitor stays read-only rather than claim a naked
    // pid is safe to signal.
    if !bind::ATOMIC_BIND {
        return Err(PlanRefusal::Unverifiable);
    }
    let started = started.ok_or(PlanRefusal::Unverifiable)?;
    let target = bind::Bound::open(pid).ok_or(PlanRefusal::Gone)?;
    // A killed-but-not-yet-reaped process can still be opened and can still
    // read its own birth stamp: on Windows it answers both right up until
    // the last handle closes, while its ancestry is already gone. Exited is
    // the honest refusal — the pane moved on, there is nothing to end.
    if target.exited() {
        return Err(PlanRefusal::Gone);
    }
    if target.started() != Some(started) {
        return Err(PlanRefusal::Stale);
    }
    #[cfg(windows)]
    let table = crate::daemon::winproc::checked_snapshot().ok_or(PlanRefusal::Unverifiable)?;
    #[cfg(windows)]
    let owned = |pid, started| windows_owns(&table, shell_pid, pid, started, process_started);
    #[cfg(not(windows))]
    let owned = |pid, _| crate::daemon::procinfo::descends_from(pid, shell_pid);
    if !owned(pid, started) {
        return Err(PlanRefusal::NotOwned);
    }
    let (members, complete) = crate::daemon::procinfo::complete_subtree(pid);
    #[cfg(windows)]
    let mut complete = complete;
    let targets = members
        .into_iter()
        .filter_map(|(member, name)| {
            let member_started = if member == pid {
                Some(started)
            } else {
                crate::daemon::procstat::read(member).started
            };
            #[cfg(windows)]
            if !member_started.is_some_and(|stamp| owned(member, stamp)) {
                complete = false;
                return None;
            }
            Some(EndProcessTarget {
                pid: member,
                name,
                started: member_started,
            })
        })
        .collect();
    Ok(EndProcessPlan { targets, complete })
}

/// Terminate exactly `scope`, in the order the plan produced it (deepest
/// first), and report one outcome per confirmed identity. Each identity is
/// re-verified against the live process and the pane's tree immediately
/// before its signal; identities that drifted are refused, and processes
/// that appeared after the plan are never added — the confirmed scope is a
/// ceiling, not a floor.
pub(crate) fn execute(shell_pid: u32, scope: &[ProcIdentity]) -> Vec<EndProcessOutcome> {
    let deadline = Instant::now() + EXEC_BUDGET;
    let mut signaled = Vec::new();
    let mut outcomes: Vec<EndProcessOutcome> = Vec::with_capacity(scope.len());
    for id in scope {
        match verify(shell_pid, id) {
            Ok((bound, name)) => {
                bound.signal();
                signaled.push((outcomes.len(), bound));
                outcomes.push(EndProcessOutcome {
                    pid: id.pid,
                    name,
                    state: EndProcessState::Failed,
                });
            }
            Err(refusal) => outcomes.push(EndProcessOutcome {
                pid: id.pid,
                // A refused row names whatever the pid names now, when it
                // names anything: for a reused pid that is exactly the
                // evidence the user needs for why it was refused.
                name: live_name(id.pid),
                state: refusal.state(),
            }),
        }
    }
    for (index, bound) in signaled {
        let left = deadline.saturating_duration_since(Instant::now());
        if bound.wait_gone(left) {
            outcomes[index].state = EndProcessState::Ended;
        }
    }
    outcomes
}

/// The pre-signal half of `execute`, factored out so the batch loop reads as
/// verify-then-signal-then-wait.
fn verify(shell_pid: u32, id: &ProcIdentity) -> Result<(bind::Bound, String), SignalRefusal> {
    // The same platform fail-closed gate as `plan`: no atomic bind, no
    // signal, however trustworthy the row looked when it was confirmed.
    if !bind::ATOMIC_BIND {
        return Err(SignalRefusal::Unverifiable);
    }
    let started = id.started.ok_or(SignalRefusal::Unverifiable)?;
    let bound = bind::Bound::open(id.pid).ok_or(SignalRefusal::Gone)?;
    // Same zombie rule as `plan`: an exited process object can still be
    // opened and still carries its birth stamp, so "opens" is not "lives".
    if bound.exited() {
        return Err(SignalRefusal::Gone);
    }
    if bound.started() != Some(started) {
        return Err(SignalRefusal::Stale);
    }
    let name = live_name(id.pid);
    #[cfg(windows)]
    let owned = crate::daemon::winproc::checked_snapshot()
        .ok_or(SignalRefusal::Unverifiable)
        .map(|table| windows_owns(&table, shell_pid, id.pid, started, process_started))?;
    #[cfg(not(windows))]
    let owned = crate::daemon::procinfo::descends_from(id.pid, shell_pid);
    if !owned {
        return Err(SignalRefusal::NotOwned);
    }
    Ok((bound, name))
}

#[cfg(windows)]
fn process_started(pid: u32) -> Option<u64> {
    crate::daemon::procstat::read(pid).started
}

/// Windows keeps the creator's numeric PID after it exits. A current
/// process born after the child can never be that creator.
#[cfg(windows)]
fn windows_owns(
    table: &[crate::daemon::winproc::Proc],
    root: u32,
    mut child: u32,
    mut child_started: u64,
    mut started: impl FnMut(u32) -> Option<u64>,
) -> bool {
    for _ in 0..64 {
        let Some(row) = table.iter().find(|row| row.pid == child) else {
            return false;
        };
        if child == root {
            return true;
        }
        let parent = row.parent;
        if parent <= 1 || parent == child {
            return false;
        }
        let Some(parent_started) = started(parent) else {
            return false;
        };
        if parent_started > child_started {
            return false;
        }
        child = parent;
        child_started = parent_started;
    }
    false
}

/// The name the pid names right now, or an empty string when it names
/// nothing readable. Display-only: never part of the identity check.
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn live_name(pid: u32) -> String {
    crate::daemon::procinfo::proc_name(pid as i32).unwrap_or_default()
}

/// The Windows table carries the kernel's name per pid already — one
/// snapshot, filtered to the pid, instead of opening the process a second
/// time just to read what the walk recorded.
#[cfg(windows)]
fn live_name(pid: u32) -> String {
    crate::daemon::winproc::snapshot()
        .into_iter()
        .find(|proc| proc.pid == pid)
        .map(|proc| proc.name)
        .unwrap_or_default()
}

#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
fn live_name(_pid: u32) -> String {
    String::new()
}

/// The OS object that binds a live process identity to the signal that ends
/// it. `open` pins the process; `started` reads the birth stamp out of the
/// same object; `exited` reports an exited-but-still-openable process object
/// (a zombie keeps its birth stamp readable, so "opens" is not "lives");
/// `signal` and `wait_gone` act through it. On platforms with no reliable
/// binding the arms fail closed — `open` reports the process absent, so
/// nothing is ever signaled on a guess.
#[cfg(windows)]
mod bind {
    use std::time::Duration;

    /// One opened process handle binds the identity to the signal: the
    /// creation FILETIME is read from this handle and the termination goes
    /// through this same handle, so the process that is terminated is the
    /// process that was verified. Primary sources: Microsoft Learn,
    /// `OpenProcess` (`PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE
    /// | PROCESS_TERMINATE`), `GetProcessTimes`, `TerminateProcess`, and
    /// `WaitForSingleObject`.
    pub(super) const ATOMIC_BIND: bool = true;

    /// One opened process handle. The identity check and the termination
    /// both run through this handle, so the process that is terminated is
    /// the process whose creation FILETIME was compared — opening a fresh
    /// handle per step is what let a reused pid receive a signal meant for
    /// its predecessor.
    pub(super) struct Bound {
        handle: windows_sys::Win32::Foundation::HANDLE,
    }

    impl Bound {
        pub(super) fn open(pid: u32) -> Option<Bound> {
            use windows_sys::Win32::System::Threading::{
                OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
                PROCESS_TERMINATE,
            };
            // SAFETY: the returned handle is checked and owned by `Bound`,
            // which closes it on drop.
            let handle = unsafe {
                OpenProcess(
                    PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE | PROCESS_TERMINATE,
                    0,
                    pid,
                )
            };
            if handle.is_null() {
                return None;
            }
            Some(Bound { handle })
        }

        pub(super) fn started(&self) -> Option<u64> {
            use windows_sys::Win32::Foundation::FILETIME;
            use windows_sys::Win32::System::Threading::GetProcessTimes;
            // SAFETY: `self.handle` is open; the four out-pointers are zeroed
            // locals of the type the call expects.
            unsafe {
                let (mut created, mut exited, mut kernel, mut user): (
                    FILETIME,
                    FILETIME,
                    FILETIME,
                    FILETIME,
                ) = std::mem::zeroed();
                if GetProcessTimes(
                    self.handle,
                    &mut created,
                    &mut exited,
                    &mut kernel,
                    &mut user,
                ) == 0
                {
                    return None;
                }
                Some((created.dwHighDateTime as u64) << 32 | created.dwLowDateTime as u64)
            }
        }

        /// Whether the bound process object is already signaled — dead, even
        /// though Windows keeps an un-reaped exited process openable (and
        /// its birth stamp readable) until the last handle on it closes.
        pub(super) fn exited(&self) -> bool {
            use windows_sys::Win32::Foundation::WAIT_OBJECT_0;
            use windows_sys::Win32::System::Threading::WaitForSingleObject;
            // SAFETY: `self.handle` is open with SYNCHRONIZE and outlives the
            // zero-length wait.
            unsafe { WaitForSingleObject(self.handle, 0) == WAIT_OBJECT_0 }
        }

        pub(super) fn signal(&self) {
            use windows_sys::Win32::System::Threading::TerminateProcess;
            // SAFETY: `self.handle` is open with PROCESS_TERMINATE.
            unsafe {
                TerminateProcess(self.handle, 1);
            }
        }

        pub(super) fn wait_gone(&self, timeout: Duration) -> bool {
            use windows_sys::Win32::Foundation::WAIT_OBJECT_0;
            use windows_sys::Win32::System::Threading::WaitForSingleObject;
            let millis = timeout.as_millis().min(u128::from(u32::MAX - 1)) as u32;
            // SAFETY: `self.handle` is open with SYNCHRONIZE and outlives the
            // wait.
            unsafe { WaitForSingleObject(self.handle, millis) == WAIT_OBJECT_0 }
        }
    }

    impl Drop for Bound {
        fn drop(&mut self) {
            // SAFETY: the handle was opened in `open` and not otherwise closed.
            unsafe {
                windows_sys::Win32::Foundation::CloseHandle(self.handle);
            }
        }
    }
}

#[cfg(target_os = "linux")]
mod bind {
    use std::os::unix::io::RawFd;
    use std::time::Duration;

    /// A pidfd binds the identity to the signal: the fd names the kernel
    /// task, the `starttime` comparison is what makes `pidfd_send_signal`
    /// conditional on the right identity, and a reused number cannot
    /// intercept the signal. Primary sources: `pidfd_open(2)`,
    /// `pidfd_send_signal(2)` and `poll(2)` (man7.org/linux-man-pages),
    /// with `/proc/<pid>/stat` field 22 (`starttime`) per the Linux
    /// `proc_pid_stat(5)` man page.
    pub(super) const ATOMIC_BIND: bool = true;

    /// A pidfd. The fd refers to the kernel's task, not to the number, so the
    /// signal below reaches the same process whose `starttime` was compared —
    /// even if the number is reused between the check and the signal, the fd
    /// still names the old task.
    pub(super) struct Bound {
        fd: RawFd,
        pid: u32,
    }

    impl Bound {
        pub(super) fn open(pid: u32) -> Option<Bound> {
            // SAFETY: `pidfd_open` takes no memory arguments; the returned fd
            // is owned by `Bound` and closed on drop.
            let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid as libc::c_int, 0) } as RawFd;
            if fd < 0 {
                return None;
            }
            Some(Bound { fd, pid })
        }

        pub(super) fn started(&self) -> Option<u64> {
            // Read by number; the comparison it feeds is what makes the later
            // signal through the fd conditional on the right identity.
            crate::daemon::procstat::read(self.pid).started
        }

        /// A pidfd poll with no timeout reports readability the moment the
        /// named task exits — including a zombie not yet reaped, which opens
        /// fine and still matches its confirmed starttime.
        pub(super) fn exited(&self) -> bool {
            let mut pfd = libc::pollfd {
                fd: self.fd,
                events: libc::POLLIN,
                revents: 0,
            };
            // SAFETY: `pfd` points at one valid pollfd for the fd's lifetime.
            unsafe { libc::poll(&mut pfd, 1, 0) == 1 }
        }

        pub(super) fn signal(&self) {
            // SAFETY: `self.fd` is a live pidfd owned by self; the signal
            // target pointer is null and the flags are zero.
            unsafe {
                libc::syscall(
                    libc::SYS_pidfd_send_signal,
                    self.fd,
                    libc::SIGTERM,
                    std::ptr::null::<libc::siginfo_t>(),
                    0,
                );
            }
        }

        pub(super) fn wait_gone(&self, timeout: Duration) -> bool {
            // A pidfd becomes readable when the process it names exits.
            let mut pfd = libc::pollfd {
                fd: self.fd,
                events: libc::POLLIN,
                revents: 0,
            };
            let millis = timeout.as_millis().min(i32::MAX as u128) as i32;
            // SAFETY: `pfd` points at one valid pollfd for the fd's lifetime.
            unsafe { libc::poll(&mut pfd, 1, millis) == 1 }
        }
    }

    impl Drop for Bound {
        fn drop(&mut self) {
            // SAFETY: the fd was opened in `open` and not otherwise closed.
            unsafe {
                libc::close(self.fd);
            }
        }
    }
}

#[cfg(target_os = "macos")]
mod bind {
    use std::time::Duration;

    /// macOS cannot bind a verified identity to the signal that ends it, so
    /// this arm fails closed. The platform's documented process APIs name a
    /// naked pid on both sides of the check: `proc_pidinfo` reads a birth
    /// stamp *for a pid* and `kill` signals *a pid*, with no pidfd-style
    /// handle whose referent is the kernel task rather than the number —
    /// nothing ties the stamp that was compared to the signal that is sent.
    /// A process that exits and has its number reused between the read and
    /// the kill would receive the signal meant for its predecessor, which is
    /// exactly what #372 refuses to risk: `ATOMIC_BIND == false` keeps every
    /// scope at `Unverifiable` and the caller's monitor read-only. Primary
    /// sources: Apple's `proc_pidinfo` (`libproc.h`, Apple Developer
    /// documentation) and `kill(2)` (XNU/BSD), neither of which offers a
    /// token-bound termination.
    pub(super) const ATOMIC_BIND: bool = false;

    /// Never constructed on this platform: `open` reports the process absent
    /// so nothing is ever signaled on a guess, and `plan`/`verify` refuse
    /// with `Unverifiable` before the refusal could be mistaken for "the
    /// process exited".
    pub(super) struct Bound {
        #[allow(dead_code)]
        pid: u32,
    }

    impl Bound {
        pub(super) fn open(_pid: u32) -> Option<Bound> {
            None
        }
        #[allow(dead_code)]
        pub(super) fn started(&self) -> Option<u64> {
            None
        }
        #[allow(dead_code)]
        pub(super) fn exited(&self) -> bool {
            true
        }
        #[allow(dead_code)]
        pub(super) fn signal(&self) {}
        #[allow(dead_code)]
        pub(super) fn wait_gone(&self, _timeout: Duration) -> bool {
            true
        }
    }
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
mod bind {
    use std::time::Duration;

    /// Platforms with no identity binding fail closed: `ATOMIC_BIND == false`
    /// keeps every scope at `Unverifiable`, `open` never binds, and nothing
    /// is ever signaled on a platform this build cannot vouch for.
    pub(super) const ATOMIC_BIND: bool = false;

    /// Never constructed on this platform: `open` reports the process absent
    /// so nothing is ever signaled on a guess.
    pub(super) struct Bound {
        #[allow(dead_code)]
        pid: u32,
    }

    impl Bound {
        pub(super) fn open(_pid: u32) -> Option<Bound> {
            None
        }
        #[allow(dead_code)]
        pub(super) fn started(&self) -> Option<u64> {
            None
        }
        #[allow(dead_code)]
        pub(super) fn exited(&self) -> bool {
            true
        }
        #[allow(dead_code)]
        pub(super) fn signal(&self) {}
        #[allow(dead_code)]
        pub(super) fn wait_gone(&self, _timeout: Duration) -> bool {
            true
        }
    }
}

#[cfg(test)]
mod tests;
