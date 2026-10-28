use super::*;
use crate::ui::presets::Fill;
use crate::ui::theme::apply_theme;
use crate::ui::theme::set_menus;
use gpui_kit::component::color_picker::ColorPickerEvent;

/// One settings session's editable theme and preview. Configuration application
/// stays on the existing window adapter; draft transitions never borrow it.
pub(crate) struct ThemeEditing {
    session: Uuid,
    editor_generation: u64,
    image_generation: u64,
    editor: Option<ThemeEditor>,
    draft: Option<(presets::Theme, presets::Theme)>,
    error: Option<String>,
}

struct ThemeEditor {
    seed: Vec<(ThemeEdit, Entity<ColorPickerState>)>,
    ansi: Vec<(ThemeEdit, Entity<ColorPickerState>)>,
    _subs: Vec<Subscription>,
}

#[derive(Clone)]
struct ThemeTarget {
    session: Uuid,
    generation: u64,
    id: String,
}

struct ImageSelection {
    target: ThemeTarget,
    generation: u64,
}

enum ThemeChange {
    Color(ThemeEdit, u32),
    Image(Option<std::path::PathBuf>),
    ImageOpacity(f32),
}

impl Default for ThemeEditing {
    fn default() -> Self {
        Self {
            session: Uuid::new_v4(),
            editor_generation: 0,
            image_generation: 0,
            editor: None,
            draft: None,
            error: None,
        }
    }
}

impl ThemeEditing {
    fn select_preset(config: &mut Config, id: &str, system_dark: bool) {
        if config.theme_follow_system {
            Self::select_slot(config, system_dark, id);
        } else {
            config.theme_preset = id.to_owned();
        }
    }

    fn select_slot(config: &mut Config, dark: bool, id: &str) {
        if dark {
            config.theme_preset_dark = id.to_owned();
        } else {
            config.theme_preset_light = id.to_owned();
        }
    }

    fn follow_system(config: &mut Config, manual_dark: bool) {
        config.theme_follow_system = true;
        let manual = config.theme_preset.clone();
        Self::select_slot(config, manual_dark, &manual);
    }

    fn stop_following_system(config: &mut Config, effective_id: String) {
        config.theme_follow_system = false;
        config.theme_preset = effective_id;
    }

    fn pin_mode(config: &mut Config, mode: ThemeMode) {
        config.theme_follow_system = false;
        config.theme_preset = if mode == ThemeMode::Dark {
            config.theme_preset_dark.clone()
        } else {
            config.theme_preset_light.clone()
        };
    }

    pub(super) fn is_dirty(&self) -> bool {
        self.draft.is_some()
    }

    pub(super) fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub(super) fn seed(&self) -> Option<&[(ThemeEdit, Entity<ColorPickerState>)]> {
        self.editor.as_ref().map(|editor| editor.seed.as_slice())
    }

    pub(super) fn ansi(&self) -> Option<&[(ThemeEdit, Entity<ColorPickerState>)]> {
        self.editor.as_ref().map(|editor| editor.ansi.as_slice())
    }

    fn preview(
        &mut self,
        mut theme: presets::Theme,
        change: ThemeChange,
    ) -> Option<presets::Theme> {
        if !theme.editable() {
            return None;
        }
        let original = self
            .draft
            .as_ref()
            .filter(|(original, _)| original.id == theme.id)
            .map(|(original, _)| original.clone())
            .unwrap_or_else(|| theme.clone());
        match change {
            ThemeChange::Color(edit, color) => match edit {
                ThemeEdit::Background => theme.background = Fill::Solid(color),
                ThemeEdit::Foreground => theme.foreground = color,
                ThemeEdit::Accent => theme.accent = color,
                ThemeEdit::Cursor => theme.caret = Some(color),
                ThemeEdit::Selection => theme.selection = Some(color),
                ThemeEdit::Ansi(i) => {
                    theme.ansi16[i] = ((color >> 16) as u8, (color >> 8) as u8, color as u8);
                }
            },
            ThemeChange::Image(path) => {
                let opacity = theme
                    .image
                    .as_ref()
                    .map(|image| image.opacity)
                    .unwrap_or(0.3);
                theme.image = path.map(|path| presets::Image { path, opacity });
            }
            ThemeChange::ImageOpacity(opacity) => {
                if let Some(image) = theme.image.as_mut() {
                    image.opacity = opacity.clamp(0.0, 1.0);
                }
            }
        }
        self.draft = (presets::to_yaml(&original) != presets::to_yaml(&theme))
            .then(|| (original, theme.clone()));
        Some(theme)
    }

    fn save(&mut self) -> std::io::Result<()> {
        let Some((_, draft)) = self.draft.as_ref() else {
            return Ok(());
        };
        if let Err(error) = presets::write_theme_file(draft) {
            self.error = Some(error.to_string());
            return Err(error);
        }
        self.draft = None;
        self.error = None;
        Ok(())
    }

    fn discard(&mut self) -> Option<presets::Theme> {
        self.leave_page();
        self.error = None;
        self.draft.take().map(|(original, _)| original)
    }

    /// Leaving the page invalidates a native image dialog, not the retained
    /// color controls, which are still used when returning to Appearance.
    pub(super) fn leave_page(&mut self) {
        self.image_generation = self.image_generation.wrapping_add(1);
    }

