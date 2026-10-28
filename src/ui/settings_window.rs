//! 设置窗口独立持有输入、草稿、快捷键录制与订阅；工作区只接收明确的操作事件。
use crate::core::actions::{CloseActiveTab, OpenSettings};
use crate::core::config::Config;
use crate::ui::settings::{SettingsSection, SettingsState};
use gpui_kit::component::{ActiveTheme as _, TitleBar};
use gpui_kit::{
    App, Context, EventEmitter, InteractiveElement as _, IntoElement, ParentElement as _, Render,
    Styled as _, Subscription, TitlebarOptions, Window, WindowBounds, WindowDecorations,
    WindowOptions, px, size,
};
use std::cell::Cell;
use std::rc::Rc;

pub(crate) type SettingsLocation = (SettingsSection, gpui_kit::Point<gpui_kit::Pixels>);
pub(crate) type SessionSources = Vec<(
    String,
    lyrel_core::host::session_operations::SessionOperation,
)>;

/// 只读工作区资料按需获取，避免复制 pane 树，也不向设置模块暴露工作区实体。
pub(crate) struct SettingsContext {
    pub owner: gpui_kit::AnyWindowHandle,
    pub shells: Rc<dyn Fn(&App) -> Vec<crate::core::shells::DetectedShell>>,
    pub live_profiles: Rc<dyn Fn(&App) -> std::collections::HashSet<uuid::Uuid>>,
    pub session_sources: Rc<dyn Fn(crate::ui::host_ops::HostId, &App) -> SessionSources>,
}

#[derive(Clone)]
pub(crate) enum SettingsEvent {
    Closed(SettingsLocation),
    ConnectSsh(uuid::Uuid),
    OpenFile(crate::ui::host_ops::SharedHost, std::path::PathBuf),
    RestartDaemon,
    RefreshShells,
    TerminalCursorChanged { force_visible: bool },
    EditorGitGutter(bool),
    LocaleChanged,
}

const DEFAULT_SIZE: (f32, f32) = (960., 700.);
const MIN_SIZE: (f32, f32) = (720., 480.);
pub(crate) fn window_options(cx: &mut App) -> WindowOptions {
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(gpui_kit::Bounds::centered(
            None,
            size(px(DEFAULT_SIZE.0), px(DEFAULT_SIZE.1)),
            cx,
        ))),
        app_id: Some("io.github.dlivxpr.lyrel".to_owned()),
        titlebar: Some(TitlebarOptions {
            title: Some(crate::ui::i18n::t(crate::ui::i18n::L10nKey::SettingsWindowTitle).into()),
            traffic_light_position: Some(crate::ui::window_chrome::traffic_light_position()),
            ..TitleBar::title_bar_options()
        }),
        window_decorations: Some(WindowDecorations::Client),
        window_background: crate::ui::theme::background_appearance(cx),
        window_min_size: Some(size(px(MIN_SIZE.0), px(MIN_SIZE.1))),
        ..Default::default()
    }
}

pub(crate) struct SettingsWindow {
    pub(crate) workspace: SettingsContext,
    pub(crate) settings: Option<SettingsState>,
    pub(crate) settings_row_width: Cell<f32>,
    pub(crate) settings_viewport_w: Cell<f32>,
    pub(crate) last_settings_location: SettingsLocation,
    title: &'static str,
    pub(crate) window: gpui_kit::AnyWindowHandle,
    pub(crate) subscriptions: Vec<Subscription>,
}
impl EventEmitter<SettingsEvent> for SettingsWindow {}

impl SettingsWindow {
    pub(crate) fn new(
        workspace: SettingsContext,
        location: SettingsLocation,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let title = crate::ui::i18n::t(crate::ui::i18n::L10nKey::SettingsWindowTitle);
        let config_watch = cx.observe_global_in::<Config>(window, |this, window, cx| {
            let title = crate::ui::i18n::t(crate::ui::i18n::L10nKey::SettingsWindowTitle);
            if this.title != title {
                window.set_window_title(title);
                this.title = title;
                this.refresh_locale_state(window, cx);
            }
            cx.notify();
        });
        let activation_watch = cx.observe_window_activation(window, |_, window, cx| {
            crate::ui::theme::replay_window_backdrop(window, cx);
        });
        let bounds_watch = cx.observe_window_bounds(window, |_, window, cx| {
            crate::ui::theme::replay_window_backdrop_on_scale_change(window, cx);
        });
        if window.window_handle() != workspace.owner {
            let weak = cx.weak_entity();
            window.on_window_should_close(cx, move |window, cx| {
                weak.update(cx, |this, cx| this.close_settings_checked(window, cx))
                    .is_err()
            });
        }
        let mut this = Self {
            workspace,
            settings: None,
            settings_row_width: Cell::new(f32::MAX),
            settings_viewport_w: Cell::new(f32::MAX),
            last_settings_location: location,
            title,
            window: window.window_handle(),
            subscriptions: vec![config_watch, activation_watch, bounds_watch],
        };
        this.build_settings_state(window, cx);
        this
    }

    pub(crate) fn owner(&self) -> gpui_kit::AnyWindowHandle {
        self.workspace.owner
    }
    #[cfg(test)]
    pub(crate) fn has_settings(&self) -> bool {
        self.settings.is_some()
    }
    pub(crate) fn recording_shortcut(&self) -> bool {
        self.settings
            .as_ref()
            .is_some_and(|s| s.shortcuts.is_recording())
    }
    pub(crate) fn close_settings(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(mut state) = self.settings.take() {
            state.shortcuts.retire();
            state
                .usage
                .update(cx, |page, cx| page.set_visible(false, cx));
            self.last_settings_location = state.navigation.location();
            cx.emit(SettingsEvent::Closed(self.last_settings_location));
            cx.notify();
        }
    }
    #[cfg(test)]
    pub(crate) fn open_settings_section(
        &mut self,
        section: SettingsSection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.settings.is_none() {
            self.build_settings_state(window, cx);
        }
        self.navigate_settings(section, None, window, cx);
    }
    pub(crate) fn refresh_locale_state(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(s) = self.active_settings() {
            use crate::ui::i18n::{L10nKey, t};
            for (input, key) in [
                (s.navigation.search(), L10nKey::SearchSettings),
                (s.ssh.filter(), L10nKey::FilterHosts),
                (s.shortcuts.search(), L10nKey::SettingsSearchShortcuts),
                (&s.agent_query, L10nKey::SettingsSearchAgents),
                (&s.shell_args_input, L10nKey::AppPlaceholderNone),
                (
                    &s.link_file_command_input,
                    L10nKey::AppPlaceholderOpenInDefaultApp,
                ),
            ] {
                input.update(cx, |state, cx| state.set_placeholder(t(key), window, cx));
            }
            if !cfg!(windows) {
                s.shell_program_input.update(cx, |state, cx| {
                    state.set_placeholder(t(L10nKey::AppPlaceholderLoginShell), window, cx)
                });
            }
            s.skills.refresh_locale(window, cx);
        }
        cx.notify();
    }
}
impl Render for SettingsWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        window.set_rem_size(px(cx.global::<Config>().ui_font_size));
        crate::ui::settings::kit::note_scale(window);
        let page = self.render_settings(window, cx);
        gpui_kit::div()
            .size_full()
            .bg(crate::ui::theme::overlay_background(cx))
            .text_color(cx.theme().foreground)
            .on_action(cx.listener(|this, _: &CloseActiveTab, window, cx| {
                this.close_settings_checked(window, cx)
            }))
            .on_action(cx.listener(|_, _: &OpenSettings, window, _| window.activate_window()))
            .child(page)
    }
}
