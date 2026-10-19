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
fn pool(
    kind: PoolKind,
    value: Option<&serde_json::Value>,
    limit: Option<&serde_json::Value>,
    reset: Option<String>,
) -> Result<Pool, UsageError> {
    Ok(Pool {
        kind,
        remaining: value.map(number).transpose()?.map(|v| v.to_string()),
        used: None,
        limit: limit.map(number).transpose()?.map(|v| v.to_string()),
        unit: "credits".into(),
        reset,
        expires: None,
        name: None,
    })
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
    let reset = time(subscription.get("currentPeriodEnd"));
    let mut pools = vec![
        pool(
            PoolKind::Monthly,
            balances.get("monthlyCredits"),
            None,
            reset,
        )?,
        pool(
            PoolKind::Purchased,
            balances.get("purchasedCredits"),
            None,
            None,
        )?,
        pool(PoolKind::Free, balances.get("freeCredits"), None, None)?,
    ];
    let windows = credits
        .get("windowLimits")
        .or_else(|| balances.get("windowLimits"));
    for (key, kind) in [
        ("fiveHour", PoolKind::FiveHour),
        ("weekly", PoolKind::Weekly),
    ] {
        let window = windows.and_then(|v| v.get(key));
        let used = window.and_then(|v| v.get("used")).map(number).transpose()?;
        let cap = window.and_then(|v| v.get("cap")).map(number).transpose()?;
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
    let since = text(subscription.get("currentPeriodStart"));
    let (status, body) = http.get(
        &UsageRequest {
            source: HttpSource::CommandSummary,
            secret: key,
            org: org.as_deref(),
            since: since.as_deref(),
        },
        proxy,
    )?;
    let summary = json(status, &body)?;
    for (field, kind, unit) in [
        ("totalCost", PoolKind::BillingUsage, "USD"),
        ("totalCount", PoolKind::RequestCount, "requests"),
    ] {
        pools.push(Pool {
            kind,
            remaining: None,
            used: summary
                .get(field)
                .map(number)
                .transpose()?
                .map(|v| v.to_string()),
            limit: None,
            unit: unit.into(),
            reset: time(subscription.get("currentPeriodEnd")),
            expires: None,
            name: None,
        });
    }
    if let Some(minutes) = credits.get("sandboxMinutes") {
        let used = minutes.get("usedMinutes").map(number).transpose()?;
        let limit = minutes.get("limitMinutes").map(number).transpose()?;
        pools.push(Pool {
            kind: PoolKind::SandboxMinutes,
            remaining: used.zip(limit).map(|(u, l)| (l - u).max(0.0).to_string()),
            used: used.map(|v| v.to_string()),
            limit: limit.map(|v| v.to_string()),
            unit: "minutes".into(),
            reset: None,
            expires: None,
            name: None,
        });
    }
    if let Some(limits) = who.get("orgLimits").and_then(|v| v.as_array()) {
        for row in limits {
            let limit = row.get("limit").map(number).transpose()?;
            let spent = row.get("spent").map(number).transpose()?;
            pools.push(Pool {
                kind: PoolKind::OrganizationLimit,
                remaining: limit.zip(spent).map(|(l, u)| (l - u).max(0.0).to_string()),
                used: spent.map(|v| v.to_string()),
                limit: limit.map(|v| v.to_string()),
                unit: "USD".into(),
                reset: time(row.get("resetAt")),
                expires: None,
                name: text(row.get("modelLabel"))
                    .or_else(|| text(row.get("model")))
                    .or_else(|| text(row.get("scope"))),
            });
        }
    }
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
    let end = time(data.get("cancelAt")).or_else(|| time(data.get("currentPeriodEnd")));
    if let Some(end) = end {
        if let Some(pool) = report
            .pools
            .iter_mut()
            .find(|p| p.kind == PoolKind::Monthly)
        {
            pool.reset = Some(end);
        }
    }
    report.plan = Some(plan);
    report.enhancement = None;
    Ok(())
}
