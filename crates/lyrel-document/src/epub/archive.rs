//! ZIP access and archive-local path rules.
//!
//! Retained from Glim `books/mod.rs`: `by_name`, uncompressed-size checks,
//! symbolic-link refusal, bounded reads and the `resolve` algorithm whose
//! rejection tests cover absolute, remote, backslash and percent-encoded
//! traversal. Lyrel adds entry-count and total-uncompressed budgets, and maps
//! encrypted entries to the protected-content answer instead of calling them
//! merely damaged.

use std::io::{Read, Seek};

use zip::ZipArchive;

use super::{CancelToken, EpubError, Limits};

/// A bounded EPUB archive.
pub(crate) struct Archive<R: Read + Seek> {
    zip: ZipArchive<R>,
}

impl<R: Read + Seek> Archive<R> {
    /// Opens the container and validates its global shape before any entry is
    /// read. Encryption is refused here: a protected book must never expose a
    /// chapter list as if it were readable.
    pub(crate) fn open(source: R, limits: &Limits) -> Result<Self, EpubError> {
        let mut zip = ZipArchive::new(source).map_err(|error| match error {
            zip::result::ZipError::UnsupportedArchive(message)
                if message.to_ascii_lowercase().contains("encrypt") =>
            {
                EpubError::Protected {
                    detail: message.into(),
                }
            }
            other => EpubError::Corrupt {
                detail: format!("the archive could not be opened: {other}"),
            },
        })?;
        if zip.len() > limits.archive_entries {
            return Err(EpubError::limit(
                "archive entries",
                limits.archive_entries as u64,
            ));
        }
        let mut total = 0u64;
        for index in 0..zip.len() {
            let file = zip.by_index(index).map_err(|error| match error {
                zip::result::ZipError::UnsupportedArchive(message)
                    if message.to_ascii_lowercase().contains("encrypt") =>
                {
                    EpubError::Protected {
                        detail: message.into(),
                    }
                }
                other => EpubError::Corrupt {
                    detail: format!("the archive directory is invalid: {other}"),
                },
            })?;
            if file.encrypted() {
                return Err(EpubError::Protected {
                    detail: format!("ZIP entry {} is encrypted", file.name()),
                });
            }
            total = total.saturating_add(file.size());
            if total > limits.archive_uncompressed {
                return Err(EpubError::limit(
                    "uncompressed archive bytes",
                    limits.archive_uncompressed,
                ));
            }
        }
        Ok(Self { zip })
    }

    pub(crate) fn has(&self, name: &str) -> bool {
        self.zip.file_names().any(|entry| entry == name)
    }

    /// Reads one member under `limit` uncompressed bytes.
    pub(crate) fn entry(
        &mut self,
        name: &str,
        limit: u64,
        cancel: &CancelToken,
    ) -> Result<Vec<u8>, EpubError> {
        super::check_cancel(cancel)?;
        let mut file = self.zip.by_name(name).map_err(|error| match error {
            zip::result::ZipError::FileNotFound => EpubError::Missing {
                entry: name.to_owned(),
            },
            zip::result::ZipError::UnsupportedArchive(message)
                if message.to_ascii_lowercase().contains("encrypt") =>
            {
                EpubError::Protected {
                    detail: format!("{name}: {message}"),
                }
            }
            other => EpubError::Corrupt {
                detail: format!("{name}: {other}"),
            },
        })?;
        if file.encrypted() {
            return Err(EpubError::Protected {
                detail: format!("ZIP entry {name} is encrypted"),
            });
        }
        if file
            .unix_mode()
            .is_some_and(|mode| mode & 0o170000 == 0o120000)
        {
            return Err(EpubError::Corrupt {
                detail: format!("EPUB resource {name} is a symbolic link"),
            });
        }
        let declared = file.size();
        let local_limit = limit;
        if declared > local_limit {
            return Err(EpubError::limit("resource bytes", local_limit));
        }
        let mut bytes = Vec::with_capacity(declared.min(64 * 1024) as usize);
        let mut chunk = [0u8; 16 * 1024];
        let mut read = 0u64;
        loop {
            super::check_cancel(cancel)?;
            let count = file.read(&mut chunk)?;
            if count == 0 {
                break;
            }
            read = read.saturating_add(count as u64);
            if read > local_limit {
                return Err(EpubError::limit("resource bytes", local_limit));
            }
            bytes.extend_from_slice(&chunk[..count]);
        }
        Ok(bytes)
    }
}

