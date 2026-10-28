//! SettingsWindow 的 SSH GPUI 适配；跨页确认和配置应用仍由窗口统一协调。
use super::*;

impl SettingsWindow {
    pub(super) fn live_ssh_profiles(&self, cx: &App) -> std::collections::HashSet<Uuid> {
        (self.workspace.live_profiles)(cx)
    }

    pub(super) fn toggle_ssh_group(&mut self, key: String, cx: &mut Context<Self>) {
        if let Some(s) = self.active_settings_mut() {
            s.ssh.toggle_group(key);
        }
        // Collapsing the list never discards the profile being edited.
        cx.notify();
    }

    pub(super) fn ssh_form_mut(&mut self) -> Option<&mut SshProfileForm> {
        self.active_settings_mut().and_then(|s| s.ssh.form.as_mut())
    }

    pub(crate) fn ssh_form_load(
        &mut self,
        profile: &SshProfile,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.ssh_form_dirty(cx) {
            let profile = profile.clone();
            self.with_settings_edits_resolved(window, cx, move |this, window, cx| {
                this.ssh_form_load(&profile, window, cx)
            });
            return;
        }
        if let Some(state) = self.settings.as_mut() {
            state.ssh.load(profile, window, cx);
        }
        cx.notify();
    }

    pub(super) fn ssh_form_collect(&self, cx: &App) -> Option<(SshProfile, SshFormErrors)> {
        self.active_settings()?
            .ssh
            .collect(&cx.global::<Config>().ssh_profiles, cx)
    }

    pub(super) fn resync_key_passphrase(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(state) = self.settings.as_mut() {
            state.ssh.resync_key_passphrase(window, cx);
        }
    }

    pub(crate) fn save_editing_profile(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Uuid> {
        let profile = self
            .active_settings()?
            .ssh
            .profile_to_save(&cx.global::<Config>().ssh_profiles, cx)?;
        let id = profile.id;
        self.update_config(cx, |cfg| SshSettings::apply_profile(cfg, &profile));
        if self
            .active_settings()
            .is_some_and(|s| s.save_error.is_some())
        {
            return None;
        }
        let failures =
            self.settings
                .as_mut()?
                .ssh
                .save_secrets(&profile, cx.global::<Config>(), cx);
        for line in failures {
            window.notify_app(line, cx);
        }
        Some(id)
    }

    pub(crate) fn cancel_ssh_form(&mut self, cx: &mut Context<Self>) {
        if let Some(state) = self.active_settings_mut() {
            state.ssh.cancel();
        }
        cx.notify();
    }

    pub(crate) fn ssh_form_dirty(&self, cx: &App) -> bool {
        self.active_settings()
            .is_some_and(|state| state.ssh.is_dirty(&cx.global::<Config>().ssh_profiles, cx))
    }

    pub(crate) fn add_new_profile(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let profile = SshProfile::new(String::new());
        self.ssh_form_load(&profile, window, cx);
        // A blank form is there to be typed into: land on its first field
        // rather than leave the keystrokes with nothing to go to. (A form held
        // back behind an unsaved-edits prompt has not opened yet, and is not
        // this profile's.)
        let first = self
            .ssh_form_mut()
            .filter(|form| form.editing == profile.id)
            .map(|form| form.name.clone());
        if let Some(first) = first {
            first.update(cx, |state, cx| state.focus(window, cx));
        }
    }

    pub(super) fn delete_profile_confirmed(&mut self, id: Uuid, cx: &mut Context<Self>) {
        if let Some(state) = self.active_settings() {
            state.ssh.forget_removed_profile(cx.global::<Config>(), id);
        }

        // Forget the entries that routed through this profile (#485) —
        // forgotten, not deleted: `forget_workspace` never sends
        // `WorkspaceRemove`, so the remote sessions keep running and a new
        // profile to the same machine rediscovers them. Recomputed here
        // rather than carried from the prompt: the set can only have shrunk
        // (a new live link) while the dialog was up.
        let cascade = crate::ui::windows::cascade_for_profile(cx, id);
        for workspace in cascade {
            crate::ui::windows::forget_workspace(cx, workspace);
        }
        self.update_config(cx, |cfg| SshSettings::remove_profile(cfg, id));
        if let Some(s) = self.active_settings_mut() {
            s.ssh.removed(id);
        }
        cx.notify();
    }

    /// Import `~/.ssh/config`, and say what that did.
    ///
    /// Every branch here ends in a notification because every branch used to
    /// end in nothing: a missing file, a file of nothing but `Host *`, and a
    /// clean import of six hosts were all the same silent button press, and the
    /// only way to tell them apart was to go count the host list.
    pub(crate) fn import_ssh_config_profiles(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // One id for all three outcomes, so pressing the button again replaces
        // what it said last time instead of stacking a second toast on top of
        // an answer that is now out of date.
        const NOTIFICATION: &str = "ssh-config-import";
        // A toast is 448pt wide. A config with a dozen unsupported keywords in
        // it would push the counts out of view, so the notification names the
        // first few and the log line below carries the whole list, with the
        // hosts each keyword was set on.
        const OPTIONS_SHOWN: usize = 5;

        let report = crate::core::ssh_config::import_report();
        let source = report.source.display().to_string();

        if !report.source_read {
            window.notify_app(
                Notification::error(t_fmt(
                    L10nKey::SettingsImportUnreadable,
                    &[("path", &source)],
                ))
                .id1::<Self>(NOTIFICATION),
                cx,
            );
            return;
        }
        if report.profiles.is_empty() {
            window.notify_app(
                Notification::warning(t_fmt(L10nKey::SettingsImportNoHosts, &[("path", &source)]))
                    .id1::<Self>(NOTIFICATION),
                cx,
            );
            return;
        }

        let read = report.profiles.len();
        let ignored = report.ignored;
        let mut stats = crate::core::ssh_config::MergeStats::default();
        self.update_config(cx, |cfg| {
            stats = SshSettings::import_profiles(cfg, report.profiles);
        });

        let dropped: Vec<String> = ignored
            .iter()
            .map(|opt| format!("{} ({})", opt.option, opt.hosts.join(", ")))
            .collect();
        log::info!(
            "imported {read} alias(es) from {source} ({} file(s) read): {} added, {} updated, \
         {} unchanged; no lyrel setting for: [{}]",
            report.files_read,
            stats.added,
            stats.updated,
            stats.unchanged,
            dropped.join("; ")
        );

        let mut notification = Notification::new()
            .with_type(NotificationType::Success)
            .title(t_plural(
                L10nKey::SettingsImportSummary,
                stats.added,
                &[
                    ("updated", &stats.updated.to_string()),
                    ("unchanged", &stats.unchanged.to_string()),
                ],
            ))
            .id1::<Self>(NOTIFICATION);
        if !ignored.is_empty() {
            let mut options: Vec<String> = ignored
                .iter()
                .take(OPTIONS_SHOWN)
                .map(|opt| opt.option.clone())
                .collect();
            let rest = ignored.len() - options.len();
            if rest > 0 {
                options.push(t_fmt(
                    L10nKey::SettingsImportMoreOptions,
                    &[("count", &rest.to_string())],
                ));
            }
            notification = notification
                .message(t_plural(
                    L10nKey::SettingsImportIgnored,
                    ignored.len(),
                    &[("options", &options.join(", "))],
                ))
                // A list of what the import could not carry is something to
                // read and act on, and four seconds is not long enough to do
                // either. The counts alone still fade on their own.
                .autohide(false);
        }
        window.notify_app(notification, cx);
    }
}
