//! Lyrel-owned ordinary file/URL navigation. Notebook/wiki constructs stay source-only.
//!
//! This module is pure resolution: it turns a raw link into a [`Target`] and
//! locates a heading anchor in document text. Following a link is split by
//! the editor inversion (#327): same-document anchors and external URLs stay
//! with the editor (`EditorPanelState::follow_link`), while locating the
//! presentation's tab/group and opening a file destination is shell work
//! (`ui::app::editor_links`).
use super::super::*;
use lyrel_core::host::Host;
use std::io;

pub(super) enum Target {
    External(url::Url),
    File(PathBuf, Option<String>),
    Anchor(String),
}

pub(super) fn parent(host: &dyn Host, path: &Path) -> io::Result<PathBuf> {
    if host.separator() == '/' {
        let path = path.to_str().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                t(L10nKey::MarkdownLinkInvalidDestination),
            )
        })?;
        let (parent, _) = path.rsplit_once('/').ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                t(L10nKey::MarkdownLinkSaveFirst),
            )
        })?;
        Ok(PathBuf::from(if parent.is_empty() { "/" } else { parent }))
    } else {
        path.parent().map(Path::to_path_buf).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                t(L10nKey::MarkdownLinkSaveFirst),
            )
        })
    }
}

pub(super) fn resolve(
    host: &dyn Host,
    document: Option<&Path>,
    raw: &str,
) -> Result<Target, L10nKey> {
    let raw = raw.trim();
    if raw.is_empty() || raw.chars().any(char::is_control) {
        return Err(L10nKey::MarkdownLinkInvalidDestination);
    }
    if let Some(anchor) = raw.strip_prefix('#') {
        return Ok(Target::Anchor(decode(anchor)?));
    }
    // A drive-letter path is not a URL scheme on a Windows owning Host.
    let windows_path = host.separator() == '\\' && host.is_absolute(Path::new(raw));
    if !windows_path && let Ok(url) = url::Url::parse(raw) {
        match url.scheme() {
            "http" | "https"
                if url.host_str().is_some()
                    && url.username().is_empty()
                    && url.password().is_none() =>
            {
                return Ok(Target::External(url));
            }
            "mailto" => return Ok(Target::External(url)),
            "file"
                if host.id().is_local()
                    && url.host_str().is_none_or(|host| host == "localhost") =>
            {
                let path = url
                    .to_file_path()
                    .map_err(|_| L10nKey::MarkdownLinkInvalidDestination)?;
                return Ok(Target::File(path, url.fragment().map(decode).transpose()?));
            }
            _ => return Err(L10nKey::MarkdownLinkUnsupportedScheme),
        }
    }
    if raw.starts_with("//") || raw.starts_with("\\\\") {
        return Err(L10nKey::MarkdownLinkUnsupportedScheme);
    }
    let (path, anchor) = raw
        .split_once('#')
        .map_or((raw, None), |(path, fragment)| (path, Some(fragment)));
    let path = decode(path)?;
    if host.separator() == '/' && (path.contains('\\') || path.contains(':')) {
        return Err(L10nKey::MarkdownLinkRemotePath);
    }
    let path = if host.is_absolute(Path::new(&path)) {
        PathBuf::from(path)
    } else {
        let document = document.ok_or(L10nKey::MarkdownLinkSaveFirst)?;
        let dir = parent(host, document).map_err(|_| L10nKey::MarkdownLinkInvalidDestination)?;
        host.join(&dir, &path)
    };
    Ok(Target::File(path, anchor.map(decode).transpose()?))
}
fn decode(value: &str) -> Result<String, L10nKey> {
    let value = percent_encoding::percent_decode_str(value)
        .decode_utf8()
        .map_err(|_| L10nKey::MarkdownLinkInvalidDestination)?
        .into_owned();
    if value.chars().any(char::is_control) {
        return Err(L10nKey::MarkdownLinkInvalidDestination);
    }
    Ok(value)
}

pub(super) fn anchor_position(source: &str, anchor: &str) -> Option<Position> {
    let mut seen: HashMap<String, usize> = HashMap::new();
    let mut fence: Option<(char, usize)> = None;
    for (row, line) in source.lines().enumerate() {
        let trimmed = line.trim_start();
        let first = trimmed.chars().next();
        if matches!(first, Some('`' | '~')) {
            let ch = first.unwrap();
            let count = trimmed.chars().take_while(|c| *c == ch).count();
            if count >= 3 {
                match fence {
                    None => fence = Some((ch, count)),
                    Some((c, n)) if ch == c && count >= n => fence = None,
                    _ => {}
                }
                continue;
            }
        }
        if fence.is_some() {
            continue;
        }
        let level = trimmed.chars().take_while(|c| *c == '#').count();
        if !(1..=6).contains(&level) || !trimmed[level..].starts_with(char::is_whitespace) {
            continue;
        }
        let title = trimmed[level..].trim().trim_end_matches('#').trim_end();
        let slug: String = title
            .chars()
            .flat_map(char::to_lowercase)
            .filter_map(|ch| {
                if ch.is_whitespace() {
                    Some('-')
                } else if ch.is_alphanumeric() || ch == '-' || ch == '_' {
                    Some(ch)
                } else {
                    None
                }
            })
            .collect();
        let count = seen.entry(slug.clone()).or_default();
        let unique = if *count == 0 {
            slug
        } else {
            format!("{slug}-{count}")
        };
        *count += 1;
        if unique == anchor {
            return Some(Position::new(row as u32, 0));
        }
    }
    None
}

/// Finds where a `#anchor` lands in the document at `path`, read on its
/// owning Host — never by opening a local lookalike. The 4 MiB cap matches
/// the bounded read the old link follower used.
pub(crate) fn locate_anchor(host: &dyn Host, path: &Path, anchor: &str) -> io::Result<Position> {
    let bytes = host.read_file(path, 4 * 1024 * 1024)?;
    let source = lyrel_document::text::decode_source(&bytes)?.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            t(L10nKey::MarkdownLinkInvalidDestination),
        )
    })?;
    anchor_position(&source.text, anchor).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            t(L10nKey::MarkdownLinkAnchorNotFound),
        )
    })
}
