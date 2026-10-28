//! SSH 设置的草稿、校验与局部展示状态；窗口只适配 GPUI 与跨页协调。
use super::*;
#[cfg(not(test))]
use crate::core::keychain::OsCredentialStore;
use crate::core::keychain::{CredentialRef, CredentialStore, key_account_from_contents};
use crate::core::ssh_profile::{
    Algorithms, AuthMode, ForwardKind, ForwardRule, HostPort, SshProfile, to_connect_string,
};
use credentials::{first_readable_key, stored_passphrase, stored_password};
use gpui_kit::component::input::TextareaState;
use gpui_kit::component::notification::NotificationType;

mod credentials;
mod hosts;
#[cfg(test)]
mod tests;
mod window;

/// 一个设置会话的 SSH 能力。输入、转换及凭据基线随会话一起释放。
pub(crate) struct SshSettings {
    form: Option<SshProfileForm>,
    open: Option<Uuid>,
    show_all: bool,
    confirm_remove: bool,
    copied: Option<Uuid>,
    filter: Entity<InputState>,
    collapsed_groups: std::collections::HashSet<String>,
    credentials: std::sync::Arc<dyn CredentialStore>,
    _subs: Vec<Subscription>,
}

impl SshSettings {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<SettingsWindow>) -> Self {
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder(t(L10nKey::FilterHosts)));
        let subscription = cx.subscribe_in(&filter, window, |_, _, ev, _, cx| {
            if matches!(ev, InputEvent::Change) {
                cx.notify();
            }
        });
        #[cfg(not(test))]
        let credentials = std::sync::Arc::new(OsCredentialStore);
        #[cfg(test)]
        let credentials =
            std::sync::Arc::new(crate::core::keychain::InMemoryCredentialStore::new());
        Self {
            form: None,
            open: None,
            show_all: false,
            confirm_remove: false,
            copied: None,
            filter,
            collapsed_groups: Default::default(),
            credentials,
            _subs: vec![subscription],
        }
    }

    pub(crate) fn filter(&self) -> &Entity<InputState> {
        &self.filter
    }

    pub(super) fn cancel(&mut self) {
        self.form = None;
        self.confirm_remove = false;
    }

    fn toggle_group(&mut self, key: String) {
        if !self.collapsed_groups.remove(&key) {
            self.collapsed_groups.insert(key);
        }
    }

    fn saved(&mut self, id: Uuid) {
        self.cancel();
        self.open = Some(id);
    }

    fn removed(&mut self, id: Uuid) {
        if self.form.as_ref().is_some_and(|form| form.editing == id) {
            self.form = None;
        }
        if self.open == Some(id) {
            self.open = None;
        }
        self.confirm_remove = false;
    }

    fn select_row(&mut self, id: Uuid) {
        if self.form.is_none() {
            self.open = Some(id);
        }
    }

    fn toggle_row(&mut self, id: Uuid) {
        if self.form.as_ref().is_some_and(|form| form.editing == id) {
            return;
        }
        self.open = if self.open == Some(id) {
            None
        } else {
            Some(id)
        };
    }

    fn toggle_show_all(&mut self) {
        self.show_all = !self.show_all;
    }

    fn request_remove(&mut self) -> bool {
        let confirmed = self.confirm_remove;
        self.confirm_remove = true;
        confirmed
    }

    fn apply_profile(cfg: &mut Config, profile: &SshProfile) {
        if let Some(slot) = cfg.ssh_profiles.iter_mut().find(|p| p.id == profile.id) {
            *slot = profile.clone();
        } else {
            cfg.ssh_profiles.push(profile.clone());
        }
    }

    fn remove_profile(cfg: &mut Config, id: Uuid) {
        cfg.ssh_profiles.retain(|p| p.id != id);
        cfg.ssh_profile_frecency.remove(&id);
    }

    fn import_profiles(
        cfg: &mut Config,
        profiles: Vec<crate::core::ssh_config::ImportedProfile>,
    ) -> crate::core::ssh_config::MergeStats {
        crate::core::ssh_config::merge_imported(&mut cfg.ssh_profiles, profiles)
    }

    fn load(
        &mut self,
        profile: &SshProfile,
        window: &mut Window,
        cx: &mut Context<SettingsWindow>,
    ) {
        let jump_name = profile
            .jump_host
            .and_then(|id| {
                cx.global::<Config>()
                    .ssh_profiles
                    .iter()
                    .find(|p| p.id == id)
                    .map(|p| p.name.clone())
            })
            .unwrap_or_default();

        let name = seed_hinted(window, cx, &profile.name, t(L10nKey::SettingsNameHint));
        let host = seed_hinted(window, cx, &profile.host, t(L10nKey::SettingsHostHint));
        let port = seed_input(window, cx, &profile.port.to_string());
        let user = seed_hinted(window, cx, &profile.user, t(L10nKey::SettingsUserHint));
        let jump = seed_input(window, cx, &jump_name);
        let forwards: Vec<ForwardRuleForm> = profile
            .forwards
            .iter()
            .map(|r| seed_forward_row(window, cx, r))
            .collect();
        // One line, comma-separated: the form is a row of single-line fields,
        // and a profile with more than one key is the exception.
        let identity_files = seed_hinted(
            window,
            cx,
            &profile.identity_files.join(", "),
            DEFAULT_KEY_HINT,
        );

        // One keychain read per host opened, not one per keystroke: the
        // password box shows what is actually stored, the way every other SSH
        // client shows it, so it can be read back, corrected or cleared
        // without connecting first.
        let loaded_password = stored_password(profile, self.credentials.as_ref());
        let loaded_key = first_readable_key(profile);
        let loaded_passphrase = loaded_key
            .as_deref()
            .map(|key| stored_passphrase(key, self.credentials.as_ref()))
            .unwrap_or_default();
        let password = cx.new(|cx| {
            InputState::new(window, cx)
                .masked(true)
                .placeholder(t(L10nKey::SettingsPasswordHint))
                .default_value(loaded_password.clone())
        });
        let passphrase = cx.new(|cx| {
            InputState::new(window, cx)
                .masked(true)
                .placeholder(t(L10nKey::SettingsPasswordHint))
                .default_value(loaded_passphrase.clone())
        });
        let proxy_command = seed_input(window, cx, profile.proxy_command.as_deref().unwrap_or(""));
        let socks = seed_input(window, cx, &host_port_text(&profile.socks_proxy));
        let http = seed_input(window, cx, &host_port_text(&profile.http_proxy));
        let kex = seed_input(window, cx, &profile.algorithms.kex.join(", "));
        let cipher = seed_input(window, cx, &profile.algorithms.cipher.join(", "));
        let mac = seed_input(window, cx, &profile.algorithms.mac.join(", "));
        let hostkey = seed_input(window, cx, &profile.algorithms.hostkey.join(", "));
        let compression = seed_input(window, cx, &profile.algorithms.compression.join(", "));
        let keepalive_interval = seed_input(
            window,
            cx,
            &profile
                .keepalive_interval_s
                .map(|n| n.to_string())
                .unwrap_or_default(),
        );
        let keepalive_count = seed_input(
            window,
            cx,
            &profile
                .keepalive_count_max
                .map(|n| n.to_string())
                .unwrap_or_default(),
        );
        let connect_timeout = seed_input(
            window,
            cx,
            &profile
                .connect_timeout_s
                .map(|n| n.to_string())
                .unwrap_or_default(),
        );
        let login_scripts = cx.new(|cx| {
            TextareaState::new(window, cx).default_value(profile.login_scripts.join("\n"))
        });

        let mut subs = Vec::new();
        // The passphrase belongs to whichever key the field above names, so
        // when that answer changes the box has to change with it. Without this
        // a form opened on one key and pointed at another would carry the
        // first key's passphrase across and save it over the second's. Host and
        // user count too: they fill in a `%h` / `%r` in the key's path.
        for input in [&identity_files, &host, &user] {
            subs.push(
                cx.subscribe_in(input, window, |this, _i, ev: &InputEvent, window, cx| {
                    if matches!(ev, InputEvent::Change) {
                        this.resync_key_passphrase(window, cx);
                    }
                }),
            );
        }
        subs.push(
            cx.subscribe_in(&login_scripts, window, |_, _, ev: &InputEvent, _, cx| {
                if matches!(ev, InputEvent::Change) {
                    cx.notify();
                }
            }),
        );
        let mut watch = vec![
            &name,
            &host,
            &port,
            &user,
            &jump,
            &password,
            &passphrase,
            &identity_files,
            &proxy_command,
            &socks,
            &http,
            &kex,
            &cipher,
            &mac,
            &hostkey,
            &compression,
            &keepalive_interval,
            &keepalive_count,
            &connect_timeout,
        ];
        for row in &forwards {
            watch.extend(forward_row_inputs(row));
        }
        for input in watch {
            subs.push(
                cx.subscribe_in(input, window, |_, _, ev: &InputEvent, _, cx| {
                    if matches!(ev, InputEvent::Change) {
                        cx.notify();
                    }
                }),
            );
        }

        let form = SshProfileForm {
            editing: profile.id,
            carry_group: profile.group.clone(),
            carry_credential_ref: profile.credential_ref.clone(),
            name,
            host,
            port,
            user,
            auth: profile.auth,
            password,
            passphrase,
            loaded_password,
            loaded_passphrase,
            loaded_endpoint: (profile.user.clone(), profile.host.clone(), profile.port),
            loaded_key,
            jump,
            forwards,
            identity_files,
            proxy_command,
            socks,
            http,
            kex,
            cipher,
            mac,
            hostkey,
            compression,
            keepalive_interval,
            keepalive_count,
            connect_timeout,
            login_scripts,
            agent_forward: profile.agent_forward,
            x11: profile.x11,
            skip_banner: profile.skip_banner,
            shell_integration: profile.shell_integration,
            remote_clipboard_write: profile.remote_clipboard_write,
            verify_host_keys: profile.verify_host_keys,
            warn_on_close: profile.warn_on_close,
            _subs: subs,
        };
        let editing = form.editing;
        let known = cx
            .global::<Config>()
            .ssh_profiles
            .iter()
            .any(|p| p.id == editing);
        self.form = Some(form);
        self.confirm_remove = false;
        // A saved host edits in its own row; a new one gets a row of its
        // own at the top, so nothing else is shown opened.
        self.open = known.then_some(editing);
    }

    /// Reads the form out of its entities and runs it past
    /// [`validate_ssh_draft`]. The profile that comes back is what the form
    /// would save; the errors are what stands in the way.
    fn collect(&self, profiles: &[SshProfile], cx: &App) -> Option<(SshProfile, SshFormErrors)> {
        let form = self.form.as_ref()?;
        let val = |e: &Entity<InputState>| e.read(cx).value().trim().to_string();
        // The multi-line and comma-separated fields do their own splitting, so
        // they travel whole rather than trimmed.
        let raw = |e: &Entity<InputState>| e.read(cx).value().to_string();

        let draft = SshFormDraft {
            id: form.editing,
            name: val(&form.name),
            group: form.carry_group.clone(),
            host: val(&form.host),
            port: val(&form.port),
            user: val(&form.user),
            jump: val(&form.jump),
            proxy_command: val(&form.proxy_command),
            socks: val(&form.socks),
            http: val(&form.http),
            auth: form.auth,
            identity_files: raw(&form.identity_files).replace(',', "\n"),
            agent_forward: form.agent_forward,
            credential_ref: form.carry_credential_ref.clone(),
            forwards: form.forwards.iter().filter_map(|r| r.collect(cx)).collect(),
            keepalive_interval: val(&form.keepalive_interval),
            keepalive_count: val(&form.keepalive_count),
            connect_timeout: val(&form.connect_timeout),
            warn_on_close: form.warn_on_close,
            skip_banner: form.skip_banner,
            shell_integration: form.shell_integration,
            remote_clipboard_write: form.remote_clipboard_write,
            login_scripts: form.login_scripts.read(cx).value().to_string(),
            x11: form.x11,
            kex: raw(&form.kex),
            cipher: raw(&form.cipher),
            mac: raw(&form.mac),
            hostkey: raw(&form.hostkey),
            compression: raw(&form.compression),
            verify_host_keys: form.verify_host_keys,
        };
        Some(validate_ssh_draft(draft, profiles))
    }

    fn profile_to_save(&self, profiles: &[SshProfile], cx: &App) -> Option<SshProfile> {
        let (profile, errors) = self.collect(profiles, cx)?;
        errors.is_empty().then_some(profile)
    }

    /// Whether the SSH profile form on screen holds edits that were never
    /// saved. Save is enabled off exactly this, so closing on it is the same
    /// question the button already answers — and it compares what the form
    /// would save even when the form cannot be saved yet, so a half-typed new
    /// host is still something Escape has to ask about.
    pub(super) fn is_dirty(&self, profiles: &[SshProfile], cx: &App) -> bool {
        let Some(form) = self.form.as_ref() else {
            return false;
        };
        let saved = profiles.iter().find(|p| p.id == form.editing).cloned();
        self.collect(profiles, cx).map(|(profile, _)| profile) != saved || form.secrets_changed(cx)
    }
}

