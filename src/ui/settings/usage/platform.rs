//! 私有 Platform 契约；余额投影不复用官方 API-key 的 total_balance 口径。
//!
//! 第一方：deepseek-ai/deepseek-harness，commit 687f282e8625c6b8d11ff58211a90524be2e4317：
//! packages/credentials/deepseek-account-platform/src/{index,protocol,details}.ts
//! https://github.com/deepseek-ai/deepseek-harness/tree/687f282e8625c6b8d11ff58211a90524be2e4317/packages/credentials/deepseek-account-platform
//! grant 是 {version:1,token,issuer} 存储对象；issuer 精确匹配配置 origin，绝非猜测 JWT iss。
//! grant 使用 x-dsh-auth-token；钱包、decimal 语法和 40003 均据上述第一方代码。
//!
//! WebToken 的 Bearer 认证仍未获第一方核验：2026-10-19 无认证读取 Platform 首页、
//! /sign_in 和 /usage 均返回 HTTP 429，无法取得官方 Web JS。显式参考来源：
//! https://github.com/shajanjp/deepseek-usage/blob/main/lib/deepseek.js (fetchUserSummary)
//! https://github.com/steipete/CodexBar/blob/main/docs/deepseek.md
//! 此路径是私有接口兼容实现，不代表官方支持或已通过真实账号验证。
use super::accounts::UsageError;
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

pub(super) const ENDPOINT: &str = "https://platform.deepseek.com/api/v0/users/get_user_summary";
const ORIGIN: &str = "https://platform.deepseek.com";
const MAX_SECRET_BYTES: usize = 2560;

/// 可持久化的非秘密来源类型；不从 token 外观猜测两种认证方式。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) enum SessionKind {
    WebToken,
    HarnessGrant,
}
impl SessionKind {
    pub(super) fn header_name(self) -> &'static str {
        match self {
            Self::WebToken => "Authorization",
            Self::HarnessGrant => "x-dsh-auth-token",
        }
    }

    pub(super) fn authorization_value(self, secret: &str) -> String {
        match self {
            Self::WebToken => format!("Bearer {secret}"),
            Self::HarnessGrant => secret.to_owned(),
        }
    }
}

/// 只保留请求所需秘密；不可调试输出或序列化整个粘贴材料。
pub(super) struct SessionMaterial {
    kind: SessionKind,
    secret: Zeroizing<String>,
}
impl SessionMaterial {
    pub(super) fn kind(&self) -> SessionKind {
        self.kind
    }

