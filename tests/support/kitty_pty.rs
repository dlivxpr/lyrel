use std::io::{Read, Write};

include!("hex.rs");

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

// This is a real PTY child, not a daemon seam. The lightweight
// `lyrel-graphics-fixture` process embeds this file, so no external
// Python/Node/shell dependency is needed on every platform. Each request has a
// stream barrier; replies are read from the actual PTY input and hex-printed
// so they cannot be intercepted as graphics again.
pub fn pty_sender() {
    if std::env::var_os("LYREL_PANE").is_none() {
        return;
    }
    raw_pty();
    let mut input = std::io::stdin().lock();
    let mut output = std::io::stdout().lock();
    let mut stats = OutputStats::default();
    output.write_all(b"\r\nKITTY_READY\r\n").unwrap();
    output.flush().unwrap();
    // Bytes a stream barrier consumed ahead of the request they belong to.
    // Another writer (a pane on a different daemon, or a replayed input) can
    // land a request line inside an earlier request's barrier read; keeping
    // it here replays the line instead of panicking or losing the request.
    let mut pending: Vec<u8> = Vec::new();
    loop {
        let mut line = Vec::new();
        loop {
            if let Some(newline) = pending.iter().position(|byte| *byte == b'\n') {
                line.extend(pending.drain(..=newline));
                line.pop();
                break;
            }
            line.append(&mut pending);
            let mut byte = [0];
            if input.read(&mut byte).unwrap() == 0 {
                return;
            }
            if byte[0] == b'\n' {
                break;
            }
            line.push(byte[0]);
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
            "TITLE" => {
                // Title scenarios end on the title itself: the shared
                // `KITTY_DONE` marker would overwrite it with its own OSC 2.
                // The plain-text `TITLE_DONE` certificate is therefore
                // written further down, after this request's kitty barrier.
                stats.record(&bytes);
                output.write_all(&bytes).unwrap();
                output.flush().unwrap();
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
            while !(reply.ends_with(b"\x1b\\")
                || (reply.starts_with(b"\x1b[") && matches!(reply.last(), Some(b't' | b'c'))))
            {
                let mut byte = [0];
                input.read_exact(&mut byte).unwrap();
                reply.push(byte[0]);
            }
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
        const UNSUPPORTED: &[u8] = b"\x1b_Gi=4294967294;ENOTSUPPORTED:Unsupported operation\x1b\\";
        let mut barrier = Vec::new();
        loop {
            if let Some(at) = pending.iter().position(|byte| *byte == b'\\') {
                barrier.extend(pending.drain(..=at));
            }
            barrier.append(&mut pending);
            while !barrier.ends_with(b"\x1b\\") {
                let mut byte = [0];
                input.read_exact(&mut byte).unwrap();
                barrier.push(byte[0]);
            }
            if barrier == UNSUPPORTED {
                break;
            }
            // A TITLE request's certificate must not exist before this
            // barrier is consumed. The test sends the next request the
            // moment it sees the certificate, and a replayed input or a
            // concurrent writer can land that line inside this read.
            // Requeue complete request lines instead of asserting away
            // the whole request.
            if !barrier.contains(&b'\n') {
                panic!(
                    "unexpected kitty barrier: {:?}",
                    String::from_utf8_lossy(&barrier)
                );
            }
            pending = std::mem::take(&mut barrier);
        }
        if operation == "TITLE" {
            write!(output, "\r\nTITLE_DONE_{serial}\r\n").unwrap();
            output.flush().unwrap();
        }
        // A non-digit terminates the serial even when older text remains
        // to its right. Never clear the row: that could delete a placement.
        // GPUI consumers observe the title after the cursor restore, not just
        // visible text that may arrive in an earlier PTY output packet.
        // A TITLE request is the exception: its whole point is the title the
        // test wrote, and this marker's own OSC 2 would replace it.
        if operation != "TITLE" {
            write!(
                output,
                "\x1b7\x1b[1;1H\r\nKITTY_DONE_{serial};\r\n\x1b8\x1b]2;KITTY_DONE_{serial};\x1b\\"
            )
            .unwrap();
            output.flush().unwrap();
        }
        if let Some(gate) = gate {
            std::fs::write(gate.with_extension("done"), []).unwrap();
        }
    }
}
