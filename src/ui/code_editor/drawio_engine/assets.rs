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

use super::session::DocumentSession;
use super::session::SessionId;

/// A reserved origin (RFC 2606 `.invalid`) that can never resolve. WebView2
/// intercepts every request under it before the network stack sees it.
pub(crate) const ORIGIN: &str = "https://drawio.lyrel.invalid";

pub(crate) const SHELL_PATH: &str = "/drawio/shell.html";
pub(crate) const ENGINE_PATH: &str = "/drawio/engine.html";

pub(crate) const SHELL_URL: &str = "https://drawio.lyrel.invalid/drawio/shell.html";
pub(crate) const ENGINE_URL: &str = "https://drawio.lyrel.invalid/drawio/engine.html";

/// macOS registers a custom WKWebView scheme; the same manifest paths and CSP
/// are served under this origin. WebKit cannot intercept https/file schemes.
pub(crate) const MAC_SCHEME: &str = "lyrel";
pub(crate) const MAC_ORIGIN: &str = "lyrel://app";
pub(crate) const MAC_SHELL_URL: &str = "lyrel://app/drawio/shell.html";
pub(crate) const MAC_ENGINE_URL: &str = "lyrel://app/drawio/engine.html";

pub(crate) const MAX_DOCUMENT_BYTES: usize = 16 * 1024 * 1024;

const SHELL_CSP: &str = "default-src 'none'; script-src 'self'; style-src 'self'; \
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
    sha256: &'static str,
    license: &'static str,
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
        sha256: "d7641f884cb58a60f0a0a97b4441cb942f0e8e4a3e76b6aa8001ed67aaf4825e",
        license: "Lyrel Apache-2.0",
        csp: Some(SHELL_CSP),
    },
    Asset {
        path: "/drawio/shell.js",
        content_type: "text/javascript; charset=utf-8",
        bytes: SHELL_JS,
        sha256: "ee5288bc2030df0779f43271918d7f0d88db4bd33890b41ec9fdee195a214693",
        license: "Lyrel Apache-2.0",
        csp: None,
    },
    Asset {
        path: "/drawio/shell.css",
        content_type: "text/css; charset=utf-8",
        bytes: SHELL_CSS,
        sha256: "269e050a80e8117f8ff00a4df12dc71e1ef88bf7ba8ba1e70c4e1a44ec6d5f04",
        license: "Lyrel Apache-2.0",
        csp: None,
    },
    Asset {
        path: ENGINE_PATH,
        content_type: "text/html; charset=utf-8",
        bytes: ENGINE_HTML,
        sha256: "25d10235a995db1f858284270f49710dfe55546854cbdf3f48c2f99128071efc",
        license: "Lyrel Apache-2.0",
        csp: Some(ENGINE_CSP),
    },
    Asset {
        path: "/drawio/engine-boot.js",
        content_type: "text/javascript; charset=utf-8",
        bytes: ENGINE_BOOT_JS,
        sha256: "ca7a4157d42e6e66dc8b61e612d48f7f668aeee94ae9cb9b6bd5f5f29aa66a26",
        license: "Lyrel Apache-2.0",
        csp: None,
    },
    Asset {
        path: "/drawio/engine-bridge.js",
        content_type: "text/javascript; charset=utf-8",
        bytes: ENGINE_BRIDGE_JS,
        sha256: "f675fcc5c086186e149bdcf6f094bdf27446c74eec3f0d860029f0692b959eec",
        license: "Lyrel Apache-2.0",
        csp: None,
    },
    Asset {
        path: "/drawio/engine.css",
        content_type: "text/css; charset=utf-8",
        bytes: ENGINE_CSS,
        sha256: "460ca790e4caa7f330f1ed1191a09470d94838563931c34558192110027a78c9",
        license: "Lyrel Apache-2.0",
        csp: None,
    },
    Asset {
        path: "/drawio/viewer-static.min.js",
        content_type: "text/javascript; charset=utf-8",
        bytes: VIEWER_JS,
        sha256: "dfa1cfb4ef24320b67131405da88511e7baba48621ddd5260eea8ec5f6425928",
        license: "Apache-2.0 draw.io v32.6.10; see assets/drawio/THIRD-PARTY-NOTICES.md",
        csp: None,
    },
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
    pub(crate) reason: &'static str,
    pub(crate) content_type: &'static str,
    pub(crate) headers: Vec<(&'static str, String)>,
    pub(crate) body: EngineBody,
}

impl EngineResponse {
    fn forbidden() -> Self {
        Self {
            status: 403,
            reason: "Forbidden",
            content_type: "text/plain; charset=utf-8",
            headers: base_headers(),
            body: EngineBody::Static(b""),
        }
    }

    fn method_not_allowed() -> Self {
        let mut response = Self {
            status: 405,
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
        reason: "OK",
        content_type: asset.content_type,
        headers,
        body: EngineBody::Static(asset.bytes),
    }
}

/// Answers one WebView request. Any method other than a plain GET, any query
/// or fragment, any percent escape and any path not in the manifest is
/// refused. The active session's document is the one dynamic response.
pub(crate) fn respond(
    method: &str,
    target: &str,
    session: Option<&DocumentSession>,
) -> EngineResponse {
    respond_from(method, target, ORIGIN, session)
}

/// The same exact-manifest policy for a platform-specific origin. Windows
/// intercepts `ORIGIN`; macOS serves the same paths from a custom scheme.
pub(crate) fn respond_from(
    method: &str,
    target: &str,
    origin: &str,
    session: Option<&DocumentSession>,
) -> EngineResponse {
    if method != "GET" {
        return EngineResponse::method_not_allowed();
    }
    let Some(path) = target.strip_prefix(origin) else {
        return EngineResponse::forbidden();
    };
    if path.is_empty()
        || path.contains('?')
        || path.contains('#')
        || path.contains('%')
        || path.contains('\\')
        || path.contains("..")
        || path.contains("//")
    {
        return EngineResponse::forbidden();
    }
    if let Some(asset) = ASSETS.iter().find(|asset| asset.path == path) {
        return asset_response(asset);
    }
    if let Some((session_id, rest)) = document_target(path)
        && rest.is_empty()
    {
        let Some(session) = session.filter(|session| session.id == session_id) else {
            return EngineResponse::forbidden();
        };
        if session.body.len() > MAX_DOCUMENT_BYTES {
            return EngineResponse::too_large();
        }
        let mut headers = base_headers();
        headers.push((
            "Content-Security-Policy",
            "default-src 'none'; sandbox".to_string(),
        ));
        return EngineResponse {
            status: 200,
            reason: "OK",
            content_type: "application/xml; charset=utf-8",
            headers,
            body: EngineBody::Shared(session.body.clone()),
        };
    }
    EngineResponse::forbidden()
}

/// Splits `/drawio/session/<32 hex>/document` into the session and the empty
/// remainder. A revision never appears in the URL: replacing the document
/// creates a new session instead of letting an old URL address new bytes.
fn document_target(path: &str) -> Option<(SessionId, &str)> {
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
    let rest = rest.strip_prefix("document")?;
    Some((id, rest))
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
            (
                asset.path,
                asset.bytes,
                asset.sha256,
                asset.license,
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
