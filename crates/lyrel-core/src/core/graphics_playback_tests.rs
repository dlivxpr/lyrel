use super::*;
use std::time::Duration;

fn command(state: &mut GraphicsState, header: &str, pixel: Option<[u8; 4]>) -> Vec<GraphicsEffect> {
    state.apply(
        Control::parse(format!("G{header}").as_bytes()).unwrap(),
        pixel.map(|data| Image {
            width: 1,
            height: 1,
            data: data.to_vec(),
            format: WireFormat::Rgba,
            compressed: false,
        }),
    )
}

fn selected(state: &GraphicsState) -> [u8; 4] {
    let root = state
        .lookup(&Control {
            id: 91,
            ..Control::default()
        })
        .unwrap();
    let selected = state.resources[&root].selected;
    let resource = state
        .resources
        .values()
        .find(|resource| resource.content_key == selected)
        .unwrap();
    let packet = resource.resident.lock().unwrap().packet.clone().unwrap();
    match GraphicsUpdate::decode_frame_owned(packet.to_vec()).unwrap() {
        GraphicsUpdate::Resource { image, .. } => image.data.as_slice().try_into().unwrap(),
        _ => panic!("a retained frame must contain actual pixels"),
    }
}

#[test]
fn controlled_playback_preserves_pixels_remaining_gap_and_finite_loops_across_consumers() {
    let mut state = GraphicsState::new(WinSize {
        cols: 20,
        rows: 10,
        cell_w: 8,
        cell_h: 16,
    });
    let start = std::time::Instant::now();
    state.controlled_now = Some(start);
    command(&mut state, "a=T,i=91,C=1,c=1,r=1", Some([255, 0, 0, 255]));
    command(&mut state, "a=f,i=91,z=40", Some([0, 255, 0, 255]));
    command(&mut state, "a=f,i=91,z=60", Some([0, 0, 255, 255]));
    command(&mut state, "a=a,i=91,r=1,z=100,s=3,v=3", None);
    let (a, _) = std::sync::mpsc::channel();
    let a = Arc::new(a);
    let (b, _) = std::sync::mpsc::channel();
    let b = Arc::new(b);
    let viewport = crate::daemon::protocol::GraphicsViewport {
        screen: 0,
        epoch: 0,
        top: None,
        cols: 20,
        rows: 10,
    };
    state.set_viewport(1, Arc::downgrade(&a), viewport);
    state.set_viewport(2, Arc::downgrade(&b), viewport);
    state.controlled_now = Some(start + Duration::from_millis(30));
    state.set_viewport(
        1,
        Arc::downgrade(&a),
        crate::daemon::protocol::GraphicsViewport {
            rows: 0,
            ..viewport
        },
    );
    state.controlled_now = Some(start + Duration::from_millis(50));
    state.remove_viewport(2);
    state.controlled_now = Some(start + Duration::from_secs(3600));
    state.advance_playback();
    assert_eq!(selected(&state), [255, 0, 0, 255]);
    state.set_viewport(1, Arc::downgrade(&a), viewport);
    state.controlled_now = Some(start + Duration::from_secs(3600) + Duration::from_millis(49));
    state.advance_playback();
    assert_eq!(selected(&state), [255, 0, 0, 255]);
    state.controlled_now = Some(start + Duration::from_secs(3600) + Duration::from_millis(50));
    let effects = state.advance_playback();
    assert_eq!(selected(&state), [0, 255, 0, 255]);
    assert!(effects.iter().all(|effect| !matches!(
        effect,
        GraphicsEffect::Frame(_) | GraphicsEffect::Update(GraphicsUpdate::Resource { .. })
    )));
    state.controlled_now = Some(start + Duration::from_secs(3600) + Duration::from_millis(90));
    state.advance_playback();
    assert_eq!(selected(&state), [0, 0, 255, 255]);
    state.controlled_now = Some(start + Duration::from_secs(3600) + Duration::from_millis(150));
    state.advance_playback();
    assert_eq!(selected(&state), [255, 0, 0, 255]);
    state.controlled_now = Some(start + Duration::from_secs(3600) + Duration::from_millis(350));
    state.advance_playback();
    assert_eq!(selected(&state), [0, 0, 255, 255]);
    let root = state
        .lookup(&Control {
            id: 91,
            ..Control::default()
        })
        .unwrap();
    assert_eq!(state.animations[&root].mode, 1);
}

