//! Command Code 第一方 CLI 的 /alpha 用量来源。
//! https://unpkg.com/command-code@1.84.0/dist/cli.mjs
//! Cookie endpoint first-party: https://commandcode.ai/assets/constants-oAoFBSr8.js
//! https://commandcode.ai/assets/use-legacy-pro-status-B1iLcEZv.js
//! Identity baseURL + user.id: /assets/auth-client-DR7oY-YF.js and root-CycwDi3N.js.
use super::*;
pub(in super::super) const SESSION_ENDPOINT: &str =
    "https://api.commandcode.ai/internal/billing/subscriptions";
pub(in super::super) const IDENTITY_ENDPOINT: &str = "https://api.commandcode.ai/auth/get-session";
fn data(value: &serde_json::Value) -> &serde_json::Value {
    value.get("data").unwrap_or(value)
}
// 第一方 CLI 1.84.0 的 getPlanTotalCredits / Sr；不从余额或未核实的字段反推月限额。
fn monthly_limit(subscription: &serde_json::Value) -> Option<f64> {
    if subscription.get("status")?.as_str()? != "active" {
        return None;
    }
    match subscription.get("planId")?.as_str()? {
        "individual-go" | "individual-go-v1" => Some(10.0),
        "individual-goat" => Some(70.0),
        "individual-pro" => Some(30.0),
        "individual-pro-v1" => Some(80.0),
        "individual-provider" => Some(15.0),
        "individual-max" => Some(150.0),
        "individual-ultra" => Some(300.0),
        "teams-pro" => Some(40.0),
        _ => None,
    }
}
pub(in super::super) fn query(
    http: &dyn BalanceHttp,
    key: &str,
    proxy: Option<&str>,
) -> Result<ProviderReport, UsageError> {
    if key.is_empty() || key.len() > 2560 || key.chars().any(char::is_whitespace) {
        return Err(UsageError::Authentication);
    }
    let who = fetch(http, HttpSource::CommandWhoami, key, None, proxy)?;
    let who = data(&who);
    if who.get("success").and_then(|v| v.as_bool()) == Some(false) {
        return Err(UsageError::Authentication);
    }
    let identity = text(who.pointer("/user/id")).ok_or(UsageError::Response)?;
    let org = text(who.pointer("/org/id"));
    let credits = fetch(http, HttpSource::CommandCredits, key, org.as_deref(), proxy)?;
    let credits = data(&credits);
    let balances = credits.get("credits").ok_or(UsageError::Response)?;
    let subscription = fetch(
        http,
        HttpSource::CommandSubscriptions,
        key,
        org.as_deref(),
        proxy,
    )?;
    let subscription = data(&subscription);
    let mut pools = Vec::with_capacity(3);
    let windows = credits
        .get("windowLimits")
        .or_else(|| balances.get("windowLimits"));
    for (key, kind) in [
        ("fiveHour", PoolKind::FiveHour),
        ("weekly", PoolKind::Weekly),
    ] {
        let window = windows.and_then(|v| v.get(key));
        let used = window
            .and_then(|v| v.get("used"))
            .filter(|v| !v.is_null())
            .map(number)
            .transpose()?;
        let cap = window
            .and_then(|v| v.get("cap"))
            .filter(|v| !v.is_null())
            .map(number)
            .transpose()?;
        pools.push(Pool {
            kind,
            remaining: used.zip(cap).map(|(u, c)| (c - u).max(0.0).to_string()),
            used: used.map(|v| v.to_string()),
            limit: cap.map(|v| v.to_string()),
            unit: "credits".into(),
            reset: time(window.and_then(|v| v.get("resetAt"))),
            expires: None,
            name: None,
        });
    }
    let remaining = balances
        .get("monthlyCredits")
        .filter(|v| !v.is_null())
        .map(number)
        .transpose()?;
    let limit = monthly_limit(subscription);
    pools.push(Pool {
        kind: PoolKind::Monthly,
        remaining: remaining.map(|v| v.to_string()),
        used: remaining
            .zip(limit)
            .map(|(r, l)| (l - r).max(0.0).to_string()),
        limit: limit.map(|v| v.to_string()),
        unit: "credits".into(),
        reset: time(subscription.get("currentPeriodEnd")),
        expires: None,
        name: None,
    });
    if !pools.iter().any(|p| p.remaining.is_some()) {
        return Err(UsageError::Response);
    }
    Ok(ProviderReport {
        provider: Provider::CommandCode,
        identity: Some(identity),
        plan: text(subscription.get("planId")).or_else(|| text(balances.get("planId"))),
        pools,
        enhancement: None,
        overdrawn: None,
        expired: None,
        plan_expires: None,
    })
}

/// 手工明确选择单个会话 Cookie；不猜 cookie 名，不保存浏览器整段 Cookie。
pub(in super::super) fn session(input: &str) -> Result<zeroize::Zeroizing<String>, UsageError> {
    let value = input
        .trim()
        .strip_prefix("Cookie:")
        .unwrap_or(input.trim())
        .trim();
    let (name, token) = value.split_once('=').ok_or(UsageError::Authentication)?;
    if value.len() > 2560
        || name.is_empty()
        || token.is_empty()
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
        || !token
            .bytes()
            .all(|b| (0x21..=0x7e).contains(&b) && !b"\";,\\".contains(&b))
    {
        return Err(UsageError::Authentication);
    }
    Ok(zeroize::Zeroizing::new(value.to_owned()))
}
/// #17 可以消费相同 HTTP/解析 Interface；浏览器材料不得调用长期 save_enhancement。
/// 身份由第一方 better-auth /auth/get-session 核对，不臆造 subscription.userId。
pub(in super::super) fn enhance(
    http: &dyn BalanceHttp,
    cookie: &str,
    report: &mut ProviderReport,
    proxy: Option<&str>,
) -> Result<(), UsageError> {
    if report.provider != Provider::CommandCode {
        return Err(UsageError::Response);
    }
    let cookie = session(cookie)?;
    let identity = fetch(
        http,
        HttpSource::CommandSessionIdentity,
        &cookie,
        None,
        proxy,
    )?;
    if identity.is_null() {
        return Err(UsageError::Authentication);
    }
    let user = text(identity.pointer("/user/id")).ok_or(UsageError::Response)?;
    if report.identity.as_deref() != Some(user.as_str()) {
        return Err(UsageError::IdentityMismatch);
    }
    let subscription = fetch(http, HttpSource::CommandEnhancement, &cookie, None, proxy)?;
    if subscription.get("success").and_then(|v| v.as_bool()) != Some(true) {
        return Err(UsageError::Service);
    }
    let data = subscription
        .get("data")
        .filter(|v| v.is_object())
        .ok_or(UsageError::Response)?;
    let plan = text(data.get("planId")).ok_or(UsageError::Response)?;
    let status = data
        .get("status")
        .and_then(|v| v.as_str())
        .ok_or(UsageError::Response)?;
    if !matches!(status, "active" | "trialing" | "past_due") {
        return Err(UsageError::Service);
    }
    if let Some(pool) = report
        .pools
        .iter_mut()
        .find(|p| p.kind == PoolKind::Monthly)
    {
        let limit = monthly_limit(data);
        pool.used = pool
            .remaining
            .as_deref()
            .and_then(|v| v.parse::<f64>().ok())
            .zip(limit)
            .map(|(r, l)| (l - r).max(0.0).to_string());
        pool.limit = limit.map(|v| v.to_string());
        if let Some(end) = time(data.get("currentPeriodEnd")) {
            pool.reset = Some(end);
        }
    }
    report.plan = Some(plan);
    report.enhancement = None;
    Ok(())
}
