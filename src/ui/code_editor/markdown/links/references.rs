//! Workspace references are read-only Host queries, not a persistent index.
use super::*;
use crate::ui::editor_text;

#[derive(Clone)]
pub(crate) struct Candidate {
    label: String,
    destination: String,
    prefix: bool,
}
impl Candidate {
    pub(crate) fn label(&self) -> &str { &self.label }
    pub(crate) fn destination(&self) -> &str { &self.destination }
}

fn index(source: &str, path: &Path) -> Result<Index, L10nKey> {
    let grammar = super::super::mdx::Grammar::for_path(Some(path));
    let root = super::super::parser::parse(source, grammar, true)
        .map_err(|_| L10nKey::MarkdownLinkInvalidDestination)?;
    Ok(Index::from_ast(source, &root))
}

fn text(host: &dyn Host, path: &Path) -> Result<String, L10nKey> {
    let bytes = host.read_file(path, 4 * 1024 * 1024).map_err(|_| L10nKey::MarkdownReferenceUnreadable)?;
    editor_text::decode_source(&bytes)
        .map_err(|_| L10nKey::MarkdownReferenceUnreadable)?
        .map(|decoded| decoded.text)
        .ok_or(L10nKey::MarkdownReferenceUnreadable)
}

/// The relative spelling is computed in the Host's path dialect, even on Windows.
fn relative(host: &dyn Host, owner: &Path, target: &Path) -> Option<String> {
    if host.id().is_local() && owner.components().next() != target.components().next() {
        return url::Url::from_file_path(target).ok().map(Into::into);
    }
    let owner = owner.to_str()?.replace(host.separator(), "/");
    let target = target.to_str()?.replace(host.separator(), "/");
    let (parent, _) = owner.rsplit_once('/')?;
    let from: Vec<_> = parent.split('/').collect();
    let to: Vec<_> = target.split('/').collect();
    let common = from.iter().zip(&to).take_while(|(a, b)| a == b).count();
    let mut parts = vec![".."; from.len() - common];
    parts.extend_from_slice(&to[common..]);
    let spelling = parts.join("/");
    Some(if spelling.split('/').next().is_some_and(|part| part.contains(':')) { format!("./{spelling}") } else { spelling })
}
fn encode(value: &str) -> String {
    percent_encoding::utf8_percent_encode(value, &percent_encoding::CONTROLS
        .add(b' ').add(b'#').add(b'?').add(b'%').add(b'(').add(b')').add(b'<').add(b'>').add(b'\\').add(b':').add(b'|').add(b'[').add(b']').add(b'"').add(b'\''))
        .to_string()
}
fn destination(value: &str) -> String {
    if value.starts_with("file://") { value.to_owned() } else { encode(value) }
}

