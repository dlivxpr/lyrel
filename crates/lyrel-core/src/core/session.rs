//! Window/workspace session value model.
//!
//! The value types live in `lyrel-protocol` and are re-exported here
//! unchanged. The test-only config-dir pinning helper stays in this crate
//! because `lyrel-core`'s own daemon tests pin the process-wide config
//! directory through it.

pub use lyrel_protocol::core::session::*;

#[cfg(test)]
pub(crate) mod test_support {
    use std::path::PathBuf;
    use std::sync::{Mutex, MutexGuard};

    static SESSION_FILE: Mutex<()> = Mutex::new(());

    pub(crate) fn lock_session_file() -> MutexGuard<'static, ()> {
        SESSION_FILE.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub(crate) fn pin_config_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lyrel-covtest-{}", std::process::id()));
        std::fs::create_dir_all(&dir).ok();
        crate::core::config::set_config_dir(dir.clone());
        dir
    }
}
