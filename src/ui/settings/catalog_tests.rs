use super::*;

#[test]
fn the_catalog_has_unique_titles_and_valid_destinations() {
    for (i, entry) in settings_search_entries().iter().enumerate() {
        assert!(
            !settings_search_entries()[..i]
                .iter()
                .any(|e| e.title == entry.title),
            "duplicate {:?}",
            entry.title
        );
        assert!(SettingsSection::ALL.contains(&entry.section));
    }
    assert!(!SettingsSection::ALL.contains(&SettingsSection::Keybindings));
}

#[test]
fn config_keys_and_cross_language_names_reach_the_same_setting() {
    for locale in ["en", "zh-CN"] {
        crate::ui::i18n::set_locale(locale);
        for (query, title, section) in [
            (
                "gui_language",
                L10nKey::SettingsLanguage,
                SettingsSection::General,
            ),
            (
                "mouse_zoom_modifier",
                L10nKey::SettingsMouseZoom,
                SettingsSection::KeyboardMouse,
            ),
            (
                "per_pane_history",
                L10nKey::SettingsPerPaneHistory,
                SettingsSection::Terminal,
            ),
            (
                "ui_font_size",
                L10nKey::SettingsUiFontSize,
                SettingsSection::Appearance,
            ),
            (
                "Shell program",
                L10nKey::SettingsProgram,
                SettingsSection::Terminal,
            ),
        ] {
            let entry = settings_search_entries()
                .iter()
                .find(|e| e.title == title)
                .unwrap();
            assert!(entry_matches(entry, query), "{locale}: {query}");
            assert_eq!(
                best_matching_section(query).unwrap().profile_label(),
                section.profile_label()
            );
        }
    }
    crate::ui::i18n::set_locale("en");
}

#[test]
fn language_search_keeps_system_mode_and_drops_retired_language_aliases() {
    let entry = settings_search_entries()
        .iter()
        .find(|entry| entry.title == L10nKey::SettingsLanguage)
        .unwrap();
    for locale in ["en", "zh-CN"] {
        crate::ui::i18n::set_locale(locale);
        for query in ["follow system", "跟随系统", "english", "chinese"] {
            assert!(entry_matches(entry, query), "{locale}: {query}");
        }
        for query in ["日本語", "Русский", "japanese", "russian"] {
            assert!(!entry_matches(entry, query), "retired alias: {query}");
        }
    }
}

#[test]
fn modified_settings_compare_their_own_values_only() {
    let mut cfg = Config::default();
    cfg.notify_threshold_secs += 1;
    let changed = settings_search_entries()
        .iter()
        .filter(|e| e.modified(&cfg))
        .map(|e| e.title)
        .collect::<Vec<_>>();
    assert_eq!(changed, vec![L10nKey::SettingsNotifyThreshold]);
}

/// A shortcut is the first thing someone searching a settings window for a
/// feature by name is after, and the Keybindings page was the one page the
/// search could not see into: searching "split" found the settings that
/// merely mention splits and never the row labelled exactly that (#444).
#[test]
fn searching_for_a_feature_finds_its_shortcut() {
    crate::ui::i18n::set_locale("en");
    let kb = SettingsSection::Keybindings;

    // Split Right and Split Down, at least — the index carries no entry
    // for either, so before this every one of these counts was zero.
    assert!(
        section_match_count(kb, "split") >= 2,
        "got {}",
        section_match_count(kb, "split")
    );
    assert!(keybinding_matches_query("SplitRight", "split right"));

    // The action name is what the docs and `keybindings.json` spell, so a
    // reader arriving from either finds the row they read about.
    assert!(keybinding_matches_query("ScmSync", "scmsync"));

    // An empty query matches no row, or clearing the box would filter the
    // page down to nothing rather than back to every row. (The count above
    // it answers `contains("")` for the one indexed entry this section has
    // always carried, which is why the page gates on the query itself
    // before it consults either.)
    assert!(!keybinding_matches_query("SplitRight", ""));
    assert_eq!(keybinding_match_count(""), 0);

    // A query this page cannot answer leaves it alone — the page only
    // filters itself when it has something to show.
    assert_eq!(section_match_count(kb, "no such action anywhere"), 0);
}

