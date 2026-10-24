use gpui_kit::component::scroll::ScrollbarMode;
use gpui_kit::component::{ActiveTheme, Theme, ThemeMode};
use gpui_kit::{
    App, Background, Hsla, Menu, MenuItem, OsAction, Pixels, Point, Styled, SystemMenuType, Window,
    WindowBackgroundAppearance, linear_color_stop, linear_gradient, point, px, rgb,
};

use crate::core::actions::*;
use crate::core::config::{Config, WindowBackdrop};
use crate::terminal::view::{
    ClearScrollback, CopyText, CutText, FindInTerminal, FindNext, FindPrevious, PasteText,
    RedoEdit, SelectAll, UndoEdit,
};
use crate::ui::i18n::{L10nKey, t};
use crate::ui::presets;
use crate::ui::presets::Fill;
#[cfg(target_os = "windows")]
use std::sync::OnceLock;

/// Where macOS puts the window's traffic lights over the tab strip: centred on
/// the row of chrome tiles they share the bar with. `x` matches `y`, as AppKit's
/// unified toolbar does (19 and 19 in a 52-point bar).
///
/// `y` is the gap between the window's top edge and the *top* of the button
/// frame, and macOS draws those buttons [`TRAFFIC_LIGHT_BUTTON_H`] tall — so the
/// lights' centre falls half that below it. Deriving the number from
/// [`crate::ui::app::title_strip_centre`], the line every title-bar row puts its
/// contents on, is what keeps the lights on the tile line: the value the 40-point bar needed was
/// rewritten by hand when the bar grew to 48, without the border in it, and left
/// the lights half a point — one device pixel on a Retina panel — below
/// everything beside them.
pub(crate) fn traffic_light_position() -> Point<Pixels> {
    point(px(TRAFFIC_LIGHT_INSET), px(TRAFFIC_LIGHT_INSET))
}

/// The height of AppKit's own standard-window-button frame, which is what the
/// lights are drawn inside. It does not move with the bar's height.
pub(crate) const TRAFFIC_LIGHT_BUTTON_H: f32 = 14.;

// The same content centre drives both the native buttons and the rows beside
// them. Keep the inset const so the leading reserve can follow it at compile
// time instead of maintaining a second copy of the bar's border arithmetic.
const TRAFFIC_LIGHT_INSET: f32 = crate::ui::app::TITLE_STRIP_CENTRE - TRAFFIC_LIGHT_BUTTON_H / 2.;

/// Close to zoom: three 14-point buttons on a 20-point pitch.
const TRAFFIC_LIGHTS_W: f32 = 54.;

/// The room the lights take at the bar's leading edge on macOS: their span,
/// with the inset they sit at on either side.
pub(crate) const TRAFFIC_LIGHTS_LEAD: f32 =
    TRAFFIC_LIGHT_INSET + TRAFFIC_LIGHTS_W + TRAFFIC_LIGHT_INSET;

pub(crate) fn set_menus(cx: &mut App) {
    cx.set_menus([
        Menu::new("lyrel").items([
            MenuItem::action(t(L10nKey::AppMenuAbout), About),
            MenuItem::action(t(L10nKey::AppMenuCheckForUpdates), CheckForUpdates),
            MenuItem::separator(),
            MenuItem::action(t(L10nKey::AppMenuSettings), OpenSettings),
            MenuItem::separator(),
            MenuItem::os_submenu(t(L10nKey::AppMenuServices), SystemMenuType::Services),
            MenuItem::separator(),
            MenuItem::action(t(L10nKey::AppMenuHideApp), HideApp),
            MenuItem::action(t(L10nKey::AppMenuHideOthers), HideOthers),
            MenuItem::action(t(L10nKey::AppMenuShowAll), ShowAll),
            MenuItem::separator(),
            MenuItem::action(t(L10nKey::AppMenuQuit), Quit),
        ]),
        Menu::new(t(L10nKey::AppMenuFile)).items([
            MenuItem::action(t(L10nKey::AppMenuNewTab), NewTab),
            MenuItem::action(t(L10nKey::AppMenuNewWorkspace), NewWorkspace),
            MenuItem::action(t(L10nKey::AppMenuNewWorktreeTab), NewWorktreeTab),
            MenuItem::separator(),
            // SSH is one of the reasons to pick lyrel and it had no entry in the
            // menu bar at all — the only routes were ⌘P and Settings, both of
            // which you have to already know about. Same labels as the palette,
            // so there is still one name per thing.
            MenuItem::action(t(L10nKey::CmdSshManageProfiles), OpenSshProfiles),
            MenuItem::action(t(L10nKey::CmdSshReconnect), RestartSshSession),
            MenuItem::separator(),
            MenuItem::action(t(L10nKey::AppMenuSplitRight), SplitRight),
            MenuItem::action(t(L10nKey::AppMenuSplitDown), SplitDown),
            MenuItem::separator(),
            MenuItem::action(t(L10nKey::AppMenuRenameTab), RenameTab),
            MenuItem::action(
                t(L10nKey::AppMenuCopyWorkingDirectory),
                CopyWorkingDirectory,
            ),
            MenuItem::action(t(L10nKey::AppMenuCopySessionId), CopyAgentSessionId),
            MenuItem::action(t(L10nKey::AppMenuForkSession), ForkAgentSession),
            MenuItem::separator(),
            MenuItem::action(t(L10nKey::AppMenuClosePaneTab), CloseActiveTab),
            MenuItem::action(t(L10nKey::AppMenuCloseOtherTabs), CloseOtherTabs),
            MenuItem::action(t(L10nKey::AppMenuCloseTabsRight), CloseTabsToTheRight),
            MenuItem::action(t(L10nKey::AppMenuReopenClosedTab), ReopenClosedTab),
            MenuItem::separator(),
            MenuItem::action(t(L10nKey::AppMenuRenameWorkspace), RenameWorkspace),
            MenuItem::action(t(L10nKey::AppMenuStopWorkspace), StopWorkspace),
            MenuItem::separator(),
            MenuItem::action(t(L10nKey::AppMenuDeleteWorkspace), DeleteWorkspace),
        ]),
        Menu::new(t(L10nKey::AppMenuEdit)).items([
            MenuItem::os_action(t(L10nKey::AppMenuUndo), UndoEdit, OsAction::Undo),
            MenuItem::os_action(t(L10nKey::AppMenuRedo), RedoEdit, OsAction::Redo),
            MenuItem::separator(),
            MenuItem::os_action(t(L10nKey::AppMenuCut), CutText, OsAction::Cut),
            MenuItem::os_action(t(L10nKey::AppMenuCopy), CopyText, OsAction::Copy),
            MenuItem::os_action(t(L10nKey::AppMenuPaste), PasteText, OsAction::Paste),
            MenuItem::os_action(t(L10nKey::AppMenuSelectAll), SelectAll, OsAction::SelectAll),
            MenuItem::separator(),
            MenuItem::action(t(L10nKey::AppMenuFind), FindInTerminal),
            MenuItem::action(t(L10nKey::AppMenuFindNext), FindNext),
            MenuItem::action(t(L10nKey::AppMenuFindPrevious), FindPrevious),
        ]),
        Menu::new(t(L10nKey::AppMenuView)).items([
            MenuItem::action(t(L10nKey::AppMenuSearchEverywhere), TogglePalette),
            MenuItem::action(t(L10nKey::CmdGoToFile), QuickOpenFile),
            MenuItem::action(t(L10nKey::EditorGoToSymbolAction), EditorGoToSymbol),
            MenuItem::action(t(L10nKey::EditorNavigateBack), EditorNavigateBack),
            MenuItem::action(t(L10nKey::EditorNavigateForward), EditorNavigateForward),
            MenuItem::action(t(L10nKey::EditorSplitRight), EditorSplitRight),
            MenuItem::separator(),
            MenuItem::action(t(L10nKey::AppMenuIncreaseFontSize), IncreaseFontSize),
            MenuItem::action(t(L10nKey::AppMenuDecreaseFontSize), DecreaseFontSize),
            MenuItem::action(t(L10nKey::AppMenuResetFontSize), ResetFontSize),
            MenuItem::separator(),
            MenuItem::action(t(L10nKey::AppMenuLeftSidebar), ToggleLeftPanel),
            MenuItem::action(t(L10nKey::AppMenuRightPanel), ToggleRightPanel),
            MenuItem::action(t(L10nKey::AppMenuCodePanel), ToggleCodePanel),
            MenuItem::separator(),
            MenuItem::action(t(L10nKey::CmdSshRemoteFiles), ToggleSftp),
            MenuItem::action(t(L10nKey::CmdSshPortForwarding), ShowSshForwards),
            MenuItem::separator(),
            MenuItem::action(t(L10nKey::AppMenuFocusNextPane), FocusNextPane),
            MenuItem::action(t(L10nKey::AppMenuFocusPreviousPane), FocusPrevPane),
            // No "Zoom Pane" here. It is bound to ⇧⌘↵, and gpui's macOS menu
            // builder has no key equivalent for `enter`: it hands AppKit the
            // literal string "enter", which takes the "e" — so the item drew
            // itself as ⇧⌘E, which is Code Panel's chord, and pressing it
            // opened the code panel. A menu item that teaches the wrong key and
            // points at another command is worse than no menu item. Zoom Pane
            // keeps its place in ⌘P, in a pane's own menu (drawn by us, with
            // the right chord), and on the Keybindings page.
            MenuItem::separator(),
            MenuItem::action(t(L10nKey::AppMenuClearScrollback), ClearScrollback),
            // No "Enter Full Screen" here: AppKit puts its own at the bottom of
            // any menu named View, so ours sat directly above it — the same
            // command twice, under two different shortcuts. ⌘↵ still works and
            // is listed on the Keybindings page.
        ]),
        Menu::new(t(L10nKey::AppMenuWindow)).items(window_menu_items(cx)),
        Menu::new(t(L10nKey::AppMenuHelp)).items([
            MenuItem::action(t(L10nKey::AppMenuDocumentation), OpenDocumentation),
            MenuItem::action(t(L10nKey::AppMenuKeyboardShortcuts), ShowKeyboardShortcuts),
            MenuItem::separator(),
            MenuItem::action(t(L10nKey::AppMenuProjectPage), OpenProject),
            MenuItem::action(t(L10nKey::AppMenuReportIssue), ReportIssue),
            MenuItem::separator(),
            MenuItem::action(t(L10nKey::AppMenuRestartServer), RestartDaemon),
        ]),
    ]);
    #[cfg(target_os = "macos")]
    fix_native_menu_key_equivalents();
}

