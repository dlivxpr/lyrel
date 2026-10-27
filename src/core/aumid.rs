//! Windows toast branding: an App User Model ID (AUMID) of our own.
//!
//! Without one, notify-rust falls back to PowerShell's AUMID and every toast
//! shows the PowerShell icon and name. Windows only honors an unpackaged app's
//! AUMID when a Start Menu shortcut carries the matching
//! `System.AppUserModel.ID`, so `init()` — called once at GUI startup — decides
//! what identity to claim and, when nothing else already supplies that
//! shortcut, writes one.
//!
//! It is deliberately reluctant to write. The installer stamps its own
//! shortcuts (see `windows-installer.iss`), so the runtime write only has to
//! cover the portable zip and installs that predate that change. It therefore
//! touches at most the single per-user `lyrel.lnk` that Inno's default install
//! owns anyway — never a second Start Menu entry beside an all-users install
//! (which would show "lyrel" twice and outlive the uninstaller), and never
//! anything at all from a `cargo` build directory (which would repoint the
//! user's installed shortcut at `target\debug`).
//!
//! The process AUMID also decides which shortcut the taskbar button borrows
//! its icon from, so branding is not free: a build output that inherits a
//! stamped shortcut pointing at a deleted target paints the shell's generic
//! icon over the one compiled into the exe. `Decision::ToastOnly` keeps the
//! toast identity in that case while leaving the taskbar button to the exe.
//!
//! Everything is best-effort. `toast_app_id()` yields the AUMID only once a
//! shortcut carrying it is in place *and* the shell has had time to index it;
//! otherwise callers keep the PowerShell identity, which looks wrong but still
//! shows up. That distinction matters: an AUMID the shell has not indexed does
//! not make `Toast::show()` fail, it makes it return success and drop the
//! toast on the floor.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// The toast/taskbar identity. Keep in sync with the `AppUserModelID` on the
/// installer shortcuts in `.github/scripts/windows-installer.iss` — a
/// mismatch silently splits the identity in two.
pub(crate) const AUMID: &str = "io.github.dlivxpr.lyrel";

/// How long we keep using the PowerShell identity after writing the shortcut
/// ourselves. The shell picks a new `.lnk` up asynchronously and silently
/// discards toasts for an AUMID it has not indexed yet; the measured lag on
/// Windows 11 was a few seconds. Overshooting only costs an unbranded toast
/// in the opening seconds of a first-ever run, so the bound is generous.
const INDEX_GRACE: Duration = Duration::from_secs(30);

/// One-time GUI-startup hook; see the module docs. Cheap after the first call.
pub(crate) fn init() {
    let _ = toast_app_id();
}

/// The AUMID to put on toasts, when (and only when) a shortcut the shell has
/// seen carries it. Memoized — the first call does the COM work.
pub(crate) fn toast_app_id() -> Option<&'static str> {
    static STATE: OnceLock<Branding> = OnceLock::new();
    match STATE.get_or_init(setup) {
        Branding::Unavailable => None,
        Branding::Ready => Some(AUMID),
        Branding::Pending(written_at) => (written_at.elapsed() >= INDEX_GRACE).then_some(AUMID),
    }
}

enum Branding {
    /// Nothing carries our AUMID and we are not in a position to add it.
    Unavailable,
    /// A shortcut carrying it was already on disk before we started, so the
    /// shell has had it since long before this process existed.
    Ready,
    /// We wrote that shortcut just now — see `INDEX_GRACE`.
    Pending(Instant),
}

/// Set the process-level AppUserModelID. `setup` decides when this runs —
/// see `Decision::ToastOnly` for the one case it must not.
fn brand_process() {
    use windows::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID;
    unsafe {
        let _ = SetCurrentProcessExplicitAppUserModelID(&windows::core::HSTRING::from(AUMID));
    }
}

