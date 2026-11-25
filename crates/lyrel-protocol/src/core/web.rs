//! Where one Side Panel preview address may go (#382).
//!
//! The rule is a pure function of the URL string, shared by the desktop's
//! browser policy, the CLI's `lyrel preview` verb and the daemon that
//! validates an agent's request. Keeping one classifier here is what keeps a
//! scheme the surface would refuse out of the request path: the GUI's own
//! policy still owns the extra paper cuts (typing completion, the blank page,
//! loopback origin), but it asks this module which top-level routes exist.
//!
//! Only HTTP and HTTPS are loadable. `about:blank` is the built-in blank page
//! the panel may show; every other scheme — local files, executables, data
//! URLs, custom protocols — is denied before a native host sees it.

/// What one address is allowed to do inside the Side Panel preview.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebRoute {
    /// An ordinary page load — `http:`, `https:` with a host.
    Load,
    /// The built-in blank page.
    Blank,
    /// Not a route the preview takes.
    Deny,
}

/// Classifies one address for the preview surface.
pub fn classify(address: &str) -> WebRoute {
    let trimmed = address.trim();
    if trimmed.is_empty() || is_blank(trimmed) {
        return WebRoute::Blank;
    }
    let Some(sep) = trimmed.find("://") else {
        // Typed input without a scheme is completed to http by the address
        // bar before it gets here; a string that reaches the surface
        // schemeless is not navigable.
        return WebRoute::Deny;
    };
    let scheme = &trimmed[..sep];
    let rest = &trimmed[sep + 3..];
    let authority = &rest[..rest.find(['/', '?', '#']).unwrap_or(rest.len())];
    match scheme.to_ascii_lowercase().as_str() {
        // A page needs a host to load: `http://` alone has nowhere to go and
        // is refused here rather than handed to a host.
        "http" | "https" if !authority.is_empty() => WebRoute::Load,
        _ => WebRoute::Deny,
    }
}

/// Whether `address` spells the built-in blank page.
pub fn is_blank(address: &str) -> bool {
    let lower = address.trim().to_ascii_lowercase();
    lower == "about:blank" || lower == "about:home"
}

/// Whether an agent may ask the workbench to open `address` as a preview.
///
/// The blank page is not a preview an agent can usefully request, so only a
/// loadable route qualifies.
pub fn is_preview_address(address: &str) -> bool {
    classify(address) == WebRoute::Load
}

/// Whether `path` (written with either `/` or `\` separators) names an HTML
/// preview file (`.html`, `.htm`, `.xhtml`, ASCII case-insensitive). `.svg`
/// uses the standalone native preview and is excluded here.
pub fn is_html_preview_str(path: &str) -> bool {
    let name = path.rsplit(['/', '\\']).next().unwrap_or("");
    let Some((stem, ext)) = name.rsplit_once('.') else {
        return false;
    };
    if stem.is_empty() {
        return false;
    }
    ext.eq_ignore_ascii_case("html")
        || ext.eq_ignore_ascii_case("htm")
        || ext.eq_ignore_ascii_case("xhtml")
}

/// Whether `path` names an HTML preview file (`.html`, `.htm`, `.xhtml`,
/// ASCII case-insensitive). `.svg` uses the standalone native preview and is
/// excluded here.
pub fn is_html_preview_path(path: &std::path::Path) -> bool {
    if let Some(s) = path.to_str() {
        return is_html_preview_str(s);
    }
    match path.extension().and_then(|ext| ext.to_str()) {
        Some(ext) => {
            ext.eq_ignore_ascii_case("html")
                || ext.eq_ignore_ascii_case("htm")
                || ext.eq_ignore_ascii_case("xhtml")
        }
        None => false,
    }
}

/// A validated agent preview target (#424).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PreviewTarget {
    /// An absolute `http(s)://` URL with an authority.
    Url(String),
    /// A workspace-local or absolute HTML file path (`.html`, `.htm`, `.xhtml`).
    HtmlFile(String),
}

const NON_HTML_PREVIEW_ERR: &str =
    "only .html, .htm, and .xhtml files can be previewed (non-HTML local files are not allowed)";

