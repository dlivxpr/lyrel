//! GUI-free insights above the `lyrel-protocol` Host seam.
//!
//! This crate owns the non-daemon analytics and read-only integrations the
//! workbench surfaces: Host-bound skill discovery/catalog/install/maintenance
//! ([`skills`]), structured coding-agent session telemetry ([`statistics`]),
//! read-only GitHub queries ([`github`]), and desktop usage account models,
//! provider scrapers and browser cookie/session readers (`usage`).
//!
//! Everything here drives real `LocalHost`/`RemoteHost` work through
//! `&dyn lyrel_protocol::host::Host`; nothing touches PTY, SSH setup, or GPUI.

pub mod github;
pub mod skills;
pub mod statistics;
#[cfg(any(target_os = "macos", target_os = "windows"))]
pub mod usage;
