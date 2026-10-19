//! Putting the bundled `lyrel` CLI on PATH, without asking and without an entry
//! point to click.
//!
//! The GUI and the CLI ship as one artifact but are two binaries: the installer
//! lays `lyrel` down beside `lyrel-app` (inside `Contents/MacOS/` on macOS, in the
//! install directory elsewhere) and neither one is on PATH by virtue of being
//! there. Rather than a "Install shell command…" menu item that most people
//! never find, the GUI links it up itself on every launch — cheap enough to run
//! unconditionally, idempotent once it has succeeded.
//!
//! Two platform shapes, for reasons that are not symmetric:
//!
//! * **macOS** — symlink the CLI into a directory that is already on PATH. The
//!   alternative, putting our own directory on PATH, would mean editing the
//!   user's shell rc: a macOS GUI app inherits nothing from the login shell and
//!   cannot export into it. Writing to someone's `.zshrc` is a far bigger thing
//!   to do unprompted than dropping one symlink.
//! * **Windows** — the reverse. Symlinks need Developer Mode or elevation, and
//!   there is no conventional user-writable bin directory on PATH to link into.
//!   `HKCU\Environment` is the native answer and needs no privileges.
//!
//! Nothing here is fatal. Every failure path logs and returns; a user whose
//! system resists all of it still has a working GUI, just no `lyrel` on PATH.
//!
//! Undoing it is asymmetric too. The Windows uninstaller strips the PATH entry
//! back out (see the `[Code]` section of `windows-installer.iss`). macOS has no
//! uninstall hook to hang that off — dragging the `.app` to the Trash leaves
//! the symlink behind, dangling. An upgrade heals
//! it (a dangling link still names `lyrel`, so the next launch repoints it); a
//! real uninstall leaves one broken entry the user removes by hand.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

/// What a run of [`install`] did, for the log line and for tests.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Turned off in config.
    Disabled,
    /// No CLI beside the GUI — a hand-assembled tree, or a stripped bundle.
    NoBundledCli,
    /// A debug build, or a binary sitting in a cargo build tree: panes were
    /// wired up, the system was left untouched.
    DevBuild,
    /// Already reachable as `lyrel`, pointing at this install.
    AlreadyInstalled(PathBuf),
    /// Freshly linked into a directory on PATH.
    Installed(PathBuf),
    /// Installed somewhere the user's PATH does not currently cover.
    #[cfg_attr(not(unix), allow(dead_code))]
    InstalledOffPath(PathBuf),
    /// Installed, but an earlier PATH entry holds a different `lyrel` that keeps
    /// winning the lookup.
    InstalledShadowed { ours: PathBuf, winner: PathBuf },
    /// Every directory we would write to is already taken by someone else's
    /// `lyrel`, and none of them is ours to move.
    #[cfg_attr(not(unix), allow(dead_code))]
    Occupied(PathBuf),
    /// Nowhere to write.
    Failed(String),
}

#[cfg(windows)]
const CLI_NAME: &str = "lyrel.exe";
#[cfg(not(windows))]
const CLI_NAME: &str = "lyrel";

/// Link the bundled CLI onto PATH, and make it reachable from panes right away.
///
/// Call this *before* the daemon is spawned: panes inherit their environment
/// from the daemon, which inherits it from this process, so the PATH entry
/// added here reaches every shell opened in this session — including the very
/// first one, and including the case where the on-disk half below fails
/// outright.
///
/// Takes the config flag rather than loading it, so startup reads `config.json`
/// once for this and the daemon decision that follows it.
pub fn install(enabled: bool) -> Outcome {
    let outcome = install_inner(enabled);
    match &outcome {
        Outcome::Disabled | Outcome::NoBundledCli | Outcome::DevBuild => {
            log::debug!("cli install skipped: {outcome:?}")
        }
        Outcome::AlreadyInstalled(p) => log::debug!("lyrel CLI already on PATH at {}", p.display()),
        Outcome::Installed(p) => log::info!("put the lyrel CLI on PATH at {}", p.display()),
        Outcome::InstalledOffPath(p) => log::warn!(
            "installed the lyrel CLI at {}, which is not on your PATH — add it to use `lyrel` \
             outside a lyrel pane",
            p.display()
        ),
        Outcome::InstalledShadowed { ours, winner } => log::warn!(
            "installed the lyrel CLI at {}, but `lyrel` outside a lyrel pane still resolves to {} — \
             remove that one, or reorder your PATH, to reach the bundled CLI",
            ours.display(),
            winner.display()
        ),
        Outcome::Occupied(p) => log::info!(
            "leaving the existing `lyrel` at {} alone; the bundled CLI was not installed",
            p.display()
        ),
        Outcome::Failed(e) => log::warn!("could not put the lyrel CLI on PATH: {e}"),
    }
    outcome
}

