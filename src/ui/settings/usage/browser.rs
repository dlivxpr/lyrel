//! Scoped provider LocalStorage import; no cookies or authorization prompts.
//! Profile discovery reads only browser metadata; session reads require an explicit profile.

mod chromium;
mod firefox;
#[cfg(test)]
mod tests;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use zeroize::Zeroizing;
#[derive(Clone, Copy)]
pub(super) enum StorageSource {
    DeepSeek,
    Kimi,
}
impl StorageSource {
    fn origin(self) -> &'static str {
        match self {
            Self::DeepSeek => "https://platform.deepseek.com",
            Self::Kimi => "https://www.kimi.com",
        }
    }
    fn key(self) -> &'static str {
        match self {
            Self::DeepSeek => "userToken",
            Self::Kimi => "access_token",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Browser {
    Chrome,
    Edge,
    Brave,
    Chromium,
    Firefox,
}

impl Browser {
    pub(super) fn all() -> &'static [Self] {
        &[
            Self::Chrome,
            Self::Edge,
            Self::Brave,
            Self::Chromium,
            Self::Firefox,
        ]
    }

    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Chrome => "Chrome",
            Self::Edge => "Edge",
            Self::Brave => "Brave",
            Self::Chromium => "Chromium",
            Self::Firefox => "Firefox",
        }
    }
}

/// Contains no paths or session values. A token change requires explicit re-import,
/// even when it may merely be a session rotation rather than an account change.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct BrowserSelection {
    browser: Browser,
    profile_id: String,
    profile_label: String,
    profile_fingerprint: String,
    token_fingerprint: String,
}

impl BrowserSelection {
    pub(super) fn browser(&self) -> Browser {
        self.browser
    }
    pub(super) fn label(&self) -> &str {
        &self.profile_label
    }
}

#[derive(Clone)]
pub(super) struct Profile {
    browser: Browser,
    id: String,
    label: String,
    path: PathBuf,
    fingerprint: String,
}

impl Profile {
    pub(super) fn id(&self) -> &str {
        &self.id
    }
    pub(super) fn label(&self) -> &str {
        &self.label
    }
}

// Deliberately no Debug, Serialize, Deserialize, or Clone implementation.
pub(super) struct SessionMaterial(Zeroizing<String>, Option<String>);

impl SessionMaterial {
    pub(super) fn secret(&self) -> &str {
        self.0.as_str()
    }
    pub(super) fn identity(&self) -> Option<&str> {
        self.1.as_deref()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum BrowserError {
    Locked,
    Denied,
    NoSession,
    Unsupported,
    ProfileGone,
    SourceChanged,
    Unreadable,
}

impl std::fmt::Display for BrowserError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Locked => "Browser storage is locked; close the selected browser and try again",
            Self::Denied => "Permission to read the selected browser profile was denied",
            Self::NoSession => "The selected profile has no session for this provider",
            Self::Unsupported => "This browser storage format is not supported",
            Self::ProfileGone => "The selected browser profile is no longer available",
            Self::SourceChanged => {
                "The selected browser session changed; explicitly import it again"
            }
            Self::Unreadable => "The selected browser storage could not be read",
        })
    }
}

impl std::error::Error for BrowserError {}

pub(super) struct BrowserReader {
    roots: Vec<(Browser, PathBuf)>,
    isolated: bool,
}

