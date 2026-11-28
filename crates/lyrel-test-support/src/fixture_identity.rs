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

/// Daemon and protocol packages whose complete sources shape the fixture.
pub const FIXTURE_IDENTITY_CRATES: &[&str] = &["crates/lyrel-core", "crates/lyrel-protocol"];

/// Additional files that define the fixture's observable behavior or build.
/// The daemon and protocol packages are hashed recursively.
pub const FIXTURE_IDENTITY_FILES: &[&str] = &[
    "crates/lyrel-test-support/src/fixture_identity.rs",
    "crates/lyrel-test-support/src/hex.rs",
    "crates/lyrel-test-support/src/pty.rs",
    "crates/lyrel-test-support/src/plaintext_pty.rs",
    "crates/lyrel-test-support/src/theme_pty.rs",
    "crates/lyrel-test-support/src/mouse_pty.rs",
    "crates/lyrel-test-support/src/glyph_pty.rs",
    "crates/lyrel-test-support/src/lib.rs",
    "crates/lyrel-test-support/src/main.rs",
    "crates/lyrel-test-support/build.rs",
    "crates/lyrel-test-support/Cargo.toml",
    "Cargo.lock",
    "assets/windows/conpty/manifest.json",
];

/// Hex SHA-256 of the fixture and its source packages, framed with
/// workspace-relative paths.
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

    let mut source_files = Vec::new();
    for relative in FIXTURE_IDENTITY_CRATES {
        collect_files(&root.join(relative), Path::new(relative), &mut source_files)?;
    }
    source_files.sort();
    for (relative, path) in source_files {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_source_changes_invalidate_a_prebuilt_fixture() {
        let sandbox = tempfile::tempdir().unwrap();
        let root = sandbox.path();
        for relative in FIXTURE_IDENTITY_FILES {
            let path = root.join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, b"fixture input").unwrap();
        }
        std::fs::create_dir_all(root.join("crates/lyrel-core")).unwrap();
        let protocol = root.join("crates/lyrel-protocol/src/daemon/protocol.rs");
        std::fs::create_dir_all(protocol.parent().unwrap()).unwrap();
        std::fs::write(&protocol, b"original wire implementation").unwrap();
        let built = fixture_source_identity(root).unwrap();

        std::fs::write(&protocol, b"updated wire implementation").unwrap();
        assert_ne!(built, fixture_source_identity(root).unwrap());
    }
}
