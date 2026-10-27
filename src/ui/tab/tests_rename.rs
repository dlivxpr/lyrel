use gpui_kit::TestAppContext;

use crate::ui::app::test_window::harness_with_tabs;

#[gpui_kit::test]
fn a_rename_box_opens_with_the_caret_after_the_name(cx: &mut TestAppContext) {
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);

    app.update_in(&mut vcx, |app, window, cx| app.start_rename(0, window, cx));
    vcx.background_executor.run_until_parked();

    app.update(&mut vcx, |app, cx| {
        let input = app
            .renaming
            .as_ref()
            .expect("the rename box is up")
            .input
            .clone();
        let state = input.read(cx);
        let value = state.value().to_string();
        assert!(!value.is_empty(), "the box starts on the current name");
        let end = value.len();
        assert_eq!(
            state.selected_range(),
            end..end,
            "typing has to continue {value:?}, not land in front of it"
        );
    });
}
#[gpui_kit::test]
fn a_rename_rides_out_other_tabs_closing_and_the_strip_reordering(cx: &mut TestAppContext) {
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 3);

    app.update_in(&mut vcx, |app, window, cx| {
        app.start_rename(2, window, cx);
        let target = app.tabs[2].id();
        let input = app.renaming.as_ref().expect("the box is up").input.clone();
        input.update(cx, |s, cx| s.set_value("mine", window, cx));

        // An unrelated close used to throw the half-typed name away
        // (#598).
        app.close_tab_inner(0, true, window, cx);
        assert!(
            app.renaming.is_some(),
            "closing another tab keeps the rename box"
        );

        // So did a drag-reorder — and the index the commit once landed
        // on had by then drifted onto a different tab.
        let order: Vec<usize> = (0..app.tabs.len()).rev().collect();
        app.apply_tab_order(&order, cx);
        assert!(app.renaming.is_some(), "a reorder keeps the rename box");

        app.commit_rename(window, cx);
        let named: Vec<_> = app
            .tabs
            .iter()
            .filter(|t| t.name().as_deref() == Some("mine"))
            .collect();
        assert_eq!(named.len(), 1, "the name landed on exactly one tab");
        assert_eq!(
            named[0].id(),
            target,
            "and that tab is the one the box was opened on"
        );
    });

    // Closing the renaming tab itself still ends the rename.
    app.update_in(&mut vcx, |app, window, cx| {
        app.start_rename(0, window, cx);
        assert!(app.renaming.is_some());
        app.close_tab_inner(0, true, window, cx);
        assert!(app.renaming.is_none(), "losing its own tab closes the box");
    });
}
