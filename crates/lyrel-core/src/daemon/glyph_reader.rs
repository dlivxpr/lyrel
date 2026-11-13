//! Ordered application of daemon-framed Glyph Protocol bodies.
//!
//! The reader's ordered part list hands each accepted/rejected body here at its
//! stream position. The authority (validation, FIFO, replies) lives in
//! [`crate::core::glyph_protocol`]; this module only sequences it against the
//! pane lock, the ring/subscriber fan-out and the controller-bound query.

use super::*;
use crate::core::glyph_protocol::{GlyphOutcome, GlyphSession, PendingQuery};

pub(super) fn consume(
    state: &Arc<Mutex<PaneState>>,
    gate: &OutputGate,
    writer: &Arc<Mutex<Box<dyn Write + Send>>>,
    body: &[u8],
    aborted: bool,
    exhausted: bool,
) {
    let outcome = {
        let mut st = state.lock().unwrap();
        st.glyphs
            .get_or_insert_with(GlyphSession::new)
            .handle(body, aborted, exhausted)
    };
    match outcome {
        GlyphOutcome::Ignore => {}
        GlyphOutcome::Pass { forward, reply } => {
            if let Some(reply) = reply {
                write_graphics_reply(writer, &reply);
            }
            if forward {
                // Re-emit the accepted body in place: consumers derive their
                // picture from the daemon's decision, and a replayed stream
                // carries exactly the accepted registrations.
                let mut framed = Vec::with_capacity(body.len() + 4);
                framed.extend_from_slice(b"\x1b_");
                framed.extend_from_slice(body);
                framed.extend_from_slice(b"\x1b\\");
                super::graphics_reader::text(state, &framed, gate, writer);
            }
        }
        GlyphOutcome::Query { cp, glossary } => request_query(state, cp, glossary),
        GlyphOutcome::Support => {
            // The `s` probe advertises only what the current controller can
            // actually paint. The daemon is the single reply writer, and the
            // answer names `fmt=glyf` — never a colour format — only when the
            // controller opted in to the complete monochrome contract. A
            // state-only or detached controller keeps the probe silent, which
            // is the protocol's truthful "unsupported" before the producer's
            // DA1 sentinel, not a fabricated yes.
            if state
                .lock()
                .unwrap()
                .subscriptions
                .controller_renders_glyphs()
            {
                write_graphics_reply(writer, b"\x1b_25a1;s;fmt=glyf\x1b\\");
            }
        }
    }
}

