//! One definition of how a git status looks, shared by the source control
//! panel, the file tree and the diff overlay's file cards.
//!
//! Keeping it in one place is the point: three hand-written A/M/D/R tables
//! drift, and the drift is invisible until someone notices the same file wears
//! two different letters in two different places.

use gpui_kit::component::ActiveTheme as _;
use lyrel_core::core::git::status::DecoStatus;

/// The single letter shown in the 14px badge column. `Ignored` has none — a
/// tree full of `!` is noise, not information.
pub(crate) fn status_glyph(s: DecoStatus) -> &'static str {
    match s {
        DecoStatus::Ignored => "",
        DecoStatus::Untracked => "?",
        DecoStatus::Added => "A",
        DecoStatus::Modified => "M",
        DecoStatus::Renamed => "R",
        DecoStatus::Deleted => "D",
        DecoStatus::Conflict => "U",
    }
}

/// Semantic roles keep their meaning across terminal palettes and are
/// contrast-checked on every UI surface in `presets.rs`.
pub(crate) fn status_color(s: DecoStatus, cx: &gpui_kit::App) -> gpui_kit::Hsla {
    let theme = cx.theme();
    match s {
        DecoStatus::Conflict => theme.danger,
        // Muted rather than danger: a deleted file is gone, not broken, and
        // the strikethrough on its name already carries the message.
        DecoStatus::Deleted => theme.muted_foreground,
        DecoStatus::Added | DecoStatus::Untracked => theme.success,
        DecoStatus::Modified => theme.warning,
        DecoStatus::Renamed => theme.info,
        DecoStatus::Ignored => theme.muted_foreground.opacity(0.7),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [DecoStatus; 7] = [
        DecoStatus::Ignored,
        DecoStatus::Untracked,
        DecoStatus::Added,
        DecoStatus::Modified,
        DecoStatus::Renamed,
        DecoStatus::Deleted,
        DecoStatus::Conflict,
    ];

    #[test]
    fn every_status_has_its_own_letter() {
        let mut seen: Vec<&str> = Vec::new();
        for s in ALL {
            let glyph = status_glyph(s);
            if s == DecoStatus::Ignored {
                assert_eq!(glyph, "", "ignored files carry no letter");
                continue;
            }
            assert_eq!(glyph.chars().count(), 1, "{s:?} should be one character");
            assert!(!seen.contains(&glyph), "{glyph} is used twice");
            seen.push(glyph);
        }
        assert_eq!(status_glyph(DecoStatus::Conflict), "U");
        assert_eq!(status_glyph(DecoStatus::Untracked), "?");
    }
}
