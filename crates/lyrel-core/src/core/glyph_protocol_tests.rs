//! Deterministic boundary coverage for the daemon-side Glyph authority.
//!
//! These are lower-level cases the real PTY consumer cannot drive cheaply:
//! malformed and oversized framing, decoded-budget rejection before point
//! storage, reply-level silence, FIFO/eviction and split APCs. The tail of
//! the file pins the aggregate retained budgets (4 MiB wire / 8 MiB decoded
//! across the whole glossary) and the `GlyphFrame` projection boundaries
//! the retain and handoff paths rely on.

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

// ---- Aggregate retained budgets ---------------------------------------------
//
// `GlyphSession::admit` may refuse a registration that is valid on its own
// when the projected glossary totals would cross the normative aggregate
// budgets below. A refusal must never look like an eviction: the definitions
// the producer already saw accepted stay registered and queryable.

/// The normative whole-glossary wire budget: 4 MiB of retained payload bytes.
const AGGREGATE_WIRE_BUDGET: usize = 4 * 1024 * 1024;
/// The normative whole-glossary decoded budget: 8 MiB at 12 bytes per point.
const AGGREGATE_DECODED_BUDGET: usize = 8 * 1024 * 1024;

/// A `points`-point single-contour simple glyph with no instructions. All
/// flags share repeat runs and no coordinate bytes, so the wire size stays
/// ~2 bytes per 255 points while the released decoder charges the full
/// point count — the same dense record rio-graphics uses for its own
/// decoded-budget test.
fn dense_points(points: usize) -> Vec<u8> {
    assert!(points > 0, "a point list needs at least one point");
    let mut out = Vec::with_capacity(14 + 2 * points.div_ceil(255));
    out.extend_from_slice(&1i16.to_be_bytes()); // numberOfContours
    out.extend_from_slice(&[0u8; 8]); // bounding box
    out.extend_from_slice(&((points - 1) as u16).to_be_bytes()); // endPtsOfContours
    out.extend_from_slice(&0u16.to_be_bytes()); // instructionLength
    // on-curve | repeat | x-same | y-same: every delta reads as zero.
    const FLAGS: u8 = 0x01 | 0x08 | 0x10 | 0x20;
    let mut left = points;
    while left > 0 {
        // Runs of 255, not 256: the repeat byte tracks count - 1.
        let run = left.min(255);
        out.push(FLAGS);
        out.push((run - 1) as u8);
        left -= run;
    }
    out
}

/// A structurally valid simple record extended with trailing slack. The
/// released decoder consumes the declared points and ignores the rest of the
/// record, so padding is the only way to approach the 64 KiB per-registration
/// wire limit with a small decoded cost.
fn padded(base: Vec<u8>, size: usize) -> Vec<u8> {
    assert!(base.len() <= size);
    let mut out = base;
    out.resize(size, 0);
    out
}

fn accepted(outcome: GlyphOutcome) {
    let (forward, _) = command(outcome);
    assert!(forward, "a spec-valid registration must be accepted");
}

fn refused_reason(outcome: GlyphOutcome, reason: &str) {
    let (forward, reply) = command(outcome);
    assert!(!forward, "an over-budget body is stripped");
    let reply = reply.expect("reply=1 surfaces the refusal");
    assert!(
        reply.ends_with(format!(";reason={reason}\x1b\\").as_bytes()),
        "{reply:?}"
    );
}

fn glossary_bit(session: &mut GlyphSession, cp: u32) -> bool {
    let body = format!("25a1;q;cp={cp:x}").into_bytes();
    match session.handle(&body, false, false) {
        GlyphOutcome::Query { glossary, .. } => glossary,
        _ => panic!("a query is frontend-bound"),
    }
}