#[test]
fn controlled_loading_waits_for_append_and_skips_gapless_frames_without_hidden_time() {
    let mut state = GraphicsState::new(WinSize {
        cols: 20,
        rows: 10,
        cell_w: 8,
        cell_h: 16,
    });
    let start = std::time::Instant::now();
    state.controlled_now = Some(start);
    command(&mut state, "a=T,i=91,C=1,c=1,r=1", Some([255, 0, 0, 255]));
    command(&mut state, "a=f,i=91,z=-1", Some([7, 7, 7, 255]));
    command(&mut state, "a=f,i=91,z=0", Some([0, 255, 0, 255]));
    command(&mut state, "a=a,i=91,s=2", None);
    let root = state
        .lookup(&Control {
            id: 91,
            ..Control::default()
        })
        .unwrap();
    let (sender, _) = std::sync::mpsc::channel();
    let sender = Arc::new(sender);
    let viewport = crate::daemon::protocol::GraphicsViewport {
        screen: 0,
        epoch: 0,
        top: None,
        cols: 20,
        rows: 10,
    };
    state.set_viewport(1, Arc::downgrade(&sender), viewport);
    state.advance_playback();
    assert_eq!(
        selected(&state),
        [0, 255, 0, 255],
        "root defaults to zero gap, negative frame is never presented"
    );
    state.controlled_now = Some(start + Duration::from_millis(39));
    state.advance_playback();
    assert!(!state.playback_snapshot(root).waiting);
    state.controlled_now = Some(start + Duration::from_millis(40));
    state.advance_playback();
    assert!(
        state.playback_snapshot(root).waiting,
        "zero on upload preserves the 40ms default"
    );
    state.set_viewport(
        1,
        Arc::downgrade(&sender),
        crate::daemon::protocol::GraphicsViewport {
            rows: 0,
            ..viewport
        },
    );
    state.controlled_now = Some(start + Duration::from_secs(500));
    command(&mut state, "a=f,i=91,z=25", Some([0, 0, 255, 255]));
    state.advance_playback();
    assert_eq!(selected(&state), [0, 255, 0, 255]);
    state.set_viewport(1, Arc::downgrade(&sender), viewport);
    state.advance_playback();
    assert_eq!(selected(&state), [0, 0, 255, 255]);
    command(&mut state, "a=a,i=91,r=4,z=0", None);
    state.controlled_now = Some(start + Duration::from_secs(500) + Duration::from_millis(24));
    state.advance_playback();
    assert!(!state.playback_snapshot(root).waiting);
    state.controlled_now = Some(start + Duration::from_secs(500) + Duration::from_millis(25));
    state.advance_playback();
    assert!(state.playback_snapshot(root).waiting);
    command(&mut state, "a=a,i=91,s=1,c=1", None);
    assert_eq!(selected(&state), [255, 0, 0, 255]);
    assert_eq!(state.playback_snapshot(root).completed, 0);
    let rejected = command(&mut state, "a=a,i=91,s=3,r=99,z=15,c=2", None);
    assert!(rejected.iter().any(|effect| matches!(effect, GraphicsEffect::Reply(reply) if reply.starts_with(b"\x1b_Gi=91;ENOENT:"))));
    assert_eq!(
        selected(&state),
        [255, 0, 0, 255],
        "invalid references cannot partially start/select"
    );
    assert_eq!(state.playback_snapshot(root).state, 1);
}

