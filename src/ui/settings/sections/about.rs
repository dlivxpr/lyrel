//! The About page and its maintenance group: version, update channel and
//! housekeeping, below the page header.

use super::kit::{self, BtnKind, Tk, fs};
use super::*;

impl SettingsWindow {
    pub(crate) fn render_settings_about(&self, cx: &mut Context<Self>) -> AnyElement {
        use crate::core::update::UpdatePhase;
        let tk = Tk::of(cx);
        let status = cx
            .try_global::<crate::core::update::UpdateStatus>()
            .cloned()
            .unwrap_or_default();
        let busy = matches!(
            status.phase,
            UpdatePhase::Checking
                | UpdatePhase::Downloading { .. }
                | UpdatePhase::Verifying
                | UpdatePhase::Installing
        );
        let transferring = matches!(
            status.phase,
            UpdatePhase::Downloading { .. } | UpdatePhase::Verifying
        );
        // A staged package whose directory has since been swept away is not an
        // offer worth making.
        let ready = status
            .ready
            .clone()
            .filter(crate::core::update::PendingUpdate::is_usable);
        // "You're running the latest version" above "27.0.0 is ready to
        // install" is a contradiction, and a reachable one; the staged package
        // is the more useful claim.
        let phase_text = localized_update_phase(&status.phase)
            .filter(|_| ready.is_none() || !matches!(status.phase, UpdatePhase::UpToDate));
        let version_desc = match &phase_text {
            Some(text) => format!("{} · {text}", env!("CARGO_PKG_VERSION")),
            None => env!("CARGO_PKG_VERSION").to_string(),
        };

        let check = h_flex()
            .gap(px(8.))
            .when(transferring, |r| {
                r.child(
                    kit::button(
                        "cancel-update-download",
                        t(L10nKey::SettingsUpdateCancel),
                        BtnKind::Link,
                    )
                    .on_click(cx.listener(|_, _, _w, cx| crate::core::update::cancel_download(cx))),
                )
            })
            .child(
                kit::button(
                    "check-update-now",
                    if matches!(status.phase, UpdatePhase::Checking) {
                        t(L10nKey::SettingsUpdateChecking)
                    } else {
                        t(L10nKey::SettingsUpdateCheckNow)
                    },
                    BtnKind::Secondary,
                )
                .disabled(busy)
                .on_click(cx.listener(|_, _, _w, cx| crate::core::update::spawn_check_forced(cx))),
            )
            .into_any_element();
        let mut rows = vec![
            self.settings_row(t(L10nKey::SettingsVersion), version_desc, check, cx)
                .into_any_element(),
        ];
        // A failure the user can act on. Persisted, so it is still here
        // tomorrow.
        if let Some(failure) = status.failure.clone() {
            let control = h_flex()
                .gap(px(12.))
                .child(
                    kit::button(
                        "update-dismiss",
                        t(L10nKey::SettingsUpdateDismiss),
                        BtnKind::Link,
                    )
                    .on_click(cx.listener(|_, _, _w, cx| crate::core::update::dismiss_failure(cx))),
                )
                .child(
                    kit::button(
                        "update-manual",
                        t(L10nKey::SettingsUpdateDownloadManually),
                        BtnKind::Link,
                    )
                    .on_click(
                        cx.listener(|_, _, _w, _cx| crate::core::update::open_releases_page()),
                    ),
                )
                .child(
                    kit::button(
                        "update-retry",
                        t(L10nKey::SettingsUpdateRetry),
                        BtnKind::Secondary,
                    )
                    .disabled(busy)
                    .on_click(cx.listener(|_, _, _w, cx| {
                        crate::core::update::dismiss_failure(cx);
                        crate::core::update::install_available(cx);
                    })),
                )
                .into_any_element();
            rows.push(
                self.settings_row(
                    t_fmt(
                        L10nKey::SettingsUpdateFailedTitle,
                        &[("version", &failure.version)],
                    ),
                    failure.detail.clone(),
                    control,
                    cx,
                )
                .into_any_element(),
            );
        }
        // Downloaded and verified: the decision left is "may I restart".
        if let Some(pending) = ready.clone() {
            let control = h_flex()
                .gap(px(12.))
                .child(
                    kit::button(
                        "discard-ready",
                        t(L10nKey::SettingsUpdateDiscard),
                        BtnKind::Link,
                    )
                    .disabled(busy)
                    .on_click(cx.listener(|_, _, _w, cx| crate::core::update::discard_pending(cx))),
                )
                .child(
                    kit::button(
                        "install-ready",
                        t(L10nKey::SettingsUpdateInstallNow),
                        BtnKind::Primary,
                    )
                    .disabled(busy)
                    .on_click(
                        cx.listener(|_, _, _w, cx| crate::core::update::install_available(cx)),
                    ),
                )
                .into_any_element();
            rows.push(
                self.settings_row(
                    t_fmt(
                        L10nKey::SettingsUpdateReady,
                        &[("version", &pending.version)],
                    ),
                    if pending.apply_on_launch {
                        t(L10nKey::SettingsUpdateReadyNextLaunch).to_string()
                    } else {
                        String::new()
                    },
                    control,
                    cx,
                )
                .into_any_element(),
            );
        }
        // Where the package cannot be installed for the user, the release page
        // is the whole update path; that cannot live only in a dialog that has
        // already been dismissed.
        if let Some(upd) = status.available.clone().filter(|_| ready.is_none()) {
            let action = if upd.installable {
                t(L10nKey::SettingsUpdateAndRelaunch)
            } else {
                t(L10nKey::SettingsUpdateViewRelease)
            };
            let control = kit::button("install-update", action, BtnKind::Primary)
                .disabled(busy)
                .on_click(cx.listener(|_, _, _w, cx| crate::core::update::install_available(cx)))
                .into_any_element();
            rows.push(
                self.settings_row(
                    t_fmt(
                        L10nKey::SettingsVersionAvailable,
                        &[("version", &upd.version)],
                    ),
                    upd.install_hint
                        .map(|hint| localized_update_install_hint(&hint))
                        .unwrap_or_default(),
                    control,
                    cx,
                )
                .into_any_element(),
            );
        }
        let settings_file = lyrel_core::core::config::config_path("config.json");
        if let Some(path) = settings_file {
            let shown = tildify(&path.to_string_lossy());
            let reveal = self
                .settings_button(
                    "reveal-settings-file",
                    t(L10nKey::SettingsReveal),
                    cx,
                    move |_, _, cx| {
                        if !path.exists() {
                            cx.global::<Config>().save();
                        }
                        cx.reveal_path(&path);
                    },
                )
                .into_any_element();
            rows.push(
                self.settings_row(t(L10nKey::SettingsSettingsFile), shown, reveal, cx)
                    .into_any_element(),
            );
        }

        let mut groups = vec![self.settings_group(None, None, rows, cx)];
        groups.push(self.render_settings_maintenance(cx));
        groups.push(
            v_flex()
                .gap(px(6.))
                .text_size(fs(12.))
                .line_height(px(17.))
                .text_color(tk.k45)
                .child(t(L10nKey::SettingsAboutDesc1))
                .child(
                    Link::new("about-github")
                        .href("https://github.com/dlivxpr/lyrel")
                        .text_color(tk.k6)
                        .child("github.com/dlivxpr/lyrel"),
                )
                .into_any_element(),
        );
        Self::settings_page(groups)
    }

