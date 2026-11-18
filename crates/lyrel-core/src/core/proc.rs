//! Process-spawn helpers.
//!
//! The std-only helpers live in `lyrel-protocol` and are re-exported here
//! unchanged; the tokio variant stays in this crate because the SSH engine is
//! the only caller and `lyrel-protocol` links no tokio.

pub use lyrel_protocol::core::proc::{hide_console, output_within};

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub fn hide_console_tokio(cmd: &mut tokio::process::Command) -> &mut tokio::process::Command {
    #[cfg(windows)]
    {
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}
