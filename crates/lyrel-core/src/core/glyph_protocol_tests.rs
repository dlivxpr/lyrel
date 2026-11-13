//! Deterministic boundary coverage for the daemon-side Glyph authority.
//!
//! These are lower-level cases the real PTY consumer cannot drive cheaply:
//! malformed and oversized framing, decoded-budget rejection before point
//! storage, reply-level silence, FIFO/eviction and split APCs.

use super::*;
use rio_vt::ansi::glyph_protocol::GlyphPayload;

fn square(side: i16) -> Vec<u8> {
    polygon(&[(0, 0), (side, 0), (side, side), (0, side)])
}

fn polygon(points: &[(i16, i16)]) -> Vec<u8> {
    let count = points.len() as u16;
    let xs: Vec<i16> = points.iter().map(|(x, _)| *x).collect();
    let ys: Vec<i16> = points.iter().map(|(_, y)| *y).collect();
    let mut out = Vec::new();
    out.extend_from_slice(&1i16.to_be_bytes());
    out.extend_from_slice(&xs.iter().copied().min().unwrap_or(0).to_be_bytes());
    out.extend_from_slice(&ys.iter().copied().min().unwrap_or(0).to_be_bytes());
    out.extend_from_slice(&xs.iter().copied().max().unwrap_or(0).to_be_bytes());
    out.extend_from_slice(&ys.iter().copied().max().unwrap_or(0).to_be_bytes());
    out.extend_from_slice(&(count - 1).to_be_bytes());
    out.extend_from_slice(&0u16.to_be_bytes());
    out.extend(std::iter::repeat_n(0x01u8, points.len()));
    let mut previous = 0i16;
    for x in &xs {
        out.extend_from_slice(&(x - previous).to_be_bytes());
        previous = *x;
    }
    let mut previous = 0i16;
    for y in &ys {
        out.extend_from_slice(&(y - previous).to_be_bytes());
        previous = *y;
    }
    out
}

fn register_body(cp: u32, glyf: &[u8], reply: Option<u8>) -> Vec<u8> {
    use base64::Engine as _;
    let payload = base64::engine::general_purpose::STANDARD.encode(glyf);
    let reply = reply.map_or_else(String::new, |mode| format!(";reply={mode}"));
    format!("25a1;r;cp={cp:x};fmt=glyf{reply};upm=1000;{payload}").into_bytes()
}

fn register_apc(cp: u32, glyf: &[u8], reply: Option<u8>) -> Vec<u8> {
    let mut out = b"\x1b_".to_vec();
    out.extend(register_body(cp, glyf, reply));
    out.extend_from_slice(b"\x1b\\");
    out
}

fn command(outcome: GlyphOutcome) -> (bool, Option<Vec<u8>>) {
    match outcome {
        GlyphOutcome::Pass { forward, reply } => (forward, reply),
        GlyphOutcome::Ignore => (false, None),
        GlyphOutcome::Query { .. } => panic!("expected a register/clear outcome"),
    }
}

#[test]
fn support_stays_silent_and_is_not_forwarded() {
    let mut session = GlyphSession::new();
    let (pass, reply) = command(session.handle(b"25a1;s", false, false));
    assert!(!pass);
    assert!(reply.is_none(), "this slice does not advertise fmt=glyf");
}

#[test]
fn a_valid_default_glyph_is_accepted_and_acknowledged() {
    let mut session = GlyphSession::new();
    let (pass, reply) = command(session.handle(
        &register_body(0x100000, &square(1000), Some(1)),
        false,
        false,
    ));
    assert!(pass, "an accepted body reaches consumers");
    assert_eq!(
        reply.unwrap(),
        b"\x1b_25a1;r;cp=100000;status=0\x1b\\".to_vec()
    );
    assert!(session.contains(0x100000));
    assert_eq!(session.len(), 1);
}

