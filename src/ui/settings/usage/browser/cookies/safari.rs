//! Safari binarycookies 的 best-effort 读取；后台不访问可能受 TCC 保护的文件。

use super::{BrowserError, CookieProvider, Profile, Values, path_matches, store};
use zeroize::Zeroizing;

pub(super) fn read(
    reader: &super::BrowserReader,
    profile: &Profile,
    provider: CookieProvider,
    now: i64,
    interactive: bool,
) -> Result<Values, BrowserError> {
    // 文件访问没有 UISkip 等同的公开 TCC 保证；即使曾授权，后台也不探测文件。
    if !interactive {
        return Err(BrowserError::FullDiskAccess);
    }
    let path = profile.path.join("Cookies/Cookies.binarycookies");
    let bytes = (reader.safari_file)(&profile.path, &path, 16 * 1024 * 1024)
        .map_err(|error| if error == BrowserError::Denied {
            BrowserError::FullDiskAccess
        } else { error })?;
    parse(&bytes, provider, now)
}

fn word(bytes: &[u8], offset: usize, big: bool) -> Result<usize, BrowserError> {
    let end = offset.checked_add(4).ok_or(BrowserError::Unreadable)?;
    let raw: [u8; 4] = bytes.get(offset..end).ok_or(BrowserError::Unreadable)?
        .try_into().map_err(|_| BrowserError::Unreadable)?;
    Ok(if big { u32::from_be_bytes(raw) } else { u32::from_le_bytes(raw) } as usize)
}

fn text(bytes: &[u8], start: usize, end: usize, max: usize) -> Result<&str, BrowserError> {
    let bytes = bytes.get(start..end).ok_or(BrowserError::Unreadable)?;
    let length = bytes.iter().position(|byte| *byte == 0).ok_or(BrowserError::Unreadable)?;
    if length > max { return Err(BrowserError::Unreadable); }
    std::str::from_utf8(&bytes[..length]).map_err(|_| BrowserError::Unreadable)
}

fn parse(bytes: &[u8], provider: CookieProvider, now: i64) -> Result<Values, BrowserError> {
    if bytes.get(..4) != Some(b"cook".as_slice()) {
        return Err(BrowserError::Unsupported);
    }
    let pages = word(bytes, 4, true)?;
    if pages > 4096 { return Err(BrowserError::Unreadable); }
    let mut offset = 8 + pages * 4;
    if offset > bytes.len() { return Err(BrowserError::Unreadable); }
    let mut values = [None, None];
    let mut matches = 0;
    let mut total_records = 0;
    for index in 0..pages {
        let size = word(bytes, 8 + index * 4, true)?;
        let end = offset.checked_add(size).ok_or(BrowserError::Unreadable)?;
        let page = bytes.get(offset..end).ok_or(BrowserError::Unreadable)?;
        offset = end;
        if page.get(..4) != Some([0, 0, 1, 0].as_slice()) {
            return Err(BrowserError::Unsupported);
        }
        let count = word(page, 4, false)?;
        total_records += count;
        if total_records > 65_536 { return Err(BrowserError::Unreadable); }
        let table_end = 12 + count * 4;
        if table_end > page.len() || word(page, table_end - 4, false)? != 0 {
            return Err(BrowserError::Unreadable);
        }
        let mut previous_end = table_end;
        for cookie in 0..count {
            let start = word(page, 8 + cookie * 4, false)?;
            if start < previous_end { return Err(BrowserError::Unreadable); }
            let size = word(page, start, false)?;
            let end = start.checked_add(size).ok_or(BrowserError::Unreadable)?;
            let record = page.get(start..end).ok_or(BrowserError::Unreadable)?;
            previous_end = end;
            if size < 56 { return Err(BrowserError::Unreadable); }
            let fields = [word(record, 16, false)?, word(record, 20, false)?,
                word(record, 24, false)?, word(record, 28, false)?, size];
            if fields[0] < 56 || fields.windows(2).any(|pair| pair[0] >= pair[1]) {
                return Err(BrowserError::Unreadable);
            }
            let host = text(record, fields[0], fields[1], 256)?;
            let name = text(record, fields[1], fields[2], 256)?;
            if !provider.hosts().contains(&host) || !provider.cookie_names().contains(&name) {
                continue;
            }
            matches += 1;
            if matches > 32 { return Err(BrowserError::Unreadable); }
            let flags = word(record, 8, false)?;
            if flags & !5 != 0 { return Err(BrowserError::Unsupported); }
            let expiry = f64::from_le_bytes(record[40..48].try_into()
                .map_err(|_| BrowserError::Unreadable)?);
            if !expiry.is_finite() { return Err(BrowserError::Unreadable); }
            // Safari 的时间原点为 2001-01-01；零/负值也不猜作永久会话。
            if expiry + 978_307_200.0 <= now as f64 { continue; }
            let path = text(record, fields[2], fields[3], 4096)?;
            if !path_matches(provider, path) { continue; }
            let value = text(record, fields[3], fields[4], 64 * 1024)?;
            store(&mut values, provider, name, Zeroizing::new(value.to_owned()))?;
        }
    }
    Ok(values)
}

