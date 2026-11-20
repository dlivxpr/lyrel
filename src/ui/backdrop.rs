//! Windows backdrop-material wiring owned by the app.
//!
//! The published gpui 0.3.9 backend no longer knows `AcrylicBackdrop`, and its
//! `set_background_appearance` never cleans up the *other* attribute group:
//! the Mica arms set `DWMWA_SYSTEMBACKDROP_TYPE` but leave a WCA accent in
//! place (a live accent keeps DWM from honoring the backdrop type), and the
//! WCA arms (`Transparent`/`Blurred`/`Opaque`) leave a previously set DWM
//! system backdrop behind. The fork the migration replaces cleaned the WCA
//! accent inside its Mica/Acrylic arms; this module restores that discipline
//! for every target, for both attribute groups, from application code.
//!
//! Only public app-level capabilities are used: the window's `HasWindowHandle`
//! HWND, the documented `DwmSetWindowAttribute`, and the same
//! `SetWindowCompositionAttribute` lookup the backend itself performs — here
//! only ever to *disable* an accent. Nothing is patched into the framework
//! and no downloaded source is modified.

use gpui_kit::Window;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute;
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleA, GetProcAddress};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    SET_WINDOW_POS_FLAGS, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER,
    SetWindowPos,
};

use crate::ui::theme::{BackdropAppearance, MaterialPlan, WcaPolicy};

/// Brings the window's two native attribute groups in line with `target`.
///
/// Runs after gpui's own `set_background_appearance`, so the WCA accent for
/// the non-DWM targets and the DWMSBT value for Mica are already set; what is
/// left is the cross-group cleanup plus the Acrylic material the backend does
/// not know, and the frame nudge a runtime switch needs to show up.
pub(crate) fn sync_native_material(window: &Window, target: BackdropAppearance, build: u32) {
    // `Window` has an inherent `window_handle()` of its own (the gpui handle),
    // so the raw-window-handle trait method has to be named explicitly.
    let handle = match HasWindowHandle::window_handle(window) {
        Ok(handle) => handle,
        Err(e) => {
            log::warn!(
                "window backdrop: cannot read the window's raw handle, \
                 {target:?} material state is stale: {e}"
            );
            return;
        }
    };
    let RawWindowHandle::Win32(win32) = handle.as_raw() else {
        log::warn!(
            "window backdrop: expected a Win32 window handle for {target:?}, got {:?}",
            handle.as_raw()
        );
        return;
    };
    let hwnd = win32.hwnd.get() as HWND;
    sync_native_material_hwnd(hwnd, target, build);
}

fn sync_native_material_hwnd(hwnd: HWND, target: BackdropAppearance, build: u32) {
    let plan = crate::ui::theme::material_plan(target, build);
    if plan == MaterialPlan::NOOP {
        return;
    }
    if plan.wca == WcaPolicy::Disable {
        set_wca_accent(hwnd, ACCENT_DISABLED, 0);
    }
    if let Some(backdrop) = plan.dwmsbt {
        set_system_backdrop(hwnd, backdrop);
    }
    if plan.wca == WcaPolicy::EnableAcrylic {
        // Non-zero alpha in gradient_color is required for WCA acrylic to render.
        set_wca_accent(hwnd, ACCENT_ENABLE_ACRYLICBLURBEHIND, 0x0100_0000);
    }
    if plan.refresh_frame {
        refresh_frame(hwnd);
    }
}

/// `DWMWA_SYSTEMBACKDROP_TYPE` (dwmapi.h), valid from build 22621 on — the
/// plan never asks for it below that, so no runtime guard is needed here.
const DWMWA_SYSTEMBACKDROP_TYPE: u32 = 38;

fn set_system_backdrop(hwnd: HWND, backdrop: u32) {
    // SAFETY: `hwnd` names a live window of this process, and `backdrop`
    // outlives the call as the attribute value's buffer.
    let result = unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_SYSTEMBACKDROP_TYPE,
            &backdrop as *const u32 as *const core::ffi::c_void,
            std::mem::size_of::<u32>() as u32,
        )
    };
    if result < 0 {
        log::warn!("DwmSetWindowAttribute(SYSTEMBACKDROP_TYPE, {backdrop}) failed: {result:#x}");
    }
}

/// `WCA_ACCENT_POLICY`, `ACCENT_DISABLED`, and `ACCENT_ENABLE_ACRYLICBLURBEHIND`.
const WCA_ACCENT_POLICY: u32 = 19;
const ACCENT_DISABLED: u32 = 0;
const ACCENT_ENABLE_ACRYLICBLURBEHIND: u32 = 4;

// Same layout the published backend uses for its own accent calls; the
// function is undocumented user32, resolved the same way it resolves it.
#[repr(C)]
struct AccentPolicy {
    accent_state: u32,
    accent_flags: u32,
    gradient_color: u32,
    animation_id: u32,
}

#[repr(C)]
struct WindowCompositionAttribData {
    attrib: u32,
    pv_data: *mut core::ffi::c_void,
    cb_data: usize,
}

