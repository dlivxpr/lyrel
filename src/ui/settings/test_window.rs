use crate::ui::app::LyrelApp;
use crate::ui::settings::{SettingsSection, SettingsWindow};
use gpui_kit::{Entity, TestAppContext, VisualTestContext};

pub(crate) fn harness(cx: &mut TestAppContext) -> (Entity<SettingsWindow>, VisualTestContext) {
    let (workspace, mut vcx) = crate::ui::app::test_window::harness(cx);
    let settings = open(&workspace, &mut vcx);
    (settings, vcx)
}

pub(crate) fn open(
    workspace: &Entity<LyrelApp>,
    vcx: &mut VisualTestContext,
) -> Entity<SettingsWindow> {
    workspace.update_in(vcx, |workspace, window, cx| {
        workspace.open_settings_section(SettingsSection::General, window, cx);
    });
    vcx.run_until_parked();
    workspace.read_with(vcx, |workspace, _| {
        workspace.settings.clone().expect("settings opened")
    })
}