#[test]
fn the_aggregate_decoded_budget_refuses_without_touching_prior_registrations() {
    let mut session = GlyphSession::new();
    // Each registration is individually valid — its wire cost is trivial — so
    // only the aggregate decoded budget can refuse the next one.
    let dense = dense_points(glyf_decode::MAX_DECODED_POINTS);
    let entries = AGGREGATE_DECODED_BUDGET / (glyf_decode::MAX_DECODED_POINTS * 12);
    for offset in 0..entries as u32 {
        accepted(session.handle(
            &register_body(0x100000 + offset, &dense, Some(0)),
            false,
            false,
        ));
    }
    let generation = session.frame().generation();
    refused_reason(
        session.handle(
            &register_body(0x100000 + entries as u32, &dense, Some(1)),
            false,
            false,
        ),
        "outline_too_large",
    );
    assert_eq!(session.len(), entries);
    assert_eq!(
        session.frame().generation(),
        generation,
        "a refused mutation must not bump the snapshot generation"
    );
    // The remainder fits a smaller outline exactly: budget accounting is by
    // decoded points, not by entry count.
    let leftover = AGGREGATE_DECODED_BUDGET - entries * glyf_decode::MAX_DECODED_POINTS * 12;
    let fits = leftover / 12;
    accepted(session.handle(
        &register_body(0x100000 + entries as u32, &dense_points(fits), Some(0)),
        false,
        false,
    ));
    refused_reason(
        session.handle(
            &register_body(
                0x100000 + entries as u32 + 1,
                &dense_points(fits + 1),
                Some(1),
            ),
            false,
            false,
        ),
        "outline_too_large",
    );
    assert!(glossary_bit(&mut session, 0x100000));
    assert!(glossary_bit(&mut session, 0x100000 + entries as u32 - 1));
    assert!(glossary_bit(&mut session, 0x100000 + entries as u32));
    assert!(!glossary_bit(&mut session, 0x100000 + entries as u32 + 1));
}

#[test]
fn the_aggregate_wire_budget_refuses_without_touching_prior_registrations() {
    let mut session = GlyphSession::new();
    // A 64 KiB retained payload with a trivial outline: each registration is
    // spec-valid, the decoded usage stays trivial, so only the aggregate wire
    // budget can refuse the next entry.
    let fat = padded(square(1000), MAX_GLYF_BYTES);
    let entries = AGGREGATE_WIRE_BUDGET / fat.len();
    for offset in 0..entries as u32 {
        accepted(session.handle(
            &register_body(0x100000 + offset, &fat, Some(0)),
            false,
            false,
        ));
    }
    refused_reason(
        session.handle(
            &register_body(0x100000 + entries as u32, &fat, Some(1)),
            false,
            false,
        ),
        "outline_too_large",
    );
    assert_eq!(session.len(), entries);
    assert!(glossary_bit(&mut session, 0x100000));
    assert!(glossary_bit(&mut session, 0x100000 + entries as u32 - 1));
    assert!(!glossary_bit(&mut session, 0x100000 + entries as u32));
}

#[test]
fn an_aggregate_refusal_at_full_glossary_is_not_a_silent_eviction() {
    let mut session = GlyphSession::new();
    // Fill every FIFO slot with tiny outlines; nothing here approaches a
    // budget, so all of them must be accepted.
    for offset in 0..GLOSSARY_CAPACITY as u32 {
        accepted(session.handle(
            &register_body(0x100000 + offset, &square(500), Some(0)),
            false,
            false,
        ));
    }
    // Replacements keep their FIFO slot. Growing the newest entries one at a
    // time stops exactly when the next one would cross the wire budget.
    let small = square(500);
    let fat = padded(square(1000), MAX_GLYF_BYTES);
    let expected_grown =
        (AGGREGATE_WIRE_BUDGET - GLOSSARY_CAPACITY * small.len()) / (fat.len() - small.len());
    let mut grown = 0usize;
    loop {
        let (forward, _) = command(session.handle(
            &register_body(
                0x100000 + GLOSSARY_CAPACITY as u32 - 1 - grown as u32,
                &fat,
                Some(0),
            ),
            false,
            false,
        ));
        if !forward {
            break;
        }
        grown += 1;
    }
    assert_eq!(grown, expected_grown, "replacement credit accounting");
    assert_eq!(session.len(), GLOSSARY_CAPACITY);
    // The refused replacement kept its small definition: its codepoint still
    // reports glossary coverage, and nothing older disappeared.
    let refused_cp = 0x100000 + GLOSSARY_CAPACITY as u32 - 1 - grown as u32;
    assert!(glossary_bit(&mut session, refused_cp));
    assert!(glossary_bit(&mut session, 0x100000));
    // A brand-new codepoint cannot buy the space by evicting a small entry:
    // the eviction credit is honoured but the result is a refusal, not a
    // silent drop of the entry the producer saw accepted.
    refused_reason(
        session.handle(&register_body(0x10fffd, &fat, Some(1)), false, false),
        "outline_too_large",
    );
    assert!(glossary_bit(&mut session, 0x100000), "no silent eviction");
    // A registration small enough to fit after that same credit is accepted
    // and evicts exactly the oldest insertion.
    accepted(session.handle(
        &register_body(0x10fffc, &square(600), Some(0)),
        false,
        false,
    ));
    assert!(!glossary_bit(&mut session, 0x100000));
    assert!(glossary_bit(&mut session, 0x100001));
    assert!(glossary_bit(&mut session, 0x10fffc));
    assert_eq!(session.len(), GLOSSARY_CAPACITY);
}

