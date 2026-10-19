use super::*;

#[gpui_kit::test]
fn mimo_manual_cookie_is_validated_before_save_and_failed_replace_preserves_old_session(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let vault = Arc::new(InMemoryCredentialStore::new());
    let http = Arc::new(Http::default());
    let (app, mut vcx) = harness(cx);
    install(&mut vcx, dir.path().join("usage.json"), vault.clone(), http.clone(), false);
    open(&app, &mut vcx);
    click(&mut vcx, "usage-add");
    click(&mut vcx, "usage-provider");
    vcx.simulate_keystrokes("down down enter");
    vcx.run_until_parked();
    type_into(&mut vcx, "usage-name", "MiMo work");
    type_into(&mut vcx, "usage-key", "serviceToken=isolated-session; userId=1024; unrelated=not-retained");
    http.reply(200, br#"{"code":0,"data":{"balance":12.5,"cashBalance":10,"giftBalance":2.5}}"#);
    http.reply(200, br#"{"code":0,"data":{"userId":"1024","idc":310}}"#);
    http.reply(200, br#"{"code":0,"data":{"expired":false,"planCode":"token-plan","currentPeriodEnd":"2026-10-31 23:59:59"}}"#);
    http.reply(200, br#"{"code":0,"data":{"usage":{"items":[{"name":"Monthly Credits","used":25,"limit":100,"percent":0.25}]}}}"#);
    click(&mut vcx, "usage-submit");
    settle(&mut vcx, |d| d.stored.accounts.len() == 1);
    let id = vcx.update(|_, cx| {
        let account = &cx.global::<UsageGlobal>().0.read(cx).stored.accounts[0];
        assert_eq!(account.provider, Provider::MiMo);
        let Balance::Provider(report) = &account.snapshot.as_ref().unwrap().balance else { panic!("provider report") };
        assert_eq!(report.identity.as_deref(), Some("1024"));
        assert_eq!(report.pools[0].remaining.as_deref(), Some("12.5"));
        account.id
    });
    let accounts = Accounts::load(dir.path().join("usage.json"), vault.clone()).unwrap();
    assert_eq!(accounts.secret(id).unwrap(), "api-platform_serviceToken=isolated-session; userId=1024");
    assert!(!std::fs::read_to_string(dir.path().join("usage.json")).unwrap().contains("isolated-session"));
    click(&mut vcx, &format!("usage-replace-{id}"));
    type_into(&mut vcx, "usage-key", "serviceToken=rejected-replacement; userId=2048");
    http.reply(401, b"{}");
    click(&mut vcx, "usage-submit");
    settle(&mut vcx, |d| d.stored.accounts[0].error == Some(UsageError::Authentication));
    assert_eq!(Accounts::load(dir.path().join("usage.json"), vault).unwrap().secret(id).unwrap(), "api-platform_serviceToken=isolated-session; userId=1024");
    let requests = http.requests.lock().unwrap();
    assert!(requests.iter().all(|r| r.0.starts_with("https://platform.xiaomimimo.com/api/v1/")));
    assert!(requests.iter().all(|r| r.1 == "Cookie"));
}

#[gpui_kit::test]
fn mimo_selected_firefox_cookies_query_in_memory_and_rotation_does_not_switch_accounts(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("isolated-browser");
    let profile = root.join("Profiles/isolated.work");
    std::fs::create_dir_all(&profile).unwrap();
    std::fs::write(root.join("profiles.ini"), "[Profile0]\nName=MiMo work\nIsRelative=1\nPath=Profiles/isolated.work\n").unwrap();
    let db = rusqlite::Connection::open(profile.join("cookies.sqlite")).unwrap();
    db.execute_batch("CREATE TABLE moz_cookies (host TEXT,name TEXT,value TEXT,path TEXT,expiry INTEGER,originAttributes TEXT);
        INSERT INTO moz_cookies VALUES ('.platform.xiaomimimo.com','api-platform_serviceToken','isolated-browser-session','/',4102444800,'');
        INSERT INTO moz_cookies VALUES ('.xiaomimimo.com','userId','1024','/',4102444800,'');
        INSERT INTO moz_cookies VALUES ('.evil.invalid','userId','9999','/',4102444800,'');").unwrap();
    let (app, mut vcx) = harness(cx);
    let vault = Arc::new(InMemoryCredentialStore::new());
    let http = Arc::new(Http::default());
    install(&mut vcx, dir.path().join("usage.json"), vault.clone(), http.clone(), false);
    open(&app, &mut vcx);
    click(&mut vcx, "usage-add");
    click(&mut vcx, "usage-provider");
    vcx.simulate_keystrokes("down down enter");
    vcx.run_until_parked();
    type_into(&mut vcx, "usage-name", "Browser MiMo");
    click(&mut vcx, "usage-source");
    vcx.simulate_keystrokes("down enter");
    vcx.run_until_parked();
    click(&mut vcx, "usage-browser");
    vcx.simulate_keystrokes("down down down down down enter");
    vcx.run_until_parked();
    let page = app.update_in(&mut vcx, |a, _, _| a.active_settings().unwrap().usage.clone());
    let deadline = std::time::Instant::now() + Duration::from_secs(8);
    loop {
        vcx.run_until_parked();
        if page.read_with(&vcx, |p, _| !p.discovering && p.profile_items.len() == 1) { break; }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    click(&mut vcx, "usage-profile");
    vcx.simulate_keystrokes("down enter");
    vcx.run_until_parked();
    http.reply(200, br#"{"code":0,"data":{"balance":"0.00","currency":"CNY"}}"#);
    http.reply(200, br#"{"code":0,"data":{"userId":"1024","idc":310}}"#);
    http.reply(200, br#"{"code":0,"data":null}"#);
    click(&mut vcx, "usage-submit");
    settle(&mut vcx, |d| d.stored.accounts.len() == 1);
    let id = vcx.update(|_, cx| {
        let account = &cx.global::<UsageGlobal>().0.read(cx).stored.accounts[0];
        assert!(matches!(account.auth, Authentication::BrowserCookies { provider: Provider::MiMo, .. }));
        let Balance::Provider(report) = &account.snapshot.as_ref().unwrap().balance else { panic!("provider report") };
        assert_eq!(report.pools[0].remaining.as_deref(), Some("0.00"));
        account.id
    });
    assert!(vault.is_empty());
    let metadata = std::fs::read_to_string(dir.path().join("usage.json")).unwrap();
    assert!(!metadata.contains("isolated-browser-session") && !metadata.contains(root.to_str().unwrap()));
    db.execute("UPDATE moz_cookies SET value='rotated-session' WHERE name='api-platform_serviceToken'", []).unwrap();
    click(&mut vcx, &format!("usage-refresh-{id}"));
    settle(&mut vcx, |d| d.stored.accounts[0].error == Some(UsageError::BrowserChanged));
    let Balance::Provider(report) = vcx.update(|_, cx| cx.global::<UsageGlobal>().0.read(cx).stored.accounts[0].snapshot.as_ref().unwrap().balance.clone()) else { panic!("old snapshot retained") };
    assert_eq!(report.identity.as_deref(), Some("1024"));
    assert_eq!(http.calls.load(Ordering::SeqCst), 3, "changed source must not start a new provider request");
    app.update_in(&mut vcx, |a, w, cx| a.close_settings(w, cx));
    vcx.executor().advance_clock(Duration::from_secs(300));
    vcx.run_until_parked();
    assert_eq!(http.calls.load(Ordering::SeqCst), 3);
}

struct StepFunLoopback(String);
impl BalanceHttp for StepFunLoopback {
    fn get(&self, request: &UsageRequest<'_>, _: Option<&str>) -> Result<(u16, Vec<u8>), UsageError> {
        let path = request.endpoint().strip_prefix("https://platform.stepfun.com").expect("StepFun target only");
        DeepSeekHttp::send(request, None, &format!("{}{path}", self.0))
    }
}

#[gpui_kit::test]
fn stepfun_manual_settings_workflow_sends_real_connect_requests_and_preserves_identity(cx: &mut TestAppContext) {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let mut requests = Vec::new();
        for response in [
            r#"{"status":"CODE_OK","planFamily":"PLAN_FAMILY_CODING","fiveHourUsageLeftRate":0.25,"weeklyUsageLeftRate":0.0}"#,
            r#"{"status":"CODE_OK","uid":"123456","activated":true}"#,
            r#"{"status":"CODE_OK","subscription":{"name":"Pro"}}"#,
        ] {
            let deadline = std::time::Instant::now() + Duration::from_secs(10);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(std::time::Instant::now() < deadline);
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Err(error) => panic!("controlled server accept: {error}"),
                }
            };
            stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            let mut header = Vec::new();
            let mut byte = [0];
            while !header.ends_with(b"\r\n\r\n") {
                assert_eq!(stream.read(&mut byte).unwrap(), 1);
                header.push(byte[0]);
            }
            let mut body = [0; 2];
            stream.read_exact(&mut body).unwrap();
            assert_eq!(&body, b"{}");
            requests.push(String::from_utf8(header).unwrap());
            write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}", response.len()).unwrap();
        }
        requests
    });
    let dir = tempfile::tempdir().unwrap();
    let vault = Arc::new(InMemoryCredentialStore::new());
    let (app, mut vcx) = harness(cx);
    install(&mut vcx, dir.path().join("usage.json"), vault.clone(), Arc::new(StepFunLoopback(format!("http://{address}"))), false);
    open(&app, &mut vcx);
    click(&mut vcx, "usage-add");
    click(&mut vcx, "usage-provider");
    vcx.simulate_keystrokes("down enter");
    vcx.run_until_parked();
    type_into(&mut vcx, "usage-name", "StepFun manual");
    type_into(&mut vcx, "usage-key", "Oasis-Token=manual-token+==; Oasis-Webid=selected-web-id");
    click(&mut vcx, "usage-submit");
    settle(&mut vcx, |d| d.stored.accounts.len() == 1);
    let account = vcx.update(|_, cx| cx.global::<UsageGlobal>().0.read(cx).stored.accounts[0].clone());
    let Balance::Provider(report) = &account.snapshot.unwrap().balance else { panic!("provider report") };
    assert_eq!(report.identity.as_deref(), Some("123456"));
    assert_eq!(report.pools[0].remaining.as_deref(), Some("25"));
    assert_eq!(report.pools[1].remaining.as_deref(), Some("0"));
    assert_eq!(Accounts::load(dir.path().join("usage.json"), vault).unwrap().secret(account.id).unwrap(), "Oasis-Token=manual-token+==; Oasis-Webid=selected-web-id");
    let requests = server.join().unwrap();
    for (request, method) in requests.iter().zip(["QueryStepPlanRateLimit", "UserInfo", "GetStepPlanStatus"]) {
        let lower = request.to_ascii_lowercase();
        assert!(request.starts_with(&format!("POST /api/step.openapi.devcenter.Dashboard/{method} HTTP/1.1\r\n")));
        assert!(lower.contains("\r\nconnect-protocol-version: 1\r\n"));
        assert!(lower.contains("\r\noasis-appid: 10300\r\n"));
        assert!(lower.contains("\r\noasis-token: manual-token+==\r\n"));
        assert!(lower.contains("\r\noasis-webid: selected-web-id\r\n"));
        assert!(lower.contains("\r\ncookie: oasis-token=manual-token%2b%3d%3d\r\n"));
        assert!(!lower.contains("authorization:") && !lower.contains("device_id"));
    }
}

#[gpui_kit::test]
fn stepfun_browser_workflow_reads_webid_from_selected_local_storage_not_cookie_or_jwt(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("isolated-browser");
    let profile = root.join("Profiles/isolated.work");
    let storage = profile.join("storage/default/https+++platform.stepfun.com/ls");
    std::fs::create_dir_all(&storage).unwrap();
    std::fs::write(root.join("profiles.ini"), "[Profile0]\nName=StepFun work\nIsRelative=1\nPath=Profiles/isolated.work\n").unwrap();
    let db = rusqlite::Connection::open(profile.join("cookies.sqlite")).unwrap();
    db.execute_batch("CREATE TABLE moz_cookies (host TEXT,name TEXT,value TEXT,path TEXT,expiry INTEGER,originAttributes TEXT);
        INSERT INTO moz_cookies VALUES ('.stepfun.com','Oasis-Token','selected-cookie','/',4102444800,'');
        INSERT INTO moz_cookies VALUES ('.stepfun.com','Oasis-Webid','wrong-cookie-device','/',4102444800,'');").unwrap();
    let local = rusqlite::Connection::open(storage.join("data.sqlite")).unwrap();
    local.execute_batch("CREATE TABLE data (key TEXT PRIMARY KEY,utf16_length INTEGER,conversion_type INTEGER,compression_type INTEGER,value BLOB);").unwrap();
    local.execute("INSERT INTO data VALUES ('web_id', 17, 1, 0, ?1)", [b"selected-local-id".as_slice()]).unwrap();
    let (app, mut vcx) = harness(cx);
    let vault = Arc::new(InMemoryCredentialStore::new());
    let http = Arc::new(Http::default());
    install(&mut vcx, dir.path().join("usage.json"), vault.clone(), http.clone(), false);
    open(&app, &mut vcx);
    click(&mut vcx, "usage-add");
    click(&mut vcx, "usage-provider");
    vcx.simulate_keystrokes("down enter");
    vcx.run_until_parked();
    type_into(&mut vcx, "usage-name", "Browser StepFun");
    click(&mut vcx, "usage-source");
    vcx.simulate_keystrokes("down enter");
    vcx.run_until_parked();
    click(&mut vcx, "usage-browser");
    vcx.simulate_keystrokes("down down down down down enter");
    vcx.run_until_parked();
    let page = app.update_in(&mut vcx, |a, _, _| a.active_settings().unwrap().usage.clone());
    let deadline = std::time::Instant::now() + Duration::from_secs(8);
    loop {
        vcx.run_until_parked();
        if page.read_with(&vcx, |p, _| !p.discovering && p.profile_items.len() == 1) { break; }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    click(&mut vcx, "usage-profile");
    vcx.simulate_keystrokes("down enter");
    vcx.run_until_parked();
    http.reply(200, br#"{"status":"CODE_OK","planFamily":"PLAN_FAMILY_CODING","fiveHourUsageLeftRate":0.5}"#);
    http.reply(200, br#"{"status":"CODE_OK","uid":"70042","activated":true}"#);
    http.reply(200, br#"{"status":"CODE_OK","subscription":{"name":"Pro"}}"#);
    click(&mut vcx, "usage-submit");
    settle(&mut vcx, |d| d.stored.accounts.len() == 1);
    let id = vcx.update(|_, cx| {
        let account = &cx.global::<UsageGlobal>().0.read(cx).stored.accounts[0];
        assert!(matches!(account.auth, Authentication::BrowserCookies { provider: Provider::StepFun, .. }));
        let Balance::Provider(report) = &account.snapshot.as_ref().unwrap().balance else { panic!("provider report") };
        assert_eq!(report.identity.as_deref(), Some("70042"));
        assert_eq!(report.pools[0].remaining.as_deref(), Some("50"));
        assert!(report.pools[1].remaining.is_none());
        account.id
    });
    assert!(vault.is_empty());
    let metadata = std::fs::read_to_string(dir.path().join("usage.json")).unwrap();
    assert!(!metadata.contains("selected-cookie") && !metadata.contains("selected-local-id") && !metadata.contains("wrong-cookie-device"));
    local.execute("UPDATE data SET value=?1 WHERE key='web_id'", [b"another-profile-id".as_slice()]).unwrap();
    click(&mut vcx, &format!("usage-refresh-{id}"));
    settle(&mut vcx, |d| d.stored.accounts[0].error == Some(UsageError::BrowserChanged));
    assert_eq!(http.calls.load(Ordering::SeqCst), 3);
    assert!(vault.is_empty());
}
