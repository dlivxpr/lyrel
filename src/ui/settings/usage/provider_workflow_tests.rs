use super::*;

fn scroll_usage_to_bottom(vcx: &mut VisualTestContext) {
    vcx.update(|window, cx| {
        window.render_frame(cx);
        window.scroll(
            "usage-refresh-all",
            gpui_kit::ScrollDelta::Pixels(gpui_kit::point(gpui_kit::px(0.), gpui_kit::px(-4000.))),
            cx,
        );
    });
    vcx.run_until_parked();
}

const KIMI: &[u8] = br#"{"usages":{"limit_5h":{"used_ratio":0.25,"reset_time":"2026-10-20T00:00:00Z"},"limit_7d":{"used_ratio":"0.5","reset_time":"2026-10-25T00:00:00Z"},"limit_month_total":{"used_ratio":0.6,"reset_time":"2026-11-01T00:00:00Z"},"limit_month_code":{"used_ratio":0.1,"reset_time":"2026-11-01T00:00:00Z"}},"boosterWallet":{"balance":{"type":"BOOSTER","amount":"1000000000","amountLeft":"500000000"},"monthlyChargeLimitEnabled":true,"monthlyChargeLimit":{"priceInCents":"2000","currency":"USD"},"monthlyUsed":{"priceInCents":"1500","currency":"USD"}}}"#;
const KIMI_ME: &[u8] = br#"{"user_id":"kimi-work","nickname":"Work"}"#;

#[gpui_kit::test]
fn kimi_api_settings_keeps_four_distinct_windows_and_failed_replacement_keeps_old_key(
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
    click(&mut vcx, "usage-provider");
    vcx.simulate_keystrokes("down enter");
    vcx.run_until_parked();
    type_into(&mut vcx, "usage-name", "Kimi work");
    type_into(&mut vcx, "usage-key", "kimi-controlled-key");
    http.reply(200, KIMI);
    http.reply(200, KIMI_ME);
    click(&mut vcx, "usage-submit");
    settle(&mut vcx, |d| d.stored.accounts.len() == 1);
    let id = vcx.update(|_, cx| {
        let account = &cx.global::<UsageGlobal>().0.read(cx).stored.accounts[0];
        let Balance::Provider(report) = &account.snapshot.as_ref().unwrap().balance else {
            panic!("provider report")
        };
        assert_eq!(report.identity.as_deref(), Some("kimi-work"));
        assert_eq!(
            report.pools[..4]
                .iter()
                .map(|p| (p.kind, p.remaining.as_deref().unwrap()))
                .collect::<Vec<_>>(),
            vec![
                (providers::PoolKind::FiveHour, "75"),
                (providers::PoolKind::Weekly, "50"),
                (providers::PoolKind::MonthlyTotal, "40"),
                (providers::PoolKind::MonthlyCode, "90")
            ]
        );
        assert_eq!(report.pools[4].remaining.as_deref(), Some("5.00"));
        assert_eq!(report.pools[4].limit.as_deref(), Some("10.00"));
        assert_eq!(report.pools[4].unit, "USD");
        assert_eq!(report.pools[5].remaining.as_deref(), Some("5.00"));
        assert_eq!(report.pools[5].used.as_deref(), Some("15.00"));
        assert_eq!(report.pools[5].limit.as_deref(), Some("20.00"));
        account.id
    });
    http.reply(401, b"{}");
    click(&mut vcx, &format!("usage-replace-{id}"));
    type_into(&mut vcx, "usage-key", "rejected-key");
    click(&mut vcx, "usage-submit");
    settle(&mut vcx, |d| {
        d.stored.accounts[0].error == Some(UsageError::Authentication)
    });
    let accounts = Accounts::load(dir.path().join("usage.json"), vault).unwrap();
    assert_eq!(accounts.secret(id).unwrap(), "kimi-controlled-key");
    assert_eq!(accounts.stored.accounts[0].provider, Provider::Kimi);
}

const KIMI_WEB: &[u8] = br#"{"ratelimit5h":{"ratio":0.25,"enabled":true,"resetTime":"2026-10-20T00:00:00Z"},"ratelimitCode5h":{"ratio":0.5,"enabled":true,"resetTime":"2026-10-20T00:00:00Z"},"ratelimit7d":{"ratio":0.1,"enabled":true,"resetTime":"2026-10-25T00:00:00Z"},"ratelimitCode7d":{"ratio":0.2,"enabled":true,"resetTime":"2026-10-25T00:00:00Z"},"subscriptionBalance":{"amountUsedRatio":0.3,"kimiCodeUsedRatio":0.1,"amount":"9007199254740993","amountLeft":"7000000000000001","unit":"UNIT_CREDIT","expireTime":"2026-11-01T00:00:00Z"},"giftBalances":[{"amountUsedRatio":0.5,"displayName":"Gift","expireTime":"2026-10-31T00:00:00Z"}],"boosterWallets":[{"moneyLeft":{"priceInCents":"500","currency":"USD"},"moneyTotal":{"priceInCents":"1000","currency":"USD"},"monthlyChargeLimitEnabled":true,"monthlyChargeLimit":{"priceInCents":"2000","currency":"USD"},"monthlyUsed":{"priceInCents":"1500","currency":"USD"}}]}"#;

