//! Assemble discovery rows from bounded workbench facts, not application access.

use gpui_kit::App;

use super::command::{Avatar, ChromeState, CommandGroup, CommandKind, Item};
use super::history::SessionHistory;
use super::{Catalog, FileList};
use crate::core::cli_agent::CLIAgent;
use crate::core::config::Config;
use crate::core::session::{WorkspaceId, WorkspaceStore};
use crate::core::shells::ShellInventory;
use crate::ui::i18n::{L10nKey, t, t_fmt};
use crate::ui::switcher::TabRow;

pub(crate) struct ActionCatalog {
    chrome: ChromeState,
    editor_navigation: Vec<Item>,
    editor_dispatch: bool,
    remote: bool,
    save_ssh: bool,
}

impl ActionCatalog {
    pub(crate) fn new(chrome: ChromeState, remote: bool, save_ssh: bool) -> Self {
        Self {
            chrome,
            editor_navigation: Vec::new(),
            editor_dispatch: false,
            remote,
            save_ssh,
        }
    }

    pub(crate) fn with_editor(mut self, navigation: Vec<Item>, dispatch: bool) -> Self {
        self.editor_navigation = navigation;
        self.editor_dispatch = dispatch;
        self
    }

    fn items(self) -> Vec<Item> {
        let mut actions = Item::actions(self.chrome);

        // The editor's navigation, while there is a file in front of it.
        actions.extend(self.editor_navigation);

        // Groups are something the sidebar draws, so they are offered only
        // while the tabs are in it. Opening a folder as a group asks the
        // system picker, which browses this computer: a path picked there
        // names nothing on a remote workspace's machine, where "Pin as
        // Group" in the file tree is the way in instead.
        actions.push(
            Item::localized(L10nKey::CmdNewGroup, CommandKind::NewGroup)
                .with_subtitle(t(L10nKey::CmdNewGroupSubtitle))
                .in_group(CommandGroup::TabsPanes),
        );
        if !self.remote {
            actions.push(
                Item::localized(
                    L10nKey::CmdOpenFolderAsGroup,
                    CommandKind::OpenFolderAsGroup,
                )
                .with_subtitle(t(L10nKey::CmdOpenFolderAsGroupSubtitle))
                .in_group(CommandGroup::TabsPanes),
            );
        }

        // The editor's text commands, while there is a buffer to run them on.
        if self.editor_dispatch {
            actions.extend(
                [
                    Item::localized(
                        L10nKey::CmdEditorTransformUppercase,
                        CommandKind::EditorTransformUppercase,
                    ),
                    Item::localized(
                        L10nKey::CmdEditorTransformLowercase,
                        CommandKind::EditorTransformLowercase,
                    ),
                    Item::localized(
                        L10nKey::CmdEditorTransformTitleCase,
                        CommandKind::EditorTransformTitleCase,
                    ),
                    Item::localized(
                        L10nKey::CmdEditorTrimTrailingWhitespace,
                        CommandKind::EditorTrimTrailingWhitespace,
                    ),
                    Item::localized(L10nKey::CmdEditorJoinLines, CommandKind::EditorJoinLines),
                    Item::localized(
                        L10nKey::CmdEditorRemoveSurroundingBrackets,
                        CommandKind::EditorRemoveSurroundingBrackets,
                    ),
                ]
                .into_iter()
                .map(|item| item.in_group(CommandGroup::View)),
            );
        }

        // Offered only where it would do something. A connection opened from a
        // saved host has nothing to save, and a pane that is not an SSH one has
        // no connection at all — either would be a row that quietly did nothing
        // (#549).
        if self.save_ssh {
            actions.push(
                Item::localized(
                    L10nKey::CmdSshSaveConnection,
                    CommandKind::SaveSshSessionAsHost,
                )
                .with_subtitle(t(L10nKey::CmdSshSaveConnectionSubtitle))
                .in_group(CommandGroup::Ssh),
            );
        }
        actions
    }
}

pub(crate) struct TerminalCatalog<'a> {
    workspace: WorkspaceId,
    shells: &'a ShellInventory,
    agents: &'a [CLIAgent],
}

impl<'a> TerminalCatalog<'a> {
    pub(crate) fn new(
        workspace: WorkspaceId,
        shells: &'a ShellInventory,
        agents: &'a [CLIAgent],
    ) -> Self {
        Self {
            workspace,
            shells,
            agents,
        }
    }

