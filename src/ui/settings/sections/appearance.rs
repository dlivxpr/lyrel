//! The text and window halves of the Appearance page: font family and size
//! rows, opacity, blur and the Windows backdrop, with the font/stepper
//! builders these rows share.

use std::rc::Rc;

use super::kit::{self, BtnKind, Stepper, Tk};
use super::shell::{MenuEntry, SearchOption};
use super::*;

impl SettingsWindow {
    /// A font menu over every installed family. `default_label` names the
    /// first row for the menus whose empty value means "follow something
    /// else".
    fn font_dropdown(
        &self,
        id: &'static str,
        current: Option<&str>,
        default_label: Option<&'static str>,
        cx: &mut Context<Self>,
        commit: impl Fn(&mut Self, String, &mut Window, &mut Context<Self>) + 'static,
    ) -> AnyElement {
        let names = self
            .active_settings()
            .map(|s| s.font_names.clone())
            .unwrap_or_default();
        let mut options: Vec<SearchOption> = Vec::with_capacity(names.len() + 1);
        let mut values: Vec<String> = Vec::with_capacity(names.len() + 1);
        if let Some(label) = default_label {
            options.push(SearchOption {
                label: label.into(),
                font: None,
            });
            values.push(label.to_string());
        }
        for name in names.iter() {
            options.push(SearchOption {
                label: name.clone().into(),
                font: Some(name.clone().into()),
            });
            values.push(name.clone());
        }
        let selected = match current {
            Some(name) => values.iter().position(|v| v == name),
            None => default_label.map(|_| 0),
        };
        let label = current
            .map(str::to_string)
            .or(default_label.map(str::to_string))
            .unwrap_or_default();
        let values = Rc::new(values);
        self.settings_search_dropdown(
            id,
            label,
            Rc::new(options),
            selected,
            Rc::new(move |this, ix, window, cx| {
                if let Some(value) = values.get(ix) {
                    commit(this, value.clone(), window, cx);
                }
            }),
            cx,
        )
    }

    fn stepper_control(
        &self,
        id: &'static str,
        value: String,
        unit: &str,
        (can_dec, can_inc): (bool, bool),
        cx: &mut Context<Self>,
        step: impl Fn(&mut Self, f32, &mut Context<Self>) + 'static,
    ) -> AnyElement {
        let tk = Tk::of(cx);
        let step = Rc::new(step);
        let (dec, inc) = (step.clone(), step);
        kit::stepper(
            id,
            Stepper {
                value,
                unit: unit.to_string(),
                can_dec,
                can_inc,
            },
            &tk,
            cx.listener(move |this, _, _w, cx| dec(this, -1., cx)),
            cx.listener(move |this, _, _w, cx| inc(this, 1., cx)),
        )
    }

