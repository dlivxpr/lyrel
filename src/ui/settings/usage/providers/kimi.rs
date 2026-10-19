//! Kimi Code CLI 用量与网页账务是不同来源，绝不互相回退。
//! API 契约：@moonshot-ai/kimi-code@2.2.0/dist/main.mjs（第一方发布产物）。
//! https://unpkg.com/@moonshot-ai/kimi-code@2.2.0/dist/main.mjs
//! 网页 descriptor 与 Connect JSON：
//! https://statics.moonshot.cn/kimi-web-seo/assets/request-zVfD9fT_.js
//! https://statics.moonshot.cn/kimi-web-seo/assets/index-BhfmT1Zf.js
//! https://statics.moonshot.cn/kimi-web-seo/assets/framework-CDjk3_xN.js
use super::*;
use zeroize::Zeroizing;

pub(in super::super) fn session(input: &str) -> Result<Zeroizing<String>, UsageError> {
    let input = input.trim();
    if input.len() > 16 * 1024 {
        return Err(UsageError::Authentication);
    }
    let value;
    let token = if input.starts_with('{') {
        value = serde_json::from_str::<serde_json::Value>(input)
            .map_err(|_| UsageError::Authentication)?;
        let object = value.as_object().ok_or(UsageError::Authentication)?;
        if object
            .keys()
            .any(|key| !matches!(key.as_str(), "access_token" | "msh_user_id"))
        {
            return Err(UsageError::Authentication);
        }
        object
            .get("access_token")
            .and_then(|v| v.as_str())
            .ok_or(UsageError::Authentication)?
    } else {
        input
            .strip_prefix("Bearer ")
            .or_else(|| input.strip_prefix("access_token="))
            .unwrap_or(input)
    };
    if token.is_empty()
        || token.len() > 2560
        || !token
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.+/=".contains(&b))
    {
        return Err(UsageError::Authentication);
    }
    Ok(Zeroizing::new(token.to_owned()))
}
fn percent(number: f64) -> String {
    let value = format!("{number:.6}");
    value.trim_end_matches('0').trim_end_matches('.').to_owned()
}
fn ratio(kind: PoolKind, value: Option<&serde_json::Value>) -> Result<Pool, UsageError> {
    let ratio = value
        .and_then(|v| v.get("used_ratio"))
        .map(number)
        .transpose()?;
    Ok(Pool {
        kind,
        remaining: ratio.map(|v| percent((1.0 - v).max(0.0) * 100.0)),
        used: ratio.map(|v| percent(v * 100.0)),
        limit: None,
        unit: "%".into(),
        reset: text(value.and_then(|v| v.get("reset_time"))),
        expires: None,
        name: None,
    })
}
fn field<'a>(
    value: &'a serde_json::Value,
    camel: &str,
    proto: &str,
) -> Option<&'a serde_json::Value> {
    value.get(camel).or_else(|| value.get(proto))
}
pub(in super::super) fn query(
    http: &dyn BalanceHttp,
    web: bool,
    secret: &str,
    proxy: Option<&str>,
) -> Result<ProviderReport, UsageError> {
    let token = session(secret)?;
    let value = fetch(
        http,
        if web {
            HttpSource::KimiWeb
        } else {
            HttpSource::KimiApi
        },
        &token,
        None,
        proxy,
    )?;
    let mut report = ProviderReport {
        provider: Provider::Kimi,
        identity: None,
        plan: None,
        pools: Vec::new(),
        enhancement: None,
        expired: None,
        plan_expires: None,
        overdrawn: if web {
            value
                .get("overdrawn")
                .map(|v| v.as_bool().ok_or(UsageError::Response))
                .transpose()?
        } else {
            None
        },
    };
    if web {
        for (key, proto, kind) in [
            ("ratelimit5h", "ratelimit_5h", PoolKind::FiveHour),
            (
                "ratelimitCode5h",
                "ratelimit_code_5h",
                PoolKind::CodeFiveHour,
            ),
            ("ratelimit7d", "ratelimit_7d", PoolKind::Weekly),
            ("ratelimitCode7d", "ratelimit_code_7d", PoolKind::CodeWeekly),
        ] {
            if let Some(window) = field(&value, key, proto) {
                let enabled = window
                    .get("enabled")
                    .map(|v| v.as_bool().ok_or(UsageError::Response))
                    .transpose()?
                    .unwrap_or(false);
                if !enabled {
                    continue;
                }
                let used = window.get("ratio").map(number).transpose()?.unwrap_or(0.0);
                report.pools.push(Pool {
                    kind,
                    remaining: Some(percent((1.0 - used).max(0.0) * 100.0)),
                    used: Some(percent(used * 100.0)),
                    limit: None,
                    unit: "%".into(),
                    reset: text(field(window, "resetTime", "reset_time")),
                    expires: None,
                    name: None,
                });
            }
        }
        if let Some(balance) = field(&value, "subscriptionBalance", "subscription_balance") {
            web_balance(&mut report, PoolKind::MonthlyTotal, balance)?;
        }
        if let Some(gifts) =
            field(&value, "giftBalances", "gift_balances").and_then(|v| v.as_array())
        {
            for balance in gifts {
                web_balance(&mut report, PoolKind::Free, balance)?;
            }
        }
        if let Some(wallets) =
            field(&value, "boosterWallets", "booster_wallets").and_then(|v| v.as_array())
        {
            for wallet in wallets {
                web_wallet(&mut report, wallet)?;
            }
        }
    } else {
        let usages = value.get("usages");
        for (key, kind) in [
            ("limit_5h", PoolKind::FiveHour),
            ("limit_7d", PoolKind::Weekly),
            ("limit_month_total", PoolKind::MonthlyTotal),
            ("limit_month_code", PoolKind::MonthlyCode),
        ] {
            report
                .pools
                .push(ratio(kind, usages.and_then(|v| v.get(key)))?);
        }
        if !report.pools.iter().any(|p| p.remaining.is_some()) {
            return Err(UsageError::Response);
        }
        if let Some(wallet) = value.get("boosterWallet") {
            api_wallet(&mut report, wallet)?;
        }
        let identity = fetch(http, HttpSource::KimiMe, &token, None, proxy)?;
        report.identity = text(identity.get("user_id"));
        report.plan = text(identity.get("user_level_name"));
    }
    if report.pools.is_empty() {
        return Err(UsageError::Response);
    }
    Ok(report)
}

