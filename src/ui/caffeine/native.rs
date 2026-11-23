//! Native idle-sleep holds for Caffeine — one process-wide request that keeps
//! the computer running Lyrel out of *idle system sleep* while leaving display
//! sleep and screen lock entirely to system policy (#363).
//!
//! - macOS: an IOKit assertion of type `PreventUserIdleSystemSleep` — the
//!   native idle *system sleep* assertion, never `PreventUserIdleDisplaySleep`.
//! - Windows: `SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED)`,
//!   owned by the UI thread, with no `ES_DISPLAY_REQUIRED`. Setting
//!   `ES_CONTINUOUS` alone clears the request.
//!
//! Both calls are cheap syscalls (microseconds, never block on I/O), so the
//! controller on the UI thread calls them synchronously and can report their
//! real success or failure as the state the status bar shows.

/// A live native request. `release` reports the platform's own answer; the
/// `Drop` backstop only covers a guard leaked by a panic.
pub(super) struct SleepHold {
    inner: Inner,
}

impl SleepHold {
    /// Acquire the process-wide idle-sleep hold. `Err` carries the platform's
    /// own failure; a `None` guard is never installed silently.
    pub(super) fn acquire(reason: &str) -> Result<Self, String> {
        #[cfg(test)]
        if let Some(error) = injected_failure() {
            return Err(error);
        }
        Ok(Self {
            inner: Inner::acquire(reason)?,
        })
    }

    /// Release the hold, reporting the platform's real answer. On failure the
    /// hold stays live (the caller keeps the guard) and the error is truth.
    pub(super) fn release(&mut self) -> Result<(), String> {
        #[cfg(test)]
        if let Some(error) = injected_failure() {
            return Err(error);
        }
        self.inner.release()
    }
}

impl Drop for SleepHold {
    fn drop(&mut self) {
        self.inner.silent_release();
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use core_foundation::base::TCFType as _;
    use core_foundation::string::{CFString, CFStringRef};

    type IOReturn = std::os::raw::c_int;

    // `PreventUserIdleSystemSleep` is the documented value of
    // `kIOPMAssertionTypePreventUserIdleSystemSleep` in <IOKit/pwr_mgt/IOPMLib.h>;
    // spelling the CFString ourselves avoids a bindgen-style dependency on
    // IOKit headers. Assertions are plain C calls, safe from any thread.
    #[link(name = "IOKit", kind = "framework")]
    unsafe extern "C" {
        fn IOPMAssertionCreateWithName(
            assertion_type: CFStringRef,
            assertion_level: u32,
            assertion_name: CFStringRef,
            assertion_id: *mut u32,
        ) -> IOReturn;
        fn IOPMAssertionRelease(assertion_id: u32) -> IOReturn;
    }

    /// `kIOPMAssertionLevelOn` in IOPMLib.h.
    const ASSERTION_LEVEL_ON: u32 = 255;
    /// `kIOReturnSuccess`.
    const IO_RETURN_SUCCESS: IOReturn = 0;

    pub(super) struct Inner {
        assertion_id: u32,
        released: bool,
    }

    impl Inner {
        pub(super) fn acquire(reason: &str) -> Result<Self, String> {
            let kind = CFString::new("PreventUserIdleSystemSleep");
            let name = CFString::new(reason);
            let mut assertion_id = 0u32;
            // SAFETY: both CFStrings outlive the call; `assertion_id` is a
            // valid out pointer. IOKit copies the strings it keeps.
            let status = unsafe {
                IOPMAssertionCreateWithName(
                    kind.as_concrete_TypeRef(),
                    ASSERTION_LEVEL_ON,
                    name.as_concrete_TypeRef(),
                    &mut assertion_id,
                )
            };
            if status != IO_RETURN_SUCCESS {
                return Err(format!(
                    "IOPMAssertionCreateWithName(PreventUserIdleSystemSleep) returned {status}"
                ));
            }
            Ok(Self {
                assertion_id,
                released: false,
            })
        }

        pub(super) fn release(&mut self) -> Result<(), String> {
            if self.released {
                return Ok(());
            }
            // SAFETY: `assertion_id` is a live assertion this guard owns.
            let status = unsafe { IOPMAssertionRelease(self.assertion_id) };
            if status != IO_RETURN_SUCCESS {
                return Err(format!("IOPMAssertionRelease returned {status}"));
            }
            self.released = true;
            Ok(())
        }

        pub(super) fn silent_release(&mut self) {
            let _ = self.release();
        }
    }

    #[test]
    fn macos_hold_is_an_idle_system_assertion_and_is_removed_on_release() {
        use core_foundation::base::CFType;
        use core_foundation::dictionary::{CFDictionary, CFDictionaryRef};
        use core_foundation::number::CFNumber;

        #[link(name = "IOKit", kind = "framework")]
        unsafe extern "C" {
            fn IOPMAssertionCopyProperties(assertion_id: u32) -> CFDictionaryRef;
        }

        let mut hold = super::SleepHold::acquire("Lyrel idle sleep test").expect("acquire");
        let id = hold.inner.assertion_id;
        let properties = || {
            // SAFETY: IOKit accepts an assertion ID and returns either null
            // or a retained dictionary owned by the caller.
            let result = unsafe { IOPMAssertionCopyProperties(id) };
            (!result.is_null()).then(|| unsafe {
                CFDictionary::<CFString, CFType>::wrap_under_create_rule(result)
            })
        };
        let info = properties().expect("the system reports an active assertion");
        assert_eq!(
            info.get(&CFString::new("AssertType"))
                .downcast::<CFString>()
                .expect("assertion type")
                .to_string(),
            "PreventUserIdleSystemSleep"
        );
        assert_eq!(
            info.get(&CFString::new("AssertLevel"))
                .downcast::<CFNumber>()
                .and_then(|level| level.to_i32()),
            Some(255)
        );
        hold.release().expect("release");
        assert!(properties().is_none(), "the system retains no assertion");
    }
}

#[cfg(windows)]
mod platform {
    use windows_sys::Win32::System::Power::{
        ES_CONTINUOUS, ES_SYSTEM_REQUIRED, SetThreadExecutionState,
    };

