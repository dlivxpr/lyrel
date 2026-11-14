// Shared real-PTY application for daemon and rooted GUI theme-query consumers.
// Input is a serial, expected CSI reply count, then hex-encoded output bytes.
pub fn theme_query_sender() {
    if std::env::var_os("LYREL_PANE").is_none() {
        return;
    }
    raw_pty();
    let mut input = std::io::stdin().lock();
    let mut output = std::io::stdout().lock();
    output.write_all(b"\r\nTHEME_READY\r\n").unwrap();
    output.flush().unwrap();
    loop {
        let mut line = Vec::new();
        loop {
            let mut byte = [0];
            if input.read(&mut byte).unwrap() == 0 {
                return;
            }
            if byte[0] == b'\n' || (cfg!(windows) && byte[0] == b'\r') {
                break;
            }
            line.push(byte[0]);
        }
        let line = String::from_utf8(line).unwrap();
        let mut fields = line.split_whitespace();
        let serial = fields.next().unwrap();
        let replies: usize = fields.next().unwrap().parse().unwrap();
        let bytes = unhex(fields.next().unwrap());
        // An optional fourth field repeats the same payload in one PTY write
        // burst, so a ring-cap test does not pay a JSON `SendInput` round trip
        // per kilobyte. Existing three-field callers keep one copy.
        let repeat: usize = fields.next().map_or(1, |n| n.parse().unwrap());
        for _ in 0..repeat {
            output.write_all(&bytes).unwrap();
        }
        output.flush().unwrap();
        if replies == usize::MAX {
            // Synchronize the PTY writer without inserting a marker or APC
            // into the incomplete CSI that the next request must finish.
            let dir = std::env::var_os("LYREL_CONFIG_DIR").unwrap();
            std::fs::write(
                std::path::Path::new(&dir).join(format!("theme-fragment-{serial}")),
                [],
            )
            .unwrap();
            continue;
        }
        // A reply is whole when its own framing says so: CSI/DCS/APC end at
        // their final byte or ST, OSC at BEL or ST. The colour probes below
        // answer `ESC ] … ; rgb:hh/hh/hh`, whose hex digits include `c` — a
        // one-byte stop would truncate every indexed-colour report.
        let reply_complete = |reply: &[u8]| {
            if reply.starts_with(b"\x1b[") {
                reply.len() > 2 && matches!(reply.last(), Some(0x40..=0x7e))
            } else {
                reply.ends_with(b"\x1b\\")
                    || (reply.starts_with(b"\x1b]") && reply.ends_with(b"\x07"))
            }
        };
        let mut received = Vec::new();
        for _ in 0..replies {
            let start = received.len();
            loop {
                let mut byte = [0];
                input.read_exact(&mut byte).unwrap();
                received.push(byte[0]);
                if reply_complete(&received[start..]) {
                    break;
                }
            }
        }
        // The daemon answers this in stream order. Any duplicate theme/DA1
        // before the barrier is retained, so exact-byte assertions catch it.
        output
            .write_all(b"\x1b_Ga=q,i=4294967294,f=7\x1b\\")
            .unwrap();
        output.flush().unwrap();
        let mut barrier = Vec::new();
        while !barrier.ends_with(b"\x1b\\") {
            let mut byte = [0];
            input.read_exact(&mut byte).unwrap();
            barrier.push(byte[0]);
        }
        let reply = b"\x1b_Gi=4294967294;ENOTSUPPORTED:Unsupported operation\x1b\\";
        assert!(barrier.ends_with(reply), "unexpected barrier {barrier:?}");
        received.extend_from_slice(&barrier[..barrier.len() - reply.len()]);
        write!(
            output,
            "\r\nTHEME_RESULT_{serial} {} END\r\n",
            hex(&received)
        )
        .unwrap();
        output.flush().unwrap();
    }
}
