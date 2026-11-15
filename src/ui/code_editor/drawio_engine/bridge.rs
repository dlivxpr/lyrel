//! Native half of the shell bridge.
//!
//! The page is untrusted input. Every accepted message must come from the one
//! shell URL, carry this session's token, use the next expected sequence and
//! fit the fixed event grammar. Anything else is dropped without touching the
//! document or the surface. A reload creates a new [`LiveSession`]; the old
//! token, session id and message sequence stop matching at once.

use std::io;
use std::sync::Arc;

use super::assets::SHELL_URL;
#[cfg(test)]
use super::session::SessionId;
use super::session::{DocumentSession, PreviewTheme, SessionToken};

pub(crate) const PROTOCOL_VERSION: u64 = 1;
pub(crate) const MAX_MESSAGE_BYTES: usize = 64 * 1024;
pub(crate) const MAX_PAGES: usize = 512;
const MAX_FAILURE_CHARS: usize = 240;

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum BridgeEvent {
    Ready,
    State {
        page: usize,
        pages: usize,
        zoom: f32,
    },
    Failed {
        message: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Reject(pub(crate) &'static str);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Command {
    Page(usize),
    Fit,
    ZoomIn,
    ZoomOut,
    ActualSize,
}

impl Command {
    fn wire_name(self) -> &'static str {
        match self {
            Self::Page(_) => "page",
            Self::Fit => "fit",
            Self::ZoomIn => "zoomIn",
            Self::ZoomOut => "zoomOut",
            Self::ActualSize => "actual",
        }
    }
}

/// One browser session: the document it may read, its token and the event
/// sequence both directions. Dropped when the native surface is retired.
pub(crate) struct LiveSession {
    document: DocumentSession,
    token: SessionToken,
    shell_url: &'static str,
    ready: bool,
    failed: bool,
    last_inbound: u64,
    last_outbound: u64,
    pages: usize,
    page: usize,
    zoom: f32,
}

impl LiveSession {
    pub(crate) fn start(body: Arc<[u8]>) -> io::Result<Self> {
        Self::start_for(body, SHELL_URL)
    }

    /// Starts a session whose top document lives at `shell_url`. Windows uses
    /// the intercepted https origin; macOS uses its custom scheme.
    pub(crate) fn start_for(body: Arc<[u8]>, shell_url: &'static str) -> io::Result<Self> {
        let document = DocumentSession::new(body)?;
        let token = SessionToken::random()?;
        Ok(Self::with_parts(document, token, shell_url))
    }

    #[cfg(test)]
    pub(crate) fn for_test(body: Arc<[u8]>, session: SessionId, token: SessionToken) -> Self {
        Self::with_parts(DocumentSession { id: session, body }, token, SHELL_URL)
    }

    fn with_parts(document: DocumentSession, token: SessionToken, shell_url: &'static str) -> Self {
        Self {
            document,
            token,
            shell_url,
            ready: false,
            failed: false,
            last_inbound: 0,
            last_outbound: 0,
            pages: 0,
            page: 0,
            zoom: 1.,
        }
    }

    #[cfg(test)]
    pub(crate) fn document_path(&self) -> String {
        self.document.path()
    }

    #[cfg(not(test))]
    pub(crate) fn document(&self) -> &DocumentSession {
        &self.document
    }

    #[cfg(test)]
    pub(crate) fn session_hex(&self) -> String {
        self.document.id.hex()
    }

    #[cfg(test)]
    pub(crate) fn token_hex(&self) -> String {
        self.token.hex()
    }

    #[cfg(test)]
    pub(crate) fn pages(&self) -> usize {
        self.pages
    }

    #[cfg(test)]
    pub(crate) fn page(&self) -> usize {
        self.page
    }

    #[cfg(test)]
    pub(crate) fn zoom(&self) -> f32 {
        self.zoom
    }

    /// The script injected into the top frame after `NavigationCompleted`.
    /// Only ASCII secrets and the fixed path reach the page; the object is
    /// parsed by the shell's frozen `bind`, once.
    pub(crate) fn bind_script(&self, theme: PreviewTheme) -> String {
        let message = serde_json::json!({
            "token": self.token.hex(),
            "session": self.document.id.hex(),
            "document": self.document.path(),
            "theme": theme.as_str(),
        });
        format!(
            "(function(){{var m={};return window.__lyrel.bind(m);}})()",
            message
        )
    }

    /// Parses and validates one web message. `source` is WebView2's report of
    /// which document sent it; it is a second gate behind the token.
    pub(crate) fn accept(&mut self, source: &str, body: &[u8]) -> Result<BridgeEvent, Reject> {
        if source != self.shell_url {
            return Err(Reject("source"));
        }
        if body.is_empty() || body.len() > MAX_MESSAGE_BYTES {
            return Err(Reject("size"));
        }
        let message: serde_json::Value =
            serde_json::from_slice(body).map_err(|_| Reject("json"))?;
        let object = message.as_object().ok_or(Reject("shape"))?;
        if object.get("v").and_then(serde_json::Value::as_u64) != Some(PROTOCOL_VERSION) {
            return Err(Reject("version"));
        }
        let session = object
            .get("session")
            .and_then(serde_json::Value::as_str)
            .ok_or(Reject("session"))?;
        if session != self.document.id.hex() {
            return Err(Reject("session"));
        }
        let token = object
            .get("token")
            .and_then(serde_json::Value::as_str)
            .ok_or(Reject("token"))?;
        if !self.token.matches(token) {
            return Err(Reject("token"));
        }
        let seq = object
            .get("seq")
            .and_then(serde_json::Value::as_u64)
            .ok_or(Reject("seq"))?;
        if seq != self.last_inbound + 1 {
            return Err(Reject("seq"));
        }
        let kind = object
            .get("kind")
            .and_then(serde_json::Value::as_str)
            .ok_or(Reject("kind"))?;
        let payload = object
            .get("payload")
            .ok_or(Reject("payload"))?
            .as_object()
            .ok_or(Reject("payload"))?;
        let event = match kind {
            "ready" => {
                if self.ready {
                    return Err(Reject("duplicate-ready"));
                }
                self.ready = true;
                BridgeEvent::Ready
            }
            "state" => {
                if !self.ready {
                    return Err(Reject("state-before-ready"));
                }
                if self.failed {
                    return Err(Reject("state-after-failure"));
                }
                let pages = payload
                    .get("pages")
                    .and_then(serde_json::Value::as_u64)
                    .ok_or(Reject("pages"))?;
                let page = payload
                    .get("page")
                    .and_then(serde_json::Value::as_u64)
                    .ok_or(Reject("page"))?;
                if pages == 0 || pages > MAX_PAGES as u64 || page >= pages {
                    return Err(Reject("page-range"));
                }
                let zoom = payload
                    .get("zoom")
                    .and_then(serde_json::Value::as_f64)
                    .unwrap_or(1.);
                if !zoom.is_finite() || !(0.01..=64.).contains(&zoom) {
                    return Err(Reject("zoom"));
                }
                self.pages = pages as usize;
                self.page = page as usize;
                self.zoom = zoom as f32;
                BridgeEvent::State {
                    page: self.page,
                    pages: self.pages,
                    zoom: self.zoom,
                }
            }
            "failed" => {
                let message = payload
                    .get("message")
                    .and_then(serde_json::Value::as_str)
                    .ok_or(Reject("failure"))?;
                let message = sanitize_failure(message);
                if message.is_empty() {
                    return Err(Reject("failure"));
                }
                self.failed = true;
                BridgeEvent::Failed { message }
            }
            _ => return Err(Reject("kind")),
        };
        self.last_inbound = seq;
        Ok(event)
    }

    /// Builds one native-to-shell command. Commands are refused until the
    /// engine has booted, so a page that never lay out does not receive
    /// document-changing work it cannot answer.
    pub(crate) fn command(&mut self, command: Command) -> Option<String> {
        if !self.ready || self.failed {
            return None;
        }
        let page = match command {
            Command::Page(page) => {
                if self.pages == 0 {
                    return None;
                }
                let page = page.min(self.pages - 1);
                Some(page)
            }
            _ => None,
        };
        self.last_outbound += 1;
        let mut message = serde_json::json!({
            "v": PROTOCOL_VERSION,
            "session": self.document.id.hex(),
            "seq": self.last_outbound,
            "cmd": command.wire_name(),
        });
        if let Some(page) = page {
            message["page"] = serde_json::json!(page);
        }
        Some(message.to_string())
    }

    #[cfg(test)]
    pub(crate) fn last_outbound(&self) -> u64 {
        self.last_outbound
    }
}

fn sanitize_failure(message: &str) -> String {
    message
        .chars()
        .filter(|character| !character.is_control())
        .take(MAX_FAILURE_CHARS)
        .collect::<String>()
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session() -> LiveSession {
        let body: Arc<[u8]> = Arc::from(b"<mxfile/>".as_slice());
        LiveSession::for_test(
            body,
            SessionId::parse("0123456789abcdef0123456789abcdef").unwrap(),
            SessionToken::random().unwrap(),
        )
    }

    fn wire(session: &LiveSession, seq: u64, kind: &str, payload: serde_json::Value) -> Vec<u8> {
        serde_json::json!({
            "v": 1,
            "session": session.session_hex(),
            "token": session.token_hex(),
            "seq": seq,
            "kind": kind,
            "payload": payload,
        })
        .to_string()
        .into_bytes()
    }

    fn ready(session: &mut LiveSession) {
        assert_eq!(
            session.accept(SHELL_URL, &wire(session, 1, "ready", serde_json::json!({}))),
            Ok(BridgeEvent::Ready)
        );
    }

    #[test]
    fn bind_script_carries_identity_but_never_the_document() {
        let session = session();
        let script = session.bind_script(PreviewTheme::Dark);
        assert!(script.contains(&session.token_hex()));
        assert!(script.contains(&session.session_hex()));
        assert!(script.contains(&session.document_path()));
        assert!(script.contains("\"theme\":\"dark\""));
        assert!(!script.contains("<mxfile"));
    }

    #[test]
    fn the_expected_sequence_is_accepted_after_a_ready_boot() {
        let mut session = session();
        ready(&mut session);
        let event = session
            .accept(
                SHELL_URL,
                &wire(
                    &session,
                    2,
                    "state",
                    serde_json::json!({"page": 1, "pages": 3, "zoom": 1.5}),
                ),
            )
            .unwrap();
        assert_eq!(
            event,
            BridgeEvent::State {
                page: 1,
                pages: 3,
                zoom: 1.5
            }
        );
        assert_eq!(session.pages(), 3);
        assert_eq!(session.page(), 1);
        assert!((session.zoom() - 1.5).abs() < f32::EPSILON);
    }

    #[test]
    fn foreign_source_token_session_replay_and_shape_are_rejected() {
        let mut session = session();
        assert_eq!(
            session.accept(
                "https://evil.invalid/drawio/shell.html",
                &wire(&session, 1, "ready", serde_json::json!({}))
            ),
            Err(Reject("source"))
        );
        assert_eq!(session.accept(SHELL_URL, b"not json"), Err(Reject("json")));
        let mut message: serde_json::Value =
            serde_json::from_slice(&wire(&session, 1, "ready", serde_json::json!({}))).unwrap();
        message["token"] = serde_json::json!("00000000000000000000000000000000");
        assert_eq!(
            session.accept(SHELL_URL, message.to_string().as_bytes()),
            Err(Reject("token"))
        );
        let mut message: serde_json::Value =
            serde_json::from_slice(&wire(&session, 1, "ready", serde_json::json!({}))).unwrap();
        message["session"] = serde_json::json!("ffffffffffffffffffffffffffffffff");
        assert_eq!(
            session.accept(SHELL_URL, message.to_string().as_bytes()),
            Err(Reject("session"))
        );
        let first = wire(&session, 1, "ready", serde_json::json!({}));
        assert_eq!(session.accept(SHELL_URL, &first), Ok(BridgeEvent::Ready));
        assert_eq!(session.accept(SHELL_URL, &first), Err(Reject("seq")));
        assert_eq!(
            session.accept(
                SHELL_URL,
                &wire(&session, 2, "state", serde_json::json!({})),
            ),
            Err(Reject("pages"))
        );
    }

    #[test]
    fn a_different_session_cannot_reuse_the_old_stream() {
        let mut first = session();
        ready(&mut first);
        let body: Arc<[u8]> = Arc::from(b"<mxfile/>".as_slice());
        let mut second = LiveSession::for_test(
            body,
            SessionId::parse("fedcba9876543210fedcba9876543210").unwrap(),
            SessionToken::random().unwrap(),
        );
        let old = wire(
            &first,
            2,
            "state",
            serde_json::json!({"page":0,"pages":1,"zoom":1}),
        );
        assert_eq!(second.accept(SHELL_URL, &old), Err(Reject("session")));
        ready(&mut second);
        assert_eq!(
            second.accept(
                SHELL_URL,
                &wire(
                    &second,
                    2,
                    "state",
                    serde_json::json!({"page": 0, "pages": 1, "zoom": 1})
                ),
            ),
            Ok(BridgeEvent::State {
                page: 0,
                pages: 1,
                zoom: 1.0
            })
        );
    }

    #[test]
    fn state_before_boot_duplicate_boot_and_failed_documents_are_explicit() {
        let mut session = session();
        assert_eq!(
            session.accept(
                SHELL_URL,
                &wire(
                    &session,
                    1,
                    "state",
                    serde_json::json!({"page":0,"pages":1,"zoom":1})
                ),
            ),
            Err(Reject("state-before-ready"))
        );
        ready(&mut session);
        assert_eq!(
            session.accept(
                SHELL_URL,
                &wire(&session, 2, "ready", serde_json::json!({}))
            ),
            Err(Reject("duplicate-ready"))
        );
        assert_eq!(
            session.accept(
                SHELL_URL,
                &wire(
                    &session,
                    2,
                    "failed",
                    serde_json::json!({"message":"bad\n\u{7f}diagram"})
                ),
            ),
            Ok(BridgeEvent::Failed {
                message: "baddiagram".to_string()
            })
        );
        assert_eq!(
            session.accept(
                SHELL_URL,
                &wire(
                    &session,
                    3,
                    "state",
                    serde_json::json!({"page":0,"pages":1,"zoom":1})
                ),
            ),
            Err(Reject("state-after-failure"))
        );
    }

    #[test]
    fn oversized_messages_and_unbooted_commands_are_refused() {
        let mut session = session();
        assert_eq!(
            session.accept(SHELL_URL, &vec![b'x'; MAX_MESSAGE_BYTES + 1]),
            Err(Reject("size"))
        );
        assert!(session.command(Command::Page(0)).is_none());
        ready(&mut session);
        assert!(session.command(Command::Page(0)).is_none());
        assert_eq!(
            session.accept(
                SHELL_URL,
                &wire(
                    &session,
                    2,
                    "state",
                    serde_json::json!({"page":0,"pages":2,"zoom":1})
                ),
            ),
            Ok(BridgeEvent::State {
                page: 0,
                pages: 2,
                zoom: 1.0
            })
        );
        let command = session.command(Command::Page(99)).unwrap();
        assert!(command.contains("\"cmd\":\"page\""));
        assert!(command.contains("\"page\":1"));
        assert_eq!(session.last_outbound(), 1);
        assert!(
            session
                .command(Command::Fit)
                .unwrap()
                .contains("\"cmd\":\"fit\"")
        );
        assert_eq!(session.last_outbound(), 2);
    }
}
