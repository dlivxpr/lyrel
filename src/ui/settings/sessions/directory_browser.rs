//! 设置 → 会话：SSH 主机的只读目录浏览器。本机挑目录走系统文件夹选择
//! 器（`Platform::prompt_for_paths`，OS 原生对话框）；SSH 主机在这里按
//! Host 契约浏览 —— 每一次列目录都经 `HostOps` 在所属主机上执行，客户
//! 端只按字符串持有远端路径，绝不用本机文件语义解释它们（AGENTS.md 的
//! Host 边界）。弹层可取消；异步结果落地时校验设置会话、主机、连接
//! 身份与旅程令牌，任何一项对不上就丢弃 —— 取消再开不会让旧旅程的数字
//! 复活。

use super::*;
use gpui_kit::component::FocusTrapElement as _;

/// SSH 主机上的只读目录浏览弹层。
///
/// `token` 来自 `SessionsState` 的单调计数器：每次打开与每次导航都换
/// 一个全新的值。它比「第几次导航」可靠 —— 取消后再打开，旧旅程的任
/// 何代际数字都不再出现第二次，迟到答案自然失效。
pub(crate) struct SessionDirectoryBrowser {
    host: HostId,
    connection: lyrel_ui_base::host_ops::SharedHost,
    /// 当前目录，按远端拼写原样持有。
    pub(crate) current: PathBuf,
    /// 这台主机上线时发布过的 home；没有它就只隐藏 Home 导航。
    home: Option<PathBuf>,
    pub(crate) entries: Vec<lyrel_core::host::Entry>,
    pub(crate) loading: bool,
    pub(crate) error: Option<String>,
    pub(crate) token: u64,
    /// 弹层的焦点句柄：打开时把焦点挪进来，Escape/Enter 才稳定到达
    /// 卡片；取消／选定时焦点还给 `return_focus`。
    focus: gpui_kit::FocusHandle,
    /// 打开弹层时的焦点，取消／选定时还回去。
    return_focus: Option<gpui_kit::FocusHandle>,
}

impl SessionDirectoryBrowser {
    fn windows_paths(&self) -> bool {
        self.connection.separator() == '\\'
    }

    /// 只有当前目录的列表成功落地才允许选定 —— 加载中或列失败时都
    /// 不该把未知状态的目录写进筛选。
    fn ready(&self) -> bool {
        !self.loading && self.error.is_none()
    }
}

/// 按所属主机的分隔符语义取 `path` 的父目录；已经到根（或路径里根本
/// 没有可退的分隔符）返回 `None`。只按字符串切分 —— 不用本机
/// `Path::parent` 解释远端路径；Windows 主机两种分隔符都认，其余只认
/// `/`，与 `agent_history::directory_matches_with` 的拆分规则一致。
/// `lyrel-core` 的 `remote_parent` 只认 `/`，不覆盖 Windows 主机。
pub(crate) fn host_parent(path: &Path, windows_paths: bool) -> Option<PathBuf> {
    let path = path.to_string_lossy();
    let is_sep = |c: char| c == '/' || (windows_paths && c == '\\');
    let trimmed = path.trim_end_matches(is_sep);
    if trimmed.is_empty() {
        return None;
    }
    let index = trimmed.rfind(is_sep)?;
    let drive_root = windows_paths
        && index == 2
        && trimmed.as_bytes()[0].is_ascii_alphabetic()
        && trimmed.as_bytes()[1] == b':';
    let end = if index == 0 || drive_root {
        index + 1
    } else {
        index
    };
    Some(PathBuf::from(&trimmed[..end]))
}

