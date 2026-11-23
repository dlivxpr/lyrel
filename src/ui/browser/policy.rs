//! Where one browser address may go (#391). The policy is a pure function of
//! the URL string — no surface, no platform — so the allow/deny rules that
//! keep the preview inside the app flow are testable on their own and shared
//! by every host.
//!
//! Supported top-level routes are HTTP, HTTPS and the built-in blank page
//! (#382). Local file and executable URL schemes, and anything else the app
//! does not route, are denied before a host ever sees them; an external
//! protocol request reaches the OS only through the explicit user action on
//! the page chrome, never through navigation.

/// What one address is allowed to do inside the Side Panel preview.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Route {
    /// An ordinary page load — `http:`, `https:`.
    Load,
    /// The built-in blank page. Hosts treat it specially where the platform
    /// needs a spelled page (a static host HTML on macOS); `about:blank`
    /// answers on both.
    Blank,
    /// Not a route the preview takes. The navigation is cancelled and the
    /// reason is logged; nothing leaves the app.
    Deny,
}

/// The scheme separator every absolute URL spells; anything without it is
/// typed input, not a scheme claim.
const SCHEME_SEP: &str = "://";

/// Classifies one address for the browser surface.
pub(crate) fn classify(address: &str) -> Route {
    let trimmed = address.trim();
    if trimmed.is_empty() || is_blank(trimmed) {
        return Route::Blank;
    }
    let Some(sep) = trimmed.find(SCHEME_SEP) else {
        // Typed input without a scheme ("localhost:3000", "example.com/path")
        // is completed to http by the address bar before it gets here; a
        // string that reaches the surface schemeless is not navigable.
        return Route::Deny;
    };
    let scheme = &trimmed[..sep];
    match scheme.to_ascii_lowercase().as_str() {
        "http" | "https" => Route::Load,
        // `about:blank` has no `://`; handled above. Everything else —
        // file:, ftp:, data:, javascript:, custom schemes — stays inside the
        // app's deny path.
        _ => Route::Deny,
    }
}

/// The origin of `address` when it points at this machine's loopback. The
/// forwarded-endpoint guard compares exactly this value: the scheme, the
/// loopback host spelling and the port together are the origin web storage
/// is scoped to.
pub(crate) fn loopback_origin(address: &str) -> Option<String> {
    lyrel_terminal::loopback::parse_loopback_url(address).map(|loopback| loopback.origin())
}

/// Whether `address` spells the built-in blank page.
pub(crate) fn is_blank(address: &str) -> bool {
    let lower = address.trim().to_ascii_lowercase();
    lower == "about:blank" || lower == "about:home"
}

/// Completes what the user typed into an absolute URL the surface can load.
/// A bare host or host:port becomes `http://…`; an allowed absolute URL is
/// returned as typed; anything else comes back `None` and the surface is not
/// asked to navigate.
pub(crate) fn complete_typed(input: &str) -> Option<String> {
    let typed = input.trim();
    if typed.is_empty() {
        return None;
    }
    if is_blank(typed) {
        return Some("about:blank".to_string());
    }
    match classify(typed) {
        Route::Load => Some(typed.to_string()),
        Route::Blank => Some("about:blank".to_string()),
        Route::Deny => {
            // No scheme separator: complete it. A scheme the policy denies is
            // completed the same way only when it parses as host:port-free
            // input — it does not, so deny stands.
            if typed.contains(SCHEME_SEP) {
                return None;
            }
            let host = typed.strip_prefix("//").unwrap_or(typed);
            if host.is_empty() || host.contains(char::is_whitespace) {
                return None;
            }
            Some(format!("http://{host}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_and_https_load() {
        assert_eq!(classify("http://example.com"), Route::Load);
        assert_eq!(classify("https://example.com/x?y=1"), Route::Load);
        assert_eq!(classify("HTTP://EXAMPLE.COM"), Route::Load);
    }

    #[test]
    fn blank_is_the_builtin_page() {
        assert_eq!(classify("about:blank"), Route::Blank);
        assert_eq!(classify(""), Route::Blank);
        assert_eq!(classify("   "), Route::Blank);
    }

    #[test]
    fn file_executable_and_custom_schemes_are_denied() {
        for denied in [
            "file:///etc/passwd",
            "file:///C:/Windows/system32/cmd.exe",
            "ftp://example.com/x",
            "data:text/html,<script>1</script>",
            "javascript:alert(1)",
            "vscode://file/x",
            "lyrel://anything",
            "http://",
        ] {
            assert_eq!(classify(denied), Route::Deny, "{denied} must not load");
        }
    }

    #[test]
    fn schemeless_input_is_denied_at_the_surface() {
        // The address bar completes these before the surface sees them.
        assert_eq!(classify("localhost:3000"), Route::Deny);
        assert_eq!(classify("example.com/path"), Route::Deny);
    }

    #[test]
    fn typed_input_completes_to_http() {
        assert_eq!(
            complete_typed("localhost:3000").as_deref(),
            Some("http://localhost:3000")
        );
        assert_eq!(
            complete_typed("example.com/path").as_deref(),
            Some("http://example.com/path")
        );
        assert_eq!(
            complete_typed("https://example.com").as_deref(),
            Some("https://example.com")
        );
        assert_eq!(complete_typed("about:blank").as_deref(), Some("about:blank"));
        assert_eq!(complete_typed(""), None);
        assert_eq!(complete_typed("file:///etc/passwd"), None);
        assert_eq!(complete_typed("a b"), None);
    }
}