fn ssh_group_key(p: &SshProfile) -> &str {
    p.group.as_deref().unwrap_or("")
}

fn ssh_group_label(key: &str) -> &str {
    match key {
        crate::core::ssh_config::IMPORTED_GROUP => "~/.ssh/config",
        "" => t(L10nKey::SettingsInLyrel),
        other => other,
    }
}

fn ssh_group_rank(key: &str) -> u8 {
    match key {
        crate::core::ssh_config::IMPORTED_GROUP => 0,
        "" => 2,
        _ => 1,
    }
}

fn ssh_row_matches(p: &SshProfile, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    let hit = |s: &str| s.to_lowercase().contains(query);
    hit(&p.name) || hit(&p.host) || hit(&p.user) || hit(&p.port.to_string())
}

/// How many *other* profiles reach the same `user@host:port` as this one.
///
/// The keychain is keyed by the endpoint, not by the profile, so two hosts that
/// differ only in how they get there — one direct, one through a jump host —
/// hand the same saved password back and forth. Every path that is about to
/// remove that password has to know this first: deleting a profile keeps the
/// secret while someone else still needs it, and forgetting one says out loud
/// who else it takes down. Both used to work the answer out on their own, which
/// is exactly how the two policies would have drifted apart.
fn profiles_sharing_endpoint(cfg: &Config, id: Uuid) -> usize {
    let Some(profile) = cfg.ssh_profiles.iter().find(|p| p.id == id) else {
        return 0;
    };
    cfg.ssh_profiles
        .iter()
        .filter(|p| {
            p.id != id && (&p.user, &p.host, p.port) == (&profile.user, &profile.host, profile.port)
        })
        .count()
}

