//! Connection-local subscriptions; no independent transport or publication authority.
mod git;
mod layout;
mod watch;

pub(super) use git::GitStreams;
pub(super) use layout::subscribe_machine;
pub(super) use watch::Watches;
