//! The route dialect: the header a client sends to have its connection bridged
//! to a remote endpoint, the ack that answers it, and the prompts a route can
//! raise while it is being established. Pure wire — the routing itself (SSH,
//! WSL, stdio bridges, installer policies) lives in `lyrel-core`, which
//! re-exports these types so existing `lyrel_core::daemon::router::*` paths
//! keep working.
//!
//! One frame kind prefixes every routed connection: the client writes a
//! `ROUTE_KIND` frame carrying a JSON `RouteHeader`; the server answers with a
//! `ROUTE_KIND` frame carrying a `RouteAck`. While the route is being
//! established the server may interleave `ROUTE_PROMPT_KIND` frames (auth
//! questions, install approvals, progress); the client answers each with a
//! `ROUTE_REPLY_KIND` frame, or leaves it unanswered when it has no stake in
//! it.

use std::io::{self, Read, Write};
use std::sync::{Arc, Mutex, OnceLock};

use serde::{Deserialize, Serialize};

use crate::daemon::protocol::{self, AuthPromptKind, AuthResponse, DaemonMsg};

pub const ROUTE_KIND: u8 = 51;

pub const ROUTE_PROMPT_KIND: u8 = 52;

pub const ROUTE_REPLY_KIND: u8 = 53;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RouteChannel {
    #[default]
    Control,
    Pane,
}

