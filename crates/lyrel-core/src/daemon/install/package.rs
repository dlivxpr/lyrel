//! Unpacking the Windows server release ZIP on the client.
//!
//! The Windows server asset is a ZIP ([`asset::ASSET_WINDOWS_X86_64`])
//! carrying the server plus the pinned
//! ConPTY set it cannot start a pane without. The archive's SHA-256 is
//! verified against the release's `checksums.txt` before this ever runs
//! (`ReleaseDownload`); what is left here is everything that digest cannot
//! say: the package holds exactly the files the release promises, under
//! exactly the flat names the installer stages, each expanding to the size
//! and CRC32 its central-directory entry declares. Anything else — a path
//! that is not a flat name, a link, a duplicate, an extra or missing file —
//! is refused, not unpacked.
//!
//! The reader is the same `async_zip` release the updater unpacks portable
//! GUI packages with, driven synchronously: the installer runs on a daemon
//! std thread, so `smol::block_on` bridges the reader's async API the same
//! way `lyrel-updater` does.

use smol::io::AsyncReadExt as _;

use super::asset;
use super::{InstallError, WindowsServerPackage};

/// One file may not expand past this. The real payload is tens of megabytes;
/// the cap is what stands between a hostile archive and the client's memory.
const MAX_FILE_BYTES: u64 = 256 * 1024 * 1024;

/// Nor the whole package past this.
const MAX_PACKAGE_BYTES: u64 = 512 * 1024 * 1024;

const READ_CHUNK: usize = 64 * 1024;

fn reject(asset: &'static str, reason: impl Into<String>) -> InstallError {
    InstallError::Package {
        asset,
        reason: reason.into(),
    }
}

/// The fixed layout name `name` spells, or `None` when the package contains
/// something the release never puts in it.
fn expected_name(name: &str) -> Option<&'static str> {
    if name == asset::WINDOWS_PACKAGE_EXE {
        return Some(asset::WINDOWS_PACKAGE_EXE);
    }
    asset::WINDOWS_PACKAGE_COMPANIONS
        .iter()
        .copied()
        .find(|companion| *companion == name)
}

