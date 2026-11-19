//! Read-only lookups the terminal — and anything else below the workbench —
//! needs about hosts, machines and workspaces, without importing the
//! controllers that produce the answers.
//!
//! Three kinds of things live here, all values or `Global` snapshots:
//!
//! - **Host facts** ([`home_for_host`], the host-label listing):
//!   what the control links reported and what the configured hosts call
//!   themselves. The label listing is pure config/WSL reading — no
//!   connection logic.
//! - **Published mirror snapshots** ([`MirrorViews`]): the
//!   workbench recomputes a small per-workspace projection whenever it
//!   writes a machine mirror, and everything below reads the projection
//!   through `try_global`. A missing snapshot means "not pulled yet",
//!   exactly as a missing mirror did.
//! - **Narrow callbacks** ([`install_accepts_input`], [`install_pane_route_for`]):
//!   synchronous questions whose answers depend on live supervisor state the
//!   workbench owns. The rule stays in one place — the workbench installs it,
//!   this module only forwards the call.
//!
//! Nothing here may hold a pane, a terminal entity, a tray action or any
//! shell controller: only protocol values and `gpui_kit` globals.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use gpui_kit::{App, AppContext as _, BorrowAppContext as _, Global};

use crate::config::Config;
use crate::i18n::{L10nKey, t};
use crate::route::PaneRoute;
use crate::session::{RemoteRef, RemoteTarget, WorkspaceId, WorkspaceStore};
use lyrel_protocol::host::HostId;

#[derive(Default)]
struct RemoteHomes(HashMap<HostId, PathBuf>);

impl Global for RemoteHomes {}

/// Publish the home from a remote control handshake, or clear it on disconnect.
pub fn publish_remote_home(cx: &mut App, host: HostId, home: Option<PathBuf>) {
    let homes = &mut cx.default_global::<RemoteHomes>().0;
    match home {
        Some(home) => {
            homes.insert(host, home);
        }
        None => {
            homes.remove(&host);
        }
    }
}

/// A completed control handshake has published this remote link's facts.
pub(crate) fn has_remote_home(cx: &App, host: HostId) -> bool {
    cx.try_global::<RemoteHomes>()
        .is_some_and(|homes| homes.0.contains_key(&host))
}

/// The directory `~` stands for on the machine lyrel is running on, or `None`
/// when it won't say.
///
/// `USERPROFILE` is the fallback rather than the only source on Windows so
/// the MSYS/Git-Bash environments that do export `HOME` keep working, and
/// the two agree in every case that matters.
pub fn local_home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .or_else(|| std::env::var_os("USERPROFILE").filter(|h| !h.is_empty()))
        .map(PathBuf::from)
}

/// The directory `~` stands for in a path that lives on `host`.
///
/// A remote host reports its home during the control handshake and it is
/// published per host by the shell, so this costs a map lookup and never a
/// round trip. Without a link, return `None` so paths are shown whole (#568).
pub fn home_for_host(cx: &App, host: HostId) -> Option<PathBuf> {
    match host.is_local() {
        true => local_home(),
        false => cx.try_global::<RemoteHomes>()?.0.get(&host).cloned(),
    }
}

// ---------------------------------------------------------------------------
// Host labels: what a configured target calls itself.
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostChoice {
    pub target: RemoteTarget,
    pub label: String,
    pub detail: String,
}

pub fn available_hosts(cx: &App) -> Vec<HostChoice> {
    let mut out: Vec<HostChoice> = Vec::new();
    let mut seen: Vec<String> = Vec::new();

    out.extend(local_stdio_host());

    for profile in &cx.global::<Config>().ssh_profiles {
        let target = RemoteTarget::Profile { id: profile.id };
        seen.push(profile.name.clone());
        out.push(HostChoice {
            detail: endpoint_label(&profile.user, &profile.host, profile.port),
            label: profile_label(profile),
            target,
        });
    }

    out.extend(wsl_hosts(cx));

    for imported in crate::ssh_config::import_profiles() {
        let alias = imported.profile.name.clone();
        if alias.trim().is_empty() || seen.iter().any(|s| s == &alias) {
            continue;
        }
        out.push(HostChoice {
            detail: endpoint_label(
                &imported.profile.user,
                &imported.profile.host,
                imported.profile.port,
            ),
            label: alias,
            target: RemoteTarget::Alias {
                alias: imported.profile.name,
            },
        });
    }
    out
}