fn install_inner(enabled: bool) -> Outcome {
    if !enabled {
        return Outcome::Disabled;
    }
    let Some(cli) = bundled_cli() else {
        return Outcome::NoBundledCli;
    };
    // Snapshot PATH *before* the prepend below, so the shadow check at the end
    // asks "what would the user's shell have found", not "what did we just put
    // in front of everything".
    let user_path = path_dirs();

    // Panes reach the CLI through the daemon's inherited environment even when
    // the on-disk half below is refused, so do this first and unconditionally.
    if let Some(dir) = cli.parent() {
        prepend_to_process_path(dir);
    }
    // A dev build gets the environment half and nothing else. A build tree
    // holds a `lyrel` too, so without this a `cargo run` would point the user's
    // real `lyrel` at a binary the next `cargo clean` deletes — and the isolated
    // instances the dev-verify flow spins up would each rewrite the PATH of the
    // machine they are meant to be kept away from. Panes still get the build
    // under test, which is the half that development actually needs.
    if cfg!(debug_assertions) || in_a_build_tree(&cli) {
        return Outcome::DevBuild;
    }

    let outcome = platform_install(&cli, &user_path);

    // Writing the file is not the same as winning the lookup. `user_path` is a
    // snapshot of the *directories*, not of their contents, so scanning it now
    // sees whatever we just wrote sitting in its real PATH position: find
    // ourselves and there is no shadow, find someone else and there is.
    //
    // This is the only report Windows gets. It appends to the user's PATH
    // rather than placing a file, so it never collides with another `lyrel` and
    // never has a reason to say `Occupied` — but an existing one earlier on
    // PATH still beats it, and "installed" alone would be a lie.
    let ours = match &outcome {
        Outcome::AlreadyInstalled(p) | Outcome::Installed(p) | Outcome::InstalledOffPath(p) => {
            p.clone()
        }
        _ => return outcome,
    };
    match first_cli_on(&user_path) {
        Some(winner) if winner != ours => Outcome::InstalledShadowed { ours, winner },
        _ => outcome,
    }
}

/// The first `lyrel` the user's shell would find, if any.
///
/// `is_file` follows symlinks on purpose: a dangling link left by an install
/// that has since been deleted is not something that wins a lookup, so it must
/// not count as a shadow.
fn first_cli_on(dirs: &[PathBuf]) -> Option<PathBuf> {
    dirs.iter()
        .map(|d| d.join(CLI_NAME))
        .find(|candidate| candidate.is_file())
}

fn path_dirs() -> Vec<PathBuf> {
    std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default()
}

/// The CLI shipped alongside this GUI, if there is one.
///
/// Resolved relative to the running executable rather than searched for: the
/// point is to install *this build's* CLI, and a PATH search would find
/// whatever is already installed — including the symlink we made last launch,
/// which would then chase its own tail.
fn bundled_cli() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let cli = exe.parent()?.join(CLI_NAME);
    // `is_file` and not `exists`: on Unix the answer must be a real binary we
    // can exec, and a stale directory of that name should read as "absent".
    cli.is_file().then_some(cli)
}

