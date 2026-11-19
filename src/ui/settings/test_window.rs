//! Settings-window test rigs: the `LyrelApp`-backed `harness` for tests that
//! need shell integration, and a standalone rig that builds a
//! `SettingsWindow` directly from a [`SettingsContext`] over an in-memory
//! credential store — no `LyrelApp`, no OS keychain — for seam tests that
//! subscribe to [`SettingsEvent`] in place of the production bridge.
use crate::core::keychain::InMemoryCredentialStore;
use crate::ui::app::LyrelApp;
use crate::ui::settings::{SettingsSection, SettingsWindow};
use crate::ui::settings_window::{SettingsContext, SettingsEvent};
use gpui_kit::{AppContext, Entity, Subscription, TestAppContext, VisualTestContext, point, px};
use lyrel_insights::usage::accounts::SecretVault;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

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

/// A `SettingsWindow` standing alone: the window root is the settings entity
/// itself, fed by a [`SettingsContext`] whose credential seam is an
/// [`InMemoryCredentialStore`]. The returned store is the one the SSH and
/// usage pages read and write — nothing here can reach the OS keychain.
pub(crate) fn harness_standalone(
    cx: &mut TestAppContext,
) -> (
    Entity<SettingsWindow>,
    Arc<InMemoryCredentialStore>,
    VisualTestContext,
) {
    lyrel_ui_base::config::pin_test_config_dir();
    cx.executor().allow_parking();
    cx.update(|cx| {
        gpui_kit::component::init(cx);
        #[cfg(test)]
        lyrel_workbench_editor::code_editor::drawio_document::test_support::mark_automated(cx);
        cx.set_global(lyrel_ui_base::config::Config::default());
        cx.set_global(lyrel_ui_base::presentation::SystemAppearance { dark: false });
        lyrel_ui_base::presets::load_registry(cx);
        crate::ui::theme::apply_theme(None, cx);
        lyrel_ui_base::host_registry::HostRegistry::insert(cx, lyrel_core::host::local::shared());
        crate::ui::keymap::init(cx);
        crate::ui::windows::WindowRegistry::init(cx);
        crate::ui::notification_history::install_source_resolver(cx);
        crate::ui::scm_snapshot::install_scm_snapshot_source(cx);
    });
    let credentials = Arc::new(InMemoryCredentialStore::new());
    let usage_vault: Arc<dyn SecretVault> = credentials.clone();
    let context_credentials = credentials.clone();
    let window = cx.add_window(|window, cx| {
        let context = SettingsContext {
            owner: window.window_handle(),
            shells: Rc::new(|_| Vec::new()),
            live_profiles: Rc::new(|_| std::collections::HashSet::new()),
            session_sources: Rc::new(|_, _| Vec::new()),
            credentials: context_credentials.clone(),
            usage_vault: usage_vault.clone(),
        };
        let location = (SettingsSection::General, point(px(0.), px(0.)));
        let view = cx.new(|cx| SettingsWindow::new(context, location, window, cx));
        gpui_kit::component::Root::new(view, window, cx)
    });
    window
        .update(cx, |_, window, _| window.activate_window())
        .unwrap();
    cx.background_executor.run_until_parked();
    let view = window
        .update(cx, |root, _, _| {
            root.view()
                .clone()
                .downcast::<SettingsWindow>()
                .expect("the window root wraps the settings window")
        })
        .unwrap();
    let vcx = VisualTestContext::from_window(window.into(), cx);
    (view, credentials, vcx)
}

/// Records every event the standalone window emits, in place of the
/// production bridge. The subscription lives as long as the returned value.
pub(crate) fn record_events(
    view: &Entity<SettingsWindow>,
    cx: &mut TestAppContext,
) -> (Rc<RefCell<Vec<SettingsEvent>>>, Subscription) {
    let log = Rc::new(RefCell::new(Vec::new()));
    let record = log.clone();
    let subscription = cx.update(|cx| {
        cx.subscribe(view, move |_, event, _| {
            record.borrow_mut().push(event.clone())
        })
    });
    (log, subscription)
}

/// The `config.json` the scratch config dir would hold, if anything saved it.
fn config_json_on_disk() -> Option<String> {
    let path = lyrel_core::core::config::config_path("config.json")?;
    std::fs::read_to_string(path).ok()
}

#[cfg(test)]
mod standalone_tests {
    use super::*;
    use gpui_kit::TestAppContext;