#[test]
fn controlled_visibility_withdraws_on_offviewport_screen_and_retired_reader() {
    let mut state = GraphicsState::new(WinSize {
        cols: 20,
        rows: 10,
        cell_w: 8,
        cell_h: 16,
    });
    state.controlled_now = Some(std::time::Instant::now());
    state.output(b"\x1b[6;5H");
    command(&mut state, "a=T,i=91,C=1,c=1,r=1", Some([255, 0, 0, 255]));
    command(&mut state, "a=f,i=91", Some([0, 255, 0, 255]));
    command(&mut state, "a=a,i=91,s=3,r=1,z=100", None);
    let root = state
        .lookup(&Control {
            id: 91,
            ..Control::default()
        })
        .unwrap();
    let (sender, _) = std::sync::mpsc::channel();
    let sender = Arc::new(sender);
    let viewport = crate::daemon::protocol::GraphicsViewport {
        screen: 0,
        epoch: 0,
        top: None,
        cols: 20,
        rows: 10,
    };
    state.set_viewport(
        1,
        Arc::downgrade(&sender),
        crate::daemon::protocol::GraphicsViewport {
            rows: 2,
            ..viewport
        },
    );
    assert!(state.playback_snapshot(root).paused);
    state.set_viewport(1, Arc::downgrade(&sender), viewport);
    assert!(!state.playback_snapshot(root).paused);
    state.output(b"\x1b[?1049h");
    assert!(state.playback_snapshot(root).paused);
    state.set_viewport(1, Arc::downgrade(&sender), viewport);
    assert!(
        state.playback_snapshot(root).paused,
        "obsolete screen/epoch report cannot revive visibility"
    );
    state.output(b"\x1b[?1049l");
    state.set_viewport(
        2,
        Arc::downgrade(&sender),
        crate::daemon::protocol::GraphicsViewport {
            epoch: state.viewport_epoch,
            ..viewport
        },
    );
    assert!(!state.playback_snapshot(root).paused);
    state.retain_viewports(|_| false);
    assert!(state.playback_snapshot(root).paused);
    command(&mut state, "a=a,i=91,c=2", None);
    assert_eq!(
        selected(&state),
        [0, 255, 0, 255],
        "explicit control applies while all readers are retired"
    );
}

#[test]
fn controlled_virtual_relative_visibility_and_all_gapless_scheduling_are_bounded() {
    let mut state = GraphicsState::new(WinSize {
        cols: 20,
        rows: 10,
        cell_w: 8,
        cell_h: 16,
    });
    state.controlled_now = Some(std::time::Instant::now());
    command(
        &mut state,
        "a=T,i=91,p=1,U=1,c=1,r=1",
        Some([255, 0, 0, 255]),
    );
    command(&mut state, "a=f,i=91,z=-1", Some([0, 255, 0, 255]));
    command(&mut state, "a=a,i=91,s=3", None);
    let root = state
        .lookup(&Control {
            id: 91,
            ..Control::default()
        })
        .unwrap();
    let (sender, _) = std::sync::mpsc::channel();
    let sender = Arc::new(sender);
    let viewport = crate::daemon::protocol::GraphicsViewport {
        screen: 0,
        epoch: 0,
        top: None,
        cols: 20,
        rows: 10,
    };
    state.set_viewport(1, Arc::downgrade(&sender), viewport);
    assert!(
        state.playback_snapshot(root).paused,
        "prototype is not a visible placement by itself"
    );
    state.output("\x1b[6;5H\x1b[38;5;91;58;5;1m\u{10eeee}\u{305}\u{305}\x1b[0m".as_bytes());
    assert!(!state.playback_snapshot(root).paused);
    state.advance_playback();
    assert!(
        state.playback_delay().is_none(),
        "an all-gapless infinite animation must never spin"
    );
    assert_eq!(selected(&state), [255, 0, 0, 255]);
    state.advance_playback();
    assert!(state.playback_delay().is_none());
    command(&mut state, "a=a,i=91,r=2,z=40", None);
    state.advance_playback();
    assert_eq!(selected(&state), [0, 255, 0, 255]);
    state.set_viewport(
        1,
        Arc::downgrade(&sender),
        crate::daemon::protocol::GraphicsViewport {
            cols: 3,
            ..viewport
        },
    );
    assert!(
        state.playback_snapshot(root).paused,
        "placeholder to the right of the consumer is not visible"
    );
    // A relative child remains visible even when its virtual parent cell lies
    // just outside the viewport. The graph is resolved by the real Rio grid.
    command(
        &mut state,
        "a=T,i=92,p=2,P=91,Q=1,H=-3,c=1,r=1,C=1",
        Some([0, 0, 255, 255]),
    );
    command(&mut state, "a=f,i=92", Some([255, 255, 255, 255]));
    command(&mut state, "a=a,i=92,s=3,r=1,z=100", None);
    let child = state
        .lookup(&Control {
            id: 92,
            ..Control::default()
        })
        .unwrap();
    assert!(!state.playback_snapshot(child).paused);
    state.output(b"\x1b[6;5H\x1b[X");
    assert!(
        state.playback_snapshot(child).paused,
        "erasing the only real virtual anchor withdraws the child"
    );
}