/// Validates an agent preview argument before any wire request is sent (#424).
///
/// Accepts `http(s)://` URLs with an authority, `.html`/`.htm`/`.xhtml`
/// filesystem paths (relative or absolute, POSIX or Windows), and local
/// `file://` URIs naming an HTML file. Refuses non-HTML files, remote
/// `file://` hosts, `about:blank`, and every other URI scheme.
pub fn parse_preview_target(input: &str) -> Result<PreviewTarget, String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(
            "preview target cannot be empty — pass an http(s) URL or a .html/.htm/.xhtml file path"
                .into(),
        );
    }
    if is_preview_address(trimmed) {
        return Ok(PreviewTarget::Url(trimmed.to_string()));
    }
    if trimmed.len() >= 7 && trimmed[..7].eq_ignore_ascii_case("file://") {
        let decoded = parse_local_file_uri(&trimmed[7..])?;
        if is_html_preview_str(&decoded) {
            return Ok(PreviewTarget::HtmlFile(decoded));
        }
        return Err(NON_HTML_PREVIEW_ERR.into());
    }
    if has_uri_scheme(trimmed) {
        return Err(
            "only http(s):// URLs and .html/.htm/.xhtml files (or file:// HTML URIs) are supported"
                .into(),
        );
    }
    if is_html_preview_str(trimmed) {
        Ok(PreviewTarget::HtmlFile(trimmed.to_string()))
    } else {
        Err(NON_HTML_PREVIEW_ERR.into())
    }
}

fn is_windows_drive_prefix(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b[2] == b'/' || b[2] == b'\\')
}

fn has_uri_scheme(s: &str) -> bool {
    if is_windows_drive_prefix(s) {
        return false;
    }
    if s.contains("://") {
        return true;
    }
    let before_slash = s.split(['/', '\\']).next().unwrap_or("");
    let Some((scheme, _)) = before_slash.split_once(':') else {
        return false;
    };
    !scheme.is_empty()
        && scheme.as_bytes()[0].is_ascii_alphabetic()
        && scheme
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.'))
}

fn parse_local_file_uri(rest: &str) -> Result<String, String> {
    let before_qf = rest.split(['?', '#']).next().unwrap_or("");
    let raw_path = if before_qf.starts_with('/') {
        before_qf
    } else if let Some(slash) = before_qf.find('/') {
        let authority = &before_qf[..slash];
        if !authority.eq_ignore_ascii_case("localhost") {
            return Err(
                "remote file:// hosts are not supported — pass a local file:// URI or workspace path"
                    .into(),
            );
        }
        &before_qf[slash..]
    } else if before_qf.is_empty() || before_qf.eq_ignore_ascii_case("localhost") {
        return Err(NON_HTML_PREVIEW_ERR.into());
    } else {
        return Err(
            "remote file:// hosts are not supported — pass a local file:// URI or workspace path"
                .into(),
        );
    };

    let decoded = percent_decode_utf8(raw_path)?;
    // `file:///C:/path/index.html` carries a leading `/` before the Windows drive letter.
    let bytes = decoded.as_bytes();
    let normalized = if bytes.len() >= 4
        && bytes[0] == b'/'
        && bytes[1].is_ascii_alphabetic()
        && bytes[2] == b':'
        && (bytes[3] == b'/' || bytes[3] == b'\\')
    {
        decoded[1..].to_string()
    } else {
        decoded
    };
    Ok(normalized)
}

fn percent_decode_utf8(input: &str) -> Result<String, String> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if i + 2 >= bytes.len() {
                return Err("invalid percent-encoding in file:// URI".into());
            }
            let hi = hex_nibble(bytes[i + 1])
                .ok_or_else(|| "invalid percent-encoding in file:// URI".to_string())?;
            let lo = hex_nibble(bytes[i + 2])
                .ok_or_else(|| "invalid percent-encoding in file:// URI".to_string())?;
            let byte = (hi << 4) | lo;
            if byte == 0 {
                return Err("NUL byte is not allowed in file:// URI".into());
            }
            out.push(byte);
            i += 3;
        } else {
            if bytes[i] == 0 {
                return Err("NUL byte is not allowed in file:// URI".into());
            }
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).map_err(|_| "file:// URI path is not valid UTF-8".to_string())
}