#[test]
fn reply_zero_is_silent_on_success_and_failure() {
    let mut session = GlyphSession::new();
    let (pass, reply) = command(session.handle(
        &register_body(0x100000, &square(1000), Some(0)),
        false,
        false,
    ));
    assert!(pass);
    assert!(reply.is_none(), "reply=0 suppresses success");

    let mut session = GlyphSession::new();
    let body = b"25a1;r;cp=41;fmt=glyf;reply=0;AAAA".to_vec();
    let (pass, reply) = command(session.handle(&body, false, false));
    assert!(!pass);
    assert!(reply.is_none(), "reply=0 suppresses failure");
}

#[test]
fn reply_two_reports_failures_only() {
    let mut session = GlyphSession::new();
    let (_, reply) = command(session.handle(
        &register_body(0x100000, &square(1000), Some(2)),
        false,
        false,
    ));
    assert!(reply.is_none(), "reply=2 suppresses success");

    let mut session = GlyphSession::new();
    let (_, reply) =
        command(session.handle(&register_body(0x61, &square(1000), Some(2)), false, false));
    let reply = reply.expect("reply=2 still reports failures");
    assert!(
        String::from_utf8_lossy(&reply).contains("reason=out_of_namespace"),
        "{reply:?}"
    );
}

#[test]
fn a_composite_outline_is_rejected_with_its_reason() {
    let mut session = GlyphSession::new();
    let mut composite = square(1000);
    composite[..2].copy_from_slice(&(-1i16).to_be_bytes());
    let (pass, reply) =
        command(session.handle(&register_body(0x100000, &composite, Some(1)), false, false));
    assert!(!pass, "a rejected body is stripped");
    let reply = reply.unwrap();
    assert!(
        reply.ends_with(b";reason=composite_unsupported\x1b\\"),
        "{reply:?}"
    );
    assert!(!session.contains(0x100000));
}

#[test]
fn a_hinted_outline_is_rejected() {
    let mut session = GlyphSession::new();
    let mut hinted = square(1000);
    // instructionLength follows the four header int16s and endPtsOfContours.
    hinted[12..14].copy_from_slice(&1u16.to_be_bytes());
    hinted.push(0);
    let (_, reply) =
        command(session.handle(&register_body(0x100000, &hinted, Some(1)), false, false));
    let reply = reply.unwrap();
    assert!(
        reply.ends_with(b";reason=hinting_unsupported\x1b\\"),
        "{reply:?}"
    );
}

#[test]
fn the_decoded_point_budget_is_rejected_before_point_storage() {
    let mut session = GlyphSession::new();
    let mut dense = Vec::new();
    dense.extend_from_slice(&1i16.to_be_bytes()); // one contour
    dense.extend_from_slice(&0i16.to_be_bytes());
    dense.extend_from_slice(&0i16.to_be_bytes());
    dense.extend_from_slice(&100i16.to_be_bytes());
    dense.extend_from_slice(&100i16.to_be_bytes());
    dense.extend_from_slice(&5461u16.to_be_bytes()); // 5462 points
    dense.extend_from_slice(&0u16.to_be_bytes()); // no hinting
    let (_, reply) =
        command(session.handle(&register_body(0x100000, &dense, Some(1)), false, false));
    let reply = reply.unwrap();
    assert!(
        reply.ends_with(b";reason=outline_too_large\x1b\\"),
        "{reply:?}"
    );
}

#[test]
fn an_oversized_wire_body_is_rejected_from_its_control_prefix() {
    let mut session = GlyphSession::new();
    let mut body = b"25a1;r;cp=100000;fmt=glyf;reply=1;upm=1000;".to_vec();
    body.extend(std::iter::repeat_n(b'A', 4096));
    let (pass, reply) = command(session.handle(&body, false, true));
    assert!(!pass);
    let reply = reply.unwrap();
    assert!(
        reply.ends_with(b";reason=payload_too_large\x1b\\"),
        "{reply:?}"
    );
}

