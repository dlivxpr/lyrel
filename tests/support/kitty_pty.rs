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

    fn raw_pty() {
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
                ENABLE_VIRTUAL_TERMINAL_PROCESSING, GetStdHandle, STD_INPUT_HANDLE,
                STD_OUTPUT_HANDLE, SetConsoleMode,
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
        output.write_all(b"\r\nKITTY_READY\r\n").unwrap();
        output.flush().unwrap();
        loop {
            let mut line = Vec::new();
            loop {
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
            match operation {
                "EXIT" => return,
                "SEND" => {
                    for part in bytes.chunks(if fragment == 0 {
                        bytes.len().max(1)
                    } else {
                        fragment
                    }) {
                        output.write_all(part).unwrap();
                        output.flush().unwrap();
                    }
                }
                "FLOOD" => {
                    output.write_all(b"\x1b_Ga=T,i=720,s=1,v=1;").unwrap();
                    // 49 MiB exceeds the 48 MiB APC cap without allocating a
                    // 98 MiB hex request or relying on an external sender.
                    let block = [b'A'; 8192];
                    for _ in 0..(49 * 1024 * 1024 / block.len()) {
                        output.write_all(&block).unwrap();
                    }
                    output.write_all(b"\x1b\\after_oversize").unwrap();
                    output.flush().unwrap();
                }
                other => panic!("unexpected PTY operation {other}"),
            }
            for _ in 0..replies {
                let mut reply = Vec::new();
                while !(reply.ends_with(b"\x1b\\")
                    || (reply.starts_with(b"\x1b[") && reply.ends_with(b"t")))
                {
                    let mut byte = [0];
                    input.read_exact(&mut byte).unwrap();
                    reply.push(byte[0]);
                }
                write!(
                    output,
                    "\x1b7\x1b[1;1H\r\nKITTY_REPLY {}\r\n\x1b8",
                    hex(&reply)
                )
                .unwrap();
            }
            // Force a parser boundary and wait for its application-visible
            // answer before the text marker. Layout updates from preceding
            // ordinary text must already be queued ahead of KITTY_DONE.
            output
                .write_all(b"\x1b_Ga=q,i=4294967294,f=32,s=1,v=1;/wAA/w==\x1b\\")
                .unwrap();
            output.flush().unwrap();
            let mut barrier = Vec::new();
            while !barrier.ends_with(b"\x1b\\") {
                let mut byte = [0];
                input.read_exact(&mut byte).unwrap();
                barrier.push(byte[0]);
            }
            assert_eq!(barrier, b"\x1b_Gi=4294967294;OK\x1b\\");
            write!(output, "\x1b7\x1b[1;1H\r\nKITTY_DONE_{serial}\r\n\x1b8").unwrap();
            output.flush().unwrap();
        }
    }
