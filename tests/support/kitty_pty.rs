use std::collections::VecDeque;
use std::io::{Read, Write};

pub fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(text, "{byte:02x}").unwrap();
    }
    text
}

pub fn unhex(text: &str) -> Vec<u8> {
    assert_eq!(text.len() % 2, 0);
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
        .collect()
}

pub fn raw_pty() {
    #[cfg(unix)]
    unsafe {
        let mut mode = std::mem::zeroed();
        assert_eq!(libc::tcgetattr(0, &mut mode), 0);
        libc::cfmakeraw(&mut mode);
        assert_eq!(libc::tcsetattr(0, libc::TCSANOW, &mode), 0);
    }
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::System::Console::{
            ENABLE_PROCESSED_OUTPUT, ENABLE_VIRTUAL_TERMINAL_INPUT,
            ENABLE_VIRTUAL_TERMINAL_PROCESSING, GetStdHandle, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
            SetConsoleMode,
        };
        assert_ne!(
            SetConsoleMode(
                GetStdHandle(STD_INPUT_HANDLE),
                ENABLE_VIRTUAL_TERMINAL_INPUT
            ),
            0
        );
        assert_ne!(
            SetConsoleMode(
                GetStdHandle(STD_OUTPUT_HANDLE),
                ENABLE_PROCESSED_OUTPUT | ENABLE_VIRTUAL_TERMINAL_PROCESSING,
            ),
            0
        );
    }
}
#[derive(Default)]
struct OutputStats {
    bytes: usize,
    uploads: usize,
    placements: usize,
    prefix: usize,
    header: Option<Vec<u8>>,
}

impl OutputStats {
    fn record(&mut self, bytes: &[u8]) {
        self.bytes += bytes.len();
        for &byte in bytes {
            if let Some(header) = &mut self.header {
                if byte == b';' || byte == 0x1b {
                    let action = header
                        .split(|b| *b == b',')
                        .find_map(|field| field.strip_prefix(b"a="));
                    match action {
                        Some(b"t" | b"T") => self.uploads += 1,
                        Some(b"p") => self.placements += 1,
                        _ => {}
                    }
                    self.header = None;
                } else if header.len() < 4096 {
                    header.push(byte);
                } else {
                    self.header = None;
                }
            } else if byte == b"\x1b_G"[self.prefix] {
                self.prefix += 1;
                if self.prefix == 3 {
                    self.prefix = 0;
                    self.header = Some(Vec::new());
                }
            } else {
                self.prefix = usize::from(byte == 0x1b);
            }
        }
    }
}

/// A whole request line the fixture writes to this pane:
/// `OP serial replies payload-hex [fragment] [gate-hex]`. Replies begin with
/// ESC, so anything else terminated by a newline can only be a request. The
/// grammar check stays strict: an unexpected byte stream still reaches the
/// reply/barrier comparisons unmodified and fails there.
fn spliceable_request(line: &[u8]) -> bool {
    if line.is_empty() || line.iter().any(|b| !(0x20..=0x7e).contains(b)) {
        return false;
    }
    let mut fields = line.split(|b| *b == b' ');
    let Some(op) = fields.next() else {
        return false;
    };
    if op.is_empty() || !op.iter().all(|b| b.is_ascii_uppercase() || *b == b'_') {
        return false;
    }
    let Some(serial) = fields.next() else {
        return false;
    };
    let Some(replies) = fields.next() else {
        return false;
    };
    let Some(payload) = fields.next() else {
        return false;
    };
    !serial.is_empty()
        && serial.iter().all(|b| b.is_ascii_digit())
        && !replies.is_empty()
        && replies.iter().all(|b| b.is_ascii_digit())
        && !payload.is_empty()
        && payload.iter().all(|b| b.is_ascii_hexdigit())
        && fields.all(|field| !field.is_empty() && field.iter().all(|b| b.is_ascii_hexdigit()))
}

