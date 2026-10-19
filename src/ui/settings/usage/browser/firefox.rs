use super::{Browser, BrowserError, Profile, io_error, make_profile, utf16};
use rusqlite::{Connection, ErrorCode, OpenFlags, OptionalExtension};
use std::collections::HashMap;
use std::fs;
use std::path::Path;

pub(super) fn profiles(
    root: &Path,
    isolated: bool,
    output: &mut Vec<Profile>,
) -> Result<(), BrowserError> {
    let ini = match fs::read_to_string(root.join("profiles.ini")) {
        Ok(ini) => ini,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            // Only profile directory names are inspected, never login databases.
            let container = if root.join("Profiles").try_exists().map_err(io_error)? {
                root.join("Profiles")
            } else {
                root.to_owned()
            };
            for entry in fs::read_dir(container).map_err(io_error)? {
                let entry = entry.map_err(io_error)?;
                if entry.file_type().map_err(io_error)?.is_dir()
                    && entry.file_name().to_string_lossy().contains('.')
                {
                    output.push(make_profile(
                        Browser::Firefox,
                        entry.path(),
                        entry.file_name().to_string_lossy().into_owned(),
                    )?);
                }
            }
            return Ok(());
        }
        Err(error) => return Err(io_error(error)),
    };
    let mut sections: Vec<(String, HashMap<String, String>)> = Vec::new();
    for line in ini.lines() {
        let line = line.trim().trim_start_matches('\u{feff}');
        if line.starts_with(';') || line.starts_with('#') || line.is_empty() {
            continue;
        }
        if let Some(name) = line
            .strip_prefix('[')
            .and_then(|line| line.strip_suffix(']'))
        {
            sections.push((name.to_owned(), HashMap::new()));
        } else if let Some((key, value)) = line.split_once('=') {
            if let Some((_, values)) = sections.last_mut() {
                values.insert(key.trim().to_owned(), value.trim().to_owned());
            }
        }
    }
    let canonical_root = if isolated {
        Some(fs::canonicalize(root).map_err(io_error)?)
    } else {
        None
    };
    for (section, fields) in sections {
        if !section.starts_with("Profile") {
            continue;
        }
        let Some(path) = fields.get("Path") else {
            return Err(BrowserError::Unsupported);
        };
        let path = match fields.get("IsRelative").map(String::as_str) {
            Some("1") => root.join(path),
            Some("0") => Path::new(path).to_path_buf(),
            _ => return Err(BrowserError::Unsupported),
        };
        match fs::metadata(&path) {
            Ok(metadata) if metadata.is_dir() => {}
            Ok(_) => continue,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(io_error(error)),
        }
        if let Some(root) = canonical_root.as_ref() {
            if !fs::canonicalize(&path).map_err(io_error)?.starts_with(root) {
                return Err(BrowserError::Denied);
            }
        }
        let label = fields
            .get("Name")
            .cloned()
            .unwrap_or_else(|| section.clone());
        output.push(make_profile(Browser::Firefox, path, label)?);
    }
    Ok(())
}

pub(super) fn read(profile: &Path) -> Result<String, BrowserError> {
    // Exact HTTPS origin only. Container/partition origin attributes are not the
    // selected default origin and must not silently supply a different account.
    let path = profile.join("storage/default/https+++platform.deepseek.com/ls/data.sqlite");
    match fs::metadata(&path) {
        Ok(metadata) if metadata.is_file() => {}
        Ok(_) => return Err(BrowserError::Unreadable),
        Err(error) => return Err(io_error(error)),
    }
    // Do not use immutable=1: it silently ignores committed WAL records. SQLite
    // performs its own shared locking and reads a consistent snapshot in-place.
    // Neither the database nor its WAL is copied to a temporary disk location.
    let db = Connection::open_with_flags(
        &path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(sql_error)?;
    db.busy_timeout(std::time::Duration::ZERO)
        .map_err(sql_error)?;
    db.execute_batch("PRAGMA query_only=ON; PRAGMA temp_store=MEMORY;")
        .map_err(sql_error)?;
    let mut statement = db
        .prepare(
            "SELECT value, conversion_type, compression_type FROM data WHERE key = 'userToken'",
        )
        .map_err(|error| match error.sqlite_error_code() {
            Some(ErrorCode::Unknown) => BrowserError::Unsupported,
            _ => sql_error(error),
        })?;
    let value = statement
        .query_row([], |row| {
            Ok((
                row.get::<_, Vec<u8>>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })
        .optional()
        .map_err(sql_error)?
        .ok_or(BrowserError::NoSession)?;
    let (bytes, conversion, compression) = value;
    // Mozilla LSValue: decompress first, then conversion NONE=raw UTF16,
    // UTF16_UTF8=UTF8. These are not heuristic encoding prefixes.
    let bytes = match compression {
        0 => bytes,
        1 => {
            let length = snap::raw::decompress_len(&bytes).map_err(|_| BrowserError::Unreadable)?;
            if length > 64 * 1024 {
                return Err(BrowserError::Unreadable);
            }
            snap::raw::Decoder::new()
                .decompress_vec(&bytes)
                .map_err(|_| BrowserError::Unreadable)?
        }
        _ => return Err(BrowserError::Unsupported),
    };
    match conversion {
        0 => utf16(&bytes),
        1 => String::from_utf8(bytes).map_err(|_| BrowserError::Unreadable),
        _ => Err(BrowserError::Unsupported),
    }
}

fn sql_error(error: rusqlite::Error) -> BrowserError {
    match error.sqlite_error_code() {
        Some(
            ErrorCode::DatabaseBusy
            | ErrorCode::DatabaseLocked
            | ErrorCode::FileLockingProtocolFailed,
        ) => BrowserError::Locked,
        Some(
            ErrorCode::PermissionDenied
            | ErrorCode::AuthorizationForStatementDenied
            | ErrorCode::ReadOnly,
        ) => BrowserError::Denied,
        _ => BrowserError::Unreadable,
    }
}
