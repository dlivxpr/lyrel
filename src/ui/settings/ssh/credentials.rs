//! GUI-only 凭据策略；配置保存成功后窗口才调用这里。
use super::*;

/// The password the keychain holds for this profile's endpoint, or nothing.
///
/// Read once, when a host is opened for editing — not per render, and not per
/// keystroke. A profile with no host yet has no endpoint to ask about: the
/// account would come out as `@:22`, which belongs to no server.
pub(super) fn stored_password(profile: &SshProfile, store: &dyn CredentialStore) -> String {
    if profile.host.trim().is_empty() {
        return String::new();
    }
    store
        .password_for(&profile.user, &profile.host, profile.port)
        .ok()
        .flatten()
        .unwrap_or_default()
}

/// The first key file a profile would offer that is actually there.
///
/// A passphrase is accounted by the key's *contents*, not by its path, so a
/// file that cannot be read is a key nothing can be stored against.
pub(super) fn first_readable_key(profile: &SshProfile) -> Option<String> {
    first_readable_key_in(&profile.identity_files, &profile.host, &profile.user)
}

/// The same answer for a form that has not been collected into a profile yet:
/// the key field as typed, with the host and user beside it filling in `%h`
/// and `%r`.
pub(super) fn first_readable_key_in(files: &[String], host: &str, user: &str) -> Option<String> {
    first_readable_key_or(files, host, user, identity_defaults)
}

/// An empty key field is not "no key": `build_spec_inner` offers the `~/.ssh`
/// defaults then, and looks their passphrases up by those exact strings. The
/// box has to follow the same list, or the most common setup — no key named,
/// an encrypted `id_ed25519` — could never be given a passphrase here.
pub(super) fn first_readable_key_or(
    files: &[String],
    host: &str,
    user: &str,
    defaults: impl FnOnce() -> Vec<String>,
) -> Option<String> {
    let candidates = if files.is_empty() {
        defaults()
    } else {
        files
            .iter()
            .map(|f| crate::core::ssh_profile::expand_identity_placeholders(f, host, user))
            .collect()
    };
    candidates
        .into_iter()
        .find(|p| std::fs::metadata(p).is_ok())
}

pub(super) fn stored_passphrase(key_path: &str, store: &dyn CredentialStore) -> String {
    let path = crate::core::ssh_profile::expand_tilde(key_path);
    let Ok(bytes) = std::fs::read(&path) else {
        return String::new();
    };
    store
        .passphrase_for_key(&key_account_from_contents(&bytes))
        .ok()
        .flatten()
        .unwrap_or_default()
}
// 自动化只允许显式临时 identity；绝不读取个人 ~/.ssh 或系统凭据。
fn identity_defaults() -> Vec<String> {
    #[cfg(not(test))]
    {
        crate::core::ssh_profile::default_identity_candidates()
    }
    #[cfg(test)]
    {
        Vec::new()
    }
}

impl SshSettings {
    /// Move the two secrets in the form into the keychain — or out of it.
    ///
    /// The config file never holds either of them, so this is the whole of
    /// what saving means for a password: an entry keyed by the endpoint the
    /// profile now names. Editing the address moves the entry rather than
    /// leaving the old one behind to be offered to a host that no longer
    /// exists, and clearing the field removes it.
    pub(super) fn save_secrets(
        &mut self,
        profile: &SshProfile,
        cfg: &Config,
        cx: &App,
    ) -> Vec<String> {
        let Some(form) = self.form.as_ref() else {
            return Vec::new();
        };
        let typed = form.password.read(cx).value().to_string();
        let typed_passphrase = form.passphrase.read(cx).value().to_string();
        let (old_user, old_host, old_port) = form.loaded_endpoint.clone();
        let was = form.loaded_password.clone();
        let was_passphrase = form.loaded_passphrase.clone();
        let moved = (old_user.as_str(), old_host.as_str(), old_port)
            != (profile.user.as_str(), profile.host.as_str(), profile.port);

        let mut failures: Vec<String> = Vec::new();
        let endpoint = |u: &str, h: &str, p: u16| format!("{u}@{h}:{p}");
        let plan = password_plan(&was, &typed, moved);

        // The entry the form read from, once it is no longer the entry the
        // form would write to. Left alone when another profile still dials the
        // same address — the keychain accounts by endpoint, not by profile.
        let stranded = plan.drop_old && !old_host.trim().is_empty();
        if stranded
            && !cfg.ssh_profiles.iter().any(|p| {
                p.id != profile.id && p.user == old_user && p.host == old_host && p.port == old_port
            })
            && let Err(e) = self
                .credentials
                .delete_password(&old_user, &old_host, old_port)
        {
            failures.push(t_fmt(
                L10nKey::SettingsCouldntForgetPassword,
                &[
                    ("endpoint", &endpoint(&old_user, &old_host, old_port)),
                    ("error", &e.to_string()),
                ],
            ));
        }
        if plan.store
            && !profile.host.trim().is_empty()
            && let Err(e) =
                self.credentials
                    .set_password(&profile.user, &profile.host, profile.port, &typed)
        {
            failures.push(t_fmt(
                L10nKey::SettingsCouldntSavePassword,
                &[
                    (
                        "endpoint",
                        &endpoint(&profile.user, &profile.host, profile.port),
                    ),
                    ("error", &e.to_string()),
                ],
            ));
        }

        // A passphrase belongs to the key it unlocks, not to this profile, so
        // a save only ever touches the entry for the key named here. Pointing
        // the profile at a different key leaves the first key's passphrase
        // alone — other hosts use that key too.
        let key = first_readable_key(profile);
        if typed_passphrase != was_passphrase {
            match key.as_deref() {
                Some(path) => self.write_key_passphrase(path, &typed_passphrase, &mut failures),
                // Nowhere to put it: the field names no key, or names one that
                // is not on this machine. Storing nothing quietly would lose a
                // passphrase the user watched themselves type.
                None if !typed_passphrase.is_empty() => {
                    failures.push(t(L10nKey::SettingsPassphraseNeedsKey).to_string())
                }
                None => {}
            }
        }

        // What the form would now read back, so a save leaves it clean.
        if let Some(form) = self.form.as_mut() {
            form.loaded_password = typed;
            form.loaded_passphrase = typed_passphrase;
            form.loaded_endpoint = (profile.user.clone(), profile.host.clone(), profile.port);
            form.loaded_key = key;
        }
        failures
    }