struct SshProfileForm {
    editing: Uuid,
    carry_group: Option<String>,
    carry_credential_ref: Option<CredentialRef>,

    name: Entity<InputState>,
    host: Entity<InputState>,
    port: Entity<InputState>,
    user: Entity<InputState>,
    auth: AuthMode,

    /// The secret half of a connection. Neither of these is part of the
    /// profile — they live in the system keychain, and the config file holds
    /// no copy — so the form carries what the keychain had when it opened and
    /// compares against it on the way out. A form that was only read writes
    /// nothing back, and one that cleared a field says so.
    password: Entity<InputState>,
    passphrase: Entity<InputState>,
    loaded_password: String,
    loaded_passphrase: String,
    /// The endpoint the password above was read for, and the key file the
    /// passphrase belongs to. An edit to the address moves the entry, and
    /// without these there is nothing left pointing at the one to remove.
    loaded_endpoint: (String, String, u16),
    loaded_key: Option<String>,

    jump: Entity<InputState>,

    forwards: Vec<ForwardRuleForm>,

    identity_files: Entity<InputState>,
    proxy_command: Entity<InputState>,
    socks: Entity<InputState>,
    http: Entity<InputState>,
    kex: Entity<InputState>,
    cipher: Entity<InputState>,
    mac: Entity<InputState>,
    hostkey: Entity<InputState>,
    compression: Entity<InputState>,
    keepalive_interval: Entity<InputState>,
    keepalive_count: Entity<InputState>,
    connect_timeout: Entity<InputState>,
    login_scripts: Entity<TextareaState>,