fn setup() -> Branding {
    let decision = decide();
    // Branding the process is what groups the taskbar button under our own
    // identity. It is safe in every decision except `ToastOnly` — the one
    // case where the registered identity resolves to a dead shortcut target,
    // so branding it would paint the shell's generic fallback icon over the
    // one compiled into this exe (build.rs). Everywhere else the group picks
    // up a live icon: our own shortcut, an install's, or this exe itself.
    if !matches!(decision, Ok(Decision::ToastOnly)) {
        brand_process();
    }
    match decision {
        Ok(Decision::Branded | Decision::ToastOnly) => Branding::Ready,
        Ok(Decision::Skip(why)) => {
            log::debug!("keeping the PowerShell toast identity: {why}");
            Branding::Unavailable
        }
        Ok(Decision::Write(lnk)) => match write_shortcut(&lnk) {
            Ok(()) => Branding::Pending(Instant::now()),
            Err(e) => {
                log::warn!("toast branding disabled, keeping the PowerShell identity: {e}");
                Branding::Unavailable
            }
        },
        Err(e) => {
            log::warn!("toast branding disabled, keeping the PowerShell identity: {e}");
            Branding::Unavailable
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Decision {
    /// A shortcut already carries our AUMID; write nothing.
    Branded,
    /// Build output may borrow the toast identity, but the shortcut carrying
    /// it points at a target that no longer exists — pinning our taskbar
    /// button to that identity would show the shell's generic fallback icon
    /// rather than the one compiled into this exe (build.rs). The process
    /// keeps the toast AUMID; it just does not claim the taskbar identity.
    ToastOnly,
    /// Create or refresh this per-user shortcut, in place.
    Write(PathBuf),
    /// Nothing we may safely write. The string is for the log.
    Skip(&'static str),
}

fn decide() -> Result<Decision, String> {
    let exe = std::env::current_exe().map_err(|e| format!("current exe: {e}"))?;
    decide_with(&exe, all_users_shortcut_path(), start_menu_shortcut_path())
}

/// The decision itself, with the three paths it consults already resolved so
/// tests can point them at temp files. `all_users_lnk` is only read when the
/// file exists; `per_user_lnk` is required — there is no writable shortcut
/// without it.
fn decide_with(
    exe: &Path,
    all_users_lnk: Option<PathBuf>,
    per_user_lnk: Option<PathBuf>,
) -> Result<Decision, String> {
    // An elevated install owns `%ProgramData%\...\lyrel.lnk`, which we cannot
    // rewrite unelevated. A per-user twin beside it would list "lyrel" twice in
    // the Start Menu and survive the uninstaller, so that file settles the
    // question on its own: branded if the installer stamped our AUMID on it,
    // unbranded until the user upgrades to an installer that does.
    if let Some(lnk) = all_users_lnk
        && lnk.is_file()
    {
        let shortcut = read_shortcut(&lnk).map_err(|e| format!("read {}: {e}", lnk.display()))?;
        let stamped = shortcut.aumid.as_deref() == Some(AUMID);
        return Ok(if !stamped {
            Decision::Skip("an all-users Start Menu shortcut owns the entry")
        } else if is_build_output(exe) && target_dead(&shortcut) {
            // Same failure mode as the per-user branch below: a build output
            // pinned to this identity would show the dead shortcut's generic
            // icon instead of its own embedded one.
            Decision::ToastOnly
        } else {
            Decision::Branded
        });
    }

    let lnk = per_user_lnk.ok_or("APPDATA is not set")?;
    let existing = if lnk.is_file() {
        Some(read_shortcut(&lnk).map_err(|e| format!("read {}: {e}", lnk.display()))?)
    } else {
        None
    };
    let stamped = existing
        .as_ref()
        .is_some_and(|s| s.aumid.as_deref() == Some(AUMID));
    let on_target = existing
        .as_ref()
        .and_then(|s| s.target.as_deref())
        .is_some_and(|t| same_path(t, exe));
    if stamped && on_target {
        return Ok(Decision::Branded);
    }

    if is_build_output(exe) {
        // `cargo run` must never repoint the installed Start Menu shortcut at
        // `target\debug`. If an install already left a stamped shortcut here,
        // toasts from the dev build still brand correctly off it — Windows
        // only asks that the AUMID be registered, not that it point at us.
        // The one exception is a stamped shortcut whose target is gone: the
        // taskbar resolves our button's icon through that shortcut, so a dev
        // window pinned to it shows the shell's generic icon rather than the
        // one compiled into this exe — ToastOnly keeps the toast identity
        // while `setup()` leaves the taskbar button to the exe's own icon.
        return Ok(if stamped {
            if existing.as_ref().is_some_and(target_dead) {
                Decision::ToastOnly
            } else {
                Decision::Branded
            }
        } else {
            Decision::Skip("running from a cargo build directory")
        });
    }

    Ok(Decision::Write(lnk))
}

/// True when `exe` sits in a `cargo` build directory rather than an install.
///
/// Two independent signals. The layout check is the offline half:
/// `target[\<triple>]\{debug,release}\lyrel-app.exe`. `CACHEDIR.TAG` is the
/// half that does not care about names — cargo writes it into every build
/// directory precisely to mark the tree as derived, so it also covers a
/// renamed `CARGO_TARGET_DIR` and the `target\...\deps\` binaries the test
/// harness runs from.
fn is_build_output(exe: &Path) -> bool {
    has_build_layout(exe)
        || exe
            .ancestors()
            .any(|dir| dir.join("CACHEDIR.TAG").is_file())
}

fn has_build_layout(exe: &Path) -> bool {
    fn named(dir: Option<&Path>, name: &str) -> bool {
        dir.and_then(Path::file_name)
            .is_some_and(|n| n.eq_ignore_ascii_case(name))
    }
    let profile = exe.parent();
    (named(profile, "debug") || named(profile, "release"))
        && exe.ancestors().any(|dir| named(Some(dir), "target"))
}

/// True only when the shortcut stores a target path that is confirmed gone.
/// `try_exists` keeps the three outcomes distinct: `false` is the dead link
/// `ToastOnly` exists for; an error (denied access, unresolved reparse point)
/// or a missing target field is ambiguous and answers `false` here so the
/// caller keeps the previous Branded behavior rather than dropping the
/// taskbar identity over an unreadable state. A raw target containing `%`
/// is ambiguous for the same reason: `SLGP_RAWPATH` returns the stored path
/// verbatim, which may hold unexpanded environment variables
/// (`%LOCALAPPDATA%\…`), and a literal-percent path always reads as missing
/// whether the real target is alive or not.
fn target_dead(shortcut: &Shortcut) -> bool {
    let Some(target) = shortcut.target.as_deref() else {
        return false;
    };
    if target.as_os_str().as_encoded_bytes().contains(&b'%') {
        return false;
    }
    matches!(target.try_exists(), Ok(false))
}

/// Windows paths are case-insensitive and a `.lnk` may hold a short (8.3) or
/// otherwise unnormalized form of the same file, so compare canonically and
/// only fall back to text when the target no longer exists.
fn same_path(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a.as_os_str().eq_ignore_ascii_case(b.as_os_str()),
    }
}

fn programs_dir(env_var: &str) -> Option<PathBuf> {
    Some(
        PathBuf::from(std::env::var_os(env_var)?)
            .join("Microsoft")
            .join("Windows")
            .join("Start Menu")
            .join("Programs"),
    )
}

/// The only shortcut we ever write: the per-user Start Menu, which is also
/// where Inno's default (non-elevated) install puts `lyrel.lnk` — so refreshing
/// it adds no entry the uninstaller does not already know how to remove.
fn start_menu_shortcut_path() -> Option<PathBuf> {
    Some(programs_dir("APPDATA")?.join("lyrel.lnk"))
}

/// The all-users twin an elevated install writes. Read-only for us.
fn all_users_shortcut_path() -> Option<PathBuf> {
    Some(programs_dir("ProgramData")?.join("lyrel.lnk"))
}

#[derive(Default)]
struct Shortcut {
    aumid: Option<String>,
    target: Option<PathBuf>,
}

/// Any prior COM init on this thread (`S_FALSE`) or another model
/// (`RPC_E_CHANGED_MODE`) is fine — we only need an initialized thread, and
/// `ShellLink` is apartment-threaded either way.
fn co_init() {
    use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    }
}

/// Read back the two properties `decide()` cares about. Absent ones come back
/// as `None`; only genuine COM failures are errors.
fn read_shortcut(lnk: &Path) -> Result<Shortcut, String> {
    use windows::Win32::Storage::EnhancedStorage::PKEY_AppUserModel_ID;
    use windows::Win32::System::Com::{
        CLSCTX_INPROC_SERVER, CoCreateInstance, IPersistFile, STGM_READ,
    };
    use windows::Win32::UI::Shell::PropertiesSystem::IPropertyStore;
    use windows::Win32::UI::Shell::{IShellLinkW, SLGP_RAWPATH, ShellLink};
    use windows::core::{BSTR, Interface};

    let lnk_w = windows::core::HSTRING::from(lnk);
    co_init();
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)
            .map_err(|e| format!("ShellLink: {e}"))?;
        let persist: IPersistFile = link.cast().map_err(|e| format!("IPersistFile: {e}"))?;
        persist
            .Load(&lnk_w, STGM_READ)
            .map_err(|e| format!("Load: {e}"))?;

        let store: IPropertyStore = link.cast().map_err(|e| format!("IPropertyStore: {e}"))?;
        // `BSTR::try_from` goes through `PropVariantToBSTR`, so it copes with
        // both the VT_LPWSTR the installer writes and the VT_BSTR we write.
        let aumid = store
            .GetValue(&PKEY_AppUserModel_ID)
            .ok()
            .and_then(|v| BSTR::try_from(&v).ok())
            .map(|s| s.to_string())
            .filter(|s| !s.is_empty());

        // SLGP_RAWPATH returns the stored target verbatim; without it the
        // shell may go looking for a moved file, including over the network.
        let mut buf = [0u16; 1024];
        let target = link
            .GetPath(&mut buf, std::ptr::null_mut(), SLGP_RAWPATH.0 as u32)
            .ok()
            .map(|()| {
                let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
                PathBuf::from(String::from_utf16_lossy(&buf[..len]))
            })
            .filter(|p| !p.as_os_str().is_empty());

        Ok(Shortcut { aumid, target })
    }
}