/// A label for a bare target when no route snapshot is at hand (a pane's
/// `PaneWorkspace` carries only the target): the live listing while it
/// exists, the target's own spelling when that is human-readable, and the
/// deleted-profile placeholder for the bare-UUID case (#485).
pub fn target_label(cx: &App, target: &RemoteTarget) -> String {
    label_from_hosts(&available_hosts(cx), target)
}

/// `target_label`'s rule applied to a listing the caller already has. The
/// switcher builds `available_hosts` once per frame and names several targets
/// from it; re-listing per target would re-read `~/.ssh/config` off the disk
/// on the render path, which is the cost `route_label` goes out of its way
/// to avoid. One rule, two entry points — so a name shown next to a pane and
/// the same name shown in the switcher cannot drift apart (#485).
pub fn label_from_hosts(hosts: &[HostChoice], target: &RemoteTarget) -> String {
    if let Some(choice) = hosts.iter().find(|h| h.target == *target) {
        return choice.label.clone();
    }
    match target {
        RemoteTarget::Profile { .. } => t(L10nKey::RemoteProfileGone).to_string(),
        other => other.to_string(),
    }
}

/// The name a route entry answers to: the live listing while the route still
/// exists, then the remembered snapshot, and — for a profile entry saved
/// before snapshots existed — a placeholder, but never the bare profile UUID
/// (#485).
pub fn route_label(cx: &App, host: &RemoteRef) -> String {
    match live_label(cx, &host.target) {
        Some(label) => label,
        None => host.route_label(t(L10nKey::RemoteProfileGone)),
    }
}

/// What the live config calls a target, reading memory only. Deliberately not
/// `available_hosts`: `route_label` runs on the render path of every remote
/// window (the workspace strip), and that listing re-reads `~/.ssh/config`
/// off the disk. The two targets it answers are the two whose name lives
/// somewhere other than the target itself; the rest spell themselves the same
/// way the listing labels them, so falling through costs nothing.
fn live_label(cx: &App, target: &RemoteTarget) -> Option<String> {
    match target {
        RemoteTarget::Profile { id } => cx
            .global::<Config>()
            .ssh_profiles
            .iter()
            .find(|p| p.id == *id)
            .map(profile_label),
        RemoteTarget::Wsl { .. } => wsl_hosts(cx)
            .into_iter()
            .find(|h| h.target == *target)
            .map(|h| h.label),
        RemoteTarget::Alias { .. }
        | RemoteTarget::Direct { .. }
        | RemoteTarget::LocalStdio { .. } => None,
    }
}

/// What a profile calls itself in a host listing: its name, or the address
/// when it was saved without one.
fn profile_label(profile: &lyrel_protocol::core::ssh_profile::SshProfile) -> String {
    if profile.name.trim().is_empty() {
        profile.host.clone()
    } else {
        profile.name.clone()
    }
}

pub fn filter_hosts(hosts: &[HostChoice], query: &str) -> Vec<HostChoice> {
    let query = query.trim();
    if query.is_empty() {
        return hosts.to_vec();
    }
    let mut scored: Vec<(i32, &HostChoice)> = hosts
        .iter()
        .filter_map(|host| host_score(query, host).map(|score| (score, host)))
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0));
    scored.into_iter().map(|(_, host)| host.clone()).collect()
}

fn host_score(query: &str, host: &HostChoice) -> Option<i32> {
    let label = crate::fuzzy::fuzzy_score(query, &host.label);
    let detail = crate::fuzzy::fuzzy_score(query, &host.detail).map(|score| score - 3);
    label.into_iter().chain(detail).max()
}

pub const LOCAL_STDIO_ENV: &str = "LYREL_LOCAL_STDIO_SERVER";

fn local_stdio_host() -> Option<HostChoice> {
    let program = std::env::var(LOCAL_STDIO_ENV)
        .ok()
        .filter(|p| !p.is_empty())?;
    let target = RemoteTarget::LocalStdio {
        program: program.clone(),
        args: vec!["--stdio".to_string()],
    };
    Some(HostChoice {
        label: format!("{target}"),
        detail: format!("{program} --stdio ({})", t(L10nKey::RemoteThisComputer)),
        target,
    })
}

fn wsl_detail() -> String {
    format!("WSL · {}", t(L10nKey::RemoteThisComputer))
}

fn wsl_hosts(cx: &App) -> Vec<HostChoice> {
    let names = cx
        .try_global::<WslDistros>()
        .map(|state| state.names.as_slice())
        .unwrap_or_default();
    wsl_choices(names)
}

