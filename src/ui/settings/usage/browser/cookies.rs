//! 仅从显式选择的 profile 读取白名单 Cookie；StepFun web_id 来自精确 LocalStorage origin。

use super::{Browser, BrowserError, BrowserReader, BrowserSelection, Profile, SessionMaterial, fingerprint, io_error};
use rusqlite::{Connection, ErrorCode, OpenFlags, params};
use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};
use zeroize::Zeroizing;

#[cfg(target_os = "macos")]
mod mac;

#[derive(Clone, Copy)]
pub(in super::super) enum CookieProvider {
    StepFun,
    MiMo,
}

impl CookieProvider {
    fn binding(self) -> &'static str {
        match self {
            Self::StepFun => "stepfun-cookie:",
            Self::MiMo => "mimo-cookie:",
        }
    }

    fn hosts(self) -> [&'static str; 3] {
        match self {
            Self::StepFun => ["platform.stepfun.com", ".platform.stepfun.com", ".stepfun.com"],
            Self::MiMo => ["platform.xiaomimimo.com", ".platform.xiaomimimo.com", ".xiaomimimo.com"],
        }
    }

    fn names(self) -> [&'static str; 2] {
        match self {
            Self::StepFun => ["Oasis-Token", "Oasis-Webid"],
            Self::MiMo => ["api-platform_serviceToken", "userId"],
        }
    }

    fn cookie_names(self) -> [&'static str; 2] {
        match self {
            // 固定两个 SQL 参数均为同一白名单键；Oasis-Webid 不是 Cookie。
            Self::StepFun => ["Oasis-Token", "Oasis-Token"],
            Self::MiMo => self.names(),
        }
    }

    fn paths(self) -> &'static [&'static str] {
        match self {
            Self::StepFun => &[
                "/api/step.openapi.devcenter.Dashboard/QueryStepPlanRateLimit",
                "/api/step.openapi.devcenter.Dashboard/GetStepPlanStatus",
                "/api/step.openapi.devcenter.Dashboard/UserInfo",
            ],
            Self::MiMo => &["/api/v1/balance", "/api/v1/tokenPlan/detail", "/api/v1/tokenPlan/usage", "/api/v1/userProfile"],
        }
    }

    fn token_fingerprint(self, material: &SessionMaterial) -> String {
        format!("{}{}", self.binding(), fingerprint(&[self.binding(), material.secret()]))
    }
}

impl BrowserReader {
    pub(in super::super) fn import_cookies(
        &self,
        profile: &Profile,
        provider: CookieProvider,
        interactive: bool,
    ) -> Result<(BrowserSelection, SessionMaterial), BrowserError> {
        let current = self.profiles(profile.browser)?.into_iter()
            .find(|candidate| candidate.id == profile.id && candidate.fingerprint == profile.fingerprint)
            .ok_or(BrowserError::ProfileGone)?;
        let material = read(&current, provider, interactive)?;
        let selection = BrowserSelection {
            browser: current.browser,
            profile_id: current.id,
            profile_label: current.label,
            profile_fingerprint: current.fingerprint,
            token_fingerprint: provider.token_fingerprint(&material),
        };
        Ok((selection, material))
    }

    pub(in super::super) fn resolve_cookies(
        &self,
        selection: &BrowserSelection,
        provider: CookieProvider,
    ) -> Result<SessionMaterial, BrowserError> {
        if !selection.token_fingerprint.starts_with(provider.binding()) {
            return Err(BrowserError::SourceChanged);
        }
        let profile = self.profiles(selection.browser)?.into_iter()
            .find(|profile| profile.id == selection.profile_id && profile.fingerprint == selection.profile_fingerprint)
            .ok_or(BrowserError::ProfileGone)?;
        // 后台解析绝不授权 Keychain 弹窗。
        let material = read(&profile, provider, false)?;
        if provider.token_fingerprint(&material) != selection.token_fingerprint {
            return Err(BrowserError::SourceChanged);
        }
        Ok(material)
    }
}

