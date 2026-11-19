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

use crate::desktop_notification::windows_toast::ACTIVATOR;

/// The toast/taskbar identity. Keep in sync with the `AppUserModelID` on the
/// installer shortcuts in `.github/scripts/windows-installer.iss` — a
/// mismatch silently splits the identity in two.
pub const AUMID: &str = "io.github.dlivxpr.lyrel";

/// How long we keep using the PowerShell identity after writing the shortcut
/// ourselves. The shell picks a new `.lnk` up asynchronously and silently
/// discards toasts for an AUMID it has not indexed yet; the measured lag on
/// Windows 11 was a few seconds. Overshooting only costs an unbranded toast
/// in the opening seconds of a first-ever run, so the bound is generous.
const INDEX_GRACE: Duration = Duration::from_secs(30);

/// One-time GUI-startup hook; see the module docs. Cheap after the first call.
pub fn init() {
    let _ = toast_app_id();
}

/// The AUMID to put on toasts, when (and only when) a shortcut the shell has
/// seen carries it. Memoized — the first call does the COM work.
pub fn toast_app_id() -> Option<&'static str> {
    static STATE: OnceLock<Branding> = OnceLock::new();
    match STATE.get_or_init(setup) {
        Branding::Unavailable => None,
        Branding::Ready => Some(AUMID),
        Branding::Pending(written_at) => (written_at.elapsed() >= INDEX_GRACE).then_some(AUMID),
    }
}

/// Build outputs may borrow branding, but only the installer-registered
/// executable may publish the toast activator class.
#[cfg(not(test))]
pub fn clickable_app_id() -> Option<&'static str> {
    let app_id = toast_app_id()?;
    let exe = std::env::current_exe().ok()?;
    has_clickable_shortcut(&exe, all_users_shortcut_path(), start_menu_shortcut_path())
        .then_some(app_id)
}

fn has_clickable_shortcut(exe: &Path, all: Option<PathBuf>, user: Option<PathBuf>) -> bool {
    [all, user]
        .into_iter()
        .flatten()
        .filter_map(|path| read_shortcut(&path).ok())
        .any(|shortcut| shortcut_points_to(&shortcut, exe) && shortcut.activator == Some(ACTIVATOR))
}

fn shortcut_points_to(shortcut: &Shortcut, exe: &Path) -> bool {
    shortcut.aumid.as_deref() == Some(AUMID)
        && shortcut
            .target
            .as_deref()
            .is_some_and(|target| same_path(target, exe))
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
        // Side-by-side installs can each have a legitimate shortcut. Prefer
        // this executable's per-user entry without rewriting the other one.
        if per_user_lnk
            .as_deref()
            .and_then(|path| read_shortcut(path).ok())
            .is_some_and(|shortcut| shortcut_points_to(&shortcut, exe))
        {
            return Ok(Decision::Branded);
        }
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

    // A portable copy can borrow branding, but must not replace a live
    // installer shortcut and strip its Notification Center activator.
    if existing.as_ref().is_some_and(|shortcut| {
        stamped && shortcut.activator == Some(ACTIVATOR) && !target_dead(shortcut)
    }) {
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
/// `target[\<triple>]\{debug,release}\[deps\]lyrel-app.exe`. A `CACHEDIR.TAG`
/// ancestor also marks the tree as derived, covering renamed
/// `CARGO_TARGET_DIR` directories when the marker is present. Standard cargo
/// layouts must not depend on that optional marker.
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
    let parent = exe.parent();
    let profile = if named(parent, "deps") {
        parent.and_then(Path::parent)
    } else {
        parent
    };
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
    activator: Option<windows::core::GUID>,
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

        Ok(Shortcut {
            aumid,
            target,
            activator: store
                .GetValue(&windows::Win32::Storage::EnhancedStorage::PKEY_AppUserModel_ToastActivatorCLSID)
                .ok()
                .and_then(|v| windows::Win32::System::Com::StructuredStorage::PropVariantToGUID(&v).ok()),
        })
    }
}

/// Create or overwrite `lnk`, pointing at the running exe and carrying our
/// AUMID. The exe has the icon compiled in (build.rs), which is what the toast
/// header shows.
fn write_shortcut(lnk: &Path) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| format!("current exe: {e}"))?;
    let activator = user_installer_registered(&exe).then_some(ACTIVATOR);
    write_shortcut_for(lnk, Some(&exe), Some(AUMID), activator)
}