#[gpui_kit::test]
fn kimi_browser_selection_reads_access_token_not_refresh_token_and_preserves_source(
    cx: &mut TestAppContext,
) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("isolated-browser");
    let storage = root.join("Profiles/work/storage/default/https+++www.kimi.com/ls");
    std::fs::create_dir_all(&storage).unwrap();
    std::fs::write(
        root.join("profiles.ini"),
        "[Profile0]\nName=Kimi work\nIsRelative=1\nPath=Profiles/work\n",
    )
    .unwrap();
    let db = rusqlite::Connection::open(storage.join("data.sqlite")).unwrap();
    db.execute_batch("CREATE TABLE data(key TEXT PRIMARY KEY, conversion_type INTEGER, compression_type INTEGER, value BLOB)").unwrap();
    for (key, value) in [
        ("access_token", "browser-kimi-token"),
        ("msh_user_id", "browser-kimi-user"),
        ("refresh_token", "never-read-refresh-token"),
    ] {
        db.execute(
            "INSERT INTO data VALUES (?1,1,0,?2)",
            rusqlite::params![key, value.as_bytes()],
        )
        .unwrap();
    }
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
    click(&mut vcx, "usage-provider");
    vcx.simulate_keystrokes("down enter");
    vcx.run_until_parked();
    click(&mut vcx, "usage-source");
    vcx.simulate_keystrokes("down down enter");
    vcx.run_until_parked();
    type_into(&mut vcx, "usage-name", "Browser Kimi");
    click(&mut vcx, "usage-browser");
    vcx.simulate_keystrokes("down down down down down enter");
    vcx.run_until_parked();
    let page = app.update_in(&mut vcx, |a, _, _| {
        a.active_settings().unwrap().usage.clone()
    });
    let deadline = std::time::Instant::now() + Duration::from_secs(8);
    loop {
        vcx.run_until_parked();
        if page.read_with(&vcx, |p, _| !p.discovering && p.profile_items.len() == 1) {
            break;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    click(&mut vcx, "usage-profile");
    vcx.simulate_keystrokes("down enter");
    vcx.run_until_parked();
    http.reply(200, KIMI_WEB);
    click(&mut vcx, "usage-submit");
    settle(&mut vcx, |d| d.stored.accounts.len() == 1);
    let id = vcx.update(|_, cx| {
        let account = &cx.global::<UsageGlobal>().0.read(cx).stored.accounts[0];
        let Balance::Provider(report) = &account.snapshot.as_ref().unwrap().balance else {
            panic!("Kimi report")
        };
        assert_eq!(report.identity.as_deref(), Some("browser-kimi-user"));
        assert_eq!(report.pools[0].remaining.as_deref(), Some("75"));
        assert_eq!(report.pools[1].remaining.as_deref(), Some("50"));
        assert_eq!(report.pools[1].unit, "%");
        let raw = report
            .pools
            .iter()
            .find(|p| p.kind == providers::PoolKind::RawBalance)
            .unwrap();
        assert_eq!(raw.limit.as_deref(), Some("9007199254740993"));
        assert_eq!(raw.remaining.as_deref(), Some("7000000000000001"));
        assert_eq!(raw.unit, "UNIT_CREDIT");
        account.id
    });
    assert!(vault.is_empty());
    let stored = std::fs::read_to_string(dir.path().join("usage.json")).unwrap();
    for secret in ["browser-kimi-token", "never-read-refresh-token"] {
        assert!(!stored.contains(secret));
    }
    assert_eq!(
        http.requests.lock().unwrap()[0].0,
        "https://www.kimi.com/apiv2/kimi.gateway.membership.v2.MembershipService/GetSubscriptionStats"
    );
    assert_eq!(
        http.requests.lock().unwrap()[0].2,
        "Bearer browser-kimi-token"
    );
    db.execute(
        "UPDATE data SET value=?1 WHERE key='access_token'",
        [b"different-kimi-user-token".as_slice()],
    )
    .unwrap();
    scroll_usage_to_bottom(&mut vcx);
    click(&mut vcx, &format!("usage-refresh-{id}"));
    settle(&mut vcx, |d| {
        d.stored.accounts[0].error == Some(UsageError::BrowserChanged)
    });
    assert_eq!(
        http.calls.load(Ordering::SeqCst),
        1,
        "must not query another account or fall back to key endpoint"
    );
}

#[gpui_kit::test]
fn kimi_manual_session_keeps_counts_and_old_snapshot_on_rate_limit_or_missing_usage(
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
    click(&mut vcx, "usage-provider");
    vcx.simulate_keystrokes("down enter");
    vcx.run_until_parked();
    click(&mut vcx, "usage-source");
    vcx.simulate_keystrokes("down enter");
    vcx.run_until_parked();
    type_into(&mut vcx, "usage-name", "Manual Kimi");
    type_into(
        &mut vcx,
        "usage-key",
        r#"{"access_token":"manual-kimi-token"}"#,
    );
    http.reply(200, KIMI_WEB);
    click(&mut vcx, "usage-submit");
    settle(&mut vcx, |d| d.stored.accounts.len() == 1);
    let id = vcx.update(|_, cx| cx.global::<UsageGlobal>().0.read(cx).stored.accounts[0].id);
    assert_eq!(
        Accounts::load(dir.path().join("usage.json"), vault.clone())
            .unwrap()
            .secret(id)
            .unwrap(),
        "manual-kimi-token"
    );
    scroll_usage_to_bottom(&mut vcx);
    for (status, body, error) in [
        (429, b"{}".as_slice(), UsageError::RateLimited),
        (200, b"{}".as_slice(), UsageError::Response),
        (0, b"{}".as_slice(), UsageError::Network),
        (401, b"{}".as_slice(), UsageError::Authentication),
    ] {
        http.reply(status, body);
        click(&mut vcx, &format!("usage-refresh-{id}"));
        settle(&mut vcx, |d| d.stored.accounts[0].error == Some(error));
        vcx.update(|_, cx| {
            let Balance::Provider(report) = &cx.global::<UsageGlobal>().0.read(cx).stored.accounts
                [0]
            .snapshot
            .as_ref()
            .unwrap()
            .balance
            else {
                panic!("Kimi report")
            };
            assert_eq!(report.pools[0].remaining.as_deref(), Some("75"));
            assert_eq!(
                report.pools[1].reset.as_deref(),
                Some("2026-10-20T00:00:00Z")
            );
            assert_eq!(
                report
                    .pools
                    .iter()
                    .find(|p| p.kind == providers::PoolKind::Booster)
                    .unwrap()
                    .remaining
                    .as_deref(),
                Some("5.00")
            );
        });
    }
    app.update_in(&mut vcx, |a, w, cx| a.close_settings(w, cx));
    vcx.executor().advance_clock(Duration::from_secs(300));
    vcx.run_until_parked();
    assert_eq!(http.calls.load(Ordering::SeqCst), 5);
}

const COMMAND_WHO: &[u8]=br#"{"success":true,"user":{"id":"command-user","userName":"work"},"org":{"id":"org_work","login":"work"}}"#;
const COMMAND_CREDITS: &[u8]=br#"{"credits":{"monthlyCredits":85,"monthlyCreditsGranted":100,"purchasedCredits":20,"freeCredits":3,"planId":"individual-pro"},"windowLimits":{"fiveHour":{"used":12,"cap":25,"resetAt":"2026-10-20T00:00:00Z","exceeded":false},"weekly":{"used":30,"cap":100,"resetAt":"2026-10-25T00:00:00Z","exceeded":false}}}"#;
const COMMAND_SUBSCRIPTION: &[u8]=br#"{"data":{"planId":"individual-pro","currentPeriodStart":"2026-10-01T00:00:00Z","currentPeriodEnd":"2026-11-01T00:00:00Z"}}"#;
const COMMAND_SUMMARY: &[u8] =
    br#"{"totalCost":15,"totalCount":123,"periodBasis":"billing-period"}"#;
fn reply_command(http: &Http) {
    http.reply(200, COMMAND_WHO);
    http.reply(200, COMMAND_CREDITS);
    http.reply(200, COMMAND_SUBSCRIPTION);
    http.reply(200, COMMAND_SUMMARY);
}

#[gpui_kit::test]
fn command_key_workflow_keeps_credit_sources_and_rolling_windows_separate(cx: &mut TestAppContext) {
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
    click(&mut vcx, "usage-provider");
    vcx.simulate_keystrokes("down down enter");
    vcx.run_until_parked();
    type_into(&mut vcx, "usage-name", "Command work");
    type_into(&mut vcx, "usage-key", "command-key");
    reply_command(&http);
    click(&mut vcx, "usage-submit");
    settle(&mut vcx, |d| d.stored.accounts.len() == 1);
    let id = vcx.update(|_, cx| {
        let account = &cx.global::<UsageGlobal>().0.read(cx).stored.accounts[0];
        assert_eq!(account.provider, Provider::CommandCode);
        let Balance::Provider(report) = &account.snapshot.as_ref().unwrap().balance else {
            panic!("Command report")
        };
        assert_eq!(report.identity.as_deref(), Some("command-user"));
        assert_eq!(report.plan.as_deref(), Some("individual-pro"));
        assert_eq!(
            report.pools[..5]
                .iter()
                .map(|p| (p.kind, p.remaining.as_deref()))
                .collect::<Vec<_>>(),
            vec![
                (providers::PoolKind::Monthly, Some("85")),
                (providers::PoolKind::Purchased, Some("20")),
                (providers::PoolKind::Free, Some("3")),
                (providers::PoolKind::FiveHour, Some("13")),
                (providers::PoolKind::Weekly, Some("70"))
            ]
        );
        assert_eq!(report.pools[5].used.as_deref(), Some("15"));
        assert_eq!(report.pools[5].unit, "USD");
        assert_eq!(report.pools[6].used.as_deref(), Some("123"));
        assert_eq!(
            report.pools[0].reset.as_deref(),
            Some("2026-11-01T00:00:00Z")
        );
        account.id
    });
    assert_eq!(
        Accounts::load(dir.path().join("usage.json"), vault)
            .unwrap()
            .secret(id)
            .unwrap(),
        "command-key"
    );
    let requests = http.requests.lock().unwrap();
    assert_eq!(
        requests[1].0,
        "https://api.commandcode.ai/alpha/billing/credits?orgId=org_work"
    );
    assert_eq!(
        requests[2].0,
        "https://api.commandcode.ai/alpha/billing/subscriptions?orgId=org_work"
    );
    assert_eq!(
        requests[0].0,
        "https://api.commandcode.ai/alpha/whoami?limits=1"
    );
    assert_eq!(
        requests[3].0,
        "https://api.commandcode.ai/alpha/usage/summary?orgId=org_work&since=2026-10-01T00%3A00%3A00Z"
    );
}

struct KimiLoopback(String);
impl BalanceHttp for KimiLoopback {
    fn get(
        &self,
        request: &UsageRequest<'_>,
        _: Option<&str>,
    ) -> Result<(u16, Vec<u8>), UsageError> {
        assert_eq!(
            request.endpoint(),
            "https://www.kimi.com/apiv2/kimi.gateway.membership.v2.MembershipService/GetSubscriptionStats"
        );
        DeepSeekHttp::send(request, None, &self.0)
    }
}
#[gpui_kit::test]
fn kimi_manual_settings_uses_real_post_body_and_bearer_without_cookie(cx: &mut TestAppContext) {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    listener.set_nonblocking(true).unwrap();
    let server = std::thread::spawn(move || {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let mut stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(std::time::Instant::now() < deadline);
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("{error}"),
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
        let header = String::from_utf8(request.clone()).unwrap();
        let length = header
            .lines()
            .find_map(|line| {
                line.to_ascii_lowercase()
                    .strip_prefix("content-length:")
                    .and_then(|v| v.trim().parse::<usize>().ok())
            })
            .unwrap();
        let mut body = vec![0; length];
        stream.read_exact(&mut body).unwrap();
        write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",KIMI_WEB.len()).unwrap();
        stream.write_all(KIMI_WEB).unwrap();
        (header, body)
    });
    let dir = tempfile::tempdir().unwrap();
    let (app, mut vcx) = harness(cx);
    let vault = Arc::new(InMemoryCredentialStore::new());
    install(
        &mut vcx,
        dir.path().join("usage.json"),
        vault,
        Arc::new(KimiLoopback(format!("http://{address}/controlled-kimi"))),
        false,
    );
    open(&app, &mut vcx);
    click(&mut vcx, "usage-add");
    click(&mut vcx, "usage-provider");
    vcx.simulate_keystrokes("down enter");
    vcx.run_until_parked();
    click(&mut vcx, "usage-source");
    vcx.simulate_keystrokes("down enter");
    vcx.run_until_parked();
    type_into(&mut vcx, "usage-name", "HTTP Kimi");
    type_into(&mut vcx, "usage-key", "controlled-http-kimi-token");
    click(&mut vcx, "usage-submit");
    settle(&mut vcx, |d| d.stored.accounts.len() == 1);
    let (header, body) = server.join().unwrap();
    let header = header.to_ascii_lowercase();
    assert!(header.starts_with("post /controlled-kimi http/1.1\r\n"));
    assert!(header.contains(&format!(
        "\r\nauthorization: {} {}\r\n",
        "bearer", "controlled-http-kimi-token"
    )));
    assert!(!header.contains("\r\ncookie:"));
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&body).unwrap(),
        serde_json::json!({"mute_notice":false})
    );
}