#[test]
fn a_replacement_returns_its_balance_to_the_aggregate() {
    let mut session = GlyphSession::new();
    let fat = padded(square(1000), MAX_GLYF_BYTES);
    let entries = AGGREGATE_WIRE_BUDGET / fat.len();
    for offset in 0..entries as u32 {
        accepted(session.handle(
            &register_body(0x100000 + offset, &fat, Some(0)),
            false,
            false,
        ));
    }
    // Shrinking one entry returns the difference to the budget: the freed
    // balance buys a new registration of exactly that size and no more.
    accepted(session.handle(
        &register_body(0x100000, &square(600), Some(0)),
        false,
        false,
    ));
    let freed = fat.len() - square(600).len();
    refused_reason(
        session.handle(
            &register_body(0x10fffd, &padded(square(700), freed + 1), Some(1)),
            false,
            false,
        ),
        "outline_too_large",
    );
    accepted(session.handle(
        &register_body(0x10fffc, &padded(square(700), freed), Some(0)),
        false,
        false,
    ));
    assert!(glossary_bit(&mut session, 0x10fffc));
    assert!(!glossary_bit(&mut session, 0x10fffd));
    // And an identical-size replacement of a maxed entry stays neutral.
    accepted(session.handle(
        &register_body(0x100001, &padded(square(800), MAX_GLYF_BYTES), Some(0)),
        false,
        false,
    ));
}

#[test]
fn clear_and_ris_release_the_aggregate_budget() {
    let mut session = GlyphSession::new();
    let fat = padded(square(1000), MAX_GLYF_BYTES);
    let entries = AGGREGATE_WIRE_BUDGET / fat.len();
    for offset in 0..entries as u32 {
        accepted(session.handle(
            &register_body(0x100000 + offset, &fat, Some(0)),
            false,
            false,
        ));
    }
    refused_reason(
        session.handle(&register_body(0x10fffd, &fat, Some(1)), false, false),
        "outline_too_large",
    );
    // `c;cp=` frees exactly that entry's retained bytes.
    let (forward, reply) = command(session.handle(b"25a1;c;cp=100000", false, false));
    assert!(forward);
    assert_eq!(
        reply.unwrap(),
        b"\x1b_25a1;c;cp=100000;status=0\x1b\\".to_vec()
    );
    accepted(session.handle(&register_body(0x10fffd, &fat, Some(0)), false, false));
    refused_reason(
        session.handle(&register_body(0x10fffc, &fat, Some(1)), false, false),
        "outline_too_large",
    );
    // `c` with no codepoint clears the whole glossary; the pane's RIS path
    // releases the same budget through `clear_all` without a `c` body.
    let (forward, reply) = command(session.handle(b"25a1;c", false, false));
    assert!(forward);
    assert_eq!(reply.unwrap(), b"\x1b_25a1;c;status=0\x1b\\".to_vec());
    assert_eq!(session.len(), 0);
    for offset in 0..entries as u32 {
        accepted(session.handle(&register_body(0xE000 + offset, &fat, Some(0)), false, false));
    }
    session.clear_all();
    assert_eq!(session.len(), 0);
    accepted(session.handle(&register_body(0x10fffb, &fat, Some(0)), false, false));
}