/// Create or overwrite `lnk`, pointing at the running exe and carrying our
/// AUMID. The exe has the icon compiled in (build.rs), which is what the toast
/// header shows.
fn write_shortcut(lnk: &Path) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| format!("current exe: {e}"))?;
    write_shortcut_for(lnk, Some(&exe), Some(AUMID))
}

/// The actual COM write, with the target and the AUMID stamped onto the
/// shortcut supplied by the caller. `aumid: None` leaves the property unset,
/// which is how tests build the "unstamped" fixtures `decide_with` must not
/// claim. `target: None` never calls `SetPath`, so the shortcut stores no
/// target at all — the case `decide_with` must not mistake for a dead one.
fn write_shortcut_for(
    lnk: &Path,
    target: Option<&Path>,
    aumid: Option<&str>,
) -> Result<(), String> {
    use windows::Win32::Storage::EnhancedStorage::PKEY_AppUserModel_ID;
    use windows::Win32::System::Com::{
        CLSCTX_INPROC_SERVER, CoCreateInstance, IPersistFile, StructuredStorage::PROPVARIANT,
    };
    use windows::Win32::UI::Shell::PropertiesSystem::IPropertyStore;
    use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
    use windows::core::Interface;

    if let Some(dir) = lnk.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    }
    let lnk_w = windows::core::HSTRING::from(lnk);

    co_init();
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)
            .map_err(|e| format!("ShellLink: {e}"))?;
        if let Some(target) = target {
            let target_w = windows::core::HSTRING::from(target);
            link.SetPath(&target_w)
                .map_err(|e| format!("SetPath: {e}"))?;
        }
        let store: IPropertyStore = link.cast().map_err(|e| format!("IPropertyStore: {e}"))?;
        if let Some(aumid) = aumid {
            store
                .SetValue(&PKEY_AppUserModel_ID, &PROPVARIANT::from(aumid))
                .map_err(|e| format!("SetValue: {e}"))?;
        }
        store.Commit().map_err(|e| format!("Commit: {e}"))?;
        let persist: IPersistFile = link.cast().map_err(|e| format!("IPersistFile: {e}"))?;
        persist
            .Save(&lnk_w, true)
            .map_err(|e| format!("Save: {e}"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    #[test]
    fn start_menu_shortcut_lives_under_programs() {
        let Some(lnk) = super::start_menu_shortcut_path() else {
            return; // no APPDATA in this environment — nothing to assert
        };
        assert_eq!(lnk.file_name().and_then(|n| n.to_str()), Some("lyrel.lnk"));
        assert_eq!(
            lnk.parent()
                .and_then(|p| p.file_name())
                .and_then(|n| n.to_str()),
            Some("Programs")
        );
    }

    /// The two Start Menu roots must be distinct files, or the "an all-users
    /// shortcut owns the entry" branch would swallow the per-user one too.
    #[test]
    fn the_two_start_menu_roots_are_distinct() {
        let (Some(user), Some(all)) = (
            super::start_menu_shortcut_path(),
            super::all_users_shortcut_path(),
        ) else {
            return;
        };
        assert_ne!(user, all);
    }

    #[test]
    fn cargo_layouts_are_recognized_as_build_output() {
        for exe in [
            r"C:\src\lyrel\target\debug\lyrel-app.exe",
            r"C:\src\lyrel\target\release\lyrel-app.exe",
            r"C:\src\lyrel\target\x86_64-pc-windows-msvc\release\lyrel-app.exe",
            r"C:\src\lyrel\TARGET\Debug\lyrel-app.exe",
        ] {
            assert!(
                super::has_build_layout(Path::new(exe)),
                "{exe} should look like a build directory"
            );
        }
    }

    #[test]
    fn install_layouts_are_not_build_output() {
        for exe in [
            r"C:\Program Files\lyrel\lyrel-app.exe",
            r"C:\Users\me\AppData\Local\Programs\lyrel\lyrel-app.exe",
            // Portable zip, extracted anywhere the user likes.
            r"D:\tools\lyrel\lyrel-app.exe",
            // A "release" *install* directory with no `target` above it.
            r"D:\lyrel\release\lyrel-app.exe",
        ] {
            let exe = Path::new(exe);
            assert!(!super::has_build_layout(exe), "{} misread", exe.display());
            assert!(!super::is_build_output(exe), "{} misread", exe.display());
        }
    }

    /// The test harness itself runs out of `target\...\deps\`, whose parent is
    /// neither `debug` nor `release` — so this is the `CACHEDIR.TAG` half of
    /// `is_build_output` proving itself against a real cargo layout.
    #[test]
    fn the_test_binary_counts_as_build_output() {
        let exe = std::env::current_exe().expect("current exe");
        assert!(
            super::is_build_output(&exe),
            "{} should be detected as a cargo build",
            exe.display()
        );
    }

    #[test]
    fn same_path_ignores_case_for_paths_that_do_not_exist() {
        assert!(super::same_path(
            Path::new(r"C:\Program Files\lyrel\LYREL-APP.EXE"),
            Path::new(r"c:\program files\lyrel\lyrel-app.exe"),
        ));
        assert!(!super::same_path(
            Path::new(r"C:\Program Files\lyrel\lyrel-app.exe"),
            Path::new(r"C:\src\lyrel\target\debug\lyrel-app.exe"),
        ));
    }

    /// Round-trips a shortcut through the real shell into a temp directory:
    /// `write_shortcut` must produce something `read_shortcut` recognizes as
    /// ours, or `decide()` would rewrite it on every single launch. Touches no
    /// Start Menu.
    #[test]
    fn a_written_shortcut_reads_back_as_ours() {
        let dir = std::env::temp_dir().join(format!("lyrel-aumid-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let lnk = dir.join("lyrel.lnk");

        let written = super::write_shortcut(&lnk);
        let read = written.as_ref().ok().map(|()| super::read_shortcut(&lnk));
        let _ = std::fs::remove_dir_all(&dir);

        written.expect("write shortcut");
        let shortcut = read.expect("read attempted").expect("read shortcut");
        assert_eq!(shortcut.aumid.as_deref(), Some(super::AUMID));
        let exe = std::env::current_exe().expect("current exe");
        let target = shortcut.target.unwrap_or_default();
        assert!(
            super::same_path(&target, &exe),
            "target {} should be {}",
            target.display(),
            exe.display()
        );
    }

    /// A `.lnk` in a temp dir, stamped and pointed wherever the test needs.
    fn lnk_fixture(dir: &Path, target: Option<&Path>, aumid: Option<&str>) -> std::path::PathBuf {
        let lnk = dir.join("lyrel.lnk");
        super::write_shortcut_for(&lnk, target, aumid).expect("write shortcut fixture");
        lnk
    }

    /// The regression behind this module's `ToastOnly`: a stamped shortcut
    /// whose target is gone must not pin a build output's taskbar button to
    /// the lyrel identity — the shell resolves that identity's icon through
    /// the dead shortcut and paints the generic fallback over the icon the
    /// exe actually carries.
    #[test]
    fn build_output_with_dead_stamped_target_is_toast_only() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let dead_target = dir.path().join("long-gone-install.exe");
        let lnk = lnk_fixture(dir.path(), Some(&dead_target), Some(super::AUMID));
        let exe = std::env::current_exe().expect("current exe");
        assert!(
            super::is_build_output(&exe),
            "test exe should read as build output"
        );
        assert_eq!(
            super::decide_with(&exe, None, Some(lnk)),
            Ok(super::Decision::ToastOnly)
        );
    }

    /// A live stamped target still lends its identity to build output — the
    /// borrowing the per-user branch documents is intentional, not to be
    /// swept away with the dead-target fix.
    #[test]
    fn build_output_with_live_stamped_target_stays_branded() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let live_target = dir.path().join("installed-copy.exe");
        std::fs::write(&live_target, b"").expect("write target");
        let lnk = lnk_fixture(dir.path(), Some(&live_target), Some(super::AUMID));
        let exe = std::env::current_exe().expect("current exe");
        assert_eq!(
            super::decide_with(&exe, None, Some(lnk)),
            Ok(super::Decision::Branded)
        );
    }

    /// A shortcut that never had a target is an ambiguous state, not a dead
    /// one: `target_dead` only trips on a stored path that is confirmed
    /// missing, so this still reads as Branded rather than ToastOnly.
    #[test]
    fn build_output_with_targetless_stamped_shortcut_stays_branded() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let lnk = lnk_fixture(dir.path(), None, Some(super::AUMID));
        let exe = std::env::current_exe().expect("current exe");
        assert_eq!(
            super::decide_with(&exe, None, Some(lnk)),
            Ok(super::Decision::Branded)
        );
    }

    /// An unstamped shortcut is not ours at all: build output keeps the old
    /// Skip answer rather than being pulled into ToastOnly territory.
    #[test]
    fn build_output_with_unstamped_shortcut_stays_skip() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let live_target = dir.path().join("other-app.exe");
        std::fs::write(&live_target, b"").expect("write target");
        let lnk = lnk_fixture(dir.path(), Some(&live_target), None);
        let exe = std::env::current_exe().expect("current exe");
        assert!(matches!(
            super::decide_with(&exe, None, Some(lnk)),
            Ok(super::Decision::Skip(_))
        ));
    }

    /// Dead all-users shortcut, same taskbar failure mode: build output gets
    /// ToastOnly there too, not a Branded that would hang it on the dead icon.
    #[test]
    fn dead_all_users_shortcut_is_toast_only_for_build_output() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let dead_target = dir.path().join("gone.exe");
        let all_users = lnk_fixture(dir.path(), Some(&dead_target), Some(super::AUMID));
        let per_user = dir.path().join("per-user").join("lyrel.lnk");
        let exe = std::env::current_exe().expect("current exe");
        assert_eq!(
            super::decide_with(&exe, Some(all_users), Some(per_user)),
            Ok(super::Decision::ToastOnly)
        );
    }

    /// `SLGP_RAWPATH` may hand back a target holding unexpanded environment
    /// variables (`%LOCALAPPDATA%\…`). Such a path reads as missing whether
    /// the real file is alive or not, so it must not qualify as a confirmed
    /// dead target — a live install's identity is not ours to drop over an
    /// ambiguous read.
    #[test]
    fn build_output_with_env_var_target_stays_branded() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let env_target = Path::new(r"%LOCALAPPDATA%\Programs\lyrel\lyrel-app.exe");
        let lnk = lnk_fixture(dir.path(), Some(env_target), Some(super::AUMID));
        let exe = std::env::current_exe().expect("current exe");
        assert_eq!(
            super::decide_with(&exe, None, Some(lnk)),
            Ok(super::Decision::Branded)
        );
    }

    /// The guard itself, independent of how the shell stored the fixture: a
    /// stored target containing `%` never counts as confirmed dead.
    #[test]
    fn an_env_var_target_is_not_a_dead_target() {
        let shortcut = super::Shortcut {
            aumid: Some(super::AUMID.to_string()),
            target: Some(std::path::PathBuf::from(
                r"%LOCALAPPDATA%\Programs\lyrel\lyrel-app.exe",
            )),
        };
        assert!(!super::target_dead(&shortcut));
    }

    /// Install layout is untouched by the new decision: a stamped shortcut
    /// pointing at a deleted target still heals by writing — the running exe
    /// may rebrand the entry at itself, which build output may not.
    #[test]
    fn installed_layout_with_dead_stamped_target_still_writes() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let dead_target = dir.path().join("previous-version.exe");
        let lnk = lnk_fixture(dir.path(), Some(&dead_target), Some(super::AUMID));
        let exe = Path::new(r"C:\Program Files\lyrel\lyrel-app.exe");
        assert_eq!(
            super::decide_with(exe, None, Some(lnk.clone())),
            Ok(super::Decision::Write(lnk))
        );
    }

    /// Manual end-to-end check, never run by CI (`--ignored` to opt in). Prints
    /// what `decide()` chose and pops a real toast, which should carry the lyrel
    /// icon and name rather than PowerShell's. On a machine that had no
    /// shortcut yet the first run writes one and stays unbranded for
    /// `INDEX_GRACE`; run it a second time to see the branded toast.
    #[test]
    #[ignore = "may touch the real Start Menu and pops a real toast"]
    fn sends_a_branded_toast() {
        let decision = match super::decide().expect("decide") {
            super::Decision::Branded | super::Decision::ToastOnly => "already branded".to_string(),
            super::Decision::Write(lnk) => format!("writing {}", lnk.display()),
            super::Decision::Skip(why) => format!("skipping: {why}"),
        };
        println!("decision: {decision}");
        println!("toast_app_id: {:?}", super::toast_app_id());
        crate::terminal::desktop_notification::notify_desktop(Some("lyrel"), "AUMID toast test");
        std::thread::sleep(std::time::Duration::from_secs(3));
    }
}
