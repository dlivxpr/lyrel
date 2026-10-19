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
mod browser;
mod platform;
mod providers;
use accounts::*;
use providers::Provider;
use uuid::Uuid;

const ENDPOINT: &str = "https://api.deepseek.com/user/balance";
#[derive(Clone, Copy)]
enum HttpSource {
    Official,
    Platform(platform::SessionKind),
    Provider(Provider, u8),
}
struct UsageRequest<'a> {
    source: HttpSource,
    secret: &'a str,
}
impl UsageRequest<'_> {
    fn endpoint(&self) -> &'static str {
        match self.source {
            HttpSource::Official => ENDPOINT,
            HttpSource::Platform(_) => platform::ENDPOINT,
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
                _ => ENDPOINT,
            },
        }
    }
    fn header_name(&self) -> &'static str {
        match self.source {
            HttpSource::Official => "Authorization",
            HttpSource::Platform(kind) => kind.header_name(),
            HttpSource::Provider(Provider::StepFun, _) => "Oasis-Token",
            HttpSource::Provider(..) => "Cookie",
        }
    }
    fn authorization_value(&self) -> String {
        match self.source {
            HttpSource::Official => format!("Bearer {}", self.secret),
            HttpSource::Platform(kind) => kind.authorization_value(self.secret),
            HttpSource::Provider(Provider::StepFun, _) => {
                self.session_field("Oasis-Token").to_owned()
            }
            HttpSource::Provider(..) => self.secret.to_owned(),
        }
    }
    fn session_field(&self, name: &str) -> &str {
        self.secret
            .split("; ")
            .filter_map(|part| part.split_once('='))
            .find_map(|(key, value)| (key == name).then_some(value))
            .unwrap_or("")
    }
}
/// The varying external HTTP boundary; source selection, parsing and transitions remain real.
trait BalanceHttp: Send + Sync {
    fn get(
        &self,
        request: &UsageRequest<'_>,
        proxy: Option<&str>,
    ) -> Result<(u16, Vec<u8>), UsageError>;
}
struct DeepSeekHttp;
impl BalanceHttp for DeepSeekHttp {
    fn get(
        &self,
        request: &UsageRequest<'_>,
        proxy: Option<&str>,
    ) -> Result<(u16, Vec<u8>), UsageError> {
        Self::send(request, proxy, request.endpoint())
    }
}
impl DeepSeekHttp {
    fn send(
        request: &UsageRequest<'_>,
        proxy: Option<&str>,
        endpoint: &str,
    ) -> Result<(u16, Vec<u8>), UsageError> {
        let mut builder = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(30)))
            .timeout_connect(Some(Duration::from_secs(10)))
            .http_status_as_error(false)
            .max_redirects(0)
            .user_agent(concat!("lyrel/", env!("CARGO_PKG_VERSION")));
        if let Some(proxy) = lyrel_core::daemon::install::proxy::resolve(endpoint, proxy) {
            builder = builder.proxy(Some(proxy));
        }
        let agent: ureq::Agent = builder.build().into();
        let mut response =
            if matches!(request.source, HttpSource::Provider(Provider::StepFun, _)) {
                agent
                    .post(endpoint)
                    .header("Content-Type", "application/json")
                    .header("Connect-Protocol-Version", "1")
                    .header("oasis-appid", "10300")
                    .header("oasis-platform", "web")
                    .header("Origin", "https://platform.stepfun.com")
                    .header("Referer", "https://platform.stepfun.com/")
                    .header("Accept", "application/json")
                    .header(request.header_name(), request.authorization_value())
                    .header("Oasis-Webid", request.session_field("Oasis-Webid"))
                    .header(
                        "Cookie",
                        format!(
                            "Oasis-Token={}",
                            url::form_urlencoded::byte_serialize(
                                request.session_field("Oasis-Token").as_bytes()
                            )
                            .collect::<String>()
                        ),
                    )
                    .send("{}")
            } else {
                let mut get = agent
                    .get(endpoint)
                    .header("Accept", "application/json")
                    .header(request.header_name(), request.authorization_value());
                if matches!(request.source, HttpSource::Provider(Provider::MiMo, _)) {
                    get = get
                        .header("Origin", "https://platform.xiaomimimo.com")
                        .header("Referer", "https://platform.xiaomimimo.com/console/balance");
                }
                get.call()
            }
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
fn query(
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
    let (status, body) = http.get(&UsageRequest { source, secret }, proxy)?;
    match source {
        HttpSource::Official => accounts::decode(status, &body),
        HttpSource::Platform(_) => platform::decode(status, &body).map(Balance::Platform),
        HttpSource::Provider(..) => unreachable!("provider query returns above"),
    }
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
    browser: Arc<browser::BrowserReader>,
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
enum Material {
    ApiKey(zeroize::Zeroizing<String>),
    Session(platform::SessionMaterial),
    Browser(browser::BrowserSelection, platform::SessionMaterial),
    ProviderSession(Provider, zeroize::Zeroizing<String>),
    Cookies(
        Provider,
        browser::BrowserSelection,
        zeroize::Zeroizing<String>,
    ),
}
impl Material {
    fn secret(&self) -> &str {
        match self {
            Self::ApiKey(v) => v,
            Self::Session(v) => v.secret(),
            Self::Browser(_, v) => v.secret(),
            Self::ProviderSession(_, v) | Self::Cookies(_, _, v) => v,
        }
    }
    fn http_source(&self) -> HttpSource {
        match self {
            Self::ApiKey(_) => HttpSource::Official,
            Self::Session(v) => HttpSource::Platform(v.kind()),
            Self::Browser(..) => HttpSource::Platform(platform::SessionKind::WebToken),
            Self::ProviderSession(provider, _) | Self::Cookies(provider, _, _) => {
                HttpSource::Provider(*provider, 0)
            }
        }
    }
    fn verified(&self) -> VerifiedSource<'_> {
        match self {
            Self::ApiKey(v) => VerifiedSource::ApiKey(v),
            Self::Session(v) => VerifiedSource::Session {
                kind: v.kind(),
                secret: v.secret(),
            },
            Self::Browser(selection, _) => VerifiedSource::Browser(selection.clone()),
            Self::ProviderSession(provider, secret) => VerifiedSource::ProviderSession {
                provider: *provider,
                secret,
            },
            Self::Cookies(provider, selection, _) => VerifiedSource::BrowserCookies {
                provider: *provider,
                selection: selection.clone(),
            },
        }
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Source {
    ApiKey,
    Session,
    Browser,
}
impl SelectItem for Source {
    type Value = Self;
    fn title(&self) -> gpui_kit::SharedString {
        t(match self {
            Self::ApiKey => L10nKey::UsageApiSource,
            Self::Session => L10nKey::UsageSessionSource,
            Self::Browser => L10nKey::UsageBrowserSource,
        })
        .into()
    }
    fn value(&self) -> &Self {
        self
    }
}
impl SelectItem for Provider {
    type Value = Self;
    fn title(&self) -> gpui_kit::SharedString {
        self.label().into()
    }
    fn value(&self) -> &Self {
        self
    }
}
#[derive(Clone)]
struct BrowserChoice(browser::Browser);
impl SelectItem for BrowserChoice {
    type Value = browser::Browser;
    fn title(&self) -> gpui_kit::SharedString {
        self.0.label().into()
    }
    fn value(&self) -> &Self::Value {
        &self.0
    }
}
#[derive(Clone)]
struct ProfileChoice {
    profile: browser::Profile,
    id: String,
}
impl SelectItem for ProfileChoice {
    type Value = String;
    fn title(&self) -> gpui_kit::SharedString {
        self.profile.label().to_string().into()
    }
    fn value(&self) -> &String {
        &self.id
    }
}
pub(crate) struct UsagePage {
    data: Entity<UsageData>,
    name: Entity<InputState>,
    key: Entity<InputState>,
    edit: Option<Edit>,
    provider: Entity<SelectState<Vec<Provider>>>,
    cadence: Entity<SelectState<Vec<Cadence>>>,
    source: Entity<SelectState<Vec<Source>>>,
    browsers: Entity<SelectState<Vec<BrowserChoice>>>,
    profiles: Entity<SelectState<Vec<ProfileChoice>>>,
    profile_items: Vec<ProfileChoice>,
    discovering: bool,
    discovery_version: u64,
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
                browser: Arc::new(browser::BrowserReader::system()),
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
        let provider = cx.new(|cx| {
            SelectState::new(
                vec![Provider::DeepSeek, Provider::StepFun, Provider::MiMo],
                None,
                window,
                cx,
            )
        });
        provider.update(cx, |s, cx| {
            s.set_selected_value(&Provider::DeepSeek, window, cx)
        });
        let cadence = cx.new(|cx| {
            SelectState::new(
                vec![Cadence(0), Cadence(60), Cadence(300)],
                None,
                window,
                cx,
            )
        });
        let source = cx.new(|cx| {
            SelectState::new(
                vec![Source::ApiKey, Source::Session, Source::Browser],
                None,
                window,
                cx,
            )
        });
        source.update(cx, |s, cx| {
            s.set_selected_value(&Source::ApiKey, window, cx)
        });
        let browsers = cx.new(|cx| {
            SelectState::new(
                browser::Browser::all()
                    .iter()
                    .copied()
                    .map(BrowserChoice)
                    .collect::<Vec<_>>(),
                None,
                window,
                cx,
            )
        });
        let profiles = cx.new(|cx| SelectState::new(Vec::<ProfileChoice>::new(), None, window, cx));
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
        subs.push(
            cx.subscribe_in(&provider, window, |page, _, event, window, cx| {
                if let SelectEvent::Confirm(Some(provider)) = event {
                    page.form_version.fetch_add(1, Ordering::SeqCst);
                    page.key.update(cx, |s, cx| s.set_value("", window, cx));
                    page.error = None;
                    page.source.update(cx, |s, cx| {
                        s.set_items(
                            if *provider == Provider::DeepSeek {
                                vec![Source::ApiKey, Source::Session, Source::Browser]
                            } else {
                                vec![Source::Session, Source::Browser]
                            },
                            window,
                            cx,
                        );
                        s.set_selected_value(
                            &if *provider == Provider::DeepSeek {
                                Source::ApiKey
                            } else {
                                Source::Session
                            },
                            window,
                            cx,
                        );
                    });
                    cx.notify();
                }
            }),
        );
        subs.push(cx.subscribe_in(
            &source,
            window,
            |page, _, _: &SelectEvent<Vec<Source>>, window, cx| {
                page.form_version.fetch_add(1, Ordering::SeqCst);
                page.error = None;
                page.key.update(cx, |s, cx| s.set_value("", window, cx));
                cx.notify();
            },
        ));
        subs.push(
            cx.subscribe_in(&browsers, window, |page, _, event, window, cx| {
                if let SelectEvent::Confirm(Some(browser)) = event {
                    page.discover_profiles(*browser, window, cx);
                }
            }),
        );
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
            provider,
            cadence,
            source,
            browsers,
            profiles,
            profile_items: Vec::new(),
            discovering: false,
            discovery_version: 0,
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
    fn discover_profiles(
        &mut self,
        browser: browser::Browser,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.discovery_version += 1;
        let version = self.discovery_version;
        self.profile_items.clear();
        self.profiles
            .update(cx, |s, cx| s.set_items(Vec::new(), window, cx));
        self.discovering = true;
        self.error = None;
        let reader = self.data.read(cx).shared.browser.clone();
        cx.spawn_in(window, async move |page, cx| {
            let result = worker(move || reader.profiles(browser)).await;
            let _ = page.update_in(cx, |page, window, cx| {
                if page.discovery_version != version {
                    return;
                }
                page.discovering = false;
                match result {
                    Ok(profiles) => {
                        page.profile_items = profiles
                            .into_iter()
                            .map(|profile| ProfileChoice {
                                id: profile.id().to_owned(),
                                profile,
                            })
                            .collect();
                        page.profiles.update(cx, |s, cx| {
                            s.set_items(page.profile_items.clone(), window, cx)
                        });
                        if page.profile_items.is_empty() {
                            page.error = Some(UsageError::BrowserMissing);
                        }
                    }
                    Err(error) => page.error = Some(browser_error(error)),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
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
        let provider = match edit {
            Edit::Replace(id) => self
                .data
                .read(cx)
                .stored
                .accounts
                .iter()
                .find(|a| a.id == id)
                .map(|a| a.provider)
                .unwrap_or_default(),
            _ => Provider::DeepSeek,
        };
        self.provider
            .update(cx, |s, cx| s.set_selected_value(&provider, window, cx));
        self.source.update(cx, |s, cx| {
            s.set_items(
                if provider == Provider::DeepSeek {
                    vec![Source::ApiKey, Source::Session, Source::Browser]
                } else {
                    vec![Source::Session, Source::Browser]
                },
                window,
                cx,
            )
        });
        let source = match edit {
            Edit::Replace(id) => self
                .data
                .read(cx)
                .stored
                .accounts
                .iter()
                .find(|a| a.id == id)
                .map(|a| match a.auth {
                    Authentication::ManualApiKey { .. } => Source::ApiKey,
                    Authentication::ManualSession { .. }
                    | Authentication::ProviderSession { .. } => Source::Session,
                    Authentication::BrowserLocalStorage { .. }
                    | Authentication::BrowserCookies { .. } => Source::Browser,
                })
                .unwrap_or(Source::ApiKey),
            _ => Source::ApiKey,
        };
        self.source
            .update(cx, |s, cx| s.set_selected_value(&source, window, cx));
        self.discovery_version += 1;
        self.discovering = false;
        self.profile_items.clear();
        self.profiles
            .update(cx, |s, cx| s.set_items(Vec::new(), window, cx));
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
        self.discovery_version += 1;
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
                let secret = zeroize::Zeroizing::new(self.key.read(cx).value().trim().to_string());
                let provider = self
                    .provider
                    .read(cx)
                    .selected_value()
                    .copied()
                    .unwrap_or_default();
                let source = self
                    .source
                    .read(cx)
                    .selected_value()
                    .copied()
                    .unwrap_or(Source::ApiKey);
                let profile = self
                    .profiles
                    .read(cx)
                    .selected_value()
                    .and_then(|id| self.profile_items.iter().find(|p| &p.id == id))
                    .map(|p| p.profile.clone());
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
                        let verified = (|| {
                            let material = match (provider, source) {
                                (Provider::DeepSeek, Source::ApiKey) => Material::ApiKey(secret),
                                (Provider::DeepSeek, Source::Session) => {
                                    Material::Session(platform::parse_manual(&secret)?)
                                }
                                (Provider::DeepSeek, Source::Browser) => {
                                    let profile = profile.ok_or(UsageError::BrowserMissing)?;
                                    let (selection, material) =
                                        shared.browser.import(&profile).map_err(browser_error)?;
                                    Material::Browser(
                                        selection,
                                        platform::parse_browser(material.secret())?,
                                    )
                                }
                                (_, Source::Session) => Material::ProviderSession(
                                    provider,
                                    providers::parse_manual(provider, &secret)?,
                                ),
                                (_, Source::Browser) => {
                                    let profile = profile.ok_or(UsageError::BrowserMissing)?;
                                    let (selection, material) = shared
                                        .browser
                                        .import_cookies(&profile, cookie_provider(provider)?, true)
                                        .map_err(browser_error)?;
                                    Material::Cookies(
                                        provider,
                                        selection,
                                        providers::parse_session(provider, material.secret())?,
                                    )
                                }
                                (_, Source::ApiKey) => return Err(UsageError::Authentication),
                            };
                            let balance = query(
                                &*shared.http,
                                material.http_source(),
                                material.secret(),
                                proxy.as_deref(),
                            )?;
                            Ok((material, balance))
                        })();
                        let mut guard = shared.accounts.lock().unwrap();
                        let accounts = guard.as_mut().map_err(|e| *e)?;
                        if gate.load(Ordering::SeqCst) != version {
                            if let Some(q) = replacement {
                                accounts.fail_replace(q, UsageError::Stale);
                            }
                            return Err(UsageError::Stale);
                        }
                        match verified {
                            Ok((material, balance)) => accounts
                                .save_verified(
                                    replacement,
                                    &name,
                                    material.verified(),
                                    balance,
                                    now(),
                                )
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
                    accounts.begin_refresh(id).map(|ticket| {
                        let auth = accounts.account(id).unwrap().auth.clone();
                        let secret = if matches!(
                            auth,
                            Authentication::BrowserLocalStorage { .. }
                                | Authentication::BrowserCookies { .. }
                        ) {
                            Ok(None)
                        } else {
                            accounts
                                .secret(id)
                                .map(|s| Some(zeroize::Zeroizing::new(s)))
                        };
                        (ticket, auth, secret)
                    })
                };
                let Some((ticket, auth, secret)) = start else {
                    return;
                };
                let result = secret.and_then(|secret| match auth {
                    Authentication::ManualApiKey { .. } => query(
                        &*shared.http,
                        HttpSource::Official,
                        secret.as_deref().map(|s| s.as_str()).unwrap_or(""),
                        proxy.as_deref(),
                    ),
                    Authentication::ManualSession { kind, .. } => query(
                        &*shared.http,
                        HttpSource::Platform(kind),
                        secret.as_deref().map(|s| s.as_str()).unwrap_or(""),
                        proxy.as_deref(),
                    ),
                    Authentication::BrowserLocalStorage { selection } => {
                        let material = shared.browser.resolve(&selection).map_err(browser_error)?;
                        let material = platform::parse_browser(material.secret())?;
                        query(
                            &*shared.http,
                            HttpSource::Platform(platform::SessionKind::WebToken),
                            material.secret(),
                            proxy.as_deref(),
                        )
                    }
                    Authentication::ProviderSession { provider, .. } => {
                        let secret = secret.as_deref().map(|s| s.as_str()).unwrap_or("");
                        let secret = providers::parse_manual(provider, secret)?;
                        query(
                            &*shared.http,
                            HttpSource::Provider(provider, 0),
                            &secret,
                            proxy.as_deref(),
                        )
                    }
                    Authentication::BrowserCookies {
                        selection,
                        provider,
                    } => {
                        let material = shared
                            .browser
                            .resolve_cookies(&selection, cookie_provider(provider)?)
                            .map_err(browser_error)?;
                        let secret = providers::parse_session(provider, material.secret())?;
                        query(
                            &*shared.http,
                            HttpSource::Provider(provider, 0),
                            &secret,
                            proxy.as_deref(),
                        )
                    }
                });
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
fn browser_error(error: browser::BrowserError) -> UsageError {
    match error {
        browser::BrowserError::Locked => UsageError::BrowserLocked,
        browser::BrowserError::Denied => UsageError::BrowserDenied,
        browser::BrowserError::NoSession | browser::BrowserError::ProfileGone => {
            UsageError::BrowserMissing
        }
        browser::BrowserError::Unsupported => UsageError::BrowserUnsupported,
        browser::BrowserError::SourceChanged => UsageError::BrowserChanged,
        browser::BrowserError::Unreadable => UsageError::BrowserUnreadable,
        browser::BrowserError::Protected => UsageError::BrowserProtected,
    }
}
fn cookie_provider(provider: Provider) -> Result<browser::CookieProvider, UsageError> {
    match provider {
        Provider::StepFun => Ok(browser::CookieProvider::StepFun),
        Provider::MiMo => Ok(browser::CookieProvider::MiMo),
        Provider::DeepSeek => Err(UsageError::Authentication),
    }
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
        UsageError::BrowserLocked => L10nKey::UsageBrowserLocked,
        UsageError::BrowserDenied => L10nKey::UsageBrowserDenied,
        UsageError::BrowserMissing => L10nKey::UsageBrowserMissing,
        UsageError::BrowserUnsupported => L10nKey::UsageBrowserUnsupported,
        UsageError::BrowserChanged => L10nKey::UsageBrowserChanged,
        UsageError::BrowserUnreadable => L10nKey::UsageBrowserUnreadable,
        UsageError::BrowserProtected => L10nKey::UsageBrowserProtected,
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
                let provider = self
                    .provider
                    .read(cx)
                    .selected_value()
                    .copied()
                    .unwrap_or_default();
                form = form.child(t(L10nKey::UsageProvider)).child(
                    Select::new(&self.provider)
                        .id("usage-provider")
                        .small()
                        .w_full()
                        .disabled(self.submitting || matches!(edit, Edit::Replace(_)))
                        .accessibility_label(t(L10nKey::UsageProvider)),
                );
                let source = self
                    .source
                    .read(cx)
                    .selected_value()
                    .copied()
                    .unwrap_or(Source::ApiKey);
                form = form.child(t(L10nKey::UsageSource)).child(
                    Select::new(&self.source)
                        .id("usage-source")
                        .small()
                        .w_full()
                        .disabled(self.submitting)
                        .accessibility_label(t(L10nKey::UsageSource)),
                );
                if source == Source::ApiKey {
                    form = form
                        .child(t(L10nKey::UsageApiKey))
                        .child(Input::new(&self.key).id("usage-key").small().w_full());
                } else {
                    form = form.child(div().text_color(muted).child(t(
                        if provider == Provider::DeepSeek {
                            L10nKey::UsagePrivateDisclosure
                        } else {
                            provider.help()
                        },
                    )));
                    if source == Source::Session {
                        form = form
                            .child(t(if provider == Provider::DeepSeek {
                                L10nKey::UsageSessionInput
                            } else {
                                L10nKey::UsageCookieInput
                            }))
                            .child(Input::new(&self.key).id("usage-key").small().w_full())
                            .when(provider == Provider::DeepSeek, |form| {
                                form.child(
                                    div().text_color(muted).child(t(L10nKey::UsageSessionHelp)),
                                )
                            });
                    } else {
                        form = form
                            .child(t(L10nKey::UsageBrowser))
                            .child(
                                Select::new(&self.browsers)
                                    .id("usage-browser")
                                    .small()
                                    .w_full()
                                    .disabled(self.submitting || self.discovering)
                                    .placeholder(t(L10nKey::UsageChooseBrowser))
                                    .accessibility_label(t(L10nKey::UsageBrowser)),
                            )
                            .child(t(L10nKey::UsageProfile))
                            .child(
                                Select::new(&self.profiles)
                                    .id("usage-profile")
                                    .small()
                                    .w_full()
                                    .disabled(self.submitting || self.discovering)
                                    .placeholder(t(L10nKey::UsageChooseProfile))
                                    .accessibility_label(t(L10nKey::UsageProfile)),
                            )
                            .child(div().text_color(muted).child(t(
                                if provider == Provider::DeepSeek {
                                    L10nKey::UsageBrowserHelp
                                } else {
                                    L10nKey::UsageCookieBrowserHelp
                                },
                            )));
                        if self.discovering {
                            form = form.child(t(L10nKey::UsageLoading));
                        }
                    }
                }
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
                                    && if self.source.read(cx).selected_value()
                                        == Some(&Source::Browser)
                                    {
                                        self.discovering
                                            || self.profiles.read(cx).selected_value().is_none()
                                    } else {
                                        self.key.read(cx).value().trim().is_empty()
                                    }),
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
                .child(
                    div()
                        .text_color(muted)
                        .child(format!("{} · {id}", account.provider.label())),
                );
            row = row.child(div().text_color(muted).child(match &account.auth {
                Authentication::ManualApiKey { .. } => t(L10nKey::UsageApiSource).to_owned(),
                Authentication::ManualSession { kind, .. } => format!(
                    "{} · {}",
                    t(L10nKey::UsageSessionSource),
                    match kind {
                        platform::SessionKind::WebToken => "userToken · Bearer",
                        platform::SessionKind::HarnessGrant => "Harness grant · x-dsh-auth-token",
                    }
                ),
                Authentication::BrowserLocalStorage { selection } => format!(
                    "{} · {} · {}",
                    t(L10nKey::UsageBrowserSource),
                    selection.browser().label(),
                    selection.label()
                ),
                Authentication::ProviderSession { .. } => t(L10nKey::UsageSessionSource).to_owned(),
                Authentication::BrowserCookies { selection, .. } => format!(
                    "{} · {} · {}",
                    t(L10nKey::UsageCookieSource),
                    selection.browser().label(),
                    selection.label()
                ),
            }));
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
                match &snapshot.balance {
                    Balance::Official(balance) => {
                        for line in &balance.balance_infos {
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
                        row = row.child(t(if balance.is_available {
                            L10nKey::UsageAvailable
                        } else {
                            L10nKey::UsageUnavailable
                        }));
                    }
                    Balance::Platform(balance) => {
                        for (label, wallets) in [
                            (L10nKey::UsageNormalWallet, &balance.normal_wallets),
                            (L10nKey::UsageBonusWallet, &balance.bonus_wallets),
                        ] {
                            for wallet in wallets {
                                row = row.child(format!(
                                    "{} {} {}",
                                    t(label),
                                    wallet.balance,
                                    wallet.currency
                                ));
                            }
                        }
                    }
                    Balance::Provider(report) => {
                        row = row
                            .child(format!(
                                "{} {}",
                                t(L10nKey::UsageIdentity),
                                report
                                    .identity
                                    .as_deref()
                                    .unwrap_or(t(L10nKey::UsageUnknownValue))
                            ))
                            .child(format!(
                                "{} {}",
                                t(L10nKey::UsagePlan),
                                report
                                    .plan
                                    .as_deref()
                                    .unwrap_or(t(L10nKey::UsageUnknownValue))
                            ));
                        if report.expired == Some(true) {
                            row = row.child(t(L10nKey::UsageUnavailable));
                        }
                        if let Some(expires) = &report.plan_expires {
                            row = row.child(format!(
                                "{} · {} {expires}",
                                t(L10nKey::UsagePlan),
                                t(L10nKey::UsageExpires)
                            ));
                        }
                        if report.pools.is_empty() {
                            row = row.child(t(L10nKey::UsageUnknown));
                        }
                        for pool in &report.pools {
                            let label = match pool.kind {
                                providers::PoolKind::FiveHour => L10nKey::UsageFiveHour,
                                providers::PoolKind::Weekly => L10nKey::UsageWeekly,
                                providers::PoolKind::Balance => L10nKey::UsageBalance,
                                providers::PoolKind::Cash => L10nKey::UsageCash,
                                providers::PoolKind::Gift => L10nKey::UsageGift,
                                providers::PoolKind::Frozen => L10nKey::UsageFrozen,
                                providers::PoolKind::Overdraft => L10nKey::UsageOverdraft,
                                providers::PoolKind::SubscriptionCredit => {
                                    L10nKey::UsageSubscriptionCredit
                                }
                                providers::PoolKind::CreditBucket => L10nKey::UsageCreditBucket,
                                providers::PoolKind::Monthly => L10nKey::UsageMonthly,
                            };
                            let unit = if pool.unit.is_empty() {
                                t(L10nKey::UsageUnknownValue)
                            } else {
                                &pool.unit
                            };
                            row = row.child(format!(
                                "{} · {} {} {}",
                                t(label),
                                t(L10nKey::UsageRemaining),
                                pool.remaining
                                    .as_deref()
                                    .unwrap_or(t(L10nKey::UsageUnknownValue)),
                                unit
                            ));
                            if let Some(name) = &pool.name {
                                row = row.child(div().text_color(muted).child(name.clone()));
                            }
                            if let Some(used) = &pool.used {
                                row = row.child(format!("{} {used} {unit}", t(L10nKey::UsageUsed)));
                            }
                            if let Some(limit) = &pool.limit {
                                row =
                                    row.child(format!("{} {limit} {unit}", t(L10nKey::UsageLimit)));
                            }
                            if matches!(
                                pool.kind,
                                providers::PoolKind::FiveHour
                                    | providers::PoolKind::Weekly
                                    | providers::PoolKind::Monthly
                                    | providers::PoolKind::SubscriptionCredit
                                    | providers::PoolKind::CreditBucket
                            ) {
                                row = row.child(format!(
                                    "{} {}",
                                    t(L10nKey::UsageReset),
                                    pool.reset
                                        .as_deref()
                                        .unwrap_or(t(L10nKey::UsageUnknownValue))
                                ));
                            }
                            if let Some(expires) = &pool.expires {
                                row = row.child(format!("{} {expires}", t(L10nKey::UsageExpires)));
                            }
                        }
                        if let Some(error) = report.enhancement {
                            row = row.child(div().text_color(danger).child(error_text(error)));
                        }
                    }
                }
                let time = chrono::DateTime::from_timestamp(snapshot.updated_at as i64, 0)
                    .map(|time| {
                        time.with_timezone(&chrono::Local)
                            .format("%Y-%m-%d %H:%M:%S %:z")
                            .to_string()
                    })
                    .unwrap_or_else(|| snapshot.updated_at.to_string());
                row = row.child(
                    div()
                        .text_color(muted)
                        .child(format!("{} {time}", t(L10nKey::UsageUpdated))),
                );
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
