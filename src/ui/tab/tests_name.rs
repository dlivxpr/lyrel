use super::*;

#[test]
fn a_rename_box_left_alone_is_not_a_rename() {
    // The box opens holding the label already on screen, and `Blur`
    // commits — so this is what happens when the user opens it, thinks
    // better of it, and clicks away.
    assert_eq!(
        rename_outcome("api", "api"),
        Rename::Unchanged,
        "an untouched box asked for nothing"
    );
    assert_eq!(
        rename_outcome("  api  ", "api"),
        Rename::Unchanged,
        "whitespace either side is not an edit"
    );
}

#[test]
fn an_emptied_rename_box_gives_the_tab_back_to_its_pane() {
    // The only way to remove a name once set, so it has to survive the
    // comparison above.
    assert_eq!(rename_outcome("", "api"), Rename::Cleared);
    assert_eq!(rename_outcome("   ", "api"), Rename::Cleared);
}

#[test]
fn a_typed_rename_box_names_the_tab() {
    assert_eq!(
        rename_outcome("billing", "api"),
        Rename::Named("billing".into())
    );
    assert_eq!(
        rename_outcome("  billing  ", "api"),
        Rename::Named("billing".into()),
        "the name is stored trimmed"
    );
}
