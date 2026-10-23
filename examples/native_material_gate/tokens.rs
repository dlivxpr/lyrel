//! 探针局部语义 token；不是生产主题，也不读取用户主题文件。
//! 默认浅/深的基础色对照批准 HTML 的 :root / [data-theme="light"]。

use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::{App, Hsla, px, rgb};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ProbeTheme {
    Light,
    Dark,
    Custom,
}

impl ProbeTheme {
    pub(crate) const ALL: [Self; 3] = [Self::Light, Self::Dark, Self::Custom];

    pub(crate) fn id(self) -> &'static str {
        match self {
            Self::Light => "light",
            Self::Dark => "dark",
            Self::Custom => "custom",
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Light => "浅色 Light",
            Self::Dark => "深色 Dark",
            Self::Custom => "自定义 Teal",
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct Palette {
    pub(crate) background: Hsla,
    pub(crate) rail: Hsla,
    pub(crate) sidebar: Hsla,
    pub(crate) foreground: Hsla,
    pub(crate) muted_foreground: Hsla,
    pub(crate) divider: Hsla,
    pub(crate) selection: Hsla,
    pub(crate) accent: Hsla,
}

impl Palette {
    fn for_theme(theme: ProbeTheme) -> Self {
        let (background, rail, sidebar, foreground, muted, divider, selection, accent) = match theme {
            ProbeTheme::Light => (
                0xfcfcfb, 0xf5f5f3, 0xfcfcfb, 0x1c1c1e,
                0x66666d, 0xdedee2, 0xe7e7e9, 0x1f6bf0,
            ),
            ProbeTheme::Dark => (
                0x18181a, 0x1e1e20, 0x18181a, 0xececed,
                0xa2a2aa, 0x3b3b42, 0x303036, 0x78a8f5,
            ),
            // 代表自定义低亮度蓝绿底，检验不能假设所有主题为中性灰。
            ProbeTheme::Custom => (
                0x102c31, 0x17383d, 0x133137, 0xe2f2ed,
                0xafc7c3, 0x4b6f70, 0x30565a, 0x9fd9cf,
            ),
        };
        Self {
            background: rgb(background).into(),
            rail: rgb(rail).into(),
            sidebar: rgb(sidebar).into(),
            foreground: rgb(foreground).into(),
            muted_foreground: rgb(muted).into(),
            divider: rgb(divider).into(),
            selection: rgb(selection).into(),
            accent: rgb(accent).into(),
        }
    }

    pub(crate) fn apply(theme: ProbeTheme, opacity: f32, font_size: f32, cx: &mut App) -> Self {
        Theme::change(
            if theme == ProbeTheme::Light { ThemeMode::Light } else { ThemeMode::Dark },
            None,
            cx,
        );
        let palette = Self::for_theme(theme);
        #[cfg(target_os = "macos")]
        sync_native_appearance(theme);
        Theme::update(cx, |kit| {
            // Kit Root 自己绘制 tokens.background；内容根不能再叠一层同色 alpha。
            kit.colors.background = palette.background.alpha(opacity);
            kit.colors.sidebar = palette.sidebar;
            kit.colors.sidebar_foreground = palette.foreground;
            kit.colors.foreground = palette.foreground;
            kit.colors.muted_foreground = palette.muted_foreground;
            kit.colors.border = palette.divider;
            kit.colors.primary = palette.accent;
            kit.colors.primary_foreground = palette.background;
            kit.colors.ring = palette.accent;
            kit.font_size = px(font_size);
        });
        palette
    }

    pub(crate) fn rail_fill(self, translucent: bool) -> Hsla {
        self.rail.alpha(if translucent { 0.20 } else { 1.0 })
    }

    pub(crate) fn sidebar_fill(self, translucent: bool) -> Hsla {
        self.sidebar.alpha(if translucent { 0.17 } else { 1.0 })
    }
}

// 与生产 theme.rs:1374-1393 一样，仅改变本进程 NSApplication appearance，
// 不写系统设置。使用现有 registry objc2 公开 API，不移植后端私有 blur 实现。
#[cfg(target_os = "macos")]
fn sync_native_appearance(theme: ProbeTheme) {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{
        NSAppearance, NSAppearanceNameAqua, NSAppearanceNameDarkAqua, NSApplication,
    };
    let Some(main_thread) = MainThreadMarker::new() else {
        return;
    };
    // SAFETY: AppKit appearance names are immutable framework constants.
    let name = unsafe {
        if theme == ProbeTheme::Light {
            NSAppearanceNameAqua
        } else {
            NSAppearanceNameDarkAqua
        }
    };
    let appearance = NSAppearance::appearanceNamed(name);
    NSApplication::sharedApplication(main_thread).setAppearance(appearance.as_deref());
}
