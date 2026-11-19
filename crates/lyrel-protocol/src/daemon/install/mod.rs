//! The naming half of remote-server installation: the release asset names,
//! the dialect-derived server filename (`lyrel-server-c{control}p{protocol}`),
//! and the version handshake a probed server answers with. Pure wire-side
//! naming — the downloader, packager, uploaders, and install policies live in
//! `lyrel-core`, which re-exports these so existing
//! `lyrel_core::daemon::install::*` paths keep working.

pub mod asset;

/// Argument a client uses to probe the server's wire versions.
pub const PROTOCOL_FLAG: &str = "--protocol";

/// This build's version, as shipped in every version handshake and install
/// request. One workspace version everywhere: `lyrel-protocol` shares the
/// workspace `version`, so this is the same string the installer in core
/// reports.
pub fn client_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// How a probed remote server answers `--protocol`: the two dialect numbers
/// and the build string. A client installs over a server only when both
/// numbers differ from its own — same-dialect means the running file already
/// speaks the wire exactly as this build would.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RemoteProtocol {
    pub control: u32,
    pub protocol: u32,
    pub build: String,
}

impl RemoteProtocol {
    pub fn of_this_build() -> RemoteProtocol {
        RemoteProtocol {
            control: crate::daemon::control::CONTROL_VERSION,
            protocol: crate::daemon::protocol::PROTOCOL_VERSION,
            build: client_version().to_string(),
        }
    }

    pub fn dialect(&self) -> (u32, u32) {
        (self.control, self.protocol)
    }

    pub fn serves(&self, other: &RemoteProtocol) -> bool {
        self.control == other.control && self.protocol == other.protocol
    }

    pub fn to_line(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    pub fn parse(stdout: &str) -> Option<RemoteProtocol> {
        let line = stdout.lines().rev().find(|l| !l.trim().is_empty())?;
        serde_json::from_str(line.trim()).ok()
    }
}