fn wsl_choices(names: &[String]) -> Vec<HostChoice> {
    names
        .iter()
        .map(|distro| HostChoice {
            target: RemoteTarget::Wsl {
                distro: distro.clone(),
            },
            label: distro.clone(),
            detail: wsl_detail(),
        })
        .collect()
}

#[derive(Default)]
struct WslDistros {
    names: Vec<String>,
    probed_at: Option<Instant>,
    in_flight: bool,
}

impl Global for WslDistros {}

const WSL_TTL: Duration = Duration::from_secs(30);

pub fn sweep_wsl(cx: &mut App) {
    if !cfg!(windows) {
        return;
    }
    {
        let state = cx.default_global::<WslDistros>();
        if state.in_flight || state.probed_at.is_some_and(|at| at.elapsed() < WSL_TTL) {
            return;
        }
    }
    cx.update_global::<WslDistros, _>(|state, _| state.in_flight = true);
    cx.spawn(async move |cx| {
        let probed = cx
            .background_spawn(async { lyrel_protocol::core::shells::wsl_distros_probed() })
            .await;
        let _ = cx.update(|cx| {
            cx.update_global::<WslDistros, _>(|state, _| adopt_probe(state, probed));
            cx.refresh_windows();
        });
    })
    .detach();
}

fn adopt_probe(state: &mut WslDistros, probed: Option<Vec<String>>) {
    if let Some(names) = probed {
        state.names = names;
    }
    state.probed_at = Some(Instant::now());
    state.in_flight = false;
}

fn endpoint_label(user: &str, host: &str, port: u16) -> String {
    let base = if user.is_empty() {
        host.to_string()
    } else {
        format!("{user}@{host}")
    };
    if port == 22 {
        base
    } else {
        format!("{base}:{port}")
    }
}

// ---------------------------------------------------------------------------
// Published mirror snapshots: what the machine tree says about a workspace,
// recomputed by the mirror owner on its own writes.
// ---------------------------------------------------------------------------

/// The per-workspace projection of a machine mirror that lower layers read:
/// which panes the tree says the workspace has, and the name a row would
/// show for it. Recomputed by `crate::ui::machine_mirror` every time it
/// writes a mirror, so a reader here never walks the tree itself.
#[derive(Clone, Debug, Default)]
pub struct WorkspaceMirror {
    pane_ids: Vec<u64>,
    /// Name or subject basename, not a localized untitled placeholder.
    name: Option<String>,
}

impl WorkspaceMirror {
    pub fn new(pane_ids: Vec<u64>, name: Option<String>) -> Self {
        Self { pane_ids, name }
    }
}

#[derive(Default)]
struct HostMirror {
    workspaces: HashMap<WorkspaceId, WorkspaceMirror>,
}

/// The last published projection per machine. A host with an entry has been
/// pulled at least once; a workspace absent from its map is the same empty
/// answer a ready mirror with no such workspace gave before snapshots
/// existed.
#[derive(Default)]
pub struct MirrorViews {
    hosts: HashMap<HostId, HostMirror>,
}

impl Global for MirrorViews {}

/// Replace the whole projection for `host` — the mirror owner installed or
/// re-pulled the machine tree, so every workspace is recomputed at once and
/// entries for workspaces the tree no longer holds are dropped.
pub fn publish_host_mirror(
    cx: &mut App,
    host: HostId,
    workspaces: Vec<(WorkspaceId, WorkspaceMirror)>,
) {
    let views = cx.default_global::<MirrorViews>();
    let entry = views.hosts.entry(host).or_default();
    entry.workspaces = workspaces.into_iter().collect();
}

fn mirror_of<'a>(
    cx: &'a App,
    entry: &lyrel_protocol::core::session::WindowView,
) -> Option<&'a WorkspaceMirror> {
    let views = cx.try_global::<MirrorViews>()?;
    let machine_ws = entry.host.as_ref().map(|r| r.workspace).unwrap_or(entry.id);
    views
        .hosts
        .get(&entry.host_id())?
        .workspaces
        .get(&machine_ws)
}

