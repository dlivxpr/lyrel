//! The Keyboard & Mouse page: input, selection and scrolling rows.

use super::*;

impl SettingsWindow {
    pub(crate) fn render_settings_input(&self, cx: &mut Context<Self>) -> AnyElement {
        let cfg = cx.global::<Config>();
        let mouse_hide = cfg.mouse_hide_while_typing;
        let focus_follows = cfg.focus_follows_mouse;
        let mouse_reporting = cfg.mouse_reporting;
        let mouse_zoom = cfg.mouse_zoom_modifier;
        let option_as_alt = cfg.macos_option_as_alt;
        let smart_select = cfg.smart_select;
        let copy_on_select = cfg.copy_on_select;
        let clip_trim = cfg.clipboard_trim_trailing_spaces;

        let focus = self.settings_switch(
            "term-focus-follows",
            focus_follows,
            cx,
            |this, on, _, cx| this.set_focus_follows_mouse(on, cx),
        );
        let hide = self.settings_switch("term-mouse-hide", mouse_hide, cx, |this, on, _, cx| {
            this.set_mouse_hide_while_typing(on, cx)
        });
        let report = self.settings_switch(
            "term-mouse-report",
            mouse_reporting,
            cx,
            |this, on, _, cx| this.set_mouse_reporting(on, cx),
        );
        let smart =
            self.settings_switch("term-smart-select", smart_select, cx, |this, on, _, cx| {
                this.set_smart_select(on, cx)
            });
        let copy = self.settings_switch(
            "term-copy-on-select",
            copy_on_select,
            cx,
            |this, on, _, cx| this.set_copy_on_select(on, cx),
        );
        let trim = self.settings_switch("term-clip-trim", clip_trim, cx, |this, on, _, cx| {
            this.set_clipboard_trim(on, cx)
        });
        // Ctrl only earns a cell where it is a different key from the platform
        // modifier: off macOS the two are the same key. A config that names
        // `ctrl` there still highlights it, in the one cell that means it.
        let mac = cfg!(target_os = "macos");
        let zoom_labels: Vec<&str> = if mac {
            vec!["⌘", "⌃", "⌥", t(L10nKey::SettingsMouseZoomOff)]
        } else {
            vec!["Ctrl", "Alt", t(L10nKey::SettingsMouseZoomOff)]
        };
        let zoom_idx = match (mouse_zoom, mac) {
            (MouseZoomModifier::Platform, _) => 0,
            (MouseZoomModifier::Ctrl, true) => 1,
            (MouseZoomModifier::Ctrl, false) => 0,
            (MouseZoomModifier::Alt, true) => 2,
            (MouseZoomModifier::Alt, false) => 1,
            (MouseZoomModifier::None, true) => 3,
            (MouseZoomModifier::None, false) => 2,
        };
        let zoom = self.settings_choice(
            "term-mouse-zoom",
            &zoom_labels,
            zoom_idx,
            cx,
            move |this, ix, _w, cx| {
                let modifier = match (ix, mac) {
                    (0, _) => MouseZoomModifier::Platform,
                    (1, true) => MouseZoomModifier::Ctrl,
                    (1, false) => MouseZoomModifier::Alt,
                    (2, true) => MouseZoomModifier::Alt,
                    _ => MouseZoomModifier::None,
                };
                this.set_mouse_zoom_modifier(modifier, cx);
            },
        );
        let shortcuts = self
            .settings_button(
                "open-keybindings",
                t(L10nKey::SettingsEditShortcuts),
                cx,
                |this, window, cx| {
                    this.navigate_settings(SettingsSection::Keybindings, None, window, cx)
                },
            )
            .into_any_element();
        let option_group = mac.then(|| {
            let control = self.settings_switch(
                "term-option-as-alt",
                option_as_alt,
                cx,
                |this, on, _, cx| this.set_macos_option_as_alt(on, cx),
            );
            self.settings_group(
                Some(t(L10nKey::SettingsKeyboard)),
                None,
                [self
                    .settings_row(
                        t(L10nKey::SettingsOptionAsMeta),
                        t(L10nKey::SettingsOptionAsMetaDesc),
                        control,
                        cx,
                    )
                    .into_any_element()],
                cx,
            )
        });

        let mut groups = vec![
            self.settings_group(
                None,
                None,
                [self
                    .settings_row(
                        t(L10nKey::SettingsSearchKeybindingsTitle),
                        t(L10nKey::SettingsKeybindingsIntroDesc),
                        shortcuts,
                        cx,
                    )
                    .into_any_element()],
                cx,
            ),
            self.settings_group(
                Some(t(L10nKey::SettingsSelectionClipboard)),
                None,
                [
                    self.settings_row(
                        t(L10nKey::SettingsSmartSelection),
                        t(L10nKey::SettingsSmartSelectionDesc),
                        smart,
                        cx,
                    ),
                    self.settings_row(
                        t(L10nKey::SettingsCopyOnSelect),
                        t(L10nKey::SettingsCopyOnSelectDesc),
                        copy,
                        cx,
                    ),
                    self.settings_row(
                        t(L10nKey::SettingsTrimTrailingSpaces),
                        t(L10nKey::SettingsTrimTrailingSpacesDesc),
                        trim,
                        cx,
                    ),
                ]
                .map(IntoElement::into_any_element),
                cx,
            ),
        ];
        groups.extend(option_group);
        groups.push(
            self.settings_group(
                Some(t(L10nKey::SettingsMouse)),
                None,
                [
                    self.settings_row(
                        t(L10nKey::SettingsFocusFollowsMouse),
                        t(L10nKey::SettingsFocusFollowsMouseDesc),
                        focus,
                        cx,
                    ),
                    self.settings_row(
                        t(L10nKey::SettingsHideMouseWhileTyping),
                        t(L10nKey::SettingsHideMouseWhileTypingDesc),
                        hide,
                        cx,
                    ),
                    self.settings_row(
                        t(L10nKey::SettingsReportMouseToApps),
                        t(L10nKey::SettingsReportMouseToAppsDesc),
                        report,
                        cx,
                    ),
                    self.settings_row(
                        t(L10nKey::SettingsMouseZoom),
                        t(L10nKey::SettingsMouseZoomDesc),
                        zoom,
                        cx,
                    ),
                ]
                .map(IntoElement::into_any_element),
                cx,
            ),
        );
        Self::settings_page(groups)
    }

}
