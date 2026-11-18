//! The installation-layout markers and in-progress-replacement naming both
//! sides of an update agree on.
//!
//! The GUI's update coordinator reads the markers to recognise the running
//! installation; the updater executable writes and restores them. The byte
//! contents are frozen: a marker whose content changed means "not this
//! layout", never "a newer layout", so an old build never misreads a future
//! one.

/// Marks a per-user or all-users Inno Setup installation. Content is `inno-v1`.
pub const INNO_INSTALL_MARKER: &str = ".lyrel-inno-install";
/// Frozen marker bytes; a mismatch refuses the layout rather than guessing.
pub const INNO_INSTALL_MARKER_CONTENT: &[u8] = b"inno-v1";

/// Marks a portable ZIP installation. Content is `portable-v1`.
pub const PORTABLE_MARKER: &str = ".lyrel-portable";
/// Frozen marker bytes; a mismatch refuses the layout rather than guessing.
pub const PORTABLE_MARKER_CONTENT: &[u8] = b"portable-v1";

/// What the updater names the unique directory it moves the old portable
/// files into while replacing them.
pub const PORTABLE_BACKUP_PREFIX: &str = ".lyrel-update-backup-";

/// Lives inside a portable-update backup from before the first installed
/// file moves until the replacement is fully in place. A backup found later
/// still carrying it names a replacement that was cut short — power loss, a
/// kill — and an installation that may mix two versions; one without it is a
/// finished update whose backup deletion lost to an antivirus scan. The app
/// reads it at launch to decide whether to warn.
pub const PORTABLE_BACKUP_INCOMPLETE: &str = ".lyrel-replace-incomplete";