impl SettingsWindow {
    /// 「选择目录」入口：本机弹系统文件夹选择器；SSH 主机打开只读浏
    /// 览器，从该主机发布过的 home 开始（拿不到 home 就从根开始，列不
    /// 列得动由那台主机回答）。
    pub(crate) fn choose_sessions_directory(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(settings) = self.active_settings() else {
            return;
        };
        let host = settings.sessions.host;
        if host.is_local() {
            let generation = settings.sessions.generation;
            let session = settings.navigation.session();
            let rx = cx.prompt_for_paths(gpui_kit::PathPromptOptions {
                files: false,
                directories: true,
                multiple: false,
                prompt: Some(t(L10nKey::PanelSessionsChooseDirectory).into()),
            });
            cx.spawn_in(window, async move |this, cx| {
                // 系统对话框开着的时候用户可能已经切了主机：落地时重新
                // 校验作用域，本地路径只落到还是本机的作用域上。
                if let Ok(Ok(Some(mut paths))) = rx.await
                    && let Some(directory) = paths.pop()
                {
                    let _ = this.update_in(cx, |this, _window, cx| {
                        if let Some(settings) = this.active_settings_mut().filter(|s| {
                            s.navigation.accepts(session)
                                && s.sessions.generation == generation
                                && s.sessions.host == host
                        }) {
                            settings.sessions.set_directory(Some(directory));
                        }
                        cx.notify();
                    });
                }
            })
            .detach();
            return;
        }
        let Some(connection) = HostRegistry::lookup(cx, host).filter(|host| host.is_connected())
        else {
            return;
        };
        let home = crate::ui::desktop_lookups::home_for_host(cx, host);
        let start = home.clone().unwrap_or_else(|| PathBuf::from("/"));
        let focus = cx.focus_handle();
        if let Some(settings) = self.active_settings_mut() {
            settings.sessions.directory_browser_seq += 1;
            let token = settings.sessions.directory_browser_seq;
            settings.sessions.directory_browser = Some(SessionDirectoryBrowser {
                host,
                connection: connection.clone(),
                current: start.clone(),
                home,
                entries: Vec::new(),
                loading: true,
                error: None,
                token,
                focus: focus.clone(),
                return_focus: window.focused(cx),
            });
        }
        window.focus(&focus, cx);
        cx.notify();
        self.browse_sessions_directory(start, window, cx);
    }

    /// 在浏览器里列出 `target`（在所属主机的线程池上执行，落地校验旅
    /// 程令牌）。每次导航先换新令牌，再发请求。
    fn browse_sessions_directory(
        &mut self,
        target: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(settings) = self.active_settings_mut() else {
            return;
        };
        let Some(browser) = settings.sessions.directory_browser.as_mut() else {
            return;
        };
        settings.sessions.directory_browser_seq += 1;
        browser.token = settings.sessions.directory_browser_seq;
        browser.current = target.clone();
        browser.loading = true;
        browser.error = None;
        let session = settings.navigation.session();
        let token = browser.token;
        let host = browser.host;
        let owner = browser.connection.clone();
        HostOps::run_in(
            owner.clone(),
            window,
            cx,
            move |host| host.read_dir(&target, None),
            move |this, result, _window, cx| {
                this.sessions_directory_landed(session, token, host, owner, result, cx);
            },
        );
    }

    /// 列目录落地：设置会话、当前主机、连接身份、旅程令牌全部对得上
    /// 才写回浏览器；对不上的答案是旧旅程的，丢弃。
    pub(crate) fn sessions_directory_landed(
        &mut self,
        session: gpui_kit::EntityId,
        token: u64,
        host: HostId,
        connection: lyrel_ui_base::host_ops::SharedHost,
        result: std::io::Result<Vec<lyrel_core::host::Entry>>,
        cx: &mut Context<Self>,
    ) {
        let Some(settings) = self.active_settings_mut() else {
            return;
        };
        if !settings.navigation.accepts(session) || settings.sessions.host != host {
            return;
        }
        let Some(browser) = settings.sessions.directory_browser.as_mut() else {
            return;
        };
        if browser.token != token
            || browser.host != host
            || !std::sync::Arc::ptr_eq(&browser.connection, &connection)
        {
            return;
        }
        match result {
            Ok(mut entries) => {
                browser.loading = false;
                browser.error = None;
                // 只读浏览、只要目录；软链目标不明，跳过免得点进一个文件。
                entries.retain(|entry| entry.is_dir && !entry.is_symlink);
                entries.sort_by(|a, b| a.name.cmp(&b.name));
                browser.entries = entries;
            }
            Err(error) => {
                browser.loading = false;
                browser.error = Some(error.to_string());
            }
        }
        cx.notify();
    }

    /// Escape / 取消：丢掉浏览器。进行中的列目录会因令牌对不上而丢弃，
    /// 焦点还给打开弹层的那个控件。
    pub(crate) fn cancel_sessions_directory(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let return_focus = self
            .active_settings_mut()
            .and_then(|settings| settings.sessions.directory_browser.take())
            .and_then(|browser| browser.return_focus);
        if let Some(handle) = return_focus {
            window.focus(&handle, cx);
        }
        cx.notify();
    }