    pub(crate) fn render_settings_appearance(&self, cx: &mut Context<Self>) -> AnyElement {
        use lyrel_core::core::config::{
            FONT_SIZE_MAX, FONT_SIZE_MIN, LINE_HEIGHT_MAX, LINE_HEIGHT_MIN, UI_FONT_SIZE_MAX,
            UI_FONT_SIZE_MIN,
        };
        let cfg = cx.global::<Config>();
        let cursor_style = cfg.cursor_style;
        let prompt_cursor_style = cfg.prompt_cursor_style;
        let cursor_blink = cfg.cursor_blink;
        let motion_preference = cfg.motion_preference;
        let font_thicken = cfg.font_thicken;
        let font_family = cfg.font_family.clone();
        let font_bold = cfg.font_family_bold.clone();
        let font_italic = cfg.font_family_italic.clone();
        let ui_font = cfg.ui_font_family.clone();
        let font_ligatures = cfg.font_features.as_ref().is_some_and(|features| {
            features.is_calt_enabled() == Some(true)
                || features
                    .tag_value_list()
                    .iter()
                    .any(|(tag, value)| tag == "liga" && *value != 0)
        });
        let font_size = cfg.font_size;
        let line_height = cfg.line_height;
        let ui_font_size = self.ui_font_size(cx);

        let font_family_control = self.font_dropdown(
            "font-family",
            Some(&font_family),
            None,
            cx,
            |this, name, _w, cx| this.commit_font_family(name, cx),
        );
        let font_bold_control = self.font_dropdown(
            "font-family-bold",
            font_bold.as_deref(),
            Some(font_default_label()),
            cx,
            |this, name, _w, cx| this.commit_font_family_emphasis(true, name, cx),
        );
        let font_italic_control = self.font_dropdown(
            "font-family-italic",
            font_italic.as_deref(),
            Some(font_default_label()),
            cx,
            |this, name, _w, cx| this.commit_font_family_emphasis(false, name, cx),
        );
        let ui_font_control = self.font_dropdown(
            "ui-font-family",
            ui_font.as_deref(),
            Some(ui_font_default_label()),
            cx,
            |this, name, window, cx| this.commit_ui_font_family(name, window, cx),
        );
        let font_size_control = self.stepper_control(
            "font-size",
            format!("{font_size:.0}"),
            " pt",
            (font_size > FONT_SIZE_MIN, font_size < FONT_SIZE_MAX),
            cx,
            |this, dir, cx| this.change_font_size(dir * FONT_SIZE_STEP, cx),
        );
        let line_height_control = self.stepper_control(
            "line-height",
            format!("{line_height:.2}"),
            "",
            (line_height > LINE_HEIGHT_MIN, line_height < LINE_HEIGHT_MAX),
            cx,
            |this, dir, cx| this.change_line_height(dir * LINE_HEIGHT_STEP, cx),
        );
        let ui_font_size_control = self.stepper_control(
            "ui-font-size",
            format!("{ui_font_size:.0}"),
            " pt",
            (
                ui_font_size > UI_FONT_SIZE_MIN,
                ui_font_size < UI_FONT_SIZE_MAX,
            ),
            cx,
            |this, dir, cx| this.change_ui_font_size(dir * UI_FONT_SIZE_STEP, cx),
        );
        let ligatures =
            self.settings_switch("font-ligatures", font_ligatures, cx, |this, on, _, cx| {
                this.set_font_ligatures(on, cx)
            });
        // macOS alone dilates glyph strokes, so elsewhere there is no row.
        let thicken_row = cfg!(target_os = "macos").then(|| {
            let control =
                self.settings_switch("font-thicken", font_thicken, cx, |this, on, _, cx| {
                    this.set_font_thicken(on, cx)
                });
            self.settings_row(
                t(L10nKey::SettingsFontThicken),
                t(L10nKey::SettingsFontThickenDesc),
                control,
                cx,
            )
            .into_any_element()
        });

        let cursor_idx = match cursor_style {
            CursorStyle::Block => 0,
            CursorStyle::Bar => 1,
            CursorStyle::Underline => 2,
        };
        let cursor_style_control = self.settings_choice(
            "cursor-style",
            &[
                t(L10nKey::CursorShapeBlock),
                t(L10nKey::CursorShapeBar),
                t(L10nKey::CursorShapeUnderline),
            ],
            cursor_idx,
            cx,
            |this, ix, _w, cx| {
                let style = match ix {
                    0 => CursorStyle::Block,
                    1 => CursorStyle::Bar,
                    _ => CursorStyle::Underline,
                };
                this.set_cursor_style(style, cx);
            },
        );
        let prompt_cursor_idx = PROMPT_CURSOR_SHAPES
            .iter()
            .position(|s| *s == prompt_cursor_style)
            .unwrap_or(0);
        let prompt_cursor_labels = prompt_cursor_shape_labels();
        let prompt_cursor_labels: Vec<&str> =
            prompt_cursor_labels.iter().map(String::as_str).collect();
        let prompt_cursor_style_control = self.settings_choice(
            "prompt-cursor-style",
            &prompt_cursor_labels,
            prompt_cursor_idx,
            cx,
            |this, ix, _w, cx| {
                if let Some(style) = PROMPT_CURSOR_SHAPES.get(ix) {
                    this.set_prompt_cursor_style(*style, cx);
                }
            },
        );
        let blink = self.settings_switch("cursor-blink", cursor_blink, cx, |this, on, _, cx| {
            this.set_cursor_blink(on, cx)
        });
        let motion_index = match motion_preference {
            MotionPreference::System => 0,
            MotionPreference::Reduce => 1,
            MotionPreference::Allow => 2,
        };
        let motion_control = self.settings_choice(
            "motion-preference",
            &[
                t(L10nKey::SettingsMotionFollowSystem),
                t(L10nKey::SettingsMotionReduce),
                t(L10nKey::SettingsMotionAllow),
            ],
            motion_index,
            cx,
            |this, index, _window, cx| {
                let preference = match index {
                    0 => MotionPreference::System,
                    1 => MotionPreference::Reduce,
                    _ => MotionPreference::Allow,
                };
                this.set_motion_preference(preference, cx);
            },
        );

        let mut groups = vec![self.render_theme_group(cx)];
        groups.extend(self.render_theme_editor_group(cx));
        groups.push(
            self.settings_group(
                Some(t(L10nKey::SettingsMotionGroup)),
                None,
                [self
                    .settings_row(
                        t(L10nKey::SettingsMotionPreference),
                        t(L10nKey::SettingsMotionPreferenceDesc),
                        motion_control,
                        cx,
                    )
                    .into_any_element()],
                cx,
            ),
        );
        groups.push(
            self.settings_group(
                Some(t(L10nKey::SettingsTerminalFontGroup)),
                None,
                [
                    self.settings_row(
                        t(L10nKey::SettingsFontFamily),
                        t(L10nKey::SettingsFontFamilyDesc),
                        font_family_control,
                        cx,
                    )
                    .into_any_element(),
                    self.settings_row(
                        t(L10nKey::SettingsFontSize),
                        t(L10nKey::SettingsFontSizeDesc),
                        font_size_control,
                        cx,
                    )
                    .into_any_element(),
                    self.settings_row(
                        t(L10nKey::SettingsLineHeight),
                        t(L10nKey::SettingsLineHeightDesc),
                        line_height_control,
                        cx,
                    )
                    .into_any_element(),
                    self.settings_row(
                        t(L10nKey::SettingsBoldFont),
                        t(L10nKey::SettingsBoldFontDesc),
                        font_bold_control,
                        cx,
                    )
                    .into_any_element(),
                    self.settings_row(
                        t(L10nKey::SettingsItalicFont),
                        t(L10nKey::SettingsItalicFontDesc),
                        font_italic_control,
                        cx,
                    )
                    .into_any_element(),
                    self.settings_row(
                        t(L10nKey::SettingsFontLigatures),
                        t(L10nKey::SettingsFontLigaturesDesc),
                        ligatures,
                        cx,
                    )
                    .into_any_element(),
                ]
                .into_iter()
                .chain(thicken_row),
                cx,
            ),
        );
        groups.push(
            self.settings_group(
                Some(t(L10nKey::SettingsInterfaceFontGroup)),
                None,
                [
                    self.settings_row(
                        t(L10nKey::SettingsUiFontFamily),
                        t(L10nKey::SettingsUiFontFamilyDesc),
                        ui_font_control,
                        cx,
                    ),
                    self.settings_row(
                        t(L10nKey::SettingsUiFontSize),
                        t(L10nKey::SettingsUiFontSizeDesc),
                        ui_font_size_control,
                        cx,
                    ),
                ]
                .map(IntoElement::into_any_element),
                cx,
            ),
        );
        groups.push(
            self.settings_group(
                Some(t(L10nKey::SettingsCursor)),
                None,
                [
                    self.settings_row(
                        t(L10nKey::SettingsCursorShape),
                        t(L10nKey::SettingsCursorShapeDesc),
                        cursor_style_control,
                        cx,
                    ),
                    self.settings_row(
                        t(L10nKey::SettingsPromptCursorShape),
                        t(L10nKey::SettingsPromptCursorShapeDesc),
                        prompt_cursor_style_control,
                        cx,
                    ),
                    self.settings_row(
                        t(L10nKey::SettingsCursorBlink),
                        t(L10nKey::SettingsCursorBlinkDesc),
                        blink,
                        cx,
                    ),
                ]
                .map(IntoElement::into_any_element),
                cx,
            ),
        );
        groups.push(self.render_window_section(cx));
        Self::settings_page(groups)
    }

