//! Engine-internal conformance: exact assets, the request policy, and the
//! native/shell/session handshake.
//!
//! These tests do not start a WebView or a GUI window. The real engine surface
//! and its policy hooks are exercised separately in a product window; this
//! suite pins the bytes, paths, CSP, token and sequence contract the native
//! handler relies on.

use std::sync::Arc;

use super::assets::{self, EngineBody, ENGINE_URL, SHELL_URL};
use super::bridge::{BridgeEvent, Command, LiveSession, MAX_MESSAGE_BYTES};
use super::document::{self, DiagramContent, RootKind};
use super::session::{DocumentSession, PreviewTheme, SessionId, SessionToken};

fn asset_bytes(path: &str) -> &'static [u8] {
    assets::asset_manifest()
        .into_iter()
        .find(|entry| entry.0 == path)
        .map(|entry| entry.1)
        .expect("manifest entry")
}

fn request(path: &str) -> assets::EngineResponse {
    assets::respond(
        "GET",
        &format!("{}{path}", assets::ORIGIN),
        None,
    )
}

#[test]
fn every_distributed_asset_matches_its_pinned_hash_and_has_a_license() {
    for (path, bytes, expected, license, csp) in assets::asset_manifest() {
        assert!(!bytes.is_empty(), "{path} is empty");
        assert_eq!(
            document::sha256_hex(bytes),
            expected,
            "{path} bytes changed without updating PROVENANCE"
        );
        assert!(!license.is_empty(), "{path} has no license record");
        assert!(
            path.starts_with("/drawio/"),
            "{path} lives outside the fixed prefix"
        );
        if path.ends_with(".html") {
            assert!(csp, "{path} must carry a response CSP");
        }
    }
    let viewer = asset_bytes("/drawio/viewer-static.min.js");
    assert!(viewer.len() > 8_000_000, "the real viewer bundle is present");
    assert!(
        viewer.windows(16).any(|w| w == b"DOMPurify 3.4.17"),
        "the vendored sanitizer version is the fixed one"
    );
    assert!(
        viewer.windows(12).any(|w| w == b"allowEval=!1"),
        "the viewer disables stencil eval"
    );
}

#[test]
fn distribution_notices_and_engine_provenance_are_present() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/drawio");
    let license = std::fs::read_to_string(root.join("LICENSE")).unwrap();
    assert!(license.starts_with("                                 Apache License"));
    let notices = std::fs::read_to_string(root.join("THIRD-PARTY-NOTICES.md")).unwrap();
    for expected in [
        "spin.js",
        "DOMPurify",
        "pako",
        "roughjs",
        "hachure-fill",
        "path-data-parser",
        "points-on-curve",
        "points-on-path",
        "mxGraph",
        "shapes",
        "stencils",
        "extensions.min.js",
    ] {
        assert!(notices.contains(expected), "notices omit {expected}");
    }
    for license_file in [
        "MIT-spin-2.0.0.txt",
        "Apache-2.0-DOMPurify-3.4.17.txt",
        "MIT-pako-2.2.0.txt",
        "Zlib-pako-2.2.0.txt",
        "MIT-roughjs-4.6.6.txt",
        "MIT-hachure-fill.txt",
        "MIT-path-data-parser.txt",
        "MIT-points-on-curve.txt",
        "MIT-points-on-path.txt",
        "Apache-2.0-mxgraph-4.2.2.txt",
    ] {
        assert!(
            root.join("LICENSES").join(license_file).is_file(),
            "missing distributed license {license_file}"
        );
    }
    let provenance = std::fs::read_to_string(root.join("PROVENANCE.json")).unwrap();
    let provenance: serde_json::Value = serde_json::from_str(&provenance).unwrap();
    assert_eq!(provenance["engine"]["commit"], "c72c9a98b7a6ce392556ed04c05e0ff19f43f5c3");
    assert_eq!(
        provenance["artifact"]["war_sha256"],
        "a57b06b3b6a876a351dacfe65d027d9509319cd030e9bacf6ae04a034d66a38f"
    );
    assert_eq!(
        provenance["artifact"]["viewer_sha256"],
        document::sha256_hex(asset_bytes("/drawio/viewer-static.min.js"))
    );
}