/// The key equivalent a native menu item should carry instead of `key`, or
/// `None` when gpui already mapped it (or it is a plain character). gpui
/// 0.3.9's `key_to_native` no longer maps `"enter"`/`"tab"` — that mapping
/// was a fork patch — so an item bound to one of those keys would keep the
/// literal word as its keyEquivalent: AppKit renders and matches its first
/// character (⌘E for "enter", ⌘T for "tab"), which is a wrong glyph that can
/// shadow another command's chord. The rewrite uses only the official
/// NSMenuItem API and never touches the binding itself — the keystroke in
/// the keymap stays `secondary-enter`/`shift-tab`, only the menu's native
/// equivalent string is corrected to the control character it means.
///
/// Pure function so the table is unit-testable on every platform.
#[cfg(any(target_os = "macos", test))]
fn native_menu_key_equivalent_fix(key: &str) -> Option<&'static str> {
    match key {
        "enter" => Some("\r"),
        "tab" => Some("\t"),
        _ => None,
    }
}

/// Walks the just-installed main menu and repairs enter/tab key equivalents
/// in place. Runs after every `set_menus`: the menu bar is rebuilt on theme,
/// locale and workspace-roster changes, and each rebuild reintroduces the
/// unmapped strings.
#[cfg(target_os = "macos")]
fn fix_native_menu_key_equivalents() {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSApplication, NSMenu};
    use objc2_foundation::NSString;

    fn walk(menu: &NSMenu) {
        for item in menu.itemArray().iter() {
            if let Some(submenu) = item.submenu() {
                walk(&submenu);
            }
            if let Some(fixed) = native_menu_key_equivalent_fix(&item.keyEquivalent().to_string()) {
                item.setKeyEquivalent(&NSString::from_str(fixed));
            }
        }
    }

    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let app = NSApplication::sharedApplication(mtm);
    if let Some(menu) = app.mainMenu() {
        walk(&menu);
    }
}

fn window_menu_items(cx: &App) -> Vec<MenuItem> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let order = crate::ui::windows::menu_order(cx);
    let store = crate::core::session::WorkspaceStore::all(cx);

    let slot_action = crate::ui::tab_strip::select_workspace_action;

    let mut items = vec![
        MenuItem::action(t(L10nKey::AppMenuMinimize), MinimizeWindow),
        MenuItem::action(t(L10nKey::AppMenuZoom), ZoomWindow),
        MenuItem::separator(),
    ];
    let workspace_start = items.len();
    // Slot order, open and closed interleaved: the item's position is its
    // number and its shortcut, so it cannot move when a window closes (#760).
    // A closed one says so with its age instead.
    for (i, (id, open)) in order.iter().enumerate() {
        let Some(workspace) = store.get(*id) else {
            continue;
        };
        let Some(action) = slot_action(i) else { break };
        let name = crate::ui::machine_mirror::display_name(cx, workspace)
            .unwrap_or_else(|| t(L10nKey::WindowUntitled).to_string());
        let label = if *open {
            name
        } else {
            format!(
                "{}  —  {}",
                name,
                crate::ui::home::relative_time(now, workspace.last_active)
            )
        };
        items.push(MenuItem::Action {
            name: label.into(),
            action,
            os_action: None,
            checked: false,
            disabled: false,
        });
    }
    if items.len() == workspace_start {
        items.push(MenuItem::action(
            t(L10nKey::AppMenuNewWorkspace),
            NewWorkspace,
        ));
    }
    items
}

pub(crate) fn window_background(bg: &presets::ActiveBackground) -> Background {
    window_background_with_alpha(bg, bg.opacity.unwrap_or(1.0))
}

/// The same preset fill with the alpha channel forced to 1 — used by the
/// settings overlay, which must stay opaque (workspace translucency must
/// never show through it) while still rendering the preset's gradient
/// design instead of collapsing to a flat solid color.
pub(crate) fn window_background_opaque(bg: &presets::ActiveBackground) -> Background {
    window_background_with_alpha(bg, 1.0)
}

/// The workspace's own fill: the preset at whatever alpha the window opacity
/// and backdrop material asked for.
pub(crate) fn workspace_background(cx: &App) -> Background {
    match cx.try_global::<presets::ActiveBackground>() {
        Some(bg) => window_background(bg),
        None => cx.theme().background.into(),
    }
}

/// The fill for a full-window overlay (settings, the opened file, the diff
/// view). Always opaque: the overlay covers the whole workspace, so window
/// translucency and the backdrop material must stop at it instead of showing
/// desktop through its text. Overlays pair this with
/// `app::overlay_surface_layers`, because their own opaque fill hides the
/// theme background image the workspace root paints beneath them.
pub(crate) fn overlay_background(cx: &App) -> Background {
    match cx.try_global::<presets::ActiveBackground>() {
        Some(bg) => window_background_opaque(bg),
        None => cx.theme().background.alpha(1.0).into(),
    }
}

fn window_background_with_alpha(bg: &presets::ActiveBackground, alpha: f32) -> Background {
    let stop = |c: u32| -> Hsla {
        let mut h: Hsla = rgb(c).into();
        h.a = alpha;
        h
    };
    match bg.fill {
        Fill::Solid(c) => stop(c).into(),
        Fill::Vertical { top, bottom } => linear_gradient(
            180.,
            linear_color_stop(stop(top), 0.),
            linear_color_stop(stop(bottom), 1.),
        ),
        Fill::Horizontal { left, right } => linear_gradient(
            90.,
            linear_color_stop(stop(left), 0.),
            linear_color_stop(stop(right), 1.),
        ),
    }
}

#[derive(Clone, Copy)]
pub(crate) struct SystemAppearance {
    dark: bool,
}

impl gpui_kit::Global for SystemAppearance {}

fn is_dark(appearance: gpui_kit::WindowAppearance) -> bool {
    matches!(
        appearance,
        gpui_kit::WindowAppearance::Dark | gpui_kit::WindowAppearance::VibrantDark
    )
}

pub(crate) fn refresh_system_appearance(cx: &mut App) {
    let dark = is_dark(cx.window_appearance());
    cx.set_global(SystemAppearance { dark });
}

pub(crate) fn note_system_appearance(window: &Window, cx: &mut App) {
    let dark = is_dark(window.appearance());
    cx.set_global(SystemAppearance { dark });
}

pub(crate) fn system_dark(cx: &App) -> bool {
    cx.try_global::<SystemAppearance>()
        .is_some_and(|appearance| appearance.dark)
}

pub(crate) fn effective_preset_id(cx: &App) -> String {
    let config = cx.global::<Config>();
    if !config.theme_follow_system {
        config.theme_preset.clone()
    } else if system_dark(cx) {
        config.theme_preset_dark.clone()
    } else {
        config.theme_preset_light.clone()
    }
}

/// Default background alpha used while a backdrop material is active and
/// neither the theme nor the config overrides the opacity — a fully opaque
/// fill would hide the material behind it.
pub(crate) const SYSTEM_MATERIAL_OPACITY: f32 = 0.82;

/// What a window's background resolves to, including the material the
/// published gpui 0.3.9 no longer names: `AcrylicBackdrop` (the fork's
/// variant, `DWMSBT_TRANSIENTWINDOW`, never shipped). The app owns this state
/// and drives the native attributes for it itself — see `ui::backdrop` — so
/// the Acrylic setting and its build fallback survive the migration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
// The DWM-material variants exist only for Windows builds and the tests that
// exercise the fallback tables on every platform; elsewhere they are never
// constructed, and gating the variants out would split every match on them.
#[cfg_attr(not(any(target_os = "windows", test)), allow(dead_code))]
pub(crate) enum BackdropAppearance {
    Opaque,
    Transparent,
    Blurred,
    MicaBackdrop,
    MicaAltBackdrop,
    AcrylicBackdrop,
}

impl BackdropAppearance {
    /// What gpui is told. Every variant but Acrylic maps onto the same-named
    /// public variant; Acrylic rides on `Transparent` — a transparent frame
    /// buffer with grayscale text — while `ui::backdrop` selects
    /// `DWMSBT_TRANSIENTWINDOW` itself. The published Windows backend treats
    /// every non-`Opaque` appearance identically for clearing and subpixel
    /// rendering, so the carrier is behaviourally neutral.
    pub(crate) fn as_gpui(self) -> WindowBackgroundAppearance {
        match self {
            Self::Opaque => WindowBackgroundAppearance::Opaque,
            Self::Transparent => WindowBackgroundAppearance::Transparent,
            Self::Blurred => WindowBackgroundAppearance::Blurred,
            Self::MicaBackdrop => WindowBackgroundAppearance::MicaBackdrop,
            Self::MicaAltBackdrop => WindowBackgroundAppearance::MicaAltBackdrop,
            Self::AcrylicBackdrop => WindowBackgroundAppearance::Transparent,
        }
    }

    /// Whether the window content sits over a blur/mica/acrylic layer instead
    /// of plain transparency — the set `material_active` used to match on the
    /// gpui enum, Acrylic included.
    pub(crate) fn is_material(self) -> bool {
        matches!(
            self,
            Self::Blurred | Self::MicaBackdrop | Self::MicaAltBackdrop | Self::AcrylicBackdrop
        )
    }
}

/// The native attribute writes a Windows window needs for a target material,
/// beyond what the published backend's `set_background_appearance` already
/// did. Two attribute groups decide what shows behind the window, and the
/// backend never cleans the group it did not just set:
///
/// - the WCA accent (`SetWindowCompositionAttribute`), which the backend sets
///   for `Opaque`/`Transparent`/`Blurred` but never disables for a DWM
///   material — a live accent keeps DWM from honoring the backdrop type;
/// - `DWMWA_SYSTEMBACKDROP_TYPE` (`DwmSetWindowAttribute`), which the backend
///   sets for Mica/Mica Alt but never clears on the way back to a WCA
///   appearance, and which has no Acrylic value in the published enum at all.
#[cfg(any(target_os = "windows", test))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct MaterialPlan {
    /// Disable the WCA accent (`ACCENT_DISABLED`), so DWM honors the system
    /// backdrop type being requested.
    pub(crate) wca_disable: bool,
    /// Value to write to `DWMWA_SYSTEMBACKDROP_TYPE`: the material to select,
    /// or `DWMSBT_NONE` to clear one set earlier. `None` below build 22621,
    /// where the attribute does not exist.
    pub(crate) dwmsbt: Option<u32>,
    /// Nudge the non-client frame (`SWP_FRAMECHANGED`) so DWM re-renders the
    /// backdrop immediately — what the fork's backend did on every DWM write
    /// and the published one omits, leaving a runtime switch invisible until
    /// the next resize.
    pub(crate) refresh_frame: bool,
}