#[test]
fn an_aborted_body_is_stripped_without_a_reply() {
    let mut session = GlyphSession::new();
    let (pass, reply) = command(session.handle(b"25a1;r;cp=100000;", true, false));
    assert!(!pass);
    assert!(reply.is_none());
}

#[test]
fn clear_reports_success_even_for_an_empty_slot() {
    let mut session = GlyphSession::new();
    let (pass, reply) = command(session.handle(b"25a1;c;cp=100000", false, false));
    assert!(pass);
    assert_eq!(
        reply.unwrap(),
        b"\x1b_25a1;c;cp=100000;status=0\x1b\\".to_vec()
    );
}

#[test]
fn clear_all_removes_every_registration() {
    let mut session = GlyphSession::new();
    session.handle(
        &register_body(0x100000, &square(1000), Some(0)),
        false,
        false,
    );
    session.handle(
        &register_body(0x100001, &square(1000), Some(0)),
        false,
        false,
    );
    let (pass, reply) = command(session.handle(b"25a1;c", false, false));
    assert!(pass);
    assert_eq!(reply.unwrap(), b"\x1b_25a1;c;status=0\x1b\\".to_vec());
    assert_eq!(session.len(), 0);
}

#[test]
fn a_clear_outside_the_namespace_is_refused() {
    let mut session = GlyphSession::new();
    let (pass, reply) = command(session.handle(b"25a1;c;cp=41", false, false));
    assert!(!pass);
    assert_eq!(
        reply.unwrap(),
        b"\x1b_25a1;c;status=1;reason=out_of_namespace\x1b\\".to_vec()
    );
}

#[test]
fn the_glossary_keeps_1024_slots_and_replaces_in_place() {
    let mut session = GlyphSession::new();
    for offset in 0..1024u32 {
        session.handle(
            &register_body(0x100000 + offset, &square(1000), Some(0)),
            false,
            false,
        );
    }
    assert_eq!(session.len(), 1024);
    session.handle(
        &register_body(0x100000, &square(500), Some(0)),
        false,
        false,
    );
    assert_eq!(session.len(), 1024, "replacement does not consume a slot");
    session.handle(
        &register_body(0x10fffd, &square(1000), Some(0)),
        false,
        false,
    );
    assert_eq!(session.len(), 1024);
    assert!(!session.contains(0x100000), "oldest entry was evicted");
    assert!(session.contains(0x10fffd));
}

#[test]
fn a_query_reports_the_registry_bit_at_its_stream_position() {
    let mut session = GlyphSession::new();
    session.handle(
        &register_body(0x100000, &square(1000), Some(0)),
        false,
        false,
    );
    match session.handle(b"25a1;q;cp=100000", false, false) {
        GlyphOutcome::Query { cp, glossary } => {
            assert_eq!(cp, 0x100000);
            assert!(glossary);
        }
        _ => panic!("a query is frontend-bound"),
    }
    match session.handle(b"25a1;q;cp=100001", false, false) {
        GlyphOutcome::Query { glossary, .. } => assert!(!glossary),
        _ => panic!("a query is frontend-bound"),
    }
}

#[test]
fn the_sniffer_lifts_a_split_glyph_apc_and_passes_other_apcs() {
    let mut sniffer = GlyphSniffer::new();
    let body = register_apc(0x100000, &square(1000), Some(1));
    let (head, tail) = body.split_at(8);
    assert!(matches!(
        &sniffer.sniff(head),
        GlyphSniffed::Segments(parts) if parts.is_empty()
    ));
    let GlyphSniffed::Segments(parts) = sniffer.sniff(tail) else {
        panic!("the completed body is lifted");
    };
    assert!(matches!(
        parts.as_slice(),
        [GlyphSegment::Command { aborted: false, .. }]
    ));

    let mut sniffer = GlyphSniffer::new();
    let GlyphSniffed::Plain(bytes) = sniffer.sniff(b"\x1b_Ga=q,i=1\x1b\\after") else {
        panic!("a kitty APC is not ours and stays borrowed output");
    };
    assert_eq!(bytes, b"\x1b_Ga=q,i=1\x1b\\after");
}

