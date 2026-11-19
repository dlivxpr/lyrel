//! How well a row answers a query. One scorer for every tab, so the All tab
//! can put two tabs' best rows side by side and have the order mean something.
//!
//! The raw subsequence scorer lives in `lyrel-ui-base` (`fuzzy::fuzzy_score`),
//! shared with the host listings; this module adds the command-row policy —
//! subtitles, aliases and the frecency nudge — on top of it.

use super::command::Item;
use lyrel_ui_base::fuzzy::fuzzy_score;

/// Whether every whitespace-separated word of `query` occurs in `text`,
/// ignoring case.
fn words_appear_in(query: &str, text: &str) -> bool {
    let text = text.to_lowercase();
    query
        .split_whitespace()
        .all(|word| text.contains(&word.to_lowercase()))
}

/// How far behind the visible label an alias hit lands. An alias *is* the
/// command's own name, only in another language, so the gap is small — but two
/// commands that both match must still be ordered by the text on screen.
const ALIAS_PENALTY: i32 = 10;

pub(crate) fn item_score(query: &str, cmd: &Item) -> Option<i32> {
    let title = fuzzy_score(query, &cmd.title);
    // A subtitle is prose — a description, a host — and a query's letters
    // strewn across a sentence are no match for it: "theme" found "Git:
    // Discard All Changes" through "Throws away every uncommitted change". The
    // words typed have to be in it as typed.
    let subtitle = cmd
        .subtitle
        .as_deref()
        .filter(|s| words_appear_in(query, s))
        .and_then(|s| fuzzy_score(query, s))
        .map(|s| s / 2 - 25);
    // Aliases only ever decide whether a row is in the list and where it sits.
    // The row keeps rendering `title` untouched, so a hit on wording the user
    // cannot see can never end up underlined against the wrong characters.
    let alias = cmd
        .aliases
        .iter()
        .filter_map(|a| fuzzy_score(query, a))
        .max()
        .map(|s| s - ALIAS_PENALTY);
    [title, subtitle, alias].into_iter().flatten().max()
}

/// A bounded nudge, not a re-ranking. Two commands that match the query about
/// equally well should come out in the order they actually get run, but a
/// command used daily must not outrank a plainly better match — a single extra
/// matched character is worth 16 before bonuses.
pub(crate) fn frecency_bonus(used: f64) -> i32 {
    const CEILING: f64 = 24.0;
    if used <= 0.0 {
        return 0;
    }
    (used.ln_1p() * 8.0).min(CEILING).round() as i32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::search::CommandKind;
    use lyrel_ui_base::i18n::L10nKey;

    #[test]
    fn frecency_nudges_without_overruling_the_match() {
        assert_eq!(frecency_bonus(0.0), 0, "an unused command gets nothing");
        assert!(frecency_bonus(1.0) > 0, "one use is worth something");
        // Monotone in usage.
        assert!(frecency_bonus(20.0) > frecency_bonus(1.0));
        // Bounded: never worth more than one and a half matched characters,
        // so a command run a thousand times still loses to a better match.
        assert!(
            frecency_bonus(1_000.0) <= 24,
            "frecency is a tiebreak, not a ranking"
        );
        assert_eq!(frecency_bonus(1_000.0), frecency_bonus(10_000.0));
    }

    #[test]
    fn subtitle_matches_are_found_but_discounted() {
        let cmd =
            Item::new("prod-web", CommandKind::NewTab).with_subtitle("deploy@10.0.0.5".to_string());
        assert!(item_score("10.0.0", &cmd).is_some());
        let title_hit = item_score("prod", &cmd).expect("title matches");
        let subtitle_hit = item_score("deploy", &cmd).expect("subtitle matches");
        assert!(title_hit > subtitle_hit);
    }

    #[test]
    fn the_remote_files_panel_is_found_as_sftp() {
        let cmd =
            Item::localized(L10nKey::CmdSshRemoteFiles, CommandKind::ToggleSftp).with_alias("SFTP");
        assert!(item_score("sftp", &cmd).is_some());
    }

    #[test]
    fn letters_strewn_across_a_description_do_not_match_it() {
        let cmd = Item::new("Git: Discard All Changes", CommandKind::NewTab)
            .with_subtitle("Throws away every uncommitted change in the working tree.".to_string());
        assert!(item_score("theme", &cmd).is_none());
        assert!(item_score("uncommitted", &cmd).is_some());
        assert!(item_score("working tree", &cmd).is_some());
    }

    #[test]
    fn a_translated_command_is_still_found_by_its_english_name() {
        lyrel_ui_base::i18n::set_locale("zh-CN");
        let cmd = Item::localized(L10nKey::CmdChangeTheme, CommandKind::OpenThemePicker);
        assert!(
            !cmd.title.to_lowercase().contains("theme"),
            "the row shows the Chinese label: {:?}",
            cmd.title
        );
        assert!(
            item_score("theme", &cmd).is_some(),
            "an English query must find the Chinese-labelled row"
        );
        assert!(
            item_score("テーマ", &cmd).is_none(),
            "retired translations must not remain as hidden search aliases"
        );
        // The displayed label still works, and still wins: the same query
        // against the locale that shows it scores higher.
        let zh = item_score("更改主题", &cmd).expect("the shown label matches");
        lyrel_ui_base::i18n::set_locale("en");
        let en = Item::localized(L10nKey::CmdChangeTheme, CommandKind::OpenThemePicker);
        assert!(zh > 0 && en.title.contains("Theme"));
        assert!(
            item_score("theme", &en).unwrap() > item_score("theme", &cmd).unwrap(),
            "a hit on the visible label must outrank the same hit on an alias"
        );
    }
}
