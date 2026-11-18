//! One integration-test binary for `lyrel-core`.
//!
//! Cargo links one executable per top-level `tests/*.rs` file. The skills and
//! statistics consumers share this crate's graph and nothing else, so they live
//! under `suite/` and are mounted here instead of paying one heavy link each.
//! Module paths are the old file stems, which keeps `cargo test <filter>`
//! working by substring; `--exact` callers must use the prefixed test name.

#[path = "suite/skills.rs"]
mod skills;
#[path = "suite/skills_catalog.rs"]
mod skills_catalog;
#[path = "suite/skills_install.rs"]
mod skills_install;
#[path = "suite/skills_maintenance.rs"]
mod skills_maintenance;
#[path = "suite/statistics_pricing.rs"]
mod statistics_pricing;
