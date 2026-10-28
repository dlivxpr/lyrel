use super::arguments::{join_shell_args, split_shell_args};
use super::*;
use crate::core::config::ShellConfig;

/// The one rule the "Start in" custom path lives by (#601): empty means unset
/// and saves; anything else must name a directory that exists, because the
/// daemon's picker skips a path that is not one and every new pane then
/// silently starts somewhere else. Settings refuses to save such a value and
/// marks it red — both decide through this, so the red line and the not-saved
/// config always agree. Local on purpose: this is the local daemon's config.
pub(super) fn wd_path_saveable(path: &str) -> bool {
    let path = path.trim();
    path.is_empty() || std::path::Path::new(path).is_dir()
}

impl SettingsWindow {
    pub(super) fn build_shell_inputs(
        &mut self,
        subs: &mut Vec<Subscription>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (Entity<InputState>, Entity<InputState>, Entity<InputState>) {
        let cfg = cx.global::<Config>();
        let (shell_program, shell_args) = match &cfg.shell {
            // Quote each argument back into field text rather than joining on
            // spaces: `join(" ")` cannot spell an argument that contains one,
            // so a perfectly legal `"args": ["-c", "echo hi"]` in config.json
            // refilled as three words and re-committed as three argv the moment
            // the field lost focus — the user never typed a thing (#551).
            // `join_shell_args` quotes only what needs it, and `commit_shell`'s
            // matching `split_shell_args` parses it back losslessly.
            Some(s) => (s.program.clone(), join_shell_args(&s.args)),
            None => (String::new(), String::new()),
        };
        let wd_path = cfg.working_directory.path.clone();
        let platform_default = if cfg!(windows) {
            "PowerShell"
        } else {
            t(L10nKey::AppPlaceholderLoginShell)
        };
        let shell_program_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(platform_default)
                .default_value(shell_program)
        });
        let shell_args_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t(L10nKey::AppPlaceholderNone))
                .default_value(shell_args)
        });
        let wd_path_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("/path/to/directory")
                .default_value(wd_path)
        });
        let commit_shell = |this: &mut Self, ev: &InputEvent, cx: &mut Context<Self>| {
            if matches!(ev, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                this.commit_shell(cx);
            }
        };
        let commit_wd = |this: &mut Self, ev: &InputEvent, cx: &mut Context<Self>| {
            if matches!(ev, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                this.commit_working_directory_path(cx);
            }
        };
        subs.push(
            cx.subscribe_in(&shell_program_input, window, move |this, _i, ev, _w, cx| {
                commit_shell(this, ev, cx)
            }),
        );
        subs.push(
            cx.subscribe_in(&shell_args_input, window, move |this, _i, ev, _w, cx| {
                commit_shell(this, ev, cx)
            }),
        );
        subs.push(
            cx.subscribe_in(&wd_path_input, window, move |this, _i, ev, _w, cx| {
                commit_wd(this, ev, cx)
            }),
        );
        (shell_program_input, shell_args_input, wd_path_input)
    }

    pub(super) fn build_link_file_command_input(
        &mut self,
        subs: &mut Vec<Subscription>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<InputState> {
        let value = cx
            .global::<Config>()
            .link_file_command
            .clone()
            .unwrap_or_default();
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t(L10nKey::AppPlaceholderOpenInDefaultApp))
                .default_value(value)
        });
        subs.push(
            cx.subscribe_in(&input, window, move |this, _i, ev, _w, cx| {
                if matches!(ev, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                    this.commit_link_file_command(cx);
                }
            }),
        );
        input
    }

    pub(super) fn build_http_proxy_input(
        &mut self,
        subs: &mut Vec<Subscription>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<InputState> {
        let value = cx.global::<Config>().http_proxy.clone().unwrap_or_default();
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("http://127.0.0.1:7890")
                .default_value(value)
        });
        subs.push(
            cx.subscribe_in(&input, window, move |this, _i, ev, _w, cx| {
                if matches!(ev, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                    this.commit_http_proxy(cx);
                }
            }),
        );
        input
    }

    fn commit_http_proxy(&mut self, cx: &mut Context<Self>) {
        let Some(value) = self
            .active_settings()
            .map(|s| s.http_proxy_input.read(cx).value().trim().to_string())
        else {
            return;
        };
        // Keep an unusable value out of `config.json`. The row renders a hint
        // under the input, so the typo does not silently vanish either.
        if !value.is_empty() && !lyrel_core::daemon::install::proxy::is_valid_manual(&value) {
            cx.notify();
            return;
        }
        let value = (!value.is_empty()).then_some(value);
        let cfg = cx.global_mut::<Config>();
        if cfg.http_proxy == value {
            return;
        }
        cfg.http_proxy = value;
        self.persist_settings_config(cx);
        cx.notify();
    }

    fn commit_link_file_command(&mut self, cx: &mut Context<Self>) {
        let Some(command) = self.active_settings().map(|s| {
            s.link_file_command_input
                .read(cx)
                .value()
                .trim()
                .to_string()
        }) else {
            return;
        };
        let command = if command.is_empty() {
            None
        } else {
            Some(command)
        };
        let cfg = cx.global_mut::<Config>();
        if cfg.link_file_command == command {
            return;
        }
        cfg.link_file_command = command;
        self.persist_settings_config(cx);
        cx.notify();
    }
    /// Saves the shell after the detected-shell menu wrote into the field.
    ///
    /// The field itself only commits on Enter or blur, and picking from a menu
    /// is neither — without this the choice would sit in the box unsaved until
    /// the user happened to click into it and out again.
    pub(crate) fn commit_shell_from_picker(&mut self, cx: &mut Context<Self>) {
        self.commit_shell(cx);
        cx.notify();
    }

    fn commit_shell(&mut self, cx: &mut Context<Self>) {
        let Some(settings) = self.active_settings() else {
            return;
        };
        let program = settings
            .shell_program_input
            .read(cx)
            .value()
            .trim()
            .to_string();
        // Split the way a command line splits, not on raw whitespace: the text
        // goes on to become argv verbatim, and `split_whitespace` kept the
        // quote characters while tearing the quoted string apart — `-c "echo
        // hi"` reached the shell as `["-c", "\"echo", "hi\""]` (#551).
        let raw_args = settings.shell_args_input.read(cx).value();
        let args: Vec<String> = match split_shell_args(&raw_args) {
            Ok(args) => args,
            // A quote that never closes names no argv at all, so this text
            // cannot go into `config.json` — the proxy row's pattern: leave the
            // stored arguments alone and let the row explain why under the
            // input after this notify. The rest of the row still commits: the
            // Program the user just typed, or picked from the menu, is not
            // theirs to lose over the field below it.
            Err(_) => {
                cx.notify();
                let cfg = cx.global::<Config>();
                cfg.shell
                    .as_ref()
                    .map(|s| s.args.clone())
                    .unwrap_or_default()
            }
        };
        let shell = if program.is_empty() {
            None
        } else {
            Some(ShellConfig { program, args })
        };
        {
            let cfg = cx.global_mut::<Config>();
            if cfg.shell == shell {
                return;
            }
            cfg.shell = shell;
            self.persist_settings_config(cx);
        }
        // Shell discovery runs off the UI thread and now includes the saved
        // configured shell, so refresh the menu without blocking Settings.
        cx.emit(SettingsEvent::RefreshShells);
    }

    pub(crate) fn set_working_directory_strategy(
        &mut self,
        strategy: crate::core::config::WdStrategy,
        cx: &mut Context<Self>,
    ) {
        let cfg = cx.global_mut::<Config>();
        if cfg.working_directory.strategy == strategy {
            return;
        }
        cfg.working_directory.strategy = strategy;
        self.persist_settings_config(cx);
        cx.notify();
    }

    fn commit_working_directory_path(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self
            .active_settings()
            .map(|s| s.wd_path_input.read(cx).value().trim().to_string())
        else {
            return;
        };
        // A typo here is not a directory, and the daemon then silently falls
        // back to its own cwd for every new pane — "new shells don't start in
        // my project" reads as a lyrel bug rather than a typo (#601). Refuse
        // to save, the proxy row's pattern (#551): the field keeps the text,
        // the settings row explains in red, and the last good value stays in
        // config.json.
        if !wd_path_saveable(&path) {
            cx.notify();
            return;
        }
        let cfg = cx.global_mut::<Config>();
        if cfg.working_directory.path == path {
            return;
        }
        cfg.working_directory.path = path;
        self.persist_settings_config(cx);
        cx.notify();
    }

    pub(crate) fn active_settings(&self) -> Option<&SettingsState> {
        self.settings.as_ref()
    }

    pub(crate) fn active_settings_mut(&mut self) -> Option<&mut SettingsState> {
        self.settings.as_mut()
    }
}

