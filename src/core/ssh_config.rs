//! The `~/.ssh/config` import/merge parser lives in `lyrel-ui-base` (pure
//! parsing, no GPUI); this facade keeps `crate::core::ssh_config::` working.

pub use lyrel_ui_base::ssh_config::{
    IMPORTED_GROUP, IgnoredOption, ImportedProfile, ImportReport, MergeStats, ResolvedAlias,
    alias_still_resolves, import_profiles, import_profiles_from, import_report,
    import_report_from, merge_imported, resolve_alias_to_profile, resolve_alias_to_profile_from,
};
