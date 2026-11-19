use super::{Browser, BrowserError, Profile, io_error, make_profile, utf16};
use rusty_leveldb::{DB, in_memory};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::Path;

// Bound the whole recovery input, not just the selected token. These fixed
// desktop budgets include unreferenced tables/logs copied into the memory Env.
const MAX_FILES: usize = 512;
const MAX_FILE_BYTES: u64 = 16 * 1024 * 1024;
const MAX_TOTAL_BYTES: u64 = 64 * 1024 * 1024;

pub(super) fn profiles(
    browser: Browser,
    root: &Path,
    output: &mut Vec<Profile>,
) -> Result<(), BrowserError> {
    let metadata: serde_json::Value = match fs::read(root.join("Local State")) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|_| BrowserError::Unreadable)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => serde_json::Value::Null,
        Err(error) => return Err(io_error(error)),
    };
    let cache = metadata
        .pointer("/profile/info_cache")
        .and_then(|cache| cache.as_object());
    for entry in fs::read_dir(root).map_err(io_error)? {
        let entry = entry.map_err(io_error)?;
        if !entry.file_type().map_err(io_error)?.is_dir() {
            continue;
        }
        let name = entry.file_name();
        let Some(id) = name.to_str() else {
            continue;
        };
        if id != "Default"
            && !id.starts_with("Profile ")
            && !cache.is_some_and(|cache| cache.contains_key(id))
        {
            continue;
        }
        let label = cache
            .and_then(|cache| cache.get(id))
            .and_then(|profile| profile.get("name"))
            .and_then(|name| name.as_str())
            .filter(|name| !name.is_empty())
            .unwrap_or(id)
            .to_owned();
        output.push(make_profile(browser, entry.path(), label)?);
    }
    Ok(())
}

pub(super) fn read(profile: &Path, origin: &str, name: &str) -> Result<String, BrowserError> {
    let path = profile.join("Local Storage/leveldb");
    if !path.try_exists().map_err(io_error)? {
        // Chromium's new SQLite LocalStorage backend is a separate format, not
        // a reason to silently read another browser, profile, or credential.
        return Err(
            if profile
                .join("LocalStorage")
                .try_exists()
                .map_err(io_error)?
            {
                BrowserError::Unsupported
            } else {
                BrowserError::NoSession
            },
        );
    }
    if !fs::canonicalize(&path)
        .map_err(io_error)?
        .starts_with(profile)
    {
        return Err(BrowserError::Denied);
    }
    let lock = File::open(path.join("LOCK")).map_err(io_error)?;
    fs2::FileExt::try_lock_shared(&lock).map_err(|error| {
        if error.kind() == std::io::ErrorKind::WouldBlock
            || matches!(error.raw_os_error(), Some(32 | 33))
        {
            BrowserError::Locked
        } else {
            io_error(error)
        }
    })?;
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        // Chromium's POSIX backend uses fcntl record locks, while fs2 uses
        // flock. Honor both: a shared record lock conflicts with its writer.
        // SAFETY: flock is a plain C struct; zero is valid for its fields.
        let mut record: libc::flock = unsafe { std::mem::zeroed() };
        record.l_type = libc::F_RDLCK as _;
        record.l_whence = libc::SEEK_SET as _;
        // SAFETY: the descriptor stays open and record points to a valid flock.
        if unsafe { libc::fcntl(lock.as_raw_fd(), libc::F_SETLK, &record) } == -1 {
            let error = std::io::Error::last_os_error();
            return Err(
                if matches!(error.raw_os_error(), Some(libc::EAGAIN | libc::EACCES)) {
                    BrowserError::Locked
                } else {
                    io_error(error)
                },
            );
        }
    }

    // The engine needs manifest/table/log recovery, but its normal disk backend
    // writes files. Supply only a memory Env. No source file is opened writable,
    // and no secret database or extracted value is copied to temporary disk.
    let mut options = in_memory();
    options.create_if_missing = false;
    options.paranoid_checks = true;
    let memory_root = Path::new("lyrel-browser-local-storage-memory");
    let mut files = Vec::new();
    let mut expected_total = 0u64;
    for entry in fs::read_dir(&path).map_err(io_error)? {
        let entry = entry.map_err(io_error)?;
        if !entry.file_type().map_err(io_error)?.is_file() {
            continue;
        }
        let name = entry.file_name();
        let Some(name_str) = name.to_str() else {
            continue;
        };
        if name_str != "CURRENT"
            && !name_str.starts_with("MANIFEST-")
            && !matches!(
                entry.path().extension().and_then(|ext| ext.to_str()),
                Some("log" | "ldb" | "sst")
            )
        {
            continue;
        }
        let metadata = entry.metadata().map_err(io_error)?;
        let len = metadata.len();
        expected_total = expected_total
            .checked_add(len)
            .ok_or(BrowserError::Unreadable)?;
        if files.len() == MAX_FILES || len > MAX_FILE_BYTES || expected_total > MAX_TOTAL_BYTES {
            return Err(BrowserError::Unreadable);
        }
        files.push((entry.path(), name, len));
    }
    let mut total = 0u64;
    let mut buffer = [0u8; 16 * 1024];
    for (path, name, expected_len) in files {
        let mut source = File::open(path).map_err(io_error)?;
        let metadata = source.metadata().map_err(io_error)?;
        if !metadata.is_file() || metadata.len() != expected_len {
            return Err(BrowserError::Unreadable);
        }
        let mut destination = options
            .env
            .open_writable_file(&memory_root.join(name))
            .map_err(|_| BrowserError::Unreadable)?;
        let mut copied = 0u64;
        loop {
            let count = source.read(&mut buffer).map_err(io_error)?;
            if count == 0 {
                break;
            }
            copied += count as u64;
            total += count as u64;
            // Recheck actual reads before writing to the memory Env. A file
            // growing after stat cannot exceed either budget or its proof.
            if copied > expected_len || copied > MAX_FILE_BYTES || total > MAX_TOTAL_BYTES {
                return Err(BrowserError::Unreadable);
            }
            destination.write_all(&buffer[..count]).map_err(io_error)?;
        }
        if copied != expected_len || source.metadata().map_err(io_error)?.len() != expected_len {
            return Err(BrowserError::Unreadable);
        }
    }
    let mut db = DB::open(memory_root, options).map_err(|_| BrowserError::Unreadable)?;
    // Read only the explicitly allowlisted DOMString; never enumerate stored keys.
    let snapshot = db.get_snapshot();
    let mut latin = format!("_{origin}\0\x01").into_bytes();
    latin.extend_from_slice(name.as_bytes());
    let mut wide = format!("_{origin}\0\x00").into_bytes();
    for word in name.encode_utf16() {
        wide.extend_from_slice(&word.to_le_bytes());
    }
    for key in [&latin[..], &wide[..]] {
        if let Some(value) = db
            .get_at(&snapshot, key)
            .map_err(|_| BrowserError::Unreadable)?
        {
            return match value.split_first() {
                Some((0, bytes)) => utf16(bytes),
                Some((1, bytes)) => Ok(bytes.iter().map(|byte| char::from(*byte)).collect()),
                _ => Err(BrowserError::Unsupported),
            };
        }
    }
    Err(BrowserError::NoSession)
}