// ---- GlyphFrame projection ---------------------------------------------------

fn frame_raw(generation: u64, declared: u32, entries: &[u8]) -> Vec<u8> {
    let mut out = b"GLS1".to_vec();
    out.extend_from_slice(&generation.to_le_bytes());
    out.extend_from_slice(&declared.to_le_bytes());
    out.extend_from_slice(entries);
    out
}

/// One frame entry laid out exactly as `GlyphFrame::decode` reads it:
/// cp(4) upm(2) width(1) flags(1) align(1) optional aw/lh/ymin pad(16)
/// len(4) glyf. `flags` bits 3..=5 carry the size tag, bits 0..=2 mark which
/// optional fields `options` supplies; `align` is h in bits 0..=1 and v in
/// bits 2..=4.
fn frame_entry(
    cp: u32,
    upm: u16,
    width: u8,
    flags: u8,
    align: u8,
    options: &[u8],
    pad: [f32; 4],
    glyf: &[u8],
) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&cp.to_le_bytes());
    out.extend_from_slice(&upm.to_le_bytes());
    out.push(width);
    out.push(flags);
    out.push(align);
    out.extend_from_slice(options);
    for value in pad {
        out.extend_from_slice(&value.to_bits().to_le_bytes());
    }
    out.extend_from_slice(&(glyf.len() as u32).to_le_bytes());
    out.extend_from_slice(glyf);
    out
}

fn frame_entry_default(cp: u32, glyf: &[u8]) -> Vec<u8> {
    // flags 0x10 = size tag `contain` (2 << 3), no optional fields; align 0x05
    // = horizontal center (1) | vertical center (1 << 2) — the protocol
    // defaults a plain `r` body produces, so this entry is reachable from
    // real traffic.
    frame_entry(cp, 1000, 1, 0x10, 0x05, &[], [0.0; 4], glyf)
}

#[test]
fn a_frame_projection_restores_fifo_age_and_generation() {
    let mut session = GlyphSession::new();
    accepted(session.handle(
        &register_body(0x100000, &square(1000), Some(0)),
        false,
        false,
    ));
    accepted(session.handle(
        &register_body(0x100001, &square(1000), Some(0)),
        false,
        false,
    ));
    // A replacement keeps its insertion age; the projection must carry that
    // ordering, not codepoint order, or a restored glossary evicts wrongly.
    accepted(session.handle(
        &register_body(0x100000, &square(600), Some(0)),
        false,
        false,
    ));
    accepted(session.handle(
        &register_body(0x100002, &square(1000), Some(0)),
        false,
        false,
    ));
    let frame = session.frame();
    let decoded = GlyphFrame::decode(&frame.encode()).expect("a produced frame is well-formed");
    let mut restored =
        GlyphSession::from_frame(decoded).expect("the authority's own snapshot must re-admit");
    assert_eq!(restored.frame().generation(), frame.generation());
    let cps: Vec<u32> = restored.frame().glyphs().iter().map(|g| g.cp).collect();
    assert_eq!(cps, [0x100000, 0x100001, 0x100002]);
    assert_eq!(
        restored.frame().glyphs()[0].glyf,
        square(600),
        "the replacement definition is the one restored"
    );
    // Two registrations past capacity evict the two oldest insertions.
    for offset in 3..=GLOSSARY_CAPACITY as u32 + 1 {
        accepted(restored.handle(
            &register_body(0x100000 + offset, &square(700), Some(0)),
            false,
            false,
        ));
    }
    assert_eq!(restored.len(), GLOSSARY_CAPACITY);
    assert!(!restored.contains(0x100000));
    assert!(!restored.contains(0x100001));
    assert!(restored.contains(0x100002));
    assert!(restored.contains(0x100000 + GLOSSARY_CAPACITY as u32 + 1));
}

