use super::*;
use rusty_leveldb::{DB, Options};
use std::fs;

fn chromium_profile(root: &Path, name: &str, token: &str) -> PathBuf {
    let profile = root.join(name);
    let storage = profile.join("Local Storage/leveldb");
    fs::create_dir_all(&storage).unwrap();
    let mut db = DB::open(&storage, Options::default()).unwrap();
    db.put(
        b"_https://platform.deepseek.com\0\x01userToken",
        format!("\x01{token}").as_bytes(),
    )
    .unwrap();
    db.put(
        b"_https://platform.deepseek.com.evil.test\0\x01userToken",
        b"\x01wrong-origin",
    )
    .unwrap();
    db.put(
        b"_http://platform.deepseek.com\0\x01userToken",
        b"\x01wrong-scheme",
    )
    .unwrap();
    db.flush().unwrap();
    profile
}

fn firefox_profile(root: &Path, value: &[u8], conversion: i64, compression: i64) -> PathBuf {
    let profile = root.join("Profiles/test.default-release");
    let storage = profile.join("storage/default/https+++platform.deepseek.com/ls");
    fs::create_dir_all(&storage).unwrap();
    fs::write(
        root.join("profiles.ini"),
        "[Profile0]\nName=Work Firefox\nIsRelative=1\nPath=Profiles/test.default-release\n",
    )
    .unwrap();
    let db = rusqlite::Connection::open(storage.join("data.sqlite")).unwrap();
    db.execute_batch("CREATE TABLE data (key TEXT PRIMARY KEY, utf16_length INTEGER NOT NULL, conversion_type INTEGER NOT NULL, compression_type INTEGER NOT NULL, value BLOB NOT NULL)").unwrap();
    db.execute(
        "INSERT INTO data VALUES ('userToken', 16, ?1, ?2, ?3)",
        rusqlite::params![conversion, compression, value],
    )
    .unwrap();
    profile
}

#[test]
fn selected_browser_profiles_are_metadata_only_and_keep_titles() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join("Default")).unwrap();
    fs::create_dir(root.path().join("Profile 2")).unwrap();
    // A corrupt secret database must not prevent profile discovery.
    fs::create_dir_all(root.path().join("Default/Local Storage/leveldb")).unwrap();
    fs::write(
        root.path().join("Default/Local Storage/leveldb/CURRENT"),
        b"not-a-db",
    )
    .unwrap();
    fs::write(
        root.path().join("Local State"),
        r#"{"profile":{"info_cache":{"Default":{"name":"Personal"},"Profile 2":{"name":"Work"}}}}"#,
    )
    .unwrap();
    let reader = BrowserReader::with_root(Browser::Chrome, root.path());
    let profiles = reader.profiles(Browser::Chrome).unwrap();
    assert_eq!(
        profiles
            .iter()
            .map(|p| (p.id(), p.label()))
            .collect::<Vec<_>>(),
        vec![("Default", "Personal"), ("Profile 2", "Work")]
    );
    assert!(reader.profiles(Browser::Edge).unwrap().is_empty());
}

#[test]
fn chromium_import_reads_real_leveldb_and_persists_only_binding() {
    let root = tempfile::tempdir().unwrap();
    let profile = chromium_profile(root.path(), "Default", "fixture-secret-one");
    let reader = BrowserReader::with_root(Browser::Brave, root.path());
    let profiles = reader.profiles(Browser::Brave).unwrap();
    let (selection, material) = reader.import(&profiles[0]).unwrap();
    assert_eq!(material.secret(), "fixture-secret-one");
    let json = serde_json::to_string(&selection).unwrap();
    assert!(!json.contains("fixture-secret-one"));
    assert!(!json.contains(root.path().to_str().unwrap()));
    let restored: BrowserSelection = serde_json::from_str(&json).unwrap();
    assert_eq!(
        reader.resolve(&restored).unwrap().secret(),
        "fixture-secret-one"
    );
    let storage = profile.join("Local Storage/leveldb");
    let mut db = DB::open(&storage, Options::default()).unwrap();
    db.put(
        b"_https://platform.deepseek.com\0\x01userToken",
        b"\x01different-account",
    )
    .unwrap();
    db.flush().unwrap();
    drop(db);
    assert!(matches!(
        reader.resolve(&restored),
        Err(BrowserError::SourceChanged)
    ));
}

#[test]
fn locked_leveldb_is_not_bypassed() {
    let root = tempfile::tempdir().unwrap();
    let profile = chromium_profile(root.path(), "Default", "fixture-locked-secret");
    let reader = BrowserReader::with_root(Browser::Edge, root.path());
    let profiles = reader.profiles(Browser::Edge).unwrap();
    let _lock = DB::open(profile.join("Local Storage/leveldb"), Options::default()).unwrap();
    assert!(matches!(
        reader.import(&profiles[0]),
        Err(BrowserError::Locked)
    ));
}

