//! Host-backed read-only loopback HTTP server for HTML preview (#420, ADR 0002).
//!
//! Keeps `file://` completely forbidden at the protocol and native WebView
//! layers while letting both local and remote workspaces preview `.html`,
//! `.htm`, and `.xhtml` files (along with relative static assets inside the
//! entry file's parent directory) over `127.0.0.1:<ephemeral>`.
//!
//! All filesystem checks, canonicalization, and reads go through [`SharedHost`]
//! — this module never touches `std::fs` directly.

use std::collections::HashMap;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use lyrel_core::core::web::is_html_preview_path;
use lyrel_ui_base::host_ops::{HostId, SharedHost};
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC};

/// Maximum number of external system-browser mounts kept alive simultaneously.
pub(crate) const EXTERNAL_MOUNT_CAP: usize = 32;

/// Upper bound for any single served asset (64 MiB).
const MAX_ASSET_BYTES: u64 = 64 * 1024 * 1024;

/// Maximum incoming HTTP request header size (64 KiB).
const MAX_REQUEST_HEADER_BYTES: usize = 64 * 1024;

/// Read/write socket timeout for loopback preview requests.
const SOCKET_TIMEOUT: Duration = Duration::from_secs(5);

/// Characters percent-encoded when building the entry file URL segment.
const SEGMENT_ENCODE_SET: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');

/// Process-wide Host-backed HTML preview loopback server.
///
/// Lazy-starts a single `127.0.0.1:0` listener on the first mount and shuts it
/// down automatically as soon as all in-app and external mounts are gone.
#[derive(Clone, Default)]
pub(crate) struct HtmlPreviewService {
    inner: Arc<Mutex<ServiceInner>>,
}

impl gpui_kit::Global for HtmlPreviewService {}

impl HtmlPreviewService {
    #[cfg(test)]
    pub(crate) fn is_running(&self) -> bool {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .server
            .is_some()
    }

    #[cfg(test)]
    pub(crate) fn bound_addr(&self) -> Option<SocketAddr> {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .server
            .as_ref()
            .map(|server| server.addr)
    }

    #[cfg(test)]
    pub(crate) fn active_mount_count(&self) -> usize {
        let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner.in_app.len() + inner.external.len()
    }

    /// Mounts `html_file` on `host` for an in-app Browser tab.
    ///
    /// Returns an RAII [`HtmlPreviewMount`] handle; dropping it revokes the
    /// mount's token and stops the loopback listener if no mounts remain.
    /// Must run off the UI thread because it performs blocking [`Host`] I/O.
    pub(crate) fn mount_in_app(
        &self,
        host: SharedHost,
        html_file: &Path,
    ) -> io::Result<HtmlPreviewMount> {
        let resolved = resolve_mount_target(&host, html_file)?;
        let token = mint_token();
        let entry = MountEntry {
            token: token.clone(),
            host,
            host_id: resolved.host_id,
            separator: resolved.separator,
            canonical_file: resolved.canonical_file.clone(),
            canonical_root: resolved.canonical_root.clone(),
            entry_name: resolved.entry_name.clone(),
        };

        let addr = {
            let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            let addr = inner.ensure_server(&self.inner)?;
            inner.in_app.insert(token.clone(), entry);
            addr
        };

        let origin = format!("http://{addr}");
        let encoded_name =
            percent_encoding::utf8_percent_encode(&resolved.entry_name, SEGMENT_ENCODE_SET);
        let url = format!("{origin}/p/{token}/{encoded_name}");

        Ok(HtmlPreviewMount {
            service: Arc::clone(&self.inner),
            token,
            url,
            origin,
            canonical_file: resolved.canonical_file,
            canonical_root: resolved.canonical_root,
            host_id: resolved.host_id,
            separator: resolved.separator,
        })
    }

    /// Mounts `html_file` on `host` for the external system browser.
    ///
    /// Deduplicates by `(host.id(), canonical_file)`, promotes existing hits to
    /// the most-recently-used slot, and evicts the least-recently-used entry
    /// when exceeding [`EXTERNAL_MOUNT_CAP`].
    /// Must run off the UI thread because it performs blocking [`Host`] I/O.
    pub(crate) fn mount_external(&self, host: SharedHost, html_file: &Path) -> io::Result<String> {
        let resolved = resolve_mount_target(&host, html_file)?;
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let addr = inner.ensure_server(&self.inner)?;

        if let Some(pos) = inner.external.iter().position(|entry| {
            entry.host_id == resolved.host_id
                && same_host_path(
                    &entry.canonical_file,
                    &resolved.canonical_file,
                    entry.separator,
                )
        }) {
            let mut existing = inner.external.remove(pos);
            // Refresh the SharedHost reference in case the connection was replaced.
            existing.host = host;
            let url = mount_url(addr, &existing.token, &existing.entry_name);
            inner.external.push(existing);
            return Ok(url);
        }

        let token = mint_token();
        let entry = MountEntry {
            token: token.clone(),
            host,
            host_id: resolved.host_id,
            separator: resolved.separator,
            canonical_file: resolved.canonical_file,
            canonical_root: resolved.canonical_root,
            entry_name: resolved.entry_name.clone(),
        };
        if inner.external_cap == 0 {
            return Err(io::Error::other("external mount cap is zero"));
        }
        while inner.external.len() >= inner.external_cap {
            inner.external.remove(0);
        }
        let url = mount_url(addr, &token, &resolved.entry_name);
        inner.external.push(entry);
        Ok(url)
    }