#[test]
fn controlled_current_frame_edits_preserve_timing_and_follow_new_content_keys() {
    let mut state = GraphicsState::new(WinSize {
        cols: 20,
        rows: 10,
        cell_w: 8,
        cell_h: 16,
    });
    let start = std::time::Instant::now();
    state.controlled_now = Some(start);
    command(&mut state, "a=T,i=91,C=1,c=1,r=1", Some([255, 0, 0, 255]));
    command(&mut state, "a=f,i=91,z=70", Some([0, 255, 0, 255]));
    command(&mut state, "a=a,i=91,s=3,r=1,z=100", None);
    let root = state
        .lookup(&Control {
            id: 91,
            ..Control::default()
        })
        .unwrap();
    let (sender, _) = std::sync::mpsc::channel();
    let sender = Arc::new(sender);
    state.set_viewport(
        1,
        Arc::downgrade(&sender),
        crate::daemon::protocol::GraphicsViewport {
            screen: 0,
            epoch: 0,
            top: None,
            cols: 20,
            rows: 10,
        },
    );
    state.controlled_now = Some(start + Duration::from_millis(20));
    command(&mut state, "a=f,i=91,r=1,X=1", Some([255, 255, 0, 255]));
    assert_eq!(selected(&state), [255, 255, 0, 255]);
    assert_eq!(state.playback_snapshot(root).remaining_ms, 80);
    state.controlled_now = Some(start + Duration::from_millis(100));
    state.advance_playback();
    assert_eq!(selected(&state), [0, 255, 0, 255]);
    state.controlled_now = Some(start + Duration::from_millis(110));
    command(&mut state, "a=f,i=91,r=2,X=1", Some([0, 0, 255, 255]));
    assert_eq!(selected(&state), [0, 0, 255, 255]);
    assert_eq!(state.playback_snapshot(root).remaining_ms, 60);
    state.controlled_now = Some(start + Duration::from_millis(169));
    state.advance_playback();
    assert_eq!(selected(&state), [0, 0, 255, 255]);
    state.controlled_now = Some(start + Duration::from_millis(170));
    state.advance_playback();
    assert_eq!(selected(&state), [255, 255, 0, 255]);
    state.controlled_now = Some(start + Duration::from_millis(175));
    command(
        &mut state,
        "a=f,i=91,r=1,X=1,z=25",
        Some([255, 0, 255, 255]),
    );
    assert_eq!(selected(&state), [255, 0, 255, 255]);
    assert_eq!(state.playback_snapshot(root).remaining_ms, 25);
    state.controlled_now = Some(start + Duration::from_millis(200));
    state.advance_playback();
    assert_eq!(selected(&state), [0, 0, 255, 255]);
}