    agent_forward: bool,
    x11: bool,
    skip_banner: bool,
    shell_integration: bool,
    remote_clipboard_write: bool,
    verify_host_keys: Option<bool>,
    warn_on_close: Option<bool>,

    _subs: Vec<Subscription>,
}

impl SshProfileForm {
    /// Whether the group that identifies the host — name, host, port, user —
    /// is still untouched. Every field notifies on change, so the form
    /// re-renders on each keystroke; without this a new host would be told it
    /// needs a host before anyone had the chance to type one. Same deal the
    /// forward rows strike with `ForwardRuleForm::is_blank`.
    fn core_is_blank(&self, cx: &App) -> bool {
        // The port is not in the list: it opens on 22 and is never empty, so
        // counting it meant a brand-new host was never "untouched" and the
        // form opened with "Needs a host" already in red under an empty box
        // nobody had reached yet.
        [&self.name, &self.host, &self.user]
            .iter()
            .all(|e| e.read(cx).value().trim().is_empty())
    }

    /// Whether either secret differs from what the keychain handed over.
    ///
    /// Nothing about a password reaches the profile, so the dirty check that
    /// compares profiles cannot see one being typed — without this, Save stays
    /// greyed out over a password the user just entered.
    ///
    /// Untrimmed on purpose: a trailing space is a character of the secret,
    /// and the server is the one that decides whether it belongs.
    fn secrets_changed(&self, cx: &App) -> bool {
        self.password.read(cx).value().as_ref() != self.loaded_password
            || self.passphrase.read(cx).value().as_ref() != self.loaded_passphrase
    }

