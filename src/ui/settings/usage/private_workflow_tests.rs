use super::*;

fn sample_firefox(root: &std::path::Path, name: &str, token: &str) -> std::path::PathBuf {
    let storage = root.join(format!(
        "Profiles/{name}/storage/default/https+++platform.deepseek.com/ls"
    ));
    std::fs::create_dir_all(&storage).unwrap();
    let path = storage.join("data.sqlite");
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch("CREATE TABLE data (key TEXT PRIMARY KEY, utf16_length INTEGER NOT NULL, conversion_type INTEGER NOT NULL, compression_type INTEGER NOT NULL, value BLOB NOT NULL)").unwrap();
    db.execute(
        "INSERT INTO data VALUES ('userToken', ?1, 1, 0, ?2)",
        rusqlite::params![token.len() as i64, token.as_bytes()],
    )
    .unwrap();
    db.execute(
        "INSERT INTO data VALUES ('refresh_token', 1, 1, 0, ?1)",
        [b"must-not-read-or-save".as_slice()],
    )
    .unwrap();
    path
}

#[gpui_kit::test]
fn browser_import_uses_selected_profile_keeps_material_out_of_vault_and_refuses_locked_or_changed_source(
    cx: &mut TestAppContext,
) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("isolated-browser");
    sample_firefox(&root, "a.personal", "unselected-personal-token");
    let selected = sample_firefox(&root, "z.work", "selected-work-token");
    std::fs::write(root.join("profiles.ini"), "[Profile0]\nName=Personal\nIsRelative=1\nPath=Profiles/a.personal\n[Profile1]\nName=Work\nIsRelative=1\nPath=Profiles/z.work\n").unwrap();
    let (app, mut vcx) = harness(cx);
    let vault = Arc::new(InMemoryCredentialStore::new());
    let http = Arc::new(Http::default());
    install(
        &mut vcx,
        dir.path().join("usage.json"),
        vault.clone(),
        http.clone(),
        false,
    );
    open(&app, &mut vcx);
    click(&mut vcx, "usage-add");
    click(&mut vcx, "usage-source");
    vcx.simulate_keystrokes("down down enter");
    vcx.run_until_parked();
    type_into(&mut vcx, "usage-name", "Work browser");
    click(&mut vcx, "usage-browser");
    vcx.simulate_keystrokes("down down down down down enter");
    vcx.run_until_parked();
    let page = app.update_in(&mut vcx, |a, _, _| {
        a.active_settings().unwrap().usage.clone()
    });
    let deadline = std::time::Instant::now() + Duration::from_secs(8);
    loop {
        vcx.run_until_parked();
        if page.read_with(&vcx, |p, _| !p.discovering && p.profile_items.len() == 2) {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "isolated profile discovery did not finish"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(
        http.calls.load(Ordering::SeqCst),
        0,
        "profile discovery must not query sessions"
    );
    click(&mut vcx, "usage-profile");
    vcx.simulate_keystrokes("down down enter");
    vcx.run_until_parked();
    http.reply(200, PLATFORM);
    click(&mut vcx, "usage-submit");
    settle(&mut vcx, |d| d.stored.accounts.len() == 1);
    let id = vcx.update(|_, cx| {
        let account = &cx.global::<UsageGlobal>().0.read(cx).stored.accounts[0];
        let Authentication::BrowserLocalStorage { selection } = &account.auth else {
            panic!("expected browser source")
        };
        assert_eq!(selection.label(), "Work");
        account.id
    });
    assert!(
        vault.is_empty(),
        "browser material must never enter CredentialStore"
    );
    let metadata = std::fs::read_to_string(dir.path().join("usage.json")).unwrap();
    for secret in [
        "selected-work-token",
        "unselected-personal-token",
        "must-not-read-or-save",
    ] {
        assert!(!metadata.contains(secret));
    }
    assert!(!metadata.contains(root.to_str().unwrap()));
    assert_eq!(
        http.requests.lock().unwrap()[0].2,
        format!("{} {}", "Bearer", "selected-work-token")
    );
    click(&mut vcx, "usage-cadence");
    vcx.simulate_keystrokes("down enter");
    settle(&mut vcx, |d| d.stored.interval_secs == 60);
    let lock = rusqlite::Connection::open(&selected).unwrap();
    lock.execute_batch("BEGIN EXCLUSIVE").unwrap();
    vcx.executor().advance_clock(Duration::from_secs(60));
    settle(&mut vcx, |d| {
        d.stored.accounts[0].error == Some(UsageError::BrowserLocked)
    });
    assert_eq!(
        http.calls.load(Ordering::SeqCst),
        1,
        "locked profile must not query another account"
    );
    lock.execute_batch("ROLLBACK").unwrap();
    lock.execute(
        "UPDATE data SET value=?1 WHERE key='userToken'",
        [b"another-account-token".as_slice()],
    )
    .unwrap();
    click(&mut vcx, &format!("usage-refresh-{id}"));
    settle(&mut vcx, |d| {
        d.stored.accounts[0].error == Some(UsageError::BrowserChanged)
    });
    assert_eq!(http.calls.load(Ordering::SeqCst), 1);
    assert!(vault.is_empty());
    app.update_in(&mut vcx, |a, w, cx| a.close_settings(w, cx));
    vcx.executor().advance_clock(Duration::from_secs(300));
    vcx.run_until_parked();
    assert_eq!(http.calls.load(Ordering::SeqCst), 1);
}

struct LoopbackHttp(String);
impl BalanceHttp for LoopbackHttp {
    fn get(
        &self,
        request: &UsageRequest<'_>,
        _: Option<&str>,
    ) -> Result<(u16, Vec<u8>), UsageError> {
        assert_eq!(request.endpoint(), platform::ENDPOINT);
        DeepSeekHttp::send(request, None, &self.0)
    }
}

#[gpui_kit::test]
fn harness_grant_settings_workflow_uses_real_http_header_and_persists_only_validated_token(
    cx: &mut TestAppContext,
) {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let mut stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "HTTP workflow never connected"
                    );
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(e) => panic!("controlled HTTP accept: {e}"),
            }
        };
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut request = Vec::new();
        let mut byte = [0];
        while !request.ends_with(b"\r\n\r\n") {
            assert_eq!(stream.read(&mut byte).unwrap(), 1);
            request.push(byte[0]);
        }
        write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", PLATFORM.len()).unwrap();
        stream.write_all(PLATFORM).unwrap();
        String::from_utf8(request).unwrap()
    });
    let dir = tempfile::tempdir().unwrap();
    let vault = Arc::new(InMemoryCredentialStore::new());
    let (app, mut vcx) = harness(cx);
    install(
        &mut vcx,
        dir.path().join("usage.json"),
        vault.clone(),
        Arc::new(LoopbackHttp(format!("http://{address}/controlled-summary"))),
        false,
    );
    open(&app, &mut vcx);
    click(&mut vcx, "usage-add");
    click(&mut vcx, "usage-source");
    vcx.simulate_keystrokes("down enter");
    vcx.run_until_parked();
    type_into(&mut vcx, "usage-name", "Harness");
    type_into(
        &mut vcx,
        "usage-key",
        r#"{"version":1,"token":"controlled-grant-token","issuer":"https://platform.deepseek.com"}"#,
    );
    click(&mut vcx, "usage-submit");
    settle(&mut vcx, |d| d.stored.accounts.len() == 1);
    let request = server.join().unwrap().to_ascii_lowercase();
    assert!(request.starts_with("get /controlled-summary http/1.1\r\n"));
    assert!(request.contains("\r\nx-dsh-auth-token: controlled-grant-token\r\n"));
    assert!(!request.contains("\r\nauthorization:") && !request.contains("\r\ncookie:"));
    let accounts = Accounts::load(dir.path().join("usage.json"), vault.clone()).unwrap();
    let id = accounts.stored.accounts[0].id;
    assert_eq!(accounts.secret(id).unwrap(), "controlled-grant-token");
    assert!(matches!(
        accounts.account(id).unwrap().auth,
        Authentication::ManualSession {
            kind: platform::SessionKind::HarnessGrant,
            ..
        }
    ));
    assert!(
        !std::fs::read_to_string(dir.path().join("usage.json"))
            .unwrap()
            .contains("controlled-grant-token")
    );
}

