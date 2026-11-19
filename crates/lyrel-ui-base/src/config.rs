//! The GPUI `Config` global: the GUI wrapper around `lyrel-protocol`'s pure
//! configuration value, plus the scratch-directory guards that keep GUI tests
//! off the real user config.
//!
//! Exports here are explicit — the protocol config surface is re-exported by
//! name, never by glob, so `Config` below cannot shadow (or be shadowed by)
//! another `Config` arriving through a wildcard.

use lyrel_protocol::core::config::FontFeatures;

pub use lyrel_protocol::core::config::Config as CoreConfig;
pub use lyrel_protocol::core::config::LoadOutcome;

#[derive(Debug, Clone, Default)]
pub struct Config(pub CoreConfig);

impl gpui_kit::Global for Config {}

impl Config {
    pub fn load() -> Self {
        Self::load_with_outcome().0
    }

    pub fn load_with_outcome() -> (Self, LoadOutcome) {
        #[cfg(any(test, feature = "test-guards"))]
        {
            assert_scratch_config_dir("Config::load_with_outcome");
            let _guard = Self::lock_config_file();
            let (core, outcome) = CoreConfig::load_with_outcome();
            (Self(core), outcome)
        }
        #[cfg(not(any(test, feature = "test-guards")))]
        {
            let (core, outcome) = CoreConfig::load_with_outcome();
            (Self(core), outcome)
        }
    }
}

#[cfg(any(test, feature = "test-guards"))]
impl Config {
    /// GUI tests share the process's scratch config directory; any of them can
    /// save between a test's own write and the read that verifies it. Hold the
    /// returned guard across a whole save→load→assert sequence. It is a
    /// reentrant mutex because `try_save`/`load_with_outcome` take the same
    /// lock while the guard is held on this thread.
    pub fn lock_config_file() -> parking_lot::ReentrantMutexGuard<'static, ()> {
        WRITE_LOCK.lock()
    }

    pub fn save(&self) {
        if let Err(error) = self.try_save() {
            log::warn!("failed to save config: {error}");
        }
    }

    pub fn try_save(&self) -> std::io::Result<()> {
        assert_scratch_config_dir("Config::try_save");
        let _guard = Self::lock_config_file();
        self.0.try_save()
    }
}

/// GUI tests have separate App globals but share the process's scratch
/// directory. Serialize their config-file writes *and* reads the way a real
/// UI event loop does; reentrant so a test can hold the guard across calls
/// that save or load again on the same thread.
#[cfg(any(test, feature = "test-guards"))]
static WRITE_LOCK: std::sync::LazyLock<parking_lot::ReentrantMutex<()>> =
    std::sync::LazyLock::new(|| parking_lot::ReentrantMutex::new(()));

#[cfg(any(test, feature = "test-guards"))]
fn is_real_user_config_dir(dir: Option<&std::path::Path>) -> bool {
    dir.is_some() && dir == lyrel_protocol::core::config::default_config_dir().as_deref()
}

#[cfg(any(test, feature = "test-guards"))]
fn assert_scratch_config_dir(what: &str) {
    assert!(
        !is_real_user_config_dir(lyrel_protocol::core::config::config_dir_path().as_deref()),
        "{what} in a test would touch the real user config dir ({}). \
         Call `pin_test_config_dir()` at the top of the test/harness first.",
        lyrel_protocol::core::config::config_dir_path()
            .unwrap_or_default()
            .display(),
    );
}

#[cfg(any(test, feature = "test-guards"))]
pub fn pin_test_config_dir() {
    static PIN: std::sync::Once = std::sync::Once::new();
    PIN.call_once(|| {
        // The override lives for the process, but PID reuse must not import
        // a previous test process's files or permissions.
        let dir = tempfile::Builder::new()
            .prefix("lyrel-covtest-")
            .tempdir()
            .expect("create isolated GUI test config directory");
        lyrel_protocol::core::config::set_config_dir(dir.keep());
    });
}

impl std::ops::Deref for Config {
    type Target = CoreConfig;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::ops::DerefMut for Config {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl From<CoreConfig> for Config {
    fn from(config: CoreConfig) -> Self {
        Self(config)
    }
}

pub fn gpui_font_features(features: &FontFeatures) -> gpui_kit::FontFeatures {
    gpui_kit::FontFeatures(std::sync::Arc::new(features.tag_value_list().to_vec()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn the_real_config_dir_is_the_only_one_the_guard_rejects() {
        if let Some(real) = lyrel_protocol::core::config::default_config_dir() {
            assert!(is_real_user_config_dir(Some(&real)));
            assert!(!is_real_user_config_dir(Some(&real.join("scratch"))));
        }
        assert!(!is_real_user_config_dir(Some(Path::new(
            "/tmp/lyrel-scratch"
        ))));
        assert!(!is_real_user_config_dir(None));
    }

    #[test]
    fn pinning_lands_outside_the_real_config_dir() {
        pin_test_config_dir();
        assert!(!is_real_user_config_dir(
            lyrel_protocol::core::config::config_dir_path().as_deref()
        ));
    }

    #[test]
    fn font_features_match_gpui_byte_for_byte() {
        const JSON: &str = r#"{"calt":true,"liga":1,"ss01":0,"zero":false,"bad":1,"kern":null}"#;

        let ours: FontFeatures = serde_json::from_str(JSON).unwrap();
        let theirs: gpui_kit::FontFeatures = serde_json::from_str(JSON).unwrap();

        assert_eq!(ours.tag_value_list(), theirs.tag_value_list());
        assert_eq!(ours.is_calt_enabled(), theirs.is_calt_enabled());
        assert_eq!(
            serde_json::to_string(&ours).unwrap(),
            serde_json::to_string(&theirs).unwrap()
        );
        assert_eq!(gpui_font_features(&ours), theirs);
    }
}