#[cfg(any(target_os = "windows", test))]
impl MaterialPlan {
    pub(crate) const NOOP: Self = Self {
        wca_disable: false,
        dwmsbt: None,
        refresh_frame: false,
    };
}

/// `DWM_SYSTEMBACKDROP_TYPE` values (dwmapi.h): off, Mica, Acrylic, Mica Alt.
#[cfg(any(target_os = "windows", test))]
pub(crate) const DWMSBT_NONE: u32 = 1;
#[cfg(any(target_os = "windows", test))]
pub(crate) const DWMSBT_MAINWINDOW: u32 = 2;
#[cfg(any(target_os = "windows", test))]
pub(crate) const DWMSBT_TRANSIENTWINDOW: u32 = 3;
#[cfg(any(target_os = "windows", test))]
pub(crate) const DWMSBT_TABBEDWINDOW: u32 = 4;

/// Pure function so the plan table is unit-testable on every platform.
#[cfg(any(target_os = "windows", test))]
pub(crate) fn material_plan(target: BackdropAppearance, build: u32) -> MaterialPlan {
    use BackdropAppearance::*;
    if build < 22_621 {
        // No DWMWA_SYSTEMBACKDROP_TYPE below Windows 11 22H2: nothing was ever
        // settable, so nothing needs clearing, and the appearance table has
        // already fallen back to the WCA family.
        return MaterialPlan::NOOP;
    }
    match target {
        MicaBackdrop | MicaAltBackdrop | AcrylicBackdrop => MaterialPlan {
            wca_disable: true,
            dwmsbt: Some(match target {
                MicaBackdrop => DWMSBT_MAINWINDOW,
                MicaAltBackdrop => DWMSBT_TABBEDWINDOW,
                _ => DWMSBT_TRANSIENTWINDOW,
            }),
            refresh_frame: true,
        },
        Opaque | Transparent | Blurred => MaterialPlan {
            // The backend's WCA arms leave a previously set DWMSBT untouched;
            // without clearing it the old Mica/acrylic keeps fighting the
            // accent that was just requested.
            wca_disable: false,
            dwmsbt: Some(DWMSBT_NONE),
            refresh_frame: true,
        },
    }
}

/// Maps the requested backdrop to the window appearance, falling back
/// down the chain when the OS build predates a material: Mica needs Windows
/// 11 22H2 (build 22621), acrylic needs 1809 (build 17763); below that plain
/// translucency is the best Windows can do. Pure function so the fallback
/// table is unit-testable on every platform.
#[cfg(any(target_os = "windows", test))]
pub(crate) fn windows_background_appearance(
    backdrop: WindowBackdrop,
    blur: bool,
    build: u32,
) -> BackdropAppearance {
    match backdrop {
        WindowBackdrop::Auto => {
            if blur {
                BackdropAppearance::Blurred
            } else {
                BackdropAppearance::Transparent
            }
        }
        WindowBackdrop::Blur => {
            if build >= 17_763 {
                BackdropAppearance::Blurred
            } else {
                BackdropAppearance::Transparent
            }
        }
        WindowBackdrop::Mica => {
            if build >= 22_621 {
                BackdropAppearance::MicaBackdrop
            } else if build >= 17_763 {
                BackdropAppearance::Blurred
            } else {
                BackdropAppearance::Transparent
            }
        }
        WindowBackdrop::MicaAlt => {
            if build >= 22_621 {
                BackdropAppearance::MicaAltBackdrop
            } else if build >= 17_763 {
                BackdropAppearance::Blurred
            } else {
                BackdropAppearance::Transparent
            }
        }
        WindowBackdrop::Acrylic => {
            if build >= 22_621 {
                // Native acrylic (DWMSBT_TRANSIENTWINDOW) on Windows 11 22H2+,
                // applied by the app itself — see `ui::backdrop`.
                BackdropAppearance::AcrylicBackdrop
            } else if build >= 17_763 {
                // Classic WCA acrylic (ACCENT_ENABLE_ACRYLICBLURBEHIND).
                BackdropAppearance::Blurred
            } else {
                BackdropAppearance::Transparent
            }
        }
        WindowBackdrop::Off => BackdropAppearance::Transparent,
    }
}

/// The backdrop presets actually worth offering on a given Windows build.
/// Mica, Mica Alt and native acrylic all need Windows 11 22H2 (22621); below
/// that every one of them collapses onto the same classic WCA blur that
/// `Blur` already offers, so listing them would present several choices that
/// render identically — hiding them beats misrepresenting them. Below 1809
/// (17763) there is no blur API at all, so only plain translucency is left.
/// Pure function so the table is unit-testable on every platform.
#[cfg(any(target_os = "windows", test))]
pub(crate) fn supported_backdrops_for(build: u32) -> &'static [WindowBackdrop] {
    if build >= 22_621 {
        &[
            WindowBackdrop::Auto,
            WindowBackdrop::Blur,
            WindowBackdrop::Mica,
            WindowBackdrop::MicaAlt,
            WindowBackdrop::Acrylic,
            WindowBackdrop::Off,
        ]
    } else if build >= 17_763 {
        &[
            WindowBackdrop::Auto,
            WindowBackdrop::Blur,
            WindowBackdrop::Off,
        ]
    } else {
        &[WindowBackdrop::Auto, WindowBackdrop::Off]
    }
}

#[cfg(target_os = "windows")]
pub(crate) fn supported_backdrops() -> &'static [WindowBackdrop] {
    supported_backdrops_for(windows_build_number())
}

#[cfg(target_os = "windows")]
pub(crate) fn windows_build_number() -> u32 {
    static BUILD: OnceLock<u32> = OnceLock::new();
    *BUILD.get_or_init(|| {
        use windows_sys::Wdk::System::SystemServices::RtlGetVersion;
        use windows_sys::Win32::System::SystemInformation::OSVERSIONINFOW;
        let mut info: OSVERSIONINFOW = unsafe { std::mem::zeroed() };
        // RtlGetVersion is not subject to the app-compat version lying that
        // GetVersionEx is, and unlike GetVersionEx it does not require
        // dwOSVersionInfoSize to be pre-filled; set it anyway so the call
        // matches the documented contract.
        info.dwOSVersionInfoSize = std::mem::size_of_val(&info) as u32;
        let status = unsafe { RtlGetVersion(&mut info) };
        if status == 0 {
            info.dwBuildNumber
        } else {
            // Unknown build: treat it as too old, so the fallback chain picks
            // the most conservative material instead of a silent no-op.
            0
        }
    })
}

pub(crate) fn resolved_background_appearance(
    backdrop: WindowBackdrop,
    blur: bool,
) -> BackdropAppearance {
    #[cfg(target_os = "windows")]
    {
        windows_background_appearance(backdrop, blur, windows_build_number())
    }
    #[cfg(not(target_os = "windows"))]
    {
        // `window_backdrop` is a Windows-only setting and this platform has
        // no UI to change or clear it (only "Reset window overrides", which
        // also clobbers opacity). If any variant pinned the appearance
        // here, the local blur switch would silently show a checked state
        // that isn't rendered — so every variant, including Off and Blur,
        // defers to the legacy toggle. (A Windows-synced "off" therefore
        // does not force transparency on macOS/Linux; the local blur switch
        // stays authoritative.)
        let _ = backdrop;
        if blur {
            BackdropAppearance::Blurred
        } else {
            BackdropAppearance::Transparent
        }
    }
}

/// Whether the resolved backdrop on this platform is actually a material —
/// i.e. the window content sits over a blur/mica layer instead of plain
/// transparency. Derived from `resolved_background_appearance` so the
/// opacity defaulting and the appearance resolution can never disagree: on
/// very old builds `Blur`/`Acrylic` fall back to `Transparent`, and a
/// see-through window with no material behind it gets no special opacity.
///
/// `Auto` never counts, even when the legacy blur toggle resolves it to
/// `Blurred`. It is the default for every config written before this
/// setting existed, and those windows were opaque with opaque sidebars —
/// treating them as materials would silently turn an untouched install
/// see-through on first launch after an update. Only an explicit pick in
/// the backdrop dropdown opts a window into the translucent defaults.
pub(crate) fn material_active(backdrop: WindowBackdrop, blur: bool) -> bool {
    cfg!(target_os = "windows")
        && backdrop != WindowBackdrop::Auto
        && resolved_background_appearance(backdrop, blur).is_material()
}

/// The window's background alpha when neither the config nor the theme sets
/// an explicit opacity: translucent enough to show an active material,
/// fully opaque otherwise. Single source of truth for `apply_theme` and the
/// settings slider, so the two can never drift apart.
pub(crate) fn default_window_opacity(backdrop: WindowBackdrop, blur: bool) -> f32 {
    if material_active(backdrop, blur) {
        SYSTEM_MATERIAL_OPACITY
    } else {
        1.0
    }
}

/// Tint above the shared workspace background, not another native material.
/// Any translucent window needs translucent sidebars: that includes plain
/// transparency, Windows Auto with an opacity override, and macOS native blur.
/// Retains 85% of the terminal's native-background share. Windows needs an
/// alpha budget because gpui-pre-windows 0.3.9 adds destination alpha (ONE)
/// instead of using src-over attenuation; a fixed 0.15 saturates at opacity 0.85.
/// With an opaque window the original semantic surface remains fully opaque.
/// `theme.sidebar` itself stays solid for components on the settings overlay.
pub(crate) fn workspace_surface_color(cx: &App) -> Hsla {
    translucent_surface(cx.theme().sidebar, cx)
}

/// The left tab rail's fill: the one tinted surface in the workspace (see
/// `Neutrals::rail`), with the same translucency rule as
/// [`workspace_surface_color`].
pub(crate) fn rail_surface_color(cx: &App) -> Hsla {
    let base: Hsla = rgb(cx.global::<presets::Surfaces>().rail.base).into();
    translucent_surface(base, cx)
}