pub fn unpack(asset: &'static str, bytes: Vec<u8>) -> Result<WindowsServerPackage, InstallError> {
    let mut archive = smol::block_on(async_zip::base::read1::seek::ZipArchiveReader::open(
        smol::io::Cursor::new(bytes),
    ))
    .map_err(|error| reject(asset, format!("it is not a readable ZIP: {error}")))?;
    // Owned copies: opening an entry borrows the archive mutably, so the
    // central directory cannot be read lazily out of it mid-loop.
    let cdrs = archive.cdrs().to_vec();

    let mut server: Option<Vec<u8>> = None;
    let mut companions: Vec<Option<Vec<u8>>> = vec![None; asset::WINDOWS_PACKAGE_COMPANIONS.len()];
    let mut expanded_bytes = 0u64;

    for (index, cdr) in cdrs.iter().enumerate() {
        let name = cdr
            .insecure_file_name
            .as_str()
            .ok_or_else(|| reject(asset, format!("entry {index}'s name is not readable")))?;
        // Rejected before the whitelist on purpose: a crafted `../…` or
        // drive-letter path deserves "unsafe", not the milder "unexpected".
        if name.contains(['/', '\\']) || name.contains(':') {
            return Err(reject(
                asset,
                format!("the package path is not a flat file name: {name:?}"),
            ));
        }
        let Some(expected) = expected_name(name) else {
            return Err(reject(
                asset,
                format!("the package contains a file the release never ships: {name:?}"),
            ));
        };
        if name.ends_with('/') {
            return Err(reject(
                asset,
                format!("the package contains a directory entry: {name}"),
            ));
        }
        // The old `ZipEntry::unix_permissions`: the high byte of `v_made_by`
        // names the host that wrote the attributes, and on Unix the mode sits
        // in the high half of `exter_attr`.
        let unix_mode = matches!(
            async_zip::AttributeCompatibility::try_from(cdr.cdrh.v_made_by >> 8),
            Ok(async_zip::AttributeCompatibility::Unix)
        )
        .then_some((cdr.cdrh.exter_attr >> 16) as u16);
        if unix_mode.is_some_and(|mode| mode & 0o170000 == 0o120000) {
            return Err(reject(
                asset,
                format!("the package contains a symbolic link: {name}"),
            ));
        }
        let slot = if expected == asset::WINDOWS_PACKAGE_EXE {
            &mut server
        } else {
            let position = asset::WINDOWS_PACKAGE_COMPANIONS
                .iter()
                .position(|companion| *companion == expected)
                .expect("expected_name only returns layout names");
            &mut companions[position]
        };

        let declared = cdr
            .uncompressed_size()
            .map_err(|error| reject(asset, format!("reading entry {name}: {error}")))?;
        if declared > MAX_FILE_BYTES {
            return Err(reject(
                asset,
                format!("{name} declares {declared} bytes; the limit is {MAX_FILE_BYTES}"),
            ));
        }
        expanded_bytes = expanded_bytes
            .checked_add(declared)
            .ok_or_else(|| reject(asset, "the package's expanded-size total overflowed"))?;
        if expanded_bytes > MAX_PACKAGE_BYTES {
            return Err(reject(
                asset,
                format!("the package expands past the {MAX_PACKAGE_BYTES} byte limit"),
            ));
        }

        let mut reader = smol::block_on(archive.file(index))
            .map_err(|error| reject(asset, format!("opening {name}: {error}")))?;
        let mut buffer = [0u8; READ_CHUNK];
        let mut out: Vec<u8> = Vec::with_capacity((declared as usize).min(READ_CHUNK * 16));
        smol::block_on(async {
            loop {
                let read = reader
                    .read(&mut buffer)
                    .await
                    .map_err(|error| format!("extracting {name}: {error}"))?;
                if read == 0 {
                    break;
                }
                out.extend_from_slice(&buffer[..read]);
                if out.len() as u64 > MAX_FILE_BYTES {
                    return Err(format!(
                        "{name} expands past the {MAX_FILE_BYTES} byte limit"
                    ));
                }
            }
            Ok::<_, String>(())
        })
        .map_err(|reason| reject(asset, reason))?;
        if out.len() as u64 != declared {
            return Err(reject(
                asset,
                format!("{name} expands to {} bytes, expected {declared}", out.len()),
            ));
        }
        // The reader itself checked the bytes against the entry's CRC32 and
        // declared sizes when the read hit EOF (`validate_file_on_eof` is on
        // by default), so a stream that got this far is already verified.
        if slot.replace(out).is_some() {
            return Err(reject(asset, format!("the package contains {name} twice")));
        }
    }

    let server = server
        .ok_or_else(|| reject(asset, format!("{} is missing", asset::WINDOWS_PACKAGE_EXE)))?;
    let mut files = Vec::with_capacity(asset::WINDOWS_PACKAGE_COMPANIONS.len());
    for (position, companion) in asset::WINDOWS_PACKAGE_COMPANIONS.iter().enumerate() {
        let bytes = companions[position]
            .take()
            .ok_or_else(|| reject(asset, format!("{companion} is missing")))?;
        files.push((*companion, bytes));
    }
    Ok(WindowsServerPackage {
        server,
        companions: files,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SERVER: &[u8] = b"MZ\x90\x00...lyrel-server.exe";
    const CONPTY_DLL: &[u8] = b"MZ\x90\x00...the pinned conpty.dll";
    const OPENCONSOLE: &[u8] = b"MZ\x90\x00...the pinned OpenConsole.exe";
    const CONPTY_LICENSE: &[u8] = b"MIT license notice for ConPTY";
    const LICENSE: &[u8] = b"Apache-2.0 license notice for lyrel";

    /// The release payload as `Compress-Archive` would lay it out: flat names,
    /// fixed order. Stored entries keep the fixture bytes inspectable.
    fn zip_bytes(entries: &[(&str, &[u8])]) -> Vec<u8> {
        smol::block_on(async {
            let mut output = Vec::new();
            {
                let mut writer = async_zip::base::write::ZipFileWriter::new(&mut output);
                for (name, bytes) in entries {
                    let options = async_zip::ZipEntryBuilder::new(
                        (*name).into(),
                        async_zip::Compression::Stored,
                    );
                    writer.write_entry_whole(options, bytes).await.unwrap();
                }
                writer.close().await.unwrap();
            }
            output
        })
    }

    fn release_zip() -> Vec<u8> {
        zip_bytes(&[
            (asset::WINDOWS_PACKAGE_EXE, SERVER),
            ("conpty.dll", CONPTY_DLL),
            ("OpenConsole.exe", OPENCONSOLE),
            ("LICENSE-ConPTY.txt", CONPTY_LICENSE),
            ("LICENSE.txt", LICENSE),
        ])
    }

    #[test]
    fn the_release_layout_unpacks_into_the_staging_order() {
        let package = unpack(asset::ASSET_WINDOWS_X86_64, release_zip()).expect("the payload");
        assert_eq!(package.server, SERVER);
        let companions: Vec<(&str, &[u8])> = package
            .companions
            .iter()
            .map(|(name, bytes)| (*name, bytes.as_slice()))
            .collect();
        assert_eq!(
            companions,
            vec![
                ("conpty.dll", CONPTY_DLL),
                ("OpenConsole.exe", OPENCONSOLE),
                ("LICENSE-ConPTY.txt", CONPTY_LICENSE),
                ("LICENSE.txt", LICENSE),
            ]
        );
        let expected_total = SERVER.len()
            + CONPTY_DLL.len()
            + OPENCONSOLE.len()
            + CONPTY_LICENSE.len()
            + LICENSE.len();
        assert_eq!(package.total_bytes(), expected_total as u64);
    }

    #[test]
    fn deflated_entries_unpack_the_same_way() {
        let bytes = smol::block_on(async {
            let mut output = Vec::new();
            {
                let mut writer = async_zip::base::write::ZipFileWriter::new(&mut output);
                for (name, payload) in [
                    (asset::WINDOWS_PACKAGE_EXE, SERVER),
                    ("conpty.dll", CONPTY_DLL),
                    ("OpenConsole.exe", OPENCONSOLE),
                    ("LICENSE-ConPTY.txt", CONPTY_LICENSE),
                    ("LICENSE.txt", LICENSE),
                ] {
                    let options = async_zip::ZipEntryBuilder::new(
                        name.into(),
                        async_zip::Compression::Deflate,
                    );
                    writer.write_entry_whole(options, payload).await.unwrap();
                }
                writer.close().await.unwrap();
            }
            output
        });
        let package = unpack(asset::ASSET_WINDOWS_X86_64, bytes).expect("deflated payload");
        assert_eq!(package.server, SERVER);
    }

    #[test]
    fn a_payload_corrupted_after_signing_fails_crc() {
        let mut bytes = release_zip();
        // Stored entries keep the payload verbatim in the archive, so flipping
        // a byte of the server's bytes leaves the central directory's CRC
        // naming the original.
        let at = bytes
            .windows(SERVER.len())
            .position(|window| window == SERVER)
            .expect("the stored payload is findable");
        bytes[at] ^= 0xFF;
        let err = unpack(asset::ASSET_WINDOWS_X86_64, bytes)
            .err()
            .expect("invalid Windows server package was accepted");
        assert!(matches!(err, InstallError::Package { .. }), "{err:?}");
        assert!(err.to_string().contains("CRC32"), "{err}");
    }

    #[test]
    fn an_escaping_path_is_refused_as_unsafe() {
        for name in ["../evil.exe", "..\\evil.exe", "sub/dir.exe", "C:\\evil.exe"] {
            let err = unpack(
                asset::ASSET_WINDOWS_X86_64,
                zip_bytes(&[(name, b"evil"), ("lyrel-server.exe", SERVER)]),
            )
            .err()
            .expect("invalid Windows server package was accepted");
            let InstallError::Package { reason, .. } = &err else {
                panic!("{name}: expected a package refusal, got {err:?}");
            };
            assert!(reason.contains("flat file name"), "{name}: {reason}");
        }
    }

    #[test]
    fn an_unknown_file_is_refused() {
        let err = unpack(
            asset::ASSET_WINDOWS_X86_64,
            zip_bytes(&[
                (asset::WINDOWS_PACKAGE_EXE, SERVER),
                ("conpty.dll", CONPTY_DLL),
                ("OpenConsole.exe", OPENCONSOLE),
                ("LICENSE-ConPTY.txt", CONPTY_LICENSE),
                ("LICENSE.txt", LICENSE),
                ("payload.exe", b"MZ not from the release"),
            ]),
        )
        .err()
        .expect("invalid Windows server package was accepted");
        assert!(err.to_string().contains("payload.exe"), "{err}");
    }

    #[test]
    fn a_missing_file_is_named() {
        let err = unpack(
            asset::ASSET_WINDOWS_X86_64,
            zip_bytes(&[
                (asset::WINDOWS_PACKAGE_EXE, SERVER),
                ("conpty.dll", CONPTY_DLL),
                ("LICENSE-ConPTY.txt", CONPTY_LICENSE),
                ("LICENSE.txt", LICENSE),
            ]),
        )
        .err()
        .expect("invalid Windows server package was accepted");
        assert!(err.to_string().contains("OpenConsole.exe"), "{err}");
    }

    #[test]
    fn a_duplicate_file_is_refused() {
        let err = unpack(
            asset::ASSET_WINDOWS_X86_64,
            zip_bytes(&[
                (asset::WINDOWS_PACKAGE_EXE, SERVER),
                ("conpty.dll", CONPTY_DLL),
                ("conpty.dll", b"MZ the other one"),
            ]),
        )
        .err()
        .expect("invalid Windows server package was accepted");
        assert!(err.to_string().contains("twice"), "{err}");
    }

    #[test]
    fn something_that_is_not_a_zip_is_refused() {
        let err = unpack(asset::ASSET_WINDOWS_X86_64, b"MZ the bare exe".to_vec())
            .err()
            .expect("invalid Windows server package was accepted");
        assert!(err.to_string().contains("not a readable ZIP"), "{err}");
    }
}
