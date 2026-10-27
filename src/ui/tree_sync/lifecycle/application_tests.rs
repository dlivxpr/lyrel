use super::*;
use super::super::*;

/// A real presentation action on an existing tab is new work too: the recovery
/// may refresh its machine baseline, but cannot replace the live terminal view.
#[gpui_kit::test]
fn a_late_recovery_keeps_the_users_new_pip_and_terminal_identity(
    cx: &mut gpui_kit::TestAppContext,
) {
    let (app, mut vcx, _streams) = crate::ui::app::test_window::harness_with_tabs(cx, 1);
    let handle = vcx.window_handle();
    let weak = app.downgrade();
    let (ws, epoch, machine, pane) = app.update_in(&mut vcx, |app, window, cx| {
        let mut view = crate::core::session::WindowView::default();
        view.id = app.workspace;
        let ws = view.id;
        WorkspaceStore::install_for_test(
            cx,
            crate::core::session::WindowViews {
                views: vec![view],
                active: Some(ws),
            },
        );
        crate::ui::windows::WindowRegistry::init(cx);
        crate::ui::windows::WindowRegistry::register(cx, ws, handle, weak);
        let tab = app.tabs[0].id();
        let mut machine_tab = TreeTab::leaf(1);
        machine_tab.id = tab;
        let machine = Machine {
            workspaces: vec![Workspace {
                id: ws,
                tabs: vec![machine_tab],
                active_tab: Some(tab),
                ..Default::default()
            }],
            panes: vec![PaneRecord::new(1)],
        };
        mark_window_informed(cx, ws);
        let (epoch, _) = cx.default_global::<TreeSync>().windows.get_mut(&ws).unwrap()
            .begin_hydration(Adopt::Replace, vec![tab]);
        let pane = app.tabs[0].pane.first_leaf().unwrap();
        window.focus(&pane.focus_handle(cx), cx);
        (ws, epoch, machine, pane)
    });

    vcx.dispatch_action(crate::core::actions::TogglePanePip);
    vcx.update(|_, cx| {
        assert_eq!(app.read(cx).tabs[0].pip_slot(cx).unwrap().entity_id(), pane.entity_id());
        let (machine, mirror, session) = layout_of(machine, ws).unwrap();
        finish_hydration(
            cx,
            ws,
            epoch,
            Adopt::Replace,
            Ok((machine, mirror, session, Arrival::Adopted)),
        );
    });
    vcx.update(|window, cx| {
        let app = app.read(cx);
        assert_eq!(app.tabs.len(), 1);
        assert_eq!(app.tabs[0].pip_slot(cx).unwrap().entity_id(), pane.entity_id());
        assert_eq!(app.tabs[0].pane.first_leaf().unwrap().entity_id(), pane.entity_id());
        assert!(pane.contains_focused(window, cx), "recovery must not steal the user's pane focus");
    });
}
