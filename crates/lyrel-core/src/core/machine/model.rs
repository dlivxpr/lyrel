//! Durable identities, workspace and pane records, and machine delta schema.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::core::cli_agent::CLIAgent;
use crate::core::group_key::{AutoKey, GroupId, WorkspaceGroups};
use crate::core::session::WorkspaceId;
use crate::daemon::protocol::{NativeSshSpec, ShellSpec};

use super::layout::{Side, Tab, TabId};

pub const MAX_WORKSPACES: usize = 1024;

pub const MAX_PANES: usize = 16 * 1024;

/// How long a closed tab stays reopenable (#1021).
///
/// Long enough to cover "closed it before lunch, want it back after", short
/// enough that a workspace's recently-closed list does not turn into an archive
/// of terminal output nobody asked to keep: each entry pins its panes' last
/// screens on disk, and those go when the entry does.
pub const CLOSED_TAB_TTL: Duration = Duration::from_secs(24 * 60 * 60);

/// How many closed tabs a workspace remembers, newest kept. The same bound the
/// window's in-memory list has always had, so a workspace that moves from one
/// to the other reopens exactly as far back as it did before.
pub const MAX_CLOSED_TABS: usize = 20;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Machine {
    #[serde(default)]
    pub workspaces: Vec<Workspace>,
    #[serde(default)]
    pub panes: Vec<PaneRecord>,
}