    /// A blank secret deletes rather than stores: an empty string is not a
    /// passphrase, and leaving one behind would keep offering it.
    fn write_key_passphrase(&self, key_path: &str, secret: &str, failures: &mut Vec<String>) {
        let path = crate::core::ssh_profile::expand_tilde(key_path);
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(e) => {
                failures.push(t_fmt(
                    L10nKey::SettingsCouldntSavePassphrase,
                    &[("key", key_path), ("error", &e.to_string())],
                ));
                return;
            }
        };
        let account = key_account_from_contents(&bytes);
        let result = if secret.is_empty() {
            self.credentials.delete_key_passphrase(&account)
        } else {
            self.credentials
                .set_key_passphrase(&account, secret)
                .map(|_| ())
        };
        if let Err(e) = result {
            failures.push(t_fmt(
                L10nKey::SettingsCouldntSavePassphrase,
                &[("key", key_path), ("error", &e.to_string())],
            ));
        }
    }

    /// Point the passphrase box at the key the form now names.
    ///
    /// A passphrase is stored against the contents of the key it unlocks, so
    /// the box is only ever right about one key at a time. A box the user has
    /// started typing in is left alone — it is the one place where what is on
    /// screen outranks what the keychain holds.
    pub(super) fn resync_key_passphrase(
        &mut self,
        window: &mut Window,
        cx: &mut Context<SettingsWindow>,
    ) {
        let Some(form) = self.form.as_ref() else {
            return;
        };
        let files = split_lines(&form.identity_files.read(cx).value().replace(',', "\n"));
        let host = form.host.read(cx).value().trim().to_string();
        let user = form.user.read(cx).value().trim().to_string();
        let key = first_readable_key_in(&files, &host, &user);
        if key == form.loaded_key {
            return;
        }
        let stored = key
            .as_deref()
            .map(|key| stored_passphrase(key, self.credentials.as_ref()))
            .unwrap_or_default();
        // A box the user has already typed in keeps what they typed — only
        // the key it will be saved against moves under it.
        let untouched = form.passphrase.read(cx).value().as_ref() == form.loaded_passphrase;
        let input = form.passphrase.clone();
        if let Some(form) = self.form.as_mut() {
            form.loaded_key = key;
            form.loaded_passphrase = stored.clone();
        }
        if untouched {
            input.update(cx, |i, cx| i.set_value(stored, window, cx));
        }
        cx.notify();
    }

    pub(super) fn forget_removed_profile(&self, cfg: &Config, id: Uuid) {
        // "Forget password" lives on the menu that is about to stop existing,
        // so deleting the profile used to strand its keychain entry with no UI
        // left to remove it. Only let go of the secret when nothing else on the
        // list still points at the same endpoint.
        let endpoint = cfg
            .ssh_profiles
            .iter()
            .find(|p| p.id == id)
            .map(|p| (p.user.clone(), p.host.clone(), p.port));
        let shared = profiles_sharing_endpoint(cfg, id) > 0;
        if let Some((user, host, port)) = endpoint.filter(|_| !shared) {
            let _ = self.credentials.delete_password(&user, &host, port);
        }
        // The same argument for the key passphrases this profile taught the
        // app about: the comment above says "the secret", but until now only
        // the password was let go of, so a deleted profile stranded its
        // passphrase entries with no UI left to reach them. A key is only
        // forgotten when no surviving profile still lists it.
        let mine = cfg
            .ssh_profiles
            .iter()
            .find(|p| p.id == id)
            .map(|p| p.expanded_identity_files())
            .unwrap_or_default();
        let kept: std::collections::HashSet<String> = cfg
            .ssh_profiles
            .iter()
            .filter(|p| p.id != id)
            .flat_map(|p| p.expanded_identity_files())
            .collect();
        for path in mine.iter().filter(|p| !kept.contains(*p)) {
            // Keyed by the key file's contents, so a key already gone from
            // disk cannot be looked up — and has no live passphrase to leak.
            let Ok(bytes) = std::fs::read(path) else {
                continue;
            };
            let account = crate::core::keychain::key_account_from_contents(&bytes);
            let _ = self.credentials.delete_key_passphrase(&account);
        }
    }
}