/// Whether this executable is sitting in a cargo build directory.
///
/// `cfg!(debug_assertions)` alone catches `cargo run` but not
/// `cargo run --release`, which would otherwise aim the developer's real `lyrel`
/// at a build tree. Matches both `target/release/` and the
/// `target/<triple>/release/` shape that `--target` produces.
fn in_a_build_tree(cli: &Path) -> bool {
    let Some(profile_dir) = cli.parent() else {
        return false;
    };
    let named_after_a_profile = profile_dir
        .file_name()
        .is_some_and(|n| n == "debug" || n == "release");
    named_after_a_profile
        && profile_dir
            .ancestors()
            .any(|a| a.file_name() == Some("target".as_ref()))
}

/// Make the CLI reachable from this process's children (the daemon, and so
/// every pane) without waiting for the on-disk install to take effect.
fn prepend_to_process_path(dir: &Path) {
    let current = std::env::var_os("PATH").unwrap_or_default();
    match path_with_dir_first(&current, dir) {
        // SAFETY: single-threaded startup — this runs from `main` before the
        // daemon is spawned and before gpui's executor exists, so there is no
        // concurrent reader of the environment.
        Some(Ok(path)) => unsafe { std::env::set_var("PATH", path) },
        Some(Err(e)) => log::warn!("could not extend PATH with {}: {e}", dir.display()),
        None => {}
    }
}

/// The PATH `dir` belongs at the front of, or `None` when it is already listed.
///
/// Prepended rather than appended so it wins over a stale copy left on PATH by
/// an older install — inside a lyrel pane, `lyrel` should mean the lyrel you are
/// sitting in.
///
/// Split out from the `set_var` above so the joining rule can be tested without
/// a test mutating the process environment out from under its neighbours.
fn path_with_dir_first(
    current: &OsStr,
    dir: &Path,
) -> Option<Result<OsString, std::env::JoinPathsError>> {
    if std::env::split_paths(current).any(|p| p == dir) {
        return None;
    }
    let joined = std::iter::once(dir.to_path_buf())
        .chain(std::env::split_paths(current))
        .collect::<Vec<_>>();
    Some(std::env::join_paths(joined))
}

// ---- Unix ------------------------------------------------------------------

#[cfg(unix)]
fn platform_install(cli: &Path, user_path: &[PathBuf]) -> Outcome {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let candidates = candidate_dirs(user_path, home.as_deref());

    let mut occupied = None;
    let mut last_error = None;
    for dir in &candidates {
        match place(dir, cli) {
            Ok(Placement::Already(p)) => return Outcome::AlreadyInstalled(p),
            Ok(Placement::Wrote(p)) => {
                return if user_path.contains(dir) {
                    Outcome::Installed(p)
                } else {
                    Outcome::InstalledOffPath(p)
                };
            }
            // Someone else's `lyrel` lives here. Keep going rather than giving
            // up: a later candidate may be free, and if ours still loses the
            // lookup the shadow check in `install_inner` says so.
            Ok(Placement::Occupied(p)) => {
                occupied.get_or_insert(p);
            }
            Err(e) => last_error = Some(format!("{}: {e}", dir.display())),
        }
    }
    if let Some(p) = occupied {
        return Outcome::Occupied(p);
    }
    Outcome::Failed(last_error.unwrap_or_else(|| "no writable directory on PATH".into()))
}

/// The directories worth linking into, best first.
///
/// Deliberately a fixed list intersected with PATH rather than "the first
/// writable directory on PATH". Version-manager shim directories — pyenv,
/// rbenv, asdf, mise — sit at the *front* of PATH on a great many machines and
/// are writable, which makes them exactly what a first-writable scan picks. A
/// binary dropped there survives until that tool next rehashes and deletes
/// every file it did not put there. The failure is silent and arrives days
/// later, so the safe set is enumerated instead of discovered.
///
/// `~/.local/bin` is the fallback and is offered even when PATH does not list
/// it: an unreachable install the log names is a better outcome than no install
/// at all, and it is the one directory here we can always create.
///
/// `home` is a parameter rather than a `$HOME` read so tests can exercise this
/// without mutating the environment of every test running beside them.
#[cfg(unix)]
fn candidate_dirs(path_dirs: &[PathBuf], home: Option<&Path>) -> Vec<PathBuf> {
    let under_home = |rel: &str| home.map(|h| h.join(rel));

    let preferred: Vec<PathBuf> = [
        Some(PathBuf::from("/opt/homebrew/bin")),
        Some(PathBuf::from("/usr/local/bin")),
        under_home(".local/bin"),
        under_home("bin"),
        under_home(".cargo/bin"),
    ]
    .into_iter()
    .flatten()
    .collect();

    let mut out: Vec<PathBuf> = preferred
        .iter()
        .filter(|d| path_dirs.contains(d))
        .cloned()
        .collect();
    if let Some(fallback) = under_home(".local/bin").filter(|f| !out.contains(f)) {
        out.push(fallback);
    }
    out
}

