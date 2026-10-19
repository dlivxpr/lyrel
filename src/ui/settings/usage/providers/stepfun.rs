//! StepFun 控制台 ConnectRPC 私有契约，2026-10-19 第一方公开产物核验。
//! https://platform.stepfun.com/_next/static/chunks/28300-12e20e884619af74.js
//! Dashboard 服务/消息描述符、Oasis 请求拦截器；web_id 来自 LocalStorage，绝不从 JWT 推导。
//! https://platform.stepfun.com/_next/static/chunks/app/(dashboard)/account-overview/page-e82dc4ae140e2d0f.js
//! QueryStepPlanRateLimit/GetStepPlanStatus 空请求、planFamily 分支及 creditBuckets。
use super::*;
use serde_json::Value;

fn field<'a>(value: &'a Value, camel: &str, proto: &str) -> Option<&'a Value> {
    value.get(camel).or_else(||value.get(proto))
}
fn checked(value: &Value) -> Result<(), UsageError> {
    let code = value.get("code").or_else(||value.get("status")).ok_or(UsageError::Response)?;
    if matches!(code.as_i64(), Some(100001 | 100002 | 100003 | 120000))
        || matches!(code.as_str(), Some("unauthenticated" | "permission_denied" | "CODE_TOKEN_MISSING" | "CODE_TOKEN_ILLEGAL" | "CODE_TOKEN_EXPIRED")) { return Err(UsageError::Authentication); }
    if code.as_str() == Some("resource_exhausted") { return Err(UsageError::RateLimited); }
    if code.as_i64() == Some(1) || code.as_str() == Some("CODE_OK") { Ok(()) } else { Err(UsageError::Service) }
}
pub(super) fn response(status: u16, body: &[u8]) -> Result<Value, UsageError> {
    match status { 401|403 => return Err(UsageError::Authentication), 429 => return Err(UsageError::RateLimited), _ => {} }
    if body.len() > 64 * 1024 { return Err(UsageError::Response); }
    let value: Value = serde_json::from_slice(body).map_err(|_|UsageError::Response)?;
    if let Err(error @ (UsageError::Authentication | UsageError::RateLimited)) = checked(&value) { return Err(error); }
    if !(200..300).contains(&status) { return Err(UsageError::Service); }
    Ok(value)
}
fn timestamp(value: Option<&Value>) -> Result<Option<String>, UsageError> {
    let Some(value) = decimal(value)? else { return Ok(None) };
    let seconds: i64 = value.parse().map_err(|_|UsageError::Response)?;
    if seconds <= 0 { return Ok(None); }
    chrono::DateTime::from_timestamp(seconds, 0).map(|time|Some(time.to_rfc3339())).ok_or(UsageError::Response)
}
fn ratio(value: Option<&Value>) -> Result<Option<String>, UsageError> {
    let Some(value) = decimal(value)? else { return Ok(None) };
    let value: f64 = value.parse().map_err(|_|UsageError::Response)?;
    if !(0.0..=1.0).contains(&value) { return Err(UsageError::Response); }
    Ok(Some((value * 100.0).to_string()))
}
fn pool(kind: PoolKind, remaining: Option<String>, unit: &str, reset: Option<String>) -> Pool {
    Pool { kind, name: None, remaining, used: None, limit: None, unit: unit.to_owned(), reset, expires: None }
}

