//! Connection identity and the single attachment/GUI handover authority.
use std::io;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::core::machine::{self, Attachment, MachineStore};
use crate::daemon::control::{ControlEvent, ControlHello, ControlServerMsg, LinkShutdown, WorkspaceId};

use super::output::Sink;

#[derive(Default)]
pub struct AttachRegistry {
    live: Mutex<Vec<Live>>,
    guis: Mutex<Vec<GuiLive>>,
    handover: Mutex<()>,
}

struct Live {
    workspace: String,
    conn: u64,
    token: String,
    hostname: String,
    sink: Arc<Sink>,
    shutdown: Arc<dyn LinkShutdown>,
    dedicated: bool,
}

struct Evicted {
    hostname: String,
    sink: Arc<Sink>,
    shutdown: Arc<dyn LinkShutdown>,
    dedicated: bool,
}

struct GuiLive {
    conn: u64,
    sink: Arc<Sink>,
}

impl AttachRegistry {
    fn handover(&self) -> std::sync::MutexGuard<'_, ()> {
        self.handover.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn holder(&self, workspace: &str) -> Option<(String, String)> {
        self.locked()
            .iter()
            .find(|l| l.workspace == workspace)
            .map(|l| (l.token.clone(), l.hostname.clone()))
    }

    pub fn len(&self) -> usize {
        self.locked().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn register_gui(&self, conn: u64, sink: Arc<Sink>) {
        let mut guis = self.guis.lock().unwrap_or_else(|e| e.into_inner());
        guis.retain(|gui| gui.conn != conn);
        guis.push(GuiLive { conn, sink });
    }

    fn unregister_gui(&self, conn: u64) {
        self.guis
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|gui| gui.conn != conn);
    }

    fn open_gui(&self, path: Option<String>, workspace: Option<WorkspaceId>) -> bool {
        // The newest GUI connection belongs to the most recently started app
        // process. Window recency is resolved inside that process, where GPUI
        // owns the authoritative focus state.
        loop {
            let target = self
                .guis
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .last()
                .map(|gui| (gui.conn, Arc::clone(&gui.sink)));
            let Some((conn, sink)) = target else {
                return false;
            };
            let event = ControlServerMsg::Event(ControlEvent::GuiOpen {
                path: path.clone(),
                workspace,
            });
            if sink.send(&event).is_ok() {
                return true;
            }
            self.unregister_gui(conn);
        }
    }

    fn claim(
        &self,
        workspace: &str,
        conn: u64,
        holder: &Holder,
        dedicated: bool,
    ) -> Option<Evicted> {
        let mut live = self.locked();
        let evicted = match live.iter().position(|l| l.workspace == workspace) {
            Some(i) if live[i].conn == conn => {
                live[i].token = holder.token.clone();
                None
            }
            Some(i) => {
                let old = live.remove(i);
                Some(Evicted {
                    hostname: old.hostname,
                    sink: old.sink,
                    shutdown: old.shutdown,
                    dedicated: old.dedicated,
                })
            }
            None => None,
        };
        if !live.iter().any(|l| l.workspace == workspace) {
            live.push(Live {
                workspace: workspace.to_string(),
                conn,
                token: holder.token.clone(),
                hostname: holder.hostname.clone(),
                sink: Arc::clone(&holder.sink),
                shutdown: Arc::clone(&holder.shutdown),
                dedicated,
            });
        }
        evicted
    }

    fn forget_workspace(&self, workspace: &str) {
        self.locked().retain(|l| l.workspace != workspace);
    }

    fn release(&self, workspace: &str, conn: u64) -> bool {
        let mut live = self.locked();
        let before = live.len();
        live.retain(|l| !(l.workspace == workspace && l.conn == conn));
        live.len() != before
    }

    fn release_conn(&self, conn: u64) -> Vec<String> {
        let mut live = self.locked();
        let mut released = Vec::new();
        live.retain(|l| {
            if l.conn == conn {
                released.push(l.workspace.clone());
                false
            } else {
                true
            }
        });
        released
    }

    fn locked(&self) -> std::sync::MutexGuard<'_, Vec<Live>> {
        self.live.lock().unwrap_or_else(|e| e.into_inner())
    }
}

struct Holder {
    token: String,
    hostname: String,
    sink: Arc<Sink>,
    shutdown: Arc<dyn LinkShutdown>,
}

static NEXT_CONN: AtomicU64 = AtomicU64::new(1);

/// One negotiated peer identity, shared by handshake attachment and requests.
/// No request can mutate either attachment table outside the handover policy.
pub(super) struct ConnectionIdentity {
    id: u64,
    holder: Holder,
    registry: Arc<AttachRegistry>,
}

impl ConnectionIdentity {
    pub(super) fn new(
        hello: &ControlHello,
        sink: &Arc<Sink>,
        shutdown: &Arc<dyn LinkShutdown>,
        registry: Arc<AttachRegistry>,
    ) -> Self {
        Self {
            id: NEXT_CONN.fetch_add(1, Ordering::Relaxed),
            holder: Holder {
                token: hello.client_token.clone(),
                hostname: hello.client_hostname.clone(),
                sink: Arc::clone(sink),
                shutdown: Arc::clone(shutdown),
            },
            registry,
        }
    }

