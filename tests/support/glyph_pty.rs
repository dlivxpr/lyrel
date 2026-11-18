// Real PTY producer for the retained-glyph consumer tests.
//
// The lightweight fixture process embeds this file. It deliberately does not
// emulate a terminal: the old-client regression reads raw daemon frames, and
// the recovery consumer only needs the original registration bytes to leave
// the ring before it attaches.

pub fn glyph_consumer_fixture_sender() {
    if std::env::var_os("LYREL_PANE").is_none() {
        return;
    }
    use std::io::{Read as _, Write as _};
    let mut input = std::io::stdin().lock();
    let mut output = std::io::stdout().lock();
    output.write_all(b"\r\nGLYPH_READY\r\n").unwrap();
    output.flush().unwrap();
    loop {
        let mut line = Vec::new();
        loop {
            let mut byte = [0u8];
            if input.read(&mut byte).unwrap() == 0 {
                return;
            }
            if byte[0] == b'\n' || (cfg!(windows) && byte[0] == b'\r') {
                break;
            }
            line.push(byte[0]);
        }
        let line = String::from_utf8_lossy(&line);
        let mut fields = line.split_whitespace();
        match fields.next() {
            Some("SEND") => {
                let bytes = unhex(fields.next().expect("hex payload"));
                output.write_all(&bytes).unwrap();
                output.flush().unwrap();
            }
            Some("FLOOD") => {
                // Generated here so a ring-cap write never becomes a huge hex
                // command line; the count is exact and the producer exits after.
                let mut left: usize = fields.next().expect("flood bytes").parse().unwrap();
                let block = b"glyph replay filler\r\n";
                while left > 0 {
                    let take = block.len().min(left);
                    output.write_all(&block[..take]).unwrap();
                    left -= take;
                }
                output.flush().unwrap();
            }
            Some("EXIT") => return,
            // ConPTY may echo or split its own line endings; ignore them.
            _ => {}
        }
    }
}
