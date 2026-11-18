//! The standalone updater's library half: the single authoritative owner of
//! the update contract shared by the GUI's update coordinator
//! (`lyrel::core::update`) and the `lyrel-updater` executable.
//!
//! Everything that used to be copy-pasted between the two sides lives here:
//! the installation-layout markers and backup naming (`markers`), the
//! `VerifyPlan`/`InstallPlan` argv construction and parsing (the platform
//! modules), and the terminal `UpdateOutcome` record the helper leaves for
//! the next GUI launch. The executable in `main.rs` is a thin dispatch over
//! the platform modules; the GUI links only this library.

pub mod outcome;

#[cfg(target_os = "macos")]
pub mod macos;
#[cfg(target_os = "windows")]
pub mod markers;
#[cfg(target_os = "windows")]
pub mod windows;

#[cfg(any(target_os = "macos", target_os = "windows"))]
use std::ffi::OsString;
#[cfg(any(target_os = "macos", target_os = "windows"))]
use std::path::Path;
#[cfg(any(target_os = "macos", target_os = "windows"))]
use std::path::PathBuf;

/// The named options every install verb accepts after its positional
/// arguments.
///
/// Everything the caller needs the updater process to know travels this way —
/// never through the environment. An elevated (UAC) child does not inherit
/// the spawning GUI's environment, so a `LYREL_CONFIG_DIR` set there would
/// silently fall back to the *administrator's* config directory under an
/// over-the-shoulder elevation (#504).
#[cfg(any(target_os = "macos", target_os = "windows"))]
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TailOptions {
    pub config_dir: Option<PathBuf>,
    pub result_file: Option<PathBuf>,
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
impl TailOptions {
    /// The flags in the canonical order the GUI emits them.
    pub fn argv(&self) -> Vec<OsString> {
        let mut args = Vec::new();
        if let Some(dir) = &self.config_dir {
            args.push(OsString::from("--config-dir"));
            args.push(dir.as_os_str().to_os_string());
        }
        if let Some(outcome) = &self.result_file {
            args.push(OsString::from("--result-file"));
            args.push(outcome.as_os_str().to_os_string());
        }
        args
    }

    /// Parses the two flags this type owns out of `args`, consuming them.
    /// Returns `Ok(false)` when `flag` is neither, leaving `args` untouched
    /// past it.
    pub(crate) fn parse_flag(
        &mut self,
        flag: &str,
        args: &mut impl Iterator<Item = OsString>,
        usage: impl Fn() -> String,
    ) -> Result<bool, String> {
        match flag {
            "--config-dir" => self.config_dir = Some(next_path(args, &usage)?),
            "--result-file" => self.result_file = Some(next_path(args, &usage)?),
            _ => return Ok(false),
        }
        Ok(true)
    }

    /// Applies the config-directory override to this process and re-exports
    /// it for the children this process spawns. Callers invoke this only
    /// once argument parsing has finished, before any thread exists.
    pub fn apply(&self) {
        if let Some(dir) = &self.config_dir {
            apply_config_dir(dir);
        }
    }
}

/// The override the `--config-dir` argument carries: this process answers for
/// the caller's config directory, and so does every child it spawns — the
/// relaunched app above all.
#[cfg(any(target_os = "macos", target_os = "windows"))]
pub fn apply_config_dir(dir: &Path) {
    lyrel_core::core::config::set_config_dir(dir.to_path_buf());
    // Re-exported so children of this process keep answering for the same
    // config directory. Safe here: argument parsing runs before any thread
    // exists.
    unsafe { std::env::set_var("LYREL_CONFIG_DIR", dir) };
}

/// The terminal outcome of the attempt, for the next GUI launch to merge into
/// the update state (#540). Best-effort: a result that cannot be recorded
/// goes to the log like every other updater detail. On paths that relaunch
/// the previous app this runs *before* the relaunch, so the GUI finds the
/// outcome already on disk when it starts.
#[cfg(any(target_os = "macos", target_os = "windows"))]
pub fn report_outcome(
    result_file: Option<&Path>,
    log: &Path,
    version: &str,
    result: &Result<(), String>,
) {
    let Some(path) = result_file else { return };
    let outcome = outcome::UpdateOutcome {
        version: version.to_string(),
        ok: result.is_ok(),
        detail: result.as_ref().err().cloned(),
    };
    if let Err(error) = outcome::write_outcome(path, &outcome) {
        log_line(
            log,
            &format!(
                "could not record the update outcome at {}: {error}",
                path.display()
            ),
        );
    }
}

/// One line on a best-effort log; both platform modules share it so the log
/// format stays uniform.
#[cfg(any(target_os = "macos", target_os = "windows"))]
pub fn log_line(path: &Path, message: &str) {
    use std::io::Write as _;
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(file, "{message}");
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
pub(crate) fn next_path(
    args: &mut impl Iterator<Item = OsString>,
    usage: impl Fn() -> String,
) -> Result<PathBuf, String> {
    args.next().map(PathBuf::from).ok_or_else(usage)
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
pub(crate) fn next_string(
    args: &mut impl Iterator<Item = OsString>,
    usage: impl Fn() -> String,
) -> Result<String, String> {
    args.next()
        .and_then(|arg| arg.into_string().ok())
        .ok_or_else(usage)
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
pub(crate) fn next_pid(
    args: &mut impl Iterator<Item = OsString>,
    usage: impl Fn() -> String,
) -> Result<u32, String> {
    next_string(args, usage)?
        .parse::<u32>()
        .map_err(|_| "parent pid is not an unsigned integer".to_string())
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
pub(crate) fn reject_extra(
    args: &mut impl Iterator<Item = OsString>,
    usage: impl Fn() -> String,
) -> Result<(), String> {
    if args.next().is_some() {
        Err(usage())
    } else {
        Ok(())
    }
}

/// What `capabilities` prints, one per line — the verbs the GUI requires
/// before it points a UAC prompt at the installed updater. An updater that
/// predates the verbs never prints them.
#[cfg(target_os = "windows")]
pub const ELEVATION_CAPABILITIES: [&str; 3] =
    ["elevated-stage", "install-elevated", "relaunch-watcher"];

/// Whether a `capabilities` answer covers the elevation chain. The pure half
/// of the GUI's probe, kept next to the list this crate prints so the two
/// cannot drift apart.
#[cfg(target_os = "windows")]
pub fn capabilities_cover_elevation(stdout: &[u8]) -> bool {
    let Ok(text) = std::str::from_utf8(stdout) else {
        return false;
    };
    let tokens: std::collections::HashSet<&str> = text.lines().map(str::trim).collect();
    ELEVATION_CAPABILITIES
        .iter()
        .all(|capability| tokens.contains(capability))
}

#[cfg(all(test, any(target_os = "macos", target_os = "windows")))]
mod tests {
    use super::*;

    /// The tail flags round-trip through the same parser the executable's
    /// verbs use (each platform's TailOptions composes this one).
    #[test]
    fn tail_options_round_trip_through_argv_and_parse() {
        let options = TailOptions {
            config_dir: Some(PathBuf::from("/tmp/lyrel cfg")),
            result_file: Some(PathBuf::from("/tmp/lyrel cfg/update-outcome.json")),
        };
        let argv = options.argv();
        assert_eq!(
            argv,
            [
                OsString::from("--config-dir"),
                OsString::from("/tmp/lyrel cfg"),
                OsString::from("--result-file"),
                OsString::from("/tmp/lyrel cfg/update-outcome.json"),
            ]
        );

        let mut parsed = TailOptions::default();
        let mut args = argv.into_iter();
        while let Some(flag) = args.next() {
            assert!(
                parsed
                    .parse_flag(flag.to_str().unwrap(), &mut args, || "usage".to_string())
                    .unwrap(),
                "every emitted flag is recognised"
            );
        }
        assert_eq!(parsed, options);
        // Each flag stands alone; a machine with no resolvable config dir
        // simply passes neither, which is the updater's default.
        assert!(TailOptions::default().argv().is_empty());
    }
}
