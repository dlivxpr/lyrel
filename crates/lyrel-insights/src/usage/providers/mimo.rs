//! Xiaomi MiMo 控制台公开前端契约，核验于 2026-10-19；不是公开第三方 API。
//! https://platform.xiaomimimo.com/static/main.f2b8e14e661a38ce.chunk.js
//! HTTP 模块 29618（/api/v1 + code/data）、store 18781（userProfile）。
//! https://platform.xiaomimimo.com/static/904.6fb4d0fff1570f70.chunk.js
//! 模块 95952（GET /balance 与金额字段）。
//! https://platform.xiaomimimo.com/static/279.8ef7328eee268de2.chunk.js
//! 模块 70793（detail.currentPeriodEnd 为 UTC；usage.items，而不是 monthUsage）。
use super::fetch_console as fetch;
use super::*;

fn data(value: &serde_json::Value) -> Result<&serde_json::Value, UsageError> {
    match value.get("code").and_then(|v| v.as_i64()) {
        Some(0 | 200) => value.get("data").ok_or(UsageError::Response),
        Some(302 | 401 | 403) => Err(UsageError::Authentication),
        Some(429 | 400429) => Err(UsageError::RateLimited),
        Some(_) => Err(UsageError::Service),
        None => Err(UsageError::Response),
    }
}

pub(super) fn query(
    http: &dyn BalanceHttp,
    secret: &str,
    proxy: Option<&str>,
) -> Result<ProviderReport, UsageError> {
    let secret = parse_session(Provider::MiMo, secret)?;
    let cookie_id = secret
        .split("; ")
        .find_map(|p| p.strip_prefix("userId="))
        .ok_or(UsageError::Authentication)?;
    let balance = fetch(http, Provider::MiMo, 0, &secret, proxy)?;
    let balance = data(&balance)?;
    if !balance.is_object() {
        return Err(UsageError::Response);
    }
    let profile = fetch(http, Provider::MiMo, 3, &secret, proxy)?;
    let profile = data(&profile)?;
    let identity = text(profile.get("userId"))
        .or_else(|| {
            profile
                .get("userId")
                .and_then(|v| v.as_u64())
                .map(|n| n.to_string())
        })
        .ok_or(UsageError::Response)?;
    if identity != cookie_id {
        return Err(UsageError::Authentication);
    }
    let currency = text(balance.get("currency")).unwrap_or_else(|| {
        match profile.get("idc").and_then(|v| v.as_i64()) {
            Some(310) => "CNY",
            Some(_) => "USD",
            None => "",
        }
        .to_owned()
    });
    let mut report = ProviderReport {
        provider: Provider::MiMo,
        identity: Some(identity),
        plan: None,
        expired: None,
        plan_expires: None,
        pools: Vec::new(),
        enhancement: None,
        overdrawn: None,
    };
    for (kind, field) in [
        (PoolKind::Balance, "balance"),
        (PoolKind::Cash, "cashBalance"),
        (PoolKind::Gift, "giftBalance"),
        (PoolKind::Frozen, "frozenBalance"),
    ] {
        report.pools.push(Pool {
            kind,
            name: None,
            remaining: decimal(balance.get(field))?,
            used: None,
            limit: None,
            unit: currency.clone(),
            reset: None,
            expires: None,
        });
    }
    if balance.get("overdraftLimit").is_some() || balance.get("remainingOverdraftLimit").is_some() {
        report.pools.push(Pool {
            kind: PoolKind::Overdraft,
            name: None,
            remaining: decimal(balance.get("remainingOverdraftLimit"))?,
            used: None,
            limit: decimal(balance.get("overdraftLimit"))?,
            unit: currency.clone(),
            reset: None,
            expires: None,
        });
    }
    let plan = (|| {
        let detail = fetch(http, Provider::MiMo, 1, &secret, proxy)?;
        let detail = data(&detail)?;
        if detail.is_null() {
            return Ok(());
        }
        if !detail.is_object() {
            return Err(UsageError::Response);
        }
        report.plan = text(detail.get("planName")).or_else(|| text(detail.get("planCode")));
        report.expired = detail.get("expired").and_then(|v| v.as_bool());
        let end = text(detail.get("currentPeriodEnd")).map(|s| {
            if s.ends_with('Z') || s.ends_with("UTC") {
                s
            } else {
                format!("{s} UTC")
            }
        });
        report.plan_expires = end.clone();
        if detail.get("expired").and_then(|v| v.as_bool()) == Some(true) {
            report.pools.push(Pool {
                kind: PoolKind::Monthly,
                name: None,
                remaining: None,
                used: None,
                limit: None,
                unit: "Credits".to_owned(),
                reset: None,
                expires: end,
            });
            return Ok(());
        }
        let usage = fetch(http, Provider::MiMo, 2, &secret, proxy)?;
        let usage = data(&usage)?;
        let Some(items) = usage.pointer("/usage/items").and_then(|v| v.as_array()) else {
            report.pools.push(Pool {
                kind: PoolKind::Monthly,
                name: None,
                remaining: None,
                used: None,
                limit: None,
                unit: "Credits".to_owned(),
                reset: None,
                expires: end,
            });
            return Ok(());
        };
        if items.len() > 64 {
            return Err(UsageError::Response);
        }
        for item in items {
            let mut pool = Pool {
                kind: PoolKind::Monthly,
                name: text(item.get("name")),
                remaining: None,
                used: decimal(item.get("used"))?,
                limit: decimal(item.get("limit"))?,
                unit: "Credits".to_owned(),
                reset: None,
                expires: end.clone(),
            };
            if pool.used.is_none() && pool.limit.is_none() {
                if let Some(percent) = decimal(item.get("percent"))? {
                    let percent: f64 = percent.parse().map_err(|_| UsageError::Response)?;
                    if !(0.0..=1.0).contains(&percent) {
                        return Err(UsageError::Response);
                    }
                    pool.used = Some((percent * 100.0).to_string());
                    pool.unit = "%".to_owned();
                }
            }
            report.pools.push(pool);
        }
        Ok(())
    })();
    if let Err(error) = plan {
        if error == UsageError::Authentication {
            return Err(error);
        }
        report.enhancement = Some(error);
    }
    Ok(report)
}
