//! Optional Windows Explorer context-menu integration.
//!
//! The registry is user-visible system state, so lyrel never writes these verbs
//! on its own: the Windows installer offers a task checkbox and invokes
//! [`register`], and its uninstaller always invokes [`unregister`]. There is no
//! runtime setting — the same install-time-only treatment VS Code and Git for
//! Windows give their shell entries. Keeping the key layout here rather than in
//! the .iss keeps one description of it in the tree.
//!
//! Both verbs invoke the GUI-subsystem `lyrel-app.exe` directly so Explorer never
//! allocates a transient console. The app first offers the path to an already
//! running GUI through `GuiOpen`, then continues normal startup when no GUI
//! receives it.

#[cfg(any(windows, test))]
use std::ffi::OsString;
#[cfg(any(windows, test))]
use std::path::Path;
#[cfg(windows)]
use std::path::PathBuf;

#[cfg(windows)]
use anyhow::Context as _;
use anyhow::Result;

#[cfg(windows)]
use crate::ui::i18n::{L10nKey, t};

#[cfg(any(windows, test))]
const DIRECTORY_KEY: &str = r"Software\Classes\Directory\shell\lyrel";
#[cfg(any(windows, test))]
const BACKGROUND_KEY: &str = r"Software\Classes\Directory\Background\shell\lyrel";

#[cfg(any(windows, test))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Location {
    Directory,
    Background,
}

#[cfg(any(windows, test))]
impl Location {
    fn key(self) -> &'static str {
        match self {
            Self::Directory => DIRECTORY_KEY,
            Self::Background => BACKGROUND_KEY,
        }
    }

    /// The wording Explorer shows, in the language the app is set to.
    ///
    /// Explorer reads this string from the registry, not from lyrel, so what is
    /// written here is what a user sees until something writes it again — an
    /// install-time snapshot of the locale. Two things keep that snapshot
    /// honest: `register` sets the locale from the config before building the
    /// entries, and [`refresh_labels`] restates them whenever the language
    /// changes — from the picker in Settings or from a hand-edited config.
    #[cfg(windows)]
    fn label(self) -> &'static str {
        t(match self {
            Self::Directory => L10nKey::ExplorerMenuOpenIn,
            Self::Background => L10nKey::ExplorerMenuOpenHere,
        })
    }

    fn placeholder(self) -> &'static str {
        match self {
            // `%1` is the selected directory passed to a normal static verb.
            Self::Directory => "%1",
            // Explorer expands `%V` to the folder whose background was clicked.
            Self::Background => "%V",
        }
    }
}

#[cfg(any(windows, test))]
#[derive(Debug)]
struct Registration {
    location: Location,
    icon: OsString,
    command: OsString,
}

#[cfg(any(windows, test))]
fn replace_entry_with(
    registration: &Registration,
    delete: impl FnOnce(&str) -> Result<()>,
    write: impl FnOnce(&Registration) -> Result<()>,
) -> Result<()> {
    // The verb root belongs exclusively to lyrel. Replacing it at the operation
    // boundary prevents stale shell values or handler subkeys from surviving
    // an update and changing visibility or execution semantics.
    delete(registration.location.key())?;
    write(registration)
}

#[cfg(any(windows, test))]
impl Registration {
    fn new(location: Location, app: &Path) -> Self {
        Self {
            location,
            icon: app.as_os_str().to_os_string(),
            command: quoted_open_command(app, location.placeholder()),
        }
    }
}

/// Register both Explorer verbs for the current user.
pub fn register() -> Result<()> {
    platform_register()
}

/// Remove only the two Explorer verb trees owned by lyrel.
pub fn unregister() -> Result<()> {
    platform_unregister()
}

/// Restate the verb labels in the language the UI now runs in.
///
/// The registry holds whatever wording was current when the installer ran, so
/// without this a user who switches lyrel to Chinese keeps English entries in
/// Explorer for the life of the install — the one place in the product where
/// the language setting would not reach.
///
/// Only keys that already exist are rewritten. Offering the menu is the
/// installer's checkbox and declining it is the user's decision; changing a
/// language must never be what puts the verbs back.
///
/// Best-effort by design: a failure here costs a log line, never a language
/// change the user asked for.
pub fn refresh_labels() {
    if let Err(error) = platform_refresh_labels() {
        log::warn!("could not restate the Explorer context-menu labels: {error}");
    }
}

