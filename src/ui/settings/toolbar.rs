use super::*;
use crate::ui::caffeine::CaffeineState;
use gpui_kit::component::switch::Switch;
use lyrel_core::core::config::ToolbarVisibility;

impl SettingsWindow {
    pub(super) fn render_settings_toolbar(&self, cx: &mut Context<Self>) -> AnyElement {
        use L10nKey::*;
        let title_bar = self.toolbar_group(
            ToolbarTitleBar,
            &[
                ("toolbar-cpu", ToolbarCpu, |toolbar| &mut toolbar.cpu),
                ("toolbar-ram", ToolbarRam, |toolbar| &mut toolbar.ram),
                ("toolbar-gpu", ToolbarGpu, |toolbar| &mut toolbar.gpu),
            ],
            cx,
        );
        let status_bar = self.toolbar_group(
            ToolbarStatusBar,
            &[
                ("toolbar-settings", ToolbarSettingsEntry, |toolbar| {
                    &mut toolbar.settings
                }),
                ("toolbar-host", ToolbarHost, |toolbar| &mut toolbar.host),
                ("toolbar-path", ToolbarPath, |toolbar| &mut toolbar.path),
                ("toolbar-git", ToolbarGit, |toolbar| &mut toolbar.git),
                ("toolbar-running", ToolbarRunning, |toolbar| {
                    &mut toolbar.running
                }),
                ("toolbar-caffeine", StatusCaffeineTitle, |toolbar| {
                    &mut toolbar.caffeine
                }),
                ("toolbar-clock", ToolbarClock, |toolbar| &mut toolbar.clock),
                ("toolbar-notifications", ToolbarNotifications, |toolbar| {
                    &mut toolbar.notifications
                }),
            ],
            cx,
        );
        use gpui_kit::component::ActiveTheme as _;
        v_flex()
            .gap_4()
            .child(title_bar)
            .child(div().border_t_1().border_color(cx.theme().border))
            .child(status_bar)
            .into_any_element()
    }

    fn toolbar_group(
        &self,
        title: L10nKey,
        settings: &[ToolbarSetting],
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut toolbar = cx.global::<Config>().toolbar;
        let rows: Vec<_> = settings
            .iter()
            .map(|&(id, title, field)| {
                let is_caffeine = title == L10nKey::StatusCaffeineTitle;
                let control = Switch::new(id)
                    .small()
                    .checked(*field(&mut toolbar))
                    .accessibility_label(t(title))
                    .on_change(cx.listener(move |this, on: &bool, window, cx| {
                        this.update_config(cx, |cfg| *field(&mut cfg.toolbar) = *on);
                        if is_caffeine && !on {
                            cx.default_global::<CaffeineState>();
                            CaffeineState::reset(cx);
                            if let Some(error) = cx.global::<CaffeineState>().last_error() {
                                let message =
                                    t_fmt(L10nKey::StatusCaffeineFooterError, &[("error", error)]);
                                window.notify_app(Notification::error(message), cx);
                            }
                        }
                        cx.refresh_windows();
                    }))
                    .into_any_element();
                let desc = if is_caffeine {
                    t(L10nKey::ToolbarCaffeineDescription)
                } else {
                    ""
                };
                self.settings_row(t(title), desc, control, cx)
                    .min_h(gpui_kit::rems(1.875))
                    .py_1()
                    .into_any_element()
            })
            .collect();
        self.settings_group(Some(t(title)), None, rows, cx)
    }
}

type ToolbarSetting = (
    &'static str,
    L10nKey,
    fn(&mut ToolbarVisibility) -> &mut bool,
);