#[test]
fn a_row_is_marked_by_its_label_or_by_the_keywords_behind_it() {
    // Straight label hit.
    assert!(row_matches_query(
        SettingsSection::Appearance,
        "Blur",
        "blur"
    ));
    // Keyword hit: the label says "Theme" and nothing more, but the index
    // says that row answers "palette".
    assert!(row_matches_query(
        SettingsSection::Appearance,
        t(L10nKey::SettingsThemeIntroTitle),
        "palette"
    ));
    // A row on some other page is not a hit just because the query matches
    // an entry elsewhere.
    assert!(!row_matches_query(
        SettingsSection::Terminal,
        "Blur",
        "palette"
    ));
    // An empty query marks nothing at all, so no page ever renders greyed
    // out just because the field is focused.
    assert!(!row_matches_query(SettingsSection::Appearance, "Blur", ""));
}

#[test]
fn a_query_that_matches_nothing_is_distinguishable_from_one_that_does() {
    assert_eq!(total_match_count("zzqqxx"), 0);
    assert!(total_match_count("blur") > 0);
    assert!(total_match_count("palette") > 0);
}

#[test]
fn every_section_has_search_entries() {
    for section in SettingsSection::ALL {
        let n = settings_search_entries()
            .iter()
            .filter(|e| e.section == section)
            .count();
        assert!(
            n > 0,
            "section {:?} has no search entries",
            section.profile_label()
        );
    }
}

#[test]
fn best_matching_section_can_reach_every_section() {
    for section in SettingsSection::ALL {
        let entry = settings_search_entries()
            .iter()
            .find(|e| e.section == section)
            .expect("checked by every_section_has_search_entries");
        let query = t(entry.title).to_lowercase();
        let landed = best_matching_section(&query);
        assert!(
            landed.is_some(),
            "query {query:?} matched nothing at all (section {:?})",
            section.profile_label()
        );
    }
}

#[test]
fn previously_unsearchable_settings_are_findable() {
    use SettingsSection::*;
    let cases = [
        ("opacity", Appearance),
        ("blur", Appearance),
        ("completion", Terminal),
        ("ctrl-r", Terminal),
        ("grouping", General),
        ("threshold", General),
        ("report mouse", KeyboardMouse),
        ("nushell", Terminal),
        ("open files with", Terminal),
        ("bell", Terminal),
        ("known_hosts", Ssh),
        ("symlink", Agents),
        // Rows the index had no entry for at all, so the query counted
        // nothing, no badge appeared and no row lit up: the whole Updates
        // group, and Smooth scrolling between two rows that were both
        // findable.
        ("smooth", Terminal),
        ("metered", About),
        ("automatic", About),
        // A headline feature the index had never heard of: "background
        // image" matched nothing, and typing it walked the page to About
        // because "background" alone hits Download updates in the
        // background.
        ("background image", Appearance),
        ("wallpaper", Appearance),
        ("image opacity", Appearance),
        #[cfg(target_os = "windows")]
        ("material", Appearance),
        #[cfg(target_os = "windows")]
        ("mica", Appearance),
        #[cfg(target_os = "windows")]
        ("acrylic", Appearance),
    ];
    for (query, expected) in cases {
        assert_eq!(
            best_matching_section(query).map(|s| s.profile_label()),
            Some(expected.profile_label()),
            "query {query:?} should land on {:?}",
            expected.profile_label()
        );
    }
}

/// The `lyrel` CLI exists so scripts and coding agents can drive lyrel, so it
/// lives with the other agent integrations rather than under About.
#[test]
fn command_line_tool_is_searchable_under_agents() {
    let entry = settings_search_entries()
        .iter()
        .find(|entry| entry.title == L10nKey::SettingsInstallCliOnPath)
        .expect("the CLI setting should be searchable");

    assert_eq!(entry.section.profile_label(), "settings:agents");
}

#[test]
fn hermes_integration_has_one_search_destination() {
    assert_eq!(section_match_count(SettingsSection::Agents, "hermes"), 1);
}