impl BrowserReader {
    /// Only records well-known roots. It does not access any browser files.
    pub(super) fn system() -> Self {
        let mut roots = Vec::new();
        #[cfg(target_os = "windows")]
        {
            if let Some(base) = std::env::var_os("LOCALAPPDATA").map(PathBuf::from) {
                for (browser, relative) in [
                    (Browser::Chrome, "Google/Chrome/User Data"),
                    (Browser::Edge, "Microsoft/Edge/User Data"),
                    (Browser::Brave, "BraveSoftware/Brave-Browser/User Data"),
                    (Browser::Chromium, "Chromium/User Data"),
                ] {
                    roots.push((browser, base.join(relative)));
                }
            }
            if let Some(base) = std::env::var_os("APPDATA").map(PathBuf::from) {
                roots.push((Browser::Firefox, base.join("Mozilla/Firefox")));
            }
        }
        #[cfg(target_os = "macos")]
        if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
            let base = home.join("Library/Application Support");
            for (browser, relative) in [
                (Browser::Chrome, "Google/Chrome"),
                (Browser::Edge, "Microsoft Edge"),
                (Browser::Brave, "BraveSoftware/Brave-Browser"),
                (Browser::Chromium, "Chromium"),
                (Browser::Firefox, "Firefox"),
            ] {
                roots.push((browser, base.join(relative)));
            }
        }
        #[cfg(target_os = "linux")]
        if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
            let config = std::env::var_os("XDG_CONFIG_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join(".config"));
            for (browser, relative) in [
                (Browser::Chrome, "google-chrome"),
                (Browser::Edge, "microsoft-edge"),
                (Browser::Brave, "BraveSoftware/Brave-Browser"),
                (Browser::Chromium, "chromium"),
            ] {
                roots.push((browser, config.join(relative)));
            }
            roots.push((Browser::Firefox, home.join(".mozilla/firefox")));
            roots.push((
                Browser::Firefox,
                home.join("snap/firefox/common/.mozilla/firefox"),
            ));
            roots.push((
                Browser::Firefox,
                home.join(".var/app/org.mozilla.firefox/.mozilla/firefox"),
            ));
            roots.push((
                Browser::Chromium,
                home.join("snap/chromium/common/chromium"),
            ));
        }
        Self {
            roots,
            isolated: false,
        }
    }

    /// A real isolated user-data root, not a fake storage implementation. Does not
    /// consult system roots, and rejects Firefox paths outside the supplied root.
    pub(super) fn with_root(browser: Browser, root: impl Into<PathBuf>) -> Self {
        Self {
            roots: vec![(browser, root.into())],
            isolated: true,
        }
    }

    pub(super) fn profiles(&self, browser: Browser) -> Result<Vec<Profile>, BrowserError> {
        let mut profiles = Vec::new();
        for (_, root) in self
            .roots
            .iter()
            .filter(|(candidate, _)| *candidate == browser)
        {
            match fs::metadata(root) {
                Ok(metadata) if metadata.is_dir() => {}
                Ok(_) => continue,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(io_error(error)),
            }
            if browser == Browser::Firefox {
                firefox::profiles(root, self.isolated, &mut profiles)?;
            } else {
                chromium::profiles(browser, root, &mut profiles)?;
            }
        }
        profiles.sort_by(|a, b| a.id.cmp(&b.id).then(a.fingerprint.cmp(&b.fingerprint)));
        profiles.dedup_by(|a, b| a.fingerprint == b.fingerprint);
        Ok(profiles)
    }

    pub(super) fn import(
        &self,
        profile: &Profile,
    ) -> Result<(BrowserSelection, SessionMaterial), BrowserError> {
        self.import_for(profile, StorageSource::DeepSeek)
    }

    pub(super) fn import_for(
        &self,
        profile: &Profile,
        source: StorageSource,
    ) -> Result<(BrowserSelection, SessionMaterial), BrowserError> {
        // Resolve against fresh metadata: a stale UI row or a foreign reader must
        // not authorize reading an arbitrary path.
        let current = self
            .profiles(profile.browser)?
            .into_iter()
            .find(|candidate| candidate.fingerprint == profile.fingerprint)
            .ok_or(BrowserError::ProfileGone)?;
        let material = read_session(&current, source)?;
        let selection = BrowserSelection {
            browser: current.browser,
            profile_id: current.id,
            profile_label: current.label,
            profile_fingerprint: current.fingerprint,
            token_fingerprint: material_fingerprint(source, &material),
        };
        Ok((selection, material))
    }

    pub(super) fn resolve(
        &self,
        selection: &BrowserSelection,
    ) -> Result<SessionMaterial, BrowserError> {
        self.resolve_for(selection, StorageSource::DeepSeek)
    }

    pub(super) fn resolve_for(
        &self,
        selection: &BrowserSelection,
        source: StorageSource,
    ) -> Result<SessionMaterial, BrowserError> {
        let profile = self
            .profiles(selection.browser)?
            .into_iter()
            .find(|profile| {
                profile.id == selection.profile_id
                    && profile.fingerprint == selection.profile_fingerprint
            })
            .ok_or(BrowserError::ProfileGone)?;
        let material = read_session(&profile, source)?;
        if material_fingerprint(source, &material) != selection.token_fingerprint {
            return Err(BrowserError::SourceChanged);
        }
        Ok(material)
    }
}