fn translucent_surface(base: Hsla, cx: &App) -> Hsla {
    let Some(opacity) = cx
        .try_global::<presets::ActiveBackground>()
        .and_then(|bg| bg.opacity)
        .filter(|opacity| *opacity < 1.0)
    else {
        return base;
    };
    let tint_alpha = if cfg!(target_os = "windows") {
        0.15 * (1.0 - opacity)
    } else {
        0.15
    };
    base.alpha(tint_alpha)
}

/// The backdrop presets offered in the settings dropdown: everything the
/// current build supports, plus the stored value even if it is not
/// supported here (e.g. Mica synced from a newer machine) — so the label
/// always matches what the window actually resolves to instead of quietly
/// showing "Auto" while a fallback appearance is applied.
#[cfg(target_os = "windows")]
pub(crate) fn backdrop_options(current: WindowBackdrop) -> Vec<WindowBackdrop> {
    let mut list = supported_backdrops().to_vec();
    if !list.contains(&current) {
        list.push(current);
    }
    list
}

/// The window's background alpha: the config override, else the theme's,
/// else the backdrop's default.
fn effective_window_opacity(config: &Config, theme: &presets::Theme, blur: bool) -> f32 {
    config
        .window_opacity
        .or(theme.opacity)
        .unwrap_or_else(|| default_window_opacity(config.window_backdrop, blur))
}

/// What a window is actually handed: the resolved backdrop, except that on
/// Windows a fully opaque fill over no DWM material is reported as
/// `Opaque`.
///
/// gpui only rasterizes ClearType (subpixel) glyphs into an `Opaque` window
/// — subpixel coverage cannot be alpha-blended onto whatever is behind a
/// translucent one — so asking for `Transparent` (or the WCA blur) while
/// painting every pixel at alpha 1 silently drops every glyph to grayscale
/// antialiasing for nothing: nothing behind the window can show through
/// anyway (#1120). The DWM materials (Mica / Mica Alt / Acrylic) are left as
/// they are: switching away from one through `Opaque` would not clear the
/// system backdrop type, and they default to a translucent fill regardless.
pub(crate) fn windows_window_appearance(
    resolved: BackdropAppearance,
    opacity: f32,
) -> BackdropAppearance {
    match resolved {
        BackdropAppearance::Transparent | BackdropAppearance::Blurred if opacity >= 1.0 => {
            BackdropAppearance::Opaque
        }
        other => other,
    }
}

fn window_appearance(backdrop: WindowBackdrop, blur: bool, opacity: f32) -> BackdropAppearance {
    let resolved = resolved_background_appearance(backdrop, blur);
    if cfg!(target_os = "windows") {
        windows_window_appearance(resolved, opacity)
    } else {
        resolved
    }
}

/// The appearance the app's material state currently resolves to — the value
/// `apply_theme` applies to every window, `sync_window_backdrop` applies to a
/// freshly opened one, and `replay_window_backdrop` re-applies after a
/// lifecycle event that can drop the window's native material state.
pub(crate) fn resolved_window_backdrop(cx: &App) -> BackdropAppearance {
    let config = cx.global::<Config>();
    let theme = presets::by_id(cx, &effective_preset_id(cx));
    let blur = config.window_blur.unwrap_or(theme.blur);
    let opacity = effective_window_opacity(config, &theme, blur);
    window_appearance(config.window_backdrop, blur, opacity)
}

pub(crate) fn background_appearance(cx: &App) -> WindowBackgroundAppearance {
    resolved_window_backdrop(cx).as_gpui()
}

/// The appearance last handed to each live window, so `apply_theme` can skip
/// a `set_background_appearance` that would change nothing.
///
/// Worth the bookkeeping because the call is no longer cheap: with a DWM
/// material selected, gpui answers it by re-setting
/// `DWMWA_SYSTEMBACKDROP_TYPE` and the app's own sync
/// (`ui::backdrop::sync_native_material`) forces a non-client frame
/// recalculation (`SetWindowPos` with `SWP_FRAMECHANGED`). `apply_theme` runs
/// on *every* `Config` mutation in *every* window, so dragging the opacity
/// slider — which never changes the appearance — would otherwise recalc the
/// frame once per mouse-move sample and visibly stutter the drag.
#[derive(Default)]
struct AppliedAppearance(std::collections::HashMap<gpui_kit::WindowId, BackdropAppearance>);

impl gpui_kit::Global for AppliedAppearance {}

/// Records `appearance` as what `window` was last handed and reports the
/// previous record. Entries for closed windows are dropped first: window ids
/// come from a slotmap and are reused, so a stale entry could otherwise
/// suppress the very first call for a brand-new window.
fn record_applied_appearance(
    window: &Window,
    appearance: BackdropAppearance,
    cx: &mut App,
) -> Option<BackdropAppearance> {
    #[cfg(target_os = "windows")]
    note_window_scale(window, window.scale_factor(), cx);
    let id = window.window_handle().window_id();
    let live: std::collections::HashSet<gpui_kit::WindowId> =
        cx.windows().iter().map(|w| w.window_id()).collect();
    let applied = cx.default_global::<AppliedAppearance>();
    applied.0.retain(|known, _| live.contains(known));
    applied.0.insert(id, appearance)
}

/// Records `appearance` for `window` and reports whether it differs from
/// what that window was last given.
fn take_appearance_change(window: &Window, appearance: BackdropAppearance, cx: &mut App) -> bool {
    let id = window.window_handle().window_id();
    if cx
        .try_global::<AppliedAppearance>()
        .and_then(|applied| applied.0.get(&id).copied())
        == Some(appearance)
    {
        return false;
    }
    record_applied_appearance(window, appearance, cx) != Some(appearance)
}

/// Hands the window the gpui carrier for `appearance` and, on Windows, brings
/// the two native attribute groups (WCA accent, DWM system backdrop) in line
/// with it — the cleanup and Acrylic selection the published backend does not
/// do. Single choke point so a window can never carry half a material switch.
pub(crate) fn apply_window_backdrop(window: &Window, appearance: BackdropAppearance) {
    window.set_background_appearance(appearance.as_gpui());
    #[cfg(target_os = "windows")]
    crate::ui::backdrop::sync_native_material(window, appearance, windows_build_number());
}

/// Hands `appearance` to the window unless the record already matches —
/// unless `replay` forces the hand-off. A lifecycle event (activation, a DPI
/// change) can drop the window's native material state while the resolved
/// value stays the same, and the change gate would swallow exactly the
/// re-apply that is needed; `replay` distinguishes that case from a true
/// appearance change. Returns whether the window was actually handed the
/// appearance, i.e. whether the native writes ran.
fn apply_backdrop_gated(
    window: &Window,
    appearance: BackdropAppearance,
    replay: bool,
    cx: &mut App,
) -> bool {
    if take_appearance_change(window, appearance, cx) || replay {
        apply_window_backdrop(window, appearance);
        return true;
    }
    false
}

/// Applies the currently resolved backdrop to a freshly opened window. Its
/// creation options only carry the gpui carrier (`background_appearance`), so
/// without this a window that opens without its own `apply_theme` pass — the
/// settings window — would keep the carrier's WCA accent instead of the
/// configured material. Cached per window, so repeat calls cost nothing.
/// Returns whether the window was handed the appearance.
pub(crate) fn sync_window_backdrop(window: &Window, cx: &mut App) -> bool {
    let appearance = resolved_window_backdrop(cx);
    apply_backdrop_gated(window, appearance, false, cx)
}

/// Replays the currently resolved backdrop onto a live window after an event
/// that can silently drop the native material state — an activation change or
/// a DPI change. The resolved value did not change (the *window's* state
/// did), so the change gate must not decide here. Windows-only: the
/// app-owned material (`ui::backdrop`) and the DWM drift this counters exist
/// there; other platforms keep the appearance their compositor already
/// holds, so their windows are left alone. Returns whether the native writes
/// ran.
pub(crate) fn replay_window_backdrop(window: &Window, cx: &mut App) -> bool {
    #[cfg(target_os = "windows")]
    {
        let appearance = resolved_window_backdrop(cx);
        apply_backdrop_gated(window, appearance, true, cx)
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (window, cx);
        false
    }
}

/// The scale factor each live window was last seen with, so the bounds
/// observer can tell a DPI change from a plain move or resize.
#[cfg(target_os = "windows")]
#[derive(Default)]
struct WindowBackdropScales(std::collections::HashMap<gpui_kit::WindowId, f32>);

#[cfg(target_os = "windows")]
impl gpui_kit::Global for WindowBackdropScales {}

/// Records `scale` for `window` and reports whether it changed. The first
/// observation only establishes the baseline and reports no change: a freshly
/// opened window was already handed its appearance, so there is nothing to
/// restore yet.
///
/// The bounds observer runs this on every `WM_SIZE`/`WM_MOVE` — once per
/// mouse-move while the window is dragged — so the unchanged case is decided
/// by one map lookup with no allocation; the pruning collect (same
/// slotmap-reuse discipline as `record_applied_appearance`) only runs when
/// the scale actually moved or the window is first seen.
#[cfg(target_os = "windows")]
fn note_window_scale(window: &Window, scale: f32, cx: &mut App) -> bool {
    let id = window.window_handle().window_id();
    // Fast path first: an unchanged scale is one borrowed lookup, no
    // allocation, and the borrow ends before the collect below needs `cx`.
    let previous = cx
        .try_global::<WindowBackdropScales>()
        .and_then(|scales| scales.0.get(&id).copied());
    if previous == Some(scale) {
        return false;
    }
    let live: std::collections::HashSet<gpui_kit::WindowId> =
        cx.windows().iter().map(|w| w.window_id()).collect();
    let scales = cx.default_global::<WindowBackdropScales>();
    scales.0.retain(|known, _| live.contains(known));
    scales.0.insert(id, scale).is_some()
}

/// Replays the material when — and only when — the window's scale factor
/// just changed. A DPI change (a move onto a display with a different one
/// arrives as `WM_DPICHANGED`) recreates the window's frame and can drop the
/// native material, while a plain move or resize keeps it — and must not
/// re-run the DWM writes once per `WM_SIZE`/`WM_MOVE`, the same drag stutter
/// the appearance cache exists to prevent. Returns whether the replay ran.
/// Windows-only for the same reason as `replay_window_backdrop`.
pub(crate) fn replay_window_backdrop_on_scale_change(window: &Window, cx: &mut App) -> bool {
    #[cfg(target_os = "windows")]
    {
        let scale = window.scale_factor();
        if note_window_scale(window, scale, cx) {
            return replay_window_backdrop(window, cx);
        }
        false
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (window, cx);
        false
    }
}