#[gpui_kit::test]
fn escape_dismisses_source_picker_before_form_and_restores_previous_focus(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let (app, mut vcx) = harness(cx);
    install(
        &mut vcx,
        dir.path().join("usage.json"),
        Arc::new(InMemoryCredentialStore::new()),
        Arc::new(Http::default()),
        false,
    );
    open(&app, &mut vcx);
    // The themed native Button deliberately preserves focus on pointer activation.
    // Keyboard-trigger focus restoration is exercised by the narrow-window test.
    let previous_focus = vcx.update(|window, cx| window.focused(cx).unwrap());
    click(&mut vcx, "usage-add");
    vcx.update(|window, cx| {
        window.render_frame(cx);
        window.within("usage-source").click("input", cx);
    });
    vcx.run_until_parked();
    vcx.simulate_keystrokes("escape");
    vcx.run_until_parked();
    app.update_in(&mut vcx, |app, _, cx| {
        assert!(
            app.active_settings().unwrap().usage.read(cx).edit.is_some(),
            "first Escape belongs to the source picker"
        );
    });
    vcx.update(|window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("usage-source").expanded(), Some(false));
    });
    vcx.simulate_keystrokes("escape");
    vcx.run_until_parked();
    app.update_in(&mut vcx, |app, _, cx| {
        assert!(app.active_settings().unwrap().usage.read(cx).edit.is_none())
    });
    vcx.update(|window, cx| {
        window.render_frame(cx);
        assert!(previous_focus.is_focused(window));
    });
}
