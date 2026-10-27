use super::*;
use crate::ui::presets::Fill;
use crate::ui::theme::apply_theme;
use crate::ui::theme::set_menus;
use gpui_kit::component::color_picker::ColorPickerEvent;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ThemeEdit {
    Background,
    Foreground,
    Accent,
    Cursor,
    Selection,
    Ansi(usize),
}

/// The 16 ANSI slots in the order a terminal numbers them: 0-7, then their
/// bright twins. A theme author needs to know that slot 9 is what `\e[91m`
/// paints — "Color 9" does not say that, and "Bright red" does.
const ANSI_COLOR_LABELS: [L10nKey; 16] = [
    L10nKey::AppThemeAnsiBlack,
    L10nKey::AppThemeAnsiRed,
    L10nKey::AppThemeAnsiGreen,
    L10nKey::AppThemeAnsiYellow,
    L10nKey::AppThemeAnsiBlue,
    L10nKey::AppThemeAnsiMagenta,
    L10nKey::AppThemeAnsiCyan,
    L10nKey::AppThemeAnsiWhite,
    L10nKey::AppThemeAnsiBrightBlack,
    L10nKey::AppThemeAnsiBrightRed,
    L10nKey::AppThemeAnsiBrightGreen,
    L10nKey::AppThemeAnsiBrightYellow,
    L10nKey::AppThemeAnsiBrightBlue,
    L10nKey::AppThemeAnsiBrightMagenta,
    L10nKey::AppThemeAnsiBrightCyan,
    L10nKey::AppThemeAnsiBrightWhite,
];

/// Looked up at render time, never stored: the editor outlives a language
/// change, and a label cached when it opened would stay in the old language.
pub(crate) fn theme_edit_label(edit: ThemeEdit) -> &'static str {
    t(match edit {
        ThemeEdit::Background => L10nKey::AppThemeColorBackground,
        ThemeEdit::Foreground => L10nKey::AppThemeColorForeground,
        ThemeEdit::Accent => L10nKey::AppThemeColorAccent,
        ThemeEdit::Cursor => L10nKey::AppThemeColorCursor,
        ThemeEdit::Selection => L10nKey::AppThemeColorSelection,
        ThemeEdit::Ansi(i) => ANSI_COLOR_LABELS[i.min(15)],
    })
}

fn hsla_to_u32(color: gpui_kit::Hsla) -> u32 {
    let rgba: gpui_kit::Rgba = color.into();
    let to = |f: f32| (f.clamp(0.0, 1.0) * 255.0).round() as u32;
    (to(rgba.r) << 16) | (to(rgba.g) << 8) | to(rgba.b)
}
/// The name the settings backdrop menu gives a material.
#[cfg(target_os = "windows")]
pub(crate) fn window_backdrop_label_key(backdrop: WindowBackdrop) -> L10nKey {
    match backdrop {
        WindowBackdrop::Auto => L10nKey::SettingsBackdropAuto,
        WindowBackdrop::Blur => L10nKey::SettingsBackdropBlur,
        WindowBackdrop::Mica => L10nKey::SettingsBackdropMica,
        WindowBackdrop::MicaAlt => L10nKey::SettingsBackdropMicaAlt,
        WindowBackdrop::Acrylic => L10nKey::SettingsBackdropAcrylic,
        WindowBackdrop::Off => L10nKey::SettingsBackdropOff,
    }
}
/// Clears the window overrides that are effective on the current platform.
/// `backdrop_is_local` models whether the Windows-only backdrop participates
/// in this platform's rendering and therefore belongs to its reset operation.
fn clear_window_override_values(config: &mut Config, backdrop_is_local: bool) {
    config.window_opacity = None;
    config.window_blur = None;
    if backdrop_is_local {
        config.window_backdrop = WindowBackdrop::Auto;
    }
}
impl SettingsWindow {
    pub(crate) fn set_preset(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.theme_draft_dirty() {
            let id = id.to_owned();
            self.with_settings_edits_resolved(window, cx, move |this, window, cx| {
                this.set_preset(&id, window, cx)
            });
            return;
        }
        let dark = crate::ui::theme::system_dark(cx);
        let cfg = cx.global_mut::<Config>();
        if !cfg.theme_follow_system {
            cfg.theme_preset = id.to_owned();
        } else if dark {
            cfg.theme_preset_dark = id.to_owned();
        } else {
            cfg.theme_preset_light = id.to_owned();
        }
        self.after_theme_change(window, cx);
    }
    pub(crate) fn set_slot_preset(
        &mut self,
        dark_slot: bool,
        id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.theme_draft_dirty() {
            let id = id.to_string();
            self.with_settings_edits_resolved(window, cx, move |this, window, cx| {
                this.set_slot_preset(dark_slot, &id, window, cx)
            });
            return;
        }
        let cfg = cx.global_mut::<Config>();
        if dark_slot {
            cfg.theme_preset_dark = id.to_string();
        } else {
            cfg.theme_preset_light = id.to_string();
        }
        self.after_theme_change(window, cx);
    }

