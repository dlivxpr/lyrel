use super::*;
use crate::ui::dialog;
use gpui_kit::TestSupportExt as _;
use gpui_kit::assets::IconName;
use gpui_kit::base::FocusTrapElement as _;
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};

fn source_label(auth: &Authentication) -> String {
    match auth {
        Authentication::ManualApiKey { .. } => t(L10nKey::UsageApiSource).into(),
        Authentication::ManualSession { kind, .. } => format!(
            "{} · {}",
            t(L10nKey::UsageSessionSource),
            match kind {
                platform::SessionKind::WebToken => "userToken · Bearer",
                platform::SessionKind::HarnessGrant => "Harness grant · x-dsh-auth-token",
            }
        ),
        Authentication::ProviderSession { .. } => t(L10nKey::UsageSessionSource).into(),
        Authentication::BrowserLocalStorage { selection }
        | Authentication::BrowserCookies { selection, .. } => format!(
            "{} · {} · {}",
            t(L10nKey::UsageBrowserSource),
            selection.browser().label(),
            selection.label()
        ),
    }
}
fn interface_label(provider: Provider, api_key: bool) -> &'static str {
    t(if provider == Provider::DeepSeek && api_key {
        L10nKey::UsageOfficialApi
    } else {
        L10nKey::UsagePrivateApi
    })
}
fn menu_item(id: String, label: &'static str) -> PopupMenuItem {
    PopupMenuItem::element(move |_, _| {
        div()
            .id(id.clone())
            .test_support()
            .aria_label(label)
            .child(label)
    })
}
fn render_pool(pool: &providers::Pool, cx: &gpui_kit::App) -> gpui_kit::Div {
    let unit = if pool.unit == "requests" {
        t(L10nKey::UsageRequests)
    } else {
        &pool.unit
    };
    let mut metric = v_flex().min_w_0().gap_1().child(
        h_flex()
            .flex_wrap()
            .gap_2()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(pool_label(pool.kind))
                    .when_some(pool.name.as_ref(), |label, name| {
                        label.child(format!(" · {name}"))
                    }),
            )
            .child(
                div()
                    .text_xs()
                    .font_weight(gpui_kit::FontWeight::MEDIUM)
                    .child(format!(
                        "{} {} {}",
                        t(L10nKey::UsageRemaining),
                        pool.remaining
                            .as_deref()
                            .unwrap_or(t(L10nKey::UsageUnknown)),
                        unit
                    )),
            ),
    );
    // Only an upstream percentage or a real remaining/limit pair can draw a bar.
    // Original decimal strings remain the displayed source of truth (including Credits).
    let remaining = pool
        .remaining
        .as_ref()
        .and_then(|v| v.parse::<f64>().ok())
        .filter(|v| v.is_finite() && *v >= 0.);
    let limit = if pool.unit == "%" {
        Some(100.)
    } else {
        pool.limit
            .as_ref()
            .and_then(|v| v.parse::<f64>().ok())
            .filter(|v| v.is_finite() && *v > 0.)
    };
    if let Some(ratio) = remaining
        .zip(limit)
        .map(|(remaining, limit)| (remaining / limit).clamp(0., 1.) as f32)
    {
        metric = metric.child(
            div()
                .h(gpui_kit::rems(0.375))
                .w_full()
                .rounded_sm()
                .bg(cx.theme().muted)
                .child(
                    div()
                        .h_full()
                        .w(gpui_kit::relative(ratio))
                        .rounded_sm()
                        .bg(cx.theme().primary),
                ),
        );
    }
    let mut detail = h_flex()
        .flex_wrap()
        .gap_2()
        .text_xs()
        .text_color(cx.theme().muted_foreground);
    for (label, value) in [
        (L10nKey::UsageUsed, &pool.used),
        (L10nKey::UsageLimit, &pool.limit),
    ] {
        if let Some(value) = value {
            detail = detail.child(format!("{} {value} {unit}", t(label)));
        }
    }
    for (label, value) in [
        (L10nKey::UsageReset, &pool.reset),
        (L10nKey::UsageExpires, &pool.expires),
    ] {
        if let Some(value) = value {
            detail = detail.child(format!("{} {value}", t(label)));
        }
    }
    metric = metric.child(detail);
    metric
}
impl UsagePage {
    fn open_add(&mut self, provider: Provider, window: &mut Window, cx: &mut Context<Self>) {
        self.open_edit(Edit::Add, window, cx);
        self.provider.update(cx, |state, cx| {
            state.set_selected_value(&provider, window, cx)
        });
        self.source.update(cx, |state, cx| {
            state.set_items(provider.sources(), window, cx);
            state.set_selected_value(
                &if matches!(provider, Provider::StepFun | Provider::MiMo) {
                    Source::Session
                } else {
                    Source::ApiKey
                },
                window,
                cx,
            );
        });
        self.browsers.update(cx, |state, cx| {
            state.set_items(browser_choices(provider, false), window, cx)
        });
    }
    fn render_header(&self, cx: &Context<Self>) -> impl IntoElement {
        let data = self.data.read(cx);
        let refreshing = !self.pending.is_empty() || data.stored.accounts.iter().any(|a| a.busy);
        h_flex()
            .id("usage-header")
            .test_support()
            .w_full()
            .min_w_0()
            .flex_none()
            .flex_wrap()
            .gap_2()
            .pb_3()
            .child(
                div()
                    .flex_1()
                    .min_w(gpui_kit::rems(12.))
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(t(if data.loading || refreshing {
                        L10nKey::UsageLoading
                    } else {
                        L10nKey::UsagePageVisible
                    })),
            )
            .child(
                h_flex()
                    .flex_wrap()
                    .min_w_0()
                    .gap_2()
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(t(L10nKey::UsageCadence)),
                    )
                    .child(
                        // Select 的外层为 size_full；由容器给定两个轴，避免挤出工具栏。
                        div().w(gpui_kit::rems(10.)).h_7().flex_none().child(
                            Select::new(&self.cadence)
                                .id("usage-cadence")
                                .small()
                                .disabled(self.submitting)
                                .accessibility_label(t(L10nKey::UsageCadence)),
                        ),
                    )
                    .child(
                        button(
                            "usage-refresh-all",
                            t(L10nKey::UsageRefreshAll),
                            BtnKind::Secondary,
                        )
                        .ghost()
                        .icon(IconName::RefreshCw)
                        .loading(refreshing)
                        .disabled(
                            data.loading
                                || !data
                                    .stored
                                    .accounts
                                    .iter()
                                    .any(|a| a.enabled && !a.removing),
                        )
                        .on_click(cx.listener(|page, _, _, cx| page.refresh_all(cx))),
                    ),
            )
    }
    fn render_provider(
        &self,
        provider: Provider,
        accounts: &[Account],
        cx: &Context<Self>,
    ) -> gpui_kit::AnyElement {
        let data = self.data.read(cx);
        let mut cards = h_flex().items_start().flex_wrap().gap_3().w_full();
        let mut count = 0;
        for account in accounts.iter().filter(|a| a.provider == provider) {
            count += 1;
            cards = cards.child(self.render_account(account, cx));
        }
        if count % 2 == 1 {
            // An empty layout slot keeps a single account at the same width as
            // its two-column peers. It has no text, role, focus or account ID.
            cards = cards.child(
                div()
                    .flex_1()
                    .flex_basis(gpui_kit::relative(0.48))
                    .min_w(gpui_kit::rems(18.))
                    .max_w_full(),
            );
        }
        let mut section = v_flex()
            .id(format!("usage-provider-{provider:?}"))
            .gap_2()
            .w_full()
            .child(
                h_flex()
                    .gap_2()
                    .flex_wrap()
                    .child(
                        div()
                            .size_5()
                            .flex_none()
                            .rounded_sm()
                            .bg(cx.theme().foreground)
                            .text_color(cx.theme().background)
                            .text_xs()
                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(match provider {
                                Provider::Kimi => "K",
                                Provider::CommandCode => "C",
                                Provider::StepFun => "S",
                                Provider::DeepSeek => "D",
                                Provider::MiMo => "M",
                            }),
                    )
                    .child(
                        div()
                            .text_sm()
                            .font_weight(gpui_kit::FontWeight::MEDIUM)
                            .child(provider.label()),
                    )
                    .child(
                        div()
                            .rounded_sm()
                            .px_1()
                            .text_xs()
                            .bg(cx.theme().muted)
                            .text_color(cx.theme().muted_foreground)
                            .child(interface_label(provider, provider == Provider::DeepSeek)),
                    )
                    .child(div().flex_1())
                    .child(
                        button(
                            format!("usage-add-{provider:?}"),
                            t(L10nKey::UsageAdd),
                            BtnKind::Secondary,
                        )
                        .ghost()
                        .icon(IconName::Plus)
                        .accessibility_label(format!(
                            "{} · {}",
                            t(L10nKey::UsageAdd),
                            provider.label()
                        ))
                        .disabled(self.submitting || data.loading || data.error.is_some())
                        .on_click(cx.listener(
                            move |page, _, window, cx| page.open_add(provider, window, cx),
                        )),
                    ),
            );
        if count == 0 {
            section = section.child(
                div()
                    .rounded_md()
                    .bg(cx.theme().muted)
                    .px_3()
                    .py_2()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(t(L10nKey::UsageProviderEmpty)),
            );
        } else {
            section = section.child(cards);
        }
        section.into_any_element()
    }
    fn render_account(&self, account: &Account, cx: &Context<Self>) -> gpui_kit::AnyElement {
        let id = account.id;
        let muted = cx.theme().muted_foreground;
        let danger = cx.theme().danger;
        let enabled = account.enabled;
        let blocked = account.removing || self.submitting;
        let enhancement_blocked = blocked || account.busy;
        let submitting = self.submitting;
        let command_code = account.provider == Provider::CommandCode;
        let page = cx.entity().downgrade();
        let menu = Button::new(format!("usage-more-{id}"))
            .small()
            .ghost()
            .icon(IconName::Ellipsis)
            .accessibility_label(format!("{} · {}", t(L10nKey::UsageMore), account.name))
            .tooltip(t(L10nKey::UsageMore))
            .on_mouse_down(
                gpui_kit::MouseButton::Left,
                cx.listener(|page, _, window, cx| {
                    page.return_focus = window.focused(cx);
                }),
            )
            .capture_action(cx.listener(
                |page, _: &gpui_kit::base::actions::Confirm, window, cx| {
                    page.return_focus = window.focused(cx);
                },
            ))
            .dropdown_menu(move |mut menu, _window, cx| {
                // Popover has already focused its own surface before building this menu.
                let return_focus = page
                    .upgrade()
                    .and_then(|page| page.read(cx).return_focus.clone());
                for (command, label, edit) in [
                    ("rename", L10nKey::UsageRename, Edit::Rename(id)),
                    ("replace", L10nKey::UsageReplace, Edit::Replace(id)),
                ] {
                    let page = page.clone();
                    let return_focus = return_focus.clone();
                    menu = menu.item(
                        menu_item(format!("usage-{command}-{id}"), t(label))
                            .disabled(blocked)
                            .on_click(move |_, window, cx| {
                                let _ = page.update(cx, |page, cx| {
                                    page.open_edit(edit, window, cx);
                                    page.return_focus = return_focus.clone();
                                });
                            }),
                    );
                }
                let owner = page.clone();
                menu = menu.item(
                    menu_item(
                        format!("usage-toggle-{id}"),
                        t(if enabled {
                            L10nKey::UsageDisable
                        } else {
                            L10nKey::UsageEnable
                        }),
                    )
                    .disabled(blocked)
                    .on_click(move |_, window, cx| {
                        let _ = owner.update(cx, |page, cx| {
                            page.work(window, cx, move |a| a.set_enabled(id, !enabled))
                        });
                    }),
                );
                if command_code {
                    let owner = page.clone();
                    let return_focus = return_focus.clone();
                    menu = menu.item(
                        menu_item(format!("usage-enhance-{id}"), t(L10nKey::UsageEnhance))
                            .disabled(enhancement_blocked)
                            .on_click(move |_, window, cx| {
                                let _ = owner.update(cx, |page, cx| {
                                    page.open_edit(Edit::Enhance(id), window, cx);
                                    page.return_focus = return_focus.clone();
                                });
                            }),
                    );
                }
                let owner = page.clone();
                menu.separator().item(
                    menu_item(format!("usage-remove-{id}"), t(L10nKey::UsageRemove))
                        .disabled(submitting)
                        .on_click(move |_, window, cx| {
                            let _ = owner.update(cx, |page, cx| {
                                page.open_edit(Edit::Remove(id), window, cx);
                                page.return_focus = return_focus.clone();
                            });
                        }),
                )
            });
        let mut row = v_flex()
            .id(format!("usage-account-{id}"))
            .test_support()
            .aria_label(account.name.clone())
            .flex_1()
            .flex_basis(gpui_kit::relative(0.48))
            .min_w(gpui_kit::rems(18.))
            .max_w_full()
            .gap(gpui_kit::rems(0.625))
            .p_3()
            .rounded_xl()
            .bg(crate::ui::settings::kit::Tk::of(cx).k04)
            .child(
                h_flex()
                    .items_start()
                    .gap_1()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .font_weight(gpui_kit::FontWeight::MEDIUM)
                            .child(account.name.clone()),
                    )
                    .child(
                        Button::new(format!("usage-refresh-{id}"))
                            .small()
                            .ghost()
                            .icon(IconName::RefreshCw)
                            .accessibility_label(format!(
                                "{} · {}",
                                t(L10nKey::UsageRefresh),
                                account.name
                            ))
                            .tooltip(t(L10nKey::UsageRefresh))
                            .loading(self.pending.contains(&id) || account.busy)
                            .disabled(!enabled || account.removing || self.pending.contains(&id))
                            .on_click(cx.listener(move |page, _, _, cx| page.refresh(id, cx))),
                    )
                    .child(menu),
            )
            .child(self.render_account_metadata(account, cx));
        if let Some(snapshot) = &account.snapshot {
            match &snapshot.balance {
                Balance::Official(balance) => {
                    for line in &balance.balance_infos {
                        row = row
                            .child(
                                h_flex()
                                    .flex_wrap()
                                    .gap_2()
                                    .child(
                                        div()
                                            .flex_1()
                                            .text_xs()
                                            .text_color(muted)
                                            .child(t(L10nKey::UsageBalance)),
                                    )
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_weight(gpui_kit::FontWeight::MEDIUM)
                                            .child(format!(
                                                "{} {}",
                                                line.total_balance, line.currency
                                            )),
                                    ),
                            )
                            .child(div().text_xs().text_color(muted).child(format!(
                                "{} {} {} · {} {} {}",
                                t(L10nKey::UsageGranted),
                                line.granted_balance,
                                line.currency,
                                t(L10nKey::UsageToppedUp),
                                line.topped_up_balance,
                                line.currency
                            )));
                    }
                    row = row.child(div().text_xs().text_color(muted).child(t(
                        if balance.is_available {
                            L10nKey::UsageAvailable
                        } else {
                            L10nKey::UsageUnavailable
                        },
                    )));
                }
                Balance::Platform(balance) => {
                    for (label, wallets) in [
                        (L10nKey::UsageNormalWallet, &balance.normal_wallets),
                        (L10nKey::UsageBonusWallet, &balance.bonus_wallets),
                    ] {
                        for wallet in wallets {
                            row = row.child(
                                h_flex()
                                    .flex_wrap()
                                    .gap_2()
                                    .child(
                                        div().flex_1().text_xs().text_color(muted).child(t(label)),
                                    )
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_weight(gpui_kit::FontWeight::MEDIUM)
                                            .child(format!(
                                                "{} {}",
                                                wallet.balance, wallet.currency
                                            )),
                                    ),
                            );
                        }
                    }
                }
                Balance::Provider(report) => {
                    if report.overdrawn == Some(true) {
                        row = row.child(div().text_color(danger).child(t(L10nKey::UsageOverdrawn)));
                    }
                    if report.expired == Some(true) {
                        row = row.child(t(L10nKey::UsageUnavailable));
                    }
                    for pool in &report.pools {
                        row = row.child(render_pool(pool, cx));
                    }
                    if let Some(error) = report.enhancement {
                        row = row.child(div().text_color(danger).child(format!(
                            "{} · {}",
                            t(L10nKey::UsageEnhancementFailed),
                            error_text(error)
                        )));
                    }
                }
            }
        } else {
            row = row.child(t(L10nKey::UsageUnknown));
        }
        if let Some(error) = account.error {
            row = row.child(div().text_color(danger).child(error_text(error)));
        }
        let mut footer = v_flex()
            .gap_1()
            .id(format!("usage-updated-{id}"))
            .test_support()
            .border_t_1()
            .border_color(cx.theme().border)
            .pt_2()
            .text_xs()
            .text_color(muted);
        if let Some(snapshot) = &account.snapshot {
            let time = chrono::DateTime::from_timestamp(snapshot.updated_at as i64, 0)
                .map(|time| {
                    time.with_timezone(&chrono::Local)
                        .format("%Y-%m-%d %H:%M:%S %:z")
                        .to_string()
                })
                .unwrap_or_else(|| snapshot.updated_at.to_string());
            footer = footer.child(format!("{} {time}", t(L10nKey::UsageUpdated)));
            if account.error.is_some() {
                footer = footer.child(t(L10nKey::UsageOldSnapshot));
            }
        } else {
            footer = footer.child(t(L10nKey::UsageUnknown));
        }
        row = row.child(footer);

        row.into_any_element()
    }
    fn render_account_metadata(&self, account: &Account, cx: &Context<Self>) -> gpui_kit::Div {
        let mut metadata = h_flex()
            .min_w_0()
            .flex_wrap()
            .gap_2()
            .text_xs()
            .text_color(cx.theme().muted_foreground);
        if let Some(Snapshot {
            balance: Balance::Provider(report),
            ..
        }) = &account.snapshot
        {
            if let Some(identity) = &report.identity {
                metadata = metadata.child(format!("{}: {identity}", t(L10nKey::UsageIdentity)));
            } else {
                metadata = metadata.child(format!(
                    "{}: {}",
                    t(L10nKey::UsageIdentity),
                    t(L10nKey::UsageUnknown)
                ));
            }
            if let Some(plan) = &report.plan {
                metadata = metadata.child(plan.clone());
            }
            if let Some(expires) = &report.plan_expires {
                metadata = metadata.child(format!(
                    "{} · {} {expires}",
                    t(L10nKey::UsagePlan),
                    t(L10nKey::UsageExpires)
                ));
            }
        }
        metadata = metadata.child(
            div()
                .min_w_0()
                .rounded_sm()
                .px_1()
                .bg(cx.theme().muted)
                .child(source_label(&account.auth)),
        );
        // DeepSeek supports both official key queries and private sessions.
        // The account source must remain explicit even under its official badge.
        if account.provider == Provider::DeepSeek
            && !matches!(account.auth, Authentication::ManualApiKey { .. })
        {
            metadata = metadata.child(interface_label(account.provider, false));
        }
        if account.provider == Provider::CommandCode {
            if let Some(selection) = &account.enhancement_browser {
                metadata = metadata.child(format!(
                    "{} · {} · {} · {}",
                    t(L10nKey::UsageEnhancementBrowserSource),
                    selection.browser().label(),
                    selection.label(),
                    selection.cookie_name().unwrap_or("")
                ));
            } else if account.enhancement_slot.is_some() {
                metadata = metadata.child(t(L10nKey::UsageEnhancementManualSource));
            }
        }
        metadata.child(
            div()
                .text_color(if account.removing {
                    cx.theme().danger
                } else {
                    cx.theme().muted_foreground
                })
                .child(t(if account.removing {
                    L10nKey::UsageRemoving
                } else if self.pending.contains(&account.id) || account.busy {
                    L10nKey::UsageLoading
                } else if !account.enabled {
                    L10nKey::UsageDisabled
                } else {
                    L10nKey::UsageEnabled
                })),
        )
    }
    fn render_form(&self, edit: Edit, cx: &Context<Self>) -> gpui_kit::AnyElement {
        let danger = cx.theme().danger;
        let muted = cx.theme().muted_foreground;

        let mut form = dialog::body()
            .id("usage-form-body")
            .test_support()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.form_scroll);
        let title = match edit {
            Edit::Add => L10nKey::UsageAdd,
            Edit::Rename(_) => L10nKey::UsageRename,
            Edit::Replace(_) => L10nKey::UsageReplace,
            Edit::Remove(_) => L10nKey::UsageRemove,
            Edit::Enhance(_) => L10nKey::UsageEnhance,
        };

        if matches!(edit, Edit::Remove(_)) {
            form = form
                .child(t(L10nKey::UsageRemoveWarning))
                .child(self.name.read(cx).value());
        } else if matches!(edit, Edit::Enhance(_)) {
            form = form.child(self.name.read(cx).value());
        } else {
            form = form.child(dialog::labelled(
                t(L10nKey::UsageName),
                Input::new(&self.name)
                    .id("usage-name")
                    .w_full()
                    .disabled(self.submitting)
                    .aria_label(t(L10nKey::UsageName)),
                cx,
            ));
        }
        let provider = self
            .provider
            .read(cx)
            .selected_value()
            .copied()
            .unwrap_or_default();
        if matches!(edit, Edit::Add | Edit::Replace(_)) {
            form = form.child(dialog::labelled_control(
                t(L10nKey::UsageProvider),
                Select::new(&self.provider)
                    .id("usage-provider")
                    .small()
                    .w_full()
                    .disabled(self.submitting || matches!(edit, Edit::Replace(_)))
                    .accessibility_label(t(L10nKey::UsageProvider)),
                cx,
            ));
        }
        if matches!(edit, Edit::Add | Edit::Replace(_) | Edit::Enhance(_)) {
            let source = self
                .source
                .read(cx)
                .selected_value()
                .copied()
                .unwrap_or(Source::ApiKey);
            form = form.child(dialog::labelled_control(
                t(L10nKey::UsageSource),
                Select::new(&self.source)
                    .id("usage-source")
                    .small()
                    .w_full()
                    .disabled(self.submitting)
                    .accessibility_label(t(L10nKey::UsageSource)),
                cx,
            ));
            if source == Source::ApiKey {
                form = form.child(div().text_xs().text_color(muted).child(t(match provider {
                    Provider::DeepSeek => L10nKey::UsageOfficial,
                    Provider::Kimi => L10nKey::UsageKimiDisclosure,
                    Provider::CommandCode => L10nKey::UsageCommandDisclosure,
                    _ => provider.help(),
                })));
                form = form.child(dialog::labelled(
                    t(L10nKey::UsageApiKey),
                    Input::new(&self.key)
                        .id("usage-key")
                        .w_full()
                        .disabled(self.submitting)
                        .aria_label(t(L10nKey::UsageApiKey)),
                    cx,
                ));
            } else {
                if matches!(edit, Edit::Enhance(_)) {
                    form = form.child(t(L10nKey::UsageCommandDisclosure));
                }
                form = form.child(div().text_color(muted).child(t(
                    if provider == Provider::DeepSeek {
                        L10nKey::UsagePrivateDisclosure
                    } else {
                        provider.help()
                    },
                )));
                if source == Source::Session {
                    let label = t(if provider == Provider::DeepSeek {
                        L10nKey::UsageSessionInput
                    } else if provider == Provider::Kimi {
                        L10nKey::UsageSessionSource
                    } else {
                        L10nKey::UsageCookieInput
                    });
                    form = form
                        .child(dialog::labelled(
                            label,
                            Input::new(&self.key)
                                .id("usage-key")
                                .w_full()
                                .disabled(self.submitting)
                                .aria_label(label),
                            cx,
                        ))
                        .when(provider == Provider::DeepSeek, |form| {
                            form.child(div().text_xs().text_color(muted).child(t(provider.help())))
                        });
                } else {
                    form = form
                        .child(dialog::labelled_control(
                            t(L10nKey::UsageBrowser),
                            Select::new(&self.browsers)
                                .id("usage-browser")
                                .small()
                                .w_full()
                                .disabled(self.submitting || self.discovering)
                                .placeholder(t(L10nKey::UsageChooseBrowser))
                                .accessibility_label(t(L10nKey::UsageBrowser)),
                            cx,
                        ))
                        .child(dialog::labelled_control(
                            t(L10nKey::UsageProfile),
                            Select::new(&self.profiles)
                                .id("usage-profile")
                                .small()
                                .w_full()
                                .disabled(self.submitting || self.discovering)
                                .placeholder(t(L10nKey::UsageChooseProfile))
                                .accessibility_label(t(L10nKey::UsageProfile)),
                            cx,
                        ))
                        .child(div().text_color(muted).child(t(match provider {
                            Provider::Kimi => L10nKey::UsageKimiHelp,
                            Provider::CommandCode => L10nKey::UsageCommandBrowserHelp,
                            Provider::DeepSeek => L10nKey::UsageBrowserHelp,
                            _ => L10nKey::UsageCookieBrowserHelp,
                        })));
                    if matches!(edit, Edit::Enhance(_)) {
                        form = form
                            .child(dialog::labelled(
                                t(L10nKey::UsageCommandCookieName),
                                Input::new(&self.cookie_name)
                                    .id("usage-cookie-name")
                                    .w_full()
                                    .disabled(self.submitting)
                                    .aria_label(t(L10nKey::UsageCommandCookieName)),
                                cx,
                            ))
                            .child(
                                div()
                                    .text_color(muted)
                                    .child(t(L10nKey::UsageCommandCookieNameHelp)),
                            );
                    }
                    if self.discovering {
                        form = form.child(t(L10nKey::UsageLoading));
                    }
                }
            }
        }
        if let Some(error) = self.error {
            form = form.child(div().text_color(danger).child(error_text(error)));
        }
        let footer = dialog::footer(cx)
            .child(
                button("usage-cancel", t(L10nKey::UsageCancel), BtnKind::Secondary)
                    .on_click(cx.listener(|p, _, w, cx| p.cancel(w, cx))),
            )
            .child(
                button(
                    "usage-submit",
                    t(if self.submitting {
                        L10nKey::UsageLoading
                    } else if matches!(edit, Edit::Add | Edit::Replace(_) | Edit::Enhance(_)) {
                        L10nKey::UsageVerify
                    } else if matches!(edit, Edit::Remove(_)) {
                        L10nKey::UsageRemoveCommit
                    } else {
                        L10nKey::UsageSave
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
                        || (matches!(edit, Edit::Add | Edit::Replace(_) | Edit::Enhance(_))
                            && if self.source.read(cx).selected_value() == Some(&Source::Browser) {
                                self.discovering
                                    || self.profiles.read(cx).selected_value().is_none()
                                    || (matches!(edit, Edit::Enhance(_))
                                        && !browser::valid_command_cookie_name(
                                            self.cookie_name.read(cx).value().trim(),
                                        ))
                            } else {
                                self.key.read(cx).value().trim().is_empty()
                            }),
                )
                .on_click(cx.listener(|p, _, w, cx| p.submit(w, cx))),
            );
        dialog::card("usage-form", t(title).trim_end_matches('…'), 540., cx)
            .w(gpui_kit::rems(34.))
            .max_w_full()
            .max_h_full()
            .child(dialog::header(t(title).trim_end_matches('…'), cx))
            .child(form)
            .child(footer)
            .focus_trap("usage-form-trap", &self.form_focus)
            .into_any_element()
    }

    pub(super) fn render_overlay(&self, cx: &Context<Self>) -> Option<gpui_kit::AnyElement> {
        let edit = self.edit?;
        Some(
            div()
                .absolute()
                .inset_0()
                .occlude()
                .flex()
                .items_center()
                .justify_center()
                .p_3()
                .bg(cx.theme().background.opacity(0.88))
                .on_key_down(
                    cx.listener(|page, event: &gpui_kit::KeyDownEvent, window, cx| {
                        if event.keystroke.key == "escape" {
                            page.cancel(window, cx);
                            cx.stop_propagation();
                        }
                    }),
                )
                .child(self.render_form(edit, cx))
                .into_any_element(),
        )
    }
}
impl Render for UsagePage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let data = self.data.read(cx);
        let mut sections = v_flex().gap_6().p_1();
        for (title, providers) in [
            (
                L10nKey::UsageCodingPlans,
                &[Provider::Kimi, Provider::CommandCode, Provider::StepFun][..],
            ),
            (
                L10nKey::UsageApiAccounts,
                &[Provider::DeepSeek, Provider::MiMo][..],
            ),
        ] {
            let mut group = v_flex().gap_3().min_w_0().child(
                div()
                    .pb_2()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .text_sm()
                    .font_weight(gpui_kit::FontWeight::MEDIUM)
                    .child(t(title)),
            );
            for &provider in providers {
                group = group.child(self.render_provider(provider, &data.stored.accounts, cx));
            }
            sections = sections.child(group);
        }
        if let Some(error) = data.error.or(if self.edit.is_none() {
            self.error
        } else {
            None
        }) {
            sections = sections.child(div().text_color(cx.theme().danger).child(error_text(error)));
        }
        let scroll = div()
            .id("usage-content")
            .test_support()
            .size_full()
            .min_h_0()
            .w_full()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .child(sections);
        let page = v_flex()
            .id("usage-page")
            .relative()
            .size_full()
            .min_h_0()
            .min_w_0()
            .text_sm()
            .child(self.render_header(cx))
            .child(crate::ui::scrollbar::with_vertical_scrollbar(
                "usage-scrollbar",
                scroll,
                &self.scroll,
            ));
        page
    }
}