    fn render_window_section(&self, cx: &mut Context<Self>) -> AnyElement {
        let tk = Tk::of(cx);
        let config = cx.global::<Config>();
        let overridden = window_overrides_active(config, cfg!(target_os = "windows"));
        let dim_inactive_panes = config.dim_inactive_panes;
        let opacity = SettingsWindow::effective_window_opacity(cx);

        const MIN: f32 = 0.2;
        const MAX: f32 = 1.0;
        let app = cx.entity().downgrade();
        let release_app = app.clone();
        let opacity_control = kit::slider(
            "window-opacity",
            (opacity - MIN) / (MAX - MIN),
            format!("{:.0}%", opacity * 100.),
            &tk,
            Rc::new(move |f, _window, cx| {
                let v = ((MIN + f * (MAX - MIN)) * 100.).round() / 100.;
                cx.global_mut::<Config>().window_opacity = Some(v.clamp(MIN, MAX));
                // The drag previews: repaint only, the release callback persists.
                let _ = app.update(cx, |_, cx| {
                    cx.emit(SettingsEvent::ApplyTheme {
                        persist: false,
                        refresh_windows: false,
                    });
                    cx.notify();
                });
            }),
            Rc::new(move |_window, cx| {
                let _ = release_app.update(cx, |this, cx| this.persist_settings_config(cx));
            }),
        );

        // Windows exposes the native backdrop materials directly; macOS keeps
        // the simple blur toggle, which drives its vibrancy.
        #[cfg(target_os = "windows")]
        let blur_control = {
            let current = config.window_backdrop;
            let entries = crate::ui::theme::backdrop_options(current)
                .into_iter()
                .map(|backdrop| {
                    MenuEntry::item(
                        t(window_backdrop_label_key(backdrop)),
                        backdrop == current,
                        move |this, window, cx| this.set_window_backdrop(backdrop, window, cx),
                    )
                })
                .collect();
            self.settings_dropdown(
                "window-backdrop",
                t(window_backdrop_label_key(current)),
                None,
                entries,
                cx,
            )
        };
        #[cfg(not(target_os = "windows"))]
        let blur_control = {
            let theme = presets::by_id(cx, &crate::ui::theme::effective_preset_id(cx));
            let blur = cx.global::<Config>().window_blur.unwrap_or(theme.blur);
            self.settings_switch("window-blur", blur, cx, |this, on, window, cx| {
                this.set_window_blur(on, window, cx)
            })
        };
        // `Auto` is the one backdrop that still defers to the legacy blur
        // flag. Offer that switch exactly when it has an effect — otherwise a
        // stored `window_blur: true` would blur the window with no control to
        // clear it.
        #[cfg(target_os = "windows")]
        let auto_blur_row =
            (cx.global::<Config>().window_backdrop == WindowBackdrop::Auto).then(|| {
                let theme = presets::by_id(cx, &crate::ui::theme::effective_preset_id(cx));
                let blur = cx.global::<Config>().window_blur.unwrap_or(theme.blur);
                let control =
                    self.settings_switch("window-blur", blur, cx, |this, on, window, cx| {
                        this.set_window_blur(on, window, cx)
                    });
                self.settings_row(
                    t(L10nKey::SettingsBlur),
                    t(L10nKey::SettingsBlurAutoDesc),
                    control,
                    cx,
                )
                .into_any_element()
            });
        #[cfg(not(target_os = "windows"))]
        let auto_blur_row: Option<AnyElement> = None;
        let dim = self.settings_switch(
            "dim-inactive-panes",
            dim_inactive_panes,
            cx,
            |this, on, _, cx| this.set_dim_inactive_panes(on, cx),
        );
        let follow_theme = overridden.then(|| {
            h_flex()
                .pt(px(4.))
                .pb(px(8.))
                .child(
                    kit::button(
                        "follow-theme-window",
                        t(L10nKey::FollowTheme),
                        BtnKind::Link,
                    )
                    .on_click(
                        cx.listener(|this, _, window, cx| this.reset_window_overrides(window, cx)),
                    ),
                )
                .into_any_element()
        });

        let mut rows = vec![
            self.settings_row(
                t(L10nKey::SettingsOpacity),
                t(L10nKey::SettingsOpacityDesc),
                opacity_control,
                cx,
            )
            .into_any_element(),
            self.settings_row(
                t(if cfg!(target_os = "windows") {
                    L10nKey::SettingsBackdrop
                } else {
                    L10nKey::SettingsBlur
                }),
                t(if cfg!(target_os = "windows") {
                    L10nKey::SettingsBackdropDesc
                } else {
                    L10nKey::SettingsBlurDesc
                }),
                blur_control,
                cx,
            )
            .into_any_element(),
        ];
        rows.extend(auto_blur_row);
        rows.extend(follow_theme);
        rows.push(
            self.settings_row(
                t(L10nKey::SettingsDimInactivePanes),
                t(L10nKey::SettingsDimInactivePanesDesc),
                dim,
                cx,
            )
            .into_any_element(),
        );
        self.settings_group(Some(t(L10nKey::SettingsTransparency)), None, rows, cx)
    }

}