/// Only a per-user installer registration can supply an activator for
/// a repaired shortcut. Other copies keep delivery without it.
fn user_installer_registered(exe: &Path) -> bool {
    use std::os::windows::ffi::OsStringExt;
    use std::ptr::{null, null_mut};
    use windows::core::HSTRING;
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_SZ, RegGetValueW};

    let key = HSTRING::from(format!(
        r"Software\Classes\CLSID\{{{ACTIVATOR:?}}}\LocalServer32"
    ));
    let mut bytes = 0;
    // SAFETY: A null buffer queries the size; the next call advertises its
    // actual capacity and fails if the registry value grows between reads.
    let read = |buffer, size: &mut u32| unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            null(),
            RRF_RT_REG_SZ,
            null_mut(),
            buffer,
            size,
        )
    };
    if read(null_mut(), &mut bytes) != ERROR_SUCCESS
        || !(6..=65_536).contains(&bytes)
        || bytes % 2 != 0
    {
        return false;
    }
    let mut value = vec![0u16; bytes as usize / 2];
    if read(value.as_mut_ptr().cast(), &mut bytes) != ERROR_SUCCESS {
        return false;
    }
    let Some(path) = value[..bytes as usize / 2]
        .strip_prefix(&[u16::from(b'"')])
        .and_then(|path| path.strip_suffix(&[u16::from(b'"'), 0]))
    else {
        return false;
    };
    same_path(Path::new(&std::ffi::OsString::from_wide(path)), exe)
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
    activator: Option<windows::core::GUID>,
) -> Result<(), String> {
    use windows::Win32::Storage::EnhancedStorage::{
        PKEY_AppUserModel_ID, PKEY_AppUserModel_ToastActivatorCLSID,
    };
    use windows::Win32::System::Com::{
        CLSCTX_INPROC_SERVER, CoCreateInstance, IPersistFile,
        StructuredStorage::{InitPropVariantFromCLSID, PROPVARIANT},
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
        if let Some(activator) = activator {
            let value =
                InitPropVariantFromCLSID(&activator).map_err(|e| format!("activator: {e}"))?;
            store
                .SetValue(&PKEY_AppUserModel_ToastActivatorCLSID, &value)
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
            r"C:\src\lyrel\target\debug\deps\lyrel_app-abc123.exe",
            r"C:\src\lyrel\target\release\deps\lyrel_app-abc123.exe",
            r"C:\src\lyrel\target\x86_64-pc-windows-msvc\release\deps\lyrel_app-abc123.exe",
            r"C:\src\lyrel\TARGET\Debug\DEPS\lyrel_app-abc123.exe",
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
            r"D:\lyrel\debug\deps\lyrel-app.exe",
            r"D:\lyrel\target\installed\deps\lyrel-app.exe",
        ] {
            let exe = Path::new(exe);
            assert!(!super::has_build_layout(exe), "{} misread", exe.display());
            assert!(!super::is_build_output(exe), "{} misread", exe.display());
        }
    }

    /// The test harness itself runs out of `target\...\deps\`. It must count
    /// as build output even when cargo has not left a `CACHEDIR.TAG` marker.
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
        super::write_shortcut_for(&lnk, target, aumid, None).expect("write shortcut fixture");
        lnk
    }

    #[test]
    fn a_matching_per_user_activator_survives_a_different_all_users_shortcut() {
        let dir = tempfile::TempDir::new().unwrap();
        let user_exe = dir.path().join("user.exe");
        let all_exe = dir.path().join("all.exe");
        std::fs::write(&user_exe, b"").unwrap();
        std::fs::write(&all_exe, b"").unwrap();
        let user = dir.path().join("user.lnk");
        let all = dir.path().join("all.lnk");
        super::write_shortcut_for(&all, Some(&all_exe), Some(super::AUMID), None).unwrap();
        super::write_shortcut_for(
            &user,
            Some(&user_exe),
            Some(super::AUMID),
            Some(super::ACTIVATOR),
        )
        .unwrap();
        assert_eq!(
            super::decide_with(&user_exe, Some(all.clone()), Some(user.clone())),
            Ok(super::Decision::Branded)
        );
        assert!(super::has_clickable_shortcut(
            &user_exe,
            Some(all.clone()),
            Some(user.clone())
        ));
        assert_eq!(
            super::decide_with(&all_exe, None, Some(user.clone())),
            Ok(super::Decision::Branded),
            "a portable copy must not rewrite the installer's shortcut"
        );
        assert!(!super::has_clickable_shortcut(
            &all_exe,
            Some(all),
            Some(user)
        ));
    }

    #[test]
    fn an_unregistered_copy_does_not_claim_the_activator_during_shortcut_repair() {
        let dir = tempfile::TempDir::new().unwrap();
        let shortcut = dir.path().join("lyrel.lnk");
        super::write_shortcut(&shortcut).unwrap();
        assert_ne!(
            super::read_shortcut(&shortcut).unwrap().activator,
            Some(super::ACTIVATOR)
        );
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
            activator: None,
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
        crate::desktop_notification::notify_desktop(Some("lyrel"), "AUMID toast test");
        std::thread::sleep(std::time::Duration::from_secs(3));
    }
}
