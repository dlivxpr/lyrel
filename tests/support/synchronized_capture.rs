// Same logical range and 40-column geometry at the rooted GUI and real CLI
// exits. Neither consumer derives its expected text from the other's cache.
pub const SYNC_CAPTURE_TEXT: &str =
    "sync_宽e\u{301}_wrap_0123456789_abcdefghijklmnopqrstuvwxyz_0123456789";
pub const SYNC_NEXT_TEXT: &str = "next_宽e\u{301}_0123456789_abcdefghijklmnopqrstuvwxyz";

pub fn synchronized_capture_frame() -> Vec<u8> {
    // raw_pty deliberately replaces Windows output mode flags without EOL
    // wrap. This scenario compares consumers at the same explicit VT mode,
    // rather than assuming the child console inherited Unix-style autowrap.
    format!("\x1b[?7h\x1b[2J\x1b[3J\x1b[H\x1b[?2026h\x1b[4;1H{SYNC_CAPTURE_TEXT}").into_bytes()
}