    /// 选定当前目录作为筛选，并关掉浏览器。选定前重新校验这台主机还
    /// 连着、还是原来那个连接 —— 重连过的主机不再替旧浏览的答案背书。
    pub(crate) fn confirm_sessions_directory(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(settings) = self.active_settings_mut() else {
            return;
        };
        let Some(browser) = settings.sessions.directory_browser.as_ref() else {
            return;
        };
        if !browser.ready() {
            return;
        }
        let host = browser.host;
        let directory = browser.current.clone();
        let live = HostRegistry::lookup(cx, host)
            .filter(|current| current.is_connected())
            .is_some_and(|current| std::sync::Arc::ptr_eq(&current, &browser.connection));
        if !live {
            settings.sessions.directory_browser = None;
            cx.notify();
            return;
        }
        let return_focus = settings
            .sessions
            .directory_browser
            .take()
            .and_then(|browser| browser.return_focus);
        settings.sessions.set_directory(Some(directory));
        if let Some(handle) = return_focus {
            window.focus(&handle, cx);
        }
        cx.notify();
    }

    /// 目录浏览弹层：当前路径 + Home / 上级导航 + 子目录列表，页脚取
    /// 消 / 选定。整层只发 `read_dir`，不写任何东西。Escape 取消、
    /// Enter 在列表成功落地后选定。
    pub(crate) fn render_sessions_directory_overlay(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let settings = self.active_settings()?;
        let browser = settings.sessions.directory_browser.as_ref()?;
        let tk = Tk::of(cx);
        let title = t(L10nKey::PanelSessionsChooseDirectory);

        let home = browser.home.clone().filter(|home| home != &browser.current);
        let parent = host_parent(&browser.current, browser.windows_paths());

        let mut body = v_flex()
            .w_full()
            .min_h_0()
            .gap(px(8.))
            .child(
                div()
                    .id("sessions-directory-current")
                    .test_support()
                    .w_full()
                    .px(px(8.))
                    .py(px(6.))
                    .rounded(px(7.))
                    .bg(crate::ui::dialog::well_fill(cx))
                    .font_family(kit::Tk::mono(cx))
                    .text_size(fs(12.))
                    .truncate()
                    .child(browser.current.to_string_lossy().into_owned()),
            )
            .child(
                h_flex()
                    .items_center()
                    .gap(px(6.))
                    .child(
                        kit::button(
                            "sessions-directory-home",
                            t(L10nKey::SettingsWdHome),
                            kit::BtnKind::Secondary,
                        )
                        .disabled(home.is_none())
                        .on_click(cx.listener(|this, _, window, cx| {
                            if let Some(home) = this
                                .active_settings()
                                .and_then(|s| s.sessions.directory_browser.as_ref())
                                .and_then(|browser| browser.home.clone())
                            {
                                this.browse_sessions_directory(home, window, cx);
                            }
                        })),
                    )
                    .child(
                        kit::button("sessions-directory-up", "..", kit::BtnKind::Secondary)
                            .disabled(parent.is_none())
                            .on_click(cx.listener(|this, _, window, cx| {
                                if let Some(parent) = this
                                    .active_settings()
                                    .and_then(|s| s.sessions.directory_browser.as_ref())
                                    .and_then(|browser| {
                                        host_parent(&browser.current, browser.windows_paths())
                                    })
                                {
                                    this.browse_sessions_directory(parent, window, cx);
                                }
                            })),
                    ),
            );
        if browser.loading {
            body = body.child(
                div()
                    .id("sessions-directory-loading")
                    .test_support()
                    .px(px(8.))
                    .py(px(10.))
                    .text_size(fs(12.))
                    .text_color(tk.k45)
                    .child(t(L10nKey::SftpLoading)),
            );
        }
        if let Some(error) = &browser.error {
            body = body.child(
                div()
                    .id("sessions-directory-error")
                    .test_support()
                    .px(px(8.))
                    .py(px(10.))
                    .text_size(fs(12.))
                    .text_color(tk.danger)
                    .child(error.clone()),
            );
        }
        if !browser.loading && browser.error.is_none() {
            let mut list = v_flex()
                .id("sessions-directory-entries")
                .test_support()
                .w_full()
                .max_h(px(280.))
                .min_h_0()
                .overflow_y_scroll()
                .gap(px(1.));
            if browser.entries.is_empty() {
                list = list.child(
                    div()
                        .id("sessions-directory-empty")
                        .test_support()
                        .px(px(8.))
                        .py(px(10.))
                        .text_size(fs(12.))
                        .text_color(tk.k45)
                        .child(t(L10nKey::SftpEmptyDirectory)),
                );
            }
            for entry in &browser.entries {
                let name = entry.name.clone();
                let child = browser.connection.join(&browser.current, &entry.name);
                list = list.child(
                    kit::menu_row(
                        gpui_kit::ElementId::Name(gpui_kit::SharedString::from(format!(
                            "sessions-directory-entry-{name}"
                        ))),
                        false,
                        &tk,
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(fs(12.))
                            .child(format!("{name}/")),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.browse_sessions_directory(child.clone(), window, cx);
                    })),
                );
            }
            body = body.child(list);
        }
        let ready = browser.ready();
        let footer = h_flex()
            .flex_1()
            .child(
                kit::button(
                    "sessions-directory-cancel",
                    t(L10nKey::Cancel),
                    kit::BtnKind::Secondary,
                )
                .on_click(cx.listener(|this, _, window, cx| {
                    this.cancel_sessions_directory(window, cx);
                })),
            )
            .child(div().flex_1())
            .child(
                kit::button(
                    "sessions-directory-confirm",
                    t(L10nKey::PanelSessionsChooseDirectory),
                    kit::BtnKind::Primary,
                )
                .disabled(!ready)
                .on_click(cx.listener(|this, _, window, cx| {
                    this.confirm_sessions_directory(window, cx);
                })),
            );
        let card = crate::ui::dialog::card("sessions-directory-dialog", title, 560., cx)
            .max_w_full()
            .max_h_full()
            .child(crate::ui::dialog::header(title, cx))
            .child(
                div()
                    .id("sessions-directory-body")
                    .test_support()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px_3()
                    .py_2()
                    .child(body),
            )
            .child(crate::ui::dialog::footer(cx).child(footer))
            .focus_trap("sessions-directory-focus-trap", &browser.focus)
            .on_key_down(
                cx.listener(|this, event: &gpui_kit::KeyDownEvent, window, cx| {
                    match event.keystroke.key.as_str() {
                        "escape" => {
                            this.cancel_sessions_directory(window, cx);
                            cx.stop_propagation();
                        }
                        "enter" => {
                            // 与按钮同一把闸：列表没成功落地，键盘也选定不了。
                            this.confirm_sessions_directory(window, cx);
                            cx.stop_propagation();
                        }
                        _ => {}
                    }
                }),
            );
        Some(
            div()
                .id("sessions-directory-overlay")
                .test_support()
                .absolute()
                .inset_0()
                .occlude()
                .flex()
                .items_center()
                .justify_center()
                .p_3()
                .bg(crate::ui::theme::overlay_background(cx))
                .child(card)
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_parent_walks_posix_paths_and_stops_at_root() {
        assert_eq!(
            host_parent(Path::new("/home/dev/repo"), false).as_deref(),
            Some(Path::new("/home/dev"))
        );
        assert_eq!(
            host_parent(Path::new("/home"), false).as_deref(),
            Some(Path::new("/"))
        );
        assert_eq!(host_parent(Path::new("/"), false), None);
        assert_eq!(
            host_parent(Path::new("dev/repo"), false).as_deref(),
            Some(Path::new("dev"))
        );
        assert_eq!(host_parent(Path::new("repo"), false), None);
    }

    #[test]
    fn host_parent_walks_windows_paths_and_stops_at_drive_root() {
        assert_eq!(
            host_parent(Path::new("C:\\Users\\dev"), true).as_deref(),
            Some(Path::new("C:\\Users"))
        );
        // 盘符根的父必须保留尾部分隔符，退成「C:」会是另一个语义。
        assert_eq!(
            host_parent(Path::new("C:\\Users"), true).as_deref(),
            Some(Path::new("C:\\"))
        );
        assert_eq!(host_parent(Path::new("C:\\"), true), None);
        assert_eq!(
            host_parent(Path::new("/x"), true).as_deref(),
            Some(Path::new("/"))
        );
        assert_eq!(
            host_parent(Path::new("C:/Users/dev"), true).as_deref(),
            Some(Path::new("C:/Users"))
        );
    }

    #[test]
    fn host_parent_uses_the_owning_hosts_separator_rules() {
        // POSIX 主机上反斜杠是合法文件名字符：「dev\repo」是一个整体段。
        assert_eq!(
            host_parent(Path::new("/home/dev\\repo"), false).as_deref(),
            Some(Path::new("/home"))
        );
        assert_eq!(
            host_parent(Path::new("/home/dev\\repo"), true).as_deref(),
            Some(Path::new("/home/dev"))
        );
        // Windows 主机两种分隔符都认。
        assert_eq!(
            host_parent(Path::new("C:\\Users/dev"), true).as_deref(),
            Some(Path::new("C:\\Users"))
        );
    }
}
