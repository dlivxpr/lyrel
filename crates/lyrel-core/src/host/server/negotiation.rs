//! Control HELLO and feature negotiation without changing wire policy.
use std::io::{self, Read};
use std::path::PathBuf;

use crate::daemon::control::{
    CONTROL_VERSION, ControlClientMsg, ControlHello, ControlHelloOk, ControlServerMsg, feature,
};
use crate::host::Host;

use super::Services;
use super::output::Sink;

pub(super) fn handshake<R: Read>(
    r: &mut R,
    sink: &Sink,
    host: &dyn Host,
    services: &Services,
) -> io::Result<Option<ControlHello>> {
    let hello = match ControlClientMsg::read(r)? {
        ControlClientMsg::Hello(h) => h,
        other => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("control peer opened with {other:?} instead of HELLO"),
            ));
        }
    };

    let mut features = vec![
        feature::CONTROL.to_string(),
        feature::HOST_RPC.to_string(),
        feature::READ_LINK.to_string(),
        feature::STDIO_BRIDGE.to_string(),
        // Every host this server fronts can search its own files; the
        // request only needs a name so an older client's peer is never sent
        // it (see `RemoteHost::search_content`).
        feature::CONTENT_SEARCH.to_string(),
        feature::SESSION_OPERATIONS.to_string(),
        feature::HERMES_HISTORY.to_string(),
        feature::HERMES_STATISTICS.to_string(),
        feature::OPENCODE_STATISTICS.to_string(),
    ];
    if host.id().is_local() {
        features.push(feature::HOST_METRICS.to_string());
    }
    if host.agent_environment().is_ok() {
        features.push(feature::AGENT_ENVIRONMENT.to_string());
    }
    // Only where there are panes to ask about. A control peer serving no panes
    // would answer every ask with an empty list, which reads to a client as
    // "nothing is listening" rather than as "I cannot tell you".
    if services.panes.is_some() {
        features.push(feature::PANE_PROCS.to_string());
    }
    if services.machine.is_some() {
        features.push(feature::MACHINE_TREE.to_string());
    }
    // A sleeping tab is a mark in the tree *and* stopped panes; a peer that can
    // do only half of that would leave a tab asleep with its shells running,
    // or its shells stopped with nothing saying the tab is meant to wake.
    if services.machine.is_some() && services.panes.is_some() {
        features.push(feature::TAB_HIBERNATE.to_string());
        // The same pair for the same reason: a closed tab is an entry in the
        // tree *and* screens kept by whoever stops the panes.
        features.push(feature::CLOSED_TABS.to_string());
    }
    // The pane daemon's features ride along, same as `protocol_version` above:
    // a pane connection answers exactly one message, so a client that wanted
    // them from `ClientMsg::Version` would pay a whole routed connection — for
    // ssh and WSL, a whole bridge process — per pane. The hello already travels
    // once per host and is cached on the link, so this is where a client can
    // afford to learn what the panes it routes here will do (the resize echo
    // above all). Additive names are safe: every check on either end asks for
    // a name, never for the exact list.
    features.extend(crate::daemon::protocol::DaemonVersion::current().features);

    sink.send(&ControlServerMsg::HelloOk(ControlHelloOk {
        control_version: CONTROL_VERSION,
        protocol_version: crate::daemon::protocol::PROTOCOL_VERSION,
        build: env!("CARGO_PKG_VERSION").to_string(),
        separator: host.separator(),
        home: home_dir()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default(),
        agent_environment: host.agent_environment().ok(),
        features,
        instance: crate::daemon::control::server_instance().to_string(),
    }))?;

    if hello.control_version != CONTROL_VERSION {
        log::warn!(
            "control peer speaks v{}, this build speaks v{CONTROL_VERSION}; closing",
            hello.control_version
        );
        return Ok(None);
    }
    Ok(Some(hello))
}

fn home_dir() -> Option<PathBuf> {
    let pick = |k: &str| {
        std::env::var_os(k)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    pick("HOME").or_else(|| pick("USERPROFILE"))
}

#[cfg(all(test, unix))]
mod tests;
