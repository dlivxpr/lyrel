//! Native GUI language preferences, never LANG/LC_ALL or a PTY locale.

#[cfg(windows)]
pub(super) fn preferred_ui_language() -> Option<String> {
    use windows_sys::Win32::Globalization::{GetUserPreferredUILanguages, MUI_LANGUAGE_NAME};

    let mut count = 0;
    let mut len = 0;
    // SAFETY: the sizing call explicitly accepts a null buffer with zero size;
    // both output pointers refer to live locals.
    if unsafe {
        GetUserPreferredUILanguages(
            MUI_LANGUAGE_NAME,
            &mut count,
            std::ptr::null_mut(),
            &mut len,
        )
    } == 0
        || len == 0
    {
        return None;
    }
    let mut languages = vec![0u16; len as usize];
    // SAFETY: the writable buffer has the exact UTF-16 capacity requested by
    // Windows. A changed preference or other failure falls back to English.
    if unsafe {
        GetUserPreferredUILanguages(
            MUI_LANGUAGE_NAME,
            &mut count,
            languages.as_mut_ptr(),
            &mut len,
        )
    } == 0
        || count == 0
    {
        return None;
    }
    let end = languages.iter().position(|&unit| unit == 0)?;
    if end == 0 {
        return None;
    }
    String::from_utf16(&languages[..end]).ok()
}

#[cfg(target_os = "macos")]
pub(super) fn preferred_ui_language() -> Option<String> {
    // Unlike currentLocale, preferredLanguages is the user's ordered UI
    // language preference, not their region or shell character environment.
    objc2_foundation::NSLocale::preferredLanguages()
        .firstObject()
        .map(|language| language.to_string())
}

#[cfg(not(any(windows, target_os = "macos")))]
pub(super) fn preferred_ui_language() -> Option<String> {
    None
}
