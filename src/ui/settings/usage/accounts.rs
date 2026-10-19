//! Desktop-only usage accounts. No Host, remote credentials or agent configuration.
use crate::core::keychain::CredentialStore;
use serde::{Deserialize, Serialize};
use std::{path::PathBuf, sync::Arc};
use uuid::Uuid;

pub(super) const SERVICE: &str = "lyrel-usage";
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) enum UsageError {
    Authentication,
    RateLimited,
    Network,
    Credentials,
    Persistence,
    Response,
    Service,
    Stale,
    BrowserLocked,
    BrowserDenied,
    BrowserMissing,
    BrowserUnsupported,
    BrowserChanged,
    BrowserUnreadable,
    BrowserProtected,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct BalanceLine {
    pub currency: String,
    pub total_balance: String,
    pub granted_balance: String,
    pub topped_up_balance: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct OfficialBalance {
    pub is_available: bool,
    pub balance_infos: Vec<BalanceLine>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub(super) enum Balance {
    Official(OfficialBalance),
    Platform(super::platform::PlatformBalance),
    Provider(super::providers::ProviderReport),
}
impl Balance {
    #[cfg(test)]
    pub fn official(&self) -> Option<&OfficialBalance> {
        match self {
            Self::Official(value) => Some(value),
            _ => None,
        }
    }
}
pub(super) enum VerifiedSource<'a> {
    ApiKey(&'a str),
    Session {
        kind: super::platform::SessionKind,
        secret: &'a str,
    },
    Browser(super::browser::BrowserSelection),
    ProviderSession { provider: super::providers::Provider, secret: &'a str },
    BrowserCookies { selection: super::browser::BrowserSelection, provider: super::providers::Provider },
}
#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Snapshot {
    pub balance: Balance,
    pub updated_at: u64,
}
/// Authentication source belongs to the account, not its display name or Host.
#[derive(Clone, Serialize, Deserialize)]
pub(super) enum Authentication {
    ManualApiKey {
        slot: Uuid,
    },
    ManualSession {
        slot: Uuid,
        kind: super::platform::SessionKind,
    },
    BrowserLocalStorage {
        selection: super::browser::BrowserSelection,
    },
    ProviderSession { slot: Uuid, provider: super::providers::Provider },
    BrowserCookies { selection: super::browser::BrowserSelection, provider: super::providers::Provider },
}
impl Authentication {
    fn slot(&self) -> Option<Uuid> {
        match self {
            Self::ManualApiKey { slot } | Self::ManualSession { slot, .. } | Self::ProviderSession { slot, .. } => Some(*slot),
            Self::BrowserLocalStorage { .. } | Self::BrowserCookies { .. } => None,
        }
    }
}
#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Account {
    pub id: Uuid,
    pub name: String,
    pub auth: Authentication,
    #[serde(default)]
    pub provider: super::providers::Provider,
    pub enabled: bool,
    pub snapshot: Option<Snapshot>,
    pub error: Option<UsageError>,
    #[serde(default)]
    retired_slots: Vec<Uuid>,
    #[serde(default)]
    pub removing: bool,
    #[serde(skip)]
    revision: u64,
    #[serde(skip)]
    pub busy: bool,
}
#[derive(Clone, Default, Serialize, Deserialize)]
pub(super) struct StoredAccounts {
    pub accounts: Vec<Account>,
    pub interval_secs: u64,
}
#[derive(Clone, Copy)]
pub(super) struct Query {
    pub id: Uuid,
    revision: u64,
}

pub(super) struct Accounts {
    pub stored: StoredAccounts,
    path: PathBuf,
    vault: Arc<dyn CredentialStore>,
}
impl Accounts {
    pub fn load(path: PathBuf, vault: Arc<dyn CredentialStore>) -> Result<Self, UsageError> {
        let stored = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice::<StoredAccounts>(&bytes)
                .map_err(|_| UsageError::Persistence)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => StoredAccounts::default(),
            Err(_) => return Err(UsageError::Persistence),
        };
        // Refuse ambiguous account identities instead of querying the wrong key.
        let mut ids = std::collections::HashSet::new();
        let mut slots = std::collections::HashSet::new();
        for account in &stored.accounts {
            if !ids.insert(account.id)
                || account.auth.slot().is_some_and(|slot| !slots.insert(slot))
            {
                return Err(UsageError::Persistence);
            }
        }
        Ok(Self {
            stored,
            path,
            vault,
        })
    }
    pub fn account(&self, id: Uuid) -> Option<&Account> {
        self.stored.accounts.iter().find(|a| a.id == id)
    }
    fn account_mut(&mut self, id: Uuid) -> Option<&mut Account> {
        self.stored.accounts.iter_mut().find(|a| a.id == id)
    }
    fn persist(&self, stored: &StoredAccounts) -> Result<(), UsageError> {
        let bytes = serde_json::to_vec_pretty(stored).map_err(|_| UsageError::Persistence)?;
        lyrel_core::core::config::write_atomic_private(&self.path, &bytes)
            .map_err(|_| UsageError::Persistence)
    }
    pub fn secret(&self, id: Uuid) -> Result<String, UsageError> {
        let account = self.account(id).ok_or(UsageError::Stale)?;
        if account.removing {
            return Err(UsageError::Stale);
        }
        self.vault
            .get(
                SERVICE,
                &account
                    .auth
                    .slot()
                    .ok_or(UsageError::BrowserMissing)?
                    .to_string(),
            )
            .map_err(|_| UsageError::Credentials)?
            .ok_or(UsageError::Credentials)
    }
    pub fn begin_refresh(&mut self, id: Uuid) -> Option<Query> {
        let account = self.account_mut(id)?;
        if !account.enabled || account.busy || account.removing {
            return None;
        }
        account.busy = true;
        Some(Query {
            id,
            revision: account.revision,
        })
    }
    pub fn begin_replace(&mut self, id: Uuid) -> Option<Query> {
        let account = self.account_mut(id)?;
        if account.removing {
            return None;
        }
        account.revision += 1;
        account.busy = true;
        Some(Query {
            id,
            revision: account.revision,
        })
    }
    pub fn fail_replace(&mut self, query: Query, error: UsageError) {
        if let Some(a) = self
            .account_mut(query.id)
            .filter(|a| a.revision == query.revision)
        {
            a.busy = false;
            a.error = Some(error);
        }
    }
    pub fn finish_refresh(&mut self, query: Query, result: Result<Balance, UsageError>, now: u64) {
        let Some(account) = self
            .account_mut(query.id)
            .filter(|a| a.revision == query.revision && !a.removing)
        else {
            return;
        };
        account.busy = false;
        match result {
            Ok(balance) => {
                account.snapshot = Some(Snapshot {
                    balance,
                    updated_at: now,
                });
                account.error = None;
            }
            Err(error) => account.error = Some(error),
        }
        if self.persist(&self.stored).is_err() {
            self.account_mut(query.id).unwrap().error = Some(UsageError::Persistence);
        }
    }
    /// Validate remotely first; stage into a NEW slot. A failed write never overwrites the old key.
    pub fn save_verified(
        &mut self,
        replace: Option<Query>,
        name: &str,
        source: VerifiedSource<'_>,
        balance: Balance,
        now: u64,
    ) -> Result<Uuid, UsageError> {
        if let Some(query) = replace {
            if !self
                .account(query.id)
                .is_some_and(|a| a.revision == query.revision && !a.removing)
            {
                return Err(UsageError::Stale);
            }
        }
        let id = replace.map_or_else(Uuid::new_v4, |q| q.id);
        let (secret, kind) = match &source {
            VerifiedSource::ApiKey(secret) => (Some(*secret), None),
            VerifiedSource::Session { kind, secret } => (Some(*secret), Some(*kind)),
            VerifiedSource::Browser(_) => (None, None),
            VerifiedSource::ProviderSession { secret, .. } => (Some(*secret), None),
            VerifiedSource::BrowserCookies { .. } => (None, None),
        };
        let slot = secret.map(|_| Uuid::new_v4());
        if let (Some(secret), Some(slot)) = (secret, slot) {
            if secret.len() > 2560 || self.vault.set(SERVICE, &slot.to_string(), secret).is_err() {
                if let Some(query) = replace {
                    self.fail_replace(query, UsageError::Credentials);
                }
                return Err(UsageError::Credentials);
            }
        }
        let auth = match source {
            VerifiedSource::ApiKey(_) => Authentication::ManualApiKey {
                slot: slot.unwrap(),
            },
            VerifiedSource::Session { .. } => Authentication::ManualSession {
                slot: slot.unwrap(),
                kind: kind.unwrap(),
            },
            VerifiedSource::Browser(selection) => Authentication::BrowserLocalStorage { selection },
            VerifiedSource::ProviderSession { provider, .. } => Authentication::ProviderSession { slot: slot.unwrap(), provider },
            VerifiedSource::BrowserCookies { selection, provider } => Authentication::BrowserCookies { selection, provider },
        };
        let provider = match &balance { Balance::Provider(report) => report.provider, _ => super::providers::Provider::DeepSeek };
        let mut next = self.stored.clone();
        if let Some(account) = next.accounts.iter_mut().find(|a| a.id == id) {
            account.retired_slots.extend(account.auth.slot());
            account.auth = auth;
            account.provider = provider;
            account.snapshot = Some(Snapshot {
                balance,
                updated_at: now,
            });
            account.error = None;
            account.busy = false;
        } else {
            next.accounts.push(Account {
                id,
                name: name.to_owned(),
                auth,
                provider,
                enabled: true,
                snapshot: Some(Snapshot {
                    balance,
                    updated_at: now,
                }),
                error: None,
                retired_slots: Vec::new(),
                removing: false,
                revision: 0,
                busy: false,
            });
        }
        if let Err(error) = self.persist(&next) {
            if let Some(slot) = slot {
                let _ = self.vault.delete(SERVICE, &slot.to_string());
            }
            if let Some(query) = replace {
                self.fail_replace(query, error);
            }
            return Err(error);
        }
        self.stored = next;
        // Retain references until deletion succeeds; removal can safely retry its own slots.
        let retired = self.account(id).unwrap().retired_slots.clone();
        for old in retired {
            if self.vault.delete(SERVICE, &old.to_string()).is_ok() {
                self.account_mut(id)
                    .unwrap()
                    .retired_slots
                    .retain(|s| *s != old);
            } else {
                self.account_mut(id).unwrap().error = Some(UsageError::Credentials);
            }
        }
        if self.persist(&self.stored).is_err() {
            self.account_mut(id).unwrap().error = Some(UsageError::Persistence);
        }
        Ok(id)
    }
    pub fn rename(&mut self, id: Uuid, name: &str) -> Result<(), UsageError> {
        let mut next = self.stored.clone();
        next.accounts
            .iter_mut()
            .find(|a| a.id == id)
            .ok_or(UsageError::Stale)?
            .name = name.to_owned();
        self.persist(&next)?;
        self.stored = next;
        Ok(())
    }
    pub fn set_enabled(&mut self, id: Uuid, enabled: bool) -> Result<(), UsageError> {
        let mut next = self.stored.clone();
        let a = next
            .accounts
            .iter_mut()
            .find(|a| a.id == id)
            .ok_or(UsageError::Stale)?;
        a.enabled = enabled;
        a.revision += 1;
        a.busy = false;
        self.persist(&next)?;
        self.stored = next;
        Ok(())
    }
    pub fn set_interval(&mut self, secs: u64) -> Result<(), UsageError> {
        let mut next = self.stored.clone();
        next.interval_secs = secs;
        self.persist(&next)?;
        self.stored = next;
        Ok(())
    }
    pub fn remove(&mut self, id: Uuid) -> Result<(), UsageError> {
        let mut next = self.stored.clone();
        let a = next
            .accounts
            .iter_mut()
            .find(|a| a.id == id)
            .ok_or(UsageError::Stale)?;
        a.removing = true;
        a.enabled = false;
        a.revision += 1;
        a.busy = false;
        self.persist(&next)?;
        self.stored = next;
        let a = self.account(id).unwrap();
        let slots = a
            .auth
            .slot()
            .into_iter()
            .chain(a.retired_slots.iter().copied())
            .collect::<Vec<_>>();
        for slot in slots {
            if self.vault.delete(SERVICE, &slot.to_string()).is_err() {
                self.account_mut(id).unwrap().error = Some(UsageError::Credentials);
                return Err(UsageError::Credentials);
            }
        }
        let mut next = self.stored.clone();
        next.accounts.retain(|a| a.id != id);
        self.persist(&next)?;
        self.stored = next;
        Ok(())
    }
}

pub(super) fn decode(status: u16, body: &[u8]) -> Result<Balance, UsageError> {
    match status {
        401 | 403 => return Err(UsageError::Authentication),
        429 => return Err(UsageError::RateLimited),
        200 => {}
        _ => return Err(UsageError::Service),
    }
    let balance: OfficialBalance =
        serde_json::from_slice(body).map_err(|_| UsageError::Response)?;
    let decimal = |s: &str| {
        let s = s.strip_prefix('-').unwrap_or(s);
        let mut split = s.split('.');
        let whole = split.next().unwrap_or("");
        !whole.is_empty()
            && whole.bytes().all(|b| b.is_ascii_digit())
            && split
                .next()
                .is_none_or(|v| !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit()))
            && split.next().is_none()
    };
    if balance.balance_infos.is_empty()
        || balance.balance_infos.iter().any(|line| {
            !matches!(line.currency.as_str(), "CNY" | "USD")
                || !decimal(&line.total_balance)
                || !decimal(&line.granted_balance)
                || !decimal(&line.topped_up_balance)
        })
    {
        return Err(UsageError::Response);
    }
    Ok(Balance::Official(balance))
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