    /// Immediately removes every in-app and external mount belonging to
    /// `host_id` and stops the listener if no active mounts remain.
    pub(crate) fn purge_host(&self, host_id: HostId) {
        let stopped = {
            let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            inner.in_app.retain(|_, entry| entry.host_id != host_id);
            inner.external.retain(|entry| entry.host_id != host_id);
            inner.take_server_if_idle()
        };
        if let Some(server) = stopped {
            server.stop_and_join();
        }
    }
}

/// RAII handle for an active in-app HTML preview mount.
pub(crate) struct HtmlPreviewMount {
    service: Arc<Mutex<ServiceInner>>,
    token: String,
    url: String,
    origin: String,
    canonical_file: PathBuf,
    canonical_root: PathBuf,
    host_id: HostId,
    separator: char,
}

impl HtmlPreviewMount {
    pub(crate) fn url(&self) -> &str {
        &self.url
    }

    pub(crate) fn origin(&self) -> &str {
        &self.origin
    }

    pub(super) fn has_origin(&self, url: &str) -> bool {
        url.strip_prefix(&self.origin)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with(['/', '?', '#']))
    }

    pub(crate) fn canonical_file(&self) -> &Path {
        &self.canonical_file
    }

    pub(crate) fn host_id(&self) -> HostId {
        self.host_id
    }

    /// Translates a surface URL on this mount's `/p/{token}/...` prefix back to
    /// a clean Host path string so internal loopback URLs never leak into the
    /// address bar.
    pub(crate) fn display_path_for_url(&self, url: &str) -> Option<String> {
        let stripped = strip_query_and_fragment(url);
        let raw_rel = stripped
            .strip_prefix(self.origin.as_str())?
            .strip_prefix("/p/")?
            .strip_prefix(self.token.as_str())?
            .strip_prefix('/')?;
        let segments = decode_rel_segments(raw_rel).ok()?;
        if segments.is_empty() {
            return None;
        }
        let mut out = self.canonical_root.to_string_lossy().into_owned();
        for seg in segments {
            if !out.is_empty() && !out.ends_with(self.separator) && !out.ends_with('/') {
                out.push(self.separator);
            }
            out.push_str(&seg);
        }
        Some(out)
    }
}

impl Drop for HtmlPreviewMount {
    fn drop(&mut self) {
        let stopped = {
            let mut inner = self.service.lock().unwrap_or_else(|e| e.into_inner());
            inner.in_app.remove(&self.token);
            inner.take_server_if_idle()
        };
        if let Some(server) = stopped {
            server.stop_and_join();
        }
    }
}

struct ServiceInner {
    server: Option<RunningServer>,
    in_app: HashMap<String, MountEntry>,
    external: Vec<MountEntry>,
    external_cap: usize,
}

impl Default for ServiceInner {
    fn default() -> Self {
        Self::with_external_cap(EXTERNAL_MOUNT_CAP)
    }
}

impl ServiceInner {
    fn with_external_cap(external_cap: usize) -> Self {
        Self {
            server: None,
            in_app: HashMap::new(),
            external: Vec::new(),
            external_cap,
        }
    }

    fn ensure_server(&mut self, owner: &Arc<Mutex<ServiceInner>>) -> io::Result<SocketAddr> {
        if let Some(server) = &self.server {
            return Ok(server.addr);
        }
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let addr = listener.local_addr()?;
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let weak = Arc::downgrade(owner);
        let thread = std::thread::Builder::new()
            .name("lyrel-html-preview".into())
            .spawn(move || accept_loop(listener, thread_stop, weak))?;
        self.server = Some(RunningServer {
            addr,
            stop,
            thread: Some(thread),
        });
        Ok(addr)
    }

    fn take_server_if_idle(&mut self) -> Option<RunningServer> {
        if self.in_app.is_empty() && self.external.is_empty() {
            self.server.take()
        } else {
            None
        }
    }

    fn lookup_mount(&self, token: &str) -> Option<(SharedHost, PathBuf)> {
        if let Some(entry) = self.in_app.get(token) {
            return Some((Arc::clone(&entry.host), entry.canonical_root.clone()));
        }
        self.external
            .iter()
            .find(|entry| entry.token == token)
            .map(|entry| (Arc::clone(&entry.host), entry.canonical_root.clone()))
    }
}

impl Drop for ServiceInner {
    fn drop(&mut self) {
        if let Some(server) = self.server.take() {
            server.stop_and_join();
        }
    }
}