    pub(super) fn secret(&self) -> &str {
        &self.secret
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Wallet {
    pub currency: String,
    pub balance: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct PlatformBalance {
    pub normal_wallets: Vec<Wallet>,
    pub bonus_wallets: Vec<Wallet>,
}

fn material(kind: SessionKind, token: &str) -> Result<SessionMaterial, UsageError> {
    if token.is_empty()
        || token.len() > MAX_SECRET_BYTES
        || token.starts_with("sk-")
        || !token.bytes().all(|b| match kind {
            SessionKind::WebToken => b.is_ascii_alphanumeric() || b"-_.+/=".contains(&b),
            SessionKind::HarnessGrant => (0x21..=0x7e).contains(&b),
        })
    {
        return Err(UsageError::Authentication);
    }
    Ok(SessionMaterial {
        kind,
        secret: Zeroizing::new(token.to_owned()),
    })
}

/// 明确字段决定来源；不会将 Cookie、API key 或其他域的请求降级为裸 token。
pub(super) fn parse_manual(input: &str) -> Result<SessionMaterial, UsageError> {
    let input = input.trim();
    if input.len() > 16 * 1024 {
        return Err(UsageError::Authentication);
    }
    if input.starts_with('{') {
        return parse_object(input);
    }
    if input.starts_with("curl ") || input.starts_with("curl\t") {
        return parse_curl(input);
    }
    for field in ["userToken", "user_token"] {
        if let Some(rest) = input.strip_prefix(field) {
            let rest = rest.trim_start();
            if let Some(value) = rest.strip_prefix('=').or_else(|| rest.strip_prefix(':')) {
                let value = value.trim();
                let value = unquote(value);
                return material(SessionKind::WebToken, value);
            }
        }
    }
    material(SessionKind::WebToken, input)
}

/// 解析选定 userToken 的原始 DOMString，不把浏览器来源升级成 Harness grant。
/// JSON 字符串是 LocalStorage DOMString 的兼容输入；value/userToken 包装来自
/// vortex/第三方参考，尚未取得第一方 Web JS 证明，不能据此宣称官方契约。
/// 调用方保留原始存储值用于来源绑定，只将归一化秘密留在请求内存。
pub(super) fn parse_browser(input: &str) -> Result<SessionMaterial, UsageError> {
    let input = input.trim();
    if input.starts_with("userToken=") || input.starts_with("user_token=") {
        return Err(UsageError::Authentication);
    }
    if input.len() > 16 * 1024 {
        return Err(UsageError::Authentication);
    }
    if !input.starts_with('{') && !input.starts_with('"') {
        return material(SessionKind::WebToken, input);
    }
    let value: serde_json::Value =
        serde_json::from_str(input).map_err(|_| UsageError::Authentication)?;
    let token = if let Some(token) = value.as_str() {
        token
    } else {
        let object = value.as_object().ok_or(UsageError::Authentication)?;
        if object
            .keys()
            .any(|key| !matches!(key.as_str(), "value" | "userToken" | "expires"))
            || (object.contains_key("value") && object.contains_key("userToken"))
        {
            return Err(UsageError::Authentication);
        }
        let token_value = object
            .get("value")
            .or_else(|| object.get("userToken"))
            .ok_or(UsageError::Authentication)?;
        if let Some(token) = token_value.as_str() {
            token
        } else {
            let wrapped = token_value.as_object().ok_or(UsageError::Authentication)?;
            if !object.contains_key("userToken")
                || wrapped
                    .keys()
                    .any(|key| !matches!(key.as_str(), "value" | "expires"))
            {
                return Err(UsageError::Authentication);
            }
            wrapped
                .get("value")
                .and_then(|v| v.as_str())
                .ok_or(UsageError::Authentication)?
        }
    };
    material(SessionKind::WebToken, token)
}

fn unquote(value: &str) -> &str {
    if value.len() >= 2
        && ((value.starts_with('"') && value.ends_with('"'))
            || (value.starts_with('\'') && value.ends_with('\'')))
    {
        &value[1..value.len() - 1]
    } else {
        value
    }
}

fn parse_object(input: &str) -> Result<SessionMaterial, UsageError> {
    let value: serde_json::Value =
        serde_json::from_str(input).map_err(|_| UsageError::Authentication)?;
    let object = value.as_object().ok_or(UsageError::Authentication)?;
    if object.keys().any(|key| {
        key.eq_ignore_ascii_case("cookie")
            || key.eq_ignore_ascii_case("api_key")
            || key.eq_ignore_ascii_case("apiKey")
    }) {
        return Err(UsageError::Authentication);
    }
    let web = object.get("userToken").or_else(|| object.get("user_token"));
    if let Some(web) = web {
        if (object.contains_key("userToken") && object.contains_key("user_token"))
            || ["kind", "payload", "version", "token", "issuer"]
                .iter()
                .any(|key| object.contains_key(*key))
        {
            return Err(UsageError::Authentication);
        }
        let token = web
            .as_str()
            .or_else(|| web.get("value").and_then(|v| v.as_str()))
            .ok_or(UsageError::Authentication)?;
        return material(SessionKind::WebToken, token);
    }
    // 官方 CredentialStore 的 record 包装是 kind:grant + payload；也接受其原始 payload。
    let grant = if object.get("kind").and_then(|v| v.as_str()) == Some("grant") {
        if ["version", "token", "issuer"]
            .iter()
            .any(|key| object.contains_key(*key))
        {
            return Err(UsageError::Authentication);
        }
        object.get("payload").ok_or(UsageError::Authentication)?
    } else {
        &value
    };
    if grant.get("version").and_then(|v| v.as_u64()) != Some(1)
        || grant.get("issuer").and_then(|v| v.as_str()) != Some(ORIGIN)
    {
        return Err(UsageError::Authentication);
    }
    material(
        SessionKind::HarnessGrant,
        grant
            .get("token")
            .and_then(|v| v.as_str())
            .ok_or(UsageError::Authentication)?,
    )
}

// 不执行 shell。只接受可明确解析的单请求 curl，不推测 shell 展开、重定向或命令拼接。
fn curl_words(input: &str) -> Result<Vec<&str>, UsageError> {
    let mut words = Vec::new();
    let mut rest = input;
    while !rest.trim_start().is_empty() {
        rest = rest.trim_start();
        if let Some(continued) = rest
            .strip_prefix("\\\r\n")
            .or_else(|| rest.strip_prefix("\\\n"))
        {
            rest = continued;
            continue;
        }
        if rest.starts_with('"') || rest.starts_with('\'') {
            let quote = rest.as_bytes()[0] as char;
            let end = rest[1..].find(quote).ok_or(UsageError::Authentication)? + 1;
            words.push(&rest[1..end]);
            rest = &rest[end + 1..];
            if !rest.is_empty() && !rest.starts_with(char::is_whitespace) {
                return Err(UsageError::Authentication);
            }
        } else {
            let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
            let word = &rest[..end];
            if word.bytes().any(|b| b"\"'`$;&|<>\\".contains(&b)) {
                return Err(UsageError::Authentication);
            }
            words.push(word);
            rest = &rest[end..];
        }
    }
    Ok(words)
}

fn parse_curl(input: &str) -> Result<SessionMaterial, UsageError> {
    let words = curl_words(input)?;
    let mut args = words.iter().copied().skip(1);
    let mut endpoint_seen = false;
    let mut token = None;
    while let Some(arg) = args.next() {
        match arg {
            "-H" | "--header" => {
                let header = args.next().ok_or(UsageError::Authentication)?;
                let (name, value) = header.split_once(':').ok_or(UsageError::Authentication)?;
                let name = name.trim();
                if name.eq_ignore_ascii_case("cookie")
                    || name.eq_ignore_ascii_case("host")
                    || name.eq_ignore_ascii_case("x-dsh-auth-token")
                {
                    return Err(UsageError::Authentication);
                }
                if name.eq_ignore_ascii_case("authorization") {
                    if token.is_some() {
                        return Err(UsageError::Authentication);
                    }
                    token = Some(
                        value
                            .trim()
                            .strip_prefix("Bearer ")
                            .ok_or(UsageError::Authentication)?,
                    );
                }
            }
            "-X" | "--request" => {
                if args.next() != Some("GET") {
                    return Err(UsageError::Authentication);
                }
            }
            "--compressed" => {}
            "--url" => {
                if endpoint_seen || args.next() != Some(ENDPOINT) {
                    return Err(UsageError::Authentication);
                }
                endpoint_seen = true;
            }
            ENDPOINT if !endpoint_seen => endpoint_seen = true,
            _ => return Err(UsageError::Authentication),
        }
    }
    if !endpoint_seen {
        return Err(UsageError::Authentication);
    }
    material(
        SessionKind::WebToken,
        token.ok_or(UsageError::Authentication)?,
    )
}

/// 只投影两类钱包；响应中的 token、用户资料和错误文本不进入快照或错误对象。
pub(super) fn decode(status: u16, body: &[u8]) -> Result<PlatformBalance, UsageError> {
    if status == 401 {
        return Err(UsageError::Authentication);
    }
    let value = if body.len() <= 64 * 1024 {
        serde_json::from_slice::<serde_json::Value>(body).ok()
    } else {
        None
    };
    if value
        .as_ref()
        .and_then(|v| v.get("code"))
        .and_then(|v| v.as_i64())
        == Some(40003)
    {
        return Err(UsageError::Authentication);
    }
    if status == 429 {
        return Err(UsageError::RateLimited);
    }
    if !(200..300).contains(&status) {
        return Err(UsageError::Service);
    }
    let value = value.ok_or(UsageError::Response)?;
    let code = value
        .get("code")
        .and_then(|v| v.as_i64())
        .ok_or(UsageError::Response)?;
    if code != 0 {
        // 40002 的认证语义没有第一方证明，不能将其冒称为会话过期。
        return Err(UsageError::Service);
    }
    let data = value.get("data").ok_or(UsageError::Response)?;
    let biz_code = data
        .get("biz_code")
        .and_then(|v| v.as_i64())
        .ok_or(UsageError::Response)?;
    if biz_code != 0 {
        return Err(UsageError::Service);
    }
    let payload = data.get("biz_data").ok_or(UsageError::Response)?;
    Ok(PlatformBalance {
        normal_wallets: wallets(payload.get("normal_wallets"))?,
        bonus_wallets: wallets(payload.get("bonus_wallets"))?,
    })
}

fn wallets(value: Option<&serde_json::Value>) -> Result<Vec<Wallet>, UsageError> {
    value
        .and_then(|v| v.as_array())
        .ok_or(UsageError::Response)?
        .iter()
        .map(|row| {
            let currency = row
                .get("currency")
                .and_then(|v| v.as_str())
                .ok_or(UsageError::Response)?;
            let balance = row
                .get("balance")
                .and_then(|v| v.as_str())
                .ok_or(UsageError::Response)?;
            if !matches!(currency, "CNY" | "USD") || !decimal(balance) {
                return Err(UsageError::Response);
            }
            Ok(Wallet {
                currency: currency.to_owned(),
                balance: balance.to_owned(),
            })
        })
        .collect()
}

// 与第一方 /^-?(?:\d+(?:\.\d*)?|\.\d+)(?:e[+-]?\d+)?$/i 等价；不做浮点换算。
fn decimal(value: &str) -> bool {
    let bytes = value.as_bytes();
    let mut i = usize::from(bytes.first() == Some(&b'-'));
    let mut digits = 0;
    while bytes.get(i).is_some_and(u8::is_ascii_digit) {
        i += 1;
        digits += 1;
    }
    if bytes.get(i) == Some(&b'.') {
        i += 1;
        while bytes.get(i).is_some_and(u8::is_ascii_digit) {
            i += 1;
            digits += 1;
        }
    }
    if digits == 0 {
        return false;
    }
    if matches!(bytes.get(i), Some(b'e' | b'E')) {
        i += 1;
        if matches!(bytes.get(i), Some(b'+' | b'-')) {
            i += 1;
        }
        let exponent_start = i;
        while bytes.get(i).is_some_and(u8::is_ascii_digit) {
            i += 1;
        }
        if i == exponent_start {
            return false;
        }
    }
    i == bytes.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browser_dom_strings_normalize_only_web_tokens() {
        for input in [
            "web-token",
            r#""web-token""#,
            r#"{"value":"web-token","expires":123}"#,
            r#"{"userToken":"web-token"}"#,
            r#"{"userToken":{"value":"web-token","expires":123}}"#,
        ] {
            let material = parse_browser(input).unwrap();
            assert_eq!(material.kind(), SessionKind::WebToken);
            assert_eq!(material.secret(), "web-token");
        }
        for input in [
            "Cookie: userToken=web-token",
            "sk-api-key",
            "userToken=web-token",
            r#""sk-api-key""#,
            r#"{"value":"sk-api-key"}"#,
            r#"{"version":1,"token":"opaque-grant","issuer":"https://platform.deepseek.com"}"#,
            r#"{"kind":"grant","payload":{"version":1,"token":"opaque-grant","issuer":"https://platform.deepseek.com"}}"#,
            r#"{"value":"one","userToken":"two"}"#,
            r#"{"userToken":"web-token","Cookie":"other=value"}"#,
            r#"{"userToken":{"value":"web-token","Cookie":"other=value"}}"#,
        ] {
            assert!(parse_browser(input).is_err());
        }
    }

    #[test]
    fn explicit_grants_are_not_classified_by_an_uncontracted_token_prefix() {
        let input =
            r#"{"version":1,"token":"dsh_mock_opaque","issuer":"https://platform.deepseek.com"}"#;
        assert_eq!(
            parse_manual(input).unwrap().kind(),
            SessionKind::HarnessGrant
        );
    }

    #[test]
    fn manual_web_sources_keep_only_the_required_token() {
        for input in [
            "eyJhbGci.eyJ1c2Vy.signature",
            "userToken=eyJhbGci.eyJ1c2Vy.signature",
            "user_token: eyJhbGci.eyJ1c2Vy.signature",
            r#"{"userToken":"eyJhbGci.eyJ1c2Vy.signature","unused":"discard"}"#,
            r#"{"user_token":{"value":"eyJhbGci.eyJ1c2Vy.signature","expires":123}}"#,
        ] {
            let material = parse_manual(input).unwrap();
            assert_eq!(material.kind(), SessionKind::WebToken);
            assert_eq!(material.secret(), "eyJhbGci.eyJ1c2Vy.signature");
        }
    }

    #[test]
    fn explicit_harness_grant_checks_the_stored_issuer_not_a_jwt_claim() {
        for input in [
            r#"{"version":1,"token":"opaque-grant","issuer":"https://platform.deepseek.com"}"#,
            r#"{"kind":"grant","payload":{"version":1,"token":"opaque-grant","issuer":"https://platform.deepseek.com"}}"#,
        ] {
            let material = parse_manual(input).unwrap();
            assert_eq!(material.kind(), SessionKind::HarnessGrant);
            assert_eq!(material.secret(), "opaque-grant");
        }
        for issuer in [
            "https://evil.example",
            "http://platform.deepseek.com",
            "https://platform.deepseek.com/",
            "https://platform.deepseek.com.evil.example",
        ] {
            let input = serde_json::json!({"version":1,"token":"opaque-grant","issuer":issuer});
            assert!(parse_manual(&input.to_string()).is_err());
        }
        for input in [
            r#"{"version":2,"token":"opaque-grant","issuer":"https://platform.deepseek.com"}"#,
            r#"{"token":"opaque-grant"}"#,
            "x-dsh-auth-token: opaque-grant",
        ] {
            assert!(parse_manual(input).is_err());
        }
    }

    #[test]
    fn curl_is_bound_to_the_fixed_platform_endpoint() {
        let material = parse_manual("curl 'https://platform.deepseek.com/api/v0/users/get_user_summary' -H 'Authorization: Bearer web-token' -H 'Accept: application/json'").unwrap();
        assert_eq!(material.kind(), SessionKind::WebToken);
        assert_eq!(material.secret(), "web-token");
        for input in [
            "curl 'https://evil.example/api/v0/users/get_user_summary' -H 'Authorization: Bearer web-token'",
            "curl 'https://platform.deepseek.com.evil.example/api/v0/users/get_user_summary' -H 'Authorization: Bearer web-token'",
            "curl 'https://api.deepseek.com/user/balance' -H 'Authorization: Bearer web-token'",
            "curl 'https://platform.deepseek.com/api/v0/users/get_user_summary' -H 'Authorization: Bearer web-token' -b 'session=cookie'",
            "curl 'https://platform.deepseek.com/api/v0/users/get_user_summary' -H 'Authorization: Bearer web-token' -H 'Cookie: session=cookie'",
            "curl 'https://platform.deepseek.com/api/v0/users/get_user_summary' -H 'Authorization: Bearer one' -H 'Authorization: Bearer two'",
            "curl 'https://platform.deepseek.com/api/v0/users/get_user_summary' -H 'Authorization: Bearer web-token' https://evil.example",
        ] {
            assert!(parse_manual(input).is_err());
        }
    }

    #[test]
    fn unrelated_credentials_and_ambiguous_sources_are_not_sessions() {
        for input in [
            "",
            "sk-official-api-key",
            "Cookie: userToken=web-token",
            "userToken=web-token; other=value",
            r#"{"api_key":"sk-official-api-key"}"#,
            r#"{"userToken":"sk-official-api-key"}"#,
            r#"{"userToken":"one","user_token":"two"}"#,
            r#"{"userToken":"one","Cookie":"other=value"}"#,
            r#"{"userToken":"one","issuer":"https://evil.example"}"#,
            "userToken=first\r\nAuthorization: injected",
            "curl https://evil.example -H 'Authorization: Bearer sk-official-api-key'",
        ] {
            assert!(parse_manual(input).is_err());
        }
    }

    #[test]
    fn authentication_kind_selects_a_distinct_header_without_converting_tokens() {
        assert_eq!(SessionKind::WebToken.header_name(), "Authorization");
        assert_eq!(
            SessionKind::WebToken.authorization_value("web-token"),
            "Bearer web-token"
        );
        assert_eq!(SessionKind::HarnessGrant.header_name(), "x-dsh-auth-token");
        assert_eq!(
            SessionKind::HarnessGrant.authorization_value("grant-token"),
            "grant-token"
        );
        let stored = serde_json::to_string(&SessionKind::HarnessGrant).unwrap();
        assert_eq!(
            serde_json::from_str::<SessionKind>(&stored).unwrap(),
            SessionKind::HarnessGrant
        );
    }

    #[test]
    fn platform_wallets_remain_separate_with_exact_decimal_spelling() {
        let balance = decode(200, br#"{"code":0,"data":{"biz_code":0,"biz_data":{"normal_wallets":[{"currency":"CNY","balance":"0E-16"},{"currency":"USD","balance":"5.0000000000000000"}],"bonus_wallets":[{"currency":"CNY","balance":".5"}],"token":"must-not-survive"}}}"#).unwrap();
        assert_eq!(
            balance.normal_wallets,
            vec![
                Wallet {
                    currency: "CNY".into(),
                    balance: "0E-16".into()
                },
                Wallet {
                    currency: "USD".into(),
                    balance: "5.0000000000000000".into()
                }
            ]
        );
        assert_eq!(
            balance.bonus_wallets,
            vec![Wallet {
                currency: "CNY".into(),
                balance: ".5".into()
            }]
        );
        let persisted = serde_json::to_string(&balance).unwrap();
        assert!(!persisted.contains("must-not-survive"));
        assert!(!persisted.contains("total_balance"));
        assert_eq!(
            serde_json::from_str::<PlatformBalance>(&persisted).unwrap(),
            balance
        );
    }

    #[test]
    fn the_first_party_big_decimal_grammar_is_not_float_parsing() {
        for value in ["0E-16", "-0", "1.", ".1", "-.25", "1e+99999", "-1E-99999"] {
            let body = serde_json::json!({"code":0,"data":{"biz_code":0,"biz_data":{"normal_wallets":[{"currency":"CNY","balance":value}],"bonus_wallets":[]}}});
            assert_eq!(
                decode(200, body.to_string().as_bytes())
                    .unwrap()
                    .normal_wallets[0]
                    .balance,
                value
            );
        }
        for value in [
            "NaN",
            "Infinity",
            "-Infinity",
            "+1",
            "1e",
            "0x10",
            " 1",
            "1 ",
            ".",
            "1_000",
        ] {
            let body = serde_json::json!({"code":0,"data":{"biz_code":0,"biz_data":{"normal_wallets":[{"currency":"CNY","balance":value}],"bonus_wallets":[]}}});
            assert_eq!(
                decode(200, body.to_string().as_bytes()),
                Err(UsageError::Response)
            );
        }
    }

    #[test]
    fn invalid_authorization_is_never_reported_as_zero_balance() {
        assert_eq!(decode(401, b"not json"), Err(UsageError::Authentication));
        for status in [200, 400, 403, 500] {
            assert_eq!(
                decode(status, br#"{"code":40003}"#),
                Err(UsageError::Authentication)
            );
        }
        assert_eq!(decode(200, br#"{"code":40002}"#), Err(UsageError::Service));
        assert_eq!(decode(429, b""), Err(UsageError::RateLimited));
        assert_eq!(decode(503, b""), Err(UsageError::Service));
    }

    #[test]
    fn missing_or_malformed_wallets_are_not_successful_empty_readings() {
        for body in [
            r#"{"code":0,"data":{"biz_code":0,"biz_data":{"normal_wallets":[]}}}"#,
            r#"{"code":0,"data":{"biz_code":0,"biz_data":{"normal_wallets":null,"bonus_wallets":[]}}}"#,
            r#"{"code":0,"data":{"biz_code":0,"biz_data":{"normal_wallets":[{"currency":"EUR","balance":"1"}],"bonus_wallets":[]}}}"#,
            r#"{"code":0,"data":{"biz_code":0,"biz_data":{"normal_wallets":[{"currency":"CNY","balance":1}],"bonus_wallets":[]}}}"#,
            r#"{"code":0,"data":{"biz_code":1,"biz_data":{"normal_wallets":[],"bonus_wallets":[]}}}"#,
            r#"{"code":0,"data":{"biz_data":{"normal_wallets":[],"bonus_wallets":[]}}}"#,
            r#"{"is_available":true,"balance_infos":[]}"#,
        ] {
            assert!(decode(200, body.as_bytes()).is_err());
        }
        assert_eq!(decode(200, br#"{"code":0,"data":{"biz_code":0,"biz_data":{"normal_wallets":[],"bonus_wallets":[]}}}"#).unwrap(), PlatformBalance { normal_wallets: vec![], bonus_wallets: vec![] });
    }
}