#[test]
fn controlled_frame_mutations_preserve_wait_pause_gap_numbering_and_loop_progress() {
    for mode in [1, 2, 3] {
        for hidden in [false, true] {
            let mut state = GraphicsState::new(WinSize { cols:20, rows:10, cell_w:8, cell_h:16 });
            let start = std::time::Instant::now();
            state.controlled_now = Some(start);
            command(&mut state, "a=T,i=91,C=1,c=1,r=1", Some([255,0,0,255]));
            command(&mut state, "a=f,i=91,z=80", Some([0,255,0,255]));
            command(&mut state, "a=f,i=91,z=120", Some([0,0,255,255]));
            let (sender, _) = std::sync::mpsc::channel();
            let sender = Arc::new(sender);
            let viewport = crate::daemon::protocol::GraphicsViewport { screen:0, epoch:0, top:None, cols:20, rows:10 };
            state.set_viewport(1, Arc::downgrade(&sender), viewport);
            command(&mut state, &format!("a=a,i=91,s={mode},c=2,v=3"), None);
            let root = state.lookup(&Control { id:91, ..Control::default() }).unwrap();
            state.controlled_now = Some(start + Duration::from_millis(30));
            if hidden { state.remove_viewport(1); }
            let before = state.playback_snapshot(root);
            command(&mut state, "a=c,i=91,r=3,c=2,C=1,z=999", None);
            assert_eq!(selected(&state), [0,0,255,255]);
            let after = state.playback_snapshot(root);
            assert_eq!((after.state, after.completed, after.remaining_ms, after.paused),
                (before.state, before.completed, before.remaining_ms, before.paused), "compose must not reinterpret z or reset current gap");
            command(&mut state, "a=f,i=91,r=2,X=1", Some([9,8,7,255]));
            assert_eq!(selected(&state), [9,8,7,255]);
            assert_eq!(state.playback_snapshot(root).remaining_ms, before.remaining_ms);
            // Remove a frame before current: selected content and its deadline survive.
            command(&mut state, "a=d,d=f,i=91,r=1", None);
            assert_eq!(selected(&state), [9,8,7,255]);
            assert_eq!(state.playback_snapshot(root).remaining_ms, before.remaining_ms);
            assert_eq!(state.frame_key(root, 1), Some(state.resources[&root].selected));
            command(&mut state, "a=f,i=91,z=160", Some([255,255,0,255]));
            // Delete current root: next content becomes root and keeps its own gap.
            command(&mut state, "a=d,d=f,i=91,r=1", None);
            assert_eq!(selected(&state), [0,0,255,255]);
            assert_eq!(state.playback_snapshot(root).remaining_ms, 120);
            assert_eq!(state.playback_snapshot(root).state, mode as u8);
            assert_eq!(state.playback_snapshot(root).completed, before.completed);
            command(&mut state, "a=d,d=F,i=91,r=999", None);
            assert_eq!(selected(&state), [0,0,255,255]);
            command(&mut state, "a=d,d=f,i=91", None);
            assert_eq!(selected(&state), [0,0,255,255]);
            command(&mut state, "a=d,d=F,i=91", None);
            assert!(state.lookup(&Control { id:91, ..Control::default() }).is_none());
            state.advance_playback();
            assert!(state.playback_delay().is_none());
        }
    }
}

