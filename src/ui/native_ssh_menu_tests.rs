// Production host-menu pointer path, backed by the isolated native SSH Host.
use super::*;
use crate::ui::desktop_lookups;
use gpui_kit::test::{TestEventExt as _, TestQueryExt as _, TestWindowExt as _};
use lyrel_core::daemon::control::{ControlRequest, ReplyOk};

#[gpui_kit::test]
#[ignore = "requires isolated pinned native SSH/SFTP and same-commit server; run alone"]
fn native_ssh_host_menu_disconnect_preserves_other_window(cx: &mut gpui_kit::TestAppContext) {
    let binary = std::path::PathBuf::from(
        std::env::var_os("LYREL_TEST_SSH_LOCAL_SERVER").expect("same-commit server"),
    );
    let fixture = crate::native_ssh_graphics_fixture::NativeSshFixture::start(&binary);
    lyrel_core::core::config::set_config_dir(fixture.local_config_dir());
    let spec = fixture.spec();
    let target = RemoteTarget::direct(&spec.user, &spec.host, spec.port);
    let host = fixture.connect_control_as(
        &target.connection_key(),
        "isolated-menu-client",
        "isolated-menu-window",
    );
    let remote = WorkspaceId::new();
    assert!(matches!(
        host.client()
            .call(ControlRequest::WorkspaceCreate {
                name: Some("isolated menu disconnect".into()),
                workspace: Some(remote)
            })
            .unwrap(),
        ReplyOk::WorkspaceTree(_)
    ));
    struct Owned(
        std::sync::Arc<lyrel_core::host::remote::RemoteHost>,
        WorkspaceId,
    );
    impl Drop for Owned {
        fn drop(&mut self) {
            let _ = self
                .0
                .client()
                .call(ControlRequest::WorkspaceRemove { workspace: self.1 });
        }
    }
    let _owned = Owned(host.clone(), remote);
    let (app, mut vcx) = crate::ui::app::test_window::harness(cx);
    let (other, other_vcx, _streams) = crate::ui::app::test_window::harness_with_tabs(cx, 1);
    let workspace = app.read_with(cx, |app, _| app.workspace);
    let other_workspace = other.read_with(cx, |app, _| app.workspace);
    let handle = vcx.window_handle();
    cx.update(|cx| {
        lyrel_ui_base::i18n::set_locale("en");
        let mut remote_view = lyrel_ui_base::session::WindowView::on_remote(
            lyrel_ui_base::session::RemoteRef::new(target.clone(), remote),
        );
        remote_view.id = workspace;
        remote_view.open = true;
        let local = lyrel_ui_base::session::WindowView {
            id: other_workspace,
            open: true,
            ..Default::default()
        };
        WorkspaceStore::install_for_test(
            cx,
            lyrel_ui_base::session::WindowViews {
                views: vec![remote_view, local],
                active: Some(workspace),
            },
        );
        crate::ui::windows::WindowRegistry::init(cx);
        crate::ui::windows::WindowRegistry::register(cx, workspace, handle, app.downgrade());
        crate::ui::windows::WindowRegistry::register(
            cx,
            other_workspace,
            other_vcx.window_handle(),
            other.downgrade(),
        );
        desktop_lookups::HostLinks::insert(cx, host.clone(), host.home());
    });
    app.update_in(&mut vcx, |app, window, cx| app.open_switcher(window, cx));
    vcx.run_until_parked();
    vcx.update(|window, cx| {
        window.render_frame(cx);
        let more = ("switcher-row-more", workspace.element_key() as usize);
        let more_position = window.find(more).bounds().center();
        window.pointer_move(more_position, None, cx);
        window.click(more, cx);
        let disconnect = window.find_by_label("Disconnect").bounds().center();
        window.pointer_down(disconnect, gpui_kit::MouseButton::Left, cx);
        window.pointer_up(disconnect, gpui_kit::MouseButton::Left, cx);
    });
    vcx.run_until_parked();
    assert_eq!(
        vcx.update(|_, cx| RemoteLinks::status_of(cx, workspace)),
        Some(crate::ui::remote_workspace::RemoteStatus::Disconnected)
    );
    assert_eq!(app.read_with(cx, |app, _| app.workspace), workspace);
    assert_eq!(
        other.read_with(cx, |app, _| (app.workspace, app.tabs.len())),
        (other_workspace, 1),
        "the production menu must not invalidate an unrelated rooted window"
    );
    assert!(vcx.update(|_, cx| desktop_lookups::HostLinks::get(cx, target.host_id()).is_none()));
}