fn read(profile: &Profile, provider: CookieProvider, interactive: bool) -> Result<SessionMaterial, BrowserError> {
    let now = SystemTime::now().duration_since(UNIX_EPOCH)
        .map_err(|_| BrowserError::Unreadable)?.as_secs();
    let now = i64::try_from(now).map_err(|_| BrowserError::Unreadable)?;
    let values = if profile.browser == Browser::Firefox {
        firefox(profile, provider, now)?
    } else {
        chromium(profile, provider, now, interactive)?
    };
    let [first, second] = values;
    let first = first.ok_or(BrowserError::NoSession)?;
    let first = if matches!(provider, CookieProvider::StepFun) { decode_stepfun_token(first)? } else { first };
    let second = match provider {
        CookieProvider::StepFun => Zeroizing::new(if profile.browser == Browser::Firefox {
            super::firefox::read_for(&profile.path, "https://platform.stepfun.com", "web_id")?
        } else {
            super::chromium::read_for(&profile.path, "https://platform.stepfun.com", "web_id")?
        }),
        CookieProvider::MiMo => second.ok_or(BrowserError::NoSession)?,
    };
    validate_value(&second)?;
    let names = provider.names();
    let capacity = names[0].len() + 1 + first.len() + 2 + names[1].len() + 1 + second.len();
    let mut header = Zeroizing::new(String::with_capacity(capacity));
    header.push_str(names[0]);
    header.push('=');
    header.push_str(&first);
    header.push_str("; ");
    header.push_str(names[1]);
    header.push('=');
    header.push_str(&second);
    Ok(SessionMaterial(header))
}

type Values = [Option<Zeroizing<String>>; 2];

fn decode_stepfun_token(value: Zeroizing<String>) -> Result<Zeroizing<String>, BrowserError> {
    if !value.contains('%') { return Ok(value); }
    let bytes = value.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if index + 2 >= bytes.len() || !bytes[index + 1].is_ascii_hexdigit() || !bytes[index + 2].is_ascii_hexdigit() {
                return Err(BrowserError::Unreadable);
            }
            index += 3;
        } else {
            index += 1;
        }
    }
    // URI 解码，不采用 application/x-www-form-urlencoded 的 '+'→空格规则。
    let decoded = percent_encoding::percent_decode_str(&value).decode_utf8().map_err(|_| BrowserError::Unreadable)?;
    let decoded = Zeroizing::new(decoded.into_owned());
    validate_value(&decoded)?;
    Ok(decoded)
}

fn validate_value(value: &str) -> Result<(), BrowserError> {
    if value.is_empty() { return Err(BrowserError::NoSession); }
    if value.len() > 64 * 1024 || !value.bytes().all(|byte| matches!(byte, 0x21 | 0x23..=0x2b | 0x2d..=0x3a | 0x3c..=0x5b | 0x5d..=0x7e)) {
        return Err(BrowserError::Unreadable);
    }
    Ok(())
}

fn store(values: &mut Values, provider: CookieProvider, name: &str, value: Zeroizing<String>) -> Result<(), BrowserError> {
    // 拒绝头注入，不猜编码或拆解任意 Cookie 头。
    validate_value(&value)?;
    let index = provider.cookie_names().iter().position(|candidate| *candidate == name).ok_or(BrowserError::Unreadable)?;
    // 不按数据库行顺序猜账号，也不跨 scope 合并同名 Cookie。
    if values[index].is_some() {
        return Err(BrowserError::Unreadable);
    }
    values[index] = Some(value);
    Ok(())
}

fn path_matches(provider: CookieProvider, cookie_path: &str) -> bool {
    cookie_path.starts_with('/') && provider.paths().iter().all(|request| {
        *request == cookie_path || request.strip_prefix(cookie_path).is_some_and(|suffix| {
            cookie_path.ends_with('/') || suffix.starts_with('/')
        })
    })
}

