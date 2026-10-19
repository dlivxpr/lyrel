use super::kit::BtnKind;
use crate::{
    core::{config::Config, keychain::OsCredentialStore},
    ui::{
        app::LyrelApp,
        i18n::{L10nKey, t},
    },
};
use gpui_kit::component::button::{Button, ButtonCustomVariant, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::select::{Select, SelectEvent, SelectItem, SelectState};
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Sizable as _, h_flex, v_flex};
use gpui_kit::{
    Context, Entity, Focusable as _, Global, Subscription, Task, Window, div, prelude::*,
};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};
mod accounts;
use accounts::*;
use uuid::Uuid;

const ENDPOINT: &str = "https://api.deepseek.com/user/balance";
/// The only varying external request boundary. Parsing and account transitions remain real.
trait BalanceHttp: Send + Sync {
    fn get(&self, key: &str, proxy: Option<&str>) -> Result<(u16, Vec<u8>), UsageError>;
}
struct DeepSeekHttp;
impl BalanceHttp for DeepSeekHttp {
    fn get(&self, key: &str, proxy: Option<&str>) -> Result<(u16, Vec<u8>), UsageError> {
        let mut builder = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(30)))
            .timeout_connect(Some(Duration::from_secs(10)))
            .http_status_as_error(false)
            .max_redirects(0)
            .user_agent(concat!("lyrel/", env!("CARGO_PKG_VERSION")));
        if let Some(proxy) = lyrel_core::daemon::install::proxy::resolve(ENDPOINT, proxy) {
            builder = builder.proxy(Some(proxy));
        }
        let agent: ureq::Agent = builder.build().into();
        let mut response = agent
            .get(ENDPOINT)
            .header("Accept", "application/json")
            .header("Authorization", format!("Bearer {key}"))
            .call()
            .map_err(|_| UsageError::Network)?;
        let status = response.status().as_u16();
        let body = response
            .body_mut()
            .with_config()
            .limit(64 * 1024)
            .read_to_vec()
            .map_err(|_| UsageError::Network)?;
        Ok((status, body))
    }
}
fn query(http: &dyn BalanceHttp, secret: &str, proxy: Option<&str>) -> Result<Balance, UsageError> {
    if secret.is_empty() || secret.contains(char::is_whitespace) {
        return Err(UsageError::Authentication);
    }
    let (status, body) = http.get(secret, proxy)?;
    accounts::decode(status, &body)
}
fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
async fn worker<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    let (tx, rx) = smol::channel::bounded(1);
    std::thread::spawn(move || {
        let _ = tx.send_blocking(f());
    });
    rx.recv().await.expect("usage worker completed")
}
struct Shared {
    accounts: Mutex<Result<Accounts, UsageError>>,
    http: Arc<dyn BalanceHttp>,
    presentation: AtomicU64,
}
struct UsageData {
    shared: Arc<Shared>,
    stored: StoredAccounts,
    error: Option<UsageError>,
    presentation: u64,
    loading: bool,
}
impl Global for UsageGlobal {}
struct UsageGlobal(Entity<UsageData>);