#[test]
fn the_shell_and_engine_carry_strict_csp_and_sandboxing() {
    let shell = request(assets::SHELL_PATH);
    assert_eq!(shell.status, 200);
    let shell_csp = assets::shell_csp();
    for directive in [
        "default-src 'none'",
        "script-src 'self'",
        "connect-src 'self'",
        "frame-src 'self'",
        "frame-ancestors 'none'",
        "worker-src 'none'",
        "object-src 'none'",
        "base-uri 'none'",
    ] {
        assert!(shell_csp.contains(directive), "shell CSP lacks {directive}");
    }
    assert!(!shell_csp.contains("unsafe-eval"));
    let engine_csp = assets::engine_csp();
    for directive in [
        "script-src 'self'",
        "style-src 'self' 'unsafe-inline'",
        "img-src 'self' data: blob:",
        "frame-ancestors 'self'",
    ] {
        assert!(engine_csp.contains(directive), "engine CSP lacks {directive}");
    }
    assert!(!engine_csp.contains("unsafe-eval"));

    let shell_js = std::str::from_utf8(asset_bytes("/drawio/shell.js")).unwrap();
    assert!(shell_js.contains("allow-scripts allow-same-origin"));
    for forbidden in [
        "allow-popups",
        "allow-top-navigation",
        "allow-downloads",
        "allow-forms",
        "allow-modals",
        "allow-pointer-lock",
    ] {
        assert!(!shell_js.contains(forbidden), "shell grants {forbidden}");
    }
    assert!(
        shell_js.contains("frame.contentWindow.postMessage(message, ORIGIN)"),
        "engine messages use the exact origin"
    );
    assert!(!shell_js.contains("postMessage(message, \"*\")"));
    let engine_js = std::str::from_utf8(asset_bytes("/drawio/engine-bridge.js")).unwrap();
    assert!(engine_js.contains("post({ v: 1, kind: kind"));
    assert!(engine_js.contains("event.origin !== ORIGIN"));
    let boot = std::str::from_utf8(asset_bytes("/drawio/engine-boot.js")).unwrap();
    assert!(!boot.contains("viewer.diagrams.net"));
    assert!(boot.contains("window.mxBasePath = origin"));
    assert!(boot.contains("pwa: \"0\""));
}

#[test]
fn requests_are_exact_get_manifest_only() {
    let shell = request(assets::SHELL_PATH);
    assert_eq!((shell.status, shell.content_type), (200, "text/html; charset=utf-8"));
    assert!(shell.headers.iter().any(|(name, _)| *name == "Content-Security-Policy"));
    assert_eq!(request("/drawio/viewer-static.min.js").status, 200);
    for refused in [
        "/drawio/missing.js",
        "/drawio/../secret",
        "/drawio/%2e%2e/secret",
        "/drawio//shell.html",
        "/drawio/shell.html?x=1",
        "/drawio/shell.html#x",
        "/drawio/shell.html/",
        "/other/shell.html",
        "/",
    ] {
        assert_eq!(request(refused).status, 403, "{refused} was not refused");
    }
    let post = assets::respond("POST", &format!("{}{}", assets::ORIGIN, assets::SHELL_PATH), None);
    assert_eq!(post.status, 405);
    let foreign = assets::respond("GET", "https://example.invalid/drawio/shell.html", None);
    assert_eq!(foreign.status, 403);
    let data = assets::respond("GET", "data:text/html,<script></script>", None);
    assert_eq!(data.status, 403);
}

#[test]
fn only_the_live_session_document_is_addressable() {
    let body: Arc<[u8]> = Arc::from(b"<mxfile><diagram id=\"a\" name=\"A\"/></mxfile>".as_slice());
    let session = DocumentSession::new(body).unwrap();
    let path = session.path();
    let response = assets::respond("GET", &format!("{}{path}", assets::ORIGIN), Some(&session));
    assert_eq!(response.status, 200);
    assert_eq!(response.content_type, "application/xml; charset=utf-8");
    assert!(matches!(response.body, EngineBody::Shared(_)));
    assert!(
        response
            .headers
            .iter()
            .any(|(name, value)| *name == "Content-Security-Policy" && value.contains("sandbox"))
    );
    let other = DocumentSession::new(Arc::from(b"<mxfile/>".as_slice())).unwrap();
    let refused = assets::respond("GET", &format!("{}{path}", assets::ORIGIN), Some(&other));
    assert_eq!(refused.status, 403);
    let missing = assets::respond("GET", &format!("{}{path}", assets::ORIGIN), None);
    assert_eq!(missing.status, 403);
    let oversized = DocumentSession::new(Arc::from(vec![b' '; assets::MAX_DOCUMENT_BYTES + 1])).unwrap();
    let too_large = assets::respond(
        "GET",
        &format!("{}{}", assets::ORIGIN, oversized.path()),
        Some(&oversized),
    );
    assert_eq!(too_large.status, 413);
}

