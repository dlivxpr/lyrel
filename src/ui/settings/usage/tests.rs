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
            "first-secret",
            decode(200, REPLY).unwrap(),
            100,
        )
        .unwrap();
    let second = accounts
        .save_verified(
            None,
            "same",
            "second-secret",
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
        .save_verified(None, "a", "old", decode(200, REPLY).unwrap(), 100)
        .unwrap();
    let query = accounts.begin_refresh(id).unwrap();
    assert!(accounts.begin_refresh(id).is_none());
    let replacement = accounts.begin_replace(id).unwrap();
    vault.fail.store(true, Ordering::SeqCst);
    assert_eq!(
        accounts.save_verified(
            Some(replacement),
            "a",
            "new",
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
            "new",
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
        .save_verified(None, "a", "key", decode(200, REPLY).unwrap(), 100)
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
        .save_verified(None, "a", "old", decode(200, REPLY).unwrap(), 100)
        .unwrap();
    let replacement = accounts.begin_replace(id).unwrap();
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    assert_eq!(
        accounts.save_verified(
            Some(replacement),
            "a",
            "new",
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
