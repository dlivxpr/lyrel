use std::rc::Rc;

use gpui_kit::{App, AppContext as _, Context, Entity, EntityId, EventEmitter, Task, Window, px};

use super::{PendingPane, PendingSpawn};
use crate::core::session::WorkspaceId;
use crate::daemon::protocol::NativeSshSpec;
use crate::terminal::remote::handshake::{PaneHandshake, attach_refused};
use crate::terminal::view::{NativeSshParts, ShellParts, TerminalView};
use crate::terminal::{PaneRoute, SpawnCancellation, TermSize};
use crate::ui::tab::TabSet;

/// The workbench supplies only an immutable slot/workspace lease, never itself.
pub(super) type SlotLease = Rc<dyn Fn(EntityId, Option<WorkspaceId>, &App) -> bool>;

#[derive(Default)]
pub(super) struct Creation {
    native: Option<Box<NativeSshSpec>>,
    route: Option<PaneRoute>,
    lease: Option<SlotLease>,
    generation: u64,
    task: Option<Task<()>>,
    cancellation: Option<SpawnCancellation>,
    ready: Option<OwnedTerminal>,
}

impl Creation {
    pub(super) fn ssh_spec(&self) -> Option<Box<NativeSshSpec>> {
        self.native.as_ref().map(|spec| Box::new(spec.without_secrets()))
    }
}

impl Drop for Creation {
    fn drop(&mut self) {
        if let Some(cancellation) = &self.cancellation { cancellation.cancel(); }
    }
}

struct CancelOnDrop(SpawnCancellation);
impl Drop for CancelOnDrop {
    fn drop(&mut self) { self.0.cancel(); }
}

pub(crate) struct SpawnFinished;
impl EventEmitter<SpawnFinished> for PendingPane {}

pub(crate) enum PendingLaunch {
    None,
    Command(String),
    Resume {
        agent: Option<crate::core::cli_agent::CLIAgent>,
        session: Option<String>,
        argv: Option<Vec<String>>,
        scope: Option<lyrel_core::host::session_operations::SessionScope>,
    },
}

pub(crate) struct PaneInstallation {
    view: Entity<TerminalView>,
    focused: bool,
    launch: PendingLaunch,
}

impl PaneInstallation {
    pub(crate) fn into_parts(self) -> (Entity<TerminalView>, bool, PendingLaunch) {
        (self.view, self.focused, self.launch)
    }
}

enum CreatedTerminal {
    Shell(ShellParts),
    Native(NativeSshParts),
}

struct OwnedTerminal {
    parts: Option<CreatedTerminal>,
    executor: gpui_kit::BackgroundExecutor,
}

impl Drop for OwnedTerminal {
    fn drop(&mut self) {
        let Some(parts) = self.parts.take() else { return; };
        // Claimable local shells are reclaimed by EOF. Existing attachments
        // only detach. Remote and native fresh children require an explicit kill.
        let kill = match &parts {
            CreatedTerminal::Shell(parts) if !parts.restored => {
                let route = PaneRoute::for_workspace(parts.workspace.as_ref());
                (!route.is_local()).then_some((route, parts.pane_id))
            }
            CreatedTerminal::Native(parts) => Some((PaneRoute::Local, parts.pane_id())),
            _ => None,
        };
        self.executor.spawn(async move {
            if let Some((route, pane)) = kill {
                crate::terminal::RemoteTerminal::kill_pane_on(&route, pane);
            }
            drop(parts);
        }).detach();
    }
}

impl PendingSpawn {
    fn create(self, route: &PaneRoute, attach_only: bool, native: Option<Box<NativeSshSpec>>, cancellation: &SpawnCancellation) -> anyhow::Result<CreatedTerminal> {
        if cancellation.cancelled() { anyhow::bail!("pane creation cancelled"); }
        if let Some(spec) = native {
            let persist = Box::new(spec.without_secrets());
            let (terminal, pane_id) = PaneHandshake::spawn_native_ssh(
                TermSize::new(80, 24), 8, 17, self.working_directory, spec,
            )?;
            return Ok(CreatedTerminal::Native(NativeSshParts::new(terminal, pane_id, persist)));
        }
        let attached = match self.restore_pane {
            Some(pane) => match PaneHandshake::attach_on(route, TermSize::new(80, 24), 8, 17, pane) {
                Ok(terminal) => Some((terminal, pane, None)),
                Err(error) if !attach_only && attach_refused(&error) => None,
                Err(error) => return Err(error.context(format!("could not reach pane {pane}, which may still be running"))),
            },
            None if attach_only => anyhow::bail!("an attach-only sidecar has no owning pane"),
            None => None,
        };
        let restored = attached.is_some();
        let (terminal, pane_id, shell_spec) = match attached {
            Some(parts) => parts,
            None => {
                if cancellation.cancelled() { anyhow::bail!("pane creation cancelled"); }
                // Launch preparation and replay remain daemon-owned. This is
                // only the existing restore request, not another replay path.
                let restore = self.restore_pane.map(|pane_id| crate::daemon::protocol::RestoreFrom {
                    pane_id,
                    banner: Some(crate::ui::i18n::t(crate::ui::i18n::L10nKey::PaneRestoredScreenBanner).to_string()),
                });
                let (terminal, pane) = PaneHandshake::spawn_on(
                    &route, TermSize::new(80, 24), 8, 17, self.working_directory,
                    self.shell.clone(), self.owner.map(|owner| owner.to_string()), restore, Some(cancellation),
                )?;
                (terminal, pane, self.shell)
            }
        };
        Ok(CreatedTerminal::Shell(ShellParts::new(
            terminal, pane_id, shell_spec, self.workspace, restored, self.owner,
        )))
    }
}

