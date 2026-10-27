use super::*;
#[test]
fn copy_on_select_copies_the_buffer_the_gesture_touched() {
    assert_eq!(select_end_copy(false, true, false), SelectEndCopy::None);
    assert_eq!(select_end_copy(false, false, true), SelectEndCopy::None);

    assert_eq!(select_end_copy(true, true, false), SelectEndCopy::Grid);
    assert_eq!(select_end_copy(true, false, true), SelectEndCopy::Editor);

    assert_eq!(select_end_copy(true, false, false), SelectEndCopy::None);

    assert_eq!(select_end_copy(true, true, true), SelectEndCopy::Grid);
}

#[test]
fn smooth_scroll_step_accumulates_and_clamps() {
    assert_eq!(smooth_scroll_step(0, 0.0, 0.4, 100), (0, 0.4));
    let (jump, frac) = smooth_scroll_step(0, 0.4, 0.8, 100);
    assert_eq!(jump, 1);
    assert!((frac - 0.2).abs() < 1e-4);
    let (jump, frac) = smooth_scroll_step(5, 0.2, -0.5, 100);
    assert_eq!(jump, -1);
    assert!((frac - 0.7).abs() < 1e-4);
    assert_eq!(smooth_scroll_step(3, 0.5, -10.0, 100), (-3, 0.0));
    assert_eq!(smooth_scroll_step(98, 0.0, 7.3, 100), (2, 0.0));
    assert_eq!(smooth_scroll_step(0, 0.0, 2.5, 0), (0, 0.0));
}

#[test]
fn scroll_anim_step_converges_and_lands() {
    let frame = SCROLL_ANIM_FRAME;
    // A notch is spread over frames instead of being applied whole.
    let (step, last) = scroll_anim_step(3.0, frame);
    assert!(!last);
    assert!(
        step > 0. && step < 3.0,
        "took {step} of 3 lines in one frame"
    );

    // And it converges: no notch is left hanging.
    let mut remaining = 3.0_f32;
    let mut frames = 0u32;
    loop {
        let (step, last) = scroll_anim_step(remaining, frame);
        remaining -= step;
        frames += 1;
        if last {
            break;
        }
        assert!(frames < 200, "still {remaining} lines short after {frames}");
    }
    assert!(remaining.abs() < 1e-4, "landed {remaining} lines off");
    // Slow enough to read as motion, fast enough not to feel like lag.
    let ms = frames * SCROLL_ANIM_FRAME.as_millis() as u32;
    assert!((60..=200).contains(&ms), "a 3-line notch took {ms}ms");
}

#[test]
fn scroll_anim_step_covers_dropped_frames() {
    // A late tick has to make up the ground it missed, or a busy pane would
    // scroll slower than an idle one.
    let (one, _) = scroll_anim_step(10.0, SCROLL_ANIM_FRAME);
    let (four, _) = scroll_anim_step(10.0, SCROLL_ANIM_FRAME * 4);
    assert!(four > one * 2., "{four} should far outpace {one}");
    // But the catch-up is capped, so a tick after a long stall never
    // overshoots what was actually asked for.
    let (stalled, _) = scroll_anim_step(10.0, std::time::Duration::from_secs(5));
    assert!(
        stalled > four && stalled <= 10.0,
        "stalled tick took {stalled}"
    );
}

#[test]
fn scroll_anim_step_lands_on_a_negligible_remainder() {
    let (step, last) = scroll_anim_step(-0.005, SCROLL_ANIM_FRAME);
    assert!(last);
    assert_eq!(step, -0.005);
}

#[test]
fn drag_scroll_step_scales_with_overshoot_and_caps() {
    assert_eq!(drag_scroll_step(0.2), 1);
    assert_eq!(drag_scroll_step(-0.2), -1);
    assert_eq!(drag_scroll_step(3.5), 4);
    assert_eq!(drag_scroll_step(-3.5), -4);
    assert_eq!(drag_scroll_step(50.0), 8);
    assert_eq!(drag_scroll_step(-50.0), -8);
}

/// #668: the platform modifier is the wrong key to hardwire a zoom to —
/// on a Mac it is ⌘, which people are already holding for something
/// else — so which key zooms is a setting, all the way down to none.
#[test]
fn the_zoom_modifier_is_configurable_and_can_be_turned_off() {
    for os in [
        OperatingSystem::Mac,
        OperatingSystem::Windows,
        OperatingSystem::Linux,
    ] {
        let secondary = if os == OperatingSystem::Mac {
            Modifiers {
                platform: true,
                ..Modifiers::none()
            }
        } else {
            Modifiers::control()
        };
        let alt = Modifiers::alt();
        assert!(zoom_wheel(MouseZoomModifier::Platform, &secondary, os));
        assert!(
            !zoom_wheel(MouseZoomModifier::None, &secondary, os),
            "off is off"
        );
        assert!(!zoom_wheel(MouseZoomModifier::Alt, &secondary, os));
        assert!(zoom_wheel(MouseZoomModifier::Alt, &alt, os));
        assert!(!zoom_wheel(MouseZoomModifier::Platform, &alt, os));
        assert!(zoom_wheel(
            MouseZoomModifier::Ctrl,
            &Modifiers::control(),
            os
        ));
        assert_eq!(
            zoom_wheel(MouseZoomModifier::Platform, &Modifiers::control(), os),
            os != OperatingSystem::Mac,
            "off macOS the platform modifier is Ctrl itself"
        );
        // A second modifier is somebody else's gesture, whichever key is bound.
        let both = Modifiers { shift: true, ..alt };
        assert!(!zoom_wheel(MouseZoomModifier::Alt, &both, os));
        assert!(!zoom_wheel(
            MouseZoomModifier::Platform,
            &Modifiers::none(),
            os
        ));
    }
}

/// A detent is one step however many lines the platform bills it as —
/// macOS calls a single notch five.
#[test]
fn a_wheel_detent_zooms_by_exactly_one_step() {
    assert_eq!(zoom_scroll_steps(4.9, 0., false), (1, 0.));
    assert_eq!(zoom_scroll_steps(-4.9, 0., false), (-1, 0.));
    assert_eq!(zoom_scroll_steps(0., 0., false), (0, 0.));
}

/// A trackpad has no detents, so a flick arrives as a stream of slivers.
/// Paying out a step per sliver would run the font from end to end.
#[test]
fn a_trackpad_flick_adds_up_to_whole_steps() {
    let (mut debt, mut steps) = (0., 0);
    for _ in 0..20 {
        let (s, d) = zoom_scroll_steps(1. / 3., debt, true);
        steps += s;
        debt = d;
    }
    assert_eq!(steps, 2, "twenty thirds of a line is two steps, not twenty");
    assert!(debt > 0., "and the remainder was dropped instead of kept");
}