fn open(profile: &Path, path: &Path) -> Result<Connection, BrowserError> {
    let metadata = fs::metadata(path).map_err(io_error)?;
    if !metadata.is_file() {
        return Err(BrowserError::Unreadable);
    }
    if !fs::canonicalize(path).map_err(io_error)?.starts_with(profile) {
        return Err(BrowserError::Denied);
    }
    // 不使用 immutable，不复制数据库或 WAL：SQLite 共享锁下直接读取当前快照。
    let db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX).map_err(sql_error)?;
    db.busy_timeout(std::time::Duration::ZERO).map_err(sql_error)?;
    db.execute_batch("PRAGMA query_only=ON; PRAGMA temp_store=MEMORY;").map_err(sql_error)?;
    Ok(db)
}

fn firefox(profile: &Profile, provider: CookieProvider, now: i64) -> Result<Values, BrowserError> {
    let db = open(&profile.path, &profile.path.join("cookies.sqlite"))?;
    let mut statement = db.prepare(
        "SELECT name,path,length(value),value FROM moz_cookies WHERE host IN (?1,?2,?3) AND name IN (?4,?5) AND originAttributes='' AND expiry>?6 LIMIT 33"
    ).map_err(schema_error)?;
    let hosts = provider.hosts();
    let names = provider.cookie_names();
    let mut rows = statement.query(params![hosts[0],hosts[1],hosts[2],names[0],names[1],now]).map_err(sql_error)?;
    let mut values = [None, None];
    let mut count = 0;
    while let Some(row) = rows.next().map_err(sql_error)? {
        count += 1;
        if count > 32 { return Err(BrowserError::Unreadable); }
        let path: String = row.get(1).map_err(sql_error)?;
        if !path_matches(provider, &path) { continue; }
        let length: i64 = row.get(2).map_err(sql_error)?;
        if !(0..=64 * 1024).contains(&length) { return Err(BrowserError::Unreadable); }
        let name: String = row.get(0).map_err(sql_error)?;
        let value = Zeroizing::new(row.get::<_, String>(3).map_err(sql_error)?);
        store(&mut values, provider, &name, value)?;
    }
    Ok(values)
}