#[test]
fn firefox_reads_real_snappy_utf8_and_utf16_blobs() {
    for (conversion, compression, raw) in [
        (1, 0, b"fixture-firefox-token".to_vec()),
        (
            1,
            1,
            snap::raw::Encoder::new()
                .compress_vec(b"fixture-firefox-token")
                .unwrap(),
        ),
        (
            0,
            0,
            "fixture-firefox-token"
                .encode_utf16()
                .flat_map(u16::to_le_bytes)
                .collect(),
        ),
        (
            0,
            1,
            snap::raw::Encoder::new()
                .compress_vec(
                    &"fixture-firefox-token"
                        .encode_utf16()
                        .flat_map(u16::to_le_bytes)
                        .collect::<Vec<_>>(),
                )
                .unwrap(),
        ),
    ] {
        let root = tempfile::tempdir().unwrap();
        firefox_profile(root.path(), &raw, conversion, compression);
        let reader = BrowserReader::with_root(Browser::Firefox, root.path());
        let profiles = reader.profiles(Browser::Firefox).unwrap();
        assert_eq!(profiles[0].label(), "Work Firefox");
        assert_eq!(
            reader.import(&profiles[0]).unwrap().1.secret(),
            "fixture-firefox-token"
        );
    }
}

#[test]
fn firefox_wal_reads_committed_value_without_copying_database() {
    let root = tempfile::tempdir().unwrap();
    let profile = firefox_profile(root.path(), b"old-token", 1, 0);
    let path = profile.join("storage/default/https+++platform.deepseek.com/ls/data.sqlite");
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0;")
        .unwrap();
    db.execute(
        "UPDATE data SET value=?1 WHERE key='userToken'",
        [b"wal-token".as_slice()],
    )
    .unwrap();
    let reader = BrowserReader::with_root(Browser::Firefox, root.path());
    let profiles = reader.profiles(Browser::Firefox).unwrap();
    assert_eq!(reader.import(&profiles[0]).unwrap().1.secret(), "wal-token");
}

#[test]
fn missing_profile_or_exact_origin_never_switches_to_another_session() {
    let root = tempfile::tempdir().unwrap();
    let profile = chromium_profile(root.path(), "Default", "fixture-binding");
    chromium_profile(root.path(), "Profile 1", "other-profile");
    let reader = BrowserReader::with_root(Browser::Chromium, root.path());
    let profiles = reader.profiles(Browser::Chromium).unwrap();
    let (selection, _) = reader.import(&profiles[0]).unwrap();
    fs::rename(&profile, root.path().join("Moved")).unwrap();
    assert!(matches!(
        reader.resolve(&selection),
        Err(BrowserError::ProfileGone)
    ));
    let root = tempfile::tempdir().unwrap();
    let profile = firefox_profile(root.path(), b"fixture-origin-token", 1, 0);
    fs::rename(
        profile.join("storage/default/https+++platform.deepseek.com"),
        profile.join("storage/default/https+++platform.deepseek.com^userContextId=1"),
    )
    .unwrap();
    let reader = BrowserReader::with_root(Browser::Firefox, root.path());
    let profiles = reader.profiles(Browser::Firefox).unwrap();
    assert!(matches!(
        reader.import(&profiles[0]),
        Err(BrowserError::NoSession)
    ));
}

#[test]
fn firefox_exclusive_database_lock_is_reported_without_bypass() {
    let root = tempfile::tempdir().unwrap();
    let profile = firefox_profile(root.path(), b"fixture-locked-firefox", 1, 0);
    let db = rusqlite::Connection::open(
        profile.join("storage/default/https+++platform.deepseek.com/ls/data.sqlite"),
    )
    .unwrap();
    db.execute_batch("BEGIN EXCLUSIVE;").unwrap();
    let reader = BrowserReader::with_root(Browser::Firefox, root.path());
    let profiles = reader.profiles(Browser::Firefox).unwrap();
    assert!(matches!(
        reader.import(&profiles[0]),
        Err(BrowserError::Locked)
    ));
}

#[test]
fn chromium_utf16_and_deleted_token_use_real_database_semantics() {
    let root = tempfile::tempdir().unwrap();
    let profile = chromium_profile(root.path(), "Default", "fixture-old-token");
    let storage = profile.join("Local Storage/leveldb");
    let mut db = DB::open(&storage, Options::default()).unwrap();
    db.delete(b"_https://platform.deepseek.com\0\x01userToken")
        .unwrap();
    let key = b"_https://platform.deepseek.com\0\x00u\0s\0e\0r\0T\0o\0k\0e\0n\0";
    let mut value = vec![0];
    value.extend(
        "fixture-utf16-token"
            .encode_utf16()
            .flat_map(u16::to_le_bytes),
    );
    db.put(key, &value).unwrap();
    db.flush().unwrap();
    drop(db);
    let reader = BrowserReader::with_root(Browser::Chrome, root.path());
    let profiles = reader.profiles(Browser::Chrome).unwrap();
    assert_eq!(
        reader.import(&profiles[0]).unwrap().1.secret(),
        "fixture-utf16-token"
    );
    let mut db = DB::open(&storage, Options::default()).unwrap();
    db.delete(key).unwrap();
    db.flush().unwrap();
    drop(db);
    // HTTP and look-alike-host tokens still exist, but are never candidates.
    assert!(matches!(
        reader.import(&profiles[0]),
        Err(BrowserError::NoSession)
    ));
}

#[test]
fn unknown_firefox_compression_is_not_mistaken_for_plaintext() {
    let root = tempfile::tempdir().unwrap();
    firefox_profile(root.path(), b"fixture-unknown-format", 1, 9);
    let reader = BrowserReader::with_root(Browser::Firefox, root.path());
    let profiles = reader.profiles(Browser::Firefox).unwrap();
    assert!(matches!(
        reader.import(&profiles[0]),
        Err(BrowserError::Unsupported)
    ));
}
