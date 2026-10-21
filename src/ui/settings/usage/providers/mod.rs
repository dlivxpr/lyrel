use super::{
    BalanceHttp, HttpSource, UsageRequest,
    accounts::{Balance, UsageError},
};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;
pub(super) mod command_code;
pub(super) mod kimi;
mod mimo;
mod stepfun;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(super) enum Provider {
    #[default]
    DeepSeek,
    Kimi,
    CommandCode,
    StepFun,
    MiMo,
}
impl Provider {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::DeepSeek => "DeepSeek",
            Self::Kimi => "Kimi Code",
            Self::CommandCode => "Command Code",
            Self::StepFun => "StepFun Plan",
            Self::MiMo => "Xiaomi MiMo",
        }
    }
    pub(super) fn help(self) -> super::L10nKey {
        match self {
            Self::DeepSeek => super::L10nKey::UsageSessionHelp,
            Self::Kimi => super::L10nKey::UsageKimiHelp,
            Self::CommandCode => super::L10nKey::UsageCommandSessionHelp,
            Self::StepFun => super::L10nKey::UsageStepFunHelp,
            Self::MiMo => super::L10nKey::UsageMiMoHelp,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) enum PoolKind {
    FiveHour,
    Weekly,
    MonthlyTotal,
    MonthlyCode,
    Monthly,
    Free,
    Booster,
    Coding,
    CodeFiveHour,
    CodeWeekly,
    BoosterMonthly,
    RawBalance,
    Balance,
    Cash,
    Gift,
    Frozen,
    Overdraft,
    SubscriptionCredit,
    CreditBucket,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Pool {
    pub kind: PoolKind,

    pub remaining: Option<String>,
    pub used: Option<String>,
    pub limit: Option<String>,
    pub unit: String,
    pub reset: Option<String>,
    pub expires: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct ProviderReport {
    pub provider: Provider,
    pub identity: Option<String>,
    pub plan: Option<String>,
    pub pools: Vec<Pool>,
    pub enhancement: Option<UsageError>,
    #[serde(default)]
    pub overdrawn: Option<bool>,
    #[serde(default)]
    pub expired: Option<bool>,
    #[serde(default)]
    pub plan_expires: Option<String>,
}
pub(super) fn json(status: u16, bytes: &[u8]) -> Result<serde_json::Value, UsageError> {
    match status {
        401 | 403 => return Err(UsageError::Authentication),
        429 => return Err(UsageError::RateLimited),
        200..=299 => {}
        _ => return Err(UsageError::Service),
    }
    if bytes.len() > 64 * 1024 {
        return Err(UsageError::Response);
    }
    serde_json::from_slice(bytes).map_err(|_| UsageError::Response)
}
pub(super) fn number(v: &serde_json::Value) -> Result<f64, UsageError> {
    let n = v
        .as_f64()
        .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
        .ok_or(UsageError::Response)?;
    if !n.is_finite() || n < 0.0 {
        return Err(UsageError::Response);
    }
    Ok(n)
}
pub(super) fn text(v: Option<&serde_json::Value>) -> Option<String> {
    v.and_then(|v| v.as_str())
        .filter(|s| !s.is_empty() && s.len() <= 256 && !s.chars().any(char::is_control))
        .map(str::to_owned)
}
pub(super) fn time(value: Option<&serde_json::Value>) -> Option<String> {
    text(value).or_else(|| {
        value
            .and_then(|v| v.as_i64())
            .and_then(chrono::DateTime::from_timestamp_millis)
            .map(|v| v.to_rfc3339())
    })
}
pub(super) fn fetch(
    http: &dyn BalanceHttp,
    source: HttpSource,
    secret: &str,
    org: Option<&str>,
    proxy: Option<&str>,
) -> Result<serde_json::Value, UsageError> {
    let (status, body) = http.get(
        &UsageRequest {
            source,
            secret,
            org,
        },
        proxy,
    )?;
    json(status, &body)
}
pub(super) fn decimal(v: Option<&serde_json::Value>) -> Result<Option<String>, UsageError> {
    let Some(v) = v.filter(|v| !v.is_null()) else {
        return Ok(None);
    };
    let s = if let Some(s) = v.as_str() {
        s.to_owned()
    } else if v.is_number() {
        v.to_string()
    } else {
        return Err(UsageError::Response);
    };
    let n: f64 = s.parse().map_err(|_| UsageError::Response)?;
    if !n.is_finite() {
        return Err(UsageError::Response);
    }
    Ok(Some(s))
}
pub(super) fn fetch_console(
    http: &dyn BalanceHttp,
    provider: Provider,
    route: u8,
    secret: &str,
    proxy: Option<&str>,
) -> Result<serde_json::Value, UsageError> {
    let (status, body) = http.get(
        &UsageRequest {
            source: HttpSource::Provider(provider, route),
            secret,
            org: None,
        },
        proxy,
    )?;
    if provider == Provider::MiMo && status == 302 {
        return Err(UsageError::Authentication);
    }
    if provider == Provider::StepFun {
        stepfun::response(status, &body)
    } else {
        json(status, &body)
    }
}
pub(super) fn query(
    http: &dyn BalanceHttp,
    provider: Provider,
    secret: &str,
    proxy: Option<&str>,
) -> Result<Balance, UsageError> {
    match provider {
        Provider::StepFun => stepfun::query(http, secret, proxy),
        Provider::MiMo => mimo::query(http, secret, proxy),
        _ => Err(UsageError::Authentication),
    }
    .map(Balance::Provider)
}
pub(super) fn parse_manual(
    provider: Provider,
    input: &str,
) -> Result<Zeroizing<String>, UsageError> {
    let material = parse_session(provider, input)?;
    if material.len() > 2560 {
        return Err(UsageError::Credentials);
    }
    Ok(material)
}
pub(super) fn parse_session(
    provider: Provider,
    input: &str,
) -> Result<Zeroizing<String>, UsageError> {
    if input.len() > 64 * 1024 || input.chars().any(char::is_control) {
        return Err(UsageError::Authentication);
    }
    let input = input
        .trim()
        .strip_prefix("Cookie:")
        .unwrap_or(input.trim())
        .trim();
    let mut fields = std::collections::HashMap::new();
    for part in input.split(';').map(str::trim).filter(|p| !p.is_empty()) {
        let (name, value) = part.split_once('=').ok_or(UsageError::Authentication)?;
        if !matches!(
            name,
            "Oasis-Token" | "Oasis-Webid" | "api-platform_serviceToken" | "serviceToken" | "userId"
        ) {
            continue;
        }
        if value.is_empty()
            || !value
                .bytes()
                .all(|b| (0x21..=0x7e).contains(&b) && b != b';' && b != b',')
            || fields.insert(name, value).is_some()
        {
            return Err(UsageError::Authentication);
        }
    }
    let value = match provider {
        Provider::StepFun => {
            if fields.contains_key("serviceToken")
                || fields.contains_key("api-platform_serviceToken")
                || fields.contains_key("userId")
            {
                return Err(UsageError::Authentication);
            }
            let token = fields
                .get("Oasis-Token")
                .ok_or(UsageError::Authentication)?;
            let id = fields
                .get("Oasis-Webid")
                .ok_or(UsageError::Authentication)?;
            format!("Oasis-Token={token}; Oasis-Webid={id}")
        }
        Provider::MiMo => {
            if fields.contains_key("Oasis-Token")
                || fields.contains_key("Oasis-Webid")
                || (fields.contains_key("serviceToken")
                    && fields.contains_key("api-platform_serviceToken"))
            {
                return Err(UsageError::Authentication);
            }
            let token = fields
                .get("api-platform_serviceToken")
                .or_else(|| fields.get("serviceToken"))
                .ok_or(UsageError::Authentication)?;
            let id = fields.get("userId").ok_or(UsageError::Authentication)?;
            format!("api-platform_serviceToken={token}; userId={id}")
        }
        _ => return Err(UsageError::Authentication),
    };
    Ok(Zeroizing::new(value))
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::settings::usage::{BalanceHttp, UsageRequest};
    use std::sync::Mutex;

    struct Http {
        replies: Mutex<std::collections::VecDeque<(u16, Vec<u8>)>>,
        seen: Mutex<Vec<(String, String)>>,
    }
    impl BalanceHttp for Http {
        fn get(
            &self,
            request: &UsageRequest<'_>,
            _: Option<&str>,
        ) -> Result<(u16, Vec<u8>), UsageError> {
            self.seen
                .lock()
                .unwrap()
                .push((request.endpoint().to_owned(), request.authorization_value()));
            self.replies
                .lock()
                .unwrap()
                .pop_front()
                .ok_or(UsageError::Network)
        }
    }

    #[test]
    fn manual_session_sources_cannot_cross_providers_or_inject_headers() {
        assert!(
            parse_manual(
                Provider::StepFun,
                "api-platform_serviceToken=other; userId=123"
            )
            .is_err()
        );
        assert!(parse_manual(Provider::MiMo, "Oasis-Token=other").is_err());
        assert!(parse_manual(Provider::MiMo, "serviceToken=t").is_err());
        assert!(parse_manual(Provider::MiMo, "serviceToken=t; userId=one; userId=two").is_err());
        assert!(parse_manual(Provider::StepFun, "Oasis-Token=t\r\nHost: evil.invalid").is_err());
        let material = parse_manual(
            Provider::MiMo,
            "serviceToken=abc==; userId=1024; unrelated=do-not-store",
        )
        .unwrap();
        assert_eq!(
            material.as_str(),
            "api-platform_serviceToken=abc==; userId=1024"
        );
        let material = parse_manual(
            Provider::StepFun,
            "Oasis-Token=step-session; Oasis-Webid=device; unrelated=do-not-store",
        )
        .unwrap();
        assert_eq!(
            material.as_str(),
            "Oasis-Token=step-session; Oasis-Webid=device"
        );
    }

    #[test]
    fn vault_size_limit_applies_to_manual_secrets_not_browser_request_material() {
        let token = "t".repeat(3000);
        let input = format!("Oasis-Token={token}; Oasis-Webid=web-id");
        assert_eq!(
            parse_manual(Provider::StepFun, &input).err(),
            Some(UsageError::Credentials)
        );
        assert_eq!(
            parse_session(Provider::StepFun, &input).unwrap().as_str(),
            input
        );
    }

    #[test]
    fn rejected_session_never_becomes_a_balance_or_sends_to_another_provider() {
        let http = Http {
            replies: Mutex::new([(401, b"{}".to_vec())].into()),
            seen: Mutex::new(Vec::new()),
        };
        let material = parse_manual(Provider::MiMo, "serviceToken=only-mimo; userId=100").unwrap();
        assert_eq!(
            query(&http, Provider::MiMo, &material, None),
            Err(UsageError::Authentication)
        );
        let seen = http.seen.lock().unwrap();
        assert_eq!(seen[0].0, "https://platform.xiaomimimo.com/api/v1/balance");
        assert_eq!(seen[0].1, "api-platform_serviceToken=only-mimo; userId=100");
    }

    #[test]
    fn mimo_preserves_first_party_usage_shape_currency_identity_and_utc_expiry() {
        let http = Http { replies: Mutex::new([
            (200, br#"{"code":0,"data":{"balance":"12.50","cashBalance":"10","giftBalance":"2.50","currency":"CNY"}}"#.to_vec()),
            (200, br#"{"code":0,"data":{"userId":"1024","idc":310}}"#.to_vec()),
            (200, br#"{"code":0,"data":{"planCode":"personal:monthly","currentPeriodEnd":"2026-10-31 23:59:59","expired":false}}"#.to_vec()),
            (200, br#"{"code":0,"data":{"usage":{"items":[{"name":"compensation_total_token","used":25,"limit":100,"percent":0.25}]}}}"#.to_vec()),
        ].into()), seen: Mutex::new(Vec::new()) };
        let Balance::Provider(report) = query(
            &http,
            Provider::MiMo,
            "serviceToken=session; userId=1024",
            None,
        )
        .unwrap() else {
            panic!("provider report")
        };
        assert_eq!(report.identity.as_deref(), Some("1024"));
        assert_eq!(report.pools[0].remaining.as_deref(), Some("12.50"));
        assert_eq!(report.pools[0].unit, "CNY");
        assert_eq!(report.pools[3].remaining, None);
        let pool = &report.pools[4];
        assert_eq!(pool.name.as_deref(), Some("compensation_total_token"));
        assert_eq!(pool.used.as_deref(), Some("25"));
        assert_eq!(pool.limit.as_deref(), Some("100"));
        assert_eq!(pool.expires.as_deref(), Some("2026-10-31 23:59:59 UTC"));
        assert!(pool.reset.is_none(), "expiry is not an invented reset time");
    }

    #[test]
    fn mimo_rejects_mismatched_identity_and_distinguishes_business_rate_limit() {
        let http = Http {
            replies: Mutex::new(
                [
                    (200, br#"{"code":0,"data":{"balance":1}}"#.to_vec()),
                    (
                        200,
                        br#"{"code":0,"data":{"userId":"other","idc":310}}"#.to_vec(),
                    ),
                ]
                .into(),
            ),
            seen: Mutex::new(Vec::new()),
        };
        assert_eq!(
            query(
                &http,
                Provider::MiMo,
                "serviceToken=session; userId=1024",
                None
            ),
            Err(UsageError::Authentication)
        );
        let http = Http {
            replies: Mutex::new([(200, br#"{"code":400429}"#.to_vec())].into()),
            seen: Mutex::new(Vec::new()),
        };
        assert_eq!(
            query(
                &http,
                Provider::MiMo,
                "serviceToken=session; userId=1024",
                None
            ),
            Err(UsageError::RateLimited)
        );
        let http = Http {
            replies: Mutex::new([(302, b"{}".to_vec())].into()),
            seen: Mutex::new(Vec::new()),
        };
        assert_eq!(
            query(
                &http,
                Provider::MiMo,
                "serviceToken=session; userId=1024",
                None
            ),
            Err(UsageError::Authentication)
        );
    }

    #[test]
    fn stepfun_coding_windows_and_credit_family_are_not_combined() {
        let http = Http { replies: Mutex::new([
            (200, br#"{"status":"CODE_OK","planFamily":"PLAN_FAMILY_CODING","fiveHourUsageLeftRate":0.25,"fiveHourUsageResetTime":"1792400000","weeklyUsageLeftRate":0.0}"#.to_vec()),
            (200, br#"{"status":"CODE_OK","uid":"9007199254740993","activated":true}"#.to_vec()),
            (200, br#"{"status":"CODE_OK","subscription":{"name":"Plan Pro","status":"STEP_PLAN_SUB_STATUS_EXPIRED","expiredAt":"1793000000"}}"#.to_vec()),
        ].into()), seen: Mutex::new(Vec::new()) };
        let Balance::Provider(report) = query(
            &http,
            Provider::StepFun,
            "Oasis-Token=fixture; Oasis-Webid=device",
            None,
        )
        .unwrap() else {
            panic!("provider report")
        };
        assert_eq!(report.plan.as_deref(), Some("Plan Pro"));
        assert_eq!(report.identity.as_deref(), Some("9007199254740993"));
        assert_eq!(
            report.plan_expires.as_deref(),
            Some("2026-10-26T07:33:20+00:00")
        );
        assert_eq!(report.expired, Some(true));
        assert_eq!(
            report.pools[0].reset.as_deref(),
            Some("2026-10-19T08:53:20+00:00")
        );
        assert_eq!(report.pools[0].remaining.as_deref(), Some("25"));
        assert_eq!(report.pools[0].unit, "%");
        assert_eq!(report.pools[1].remaining.as_deref(), Some("0"));
        assert!(report.pools[1].reset.is_none());
        let http = Http { replies: Mutex::new([
            (200, br#"{"status":"CODE_OK","planFamily":"PLAN_FAMILY_TOKEN_V3","planCreditRateLimit":{"subscriptionCreditResetTime":"1792400000","creditBuckets":[{"type":"TYPE_SUBSCRIPTION","creditTotal":"9007199254740993","creditResidual":"1234567","expireAt":"1793000000","nextResetAt":"1792400000"}]}}"#.to_vec()),
            (200, br#"{"status":"CODE_OK","uid":"42","activated":true}"#.to_vec()),
            (200, br#"{"status":"CODE_OK","subscription":{"name":"Token Pro"}}"#.to_vec()),
        ].into()), seen: Mutex::new(Vec::new()) };
        let Balance::Provider(report) = query(
            &http,
            Provider::StepFun,
            "Oasis-Token=fixture; Oasis-Webid=device",
            None,
        )
        .unwrap() else {
            panic!("provider report")
        };
        assert_eq!(report.pools[0].remaining.as_deref(), Some("1234567"));
        assert_eq!(report.pools[0].limit.as_deref(), Some("9007199254740993"));
        assert_eq!(report.pools[0].unit, "Credit");
        assert_eq!(
            report.pools[0].reset.as_deref(),
            Some("2026-10-19T08:53:20+00:00")
        );
        assert_eq!(
            report.pools[0].expires.as_deref(),
            Some("2026-10-26T07:33:20+00:00")
        );
        assert!(
            report
                .pools
                .iter()
                .all(|pool| pool.kind != PoolKind::FiveHour)
        );
    }

    #[test]
    fn stepfun_missing_ratio_and_unknown_family_never_become_full_quota() {
        let http = Http {
            replies: Mutex::new(
                [
                    (
                        200,
                        br#"{"status":"CODE_OK","planFamily":"PLAN_FAMILY_CODING"}"#.to_vec(),
                    ),
                    (
                        200,
                        br#"{"status":"CODE_OK","uid":"42","activated":true}"#.to_vec(),
                    ),
                    (200, br#"{"status":"CODE_OK"}"#.to_vec()),
                ]
                .into(),
            ),
            seen: Mutex::new(Vec::new()),
        };
        let Balance::Provider(report) = query(
            &http,
            Provider::StepFun,
            "Oasis-Token=fixture; Oasis-Webid=device",
            None,
        )
        .unwrap() else {
            panic!("provider report")
        };
        assert!(report.pools.iter().all(|p| p.remaining.is_none()));
        let http = Http {
            replies: Mutex::new(
                [(
                    200,
                    br#"{"status":"CODE_OK","planFamily":"FUTURE_FAMILY"}"#.to_vec(),
                )]
                .into(),
            ),
            seen: Mutex::new(Vec::new()),
        };
        assert_eq!(
            query(
                &http,
                Provider::StepFun,
                "Oasis-Token=fixture; Oasis-Webid=device",
                None
            ),
            Err(UsageError::Response)
        );
    }
}