struct RunningServer {
    addr: SocketAddr,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl RunningServer {
    fn stop_and_join(mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect_timeout(&self.addr, Duration::from_millis(250));
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
    }
}

#[derive(Clone)]
struct MountEntry {
    token: String,
    host: SharedHost,
    host_id: HostId,
    separator: char,
    canonical_file: PathBuf,
    canonical_root: PathBuf,
    entry_name: String,
}

struct ResolvedMount {
    host_id: HostId,
    separator: char,
    canonical_file: PathBuf,
    canonical_root: PathBuf,
    entry_name: String,
}

fn resolve_mount_target(host: &SharedHost, html_file: &Path) -> io::Result<ResolvedMount> {
    if !is_html_preview_path(html_file) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "HTML preview requires a .html, .htm, or .xhtml file",
        ));
    }
    let sep = host.separator();
    let canonical_file = host.canonicalize(html_file)?;
    if !is_html_preview_path(&canonical_file) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "HTML preview target must resolve to a .html, .htm, or .xhtml file",
        ));
    }
    let meta = host.stat(&canonical_file)?;
    if meta.is_dir {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "HTML preview path points to a directory",
        ));
    }
    let (parent, entry_name) = split_parent_and_name(&canonical_file, sep).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "could not determine parent directory for HTML preview file",
        )
    })?;
    let canonical_root = host.canonicalize(&parent)?;
    if !is_within_root(&canonical_file, &canonical_root, sep) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "HTML preview file is outside its canonical parent directory",
        ));
    }
    Ok(ResolvedMount {
        host_id: host.id(),
        separator: sep,
        canonical_file,
        canonical_root,
        entry_name,
    })
}

fn mint_token() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

fn mount_url(addr: SocketAddr, token: &str, entry_name: &str) -> String {
    let encoded_name = percent_encoding::utf8_percent_encode(entry_name, SEGMENT_ENCODE_SET);
    format!("http://{addr}/p/{token}/{encoded_name}")
}

fn accept_loop(listener: TcpListener, stop: Arc<AtomicBool>, inner: Weak<Mutex<ServiceInner>>) {
    for stream in listener.incoming() {
        if stop.load(Ordering::SeqCst) {
            break;
        }
        let Ok(stream) = stream else {
            if stop.load(Ordering::SeqCst) {
                break;
            }
            continue;
        };
        let Some(service) = inner.upgrade() else {
            break;
        };
        let _ = std::thread::Builder::new()
            .name("lyrel-html-preview-req".into())
            .spawn(move || handle_connection(stream, &service));
    }
}

fn handle_connection(mut stream: TcpStream, service: &Arc<Mutex<ServiceInner>>) {
    let _ = stream.set_read_timeout(Some(SOCKET_TIMEOUT));
    let _ = stream.set_write_timeout(Some(SOCKET_TIMEOUT));

    let request = match read_request_line(&stream) {
        Ok(req) => req,
        Err(()) => {
            let _ = write_simple_response(&mut stream, 400, "Bad Request", &[]);
            return;
        }
    };

    let is_head = match request.method.as_str() {
        "GET" => false,
        "HEAD" => true,
        _ => {
            let _ = write_simple_response(
                &mut stream,
                405,
                "Method Not Allowed",
                &[("Allow", "GET, HEAD")],
            );
            return;
        }
    };

    let target = strip_query_and_fragment(&request.target);
    let Some(rest) = target.strip_prefix("/p/") else {
        let _ = write_simple_response(&mut stream, 404, "Not Found", &[]);
        return;
    };
    let Some((token, raw_rel)) = rest.split_once('/') else {
        let _ = write_simple_response(&mut stream, 404, "Not Found", &[]);
        return;
    };
    if token.is_empty() || raw_rel.is_empty() || raw_rel.ends_with('/') {
        let _ = write_simple_response(&mut stream, 404, "Not Found", &[]);
        return;
    }

    let Some((host, canonical_root)) = service
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .lookup_mount(token)
    else {
        let _ = write_simple_response(&mut stream, 404, "Not Found", &[]);
        return;
    };

    let segments = match decode_rel_segments(raw_rel) {
        Ok(segments) => segments,
        Err(RelPathError::Forbidden) => {
            let _ = write_simple_response(&mut stream, 403, "Forbidden", &[]);
            return;
        }
        Err(RelPathError::NotFound) => {
            let _ = write_simple_response(&mut stream, 404, "Not Found", &[]);
            return;
        }
    };

    let mut candidate = canonical_root.clone();
    for segment in &segments {
        candidate = host.join(&candidate, segment);
    }

    let canonical_target = match host.canonicalize(&candidate) {
        Ok(path) => path,
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            let _ = write_simple_response(&mut stream, 404, "Not Found", &[]);
            return;
        }
        Err(_) => {
            let _ = write_simple_response(&mut stream, 403, "Forbidden", &[]);
            return;
        }
    };

    if !is_within_root(&canonical_target, &canonical_root, host.separator()) {
        let _ = write_simple_response(&mut stream, 403, "Forbidden", &[]);
        return;
    }

    let meta = match host.stat(&canonical_target) {
        Ok(meta) => meta,
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            let _ = write_simple_response(&mut stream, 404, "Not Found", &[]);
            return;
        }
        Err(_) => {
            let _ = write_simple_response(&mut stream, 403, "Forbidden", &[]);
            return;
        }
    };
    if meta.is_dir {
        let _ = write_simple_response(&mut stream, 404, "Not Found", &[]);
        return;
    }

    let bytes = match host.read_file(&canonical_target, MAX_ASSET_BYTES) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            let _ = write_simple_response(&mut stream, 404, "Not Found", &[]);
            return;
        }
        Err(err) if err.kind() == io::ErrorKind::FileTooLarge => {
            let _ = write_simple_response(&mut stream, 413, "Payload Too Large", &[]);
            return;
        }
        Err(_) => {
            let _ = write_simple_response(&mut stream, 500, "Internal Server Error", &[]);
            return;
        }
    };

    let content_type = mime_for_path(&canonical_target);
    let _ = write_asset_response(&mut stream, content_type, &bytes, is_head);
}

