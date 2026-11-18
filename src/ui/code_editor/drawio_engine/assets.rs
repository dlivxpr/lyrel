//! Fixed offline engine assets and the only HTTP-shaped surface they expose.
//!
//! Every byte the WebView can load is listed here. The native request handler
//! answers from this table and nothing else: there is no directory mapping, no
//! path joining and no fallback to the network. Paths are matched exactly, so a
//! traversal or a percent-encoded variant is not normalised into a match.
//!
//! The vendored `viewer-static.min.js` is the official draw.io release viewer
//! build (v32.6.10): it embeds the graph engine, draw.io editor classes, the
//! shape bundle and the stencil bundle, but not `extensions.min.js`. Its
//! provenance and third-party notices live in `assets/drawio/`.

use std::sync::Arc;

use lyrel_document::drawio::MAX_DOCUMENT_BYTES;

use super::session::SessionId;

/// A reserved origin (RFC 2606 `.invalid`) that can never resolve. WebView2
/// intercepts every request under it before the network stack sees it.
#[cfg(any(windows, test))]
pub(crate) const ORIGIN: &str = "https://drawio.lyrel.invalid";

pub(crate) const SHELL_PATH: &str = "/drawio/shell.html";
pub(crate) const ENGINE_PATH: &str = "/drawio/engine.html";

#[cfg(any(windows, test))]
pub(crate) const SHELL_URL: &str = "https://drawio.lyrel.invalid/drawio/shell.html";
#[cfg(any(windows, test))]
pub(crate) const ENGINE_URL: &str = "https://drawio.lyrel.invalid/drawio/engine.html";

/// macOS registers a custom WKWebView scheme; the same manifest paths and CSP
/// are served under this origin. WebKit cannot intercept https/file schemes.
/// The WKWebView host is a non-test platform component, so the URLs it alone
/// uses share that gate instead of compiling as dead constants in lib tests.
#[cfg(all(target_os = "macos", not(test)))]
pub(crate) const MAC_SCHEME: &str = "lyrel";
#[cfg(all(target_os = "macos", not(test)))]
pub(crate) const MAC_ORIGIN: &str = "lyrel://app";
#[cfg(all(target_os = "macos", not(test)))]
pub(crate) const MAC_SHELL_URL: &str = "lyrel://app/drawio/shell.html";
#[cfg(all(target_os = "macos", not(test)))]
pub(crate) const MAC_ENGINE_URL: &str = "lyrel://app/drawio/engine.html";

const SHELL_CSP: &str = "default-src 'none'; script-src 'self'; \
img-src 'none'; font-src 'none'; connect-src 'self'; media-src 'none'; \
object-src 'none'; frame-src 'self'; child-src 'self'; worker-src 'none'; \
manifest-src 'none'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'";

const ENGINE_CSP: &str = "default-src 'none'; script-src 'self'; \
style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; font-src 'self' data:; \
connect-src 'self'; media-src 'none'; object-src 'none'; frame-src 'none'; \
child-src 'none'; worker-src 'none'; manifest-src 'none'; base-uri 'none'; \
form-action 'none'; frame-ancestors 'self'";

struct Asset {
    path: &'static str,
    content_type: &'static str,
    bytes: &'static [u8],
    csp: Option<&'static str>,
}

const SHELL_HTML: &[u8] = include_bytes!("web/shell.html");
const SHELL_JS: &[u8] = include_bytes!("web/shell.js");
const SHELL_CSS: &[u8] = include_bytes!("web/shell.css");
const ENGINE_HTML: &[u8] = include_bytes!("web/engine.html");
const ENGINE_BOOT_JS: &[u8] = include_bytes!("web/engine-boot.js");
const ENGINE_BRIDGE_JS: &[u8] = include_bytes!("web/engine-bridge.js");
const ENGINE_CSS: &[u8] = include_bytes!("web/engine.css");
const VIEWER_JS: &[u8] = include_bytes!("../../../../assets/drawio/viewer-static.min.js");