    fn rebuild(&mut self, id: &str) -> ThemeTarget {
        self.editor_generation = self.editor_generation.wrapping_add(1);
        self.leave_page();
        self.editor = None;
        ThemeTarget {
            session: self.session,
            generation: self.editor_generation,
            id: id.to_owned(),
        }
    }

    fn accepts(&self, target: &ThemeTarget, effective_id: &str) -> bool {
        self.session == target.session
            && self.editor_generation == target.generation
            && target.id == effective_id
    }

    fn begin_image_selection(&mut self, id: &str) -> ImageSelection {
        self.leave_page();
        ImageSelection {
            target: ThemeTarget {
                session: self.session,
                generation: self.editor_generation,
                id: id.to_owned(),
            },
            generation: self.image_generation,
        }
    }

    fn accepts_image(&self, selection: &ImageSelection, effective_id: &str) -> bool {
        self.accepts(&selection.target, effective_id)
            && self.image_generation == selection.generation
    }
}
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
        ThemeEditing::select_preset(cx.global_mut::<Config>(), id, dark);
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
        ThemeEditing::select_slot(cx.global_mut::<Config>(), dark_slot, id);
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
            let manual_dark = presets::by_id(cx, &cx.global::<Config>().theme_preset).dark;
            ThemeEditing::follow_system(cx.global_mut::<Config>(), manual_dark);
        } else {
            let effective = crate::ui::theme::effective_preset_id(cx);
            ThemeEditing::stop_following_system(cx.global_mut::<Config>(), effective);
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
                ThemeEditing::pin_mode(cx.global_mut::<Config>(), mode);
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
            ThemeEditing::select_slot(cx.global_mut::<Config>(), dark_slot, id);
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
        self.active_settings().is_some_and(|s| s.theme.is_dirty())
    }

    pub(crate) fn save_theme_draft(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(state) = self.active_settings_mut() else {
            return true;
        };
        if let Err(error) = state.theme.save() {
            crate::ui::host_ops::HostOps::notify_err(
                window,
                cx,
                t(L10nKey::ThemeSaveFailed),
                &error,
            );
            return false;
        }
        cx.notify();
        true
    }

    pub(crate) fn cancel_theme_draft(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(original) = self
            .active_settings_mut()
            .and_then(|state| state.theme.discard())
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

    fn preview_theme_change(
        &mut self,
        id: &str,
        change: ThemeChange,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let theme = crate::ui::presets::by_id(cx, id);
        let Some(theme) = self
            .active_settings_mut()
            .and_then(|state| state.theme.preview(theme, change))
        else {
            return;
        };
        let mut themes = crate::ui::presets::all(cx);
        if let Some(slot) = themes.iter_mut().find(|t| t.id == id) {
            *slot = theme;
        }
        cx.set_global(crate::ui::presets::Themes(themes));
        apply_theme(Some(window), cx);
        cx.notify();
    }

    fn edit_active_theme(
        &mut self,
        id: &str,
        edit: ThemeEdit,
        value: gpui_kit::Hsla,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.preview_theme_change(id, ThemeChange::Color(edit, hsla_to_u32(value)), window, cx);
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
        let id = crate::ui::theme::effective_preset_id(cx);
        let Some(selection) = self
            .active_settings_mut()
            .map(|state| state.theme.begin_image_selection(&id))
        else {
            return;
        };
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
                        this.apply_theme_image_selection(selection, path, window, cx);
                    });
                }
            }
        })
        .detach();
    }

    fn apply_theme_image_selection(
        &mut self,
        selection: ImageSelection,
        path: std::path::PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let id = crate::ui::theme::effective_preset_id(cx);
        if !self
            .active_settings()
            .is_some_and(|state| state.theme.accepts_image(&selection, &id))
        {
            return;
        }
        self.preview_theme_change(&id, ThemeChange::Image(Some(path)), window, cx);
        self.rebuild_theme_editor(window, cx);
    }

    pub(crate) fn remove_theme_image(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let id = crate::ui::theme::effective_preset_id(cx);
        self.preview_theme_change(&id, ThemeChange::Image(None), window, cx);
        self.rebuild_theme_editor(window, cx);
    }

    pub(crate) fn set_theme_image_opacity(
        &mut self,
        v: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let id = crate::ui::theme::effective_preset_id(cx);
        self.preview_theme_change(&id, ThemeChange::ImageOpacity(v), window, cx);
    }

    pub(crate) fn rebuild_theme_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let id = crate::ui::theme::effective_preset_id(cx);
        let Some(target) = self
            .active_settings_mut()
            .map(|state| state.theme.rebuild(&id))
        else {
            return;
        };
        let theme = crate::ui::presets::by_id(cx, &id);
        if !theme.editable() {
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
                let target = target.clone();
                subs.push(cx.subscribe_in(
                    &state,
                    window,
                    move |this, _picker, ev: &ColorPickerEvent, window, cx| {
                        let ColorPickerEvent::Change(Some(value)) = ev else {
                            return;
                        };
                        let id = crate::ui::theme::effective_preset_id(cx);
                        if this
                            .active_settings()
                            .is_some_and(|state| state.theme.accepts(&target, &id))
                        {
                            this.edit_active_theme(&id, edit, *value, window, cx);
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
            s.theme.editor = Some(ThemeEditor {
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

#[cfg(test)]
#[path = "theme_edit_tests.rs"]
mod consumer_tests;
