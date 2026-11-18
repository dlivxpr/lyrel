//! GUI-free usage account models, provider scrapers, and the platform
//! session contract. Desktop browser cookie/session readers live under
//! [`browser`] (macOS/Windows only).
//!
//! The root `lyrel` GUI keeps every GPUI settings view; it consumes this
//! module for models, decoding and scrapers while injecting the OS
//! credential vault through [`accounts::SecretVault`].

pub mod accounts;
#[cfg(any(target_os = "macos", target_os = "windows"))]
pub mod browser;
pub mod platform;
pub mod providers;

use accounts::{Balance, UsageError};
use providers::Provider;

const ENDPOINT: &str = "https://api.deepseek.com/user/balance";

#[derive(Clone, Copy)]
pub enum HttpSource {
    Official,
    Platform(platform::SessionKind),
    KimiApi,
    KimiMe,
    KimiWeb,
    CommandWhoami,
    CommandCredits,
    CommandSubscriptions,
    CommandSessionIdentity,
    CommandEnhancement,
    Provider(Provider, u8),
}

pub struct UsageRequest<'a> {
    pub source: HttpSource,
    pub secret: &'a str,
    pub org: Option<&'a str>,
}

impl UsageRequest<'_> {
    pub fn endpoint(&self) -> String {
        let base = match self.source {
            HttpSource::Official => ENDPOINT,
            HttpSource::Platform(_) => platform::ENDPOINT,
            HttpSource::KimiApi => "https://api.kimi.com/coding/v1/usages",
            HttpSource::KimiMe => "https://api.kimi.com/coding/v1/me",
            HttpSource::KimiWeb => {
                "https://www.kimi.com/apiv2/kimi.gateway.membership.v2.MembershipService/GetSubscriptionStats"
            }
            HttpSource::CommandWhoami => "https://api.commandcode.ai/alpha/whoami?limits=1",
            HttpSource::CommandCredits => "https://api.commandcode.ai/alpha/billing/credits",
            HttpSource::CommandSubscriptions => {
                "https://api.commandcode.ai/alpha/billing/subscriptions"
            }
            HttpSource::CommandSessionIdentity => providers::command_code::IDENTITY_ENDPOINT,
            HttpSource::CommandEnhancement => providers::command_code::SESSION_ENDPOINT,
            HttpSource::Provider(provider, route) => match (provider, route) {
                (Provider::StepFun, 0) => {
                    "https://platform.stepfun.com/api/step.openapi.devcenter.Dashboard/QueryStepPlanRateLimit"
                }
                (Provider::StepFun, 1) => {
                    "https://platform.stepfun.com/api/step.openapi.devcenter.Dashboard/GetStepPlanStatus"
                }
                (Provider::StepFun, _) => {
                    "https://platform.stepfun.com/api/step.openapi.devcenter.Dashboard/UserInfo"
                }
                (Provider::MiMo, 0) => "https://platform.xiaomimimo.com/api/v1/balance",
                (Provider::MiMo, 1) => "https://platform.xiaomimimo.com/api/v1/tokenPlan/detail",
                (Provider::MiMo, 2) => "https://platform.xiaomimimo.com/api/v1/tokenPlan/usage",
                (Provider::MiMo, _) => "https://platform.xiaomimimo.com/api/v1/userProfile",
                _ => unreachable!("console provider route"),
            },
        };
        let mut params = url::form_urlencoded::Serializer::new(String::new());
        if let Some(org) = self.org {
            params.append_pair("orgId", org);
        }
        let query = params.finish();
        if query.is_empty() {
            base.to_owned()
        } else {
            format!("{base}?{query}")
        }
    }

    pub fn header_name(&self) -> &'static str {
        match self.source {
            HttpSource::CommandEnhancement | HttpSource::CommandSessionIdentity => "Cookie",
            HttpSource::Official
            | HttpSource::KimiApi
            | HttpSource::KimiMe
            | HttpSource::KimiWeb
            | HttpSource::CommandWhoami
            | HttpSource::CommandCredits
            | HttpSource::CommandSubscriptions => "Authorization",
            HttpSource::Platform(kind) => kind.header_name(),
            HttpSource::Provider(Provider::StepFun, _) => "Oasis-Token",
            HttpSource::Provider(..) => "Cookie",
        }
    }

    pub fn authorization_value(&self) -> String {
        match self.source {
            HttpSource::CommandEnhancement | HttpSource::CommandSessionIdentity => {
                self.secret.to_owned()
            }
            HttpSource::Official
            | HttpSource::KimiApi
            | HttpSource::KimiMe
            | HttpSource::KimiWeb
            | HttpSource::CommandWhoami
            | HttpSource::CommandCredits
            | HttpSource::CommandSubscriptions => format!("Bearer {}", self.secret),
            HttpSource::Platform(kind) => kind.authorization_value(self.secret),
            HttpSource::Provider(Provider::StepFun, _) => {
                self.session_field("Oasis-Token").to_owned()
            }
            HttpSource::Provider(..) => self.secret.to_owned(),
        }
    }

    pub fn session_field(&self, name: &str) -> &str {
        self.secret
            .split("; ")
            .filter_map(|part| part.split_once('='))
            .find_map(|(key, value)| (key == name).then_some(value))
            .unwrap_or("")
    }
}

/// The varying external HTTP boundary; source selection, parsing and transitions remain real.
pub trait BalanceHttp: Send + Sync {
    fn get(
        &self,
        request: &UsageRequest<'_>,
        proxy: Option<&str>,
    ) -> Result<(u16, Vec<u8>), UsageError>;
}

pub fn query(
    http: &dyn BalanceHttp,
    source: HttpSource,
    secret: &str,
    proxy: Option<&str>,
) -> Result<Balance, UsageError> {
    if let HttpSource::Provider(provider, _) = source {
        return providers::query(http, provider, secret, proxy);
    }
    if secret.is_empty() || secret.contains(char::is_whitespace) {
        return Err(UsageError::Authentication);
    }
    let (status, body) = http.get(
        &UsageRequest {
            source,
            secret,
            org: None,
        },
        proxy,
    )?;
    match source {
        HttpSource::Official => accounts::decode(status, &body),
        HttpSource::Platform(_) => platform::decode(status, &body).map(Balance::Platform),
        _ => Err(UsageError::Response),
    }
}

pub fn query_provider(
    http: &dyn BalanceHttp,
    provider: Provider,
    source: HttpSource,
    secret: &str,
    proxy: Option<&str>,
) -> Result<Balance, UsageError> {
    match provider {
        Provider::DeepSeek => query(http, source, secret, proxy),
        Provider::Kimi => {
            providers::kimi::query(http, !matches!(source, HttpSource::Official), secret, proxy)
                .map(Balance::Provider)
        }
        Provider::CommandCode => {
            providers::command_code::query(http, secret, proxy).map(Balance::Provider)
        }
        Provider::StepFun | Provider::MiMo => providers::query(http, provider, secret, proxy),
    }
}