/// The next input byte: spliced requests first, then the stream. `None` on a
/// clean EOF, matching the command-line reader's exit condition.
fn next_byte(input: &mut impl Read, pending: &mut VecDeque<u8>) -> std::io::Result<Option<u8>> {
    if let Some(byte) = pending.pop_front() {
        return Ok(Some(byte));
    }
    let mut byte = [0];
    match input.read(&mut byte)? {
        0 => Ok(None),
        _ => Ok(Some(byte[0])),
    }
}

/// Read bytes until `done`, moving complete request lines that the test's
/// next write spliced into this stream position over to `pending`. Commands
/// and the daemon's answers share the one pty input: a producer written
/// without waiting for `KITTY_DONE` (the production `commit_text` path does
/// not) is already queued when a late answer arrives. The comparison callers
/// make on `buffer` stays exact — the spliced line is replayed as the next
/// request, not skipped.
fn read_until(
    input: &mut impl Read,
    pending: &mut VecDeque<u8>,
    buffer: &mut Vec<u8>,
    done: impl Fn(&[u8]) -> bool,
) -> std::io::Result<()> {
    while !done(&buffer[..]) {
        let byte = match next_byte(input, pending)? {
            Some(byte) => byte,
            None => panic!("pty input ended inside a reply read"),
        };
        buffer.push(byte);
        if byte != b'\n' {
            continue;
        }
        let end = buffer.len();
        let start = buffer[..end - 1]
            .iter()
            .rposition(|b| *b == b'\n')
            .map_or(0, |at| at + 1);
        if spliceable_request(&buffer[start..end - 1]) {
            pending.extend(buffer.splice(start..end, []));
        }
    }
    Ok(())
}

