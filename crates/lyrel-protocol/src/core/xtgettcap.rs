//! XTGETTCAP discovery policy values; the sniffer engine that produces them
//! shares the daemon's graphics APC framing and therefore stays in
//! `lyrel-core` (`lyrel_core::core::xtgettcap::{Sniffer, MirrorProcessor}`).

/// Rio has already decoded the names and generated one DCS segment per name.
/// Never inherit its values, product identity or capability table: decide by
/// decoded name and generate only values whose full product path is verified.
pub fn response(generated: &str) -> Vec<u8> {
    let mut reply = Vec::with_capacity(generated.len());
    for segment in generated.split_terminator("\x1b\\") {
        let Some(fields) = segment
            .strip_prefix("\x1bP1+r")
            .or_else(|| segment.strip_prefix("\x1bP0+r"))
        else {
            continue;
        };
        let name = fields.split('=').next().unwrap_or_default();
        let mut decoded = [0; 6];
        let bytes = name.as_bytes();
        let valid = bytes.len().is_multiple_of(2)
            && bytes.len() <= decoded.len() * 2
            && bytes
                .chunks_exact(2)
                .zip(decoded.iter_mut())
                .all(|(pair, slot)| {
                    let digit = |byte: u8| match byte {
                        b'0'..=b'9' => Some(byte - b'0'),
                        b'a'..=b'f' => Some(byte - b'a' + 10),
                        b'A'..=b'F' => Some(byte - b'A' + 10),
                        _ => None,
                    };
                    match (digit(pair[0]), digit(pair[1])) {
                        (Some(hi), Some(lo)) => {
                            *slot = hi * 16 + lo;
                            true
                        }
                        _ => false,
                    }
                });
        let value: Option<&[u8]> = if valid {
            match &decoded[..bytes.len() / 2] {
                b"Co" | b"colors" => Some(b"323536"),
                b"RGB" => Some(b"382F382F38"),
                _ => None,
            }
        } else {
            None
        };
        reply.extend_from_slice(if value.is_some() {
            b"\x1bP1+r"
        } else {
            b"\x1bP0+r"
        });
        reply.extend_from_slice(name.as_bytes());
        if let Some(value) = value {
            reply.push(b'=');
            reply.extend_from_slice(value);
        }
        reply.extend_from_slice(b"\x1b\\");
    }
    reply
}

/// One framed piece of the sniffed stream: plain terminal output to forward,
/// or a complete discovery request the XTGETTCAP owner answers.
pub enum Segment {
    Output(Vec<u8>),
    Query(Vec<u8>),
}

pub enum Sniffed<'a> {
    Plain(&'a [u8]),
    Segments(Vec<Segment>),
}