impl RouteChannel {
    /// The remote command a bridge of this channel must run: a pane route
    /// speaks the pane dialect, so the far end adds `--pane`.
    pub fn bridge_command(self, base: &str) -> String {
        match self {
            RouteChannel::Control => base.to_string(),
            RouteChannel::Pane => format!("{base} --pane"),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum RouteTarget {
    Ssh(Box<super::protocol::NativeSshSpec>),
    Wsl { distro: String },
    LocalStdio { program: String, args: Vec<String> },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RouteAction {
    #[default]
    Forward,
    RestartServer,
    ReplaceServer,
    /// [`RouteAction::ReplaceServer`] that uploads even over a server already
    /// speaking our dialect. A daemon that predates it fails to decode the
    /// header, so a client asks for it only where the local daemon advertises
    /// [`super::protocol::FEATURE_UPDATE_SERVER`].
    UpdateServer,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RouteHeader {
    pub target: RouteTarget,
    #[serde(default)]
    pub server_command: Option<String>,
    #[serde(default)]
    pub channel: RouteChannel,
    #[serde(default)]
    pub action: RouteAction,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RouteAck {
    pub ok: bool,
    #[serde(default)]
    pub link: Option<String>,
    #[serde(default)]
    pub action: Option<RouteAction>,
    #[serde(default)]
    pub error: Option<String>,
}

impl RouteHeader {
    pub fn ssh(spec: super::protocol::NativeSshSpec) -> RouteHeader {
        RouteHeader {
            target: RouteTarget::Ssh(Box::new(spec)),
            server_command: None,
            channel: RouteChannel::Control,
            action: RouteAction::Forward,
        }
    }

    pub fn for_pane(mut self) -> RouteHeader {
        self.channel = RouteChannel::Pane;
        self
    }

    pub fn restart_server(mut self) -> RouteHeader {
        self.action = RouteAction::RestartServer;
        self
    }

    pub fn replace_server(mut self) -> RouteHeader {
        self.action = RouteAction::ReplaceServer;
        self
    }

    pub fn update_server(mut self) -> RouteHeader {
        self.action = RouteAction::UpdateServer;
        self
    }

    pub fn wsl(distro: impl Into<String>) -> RouteHeader {
        RouteHeader {
            target: RouteTarget::Wsl {
                distro: distro.into(),
            },
            server_command: None,
            channel: RouteChannel::Control,
            action: RouteAction::Forward,
        }
    }

    pub fn local_stdio(program: impl Into<String>, args: &[&str]) -> RouteHeader {
        RouteHeader {
            target: RouteTarget::LocalStdio {
                program: program.into(),
                args: args.iter().map(|a| (*a).to_string()).collect(),
            },
            server_command: None,
            channel: RouteChannel::Control,
            action: RouteAction::Forward,
        }
    }

    pub fn write<W: Write>(&self, w: &mut W) -> io::Result<()> {
        let payload =
            serde_json::to_vec(self).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        protocol::write_frame(w, ROUTE_KIND, &payload)?;
        w.flush()
    }

    pub fn decode(payload: &[u8]) -> io::Result<RouteHeader> {
        serde_json::from_slice(payload).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }

    pub fn describe(&self) -> String {
        match &self.target {
            RouteTarget::Ssh(spec) => format!("ssh {}@{}:{}", spec.user, spec.host, spec.port),
            RouteTarget::Wsl { distro } => format!("wsl {distro}"),
            RouteTarget::LocalStdio { program, .. } => format!("local {program}"),
        }
    }
}

/// One phase of a remote-server install, reported back across the route so the
/// initiating UI can say what is happening. Pure wire value: the installer
/// that produces it lives in `lyrel-core`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstallPhase {
    Downloading {
        done: u64,
        total: Option<u64>,
    },
    Uploading {
        done: u64,
        total: u64,
    },
    /// The bytes are there and the far end is being waited on. Both a first
    /// install and a restart end here, which is why its caption says starting
    /// rather than restarting.
    Restarting,
}

impl InstallPhase {
    pub fn fraction(&self) -> Option<f32> {
        let (done, total) = match *self {
            InstallPhase::Downloading { done, total } => (done, total?),
            InstallPhase::Uploading { done, total } => (done, total),
            InstallPhase::Restarting => return None,
        };
        if total == 0 {
            return None;
        }
        Some((done as f32 / total as f32).clamp(0.0, 1.0))
    }
}

/// A remote daemon whose version does not match the server this build would
/// install. Pure wire value: the mismatch records the installer keeps live in
/// `lyrel-core`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MismatchedRemoteDaemon {
    pub host: String,
    pub running_version: Option<String>,
    pub running_exe: Option<String>,
    pub wanted_version: String,
}

/// The install request half of a route prompt. Pure wire shape; converting it
/// into the installer's request type and back is the installer's business and
/// lives in `lyrel-core`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallRequestWire {
    pub host: String,
    pub version: String,
    pub asset: String,
    pub source_url: String,
    pub remote_path: String,
    pub size_bytes: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoutePrompt {
    Auth {
        request_id: u64,
        prompt: AuthPromptKind,
    },
    Install {
        request_id: u64,
        request: Box<InstallRequestWire>,
    },
    Mismatch {
        daemons: Vec<MismatchedRemoteDaemon>,
    },
    InstallProgress {
        host: String,
        phase: InstallPhase,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RouteReply {
    Auth {
        request_id: u64,
        response: AuthResponse,
    },
    Install {
        request_id: u64,
        approve: bool,
    },
}

impl RoutePrompt {
    pub fn write<W: Write>(&self, w: &mut W) -> io::Result<()> {
        let payload =
            serde_json::to_vec(self).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        protocol::write_frame(w, ROUTE_PROMPT_KIND, &payload)?;
        w.flush()
    }

    pub fn decode(payload: &[u8]) -> io::Result<RoutePrompt> {
        serde_json::from_slice(payload).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }
}

impl RouteReply {
    pub fn write<W: Write>(&self, w: &mut W) -> io::Result<()> {
        let payload =
            serde_json::to_vec(self).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        protocol::write_frame(w, ROUTE_REPLY_KIND, &payload)?;
        w.flush()
    }

    pub fn decode(payload: &[u8]) -> io::Result<RouteReply> {
        serde_json::from_slice(payload).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }
}

/// Who answers auth questions raised mid-route. The default turns every
/// question away; a GUI installs a responder that can ask a person.
pub trait RouteAuthResponder: Send + Sync {
    fn respond(&self, machine: &RouteTarget, prompt: &AuthPromptKind) -> AuthResponse;
}

pub struct CancelAuth;

impl RouteAuthResponder for CancelAuth {
    fn respond(&self, _machine: &RouteTarget, _prompt: &AuthPromptKind) -> AuthResponse {
        AuthResponse::Cancelled
    }
}

static AUTH_RESPONDER: OnceLock<Mutex<Arc<dyn RouteAuthResponder>>> = OnceLock::new();

fn auth_responder_slot() -> &'static Mutex<Arc<dyn RouteAuthResponder>> {
    AUTH_RESPONDER.get_or_init(|| Mutex::new(Arc::new(CancelAuth)))
}

pub fn set_route_auth_responder(responder: Arc<dyn RouteAuthResponder>) {
    if let Ok(mut slot) = auth_responder_slot().lock() {
        *slot = responder;
    }
}

pub fn route_auth_responder() -> Arc<dyn RouteAuthResponder> {
    auth_responder_slot()
        .lock()
        .map(|slot| slot.clone())
        .unwrap_or_else(|_| Arc::new(CancelAuth))
}

impl RouteAck {
    /// The ack a finished bridge sends: it linked `link_label` and will now
    /// relay bytes.
    pub fn forward_link(link_label: &str) -> RouteAck {
        RouteAck {
            ok: true,
            link: Some(link_label.to_string()),
            action: Some(RouteAction::Forward),
            error: None,
        }
    }

    /// The ack a route that performed an action (restart/replace/update)
    /// sends instead of linking.
    pub fn acted(action: RouteAction) -> RouteAck {
        RouteAck {
            ok: true,
            link: None,
            action: Some(action),
            error: None,
        }
    }

    pub fn failed(error: String) -> RouteAck {
        RouteAck {
            ok: false,
            link: None,
            action: None,
            error: Some(error),
        }
    }

    pub fn performed(&self, action: RouteAction) -> bool {
        self.ok && self.action == Some(action)
    }

    pub fn write<W: Write>(&self, w: &mut W) -> io::Result<()> {
        let payload =
            serde_json::to_vec(self).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        protocol::write_frame(w, ROUTE_KIND, &payload)?;
        w.flush()
    }

    pub fn read<R: io::Read>(r: &mut R) -> io::Result<RouteAck> {
        let (kind, payload) = protocol::read_frame(r)?;
        if kind != ROUTE_KIND {
            if let Ok(DaemonMsg::Error(e)) = DaemonMsg::from_frame(kind, payload) {
                return Err(io::Error::other(e));
            }
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("expected a route ack, got kind {kind}"),
            ));
        }
        RouteAck::from_payload(&payload)
    }

    fn from_payload(payload: &[u8]) -> io::Result<RouteAck> {
        let ack: RouteAck = serde_json::from_slice(payload)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        if !ack.ok {
            return Err(io::Error::other(
                ack.error.unwrap_or_else(|| "route refused".into()),
            ));
        }
        Ok(ack)
    }
}

/// Negotiate a route, answering mid-route prompts with `answer`.
///
/// `answer` sees every `RoutePrompt` the server raises while the route is
/// being established and returns the `RouteReply` to send back, or `None` to
/// leave that prompt unanswered. The thin-client default — [`negotiate`] —
/// answers auth through the registered [`RouteAuthResponder`] and declines
/// install approvals; `lyrel-core` wraps this with the install policies that
/// need the daemon's installer.
pub fn negotiate_with<S, F>(
    stream: &mut S,
    header: &RouteHeader,
    mut answer: F,
) -> io::Result<RouteAck>
where
    for<'a> &'a mut S: Read + Write,
    F: FnMut(&RouteTarget, RoutePrompt) -> Option<RouteReply>,
{
    header.write(&mut &mut *stream)?;
    loop {
        let (kind, payload) = protocol::read_frame(&mut &mut *stream)?;
        match kind {
            ROUTE_KIND => return RouteAck::from_payload(&payload),
            ROUTE_PROMPT_KIND => {
                if let Some(reply) = answer(&header.target, RoutePrompt::decode(&payload)?) {
                    reply.write(&mut &mut *stream)?;
                }
            }
            other => {
                if let Ok(DaemonMsg::Error(e)) = DaemonMsg::from_frame(other, payload) {
                    return Err(io::Error::other(e));
                }
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("expected a route ack, got kind {other}"),
                ));
            }
        }
    }
}

/// The thin-client route negotiation: auth prompts go to the registered
/// responder, install approvals are declined, and informational prompts
/// (mismatch rosters, progress) are left to the peer that cares.
pub fn negotiate<S>(stream: &mut S, header: &RouteHeader) -> io::Result<RouteAck>
where
    for<'a> &'a mut S: Read + Write,
{
    negotiate_with(stream, header, |target, prompt| match prompt {
        RoutePrompt::Auth { request_id, prompt } => {
            let response = route_auth_responder().respond(target, &prompt);
            Some(RouteReply::Auth {
                request_id,
                response,
            })
        }
        RoutePrompt::Install { request_id, .. } => Some(RouteReply::Install {
            request_id,
            approve: false,
        }),
        RoutePrompt::Mismatch { .. } | RoutePrompt::InstallProgress { .. } => None,
    })
}