pub(super) fn query(http: &dyn BalanceHttp, secret: &str, proxy: Option<&str>) -> Result<ProviderReport, UsageError> {
    let secret = parse_manual(Provider::StepFun, secret)?;
    let rate = fetch(http, Provider::StepFun, 0, &secret, proxy)?;
    checked(&rate)?;
    let family = field(&rate,"planFamily","plan_family");
    let coding = family.is_some_and(|v|v.as_i64() == Some(1) || v.as_str() == Some("PLAN_FAMILY_CODING"));
    let token = family.is_some_and(|v|matches!(v.as_i64(),Some(2|3)) || matches!(v.as_str(),Some("PLAN_FAMILY_TOKEN"|"PLAN_FAMILY_TOKEN_V3")));
    let unspecified = family.is_none_or(|v|v.as_i64() == Some(0) || v.as_str() == Some("PLAN_FAMILY_UNSPECIFIED"));
    if !coding && !token && !unspecified { return Err(UsageError::Response); }
    let identity = fetch(http, Provider::StepFun, 2, &secret, proxy)?;
    checked(&identity)?;
    let uid = text(identity.get("uid")).or_else(|| identity.get("uid").and_then(|v|v.as_u64()).map(|n|n.to_string())).filter(|uid|uid.parse::<u64>().is_ok()).ok_or(UsageError::Response)?;
    let mut report = ProviderReport { provider: Provider::StepFun, identity: Some(uid), plan: None, expired: None, plan_expires: None, pools: Vec::new(), enhancement: None };
    if coding {
        for (kind, remaining, remaining_proto, reset, reset_proto) in [
            (PoolKind::FiveHour,"fiveHourUsageLeftRate","five_hour_usage_left_rate","fiveHourUsageResetTime","five_hour_usage_reset_time"),
            (PoolKind::Weekly,"weeklyUsageLeftRate","weekly_usage_left_rate","weeklyUsageResetTime","weekly_usage_reset_time"),
        ] {
            report.pools.push(pool(kind, ratio(field(&rate,remaining,remaining_proto))?, "%", timestamp(field(&rate,reset,reset_proto))?));
        }
    } else if token {
        if let Some(credit) = field(&rate,"planCreditRateLimit","plan_credit_rate_limit") {
            let reset = timestamp(field(credit,"subscriptionCreditResetTime","subscription_credit_reset_time"))?;
            if let Some(buckets) = field(credit,"creditBuckets","credit_buckets").and_then(|v|v.as_array()).filter(|v|!v.is_empty()) {
                if buckets.len() > 64 { return Err(UsageError::Response); }
                for bucket in buckets {
                    let mut item = pool(PoolKind::CreditBucket, decimal(field(bucket,"creditResidual","credit_residual"))?, "Credit", timestamp(field(bucket,"nextResetAt","next_reset_at"))?);
                    item.name = text(bucket.get("type")).or_else(||bucket.get("type").and_then(|v|v.as_i64()).map(|n|n.to_string()));
                    item.limit = decimal(field(bucket,"creditTotal","credit_total"))?;
                    item.expires = timestamp(field(bucket,"expireAt","expire_at"))?;
                    if item.reset.is_none() && bucket.get("type").is_some_and(|v|v.as_i64()==Some(1) || v.as_str()==Some("TYPE_SUBSCRIPTION")) { item.reset = reset.clone(); }
                    report.pools.push(item);
                }
                report.pools.sort_by(|a,b|a.expires.cmp(&b.expires));
            } else {
                report.pools.push(pool(PoolKind::SubscriptionCredit, ratio(field(credit,"subscriptionCreditLeftRate","subscription_credit_left_rate"))?, "%", reset));
                if let Some(value) = field(credit,"topupCreditLeftRate","topup_credit_left_rate") {
                    let mut topup = pool(PoolKind::CreditBucket, ratio(Some(value))?, "%", None);
                    topup.name = Some("topup_credit_left_rate".to_owned());
                    report.pools.push(topup);
                }
            }
        } else {
            report.pools.push(pool(PoolKind::SubscriptionCredit, None, "Credit", None));
        }
    }
    let status = fetch(http, Provider::StepFun, 1, &secret, proxy).and_then(|value| { checked(&value)?; Ok(value) });
    match status {
        Ok(status) => {
            report.plan = text(status.pointer("/subscription/name"));
            if let Some(subscription) = status.get("subscription") {
                report.plan_expires = timestamp(field(subscription,"expiredAt","expired_at"))?;
                report.expired = subscription.get("status").and_then(|v| {
                    if v.as_i64()==Some(2) || v.as_str().is_some_and(|s| s=="EXPIRED" || s.ends_with("_EXPIRED")) { Some(true) }
                    else if matches!(v.as_i64(),Some(1|3)) || v.as_str().is_some_and(|s| s=="ACTIVE" || s.ends_with("_ACTIVE") || s.ends_with("CANCEL_PENDING")) { Some(false) }
                    else { None }
                });
            }
        }
        Err(UsageError::Authentication) => return Err(UsageError::Authentication),
        Err(error) => report.enhancement = Some(error),
    }
    Ok(report)
}
