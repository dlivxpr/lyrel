//! GUI session setup and grant routing; no offer wait owns a pane lock.
use super::*;
use crate::daemon::pane::{GraphicsAdmissionBroker, OutputGate};
use std::sync::mpsc::{Sender, sync_channel};
use std::thread::JoinHandle;

enum Attached {
    Controller(u64),
    Observer(u64),
}
struct Session {
    pane: Arc<DaemonPane>,
    broker: Arc<GraphicsAdmissionBroker>,
    endpoint: u64,
    sender: Option<Arc<Sender<DaemonMsg>>>,
    messages: Option<Receiver<ClientMsg>>,
    shutdown: Stream,
    writer: Option<JoinHandle<()>>,
    reader: Option<JoinHandle<()>>,
    attached: Option<Attached>,
}
impl Drop for Session {
    fn drop(&mut self) {
        self.broker.unregister(self.endpoint);
        self.pane.remove_graphics_viewport(self.endpoint);
        match self.attached.take() {
            Some(Attached::Controller(epoch)) => {
                self.pane.detach(epoch);
            }
            Some(Attached::Observer(id)) => self.pane.unobserve(id),
            None => {}
        }
        self.sender = None;
        self.messages = None;
        let _ = self.shutdown.shutdown(std::net::Shutdown::Read);
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
        if let Some(writer) = self.writer.take() {
            let _ = writer.join();
        }
    }
}

pub(super) fn run(
    pane: Arc<DaemonPane>,
    mut read_stream: Stream,
    write_stream: Stream,
    registry: Arc<Registry>,
    endpoint: u64,
    observer: bool,
    allow_clipboard: bool,
    mut ready: Option<DaemonMsg>,
    mut claim: Option<UnclaimedSpawn>,
) -> anyhow::Result<()> {
    read_stream.set_read_timeout(Some(INPUT_RETIRE_WAIT))?;
    let broker = pane.graphics_admission_broker();
    let (tx, output) = mpsc::channel();
    let sender = Arc::new(tx);
    broker.register(endpoint, &sender);
    let gate = if observer {
        Arc::new(OutputGate::new())
    } else {
        pane.gate()
    };
    let shutdown = read_stream.try_clone()?;
    let (input, messages) = sync_channel(64);
    let input_broker = broker.clone();
    let input_pane = pane.clone();
    let input_sender = Arc::downgrade(&sender);
    let reader = std::thread::Builder::new().name("lyrel-graphics-grants".into()).spawn(move || {
        let mut pending = Vec::new();
        while let Ok(Some(message)) = read_active_input(&mut read_stream, &mut pending, || input_sender.strong_count() != 0) {
            match message {
                ClientMsg::GraphicsGrant { token, accepted } => input_broker.grant(endpoint, token, accepted),
                // Apply this connection's viewport before its following
                // pressure request, not behind the ordinary-input queue.
                ClientMsg::GraphicsViewport(viewport) => input_pane.graphics_viewport(endpoint, &input_sender, viewport),
                ClientMsg::GraphicsPressure { key } => {
                    let reclaimed = input_pane.pressure_graphics(key);
                    if let Some(sender) = input_sender.upgrade() {
                        let _ = sender.send(DaemonMsg::GraphicsPressureResult { key, reclaimed });
                    }
                }
                ClientMsg::Detach => {
                    input_broker.unregister(endpoint);
                    let _ = input.try_send(ClientMsg::Detach);
                    break;
                }
                message => {
                    // Never park Grant/Viewport/Pressure behind keyboard traffic. The
                    // bounded queue closes with a truthful error on overload.
                    if matches!(&message, ClientMsg::Input(bytes) if bytes.len() > (1 << 20)) || input.try_send(message).is_err() {
                        if let Some(sender) = input_sender.upgrade() { let _ = sender.send(DaemonMsg::Error("Pane input backlog exhausted during graphics admission; reconnect".into())); }
                        break;
                    }
                }
            }
        }
        input_broker.unregister(endpoint);
    })?;
    let writer = spawn_writer(output, write_stream, gate.clone());
    let mut session = Session {
        pane: pane.clone(),
        broker,
        endpoint,
        sender: Some(sender),
        messages: Some(messages),
        shutdown,
        writer: Some(writer),
        reader: Some(reader),
        attached: None,
    };
    if claim.is_some() && pane.alive() {
        registry.insert(pane.clone());
    }
    // A pending desktop must not stop the PTY. If retained identities change
    // during its offers, release that complete attempt before taking a fresh
    // snapshot. Ordinary text/resize does not invalidate an applicable lease.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    let admitted = (|| -> Result<Attached, String> {
        loop {
            let attached = if let Some(admission) = pane.prepare_graphics_replay(endpoint)? {
                if let Some(ready) = ready.take() {
                    let _ = session.sender.as_ref().unwrap().send(ready);
                }
                if observer {
                    pane.observe_admitted(
                        session.sender.as_ref().unwrap().clone(),
                        gate.clone(),
                        admission,
                    )?
                    .map(Attached::Observer)
                } else {
                    pane.attach_admitted(
                        session.sender.as_ref().unwrap().clone(),
                        allow_clipboard,
                        admission,
                    )?
                    .map(Attached::Controller)
                }
            } else {
                None
            };
            if let Some(attached) = attached {
                return Ok(attached);
            }
            if std::time::Instant::now() >= deadline {
                return Err(
                    "Graphics session kept changing during replay admission; reconnect".into(),
                );
            }
        }
    })();
    let attached = match admitted {
        Ok(attached) => attached,
        Err(error) => {
            let _ = session
                .sender
                .as_ref()
                .unwrap()
                .send(DaemonMsg::Error(error));
            return Ok(());
        }
    };
    if let Some(claim) = &mut claim {
        claim.claimed = true;
    }
    session.attached = Some(attached);
    // The pane is now the sender's only strong owner. Takeover/observer expiry
    // closes the writer naturally; broker records are weak and cannot pin it.
    session.sender = None;
    while let Ok(message) = session.messages.as_ref().unwrap().recv() {
        match session.attached.as_ref().unwrap() {
            Attached::Observer(id) => match message {
                ClientMsg::Detach => break,
                ClientMsg::Input(_)
                | ClientMsg::Resize(_)
                | ClientMsg::ClearPresentation { .. } => {
                    if !pane.tell_observer(
                        *id,
                        DaemonMsg::Error(
                            "this connection is a read-only observer; attach to write".into(),
                        ),
                    ) {
                        break;
                    }
                }
                _ => {}
            },
            Attached::Controller(epoch) => {
                if !pane.controls(*epoch) {
                    break;
                }
                match message {
                    ClientMsg::Input(bytes) => pane.write_input(&bytes),
                    ClientMsg::Resize(size) => pane.resize(size),
                    ClientMsg::ClearPresentation { history } => pane.clear_presentation(history),
                    ClientMsg::AuthResponse {
                        request_id,
                        response,
                    } => pane.deliver_auth_response(request_id, response),
                    ClientMsg::Detach => break,
                    ClientMsg::Kill { pane_id } => {
                        kill_pane(&registry, pane_id);
                        if pane_id == pane.id {
                            break;
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    let reclaimable = match session.attached.as_ref() {
        Some(Attached::Controller(epoch)) => pane.controls(*epoch) && !pane.alive(),
        _ => false,
    };
    drop(session);
    if reclaimable {
        registry.remove(pane.id);
    }
    Ok(())
}