#[test]
fn a_full_frame_round_trip_preserves_layout_and_payload_fields() {
    // Register through the real wire path with a fully specified layout so
    // the round trip is checked against parser-produced values.
    let mut session = GlyphSession::new();
    let mut body = Vec::new();
    body.extend_from_slice(b"25a1;r;cp=100000;fmt=glyf;reply=0;upm=2048;width=2;");
    body.extend_from_slice(
        b"aw=1024;lh=1536;ymin=-148;size=cover;align=end,baseline;pad=0.1,0.2,0.3,0.4;",
    );
    use base64::Engine as _;
    body.extend_from_slice(
        base64::engine::general_purpose::STANDARD
            .encode(square(1000))
            .as_bytes(),
    );
    accepted(session.handle(&body, false, false));
    let frame = session.frame();
    let decoded = GlyphFrame::decode(&frame.encode()).unwrap();
    assert_eq!(decoded, frame);
    let glyph = &decoded.glyphs()[0];
    assert_eq!(glyph.upm, 2048);
    assert_eq!(glyph.width, 2);
    assert_eq!(glyph.layout.aw, Some(1024));
    assert_eq!(glyph.layout.lh, Some(1536));
    assert_eq!(glyph.layout.ymin, Some(-148));
    assert_eq!(glyph.layout.size, GlyphSize::Cover);
    assert_eq!(glyph.layout.align_h, GlyphAlignH::End);
    assert_eq!(glyph.layout.align_v, GlyphAlignV::Baseline);
    assert_eq!(glyph.layout.pad, [0.1f32, 0.2, 0.3, 0.4]);
    let restored = GlyphSession::from_frame(decoded).unwrap();
    assert!(restored.contains(0x100000));
}

#[test]
fn a_frame_decoder_refuses_out_of_domain_input() {
    let ok = |entries: &[u8]| frame_raw(1, 1, entries);
    // Wrong magic, truncation mid-entry, trailing slack.
    let mut bad_magic = ok(&frame_entry_default(0xE000, &square(100)));
    bad_magic[0] = b'X';
    assert!(GlyphFrame::decode(&bad_magic).is_none());
    let full = ok(&frame_entry_default(0xE000, &square(100)));
    assert!(GlyphFrame::decode(&full[..full.len() - 1]).is_none());
    let mut trailing = full.clone();
    trailing.push(0);
    assert!(GlyphFrame::decode(&trailing).is_none());
    // A declared count beyond the 1024-entry glossary capacity.
    assert!(GlyphFrame::decode(&frame_raw(1, GLOSSARY_CAPACITY as u32 + 1, &[])).is_none());
    // A declared entry count the bytes do not actually carry.
    assert!(
        GlyphFrame::decode(&frame_raw(1, 2, &frame_entry_default(0xE000, &square(100)))).is_none()
    );
    // A non-PUA codepoint, then a duplicated one.
    assert!(GlyphFrame::decode(&ok(&frame_entry_default(0x41, &square(100)))).is_none());
    let mut twice = frame_entry_default(0xE000, &square(100));
    twice.extend_from_slice(&frame_entry_default(0xE000, &square(200)));
    assert!(GlyphFrame::decode(&frame_raw(1, 2, &twice)).is_none());
    // width outside {1,2}, reserved flag bits, an unknown size tag, an
    // unknown horizontal-align tag, a vertical-align tag above 3, reserved
    // align bits.
    for entry in [
        frame_entry(0xE000, 1000, 3, 0x10, 0x05, &[], [0.0; 4], &square(100)),
        frame_entry(0xE000, 1000, 0, 0x10, 0x05, &[], [0.0; 4], &square(100)),
        frame_entry(0xE000, 1000, 1, 0x40, 0x05, &[], [0.0; 4], &square(100)),
        frame_entry(0xE000, 1000, 1, 0x28, 0x05, &[], [0.0; 4], &square(100)),
        frame_entry(0xE000, 1000, 1, 0x10, 0x07, &[], [0.0; 4], &square(100)),
        frame_entry(0xE000, 1000, 1, 0x10, 0x15, &[], [0.0; 4], &square(100)),
        frame_entry(0xE000, 1000, 1, 0x10, 0x20, &[], [0.0; 4], &square(100)),
    ] {
        assert!(GlyphFrame::decode(&ok(&entry)).is_none(), "{entry:?}");
    }
    // pad must be finite fractions in 0..=1.
    for bad_pad in [
        [f32::NAN, 0.0, 0.0, 0.0],
        [f32::INFINITY, 0.0, 0.0, 0.0],
        [1.5, 0.0, 0.0, 0.0],
        [0.0, 0.0, 0.0, -0.5],
    ] {
        let entry = frame_entry(0xE000, 1000, 1, 0x10, 0x05, &[], bad_pad, &square(100));
        assert!(GlyphFrame::decode(&ok(&entry)).is_none(), "{bad_pad:?}");
    }
    // A per-entry payload past the 64 KiB wire limit, and a declared length
    // the frame does not actually carry.
    let oversized = frame_entry(
        0xE000,
        1000,
        1,
        0x10,
        0x05,
        &[],
        [0.0; 4],
        &vec![0u8; MAX_GLYF_BYTES + 1],
    );
    assert!(GlyphFrame::decode(&ok(&oversized)).is_none());
    let mut lying = frame_entry_default(0xE000, &square(100));
    let len_at = 4 + 2 + 1 + 1 + 1 + 16;
    lying[len_at..len_at + 4].copy_from_slice(&(MAX_GLYF_BYTES as u32).to_le_bytes());
    assert!(GlyphFrame::decode(&ok(&lying)).is_none());
}