/// The interface face gpui-component started out with, read once before
/// anything overrode it.
///
/// [`Theme::change`] only rewrites `font_family` when the theme config names
/// one, and none of ours does — so a face written into the theme below stays
/// written for the life of the process. Keeping the stock value here is what
/// lets **Interface font family → Default** put the system face back on the
/// spot; without it the setting would save `None`, leave the window looking
/// exactly as it did, and only come true at the next launch.
struct StockUiFont(gpui_kit::SharedString);

impl gpui_kit::Global for StockUiFont {}

pub(crate) fn apply_theme(mut window: Option<&mut Window>, cx: &mut App) {
    let follow = cx.global::<Config>().theme_follow_system;
    if follow {
        sync_native_appearance(None);
        #[cfg(target_os = "macos")]
        refresh_system_appearance(cx);
    }
    let theme = presets::by_id(cx, &effective_preset_id(cx));
    // Cache the mode beside the machine tree: the daemon is a separate process
    // and reads it back when it spawns a Windows pane, where ConPTY drops an
    // OSC 11 background query before lyrel's emulator can answer it. Derived
    // state, so it deliberately stays out of the user's `config.json`.
    crate::core::machine::note_appearance(theme.dark);
    let config = cx.global::<Config>();
    let mode = if theme.dark {
        ThemeMode::Dark
    } else {
        ThemeMode::Light
    };
    let blur = config.window_blur.unwrap_or(theme.blur);
    // A material needs some translucency to be visible; without an explicit
    // opacity override it defaults to SYSTEM_MATERIAL_OPACITY instead of
    // 1.0. Derived from the *resolved* appearance so old builds where
    // Blur/Acrylic fall back to plain transparency stay opaque by default.
    let fill_opacity = effective_window_opacity(config, &theme, blur);
    let opacity = (fill_opacity < 1.0).then_some(fill_opacity);
    if !follow {
        sync_native_appearance(Some(theme.dark));
    }
    let m = theme.neutrals();
    let surfaces = theme.surfaces();
    let sem = theme.semantics();
    let interaction = theme.interactions();
    let active = theme.active_palette(config.theme_legible_palette);

    let backdrop = config.window_backdrop;
    let ui_font_family = config.ui_font_family.clone();
    let ui_font_size = px(config.ui_font_size);

    if let Some(window) = window.as_deref_mut() {
        let appearance = window_appearance(backdrop, blur, fill_opacity);
        apply_backdrop_gated(window, appearance, false, cx);
        // Opacity, blur and backdrop are app-wide settings, but they are
        // changed from whichever window the settings page is in — its own,
        // now. Hand the same appearance to every other window too. The one
        // being updated refuses a nested update, and was handled above.
        for other in cx.windows() {
            let _ = other.update(cx, |_, window, cx| {
                apply_backdrop_gated(window, appearance, false, cx);
            });
        }
    }

    Theme::change(mode, window.as_deref_mut(), cx);
    cx.set_global(active);
    cx.set_global(presets::ActiveBackground {
        fill: theme.background.clone(),
        opacity,
        image: theme.image.clone(),
    });
    cx.set_global(surfaces.clone());
    cx.set_global(interaction);
    cx.set_global(theme.parking());
    cx.set_global(presets::ActiveAccent(m.accent));
    // Same treatment as `Surfaces`: derived once here rather than recomputed
    // in `render`, because the graph reads it once per visible row per frame
    // and each entry costs a contrast bisection on three surfaces.
    cx.set_global(presets::ActiveLanes(theme.lanes()));

    if !cx.has_global::<StockUiFont>() {
        let stock = Theme::global(cx).font_family.clone();
        cx.set_global(StockUiFont(stock));
    }
    let stock = cx.global::<StockUiFont>().0.clone();

    let t = Theme::global_mut(cx);
    t.font_size = ui_font_size;
    // Assigned in both directions, never only when set: this is the one place
    // that decides the chrome's face, so it has to say "back to stock" as
    // plainly as it says "use Inter".
    t.font_family = match ui_font_family {
        Some(family) if !family.trim().is_empty() => family.into(),
        _ => stock,
    };
    let mut base: Hsla = rgb(m.background).into();
    if let Some(o) = opacity {
        base.a = o;
    }
    t.background = base;
    t.foreground = rgb(m.foreground).into();
    t.border = rgb(m.border).into();
    t.secondary = rgb(m.secondary).into();
    // Ink, not fill. gpui-component resolves both of these from its *stock*
    // foreground, so a button's label and the detail panel's section headings
    // were the only text in the window not written in the preset's own colour.
    t.button_foreground = rgb(m.foreground).into();
    t.secondary_foreground = rgb(m.foreground).into();
    t.muted = rgb(m.muted).into();
    t.muted_foreground = rgb(m.muted_foreground).into();
    // Components read both ThemeColor fields and layered tokens. Keep both
    // routes on our palette instead of leaking the component library defaults.
    t.tokens.foreground = Hsla::from(rgb(m.foreground)).into();
    t.tokens.border = Hsla::from(rgb(m.border)).into();
    t.tokens.muted = Hsla::from(rgb(m.muted)).into();
    t.tokens.muted_foreground = Hsla::from(rgb(m.muted_foreground)).into();
    t.tokens.secondary = Hsla::from(rgb(m.secondary)).into();
    t.tokens.secondary_foreground = Hsla::from(rgb(m.foreground)).into();
    t.button = rgb(m.popover).into();
    t.button_secondary = rgb(m.secondary).into();
    t.button_secondary_foreground = rgb(m.foreground).into();
    t.tokens.button = Hsla::from(rgb(m.popover)).into();
    t.tokens.button_foreground = Hsla::from(rgb(m.foreground)).into();
    t.tokens.button_secondary = Hsla::from(rgb(m.secondary)).into();
    t.tokens.button_secondary_foreground = Hsla::from(rgb(m.foreground)).into();
    t.popover = rgb(m.popover).into();
    t.tokens.popover = Hsla::from(rgb(m.popover)).into();
    t.tokens.popover_foreground = Hsla::from(rgb(m.foreground)).into();
    // The field beside the token, not a duplicate of it: gpui-component reads
    // this one for every tooltip, dropdown menu and date picker it draws. Left
    // unset it keeps the stock near-white, so menus and tooltips ignored the
    // preset and came out brighter than the window they float over.
    t.popover_foreground = rgb(m.foreground).into();

    let accent_fill = rgb(interaction.choice);
    let accent_text: Hsla = rgb(interaction.choice_ink).into();
    t.accent = accent_fill.into();
    t.accent_foreground = accent_text;
    t.tokens.accent = Hsla::from(accent_fill).into();
    t.tokens.accent_foreground = accent_text.into();

    let primary_base: Hsla = rgb(interaction.primary.fill).into();
    let primary_hover: Hsla = rgb(interaction.primary_hover).into();
    let primary_active: Hsla = rgb(interaction.primary_pressed).into();
    let primary_text: Hsla = rgb(interaction.primary.on_fill).into();
    t.primary = primary_base;
    t.primary_hover = primary_hover;
    t.primary_active = primary_active;
    t.primary_foreground = primary_text;
    t.button_primary_foreground = primary_text;
    t.tokens.primary = primary_base.into();
    t.tokens.primary_hover = primary_hover.into();
    t.tokens.primary_active = primary_active.into();
    t.tokens.primary_foreground = primary_text.into();
    t.tokens.button_primary = primary_base.into();
    t.tokens.button_primary_hover = primary_hover.into();
    t.tokens.button_primary_active = primary_active.into();
    t.tokens.button_primary_foreground = primary_text.into();

    let steps = |c: u32| {
        (
            Hsla::from(rgb(c)),
            Hsla::from(rgb(presets::mix(c, m.background, 0.15))),
            Hsla::from(rgb(presets::mix(c, m.foreground, 0.15))),
        )
    };

    let (ink, ink_hover, ink_active) = steps(sem.danger.ink);
    let (fill, fill_hover, fill_active) = steps(sem.danger.fill);
    let on_fill = Hsla::from(rgb(sem.danger.on_fill));
    t.danger = ink;
    t.danger_hover = ink_hover;
    t.danger_active = ink_active;
    t.danger_foreground = on_fill;
    t.tokens.danger = fill.into();
    t.tokens.danger_hover = fill_hover.into();
    t.tokens.danger_active = fill_active.into();
    t.tokens.danger_foreground = on_fill.into();
    t.tokens.button_danger = fill.into();
    t.tokens.button_danger_hover = fill_hover.into();
    t.tokens.button_danger_active = fill_active.into();
    t.tokens.button_danger_foreground = on_fill.into();

    let (ink, ink_hover, ink_active) = steps(sem.warning.ink);
    let (fill, fill_hover, fill_active) = steps(sem.warning.fill);
    let on_fill = Hsla::from(rgb(sem.warning.on_fill));
    t.warning = ink;
    t.warning_hover = ink_hover;
    t.warning_active = ink_active;
    t.warning_foreground = on_fill;
    t.tokens.warning = fill.into();
    t.tokens.warning_hover = fill_hover.into();
    t.tokens.warning_active = fill_active.into();
    t.tokens.warning_foreground = on_fill.into();
    t.tokens.button_warning = fill.into();
    t.tokens.button_warning_hover = fill_hover.into();
    t.tokens.button_warning_active = fill_active.into();
    t.tokens.button_warning_foreground = on_fill.into();

    let (ink, ink_hover, ink_active) = steps(sem.success.ink);
    let (fill, fill_hover, fill_active) = steps(sem.success.fill);
    let on_fill = Hsla::from(rgb(sem.success.on_fill));
    t.success = ink;
    t.success_hover = ink_hover;
    t.success_active = ink_active;
    t.success_foreground = on_fill;
    t.tokens.success = fill.into();
    t.tokens.success_hover = fill_hover.into();
    t.tokens.success_active = fill_active.into();
    t.tokens.success_foreground = on_fill.into();
    t.tokens.button_success = fill.into();
    t.tokens.button_success_hover = fill_hover.into();
    t.tokens.button_success_active = fill_active.into();
    t.tokens.button_success_foreground = on_fill.into();

    let (ink, ink_hover, ink_active) = steps(sem.info.ink);
    let (fill, fill_hover, fill_active) = steps(sem.info.fill);
    let on_fill = Hsla::from(rgb(sem.info.on_fill));
    t.info = ink;
    t.info_hover = ink_hover;
    t.info_active = ink_active;
    t.info_foreground = on_fill;
    t.tokens.info = fill.into();
    t.tokens.info_hover = fill_hover.into();
    t.tokens.info_active = fill_active.into();
    t.tokens.info_foreground = on_fill.into();
    t.tokens.button_info = fill.into();
    t.tokens.button_info_hover = fill_hover.into();
    t.tokens.button_info_active = fill_active.into();
    t.tokens.button_info_foreground = on_fill.into();

    // The command-line highlighter paints commands, flags, paths, strings and
    // operators with these, and the find bar flags a bad regex with `red`.
    // Unset they hold gpui-component's stock ramp, one ramp for every dark
    // preset and one for every light one — so the line you were typing kept a
    // palette the output right above it had already left behind.
    t.red = rgb(theme.ansi_ink(1)).into();
    t.green = rgb(theme.ansi_ink(2)).into();
    t.yellow = rgb(theme.ansi_ink(3)).into();
    t.blue = rgb(theme.ansi_ink(4)).into();
    t.magenta = rgb(theme.ansi_ink(5)).into();
    t.cyan = rgb(theme.ansi_ink(6)).into();

    t.link = rgb(sem.link.ink).into();
    t.link_hover = rgb(presets::mix(sem.link.ink, m.foreground, 0.25)).into();
    t.link_active = rgb(presets::mix(sem.link.ink, m.background, 0.20)).into();
    t.tokens.link = Hsla::from(rgb(sem.link.ink)).into();
    t.tokens.link_hover = Hsla::from(rgb(presets::mix(sem.link.ink, m.foreground, 0.25))).into();
    t.tokens.link_active = Hsla::from(rgb(presets::mix(sem.link.ink, m.background, 0.20))).into();

    let knob = if presets::is_lighter(m.background, m.foreground) {
        m.background
    } else {
        m.foreground
    };
    t.tokens.background = Hsla::from(rgb(m.background)).into();
    t.tokens.switch_thumb = Hsla::from(rgb(knob)).into();
    t.tokens.switch = Hsla::from(rgb(surfaces.window.selected)).into();

    // On-switches, sliders and primary actions share the accent role.
    t.tokens.slider_bar = Hsla::from(rgb(m.accent)).into();
    t.tokens.slider_thumb = Hsla::from(rgb(knob)).into();

    t.caret = rgb(m.caret).into();
    t.selection = rgb(m.selection).into();

    let scrollbar_thumb: Hsla = rgb(presets::mix(m.background, m.foreground, 0.18)).into();
    let scrollbar_thumb_hover: Hsla = rgb(presets::mix(m.background, m.foreground, 0.34)).into();
    t.scrollbar = gpui_kit::transparent_black();
    t.scrollbar_thumb = scrollbar_thumb;
    t.scrollbar_thumb_hover = scrollbar_thumb_hover;
    t.tokens.scrollbar = gpui_kit::transparent_black().into();
    t.tokens.scrollbar_thumb = scrollbar_thumb.into();
    t.tokens.scrollbar_thumb_hover = scrollbar_thumb_hover.into();

    // Not `should_auto_hide_scrollbars()`. That preference answers a question
    // about *legacy* scrollbars — the ones that take a gutter out of the layout
    // — and macOS says "don't hide them" for anyone with a mouse plugged in.
    // Ours are overlay bars painted on top of the content, so honouring it
    // parked an opaque bar over the switcher's tab column for the whole time
    // the panel was open, with nothing to fade it out. Every list in the app
    // gets the same bar, so it fades everywhere or nowhere.
    t.scrollbar_mode = ScrollbarMode::Scrolling;

    t.radius = px(8.);

    // Flat controls, floating panels. `Theme::shadow` gates exactly one thing —
    // the `shadow_xs` an inline control (button, input, select trigger,
    // checkbox, radio, slider knob) paints under itself — and never the drop
    // shadow on a menu, tooltip or popover, which every one of those draws
    // unconditionally. Left on, every field and button in the window carried a
    // faint lift that nothing else here has: this chrome separates surfaces with
    // low-contrast fills and hairlines, so a control sitting a millimetre above
    // the panel was the one place claiming depth, and it read as a rendering
    // artefact rather than as a material. Panels that really do float keep
    // their shadow.
    t.shadow = false;

    let sidebar_bg = Hsla::from(rgb(m.sidebar));
    // The current tab is a place, not an action: a neutral step of the rail's
    // own ladder plus a heavier title, never the accent. Blue is left to the
    // things that can be pressed.
    let sidebar_sel = rgb(surfaces.sidebar.selected);
    // `t.sidebar` stays the opaque theme token: the settings theme picker
    // paints with it on top of the (opaque) settings overlay, so diluting
    // it would wash out that panel. The workspace sidebar/right-panel
    // surfaces get their translucent variant at render time instead —
    // see `workspace_surface_color`.
    t.sidebar = sidebar_bg.into();
    t.tokens.sidebar = sidebar_bg.into();
    // The lighter tier: every `sidebar_border` site is a pane meeting another
    // pane, and both already carry their own fill. See `Neutrals`.
    t.sidebar_border = rgb(m.divider).into();
    t.sidebar_foreground = rgb(surfaces.sidebar.text_resting).into();
    t.sidebar_accent = sidebar_sel.into();
    t.tokens.sidebar_accent = Hsla::from(sidebar_sel).into();
    t.sidebar_accent_foreground = rgb(surfaces.sidebar.text_selected).into();

    t.list.active_highlight = true;
    t.list_active = rgb(interaction.choice).into();
    t.list_active_border = rgb(interaction.choice).into();
    t.list_hover = rgb(surfaces.popover.hover).into();

    // gpui-component's `input` is the border role, also used by default
    // buttons. Giving it the field background would erase their outlines.
    t.input = rgb(interaction.input_border).into();
    t.tokens.input = Hsla::from(rgb(interaction.input_border)).into();

    let button_hover: Hsla = rgb(surfaces.window.hover).into();
    let button_active: Hsla = rgb(surfaces.window.selected).into();
    t.tokens.button_hover = button_hover.into();
    t.tokens.button_active = button_active.into();
    t.tokens.secondary_hover = button_hover.into();
    t.tokens.secondary_active = button_active.into();
    t.tokens.button_secondary_hover = button_hover.into();
    t.tokens.button_secondary_active = button_active.into();

    t.ring = rgb(m.accent).into();
    t.tokens.ring = Hsla::from(rgb(m.accent)).into();
    // Every splitter and panel edge lights up with this while you drag it, and
    // the drop target for a dragged-in file tints with it. Unset it is a fixed
    // blue from gpui-component's stock theme — the same blue under all nine
    // presets. It is an interaction highlight, so it belongs with the focus
    // ring and the on-switch.
    t.drag_border = rgb(m.accent).into();

    // Keep the editor body transparent so it inherits gradients and wallpaper.
    // The published editor paints scrolling text before the fixed line-number
    // gutter, so that gutter must be opaque to cover text scrolling beneath it.
    // Match the existing document surface; only this narrow column stops showing
    // the background image. Active-line and invisible ink stay translucent.
    let ink: Hsla = rgb(m.foreground).into();
    let mut highlight = (*t.highlight_theme).clone();
    highlight.style.editor_background = Some(gpui_kit::transparent_black());
    highlight.style.editor_gutter_background = Some(t.sidebar);
    highlight.style.editor_active_line = Some(ink.opacity(0.06));
    highlight.style.editor_line_number = Some(rgb(m.muted_foreground).into());
    highlight.style.editor_active_line_number = Some(ink);
    highlight.style.editor_invisible = Some(ink.opacity(0.25));
    t.highlight_theme = std::sync::Arc::new(highlight);
    Theme::sync_base(cx);

    #[cfg(target_os = "macos")]
    if let Some(window) = window.as_deref_mut() {
        window.set_traffic_light_position(traffic_light_position());
    }
}