fn set_wca_accent(hwnd: HWND, accent_state: u32, gradient_color: u32) -> bool {
    type SetWindowCompositionAttributeFn =
        unsafe extern "system" fn(HWND, *mut WindowCompositionAttribData) -> i32;
    // SAFETY: user32 is loaded in every GUI process; the transmute targets
    // the signature the backend itself uses for this export, and `accent`
    // outlives the call.
    unsafe {
        let user32 = GetModuleHandleA(c"user32.dll".as_ptr().cast());
        if user32.is_null() {
            log::warn!(
                "window backdrop: cannot update the WCA accent, user32.dll handle unavailable: {}",
                std::io::Error::last_os_error()
            );
            return false;
        }
        let Some(set_wca) =
            GetProcAddress(user32, c"SetWindowCompositionAttribute".as_ptr().cast())
        else {
            log::warn!(
                "window backdrop: user32!SetWindowCompositionAttribute is unavailable: {}",
                std::io::Error::last_os_error()
            );
            return false;
        };
        let set_wca: SetWindowCompositionAttributeFn = std::mem::transmute(set_wca);
        let mut accent = AccentPolicy {
            accent_state,
            accent_flags: 0,
            gradient_color,
            animation_id: 0,
        };
        let mut data = WindowCompositionAttribData {
            attrib: WCA_ACCENT_POLICY,
            pv_data: &mut accent as *mut AccentPolicy as *mut core::ffi::c_void,
            cb_data: std::mem::size_of::<AccentPolicy>(),
        };
        if set_wca(hwnd, &mut data) == 0 {
            log::warn!(
                "window backdrop: SetWindowCompositionAttribute({accent_state}) failed: {}",
                std::io::Error::last_os_error()
            );
            return false;
        }
        true
    }
}

/// What the fork's backend did after changing a DWM backdrop and what the
/// published one omits: without the frame nudge a runtime switch can keep
/// showing the previous material until the next resize.
fn refresh_frame(hwnd: HWND) {
    const FLAGS: SET_WINDOW_POS_FLAGS =
        SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED;
    // SAFETY: `hwnd` names a live window of this process; the flags leave
    // position, size, z-order and activation untouched.
    let ok = unsafe { SetWindowPos(hwnd, std::ptr::null_mut(), 0, 0, 0, 0, FLAGS) };
    if ok == 0 {
        log::warn!(
            "window backdrop: the frame nudge (SetWindowPos SWP_FRAMECHANGED) failed, \
             the material switch may not show until the next resize: {}",
            std::io::Error::last_os_error()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::Graphics::Dwm::DwmGetWindowAttribute;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, WS_EX_NOREDIRECTIONBITMAP, WS_OVERLAPPEDWINDOW,
    };

    fn query_dwm_backdrop(hwnd: HWND) -> u32 {
        let mut value: u32 = 0;
        let hr = unsafe {
            DwmGetWindowAttribute(
                hwnd,
                DWMWA_SYSTEMBACKDROP_TYPE,
                &mut value as *mut u32 as *mut core::ffi::c_void,
                std::mem::size_of::<u32>() as u32,
            )
        };
        assert!(hr >= 0, "DwmGetWindowAttribute failed: {hr:#x}");
        value
    }

    #[test]
    fn windows_blur_retains_native_backdrop_after_gpui_fast_handoff() {
        let build = crate::ui::theme::windows_build_number();
        if build < 22_621 {
            return;
        }
        // Static class name "STATIC" is pre-registered in every Win32 process.
        let class: Vec<u16> = "STATIC\0".encode_utf16().collect();
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_NOREDIRECTIONBITMAP,
                class.as_ptr(),
                std::ptr::null(),
                WS_OVERLAPPEDWINDOW,
                0,
                0,
                200,
                200,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null(),
            )
        };
        assert!(!hwnd.is_null(), "CreateWindowExW failed");

        // Simulate gpui-fast-windows 0.1.16 `fast::backdrop::blur(hwnd)` on build >= 22621:
        // disables WCA and requests DWMSBT_TRANSIENTWINDOW (3).
        assert!(set_wca_accent(hwnd, ACCENT_DISABLED, 0));
        set_system_backdrop(hwnd, crate::ui::theme::DWMSBT_TRANSIENTWINDOW);

        sync_native_material_hwnd(hwnd, BackdropAppearance::Blurred, build);

        let dwmsbt = query_dwm_backdrop(hwnd);
        // Verify WCA acrylic call succeeds against the real HWND after clearing DWMSBT_NONE.
        let wca_ok = set_wca_accent(hwnd, ACCENT_ENABLE_ACRYLICBLURBEHIND, 0x0100_0000);
        let _ = unsafe { DestroyWindow(hwnd) };

        assert_eq!(
            dwmsbt,
            crate::ui::theme::DWMSBT_NONE,
            "Blurred must clear gpui-fast's DWMSBT_TRANSIENTWINDOW so WCA blur controls the surface"
        );
        assert!(
            wca_ok,
            "SetWindowCompositionAttribute(ACCENT_ENABLE_ACRYLICBLURBEHIND) must succeed"
        );
        assert_eq!(
            crate::ui::theme::material_plan(BackdropAppearance::Blurred, build).wca,
            WcaPolicy::EnableAcrylic
        );
    }
}
