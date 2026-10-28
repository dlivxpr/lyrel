use super::*;

impl SettingsWindow {
    /// Every family the settings font menus offer, with the configured ones
    /// kept even when the system no longer has them — a menu that cannot show
    /// the current value cannot say what it is.
    fn settings_font_names(cx: &App) -> Vec<String> {
        let cfg = cx.global::<Config>();
        let mut names = cx.text_system().all_font_names();
        for configured in [
            Some(&cfg.font_family),
            cfg.ui_font_family.as_ref(),
            cfg.font_family_bold.as_ref(),
            cfg.font_family_italic.as_ref(),
        ]
        .into_iter()
        .flatten()
        {
            if !names.contains(configured) {
                names.push(configured.clone());
            }
        }
        names.sort_unstable();
        names.dedup();
        names
    }

    /// Build the settings page's state against `window` — the window it will
    /// be drawn in, which owns its inputs' focus.
    pub(crate) fn build_settings_state(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let focus_handle = cx.focus_handle();
        let mut subs = Vec::new();
        let font_names = std::rc::Rc::new(Self::settings_font_names(cx));
        let (shell_program_input, shell_args_input, wd_path_input) =
            self.build_shell_inputs(&mut subs, window, cx);
        let link_file_command_input = self.build_link_file_command_input(&mut subs, window, cx);
        let http_proxy_input = self.build_http_proxy_input(&mut subs, window, cx);
        let skills = self.build_skills_state(&mut subs, window, cx);
        // One query box for whichever popover is open — a theme list or a
        // font list — since only one is ever open at a time.
        // Shared by every searchable dropdown on the page, so the hint is the
        // generic one; without it the field was a bare caret under the list.
        let menu_query =
            cx.new(|cx| InputState::new(window, cx).placeholder(t(L10nKey::SearchTheme)));
        subs.push(
            cx.subscribe_in(&menu_query, window, |this, input, ev, _w, cx| {
                if matches!(ev, InputEvent::Change) {
                    // A new query starts from the top; the clear that opens a
                    // popover keeps the row it opened on.
                    let typed = !input.read(cx).value().is_empty();
                    if let Some(s) = this.active_settings_mut().filter(|_| typed) {
                        s.menu_hi = 0;
                    }
                    cx.notify();
                }
            }),
        );
        let agent_query =
            cx.new(|cx| InputState::new(window, cx).placeholder(t(L10nKey::SettingsSearchAgents)));
        subs.push(
            cx.subscribe_in(&agent_query, window, |_this, _i, ev, _w, cx| {
                if matches!(ev, InputEvent::Change) {
                    cx.notify();
                }
            }),
        );
        let settings_search = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t(crate::ui::i18n::L10nKey::SearchSettings))
        });
        subs.push(
            cx.subscribe_in(&settings_search, window, |this, _i, ev, _w, cx| {
                if matches!(ev, InputEvent::Change) {
                    this.autoselect_settings_search(cx);
                    if let Some(s) = this.active_settings_mut() {
                        s.reveal_first_hit.set(true);
                    }
                    cx.notify();
                }
            }),
        );

        let shortcut_search = cx
            .new(|cx| InputState::new(window, cx).placeholder(t(L10nKey::SettingsSearchShortcuts)));
        subs.push(
            cx.subscribe_in(&shortcut_search, window, |_, _, ev, _, cx| {
                if matches!(ev, InputEvent::Change) {
                    cx.notify();
                }
            }),
        );


        let content_scroll = gpui_kit::ScrollHandle::new();
        content_scroll.set_offset(self.last_settings_location.1);
        let search_anchor = gpui_kit::ScrollAnchor::for_handle(content_scroll.clone());
        let usage = cx.new(|cx| crate::ui::settings::usage::UsagePage::new(window, cx));
        // Account forms render above the whole settings shell, outside UsagePage.
        subs.push(cx.observe(&usage, |_, _, cx| cx.notify()));
        if self.last_settings_location.0 == SettingsSection::Usage {
            usage.update(cx, |page, cx| page.set_visible(true, cx));
        }

        self.settings = Some(SettingsState {
            focus_handle: focus_handle.clone(),
            section: self.last_settings_location.0,
            usage,
            search: settings_search,
            skills,
            shortcut_search,
            modified_only: false,
            search_active: false,
            search_return_offset: self.last_settings_location.1,
            search_selection: 0,
            search_rows: std::cell::RefCell::new(None),
            focused_setting: None,
            content_scroll,
            search_anchor,
            reveal_first_hit: Cell::new(false),
            font_names,
            shell_program_input,
            shell_args_input,
            wd_path_input,
            link_file_command_input,
            http_proxy_input,
            theme_editor: None,
            theme_draft: None,
            theme_draft_error: None,
            save_error: None,
            saved_config: cx.global::<Config>().clone(),
            menu: None,
            menu_query,
            menu_hi: 0,
            menu_scroll: gpui_kit::ScrollHandle::new(),
            focused: std::cell::RefCell::new(None),
            kb_conflict: None,
            recording: None,
            rebinding_note: None,
            ssh: ssh::SshSettings::new(window, cx),
            agent_hooks_host: crate::ui::host_ops::HostId::LOCAL,
            agent_hooks_states: crate::ui::settings::AgentHooksView::Loading,
            agent_hooks_seq: 0,
            agent_hooks_note: None,
            agent_query,
            agent_show_all: false,
            agent_touched: std::collections::HashSet::new(),
            agent_busy: std::collections::HashSet::new(),
            statistics: crate::ui::settings::statistics::StatisticsState::new(window, cx),
            _subs: subs,
        });
        let search_focus = self
            .settings
            .as_ref()
            .map(|s| s.search.read(cx).focus_handle(cx));
        match search_focus {
            Some(handle) => window.focus(&handle, cx),
            None => window.focus(&focus_handle, cx),
        }
        self.rebuild_theme_editor(window, cx);
        self.ensure_agent_hooks_loaded(cx);
        self.skills_section_changed(self.last_settings_location.0, cx);
        self.load_statistics(cx);
        cx.notify();
    }
    pub(crate) fn select_settings_section(
        &mut self,
        target: SettingsSection,
        cx: &mut Context<Self>,
    ) {
        if let Some(s) = self.settings.as_mut() {
            let moved = s.section != target;
            s.section = target;
            s.recording = None;
            // Arriving on a page with a query live means arriving to look for
            // what the nav badge counted, so take the page to it.
            if moved {
                s.reveal_first_hit.set(true);
            }
            if moved {
                s.statistics.invalidate();
            }
            if target == SettingsSection::Agents {
                s.agent_hooks_states = crate::ui::settings::AgentHooksView::Loading;
            }
            s.usage.update(cx, |page, cx| {
                page.set_visible(target == SettingsSection::Usage, cx)
            });
        }
        self.ensure_agent_hooks_loaded(cx);
        self.skills_section_changed(target, cx);
        if target == SettingsSection::Statistics {
            self.load_statistics(cx);
        }
        cx.notify();
    }

    fn ensure_agent_hooks_loaded(&mut self, cx: &mut Context<Self>) {
        if self
            .active_settings()
            .is_some_and(|s| s.section == SettingsSection::Agents)
        {
            self.load_agent_hooks_states(cx);
        }
    }
    pub(crate) fn autoselect_settings_search(&mut self, cx: &mut Context<Self>) {
        if let Some(s) = self.settings.as_mut() {
            let active = !s.search.read(cx).value().trim().is_empty() || s.modified_only;
            if active && !s.search_active {
                s.search_return_offset = s.content_scroll.offset();
            }
            if active {
                s.content_scroll.set_offset(gpui_kit::point(px(0.), px(0.)));
            } else if s.search_active {
                s.content_scroll.set_offset(s.search_return_offset);
            }
            s.search_active = active;
            s.usage.update(cx, |page, cx| {
                page.set_visible(s.section == SettingsSection::Usage && !active, cx)
            });
            s.search_selection = 0;
            if active {
                s.focused_setting = None;
            }
        }
        cx.notify();
    }
}