/// Shared opaque floating surface. In-window GPUI layers cannot blur the
/// terminal beneath them, so depth comes from the edge and shadow instead.
pub(crate) fn floating_surface<T: Styled>(element: T, cx: &App) -> T {
    let theme = cx.theme();
    element
        .bg(theme.popover)
        .text_color(theme.popover_foreground)
        .border_1()
        .border_color(theme.border.opacity(0.65))
        .rounded(crate::ui::rounding::POPOVER_RADIUS)
        .shadow_xl()
}

/// One device pixel: the v4 hairline, 0.5px on a Retina panel and a whole
/// pixel on a 1x one. A literal `px(0.5)` would be antialiased down to half
/// its ink on a 1x display, under the contrast floor `divider` is held to.
pub(crate) fn hairline(window: &Window) -> Pixels {
    px(1. / window.scale_factor().max(1.))
}

/// Tabular figures, so a column of counts, sizes and positions keeps its
/// digits on one grid while the numbers change under it.
pub(crate) fn tabular_figures() -> gpui_kit::FontFeatures {
    gpui_kit::FontFeatures(std::sync::Arc::new(vec![("tnum".to_string(), 1)]))
}

/// The v4 primary button: an inverted neutral — body ink as the fill, the
/// surface it sits on as the label — rather than the accent, which stays with
/// focus and the things that are on. `surface` is the opaque fill under the
/// button, so the label reads as cut out of it. Disabled, gpui-component
/// drops the fill to 15% of itself, the faint well the rest of v4 uses.
pub(crate) fn inverted_button(
    surface: Hsla,
    cx: &App,
) -> gpui_kit::component::button::ButtonCustomVariant {
    let ink = cx.theme().foreground;
    gpui_kit::component::button::ButtonCustomVariant::new(cx)
        .color(ink)
        .foreground(surface)
        .hover(ink.blend(surface.opacity(0.14)))
        .active(ink.blend(surface.opacity(0.24)))
}

pub(crate) fn apply_cursor_hide_mode(cx: &mut App) {
    let mode = if cx.global::<Config>().mouse_hide_while_typing {
        gpui_kit::CursorHideMode::OnTypingAndAction
    } else {
        gpui_kit::CursorHideMode::Never
    };
    cx.set_cursor_hide_mode(mode);
}

