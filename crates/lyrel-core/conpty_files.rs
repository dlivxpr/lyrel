//! Fixed ConPTY inputs shared by the build and Windows pane startup.
use std::io;
use std::path::Path;

use sha2::{Digest, Sha256};

const MANIFEST: &str = include_str!("../../assets/windows/conpty/manifest.json");
pub const PREPARE_HINT: &str = "run `pwsh scripts/prepare-conpty.ps1` for a source build, or restore the matching lyrel package";

pub fn architecture(arch: &str) -> Option<&'static str> {
    match arch {
        "x86_64" => Some("x64"),
        _ => None,
    }
}

/// The exact pinned bytes identify both architecture and package version.
/// The preparation script additionally checks PE versions and signatures.
/// `split_arch` distinguishes the prepared tree from an executable directory.
pub fn read_verified(
    dir: &Path,
    arch: &str,
    split_arch: bool,
) -> io::Result<[(&'static str, Vec<u8>); 3]> {
    read_verified_with_remedy(dir, arch, split_arch, PREPARE_HINT)
}

/// `remedy` closes every error: the build and an installed app point at
/// [`PREPARE_HINT`], while the updater — where the rejected input is a
/// downloaded release package, not a developer's prepare step — names the
/// release package instead.
pub fn read_verified_with_remedy(
    dir: &Path,
    arch: &str,
    split_arch: bool,
    remedy: &str,
) -> io::Result<[(&'static str, Vec<u8>); 3]> {
    let manifest: serde_json::Value = serde_json::from_str(MANIFEST)?;
    read_verified_manifest_with_remedy(dir, arch, split_arch, &manifest, remedy)
}

/// The manifest must be trusted: either the compiled-in source pin or the
/// target release pin authenticated with its entire archive. An adjacent,
/// independently supplied manifest is never a trust anchor.
pub fn read_verified_manifest_with_remedy(
    dir: &Path,
    arch: &str,
    split_arch: bool,
    manifest: &serde_json::Value,
    remedy: &str,
) -> io::Result<[(&'static str, Vec<u8>); 3]> {
    let read = |name: &'static str,
                expected: Option<&str>|
     -> io::Result<(&'static str, Vec<u8>)> {
        let expected = expected.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "ConPTY manifest is missing a digest",
            )
        })?;
        if expected.len() != 64 || !expected.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid ConPTY SHA-256",
            ));
        }
        let mut digest = [0; 32];
        for (i, byte) in digest.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&expected[i * 2..i * 2 + 2], 16)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        }
        let path = if split_arch && name != "LICENSE-ConPTY.txt" {
            dir.join(arch).join(name)
        } else {
            dir.join(name)
        };
        let bytes = std::fs::read(&path).map_err(|e| {
            io::Error::new(
                e.kind(),
                format!("ConPTY input {}: {e}; {remedy}", path.display()),
            )
        })?;
        if Sha256::digest(&bytes).as_slice() != digest {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "ConPTY input {} has a SHA-256 mismatch (wrong version, architecture, or damaged file); {remedy}",
                    path.display()
                ),
            ));
        }
        Ok((name, bytes))
    };
    Ok([
        read("conpty.dll", manifest["files"][arch]["conpty.dll"].as_str())?,
        read(
            "OpenConsole.exe",
            manifest["files"][arch]["OpenConsole.exe"].as_str(),
        )?,
        read("LICENSE-ConPTY.txt", manifest["licenseSha256"].as_str())?,
    ])
}