/// The pane ids the machine tree says `entry`'s workspace has — what the
/// liveness probe asks the daemon about. `None` when the machine has never
/// been pulled; a pulled machine that never heard of the workspace answers
/// `Some(Vec::new())`, the same two answers the mirror itself gave.
pub fn pane_ids(cx: &App, entry: &lyrel_protocol::core::session::WindowView) -> Option<Vec<u64>> {
    let host = entry.host_id();
    match mirror_of(cx, entry) {
        Some(mirror) => Some(mirror.pane_ids.clone()),
        None if cx
            .try_global::<MirrorViews>()
            .is_some_and(|views| views.hosts.contains_key(&host)) =>
        {
            Some(Vec::new())
        }
        None => None,
    }
}

/// The name the machine has for a workspace — what a notification title or a
/// row shows. Falls back to the store entry's own label when the mirror has
/// nothing to say, exactly as the mirror's display-name read always did.
pub fn display_name_for(cx: &App, client_ws: WorkspaceId) -> Option<String> {
    let entry = crate::session::WorkspaceStore::all(cx).get(client_ws)?;
    match mirror_of(cx, entry) {
        Some(mirror) => Some(
            mirror
                .name
                .clone()
                .unwrap_or_else(|| t(L10nKey::WindowUntitled).to_string()),
        ),
        None => entry.label.clone(),
    }
}

// ---------------------------------------------------------------------------
// Narrow callback: the route a workspace's panes attach over.
// ---------------------------------------------------------------------------

/// The route question has no bounded write set — the answer is rebuilt from
/// the store, the keychain, the link table and the config every time — so it
/// is a rule the workbench installs, not a snapshot it publishes. The rule
/// lives in `crate::ui::remote_workspace`; installed once at startup.
static PANE_ROUTE: Mutex<Option<fn(&App, WorkspaceId) -> PaneRoute>> = Mutex::new(None);

/// Install the pane-route rule. Called once, at startup, by the workbench.
pub fn install_pane_route_for(rule: fn(&App, WorkspaceId) -> PaneRoute) {
    if let Ok(mut slot) = PANE_ROUTE.lock() {
        *slot = Some(rule);
    }
}

