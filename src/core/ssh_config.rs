//! The `~/.ssh/config` import/merge parser lives in `lyrel-ui-base` (pure
//! parsing, no GPUI); this facade keeps `crate::core::ssh_config::` working.

pub use lyrel_ui_base::ssh_config::{
    IMPORTED_GROUP, ImportedProfile, MergeStats, alias_still_resolves, import_report,
    merge_imported, resolve_alias_to_profile,
};
