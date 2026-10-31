use super::*;
use std::time::Duration;

fn command(state: &mut GraphicsState, header: &str, pixel: Option<[u8; 4]>) -> Vec<GraphicsEffect> {
    state.apply(Control::parse(format!("G{header}").as_bytes()).unwrap(), pixel.map(|data| Image {
        width: 1, height: 1, data: data.to_vec(), format: WireFormat::Rgba, compressed: false,
    }))
}

fn selected(state: &GraphicsState) -> [u8; 4] {
    let root = state.lookup(&Control { id: 91, ..Control::default() }).unwrap();
    let selected = state.resources[&root].selected;
    let resource = state.resources.values().find(|resource| resource.content_key == selected).unwrap();
    let packet = resource.resident.lock().unwrap().packet.clone().unwrap();
    match GraphicsUpdate::decode_frame_owned(packet.to_vec()).unwrap() {
        GraphicsUpdate::Resource { image, .. } => image.data.as_slice().try_into().unwrap(),
        _ => panic!("a retained frame must contain actual pixels"),
    }
}

#[test]
fn controlled_playback_preserves_pixels_remaining_gap_and_finite_loops_across_consumers() {
    let mut state = GraphicsState::new(WinSize { cols: 20, rows: 10, cell_w: 8, cell_h: 16 });
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
    let viewport = crate::daemon::protocol::GraphicsViewport { screen: 0, epoch: 0, top: None, cols: 20, rows: 10 };
    state.set_viewport(1, Arc::downgrade(&a), viewport);
    state.set_viewport(2, Arc::downgrade(&b), viewport);
    state.controlled_now = Some(start + Duration::from_millis(30));
    state.set_viewport(1, Arc::downgrade(&a), crate::daemon::protocol::GraphicsViewport { rows: 0, ..viewport });
    state.controlled_now = Some(start + Duration::from_millis(50));
    state.remove_viewport(2);
    state.controlled_now = Some(start + Duration::from_secs(3600));
    assert!(state.advance_playback().is_empty());
    assert_eq!(selected(&state), [255, 0, 0, 255]);
    state.set_viewport(1, Arc::downgrade(&a), viewport);
    state.controlled_now = Some(start + Duration::from_secs(3600) + Duration::from_millis(49));
    state.advance_playback();
    assert_eq!(selected(&state), [255, 0, 0, 255]);
    state.controlled_now = Some(start + Duration::from_secs(3600) + Duration::from_millis(50));
    let effects = state.advance_playback();
    assert_eq!(selected(&state), [0, 255, 0, 255]);
    assert!(effects.iter().all(|effect| !matches!(effect, GraphicsEffect::Frame(_) | GraphicsEffect::Update(GraphicsUpdate::Resource { .. }))));
    state.controlled_now = Some(start + Duration::from_secs(3600) + Duration::from_millis(90));
    state.advance_playback();
    assert_eq!(selected(&state), [0, 0, 255, 255]);
    state.controlled_now = Some(start + Duration::from_secs(3600) + Duration::from_millis(150));
    state.advance_playback();
    assert_eq!(selected(&state), [255, 0, 0, 255]);
    state.controlled_now = Some(start + Duration::from_secs(3600) + Duration::from_millis(350));
    state.advance_playback();
    assert_eq!(selected(&state), [0, 0, 255, 255]);
    let root = state.lookup(&Control { id: 91, ..Control::default() }).unwrap();
    assert_eq!(state.animations[&root].mode, 1);
}

#[test]
fn controlled_loading_waits_for_append_and_skips_gapless_frames_without_hidden_time() {
    let mut state = GraphicsState::new(WinSize { cols: 20, rows: 10, cell_w: 8, cell_h: 16 });
    let start = std::time::Instant::now();
    state.controlled_now = Some(start);
    command(&mut state, "a=T,i=91,C=1,c=1,r=1", Some([255, 0, 0, 255]));
    command(&mut state, "a=f,i=91,z=-1", Some([7, 7, 7, 255]));
    command(&mut state, "a=f,i=91,z=0", Some([0, 255, 0, 255]));
    command(&mut state, "a=a,i=91,s=2", None);
    let root = state.lookup(&Control { id: 91, ..Control::default() }).unwrap();
    let (sender, _) = std::sync::mpsc::channel();
    let sender = Arc::new(sender);
    let viewport = crate::daemon::protocol::GraphicsViewport { screen: 0, epoch: 0, top: None, cols: 20, rows: 10 };
    state.set_viewport(1, Arc::downgrade(&sender), viewport);
    state.advance_playback();
    assert_eq!(selected(&state), [0, 255, 0, 255], "root defaults to zero gap, negative frame is never presented");
    state.controlled_now = Some(start + Duration::from_millis(39));
    state.advance_playback();
    assert!(!state.playback_snapshot(root).waiting);
    state.controlled_now = Some(start + Duration::from_millis(40));
    state.advance_playback();
    assert!(state.playback_snapshot(root).waiting, "zero on upload preserves the 40ms default");
    state.set_viewport(1, Arc::downgrade(&sender), crate::daemon::protocol::GraphicsViewport { rows: 0, ..viewport });
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
    assert_eq!(selected(&state), [255, 0, 0, 255], "invalid references cannot partially start/select");
    assert_eq!(state.playback_snapshot(root).state, 1);
}