#[test]
fn the_native_shell_handshake_retires_tokens_and_sequences() {
    let document: Arc<[u8]> = Arc::from(b"<mxfile/>".as_slice());
    let mut session = LiveSession::start(document).unwrap();
    assert!(session.bind_script(PreviewTheme::Dark).contains("\"theme\":\"dark\""));

    let message = |session: &LiveSession, seq: u64, kind: &str, payload: serde_json::Value| {
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
    };

    assert_eq!(
        session.accept("https://example.invalid/drawio/shell.html", b"{}"),
        Err(super::bridge::Reject("source"))
    );
    assert_eq!(
        session.accept(SHELL_URL, &vec![b'x'; MAX_MESSAGE_BYTES + 1]),
        Err(super::bridge::Reject("size"))
    );
    assert_eq!(
        session.accept(SHELL_URL, &message(&session, 1, "ready", serde_json::json!({}))),
        Ok(BridgeEvent::Ready)
    );
    assert_eq!(
        session.accept(
            SHELL_URL,
            &message(
                &session,
                2,
                "state",
                serde_json::json!({"page": 1, "pages": 3, "zoom": 1.25}),
            ),
        ),
        Ok(BridgeEvent::State {
            page: 1,
            pages: 3,
            zoom: 1.25
        })
    );
    let page = session.command(Command::Page(2)).unwrap();
    let page: serde_json::Value = serde_json::from_str(&page).unwrap();
    assert_eq!(page["cmd"], "page");
    assert_eq!(page["page"], 2);
    assert_eq!(page["seq"], 1);
    let fit = session.command(Command::Fit).unwrap();
    assert!(fit.contains("\"cmd\":\"fit\""));

    // A different surface cannot replay the old stream, even with a fresh
    // ready: the session id and token differ.
    let mut replacement = LiveSession::start(Arc::from(b"<mxfile/>".as_slice())).unwrap();
    assert_eq!(
        replacement.accept(
            SHELL_URL,
            &message(&session, 3, "state", serde_json::json!({"page":0,"pages":1,"zoom":1})),
        ),
        Err(super::bridge::Reject("session"))
    );
    let bind = replacement.bind_script(PreviewTheme::Light);
    assert!(bind.contains(&replacement.token_hex()));
    assert_ne!(bind, session.bind_script(PreviewTheme::Light));

    // A test-only identity pair can be constructed without exposing a random
    // token to production code.
    let fixed = LiveSession::for_test(
        Arc::from(b"<mxfile/>".as_slice()),
        SessionId::parse("0123456789abcdef0123456789abcdef").unwrap(),
        SessionToken::random().unwrap(),
    );
    assert_eq!(fixed.session_hex(), "0123456789abcdef0123456789abcdef");
    assert!(fixed.document_path().ends_with("/document"));
}

#[test]
fn representative_compressed_and_uncompressed_documents_parse() {
    let compressed = br#"<mxfile host="test"><diagram id="p1" name="Packed">rVZdb9owEP01SLwUKeTDBB5b6NqHqVK7TkXiCbkQa0mVxFnstN2/3zlOIAk0oFTCSnD33t3z2OfXjx9XBT8GqQ</diagram><diagram id="p2" name="Packed 2">eNpVj8FqAzEMRP9F5x5s2bK9WfpDaW5NwmY30G5TbA0t/fuuL+1UJLO8</diagram></mxfile>"#;
    let parsed = document::parse(compressed).unwrap();
    assert_eq!(parsed.root, RootKind::File);
    assert_eq!(parsed.pages.len(), 2);
    assert_eq!(parsed.pages[0].content, DiagramContent::Compressed);

    let plain = br#"<mxfile><diagram id="a" name="One"><mxGraphModel><root><mxCell id="0"/></root></mxGraphModel></diagram><diagram id="b"><mxGraphModel><root/></mxGraphModel></diagram></mxfile>"#;
    let parsed = document::parse(plain).unwrap();
    assert_eq!(parsed.pages.len(), 2);
    assert_eq!(parsed.pages[0].content, DiagramContent::Inline);
    assert_eq!(parsed.pages[0].name.as_deref(), Some("One"));
}

#[test]
fn engine_urls_are_reserved_and_never_resolve() {
    assert_eq!(assets::ORIGIN, "https://drawio.lyrel.invalid");
    assert_eq!(SHELL_URL, "https://drawio.lyrel.invalid/drawio/shell.html");
    assert_eq!(ENGINE_URL, "https://drawio.lyrel.invalid/drawio/engine.html");
    let paths: Vec<_> = assets::asset_paths().collect();
    assert!(paths.contains(&assets::SHELL_PATH));
    assert!(paths.contains(&assets::ENGINE_PATH));
}