pub(crate) fn query(host: &dyn Host, owner: &Path, roots: &[PathBuf], query: &str, anchors: bool, current: Option<&str>, cancelled: &std::sync::atomic::AtomicBool) -> Result<Vec<Candidate>, L10nKey> {
    let mut files = Vec::new();
    let mut pending = roots.to_vec();
    let mut seen = std::collections::HashSet::new();
    while let Some(directory) = pending.pop() {
        if cancelled.load(std::sync::atomic::Ordering::Relaxed) { return Ok(Vec::new()); }
        let canonical = host.canonicalize(&directory).map_err(|_| L10nKey::MarkdownReferenceUnreadable)?;
        if !seen.insert(canonical.clone()) { continue; }
        // Never traverse symlinked directories, or leave the captured workspace roots.
        for entry in host.read_dir(&canonical, Some(&canonical)).map_err(|_| L10nKey::MarkdownReferenceUnreadable)? {
            if entry.name == ".git" || entry.ignored || entry.is_symlink { continue; }
            let path = host.join(&directory, &entry.name);
            if entry.is_dir {
                pending.push(path);
            } else if super::super::super::is_markdown_path(&path) {
                files.push(path);
            }
        }
    }
    files.push(owner.to_path_buf());
    files.sort();
    files.dedup();
    let (file_query, fragment) = query.split_once('#').map_or((query, None), |(file, fragment)| (file, Some(fragment)));
    let needle = percent_encoding::percent_decode_str(file_query).decode_utf8().map_err(|_| L10nKey::MarkdownLinkInvalidDestination)?.to_lowercase();
    let anchor_needle = percent_encoding::percent_decode_str(fragment.unwrap_or(query)).decode_utf8().map_err(|_| L10nKey::MarkdownLinkInvalidDestination)?.to_lowercase();
    let mut out = Vec::new();
    let mut unreadable = false;
    let mut ambiguous = false;
    for path in files {
        if cancelled.load(std::sync::atomic::Ordering::Relaxed) { return Ok(Vec::new()); }
        let Some(spelling) = relative(host, owner, &path) else { continue; };
        let lower = spelling.to_lowercase();
        if !anchors && !lower.contains(&needle) { continue; }
        if fragment.is_some() || anchors {
            if file_query.is_empty() && !anchors && path != owner { continue; }
            let source = if path == owner {
                current.map(str::to_owned).map(Ok).unwrap_or_else(|| text(host, &path))
            } else { text(host, &path) };
            let source = match source { Ok(source) => source, Err(_) => { unreadable = true; continue; } };
            if cancelled.load(std::sync::atomic::Ordering::Relaxed) { return Ok(Vec::new()); }
            let index = index(&source, &path)?;
            for (anchor, _) in &index.headings {
                let lower = anchor.to_lowercase();
                if !lower.contains(&anchor_needle) { continue; }
                // Duplicate explicit IDs cannot be navigated unambiguously.
                if index.headings.iter().filter(|(id, _)| id == anchor).count() != 1 { ambiguous = true; continue; }
                out.push(Candidate { label: anchor.clone(), destination: format!("{}#{}", if path == owner { String::new() } else { destination(&spelling) }, encode(anchor)), prefix: lower.starts_with(&anchor_needle) });
            }
        } else {
            out.push(Candidate { label: spelling.clone(), destination: destination(&spelling), prefix: lower.starts_with(&needle) });
        }
    }
    out.sort_by(|a, b| {
        b.prefix.cmp(&a.prefix).then_with(|| a.label.cmp(&b.label)).then_with(|| a.destination.cmp(&b.destination))
    });
    out.truncate(24);
    if out.is_empty() && ambiguous { return Err(L10nKey::MarkdownReferenceAmbiguous); }
    if out.is_empty() && unreadable { return Err(L10nKey::MarkdownReferenceUnreadable); }
    Ok(out)
}

pub(crate) fn anchor_position(host: &dyn Host, path: &Path, fragment: &str, current: Option<String>) -> Result<lsp_types::Position, L10nKey> {
    let source = current.map(Ok).unwrap_or_else(|| text(host, path))?;
    let index = index(&source, path)?;
    let decoded = percent_encoding::percent_decode_str(fragment).decode_utf8().map_err(|_| L10nKey::MarkdownLinkInvalidDestination)?;
    let mut matches = index.headings.iter().filter(|(id, _)| id == decoded.as_ref());
    let at = matches.next().map(|(_, at)| *at).ok_or(L10nKey::MarkdownLinkAnchorNotFound)?;
    if matches.next().is_some() { return Err(L10nKey::MarkdownReferenceAmbiguous); }
    let before = &source[..at];
    Ok(lsp_types::Position::new(before.bytes().filter(|byte| *byte == b'\n').count() as u32,
        before.rsplit('\n').next().unwrap_or("").encode_utf16().count() as u32))
}

pub(crate) fn file_path(host: &dyn Host, path: &Path) -> Result<PathBuf, L10nKey> {
    match host.stat(path) {
        Ok(_) => return host.canonicalize(path).map_err(|_| L10nKey::MarkdownReferenceUnreadable),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && path.extension().is_none() => {},
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Err(L10nKey::MarkdownReferenceMissing),
        Err(_) => return Err(L10nKey::MarkdownReferenceUnreadable),
    }
    let mut found = None;
    for extension in ["md", "markdown", "mdx"] {
        let candidate = path.with_extension(extension);
        match host.stat(&candidate) {
            Ok(meta) if !meta.is_dir => {
                if found.is_some() { return Err(L10nKey::MarkdownReferenceAmbiguous); }
                found = Some(candidate);
            }
            Ok(_) => {},
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {},
            Err(_) => return Err(L10nKey::MarkdownReferenceUnreadable),
        }
    }
    let path = found.ok_or(L10nKey::MarkdownReferenceMissing)?;
    host.canonicalize(&path).map_err(|_| L10nKey::MarkdownReferenceUnreadable)
}