    fn render_settings_maintenance(&self, cx: &mut Context<Self>) -> AnyElement {
        let stale_daemon = crate::daemon::spawn::local_daemon_stale_build();
        // Whether picking up the new build costs the user their running panes
        // decides what this offer is, so it decides what it says.
        let stale_daemon_note = if crate::daemon::spawn::local_daemon_supports(
            crate::daemon::protocol::FEATURE_HANDOFF,
        ) {
            L10nKey::SettingsDaemonStaleDescInPlace
        } else {
            L10nKey::SettingsDaemonStaleDesc
        };
        let cfg = cx.global::<Config>();
        let check_for_updates = cfg.check_for_updates;
        let auto_download = cfg.auto_download_updates;
        let Some(http_proxy_input) = self.active_settings().map(|s| s.http_proxy_input.clone())
        else {
            return div().into_any_element();
        };
        // Only flags a committed value: the input commits on Enter/blur.
        let http_proxy_value = http_proxy_input.read(cx).value().trim().to_string();
        let http_proxy_error = (!http_proxy_value.is_empty()
            && !lyrel_core::daemon::install::proxy::is_valid_manual(&http_proxy_value))
        .then(|| t(L10nKey::SettingsAppHttpProxyInvalid).to_string());
        let http_proxy =
            self.settings_checked_input(&http_proxy_input, FIELD_W, http_proxy_error, cx);
        let check =
            self.settings_switch("check-updates", check_for_updates, cx, |this, on, _, cx| {
                this.set_check_for_updates(on, cx)
            });
        let auto = self.settings_switch(
            "auto-download-updates",
            auto_download,
            cx,
            |this, on, _, cx| this.set_auto_download_updates(on, cx),
        );
        let restart = self
            .settings_button(
                "restart-daemon",
                t(L10nKey::SettingsRestartServer),
                cx,
                |this, window, cx| this.restart_daemon(window, cx),
            )
            .into_any_element();
        // A stale server has a more specific thing to say than the row's
        // standing description, and it ends with the same button.
        let server_desc = match stale_daemon.as_deref() {
            Some(build) => format!(
                "{} {}",
                t_fmt(L10nKey::SettingsDaemonStale, &[("build", build)]),
                t(stale_daemon_note)
            ),
            None => t(L10nKey::SettingsServerDesc).to_string(),
        };
        self.settings_group(
            Some(t(L10nKey::SettingsUpdates)),
            None,
            [
                self.settings_row(
                    t(L10nKey::SettingsAutoDownload),
                    t(L10nKey::SettingsAutoDownloadDesc),
                    auto,
                    cx,
                ),
                self.settings_row(
                    t(L10nKey::SettingsCheckUpdatesOnLaunch),
                    t(L10nKey::SettingsCheckUpdatesDesc),
                    check,
                    cx,
                ),
                self.settings_row(
                    t(L10nKey::SettingsAppHttpProxy),
                    t(L10nKey::SettingsAppHttpProxyDesc),
                    http_proxy,
                    cx,
                ),
                self.settings_row(t(L10nKey::SettingsServer), server_desc, restart, cx),
            ]
            .map(IntoElement::into_any_element),
            cx,
        )
    }
}