    pub(super) fn register_gui(&self) {
        self.registry.register_gui(self.id, Arc::clone(&self.holder.sink));
    }

    pub(super) fn unregister_gui(&self) {
        self.registry.unregister_gui(self.id);
    }

    pub(super) fn open_gui(&self, path: Option<String>, workspace: Option<WorkspaceId>) -> bool {
        self.registry.open_gui(path, workspace)
    }

    pub(super) fn attach(
        &self,
        machine: Option<&MachineStore>,
        workspace: &str,
        dedicated: bool,
    ) -> io::Result<Option<String>> {
        if machine.is_none() {
            return Err(io::Error::other(
                "this server does not serve the machine tree",
            ));
        }
        let tree_id: Option<crate::core::session::WorkspaceId> = workspace.parse().ok();
        let (displaced, evicted) = {
            let _handover = self.registry.handover();
            let attachment = Attachment::new(self.holder.token.clone(), self.holder.hostname.clone());
            let displaced = match (machine, tree_id) {
                (Some(machine), Some(id)) => machine.attach(id, attachment),
                _ => None,
            };
            let evicted = self
                .registry
                .claim(workspace, self.id, &self.holder, dedicated);
            (displaced, evicted)
        };

        if let Some(evicted) = evicted {
            log::info!(
                "workspace {workspace} taken over by {} from {}",
                self.holder.hostname,
                evicted.hostname
            );
            let notice = ControlEvent::Preempted {
                workspace: workspace.to_string(),
                by: self.holder.hostname.clone(),
            };
            if let Err(e) = evicted.sink.send(&ControlServerMsg::Event(notice)) {
                log::debug!("could not tell the displaced session about {workspace}: {e}");
            }
            if evicted.dedicated {
                let _ = evicted.shutdown.shutdown_link();
            }
        }

        Ok(displaced
            .filter(|a| a.token != self.holder.token)
            .map(|a| a.hostname))
    }

    pub(super) fn detach(
        &self,
        machine: Option<&MachineStore>,
        workspace: &str,
    ) -> io::Result<bool> {
        if machine.is_none() {
            return Err(io::Error::other(
                "this server does not serve the machine tree",
            ));
        }
        let _handover = self.registry.handover();
        let released = self.registry.release(workspace, self.id);
        let forgotten = match (machine, workspace.parse().ok()) {
            (Some(machine), Some(id)) => machine.detach(id, &self.holder.token),
            _ => false,
        };
        Ok(released || forgotten)
    }

    pub(super) fn remove_workspace(
        &self,
        machine: &MachineStore,
        workspace: WorkspaceId,
        origin: Option<machine::SubscriberId>,
    ) -> io::Result<Vec<u64>> {
        let _handover = self.registry.handover();
        let panes = machine.workspace_delete(workspace, origin)?;
        self.registry.forget_workspace(&workspace.to_string());
        Ok(panes)
    }

    pub(super) fn release_all(&self, machine: Option<&MachineStore>) {
        let _handover = self.registry.handover();
        let released = self.registry.release_conn(self.id);
        for workspace in released {
            if let (Some(machine), Some(id)) = (machine, workspace.parse().ok()) {
                machine.detach(id, &self.holder.token);
            }
        }
    }
}

#[cfg(test)]
mod gui_registry_tests {
    use super::*;
    use std::io::{Cursor, Write};

    struct SharedWriter(Arc<Mutex<Vec<u8>>>);

    impl Write for SharedWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn gui_open_is_delivered_only_while_a_gui_is_registered() {
        let registry = AttachRegistry::default();
        assert!(!registry.open_gui(Some("/work".into()), None));

        let bytes = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::new(Sink::new(SharedWriter(Arc::clone(&bytes))));
        registry.register_gui(7, sink);
        assert!(registry.open_gui(Some("/work".into()), None));

        let frame = bytes.lock().unwrap().clone();
        assert_eq!(
            ControlServerMsg::read(&mut Cursor::new(frame)).unwrap(),
            ControlServerMsg::Event(ControlEvent::GuiOpen {
                path: Some("/work".into()),
                workspace: None,
            })
        );

        registry.unregister_gui(7);
        assert!(!registry.open_gui(None, None));
    }

    #[test]
    fn gui_open_carries_the_workspace_the_caller_named() {
        let registry = AttachRegistry::default();
        let bytes = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::new(Sink::new(SharedWriter(Arc::clone(&bytes))));
        registry.register_gui(7, sink);

        let made = WorkspaceId::new();
        assert!(registry.open_gui(None, Some(made)));

        let frame = bytes.lock().unwrap().clone();
        assert_eq!(
            ControlServerMsg::read(&mut Cursor::new(frame)).unwrap(),
            ControlServerMsg::Event(ControlEvent::GuiOpen {
                path: None,
                workspace: Some(made),
            }),
            "the GUI has to be told which workspace, not left to guess a window"
        );
    }
}

#[cfg(all(test, unix))]
mod tests;
