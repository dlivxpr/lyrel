// Shared identity of every input that shapes the lightweight graphics fixture
// process (`lyrel-graphics-fixture` binary in `crates/lyrel-test-support`).
//
// The desktop consumer tests compare this identity with the one the fixture
// embeds at build time, so a binary left over from another revision — or
// built with a different source tree — is rebuilt instead of silently
// standing in for the current protocol. It is a staleness check, not a
// security boundary.

use std::io;
use std::path::{Path, PathBuf};

use sha2::Digest as _;

/// Files outside `crates/lyrel-core` that define the fixture's observable
/// protocol. Core sources are hashed recursively by
/// [`fixture_source_identity`], because the daemon the fixture hosts is core.
pub const FIXTURE_IDENTITY_FILES: &[&str] = &[
    "crates/lyrel-test-support/src/fixture_identity.rs",
    "crates/lyrel-test-support/src/hex.rs",
    "crates/lyrel-test-support/src/pty.rs",
    "crates/lyrel-test-support/src/theme_pty.rs",
    "crates/lyrel-test-support/src/mouse_pty.rs",
    "crates/lyrel-test-support/src/glyph_pty.rs",
    "crates/lyrel-test-support/src/main.rs",
    "crates/lyrel-test-support/build.rs",
    "crates/lyrel-test-support/Cargo.toml",
    "Cargo.lock",
    "assets/windows/conpty/manifest.json",
];

/// Hex SHA-256 over the fixed support files plus every file under
/// `crates/lyrel-core`, framed with its workspace-relative path.
pub fn fixture_source_identity(root: &Path) -> io::Result<String> {
    use std::fmt::Write as _;

    let mut hasher = sha2::Sha256::new();
    for relative in FIXTURE_IDENTITY_FILES {
        let path = root.join(relative);
        let bytes = std::fs::read(&path).map_err(|error| {
            io::Error::new(
                error.kind(),
                format!("fixture identity input {}: {error}", path.display()),
            )
        })?;
        hash_bytes(&mut hasher, Path::new(relative), &bytes);
    }

    let mut core_files = Vec::new();
    collect_files(
        &root.join("crates/lyrel-core"),
        Path::new("crates/lyrel-core"),
        &mut core_files,
    )?;
    core_files.sort();
    for (relative, path) in core_files {
        let bytes = std::fs::read(&path).map_err(|error| {
            io::Error::new(
                error.kind(),
                format!("fixture identity input {}: {error}", path.display()),
            )
        })?;
        hash_bytes(&mut hasher, &relative, &bytes);
    }

    let mut identity = String::with_capacity(64);
    for byte in hasher.finalize() {
        write!(&mut identity, "{byte:02x}").unwrap();
    }
    Ok(identity)
}

fn hash_bytes(hasher: &mut sha2::Sha256, relative: &Path, bytes: &[u8]) {
    let name = relative.to_string_lossy().replace('\\', "/");
    hasher.update((name.len() as u64).to_le_bytes());
    hasher.update(name.as_bytes());
    hasher.update((bytes.len() as u64).to_le_bytes());
    hasher.update(bytes);
}

fn collect_files(
    directory: &Path,
    relative: &Path,
    files: &mut Vec<(PathBuf, PathBuf)>,
) -> io::Result<()> {
    let mut entries: Vec<_> = std::fs::read_dir(directory)
        .map_err(|error| {
            io::Error::new(
                error.kind(),
                format!(
                    "fixture identity directory {}: {error}",
                    directory.display()
                ),
            )
        })?
        .collect::<Result<_, _>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let file_type = entry.file_type()?;
        let child = relative.join(entry.file_name());
        if file_type.is_dir() {
            // A package-local build directory can never be a protocol input.
            if entry.file_name().to_str() == Some("target") {
                continue;
            }
            collect_files(&entry.path(), &child, files)?;
        } else if file_type.is_file() {
            files.push((child, entry.path()));
        }
    }
    Ok(())
}
