use super::super::super::test_support::*;
use super::*;
use crate::daemon::control::{ControlClientMsg, ControlReply, ControlRequest, ReplyOk};
use std::io::Write;

#[test]
fn a_stream_slot_comes_back_however_it_leaves() {
    let count = Arc::new(AtomicUsize::new(0));

    {
        let slot = StreamSlot(Arc::clone(&count));
        slot.0.fetch_add(1, Ordering::AcqRel);
        assert_eq!(count.load(Ordering::Acquire), 1);
    }
    assert_eq!(count.load(Ordering::Acquire), 0, "released on drop");

    let refused = StreamSlot(Arc::clone(&count));
    refused.0.fetch_add(1, Ordering::AcqRel);
    drop(refused);
    assert_eq!(count.load(Ordering::Acquire), 0);

    let panicking = Arc::clone(&count);
    let _ = std::thread::spawn(move || {
        let slot = StreamSlot(panicking);
        slot.0.fetch_add(1, Ordering::AcqRel);
        panic!("the read blew up");
    })
    .join();
    assert_eq!(
        count.load(Ordering::Acquire),
        0,
        "a panicking stream must not take its slot with it"
    );
}

#[test]
fn git_stream_delivers_the_same_lines_as_the_buffered_read() {
    let (mut client, _hello) = raw();
    let here = env!("CARGO_MANIFEST_DIR");
    let args: Vec<String> = ["log", "--oneline", "-n", "30"]
        .iter()
        .map(|s| s.to_string())
        .collect();

    let buffered = match ask(
        &mut client,
        1,
        ControlRequest::Git {
            cwd: here.to_string(),
            args: args.clone(),
        },
    ) {
        ControlReply::Ok(ReplyOk::Output(o)) => o,
        other => panic!("expected output, got {other:?}"),
    };
    if buffered.status != Some(0) {
        return;
    }
    let expected: Vec<String> = String::from_utf8_lossy(&buffered.stdout)
        .lines()
        .map(str::to_string)
        .collect();

    ControlClientMsg::Request {
        req_id: 2,
        req: ControlRequest::GitStream {
            id: 77,
            cwd: here.to_string(),
            args,
        },
    }
    .encode(&mut client)
    .unwrap();
    client.flush().unwrap();

    let mut split = crate::core::git::LineSplitter::default();
    let mut got: Vec<String> = Vec::new();
    let mut accepted = false;
    let code = loop {
        match ControlServerMsg::read(&mut client).unwrap() {
            ControlServerMsg::Response { req_id: 2, reply } => {
                assert!(
                    matches!(reply, ControlReply::Ok(ReplyOk::Unit)),
                    "{reply:?}"
                );
                accepted = true;
            }
            ControlServerMsg::Event(ControlEvent::GitChunk { id, bytes }) => {
                assert_eq!(id, 77, "chunks carry the id the client chose");
                split.push(&bytes, |l| got.push(l.to_string()));
            }
            ControlServerMsg::Event(ControlEvent::GitEnd { id, code, failed }) => {
                assert_eq!(id, 77);
                assert!(!failed, "git ran");
                break code;
            }
            other => panic!("unexpected {other:?}"),
        }
    };
    split.finish(|l| got.push(l.to_string()));
    assert!(accepted, "the request was answered");
    assert_eq!(code, Some(0));
    assert_eq!(got, expected, "streamed lines match the buffered read");
    assert!(!got.is_empty(), "this repo has commits");
}