    /// Which credential boxes the method actually uses — the same split
    /// `ssh_connect::build_spec_inner` makes when it decides what to hand the
    /// daemon. A password box under "Agent" would be a secret that is stored
    /// and then never offered.
    fn wants_password(&self) -> bool {
        matches!(self.auth, AuthMode::Auto | AuthMode::Password)
    }

    /// A passphrase is filed against a key's contents, so the box only shows
    /// once the form names a key that is actually on this machine.
    fn wants_passphrase(&self) -> bool {
        matches!(self.auth, AuthMode::Auto | AuthMode::PublicKey) && self.loaded_key.is_some()
    }
}

/// What saving does to the keychain for the password field: which entry to
/// drop, and whether to write one.
///
/// A plain function because these are the cases a keychain makes expensive to
/// reach by hand — an address edited out from under a saved password, a field
/// cleared to mean "stop remembering this", a form opened and closed without a
/// keystroke.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct PasswordPlan {
    drop_old: bool,
    store: bool,
}

fn password_plan(was: &str, typed: &str, moved: bool) -> PasswordPlan {
    PasswordPlan {
        // Only what this form read is ours to drop, and only once it is no
        // longer the entry this form would write to.
        drop_old: !was.is_empty() && (moved || typed.is_empty()),
        // A move rewrites even an unchanged secret: the account it is filed
        // under is the address, and the address is what changed.
        store: !typed.is_empty() && (typed != was || moved),
    }
}

struct ForwardRuleForm {
    kind: ForwardKind,
    bind_host: Entity<InputState>,
    bind_port: Entity<InputState>,
    target_host: Entity<InputState>,
    target_port: Entity<InputState>,
    description: Entity<InputState>,
    enabled: bool,
}