struct ParsedRequestLine {
    method: String,
    target: String,
}

fn read_request_line(stream: &TcpStream) -> Result<ParsedRequestLine, ()> {
    let mut reader = BufReader::new(stream.try_clone().map_err(|_| ())?);
    let mut total = 0usize;
    let mut request_line = String::new();
    let n = (&mut reader)
        .take(MAX_REQUEST_HEADER_BYTES as u64)
        .read_line(&mut request_line)
        .map_err(|_| ())?;
    if n == 0 {
        return Err(());
    }
    total += n;

    // Drain remaining headers until the blank line so the client sees a clean
    // HTTP exchange before the response is sent.
    let mut header_line = String::new();
    loop {
        header_line.clear();
        let remaining = MAX_REQUEST_HEADER_BYTES.saturating_sub(total);
        if remaining == 0 {
            return Err(());
        }
        let read = (&mut reader)
            .take(remaining as u64)
            .read_line(&mut header_line)
            .map_err(|_| ())?;
        if read == 0 || header_line == "\r\n" || header_line == "\n" {
            break;
        }
        total += read;
    }

    let trimmed = request_line.trim_end_matches(['\r', '\n']);
    let mut parts = trimmed.split(' ');
    let method = parts.next().ok_or(())?;
    let target = parts.next().ok_or(())?;
    let version = parts.next().ok_or(())?;
    if method.is_empty()
        || target.is_empty()
        || !version.starts_with("HTTP/1.")
        || parts.next().is_some()
    {
        return Err(());
    }
    Ok(ParsedRequestLine {
        method: method.to_string(),
        target: target.to_string(),
    })
}

fn write_simple_response(
    stream: &mut TcpStream,
    status: u16,
    reason: &str,
    extra_headers: &[(&str, &str)],
) -> io::Result<()> {
    let mut head = format!(
        "HTTP/1.1 {status} {reason}\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nContent-Length: 0\r\nConnection: close\r\n"
    );
    for (name, value) in extra_headers {
        head.push_str(name);
        head.push_str(": ");
        head.push_str(value);
        head.push_str("\r\n");
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes())?;
    stream.flush()
}

fn write_asset_response(
    stream: &mut TcpStream,
    content_type: &str,
    bytes: &[u8],
    is_head: bool,
) -> io::Result<()> {
    let head = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nConnection: close\r\n\r\n",
        bytes.len()
    );
    stream.write_all(head.as_bytes())?;
    if !is_head {
        stream.write_all(bytes)?;
    }
    stream.flush()
}

fn strip_query_and_fragment(target: &str) -> &str {
    target.split(['?', '#']).next().unwrap_or(target)
}

enum RelPathError {
    Forbidden,
    NotFound,
}

fn decode_rel_segments(raw_rel: &str) -> Result<Vec<String>, RelPathError> {
    if raw_rel.contains('\\') || raw_rel.contains('\0') {
        return Err(RelPathError::Forbidden);
    }
    let mut segments = Vec::new();
    for raw_seg in raw_rel.split('/') {
        if raw_seg.is_empty() {
            continue;
        }
        let decoded = strict_percent_decode(raw_seg).map_err(|()| RelPathError::Forbidden)?;
        if decoded.contains('\0') || decoded.contains('\\') || decoded.contains('/') {
            return Err(RelPathError::Forbidden);
        }
        if decoded == ".." {
            return Err(RelPathError::Forbidden);
        }
        if decoded == "." {
            continue;
        }
        segments.push(decoded);
    }
    if segments.is_empty() {
        return Err(RelPathError::NotFound);
    }
    Ok(segments)
}

fn strict_percent_decode(input: &str) -> Result<String, ()> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if i + 2 >= bytes.len() {
                return Err(());
            }
            let hi = hex_nibble(bytes[i + 1]).ok_or(())?;
            let lo = hex_nibble(bytes[i + 2]).ok_or(())?;
            let byte = (hi << 4) | lo;
            if byte == 0 {
                return Err(());
            }
            out.push(byte);
            i += 3;
        } else if bytes[i] == 0 {
            return Err(());
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).map_err(|_| ())
}