#[test]
fn from_frame_refuses_glyphs_no_wire_registration_could_have_produced() {
    // Payloads the released decoder itself rejects — composite, hinted and
    // empty — must fail the whole restore closed rather than resurrect a
    // definition the application never managed to register.
    let mut composite = square(100);
    composite[..2].copy_from_slice(&(-1i16).to_be_bytes());
    let mut hinted = square(100);
    hinted[12..14].copy_from_slice(&1u16.to_be_bytes());
    hinted.push(0);
    for glyf in [composite, hinted, Vec::new()] {
        let frame = GlyphFrame::decode(&frame_raw(7, 1, &frame_entry_default(0xE000, &glyf)))
            .expect("frame decode itself is structural");
        assert!(GlyphSession::from_frame(frame).is_none());
    }
}

#[test]
fn a_frame_refuses_scalar_values_the_wire_parser_would_reject() {
    // The wire parser rejects `upm=0`, `aw=0` and `lh=0` as malformed. A
    // snapshot carrying any of them describes a registration that could never
    // have been accepted, so every consumer's shared decoder must refuse it —
    // the GUI applies decoded entries without a second domain check, and the
    // handoff path rebuilds a session from them.
    for entry in [
        frame_entry(0xE000, 0, 1, 0x10, 0x05, &[], [0.0; 4], &square(100)),
        frame_entry(0xE000, 1000, 1, 0x11, 0x05, &[0, 0], [0.0; 4], &square(100)),
        frame_entry(0xE000, 1000, 1, 0x12, 0x05, &[0, 0], [0.0; 4], &square(100)),
    ] {
        assert!(
            GlyphFrame::decode(&frame_raw(7, 1, &entry)).is_none(),
            "a value the wire parser rejects must not decode: {entry:?}"
        );
    }
}

#[test]
fn the_pending_query_queue_stays_bounded_and_matches_token_and_cp() {
    let mut queries = PendingQueries::default();
    for token in 0..MAX_PENDING_QUERIES as u64 {
        queries.push(PendingQuery {
            token,
            cp: 0xE000 + token as u32,
            epoch: 0,
            glossary: true,
        });
    }
    // One answer past the bound retires the oldest pending entry instead of
    // growing without limit.
    queries.push(PendingQuery {
        token: MAX_PENDING_QUERIES as u64,
        cp: 0xF000,
        epoch: 0,
        glossary: false,
    });
    assert_eq!(queries.len(), MAX_PENDING_QUERIES);
    assert!(
        queries.take(0, 0xE000).is_none(),
        "the evicted query cannot answer"
    );
    // A reply must match both token and codepoint.
    let last = MAX_PENDING_QUERIES as u64;
    assert!(queries.take(last, 0xE123).is_none());
    assert!(queries.take(last, 0xF000).is_some());
}