/// Build a command line without converting the executable path through UTF-8.
///
/// Quotes are unconditional: both the executable and the Explorer-substituted
/// folder can contain spaces. Windows file names cannot contain a quote, so the
/// resulting command is unambiguous without an additional escape layer.
#[cfg(any(windows, test))]
fn quoted_open_command(executable: &Path, placeholder: &str) -> OsString {
    let mut command = OsString::from("\"");
    command.push(executable.as_os_str());
    command.push("\" --open-path \"");
    command.push(placeholder);
    command.push("\"");
    command
}

#[cfg(windows)]
fn application_path() -> Result<PathBuf> {
    let app = std::env::current_exe().context("locating the running lyrel application")?;
    app.parent()
        .context("the running lyrel application has no parent directory")?;
    Ok(app)
}

#[cfg(any(windows, test))]
fn registrations(app: &Path) -> [Registration; 2] {
    [
        Registration::new(Location::Directory, app),
        Registration::new(Location::Background, app),
    ]
}

#[cfg(windows)]
mod windows {
    use super::*;
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt as _;

    use windows_sys::Win32::Foundation::{
        ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND, ERROR_SUCCESS,
    };
    use windows_sys::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, KEY_READ, KEY_WRITE, REG_OPTION_NON_VOLATILE, REG_SZ, RegCloseKey,
        RegCreateKeyExW, RegDeleteTreeW, RegOpenKeyExW, RegSetValueExW,
    };
    use windows_sys::Win32::UI::Shell::{SHCNE_ASSOCCHANGED, SHCNF_IDLIST, SHChangeNotify};

    struct RegistryKey(HKEY);

    impl Drop for RegistryKey {
        fn drop(&mut self) {
            // SAFETY: `RegistryKey` is created only from a successful Win32
            // open/create call and owns exactly one handle.
            unsafe {
                RegCloseKey(self.0);
            }
        }
    }

    fn wide(value: &OsStr) -> Vec<u16> {
        value.encode_wide().chain(std::iter::once(0)).collect()
    }

    fn io_error(action: &str, code: u32) -> anyhow::Error {
        anyhow::anyhow!(
            "{action}: {}",
            std::io::Error::from_raw_os_error(code as i32)
        )
    }

    fn create_key(path: &str) -> Result<RegistryKey> {
        let path = wide(OsStr::new(path));
        let mut key: HKEY = std::ptr::null_mut();
        // SAFETY: all input pointers reference live locals; the optional class,
        // security and disposition pointers are null as permitted by the API.
        let code = unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                path.as_ptr(),
                0,
                std::ptr::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_READ | KEY_WRITE,
                std::ptr::null(),
                &mut key,
                std::ptr::null_mut(),
            )
        };
        if code != ERROR_SUCCESS {
            return Err(io_error("creating the lyrel Explorer registry key", code));
        }
        Ok(RegistryKey(key))
    }

    fn set_string(key: &RegistryKey, name: Option<&OsStr>, value: &OsStr) -> Result<()> {
        let name = name.map(wide);
        let name_ptr = name.as_ref().map_or(std::ptr::null(), |name| name.as_ptr());
        let value = wide(value);
        let byte_len = u32::try_from(value.len() * 2)
            .context("the lyrel Explorer registry value is too long")?;
        // SAFETY: the key is writable, and both optional name and value point
        // to live NUL-terminated UTF-16 buffers for the duration of the call.
        let code =
            unsafe { RegSetValueExW(key.0, name_ptr, 0, REG_SZ, value.as_ptr().cast(), byte_len) };
        if code == ERROR_SUCCESS {
            Ok(())
        } else {
            Err(io_error("writing a lyrel Explorer registry value", code))
        }
    }

    fn write_entry_contents(registration: &Registration) -> Result<()> {
        let root = create_key(registration.location.key())?;
        set_string(&root, None, OsStr::new(registration.location.label()))?;
        set_string(&root, Some(OsStr::new("Icon")), &registration.icon)?;

        let command_path = format!(r"{}\command", registration.location.key());
        let command = create_key(&command_path)?;
        set_string(&command, None, &registration.command)
    }

    fn delete_tree(path: &str) -> Result<()> {
        let path = wide(OsStr::new(path));
        // SAFETY: `path` is a live, NUL-terminated UTF-16 string. Only lyrel's
        // own verb key is named, never a shared parent such as `shell`.
        let code = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, path.as_ptr()) };
        match code {
            ERROR_SUCCESS | ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND => Ok(()),
            other => Err(io_error("removing the lyrel Explorer registry key", other)),
        }
    }

    fn notify_explorer() {
        // SAFETY: `SHCNE_ASSOCCHANGED` with `SHCNF_IDLIST` carries no item
        // pointers. This is a cache invalidation hint after the registry write.
        unsafe {
            SHChangeNotify(
                SHCNE_ASSOCCHANGED as i32,
                SHCNF_IDLIST,
                std::ptr::null(),
                std::ptr::null(),
            );
        }
    }

    pub(super) fn register() -> Result<()> {
        let app = application_path()?;
        for registration in registrations(&app) {
            replace_entry_with(&registration, delete_tree, write_entry_contents)?;
        }
        notify_explorer();
        Ok(())
    }

    pub(super) fn unregister() -> Result<()> {
        // Try both removals even when one fails, so a damaged first key cannot
        // strand the independent second menu entry forever.
        let directory = delete_tree(DIRECTORY_KEY);
        let background = delete_tree(BACKGROUND_KEY);
        directory?;
        background?;
        notify_explorer();
        Ok(())
    }

    /// The verb key, or `None` when lyrel's menu is not installed.
    ///
    /// Deliberately open rather than create: this is the call that makes
    /// [`refresh_labels`] unable to resurrect a menu the user removed.
    fn open_existing(path: &str) -> Result<Option<RegistryKey>> {
        let path = wide(OsStr::new(path));
        let mut key: HKEY = std::ptr::null_mut();
        // SAFETY: `path` is a live, NUL-terminated UTF-16 string and `key` is a
        // live local the API fills in only on success.
        let code = unsafe {
            RegOpenKeyExW(
                HKEY_CURRENT_USER,
                path.as_ptr(),
                0,
                KEY_READ | KEY_WRITE,
                &mut key,
            )
        };
        match code {
            ERROR_SUCCESS => Ok(Some(RegistryKey(key))),
            ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND => Ok(None),
            other => Err(io_error("opening the lyrel Explorer registry key", other)),
        }
    }

    fn shell_config_dir() -> Result<PathBuf> {
        use ::windows::Win32::System::Com::CoTaskMemFree;
        use ::windows::core::PWSTR;
        use std::os::windows::ffi::OsStringExt as _;
        use windows_sys::Win32::UI::Shell::{
            FOLDERID_RoamingAppData, KF_FLAG_DONT_VERIFY, SHGetKnownFolderPath,
        };

        let mut roaming = PWSTR::null();
        // SAFETY: request this account's current known folder without creating
        // or checking it. Process-local APPDATA overrides are not authority for HKCU.
        let status = unsafe {
            SHGetKnownFolderPath(
                &FOLDERID_RoamingAppData,
                KF_FLAG_DONT_VERIFY as u32,
                std::ptr::null_mut(),
                &mut roaming.0,
            )
        };
        let path = if status >= 0 && !roaming.is_null() {
            // SAFETY: a successful lookup returns a live NUL-terminated path.
            Some(PathBuf::from(std::ffi::OsString::from_wide(unsafe {
                roaming.as_wide()
            })))
        } else {
            None
        };
        // SAFETY: the API requires release on success and failure; NULL is valid.
        unsafe { CoTaskMemFree(Some(roaming.0.cast())) };
        let mut path = path.ok_or_else(|| {
            anyhow::anyhow!("Windows roaming profile is unavailable (HRESULT {status:#x})")
        })?;
        path.push("lyrel");
        Ok(path)
    }

    pub(super) fn refresh_labels() -> Result<()> {
        let Some(instance) = crate::core::config::config_dir_path() else {
            return Ok(());
        };
        // Explorer's process-global verbs belong to the account's normal
        // namespace, not a CLI, environment or portable profile override.
        if instance != shell_config_dir()? {
            return Ok(());
        }
        let mut restated = false;
        for location in [Location::Directory, Location::Background] {
            let Some(key) = open_existing(location.key())? else {
                continue;
            };
            set_string(&key, None, OsStr::new(location.label()))?;
            restated = true;
        }
        // Only worth waking the shell when something actually moved; a user who
        // never installed the menu changes languages for free.
        if restated {
            notify_explorer();
        }
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use windows_sys::Win32::System::Registry::{
            REG_CREATED_NEW_KEY, RRF_RT_REG_SZ, RegGetValueW, RegOverridePredefKey,
        };

        struct RegistrySandbox {
            root: RegistryKey,
            name: Vec<u16>,
            mapped: bool,
        }

        impl RegistrySandbox {
            fn new() -> Self {
                let name = wide(OsStr::new(&format!(
                    "lyrel-explorer-test-{}",
                    uuid::Uuid::new_v4()
                )));
                let mut root = std::ptr::null_mut();
                let mut disposition = 0;
                // SAFETY: create only this subprocess's unique, owned root.
                let code = unsafe {
                    RegCreateKeyExW(
                        HKEY_CURRENT_USER,
                        name.as_ptr(),
                        0,
                        std::ptr::null(),
                        REG_OPTION_NON_VOLATILE,
                        KEY_READ | KEY_WRITE,
                        std::ptr::null(),
                        &mut root,
                        &mut disposition,
                    )
                };
                assert_eq!(code, ERROR_SUCCESS, "create owned registry root");
                assert_eq!(
                    disposition, REG_CREATED_NEW_KEY,
                    "refuse an existing registry root"
                );
                let mut sandbox = Self {
                    root: RegistryKey(root),
                    name,
                    mapped: false,
                };
                // SAFETY: the mapping is process-local, and this is an isolated
                // one-test child. No real Explorer verb can be opened after it.
                let code = unsafe { RegOverridePredefKey(HKEY_CURRENT_USER, sandbox.root.0) };
                assert_eq!(code, ERROR_SUCCESS, "map owned registry root");
                sandbox.mapped = true;
                sandbox
            }
        }

        impl Drop for RegistrySandbox {
            fn drop(&mut self) {
                // SAFETY: restore this child's mapping before deleting only
                // the unique root that it created, never the real verb keys.
                let restored = if self.mapped {
                    unsafe { RegOverridePredefKey(HKEY_CURRENT_USER, std::ptr::null_mut()) }
                } else {
                    ERROR_SUCCESS
                };
                let deleted = if restored == ERROR_SUCCESS {
                    unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, self.name.as_ptr()) }
                } else {
                    restored
                };
                if restored != ERROR_SUCCESS || deleted != ERROR_SUCCESS {
                    eprintln!(
                        "owned registry cleanup failed: restore={restored}, delete={deleted}"
                    );
                    std::process::exit(1);
                }
            }
        }

        fn stored_label(location: Location) -> String {
            let path = wide(OsStr::new(location.key()));
            let mut value = [0u16; 256];
            let mut bytes = (value.len() * std::mem::size_of::<u16>()) as u32;
            // SAFETY: the output buffer has the advertised capacity; HKCU is
            // mapped to the owned root for the lifetime of this fixture.
            let code = unsafe {
                RegGetValueW(
                    HKEY_CURRENT_USER,
                    path.as_ptr(),
                    std::ptr::null(),
                    RRF_RT_REG_SZ,
                    std::ptr::null_mut(),
                    value.as_mut_ptr().cast(),
                    &mut bytes,
                )
            };
            assert_eq!(code, ERROR_SUCCESS, "read owned Explorer label");
            String::from_utf16(&value[..bytes as usize / 2 - 1]).unwrap()
        }

        #[test]
        fn configuration_namespaces_preserve_the_installed_explorer_menu() {
            for scenario in ["cli", "environment", "profile", "default-explicit"] {
                let root = tempfile::TempDir::new().unwrap();
                let mut command = std::process::Command::new(std::env::current_exe().unwrap());
                command
                    .args([
                        "--exact",
                        "core::explorer_context_menu::windows::tests::registry_namespace_fixture",
                        "--ignored",
                        "--nocapture",
                    ])
                    .env("LYREL_EXPLORER_NAMESPACE_CASE", scenario)
                    .env("LYREL_EXPLORER_NAMESPACE_ROOT", root.path())
                    .env("APPDATA", root.path().join("roaming"))
                    .env("USERPROFILE", root.path())
                    .env_remove("LYREL_CONFIG_DIR")
                    .env_remove("LYREL_DATA_DIR");
                if scenario == "environment" {
                    command.env("LYREL_CONFIG_DIR", root.path().join("isolated"));
                }
                let output = command.output().unwrap();
                assert!(
                    output.status.success(),
                    "{scenario}: {}\n{}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                );
            }
        }

        #[test]
        #[ignore = "owned process-local registry child; run through the namespace regression"]
        fn registry_namespace_fixture() {
            let scenario =
                std::env::var("LYREL_EXPLORER_NAMESPACE_CASE").expect("owned child case");
            assert!(matches!(
                scenario.as_str(),
                "cli" | "environment" | "profile" | "default-explicit"
            ));
            let root = PathBuf::from(std::env::var_os("LYREL_EXPLORER_NAMESPACE_ROOT").unwrap());
            let default = shell_config_dir().unwrap();
            match scenario.as_str() {
                "cli" => crate::core::config::set_config_dir(root.join("isolated")),
                "default-explicit" => crate::core::config::set_config_dir(default),
                _ => {}
            }
            let _sandbox = RegistrySandbox::new();
            crate::ui::i18n::set_locale("en");
            let previous = "installed label owned by the default instance";
            for location in [Location::Directory, Location::Background] {
                let key = create_key(location.key()).unwrap();
                set_string(&key, None, OsStr::new(previous)).unwrap();
            }
            // This is the same public entry point used by startup, Settings,
            // and the config watcher, not a mock of registry write calls.
            super::super::refresh_labels();
            for location in [Location::Directory, Location::Background] {
                let expected = if scenario == "default-explicit" {
                    location.label()
                } else {
                    previous
                };
                assert_eq!(
                    stored_label(location),
                    expected,
                    "{scenario}: {:?}",
                    location
                );
            }
        }
    }
}