impl Machine {
    /// Workspaces users can list, open, or navigate independently. A foreign
    /// view is an internal owning-daemon projection, not another workspace.
    /// Pane ownership and sidecar consumers must still read the complete tree.
    pub fn user_workspaces(&self) -> impl Iterator<Item = &Workspace> {
        self.workspaces
            .iter()
            .filter(|workspace| workspace.foreign_view.is_none())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attachment {
    /// Proof that a connection is the one holding the workspace, so it stays
    /// between that connection and the server: it goes over no wire and onto
    /// no disk. A peer asking who holds a workspace gets the name and the
    /// time, never the means to pose as them.
    #[serde(skip)]
    pub token: String,
    #[serde(default)]
    pub hostname: String,
    #[serde(default)]
    pub since: u64,
}

impl Attachment {
    pub fn new(token: impl Into<String>, hostname: impl Into<String>) -> Attachment {
        Attachment {
            token: token.into(),
            hostname: hostname.into(),
            since: unix_now(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Workspace {
    #[serde(default)]
    pub id: WorkspaceId,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub last_active: u64,
    #[serde(default)]
    pub tabs: Vec<Tab>,
    #[serde(default)]
    pub active_tab: Option<TabId>,
    /// Who is holding this workspace right now. Answered over the wire so a
    /// peer can see the workspace is spoken for, but stripped before the
    /// document is written: an attachment belongs to a live connection, and
    /// one read back at boot would name a holder that no longer exists.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attachment: Option<Attachment>,
    /// The workspace's pinned sidebar groups and which groups are folded.
    /// Kept here, beside the tabs that point into it, so every window onto
    /// the workspace draws the same groups in the same order.
    #[serde(default, skip_serializing_if = "WorkspaceGroups::is_empty")]
    pub groups: WorkspaceGroups,
    /// Tabs closed from this workspace that ⌘⇧T can still put back, oldest
    /// first (#1021).
    ///
    /// Kept here rather than in the window that closed them so the list
    /// survives the window, the app and a restart of either, and reads the
    /// same to every client of the workspace — a remote one included. Each
    /// entry's panes were stopped when it closed; what it keeps is their
    /// records in [`Machine::panes`] and, through those, their last screens on
    /// disk, which is everything a reopen restores from. Bounded by
    /// [`MAX_CLOSED_TABS`] and [`CLOSED_TAB_TTL`].
    ///
    /// Left out of the document while empty, so a tree that never closed a tab
    /// this way reads the same to a build that predates the list.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub closed: Vec<ClosedTab>,
    /// A local owning-daemon projection of a mixed remote workspace.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub foreign_view: Option<crate::core::pane_sidecar::ForeignWorkspace>,
}

/// A tab closed from a workspace, as the machine keeps it for reopening.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClosedTab {
    /// The tab as it stood when it closed: its id, name, group and layout. The
    /// pane ids in `root` are dead by the time anyone reads this; they are the
    /// keys a reopen asks the daemon to restore each pane's screen from.
    pub tab: Tab,
    /// When it closed, in seconds since the Unix epoch.
    #[serde(default)]
    pub closed_at: u64,
}

/// What [`super::MachineStore::tab_reopen`] hands back: the closed tab and the
/// records of the panes it held, which are what its layout is rebuilt from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReopenedTab {
    pub tab: Tab,
    #[serde(default)]
    pub closed_at: u64,
    #[serde(default)]
    pub panes: Vec<PaneRecord>,
}

/// What closing a tab into the recently-closed list leaves for the caller to
/// do to the processes — see [`super::MachineStore::tab_close_remembered`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Remembered {
    /// The tab's panes. Their processes are to be stopped with their screens
    /// kept, because the entry that now names them restores from those.
    pub stopped: Vec<u64>,
    /// Panes the closing client held that the tree never recorded in the tab.
    /// Nothing can bring them back, so they end the way a close always ended
    /// a pane: screen and all.
    pub strays: Vec<u64>,
    /// Panes of entries the list outgrew, whose screens nothing will ask for
    /// again.
    pub dropped: Vec<u64>,
}

impl Default for Workspace {
    fn default() -> Self {
        Workspace {
            id: WorkspaceId::new(),
            name: None,
            last_active: unix_now(),
            tabs: Vec::new(),
            active_tab: None,
            attachment: None,
            groups: WorkspaceGroups::default(),
            closed: Vec::new(),
            foreign_view: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PaneRecord {
    pub id: u64,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub title: String,
    /// The title the terminal itself reports (OSC 0/2): a shell writes
    /// `user@host:~/dir` from its integration, an agent overwrites it with what
    /// it is working on. Distinct from `title` above, which is the foreground
    /// process name.
    ///
    /// This is the name the window that owns the pane puts on its tab, so it is
    /// also what anyone else has to read to agree with that window — the
    /// switcher listing a workspace it does not own, `lyrel tab ls` across a
    /// socket. `None` until the pane emits one; a pane that resets its title
    /// clears it back.
    #[serde(default)]
    pub osc_title: Option<String>,
    #[serde(default)]
    pub ssh_spec: Option<Box<NativeSshSpec>>,
    #[serde(default)]
    #[serde(deserialize_with = "deserialize_optional_agent_facts")]
    pub agent: Option<AgentFacts>,
    /// What the pane is actually running, resolved: the spawn's override if it
    /// had one, otherwise the shell the config named at the time.
    ///
    /// Without it a pane rebuilt from this tree comes back on whatever the
    /// default shell is now, so a daemon restart silently turns a bash pane
    /// into a PowerShell one. The rest of the record describes where a pane is
    /// and what is running in it; this is the part that says what it *is*.
    #[serde(default)]
    pub shell: Option<ShellSpec>,
    #[serde(default)]
    pub live: bool,
}

impl PaneRecord {
    pub fn new(id: u64) -> PaneRecord {
        PaneRecord {
            id,
            cwd: None,
            title: String::new(),
            osc_title: None,
            ssh_spec: None,
            agent: None,
            shell: None,
            live: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentFacts {
    pub agent: CLIAgent,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub launch_argv: Option<Vec<String>>,
    #[serde(
        default,
        serialize_with = "crate::host::session_operations::serialize_durable_scope"
    )]
    pub session_scope: Option<crate::host::session_operations::SessionScope>,
    #[serde(default)]
    pub status: Option<crate::core::cli_agent::AgentStatus>,
}

fn deserialize_optional_agent_facts<'de, D>(deserializer: D) -> Result<Option<AgentFacts>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    let Some(value) = value else {
        return Ok(None);
    };
    if value
        .get("agent")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|name| {
            serde_json::from_value::<CLIAgent>(serde_json::Value::String(name.to_string())).is_err()
        })
    {
        return Ok(None);
    }
    serde_json::from_value(value)
        .map(Some)
        .map_err(serde::de::Error::custom)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PaneSeed {
    pub pane: u64,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub ssh_spec: Option<Box<NativeSshSpec>>,
    #[serde(default)]
    #[serde(deserialize_with = "deserialize_optional_agent_facts")]
    pub agent: Option<AgentFacts>,
    /// See [`PaneRecord::shell`]. Carried here too so a pane that reaches the
    /// tree as a seed — a split, a `lyrel` CLI call — names its shell from the
    /// start rather than only once the daemon has observed it.
    #[serde(default)]
    pub shell: Option<ShellSpec>,
}

impl PaneSeed {
    pub fn bare(pane: u64) -> PaneSeed {
        PaneSeed {
            pane,
            cwd: None,
            ssh_spec: None,
            agent: None,
            shell: None,
        }
    }

    /// The record `register_pane` mints for this seed. Public because the
    /// window that pushed the seed mirrors the machine's tree, and this
    /// record reaches it in nothing it is sent — a client is left out of the
    /// deltas its own ops raise — so it puts the same record in its mirror
    /// itself (#612).
    pub fn into_record(self, live: bool) -> PaneRecord {
        PaneRecord {
            id: self.pane,
            cwd: self.cwd,
            title: String::new(),
            osc_title: None,
            ssh_spec: self.ssh_spec.map(|s| Box::new(s.without_secrets())),
            agent: self.agent,
            shell: self.shell,
            live,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TabCloseReason {
    Explicit,
    Emptied,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LayoutDelta {
    WorkspaceCreated {
        workspace: Workspace,
    },
    WorkspaceRenamed {
        name: Option<String>,
    },
    WorkspaceDeleted,
    WorkspaceTouched {
        last_active: u64,
    },
    ActiveTabChanged {
        tab: TabId,
    },
    TabCreated {
        at: usize,
        tab: Tab,
    },
    TabClosed {
        tab: TabId,
        reason: TabCloseReason,
    },
    TabRenamed {
        tab: TabId,
        name: Option<String>,
    },
    TabMoved {
        tab: TabId,
        to: usize,
    },
    TabRegrouped {
        tab: TabId,
        group: Option<GroupId>,
        #[serde(default)]
        last_auto: Option<AutoKey>,
    },
    GroupsChanged {
        groups: WorkspaceGroups,
    },
    TabRestructured {
        tab: Tab,
        pane: Option<PaneRecord>,
    },
    RatioChanged {
        tab: TabId,
        path: Vec<Side>,
        ratio: f32,
    },
    PaneFacts {
        pane: PaneRecord,
    },
}

pub fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests;