fn chromium(profile: &Profile, provider: CookieProvider, now: i64, interactive: bool) -> Result<Values, BrowserError> {
    #[cfg(target_os = "windows")]
    {
        let state = profile.path.parent().ok_or(BrowserError::Unreadable)?.join("Local State");
        match fs::metadata(&state) {
            Ok(metadata) => {
                if metadata.len() > 4 * 1024 * 1024 { return Err(BrowserError::Unreadable); }
                let state: serde_json::Value = serde_json::from_slice(&fs::read(state).map_err(io_error)?).map_err(|_| BrowserError::Unreadable)?;
                if state.get("os_crypt").and_then(|crypt| crypt.get("app_bound_encrypted_key")).is_some() {
                    return Err(BrowserError::Protected);
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(io_error(error)),
        }
    }
    let network = profile.path.join("Network/Cookies");
    let path = match fs::metadata(&network) {
        Ok(_) => network,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => profile.path.join("Cookies"),
        Err(error) => return Err(io_error(error)),
    };
    let db = open(&profile.path, &path)?;
    let transaction = db.unchecked_transaction().map_err(sql_error)?;
    let version: String = transaction.query_row("SELECT CAST(value AS TEXT) FROM meta WHERE key='version'", [], |row| row.get(0)).map_err(schema_error)?;
    let version = version.parse::<i64>().map_err(|_| BrowserError::Unsupported)?;
    if version < 1 { return Err(BrowserError::Unsupported); }
    let now = now.checked_add(11_644_473_600).and_then(|value| value.checked_mul(1_000_000)).ok_or(BrowserError::Unreadable)?;
    let mut statement = transaction.prepare(
        "SELECT host_key,name,path,length(value),length(encrypted_value),value,encrypted_value FROM cookies WHERE host_key IN (?1,?2,?3) AND name IN (?4,?5) AND top_frame_site_key='' AND has_cross_site_ancestor=0 AND ((has_expires=0 AND is_persistent=0) OR (has_expires=1 AND expires_utc>?6)) LIMIT 33"
    ).map_err(schema_error)?;
    let hosts = provider.hosts();
    let names = provider.cookie_names();
    let mut rows = statement.query(params![hosts[0],hosts[1],hosts[2],names[0],names[1],now]).map_err(sql_error)?;
    let mut values = [None, None];
    #[cfg(target_os = "macos")]
    let mut password = None;
    #[cfg(not(target_os = "macos"))]
    let _ = (interactive, version);
    let mut count = 0;
    while let Some(row) = rows.next().map_err(sql_error)? {
        count += 1;
        if count > 32 { return Err(BrowserError::Unreadable); }
        let path: String = row.get(2).map_err(sql_error)?;
        if !path_matches(provider, &path) { continue; }
        let plain_length: i64 = row.get(3).map_err(sql_error)?;
        let encrypted_length: i64 = row.get(4).map_err(sql_error)?;
        if !(0..=64 * 1024).contains(&plain_length) || !(0..=64 * 1024 + 128).contains(&encrypted_length) {
            return Err(BrowserError::Unreadable);
        }
        let name: String = row.get(1).map_err(sql_error)?;
        let value = if encrypted_length == 0 {
            Zeroizing::new(row.get::<_, String>(5).map_err(sql_error)?)
        } else {
            // 明文与密文并存不是可猜测的回退路径。
            if plain_length != 0 { return Err(BrowserError::Unreadable); }
            let encrypted = Zeroizing::new(row.get::<_, Vec<u8>>(6).map_err(sql_error)?);
            if encrypted.starts_with(b"v20") { return Err(BrowserError::Protected); }
            #[cfg(target_os = "macos")]
            {
                if !encrypted.starts_with(b"v10") { return Err(BrowserError::Unsupported); }
                if password.is_none() { password = Some(mac::password(profile.browser, interactive)?); }
                let host: String = row.get(0).map_err(sql_error)?;
                mac::decrypt(&encrypted, password.as_ref().ok_or(BrowserError::Denied)?, &host, version)?
            }
            #[cfg(not(target_os = "macos"))]
            { return Err(BrowserError::Unsupported); }
        };
        store(&mut values, provider, &name, value)?;
    }
    Ok(values)
}

fn schema_error(error: rusqlite::Error) -> BrowserError {
    if error.sqlite_error_code() == Some(ErrorCode::Unknown) { BrowserError::Unsupported } else { sql_error(error) }
}

fn sql_error(error: rusqlite::Error) -> BrowserError {
    match error.sqlite_error_code() {
        Some(ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked | ErrorCode::FileLockingProtocolFailed) => BrowserError::Locked,
        Some(ErrorCode::PermissionDenied | ErrorCode::AuthorizationForStatementDenied | ErrorCode::ReadOnly) => BrowserError::Denied,
        _ => BrowserError::Unreadable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::{Connection, params};
    use std::fs;
    use std::path::Path;

    fn firefox(root: &Path) -> (BrowserReader, Profile, Connection) {
        let path = root.join("Profiles/isolated.default");
        fs::create_dir_all(&path).unwrap();
        fs::write(root.join("profiles.ini"), "[Profile0]\nName=Isolated\nIsRelative=1\nPath=Profiles/isolated.default\n").unwrap();
        let db = Connection::open(path.join("cookies.sqlite")).unwrap();
        db.execute_batch("CREATE TABLE moz_cookies (id INTEGER PRIMARY KEY, originAttributes TEXT NOT NULL DEFAULT '', name TEXT, value TEXT, host TEXT, path TEXT, expiry INTEGER, lastAccessed INTEGER, creationTime INTEGER, isSecure INTEGER, isHttpOnly INTEGER, inBrowserElement INTEGER DEFAULT 0, sameSite INTEGER DEFAULT 0, rawSameSite INTEGER DEFAULT 0, schemeMap INTEGER DEFAULT 2, isPartitionedAttributeSet INTEGER DEFAULT 0, UNIQUE(name,host,path,originAttributes));").unwrap();
        let reader = BrowserReader::with_root(Browser::Firefox, root);
        let profile = reader.profiles(Browser::Firefox).unwrap().remove(0);
        (reader, profile, db)
    }

    fn cookie(db: &Connection, host: &str, name: &str, value: &str, path: &str, expiry: i64, attrs: &str) {
        db.execute("INSERT INTO moz_cookies (host,name,value,path,expiry,originAttributes,lastAccessed,creationTime,isSecure,isHttpOnly) VALUES (?1,?2,?3,?4,?5,?6,0,0,1,1)", params![host,name,value,path,expiry,attrs]).unwrap();
    }

    fn firefox_web_id(profile: &Path, value: &str) -> Connection {
        let storage = profile.join("storage/default/https+++platform.stepfun.com/ls");
        fs::create_dir_all(&storage).unwrap();
        let db = Connection::open(storage.join("data.sqlite")).unwrap();
        db.execute_batch("CREATE TABLE data (key TEXT PRIMARY KEY, utf16_length INTEGER NOT NULL, conversion_type INTEGER NOT NULL, compression_type INTEGER NOT NULL, value BLOB NOT NULL);").unwrap();
        db.execute("INSERT INTO data VALUES ('web_id',?1,1,0,?2)",params![value.len() as i64,value.as_bytes()]).unwrap();
        db
    }

    #[test]
    fn stepfun_cannot_substitute_cookie_for_missing_localstorage_web_id() {
        let root = tempfile::tempdir().unwrap();
        let (reader,profile,db) = firefox(root.path());
        cookie(&db,".stepfun.com","Oasis-Token","fixture-step","/",i64::MAX,"");
        cookie(&db,".stepfun.com","Oasis-Webid","wrong-cookie","/",i64::MAX,"");
        assert!(matches!(reader.import_cookies(&profile,CookieProvider::StepFun,true),Err(BrowserError::NoSession)));
        firefox_web_id(&profile.path,"wrong-origin");
        let storage = profile.path.join("storage/default");
        fs::rename(storage.join("https+++platform.stepfun.com"),storage.join("https+++platform.stepfun.com.evil.test")).unwrap();
        assert!(matches!(reader.import_cookies(&profile,CookieProvider::StepFun,true),Err(BrowserError::NoSession)));
    }

    #[test]
    fn stepfun_web_id_reads_live_localstorage_wal_and_binds_its_rotation() {
        let root = tempfile::tempdir().unwrap();
        let (reader,profile,db) = firefox(root.path());
        cookie(&db,".stepfun.com","Oasis-Token","fixture-step","/",i64::MAX,"");
        let local = firefox_web_id(&profile.path,"before");
        local.execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0;").unwrap();
        local.execute("UPDATE data SET value=?1 WHERE key='web_id'",[b"wal-device".as_slice()]).unwrap();
        let (selection,material) = reader.import_cookies(&profile,CookieProvider::StepFun,true).unwrap();
        assert_eq!(material.secret(),"Oasis-Token=fixture-step; Oasis-Webid=wal-device");
        local.execute("UPDATE data SET value=?1 WHERE key='web_id'",[b"changed-device".as_slice()]).unwrap();
        assert!(matches!(reader.resolve_cookies(&selection,CookieProvider::StepFun),Err(BrowserError::SourceChanged)));
    }

    #[test]
    fn stepfun_cookie_token_uses_strict_uri_decoding_without_form_plus_conversion() {
        let root = tempfile::tempdir().unwrap();
        let (reader,profile,db) = firefox(root.path());
        firefox_web_id(&profile.path,"device");
        cookie(&db,".stepfun.com","Oasis-Token","fixture%2Ftoken+plus%3D","/",i64::MAX,"");
        assert_eq!(reader.import_cookies(&profile,CookieProvider::StepFun,true).unwrap().1.secret(),"Oasis-Token=fixture/token+plus=; Oasis-Webid=device");
        for value in ["bad%", "bad%2", "bad%GG", "%ff", "%0d%0aInjected%3Aheader", "%3Bother%3Dsecret"] {
            db.execute("UPDATE moz_cookies SET value=?1 WHERE name='Oasis-Token'",[value]).unwrap();
            assert!(matches!(reader.import_cookies(&profile,CookieProvider::StepFun,true),Err(BrowserError::Unreadable)));
        }
    }

    #[test]
    fn firefox_import_returns_only_provider_cookies_and_secret_free_binding() {
        let root = tempfile::tempdir().unwrap();
        let (reader, profile, db) = firefox(root.path());
        cookie(&db,".stepfun.com","Oasis-Token","fixture-step","/",i64::MAX,"");
        firefox_web_id(&profile.path,"fixture-device");
        cookie(&db,"platform.stepfun.com","Oasis-Webid","must-ignore-cookie","/",i64::MAX,"");
        cookie(&db,".stepfun.com","analytics","must-not-import","/",i64::MAX,"");
        let (selection, material) = reader.import_cookies(&profile, CookieProvider::StepFun, true).unwrap();
        assert_eq!(material.secret(),"Oasis-Token=fixture-step; Oasis-Webid=fixture-device");
        let saved = serde_json::to_string(&selection).unwrap();
        assert!(!saved.contains("fixture-step"));
        assert!(!saved.contains(root.path().to_str().unwrap()));
        let restored = serde_json::from_str(&saved).unwrap();
        assert_eq!(reader.resolve_cookies(&restored,CookieProvider::StepFun).unwrap().secret(),material.secret());
        assert!(matches!(reader.resolve_cookies(&restored,CookieProvider::MiMo),Err(BrowserError::SourceChanged)));
    }

    #[test]
    fn firefox_reads_live_wal_and_requires_explicit_reimport_after_rotation() {
        let root = tempfile::tempdir().unwrap();
        let (reader, profile, db) = firefox(root.path());
        db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0;").unwrap();
        cookie(&db,".xiaomimimo.com","api-platform_serviceToken","wal-token","/api",i64::MAX,"");
        cookie(&db,"platform.xiaomimimo.com","userId","user-7","/",i64::MAX,"");
        let (selection, material) = reader.import_cookies(&profile,CookieProvider::MiMo,true).unwrap();
        assert_eq!(material.secret(),"api-platform_serviceToken=wal-token; userId=user-7");
        db.execute("UPDATE moz_cookies SET value='rotated' WHERE name='api-platform_serviceToken'",[]).unwrap();
        assert!(matches!(reader.resolve_cookies(&selection,CookieProvider::MiMo),Err(BrowserError::SourceChanged)));
    }

    #[test]
    fn firefox_ignores_wrong_domain_expired_path_and_container_sessions() {
        for (host,path,expiry,attrs) in [
            (".stepfun.com.evil.test","/",i64::MAX,""),
            ("stepfun.com","/",i64::MAX,""),
            (".stepfun.com","/api/other",i64::MAX,""),
            (".stepfun.com","/api/step.openapi.devcenter.Dashboard/QueryStepPlanRateLimit",i64::MAX,""),
            (".stepfun.com","/",1,""),
            (".stepfun.com","/",i64::MAX,"^userContextId=1"),
            (".stepfun.com","/",i64::MAX,"^partitionKey=%28https%2Cother.test%29"),
        ] {
            let root = tempfile::tempdir().unwrap();
            let (reader,profile,db) = firefox(root.path());
            cookie(&db,host,"Oasis-Token","excluded",path,expiry,attrs);
            assert!(matches!(reader.import_cookies(&profile,CookieProvider::StepFun,true),Err(BrowserError::NoSession)));
        }
    }

    #[test]
    fn firefox_rejects_ambiguous_duplicates_and_header_injection() {
        let root = tempfile::tempdir().unwrap();
        let (reader,profile,db) = firefox(root.path());
        cookie(&db,".stepfun.com","Oasis-Token","one","/",i64::MAX,"");
        cookie(&db,"platform.stepfun.com","Oasis-Token","two","/",i64::MAX,"");
        assert!(matches!(reader.import_cookies(&profile,CookieProvider::StepFun,true),Err(BrowserError::Unreadable)));
        db.execute("DELETE FROM moz_cookies",[]).unwrap();
        cookie(&db,".stepfun.com","Oasis-Token","injected; other=secret","/",i64::MAX,"");
        assert!(matches!(reader.import_cookies(&profile,CookieProvider::StepFun,true),Err(BrowserError::Unreadable)));
    }

    #[test]
    fn firefox_mimo_needs_both_exact_cookie_names() {
        let root = tempfile::tempdir().unwrap();
        let (reader,profile,db) = firefox(root.path());
        cookie(&db,".xiaomimimo.com","api-platform_serviceToken","token","/",i64::MAX,"");
        assert!(matches!(reader.import_cookies(&profile,CookieProvider::MiMo,true),Err(BrowserError::NoSession)));
        cookie(&db,".xiaomimimo.com","userId","42","/",i64::MAX,"");
        assert_eq!(reader.import_cookies(&profile,CookieProvider::MiMo,true).unwrap().1.secret(),"api-platform_serviceToken=token; userId=42");
    }

    #[test]
    fn firefox_locked_database_is_reported_without_copy_or_bypass() {
        let root = tempfile::tempdir().unwrap();
        let (reader,profile,db) = firefox(root.path());
        cookie(&db,".stepfun.com","Oasis-Token","locked","/",i64::MAX,"");
        db.execute_batch("BEGIN EXCLUSIVE;").unwrap();
        assert!(matches!(reader.import_cookies(&profile,CookieProvider::StepFun,true),Err(BrowserError::Locked)));
    }

    #[test]
    fn firefox_removed_profile_never_falls_back_to_another_profile() {
        let root = tempfile::tempdir().unwrap();
        let (reader,profile,db) = firefox(root.path());
        cookie(&db,".stepfun.com","Oasis-Token","selected","/",i64::MAX,"");
        firefox_web_id(&profile.path,"selected-device");
        let (selection,_) = reader.import_cookies(&profile,CookieProvider::StepFun,true).unwrap();
        drop(db);
        let other = root.path().join("Profiles/other.default");
        fs::create_dir_all(&other).unwrap();
        let other_db = Connection::open(other.join("cookies.sqlite")).unwrap();
        other_db.execute_batch("CREATE TABLE moz_cookies (host TEXT,name TEXT,value TEXT,path TEXT,expiry INTEGER,originAttributes TEXT); INSERT INTO moz_cookies VALUES ('.stepfun.com','Oasis-Token','other-account','/',9223372036854775807,'');").unwrap();
        fs::write(root.path().join("profiles.ini"), "[Profile0]\nName=Selected\nIsRelative=1\nPath=Profiles/isolated.default\n[Profile1]\nName=Other\nIsRelative=1\nPath=Profiles/other.default\n").unwrap();
        fs::rename(&profile.path,root.path().join("moved")).unwrap();
        assert!(matches!(reader.resolve_cookies(&selection,CookieProvider::StepFun),Err(BrowserError::ProfileGone)));
    }

    fn chromium_fixture(root: &Path) -> (BrowserReader, Profile, Connection) {
        let path = root.join("Default/Network");
        fs::create_dir_all(&path).unwrap();
        let db = Connection::open(path.join("Cookies")).unwrap();
        db.execute_batch("CREATE TABLE meta (key LONGVARCHAR NOT NULL UNIQUE PRIMARY KEY, value LONGVARCHAR); INSERT INTO meta VALUES ('version','24'); CREATE TABLE cookies (creation_utc INTEGER NOT NULL DEFAULT 0, host_key TEXT NOT NULL, top_frame_site_key TEXT NOT NULL DEFAULT '', name TEXT NOT NULL, value TEXT NOT NULL DEFAULT '', encrypted_value BLOB NOT NULL DEFAULT X'', path TEXT NOT NULL DEFAULT '/', expires_utc INTEGER NOT NULL DEFAULT 0, is_secure INTEGER NOT NULL DEFAULT 1, is_httponly INTEGER NOT NULL DEFAULT 1, last_access_utc INTEGER NOT NULL DEFAULT 0, has_expires INTEGER NOT NULL DEFAULT 0, is_persistent INTEGER NOT NULL DEFAULT 0, priority INTEGER NOT NULL DEFAULT 1, samesite INTEGER NOT NULL DEFAULT -1, source_scheme INTEGER NOT NULL DEFAULT 2, source_port INTEGER NOT NULL DEFAULT 443, last_update_utc INTEGER NOT NULL DEFAULT 0, source_type INTEGER NOT NULL DEFAULT 0, has_cross_site_ancestor INTEGER NOT NULL DEFAULT 0);").unwrap();
        let reader = BrowserReader::with_root(Browser::Chrome, root);
        let profile = reader.profiles(Browser::Chrome).unwrap().remove(0);
        (reader, profile, db)
    }

    #[test]
    fn chromium_plaintext_cookie_respects_partition_scope_and_live_wal() {
        let root = tempfile::tempdir().unwrap();
        let (reader,profile,db) = chromium_fixture(root.path());
        db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0; INSERT INTO cookies (host_key,name,value) VALUES ('.stepfun.com','Oasis-Token','committed-wal'); INSERT INTO cookies (host_key,name,value,top_frame_site_key) VALUES ('.stepfun.com','Oasis-Token','partitioned','https://other.test'); INSERT INTO cookies (host_key,name,value,has_cross_site_ancestor) VALUES ('.stepfun.com','Oasis-Token','cross-site',1); INSERT INTO cookies (host_key,name,value,has_expires,is_persistent,expires_utc) VALUES ('.stepfun.com','Oasis-Token','expired',1,1,1);").unwrap();
        let storage = profile.path.join("Local Storage/leveldb");
        fs::create_dir_all(&storage).unwrap();
        let mut local = rusty_leveldb::DB::open(&storage,rusty_leveldb::Options::default()).unwrap();
        local.put(b"_https://platform.stepfun.com\0\x01web_id",b"\x01chromium-device").unwrap();
        local.put(b"_https://platform.deepseek.com\0\x01web_id",b"\x01wrong-origin").unwrap();
        local.put(b"_https://platform.stepfun.com\0\x01Oasis-Webid",b"\x01wrong-key").unwrap();
        local.flush().unwrap();
        drop(local);
        let (_,material) = reader.import_cookies(&profile,CookieProvider::StepFun,true).unwrap();
        assert_eq!(material.secret(),"Oasis-Token=committed-wal; Oasis-Webid=chromium-device");
    }

    #[test]
    fn chromium_v20_is_protected_without_decryption_or_keychain_access() {
        let root = tempfile::tempdir().unwrap();
        let (reader,profile,db) = chromium_fixture(root.path());
        db.execute("INSERT INTO cookies (host_key,name,encrypted_value) VALUES ('.stepfun.com','Oasis-Token',?1)",[b"v20isolated-protected-fixture".as_slice()]).unwrap();
        assert!(matches!(reader.import_cookies(&profile,CookieProvider::StepFun,true),Err(BrowserError::Protected)));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_app_bound_key_is_protected_and_non_abe_v10_is_unsupported() {
        let root = tempfile::tempdir().unwrap();
        let (reader,profile,db) = chromium_fixture(root.path());
        fs::write(root.path().join("Local State"),r#"{"os_crypt":{"app_bound_encrypted_key":"fixture-not-a-key"}}"#).unwrap();
        assert!(matches!(reader.import_cookies(&profile,CookieProvider::StepFun,true),Err(BrowserError::Protected)));
        fs::write(root.path().join("Local State"),"{}").unwrap();
        db.execute("INSERT INTO cookies (host_key,name,encrypted_value) VALUES ('.stepfun.com','Oasis-Token',?1)",[b"v10isolated-unsupported-fixture".as_slice()]).unwrap();
        assert!(matches!(reader.import_cookies(&profile,CookieProvider::StepFun,true),Err(BrowserError::Unsupported)));
    }
}