#[test]
fn an_aborted_apc_is_dropped_and_later_text_still_parses() {
    let mut sniffer = GlyphSniffer::new();
    let GlyphSniffed::Segments(parts) = sniffer.sniff(b"\x1b_25a1;r;cp=100000;abc\x18after") else {
        panic!("an aborted body is framed");
    };
    let mut commands = 0;
    let mut text = Vec::new();
    for part in parts {
        match part {
            GlyphSegment::Command { aborted, .. } => {
                assert!(aborted);
                commands += 1;
            }
            GlyphSegment::Output(run) => text.extend_from_slice(&run),
        }
    }
    assert_eq!(commands, 1);
    assert_eq!(text, b"after".to_vec());
}

#[test]
fn an_oversized_apc_reports_only_its_control_prefix() {
    let mut sniffer = GlyphSniffer::new();
    let mut bytes = b"\x1b_25a1;r;cp=100000;fmt=glyf;reply=1;upm=1000;".to_vec();
    bytes.extend(std::iter::repeat_n(b'A', MAX_APC_BODY + 16));
    bytes.extend_from_slice(b"\x1b\\tail");
    let GlyphSniffed::Segments(parts) = sniffer.sniff(&bytes) else {
        panic!("oversized framing still completes");
    };
    let mut saw_exhausted = false;
    let mut text = Vec::new();
    for part in parts {
        match part {
            GlyphSegment::Command {
                body,
                exhausted: true,
                ..
            } => {
                assert!(body.starts_with(b"25a1;r;cp=100000;"));
                saw_exhausted = true;
            }
            GlyphSegment::Command {
                exhausted: false, ..
            } => {
                panic!("an oversized body must not be reported complete")
            }
            GlyphSegment::Output(run) => text.extend_from_slice(&run),
        }
    }
    assert!(saw_exhausted);
    assert_eq!(text, b"tail".to_vec());
}

#[test]
fn a_partial_prefix_across_feeds_is_not_emitted_as_text() {
    let mut sniffer = GlyphSniffer::new();
    assert!(matches!(
        &sniffer.sniff(b"\x1b_2"),
        GlyphSniffed::Segments(parts) if parts.is_empty()
    ));
    let GlyphSniffed::Segments(parts) = sniffer.sniff(b"5a1;q;cp=41\x1b\\") else {
        panic!("the completed prefix is framed");
    };
    assert!(matches!(
        parts.as_slice(),
        [GlyphSegment::Command { aborted: false, .. }]
    ));
}

#[test]
fn an_unknown_verb_is_stripped_silently() {
    let mut session = GlyphSession::new();
    let (pass, reply) = command(session.handle(b"25a1;z;cp=100000", false, false));
    assert!(!pass);
    assert!(reply.is_none());
}

#[test]
fn a_colour_payload_is_rejected_by_this_slice() {
    let mut session = GlyphSession::new();
    // `fmt=colrv0` with a structurally valid empty-ish container is parsed by
    // the released parser; this build advertises only glyf and must refuse it.
    let body = b"25a1;r;cp=100000;fmt=colrv0;reply=1;AAAA".to_vec();
    let (pass, reply) = command(session.handle(&body, false, false));
    assert!(!pass);
    assert!(reply.is_some());
}

#[test]
fn a_register_body_survives_the_official_parser_unchanged() {
    // Guard against the authority drifting from the released wire parser.
    let body = register_body(0x100000, &square(1000), Some(1));
    match rio_vt::ansi::glyph_protocol::parse(&body) {
        Ok(rio_vt::ansi::glyph_protocol::GlyphCommand::Register { cp, payload, .. }) => {
            assert_eq!(cp, 0x100000);
            assert!(matches!(payload, GlyphPayload::Glyf { .. }));
        }
        other => panic!("released parser disagreed: {other:?}"),
    }
}
