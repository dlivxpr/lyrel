//! The `L10nKey` catalog and its `en`/`zh-CN` translations live in
//! `lyrel-ui-base`; this facade keeps `crate::ui::i18n::` working.

pub use lyrel_ui_base::i18n::{
    L10nKey, LANGUAGE_MODES, agent_result_text, alias_translations, current_language_code,
    language_mode_label, set_locale, t, t_fmt, t_plural,
};