    pub(crate) fn set_theme_follow_system(
        &mut self,
        on: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.theme_draft_dirty() {
            self.with_settings_edits_resolved(window, cx, move |this, window, cx| {
                this.set_theme_follow_system(on, window, cx)
            });
            return;
        }
        if on {
            let manual = cx.global::<Config>().theme_preset.clone();
            let manual_dark = crate::ui::presets::by_id(cx, &manual).dark;
            let cfg = cx.global_mut::<Config>();
            cfg.theme_follow_system = true;
            if manual_dark {
                cfg.theme_preset_dark = manual;
            } else {
                cfg.theme_preset_light = manual;
            }
        } else {
            let effective = crate::ui::theme::effective_preset_id(cx);
            let cfg = cx.global_mut::<Config>();
            cfg.theme_follow_system = false;
            cfg.theme_preset = effective;
        }
        self.after_theme_change(window, cx);
    }

    /// The Appearance page's three modes. System follows the OS between the
    /// light and the dark slot; Light and Dark pin the matching slot's theme.
    pub(crate) fn set_theme_mode(
        &mut self,
        mode: crate::ui::settings::ThemeMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use crate::ui::settings::ThemeMode;
        if self.theme_draft_dirty() {
            self.with_settings_edits_resolved(window, cx, move |this, window, cx| {
                this.set_theme_mode(mode, window, cx)
            });
            return;
        }
        match mode {
            ThemeMode::System => {
                if !cx.global::<Config>().theme_follow_system {
                    self.set_theme_follow_system(true, window, cx);
                }
            }
            ThemeMode::Light | ThemeMode::Dark => {
                let cfg = cx.global_mut::<Config>();
                cfg.theme_follow_system = false;
                cfg.theme_preset = if mode == ThemeMode::Dark {
                    cfg.theme_preset_dark.clone()
                } else {
                    cfg.theme_preset_light.clone()
                };
                self.after_theme_change(window, cx);
            }
        }
    }

    /// A pick from a theme slot's menu. Pinned to one mode, the pick is also
    /// what that mode's slot remembers, so System picks it back up.
    pub(crate) fn pick_slot_theme(
        &mut self,
        dark_slot: bool,
        id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if cx.global::<Config>().theme_follow_system {
            self.set_slot_preset(dark_slot, id, window, cx);
            return;
        }
        if !self.theme_draft_dirty() {
            let cfg = cx.global_mut::<Config>();
            if dark_slot {
                cfg.theme_preset_dark = id.to_string();
            } else {
                cfg.theme_preset_light = id.to_string();
            }
        }
        self.set_preset(id, window, cx);
    }

    pub(crate) fn set_theme_legible_palette(
        &mut self,
        on: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.global_mut::<Config>().theme_legible_palette = on;
        self.after_theme_change(window, cx);
    }

    fn after_theme_change(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.apply_theme_change(true, window, cx);
    }

    /// Repaints everything a theme change touches. `persist` is false for a
    /// palette preview, which repaints on every arrow key and must not turn
    /// each of those keystrokes into a `config.json` write.
    fn apply_theme_change(&mut self, persist: bool, window: &mut Window, cx: &mut Context<Self>) {
        apply_theme(Some(window), cx);
        set_menus(cx);
        if persist {
            self.persist_settings_config(cx);
        }
        self.rebuild_theme_editor(window, cx);
        cx.notify();
    }