/// Hand one frontend-bound query to the pane's current controller only. The
/// daemon remains the single reply writer; with no controller the query stays
/// unanswered (the protocol's frontend-bound rule, not a fabricated negative).
fn request_query(state: &Arc<Mutex<PaneState>>, cp: u32, glossary: bool) {
    let mut st = state.lock().unwrap();
    // A controller that never sent `SubscribeGlyphState` cannot decode the
    // query kind. Leave the frontend-bound request unanswered rather than
    // breaking an old client's stream.
    if !st.subscriptions.controller_accepts_glyph_state() {
        return;
    }
    let Some(epoch) = st.subscriptions.controller_epoch() else {
        return;
    };
    let token = st.next_glyph_token;
    st.next_glyph_token = st.next_glyph_token.wrapping_add(1);
    st.glyph_queries.push(PendingQuery {
        token,
        cp,
        epoch,
        glossary,
    });
    if !st
        .subscriptions
        .tell_controller(DaemonMsg::GlyphQuery { token, cp })
    {
        st.glyph_queries.take(token, cp);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    fn pane_with_controller() -> (Arc<Mutex<PaneState>>, mpsc::Receiver<DaemonMsg>) {
        let mut pane = test_support::test_state(true);
        let (tx, rx) = mpsc::channel();
        attach_subscriber_with_features(
            &mut pane,
            tx,
            &OutputGate::new(),
            false,
            false,
            false,
            // Glyph state — the controller may receive `GlyphQuery` frames —
            // and no render opt-in. Query tests bind the state credential
            // only; support probes attach through `pane_with_renderer`.
            true,
            false,
        );
        (Arc::new(Mutex::new(pane)), rx)
    }

    /// A controller that opted in to both the state credential and the
    /// complete monochrome render contract — the `s` probe's reply gate.
    fn pane_with_renderer() -> (Arc<Mutex<PaneState>>, mpsc::Receiver<DaemonMsg>) {
        let mut pane = test_support::test_state(true);
        let (tx, rx) = mpsc::channel();
        attach_subscriber_with_features(
            &mut pane,
            tx,
            &OutputGate::new(),
            false,
            false,
            false,
            true,
            true,
        );
        (Arc::new(Mutex::new(pane)), rx)
    }

    #[test]
    fn a_query_reaches_the_controller_and_composes_one_reply() {
        let (state, rx) = pane_with_controller();
        let gate = OutputGate::new();
        let writer = test_support::null_writer();
        consume(&state, &gate, &writer, b"25a1;q;cp=100000", false, false);
        let token = loop {
            match rx.recv_timeout(Duration::from_secs(1)) {
                Ok(DaemonMsg::GlyphQuery { token, cp }) => {
                    assert_eq!(cp, 0x100000);
                    break token;
                }
                Ok(_) => continue,
                Err(error) => panic!("expected GlyphQuery, got {error:?}"),
            }
        };
        let epoch = state
            .lock()
            .unwrap()
            .subscriptions
            .controller_epoch()
            .unwrap();
        let reply = state
            .lock()
            .unwrap()
            .take_glyph_query_reply(epoch, token, 0x100000, false)
            .expect("the pending query is answered");
        assert_eq!(reply, "\x1b_25a1;q;cp=100000;status=\x1b\\");
        assert!(
            state
                .lock()
                .unwrap()
                .take_glyph_query_reply(epoch, token, 0x100000, false)
                .is_none(),
            "a duplicate answer is ignored"
        );
    }

    #[test]
    fn a_query_without_a_controller_stays_silent() {
        let pane = test_support::test_state(true);
        let state = Arc::new(Mutex::new(pane));
        let gate = OutputGate::new();
        let writer = test_support::null_writer();
        consume(&state, &gate, &writer, b"25a1;q;cp=100000", false, false);
        assert_eq!(state.lock().unwrap().glyph_queries.len(), 0);
    }

    #[test]
    fn a_stale_epoch_never_writes_a_reply() {
        let (state, rx) = pane_with_controller();
        let gate = OutputGate::new();
        let writer = test_support::null_writer();
        consume(&state, &gate, &writer, b"25a1;q;cp=100000", false, false);
        let token = loop {
            match rx.recv_timeout(Duration::from_secs(1)) {
                Ok(DaemonMsg::GlyphQuery { token, .. }) => break token,
                Ok(_) => continue,
                Err(error) => panic!("expected GlyphQuery, got {error:?}"),
            }
        };
        let epoch = state
            .lock()
            .unwrap()
            .subscriptions
            .controller_epoch()
            .unwrap();
        assert!(
            state
                .lock()
                .unwrap()
                .take_glyph_query_reply(epoch + 1, token, 0x100000, true)
                .is_none()
        );
        assert_eq!(state.lock().unwrap().glyph_queries.len(), 1);
    }

    #[test]
    fn a_support_probe_answers_only_for_a_rendering_controller() {
        // The render opt-in answers `fmt=glyf`; the daemon writes it to the
        // PTY input side so the producer reads it in stream order.
        let (state, _rx) = pane_with_renderer();
        let gate = OutputGate::new();
        let (writer, bytes) = test_support::capture_writer();
        consume(&state, &gate, &writer, b"25a1;s", false, false);
        assert_eq!(
            bytes.lock().unwrap().as_slice(),
            b"\x1b_25a1;s;fmt=glyf\x1b\\",
            "a rendering controller earns the truthful glyf reply"
        );
    }

    #[test]
    fn a_state_only_controller_keeps_the_support_probe_silent() {
        // A controller that subscribes to glyph state but never opted in to
        // rendering is exactly the pre-v1.12 peer: it understands `GlyphQuery`
        // and `GlyphState` frames yet cannot paint registered outlines, so
        // the daemon must not advertise `fmt=glyf` to it.
        let (state, _rx) = pane_with_controller();
        let gate = OutputGate::new();
        let (writer, bytes) = test_support::capture_writer();
        consume(&state, &gate, &writer, b"25a1;s", false, false);
        assert!(
            bytes.lock().unwrap().is_empty(),
            "a state-only controller keeps the support reply silent"
        );
    }
}

#[cfg(test)]
mod pipeline_tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn a_framed_query_survives_extraction_and_reaches_the_controller() {
        let mut pane = test_support::test_state(true);
        let (tx, rx) = mpsc::channel();
        attach_subscriber_with_features(
            &mut pane,
            tx,
            &OutputGate::new(),
            false,
            false,
            false,
            true,
            false,
        );
        let state = Arc::new(Mutex::new(pane));
        let gate = OutputGate::new();
        let writer = test_support::null_writer();
        let mut clipboard = ClipboardSniffer::default();
        clipboard.set_controller(Some(1), true);
        let mut glyph = GlyphSniffer::new();
        let mut graphics = GraphicsSniffer::new_local(true);
        let mut discovery = crate::core::xtgettcap::Sniffer::default();
        let extracted = graphics_reader::extract(
            b"\x1b_25a1;q;cp=100000\x1b\\",
            &mut clipboard,
            &mut glyph,
            &mut graphics,
            &mut || Some(true),
            &mut discovery,
        );
        graphics_reader::consume(extracted, &state, &gate, &writer, Some(1), &mut graphics);
        loop {
            match rx.recv_timeout(Duration::from_secs(1)) {
                Ok(DaemonMsg::GlyphQuery { cp, .. }) => {
                    assert_eq!(cp, 0x100000);
                    return;
                }
                Ok(_) => continue,
                Err(error) => panic!("expected GlyphQuery through the pipeline, got {error:?}"),
            }
        }
    }
}