#[cfg(test)]
mod tests {
    use super::super::*;
    use super::super::super::StorageSource;

    fn fixture(root: &Path, host: &str, path: &str, expiry: f64) -> BrowserReader {
        let mut records = Vec::new();
        for (name, value) in [("api-platform_serviceToken", "safari-token"), ("userId", "user-17"), ("analytics", "must-not-import")] {
            let mut record = vec![0u8; 56];
            record[8..12].copy_from_slice(&5u32.to_le_bytes());
            record[40..48].copy_from_slice(&expiry.to_le_bytes());
            for (offset, text) in [(16, host), (20, name), (24, path), (28, value)] {
                let start = record.len() as u32;
                record[offset..offset + 4].copy_from_slice(&start.to_le_bytes());
                record.extend_from_slice(text.as_bytes());
                record.push(0);
            }
            let size = record.len() as u32;
            record[..4].copy_from_slice(&size.to_le_bytes());
            records.push(record);
        }
        let mut page = vec![0, 0, 1, 0];
        page.extend_from_slice(&(records.len() as u32).to_le_bytes());
        let mut offset = 12 + 4 * records.len();
        for record in &records {
            page.extend_from_slice(&(offset as u32).to_le_bytes());
            offset += record.len();
        }
        page.extend_from_slice(&[0; 4]);
        for record in records { page.extend_from_slice(&record); }
        let mut bytes = b"cook".to_vec();
        bytes.extend_from_slice(&1u32.to_be_bytes());
        bytes.extend_from_slice(&(page.len() as u32).to_be_bytes());
        bytes.extend_from_slice(&page);
        bytes.extend_from_slice(&[0; 8]);
        fs::create_dir_all(root.join("Cookies")).unwrap();
        fs::write(root.join("Cookies/Cookies.binarycookies"), bytes).unwrap();
        BrowserReader::with_root(Browser::Safari, root)
    }

    #[test]
    fn browser_reader_imports_binarycookies_and_background_never_opens_protected_file() {
        let root = tempfile::tempdir().unwrap();
        let mut reader = fixture(root.path(), ".xiaomimimo.com", "/", 4_000_000_000.0);
        let profile = reader.profiles(Browser::Safari).unwrap().remove(0);
        assert!(matches!(reader.import_for(&profile, StorageSource::DeepSeek), Err(BrowserError::Unsupported)));
        let (selection, material) = reader.import_cookies(&profile, CookieProvider::MiMo, true).unwrap();
        assert_eq!(material.secret(), "api-platform_serviceToken=safari-token; userId=user-17");
        assert!(!serde_json::to_string(&selection).unwrap().contains("safari-token"));
        reader.safari_file = |_, _, _| panic!("background must not open protected Safari files");
        assert!(matches!(reader.resolve_cookies(&selection, CookieProvider::MiMo), Err(BrowserError::FullDiskAccess)));
        assert!(matches!(reader.import_cookies(&profile, CookieProvider::MiMo, false), Err(BrowserError::FullDiskAccess)));
    }

    #[test]
    fn browser_reader_rejects_expired_wrong_scope_and_malformed_binarycookies() {
        for (host, path, expiry) in [(".xiaomimimo.com.evil.test", "/", 4_000_000_000.0),
            (".xiaomimimo.com", "/api/other", 4_000_000_000.0), (".xiaomimimo.com", "/", 1.0)] {
            let root = tempfile::tempdir().unwrap();
            let reader = fixture(root.path(), host, path, expiry);
            let profile = reader.profiles(Browser::Safari).unwrap().remove(0);
            assert!(matches!(reader.import_cookies(&profile, CookieProvider::MiMo, true), Err(BrowserError::NoSession)));
        }
        for bytes in [b"cook\0\0\0\x01\xff\xff\xff\xff".as_slice(), b"not-cookies", b"cook\xff\xff\xff\xff"] {
            let root = tempfile::tempdir().unwrap();
            let reader = fixture(root.path(), ".xiaomimimo.com", "/", 4_000_000_000.0);
            let profile = reader.profiles(Browser::Safari).unwrap().remove(0);
            fs::write(root.path().join("Cookies/Cookies.binarycookies"), bytes).unwrap();
            assert!(matches!(reader.import_cookies(&profile, CookieProvider::MiMo, true), Err(BrowserError::Unreadable | BrowserError::Unsupported)));
        }
    }

    #[test]
    fn browser_reader_reports_fda_denial_and_lock_at_external_file_seam() {
        let root = tempfile::tempdir().unwrap();
        let mut reader = fixture(root.path(), ".xiaomimimo.com", "/", 4_000_000_000.0);
        let profile = reader.profiles(Browser::Safari).unwrap().remove(0);
        reader.safari_file = |_, _, _| Err(BrowserError::Denied);
        assert!(matches!(reader.import_cookies(&profile, CookieProvider::MiMo, true), Err(BrowserError::FullDiskAccess)));
        reader.safari_file = |_, _, _| Err(BrowserError::Locked);
        assert!(matches!(reader.import_cookies(&profile, CookieProvider::MiMo, true), Err(BrowserError::Locked)));
    }
}