    /// Recording a shortcut must ask the shell to rebind through the event,
    /// not reach for the keymap itself — the standalone window has no shell,
    /// so the event is the observable effect.
    #[gpui_kit::test]
    fn recording_a_shortcut_emits_the_rebind_event(cx: &mut TestAppContext) {
        let (view, _store, mut vcx) = harness_standalone(cx);
        let (events, _sub) = record_events(&view, cx);
        view.update_in(&mut vcx, |view, window, cx| {
            view.select_settings_section(SettingsSection::Keybindings, cx);
            view.start_recording_key("NewTab".to_string(), window, cx);
        });
        vcx.simulate_keystrokes("secondary-shift-f9");
        // The commit lands after `RECORD_COMMIT_DELAY_MS`; pump real time the
        // way the shell-backed recording tests do.
        let until = std::time::Instant::now() + std::time::Duration::from_secs(3);
        loop {
            vcx.background_executor.run_until_parked();
            std::thread::sleep(std::time::Duration::from_millis(20));
            let done = events
                .borrow()
                .iter()
                .any(|event| matches!(event, SettingsEvent::RebindKeymap));
            if done || std::time::Instant::now() > until {
                break;
            }
        }
        let binding = vcx.update(|_, cx| {
            serde_json::to_value(
                cx.global::<lyrel_ui_base::config::Config>()
                    .keybindings
                    .get("NewTab"),
            )
            .expect("a binding serializes")
        });
        assert_eq!(
            binding,
            serde_json::json!(["secondary-shift-f9"]),
            "the recorded chord is persisted into the config"
        );
        assert!(
            events
                .borrow()
                .iter()
                .any(|event| matches!(event, SettingsEvent::RebindKeymap)),
            "the rebind request must reach the shell through SettingsEvent"
        );
        view.update_in(&mut vcx, |view, _, _| {
            assert!(
                !view.recording_shortcut(),
                "capture ends once the chord commits"
            );
        });
    }

    /// Arrowing through a theme slot menu previews the highlighted preset in
    /// memory only: no `config.json` write until the pick is confirmed, and
    /// the slots the preview replaced come back when the menu closes without
    /// one. The subscribing "bridge" below stands in for
    /// `src/ui/app/settings_bridge.rs`, which is what persists on
    /// `ApplyTheme { persist: true }`.
    #[gpui_kit::test]
    fn theme_menu_preview_restores_on_cancel_and_persists_on_confirm(cx: &mut TestAppContext) {
        let (view, _store, mut vcx) = harness_standalone(cx);
        let (events, _events_sub) = record_events(&view, cx);
        // The bridge half of the seam: repaint nothing here, only save on a
        // committed apply.
        let _bridge_sub = cx.update(|cx| {
            cx.subscribe(&view, |_, event, cx| {
                if let SettingsEvent::ApplyTheme { persist: true, .. } = event {
                    cx.global::<lyrel_ui_base::config::Config>()
                        .try_save()
                        .expect("the scratch config saves");
                }
            })
        });

        let original = vcx.update(|_, cx| {
            cx.global_mut::<lyrel_ui_base::config::Config>()
                .theme_preset = "dark".to_string();
            "dark".to_string()
        });
        view.update_in(&mut vcx, |view, window, cx| {
            view.open_settings_section(SettingsSection::Appearance, window, cx);
            view.toggle_settings_menu("theme-slot-dark".into(), 0, false, window, cx);
        });
        // Preview: arrow down twice — dark → dracula → harbor — nothing saved.
        view.update_in(&mut vcx, |view, _, cx| {
            view.move_theme_menu_highlight(true, true, cx);
            view.move_theme_menu_highlight(true, true, cx);
        });
        let previewed = vcx.update(|_, cx| {
            cx.global::<lyrel_ui_base::config::Config>()
                .theme_preset
                .clone()
        });
        assert_eq!(previewed, "harbor", "the highlight lives in memory");
        assert!(
            events
                .borrow()
                .iter()
                .any(|event| matches!(event, SettingsEvent::ApplyTheme { persist: false, .. })),
            "each arrow emits the non-persisting preview apply"
        );
        assert!(
            !events
                .borrow()
                .iter()
                .any(|event| matches!(event, SettingsEvent::ApplyTheme { persist: true, .. })),
            "previews never ask for a save"
        );
        let on_disk = config_json_on_disk().unwrap_or_default();
        assert!(
            !on_disk.contains("harbor"),
            "arrowing through the menu must not write config.json"
        );

        // Cancel: closing the menu without a pick puts the original back.
        view.update_in(&mut vcx, |view, window, cx| {
            view.close_settings_menu(window, cx);
        });
        let restored = vcx.update(|_, cx| {
            cx.global::<lyrel_ui_base::config::Config>()
                .theme_preset
                .clone()
        });
        assert_eq!(restored, original, "cancel restores the applied preset");
        assert!(
            config_json_on_disk()
                .unwrap_or_default()
                .contains("\"dark\"")
                || config_json_on_disk().is_none(),
            "a cancel still writes nothing"
        );

        // Confirm: Escape's counterpart — Enter — commits the highlighted
        // preset, and only that write reaches config.json.
        view.update_in(&mut vcx, |view, window, cx| {
            view.toggle_settings_menu("theme-slot-dark".into(), 0, false, window, cx);
            view.move_theme_menu_highlight(true, true, cx);
            // What the popover's Enter handler runs:
            view.close_settings_menu(window, cx);
            view.pick_slot_theme(true, "dracula", window, cx);
        });
        let committed = vcx.update(|_, cx| {
            cx.global::<lyrel_ui_base::config::Config>()
                .theme_preset
                .clone()
        });
        assert_eq!(committed, "dracula", "the confirmed pick sticks");
        assert!(
            config_json_on_disk().is_some_and(|json| json.contains("dracula")),
            "the confirming pick is what writes config.json"
        );
        assert!(
            events
                .borrow()
                .iter()
                .any(|event| matches!(event, SettingsEvent::ApplyTheme { persist: true, .. })),
            "the commit asks the shell to persist"
        );
    }
}
