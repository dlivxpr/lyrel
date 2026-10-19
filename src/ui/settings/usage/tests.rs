use super::*;
use crate::core::keychain::{CredentialError, CredentialResult, InMemoryCredentialStore};
use std::sync::atomic::{AtomicBool, Ordering};

const REPLY: &[u8] = br#"{"is_available":true,"balance_infos":[{"currency":"CNY","total_balance":"110.00","granted_balance":"10.00","topped_up_balance":"100.00"}]}"#;

#[test]
fn verified_accounts_keep_identity_and_secrets_separate() {
    let dir = tempfile::tempdir().unwrap();
    let vault = Arc::new(InMemoryCredentialStore::new());
    let mut accounts = Accounts::load(dir.path().join("usage.json"), vault.clone()).unwrap();
    let first = accounts
        .save_verified(
            None,
            "same",
            VerifiedSource::ApiKey("first-secret"),
            decode(200, REPLY).unwrap(),
            100,
        )
        .unwrap();
    let second = accounts
        .save_verified(
            None,
            "same",
            VerifiedSource::ApiKey("second-secret"),
            decode(200, REPLY).unwrap(),
            101,
        )
        .unwrap();
    accounts.rename(first, "personal").unwrap();
    accounts.set_enabled(second, false).unwrap();
    let saved = Accounts::load(dir.path().join("usage.json"), vault.clone()).unwrap();
    assert_eq!(saved.account(first).unwrap().name, "personal");
    assert!(!saved.account(second).unwrap().enabled);
    assert_eq!(
        saved
            .account(first)
            .unwrap()
            .snapshot
            .as_ref()
            .unwrap()
            .balance
            .official()
            .unwrap()
            .balance_infos[0]
            .total_balance,
        "110.00"
    );
    let text = std::fs::read_to_string(dir.path().join("usage.json")).unwrap();
    assert!(!text.contains("first-secret") && !text.contains("second-secret"));
    accounts.remove(first).unwrap();
    assert!(accounts.account(first).is_none());
    assert_eq!(accounts.secret(second).unwrap(), "second-secret");
    assert_eq!(vault.len(), 1);
}

#[derive(Default)]
struct FailingVault {
    inner: InMemoryCredentialStore,
    fail: AtomicBool,
}
impl CredentialStore for FailingVault {
    fn get(&self, s: &str, a: &str) -> CredentialResult<Option<String>> {
        self.inner.get(s, a)
    }
    fn set(&self, s: &str, a: &str, v: &str) -> CredentialResult<()> {
        if self.fail.load(Ordering::SeqCst) {
            return Err(CredentialError::Backend("locked".into()));
        }
        self.inner.set(s, a, v)
    }
    fn delete(&self, s: &str, a: &str) -> CredentialResult<()> {
        self.inner.delete(s, a)
    }
}

#[test]
fn replacement_failure_preserves_old_key_and_late_responses_cannot_cross_accounts() {
    let dir = tempfile::tempdir().unwrap();
    let vault = Arc::new(FailingVault::default());
    let mut accounts = Accounts::load(dir.path().join("usage.json"), vault.clone()).unwrap();
    let id = accounts
        .save_verified(
            None,
            "a",
            VerifiedSource::ApiKey("old"),
            decode(200, REPLY).unwrap(),
            100,
        )
        .unwrap();
    let query = accounts.begin_refresh(id).unwrap();
    assert!(accounts.begin_refresh(id).is_none());
    let replacement = accounts.begin_replace(id).unwrap();
    vault.fail.store(true, Ordering::SeqCst);
    assert_eq!(
        accounts.save_verified(
            Some(replacement),
            "a",
            VerifiedSource::ApiKey("new"),
            decode(200, REPLY).unwrap(),
            101
        ),
        Err(UsageError::Credentials)
    );
    assert_eq!(accounts.secret(id).unwrap(), "old");
    accounts.finish_refresh(query, Err(UsageError::Authentication), 102);
    assert_eq!(
        accounts
            .account(id)
            .unwrap()
            .snapshot
            .as_ref()
            .unwrap()
            .updated_at,
        100
    );
    vault.fail.store(false, Ordering::SeqCst);
    let replacement = accounts.begin_replace(id).unwrap();
    accounts
        .save_verified(
            Some(replacement),
            "a",
            VerifiedSource::ApiKey("new"),
            decode(200, REPLY).unwrap(),
            103,
        )
        .unwrap();
    accounts.finish_refresh(query, Err(UsageError::RateLimited), 104);
    assert_eq!(
        accounts
            .account(id)
            .unwrap()
            .snapshot
            .as_ref()
            .unwrap()
            .updated_at,
        103
    );
    assert_eq!(accounts.secret(id).unwrap(), "new");
    let late = accounts.begin_refresh(id).unwrap();
    accounts.remove(id).unwrap();
    accounts.finish_refresh(late, decode(200, REPLY), 105);
    assert!(accounts.account(id).is_none());
}