#[cfg(test)]
mod tests {
    use super::wd_path_saveable;
    use crate::core::config::{Config, ShellConfig};
    use crate::ui::settings::{SettingsSection, test_window};
    use gpui_kit::{Focusable as _, TestAppContext};

    #[test]
    fn a_start_in_path_saves_only_when_it_names_a_real_directory() {
        // Empty is "unset", not a broken path.
        assert!(wd_path_saveable(""));
        assert!(wd_path_saveable("   "));
        let real = std::env::temp_dir();
        let real = real.to_str().expect("temp dir is utf-8 here");
        assert!(wd_path_saveable(real), "{real} exists");
        assert!(
            wd_path_saveable(&format!("  {real}  ")),
            "the commit trims, so the check trims too"
        );
        assert!(!wd_path_saveable("/definitely/not/a/real/dir"));
        // A file is not a directory the shell can start in.
        let file = std::env::temp_dir().join("lyrel-wd-saveable-probe");
        std::fs::write(&file, b"x").expect("write probe file");
        assert!(!wd_path_saveable(file.to_str().expect("utf-8")));
        let _ = std::fs::remove_file(&file);
    }

    #[gpui_kit::test]
    fn shell_field_enter_preserves_argv_and_invalid_drafts(cx: &mut TestAppContext) {
        crate::core::config::pin_test_config_dir();
        let (workspace, mut vcx) = crate::ui::app::test_window::harness(cx);
        let args: Vec<String> = [
            "-c",
            "echo hi",
            "",
            "it's \"quoted\"",
            r"C:\Program Files\lyrel",
            "#1",
        ]
        .into_iter()
        .map(str::to_string)
        .collect();
        vcx.update(|_, cx| {
            cx.global_mut::<Config>().shell = Some(ShellConfig {
                program: "lyrel-parser-shell".into(),
                args: args.clone(),
            });
        });
        let settings = test_window::open(&workspace, &mut vcx);
        settings.update_in(&mut vcx, |settings, window, cx| {
            settings.open_settings_section(SettingsSection::Terminal, window, cx);
            let input = settings.active_settings().unwrap().shell_args_input.clone();
            window.focus(&input.read(cx).focus_handle(cx), cx);
        });
        vcx.run_until_parked();
        vcx.simulate_keystrokes("enter");
        vcx.run_until_parked();
        vcx.update(|_, cx| {
            assert_eq!(cx.global::<Config>().shell.as_ref().unwrap().args, args);
        });

        settings.update_in(&mut vcx, |settings, window, cx| {
            let state = settings.active_settings().unwrap();
            let program = state.shell_program_input.clone();
            let input = state.shell_args_input.clone();
            program.update(cx, |program, cx| {
                program.set_value("lyrel-parser-replacement", window, cx)
            });
            input.update(cx, |input, cx| {
                input.set_value("-c 'unfinished", window, cx)
            });
            window.focus(&input.read(cx).focus_handle(cx), cx);
        });
        vcx.run_until_parked();
        vcx.simulate_keystrokes("enter");
        vcx.run_until_parked();
        settings.update_in(&mut vcx, |settings, _, cx| {
            let shell = cx.global::<Config>().shell.as_ref().unwrap();
            assert_eq!(shell.program, "lyrel-parser-replacement");
            assert_eq!(shell.args, args, "invalid arguments retain the saved argv");
            assert_eq!(
                settings
                    .active_settings()
                    .unwrap()
                    .shell_args_input
                    .read(cx)
                    .value()
                    .as_str(),
                "-c 'unfinished",
                "the invalid draft stays editable"
            );
        });
    }
}