// This is a real PTY child, not a daemon seam. Running the current test
// binary avoids an external Python/Node/shell dependency on every platform.
// Each request has a stream barrier; replies are read from the actual PTY
// input and hex-printed so they cannot be intercepted as graphics again.
pub fn pty_sender() {
    if std::env::var_os("LYREL_PANE").is_none() {
        return;
    }
    raw_pty();
    let mut input = std::io::stdin().lock();
    let mut output = std::io::stdout().lock();
    let mut stats = OutputStats::default();
    // Request lines the test wrote while this loop was still reading a
    // reply or barrier for the previous serial; they run next, in order.
    let mut pending = VecDeque::new();
    output.write_all(b"\r\nKITTY_READY\r\n").unwrap();
    output.flush().unwrap();
    loop {
        let mut line = Vec::new();
        loop {
            let Some(byte) = next_byte(&mut input, &mut pending).unwrap() else {
                return;
            };
            if byte == b'\n' {
                break;
            }
            line.push(byte);
        }
        let line = String::from_utf8(line).unwrap();
        let mut fields = line.split_whitespace();
        let operation = fields.next().unwrap();
        let serial = fields.next().unwrap();
        let replies: usize = fields.next().unwrap().parse().unwrap();
        let bytes = unhex(fields.next().unwrap());
        let fragment: usize = fields.next().unwrap_or("0").parse().unwrap();
        let gate = fields
            .next()
            .map(|path| std::path::PathBuf::from(String::from_utf8(unhex(path)).unwrap()));
        if operation == "WAIT" {
            write!(output, "\r\nKITTY_WAITING_{serial}\r\n").unwrap();
            output.flush().unwrap();
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
            while !gate.as_ref().expect("WAIT requires a gate").exists() {
                assert!(std::time::Instant::now() < deadline, "PTY gate deadline");
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
        }
        match operation {
            "EXIT" => return,
            "STAT" => {
                write!(output, "\x1b7\x1b[2;1H\r\nKITTY_STATS_{serial} pid={} bytes={} uploads={} placements={}\r\n\x1b8", std::process::id(), stats.bytes, stats.uploads, stats.placements).unwrap();
                output.flush().unwrap();
            }
            "SEND" | "SEND_OK" | "SEND_ANY" | "WAIT" => {
                stats.record(&bytes);
                for part in bytes.chunks(if fragment == 0 {
                    bytes.len().max(1)
                } else {
                    fragment
                }) {
                    output.write_all(part).unwrap();
                    output.flush().unwrap();
                }
            }
            "FILE_SEND" => {
                // The file contains real APC bytes, not a synthetic daemon
                // event. Keep large regressions off the child's input line.
                let mut file = std::fs::File::open(String::from_utf8(bytes).unwrap()).unwrap();
                let mut buffer = vec![0; fragment.max(1)];
                loop {
                    let count = file.read(&mut buffer).unwrap();
                    if count == 0 {
                        break;
                    }
                    stats.record(&buffer[..count]);
                    output.write_all(&buffer[..count]).unwrap();
                    output.flush().unwrap();
                }
            }
            "FLOOD" => {
                stats.bytes += 49 * 1024 * 1024;
                stats.record(b"\x1b_Ga=T,i=720,s=1,v=1;");
                output.write_all(b"\x1b_Ga=T,i=720,s=1,v=1;").unwrap();
                // 49 MiB exceeds the 48 MiB APC cap without allocating a
                // 98 MiB hex request or relying on an external sender.
                let block = [b'A'; 8192];
                for _ in 0..(49 * 1024 * 1024 / block.len()) {
                    output.write_all(&block).unwrap();
                }
                stats.record(b"\x1b\\after_oversize");
                output.write_all(b"\x1b\\after_oversize").unwrap();
                output.flush().unwrap();
            }
            other => panic!("unexpected PTY operation {other}"),
        }
        let mut reply_hex = String::new();
        for _ in 0..replies {
            let mut reply = Vec::new();
            // ST terminates Kitty replies; a `t`- or `c`-final CSI terminates
            // the geometry and DA1 answers this fixture also captures.
            read_until(&mut input, &mut pending, &mut reply, |reply| {
                reply.ends_with(b"\x1b\\")
                    || (reply.starts_with(b"\x1b[") && matches!(reply.last(), Some(b't' | b'c')))
            })
            .unwrap();
            if operation == "SEND_OK" {
                assert!(
                    reply.ends_with(b";OK\x1b\\"),
                    "graphics admission failed at serial {serial}: {:?}",
                    String::from_utf8_lossy(&reply)
                );
            }
            // File capture must not remove the regular exchange transcript.
            if operation == "SEND_ANY" && gate.is_some() {
                if !reply_hex.is_empty() {
                    reply_hex.push(',');
                }
                reply_hex.push_str(&hex(&reply));
            } else {
                write!(
                    output,
                    "\x1b7\x1b[1;1H\r\nKITTY_REPLY {serial} {}\r\n\x1b8",
                    hex(&reply)
                )
                .unwrap();
            }
        }
        // A reply vocabulary other than kitty's is delivered to the test as a
        // file, not as a row that a later request would overwrite.
        if operation == "SEND_ANY"
            && let Some(path) = &gate
        {
            std::fs::write(path, &reply_hex).unwrap();
        }
        // An unsupported-format query is answered in parser order without
        // creating image work. A valid pixel query can be cancelled by resize;
        // DSR instead requires a GUI consumer and cannot serve headless tests.
        output
            .write_all(b"\x1b_Ga=q,i=4294967294,f=7\x1b\\")
            .unwrap();
        output.flush().unwrap();
        let mut barrier = Vec::new();
        read_until(&mut input, &mut pending, &mut barrier, |barrier| {
            barrier.ends_with(b"\x1b\\")
        })
        .unwrap();
        assert_eq!(
            barrier,
            b"\x1b_Gi=4294967294;ENOTSUPPORTED:Unsupported operation\x1b\\"
        );
        // A non-digit terminates the serial even when older text remains
        // to its right. Never clear the row: that could delete a placement.
        // GPUI consumers observe the title after the cursor restore, not just
        // visible text that may arrive in an earlier PTY output packet.
        write!(
            output,
            "\x1b7\x1b[1;1H\r\nKITTY_DONE_{serial};\r\n\x1b8\x1b]2;KITTY_DONE_{serial};\x1b\\"
        )
        .unwrap();
        output.flush().unwrap();
        if let Some(gate) = gate {
            std::fs::write(gate.with_extension("done"), []).unwrap();
        }
    }
}