#[cfg(unix)]
enum Placement {
    Already(PathBuf),
    Wrote(PathBuf),
    Occupied(PathBuf),
}

#[cfg(unix)]
fn place(dir: &Path, cli: &Path) -> std::io::Result<Placement> {
    std::fs::create_dir_all(dir)?;
    let target = dir.join(CLI_NAME);

    match std::fs::symlink_metadata(&target) {
        Ok(meta) if meta.file_type().is_symlink() => {
            let points_at = std::fs::read_link(&target)?;
            if points_at == cli {
                return Ok(Placement::Already(target));
            }
            // Replace only a link that is still aimed at something named
            // `lyrel`. Anything else under this name was pointed somewhere
            // deliberate by its owner, and an auto-installer is not the thing
            // that gets to overrule that.
            if points_at.file_name() != Some(CLI_NAME.as_ref()) {
                return Ok(Placement::Occupied(target));
            }
        }
        // A real binary belongs to its installer or its user, not this GUI.
        Ok(_) => return Ok(Placement::Occupied(target)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }

    write_atomically(dir, &target, cli)?;
    Ok(Placement::Wrote(target))
}

/// Write through a temporary name and rename over the target.
///
/// `std::os::unix::fs::symlink` fails outright if the destination exists, and
/// unlink-then-create leaves a window in which `lyrel` resolves to nothing. The
/// rename is atomic, so a concurrent shell either sees the old entry or the new
/// one — never neither.
#[cfg(unix)]
fn write_atomically(dir: &Path, target: &Path, cli: &Path) -> std::io::Result<()> {
    // The temp name carries the pid so two lyrel instances starting together
    // cannot collide on it.
    let tmp = dir.join(format!(".{CLI_NAME}.{}.tmp", std::process::id()));
    let _ = std::fs::remove_file(&tmp);

    let result = std::os::unix::fs::symlink(cli, &tmp);
    if let Err(e) = result {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    if let Err(e) = std::fs::rename(&tmp, target) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(())
}

// ---- Windows ---------------------------------------------------------------

/// The `Path` value to write back, or `None` when `dir` is already listed.
///
/// UTF-16 in and UTF-16 out. Round-tripping the user's PATH through `String`
/// would run it past a lossy conversion, and a value the registry holds but
/// Rust cannot represent would come back with `U+FFFD` where its characters
/// used to be — the exact "installer permanently corrupts a PATH" failure the
/// rest of this function is careful to avoid.
///
/// Built platform-independently so the joining and matching rules are testable
/// away from a real registry.
#[cfg(any(windows, test))]
fn user_path_with_dir(existing: &[u16], dir: &[u16]) -> Option<Vec<u16>> {
    const SEMICOLON: u16 = b';' as u16;
    const BACKSLASH: u16 = b'\\' as u16;

    fn trim_trailing(s: &[u16], c: u16) -> &[u16] {
        &s[..s.iter().rposition(|&x| x != c).map_or(0, |i| i + 1)]
    }
    // ASCII folding only. Drive letters and separators are all that has to
    // match case-insensitively here, and full Unicode case folding on a PATH
    // entry would be a way to make two distinct directories compare equal.
    fn fold(c: u16) -> u16 {
        const UPPER: std::ops::RangeInclusive<u16> = (b'A' as u16)..=(b'Z' as u16);
        if UPPER.contains(&c) { c + 32 } else { c }
    }
    fn same_entry(a: &[u16], b: &[u16]) -> bool {
        let (a, b) = (trim_trailing(a, BACKSLASH), trim_trailing(b, BACKSLASH));
        a.len() == b.len() && a.iter().zip(b).all(|(&x, &y)| fold(x) == fold(y))
    }

    if existing
        .split(|&c| c == SEMICOLON)
        .any(|e| same_entry(e, dir))
    {
        return None;
    }

    // A trailing `;` is legal but leaves an empty entry, which some tools read
    // as "the current directory" — trim before joining.
    let head = trim_trailing(existing, SEMICOLON);

    let mut out = Vec::with_capacity(head.len() + 1 + dir.len());
    out.extend_from_slice(head);
    if !head.is_empty() {
        out.push(SEMICOLON);
    }
    out.extend_from_slice(dir);
    Some(out)
}

/// Append the CLI's directory to the *user's* PATH in the registry.
///
/// Reads `HKCU\Environment` rather than the process PATH on purpose. The
/// process value is the machine and user PATHs already merged, so writing it
/// back into the user hive would copy every system entry into HKCU — the
/// classic way installers permanently corrupt a PATH.
#[cfg(windows)]
fn platform_install(cli: &Path, _user_path: &[PathBuf]) -> Outcome {
    use std::os::windows::ffi::OsStrExt as _;
    use windows_sys::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, KEY_READ, KEY_WRITE, REG_EXPAND_SZ, REG_SZ, RegCloseKey,
        RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        HWND_BROADCAST, SMTO_ABORTIFHUNG, SendMessageTimeoutW, WM_SETTINGCHANGE,
    };

    let Some(dir) = cli.parent() else {
        return Outcome::Failed("the CLI has no parent directory".into());
    };

    let wide = |s: &OsStr| {
        s.encode_wide()
            .chain(std::iter::once(0))
            .collect::<Vec<u16>>()
    };
    let wide_str = |s: &str| wide(OsStr::new(s));

    let subkey = wide_str("Environment");
    let value_name = wide_str("Path");
    // Without its terminator: this one is data to be matched and joined, not a
    // string handed to the API.
    let dir_wide: Vec<u16> = dir.as_os_str().encode_wide().collect();
    let mut key: HKEY = std::ptr::null_mut();

    // SAFETY: all pointers below are to live locals, and every out-parameter is
    // initialised before the call. The key is closed on every return path.
    unsafe {
        if RegOpenKeyExW(
            HKEY_CURRENT_USER,
            subkey.as_ptr(),
            0,
            KEY_READ | KEY_WRITE,
            &mut key,
        ) != 0
        {
            return Outcome::Failed("could not open HKCU\\Environment".into());
        }

        // Read the current value. A missing `Path` is normal on a fresh
        // profile and means we are writing the first entry, not an error.
        let mut kind = 0u32;
        let mut bytes = 0u32;
        let existing: Vec<u16> = if RegQueryValueExW(
            key,
            value_name.as_ptr(),
            std::ptr::null_mut(),
            &mut kind,
            std::ptr::null_mut(),
            &mut bytes,
        ) == 0
        {
            let mut buf = vec![0u16; (bytes as usize).div_ceil(2)];
            if RegQueryValueExW(
                key,
                value_name.as_ptr(),
                std::ptr::null_mut(),
                &mut kind,
                buf.as_mut_ptr().cast(),
                &mut bytes,
            ) != 0
            {
                RegCloseKey(key);
                return Outcome::Failed("could not read the user PATH".into());
            }
            // Registry strings may or may not include their NUL.
            while buf.last() == Some(&0) {
                buf.pop();
            }
            buf
        } else {
            // Preserve REG_EXPAND_SZ if that is what was there; a fresh value
            // is a plain string.
            kind = REG_SZ;
            Vec::new()
        };

        let Some(mut updated) = user_path_with_dir(&existing, &dir_wide) else {
            RegCloseKey(key);
            return Outcome::AlreadyInstalled(cli.to_path_buf());
        };
        updated.push(0);

        let kind = if kind == REG_EXPAND_SZ {
            REG_EXPAND_SZ
        } else {
            REG_SZ
        };
        let written = RegSetValueExW(
            key,
            value_name.as_ptr(),
            0,
            kind,
            updated.as_ptr().cast(),
            (updated.len() * 2) as u32,
        );
        RegCloseKey(key);
        if written != 0 {
            return Outcome::Failed("could not write the user PATH".into());
        }

        // Without this, only processes started after the next sign-out pick the
        // change up: Explorer caches the environment it hands to what it
        // launches. The timeout keeps a hung top-level window from stalling
        // startup — the write already landed, so this is best-effort.
        let env = wide_str("Environment");
        SendMessageTimeoutW(
            HWND_BROADCAST,
            WM_SETTINGCHANGE,
            0,
            env.as_ptr() as isize,
            SMTO_ABORTIFHUNG,
            5_000,
            std::ptr::null_mut(),
        );
    }

    Outcome::Installed(cli.to_path_buf())
}

#[cfg(not(any(unix, windows)))]
fn platform_install(_cli: &Path, _user_path: &[PathBuf]) -> Outcome {
    Outcome::Failed("unsupported platform".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    fn from_wide(s: &[u16]) -> String {
        String::from_utf16(s).unwrap()
    }

    #[test]
    fn a_build_tree_binary_is_recognised_under_both_profile_layouts() {
        for p in [
            "/home/dev/lyrel/target/debug/lyrel",
            "/home/dev/lyrel/target/release/lyrel",
            "/home/dev/lyrel/target/aarch64-apple-darwin/release/lyrel",
        ] {
            assert!(in_a_build_tree(Path::new(p)), "{p} should read as a build");
        }
        for p in [
            "/Applications/lyrel.app/Contents/MacOS/lyrel",
            "/opt/lyrel/lyrel",
            "/usr/local/bin/lyrel",
            // `release` with no `target` above it is somebody's install prefix.
            "/opt/lyrel/release/lyrel",
        ] {
            assert!(!in_a_build_tree(Path::new(p)), "{p} should read as shipped");
        }
    }

    #[test]
    fn a_directory_already_on_path_is_not_prepended_twice() {
        let current = std::env::join_paths(["/usr/bin", "/opt/lyrel", "/bin"]).unwrap();
        assert!(path_with_dir_first(&current, Path::new("/opt/lyrel")).is_none());

        let added = path_with_dir_first(&current, Path::new("/opt/new"))
            .expect("a fresh directory is added")
            .expect("the join succeeds");
        let dirs: Vec<PathBuf> = std::env::split_paths(&added).collect();
        assert_eq!(dirs.first(), Some(&PathBuf::from("/opt/new")));
        assert_eq!(dirs.len(), 4);
    }

    #[test]
    fn the_user_path_gains_the_directory_once_and_losslessly() {
        // An unpaired surrogate: legal in the registry, not representable as a
        // Rust `String`. It must come back out byte for byte.
        let mut existing = wide("C:\\bin;C:\\weird");
        existing.push(0xD800);

        let updated = user_path_with_dir(&existing, &wide("C:\\lyrel")).expect("a new entry");
        assert_eq!(
            &updated[..existing.len()],
            &existing[..],
            "mangled the tail"
        );
        assert_eq!(&updated[existing.len()..], &wide(";C:\\lyrel")[..]);

        // Idempotent, and insensitive to case and to a trailing separator.
        assert!(user_path_with_dir(&updated, &wide("C:\\lyrel")).is_none());
        assert!(user_path_with_dir(&updated, &wide("c:\\LYREL")).is_none());
        assert!(user_path_with_dir(&updated, &wide("C:\\lyrel\\")).is_none());
    }

    #[test]
    fn a_trailing_separator_does_not_become_an_empty_path_entry() {
        let updated = user_path_with_dir(&wide("C:\\bin;;"), &wide("C:\\lyrel")).unwrap();
        assert_eq!(from_wide(&updated), "C:\\bin;C:\\lyrel");

        let fresh = user_path_with_dir(&[], &wide("C:\\lyrel")).unwrap();
        assert_eq!(from_wide(&fresh), "C:\\lyrel");
    }
}

#[cfg(all(test, unix))]
mod unix_tests {
    use super::*;

    fn touch(p: &Path) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, b"#!/bin/sh\n").unwrap();
    }

    fn tmpdir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("lyrel-cli-install-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_shim_directory_on_path_is_never_chosen() {
        // pyenv's shims are writable and come first on PATH; picking them would
        // get the binary deleted on the next rehash.
        let home = tmpdir("shims");
        let shims = home.join(".pyenv/shims");
        let local = home.join(".local/bin");

        let chosen = candidate_dirs(&[shims.clone(), local.clone()], Some(&home));
        assert!(!chosen.contains(&shims), "shim dir was offered: {chosen:?}");
        assert_eq!(chosen.first(), Some(&local));
    }

    #[test]
    fn an_unrelated_binary_named_lyrel_is_left_alone() {
        let dir = tmpdir("occupied");
        let bin = tmpdir("occupied-src").join("lyrel");
        touch(&bin);
        // Someone's own build, installed by hand.
        touch(&dir.join("lyrel"));

        match place(&dir, &bin).unwrap() {
            Placement::Occupied(p) => assert_eq!(p, dir.join("lyrel")),
            Placement::Already(_) => panic!("claimed someone else's binary as ours"),
            Placement::Wrote(_) => panic!("clobbered a real binary"),
        }
    }

    #[test]
    fn our_own_link_is_recognised_and_then_repointed_on_upgrade() {
        let dir = tmpdir("relink");
        let v1 = tmpdir("relink-v1").join("lyrel");
        let v2 = tmpdir("relink-v2").join("lyrel");
        touch(&v1);
        touch(&v2);

        assert!(matches!(place(&dir, &v1).unwrap(), Placement::Wrote(_)));
        // Second launch, same build: nothing to do.
        assert!(matches!(place(&dir, &v1).unwrap(), Placement::Already(_)));
        // Upgraded install: the link follows it rather than reporting a clash.
        assert!(matches!(place(&dir, &v2).unwrap(), Placement::Wrote(_)));
        assert_eq!(std::fs::read_link(dir.join("lyrel")).unwrap(), v2);
    }

    #[test]
    fn a_link_aimed_somewhere_deliberate_is_not_hijacked() {
        let dir = tmpdir("deliberate");
        let bin = tmpdir("deliberate-src").join("lyrel");
        touch(&bin);
        let elsewhere = tmpdir("deliberate-other").join("my-terminal");
        touch(&elsewhere);
        std::os::unix::fs::symlink(&elsewhere, dir.join("lyrel")).unwrap();

        assert!(matches!(place(&dir, &bin).unwrap(), Placement::Occupied(_)));
    }

    #[test]
    fn an_occupied_directory_does_not_end_the_search() {
        let taken = tmpdir("scan-taken");
        let free = tmpdir("scan-free");
        let bin = tmpdir("scan-src").join("lyrel");
        touch(&bin);
        touch(&taken.join("lyrel"));

        // Stand in for the candidate loop: the first directory is somebody
        // else's, and the second one must still get the link.
        let mut wrote = None;
        for dir in [&taken, &free] {
            if let Ok(Placement::Wrote(p)) = place(dir, &bin) {
                wrote = Some(p);
                break;
            }
        }
        assert_eq!(wrote, Some(free.join("lyrel")));
    }

    #[test]
    fn the_shadow_check_names_whoever_wins_the_lookup() {
        let early = tmpdir("shadow-early");
        let ours = tmpdir("shadow-ours");
        touch(&early.join("lyrel"));
        touch(&ours.join("lyrel"));

        let path = vec![early.clone(), ours.clone()];
        assert_eq!(first_cli_on(&path), Some(early.join("lyrel")));
        // Our own directory first: no shadow.
        assert_eq!(
            first_cli_on(&[ours.clone(), early.clone()]),
            Some(ours.join("lyrel"))
        );

        // A dangling link is not something that wins a lookup.
        let dangling = tmpdir("shadow-dangling");
        std::os::unix::fs::symlink(dangling.join("gone"), dangling.join("lyrel")).unwrap();
        assert_eq!(
            first_cli_on(&[dangling, ours.clone()]),
            Some(ours.join("lyrel"))
        );
    }
}
