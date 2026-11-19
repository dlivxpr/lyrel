//! The Terminal page: the shell group that owns the program, arguments and
//! working-directory rows, plus the prompt group below it.

use std::rc::Rc;

use super::kit::{self, Tk, fs};
use super::shell::MenuEntry;
use super::*;

impl SettingsWindow {
    fn render_shell_group(&self, cx: &mut Context<Self>) -> AnyElement {
        let tk = Tk::of(cx);
        let (program_input, args_input, wd_path_input) = match self.active_settings() {
            Some(s) => (
                s.shell_program_input.clone(),
                s.shell_args_input.clone(),
                s.wd_path_input.clone(),
            ),
            None => return div().into_any_element(),
        };
        let wd_strategy = cx.global::<Config>().working_directory.strategy;
        let platform_default = if cfg!(windows) {
            "PowerShell"
        } else {
            t(L10nKey::SettingsShellDefaultLoginShell)
        };

        // lyrel already knows which shells are installed — it lists them on the
        // new-tab button — so the field carries a menu of them. The field
        // stays: a shell lyrel did not find still has to be reachable by path.
        // Detected shells only: a `custom_shells` row carries arguments this
        // picker would silently drop.
        let shells = (self.workspace.shells)(cx);
        let current_program = program_input.read(cx).value().trim().to_string();
        let platform_default_item: SharedString = if cfg!(windows) {
            "PowerShell".into()
        } else {
            t(L10nKey::AppPlaceholderLoginShell).into()
        };
        let pick = |program: String, input: Entity<InputState>| {
            move |this: &mut Self, window: &mut Window, cx: &mut Context<Self>| {
                input.update(cx, |state, cx| state.set_value(program.clone(), window, cx));
                this.commit_shell_from_picker(cx);
            }
        };
        let mut entries = vec![MenuEntry::item(
            platform_default_item,
            current_program.is_empty(),
            pick(String::new(), program_input.clone()),
        )];
        if !shells.is_empty() {
            entries.push(MenuEntry::Separator);
        }
        for shell in &shells {
            entries.push(MenuEntry::item(
                shell.label.clone(),
                current_program == shell.program,
                pick(shell.program.clone(), program_input.clone()),
            ));
        }
        let menu_id: SharedString = "shell-program-detected".into();
        let open = self.settings_menu_open(&menu_id);
        let picker = (!shells.is_empty()).then(|| {
            let toggle = menu_id.clone();
            div()
                .id("shell-program-detected-trigger")
                .flex_shrink_0()
                .ml(px(4.))
                // Pulled into the field's 10px padding far enough that the
                // chevron's centre sits 12px from the edge — where a
                // dropdown's does, one row down.
                .mr(px(-7.))
                .size(px(18.))
                .rounded(px(4.))
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .hover(move |s| s.bg(tk.k05))
                .when(open, |d| d.bg(tk.k05))
                .child(kit::chevron_down(&tk))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.toggle_settings_menu(toggle.clone(), 0, false, window, cx)
                }))
        });
        let program_control = div()
            .relative()
            .child(
                self.settings_text_input(&program_input, FIELD_W, false, cx)
                    .children(picker),
            )
            .when(open, |d| {
                d.child(kit::popover_below(
                    true,
                    4.,
                    self.settings_menu_panel(&menu_id, entries, cx)
                        .min_w(px(FIELD_W)),
                ))
            })
            .into_any_element();
        // Args become argv verbatim, so a quote that never closes is a value
        // that cannot be saved at all — `commit_shell` refuses it, and this
        // line is the explanation (#551). The input commits on Enter/blur, so
        // a half-typed quote is never marked wrong mid-keystroke.
        let args_value = args_input.read(cx).value();
        let args_error = super::arguments::split_shell_args(&args_value)
            .is_err()
            .then(|| t(L10nKey::SettingsArgumentsInvalid).to_string());
        let args_control = self.settings_checked_input(&args_input, FIELD_W, args_error, cx);

        use lyrel_core::core::config::WdStrategy;
        let wd_idx = match wd_strategy {
            WdStrategy::Inherit => 0,
            WdStrategy::Home => 1,
            WdStrategy::Custom => 2,
        };
        let wd_radio = self.settings_choice(
            "wd-strategy",
            &[
                t(L10nKey::SettingsWdInherit),
                t(L10nKey::SettingsWdHome),
                t(L10nKey::SettingsWdCustom),
            ],
            wd_idx,
            cx,
            |this, ix, _w, cx| {
                let s = match ix {
                    0 => WdStrategy::Inherit,
                    1 => WdStrategy::Home,
                    _ => WdStrategy::Custom,
                };
                this.set_working_directory_strategy(s, cx);
            },
        );
        let mut rows = vec![
            self.settings_row(
                t(L10nKey::SettingsProgram),
                t(L10nKey::SettingsProgramDesc),
                program_control,
                cx,
            )
            .into_any_element(),
            self.settings_row(
                t(L10nKey::SettingsArguments),
                t(L10nKey::SettingsArgumentsDesc),
                args_control,
                cx,
            )
            .into_any_element(),
            self.settings_row(
                t(L10nKey::SettingsStartIn),
                t(L10nKey::SettingsStartInDesc),
                wd_radio,
                cx,
            )
            .into_any_element(),
        ];
        if wd_strategy == WdStrategy::Custom {
            // `commit_working_directory_path` refuses the same value through
            // the same predicate, so the red line and the not-saved config
            // always agree (#601).
            let value = wd_path_input.read(cx).value();
            let error = (!super::inputs::wd_path_saveable(&value))
                .then(|| t(L10nKey::SettingsWdPathInvalid).to_string());
            let control = self.settings_checked_input(&wd_path_input, PATH_FIELD_W, error, cx);
            rows.push(
                self.settings_row(
                    t(L10nKey::SettingsCustomPath),
                    t(L10nKey::SettingsCustomPathDesc),
                    control,
                    cx,
                )
                .into_any_element(),
            );
        }
        rows.push(
            div()
                .pt(px(4.))
                .text_size(fs(12.))
                .line_height(px(17.))
                .text_color(tk.k6)
                .child(t(L10nKey::SettingsShellFooter))
                .into_any_element(),
        );
        self.settings_group(
            Some(t(L10nKey::SettingsShell)),
            Some(t_fmt(
                L10nKey::SettingsShellIntro,
                &[("default", platform_default)],
            )),
            rows,
            cx,
        )
    }

    pub(crate) fn render_settings_terminal(&self, cx: &mut Context<Self>) -> AnyElement {
        let tk = Tk::of(cx);
        let cfg = cx.global::<Config>();
        let link_url = cfg.link_url;
        let ssh_loopback_forward = cfg.ssh_loopback_forward;
        let scroll_mult = cfg.mouse_scroll_multiplier;
        let smooth_scroll = cfg.smooth_scroll;
        let bell = cfg.bell;
        let link_file_open = cfg.file_open_mode();
        // A bucket highlights only on an exact match; any other value gets a
        // "Custom (N)" cell so the highlight never claims a number the config
        // does not have (#550).
        let (scrollback_sel, scrollback_custom) =
            preset_choice(&SCROLLBACK_BUCKETS, cfg.scrollback_limit, group_thousands);
        let Some(link_file_command_input) = self
            .active_settings()
            .map(|s| s.link_file_command_input.clone())
        else {
            return div().into_any_element();
        };

        let link_switch = self.settings_switch("term-link-url", link_url, cx, |this, on, _, cx| {
            this.set_link_url(on, cx)
        });
        let loopback_switch = self.settings_switch(
            "term-ssh-loopback-forward",
            ssh_loopback_forward,
            cx,
            |this, on, _, cx| this.set_ssh_loopback_forward(on, cx),
        );
        let link_file_open_radio = self.settings_choice(
            "term-link-file-open",
            &[
                t(L10nKey::SettingsOpenFilesInternal),
                t(L10nKey::SettingsOpenFilesSystem),
                t(L10nKey::SettingsOpenFilesCommand),
            ],
            match link_file_open {
                LinkFileOpen::Internal => 0,
                LinkFileOpen::System => 1,
                LinkFileOpen::Command => 2,
            },
            cx,
            |this, ix, _w, cx| {
                let mode = match ix {
                    0 => LinkFileOpen::Internal,
                    1 => LinkFileOpen::System,
                    _ => LinkFileOpen::Command,
                };
                this.set_link_file_open(mode, cx);
            },
        );
        // Only shown under `Command`: an empty box next to two working modes
        // reads as "this is what file links do".
        let link_file_command_row = (link_file_open == LinkFileOpen::Command).then(|| {
            let control = self
                .settings_text_input(&link_file_command_input, PATH_FIELD_W, false, cx)
                .into_any_element();
            self.settings_row(
                t(L10nKey::SettingsOpenFilesCommand),
                t_fmt(
                    L10nKey::SettingsOpenFilesWithDesc,
                    &[
                        ("modifier", LINK_MODIFIER_LABEL),
                        ("path", "{path}"),
                        ("line", "{line}"),
                        ("column", "{column}"),
                    ],
                ),
                control,
                cx,
            )
            .into_any_element()
        });
        let scrollback = self.settings_choice_valued(
            "term-scrollback",
            &SCROLLBACK_LABELS,
            scrollback_sel,
            scrollback_custom,
            cx,
            |this, ix, _w, cx| {
                let lines = SCROLLBACK_BUCKETS
                    .get(ix)
                    .copied()
                    .unwrap_or(Config::default().scrollback_limit);
                this.set_scrollback_limit(lines, cx);
            },
        );
        let bell_idx = match bell {
            BellMode::None => 0,
            BellMode::Visual => 1,
            BellMode::Audible => 2,
            BellMode::Both => 3,
        };
        let bell_control = self.settings_choice(
            "term-bell",
            &[
                t(L10nKey::SettingsBellModeOff),
                t(L10nKey::SettingsBellModeVisual),
                t(L10nKey::SettingsBellModeAudible),
                t(L10nKey::SettingsBellModeBoth),
            ],
            bell_idx,
            cx,
            |this, ix, _w, cx| {
                let mode = match ix {
                    0 => BellMode::None,
                    1 => BellMode::Visual,
                    2 => BellMode::Audible,
                    3 => BellMode::Both,
                    _ => BellMode::default(),
                };
                this.set_bell_mode(mode, cx);
            },
        );
        const MIN: f32 = 0.5;
        const MAX: f32 = 5.0;
        const STEP: f32 = 0.25;
        let app = cx.entity().downgrade();
        let release_app = app.clone();
        let scroll_control = kit::slider(
            "term-scroll-speed",
            (scroll_mult - MIN) / (MAX - MIN),
            format!("{scroll_mult:.2}×"),
            &tk,
            Rc::new(move |f, _window, cx| {
                let v = ((MIN + f * (MAX - MIN)) / STEP).round() * STEP;
                cx.global_mut::<Config>().mouse_scroll_multiplier = v.clamp(MIN, MAX);
                let _ = app.update(cx, |_, cx| cx.notify());
            }),
            Rc::new(move |_window, cx| {
                let _ = release_app.update(cx, |this, cx| this.persist_settings_config(cx));
            }),
        );
        let smooth = self.settings_switch(
            "term-smooth-scroll",
            smooth_scroll,
            cx,
            |this, on, _, cx| this.set_smooth_scroll(on, cx),
        );

        Self::settings_page([
            self.render_shell_group(cx),
            self.render_prompt_group(cx),
            self.settings_group(
                Some(t(L10nKey::SettingsScrolling)),
                None,
                [
                    self.settings_row(
                        t(L10nKey::SettingsScrollback),
                        t(L10nKey::SettingsScrollbackDesc),
                        scrollback,
                        cx,
                    ),
                    self.settings_row(
                        t(L10nKey::SettingsScrollSpeed),
                        t(L10nKey::SettingsScrollSpeedDesc),
                        scroll_control,
                        cx,
                    ),
                    self.settings_row(
                        t(L10nKey::SettingsSmoothScroll),
                        t(L10nKey::SettingsSmoothScrollDesc),
                        smooth,
                        cx,
                    ),
                ]
                .map(IntoElement::into_any_element),
                cx,
            ),
            self.settings_group(
                Some(t(L10nKey::SettingsBell)),
                None,
                [self
                    .settings_row(
                        t(L10nKey::SettingsTerminalBell),
                        t(L10nKey::SettingsTerminalBellDesc),
                        bell_control,
                        cx,
                    )
                    .into_any_element()],
                cx,
            ),
            self.settings_group(
                Some(t(L10nKey::SettingsLinks)),
                None,
                [
                    self.settings_row(
                        t(L10nKey::DetectUrls),
                        t_fmt(
                            L10nKey::SettingsDetectUrlsDesc,
                            &[("modifier", LINK_MODIFIER_LABEL)],
                        ),
                        link_switch,
                        cx,
                    )
                    .into_any_element(),
                    self.settings_row(
                        t(L10nKey::ForwardSshLoopbackLinks),
                        t(L10nKey::SettingsForwardSshLoopbackLinksDesc),
                        loopback_switch,
                        cx,
                    )
                    .into_any_element(),
                    self.settings_row(
                        t(L10nKey::OpenFilesWith),
                        t_fmt(
                            L10nKey::SettingsOpenFilesModeDesc,
                            &[("modifier", LINK_MODIFIER_LABEL)],
                        ),
                        link_file_open_radio,
                        cx,
                    )
                    .into_any_element(),
                ]
                .into_iter()
                .chain(link_file_command_row),
                cx,
            ),
        ])
    }

    fn render_prompt_group(&self, cx: &mut Context<Self>) -> AnyElement {
        let cfg = cx.global::<Config>();
        let prompt_editor = cfg.prompt_editor;
        let tab_completion = cfg.tab_completion;
        let history_search = cfg.history_search;
        let per_pane_history = cfg.per_pane_history;
        // Tab completion and history search are menus lyrel opens *inside* its
        // own prompt editor. With the editor off both keys belong to the
        // shell, so the switches grey out and say why; their stored values
        // are untouched and come back with it.
        let gated = |desc: L10nKey| match prompt_editor {
            true => t(desc).to_string(),
            false => format!("{} {}", t(desc), t(L10nKey::SettingsNeedsPromptEditor)),
        };
        let editor = self.settings_switch(
            "term-prompt-editor",
            prompt_editor,
            cx,
            |this, on, _, cx| this.set_prompt_editor(on, cx),
        );
        let completion = kit::switch("term-tab-completion")
            .checked(tab_completion)
            .disabled(!prompt_editor)
            .on_click(cx.listener(|this, on: &bool, _w, cx| this.set_tab_completion(*on, cx)))
            .into_any_element();
        let history = kit::switch("term-history-search")
            .checked(history_search)
            .disabled(!prompt_editor)
            .on_click(cx.listener(|this, on: &bool, _w, cx| this.set_history_search(*on, cx)))
            .into_any_element();
        let per_pane = self.settings_switch(
            "term-per-pane-history",
            per_pane_history,
            cx,
            |this, on, _, cx| this.set_per_pane_history(on, cx),
        );
        self.settings_group(
            Some(t(L10nKey::SettingsPrompt)),
            Some(t(L10nKey::SettingsPromptIntro).to_string()),
            [
                self.settings_row(
                    t(L10nKey::SettingsPromptEditor),
                    t(L10nKey::SettingsPromptEditorDesc),
                    editor,
                    cx,
                ),
                self.settings_row_gated_when(
                    t(L10nKey::SettingsTabCompletion),
                    gated(L10nKey::SettingsTabCompletionDesc),
                    completion,
                    !prompt_editor,
                    cx,
                ),
                self.settings_row_gated_when(
                    t(L10nKey::SettingsHistorySearch),
                    gated(L10nKey::SettingsHistorySearchDesc),
                    history,
                    !prompt_editor,
                    cx,
                ),
                self.settings_row(
                    t(L10nKey::SettingsPerPaneHistory),
                    t(L10nKey::SettingsPerPaneHistoryDescription),
                    per_pane,
                    cx,
                ),
            ]
            .map(IntoElement::into_any_element),
            cx,
        )
    }

}