fn cents(value: f64) -> String {
    format!("{:.2}", value / 100.0)
}
fn fixed_cents(value: f64) -> f64 {
    let v = value / 1_000_000.0;
    if v > 0.0 && v < 1.0 { 1.0 } else { v.round() }
}
fn currency(wallet: &serde_json::Value) -> String {
    text(
        field(wallet, "monthlyChargeLimit", "monthly_charge_limit").and_then(|v| v.get("currency")),
    )
    .or_else(|| text(field(wallet, "monthlyUsed", "monthly_used").and_then(|v| v.get("currency"))))
    .unwrap_or_else(|| "USD".into())
}
fn api_wallet(report: &mut ProviderReport, wallet: &serde_json::Value) -> Result<(), UsageError> {
    let balance = wallet.get("balance").ok_or(UsageError::Response)?;
    if balance.get("type").and_then(|v| v.as_str()) != Some("BOOSTER") {
        return Err(UsageError::Response);
    }
    let total = number(balance.get("amount").ok_or(UsageError::Response)?)?;
    let remaining = balance
        .get("amountLeft")
        .map(number)
        .transpose()?
        .map(fixed_cents)
        .map(cents);
    report.pools.push(Pool {
        kind: PoolKind::Booster,
        remaining,
        used: None,
        limit: Some(cents(fixed_cents(total))),
        unit: currency(wallet),
        reset: None,
        expires: None,
        name: None,
    });
    monthly_wallet(report, wallet)
}
fn monthly_wallet(
    report: &mut ProviderReport,
    wallet: &serde_json::Value,
) -> Result<(), UsageError> {
    let enabled = field(
        wallet,
        "monthlyChargeLimitEnabled",
        "monthly_charge_limit_enabled",
    )
    .and_then(|v| v.as_bool());
    let limit = field(wallet, "monthlyChargeLimit", "monthly_charge_limit")
        .and_then(|v| field(v, "priceInCents", "price_in_cents"))
        .map(number)
        .transpose()?;
    let used = field(wallet, "monthlyUsed", "monthly_used")
        .and_then(|v| field(v, "priceInCents", "price_in_cents"))
        .map(number)
        .transpose()?;
    if enabled.is_some() || limit.is_some() || used.is_some() {
        report.pools.push(Pool {
            kind: PoolKind::BoosterMonthly,
            remaining: if enabled == Some(true) {
                limit.zip(used).map(|(l, u)| cents((l - u).max(0.0)))
            } else {
                None
            },
            used: used.map(cents),
            limit: if enabled == Some(true) {
                limit.map(cents)
            } else {
                None
            },
            unit: currency(wallet),
            reset: None,
            expires: None,
            name: None,
        });
    }
    Ok(())
}
fn web_balance(
    report: &mut ProviderReport,
    kind: PoolKind,
    balance: &serde_json::Value,
) -> Result<(), UsageError> {
    let used = field(balance, "amountUsedRatio", "amount_used_ratio")
        .map(number)
        .transpose()?;
    let code = field(balance, "kimiCodeUsedRatio", "kimi_code_used_ratio")
        .map(number)
        .transpose()?;
    report.pools.push(Pool {
        kind,
        remaining: used.map(|v| percent((1.0 - v).max(0.0) * 100.0)),
        used: used.map(|v| percent(v * 100.0)),
        limit: None,
        unit: "%".into(),
        reset: None,
        expires: text(field(balance, "expireTime", "expire_time")),
        name: text(field(balance, "displayName", "display_name")),
    });
    if let Some(code) = code {
        report.pools.push(Pool {
            kind: PoolKind::MonthlyCode,
            remaining: None,
            used: Some(percent(code * 100.0)),
            limit: None,
            unit: "%".into(),
            reset: None,
            expires: text(field(balance, "expireTime", "expire_time")),
            name: text(field(balance, "displayName", "display_name")),
        });
    }
    let raw = |value: Option<&serde_json::Value>| -> Result<Option<String>, UsageError> {
        value
            .map(|v| {
                if let Some(s) = v
                    .as_str()
                    .filter(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
                {
                    Ok(s.to_owned())
                } else {
                    v.as_u64()
                        .map(|v| v.to_string())
                        .ok_or(UsageError::Response)
                }
            })
            .transpose()
    };
    let remaining = raw(field(balance, "amountLeft", "amount_left"))?;
    let limit = raw(balance.get("amount"))?;
    if remaining.is_some() || limit.is_some() {
        let unit = match balance.get("unit") {
            Some(serde_json::Value::String(unit)) => unit.clone(),
            Some(value) => match value.as_u64() {
                Some(1) => "UNIT_COUNT".into(),
                Some(2) => "UNIT_CREDIT".into(),
                Some(3) => "UNIT_CURRENCY".into(),
                _ => "UNIT_UNSPECIFIED".into(),
            },
            None => "UNIT_UNSPECIFIED".into(),
        };
        report.pools.push(Pool {
            kind: PoolKind::RawBalance,
            remaining,
            limit,
            used: None,
            unit,
            reset: None,
            expires: text(field(balance, "expireTime", "expire_time")),
            name: text(field(balance, "displayName", "display_name")),
        });
    }
    Ok(())
}
fn web_wallet(report: &mut ProviderReport, wallet: &serde_json::Value) -> Result<(), UsageError> {
    let left = field(wallet, "moneyLeft", "money_left");
    let total = field(wallet, "moneyTotal", "money_total");
    let unit = text(left.and_then(|v| v.get("currency")))
        .or_else(|| text(total.and_then(|v| v.get("currency"))))
        .ok_or(UsageError::Response)?;
    report.pools.push(Pool {
        kind: PoolKind::Booster,
        remaining: left
            .and_then(|v| field(v, "priceInCents", "price_in_cents"))
            .map(number)
            .transpose()?
            .map(cents),
        used: None,
        limit: total
            .and_then(|v| field(v, "priceInCents", "price_in_cents"))
            .map(number)
            .transpose()?
            .map(cents),
        unit,
        reset: None,
        expires: None,
        name: None,
    });
    monthly_wallet(report, wallet)
}
