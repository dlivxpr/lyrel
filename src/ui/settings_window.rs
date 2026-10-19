//! Settings in a window of its own.
//!
//! The settings page used to cover the workspace it was opened from, which hid
//! the terminal a setting was being tried out on. It now opens beside it. The
//! state and the page itself still belong to the workspace's `LyrelApp` — this
//! window only borrows it to draw — so every setting keeps the one code path it
//! had, and a change repaints both windows.

use gpui_kit::component::{ActiveTheme as _, TitleBar};
use gpui_kit::{
    App, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _, Render,
    Styled as _, Subscription, TitlebarOptions, WeakEntity, Window, WindowBounds,
    WindowDecorations, WindowOptions, prelude::FluentBuilder as _, px, size,
};

use crate::core::actions::{CloseActiveTab, OpenSettings};
use crate::core::config::Config;
use crate::ui::app::LyrelApp;

/// Big enough for the navigation column and a comfortable page beside it.
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
            // Not drawn (the title bar is the window's own), but it is the
            // name the Window menu, Mission Control and VoiceOver give it.
            title: Some(crate::ui::i18n::t(crate::ui::i18n::L10nKey::SettingsWindowTitle).into()),
            traffic_light_position: Some(crate::ui::theme::traffic_light_position()),
            ..TitleBar::title_bar_options()
        }),
        window_decorations: Some(WindowDecorations::Client),
        window_background: crate::ui::theme::background_appearance(cx),
        window_min_size: Some(size(px(MIN_SIZE.0), px(MIN_SIZE.1))),
        ..Default::default()
    }
}

pub(crate) struct SettingsWindow {
    pub(crate) app: WeakEntity<LyrelApp>,
    title: &'static str,
    _observe: Subscription,
    _release: Subscription,
    _activation_watch: Subscription,
    _bounds_watch: Subscription,
}

impl SettingsWindow {
    pub(crate) fn new(app: &Entity<LyrelApp>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        // Every settings change notifies the app, not this view; without this
        // the page would change underneath and never repaint here.
        let title = crate::ui::i18n::t(crate::ui::i18n::L10nKey::SettingsWindowTitle);
        let observe = cx.observe_in(app, window, |this, _, window, cx| {
            let title = crate::ui::i18n::t(crate::ui::i18n::L10nKey::SettingsWindowTitle);
            if this.title != title {
                window.set_window_title(title);
                this.title = title;
            }
            cx.notify();
        });
        // The workspace that owns the page is gone, so the page is too.
        let release = cx.observe_release_in(app, window, |_, _, window, _| window.remove_window());
        // This window carries the same app-owned material as a workspace
        // window, and DWM can drop it the same way — replay it on activation
        // changes and on DPI changes, exactly like the workspace window does.
        // The settings window's own open path primed it via
        // `sync_window_backdrop`.
        let activation_watch = cx.observe_window_activation(window, |_, window, cx| {
            crate::ui::theme::replay_window_backdrop(window, cx);
        });
        let bounds_watch = cx.observe_window_bounds(window, |_, window, cx| {
            crate::ui::theme::replay_window_backdrop_on_scale_change(window, cx);
        });
        // The close button asks the same question Escape does: a half-edited
        // form or theme draft gets its prompt, and the window only goes once
        // it is answered.
        let weak = app.downgrade();
        window.on_window_should_close(cx, move |window, cx| match weak.upgrade() {
            Some(app) => {
                app.update(cx, |this, cx| this.close_settings_checked(window, cx));
                false
            }
            None => true,
        });
        Self {
            app: app.downgrade(),
            title,
            _observe: observe,
            _release: release,
            _activation_watch: activation_watch,
            _bounds_watch: bounds_watch,
        }
    }

    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(app) = self.app.upgrade() {
            app.update(cx, |this, cx| this.close_settings_checked(window, cx));
        }
    }
}

impl Render for SettingsWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // The workspace window sets this in its own render; a second window
        // has to, or the page lays out against the default 16px rem.
        window.set_rem_size(px(cx.global::<Config>().ui_font_size));
        crate::ui::settings::kit::note_scale(window);
        let page = self.app.upgrade().map(|app| {
            let page = app.update(cx, |this, cx| {
                // The state can be gone for a frame between closing and the
                // window being removed.
                this.has_settings()
                    .then(|| this.render_settings(window, cx).into_any_element())
            });
            if page.is_none() {
                // Closed without passing through `close_settings` — nothing
                // is left to draw, so the window goes too.
                app.update(cx, |this, cx| this.forget_settings_window(cx));
                window.remove_window();
            }
            page
        });
        gpui_kit::div()
            .size_full()
            .bg(crate::ui::theme::overlay_background(cx))
            .text_color(cx.theme().foreground)
            // ⌘W closes this window the way it closes a tab in a workspace.
            .on_action(cx.listener(|this, _: &CloseActiveTab, window, cx| this.close(window, cx)))
            // ⌘, lands here while this window has focus; it is already open.
            .on_action(cx.listener(|_, _: &OpenSettings, window, _| window.activate_window()))
            .when_some(page.flatten(), |root, page| root.child(page))
    }
}