impl ForwardRuleForm {
    fn collect(&self, cx: &App) -> Option<ForwardRule> {
        let val = |e: &Entity<InputState>| e.read(cx).value().trim().to_string();
        let bind_port: u16 = val(&self.bind_port).parse().ok().filter(|p| *p > 0)?;
        let bind = HostPort::new(val(&self.bind_host), bind_port);
        let target = if self.kind == ForwardKind::Dynamic {
            HostPort::default()
        } else {
            let port: u16 = val(&self.target_port).parse().ok().filter(|p| *p > 0)?;
            let host = val(&self.target_host);
            if host.is_empty() {
                return None;
            }
            HostPort::new(host, port)
        };
        Some(ForwardRule {
            kind: self.kind,
            bind,
            target,
            description: val(&self.description),
            enabled: self.enabled,
        })
    }
}

/// What a blank port field means. The same number `SshProfile`'s serde default
/// writes for a config that never mentioned a port, which is why leaving the
/// field empty has to stay legal: every host imported from `~/.ssh/config`
/// leaves it empty.
const DEFAULT_SSH_PORT: u16 = 22;

/// The port each proxy scheme listens on when the field names only a host.
const DEFAULT_SOCKS_PORT: u16 = 1080;
const DEFAULT_HTTP_PROXY_PORT: u16 = 8080;

/// A port as a form field spells it. Nothing here accepts 0: every port in a
/// profile is one something has to connect to, and no listener answers on 0.
fn parse_port(s: &str) -> Option<u16> {
    s.trim().parse::<u16>().ok().filter(|p| *p > 0)
}

/// A proxy address as the form spells it: blank is "no proxy", a bare host
/// takes the scheme's default port, and anything else has to carry a port that
/// exists. This used to be `parse().unwrap_or(0)`, so `proxy.example.com:88O`
/// saved a proxy on port 0 and the failure surfaced far away, in the socket
/// layer. The default port is not a secret either — `host_port_text` writes it
/// back into the field the next time the form opens.
fn parse_host_port_checked(s: &str, default_port: u16) -> Result<Option<HostPort>, SshFieldError> {
    let s = s.trim();
    if s.is_empty() {
        return Ok(None);
    }
    match s.rsplit_once(':') {
        Some((h, p)) => match parse_port(p) {
            Some(port) => Ok(Some(HostPort::new(h.trim(), port))),
            None => Err(SshFieldError::ProxyPortRange),
        },
        None => Ok(Some(HostPort::new(s, default_port))),
    }
}

fn host_port_text(hp: &Option<HostPort>) -> String {
    hp.as_ref()
        .map(|h| format!("{}:{}", h.host, h.port))
        .unwrap_or_default()
}

fn split_list(s: &str) -> Vec<String> {
    s.split([',', ' ', '\n'])
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_string)
        .collect()
}

fn split_lines(s: &str) -> Vec<String> {
    s.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}

/// Why one field of the SSH profile form cannot be saved. A value rather than
/// a finished sentence, so the rules stay a plain function a test can call —
/// the wording, and the locale it is written in, belong to the render pass.
#[derive(Debug, Clone, PartialEq, Eq)]
enum SshFieldError {
    /// Nothing to connect to. Saved anyway, the profile used to render as an
    /// empty row in the host list and hand `TcpStream::connect` an empty name.
    HostMissing,
    /// A port field that is neither blank nor a port.
    PortRange,
    /// The same, for the port half of a proxy address.
    ProxyPortRange,
    /// The jump field names a profile no host list has.
    JumpUnknown(String),
    /// The jump field names the profile being edited.
    JumpIsSelf,
}

impl SshFieldError {
    fn message(&self) -> String {
        match self {
            Self::HostMissing => t(L10nKey::SettingsHostRequired).to_string(),
            Self::PortRange => t(L10nKey::SettingsPortInvalid).to_string(),
            Self::ProxyPortRange => t(L10nKey::SettingsProxyPortInvalid).to_string(),
            Self::JumpUnknown(name) => {
                t_fmt(L10nKey::SettingsJumpHostUnknown, &[("jump_name", name)])
            }
            Self::JumpIsSelf => t(L10nKey::SettingsJumpHostSelf).to_string(),
        }
    }
}