fn hex_nibble(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn http_and_https_with_a_host_load() {
        assert_eq!(classify("http://example.com"), WebRoute::Load);
        assert_eq!(classify("https://example.com/x?y=1"), WebRoute::Load);
        assert_eq!(classify("HTTP://EXAMPLE.COM"), WebRoute::Load);
        assert_eq!(classify("http://127.0.0.1:3000/app"), WebRoute::Load);
    }

    #[test]
    fn the_blank_page_is_its_own_route() {
        assert_eq!(classify("about:blank"), WebRoute::Blank);
        assert_eq!(classify(""), WebRoute::Blank);
        assert_eq!(classify("   "), WebRoute::Blank);
        assert!(!is_preview_address("about:blank"));
    }

    #[test]
    fn every_other_scheme_is_denied() {
        for denied in [
            "file:///etc/passwd",
            "file:///C:/Windows/system32/cmd.exe",
            "file:///tmp/index.html",
            "ftp://example.com/x",
            "data:text/html,<script>1</script>",
            "javascript:alert(1)",
            "vscode://file/x",
            "lyrel://anything",
            "http://",
            "https://",
            "localhost:3000",
        ] {
            assert_eq!(classify(denied), WebRoute::Deny, "{denied} must not load");
            assert!(!is_preview_address(denied), "{denied} is not a preview");
        }
    }

    #[test]
    fn an_agent_preview_requires_a_loadable_page() {
        assert!(is_preview_address("http://localhost:5173"));
        assert!(is_preview_address("https://example.com/preview"));
        assert!(!is_preview_address(""));
        assert!(!is_preview_address("about:blank"));
    }

    #[test]
    fn html_preview_paths_match_only_html_variants() {
        for matching in [
            "index.html",
            "page.HTM",
            "doc.Xhtml",
            "/remote/dir/app.html",
            r"C:\Users\me\prototype\index.htm",
            "a.b.html",
        ] {
            assert!(is_html_preview_str(matching), "{matching} should match");
            assert!(
                is_html_preview_path(Path::new(matching)),
                "{matching} path should match"
            );
        }
        for excluded in [
            "icon.svg",
            "ICON.SVG",
            "main.rs",
            "styles.css",
            "index.html.bak",
            ".html",
            "/dir/.html",
            r"C:\dir\.html",
            "html",
            "",
        ] {
            assert!(
                !is_html_preview_str(excluded),
                "{excluded} should not match"
            );
            assert!(
                !is_html_preview_path(Path::new(excluded)),
                "{excluded} path should not match"
            );
        }
    }

    #[test]
    fn parse_preview_target_accepts_urls_and_html_files() {
        assert_eq!(
            parse_preview_target("  https://example.com/preview?a=1  "),
            Ok(PreviewTarget::Url("https://example.com/preview?a=1".into()))
        );
        assert_eq!(
            parse_preview_target("http://127.0.0.1:5173/app"),
            Ok(PreviewTarget::Url("http://127.0.0.1:5173/app".into()))
        );
        assert_eq!(
            parse_preview_target("dist/index.html"),
            Ok(PreviewTarget::HtmlFile("dist/index.html".into()))
        );
        assert_eq!(
            parse_preview_target("/workspace/demo.HTM"),
            Ok(PreviewTarget::HtmlFile("/workspace/demo.HTM".into()))
        );
        assert_eq!(
            parse_preview_target(r"C:\Users\dev\app\index.xhtml"),
            Ok(PreviewTarget::HtmlFile(
                r"C:\Users\dev\app\index.xhtml".into()
            ))
        );
        assert_eq!(
            parse_preview_target("C:/Users/dev/app/index.html"),
            Ok(PreviewTarget::HtmlFile(
                "C:/Users/dev/app/index.html".into()
            ))
        );
        assert_eq!(
            parse_preview_target("file:///tmp/my%20site/index.html?v=2#top"),
            Ok(PreviewTarget::HtmlFile("/tmp/my site/index.html".into()))
        );
        assert_eq!(
            parse_preview_target("FILE://localhost/workspace/page.htm"),
            Ok(PreviewTarget::HtmlFile("/workspace/page.htm".into()))
        );
        assert_eq!(
            parse_preview_target("file:///C:/Users/dev/My%20App/index.html#frag"),
            Ok(PreviewTarget::HtmlFile(
                "C:/Users/dev/My App/index.html".into()
            ))
        );
    }

    #[test]
    fn parse_preview_target_rejects_non_html_files_and_unsupported_schemes() {
        assert!(parse_preview_target("   ").is_err());

        for non_html in [
            ".env",
            "src/main.rs",
            "/etc/passwd",
            "icon.svg",
            "file:///etc/passwd",
            "file:///tmp/icon.svg",
            "file:///C:/Windows/win.ini",
            r"C:\Users\dev\.env",
        ] {
            let err = parse_preview_target(non_html).expect_err(non_html);
            assert!(
                err.contains("only .html, .htm, and .xhtml"),
                "{non_html}: {err}"
            );
        }

        for bad_scheme in [
            "file://otherhost/tmp/index.html",
            "ftp://example.com/index.html",
            "data:text/html,<h1>hi</h1>",
            "javascript:alert(1)",
            "about:blank",
            "mailto:dev@example.com",
            "http://",
            "https://",
        ] {
            assert!(
                parse_preview_target(bad_scheme).is_err(),
                "{bad_scheme} must be rejected"
            );
        }

        // `classify` itself must still strictly deny `file://`.
        assert_eq!(classify("file:///tmp/index.html"), WebRoute::Deny);
    }
}
