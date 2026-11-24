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

#[cfg(test)]
mod tests {
    use super::*;

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
}