#[test]
fn controlled_loading_mutations_only_wake_for_a_successor_and_resume_after_hidden_append() {
    let mut state = GraphicsState::new(WinSize { cols:20, rows:10, cell_w:8, cell_h:16 });
    let start = std::time::Instant::now();
    state.controlled_now = Some(start);
    command(&mut state, "a=T,i=91,C=1,c=1,r=1", Some([255,0,0,255]));
    command(&mut state, "a=f,i=91,z=50", Some([0,255,0,255]));
    command(&mut state, "a=a,i=91,s=2,c=2", None);
    let root = state.lookup(&Control { id:91, ..Control::default() }).unwrap();
    let (sender, _) = std::sync::mpsc::channel();
    let sender = Arc::new(sender);
    let viewport = crate::daemon::protocol::GraphicsViewport { screen:0, epoch:0, top:None, cols:20, rows:10 };
    state.set_viewport(1, Arc::downgrade(&sender), viewport);
    state.controlled_now = Some(start + Duration::from_millis(50));
    state.advance_playback();
    assert!(state.playback_snapshot(root).waiting);
    command(&mut state, "a=c,i=91,r=1,c=2,C=1", None);
    assert!(state.playback_snapshot(root).waiting);
    command(&mut state, "a=f,i=91,r=2,X=1", Some([1,2,3,255]));
    assert!(state.playback_snapshot(root).waiting);
    command(&mut state, "a=d,d=f,i=91,r=1", None);
    assert_eq!(selected(&state), [1,2,3,255]);
    assert!(state.playback_snapshot(root).waiting);
    state.remove_viewport(1);
    command(&mut state, "a=f,i=91,z=70", Some([0,0,255,255]));
    assert!(!state.playback_snapshot(root).waiting);
    state.controlled_now = Some(start + Duration::from_secs(900));
    state.advance_playback();
    assert_eq!(selected(&state), [1,2,3,255]);
    state.set_viewport(1, Arc::downgrade(&sender), viewport);
    state.advance_playback();
    assert_eq!(selected(&state), [0,0,255,255]);
    state.controlled_now = Some(start + Duration::from_secs(900) + Duration::from_millis(69));
    state.advance_playback();
    assert!(!state.playback_snapshot(root).waiting);
    state.controlled_now = Some(start + Duration::from_secs(900) + Duration::from_millis(70));
    state.advance_playback();
    assert!(state.playback_snapshot(root).waiting);
}

#[test]
fn controlled_current_deletion_preserves_nonzero_finite_loop_progress() {
    let mut state = GraphicsState::new(WinSize { cols:20, rows:10, cell_w:8, cell_h:16 });
    let start = std::time::Instant::now();
    state.controlled_now = Some(start);
    command(&mut state, "a=T,i=91,C=1,c=1,r=1", Some([255,0,0,255]));
    command(&mut state, "a=f,i=91,z=80", Some([0,255,0,255]));
    command(&mut state, "a=f,i=91,z=120", Some([0,0,255,255]));
    command(&mut state, "a=a,i=91,s=3,r=1,z=100,v=3", None);
    let root = state.lookup(&Control { id:91, ..Control::default() }).unwrap();
    let (sender, _) = std::sync::mpsc::channel();
    let sender = Arc::new(sender);
    state.set_viewport(1, Arc::downgrade(&sender), crate::daemon::protocol::GraphicsViewport { screen:0, epoch:0, top:None, cols:20, rows:10 });
    state.controlled_now = Some(start + Duration::from_millis(300));
    state.advance_playback();
    assert_eq!(state.playback_snapshot(root).completed, 1);
    assert_eq!(selected(&state), [255,0,0,255]);
    command(&mut state, "a=d,d=f,i=91,r=1", None);
    assert_eq!(selected(&state), [0,255,0,255]);
    assert_eq!(state.playback_snapshot(root).completed, 1);
    state.controlled_now = Some(start + Duration::from_millis(380));
    state.advance_playback();
    assert_eq!(selected(&state), [0,0,255,255]);
    state.controlled_now = Some(start + Duration::from_millis(500));
    state.advance_playback();
    assert_eq!((state.playback_snapshot(root).state, state.playback_snapshot(root).completed), (1,2));
    assert_eq!(selected(&state), [0,0,255,255]);
}