#[cfg(windows)]
fn platform_register() -> Result<()> {
    windows::register()
}

#[cfg(windows)]
fn platform_unregister() -> Result<()> {
    windows::unregister()
}

#[cfg(windows)]
fn platform_refresh_labels() -> Result<()> {
    windows::refresh_labels()
}

#[cfg(not(windows))]
fn platform_register() -> Result<()> {
    anyhow::bail!("Windows Explorer integration is only available on Windows")
}

#[cfg(not(windows))]
fn platform_unregister() -> Result<()> {
    anyhow::bail!("Windows Explorer integration is only available on Windows")
}

/// Nothing to restate: the verbs exist only on Windows.
///
/// Silent rather than an error like the two above, because this one is called
/// on every language change on every platform. Refusing here would put a
/// warning in the log of every macOS and Linux user who picks a language.
#[cfg(not(windows))]
fn platform_refresh_labels() -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registration_targets_both_directory_surfaces() {
        let app = Path::new(r"C:\Program Files\lyrel\lyrel-app.exe");
        let [directory, background] = registrations(app);

        assert_eq!(directory.location.key(), DIRECTORY_KEY);
        assert_eq!(directory.icon, app.as_os_str());
        assert_eq!(
            directory.command,
            r#""C:\Program Files\lyrel\lyrel-app.exe" --open-path "%1""#
        );

        assert_eq!(background.location.key(), BACKGROUND_KEY);
        assert_eq!(background.icon, app.as_os_str());
        assert_eq!(
            background.command,
            r#""C:\Program Files\lyrel\lyrel-app.exe" --open-path "%V""#
        );
    }

    #[test]
    fn commands_quote_even_paths_without_spaces() {
        assert_eq!(
            quoted_open_command(Path::new(r"C:\lyrel\lyrel-app.exe"), "%1"),
            r#""C:\lyrel\lyrel-app.exe" --open-path "%1""#
        );
    }

    #[test]
    fn registration_replaces_the_owned_tree_before_writing() {
        let registration = Registration::new(
            Location::Directory,
            Path::new(r"C:\Program Files\lyrel\lyrel-app.exe"),
        );
        let operations = std::cell::RefCell::new(Vec::new());

        replace_entry_with(
            &registration,
            |key| {
                operations.borrow_mut().push(format!("delete:{key}"));
                Ok(())
            },
            |_| {
                operations.borrow_mut().push("write".to_string());
                Ok(())
            },
        )
        .unwrap();

        assert_eq!(
            operations.into_inner(),
            vec![format!("delete:{DIRECTORY_KEY}"), "write".to_string()]
        );
    }
}