/// Resolves only archive-local paths.
///
/// Relative `..` is allowed only while it stays inside the archive. Absolute
/// URLs, protocol-relative URLs, backslashes, NULs, percent-encoded traversal
/// and any remaining `:` are rejected; a fragment is preserved for anchors.
pub fn resolve(base: &str, href: &str) -> Option<String> {
    if href.starts_with("//") || href.contains(':') || href.contains('\\') || href.contains('\0') {
        return None;
    }
    let (path, fragment) = href
        .split_once('#')
        .map(|(p, f)| (p, Some(f)))
        .unwrap_or((href, None));
    let path = path.split('?').next().unwrap_or(path);
    let mut decoded = Vec::with_capacity(path.len());
    let bytes = path.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let hex = std::str::from_utf8(bytes.get(index + 1..index + 3)?).ok()?;
            decoded.push(u8::from_str_radix(hex, 16).ok()?);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    let path = String::from_utf8(decoded).ok()?;
    if path.contains([':', '\\', '\0']) {
        return None;
    }
    let mut components = if path.starts_with('/') || base.is_empty() {
        // An absolute reference, or a reference to a document with no base
        // path (META-INF/encryption.xml's CipherReference), resolves from the
        // archive root: there is no containing directory to drop.
        Vec::new()
    } else {
        base.split('/')
            .filter(|part| !part.is_empty())
            .map(str::to_owned)
            .collect::<Vec<_>>()
    };
    if !path.is_empty() {
        if !path.starts_with('/') && !base.is_empty() {
            components.pop();
        }
        for part in path.split('/') {
            match part {
                "" | "." => {}
                ".." => {
                    components.pop()?;
                }
                _ => components.push(part.to_owned()),
            }
        }
    }
    let mut result = components.join("/");
    if let Some(fragment) = fragment {
        result.push('#');
        result.push_str(fragment);
    }
    Some(result)
}

/// Splits a resolved href into its archive path and optional fragment.
pub(crate) fn split_fragment(href: &str) -> (&str, Option<&str>) {
    match href.split_once('#') {
        Some((path, fragment)) => (path, Some(fragment)),
        None => (href, None),
    }
}

/// The path part of a resolved href.
pub(crate) fn path_of(href: &str) -> &str {
    split_fragment(href).0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archive_paths_resolve_inside_the_book_and_every_escape_is_refused() {
        assert_eq!(
            resolve("OPS/Text/one.xhtml", "../Images/cover%20photo.png"),
            Some("OPS/Images/cover photo.png".into())
        );
        assert_eq!(
            resolve("OPS/one.xhtml", "#note"),
            Some("OPS/one.xhtml#note".into())
        );
        assert_eq!(
            resolve("OPS/one.xhtml", "two.xhtml?q=1#x"),
            Some("OPS/two.xhtml#x".into())
        );
        // META-INF/encryption.xml has no containing document: its relative
        // CipherReference resolves from the archive root.
        assert_eq!(
            resolve("", "OPS/Fonts/f.ttf"),
            Some("OPS/Fonts/f.ttf".into())
        );
        assert_eq!(
            resolve("", "OPS/Text/one.xhtml#frag"),
            Some("OPS/Text/one.xhtml#frag".into())
        );
        assert_eq!(resolve("", "../secret"), None);
        assert_eq!(
            resolve("", "/OPS/Fonts/f.ttf"),
            Some("OPS/Fonts/f.ttf".into())
        );
        for href in [
            "../../../secret",
            "file:///etc/passwd",
            "https://example.com/a",
            "//example.com/a",
            "%2e%2e/%2e%2e/%2e%2e/secret",
            r"..\secret",
            "OPS/%00",
        ] {
            assert!(resolve("OPS/Text/one.xhtml", href).is_none(), "{href}");
        }
    }

    #[test]
    fn split_fragment_keeps_the_anchor_out_of_the_resource_path() {
        assert_eq!(
            split_fragment("OPS/c1.xhtml#part"),
            ("OPS/c1.xhtml", Some("part"))
        );
        assert_eq!(split_fragment("OPS/c1.xhtml"), ("OPS/c1.xhtml", None));
    }
}