    pub(super) struct Inner {
        released: bool,
    }

    impl Inner {
        pub(super) fn acquire(_reason: &str) -> Result<Self, String> {
            // No `ES_DISPLAY_REQUIRED`: the display must still sleep and lock.
            // The returned previous state is 0 on failure — the API's own
            // truth about whether the request landed.
            let previous = unsafe { SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED) };
            if previous == 0 {
                return Err(
                    "SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED) failed"
                        .to_string(),
                );
            }
            Ok(Self { released: false })
        }

        pub(super) fn release(&mut self) -> Result<(), String> {
            if self.released {
                return Ok(());
            }
            let previous = unsafe { SetThreadExecutionState(ES_CONTINUOUS) };
            if previous == 0 {
                return Err("SetThreadExecutionState(ES_CONTINUOUS) failed".to_string());
            }
            self.released = true;
            Ok(())
        }

        pub(super) fn silent_release(&mut self) {
            let _ = self.release();
        }
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
mod platform {
    pub(super) struct Inner {
        _private: (),
    }

    impl Inner {
        pub(super) fn acquire(_reason: &str) -> Result<Self, String> {
            Err("Caffeine is not supported on this platform".to_string())
        }

        pub(super) fn release(&mut self) -> Result<(), String> {
            Ok(())
        }

        pub(super) fn silent_release(&mut self) {}
    }
}

use platform::Inner;

// Test-only switch that makes the next acquire/release report failure, so
// consumer tests can watch the app surface real error truth at this boundary
// without a platform hook for failing power APIs (there is none). Kept
// thread-local: each GUI test runs on its own thread, and a process-wide
// switch would let a forcing test race the plain round-trip tests.
#[cfg(test)]
thread_local! {
    static FORCE_FAILURE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Enable or disable the injected power-API failure for this test thread.
#[cfg(test)]
pub(crate) fn force_failure_for_test(on: bool) {
    FORCE_FAILURE.with(|flag| flag.set(on));
}

#[cfg(test)]
fn injected_failure() -> Option<String> {
    FORCE_FAILURE
        .with(|flag| flag.get())
        .then(|| "injected power-API failure".to_string())
}

#[cfg(test)]
pub(super) mod tests {
    use super::SleepHold;

    /// Real native round-trip at the boundary the app consumes: acquiring
    /// installs the request, releasing clears it. On Windows the probe reads
    /// the kernel's previous-state answer back out of the API itself.
    #[test]
    fn native_hold_round_trips() {
        let mut hold = SleepHold::acquire("lyrel test").expect("acquire the idle-sleep hold");
        hold.release().expect("release the idle-sleep hold");
    }

    #[cfg(windows)]
    #[test]
    fn windows_hold_sets_and_clears_system_sleep_only() {
        use windows_sys::Win32::System::Power::{
            ES_CONTINUOUS, ES_DISPLAY_REQUIRED, ES_SYSTEM_REQUIRED, SetThreadExecutionState,
        };

        fn read_and_clear() -> u32 {
            // SAFETY: with ES_CONTINUOUS alone, Win32 returns the previous
            // execution state of this thread and clears the request.
            let previous = unsafe { SetThreadExecutionState(ES_CONTINUOUS) };
            assert_ne!(previous, 0, "failed to read execution state");
            previous
        }

        let before = read_and_clear();
        let mut hold = SleepHold::acquire("lyrel test").expect("acquire");
        let during = read_and_clear();
        hold.release().expect("release after the probe");

        // Use a fresh request to test release on a live hold. The first
        // probe cleared the state when it verified the acquire flags.
        let mut hold = SleepHold::acquire("lyrel test").expect("reacquire");
        hold.release().expect("release");
        let after = read_and_clear();

        assert_eq!(
            before & ES_SYSTEM_REQUIRED,
            0,
            "test starts clean: {before:#x}"
        );
        assert_ne!(
            during & ES_SYSTEM_REQUIRED,
            0,
            "the kernel reports the idle-sleep request live: {during:#x}"
        );
        assert_eq!(
            after & ES_SYSTEM_REQUIRED,
            0,
            "releasing cleared the request: {after:#x}"
        );
        assert_eq!(
            during & ES_DISPLAY_REQUIRED,
            0,
            "display override never set"
        );
    }
}