    pub(crate) fn open_themes_folder(&self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dir) = crate::ui::presets::themes_dir() else {
            log::warn!("no config directory, so no themes folder to open");
            return;
        };
        // The folder is created on demand, so the first click on a fresh
        // install is also the one that can fail. Handing an absent path to the
        // file manager just opens nothing.
        if let Err(e) = std::fs::create_dir_all(&dir) {
            log::warn!("could not create {}: {e}", dir.display());
            crate::ui::host_ops::HostOps::notify_err(
                window,
                cx,
                &t_fmt(
                    L10nKey::OpenInFileManagerFailed,
                    &[("path", &dir.display().to_string())],
                ),
                &e,
            );
            return;
        }
        cx.open_with_system(&dir);
    }

    pub(crate) fn theme_draft_dirty(&self) -> bool {
        self.active_settings()
            .is_some_and(|s| s.theme_draft.is_some())
    }

    pub(crate) fn save_theme_draft(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some((_, draft)) = self.active_settings().and_then(|s| s.theme_draft.clone()) else {
            return true;
        };
        if let Err(error) = crate::ui::presets::write_theme_file(&draft) {
            if let Some(s) = self.active_settings_mut() {
                s.theme_draft_error = Some(error.to_string());
            }
            crate::ui::host_ops::HostOps::notify_err(
                window,
                cx,
                t(L10nKey::ThemeSaveFailed),
                &error,
            );
            return false;
        }
        if let Some(s) = self.active_settings_mut() {
            s.theme_draft = None;
            s.theme_draft_error = None;
        }
        cx.notify();
        true
    }

    pub(crate) fn cancel_theme_draft(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(s) = self.active_settings_mut() {
            s.theme_draft_error = None;
        }
        if let Some((original, _)) = self
            .active_settings_mut()
            .and_then(|s| s.theme_draft.take())
        {
            let mut themes = crate::ui::presets::all(cx);
            if let Some(slot) = themes.iter_mut().find(|t| t.id == original.id) {
                *slot = original;
            }
            cx.set_global(crate::ui::presets::Themes(themes));
            apply_theme(Some(window), cx);
            self.rebuild_theme_editor(window, cx);
            cx.notify();
        }
    }

    fn mutate_active_theme(
        &mut self,
        mutate: impl FnOnce(&mut crate::ui::presets::Theme),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let id = crate::ui::theme::effective_preset_id(cx);
        let mut theme = crate::ui::presets::by_id(cx, &id);
        if !theme.editable() {
            return;
        }
        mutate(&mut theme);
        if let Some(state) = self.active_settings_mut() {
            let original = state
                .theme_draft
                .as_ref()
                .filter(|(original, _)| original.id == id)
                .map(|(original, _)| original.clone())
                .unwrap_or_else(|| crate::ui::presets::by_id(cx, &id));
            if crate::ui::presets::to_yaml(&original) == crate::ui::presets::to_yaml(&theme) {
                state.theme_draft = None;
            } else {
                state.theme_draft = Some((original, theme.clone()));
            }
            let mut themes = crate::ui::presets::all(cx);
            if let Some(slot) = themes.iter_mut().find(|t| t.id == id) {
                *slot = theme;
            }
            cx.set_global(crate::ui::presets::Themes(themes));
        } else {
            if let Err(error) = crate::ui::presets::write_theme_file(&theme) {
                crate::ui::host_ops::HostOps::notify_err(
                    window,
                    cx,
                    t(L10nKey::ThemeSaveFailed),
                    &error,
                );
                return;
            }
            crate::ui::presets::load_registry(cx);
        }
        apply_theme(Some(window), cx);
        cx.notify();
    }

    pub(crate) fn edit_active_theme(
        &mut self,
        edit: ThemeEdit,
        value: gpui_kit::Hsla,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let c = hsla_to_u32(value);
        self.mutate_active_theme(
            |theme| match edit {
                ThemeEdit::Background => theme.background = Fill::Solid(c),
                ThemeEdit::Foreground => theme.foreground = c,
                ThemeEdit::Accent => theme.accent = c,
                ThemeEdit::Cursor => theme.caret = Some(c),
                ThemeEdit::Selection => theme.selection = Some(c),
                ThemeEdit::Ansi(i) => theme.ansi16[i] = ((c >> 16) as u8, (c >> 8) as u8, c as u8),
            },
            window,
            cx,
        );
    }

    pub(crate) fn effective_window_opacity(cx: &App) -> f32 {
        let config = cx.global::<Config>();
        let theme = crate::ui::presets::by_id(cx, &crate::ui::theme::effective_preset_id(cx));
        let blur = config.window_blur.unwrap_or(theme.blur);
        config.window_opacity.or(theme.opacity).unwrap_or_else(|| {
            crate::ui::theme::default_window_opacity(config.window_backdrop, blur)
        })
    }

    pub(crate) fn set_window_blur(
        &mut self,
        on: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.global_mut::<Config>().window_blur = Some(on);
        apply_theme(Some(window), cx);
        self.persist_settings_config(cx);
        cx.notify();
    }

    #[cfg(target_os = "windows")]
    pub(crate) fn set_window_backdrop(
        &mut self,
        backdrop: WindowBackdrop,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.global_mut::<Config>().window_backdrop = backdrop;
        apply_theme(Some(window), cx);
        self.persist_settings_config(cx);
        cx.notify();
    }

    pub(crate) fn reset_window_overrides(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        {
            let config = cx.global_mut::<Config>();
            clear_window_override_values(config, cfg!(target_os = "windows"));
        }
        apply_theme(Some(window), cx);
        self.persist_settings_config(cx);
        cx.notify();
    }

    pub(crate) fn pick_theme_image(&mut self, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(gpui_kit::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: None,
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await {
                if let Some(path) = paths.into_iter().next() {
                    let _ = this.update_in(cx, |this, window, cx| {
                        this.mutate_active_theme(
                            |theme| {
                                let opacity =
                                    theme.image.as_ref().map(|i| i.opacity).unwrap_or(0.3);
                                theme.image = Some(crate::ui::presets::Image { path, opacity });
                            },
                            window,
                            cx,
                        );
                        this.rebuild_theme_editor(window, cx);
                    });
                }
            }
        })
        .detach();
    }

    pub(crate) fn remove_theme_image(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.mutate_active_theme(|theme| theme.image = None, window, cx);
        self.rebuild_theme_editor(window, cx);
    }

    pub(crate) fn set_theme_image_opacity(
        &mut self,
        v: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.mutate_active_theme(
            |theme| {
                if let Some(img) = theme.image.as_mut() {
                    img.opacity = v.clamp(0.0, 1.0);
                }
            },
            window,
            cx,
        );
    }

    pub(crate) fn rebuild_theme_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings.is_none() {
            return;
        }
        let id = crate::ui::theme::effective_preset_id(cx);
        let theme = crate::ui::presets::by_id(cx, &id);
        if !theme.editable() {
            if let Some(s) = self.settings.as_mut() {
                s.theme_editor = None;
            }
            return;
        }

        let neutrals = theme.neutrals();
        let seed_specs: [(ThemeEdit, u32); 5] = [
            (ThemeEdit::Background, theme.background_color()),
            (ThemeEdit::Foreground, theme.foreground),
            (ThemeEdit::Accent, theme.accent),
            (ThemeEdit::Cursor, theme.caret.unwrap_or(theme.accent)),
            (ThemeEdit::Selection, neutrals.selection),
        ];

        let mut subs = Vec::new();
        let mut make =
            |edit: ThemeEdit, value: u32, subs: &mut Vec<Subscription>, cx: &mut Context<Self>| {
                let eff: gpui_kit::Hsla = gpui_kit::rgb(value).into();
                let state = cx.new(|cx| ColorPickerState::new(window, cx).default_value(eff));
                subs.push(cx.subscribe_in(
                    &state,
                    window,
                    move |this, _picker, ev: &ColorPickerEvent, window, cx| {
                        let ColorPickerEvent::Change(value) = ev;
                        if let Some(v) = value {
                            this.edit_active_theme(edit, *v, window, cx);
                        }
                    },
                ));
                state
            };

        let seed = seed_specs
            .iter()
            .map(|&(edit, value)| (edit, make(edit, value, &mut subs, cx)))
            .collect();
        let ansi = (0..16)
            .map(|i| {
                let (r, g, b) = theme.ansi16[i];
                let value = (r as u32) << 16 | (g as u32) << 8 | b as u32;
                (
                    ThemeEdit::Ansi(i),
                    make(ThemeEdit::Ansi(i), value, &mut subs, cx),
                )
            })
            .collect();

        if let Some(s) = self.settings.as_mut() {
            s.theme_editor = Some(ThemeEditor {
                seed,
                ansi,
                _subs: subs,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::clear_window_override_values;

    #[test]
    fn non_windows_reset_preserves_the_synced_windows_backdrop() {
        let mut config = crate::core::config::Config::default();
        config.window_opacity = Some(0.8);
        config.window_blur = Some(true);
        config.window_backdrop = crate::core::config::WindowBackdrop::Mica;

        clear_window_override_values(&mut config, false);

        assert_eq!(config.window_opacity, None);
        assert_eq!(config.window_blur, None);
        assert_eq!(
            config.window_backdrop,
            crate::core::config::WindowBackdrop::Mica,
            "an inert synchronized backdrop is not a local override to reset"
        );
    }

    #[test]
    fn windows_reset_clears_the_local_backdrop_override() {
        let mut config = crate::core::config::Config::default();
        config.window_backdrop = crate::core::config::WindowBackdrop::Acrylic;

        clear_window_override_values(&mut config, true);

        assert_eq!(
            config.window_backdrop,
            crate::core::config::WindowBackdrop::Auto
        );
    }
}
