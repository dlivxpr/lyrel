use super::{BalanceHttp, HttpSource, UsageRequest, accounts::UsageError};
use serde::{Deserialize, Serialize};
pub(super) mod command_code;
pub(super) mod kimi;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(super) enum Provider {
    #[default]
    DeepSeek,
    Kimi,
    CommandCode,
}
impl Provider {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::DeepSeek => "DeepSeek",
            Self::Kimi => "Kimi Code",
            Self::CommandCode => "Command Code",
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
    Purchased,
    Free,
    Booster,
    Coding,
    CodeFiveHour,
    CodeWeekly,
    BoosterMonthly,
    BillingUsage,
    RequestCount,
    SandboxMinutes,
    OrganizationLimit,
    RawBalance,
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
            since: None,
        },
        proxy,
    )?;
    json(status, &body)
}
