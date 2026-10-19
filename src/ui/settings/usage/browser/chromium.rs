use super::{Browser, BrowserError, Profile, io_error, make_profile, utf16};
use rusty_leveldb::{DB, in_memory};
use std::fs::{self, File};
use std::path::Path;

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

pub(super) fn read(profile: &Path) -> Result<String, BrowserError> {
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
        let mut source = File::open(entry.path()).map_err(io_error)?;
        let mut destination = options
            .env
            .open_writable_file(&memory_root.join(name))
            .map_err(|_| BrowserError::Unreadable)?;
        std::io::copy(&mut source, &mut destination).map_err(io_error)?;
    }
    let mut db = DB::open(memory_root, options).map_err(|_| BrowserError::Unreadable)?;
    // Both DOMString encodings are specified by Chromium. Never enumerate keys
    // or scan for a token-shaped value: exact origin + exact userToken only.
    let snapshot = db.get_snapshot();
    for key in [
        b"_https://platform.deepseek.com\0\x01userToken".as_slice(),
        b"_https://platform.deepseek.com\0\x00u\0s\0e\0r\0T\0o\0k\0e\0n\0".as_slice(),
    ] {
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