#[test]
fn controlled_visibility_withdraws_on_offviewport_screen_and_retired_reader() {
    let mut state = GraphicsState::new(WinSize { cols: 20, rows: 10, cell_w: 8, cell_h: 16 });
    state.controlled_now = Some(std::time::Instant::now());
    state.output(b"\x1b[6;5H");
    command(&mut state, "a=T,i=91,C=1,c=1,r=1", Some([255, 0, 0, 255]));
    command(&mut state, "a=f,i=91", Some([0, 255, 0, 255]));
    command(&mut state, "a=a,i=91,s=3,r=1,z=100", None);
    let root = state.lookup(&Control { id: 91, ..Control::default() }).unwrap();
    let (sender, _) = std::sync::mpsc::channel();
    let sender = Arc::new(sender);
    let viewport = crate::daemon::protocol::GraphicsViewport { screen: 0, epoch: 0, top: None, cols: 20, rows: 10 };
    state.set_viewport(1, Arc::downgrade(&sender), crate::daemon::protocol::GraphicsViewport { rows: 2, ..viewport });
    assert!(state.playback_snapshot(root).paused);
    state.set_viewport(1, Arc::downgrade(&sender), viewport);
    assert!(!state.playback_snapshot(root).paused);
    state.output(b"\x1b[?1049h");
    assert!(state.playback_snapshot(root).paused);
    state.set_viewport(1, Arc::downgrade(&sender), viewport);
    assert!(state.playback_snapshot(root).paused, "obsolete screen/epoch report cannot revive visibility");
    state.output(b"\x1b[?1049l");
    state.set_viewport(2, Arc::downgrade(&sender), crate::daemon::protocol::GraphicsViewport { epoch: state.viewport_epoch, ..viewport });
    assert!(!state.playback_snapshot(root).paused);
    state.retain_viewports(|_| false);
    assert!(state.playback_snapshot(root).paused);
    command(&mut state, "a=a,i=91,c=2", None);
    assert_eq!(selected(&state), [0, 255, 0, 255], "explicit control applies while all readers are retired");
}

#[test]
fn controlled_virtual_relative_visibility_and_all_gapless_scheduling_are_bounded() {
    let mut state = GraphicsState::new(WinSize { cols: 20, rows: 10, cell_w: 8, cell_h: 16 });
    state.controlled_now = Some(std::time::Instant::now());
    command(&mut state, "a=T,i=91,p=1,U=1,c=1,r=1", Some([255, 0, 0, 255]));
    command(&mut state, "a=f,i=91,z=-1", Some([0, 255, 0, 255]));
    command(&mut state, "a=a,i=91,s=3", None);
    let root = state.lookup(&Control { id: 91, ..Control::default() }).unwrap();
    let (sender, _) = std::sync::mpsc::channel();
    let sender = Arc::new(sender);
    let viewport = crate::daemon::protocol::GraphicsViewport { screen: 0, epoch: 0, top: None, cols: 20, rows: 10 };
    state.set_viewport(1, Arc::downgrade(&sender), viewport);
    assert!(state.playback_snapshot(root).paused, "prototype is not a visible placement by itself");
    state.output("\x1b[6;5H\x1b[38;5;91;58;5;1m\u{10eeee}\u{305}\u{305}\x1b[0m".as_bytes());
    assert!(!state.playback_snapshot(root).paused);
    state.advance_playback();
    assert!(state.playback_delay().is_none(), "an all-gapless infinite animation must never spin");
    assert_eq!(selected(&state), [255, 0, 0, 255]);
    state.advance_playback();
    assert!(state.playback_delay().is_none());
    command(&mut state, "a=a,i=91,r=2,z=40", None);
    state.advance_playback();
    assert_eq!(selected(&state), [0, 255, 0, 255]);
    state.set_viewport(1, Arc::downgrade(&sender), crate::daemon::protocol::GraphicsViewport { cols: 3, ..viewport });
    assert!(state.playback_snapshot(root).paused, "placeholder to the right of the consumer is not visible");
    // A relative child remains visible even when its virtual parent cell lies
    // just outside the viewport. The graph is resolved by the real Rio grid.
    command(&mut state, "a=T,i=92,p=2,P=91,Q=1,H=-3,c=1,r=1,C=1", Some([0, 0, 255, 255]));
    command(&mut state, "a=f,i=92", Some([255, 255, 255, 255]));
    command(&mut state, "a=a,i=92,s=3,r=1,z=100", None);
    let child = state.lookup(&Control { id: 92, ..Control::default() }).unwrap();
    assert!(!state.playback_snapshot(child).paused);
    state.output(b"\x1b[6;5H\x1b[X");
    assert!(state.playback_snapshot(child).paused, "erasing the only real virtual anchor withdraws the child");
}
