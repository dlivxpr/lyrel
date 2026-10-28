//! General terminal commands, composed after the Host-aware link section.

use super::*;
use gpui_kit::Entity;
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};

pub(super) fn append_terminal_menu(
    view: &Entity<TerminalView>,
    menu: PopupMenu,
    has_selection: bool,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let view = view.read(cx);
    if !view.links.menu_allowed() {
        return menu;
    }
    let menu_focus = view.focus_handle.clone();
    let menu = view.append_link_menu(menu, cx);
    let menu = menu
        .min_w(px(240.))
        .action_context(menu_focus.clone())
        .menu_element_with_disabled(
            Box::new(CopyText),
            !has_selection,
            menu_row_with_hint(t(L10nKey::AppMenuCopy), mac_only("secondary-c")),
        )
        .menu_element_with_disabled(
            Box::new(CutText),
            !has_selection,
            menu_row_with_hint(t(L10nKey::AppMenuCut), mac_only("secondary-x")),
        )
        .menu_element(
            Box::new(PasteText),
            menu_row_with_hint(t(L10nKey::AppMenuPaste), mac_only("secondary-v")),
        )
        .menu_element(
            Box::new(SelectAll),
            menu_row_with_hint(t(L10nKey::AppMenuSelectAll), mac_only("secondary-a")),
        )
        .separator()
        .menu(t(L10nKey::AppMenuFind), Box::new(FindInTerminal))
        // Inside the terminal, what gets cleared goes without
        // saying; the menu bar keeps the full "Clear Scrollback".
        .menu(t(L10nKey::TerminalContextClear), Box::new(ClearScrollback));

    // `fork_label` is lyrel-core's capability probe, and core has no
    // locale table — take the answer, not its English wording.
    let can_fork = view.agent().and_then(|a| a.fork_label()).is_some();
    let fork_ready = can_fork
        && view.remote_context().is_none()
        && view.agent_session().is_some_and(|s| s.session_id.is_some());
    // `None` over a pane with no agent; otherwise whether it knows
    // the command line it was started with.
    let launch_argv_known = view.agent().map(|_| {
        view.agent_session()
            .is_some_and(|s| s.launch_argv.is_some())
    });

    let menu = match (can_fork, fork_ready) {
        (true, true) => {
            let focus = menu_focus.clone();
            menu.separator().submenu(
                t(L10nKey::AppMenuForkSession),
                window,
                cx,
                move |submenu, _window, _cx| {
                    submenu
                        .action_context(focus.clone())
                        .menu(
                            t(L10nKey::AppMenuSplitRight),
                            Box::new(ForkAgentSessionRight),
                        )
                        .menu(t(L10nKey::AppMenuSplitLeft), Box::new(ForkAgentSessionLeft))
                        .menu(t(L10nKey::AppMenuSplitDown), Box::new(ForkAgentSessionDown))
                        .menu(t(L10nKey::AppMenuSplitUp), Box::new(ForkAgentSessionUp))
                },
            )
        }
        (true, false) => menu
            .separator()
            .item(PopupMenuItem::new(t(L10nKey::AppMenuForkSession)).disabled(true)),
        (false, _) => menu,
    };

    // Only over a running agent, and only live when the pane knows
    // the command line it was started with — there is nothing to
    // save otherwise. Beside Fork when both are there.
    let menu = match launch_argv_known {
        Some(known) => {
            let menu = if can_fork { menu } else { menu.separator() };
            menu.menu_with_disabled(
                t(L10nKey::AppMenuSaveAgentLaunchArgs),
                Box::new(SaveAgentLaunchArgs),
                !known,
            )
        }
        None => menu,
    };

    menu.separator()
        .menu(t(L10nKey::AppMenuSplitRight), Box::new(SplitRight))
        .menu(t(L10nKey::AppMenuSplitDown), Box::new(SplitDown))
        .menu(t(L10nKey::AppMenuZoomPane), Box::new(ToggleMaximizePane))
        .separator()
        .menu(t(L10nKey::AppMenuNewTab), Box::new(NewTab))
        .menu(t(L10nKey::AppMenuClosePaneTab), Box::new(CloseActiveTab))
}

pub(crate) fn menu_row_with_hint(
    label: &'static str,
    key: Option<&'static str>,
) -> impl Fn(&mut Window, &mut App) -> gpui_kit::AnyElement {
    move |_window, cx| {
        let hint = key.map(|k| {
            Kbd::new(
                gpui_kit::Keystroke::parse_with_os(k, cx.platform_info().os)
                    .expect("valid static keystroke"),
            )
            .p_0()
            .flex_nowrap()
            .border_0()
            .bg(gpui_kit::transparent_white())
        });
        h_flex()
            .w_full()
            .gap_3()
            .items_center()
            .justify_between()
            .child(label)
            .children(hint)
            .into_any_element()
    }
}

#[cfg(target_os = "macos")]
fn mac_only(key: &'static str) -> Option<&'static str> {
    Some(key)
}
#[cfg(not(target_os = "macos"))]
fn mac_only(_key: &'static str) -> Option<&'static str> {
    None
}