#[test]
fn failure_keeps_timestamp_and_unknown_is_never_zero() {
    let dir = tempfile::tempdir().unwrap();
    let mut accounts = Accounts::load(
        dir.path().join("usage.json"),
        Arc::new(InMemoryCredentialStore::new()),
    )
    .unwrap();
    let id = accounts
        .save_verified(
            None,
            "a",
            VerifiedSource::ApiKey("key"),
            decode(200, REPLY).unwrap(),
            100,
        )
        .unwrap();
    let query = accounts.begin_refresh(id).unwrap();
    accounts.finish_refresh(query, decode(429, b"{}"), 200);
    let account = accounts.account(id).unwrap();
    assert_eq!(account.error, Some(UsageError::RateLimited));
    assert_eq!(account.snapshot.as_ref().unwrap().updated_at, 100);
    assert_eq!(decode(401, b"{}"), Err(UsageError::Authentication));
    assert_eq!(decode(503, b"{}"), Err(UsageError::Service));
    assert_eq!(
        decode(200, br#"{"is_available":true,"balance_infos":[]}"#),
        Err(UsageError::Response)
    );
    assert_eq!(
        decode(
            200,
            br#"{"is_available":true,"balance_infos":[{"currency":"CNY"}]}"#
        ),
        Err(UsageError::Response)
    );
}

#[test]
fn failed_metadata_replacement_keeps_the_old_usable_credential() {
    let dir = tempfile::tempdir().unwrap();
    let vault = Arc::new(InMemoryCredentialStore::new());
    let path = dir.path().join("usage.json");
    let mut accounts = Accounts::load(path.clone(), vault.clone()).unwrap();
    let id = accounts
        .save_verified(
            None,
            "a",
            VerifiedSource::ApiKey("old"),
            decode(200, REPLY).unwrap(),
            100,
        )
        .unwrap();
    let replacement = accounts.begin_replace(id).unwrap();
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    assert_eq!(
        accounts.save_verified(
            Some(replacement),
            "a",
            VerifiedSource::ApiKey("new"),
            decode(200, REPLY).unwrap(),
            101
        ),
        Err(UsageError::Persistence)
    );
    assert_eq!(accounts.secret(id).unwrap(), "old");
    assert_eq!(vault.len(), 1);
    assert_eq!(
        accounts
            .account(id)
            .unwrap()
            .snapshot
            .as_ref()
            .unwrap()
            .updated_at,
        100
    );
}

#[test]
fn platform_sessions_store_only_validated_material_and_failed_replacement_keeps_old_source() {
    let dir = tempfile::tempdir().unwrap();
    let vault = Arc::new(FailingVault::default());
    let mut accounts = Accounts::load(dir.path().join("usage.json"), vault.clone()).unwrap();
    let platform = super::super::platform::decode(200, br#"{"code":0,"data":{"biz_code":0,"biz_data":{"normal_wallets":[{"currency":"CNY","balance":"5.0000000000000000"}],"bonus_wallets":[{"currency":"CNY","balance":"0E-16"}]}}}"#).unwrap();
    let id = accounts
        .save_verified(
            None,
            "Platform",
            VerifiedSource::Session {
                kind: super::super::platform::SessionKind::WebToken,
                secret: "web-session-secret",
            },
            Balance::Platform(platform),
            100,
        )
        .unwrap();
    assert_eq!(accounts.secret(id).unwrap(), "web-session-secret");
    assert_eq!(vault.inner.len(), 1);
    let saved = std::fs::read_to_string(dir.path().join("usage.json")).unwrap();
    assert!(!saved.contains("web-session-secret"));
    assert!(matches!(
        accounts.account(id).unwrap().auth,
        Authentication::ManualSession { .. }
    ));
    let replacement = accounts.begin_replace(id).unwrap();
    vault.fail.store(true, Ordering::SeqCst);
    assert_eq!(
        accounts.save_verified(
            Some(replacement),
            "Platform",
            VerifiedSource::ApiKey("new-key"),
            decode(200, REPLY).unwrap(),
            200
        ),
        Err(UsageError::Credentials)
    );
    assert_eq!(accounts.secret(id).unwrap(), "web-session-secret");
    assert!(matches!(
        accounts
            .account(id)
            .unwrap()
            .snapshot
            .as_ref()
            .unwrap()
            .balance,
        Balance::Platform(_)
    ));
}

#[test]
fn provider_identity_change_and_failed_vault_replacement_keep_original_account_snapshot() {
    use super::super::providers::{Provider, ProviderReport};
    let dir = tempfile::tempdir().unwrap();
    let vault = Arc::new(FailingVault::default());
    let mut accounts = Accounts::load(dir.path().join("usage.json"), vault.clone()).unwrap();
    let report = |identity: &str| {
        Balance::Provider(ProviderReport {
            provider: Provider::StepFun,
            identity: Some(identity.to_owned()),
            plan: Some("Plan Pro".to_owned()),
            expired: None,
            plan_expires: None,
            pools: Vec::new(),
            enhancement: None,
            overdrawn: None,
        })
    };
    let id = accounts
        .save_verified(
            None,
            "StepFun",
            VerifiedSource::ProviderSession {
                provider: Provider::StepFun,
                secret: "Oasis-Token=old; Oasis-Webid=web",
            },
            report("42"),
            100,
        )
        .unwrap();
    let refresh = accounts.begin_refresh(id).unwrap();
    accounts.finish_refresh(refresh, Ok(report("other-user")), 200);
    let account = accounts.account(id).unwrap();
    assert_eq!(account.error, Some(UsageError::BrowserChanged));
    assert_eq!(account.snapshot.as_ref().unwrap().updated_at, 100);
    let replacement = accounts.begin_replace(id).unwrap();
    vault.fail.store(true, Ordering::SeqCst);
    assert_eq!(
        accounts.save_verified(
            Some(replacement),
            "StepFun",
            VerifiedSource::ProviderSession {
                provider: Provider::StepFun,
                secret: "Oasis-Token=new; Oasis-Webid=web"
            },
            report("42"),
            300
        ),
        Err(UsageError::Credentials)
    );
    assert_eq!(
        accounts.secret(id).unwrap(),
        "Oasis-Token=old; Oasis-Webid=web"
    );
    assert_eq!(
        accounts
            .account(id)
            .unwrap()
            .snapshot
            .as_ref()
            .unwrap()
            .updated_at,
        100
    );
    assert_eq!(vault.inner.len(), 1);
}

#[test]
fn browser_enhancement_binding_replaces_only_manual_slot_and_late_material_cannot_restore_changed_account()
 {
    use super::super::{
        browser::BrowserSelection,
        providers::{Provider, ProviderReport},
    };
    let dir = tempfile::tempdir().unwrap();
    let vault = Arc::new(InMemoryCredentialStore::new());
    let mut accounts = Accounts::load(dir.path().join("usage.json"), vault.clone()).unwrap();
    let report = || ProviderReport {
        provider: Provider::CommandCode,
        identity: Some("command-user".into()),
        plan: Some("Pro".into()),
        pools: Vec::new(),
        enhancement: None,
        overdrawn: None,
        expired: None,
        plan_expires: None,
    };
    let selection: BrowserSelection = serde_json::from_value(serde_json::json!({
        "browser":"firefox", "profile_id":"work", "profile_label":"Work",
        "profile_fingerprint":"nonsecret-profile-reference", "token_fingerprint":"command-cookie:nonsecret-binding"
    })).unwrap();
    let id = accounts
        .save_verified(
            None,
            "Command",
            VerifiedSource::ApiKey("primary-key"),
            Balance::Provider(report()),
            100,
        )
        .unwrap();
    let ticket = accounts.begin_replace(id).unwrap();
    accounts
        .save_enhancement(ticket, "session=manual-secret", report(), 101)
        .unwrap();
    assert_eq!(vault.len(), 2);
    let ticket = accounts.begin_replace(id).unwrap();
    accounts
        .save_browser_enhancement(ticket, selection.clone(), report(), 102)
        .unwrap();
    assert_eq!(vault.len(), 1);
    assert_eq!(accounts.secret(id).unwrap(), "primary-key");
    assert!(accounts.enhancement_secret(id).unwrap().is_none());
    let late = accounts.begin_replace(id).unwrap();
    let newer = accounts.begin_replace(id).unwrap();
    accounts
        .save_verified(
            Some(newer),
            "Command",
            VerifiedSource::ApiKey("replacement-key"),
            Balance::Provider(report()),
            103,
        )
        .unwrap();
    assert_eq!(
        accounts.save_browser_enhancement(late, selection.clone(), report(), 104),
        Err(UsageError::Stale)
    );
    assert!(accounts.account(id).unwrap().enhancement_browser.is_none());
    assert_eq!(accounts.secret(id).unwrap(), "replacement-key");
    let late = accounts.begin_replace(id).unwrap();
    accounts.remove(id).unwrap();
    assert_eq!(
        accounts.save_browser_enhancement(late, selection, report(), 105),
        Err(UsageError::Stale)
    );
    assert!(
        Accounts::load(dir.path().join("usage.json"), vault.clone())
            .unwrap()
            .stored
            .accounts
            .is_empty()
    );
    assert!(vault.is_empty());
}
