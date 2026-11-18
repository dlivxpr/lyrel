use crate::pty::raw_pty;

// Real raw-mode PTY observers for the mouse ownership consumer tests.
//
// The lightweight fixture process embeds this module; expected reports are
// still asserted from the production terminal handlers on the other end of
// the PTY.

pub fn mouse_reporting_fixture_recorder() {
    use std::io::{Read as _, Write as _};
    if std::env::var_os("LYREL_PANE").is_none() {
        return;
    }
    raw_pty();
    let mut input = std::io::stdin().lock();
    let mut output = std::io::stdout().lock();
    output
        .write_all(b"\x1b[?1002h\x1b[?1006h\r\nMOUSE_READY\r\n")
        .unwrap();
    output.flush().unwrap();
    let mut report = Vec::new();
    let mut serial = 0;
    loop {
        let mut byte = [0];
        if input.read(&mut byte).unwrap() == 0 {
            return;
        }
        match byte[0] {
            4 => {
                serial += 1;
                let path = format!("mouse-{serial}.bin");
                std::fs::write("mouse-pending.bin", &report).unwrap();
                std::fs::rename("mouse-pending.bin", path).unwrap();
                report.clear();
            }
            5 => {
                output.write_all(b"\x1b[?1002l\x1b[?1006l").unwrap();
                output.flush().unwrap();
            }
            6 => {
                output.write_all(b"\x1b[?1002h\x1b[?1006h").unwrap();
                output.flush().unwrap();
            }
            7 => {
                for row in 0..72 {
                    write!(output, "WHEEL-{row:03}\r\n").unwrap();
                }
                output.write_all(b"\x1b[8;1HMOUSE-HISTORY-READY").unwrap();
                output.flush().unwrap();
            }
            8 => {
                output.write_all(b"\x1b[?1002l\x1b[?1006l\x1b[?1049h\x1b[?1007h\x1b[?1h\x1b[HALT-WHEEL-READY").unwrap();
                output.flush().unwrap();
            }
            9 => {
                output
                    .write_all(b"\x1b[?1049l\x1b[?1l\x1b[8;1HNORMAL-WHEEL-READY")
                    .unwrap();
                output.flush().unwrap();
            }
            byte => report.push(byte),
        }
    }
}

/// Wide-pane variant: negotiates UTF-8 coordinates and emits DECRQM probes.
/// The terminal's replies and mouse reports are recorded on this child's
/// real PTY stdin; fixture command bytes cannot occur inside those reports.
pub fn mouse_utf8_fixture_recorder() {
    use std::io::{Read as _, Write as _};
    if std::env::var_os("LYREL_PANE").is_none() {
        return;
    }
    raw_pty();
    let mut input = std::io::stdin().lock();
    let mut output = std::io::stdout().lock();
    output
        .write_all(b"\x1b[?1002h\x1b[?1005h\r\nUTF8_MOUSE_READY\r\n")
        .unwrap();
    output.flush().unwrap();
    let mut report = Vec::new();
    let mut serial = 0;
    let mut query_serial = 0;
    loop {
        let mut byte = [0];
        if input.read(&mut byte).unwrap() == 0 {
            return;
        }
        match byte[0] {
            4 => {
                serial += 1;
                let path = format!("mouse-utf8-{serial}.bin");
                std::fs::write("mouse-utf8-pending.bin", &report).unwrap();
                std::fs::rename("mouse-utf8-pending.bin", path).unwrap();
                report.clear();
            }
            // Probe the modes this fixture negotiates plus one it must not
            // gain. Replies come back on this same stdin, so the parent sees
            // them in the next checkpoint dump; the marker only leaves once
            // all three arrived, which keeps the exchange deterministic.
            16 => {
                output
                    .write_all(b"\x1b[?1005$p\x1b[?1006$p\x1b[?1015$p")
                    .unwrap();
                output.flush().unwrap();
                let mut replies = Vec::new();
                let mut seen = 0;
                while seen < 3 {
                    let mut b = [0];
                    if input.read(&mut b).unwrap() == 0 {
                        return;
                    }
                    report.push(b[0]);
                    replies.push(b[0]);
                    if replies.ends_with(b"$y") {
                        seen += 1;
                    }
                }
                query_serial += 1;
                write!(output, "QUERIES_DONE_{query_serial}").unwrap();
                output.flush().unwrap();
            }
            17 => {
                output.write_all(b"\x1b[?1006hSGR_ENCODING_ON").unwrap();
                output.flush().unwrap();
            }
            18 => {
                output.write_all(b"\x1b[?1005hUTF8_ENCODING_ON").unwrap();
                output.flush().unwrap();
            }
            19 => {
                output
                    .write_all(b"\x1b[?1005l\x1b[?1006lLEGACY_ENCODING")
                    .unwrap();
                output.flush().unwrap();
            }
            20 => {
                output.write_all(b"\x1b[?1002lREPORTING_OFF").unwrap();
                output.flush().unwrap();
            }
            byte => report.push(byte),
        }
    }
}