#[derive(Clone, Copy)]
enum Edit {
    Add,
    Rename(Uuid),
    Replace(Uuid),
    Remove(Uuid),
}
#[derive(Clone)]
struct Cadence(u64);
impl SelectItem for Cadence {
    type Value = u64;
    fn title(&self) -> gpui_kit::SharedString {
        t(match self.0 {
            60 => L10nKey::UsageMinute,
            300 => L10nKey::UsageFiveMinutes,
            _ => L10nKey::UsageManual,
        })
        .into()
    }
    fn value(&self) -> &u64 {
        &self.0
    }
}
pub(crate) struct UsagePage {
    data: Entity<UsageData>,
    name: Entity<InputState>,
    key: Entity<InputState>,
    edit: Option<Edit>,
    cadence: Entity<SelectState<Vec<Cadence>>>,
    return_focus: Option<gpui_kit::FocusHandle>,
    error: Option<UsageError>,
    submitting: bool,
    pending: std::collections::HashSet<Uuid>,
    visible: bool,
    visible_gate: Arc<AtomicBool>,
    form_version: Arc<AtomicU64>,
    poll: Option<Task<()>>,
    _subs: Vec<Subscription>,
}
impl UsagePage {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let data = if let Some(global) = cx.try_global::<UsageGlobal>() {
            global.0.clone()
        } else {
            let path = crate::core::config::config_path("usage-accounts.json");
            let shared = Arc::new(Shared {
                accounts: Mutex::new(Err(UsageError::Persistence)),
                http: Arc::new(DeepSeekHttp),
                presentation: AtomicU64::new(1),
            });
            let data = cx.new(|_| UsageData {
                shared: shared.clone(),
                stored: StoredAccounts::default(),
                error: None,
                presentation: 0,
                loading: true,
            });
            let entity = data.clone();
            cx.spawn(async move |_, cx| {
                let result = worker(move || {
                    let result = path
                        .ok_or(UsageError::Persistence)
                        .and_then(|p| Accounts::load(p, Arc::new(OsCredentialStore)));
                    let presentation = result.as_ref().map(|a| a.stored.clone()).map_err(|e| *e);
                    *shared.accounts.lock().unwrap() = result;
                    presentation
                })
                .await;
                let _ = entity.update(cx, |data, cx| {
                    data.loading = false;
                    match result {
                        Ok(stored) => data.stored = stored,
                        Err(error) => data.error = Some(error),
                    };
                    cx.notify();
                });
            })
            .detach();
            cx.set_global(UsageGlobal(data.clone()));
            data
        };
        let name = cx.new(|cx| InputState::new(window, cx));
        let key = cx.new(|cx| InputState::new(window, cx).masked(true));
        let cadence = cx.new(|cx| {
            SelectState::new(
                vec![Cadence(0), Cadence(60), Cadence(300)],
                None,
                window,
                cx,
            )
        });
        let secs = data.read(cx).stored.interval_secs;
        cadence.update(cx, |state, cx| state.set_selected_value(&secs, window, cx));
        let mut subs = vec![cx.observe_in(&data, window, |page, _, window, cx| {
            let secs = page.data.read(cx).stored.interval_secs;
            page.cadence
                .update(cx, |state, cx| state.set_selected_value(&secs, window, cx));
            page.restart_poll(cx);
            cx.notify();
        })];
        subs.push(cx.subscribe_in(&cadence, window, |page, _, event, _, cx| {
            if let SelectEvent::Confirm(Some(secs)) = event {
                let secs = *secs;
                page.work(cx, move |a| a.set_interval(secs));
            }
        }));
        for input in [&name, &key] {
            subs.push(
                cx.subscribe_in(input, window, |page, _, event, window, cx| {
                    if matches!(event, InputEvent::PressEnter { .. }) {
                        page.submit(window, cx);
                    }
                    cx.notify();
                }),
            );
        }
        Self {
            data,
            name,
            key,
            cadence,
            return_focus: None,
            edit: None,
            error: None,
            submitting: false,
            pending: Default::default(),
            visible: false,
            visible_gate: Arc::new(AtomicBool::new(false)),
            form_version: Arc::new(AtomicU64::new(0)),
            poll: None,
            _subs: subs,
        }
    }
    pub(crate) fn set_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        if self.visible == visible {
            return;
        }
        self.visible = visible;
        self.visible_gate.store(visible, Ordering::SeqCst);
        if !visible {
            self.form_version.fetch_add(1, Ordering::SeqCst);
            self.edit = None;
            self.error = None;
        }
        self.restart_poll(cx);
    }
    fn restart_poll(&mut self, cx: &mut Context<Self>) {
        self.poll = None;
        let secs = self.data.read(cx).stored.interval_secs;
        if !self.visible || secs == 0 {
            return;
        }
        self.poll = Some(cx.spawn(async move |page, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_secs(secs))
                    .await;
                if page
                    .update(cx, |page, cx| {
                        if !page.visible {
                            return false;
                        }
                        page.refresh_all(cx);
                        true
                    })
                    .unwrap_or(false)
                    == false
                {
                    break;
                }
            }
        }));
    }
    fn open_edit(&mut self, edit: Edit, window: &mut Window, cx: &mut Context<Self>) {
        self.form_version.fetch_add(1, Ordering::SeqCst);
        self.return_focus = window.focused(cx);
        let name = match edit {
            Edit::Add => String::new(),
            Edit::Rename(id) | Edit::Replace(id) | Edit::Remove(id) => self
                .data
                .read(cx)
                .stored
                .accounts
                .iter()
                .find(|a| a.id == id)
                .map(|a| a.name.clone())
                .unwrap_or_default(),
        };
        self.name
            .update(cx, |input, cx| input.set_value(name, window, cx));
        self.key
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.name.read(cx).focus_handle(cx).focus(window, cx);
        self.edit = Some(edit);
        self.error = None;
        cx.notify();
    }
    fn cancel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.form_version.fetch_add(1, Ordering::SeqCst);
        self.edit = None;
        self.key
            .update(cx, |input, cx| input.set_value("", window, cx));
        if let Some(focus) = self.return_focus.take() {
            focus.focus(window, cx);
        }
        cx.notify();
    }
    fn work(
        &mut self,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut Accounts) -> Result<(), UsageError> + Send + 'static,
    ) {
        let data = self.data.clone();
        let shared = data.read(cx).shared.clone();
        let was_submitting = self.submitting;
        let version = self.form_version.load(Ordering::SeqCst);
        cx.spawn(async move |page, cx| {
            let result = worker(move || {
                let mut guard = shared.accounts.lock().unwrap();
                guard.as_mut().map_err(|e| *e).and_then(f)
            })
            .await;
            publish(&data, cx).await;
            let _ = page.update(cx, |page, cx| {
                if version == page.form_version.load(Ordering::SeqCst) {
                    page.error = result.err();
                    if was_submitting && result.is_ok() {
                        page.edit = None;
                    }
                }
                if was_submitting {
                    page.submitting = false;
                }
                cx.notify();
            });
        })
        .detach();
    }
    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.submitting {
            return;
        }
        let Some(edit) = self.edit else {
            return;
        };
        let name = self.name.read(cx).value().trim().to_string();
        if name.is_empty() {
            return;
        }
        self.submitting = true;
        self.error = None;
        match edit {
            Edit::Rename(id) => self.work(cx, move |a| a.rename(id, &name)),
            Edit::Remove(id) => self.work(cx, move |a| a.remove(id)),
            Edit::Add | Edit::Replace(_) => {
                let secret = self.key.read(cx).value().trim().to_string();
                self.key
                    .update(cx, |input, cx| input.set_value("", window, cx));
                let data = self.data.clone();
                let shared = data.read(cx).shared.clone();
                let proxy = cx.global::<Config>().http_proxy.clone();
                let version = self.form_version.load(Ordering::SeqCst);
                let gate = self.form_version.clone();
                cx.spawn(async move |page, cx| {
                    let result = worker(move || {
                        let replacement = {
                            let mut guard = shared.accounts.lock().unwrap();
                            let accounts = guard.as_mut().map_err(|e| *e)?;
                            if gate.load(Ordering::SeqCst) != version {
                                return Err(UsageError::Stale);
                            }
                            match edit {
                                Edit::Replace(id) => {
                                    Some(accounts.begin_replace(id).ok_or(UsageError::Stale)?)
                                }
                                _ => None,
                            }
                        };
                        let balance = query(&*shared.http, &secret, proxy.as_deref());
                        let mut guard = shared.accounts.lock().unwrap();
                        let accounts = guard.as_mut().map_err(|e| *e)?;
                        if gate.load(Ordering::SeqCst) != version {
                            if let Some(q) = replacement {
                                accounts.fail_replace(q, UsageError::Stale);
                            }
                            return Err(UsageError::Stale);
                        }
                        match balance {
                            Ok(balance) => accounts
                                .save_verified(replacement, &name, &secret, balance, now())
                                .map(|_| ()),
                            Err(error) => {
                                if let Some(q) = replacement {
                                    accounts.fail_replace(q, error);
                                }
                                Err(error)
                            }
                        }
                    })
                    .await;
                    publish(&data, cx).await;
                    let _ = page.update(cx, |page, cx| {
                        page.submitting = false;
                        if version == page.form_version.load(Ordering::SeqCst) {
                            page.error = result.err();
                            if result.is_ok() {
                                page.edit = None;
                            }
                        }
                        cx.notify();
                    });
                })
                .detach();
            }
        }
        cx.notify();
    }
    fn refresh_all(&mut self, cx: &mut Context<Self>) {
        let ids = self
            .data
            .read(cx)
            .stored
            .accounts
            .iter()
            .filter(|a| a.enabled && !a.removing)
            .map(|a| a.id)
            .collect::<Vec<_>>();
        for id in ids {
            self.refresh(id, cx);
        }
    }
    fn refresh(&mut self, id: Uuid, cx: &mut Context<Self>) {
        if !self.visible || !self.pending.insert(id) {
            return;
        }
        let data = self.data.clone();
        let shared = data.read(cx).shared.clone();
        let proxy = cx.global::<Config>().http_proxy.clone();
        let gate = self.visible_gate.clone();
        cx.spawn(async move |page, cx| {
            worker(move || {
                let start = {
                    let mut guard = shared.accounts.lock().unwrap();
                    if !gate.load(Ordering::SeqCst) {
                        return;
                    }
                    let Ok(accounts) = guard.as_mut() else {
                        return;
                    };
                    accounts
                        .begin_refresh(id)
                        .map(|ticket| (ticket, accounts.secret(id)))
                };
                let Some((ticket, secret)) = start else {
                    return;
                };
                let result =
                    secret.and_then(|secret| query(&*shared.http, &secret, proxy.as_deref()));
                if let Ok(accounts) = shared.accounts.lock().unwrap().as_mut() {
                    accounts.finish_refresh(ticket, result, now());
                }
            })
            .await;
            publish(&data, cx).await;
            let _ = page.update(cx, |page, cx| {
                page.pending.remove(&id);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
}
async fn publish(data: &Entity<UsageData>, cx: &mut gpui_kit::AsyncApp) {
    let shared = data.read_with(cx, |data, _| data.shared.clone());
    let (version, stored) = worker(move || {
        let guard = shared.accounts.lock().unwrap();
        (
            shared.presentation.fetch_add(1, Ordering::SeqCst),
            guard.as_ref().ok().map(|a| a.stored.clone()),
        )
    })
    .await;
    let _ = data.update(cx, |data, cx| {
        if version >= data.presentation {
            data.presentation = version;
            if let Some(stored) = stored {
                data.stored = stored;
            }
            cx.notify();
        }
    });
}
fn error_text(error: UsageError) -> &'static str {
    t(match error {
        UsageError::Authentication => L10nKey::UsageAuthError,
        UsageError::RateLimited => L10nKey::UsageRateError,
        UsageError::Network => L10nKey::UsageNetworkError,
        UsageError::Credentials => L10nKey::UsageCredentialError,
        UsageError::Persistence => L10nKey::UsageSaveError,
        UsageError::Response => L10nKey::UsageResponseError,
        UsageError::Service => L10nKey::UsageServiceError,
        UsageError::Stale => L10nKey::UsageStaleError,
    })
}
fn button(
    id: impl Into<gpui_kit::ElementId>,
    label: impl Into<gpui_kit::SharedString>,
    kind: BtnKind,
) -> Button {
    Button::new(id)
        .label(label)
        .small()
        .when(kind == BtnKind::Primary, |b| b.primary())
}
impl Render for UsagePage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let stored = self.data.read(cx).stored.clone();
        let danger = cx.theme().danger;
        let muted = cx.theme().muted_foreground;
        let mut content = v_flex()
            .id("usage-page")
            .w_full()
            .min_w_0()
            .gap_4()
            .text_sm()
            .on_key_down(
                cx.listener(|page, event: &gpui_kit::KeyDownEvent, window, cx| {
                    if event.keystroke.key == "escape" && page.edit.is_some() {
                        page.cancel(window, cx);
                        cx.stop_propagation();
                    }
                }),
            )
            .child(div().text_color(muted).child(t(L10nKey::UsageScope)))
            .child(div().text_color(muted).child(t(L10nKey::UsageOfficial)))
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_2()
                    .child(
                        button("usage-add", t(L10nKey::UsageAdd), BtnKind::Secondary)
                            .disabled(
                                self.submitting
                                    || self.data.read(cx).loading
                                    || self.data.read(cx).error.is_some(),
                            )
                            .on_click(cx.listener(|p, _, w, cx| p.open_edit(Edit::Add, w, cx))),
                    )
                    .child(
                        button(
                            "usage-refresh-all",
                            t(L10nKey::UsageRefreshAll),
                            BtnKind::Secondary,
                        )
                        .on_click(cx.listener(|p, _, _, cx| p.refresh_all(cx))),
                    ),
            );
        content = content.child(
            v_flex().gap_2().child(t(L10nKey::UsageCadence)).child(
                Select::new(&self.cadence)
                    .id("usage-cadence")
                    .disabled(self.submitting)
                    .small()
                    .w_full()
                    .accessibility_label(t(L10nKey::UsageCadence)),
            ),
        );
        if self.data.read(cx).loading {
            content = content.child(t(L10nKey::UsageLoading));
        }
        if let Some(error) = self.error.or(self.data.read(cx).error) {
            content = content.child(div().text_color(danger).child(error_text(error)));
        }
        if let Some(edit) = self.edit {
            let mut form = v_flex().id("usage-form").gap_2();
            let title = match edit {
                Edit::Add => L10nKey::UsageAdd,
                Edit::Rename(_) => L10nKey::UsageRename,
                Edit::Replace(_) => L10nKey::UsageReplace,
                Edit::Remove(_) => L10nKey::UsageRemove,
            };
            form = form.child(t(title));
            if matches!(edit, Edit::Remove(_)) {
                form = form
                    .child(t(L10nKey::UsageRemoveWarning))
                    .child(self.name.read(cx).value());
            } else {
                form = form
                    .child(t(L10nKey::UsageName))
                    .child(Input::new(&self.name).id("usage-name").small().w_full());
            }
            if matches!(edit, Edit::Add | Edit::Replace(_)) {
                form = form
                    .child("DeepSeek API key")
                    .child(Input::new(&self.key).id("usage-key").small().w_full());
            }
            form = form.child(
                h_flex()
                    .gap_2()
                    .child(
                        button("usage-cancel", t(L10nKey::UsageCancel), BtnKind::Secondary)
                            .on_click(cx.listener(|p, _, w, cx| p.cancel(w, cx))),
                    )
                    .child(
                        button(
                            "usage-submit",
                            t(if self.submitting {
                                L10nKey::UsageLoading
                            } else if matches!(edit, Edit::Add | Edit::Replace(_)) {
                                L10nKey::UsageVerify
                            } else {
                                L10nKey::UsageConfirm
                            }),
                            BtnKind::Primary,
                        )
                        .custom(
                            ButtonCustomVariant::new(cx)
                                .color(cx.theme().foreground)
                                .foreground(cx.theme().background)
                                .hover(cx.theme().foreground.opacity(0.88))
                                .active(cx.theme().foreground.opacity(0.8)),
                        )
                        .disabled(
                            self.submitting
                                || self.name.read(cx).value().trim().is_empty()
                                || (matches!(edit, Edit::Add | Edit::Replace(_))
                                    && self.key.read(cx).value().trim().is_empty()),
                        )
                        .on_click(cx.listener(|p, _, w, cx| p.submit(w, cx))),
                    ),
            );
            content = content.child(form);
        }
        if stored.accounts.is_empty() && !self.data.read(cx).loading {
            content = content.child(div().text_color(muted).child(t(L10nKey::UsageEmpty)));
        }
        for account in stored.accounts {
            let id = account.id;
            let mut row = v_flex()
                .id(format!("usage-account-{id}"))
                .gap_2()
                .py_3()
                .border_t_1()
                .border_color(cx.theme().border)
                .child(account.name)
                .child(div().text_color(muted).child(format!("DeepSeek · {id}")));
            row = row.child(t(if account.removing {
                L10nKey::UsageRemoving
            } else if self.pending.contains(&id) || account.busy {
                L10nKey::UsageLoading
            } else if !account.enabled {
                L10nKey::UsageDisabled
            } else {
                L10nKey::UsageEnabled
            }));
            if let Some(snapshot) = account.snapshot {
                for line in snapshot.balance.balance_infos {
                    row = row
                        .child(format!(
                            "{} {} {}",
                            t(L10nKey::UsageBalance),
                            line.total_balance,
                            line.currency
                        ))
                        .child(format!(
                            "{} {} {} · {} {} {}",
                            t(L10nKey::UsageGranted),
                            line.granted_balance,
                            line.currency,
                            t(L10nKey::UsageToppedUp),
                            line.topped_up_balance,
                            line.currency
                        ));
                }
                let time = chrono::DateTime::from_timestamp(snapshot.updated_at as i64, 0)
                    .map(|time| {
                        time.with_timezone(&chrono::Local)
                            .format("%Y-%m-%d %H:%M:%S %:z")
                            .to_string()
                    })
                    .unwrap_or_else(|| snapshot.updated_at.to_string());
                row = row
                    .child(
                        div()
                            .text_color(muted)
                            .child(format!("{} {time}", t(L10nKey::UsageUpdated))),
                    )
                    .child(t(if snapshot.balance.is_available {
                        L10nKey::UsageAvailable
                    } else {
                        L10nKey::UsageUnavailable
                    }));
                if account.error.is_some() {
                    row = row.child(t(L10nKey::UsageOldSnapshot));
                }
            } else {
                row = row.child(t(L10nKey::UsageUnknown));
            }
            if let Some(error) = account.error {
                row = row.child(div().text_color(danger).child(error_text(error)));
            }
            let enabled = account.enabled;
            row = row.child(
                h_flex()
                    .flex_wrap()
                    .gap_2()
                    .child(
                        button(
                            format!("usage-refresh-{id}"),
                            t(L10nKey::UsageRefresh),
                            BtnKind::Secondary,
                        )
                        .disabled(!enabled || account.removing || self.pending.contains(&id))
                        .on_click(cx.listener(move |p, _, _, cx| p.refresh(id, cx))),
                    )
                    .child(
                        button(
                            format!("usage-toggle-{id}"),
                            t(if enabled {
                                L10nKey::UsageDisable
                            } else {
                                L10nKey::UsageEnable
                            }),
                            BtnKind::Secondary,
                        )
                        .disabled(account.removing || self.submitting)
                        .on_click(cx.listener(move |p, _, _, cx| {
                            p.work(cx, move |a| a.set_enabled(id, !enabled))
                        })),
                    )
                    .child(
                        button(
                            format!("usage-rename-{id}"),
                            t(L10nKey::UsageRename),
                            BtnKind::Secondary,
                        )
                        .disabled(account.removing || self.submitting)
                        .on_click(
                            cx.listener(move |p, _, w, cx| p.open_edit(Edit::Rename(id), w, cx)),
                        ),
                    )
                    .child(
                        button(
                            format!("usage-replace-{id}"),
                            t(L10nKey::UsageReplace),
                            BtnKind::Secondary,
                        )
                        .disabled(account.removing || self.submitting)
                        .on_click(
                            cx.listener(move |p, _, w, cx| p.open_edit(Edit::Replace(id), w, cx)),
                        ),
                    )
                    .child(
                        button(
                            format!("usage-remove-{id}"),
                            t(L10nKey::UsageRemove),
                            BtnKind::Secondary,
                        )
                        .disabled(self.submitting)
                        .on_click(
                            cx.listener(move |p, _, w, cx| p.open_edit(Edit::Remove(id), w, cx)),
                        ),
                    ),
            );
            content = content.child(row);
        }
        content
    }
}
impl LyrelApp {
    pub(super) fn render_settings_usage(&self, _cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        self.active_settings()
            .unwrap()
            .usage
            .clone()
            .into_any_element()
    }
    pub(super) fn render_usage_search_row(&self, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        self.settings_row(
            t(L10nKey::SettingsNavUsage),
            t(L10nKey::UsageScope),
            button(
                "usage-search-open",
                t(L10nKey::UsageOpen),
                BtnKind::Secondary,
            )
            .on_click(cx.listener(|this, _, w, cx| {
                this.navigate_settings(super::SettingsSection::Usage, None, w, cx)
            }))
            .into_any_element(),
            cx,
        )
        .into_any_element()
    }
}

#[cfg(test)]
#[path = "usage/workflow_tests.rs"]
mod workflow_tests;

impl Drop for UsagePage {
    fn drop(&mut self) {
        self.visible_gate.store(false, Ordering::SeqCst);
        self.form_version.fetch_add(1, Ordering::SeqCst);
    }
}