/// Served paths, in manifest order. Hashes pin the exact distributed bytes.
const ASSETS: [Asset; 8] = [
    Asset {
        path: SHELL_PATH,
        content_type: "text/html; charset=utf-8",
        bytes: SHELL_HTML,
        csp: Some(SHELL_CSP),
    },
    Asset {
        path: "/drawio/shell.js",
        content_type: "text/javascript; charset=utf-8",
        bytes: SHELL_JS,
        csp: None,
    },
    Asset {
        path: "/drawio/shell.css",
        content_type: "text/css; charset=utf-8",
        bytes: SHELL_CSS,
        csp: None,
    },
    Asset {
        path: ENGINE_PATH,
        content_type: "text/html; charset=utf-8",
        bytes: ENGINE_HTML,
        csp: Some(ENGINE_CSP),
    },
    Asset {
        path: "/drawio/engine-boot.js",
        content_type: "text/javascript; charset=utf-8",
        bytes: ENGINE_BOOT_JS,
        csp: None,
    },
    Asset {
        path: "/drawio/engine-bridge.js",
        content_type: "text/javascript; charset=utf-8",
        bytes: ENGINE_BRIDGE_JS,
        csp: None,
    },
    Asset {
        path: "/drawio/engine.css",
        content_type: "text/css; charset=utf-8",
        bytes: ENGINE_CSS,
        csp: None,
    },
    Asset {
        path: "/drawio/viewer-static.min.js",
        content_type: "text/javascript; charset=utf-8",
        bytes: VIEWER_JS,
        csp: None,
    },
];

/// Test-only provenance columns for the served manifest: the exact bytes hash
/// and the license record. The product table only serves bytes; the pinned
/// hashes and license strings stay the conformance contract here and in the
/// distributed `assets/drawio/PROVENANCE.json` and notices.
#[cfg(test)]
const PROVENANCE: [(&str, &str, &str); 8] = [
    (
        SHELL_PATH,
        "d7641f884cb58a60f0a0a97b4441cb942f0e8e4a3e76b6aa8001ed67aaf4825e",
        "Lyrel Apache-2.0",
    ),
    (
        "/drawio/shell.js",
        "e39559bc119142ae548e1b9a2df7b577cae039564184cb58e9d263d25789c314",
        "Lyrel Apache-2.0",
    ),
    (
        "/drawio/shell.css",
        "269e050a80e8117f8ff00a4df12dc71e1ef88bf7ba8ba1e70c4e1a44ec6d5f04",
        "Lyrel Apache-2.0",
    ),
    (
        ENGINE_PATH,
        "25d10235a995db1f858284270f49710dfe55546854cbdf3f48c2f99128071efc",
        "Lyrel Apache-2.0",
    ),
    (
        "/drawio/engine-boot.js",
        "ca7a4157d42e6e66dc8b61e612d48f7f668aeee94ae9cb9b6bd5f5f29aa66a26",
        "Lyrel Apache-2.0",
    ),
    (
        "/drawio/engine-bridge.js",
        "3df2e8ad846fd98668d0d93aa5110ea34fdc9a99f8ce4c5be29b2305e21fc2cc",
        "Lyrel Apache-2.0",
    ),
    (
        "/drawio/engine.css",
        "fe255083e55d49767e5a8a652203d89564bf1280420fd30a9da71f0b7f0627c5",
        "Lyrel Apache-2.0",
    ),
    (
        "/drawio/viewer-static.min.js",
        "dfa1cfb4ef24320b67131405da88511e7baba48621ddd5260eea8ec5f6425928",
        "Apache-2.0 draw.io v32.6.10; see assets/drawio/THIRD-PARTY-NOTICES.md",
    ),
];

