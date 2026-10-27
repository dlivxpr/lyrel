use super::kit::BtnKind;
use crate::{
    core::{config::Config, keychain::OsCredentialStore},
    ui::{
        i18n::{L10nKey, t},
        settings_window::SettingsWindow,
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
mod presentation;
mod providers;
use accounts::*;
use providers::Provider;
use uuid::Uuid;

const ENDPOINT: &str = "https://api.deepseek.com/user/balance";
#[derive(Clone, Copy)]
enum HttpSource {
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
struct UsageRequest<'a> {
    source: HttpSource,
    secret: &'a str,
    org: Option<&'a str>,
}
impl UsageRequest<'_> {
    fn endpoint(&self) -> String {
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
    fn header_name(&self) -> &'static str {
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
    fn authorization_value(&self) -> String {
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
        Self::send(request, proxy, &request.endpoint())
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
        let authorization = zeroize::Zeroizing::new(request.authorization_value());
        let mut response = if matches!(request.source, HttpSource::KimiWeb) {
            agent
                .post(endpoint)
                .header("Accept", "application/json")
                .header("Authorization", authorization.as_str())
                .header("connect-protocol-version", "1")
                .header("Content-Type", "application/json")
                .send(r#"{"mute_notice":false}"#)
        } else if matches!(request.source, HttpSource::Provider(Provider::StepFun, _)) {
            agent
                .post(endpoint)
                .header("Content-Type", "application/json")
                .header("Connect-Protocol-Version", "1")
                .header("oasis-appid", "10300")
                .header("oasis-platform", "web")
                .header("Origin", "https://platform.stepfun.com")
                .header("Referer", "https://platform.stepfun.com/")
                .header("Accept", "application/json")
                .header("Oasis-Token", authorization.as_str())
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
        } else if matches!(request.source, HttpSource::Provider(Provider::MiMo, _)) {
            agent
                .get(endpoint)
                .header("Accept", "application/json")
                .header("Cookie", authorization.as_str())
                .header("Origin", "https://platform.xiaomimimo.com")
                .header("Referer", "https://platform.xiaomimimo.com/console/balance")
                .call()
        } else {
            agent
                .get(endpoint)
                .header("Accept", "application/json")
                .header(request.header_name(), authorization.as_str())
                .call()
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
fn query_provider(
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
    Enhance(Uuid),
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
    ProviderBrowser(browser::BrowserSelection, browser::SessionMaterial),
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
            Self::ProviderSession(_, v) => v,
            Self::ProviderBrowser(_, v) => v.secret(),
            Self::Cookies(_, _, v) => v,
        }
    }
    fn http_source(&self) -> HttpSource {
        match self {
            Self::ApiKey(_) => HttpSource::Official,
            Self::Session(v) => HttpSource::Platform(v.kind()),
            Self::Browser(..) => HttpSource::Platform(platform::SessionKind::WebToken),
            Self::ProviderSession(provider, _) => {
                if *provider == Provider::Kimi {
                    HttpSource::KimiWeb
                } else {
                    HttpSource::Provider(*provider, 0)
                }
            }
            Self::ProviderBrowser(..) => HttpSource::KimiWeb,
            Self::Cookies(provider, ..) => HttpSource::Provider(*provider, 0),
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
            Self::ProviderBrowser(selection, _) => VerifiedSource::Browser(selection.clone()),
            Self::Cookies(provider, selection, _) => VerifiedSource::BrowserCookies {
                provider: *provider,
                selection: selection.clone(),
            },
        }
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
impl Provider {
    fn sources(self) -> Vec<Source> {
        match self {
            Self::CommandCode => vec![Source::ApiKey],
            Self::StepFun | Self::MiMo => vec![Source::Session, Source::Browser],
            _ => vec![Source::ApiKey, Source::Session, Source::Browser],
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
#[derive(Clone)]
struct BrowserChoice(browser::Browser);
fn browser_choices(provider: Provider, enhancement: bool) -> Vec<BrowserChoice> {
    #[cfg(not(target_os = "macos"))]
    let _ = (provider, enhancement);
    browser::Browser::all()
        .iter()
        .copied()
        .filter(|browser| {
            #[cfg(target_os = "macos")]
            if *browser == browser::Browser::Safari {
                return enhancement || provider == Provider::MiMo;
            }
            let _ = browser;
            true
        })
        .map(BrowserChoice)
        .collect()
}
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
    cookie_name: Entity<InputState>,
    edit: Option<Edit>,
    cadence: Entity<SelectState<Vec<Cadence>>>,
    source: Entity<SelectState<Vec<Source>>>,
    browsers: Entity<SelectState<Vec<BrowserChoice>>>,
    profiles: Entity<SelectState<Vec<ProfileChoice>>>,
    provider: Entity<SelectState<Vec<Provider>>>,
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
    scroll: gpui_kit::ScrollHandle,
    form_scroll: gpui_kit::ScrollHandle,
    form_focus: gpui_kit::FocusHandle,
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
        let cookie_name = cx.new(|cx| InputState::new(window, cx));
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
        let provider = cx.new(|cx| {
            SelectState::new(
                vec![
                    Provider::DeepSeek,
                    Provider::Kimi,
                    Provider::CommandCode,
                    Provider::StepFun,
                    Provider::MiMo,
                ],
                None,
                window,
                cx,
            )
        });
        provider.update(cx, |s, cx| {
            s.set_selected_value(&Provider::DeepSeek, window, cx)
        });
        let browsers = cx.new(|cx| {
            SelectState::new(browser_choices(Provider::DeepSeek, false), None, window, cx)
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
        subs.push(
            cx.subscribe_in(&cadence, window, |page, _, event, window, cx| {
                if let SelectEvent::Confirm(Some(secs)) = event {
                    let secs = *secs;
                    page.work(window, cx, move |a| a.set_interval(secs));
                }
            }),
        );
        subs.push(
            cx.subscribe_in(&provider, window, |page, _, event, window, cx| {
                if let SelectEvent::Confirm(Some(provider)) = event {
                    let items = provider.sources();
                    page.source.update(cx, |s, cx| {
                        s.set_items(items, window, cx);
                        s.set_selected_value(
                            &if matches!(provider, Provider::StepFun | Provider::MiMo) {
                                Source::Session
                            } else {
                                Source::ApiKey
                            },
                            window,
                            cx,
                        );
                    });
                    page.browsers.update(cx, |s, cx| {
                        s.set_items(browser_choices(*provider, false), window, cx)
                    });
                    page.form_version.fetch_add(1, Ordering::SeqCst);
                    page.key.update(cx, |s, cx| s.set_value("", window, cx));
                    page.error = None;
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
        for input in [&name, &key, &cookie_name] {
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
            cookie_name,
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
            scroll: gpui_kit::ScrollHandle::new(),
            form_scroll: gpui_kit::ScrollHandle::new(),
            form_focus: cx.focus_handle(),
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
            Edit::Rename(id) | Edit::Replace(id) | Edit::Remove(id) | Edit::Enhance(id) => self
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
            Edit::Replace(id) | Edit::Enhance(id) => self
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
                if matches!(edit, Edit::Enhance(_)) {
                    vec![Source::Session, Source::Browser]
                } else {
                    provider.sources()
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
            Edit::Enhance(_) => Source::Session,
            _ => Source::ApiKey,
        };
        self.source
            .update(cx, |s, cx| s.set_selected_value(&source, window, cx));
        self.discovery_version += 1;
        self.discovering = false;
        self.profile_items.clear();
        self.browsers.update(cx, |s, cx| {
            s.set_items(
                browser_choices(provider, matches!(edit, Edit::Enhance(_))),
                window,
                cx,
            );
            s.set_selected_index(None, window, cx);
        });
        self.profiles
            .update(cx, |s, cx| s.set_items(Vec::new(), window, cx));
        self.name
            .update(cx, |input, cx| input.set_value(name, window, cx));
        self.key
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.cookie_name
            .update(cx, |input, cx| input.set_value("", window, cx));
        if matches!(edit, Edit::Enhance(_)) {
            self.source.read(cx).focus_handle(cx).focus(window, cx);
        } else if matches!(edit, Edit::Remove(_)) {
            self.form_focus.focus(window, cx);
        } else {
            self.name.read(cx).focus_handle(cx).focus(window, cx);
        }
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
        window: &mut Window,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut Accounts) -> Result<(), UsageError> + Send + 'static,
    ) {
        let data = self.data.clone();
        let shared = data.read(cx).shared.clone();
        let was_submitting = self.submitting;
        let version = self.form_version.load(Ordering::SeqCst);
        cx.spawn_in(window, async move |page, cx| {
            let result = worker(move || {
                let mut guard = shared.accounts.lock().unwrap();
                guard.as_mut().map_err(|e| *e).and_then(f)
            })
            .await;
            publish(&data, cx).await;
            let _ = page.update_in(cx, |page, window, cx| {
                if version == page.form_version.load(Ordering::SeqCst) {
                    page.error = result.err();
                    if was_submitting && result.is_ok() {
                        page.cancel(window, cx);
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
    fn submit_enhancement(&mut self, id: Uuid, window: &mut Window, cx: &mut Context<Self>) {
        let input = zeroize::Zeroizing::new(self.key.read(cx).value().trim().to_owned());
        self.key.update(cx, |s, cx| s.set_value("", window, cx));
        let from_browser = self.source.read(cx).selected_value() == Some(&Source::Browser);
        let cookie_name = self.cookie_name.read(cx).value().trim().to_owned();
        let profile = self.profiles.read(cx).selected_value().and_then(|id| {
            self.profile_items
                .iter()
                .find(|choice| &choice.id == id)
                .map(|choice| choice.profile.clone())
        });
        let data = self.data.clone();
        let shared = data.read(cx).shared.clone();
        let proxy = cx.global::<Config>().http_proxy.clone();
        let version = self.form_version.load(Ordering::SeqCst);
        let gate = self.form_version.clone();
        cx.spawn_in(window, async move |page, cx| {
            let result = worker(move || {
                let (ticket, key) = {
                    let mut guard = shared.accounts.lock().unwrap();
                    let accounts = guard.as_mut().map_err(|e| *e)?;
                    if gate.load(Ordering::SeqCst) != version {
                        return Err(UsageError::Stale);
                    }
                    let ticket = accounts.begin_replace(id).ok_or(UsageError::Stale)?;
                    (ticket, accounts.secret(id).map(zeroize::Zeroizing::new))
                };
                let verified = (|| {
                    let (selection, cookie) = if from_browser {
                        if gate.load(Ordering::SeqCst) != version {
                            return Err(UsageError::Stale);
                        }
                        let profile = profile.ok_or(UsageError::BrowserMissing)?;
                        let (selection, material) = shared
                            .browser
                            .import_cookies(
                                &profile,
                                browser::CookieProvider::CommandCode(&cookie_name),
                                &browser::ImportPermission::for_form(gate.clone(), version),
                            )
                            .map_err(browser_error)?;
                        (
                            Some(selection),
                            providers::command_code::session(material.secret())?,
                        )
                    } else {
                        (None, providers::command_code::session(&input)?)
                    };
                    let key = key?;
                    let mut report =
                        providers::command_code::query(&*shared.http, &key, proxy.as_deref())?;
                    providers::command_code::enhance(
                        &*shared.http,
                        &cookie,
                        &mut report,
                        proxy.as_deref(),
                    )?;
                    Ok((selection, cookie, report))
                })();
                let mut guard = shared.accounts.lock().unwrap();
                let accounts = guard.as_mut().map_err(|e| *e)?;
                if gate.load(Ordering::SeqCst) != version {
                    accounts.fail_enhancement(ticket, UsageError::Stale);
                    return Err(UsageError::Stale);
                }
                let result = verified.and_then(|(selection, cookie, report)| {
                    if let Some(selection) = selection {
                        accounts.save_browser_enhancement(ticket, selection, report, now())
                    } else {
                        accounts.save_enhancement(ticket, &cookie, report, now())
                    }
                });
                if let Err(error) = result {
                    accounts.fail_enhancement(ticket, error);
                }
                result
            })
            .await;
            publish(&data, cx).await;
            let _ = page.update_in(cx, |page, window, cx| {
                page.submitting = false;
                if version == page.form_version.load(Ordering::SeqCst) {
                    page.error = result.err();
                    if result.is_ok() {
                        page.cancel(window, cx);
                    }
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
        if matches!(edit, Edit::Enhance(_))
            && self.source.read(cx).selected_value() == Some(&Source::Browser)
            && (!browser::valid_command_cookie_name(self.cookie_name.read(cx).value().trim())
                || self.discovering
                || self.profiles.read(cx).selected_value().is_none())
        {
            return;
        }
        let name = self.name.read(cx).value().trim().to_string();
        if name.is_empty() {
            return;
        }
        self.submitting = true;
        self.error = None;
        match edit {
            Edit::Rename(id) => self.work(window, cx, move |a| a.rename(id, &name)),
            Edit::Remove(id) => self.work(window, cx, move |a| a.remove(id)),
            Edit::Enhance(id) => self.submit_enhancement(id, window, cx),
            Edit::Add | Edit::Replace(_) => {
                let secret = zeroize::Zeroizing::new(self.key.read(cx).value().trim().to_string());
                let source = self
                    .source
                    .read(cx)
                    .selected_value()
                    .copied()
                    .unwrap_or(Source::ApiKey);
                let provider = self
                    .provider
                    .read(cx)
                    .selected_value()
                    .copied()
                    .unwrap_or_default();
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
                cx.spawn_in(window, async move |page, cx| {
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
                                (
                                    Provider::DeepSeek | Provider::Kimi | Provider::CommandCode,
                                    Source::ApiKey,
                                ) => Material::ApiKey(secret),
                                (Provider::DeepSeek, Source::Session) => {
                                    Material::Session(platform::parse_manual(&secret)?)
                                }
                                (Provider::Kimi, Source::Session) => Material::ProviderSession(
                                    provider,
                                    providers::kimi::session(&secret)?,
                                ),
                                (Provider::CommandCode, _) => {
                                    return Err(UsageError::Authentication);
                                }
                                (Provider::DeepSeek | Provider::Kimi, Source::Browser) => {
                                    let profile = profile.ok_or(UsageError::BrowserMissing)?;
                                    if provider == Provider::Kimi {
                                        let (selection, material) = shared
                                            .browser
                                            .import_for(&profile, browser::StorageSource::Kimi)
                                            .map_err(browser_error)?;
                                        providers::kimi::session(material.secret())?;
                                        Material::ProviderBrowser(selection, material)
                                    } else {
                                        let (selection, material) = shared
                                            .browser
                                            .import(&profile)
                                            .map_err(browser_error)?;
                                        Material::Browser(
                                            selection,
                                            platform::parse_browser(material.secret())?,
                                        )
                                    }
                                }
                                (_, Source::Session) => Material::ProviderSession(
                                    provider,
                                    providers::parse_manual(provider, &secret)?,
                                ),
                                (_, Source::Browser) => {
                                    let profile = profile.ok_or(UsageError::BrowserMissing)?;
                                    let (selection, material) = shared
                                        .browser
                                        .import_cookies(
                                            &profile,
                                            cookie_provider(provider)?,
                                            &browser::ImportPermission::for_form(
                                                gate.clone(),
                                                version,
                                            ),
                                        )
                                        .map_err(browser_error)?;
                                    Material::Cookies(
                                        provider,
                                        selection,
                                        providers::parse_session(provider, material.secret())?,
                                    )
                                }
                                (_, Source::ApiKey) => return Err(UsageError::Authentication),
                            };
                            let mut balance = query_provider(
                                &*shared.http,
                                provider,
                                material.http_source(),
                                material.secret(),
                                proxy.as_deref(),
                            )?;
                            if let Material::ProviderBrowser(_, browser) = &material {
                                bind_browser_identity(&mut balance, browser.identity())?;
                            }
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
                    let _ = page.update_in(cx, |page, window, cx| {
                        page.submitting = false;
                        if version == page.form_version.load(Ordering::SeqCst) {
                            page.error = result.err();
                            if result.is_ok() {
                                page.cancel(window, cx);
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
                        let provider = accounts.account(id).unwrap().provider;
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
                        let enhancement = accounts.enhancement_secret(id);
                        let enhancement_browser =
                            accounts.account(id).unwrap().enhancement_browser.clone();
                        (
                            ticket,
                            auth,
                            provider,
                            secret,
                            enhancement,
                            enhancement_browser,
                        )
                    })
                };
                let Some((ticket, auth, provider, secret, enhancement, enhancement_browser)) =
                    start
                else {
                    return;
                };
                let mut result = secret.and_then(|secret| match auth {
                    Authentication::ManualApiKey { .. } => query_provider(
                        &*shared.http,
                        provider,
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
                    Authentication::ProviderSession { provider, .. } => query_provider(
                        &*shared.http,
                        provider,
                        HttpSource::KimiWeb,
                        secret.as_deref().map(|s| s.as_str()).unwrap_or(""),
                        proxy.as_deref(),
                    ),
                    Authentication::BrowserLocalStorage { selection } => {
                        if provider == Provider::Kimi {
                            let material = shared
                                .browser
                                .resolve_for(&selection, browser::StorageSource::Kimi)
                                .map_err(browser_error)?;
                            let mut balance = query_provider(
                                &*shared.http,
                                provider,
                                HttpSource::KimiWeb,
                                material.secret(),
                                proxy.as_deref(),
                            )?;
                            bind_browser_identity(&mut balance, material.identity())?;
                            return Ok(balance);
                        }
                        let material = shared.browser.resolve(&selection).map_err(browser_error)?;
                        let material = platform::parse_browser(material.secret())?;
                        query(
                            &*shared.http,
                            HttpSource::Platform(platform::SessionKind::WebToken),
                            material.secret(),
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
                if let Ok(Balance::Provider(report)) = &mut result {
                    if report.provider == Provider::CommandCode {
                        let enhancement = if let Some(selection) = enhancement_browser {
                            if gate.load(Ordering::SeqCst) {
                                selection
                                    .cookie_name()
                                    .ok_or(UsageError::BrowserChanged)
                                    .and_then(|name| {
                                        shared
                                            .browser
                                            .resolve_cookies(
                                                &selection,
                                                browser::CookieProvider::CommandCode(name),
                                            )
                                            .map_err(browser_error)
                                            .and_then(|material| {
                                                providers::command_code::session(material.secret())
                                            })
                                            .map(Some)
                                    })
                            } else {
                                Err(UsageError::Stale)
                            }
                        } else {
                            enhancement
                        };
                        match enhancement {
                            Ok(Some(cookie)) => {
                                if let Err(error) = providers::command_code::enhance(
                                    &*shared.http,
                                    &cookie,
                                    report,
                                    proxy.as_deref(),
                                ) {
                                    report.enhancement = Some(error);
                                }
                            }
                            Err(error) => report.enhancement = Some(error),
                            Ok(None) => {}
                        }
                    }
                }
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
        #[cfg(target_os = "macos")]
        browser::BrowserError::FullDiskAccess => UsageError::BrowserFullDiskAccess,
    }
}
fn cookie_provider(provider: Provider) -> Result<browser::CookieProvider<'static>, UsageError> {
    match provider {
        Provider::StepFun => Ok(browser::CookieProvider::StepFun),
        Provider::MiMo => Ok(browser::CookieProvider::MiMo),
        _ => Err(UsageError::Authentication),
    }
}
fn error_text(error: UsageError) -> &'static str {
    t(match error {
        UsageError::Authentication => L10nKey::UsageAuthError,
        UsageError::IdentityMismatch => L10nKey::UsageIdentityMismatch,
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
        UsageError::BrowserFullDiskAccess => L10nKey::UsageBrowserFullDiskAccess,
    })
}
fn bind_browser_identity(balance: &mut Balance, identity: Option<&str>) -> Result<(), UsageError> {
    if let (Balance::Provider(report), Some(identity)) = (balance, identity) {
        if report.identity.as_deref().is_some_and(|id| id != identity) {
            return Err(UsageError::BrowserChanged);
        }
        report.identity = Some(identity.to_owned());
    }
    Ok(())
}
fn pool_label(kind: providers::PoolKind) -> &'static str {
    use providers::PoolKind;
    t(match kind {
        PoolKind::FiveHour => L10nKey::UsageFiveHour,
        PoolKind::Weekly => L10nKey::UsageWeekly,
        PoolKind::MonthlyTotal => L10nKey::UsageMonthlyTotal,
        PoolKind::MonthlyCode => L10nKey::UsageMonthlyCode,
        PoolKind::Monthly => L10nKey::UsageMonthly,
        PoolKind::Free => L10nKey::UsageFree,
        PoolKind::Booster => L10nKey::UsageBooster,
        PoolKind::Coding => L10nKey::UsageCoding,
        PoolKind::CodeFiveHour => L10nKey::UsageCodeFiveHour,
        PoolKind::CodeWeekly => L10nKey::UsageCodeWeekly,
        PoolKind::BoosterMonthly => L10nKey::UsageBoosterMonthly,
        PoolKind::RawBalance => L10nKey::UsageRawBalance,
        PoolKind::Balance => L10nKey::UsageBalance,
        PoolKind::Cash => L10nKey::UsageCash,
        PoolKind::Gift => L10nKey::UsageGift,
        PoolKind::Frozen => L10nKey::UsageFrozen,
        PoolKind::Overdraft => L10nKey::UsageOverdraft,
        PoolKind::SubscriptionCredit => L10nKey::UsageSubscriptionCredit,
        PoolKind::CreditBucket => L10nKey::UsageCreditBucket,
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
impl SettingsWindow {
    pub(super) fn usage_form_open(&self, cx: &gpui_kit::App) -> bool {
        self.active_settings().is_some_and(|s| {
            s.section == super::SettingsSection::Usage && s.usage.read(cx).edit.is_some()
        })
    }

    pub(super) fn cancel_usage_form(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(usage) = self
            .active_settings()
            .map(|settings| settings.usage.clone())
        else {
            return false;
        };
        usage.update(cx, |page, cx| {
            if page.edit.is_none() {
                return false;
            }
            page.cancel(window, cx);
            true
        })
    }
    pub(super) fn render_settings_usage(&self, _cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        self.active_settings()
            .unwrap()
            .usage
            .clone()
            .into_any_element()
    }
    pub(super) fn render_usage_overlay(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<gpui_kit::AnyElement> {
        let page = self.active_settings()?.usage.clone();
        page.update(cx, |page, cx| page.render_overlay(cx))
    }
    pub(super) fn render_usage_search_row(&self, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        self.settings_row(
            t(L10nKey::SettingsNavUsage),
            t(L10nKey::UsageSubtitle),
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
