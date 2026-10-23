//! #23 的整窗材质探针使用的私有 policy seam。
//!
//! `src/ui/theme.rs` 依赖 Config、terminal 和整个应用，不能在无会话的样例中整体加载。
//! 此处仅保留其 BackdropAppearance、MaterialPlan、Windows build/fallback 表。
//! 真正的 HWND 写入直接复用 `src/ui/backdrop.rs`，不复制框架私有实现。
//! 对照来源：theme.rs:370-611、744-753、834-837；生产文件未改动。

use gpui_kit::{Window, WindowBackgroundAppearance};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BackdropAppearance {
    Opaque,
    Transparent,
    Blurred,
    #[cfg(target_os = "windows")]
    MicaBackdrop,
    #[cfg(target_os = "windows")]
    MicaAltBackdrop,
    #[cfg(target_os = "windows")]
    AcrylicBackdrop,
}

impl BackdropAppearance {
    pub(crate) fn as_gpui(self) -> WindowBackgroundAppearance {
        match self {
            Self::Opaque => WindowBackgroundAppearance::Opaque,
            Self::Transparent => WindowBackgroundAppearance::Transparent,
            Self::Blurred => WindowBackgroundAppearance::Blurred,
            #[cfg(target_os = "windows")]
            Self::MicaBackdrop => WindowBackgroundAppearance::MicaBackdrop,
            #[cfg(target_os = "windows")]
            Self::MicaAltBackdrop => WindowBackgroundAppearance::MicaAltBackdrop,
            // 正式 gpui-pre 0.3.9 没有 AcrylicBackdrop；应用自己的 DWM seam 选择 3。
            #[cfg(target_os = "windows")]
            Self::AcrylicBackdrop => WindowBackgroundAppearance::Transparent,
        }
    }
}

#[cfg(target_os = "windows")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct MaterialPlan {
    pub(crate) wca_disable: bool,
    pub(crate) dwmsbt: Option<u32>,
    pub(crate) refresh_frame: bool,
}

#[cfg(target_os = "windows")]
impl MaterialPlan {
    pub(crate) const NOOP: Self = Self {
        wca_disable: false,
        dwmsbt: None,
        refresh_frame: false,
    };
}

#[cfg(target_os = "windows")]
pub(crate) fn material_plan(target: BackdropAppearance, build: u32) -> MaterialPlan {
    use BackdropAppearance::*;
    if build < 22_621 {
        return MaterialPlan::NOOP;
    }
    MaterialPlan {
        wca_disable: matches!(target, MicaBackdrop | MicaAltBackdrop | AcrylicBackdrop),
        dwmsbt: Some(match target {
            MicaBackdrop => 2,
            AcrylicBackdrop => 3,
            MicaAltBackdrop => 4,
            Opaque | Transparent | Blurred => 1,
        }),
        refresh_frame: true,
    }
}

#[cfg(target_os = "windows")]
pub(crate) fn windows_build_number() -> u32 {
    use std::sync::LazyLock;
    use windows_sys::Wdk::System::SystemServices::RtlGetVersion;
    use windows_sys::Win32::System::SystemInformation::OSVERSIONINFOW;
    static BUILD: LazyLock<u32> = LazyLock::new(|| {
        // SAFETY: OSVERSIONINFOW is a plain Windows output buffer; it lives through the call.
        let mut info: OSVERSIONINFOW = unsafe { std::mem::zeroed() };
        info.dwOSVersionInfoSize = std::mem::size_of_val(&info) as u32;
        if unsafe { RtlGetVersion(&mut info) } == 0 {
            info.dwBuildNumber
        } else {
            0
        }
    });
    *BUILD
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Material {
    Opaque,
    Transparent,
    Blurred,
    #[cfg(target_os = "windows")]
    Auto,
    #[cfg(target_os = "windows")]
    Mica,
    #[cfg(target_os = "windows")]
    MicaAlt,
    #[cfg(target_os = "windows")]
    Acrylic,
}

impl Material {
    pub(crate) const ALL: &'static [Self] = &[
        Self::Opaque,
        Self::Transparent,
        Self::Blurred,
        #[cfg(target_os = "windows")]
        Self::Auto,
        #[cfg(target_os = "windows")]
        Self::Mica,
        #[cfg(target_os = "windows")]
        Self::MicaAlt,
        #[cfg(target_os = "windows")]
        Self::Acrylic,
    ];

    pub(crate) fn id(self) -> &'static str {
        match self {
            Self::Opaque => "opaque",
            Self::Transparent => "transparent",
            Self::Blurred => "blurred",
            #[cfg(target_os = "windows")]
            Self::Auto => "auto",
            #[cfg(target_os = "windows")]
            Self::Mica => "mica",
            #[cfg(target_os = "windows")]
            Self::MicaAlt => "mica-alt",
            #[cfg(target_os = "windows")]
            Self::Acrylic => "acrylic",
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Opaque => "不透明 Opaque",
            Self::Transparent => "透明 Off",
            Self::Blurred => "模糊 Blur",
            #[cfg(target_os = "windows")]
            Self::Auto => "自动 Auto",
            #[cfg(target_os = "windows")]
            Self::Mica => "Mica",
            #[cfg(target_os = "windows")]
            Self::MicaAlt => "Mica Alt",
            #[cfg(target_os = "windows")]
            Self::Acrylic => "Acrylic",
        }
    }

    pub(crate) fn resolve(self, legacy_blur: bool, opacity: f32) -> BackdropAppearance {
        #[cfg(target_os = "windows")]
        let resolved = {
            let build = windows_build_number();
            match self {
                Self::Opaque => BackdropAppearance::Opaque,
                Self::Transparent => BackdropAppearance::Transparent,
                Self::Blurred if build >= 17_763 => BackdropAppearance::Blurred,
                Self::Blurred => BackdropAppearance::Transparent,
                Self::Auto if legacy_blur => BackdropAppearance::Blurred,
                Self::Auto => BackdropAppearance::Transparent,
                Self::Mica if build >= 22_621 => BackdropAppearance::MicaBackdrop,
                Self::MicaAlt if build >= 22_621 => BackdropAppearance::MicaAltBackdrop,
                Self::Acrylic if build >= 22_621 => BackdropAppearance::AcrylicBackdrop,
                Self::Mica | Self::MicaAlt | Self::Acrylic if build >= 17_763 => {
                    BackdropAppearance::Blurred
                }
                Self::Mica | Self::MicaAlt | Self::Acrylic => BackdropAppearance::Transparent,
            }
        };
        #[cfg(not(target_os = "windows"))]
        let resolved = {
            let _ = legacy_blur;
            match self {
                Self::Opaque => BackdropAppearance::Opaque,
                Self::Transparent => BackdropAppearance::Transparent,
                Self::Blurred => BackdropAppearance::Blurred,
            }
        };
        // 与生产 Windows ClearType policy 相同；不把 alpha=1 的无材质窗口误报为透明。
        #[cfg(target_os = "windows")]
        if opacity >= 1.0
            && matches!(
                resolved,
                BackdropAppearance::Transparent | BackdropAppearance::Blurred
            )
        {
            return BackdropAppearance::Opaque;
        }
        #[cfg(not(target_os = "windows"))]
        let _ = opacity;
        resolved
    }
}

pub(crate) fn apply(window: &Window, appearance: BackdropAppearance) {
    window.set_background_appearance(appearance.as_gpui());
    #[cfg(target_os = "windows")]
    crate::native_backdrop::sync_native_material(window, appearance, windows_build_number());
}