    pub(crate) fn items(
        self,
        mut rows_for: impl FnMut(WorkspaceId) -> Vec<TabRow>,
        cx: &App,
    ) -> Vec<Item> {
        let mut out = Vec::new();
        let store = WorkspaceStore::all(cx);
        let name_of = |id: WorkspaceId| {
            store
                .get(id)
                .and_then(|w| crate::ui::machine_mirror::display_name(cx, w))
                .unwrap_or_else(|| t(L10nKey::WindowUntitled).to_string())
        };
        // This window's own workspace leads, and is listed whether or not the
        // store has caught up with it: its tabs are right here in the window.
        let mut others: Vec<_> = store
            .views
            .iter()
            .filter(|w| w.id != self.workspace)
            .collect();
        others.sort_by_key(|w| std::cmp::Reverse(w.last_active));
        let workspaces = std::iter::once(self.workspace).chain(others.into_iter().map(|w| w.id));
        for id in workspaces {
            let name = name_of(id);
            let here = id == self.workspace;
            for tab in rows_for(id) {
                if here && tab.active {
                    continue;
                }
                let mut item = Item::new(
                    tab.label.clone(),
                    CommandKind::GoToTab {
                        workspace: id,
                        tab: tab.id,
                    },
                )
                .with_avatar(Avatar {
                    agent: tab.agent,
                    status: tab.status,
                    unread: tab.unread,
                    ssh: tab.ssh,
                })
                .with_alias(name.clone())
                .in_section(name.clone());
                // A label derived from the path already says where; one that
                // is a name or an agent leaves the place worth printing.
                if tab.named && !tab.path.is_empty() {
                    item = item.with_subtitle(tab.path.clone());
                } else if !tab.path.is_empty() {
                    item = item.with_alias(tab.path.clone());
                }
                if let Some(agent) = tab.agent {
                    item = item.with_alias(agent.display_name());
                }
                if !here {
                    item = item.with_note(name.clone());
                }
                out.push(item);
            }
        }

        let new_terminal: gpui_kit::SharedString = t(L10nKey::SearchSectionNewTerminal).into();
        // Every shell the window's machine has, named alike in every language
        // so the New Tab menu's "Other Shells…" row can land on exactly these
        // by typing one word. Listed in the order that menu uses.
        let default_shell = self.shells.default_name.clone();
        let usage = &cx.global::<Config>().shell_frecency;
        let now = crate::core::config::unix_now();
        for s in crate::ui::tab_strip::shells_by_frecency(
            &self.shells.shells,
            &default_shell,
            usage,
            now,
        ) {
            let mut item = Item::new(
                t_fmt(L10nKey::AppCmdShellTitle, &[("title", &s.label)]),
                CommandKind::OpenShell(s.label.clone()),
            )
            .in_section(new_terminal.clone());
            if s.label == default_shell {
                item = item.with_subtitle(t(L10nKey::ShellDefault));
            }
            out.push(item);
        }
        for agent in self.agents.iter().copied() {
            out.push(
                Item::new(
                    t_fmt(
                        L10nKey::AppCmdAgentLaunchTitle,
                        &[("name", agent.display_name())],
                    ),
                    CommandKind::LaunchAgent(agent),
                )
                .with_subtitle(agent.launch_command(&cx.global::<Config>().agent_launch))
                .with_avatar(Avatar {
                    agent: Some(agent),
                    ..Avatar::default()
                })
                .in_section(new_terminal.clone()),
            );
        }
        out
    }
}

pub(crate) struct SearchCatalog {
    actions: ActionCatalog,
    terminals: Vec<Item>,
    history: SessionHistory,
    query: Option<(
        crate::ui::host_ops::SharedHost,
        Vec<gpui_kit::Entity<crate::terminal::view::TerminalView>>,
        Vec<CLIAgent>,
    )>,
    files: FileList,
}

impl SearchCatalog {
    pub(crate) fn new(
        actions: ActionCatalog,
        terminals: Vec<Item>,
        history: SessionHistory,
        files: FileList,
    ) -> Self {
        Self {
            actions,
            terminals,
            history,
            files,
            query: None,
        }
    }

    pub(crate) fn query_sessions(
        mut self,
        host: Option<crate::ui::host_ops::SharedHost>,
        panes: Vec<gpui_kit::Entity<crate::terminal::view::TerminalView>>,
        agents: Vec<CLIAgent>,
    ) -> Self {
        self.query = host.map(|host| (host, panes, agents));
        self
    }

    pub(super) fn prepare(self, cx: &App) -> (Catalog, Option<super::history::SessionQuery>) {
        let mut catalog = Catalog::new(
            self.actions.items(),
            self.terminals,
            super::sources::host_items(cx),
        );
        let (sessions, here) = self.history.cached_items(cx);
        catalog.sessions = sessions;
        catalog.sessions_here = here;
        catalog.files = self.files;
        let query = self
            .query
            .map(|(host, panes, agents)| self.history.query(host, panes, agents));
        (catalog, query)
    }

    #[cfg(test)]
    pub(crate) fn build(self, cx: &App) -> Catalog {
        self.prepare(cx).0
    }
}