impl PendingPane {
    pub(super) fn native(mut self, spec: Box<NativeSshSpec>) -> Self {
        self.creation.native = Some(spec);
        self
    }

    pub(crate) fn start(&mut self, lease: SlotLease, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(cancellation) = self.creation.cancellation.take() { cancellation.cancel(); }
        self.creation.task.take();
        self.creation.ready.take();
        self.creation.generation += 1;
        self.creation.lease = Some(lease.clone());
        self.retrying(cx);
        let generation = self.creation.generation;
        let slot = cx.entity_id();
        let spawn = self.spawn.clone();
        let owner = spawn.owner;
        let route = self.creation.route.get_or_insert_with(|| PaneRoute::for_workspace(spawn.workspace.as_ref())).clone();
        let native = self.creation.native.clone();
        let attach_only = self.attach_only;
        let cancellation = SpawnCancellation::default();
        self.creation.cancellation = Some(cancellation.clone());
        let cancel_on_exit = CancelOnDrop(cancellation.clone());
        let executor = cx.background_executor().clone();
        let (sender, receiver) = smol::channel::bounded(1);
        cx.background_executor().spawn(async move {
            let result = spawn.create(&route, attach_only, native, &cancellation)
                .map(|parts| OwnedTerminal { parts: Some(parts), executor })
                .map_err(|error| format!("{error:#}"));
            let _ = sender.send(result).await;
        }).detach();
        self.creation.task = Some(cx.spawn_in(window, async move |this, cx| {
            let _cancel_on_exit = cancel_on_exit;
            loop {
                let live = this.update_in(cx, |_, _, cx| lease(slot, owner, cx));
                if !matches!(live, Ok(true)) { return; }
                match receiver.try_recv() {
                    Ok(result) => {
                        let _ = this.update_in(cx, |pending, window, cx| pending.receive(generation, result, window, cx));
                        return;
                    }
                    Err(smol::channel::TryRecvError::Closed) => return,
                    Err(smol::channel::TryRecvError::Empty) => {}
                }
                cx.background_executor().timer(std::time::Duration::from_millis(50)).await;
            }
        }));
    }

    fn receive(&mut self, generation: u64, result: Result<OwnedTerminal, String>, window: &mut Window, cx: &mut Context<Self>) {
        if generation != self.creation.generation { return; }
        match result {
            Ok(parts) => {
                self.creation.ready = Some(parts);
                cx.emit(SpawnFinished);
            }
            Err(reason) => {
                self.fail(reason, cx);
                if let Some(delay) = self.next_auto_retry() {
                    let lease = self.creation.lease.clone().expect("a running request has a slot lease");
                    let slot = cx.entity_id();
                    let owner = self.spawn.owner;
                    cx.spawn_in(window, async move |this, cx| {
                        cx.background_executor().timer(delay).await;
                        let _ = this.update_in(cx, |pending, window, cx| {
                            if pending.creation.generation == generation && pending.is_failed() && lease(slot, owner, cx) {
                                pending.start(lease.clone(), window, cx);
                            }
                        });
                    }).detach();
                }
            }
        }
    }

    /// Validate the retained slot and original workspace before claiming a child.
    /// No current-focus fallback is permitted, including on a failed attempt.
    pub(crate) fn take_installation(&mut self, tabs: &TabSet, workspace: WorkspaceId, window: &mut Window, cx: &mut Context<Self>) -> Option<PaneInstallation> {
        let mut ready = self.creation.ready.take()?;
        if self.spawn.owner.is_some_and(|owner| owner != workspace) || !tabs.contains_slot(cx.entity_id()) {
            return None;
        }
        let focused = self.focus_handle.contains_focused(window, cx);
        let parts = ready.parts.take()?;
        let restored = matches!(&parts, CreatedTerminal::Shell(parts) if parts.restored);
        let attach_spec = self.attach_spec.clone();
        let font_size = self.spawn.font_size;
        let view = cx.new(|cx| {
            let mut view = match parts {
                CreatedTerminal::Shell(parts) => TerminalView::from_shell_parts(parts, window, cx),
                CreatedTerminal::Native(parts) => TerminalView::from_native_ssh_parts(parts, window, cx),
            };
            view.font_size = px(font_size);
            if self.attach_only { view.set_restored_ssh_spec(attach_spec); }
            view
        });
        let launch = if restored { PendingLaunch::None }
            else if let Some(command) = self.spawn.run_on_land.clone() { PendingLaunch::Command(command) }
            else { PendingLaunch::Resume {
                agent: self.spawn.agent, session: self.spawn.agent_session_id.clone(),
                argv: self.spawn.agent_launch_argv.clone(), scope: self.spawn.agent_session_scope.clone(),
            }};
        Some(PaneInstallation { view, focused, launch })
    }
}

#[cfg(test)]
mod consumer_tests;
