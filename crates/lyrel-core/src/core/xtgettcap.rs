//! XTGETTCAP discovery policy; the upstream parser owns request decoding.

/// A non-owning terminal mirror consumes text, never XTGETTCAP. This also
/// bounds legacy snapshots saved before the daemon extracted discovery.
#[derive(Default)]
pub struct MirrorProcessor {
    inner: rio_vt::performer::handler::Processor,
    discovery: Sniffer,
}

impl MirrorProcessor {
    pub fn advance<H: rio_vt::performer::handler::Handler>(
        &mut self,
        handler: &mut H,
        bytes: &[u8],
    ) {
        match self.discovery.sniff(bytes) {
            Sniffed::Plain(bytes) => self.inner.advance(handler, bytes),
            Sniffed::Segments(segments) => {
                for segment in segments {
                    if let Segment::Output(bytes) = segment {
                        self.inner.advance(handler, &bytes);
                    }
                }
            }
        }
    }
}

/// Rio has already decoded the names and generated one DCS segment per name.
/// Never inherit its values, product identity or capability table: decide by
/// decoded name and generate only values whose full product path is verified.
pub(crate) fn response(generated: &str) -> Vec<u8> {
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

use super::kitty_graphics::ApcTokenizer;

pub(crate) enum Segment {
    Output(Vec<u8>),
    Query(Vec<u8>),
}

pub(crate) enum Sniffed<'a> {
    Plain(&'a [u8]),
    Segments(Vec<Segment>),
}

/// Strip complete, cancelled and oversized discovery requests before *any*
/// daemon/desktop/CLI Processor. Only bounded live queries reach the owner.
pub(crate) struct Sniffer(ApcTokenizer);

impl Default for Sniffer {
    fn default() -> Self {
        Self(ApcTokenizer::xtgettcap())
    }
}

impl Sniffer {
    pub(crate) fn sniff<'a>(&mut self, bytes: &'a [u8]) -> Sniffed<'a> {
        if self.0.at_boundary()
            && bytes.last() != Some(&0x1b)
            && memchr::memmem::find(bytes, b"\x1bP").is_none()
        {
            return Sniffed::Plain(bytes);
        }
        let segments = std::cell::RefCell::new(Vec::new());
        self.0.tokenize(
            bytes,
            |run| {
                let mut segments = segments.borrow_mut();
                if let Some(Segment::Output(bytes)) = segments.last_mut() {
                    bytes.extend_from_slice(run);
                } else if !run.is_empty() {
                    segments.push(Segment::Output(run.to_vec()));
                }
            },
            |body, aborted, _| {
                let mut segments = segments.borrow_mut();
                // ESC P also interrupts a preceding unfinished control string.
                // Preserve that cancellation without forwarding the query body.
                segments.push(Segment::Output(vec![0x18]));
                if !aborted {
                    let mut query = Vec::with_capacity(body.len() + 4);
                    query.extend_from_slice(b"\x1bP");
                    query.extend_from_slice(body);
                    query.extend_from_slice(b"\x1b\\");
                    segments.push(Segment::Query(query));
                }
            },
        );
        Sniffed::Segments(segments.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use crate::core::kitty_graphics::{GraphicsEffect, GraphicsState};
    use crate::daemon::protocol::WinSize;

    fn state() -> GraphicsState {
        GraphicsState::new(WinSize {
            cols: 91,
            rows: 37,
            cell_w: 9,
            cell_h: 18,
        })
    }

    fn replies(state: &mut GraphicsState, bytes: &[u8]) -> Vec<u8> {
        state
            .output(bytes)
            .into_iter()
            .flat_map(|effect| match effect {
                GraphicsEffect::Reply(bytes) => bytes,
                _ => Vec::new(),
            })
            .collect()
    }

    #[test]
    fn xtgettcap_discovers_only_verified_capabilities_in_request_order() {
        let mut state = state();
        assert_eq!(replies(&mut state, b"\x1bP+q436f;544e;524742;736978656c;697465726d32;636f;6c69;4d73;53756c78;756e6b6e6f776e\x1b\\"),
            b"\x1bP1+r436F=323536\x1b\\\x1bP0+r544E\x1b\\\x1bP1+r524742=382F382F38\x1b\\\x1bP0+r736978656C\x1b\\\x1bP0+r697465726D32\x1b\\\x1bP0+r636F\x1b\\\x1bP0+r6C69\x1b\\\x1bP0+r4D73\x1b\\\x1bP0+r53756C78\x1b\\\x1bP0+r756E6B6E6F776E\x1b\\");
    }

    fn consume(chunks: impl IntoIterator<Item = Vec<u8>>) -> (Vec<u8>, Vec<u8>) {
        let mut sniffer = super::Sniffer::default();
        let mut term = state();
        let mut plain = Vec::new();
        let mut received = Vec::new();
        for chunk in chunks {
            let segments = match sniffer.sniff(&chunk) {
                super::Sniffed::Plain(bytes) => vec![super::Segment::Output(bytes.to_vec())],
                super::Sniffed::Segments(segments) => segments,
            };
            for segment in segments {
                match segment {
                    super::Segment::Output(bytes) => {
                        received.extend(replies(&mut term, &bytes));
                        plain.extend(bytes);
                    }
                    super::Segment::Query(bytes) => received.extend(replies(&mut term, &bytes)),
                }
            }
        }
        (plain, received)
    }

    #[test]
    fn xtgettcap_fragmentation_cancellation_and_next_text_share_one_contract() {
        let stream = b"before\x1bP+q436f\x18after_CAN\x1bP+q524742\x1aafter_SUB\x1bP+q436f\x1b[0mafter_ESC\x1bP0+q636f6c6f7273;544e\x1b\\after_query";
        for split in 0..=stream.len() {
            let (plain, received) = consume([stream[..split].to_vec(), stream[split..].to_vec()]);
            assert_eq!(
                received, b"\x1bP1+r636F6C6F7273=323536\x1b\\\x1bP0+r544E\x1b\\",
                "split {split}"
            );
            assert_eq!(
                plain, b"before\x18after_CAN\x18after_SUB\x18\x1b[0mafter_ESC\x18after_query",
                "split {split}"
            );
        }
        let (_, received) = consume(stream.iter().map(|byte| vec![*byte]));
        assert_eq!(
            received,
            b"\x1bP1+r636F6C6F7273=323536\x1b\\\x1bP0+r544E\x1b\\"
        );
        let (plain, received) = consume(b"\x1bP\n+q436f\x1b\\".iter().map(|byte| vec![*byte]));
        assert_eq!(plain, b"\n\x18");
        assert_eq!(received, b"\x1bP1+r436F=323536\x1b\\");
        let (_, received) = consume(b"\x1bP+q436f\x1b\x1b\\".iter().map(|byte| vec![*byte]));
        assert!(
            received.is_empty(),
            "a second ESC cancels the first terminator"
        );
    }

    #[test]
    fn xtgettcap_limit_applies_before_every_consumer_and_recovers() {
        let stream = format!(
            "\x1bP+q{}\x1b\\after_oversize\x1bP+q436f\x1b\\",
            "436f;".repeat(2000)
        );
        for size in [1, 113, 8192, 65536] {
            let (plain, received) = consume(stream.as_bytes().chunks(size).map(<[u8]>::to_vec));
            assert_eq!(plain, b"\x18after_oversize\x18", "chunk {size}");
            assert_eq!(received, b"\x1bP1+r436F=323536\x1b\\");
        }
        let at_limit = format!("\x1bP+q436f{}\x1b\\", ";".repeat(8186));
        assert_eq!(
            consume([at_limit.into_bytes()]).1,
            b"\x1bP1+r436F=323536\x1b\\"
        );
        let past_limit = format!("\x1bP+q436f{}\x1b\\ordinary", ";".repeat(8187));
        let (plain, received) = consume([past_limit.into_bytes()]);
        assert_eq!(plain, b"\x18ordinary");
        assert!(received.is_empty());
        let header_overflow = format!("\x1bP{}+q436f\x1b\\ordinary", "0".repeat(128));
        let (plain, received) = consume([header_overflow.into_bytes()]);
        assert_eq!(plain, b"\x18ordinary");
        assert!(received.is_empty());
        let non_discovery = b"\x1bP=1s\x1b\\ordinary\x1bP1;2qabc\x1b\\";
        let (plain, _) = consume(non_discovery.iter().map(|byte| vec![*byte]));
        assert_eq!(
            plain, non_discovery,
            "other DCS protocols are not discovery"
        );
    }

    #[test]
    fn xtgettcap_negotiation_uses_the_actual_endpoint_not_this_build() {
        use crate::daemon::protocol::{ClientMsg, DaemonMsg, DaemonVersion, negotiate_pane};
        struct Peer {
            inbound: std::io::Cursor<Vec<u8>>,
            outbound: Vec<u8>,
        }
        impl std::io::Read for Peer {
            fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
                std::io::Read::read(&mut self.inbound, bytes)
            }
        }
        impl std::io::Write for Peer {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.outbound.extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        for supported in [false, true] {
            let mut version = DaemonVersion::current();
            version.build = "actual-remote-build".into();
            version
                .features
                .retain(|feature| feature != "xtgettcap-owner");
            if supported {
                version.features.push("xtgettcap-owner".into());
            }
            let mut inbound = Vec::new();
            DaemonMsg::Version(version.clone())
                .encode(&mut inbound)
                .unwrap();
            let mut peer = Peer {
                inbound: std::io::Cursor::new(inbound),
                outbound: Vec::new(),
            };
            let result = negotiate_pane(&mut peer);
            if supported {
                assert_eq!(result.unwrap(), version);
            } else {
                let error = result.unwrap_err().to_string();
                assert!(
                    error.contains("actual-remote-build")
                        && error.contains("xtgettcap-owner")
                        && error.contains("update lyrel"),
                    "{error}"
                );
                let mut written = std::io::Cursor::new(peer.outbound);
                assert!(matches!(
                    ClientMsg::read(&mut written).unwrap(),
                    ClientMsg::Negotiate { .. }
                ));
                assert_eq!(
                    written.position() as usize,
                    written.get_ref().len(),
                    "no session request or capacity follows refusal"
                );
            }
        }
    }
}
