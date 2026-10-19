use super::*;
use crate::core::keychain::{CredentialStore, InMemoryCredentialStore};
use crate::ui::{app::test_window::harness, settings::SettingsSection};
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{AppContext, TestAppContext, VisualTestContext};
use std::{collections::VecDeque, sync::mpsc};

const BALANCE: &[u8] = br#"{"is_available":true,"balance_infos":[{"currency":"CNY","total_balance":"110.00","granted_balance":"10.00","topped_up_balance":"100.00"}]}"#;
struct Reply {
    status: u16,
    body: Vec<u8>,
    release: Option<mpsc::Receiver<()>>,
}
#[derive(Default)]
struct Http {
    replies: Mutex<VecDeque<Reply>>,
    calls: AtomicU64,
    requests: Mutex<Vec<(String, String, String)>>,
}
impl Http {
    fn reply(&self, status: u16, body: &[u8]) {
        self.replies.lock().unwrap().push_back(Reply {
            status,
            body: body.to_vec(),
            release: None,
        });
    }
    fn held(&self, status: u16, body: &[u8]) -> mpsc::Sender<()> {
        let (tx, rx) = mpsc::channel();
        self.replies.lock().unwrap().push_back(Reply {
            status,
            body: body.to_vec(),
            release: Some(rx),
        });
        tx
    }
}
impl BalanceHttp for Http {
    fn get(
        &self,
        request: &UsageRequest<'_>,
        _: Option<&str>,
    ) -> Result<(u16, Vec<u8>), UsageError> {
        self.requests.lock().unwrap().push((
            request.endpoint().to_owned(),
            request.header_name().to_owned(),
            request.authorization_value(),
        ));
        self.calls.fetch_add(1, Ordering::SeqCst);
        let reply = self
            .replies
            .lock()
            .unwrap()
            .pop_front()
            .expect("unexpected external request");
        if let Some(release) = reply.release {
            release.recv_timeout(Duration::from_secs(10)).unwrap();
        }
        if reply.status == 0 {
            return Err(UsageError::Network);
        }
        Ok((reply.status, reply.body))
    }
}
fn install(
    vcx: &mut VisualTestContext,
    path: std::path::PathBuf,
    vault: Arc<dyn CredentialStore>,
    http: Arc<dyn BalanceHttp>,
    seed: bool,
) -> Option<Uuid> {
    let mut accounts = Accounts::load(path.clone(), vault).unwrap();
    let id = seed.then(|| {
        accounts
            .save_verified(
                None,
                "Personal",
                VerifiedSource::ApiKey("seed-key"),
                accounts::decode(200, BALANCE).unwrap(),
                100,
            )
            .unwrap()
    });
    let stored = accounts.stored.clone();
    let shared = Arc::new(Shared {
        accounts: Mutex::new(Ok(accounts)),
        http,
        browser: Arc::new(browser::BrowserReader::with_root(
            browser::Browser::Firefox,
            path.parent().unwrap().join("isolated-browser"),
        )),
        presentation: AtomicU64::new(1),
    });
    vcx.update(|_, cx| {
        let data = cx.new(|_| UsageData {
            shared,
            stored,
            error: None,
            presentation: 0,
            loading: false,
        });
        cx.set_global(UsageGlobal(data));
    });
    id
}
fn click(vcx: &mut VisualTestContext, id: &str) {
    vcx.update(|window, cx| {
        window.render_frame(cx);
        window.click(id.to_string(), cx);
    });
    vcx.run_until_parked();
}
fn type_into(vcx: &mut VisualTestContext, id: &str, value: &str) {
    vcx.update(|window, cx| {
        window.click(id.to_string(), cx);
        window.press("secondary-a", cx);
        window.input(value, cx);
    });
    vcx.run_until_parked();
}
fn settle(vcx: &mut VisualTestContext, condition: impl Fn(&UsageData) -> bool) {
    let deadline = std::time::Instant::now() + Duration::from_secs(8);
    loop {
        vcx.run_until_parked();
        if vcx.update(|_, cx| condition(cx.global::<UsageGlobal>().0.read(cx))) {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "usage workflow did not reach expected observable state"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}
fn open(app: &Entity<LyrelApp>, vcx: &mut VisualTestContext) {
    vcx.simulate_resize(gpui_kit::size(gpui_kit::px(1200.), gpui_kit::px(1400.)));
    app.update_in(vcx, |app, w, cx| {
        app.open_settings_section(SettingsSection::Usage, w, cx)
    });
    vcx.run_until_parked();
}

#[gpui_kit::test]
fn settings_account_lifecycle_validates_before_saving_and_removes_only_its_own_slots(
    cx: &mut TestAppContext,
) {
    let dir = tempfile::tempdir().unwrap();
    let (app, mut vcx) = harness(cx);
    let vault = Arc::new(InMemoryCredentialStore::new());
    vault
        .set_password("user", "unrelated", 22, "ssh-secret")
        .unwrap();
    let http = Arc::new(Http::default());
    install(
        &mut vcx,
        dir.path().join("usage.json"),
        vault.clone(),
        http.clone(),
        false,
    );
    assert_eq!(http.calls.load(Ordering::SeqCst), 0);
    open(&app, &mut vcx);
    http.reply(401, b"{}");
    click(&mut vcx, "usage-add");
    type_into(&mut vcx, "usage-name", "same");
    type_into(&mut vcx, "usage-key", "invalid");
    click(&mut vcx, "usage-submit");
    let page = app.update_in(&mut vcx, |a, _, _| {
        a.active_settings().unwrap().usage.clone()
    });
    let deadline = std::time::Instant::now() + Duration::from_secs(8);
    loop {
        vcx.run_until_parked();
        if page.read_with(&vcx, |p, _| p.error) == Some(UsageError::Authentication) {
            break;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(vault.len(), 1);
    assert!(!dir.path().join("usage.json").exists());
    http.reply(200, BALANCE);
    type_into(&mut vcx, "usage-key", "first-key");
    click(&mut vcx, "usage-submit");
    settle(&mut vcx, |d| {
        d.stored.accounts.iter().any(|a| {
            a.name == "same"
                && a.snapshot.as_ref().is_some_and(|s| {
                    s.balance.official().unwrap().balance_infos[0].total_balance == "110.00"
                })
        })
    });
    let first = vcx.update(|_, cx| cx.global::<UsageGlobal>().0.read(cx).stored.accounts[0].id);
    click(&mut vcx, &format!("usage-rename-{first}"));
    type_into(&mut vcx, "usage-name", "Work");
    click(&mut vcx, "usage-submit");
    settle(&mut vcx, |d| {
        d.stored
            .accounts
            .iter()
            .any(|a| a.id == first && a.name == "Work")
    });
    http.reply(200, BALANCE);
    click(&mut vcx, "usage-add");
    type_into(&mut vcx, "usage-name", "Work");
    type_into(&mut vcx, "usage-key", "second-key");
    click(&mut vcx, "usage-submit");
    settle(&mut vcx, |d| {
        d.stored
            .accounts
            .iter()
            .any(|a| a.id != first && a.name == "Work")
    });
    let second = vcx.update(|_, cx| {
        cx.global::<UsageGlobal>()
            .0
            .read(cx)
            .stored
            .accounts
            .iter()
            .find(|a| a.id != first)
            .unwrap()
            .id
    });
    click(&mut vcx, &format!("usage-toggle-{second}"));
    settle(&mut vcx, |d| {
        d.stored
            .accounts
            .iter()
            .any(|a| a.id == second && !a.enabled)
    });
    click(&mut vcx, &format!("usage-remove-{first}"));
    click(&mut vcx, "usage-cancel");
    assert_eq!(vault.len(), 3);
    click(&mut vcx, &format!("usage-remove-{first}"));
    click(&mut vcx, "usage-submit");
    settle(&mut vcx, |d| {
        !d.stored.accounts.iter().any(|a| a.id == first)
    });
    let reopened = Accounts::load(dir.path().join("usage.json"), vault.clone()).unwrap();
    assert_eq!(reopened.secret(second).unwrap(), "second-key");
    assert_eq!(
        vault
            .password_for("user", "unrelated", 22)
            .unwrap()
            .as_deref(),
        Some("ssh-secret")
    );
    assert_eq!(vault.len(), 2);
    let metadata = std::fs::read_to_string(dir.path().join("usage.json")).unwrap();
    assert!(!metadata.contains("first-key") && !metadata.contains("second-key"));
}

#[gpui_kit::test]
fn visible_page_polling_stops_on_search_navigation_and_close_even_after_late_response(
    cx: &mut TestAppContext,
) {
    let dir = tempfile::tempdir().unwrap();
    let (app, mut vcx) = harness(cx);
    let http = Arc::new(Http::default());
    let id = install(
        &mut vcx,
        dir.path().join("usage.json"),
        Arc::new(InMemoryCredentialStore::new()),
        http.clone(),
        true,
    )
    .unwrap();
    vcx.executor().advance_clock(Duration::from_secs(600));
    vcx.run_until_parked();
    assert_eq!(
        http.calls.load(Ordering::SeqCst),
        0,
        "startup must not query"
    );
    open(&app, &mut vcx);
    vcx.update(|w, cx| {
        w.render_frame(cx);
        w.within("usage-cadence").click("input", cx);
    });
    vcx.simulate_keystrokes("down enter");
    settle(&mut vcx, |d| d.stored.interval_secs == 60);
    let release = http.held(429, b"{}");
    vcx.executor().advance_clock(Duration::from_secs(59));
    vcx.run_until_parked();
    assert_eq!(http.calls.load(Ordering::SeqCst), 0);
    vcx.executor().advance_clock(Duration::from_secs(1));
    let deadline = std::time::Instant::now() + Duration::from_secs(8);
    while http.calls.load(Ordering::SeqCst) == 0 {
        vcx.run_until_parked();
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    click(&mut vcx, "usage-refresh-all");
    app.update_in(&mut vcx, |a, _, cx| {
        a.select_settings_section(SettingsSection::General, cx)
    });
    release.send(()).unwrap();
    settle(&mut vcx, |d| {
        d.stored
            .accounts
            .iter()
            .any(|a| a.id == id && a.error == Some(UsageError::RateLimited))
    });
    vcx.executor().advance_clock(Duration::from_secs(300));
    vcx.run_until_parked();
    assert_eq!(
        http.calls.load(Ordering::SeqCst),
        1,
        "late response must not rearm hidden page"
    );
    open(&app, &mut vcx);
    app.update_in(&mut vcx, |a, w, cx| {
        let search = a.active_settings().unwrap().search.clone();
        search.update(cx, |s, cx| s.set_value("DeepSeek", w, cx));
        a.autoselect_settings_search(cx);
    });
    vcx.executor().advance_clock(Duration::from_secs(300));
    vcx.run_until_parked();
    assert_eq!(http.calls.load(Ordering::SeqCst), 1);
    click(&mut vcx, "usage-search-open");
    app.update_in(&mut vcx, |a, w, cx| a.close_settings(w, cx));
    vcx.executor().advance_clock(Duration::from_secs(300));
    vcx.run_until_parked();
    assert_eq!(http.calls.load(Ordering::SeqCst), 1);
}

#[gpui_kit::test]
fn replacing_and_removing_accounts_rejects_late_refreshes_through_settings(
    cx: &mut TestAppContext,
) {
    let dir = tempfile::tempdir().unwrap();
    let (app, mut vcx) = harness(cx);
    let http = Arc::new(Http::default());
    let vault = Arc::new(InMemoryCredentialStore::new());
    let id = install(
        &mut vcx,
        dir.path().join("usage.json"),
        vault.clone(),
        http.clone(),
        true,
    )
    .unwrap();
    open(&app, &mut vcx);
    let old = http.held(200, BALANCE);
    click(&mut vcx, &format!("usage-refresh-{id}"));
    let deadline = std::time::Instant::now() + Duration::from_secs(8);
    while http.calls.load(Ordering::SeqCst) == 0 {
        vcx.run_until_parked();
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    let usd=br#"{"is_available":false,"balance_infos":[{"currency":"USD","total_balance":"0.00","granted_balance":"0.00","topped_up_balance":"0.00"}]}"#;
    http.reply(200, usd);
    click(&mut vcx, &format!("usage-replace-{id}"));
    type_into(&mut vcx, "usage-key", "replacement-key");
    click(&mut vcx, "usage-submit");
    settle(&mut vcx, |d| {
        d.stored.accounts.iter().any(|a| {
            a.id == id
                && a.snapshot.as_ref().is_some_and(|s| {
                    s.balance.official().unwrap().balance_infos[0].currency == "USD"
                })
        })
    });
    old.send(()).unwrap();
    let page = app.update_in(&mut vcx, |a, _, _| {
        a.active_settings().unwrap().usage.clone()
    });
    let deadline = std::time::Instant::now() + Duration::from_secs(8);
    loop {
        vcx.run_until_parked();
        if page.read_with(&vcx, |p, _| p.pending.is_empty()) {
            break;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    vcx.update(|_, cx| {
        let account = &cx.global::<UsageGlobal>().0.read(cx).stored.accounts[0];
        assert_eq!(
            account
                .snapshot
                .as_ref()
                .unwrap()
                .balance
                .official()
                .unwrap()
                .balance_infos[0]
                .currency,
            "USD"
        );
        assert_eq!(account.error, None);
    });
    assert_eq!(
        Accounts::load(dir.path().join("usage.json"), vault.clone())
            .unwrap()
            .secret(id)
            .unwrap(),
        "replacement-key"
    );
    let deleted = http.held(200, BALANCE);
    click(&mut vcx, &format!("usage-refresh-{id}"));
    let deadline = std::time::Instant::now() + Duration::from_secs(8);
    while http.calls.load(Ordering::SeqCst) < 3 {
        vcx.run_until_parked();
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    click(&mut vcx, &format!("usage-remove-{id}"));
    click(&mut vcx, "usage-submit");
    settle(&mut vcx, |d| d.stored.accounts.is_empty());
    deleted.send(()).unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(8);
    loop {
        vcx.run_until_parked();
        if page.read_with(&vcx, |p, _| p.pending.is_empty()) {
            break;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        Accounts::load(dir.path().join("usage.json"), vault.clone())
            .unwrap()
            .account(id)
            .is_none()
    );
    assert!(vault.is_empty());
}

#[derive(Default)]
struct LockedVault {
    inner: InMemoryCredentialStore,
    locked: AtomicBool,
}
impl CredentialStore for LockedVault {
    fn get(&self, s: &str, a: &str) -> crate::core::keychain::CredentialResult<Option<String>> {
        self.inner.get(s, a)
    }
    fn set(&self, s: &str, a: &str, v: &str) -> crate::core::keychain::CredentialResult<()> {
        if self.locked.load(Ordering::SeqCst) {
            return Err(crate::core::keychain::CredentialError::Backend(
                "locked".into(),
            ));
        }
        self.inner.set(s, a, v)
    }
    fn delete(&self, s: &str, a: &str) -> crate::core::keychain::CredentialResult<()> {
        self.inner.delete(s, a)
    }
}

#[gpui_kit::test]
fn settings_reports_vault_and_query_failures_without_losing_old_balance_or_key(
    cx: &mut TestAppContext,
) {
    let dir = tempfile::tempdir().unwrap();
    let (app, mut vcx) = harness(cx);
    let http = Arc::new(Http::default());
    let vault = Arc::new(LockedVault::default());
    let id = install(
        &mut vcx,
        dir.path().join("usage.json"),
        vault.clone(),
        http.clone(),
        true,
    )
    .unwrap();
    open(&app, &mut vcx);
    vault.locked.store(true, Ordering::SeqCst);
    http.reply(200, BALANCE);
    click(&mut vcx, &format!("usage-replace-{id}"));
    type_into(&mut vcx, "usage-key", "unpersisted-new-key");
    click(&mut vcx, "usage-submit");
    settle(&mut vcx, |d| {
        d.stored
            .accounts
            .iter()
            .any(|a| a.error == Some(UsageError::Credentials))
    });
    assert_eq!(
        Accounts::load(dir.path().join("usage.json"), vault.clone())
            .unwrap()
            .secret(id)
            .unwrap(),
        "seed-key"
    );
    assert_eq!(vault.inner.len(), 1);
    click(&mut vcx, "usage-cancel");
    for (status, body, error) in [
        (401, b"{}".as_slice(), UsageError::Authentication),
        (429, b"{}".as_slice(), UsageError::RateLimited),
        (0, b"".as_slice(), UsageError::Network),
        (
            200,
            br#"{"is_available":true,"balance_infos":[]}"#.as_slice(),
            UsageError::Response,
        ),
    ] {
        http.reply(status, body);
        click(&mut vcx, &format!("usage-refresh-{id}"));
        settle(&mut vcx, |d| {
            d.stored.accounts.iter().any(|a| a.error == Some(error))
        });
        vcx.update(|_, cx| {
            let snapshot = cx.global::<UsageGlobal>().0.read(cx).stored.accounts[0]
                .snapshot
                .as_ref()
                .unwrap();
            assert_eq!(snapshot.updated_at, 100);
            assert_eq!(
                snapshot.balance.official().unwrap().balance_infos[0].total_balance,
                "110.00"
            );
        });
    }
}

#[gpui_kit::test]
fn closing_settings_during_key_validation_cannot_create_an_account_or_save_a_secret(
    cx: &mut TestAppContext,
) {
    let dir = tempfile::tempdir().unwrap();
    let (app, mut vcx) = harness(cx);
    let http = Arc::new(Http::default());
    let vault = Arc::new(InMemoryCredentialStore::new());
    install(
        &mut vcx,
        dir.path().join("usage.json"),
        vault.clone(),
        http.clone(),
        false,
    );
    open(&app, &mut vcx);
    let release = http.held(200, BALANCE);
    click(&mut vcx, "usage-add");
    type_into(&mut vcx, "usage-name", "Cancelled");
    type_into(&mut vcx, "usage-key", "cancelled-key");
    click(&mut vcx, "usage-submit");
    let deadline = std::time::Instant::now() + Duration::from_secs(8);
    while http.calls.load(Ordering::SeqCst) == 0 {
        vcx.run_until_parked();
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    let page = app.update_in(&mut vcx, |a, _, _| {
        a.active_settings().unwrap().usage.clone()
    });
    app.update_in(&mut vcx, |a, w, cx| a.close_settings(w, cx));
    release.send(()).unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(8);
    loop {
        vcx.run_until_parked();
        if page.read_with(&vcx, |p, _| !p.submitting) {
            break;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(vault.is_empty());
    assert!(!dir.path().join("usage.json").exists());
    vcx.executor().advance_clock(Duration::from_secs(600));
    vcx.run_until_parked();
    assert_eq!(http.calls.load(Ordering::SeqCst), 1);
}

#[gpui_kit::test]
fn escape_cancels_the_usage_form_without_closing_settings_or_persisting_a_key(
    cx: &mut TestAppContext,
) {
    let dir = tempfile::tempdir().unwrap();
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
    vcx.simulate_resize(gpui_kit::size(gpui_kit::px(720.), gpui_kit::px(560.)));
    vcx.update(|w, cx| {
        w.render_frame(cx);
        for _ in 0..16 {
            if w.find("usage-add").focused() == Some(true) {
                break;
            }
            w.press("tab", cx);
        }
        assert_eq!(
            w.find("usage-add").focused(),
            Some(true),
            "Add is reachable through the native Tab order"
        );
        w.press("enter", cx);
    });
    vcx.run_until_parked();
    app.update_in(&mut vcx, |a, _, cx| {
        assert!(a.active_settings().unwrap().usage.read(cx).edit.is_some())
    });
    vcx.simulate_keystrokes("escape");
    vcx.run_until_parked();
    app.update_in(&mut vcx, |a, _, cx| {
        assert!(a.has_settings());
        let page = a.active_settings().unwrap().usage.read(cx);
        assert!(page.edit.is_none());
        assert!(page.key.read(cx).value().is_empty());
    });
    vcx.update(|w, cx| {
        w.render_frame(cx);
        assert!(w.find("usage-add").visible());
        assert_eq!(w.find("usage-add").focused(), Some(true));
    });
    assert!(vault.is_empty());
    assert_eq!(http.calls.load(Ordering::SeqCst), 0);
}

const PLATFORM: &[u8] = br#"{"code":0,"data":{"biz_code":0,"biz_data":{"normal_wallets":[{"currency":"CNY","balance":"5.0000000000000000"}],"bonus_wallets":[{"currency":"CNY","balance":"0E-16"}]}}}"#;

#[gpui_kit::test]
fn private_manual_session_verifies_then_saves_and_failed_replacement_preserves_platform_source(
    cx: &mut TestAppContext,
) {
    let dir = tempfile::tempdir().unwrap();
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
    vcx.simulate_keystrokes("down enter");
    vcx.run_until_parked();
    type_into(&mut vcx, "usage-name", "Private platform");
    type_into(
        &mut vcx,
        "usage-key",
        r#"{"userToken":"private-session-material"}"#,
    );
    http.reply(200, PLATFORM);
    click(&mut vcx, "usage-submit");
    settle(&mut vcx, |d| d.stored.accounts.len() == 1);
    let id = vcx.update(|_, cx| {
        let a = &cx.global::<UsageGlobal>().0.read(cx).stored.accounts[0];
        assert!(matches!(
            a.auth,
            Authentication::ManualSession {
                kind: platform::SessionKind::WebToken,
                ..
            }
        ));
        let Balance::Platform(balance) = &a.snapshot.as_ref().unwrap().balance else {
            panic!("private wallets must not become official balance")
        };
        assert_eq!(balance.normal_wallets[0].balance, "5.0000000000000000");
        assert_eq!(balance.bonus_wallets[0].balance, "0E-16");
        a.id
    });
    assert_eq!(vault.len(), 1);
    assert_eq!(
        Accounts::load(dir.path().join("usage.json"), vault.clone())
            .unwrap()
            .secret(id)
            .unwrap(),
        "private-session-material"
    );
    assert!(
        !std::fs::read_to_string(dir.path().join("usage.json"))
            .unwrap()
            .contains("private-session-material")
    );
    click(&mut vcx, &format!("usage-replace-{id}"));
    type_into(&mut vcx, "usage-key", "expired-session-material");
    http.reply(200, br#"{"code":40003}"#);
    click(&mut vcx, "usage-submit");
    settle(&mut vcx, |d| {
        d.stored.accounts[0].error == Some(UsageError::Authentication)
    });
    assert_eq!(
        Accounts::load(dir.path().join("usage.json"), vault.clone())
            .unwrap()
            .secret(id)
            .unwrap(),
        "private-session-material"
    );
    assert_eq!(vault.len(), 1);
    let requests = http.requests.lock().unwrap();
    assert_eq!(requests[0].0, platform::ENDPOINT);
    assert_eq!(requests[0].1, "Authorization");
    assert_eq!(
        requests[0].2,
        format!("{} {}", "Bearer", "private-session-material")
    );
}

#[path = "private_workflow_tests.rs"]
mod private_workflow_tests;

#[path = "provider_workflow_tests.rs"]
mod provider_workflow_tests;

#[path = "console_workflow_tests.rs"]
mod console_workflow_tests;
