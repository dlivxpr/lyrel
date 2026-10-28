use super::*;

#[test]
fn the_appearance_hint_round_trips_and_defaults_to_light() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("nested").join(APPEARANCE_FILE);

    assert_eq!(
        read_appearance(&path),
        Appearance { dark: false },
        "a hint nobody has written yet reads as the default preset"
    );

    write_appearance(&path, Appearance { dark: true }).unwrap();
    assert_eq!(read_appearance(&path), Appearance { dark: true });

    write_appearance(&path, Appearance { dark: false }).unwrap();
    assert_eq!(read_appearance(&path), Appearance { dark: false });

    std::fs::write(&path, "not json").unwrap();
    assert_eq!(
        read_appearance(&path),
        Appearance { dark: false },
        "a corrupt hint must not decide the background either"
    );
}
