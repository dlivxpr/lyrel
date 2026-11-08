//! Local OS dictionaries only. COM/AppKit objects never cross a worker boundary.
use std::ops::Range;

fn utf16_boundaries(text: &str) -> Vec<usize> {
    let mut boundaries = Vec::with_capacity(text.len() + 1);
    boundaries.push(0);
    for (byte, ch) in text.char_indices() {
        if ch.len_utf16() == 2 { boundaries.push(usize::MAX); }
        boundaries.push(byte + ch.len_utf8());
    }
    boundaries
}
fn byte_range(boundaries: &[usize], start: usize, length: usize) -> Option<Range<usize>> {
    let end = start.checked_add(length)?;
    let first = *boundaries.get(start)?;
    let last = *boundaries.get(end)?;
    (first != usize::MAX && last != usize::MAX && first < last).then_some(first..last)
}

#[cfg(windows)]
mod platform {
    use super::*;
    use windows::{core::{PCWSTR, PWSTR}, Win32::{Globalization::*, System::Com::*}};

    struct Apartment;
    impl Drop for Apartment { fn drop(&mut self) { unsafe { CoUninitialize() }; } }
    fn with_checker<T>(f: impl FnOnce(ISpellChecker) -> T) -> Option<T> {
        unsafe { CoInitializeEx(None, COINIT_MULTITHREADED).ok().ok()?; }
        let _apartment = Apartment;
        let factory: ISpellCheckerFactory = unsafe { CoCreateInstance(&SpellCheckerFactory, None, CLSCTX_INPROC_SERVER).ok()? };
        let mut locale = [0u16; 85];
        let count = unsafe { GetUserDefaultLocaleName(&mut locale) };
        let fallback: Vec<u16> = "en-US\0".encode_utf16().collect();
        let language = if count > 0 && unsafe { factory.IsSupported(PCWSTR(locale.as_ptr())).is_ok_and(|supported| supported.as_bool()) } {
            PCWSTR(locale.as_ptr())
        } else { PCWSTR(fallback.as_ptr()) };
        let checker = unsafe { factory.CreateSpellChecker(language).ok()? };
        Some(f(checker))
    }
    pub(super) fn check(text: &str) -> Vec<Range<usize>> {
        with_checker(|checker| {
            let mut ranges = Vec::new();
            // Chunk at paragraph boundaries: no large contiguous UTF-16 copy, and
            // no truncation of an embedded NUL by the Win32 string ABI.
            let mut base = 0;
            for paragraph in text.split_inclusive(['\n', '\0']) {
                let value = paragraph.trim_end_matches(['\n', '\0']);
                let wide: Vec<u16> = value.encode_utf16().chain(Some(0)).collect();
                let boundaries = utf16_boundaries(value);
                if let Ok(errors) = unsafe { checker.Check(PCWSTR(wide.as_ptr())) } {
                    loop {
                        let mut error = None;
                        let status = unsafe { errors.Next(&mut error) };
                        let Some(error) = error.filter(|_| status.is_ok()) else { break; };
                        if let (Ok(start), Ok(length)) = unsafe { (error.StartIndex(), error.Length()) }
                            && let Some(range) = byte_range(&boundaries, start as usize, length as usize) {
                            ranges.push((base + range.start)..(base + range.end));
                        }
                    }
                }
                base += paragraph.len();
            }
            ranges
        }).unwrap_or_default()
    }
    pub(super) fn suggestions(word: &str) -> Vec<String> {
        with_checker(|checker| {
            let wide: Vec<u16> = word.encode_utf16().chain(Some(0)).collect();
            let Ok(values) = (unsafe { checker.Suggest(PCWSTR(wide.as_ptr())) }) else { return Vec::new(); };
            let mut result = Vec::new();
            loop {
                let mut value = [PWSTR::null()];
                let mut fetched = 0;
                let status = unsafe { values.Next(&mut value, Some(&mut fetched)) };
                if fetched == 0 || status.is_err() { break; }
                if !value[0].is_null() {
                    if let Ok(value) = unsafe { value[0].to_string() } { result.push(value); }
                    unsafe { CoTaskMemFree(Some(value[0].0.cast())) };
                }
            }
            result
        }).unwrap_or_default()
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use super::*;
    use objc2_app_kit::NSSpellChecker;
    use objc2_foundation::{NSRange, NSString};
    pub(super) fn check(text: &str) -> Vec<Range<usize>> {
        objc2::rc::autoreleasepool(|_| {
            let checker = NSSpellChecker::sharedSpellChecker();
            let language = checker.language();
            let string = NSString::from_str(text);
            let boundaries = utf16_boundaries(text);
            let mut offset = 0;
            let mut ranges = Vec::new();
            loop {
                let range = unsafe { checker.checkSpellingOfString_startingAt_language_wrap_inSpellDocumentWithTag_wordCount(
                    &string, offset as isize, Some(&language), false, 0, std::ptr::null_mut()) };
                if range.length == 0 { break; }
                let Some(bytes) = byte_range(&boundaries, range.location, range.length) else { break; };
                ranges.push(bytes);
                let next = range.location.saturating_add(range.length);
                if next <= offset { break; }
                offset = next;
            }
            ranges
        })
    }
    pub(super) fn suggestions(word: &str) -> Vec<String> {
        objc2::rc::autoreleasepool(|_| {
            let checker = NSSpellChecker::sharedSpellChecker();
            let language = checker.language();
            let word = NSString::from_str(word);
            checker.guessesForWordRange_inString_language_inSpellDocumentWithTag(
                NSRange::new(0, word.length()), &word, Some(&language), 0)
                .map(|values| values.iter().map(|value| value.to_string()).collect()).unwrap_or_default()
        })
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
mod platform {
    use super::*;
    pub(super) fn check(_: &str) -> Vec<Range<usize>> { Vec::new() }
    pub(super) fn suggestions(_: &str) -> Vec<String> { Vec::new() }
}
pub(super) fn check(text: &str) -> Vec<Range<usize>> { platform::check(text) }
pub(super) fn suggestions(word: &str) -> Vec<String> { platform::suggestions(word) }