/// What the form has to fix before it can be saved, one slot per field so each
/// complaint can be printed under the control it is about.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct SshFormErrors {
    host: Option<SshFieldError>,
    port: Option<SshFieldError>,
    jump: Option<SshFieldError>,
    socks: Option<SshFieldError>,
    http: Option<SshFieldError>,
}

impl SshFormErrors {
    fn is_empty(&self) -> bool {
        self.host.is_none()
            && self.port.is_none()
            && self.jump.is_none()
            && self.socks.is_none()
            && self.http.is_none()
    }
}

/// The SSH profile form as plain text, lifted out of the `InputState` entities
/// it lives in. The rules that turn it into a profile are the part worth
/// testing, and a GPUI entity is not something a unit test can hand them, so
/// the window layer's job stops at reading the strings out.
#[derive(Debug, Clone, Default)]
struct SshFormDraft {
    id: Uuid,
    name: String,
    group: Option<String>,
    host: String,
    port: String,
    user: String,
    jump: String,
    proxy_command: String,
    socks: String,
    http: String,
    auth: AuthMode,
    identity_files: String,
    agent_forward: bool,
    credential_ref: Option<CredentialRef>,
    forwards: Vec<ForwardRule>,
    keepalive_interval: String,
    keepalive_count: String,
    connect_timeout: String,
    warn_on_close: Option<bool>,
    skip_banner: bool,
    shell_integration: bool,
    remote_clipboard_write: bool,
    login_scripts: String,
    x11: bool,
    kex: String,
    cipher: String,
    mac: String,
    hostkey: String,
    compression: String,
    verify_host_keys: Option<bool>,
}

/// The one place that decides what the form would save and what is wrong with
/// it. Both, always — never one or the other: the Escape prompt asks whether
/// the form differs from what is on disk, and a form that is merely invalid
/// still holds everything the user typed. Handing back only the errors would
/// make a brand-new invalid profile compare equal to the nothing on disk, and
/// Escape would throw the typing away without asking.
///
/// A missing `name` is deliberately not an error: the host list already falls
/// back to the host for a nameless profile, and requiring one would refuse
/// every host imported from `~/.ssh/config`.
fn validate_ssh_draft(draft: SshFormDraft, profiles: &[SshProfile]) -> (SshProfile, SshFormErrors) {
    let mut errors = SshFormErrors::default();

    let host = draft.host.trim().to_string();
    if host.is_empty() {
        errors.host = Some(SshFieldError::HostMissing);
    }

    let port_text = draft.port.trim();
    let port = match port_text.is_empty() {
        true => DEFAULT_SSH_PORT,
        false => parse_port(port_text).unwrap_or_else(|| {
            errors.port = Some(SshFieldError::PortRange);
            DEFAULT_SSH_PORT
        }),
    };

    // The field is a name but the profile stores an id, so a jump host already
    // survives its target being renamed. What it never survived was a name
    // nobody has: the lookup returned `None`, the profile saved as a direct
    // connection, and reopening the form showed an empty field.
    let jump_name = draft.jump.trim();
    let jump_host = if jump_name.is_empty() {
        None
    } else {
        let named = |p: &&SshProfile| p.name == jump_name;
        // Duplicate names resolve to whichever profile comes first, as they
        // always have. The one profile that can never be the answer is the one
        // being edited, and typing its own name is worth saying out loud
        // rather than quietly connecting direct.
        match profiles.iter().filter(named).find(|p| p.id != draft.id) {
            Some(p) => Some(p.id),
            None => {
                errors.jump = Some(match profiles.iter().any(|p| p.name == jump_name) {
                    true => SshFieldError::JumpIsSelf,
                    false => SshFieldError::JumpUnknown(jump_name.to_string()),
                });
                None
            }
        }
    };

    let proxy = |text: &str, default_port: u16, slot: &mut Option<SshFieldError>| {
        match parse_host_port_checked(text, default_port) {
            Ok(hp) => hp,
            Err(e) => {
                *slot = Some(e);
                None
            }
        }
    };
    let socks_proxy = proxy(&draft.socks, DEFAULT_SOCKS_PORT, &mut errors.socks);
    let http_proxy = proxy(&draft.http, DEFAULT_HTTP_PROXY_PORT, &mut errors.http);

    let proxy_command = draft.proxy_command.trim();
    let profile = SshProfile {
        id: draft.id,
        name: draft.name.trim().to_string(),
        group: draft.group,
        host,
        port,
        user: draft.user.trim().to_string(),
        jump_host,
        proxy_command: (!proxy_command.is_empty()).then(|| proxy_command.to_string()),
        socks_proxy,
        http_proxy,
        auth: draft.auth,
        identity_files: split_lines(&draft.identity_files),
        agent_forward: draft.agent_forward,
        credential_ref: draft.credential_ref,
        forwards: draft.forwards,
        keepalive_interval_s: draft.keepalive_interval.trim().parse().ok(),
        keepalive_count_max: draft.keepalive_count.trim().parse().ok(),
        connect_timeout_s: draft.connect_timeout.trim().parse().ok(),
        warn_on_close: draft.warn_on_close,
        skip_banner: draft.skip_banner,
        shell_integration: draft.shell_integration,
        remote_clipboard_write: draft.remote_clipboard_write,
        login_scripts: split_lines(&draft.login_scripts),
        x11: draft.x11,
        algorithms: Algorithms {
            kex: split_list(&draft.kex),
            cipher: split_list(&draft.cipher),
            mac: split_list(&draft.mac),
            hostkey: split_list(&draft.hostkey),
            compression: split_list(&draft.compression),
        },
        verify_host_keys: draft.verify_host_keys,
    };
    (profile, errors)
}