/// The route a workspace's panes would attach over, or `Local` when no rule
/// has been installed (a headless test, an early probe) — the same answer
/// `PaneRoute::for_workspace` gave for a workspace nobody built.
pub fn pane_route_for(cx: &App, workspace: WorkspaceId) -> PaneRoute {
    PANE_ROUTE
        .lock()
        .ok()
        .and_then(|rule| rule.map(|rule| rule(cx, workspace)))
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Narrow callback: whether a workspace currently takes keystrokes.
// ---------------------------------------------------------------------------

/// The one synchronous question the terminal asks about a workspace that the
/// workbench's supervisors answer from live state: is another client driving
/// it right now. The rule lives in `crate::ui::remote_workspace`; installed
/// once at startup, forwarded here so the terminal never imports the
/// supervisor.
static ACCEPTS_INPUT: Mutex<Option<fn(&App, WorkspaceId) -> bool>> = Mutex::new(None);

/// Install the accepts-input rule. Called once, at startup, by the workbench.
pub fn install_accepts_input(rule: fn(&App, WorkspaceId) -> bool) {
    if let Ok(mut slot) = ACCEPTS_INPUT.lock() {
        *slot = Some(rule);
    }
}

/// Whether `workspace` takes keystrokes. Without a shell rule, a known remote
/// workspace has no supervisor that could have attached it. Local workspaces
/// still accept input in standalone consumers.
pub fn accepts_input(cx: &App, workspace: WorkspaceId) -> bool {
    ACCEPTS_INPUT
        .lock()
        .ok()
        .and_then(|rule| rule.map(|rule| rule(cx, workspace)))
        .unwrap_or_else(|| {
            WorkspaceStore::all(cx)
                .get(workspace)
                .is_none_or(|view| view.host.is_none())
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    /// The one rule for "what do we call a target we cannot resolve" (#485),
    /// pinned where both `target_label` and the switcher's group listing read
    /// it from.
    #[test]
    fn an_unresolvable_profile_is_named_never_spelled_as_its_uuid() {
        let id = uuid::Uuid::new_v4();
        let gone = RemoteTarget::Profile { id };

        let label = label_from_hosts(&[], &gone);
        assert!(
            !label.contains(&id.to_string()),
            "a bare profile UUID reached the UI: {label}"
        );
        assert_eq!(label, t(L10nKey::RemoteProfileGone));

        // While the profile is configured, its own name wins.
        let listed = vec![HostChoice {
            target: gone.clone(),
            label: "lager".into(),
            detail: "qhw@222.29.101.16".into(),
        }];
        assert_eq!(label_from_hosts(&listed, &gone), "lager");

        // Targets that spell themselves readably never need the placeholder.
        let alias = RemoteTarget::Alias {
            alias: "build-box".into(),
        };
        assert_eq!(label_from_hosts(&[], &alias), "build-box");
    }

    #[test]
    fn endpoint_labels_hide_the_default_port() {
        assert_eq!(endpoint_label("me", "box.local", 22), "me@box.local");
        assert_eq!(endpoint_label("me", "box.local", 2222), "me@box.local:2222");
        assert_eq!(endpoint_label("", "box.local", 22), "box.local");
    }

    fn host(label: &str, detail: &str) -> HostChoice {
        HostChoice {
            target: RemoteTarget::Alias {
                alias: label.to_string(),
            },
            label: label.to_string(),
            detail: detail.to_string(),
        }
    }

    #[test]
    fn an_empty_query_keeps_every_machine_in_order() {
        let hosts = vec![
            host("gate2jup", "root@18.143.92.244"),
            host("aws-xy", "root@52.199.113.213"),
        ];
        let all = filter_hosts(&hosts, "   ");
        assert_eq!(all, hosts);
    }

    #[test]
    fn a_query_matches_the_name_or_the_endpoint() {
        let hosts = vec![
            host("gate2jup", "root@18.143.92.244"),
            host("aws-xy", "root@52.199.113.213"),
            host("orb", "default@127.0.0.1:32222"),
        ];

        let by_name = filter_hosts(&hosts, "jup");
        assert_eq!(by_name.len(), 1);
        assert_eq!(by_name[0].label, "gate2jup");

        let by_address = filter_hosts(&hosts, "52.199");
        assert_eq!(by_address.len(), 1);
        assert_eq!(by_address[0].label, "aws-xy");

        let mixed = filter_hosts(&hosts, "or");
        assert_eq!(mixed[0].label, "orb", "a name match beats an endpoint one");
    }

    #[test]
    fn a_query_nothing_matches_returns_nothing() {
        let hosts = vec![host("gate2jup", "root@18.143.92.244")];
        assert!(filter_hosts(&hosts, "zzz").is_empty());
    }

    #[test]
    fn a_wsl_row_names_the_distro_verbatim() {
        let rows = wsl_choices(&["Ubuntu-22.04".to_string(), "Arch".to_string()]);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].label, "Ubuntu-22.04");
        assert_eq!(
            rows[0].target,
            RemoteTarget::Wsl {
                distro: "Ubuntu-22.04".to_string()
            }
        );
        assert_eq!(rows[0].target.connection_key(), "wsl:Ubuntu-22.04");
        assert_eq!(rows[1].label, "Arch");
    }

    #[test]
    fn no_distros_is_no_rows() {
        assert!(wsl_choices(&[]).is_empty());
    }

    #[test]
    fn a_distro_is_found_by_name_or_by_wsl() {
        let mut hosts = vec![host("gate2jup", "root@18.143.92.244")];
        hosts.extend(wsl_choices(&["Ubuntu".to_string()]));

        let by_name = filter_hosts(&hosts, "ubun");
        assert_eq!(by_name.len(), 1);
        assert_eq!(by_name[0].label, "Ubuntu");

        let by_kind = filter_hosts(&hosts, "wsl");
        assert_eq!(by_kind.len(), 1);
        assert_eq!(by_kind[0].label, "Ubuntu");
    }

    #[test]
    fn a_failed_probe_keeps_the_distros_it_already_had() {
        let mut state = WslDistros {
            names: vec!["Ubuntu-24.04".to_string()],
            ..Default::default()
        };
        adopt_probe(&mut state, None);

        assert_eq!(state.names, vec!["Ubuntu-24.04".to_string()]);
        assert!(state.probed_at.is_some(), "the TTL still restarts");
        assert!(!state.in_flight, "the next sweep is allowed to run");
    }

    #[test]
    fn an_answered_probe_replaces_the_list_even_when_it_is_empty() {
        let mut state = WslDistros {
            names: vec!["Ubuntu-24.04".to_string()],
            ..Default::default()
        };
        adopt_probe(&mut state, Some(Vec::new()));
        assert!(state.names.is_empty());

        adopt_probe(&mut state, Some(vec!["Arch".to_string()]));
        assert_eq!(state.names, vec!["Arch".to_string()]);
    }
}