fn hex_nibble(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

fn split_parent_and_name(path: &Path, sep: char) -> Option<(PathBuf, String)> {
    let raw = path.to_string_lossy();
    let trimmed = if sep == '\\' {
        raw.trim_end_matches(['\\', '/'])
    } else {
        raw.trim_end_matches('/')
    };
    let pos = if sep == '\\' {
        trimmed.rfind(['\\', '/'])
    } else {
        trimmed.rfind('/')
    }?;
    let name = &trimmed[pos + 1..];
    if name.is_empty() {
        return None;
    }
    let parent_part = &trimmed[..pos];
    let parent = if parent_part.is_empty() {
        PathBuf::from(sep.to_string())
    } else if sep == '\\' && parent_part.len() == 2 && parent_part.as_bytes()[1] == b':' {
        PathBuf::from(format!("{parent_part}\\"))
    } else {
        PathBuf::from(parent_part)
    };
    Some((parent, name.to_string()))
}

fn path_segments(raw: &str, sep: char) -> (bool, Vec<&str>) {
    if sep == '\\' {
        let unc = raw.starts_with(r"\\") || raw.starts_with("//");
        let segs = raw.split(['\\', '/']).filter(|s| !s.is_empty()).collect();
        (unc, segs)
    } else {
        let abs = raw.starts_with('/');
        let segs = raw.split('/').filter(|s| !s.is_empty()).collect();
        (abs, segs)
    }
}

fn is_within_root(target: &Path, root: &Path, sep: char) -> bool {
    let target_raw = target.to_string_lossy();
    let root_raw = root.to_string_lossy();
    let (target_kind, target_segs) = path_segments(&target_raw, sep);
    let (root_kind, root_segs) = path_segments(&root_raw, sep);
    if target_kind != root_kind || target_segs.len() <= root_segs.len() {
        return false;
    }
    root_segs
        .iter()
        .zip(target_segs.iter())
        .all(|(r, t)| segments_equal(r, t, sep))
}

fn same_host_path(a: &Path, b: &Path, sep: char) -> bool {
    let a_raw = a.to_string_lossy();
    let b_raw = b.to_string_lossy();
    let (a_kind, a_segs) = path_segments(&a_raw, sep);
    let (b_kind, b_segs) = path_segments(&b_raw, sep);
    a_kind == b_kind
        && a_segs.len() == b_segs.len()
        && a_segs
            .iter()
            .zip(b_segs.iter())
            .all(|(x, y)| segments_equal(x, y, sep))
}

fn segments_equal(a: &str, b: &str, sep: char) -> bool {
    if sep == '\\' {
        a.eq_ignore_ascii_case(b) || a.to_lowercase() == b.to_lowercase()
    } else {
        a == b
    }
}

fn mime_for_path(path: &Path) -> &'static str {
    let raw = path.to_string_lossy();
    let name = raw.rsplit(['/', '\\']).next().unwrap_or("");
    let Some((stem, ext)) = name.rsplit_once('.') else {
        return "application/octet-stream";
    };
    if stem.is_empty() {
        return "application/octet-stream";
    }
    match ext.to_ascii_lowercase().as_str() {
        "html" | "htm" => "text/html; charset=utf-8",
        "xhtml" => "application/xhtml+xml; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" => "application/json; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "wasm" => "application/wasm",
        "txt" => "text/plain; charset=utf-8",
        "xml" => "application/xml; charset=utf-8",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lyrel_core::host::local::LocalHost;
    use lyrel_core::host::{
        ContentLimits, ContentQuery, ContentResults, Entry, Host, Meta, Output, SearchHit,
        ShellInventory, WatchSub,
    };

    /// Simulated remote POSIX Host mapping `/...` onto a local tempdir while
    /// reporting `separator() == '/'` and a remote `HostId`.
    struct PosixSandboxHost {
        id: HostId,
        root: PathBuf,
        local: SharedHost,
    }

    impl PosixSandboxHost {
        fn shared(name: &str, root: PathBuf) -> SharedHost {
            Arc::new(Self {
                id: HostId::from_connection_key(name),
                root: LocalHost::new().canonicalize(&root).unwrap_or(root),
                local: LocalHost::new(),
            })
        }

        fn to_disk(&self, path: &Path) -> PathBuf {
            let s = path.to_string_lossy();
            let rel = s.trim_start_matches('/');
            let mut out = self.root.clone();
            for part in rel.split('/').filter(|p| !p.is_empty()) {
                out.push(part);
            }
            out
        }

        fn to_wire(&self, disk: &Path) -> io::Result<PathBuf> {
            let rel = disk
                .strip_prefix(&self.root)
                .map_err(|_| io::Error::new(io::ErrorKind::PermissionDenied, "outside sandbox"))?;
            let mut s = String::from("/");
            for (ix, comp) in rel.components().enumerate() {
                if ix > 0 {
                    s.push('/');
                }
                s.push_str(&comp.as_os_str().to_string_lossy());
            }
            Ok(PathBuf::from(s))
        }
    }

    impl Host for PosixSandboxHost {
        fn id(&self) -> HostId {
            self.id
        }

        fn separator(&self) -> char {
            '/'
        }

        fn is_absolute(&self, p: &Path) -> bool {
            p.to_string_lossy().starts_with('/')
        }

        fn read_dir(&self, dir: &Path, root: Option<&Path>) -> io::Result<Vec<Entry>> {
            self.local
                .read_dir(&self.to_disk(dir), root.map(|r| self.to_disk(r)).as_deref())
        }

        fn stat(&self, p: &Path) -> io::Result<Meta> {
            self.local.stat(&self.to_disk(p))
        }

        fn read_file(&self, p: &Path, max_bytes: u64) -> io::Result<Vec<u8>> {
            self.local.read_file(&self.to_disk(p), max_bytes)
        }

        fn canonicalize(&self, p: &Path) -> io::Result<PathBuf> {
            let canonical = self.local.canonicalize(&self.to_disk(p))?;
            self.to_wire(&canonical)
        }

        fn search(
            &self,
            _roots: &[PathBuf],
            _query: &str,
            _limit: usize,
            _max_dirs: usize,
            _show_hidden: bool,
        ) -> io::Result<Vec<SearchHit>> {
            Ok(Vec::new())
        }

        fn search_content(
            &self,
            _roots: &[PathBuf],
            _query: &ContentQuery,
            _limits: &ContentLimits,
        ) -> io::Result<ContentResults> {
            Ok(ContentResults::default())
        }

        fn write_file(&self, p: &Path, bytes: &[u8]) -> io::Result<Meta> {
            self.local.write_file(&self.to_disk(p), bytes)
        }

        fn create_file_new(&self, p: &Path) -> io::Result<()> {
            self.local.create_file_new(&self.to_disk(p))
        }

        fn create_dir(&self, p: &Path, recursive: bool) -> io::Result<()> {
            self.local.create_dir(&self.to_disk(p), recursive)
        }

        fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
            self.local.rename(&self.to_disk(from), &self.to_disk(to))
        }

        fn remove(&self, p: &Path, recursive: bool) -> io::Result<()> {
            self.local.remove(&self.to_disk(p), recursive)
        }

        fn repo_root(&self, _p: &Path) -> io::Result<Option<PathBuf>> {
            Ok(None)
        }

        fn git(&self, _cwd: &Path, _args: &[&str]) -> io::Result<Output> {
            Err(io::Error::new(io::ErrorKind::Unsupported, "no git"))
        }

        fn shells(&self) -> io::Result<ShellInventory> {
            Err(io::Error::new(io::ErrorKind::Unsupported, "no shells"))
        }

        fn watch(&self, _dirs: &[PathBuf]) -> io::Result<WatchSub> {
            Err(io::Error::new(io::ErrorKind::Unsupported, "no watch"))
        }

        fn is_connected(&self) -> bool {
            true
        }
    }

    struct HttpResponse {
        status: u16,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
    }

    impl HttpResponse {
        fn header(&self, name: &str) -> Option<&str> {
            self.headers
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(name))
                .map(|(_, v)| v.as_str())
        }
    }

    fn http_request(addr: SocketAddr, method: &str, path: &str) -> HttpResponse {
        let mut stream = TcpStream::connect(addr).expect("connect to loopback preview server");
        let req = format!("{method} {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
        stream.write_all(req.as_bytes()).unwrap();
        stream.flush().unwrap();

        let mut raw = Vec::new();
        stream.read_to_end(&mut raw).unwrap();
        let split = raw
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .expect("HTTP header separator");
        let head = String::from_utf8_lossy(&raw[..split]);
        let body = raw[split + 4..].to_vec();
        let mut lines = head.split("\r\n");
        let status_line = lines.next().expect("status line");
        let status: u16 = status_line
            .split_whitespace()
            .nth(1)
            .expect("status code")
            .parse()
            .expect("numeric status");
        let headers = lines
            .filter_map(|line| {
                let (k, v) = line.split_once(':')?;
                Some((k.trim().to_string(), v.trim().to_string()))
            })
            .collect();
        HttpResponse {
            status,
            headers,
            body,
        }
    }

    fn url_path(url: &str) -> &str {
        let rest = url.strip_prefix("http://").expect("http url");
        let slash = rest.find('/').expect("path");
        &rest[slash..]
    }

    #[test]
    fn lazy_startup_single_port_and_auto_shutdown() {
        let dir = tempfile::tempdir().unwrap();
        let a_dir = dir.path().join("a");
        let b_dir = dir.path().join("b");
        std::fs::create_dir_all(&a_dir).unwrap();
        std::fs::create_dir_all(&b_dir).unwrap();
        let a_html = a_dir.join("index.html");
        let b_html = b_dir.join("other.xhtml");
        std::fs::write(&a_html, "<h1>A</h1>").unwrap();
        std::fs::write(&b_html, "<html/>").unwrap();

        let service = HtmlPreviewService::default();
        assert!(!service.is_running());
        assert_eq!(service.bound_addr(), None);
        assert_eq!(service.active_mount_count(), 0);

        let host = LocalHost::new();
        let mount_a = service.mount_in_app(host.clone(), &a_html).unwrap();
        assert!(service.is_running());
        let addr_a = service.bound_addr().expect("listener bound");
        assert_eq!(service.active_mount_count(), 1);
        assert_eq!(mount_a.origin(), format!("http://{addr_a}"));

        let mount_b = service.mount_in_app(host, &b_html).unwrap();
        assert_eq!(
            service.bound_addr(),
            Some(addr_a),
            "all mounts share one port"
        );
        assert_eq!(service.active_mount_count(), 2);

        let res_a = http_request(addr_a, "GET", url_path(mount_a.url()));
        assert_eq!(res_a.status, 200);
        assert_eq!(res_a.body, b"<h1>A</h1>");

        drop(mount_a);
        assert!(
            service.is_running(),
            "listener stays up while mount_b is alive"
        );
        assert_eq!(service.active_mount_count(), 1);
        assert_eq!(
            http_request(addr_a, "GET", url_path(mount_b.url())).status,
            200
        );

        drop(mount_b);
        assert!(
            !service.is_running(),
            "listener stops automatically when all mounts drop"
        );
        assert_eq!(service.bound_addr(), None);
        assert_eq!(service.active_mount_count(), 0);
    }

    #[test]
    fn enforces_methods_tokens_mime_types_and_headers() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let html = root.join("app.html");
        std::fs::write(
            &html,
            "<!doctype html><script type=module src='app.mjs'></script>",
        )
        .unwrap();
        std::fs::write(root.join("style.css"), "body { color: red; }").unwrap();
        std::fs::write(root.join("app.js"), "console.log(1);").unwrap();
        std::fs::write(root.join("app.mjs"), "export default 1;").unwrap();
        std::fs::write(root.join("data.json"), "{\"ok\":true}").unwrap();
        std::fs::write(root.join("logo.png"), b"\x89PNG").unwrap();
        std::fs::write(root.join("icon.svg"), "<svg/>").unwrap();

        let service = HtmlPreviewService::default();
        let host = LocalHost::new();
        let mount = service.mount_in_app(host, &html).unwrap();
        let addr = service.bound_addr().unwrap();
        let entry_path = url_path(mount.url());
        let base_prefix = entry_path.rsplit_once('/').unwrap().0;

        // Invalid token -> 404
        let bad_token = http_request(addr, "GET", "/p/0000deadbeef/app.html");
        assert_eq!(bad_token.status, 404);
        assert_eq!(bad_token.header("Cache-Control"), Some("no-store"));

        // Non-GET/HEAD -> 405
        for method in ["POST", "PUT", "DELETE", "OPTIONS", "PATCH"] {
            let res = http_request(addr, method, entry_path);
            assert_eq!(res.status, 405, "{method} must return 405");
            assert_eq!(res.header("Allow"), Some("GET, HEAD"));
            assert_eq!(res.header("Cache-Control"), Some("no-store"));
        }

        // HEAD -> 200 with Content-Length and empty body
        let head = http_request(addr, "HEAD", entry_path);
        assert_eq!(head.status, 200);
        assert_eq!(
            head.header("Content-Type"),
            Some("text/html; charset=utf-8")
        );
        assert_eq!(head.header("Cache-Control"), Some("no-store"));
        assert_eq!(head.header("X-Content-Type-Options"), Some("nosniff"));
        assert!(head.body.is_empty(), "HEAD must omit body");

        // MIME types for sibling assets
        for (file, expected_mime) in [
            ("style.css", "text/css; charset=utf-8"),
            ("app.js", "text/javascript; charset=utf-8"),
            ("app.mjs", "text/javascript; charset=utf-8"),
            ("data.json", "application/json; charset=utf-8"),
            ("logo.png", "image/png"),
            ("icon.svg", "image/svg+xml"),
        ] {
            let res = http_request(addr, "GET", &format!("{base_prefix}/{file}?v=1#hash"));
            assert_eq!(res.status, 200, "asset {file}");
            assert_eq!(res.header("Content-Type"), Some(expected_mime));
            assert_eq!(res.header("Cache-Control"), Some("no-store"));
        }
    }

    #[test]
    fn blocks_traversal_and_symlink_escapes_and_rejects_non_html() {
        let outer = tempfile::tempdir().unwrap();
        let site = outer.path().join("site");
        let outside = outer.path().join("outside");
        std::fs::create_dir_all(&site).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        let html = site.join("index.html");
        let secret = outside.join("secret.txt");
        let svg = site.join("vector.svg");
        std::fs::write(&html, "ok").unwrap();
        std::fs::write(&secret, "top-secret").unwrap();
        std::fs::write(&svg, "<svg/>").unwrap();

        let service = HtmlPreviewService::default();
        let host = LocalHost::new();

        // Non-HTML (.svg) is rejected at mount time.
        assert!(service.mount_in_app(host.clone(), &svg).is_err());
        assert!(!service.is_running());

        let mount = service.mount_in_app(host, &html).unwrap();
        let addr = service.bound_addr().unwrap();
        let base_prefix = url_path(mount.url()).rsplit_once('/').unwrap().0;

        for traversal in [
            format!("{base_prefix}/../outside/secret.txt"),
            format!("{base_prefix}/%2e%2e/outside/secret.txt"),
            format!("{base_prefix}/%2E%2E%2Foutside/secret.txt"),
            format!("{base_prefix}/sub/..\\..\\outside\\secret.txt"),
            format!("{base_prefix}/%00index.html"),
            format!("{base_prefix}/%ZZ"),
        ] {
            let res = http_request(addr, "GET", &traversal);
            assert_eq!(res.status, 403, "traversal {traversal} must return 403");
        }

        // Symlink inside `site` pointing outside `site` must be rejected with 403.
        let escape_link = site.join("escape.txt");
        let symlink_created = {
            #[cfg(unix)]
            {
                std::os::unix::fs::symlink(&secret, &escape_link).is_ok()
            }
            #[cfg(windows)]
            {
                std::os::windows::fs::symlink_file(&secret, &escape_link).is_ok()
            }
        };
        if symlink_created {
            let res = http_request(addr, "GET", &format!("{base_prefix}/escape.txt"));
            assert_eq!(
                res.status, 403,
                "symlink escape outside parent dir must return 403"
            );
        }
    }

    #[test]
    fn simulated_remote_posix_host_serves_html_and_assets_on_windows() {
        let sandbox = tempfile::tempdir().unwrap();
        let app_dir = sandbox.path().join("project").join("dist");
        let assets_dir = app_dir.join("assets");
        let outside_dir = sandbox.path().join("secret");
        std::fs::create_dir_all(&assets_dir).unwrap();
        std::fs::create_dir_all(&outside_dir).unwrap();
        std::fs::write(
            app_dir.join("index.html"),
            "<link rel=stylesheet href='assets/app.css'>",
        )
        .unwrap();
        std::fs::write(assets_dir.join("app.css"), "h1 { margin: 0; }").unwrap();
        std::fs::write(outside_dir.join("leak.txt"), "nope").unwrap();

        let remote =
            PosixSandboxHost::shared("ssh-direct:dev@box:22", sandbox.path().to_path_buf());
        let service = HtmlPreviewService::default();
        let mount = service
            .mount_in_app(remote, Path::new("/project/dist/index.html"))
            .unwrap();
        assert_eq!(
            mount.canonical_file(),
            Path::new("/project/dist/index.html")
        );
        assert_eq!(mount.canonical_root.as_path(), Path::new("/project/dist"));
        assert_eq!(
            mount.display_path_for_url(mount.url()).as_deref(),
            Some("/project/dist/index.html")
        );

        let addr = service.bound_addr().unwrap();
        let base_prefix = url_path(mount.url()).rsplit_once('/').unwrap().0;
        let css = http_request(addr, "GET", &format!("{base_prefix}/assets/app.css"));
        assert_eq!(css.status, 200);
        assert_eq!(css.header("Content-Type"), Some("text/css; charset=utf-8"));
        assert_eq!(css.body, b"h1 { margin: 0; }");

        let traversal = http_request(addr, "GET", &format!("{base_prefix}/../../secret/leak.txt"));
        assert_eq!(traversal.status, 403);
    }

    #[test]
    fn external_mounts_deduplicate_evict_lru_and_purge_host() {
        let dir = tempfile::tempdir().unwrap();
        let local = LocalHost::new();
        let remote_dir = tempfile::tempdir().unwrap();
        let remote =
            PosixSandboxHost::shared("ssh-direct:dev@remote:22", remote_dir.path().to_path_buf());

        let mut files = Vec::new();
        for i in 0..=EXTERNAL_MOUNT_CAP {
            let file = dir.path().join(format!("page_{i}.html"));
            std::fs::write(&file, format!("page {i}")).unwrap();
            files.push(file);
        }
        std::fs::write(remote_dir.path().join("remote.html"), "remote").unwrap();

        let service = HtmlPreviewService::default();
        let url_0 = service.mount_external(local.clone(), &files[0]).unwrap();
        let url_0_again = service.mount_external(local.clone(), &files[0]).unwrap();
        assert_eq!(
            url_0, url_0_again,
            "same (host, canonical_file) must reuse external mount"
        );
        assert_eq!(service.active_mount_count(), 1);

        // Fill remaining capacity up to EXTERNAL_MOUNT_CAP (0 is at LRU, 1..32 push 0 out? Wait:
        // first mount 1..31 -> total 32, then touch 0 -> 1 is LRU, then mount 32 -> 1 evicted, 0 kept!).
        let mut urls = vec![url_0.clone()];
        for file in &files[1..EXTERNAL_MOUNT_CAP] {
            urls.push(service.mount_external(local.clone(), file).unwrap());
        }
        assert_eq!(service.active_mount_count(), EXTERNAL_MOUNT_CAP);

        // Touch file[0] so file[1] becomes the LRU entry.
        assert_eq!(
            service.mount_external(local.clone(), &files[0]).unwrap(),
            url_0
        );
        // Insert file[EXTERNAL_MOUNT_CAP], exceeding cap -> file[1] evicted.
        let url_last = service
            .mount_external(local, &files[EXTERNAL_MOUNT_CAP])
            .unwrap();
        assert_eq!(service.active_mount_count(), EXTERNAL_MOUNT_CAP);

        let addr = service.bound_addr().unwrap();
        assert_eq!(http_request(addr, "GET", url_path(&url_0)).status, 200);
        assert_eq!(
            http_request(addr, "GET", url_path(&urls[1])).status,
            404,
            "LRU entry must be evicted"
        );
        assert_eq!(http_request(addr, "GET", url_path(&url_last)).status, 200);

        // Mount remote file and verify purge_host removes only remote mounts.
        let remote_url = service
            .mount_external(remote.clone(), Path::new("/remote.html"))
            .unwrap();
        assert_eq!(http_request(addr, "GET", url_path(&remote_url)).status, 200);

        service.purge_host(remote.id());
        assert_eq!(
            http_request(addr, "GET", url_path(&remote_url)).status,
            404,
            "purged remote host mount must return 404 immediately"
        );
        assert!(service.is_running(), "local external mounts remain active");

        service.purge_host(HostId::LOCAL);
        assert!(
            !service.is_running(),
            "server shuts down when all mounts are purged"
        );
    }
}
