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

use crate::ui::theme::{BackdropAppearance, MaterialPlan};

/// Brings the window's two native attribute groups in line with `target`.
///
/// Runs after gpui's own `set_background_appearance`, so the WCA accent for
/// the non-DWM targets and the DWMSBT value for Mica are already set; what is
/// left is the cross-group cleanup plus the Acrylic material the backend does
/// not know, and the frame nudge a runtime switch needs to show up.
pub(crate) fn sync_native_material(window: &Window, target: BackdropAppearance, build: u32) {
    let plan = crate::ui::theme::material_plan(target, build);
    if plan == MaterialPlan::NOOP {
        return;
    }
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
    if plan.wca_disable {
        disable_wca_accent(hwnd);
    }
    if let Some(backdrop) = plan.dwmsbt {
        set_system_backdrop(hwnd, backdrop);
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

/// `WCA_ACCENT_POLICY` and `ACCENT_DISABLED`: the accent is only ever cleared
/// here, never set — the accents lyrel asks for are applied by the backend's
/// own `set_background_appearance`, and clearing is what lets a DWM system
/// backdrop take over.
const WCA_ACCENT_POLICY: u32 = 19;
const ACCENT_DISABLED: u32 = 0;

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

fn disable_wca_accent(hwnd: HWND) {
    type SetWindowCompositionAttributeFn =
        unsafe extern "system" fn(HWND, *mut WindowCompositionAttribData) -> i32;
    // SAFETY: user32 is loaded in every GUI process; the transmute targets
    // the signature the backend itself uses for this export, and `accent`
    // outlives the call.
    unsafe {
        let user32 = GetModuleHandleA(c"user32.dll".as_ptr().cast());
        if user32.is_null() {
            log::warn!(
                "window backdrop: cannot clear the WCA accent, user32.dll handle unavailable: {}",
                std::io::Error::last_os_error()
            );
            return;
        }
        let Some(set_wca) =
            GetProcAddress(user32, c"SetWindowCompositionAttribute".as_ptr().cast())
        else {
            log::warn!(
                "window backdrop: user32!SetWindowCompositionAttribute is unavailable, \
                 a live WCA accent keeps DWM from honoring the backdrop type: {}",
                std::io::Error::last_os_error()
            );
            return;
        };
        let set_wca: SetWindowCompositionAttributeFn = std::mem::transmute(set_wca);
        let mut accent = AccentPolicy {
            accent_state: ACCENT_DISABLED,
            accent_flags: 0,
            gradient_color: 0,
            animation_id: 0,
        };
        let mut data = WindowCompositionAttribData {
            attrib: WCA_ACCENT_POLICY,
            pv_data: &mut accent as *mut AccentPolicy as *mut core::ffi::c_void,
            cb_data: std::mem::size_of::<AccentPolicy>(),
        };
        if set_wca(hwnd, &mut data) == 0 {
            log::warn!(
                "window backdrop: SetWindowCompositionAttribute(ACCENT_DISABLED) failed, \
                 a live WCA accent keeps DWM from honoring the backdrop type: {}",
                std::io::Error::last_os_error()
            );
        }
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