#[cfg(target_os = "macos")]
fn sync_native_appearance(dark: Option<bool>) {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{
        NSAppearance, NSAppearanceNameAqua, NSAppearanceNameDarkAqua, NSApplication,
    };

    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let appearance = dark.and_then(|dark| {
        let name = unsafe {
            if dark {
                NSAppearanceNameDarkAqua
            } else {
                NSAppearanceNameAqua
            }
        };
        NSAppearance::appearanceNamed(name)
    });
    NSApplication::sharedApplication(mtm).setAppearance(appearance.as_deref());
}

#[cfg(not(target_os = "macos"))]
fn sync_native_appearance(_dark: Option<bool>) {}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::TestAppContext;

    /// The lights share a line with the chrome tiles they sit beside. gpui
    /// hangs the top of the button frame `y` below the window's top edge, so
    /// their centre falls half a frame under that, and `title_strip_centre` is
    /// where every title-bar row puts its contents. Sixteen and a half points
    /// today, out of the 48pt bar.
    #[test]
    fn the_traffic_lights_sit_on_the_chrome_rows_centre() {
        let position = traffic_light_position();
        assert_eq!(position.x, position.y);
        assert_eq!(
            position.y + px(TRAFFIC_LIGHT_BUTTON_H / 2.),
            crate::ui::app::title_strip_centre()
        );
    }

    /// The strip, editor header and search band start at the lead, so the
    /// lights have to end, with their inset after them, before it does.
    #[test]
    #[cfg(target_os = "macos")]
    fn the_title_bar_lead_clears_the_traffic_lights() {
        let right = traffic_light_position().x + px(TRAFFIC_LIGHTS_W);
        assert!(right + px(TRAFFIC_LIGHT_INSET) <= px(crate::ui::app::TITLE_BAR_LEAD));
    }

    #[gpui_kit::test]
    fn effective_preset_follows_the_cached_system_appearance(cx: &mut TestAppContext) {
        cx.update(|cx| {
            cx.set_global(Config(crate::core::config::CoreConfig {
                theme_follow_system: true,
                theme_preset_light: "light-slot".into(),
                theme_preset_dark: "dark-slot".into(),
                ..Default::default()
            }));

            cx.set_global(SystemAppearance { dark: false });
            assert!(!system_dark(cx));
            assert_eq!(effective_preset_id(cx), "light-slot");

            cx.set_global(SystemAppearance { dark: true });
            assert!(system_dark(cx));
            assert_eq!(effective_preset_id(cx), "dark-slot");

            cx.global_mut::<Config>().theme_follow_system = false;
            assert_eq!(effective_preset_id(cx), Config::default().theme_preset);
        });
    }

    #[cfg(target_os = "windows")]
    #[gpui_kit::test]
    fn sidebars_preserve_desktop_transmission_in_the_windows_frame(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = crate::ui::app::test_window::harness_with_tabs(cx, 1);
        vcx.update(|window, cx| {
            // Match the production window: only LyrelApp supplies the background.
            window
                .root::<gpui_kit::component::Root>()
                .flatten()
                .unwrap()
                .update(cx, |root, _| {
                    root.style().background = Some(gpui_kit::transparent_black().into());
                });
            cx.global_mut::<Config>().window_opacity = Some(0.88);
            apply_theme(Some(window), cx);
        });
        app.update(&mut vcx, |app, cx| {
            app.right_panel_visible = true;
            cx.notify();
        });
        vcx.update(|window, cx| {
            window.draw(cx).clear(cx);
            let viewport = window.viewport_size();
            let scale = window.scale_factor();
            let quads = window.painted_quads();
            for x in [50., viewport.width.as_f32() - 50.] {
                let sample = point(
                    gpui_kit::ScaledPixels(x * scale),
                    gpui_kit::ScaledPixels((viewport.height.as_f32() - 100.) * scale),
                );
                // gpui-pre-windows 0.3.9 adds destination alpha (ONE), rather
                // than attenuating it. Observe the actual frame, not just a token.
                let alpha: f32 = quads
                    .iter()
                    .filter(|quad| {
                        quad.bounds.contains(&sample) && quad.content_mask.bounds.contains(&sample)
                    })
                    .filter_map(|quad| quad.background.as_solid())
                    .map(|fill| fill.a)
                    .sum();
                let desktop = 1. - alpha.min(1.);
                assert!(
                    (desktop - 0.102).abs() < 0.001,
                    "sidebar at x={x} transmits {desktop}, expected 85% of the terminal's 12%"
                );
            }
        });
    }

    #[gpui_kit::test]
    fn workspace_surfaces_follow_window_opacity_not_the_material_picker(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        cx.update(|cx| {
            let theme = presets::builtins().remove(0);
            cx.set_global(theme.surfaces());
            cx.set_global(Config::default());
            // Auto and Off can both carry an explicitly translucent window;
            // macOS also uses Auto with its local blur switch. None may leave
            // either large sidebar opaque over the shared window background.
            for backdrop in [
                WindowBackdrop::Auto,
                WindowBackdrop::Off,
                WindowBackdrop::Blur,
                WindowBackdrop::Mica,
                WindowBackdrop::MicaAlt,
                WindowBackdrop::Acrylic,
            ] {
                for blur in [false, true] {
                    let config = cx.global_mut::<Config>();
                    config.window_backdrop = backdrop;
                    config.window_blur = Some(blur);
                    for opacity in [0.2, 0.82, 0.999] {
                        cx.set_global(presets::ActiveBackground {
                            fill: theme.background.clone(),
                            opacity: Some(opacity),
                            image: None,
                        });
                        for fill in [workspace_surface_color(cx), rail_surface_color(cx)] {
                            assert!(
                                (0.0..=0.2).contains(&fill.a),
                                "{backdrop:?}, blur={blur}, opacity={opacity}: sidebar hides the native background"
                            );
                        }
                    }
                    for opacity in [None, Some(1.0)] {
                        cx.set_global(presets::ActiveBackground {
                            fill: theme.background.clone(),
                            opacity,
                            image: None,
                        });
                        assert_eq!(workspace_surface_color(cx).a, 1.0);
                        assert_eq!(rail_surface_color(cx).a, 1.0);
                    }
                }
            }
            // These are workspace-only projections. Components over an opaque
            // settings/overlay surface must retain their solid theme token.
            assert_eq!(cx.theme().sidebar.a, 1.0);
        });
    }

    #[test]
    fn opacity_overrides_keep_their_precedence_when_material_changes() {
        let mut theme = presets::builtins().remove(0);
        theme.opacity = Some(0.61);
        let mut config = Config::default();
        for backdrop in [
            WindowBackdrop::Auto,
            WindowBackdrop::Off,
            WindowBackdrop::Mica,
        ] {
            config.window_backdrop = backdrop;
            for blur in [false, true] {
                config.window_opacity = None;
                assert_eq!(effective_window_opacity(&config, &theme, blur), 0.61);
                config.window_opacity = Some(0.37);
                assert_eq!(effective_window_opacity(&config, &theme, blur), 0.37);
                config.window_opacity = Some(1.0);
                assert_eq!(effective_window_opacity(&config, &theme, blur), 1.0);
            }
        }
    }

    #[test]
    fn an_opaque_windows_fill_keeps_cleartype() {
        use BackdropAppearance::*;
        // A fully opaque fill shows nothing behind the window, so plain
        // translucency or the WCA blur only cost ClearType (#1120).
        assert_eq!(windows_window_appearance(Transparent, 1.0), Opaque);
        assert_eq!(windows_window_appearance(Blurred, 1.0), Opaque);
        // Anything see-through keeps what it asked for.
        assert_eq!(windows_window_appearance(Transparent, 0.9), Transparent);
        assert_eq!(windows_window_appearance(Blurred, 0.82), Blurred);
        // DWM materials are never routed through Opaque.
        for material in [MicaBackdrop, MicaAltBackdrop, AcrylicBackdrop] {
            assert_eq!(windows_window_appearance(material, 1.0), material);
        }
        // The untouched default — Auto, no blur, no opacity override — is the
        // case the report hit: it must come out opaque.
        let resolved =
            windows_background_appearance(crate::core::config::WindowBackdrop::Auto, false, 22_621);
        let opacity = default_window_opacity(crate::core::config::WindowBackdrop::Auto, false);
        assert_eq!(windows_window_appearance(resolved, opacity), Opaque);
    }

    #[test]
    fn acrylic_is_carried_by_transparent_and_driven_by_the_app() {
        use BackdropAppearance::*;
        // Every variant but Acrylic has a same-named public gpui variant…
        assert_eq!(Opaque.as_gpui(), WindowBackgroundAppearance::Opaque);
        assert_eq!(
            Transparent.as_gpui(),
            WindowBackgroundAppearance::Transparent
        );
        assert_eq!(Blurred.as_gpui(), WindowBackgroundAppearance::Blurred);
        assert_eq!(
            MicaBackdrop.as_gpui(),
            WindowBackgroundAppearance::MicaBackdrop
        );
        assert_eq!(
            MicaAltBackdrop.as_gpui(),
            WindowBackgroundAppearance::MicaAltBackdrop
        );
        // …while Acrylic rides on the transparent carrier: a transparent
        // frame buffer and grayscale text, with DWMSBT_TRANSIENTWINDOW
        // selected by `ui::backdrop` instead of the framework.
        assert_eq!(
            AcrylicBackdrop.as_gpui(),
            WindowBackgroundAppearance::Transparent
        );
    }

    #[test]
    fn native_menu_key_equivalents_fix_only_enter_and_tab() {
        // gpui's `key_to_native` passes unmapped key names through verbatim,
        // so an enter/tab binding arrives as the literal word — and only that
        // exact word may be rewritten. Anything already mapped or character-
        // sized stays untouched, or real ⌘E/⌘T chords would be clobbered.
        assert_eq!(native_menu_key_equivalent_fix("enter"), Some("\r"));
        assert_eq!(native_menu_key_equivalent_fix("tab"), Some("\t"));
        for untouched in ["e", "t", "Enter", "Tab", "enter+", "", "return"] {
            assert_eq!(native_menu_key_equivalent_fix(untouched), None);
        }
    }

    #[test]
    fn the_material_plan_cleans_both_attribute_groups() {
        use BackdropAppearance::*;
        // Below 22H2 there is no DWMWA_SYSTEMBACKDROP_TYPE: nothing settable,
        // nothing to clear, whatever the target.
        for target in [
            Opaque,
            Transparent,
            Blurred,
            MicaBackdrop,
            MicaAltBackdrop,
            AcrylicBackdrop,
        ] {
            assert_eq!(material_plan(target, 22_620), MaterialPlan::NOOP);
            assert_eq!(material_plan(target, 0), MaterialPlan::NOOP);
        }
        // The DWM materials: the WCA accent must be disabled or DWM ignores
        // the backdrop type, the type itself is selected, and the frame is
        // nudged so the switch shows without a resize.
        assert_eq!(
            material_plan(MicaBackdrop, 22_621),
            MaterialPlan {
                wca_disable: true,
                dwmsbt: Some(DWMSBT_MAINWINDOW),
                refresh_frame: true,
            }
        );
        assert_eq!(
            material_plan(AcrylicBackdrop, 26_000),
            MaterialPlan {
                wca_disable: true,
                dwmsbt: Some(DWMSBT_TRANSIENTWINDOW),
                refresh_frame: true,
            }
        );
        assert_eq!(
            material_plan(MicaAltBackdrop, 22_621),
            MaterialPlan {
                wca_disable: true,
                dwmsbt: Some(DWMSBT_TABBEDWINDOW),
                refresh_frame: true,
            }
        );
        // The WCA family: the backend sets the accent itself, but leaves a
        // previously selected DWMSBT behind — switching Mica → Blurred must
        // clear it or both materials fight.
        for target in [Opaque, Transparent, Blurred] {
            assert_eq!(
                material_plan(target, 22_621),
                MaterialPlan {
                    wca_disable: false,
                    dwmsbt: Some(DWMSBT_NONE),
                    refresh_frame: true,
                },
                "{target:?} must clear a stale DWM system backdrop"
            );
        }
    }

    #[test]
    fn windows_background_appearance_falls_back_by_build() {
        use crate::core::config::WindowBackdrop;
        let a = |b: WindowBackdrop, blur: bool, build: u32| {
            windows_background_appearance(b, blur, build)
        };

        // Auto keeps the legacy blur/translucent choice.
        assert_eq!(
            a(WindowBackdrop::Auto, false, 22_621),
            BackdropAppearance::Transparent
        );
        assert_eq!(
            a(WindowBackdrop::Auto, true, 17_763),
            BackdropAppearance::Blurred
        );

        // Blur explicitly requests the classic blurred appearance from 1809
        // on; older builds get plain translucency.
        assert_eq!(
            a(WindowBackdrop::Blur, false, 17_763),
            BackdropAppearance::Blurred
        );
        assert_eq!(
            a(WindowBackdrop::Blur, true, 17_762),
            BackdropAppearance::Transparent
        );

        // Mica needs Windows 11 22H2; 1809..22H2 falls back to classic
        // acrylic, and below 1809 there is no blur API to fall back to —
        // asking for one there would leave an unbacked see-through window.
        assert_eq!(
            a(WindowBackdrop::Mica, false, 22_621),
            BackdropAppearance::MicaBackdrop
        );
        assert_eq!(
            a(WindowBackdrop::Mica, false, 22_620),
            BackdropAppearance::Blurred
        );
        assert_eq!(
            a(WindowBackdrop::Mica, false, 17_762),
            BackdropAppearance::Transparent
        );
        assert_eq!(
            a(WindowBackdrop::MicaAlt, false, 26_000),
            BackdropAppearance::MicaAltBackdrop
        );
        assert_eq!(
            a(WindowBackdrop::MicaAlt, false, 17_763),
            BackdropAppearance::Blurred
        );
        assert_eq!(
            a(WindowBackdrop::MicaAlt, false, 17_762),
            BackdropAppearance::Transparent
        );
        // A failed RtlGetVersion reports build 0; every material must degrade
        // to plain translucency there rather than request an unsupported one.
        for backdrop in [
            WindowBackdrop::Blur,
            WindowBackdrop::Mica,
            WindowBackdrop::MicaAlt,
            WindowBackdrop::Acrylic,
        ] {
            assert_eq!(
                a(backdrop, false, 0),
                BackdropAppearance::Transparent,
                "{backdrop:?} on an unknown build must not request a material"
            );
        }

        // Acrylic resolves to the app-driven DWMSBT_TRANSIENTWINDOW material
        // on 22H2+, classic WCA acrylic from 1809 on, plain translucency
        // below that.
        assert_eq!(
            a(WindowBackdrop::Acrylic, false, 22_621),
            BackdropAppearance::AcrylicBackdrop
        );
        assert_eq!(
            a(WindowBackdrop::Acrylic, false, 22_620),
            BackdropAppearance::Blurred
        );
        assert_eq!(
            a(WindowBackdrop::Acrylic, false, 17_763),
            BackdropAppearance::Blurred
        );
        assert_eq!(
            a(WindowBackdrop::Acrylic, false, 17_762),
            BackdropAppearance::Transparent
        );

        // Off never requests a material.
        assert_eq!(
            a(WindowBackdrop::Off, false, 26_000),
            BackdropAppearance::Transparent
        );
        assert_eq!(
            a(WindowBackdrop::Off, true, 26_000),
            BackdropAppearance::Transparent
        );
    }

    #[test]
    fn auto_never_counts_as_a_material() {
        use crate::core::config::WindowBackdrop;
        // Every config written before this setting existed carries `Auto`,
        // and many of them carry `window_blur: true` from the old switch.
        // Those windows were opaque, with opaque sidebars — an update must
        // not silently turn them see-through, so `Auto` gets the plain 1.0
        // default no matter which way the legacy toggle points.
        for blur in [false, true] {
            assert!(!material_active(WindowBackdrop::Auto, blur));
            assert_eq!(default_window_opacity(WindowBackdrop::Auto, blur), 1.0);
        }
        // `Off` is an explicit "no material" and must behave the same.
        assert!(!material_active(WindowBackdrop::Off, true));
        assert_eq!(default_window_opacity(WindowBackdrop::Off, true), 1.0);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn an_explicitly_picked_material_lowers_the_default_opacity() {
        use crate::core::config::WindowBackdrop;
        // The counterpart to the check above: opting in through the dropdown
        // is what buys the translucent default, on a build that supports it.
        if windows_build_number() < 22_621 {
            return;
        }
        for backdrop in [
            WindowBackdrop::Blur,
            WindowBackdrop::Mica,
            WindowBackdrop::MicaAlt,
            WindowBackdrop::Acrylic,
        ] {
            assert!(material_active(backdrop, false), "{backdrop:?}");
            assert_eq!(
                default_window_opacity(backdrop, false),
                SYSTEM_MATERIAL_OPACITY,
                "{backdrop:?}"
            );
        }
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn non_windows_backdrop_defers_to_the_local_blur_toggle() {
        use crate::core::config::WindowBackdrop;
        // On non-Windows, `window_backdrop` is a Windows-only setting with
        // no local UI to clear it; every variant must defer to the local
        // blur toggle so the switch's checked state always matches what is
        // actually rendered.
        for backdrop in [
            WindowBackdrop::Auto,
            WindowBackdrop::Blur,
            WindowBackdrop::Mica,
            WindowBackdrop::MicaAlt,
            WindowBackdrop::Acrylic,
            WindowBackdrop::Off,
        ] {
            assert_eq!(
                resolved_background_appearance(backdrop, true),
                BackdropAppearance::Blurred,
                "{backdrop:?} with blur on should resolve to Blurred"
            );
            assert_eq!(
                resolved_background_appearance(backdrop, false),
                BackdropAppearance::Transparent,
                "{backdrop:?} with blur off should resolve to Transparent"
            );
        }
    }

    #[test]
    fn supported_backdrops_follow_the_build() {
        use crate::core::config::WindowBackdrop;
        assert_eq!(
            supported_backdrops_for(22_621),
            &[
                WindowBackdrop::Auto,
                WindowBackdrop::Blur,
                WindowBackdrop::Mica,
                WindowBackdrop::MicaAlt,
                WindowBackdrop::Acrylic,
                WindowBackdrop::Off,
            ]
        );
        // Pre-22H2: Mica, Mica Alt and Acrylic all collapse onto the very
        // same classic WCA blur that `Blur` already offers, so the dropdown
        // hides them rather than listing choices that render identically.
        assert_eq!(
            supported_backdrops_for(22_620),
            &[
                WindowBackdrop::Auto,
                WindowBackdrop::Blur,
                WindowBackdrop::Off,
            ]
        );
        assert_eq!(
            supported_backdrops_for(17_763),
            &[
                WindowBackdrop::Auto,
                WindowBackdrop::Blur,
                WindowBackdrop::Off,
            ]
        );
        // Every offered preset must resolve to a distinct appearance, or the
        // dropdown is promising a difference the window cannot deliver.
        // `Auto` is exempt: it has no fixed appearance of its own, it mirrors
        // whichever way the blur toggle happens to be set.
        for build in [22_621, 22_620, 17_763, 17_762] {
            let offered: Vec<_> = supported_backdrops_for(build)
                .iter()
                .copied()
                .filter(|b| *b != WindowBackdrop::Auto)
                .collect();
            for (i, a) in offered.iter().enumerate() {
                for b in &offered[i + 1..] {
                    assert_ne!(
                        windows_background_appearance(*a, false, build),
                        windows_background_appearance(*b, false, build),
                        "build {build}: {a:?} and {b:?} render identically"
                    );
                }
            }
        }
        // Below 1809 there is no blur API at all; only plain translucency.
        assert_eq!(
            supported_backdrops_for(17_762),
            &[WindowBackdrop::Auto, WindowBackdrop::Off]
        );
    }
}

/// The text view's style for rendered Markdown, wherever lyrel shows it.
///
/// Inline `code` gets a faint neutral fill. The text view's own default is the
/// theme's `accent`, which lyrel also paints selection with, so every code span
/// read as selected text.
pub(crate) fn markdown_style(cx: &gpui_kit::App) -> gpui_kit::component::text::TextViewStyle {
    gpui_kit::component::text::TextViewStyle::default().inline_code(gpui_kit::HighlightStyle {
        background_color: Some(cx.theme().foreground.opacity(0.08)),
        ..Default::default()
    })
}
