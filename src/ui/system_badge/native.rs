//! Native delivery for the app icon's system badge (#397).
//!
//! macOS sets the Dock tile's numeric label. Windows draws a small numeric
//! overlay for every top-level taskbar button with `ITaskbarList3`, which is
//! the documented per-button surface; applying the same aggregate to every
//! window keeps ungrouped buttons and a grouped button in agreement. The
//! caller re-applies the label on a one-second reconcile because there is no
//! window-procedure hook for `TaskbarButtonCreated` here: an Explorer restart
//! recreates the buttons without telling the app, and the next tick restores
//! the single aggregate.

#[cfg(test)]
thread_local! {
    static TEST_BADGES: std::cell::RefCell<Vec<Option<String>>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

#[cfg(all(target_os = "windows", not(test)))]
pub(super) type WindowHandles = Vec<windows::Win32::Foundation::HWND>;
#[cfg(any(not(target_os = "windows"), test))]
pub(super) type WindowHandles = ();

#[cfg(test)]
pub(super) fn take_test_badges() -> Vec<Option<String>> {
    TEST_BADGES.with(|badges| std::mem::take(&mut *badges.borrow_mut()))
}

pub(super) fn apply(
    label: Option<&str>,
    description: &str,
    windows: &WindowHandles,
) -> Result<(), String> {
    #[cfg(test)]
    {
        let _ = (description, windows);
        TEST_BADGES.with(|badges| badges.borrow_mut().push(label.map(str::to_owned)));
        return Ok(());
    }
    #[cfg(all(not(test), target_os = "windows"))]
    {
        return platform::apply(label, description, windows);
    }
    #[cfg(all(not(test), target_os = "macos"))]
    {
        let _ = windows;
        return platform::apply(label);
    }
    #[cfg(all(not(test), not(any(target_os = "windows", target_os = "macos"))))]
    {
        let _ = (label, description, windows);
        return Ok(());
    }
}

#[cfg(all(not(test), target_os = "macos"))]
mod platform {
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSApplication;
    use objc2_foundation::NSString;

    pub(super) fn apply(label: Option<&str>) -> Result<(), String> {
        let Some(mtm) = MainThreadMarker::new() else {
            return Err("the Dock badge must be set on the main thread".to_owned());
        };
        let application = NSApplication::sharedApplication(mtm);
        let tile = application.dockTile();
        let badge = label.map(NSString::from_str);
        tile.setBadgeLabel(badge.as_deref());
        Ok(())
    }
}

#[cfg(all(not(test), target_os = "windows"))]
mod platform {
    use std::cell::RefCell;
    use std::ffi::c_void;

    use windows::core::{w, HSTRING, IUnknown};
    use windows::Win32::Foundation::{COLORREF, HWND, RECT};
    use windows::Win32::Graphics::Gdi::{
        CreateBitmap, CreateCompatibleDC, CreateDIBSection, CreateFontW, CreateSolidBrush,
        DeleteDC, DeleteObject, DrawTextW, Ellipse, SelectObject, SetBkMode, SetTextColor,
        ANTIALIASED_QUALITY, BI_RGB,
        BITMAPINFO, BITMAPINFOHEADER, CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET, DIB_RGB_COLORS,
        DT_CENTER, DT_SINGLELINE, DT_VCENTER, FW_BOLD, HBRUSH, HBITMAP, HDC, HFONT, HGDIOBJ,
        OUT_DEFAULT_PRECIS, TRANSPARENT,
    };
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
    };
    use windows::Win32::UI::Shell::{ITaskbarList3, TaskbarList};
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateIconIndirect, DestroyIcon, HICON, ICONINFO,
    };

    /// The overlay is drawn at 32×32 so the taskbar can scale it for the
    /// current DPI; Windows draws it at the small-icon size of the button.
    const BADGE_PX: i32 = 32;
    /// A high-contrast badge fill: white numerals on a red disc read on both
    /// light and dark taskbars. Native overlay ink is outside the theme's
    /// semantic roles because Windows composites it over the app icon, not
    /// over a lyrel surface.
    const BADGE_FILL: COLORREF = COLORREF(0x0020_2DC4);

    thread_local! {
        static TASKBAR: RefCell<Option<ITaskbarList3>> = const { RefCell::new(None) };
        static ICON: RefCell<Option<(String, HICON)>> = const { RefCell::new(None) };
    }

    pub(super) fn apply(
        label: Option<&str>,
        description: &str,
        windows: &[HWND],
    ) -> Result<(), String> {
        if windows.is_empty() {
            // No taskbar button exists while the app is tray-retired; the
            // caller keeps the label pending and the next reconcile applies
            // it once a window is back.
            return Ok(());
        }
        let icon = match label {
            Some(label) => Some(icon_for(label)?),
            None => None,
        };
        let description = HSTRING::from(description);
        let mut first_error = None;
        TASKBAR.with(|cell| {
            let taskbar = match cached_taskbar(cell) {
                Ok(taskbar) => taskbar,
                Err(error) => {
                    first_error = Some(error);
                    return;
                }
            };
            for &hwnd in windows {
                // SAFETY: `hwnd` names a live top-level window of this
                // process, and the icon stays alive in `ICON` until it is
                // replaced or cleared.
                let result =
                    unsafe { taskbar.SetOverlayIcon(hwnd, icon.unwrap_or_default(), &description) };
                if let Err(error) = result {
                    first_error.get_or_insert_with(|| format!("SetOverlayIcon failed: {error}"));
                }
            }
        });
        match first_error {
            Some(error) => {
                // The taskbar connection may have died with Explorer. Drop the
                // cached object so the next tick creates a fresh one.
                TASKBAR.with(|cell| *cell.borrow_mut() = None);
                Err(error)
            }
            None => Ok(()),
        }
    }

    fn cached_taskbar(cell: &RefCell<Option<ITaskbarList3>>) -> Result<ITaskbarList3, String> {
        if let Some(taskbar) = cell.borrow().as_ref() {
            return Ok(taskbar.clone());
        }
        let taskbar = create_taskbar()?;
        *cell.borrow_mut() = Some(taskbar.clone());
        Ok(taskbar)
    }

    fn create_taskbar() -> Result<ITaskbarList3, String> {
        // SAFETY: COM is initialized by the GPUI Windows backend; the retry
        // covers a host that has not initialized it yet. `HrInit` is required
        // before the taskbar methods are called.
        unsafe {
            match CoCreateInstance(&TaskbarList, None::<&IUnknown>, CLSCTX_INPROC_SERVER) {
                Ok(taskbar) => {
                    taskbar
                        .HrInit()
                        .map_err(|error| format!("ITaskbarList3::HrInit failed: {error}"))?;
                    Ok(taskbar)
                }
                Err(first) => {
                    let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
                    let taskbar: ITaskbarList3 =
                        CoCreateInstance(&TaskbarList, None::<&IUnknown>, CLSCTX_INPROC_SERVER)
                            .map_err(|second| {
                                format!(
                                    "CoCreateInstance(ITaskbarList3) failed: {first}; retry: {second}"
                                )
                            })?;
                    taskbar
                        .HrInit()
                        .map_err(|error| format!("ITaskbarList3::HrInit failed: {error}"))?;
                    Ok(taskbar)
                }
            }
        }
    }

    fn icon_for(label: &str) -> Result<HICON, String> {
        let cached = ICON.with(|cell| {
            cell.borrow()
                .as_ref()
                .filter(|(cached, _)| cached == label)
                .map(|(_, icon)| *icon)
        });
        if let Some(icon) = cached {
            return Ok(icon);
        }
        let icon = draw_icon(label)?;
        ICON.with(|cell| {
            if let Some((_, old)) = cell.borrow_mut().replace((label.to_owned(), icon)) {
                // SAFETY: `old` was created by this module and is no longer
                // installed on any taskbar button after the replace above.
                unsafe {
                    let _ = DestroyIcon(old);
                }
            }
        });
        Ok(icon)
    }

    fn draw_icon(label: &str) -> Result<HICON, String> {
        let mut canvas = Canvas::new()?;
        canvas.paint(label)?;
        // SAFETY: the canvas owns valid color and mask bitmaps until it is
        // dropped after this call; the system copies them into the icon.
        unsafe { CreateIconIndirect(&canvas.icon_info()) }
            .map_err(|error| format!("CreateIconIndirect failed: {error}"))
    }

    /// One 32-bit top-down DIB plus the resources selected into its memory
    /// DC. `Drop` restores the DC and releases everything, so every early
    /// return in `new` and `paint` leaves no GDI object behind.
    struct Canvas {
        dc: HDC,
        color: HBITMAP,
        mask: HBITMAP,
        font: HFONT,
        brush: HBRUSH,
        bits: *mut u8,
        old_color: HGDIOBJ,
        old_font: HGDIOBJ,
        old_brush: HGDIOBJ,
    }

    impl Canvas {
        fn new() -> Result<Self, String> {
            // SAFETY: every handle is checked before it is selected into the
            // DC, and the caller-visible struct owns everything it creates.
            unsafe {
                let dc = CreateCompatibleDC(None);
                if dc.is_invalid() {
                    return Err("CreateCompatibleDC failed".to_owned());
                }
                let mut header = BITMAPINFOHEADER::default();
                header.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
                header.biWidth = BADGE_PX;
                // Negative height: top-down rows, matching the pixel loop.
                header.biHeight = -BADGE_PX;
                header.biPlanes = 1;
                header.biBitCount = 32;
                header.biCompression = BI_RGB.0;
                let info = BITMAPINFO {
                    bmiHeader: header,
                    bmiColors: [Default::default()],
                };
                let mut bits = std::ptr::null_mut::<c_void>();
                let color = match CreateDIBSection(
                    Some(dc),
                    &info,
                    DIB_RGB_COLORS,
                    &mut bits,
                    None,
                    0,
                ) {
                    Ok(bitmap) => bitmap,
                    Err(error) => {
                        let _ = DeleteDC(dc);
                        return Err(format!("CreateDIBSection failed: {error}"));
                    }
                };
                // A zeroed monochrome mask means "opaque everywhere"; the
                // color bitmap's alpha channel carries the disc's shape.
                let mask_bits = vec![0u8; BADGE_PX as usize * 4];
                let mask = CreateBitmap(
                    BADGE_PX,
                    BADGE_PX,
                    1,
                    1,
                    Some(mask_bits.as_ptr().cast()),
                );
                if mask.is_invalid() {
                    let _ = DeleteObject(HGDIOBJ(color.0));
                    let _ = DeleteDC(dc);
                    return Err("CreateBitmap failed".to_owned());
                }
                let brush = CreateSolidBrush(BADGE_FILL);
                let font = CreateFontW(
                    -(BADGE_PX - 10),
                    0,
                    0,
                    0,
                    FW_BOLD.0 as i32,
                    0,
                    0,
                    0,
                    DEFAULT_CHARSET,
                    OUT_DEFAULT_PRECIS,
                    CLIP_DEFAULT_PRECIS,
                    ANTIALIASED_QUALITY,
                    0,
                    w!("Segoe UI"),
                );
                if brush.is_invalid() || font.is_invalid() {
                    if !brush.is_invalid() {
                        let _ = DeleteObject(HGDIOBJ(brush.0));
                    }
                    if !font.is_invalid() {
                        let _ = DeleteObject(HGDIOBJ(font.0));
                    }
                    let _ = DeleteObject(HGDIOBJ(color.0));
                    let _ = DeleteObject(HGDIOBJ(mask.0));
                    let _ = DeleteDC(dc);
                    return Err("GDI badge resources unavailable".to_owned());
                }
                let old_color = SelectObject(dc, HGDIOBJ(color.0));
                let old_brush = SelectObject(dc, HGDIOBJ(brush.0));
                let old_font = SelectObject(dc, HGDIOBJ(font.0));
                Ok(Self {
                    dc,
                    color,
                    mask,
                    font,
                    brush,
                    bits: bits.cast(),
                    old_color,
                    old_font,
                    old_brush,
                })
            }
        }

        fn paint(&mut self, label: &str) -> Result<(), String> {
            // SAFETY: the DC and its selected objects are owned by `self`,
            // and `bits` points at `BADGE_PX²` 32-bit pixels from
            // `CreateDIBSection`.
            unsafe {
                let _ = Ellipse(self.dc, 1, 1, BADGE_PX - 1, BADGE_PX - 1);
                let _ = SetBkMode(self.dc, TRANSPARENT);
                let _ = SetTextColor(self.dc, COLORREF(0x00FF_FFFF));
                let mut text: Vec<u16> = label.encode_utf16().collect();
                let mut rect = RECT {
                    left: 0,
                    top: 0,
                    right: BADGE_PX,
                    bottom: BADGE_PX,
                };
                let drawn = DrawTextW(
                    self.dc,
                    &mut text,
                    &mut rect,
                    DT_CENTER | DT_VCENTER | DT_SINGLELINE,
                );
                if drawn == 0 {
                    return Err("DrawTextW failed".to_owned());
                }
                // GDI writes no alpha, so derive it from the painted ink: the
                // black background becomes transparent, every disc and glyph
                // pixel stays opaque.
                let pixels = std::slice::from_raw_parts_mut(
                    self.bits,
                    BADGE_PX as usize * BADGE_PX as usize * 4,
                );
                for pixel in pixels.chunks_exact_mut(4) {
                    let ink = pixel[0] | pixel[1] | pixel[2] != 0;
                    pixel[3] = if ink { 0xFF } else { 0 };
                }
            }
            Ok(())
        }

        fn icon_info(&self) -> ICONINFO {
            ICONINFO {
                fIcon: windows::core::BOOL(1),
                xHotspot: 0,
                yHotspot: 0,
                hbmMask: self.mask,
                hbmColor: self.color,
            }
        }
    }

    impl Drop for Canvas {
        fn drop(&mut self) {
            // SAFETY: all handles were created by `Canvas::new`, and the
            // original DC objects are restored before their replacements are
            // deleted.
            unsafe {
                let _ = SelectObject(self.dc, self.old_color);
                let _ = SelectObject(self.dc, self.old_font);
                let _ = SelectObject(self.dc, self.old_brush);
                let _ = DeleteObject(HGDIOBJ(self.font.0));
                let _ = DeleteObject(HGDIOBJ(self.brush.0));
                let _ = DeleteObject(HGDIOBJ(self.color.0));
                let _ = DeleteObject(HGDIOBJ(self.mask.0));
                let _ = DeleteDC(self.dc);
            }
        }
    }
}