fn make_profile(browser: Browser, path: PathBuf, label: String) -> Result<Profile, BrowserError> {
    let canonical = fs::canonicalize(&path).map_err(io_error)?;
    let id = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or(BrowserError::Unsupported)?
        .to_owned();
    let fingerprint = fingerprint(&[browser.label(), &canonical.to_string_lossy()]);
    Ok(Profile {
        browser,
        id,
        label,
        path: canonical,
        fingerprint,
    })
}

fn fingerprint(parts: &[&str]) -> String {
    let mut hash = Sha256::new();
    for part in parts {
        hash.update(part.as_bytes());
        hash.update([0]);
    }
    use std::fmt::Write as _;
    let mut encoded = String::with_capacity(64);
    for byte in hash.finalize() {
        write!(&mut encoded, "{byte:02x}").expect("writing to String");
    }
    encoded
}

fn material_fingerprint(source: StorageSource, material: &SessionMaterial) -> String {
    match source {
        StorageSource::DeepSeek => fingerprint(&["deepseek-token", material.secret()]),
        StorageSource::Kimi => fingerprint(&[
            source.origin(),
            material.secret(),
            material.identity().unwrap_or(""),
        ]),
    }
}
fn read_session(profile: &Profile, source: StorageSource) -> Result<SessionMaterial, BrowserError> {
    let token = if profile.browser == Browser::Firefox {
        firefox::read(&profile.path, source.origin(), source.key())?
    } else {
        chromium::read(&profile.path, source.origin(), source.key())?
    };
    // LocalStorage contains the raw userToken, not a Cookie header. Do not guess
    // another key or source, parse arbitrary JSON, or accept injected headers.
    if token.is_empty() {
        return Err(BrowserError::NoSession);
    }
    if token.len() > 64 * 1024 || token.chars().any(char::is_control) {
        return Err(BrowserError::Unreadable);
    }
    let identity = if matches!(source, StorageSource::Kimi) {
        let value = if profile.browser == Browser::Firefox {
            firefox::read(&profile.path, source.origin(), "msh_user_id")
        } else {
            chromium::read(&profile.path, source.origin(), "msh_user_id")
        };
        match value {
            Ok(id) if !id.is_empty() && id.len() <= 256 && !id.chars().any(char::is_control) => {
                Some(id)
            }
            Ok(_) => return Err(BrowserError::Unreadable),
            Err(BrowserError::NoSession) => None,
            Err(error) => return Err(error),
        }
    } else {
        None
    };
    Ok(SessionMaterial(Zeroizing::new(token), identity))
}

fn io_error(error: io::Error) -> BrowserError {
    match error.kind() {
        io::ErrorKind::PermissionDenied => BrowserError::Denied,
        io::ErrorKind::WouldBlock => BrowserError::Locked,
        io::ErrorKind::NotFound => BrowserError::NoSession,
        _ if matches!(error.raw_os_error(), Some(32 | 33)) && cfg!(windows) => BrowserError::Locked,
        _ => BrowserError::Unreadable,
    }
}

fn utf16(bytes: &[u8]) -> Result<String, BrowserError> {
    if bytes.len() % 2 != 0 {
        return Err(BrowserError::Unreadable);
    }
    char::decode_utf16(
        bytes
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]])),
    )
    .map(|value| value.map_err(|_| BrowserError::Unreadable))
    .collect()
}
