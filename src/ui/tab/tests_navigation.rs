use super::*;

#[test]
fn mru_puts_the_active_tab_first_and_the_last_one_used_behind_it() {
    // Tab 2 is active; 0 was used most recently before it, then 1.
    assert_eq!(mru_order(&[7, 4, 9], 2), vec![2, 0, 1]);
}

#[test]
fn mru_leads_with_the_active_tab_even_before_its_stamp_lands() {
    assert_eq!(mru_order(&[5, 0, 3], 1), vec![1, 0, 2]);
}

#[test]
fn mru_trails_never_activated_tabs_in_strip_order() {
    assert_eq!(mru_order(&[0, 6, 0, 0], 1), vec![1, 0, 2, 3]);
}

#[test]
fn mru_of_a_windowless_workspace_is_empty() {
    assert!(mru_order(&[], 0).is_empty());
}

#[test]
fn stepping_through_tabs_follows_the_strip_and_wraps() {
    let order = [0, 1, 2];
    assert_eq!(step_in_order(&order, 0, true), Some(1));
    assert_eq!(step_in_order(&order, 2, true), Some(0));
    assert_eq!(step_in_order(&order, 0, false), Some(2));
    assert_eq!(step_in_order(&order, 1, false), Some(0));
    // Pressing it again keeps going — this is not the MRU switcher, which
    // bounces between the last two tabs (#867).
    let mut at = 0;
    let seen: Vec<usize> = (0..4)
        .map(|_| {
            at = step_in_order(&order, at, true).unwrap();
            at
        })
        .collect();
    assert_eq!(seen, vec![1, 2, 0, 1]);
}

#[test]
fn stepping_follows_the_grouped_sidebar_order_not_the_index() {
    // Sidebar groups reorder the rows: index 2 is shown second.
    let order = [0, 2, 1, 3];
    assert_eq!(step_in_order(&order, 0, true), Some(2));
    assert_eq!(step_in_order(&order, 2, true), Some(1));
    assert_eq!(step_in_order(&order, 3, true), Some(0));
    assert_eq!(step_in_order(&order, 0, false), Some(3));
}

#[test]
fn stepping_has_nowhere_to_go_with_one_tab() {
    assert_eq!(step_in_order(&[], 0, true), None);
    assert_eq!(step_in_order(&[0], 0, true), None);
    assert_eq!(step_in_order(&[0], 0, false), None);
}

#[test]
fn moving_a_tab_one_slot_puts_it_where_the_neighbour_was() {
    // A move, not a trade: the neighbour it crossed shifts one slot.
    assert_eq!(one_slot_move(&[0, 1, 2], 3, 1, true), Some(vec![0, 2, 1]));
    assert_eq!(one_slot_move(&[0, 1, 2], 3, 1, false), Some(vec![1, 0, 2]));
}

#[test]
fn a_tab_moved_past_an_end_carries_the_whole_run_with_it() {
    // Through the wrap the tab lands at the far end and the run it
    // crossed shifts one slot — `[c, a, b]`, not `[c, b, a]`.
    assert_eq!(one_slot_move(&[0, 1, 2], 3, 2, true), Some(vec![2, 0, 1]));
    assert_eq!(one_slot_move(&[0, 1, 2], 3, 0, false), Some(vec![1, 2, 0]));
}

#[test]
fn a_move_inside_a_sidebar_group_leaves_the_other_groups_alone() {
    // Tabs 0, 2 and 4 are one group, 1 and 3 another: only the group's
    // own slots are reassigned, and the wrap stays inside it.
    assert_eq!(
        one_slot_move(&[0, 2, 4], 5, 2, true),
        Some(vec![0, 1, 4, 3, 2])
    );
    assert_eq!(
        one_slot_move(&[0, 2, 4], 5, 0, false),
        Some(vec![2, 1, 4, 3, 0])
    );
    assert_eq!(
        one_slot_move(&[1, 3], 5, 3, true),
        Some(vec![0, 3, 2, 1, 4])
    );
}

#[test]
fn a_move_with_nothing_to_move_is_refused() {
    assert_eq!(one_slot_move(&[], 0, 0, true), None);
    assert_eq!(one_slot_move(&[0], 3, 0, true), None);
    assert_eq!(one_slot_move(&[0, 1, 2], 3, 7, true), None);
}
