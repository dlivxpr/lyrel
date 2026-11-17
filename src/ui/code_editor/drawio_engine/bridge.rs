//! Native half of the shell bridge.
//!
//! The page is untrusted input. Every accepted message must come from the one
//! shell URL, carry this session's token, use the next expected sequence and
//! fit the fixed event grammar. Anything else is dropped without touching the
//! document or the surface. A reload creates a new [`LiveSession`]; the old
//! token, session id and message sequence stop matching at once.

use std::io;
use std::sync::Arc;

#[cfg(any(windows, test))]
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
    /// The engine's own Ctrl+S: the workbench decides which Host file the
    /// request writes; the page never learns a path.
    Save,
    /// Engine-native Ctrl+Z / Ctrl+Y. The document decides whether the live
    /// session replays it or reloads canonical bytes.
    Undo,
    Redo,
    /// The engine needs the system clipboard read and handed back.
    Paste,
    /// Serialized cells the engine copied (or cut); `cut` only records intent —
    /// the deletion already landed in the document bytes this engine commits.
    Clipboard {
        text: String,
        cut: bool,
    },
    /// The page confirmed that the sync command with this native sequence was
    /// serialized and its POST was accepted. The bytes travel through the
    /// same commit sink; this acknowledgement binds a deferred save to the
    /// request it actually sent rather than to any later document revision.
    SyncConfirmed {
        request: u64,
    },
    /// That sync POST was refused (HTTP or network). Only this request fails;
    /// the session and the in-memory canvas stay alive.
    SyncRejected {
        request: u64,
        reason: String,
    },
    /// An ordinary edit commit was refused. The canvas keeps its model, which
    /// is now unsaved; the session is not retired.
    CommitRejected {
        reason: String,
    },
    Failed {
        message: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Reject(pub(crate) &'static str);

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Command {
    Page(usize),
    Fit,
    ZoomIn,
    ZoomOut,
    ActualSize,
    /// Serialize and deliver the current engine state even without new edits,
    /// so a deferred save can capture the live page's model.
    Sync,
    /// Cells the system clipboard offered. The engine decides whether it is
    /// diagram XML or plain text.
    Paste(String),
    /// Copy the selection through the engine's own clipboard.
    CopyCells,
    /// Copy the selection and remove it as one undoable edit.
    CutCells,
    /// Delete the selection as one undoable edit.
    DeleteCells,
}

impl Command {
    fn wire_name(&self) -> &'static str {
        match self {
            Self::Page(_) => "page",
            Self::Fit => "fit",
            Self::ZoomIn => "zoomIn",
            Self::ZoomOut => "zoomOut",
            Self::ActualSize => "actual",
            Self::Sync => "sync",
            Self::Paste(_) => "paste",
            Self::CopyCells => "copy",
            Self::CutCells => "cut",
            Self::DeleteCells => "delete",
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
    /// The outbound command sequence of the newest `sync` request this
    /// session sent and has not answered yet. A page acknowledgement or
    /// failure must name exactly this request; an older sync can never
    /// confirm a newer save.
    pending_sync: Option<u64>,
    /// The newest engine commit sequence accepted on this session, so a
    /// POST that arrives after a later state can never roll bytes back.
    last_commit: u64,
    pages: usize,
    page: usize,
    zoom: f32,
}

impl LiveSession {
    #[cfg(any(windows, test))]
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
            pending_sync: None,
            last_commit: 0,
            pages: 0,
            page: 0,
            zoom: 1.,
        }
    }

    #[cfg(test)]
    pub(crate) fn document_path(&self) -> String {
        self.document.path()
    }

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
            "save" => {
                if !self.ready {
                    return Err(Reject("save-before-ready"));
                }
                if self.failed {
                    return Err(Reject("save-after-failure"));
                }
                BridgeEvent::Save
            }
            "undo" => {
                if !self.ready {
                    return Err(Reject("undo-before-ready"));
                }
                if self.failed {
                    return Err(Reject("undo-after-failure"));
                }
                BridgeEvent::Undo
            }
            "redo" => {
                if !self.ready {
                    return Err(Reject("redo-before-ready"));
                }
                if self.failed {
                    return Err(Reject("redo-after-failure"));
                }
                BridgeEvent::Redo
            }
            "paste" => {
                if !self.ready {
                    return Err(Reject("paste-before-ready"));
                }
                if self.failed {
                    return Err(Reject("paste-after-failure"));
                }
                BridgeEvent::Paste
            }
            "sync-done" => {
                if !self.ready {
                    return Err(Reject("sync-before-ready"));
                }
                if self.failed {
                    return Err(Reject("sync-after-failure"));
                }
                let request = payload
                    .get("request")
                    .and_then(serde_json::Value::as_u64)
                    .ok_or(Reject("sync-request"))?;
                if self.pending_sync != Some(request) {
                    return Err(Reject("sync-request"));
                }
                self.pending_sync = None;
                BridgeEvent::SyncConfirmed { request }
            }
            "commit-failed" => {
                if !self.ready {
                    return Err(Reject("commit-before-ready"));
                }
                if self.failed {
                    return Err(Reject("commit-after-failure"));
                }
                let sync = payload
                    .get("sync")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false);
                let status = payload
                    .get("status")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0);
                let reason = rejection_reason(
                    status,
                    payload
                        .get("message")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or_default(),
                );
                if sync {
                    let request = payload
                        .get("request")
                        .and_then(serde_json::Value::as_u64)
                        .ok_or(Reject("sync-request"))?;
                    if self.pending_sync != Some(request) {
                        return Err(Reject("sync-request"));
                    }
                    self.pending_sync = None;
                    BridgeEvent::SyncRejected { request, reason }
                } else {
                    BridgeEvent::CommitRejected { reason }
                }
            }
            "clipboard" => {
                if !self.ready {
                    return Err(Reject("clipboard-before-ready"));
                }
                if self.failed {
                    return Err(Reject("clipboard-after-failure"));
                }
                let text = payload
                    .get("text")
                    .and_then(serde_json::Value::as_str)
                    .ok_or(Reject("clipboard-text"))?;
                if text.is_empty() || text.len() > lyrel_code_editor::drawio::MAX_DOCUMENT_BYTES {
                    return Err(Reject("clipboard-text"));
                }
                BridgeEvent::Clipboard {
                    text: text.to_string(),
                    cut: payload
                        .get("cut")
                        .and_then(serde_json::Value::as_bool)
                        .unwrap_or(false),
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

    /// One engine commit POST: the newest serialized mxfile the live engine
    /// produced. Only `POST` reaches here; `sync` is a flush whose sequence
    /// may equal the last applied one, while a real commit must be newer so a
    /// reordered request cannot resurrect old bytes.
    pub(crate) fn commit(&mut self, seq: u64, body: &[u8], sync: bool) -> Result<(), Reject> {
        if !self.ready || self.failed {
            return Err(Reject("not-ready"));
        }
        if seq == 0 || body.is_empty() || body.len() > lyrel_code_editor::drawio::MAX_DOCUMENT_BYTES
        {
            return Err(Reject("commit-size"));
        }
        if seq < self.last_commit || (seq == self.last_commit && !sync) {
            return Err(Reject("commit-stale"));
        }
        if seq > self.last_commit {
            self.last_commit = seq;
        }
        Ok(())
    }

    /// Builds one native-to-shell command. Commands are refused until the
    /// engine has booted, so a page that never lay out does not receive
    /// document-changing work it cannot answer.
    pub(crate) fn command(&mut self, command: &Command) -> Option<String> {
        if !self.ready || self.failed {
            return None;
        }
        let page = match command {
            Command::Page(page) => {
                if self.pages == 0 {
                    return None;
                }
                Some((*page).min(self.pages - 1))
            }
            _ => None,
        };
        self.last_outbound += 1;
        if matches!(command, Command::Sync) {
            self.pending_sync = Some(self.last_outbound);
        }
        let mut message = serde_json::json!({
            "v": PROTOCOL_VERSION,
            "session": self.document.id.hex(),
            "seq": self.last_outbound,
            "cmd": command.wire_name(),
        });
        if let Some(page) = page {
            message["page"] = serde_json::json!(page);
        }
        if let Command::Paste(text) = command {
            // The system clipboard is already validated text; the 64 KiB
            // message budget still applies, so an oversized paste is refused
            // rather than truncated into the page.
            if text.len() > MAX_MESSAGE_BYTES / 2 {
                return None;
            }
            message["text"] = serde_json::json!(text);
        }
        Some(message.to_string())
    }

    /// The sequence of the last command actually delivered to the page. A
    /// surface that queues a sync returns it so the workbench can wait for
    /// this exact acknowledgement.
    pub(crate) fn outbound_seq(&self) -> u64 {
        self.last_outbound
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

/// A bounded, user-visible reason for a refused page request. The page may
/// name a status; free text is sanitized and capped like every other failure.
fn rejection_reason(status: u64, message: &str) -> String {
    let message = sanitize_failure(message);
    if !message.is_empty() {
        return message;
    }
    match status {
        1..=599 => format!("HTTP {status}"),
        _ => "the diagram request failed".to_string(),
    }
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
        assert!(session.command(&Command::Page(0)).is_none());
        ready(&mut session);
        assert!(session.command(&Command::Page(0)).is_none());
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
        let command = session.command(&Command::Page(99)).unwrap();
        assert!(command.contains("\"cmd\":\"page\""));
        assert!(command.contains("\"page\":1"));
        assert_eq!(session.last_outbound(), 1);
        assert!(
            session
                .command(&Command::Fit)
                .unwrap()
                .contains("\"cmd\":\"fit\"")
        );
        assert_eq!(session.last_outbound(), 2);
    }

    #[test]
    fn sync_acknowledgement_and_commit_failures_are_bound_to_their_request() {
        let mut session = session();
        ready(&mut session);
        let command = session.command(&Command::Sync).expect("sync is sent");
        assert!(command.contains("\"cmd\":\"sync\""));
        let request = session.outbound_seq();
        // Any other request number — a previous save included — is refused,
        // and a refused message does not consume the inbound sequence.
        for wrong in [request.saturating_sub(1), request + 1] {
            assert_eq!(
                session.accept(
                    SHELL_URL,
                    &wire(
                        &session,
                        2,
                        "sync-done",
                        serde_json::json!({"request": wrong})
                    ),
                ),
                Err(Reject("sync-request"))
            );
        }
        assert_eq!(
            session.accept(
                SHELL_URL,
                &wire(
                    &session,
                    2,
                    "sync-done",
                    serde_json::json!({"request": request})
                ),
            ),
            Ok(BridgeEvent::SyncConfirmed { request })
        );
        // Once answered, the same request cannot confirm another save.
        assert_eq!(
            session.accept(
                SHELL_URL,
                &wire(
                    &session,
                    3,
                    "sync-done",
                    serde_json::json!({"request": request})
                ),
            ),
            Err(Reject("sync-request"))
        );

        // A refused sync and a refused ordinary commit name their reason and
        // leave the session booted: neither retires the canvas.
        session.command(&Command::Sync).expect("second sync");
        let request = session.outbound_seq();
        assert_eq!(
            session.accept(
                SHELL_URL,
                &wire(
                    &session,
                    3,
                    "commit-failed",
                    serde_json::json!({"sync": true, "request": request, "status": 413})
                ),
            ),
            Ok(BridgeEvent::SyncRejected {
                request,
                reason: "HTTP 413".into()
            })
        );
        assert_eq!(
            session.accept(
                SHELL_URL,
                &wire(
                    &session,
                    4,
                    "commit-failed",
                    serde_json::json!({"sync": false, "status": 0,
                        "message": "the diagram request failed"})
                ),
            ),
            Ok(BridgeEvent::CommitRejected {
                reason: "the diagram request failed".into()
            })
        );
        assert!(matches!(
            session.accept(
                SHELL_URL,
                &wire(
                    &session,
                    5,
                    "state",
                    serde_json::json!({"page":0,"pages":1,"zoom":1})
                ),
            ),
            Ok(BridgeEvent::State { .. })
        ));
    }
}