pub(crate) enum EngineBody {
    Static(&'static [u8]),
    Shared(Arc<[u8]>),
}

impl EngineBody {
    pub(crate) fn bytes(&self) -> &[u8] {
        match self {
            Self::Static(bytes) => bytes,
            Self::Shared(bytes) => bytes,
        }
    }
}

pub(crate) struct EngineResponse {
    pub(crate) status: u16,
    #[cfg(any(windows, test))]
    pub(crate) reason: &'static str,
    pub(crate) content_type: &'static str,
    pub(crate) headers: Vec<(&'static str, String)>,
    pub(crate) body: EngineBody,
}

impl EngineResponse {
    fn forbidden() -> Self {
        Self {
            status: 403,
            #[cfg(any(windows, test))]
            reason: "Forbidden",
            content_type: "text/plain; charset=utf-8",
            headers: base_headers(),
            body: EngineBody::Static(b""),
        }
    }

    fn method_not_allowed() -> Self {
        let mut response = Self {
            status: 405,
            #[cfg(any(windows, test))]
            reason: "Method Not Allowed",
            content_type: "text/plain; charset=utf-8",
            headers: base_headers(),
            body: EngineBody::Static(b""),
        };
        response.headers.push(("Allow", "GET".to_string()));
        response
    }

    fn too_large() -> Self {
        Self {
            status: 413,
            #[cfg(any(windows, test))]
            reason: "Payload Too Large",
            content_type: "text/plain; charset=utf-8",
            headers: base_headers(),
            body: EngineBody::Static(b""),
        }
    }
}

fn base_headers() -> Vec<(&'static str, String)> {
    vec![
        ("Cache-Control", "no-store".to_string()),
        ("X-Content-Type-Options", "nosniff".to_string()),
        ("Referrer-Policy", "no-referrer".to_string()),
        ("Cross-Origin-Resource-Policy", "same-origin".to_string()),
    ]
}

fn asset_response(asset: &Asset) -> EngineResponse {
    let mut headers = base_headers();
    if let Some(csp) = asset.csp {
        headers.push(("Content-Security-Policy", csp.to_string()));
    }
    EngineResponse {
        status: 200,
        #[cfg(any(windows, test))]
        reason: "OK",
        content_type: asset.content_type,
        headers,
        body: EngineBody::Static(asset.bytes),
    }
}

/// Answers one WebView request. Any query or fragment, any percent escape and
/// any path not in the manifest is refused. The active session's document is
/// the one dynamic GET; a commit or sync `POST` under the same session is the
/// only write, answered by the document the shell proves belongs to it.
#[cfg(any(windows, test))]
pub(crate) fn respond(
    method: &str,
    target: &str,
    session: Option<&mut super::bridge::LiveSession>,
    body: Option<&[u8]>,
) -> (EngineResponse, Option<Arc<[u8]>>) {
    respond_from(method, target, ORIGIN, session, body)
}

/// The same exact-manifest policy for a platform-specific origin. Windows
/// intercepts `ORIGIN`; macOS serves the same paths from a custom scheme.
/// Returns the response and, for an accepted commit, the bytes it delivered.
pub(crate) fn respond_from(
    method: &str,
    target: &str,
    origin: &str,
    mut session: Option<&mut super::bridge::LiveSession>,
    body: Option<&[u8]>,
) -> (EngineResponse, Option<Arc<[u8]>>) {
    let Some(path) = target.strip_prefix(origin) else {
        return (EngineResponse::forbidden(), None);
    };
    if path.is_empty()
        || path.contains('?')
        || path.contains('#')
        || path.contains('%')
        || path.contains('\\')
        || path.contains("..")
        || path.contains("//")
    {
        return (EngineResponse::forbidden(), None);
    }
    if method == "POST" {
        // The only write the engine gets: the serialized mxfile under its own
        // session id, newest sequence first. A stale or oversized body is a
        // plain refusal; the accepted bytes leave through the sink, not the
        // response, so no diagram content is served back to the page.
        let Some((session_id, rest)) = session_target(path) else {
            return (EngineResponse::forbidden(), None);
        };
        let (seq, sync) = match rest {
            SessionRest::Commit(seq) => (seq, false),
            SessionRest::Sync(seq) => (seq, true),
            SessionRest::Document => return (EngineResponse::forbidden(), None),
        };
        let Some(session) = session
            .as_mut()
            .map(|session| &mut **session)
            .filter(|session| session.document().id == session_id)
        else {
            return (EngineResponse::forbidden(), None);
        };
        let Some(bytes) = body.filter(|bytes| !bytes.is_empty()) else {
            return (EngineResponse::forbidden(), None);
        };
        if bytes.len() > MAX_DOCUMENT_BYTES {
            return (EngineResponse::too_large(), None);
        }
        if session.commit(seq, bytes, sync).is_err() {
            return (EngineResponse::forbidden(), None);
        }
        let bytes: Arc<[u8]> = Arc::from(bytes);
        return (
            EngineResponse {
                status: 200,
                #[cfg(any(windows, test))]
                reason: "OK",
                content_type: "text/plain; charset=utf-8",
                headers: base_headers(),
                body: EngineBody::Static(b""),
            },
            Some(bytes),
        );
    }
    if method != "GET" {
        return (EngineResponse::method_not_allowed(), None);
    }
    if let Some(asset) = ASSETS.iter().find(|asset| asset.path == path) {
        return (asset_response(asset), None);
    }
    if let Some((session_id, SessionRest::Document)) = session_target(path) {
        let Some(session) = session.filter(|session| session.document().id == session_id) else {
            return (EngineResponse::forbidden(), None);
        };
        if session.document().body.len() > MAX_DOCUMENT_BYTES {
            return (EngineResponse::too_large(), None);
        }
        let mut headers = base_headers();
        headers.push((
            "Content-Security-Policy",
            "default-src 'none'; sandbox".to_string(),
        ));
        return (
            EngineResponse {
                status: 200,
                #[cfg(any(windows, test))]
                reason: "OK",
                content_type: "application/xml; charset=utf-8",
                headers,
                body: EngineBody::Shared(session.document().body.clone()),
            },
            None,
        );
    }
    (EngineResponse::forbidden(), None)
}

/// The remainder of `/drawio/session/<32 hex>/`: the document a GET serves,
/// plus the commit and sync sequences a POST may append.
enum SessionRest {
    Document,
    Commit(u64),
    Sync(u64),
}

/// Splits `/drawio/session/<32 hex>/<rest>` into the session and its trailing
/// route. A revision never appears in the URL: replacing the document
/// creates a new session instead of letting an old URL address new bytes.
fn session_target(path: &str) -> Option<(SessionId, SessionRest)> {
    let rest = path.strip_prefix("/drawio/session/")?;
    let (id, rest) = rest.split_at_checked(32)?;
    let rest = rest.strip_prefix('/')?;
    if !id
        .bytes()
        .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return None;
    }
    let id = SessionId::parse(id)?;
    if rest == "document" {
        return Some((id, SessionRest::Document));
    }
    let (kind, seq) = rest.split_once('/')?;
    if !seq.bytes().all(|byte| byte.is_ascii_digit()) || seq.len() > 20 {
        return None;
    }
    let seq: u64 = seq.parse().ok()?;
    match kind {
        "commit" => Some((id, SessionRest::Commit(seq))),
        "sync" => Some((id, SessionRest::Sync(seq))),
        _ => None,
    }
}

#[cfg(test)]
pub(crate) fn asset_paths() -> impl Iterator<Item = &'static str> {
    ASSETS.iter().map(|asset| asset.path)
}

#[cfg(test)]
pub(crate) fn asset_manifest() -> Vec<(
    &'static str,
    &'static [u8],
    &'static str,
    &'static str,
    bool,
)> {
    ASSETS
        .iter()
        .map(|asset| {
            let (_, sha256, license) = PROVENANCE
                .iter()
                .find(|(path, _, _)| *path == asset.path)
                .expect("every served asset has a provenance row");
            (
                asset.path,
                asset.bytes,
                *sha256,
                *license,
                asset.csp.is_some(),
            )
        })
        .collect()
}

#[cfg(test)]
pub(crate) fn shell_csp() -> &'static str {
    SHELL_CSP
}

#[cfg(test)]
pub(crate) fn engine_csp() -> &'static str {
    ENGINE_CSP
}