#[gpui_kit::test]
fn command_manual_enhancement_mismatch_and_expiry_never_replace_primary_key(
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
    click(&mut vcx, "usage-provider");
    vcx.simulate_keystrokes("down down enter");
    vcx.run_until_parked();
    type_into(&mut vcx, "usage-name", "Command primary");
    type_into(&mut vcx, "usage-key", "command-main-key");
    reply_command(&http);
    click(&mut vcx, "usage-submit");
    settle(&mut vcx, |d| d.stored.accounts.len() == 1);
    let id = vcx.update(|_, cx| cx.global::<UsageGlobal>().0.read(cx).stored.accounts[0].id);
    click(&mut vcx, &format!("usage-enhance-{id}"));
    type_into(
        &mut vcx,
        "usage-key",
        "better-auth.session_token=controlled-session",
    );
    reply_command(&http);
    http.reply(
        200,
        br#"{"user":{"id":"different-user"},"session":{"id":"controlled-session"}}"#,
    );
    click(&mut vcx, "usage-submit");
    settle(
        &mut vcx,
        |d| matches!(&d.stored.accounts[0].snapshot.as_ref().unwrap().balance,Balance::Provider(report) if report.enhancement==Some(UsageError::IdentityMismatch)),
    );
    let accounts = Accounts::load(dir.path().join("usage.json"), vault.clone()).unwrap();
    assert_eq!(accounts.secret(id).unwrap(), "command-main-key");
    assert!(accounts.enhancement_secret(id).unwrap().is_none());
    assert_eq!(vault.len(), 1);
    type_into(
        &mut vcx,
        "usage-key",
        "better-auth.session_token=matching-session",
    );
    reply_command(&http);
    http.reply(
        200,
        br#"{"user":{"id":"command-user"},"session":{"id":"matching-session"}}"#,
    );
    http.reply(200,br#"{"success":true,"data":{"planId":"individual-max","status":"active","currentPeriodEnd":"2026-12-01T00:00:00Z"}}"#);
    click(&mut vcx, "usage-submit");
    settle(
        &mut vcx,
        |d| matches!(&d.stored.accounts[0].snapshot.as_ref().unwrap().balance,Balance::Provider(report) if report.enhancement.is_none() && report.plan.as_deref()==Some("individual-max")),
    );
    assert_eq!(vault.len(), 2);
    reply_command(&http);
    http.reply(401, b"{}");
    click(&mut vcx, &format!("usage-refresh-{id}"));
    settle(
        &mut vcx,
        |d| matches!(&d.stored.accounts[0].snapshot.as_ref().unwrap().balance,Balance::Provider(report) if report.enhancement==Some(UsageError::Authentication)),
    );
    vcx.update(|_, cx| {
        let account = &cx.global::<UsageGlobal>().0.read(cx).stored.accounts[0];
        assert!(account.error.is_none());
        let Balance::Provider(report) = &account.snapshot.as_ref().unwrap().balance else {
            panic!("Command")
        };
        assert_eq!(report.pools[0].remaining.as_deref(), Some("85"));
        assert_eq!(report.plan.as_deref(), Some("individual-pro"));
    });
    click(&mut vcx, &format!("usage-remove-{id}"));
    click(&mut vcx, "usage-submit");
    settle(&mut vcx, |d| d.stored.accounts.is_empty());
    assert!(vault.is_empty());
}

#[gpui_kit::test]
fn kimi_settings_redirect_never_forwards_credentials_or_creates_account(cx: &mut TestAppContext) {
    use std::io::{Read, Write};
    let target = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    target.set_nonblocking(true).unwrap();
    let target_address = target.local_addr().unwrap();
    let origin = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    origin.set_nonblocking(true).unwrap();
    let address = origin.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let mut stream = loop {
            match origin.accept() {
                Ok((s, _)) => break s,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(std::time::Instant::now() < deadline);
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(e) => panic!("{e}"),
            }
        };
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut request = Vec::new();
        let mut b = [0];
        while !request.ends_with(b"\r\n\r\n") {
            assert_eq!(stream.read(&mut b).unwrap(), 1);
            request.push(b[0]);
        }
        // A close with an unread POST body can reset the TCP connection on
        // Windows, turning this redirect fixture into a transport failure.
        let headers = String::from_utf8(request).unwrap();
        assert!(headers.starts_with("POST /controlled-kimi HTTP/1.1\r\n"));
        let length = headers
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().unwrap())
            })
            .expect("production POST declares its body length");
        let mut body = vec![0; length];
        stream.read_exact(&mut body).unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&body).unwrap(),
            serde_json::json!({"mute_notice":false})
        );
        write!(stream,"HTTP/1.1 302 Found\r\nLocation: http://{target_address}/must-not-receive-token\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
    });
    let dir = tempfile::tempdir().unwrap();
    let (app, mut vcx) = harness(cx);
    let vault = Arc::new(InMemoryCredentialStore::new());
    install(
        &mut vcx,
        dir.path().join("usage.json"),
        vault.clone(),
        Arc::new(KimiLoopback(format!("http://{address}/controlled-kimi"))),
        false,
    );
    open(&app, &mut vcx);
    click(&mut vcx, "usage-add");
    click(&mut vcx, "usage-provider");
    vcx.simulate_keystrokes("down enter");
    vcx.run_until_parked();
    click(&mut vcx, "usage-source");
    vcx.simulate_keystrokes("down enter");
    vcx.run_until_parked();
    type_into(&mut vcx, "usage-name", "Redirect rejected");
    type_into(&mut vcx, "usage-key", "controlled-redirect-token");
    click(&mut vcx, "usage-submit");
    let page = app.update_in(&mut vcx, |a, _, _| {
        a.active_settings().unwrap().usage.clone()
    });
    let deadline = std::time::Instant::now() + Duration::from_secs(8);
    loop {
        vcx.run_until_parked();
        if page.read_with(&vcx, |p, _| !p.submitting) {
            assert_eq!(
                page.read_with(&vcx, |p, _| p.error),
                Some(UsageError::Service),
                "the completed request must report the rejected redirect, not a transport error"
            );
            break;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    server.join().unwrap();
    assert_eq!(
        target.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
    assert!(vault.is_empty());
    assert!(!dir.path().join("usage.json").exists());
}

fn command_browser_fixture(root: &std::path::Path) -> rusqlite::Connection {
    std::fs::create_dir_all(root.join("Profiles/a-empty")).unwrap();
    std::fs::create_dir_all(root.join("Profiles/b-work")).unwrap();
    std::fs::write(root.join("profiles.ini"), "[Profile0]\nName=Empty\nIsRelative=1\nPath=Profiles/a-empty\n[Profile1]\nName=Command work\nIsRelative=1\nPath=Profiles/b-work\n").unwrap();
    let db = rusqlite::Connection::open(root.join("Profiles/b-work/cookies.sqlite")).unwrap();
    db.execute_batch("CREATE TABLE moz_cookies(host TEXT,name TEXT,value TEXT,path TEXT,expiry INTEGER,originAttributes TEXT);
        INSERT INTO moz_cookies VALUES ('api.commandcode.ai','custom-command-session','browser-command-session','/',4102444800,'');
        INSERT INTO moz_cookies VALUES ('.commandcode.ai','__Secure-better-auth.session_token','never-send','/',4102444800,'');
        INSERT INTO moz_cookies VALUES ('.commandcode.ai.evil.invalid','custom-command-session','evil-session','/',4102444800,'');
        INSERT INTO moz_cookies VALUES ('commandcode.ai','custom-command-session','apex-host-only-never-send','/',4102444800,'');
        INSERT INTO moz_cookies VALUES ('.commandcode.ai','custom-command-session','expired-session','/',1,'');").unwrap();
    db
}

fn add_command(vcx: &mut VisualTestContext, http: &Http) -> Uuid {
    click(vcx, "usage-add");
    click(vcx, "usage-provider");
    vcx.simulate_keystrokes("down down enter");
    vcx.run_until_parked();
    type_into(vcx, "usage-name", "Command primary");
    type_into(vcx, "usage-key", "command-main-key");
    reply_command(http);
    click(vcx, "usage-submit");
    settle(vcx, |d| d.stored.accounts.len() == 1);
    vcx.update(|_, cx| cx.global::<UsageGlobal>().0.read(cx).stored.accounts[0].id)
}

fn select_command_browser(app: &Entity<LyrelApp>, vcx: &mut VisualTestContext, id: Uuid) {
    click(vcx, &format!("usage-enhance-{id}"));
    app.update_in(vcx, |a, _, cx| {
        let page = a.active_settings().unwrap().usage.read(cx);
        assert!(
            matches!(page.edit, Some(Edit::Enhance(account)) if account == id),
            "enhancement form must open through its button; submitting={}",
            page.submitting
        );
    });
    click(vcx, "usage-source");
    vcx.simulate_keystrokes("escape");
    vcx.run_until_parked();
    app.update_in(vcx, |a, _, cx| {
        assert!(matches!(a.active_settings().unwrap().usage.read(cx).edit, Some(Edit::Enhance(account)) if account == id));
    });
    click(vcx, "usage-source");
    vcx.simulate_keystrokes("down enter");
    vcx.run_until_parked();
    click(vcx, "usage-browser");
    let index = browser::Browser::all()
        .iter()
        .position(|b| *b == browser::Browser::Firefox)
        .unwrap();
    vcx.simulate_keystrokes(&format!("{}enter", "down ".repeat(index + 1)));
    vcx.run_until_parked();
    let page = app.update_in(vcx, |a, _, _| a.active_settings().unwrap().usage.clone());
    let deadline = std::time::Instant::now() + Duration::from_secs(8);
    while page.read_with(vcx, |p, _| p.discovering || p.profile_items.len() != 2) {
        vcx.run_until_parked();
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    click(vcx, "usage-profile");
    vcx.simulate_keystrokes("down down enter");
    vcx.run_until_parked();
    click(vcx, "usage-submit");
    app.update_in(vcx, |a, _, cx| {
        let page = a.active_settings().unwrap().usage.read(cx);
        assert!(!page.submitting && page.error.is_none());
        assert!(page.cookie_name.read(cx).value().is_empty());
        assert!(matches!(page.edit, Some(Edit::Enhance(account)) if account == id));
    });
    type_into(vcx, "usage-cookie-name", "custom-command-session");
}

fn reply_command_enhancement(http: &Http) {
    http.reply(
        200,
        br#"{"user":{"id":"command-user"},"session":{"id":"fixture"}}"#,
    );
    http.reply(200, br#"{"success":true,"data":{"planId":"individual-max","status":"active","currentPeriodEnd":"2026-12-01T00:00:00Z"}}"#);
}

#[gpui_kit::test]
fn command_selected_browser_profile_enhances_without_saving_cookie_and_rechecks_rotated_identity(
    cx: &mut TestAppContext,
) {
    let dir = tempfile::tempdir().unwrap();
    let db = command_browser_fixture(&dir.path().join("isolated-browser"));
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
    let id = add_command(&mut vcx, &http);
    select_command_browser(&app, &mut vcx, id);
    reply_command(&http);
    reply_command_enhancement(&http);
    click(&mut vcx, "usage-submit");
    settle(&mut vcx, |d| {
        d.stored.accounts[0].enhancement_browser.is_some()
    });
    let saved = Accounts::load(dir.path().join("usage.json"), vault.clone()).unwrap();
    assert_eq!(saved.secret(id).unwrap(), "command-main-key");
    assert!(saved.enhancement_secret(id).unwrap().is_none());
    assert_eq!(vault.len(), 1);
    let metadata = std::fs::read_to_string(dir.path().join("usage.json")).unwrap();
    for private in [
        "browser-command-session",
        "never-send",
        "evil-session",
        dir.path().to_str().unwrap(),
    ] {
        assert!(!metadata.contains(private));
    }
    let cookies = http
        .requests
        .lock()
        .unwrap()
        .iter()
        .filter(|r| r.1 == "Cookie")
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(
        cookies,
        vec![
            (
                providers::command_code::IDENTITY_ENDPOINT.into(),
                "Cookie".into(),
                "custom-command-session=browser-command-session".into()
            ),
            (
                providers::command_code::SESSION_ENDPOINT.into(),
                "Cookie".into(),
                "custom-command-session=browser-command-session".into()
            )
        ]
    );
    db.execute(
        "UPDATE moz_cookies SET value='rotated-same-user' WHERE value='browser-command-session'",
        [],
    )
    .unwrap();
    reply_command(&http);
    reply_command_enhancement(&http);
    click(&mut vcx, &format!("usage-refresh-{id}"));
    let page = app.update_in(&mut vcx, |a, _, _| {
        a.active_settings().unwrap().usage.clone()
    });
    let deadline = std::time::Instant::now() + Duration::from_secs(8);
    while page.read_with(&vcx, |p, _| p.pending.contains(&id)) {
        vcx.run_until_parked();
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    vcx.update(|_, cx| {
        let Balance::Provider(report) = &cx.global::<UsageGlobal>().0.read(cx).stored.accounts[0]
            .snapshot
            .as_ref()
            .unwrap()
            .balance
        else {
            panic!("Command report")
        };
        assert_eq!(report.plan.as_deref(), Some("individual-max"));
        assert_eq!(report.enhancement, None);
    });
    db.execute(
        "UPDATE moz_cookies SET value='different-account' WHERE value='rotated-same-user'",
        [],
    )
    .unwrap();
    reply_command(&http);
    http.reply(200, br#"{"user":{"id":"different-user"}}"#);
    click(&mut vcx, &format!("usage-refresh-{id}"));
    settle(
        &mut vcx,
        |d| matches!(&d.stored.accounts[0].snapshot.as_ref().unwrap().balance, Balance::Provider(r) if r.enhancement==Some(UsageError::IdentityMismatch)),
    );
    vcx.update(|_, cx| {
        let account = &cx.global::<UsageGlobal>().0.read(cx).stored.accounts[0];
        assert!(account.error.is_none());
        let Balance::Provider(report) = &account.snapshot.as_ref().unwrap().balance else {
            panic!("Command report")
        };
        assert_eq!(report.identity.as_deref(), Some("command-user"));
        assert_eq!(report.pools[0].remaining.as_deref(), Some("85"));
        assert_eq!(report.plan.as_deref(), Some("individual-pro"));
    });
    assert_eq!(vault.len(), 1);
    db.execute_batch("BEGIN EXCLUSIVE").unwrap();
    reply_command(&http);
    click(&mut vcx, &format!("usage-refresh-{id}"));
    settle(
        &mut vcx,
        |d| matches!(&d.stored.accounts[0].snapshot.as_ref().unwrap().balance, Balance::Provider(r) if r.enhancement==Some(UsageError::BrowserLocked)),
    );
    db.execute_batch("ROLLBACK; UPDATE moz_cookies SET expiry=1")
        .unwrap();
    reply_command(&http);
    click(&mut vcx, &format!("usage-refresh-{id}"));
    settle(
        &mut vcx,
        |d| matches!(&d.stored.accounts[0].snapshot.as_ref().unwrap().balance, Balance::Provider(r) if r.enhancement==Some(UsageError::BrowserMissing)),
    );
    assert_eq!(
        Accounts::load(dir.path().join("usage.json"), vault.clone())
            .unwrap()
            .secret(id)
            .unwrap(),
        "command-main-key"
    );
    click(&mut vcx, &format!("usage-remove-{id}"));
    click(&mut vcx, "usage-submit");
    settle(&mut vcx, |d| d.stored.accounts.is_empty());
    assert!(vault.is_empty());
}

#[gpui_kit::test]
fn command_browser_late_enhancement_after_escape_or_page_close_cannot_bind_source(
    cx: &mut TestAppContext,
) {
    let dir = tempfile::tempdir().unwrap();
    let _db = command_browser_fixture(&dir.path().join("isolated-browser"));
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
    let id = add_command(&mut vcx, &http);
    let page = app.update_in(&mut vcx, |a, _, _| {
        a.active_settings().unwrap().usage.clone()
    });
    for close_page in [false, true] {
        select_command_browser(&app, &mut vcx, id);
        reply_command(&http);
        http.reply(200, br#"{"user":{"id":"command-user"}}"#);
        let release = http.held(
            200,
            br#"{"success":true,"data":{"planId":"late-plan","status":"active"}}"#,
        );
        let before = http.calls.load(Ordering::SeqCst);
        click(&mut vcx, "usage-submit");
        let deadline = std::time::Instant::now() + Duration::from_secs(8);
        while http.calls.load(Ordering::SeqCst) < before + 6 {
            vcx.run_until_parked();
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
        if close_page {
            app.update_in(&mut vcx, |a, w, cx| a.close_settings(w, cx));
        } else {
            vcx.update(|window, cx| window.press("escape", cx));
            vcx.run_until_parked();
            app.update_in(&mut vcx, |a, _, cx| {
                assert!(
                    a.has_settings(),
                    "Escape cancels the enhancement, not the settings window"
                );
                assert!(a.active_settings().unwrap().usage.read(cx).edit.is_none());
            });
            vcx.update(|window, cx| {
                window.render_frame(cx);
                assert!(window.try_find("usage-source").is_none());
            });
        }
        release.send(()).unwrap();
        settle(&mut vcx, |d| !d.stored.accounts[0].busy);
        let deadline = std::time::Instant::now() + Duration::from_secs(8);
        while page.read_with(&vcx, |page, _| page.submitting) {
            vcx.run_until_parked();
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
        let saved = Accounts::load(dir.path().join("usage.json"), vault.clone()).unwrap();
        assert!(saved.account(id).unwrap().enhancement_browser.is_none());
        assert!(saved.enhancement_secret(id).unwrap().is_none());
        assert_eq!(saved.secret(id).unwrap(), "command-main-key");
        assert_eq!(vault.len(), 1);
        if close_page {
            vcx.executor().advance_clock(Duration::from_secs(600));
            vcx.run_until_parked();
            assert_eq!(http.calls.load(Ordering::SeqCst), before + 6);
        }
    }
}