fn forward_row_inputs(row: &ForwardRuleForm) -> [&Entity<InputState>; 5] {
    [
        &row.bind_host,
        &row.bind_port,
        &row.target_host,
        &row.target_port,
        &row.description,
    ]
}

fn seed_forward_row(
    window: &mut Window,
    cx: &mut Context<SettingsWindow>,
    rule: &ForwardRule,
) -> ForwardRuleForm {
    let port = |p: u16| if p == 0 { String::new() } else { p.to_string() };
    ForwardRuleForm {
        kind: rule.kind,
        bind_host: seed_hinted(window, cx, &rule.bind.host, "localhost"),
        bind_port: seed_hinted(window, cx, &port(rule.bind.port), "8080"),
        target_host: seed_hinted(window, cx, &rule.target.host, "127.0.0.1"),
        target_port: seed_hinted(window, cx, &port(rule.target.port), "80"),
        description: seed_hinted(
            window,
            cx,
            &rule.description,
            t(L10nKey::ForwardDescriptionPlaceholder),
        ),
        enabled: rule.enabled,
    }
}

fn seed_hinted(
    window: &mut Window,
    cx: &mut Context<SettingsWindow>,
    value: &str,
    placeholder: &'static str,
) -> Entity<InputState> {
    let value = value.to_string();
    cx.new(|cx| {
        InputState::new(window, cx)
            .placeholder(placeholder)
            .default_value(value)
    })
}

fn seed_input(
    window: &mut Window,
    cx: &mut Context<SettingsWindow>,
    value: &str,
) -> Entity<InputState> {
    let value = value.to_string();
    cx.new(|cx| InputState::new(window, cx).default_value(value))
}

/// What the key field shows while it is empty: the file ssh would reach for on
/// its own. A hint, not a value — an empty field still means "try the usual
/// `~/.ssh` keys", which is exactly what `default_identity_candidates` does.
const DEFAULT_KEY_HINT: &str = "~/.ssh/id_ed25519";
