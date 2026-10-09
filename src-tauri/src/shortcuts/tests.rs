use super::*;
#[test]
fn platform_defaults() {
    for p in [Platform::Linux, Platform::MacOs] {
        let m = Keymap::defaults(p);
        assert_eq!(m.all().len(), 26);
        for (c, _) in &m.bindings {
            assert!(!never_captured(p, c));
        }
        assert!(m.all().iter().all(|(_, l)| l.is_some()));
    }
    let m = Keymap::defaults(Platform::Linux);
    let c = parse_chord("Ctrl+PageUp").unwrap();
    assert_eq!(m.lookup(c.mods, c.key), Some(Action::ItemPrevious));
}
#[test]
fn physical_codes() {
    assert_eq!(Key::from_evdev(19), Some(Key::R));
    assert_eq!(Key::from_evdev(51), Some(Key::Comma));
    assert_eq!(Key::from_mac_keycode(33), Some(Key::BracketLeft));
    assert_eq!(Key::from_mac_keycode(23), Some(Key::Digit5));
    assert_eq!(Key::from_mac_keycode(22), Some(Key::Digit6));
    assert!(Key::from_evdev(255).is_none());
}
#[test]
fn override_validation() {
    let t: toml::Table = "[bindings]\npalette='Ctrl+Shift+X'\nclose='Ctrl+Shift+C'\nunknown='bad'"
        .parse()
        .unwrap();
    let (m, e) = Keymap::with_overrides(Platform::Linux, Some(&t));
    assert_eq!(e.len(), 2);
    assert_eq!(m.label(Action::Palette).as_deref(), Some("Ctrl+Shift+X"));
    assert_eq!(m.label(Action::Close).as_deref(), Some("Ctrl+Shift+W"));
    assert!(parse_chord("Ctrl+Ctrl+X").is_err());
    assert!(never_captured(
        Platform::Linux,
        &parse_chord("Super+Shift+X").unwrap()
    ));
    assert!(never_captured(
        Platform::MacOs,
        &parse_chord("Cmd+C").unwrap()
    ));
}
#[test]
fn overrides_can_move_unbind_and_swap_without_order_dependence() {
    for input in [
        "[bindings]\npalette=''\nclose='Ctrl+Shift+Space'",
        "[bindings]\npalette='Ctrl+Shift+W'\nclose='Ctrl+Shift+Space'",
    ] {
        let table = input.parse().unwrap();
        let (map, errors) = Keymap::with_overrides(Platform::Linux, Some(&table));
        assert!(errors.is_empty(), "{errors:?}");
        let chord = parse_chord("Ctrl+Shift+Space").unwrap();
        assert_eq!(map.lookup(chord.mods, chord.key), Some(Action::Close));
    }
    let table = "[bindings]\npalette='Ctrl+Shift+W'\nclose='Ctrl+Shift+C'"
        .parse()
        .unwrap();
    let (map, errors) = Keymap::with_overrides(Platform::Linux, Some(&table));
    assert_eq!(errors.len(), 2);
    assert_eq!(map.label(Action::Close).as_deref(), Some("Ctrl+Shift+W"));
    assert_eq!(
        map.label(Action::Palette).as_deref(),
        Some("Ctrl+Shift+Space")
    );
}
#[test]
fn action_wire_names_round_trip_and_reject_unknown() {
    for action in Action::all() {
        let json = serde_json::to_value(action).unwrap();
        assert_eq!(json, action.name());
        assert_eq!(serde_json::from_value::<Action>(json).unwrap(), action);
    }
    assert_eq!(serde_json::to_value(Action::Item(3)).unwrap(), "item-3");
    assert!(serde_json::from_value::<Action>("item-10".into()).is_err());
    assert!(serde_json::from_value::<Action>("explode".into()).is_err());
    assert!(serde_json::from_value::<Action>(7.into()).is_err());
}
#[test]
fn malformed_override_entries_are_reported_and_defaults_kept() {
    let table: toml::Table = r#"
        colors = 1
        [bindings]
        palette = 5
        close = "Hyper+Shift+X"
        settings = "Ctrl+Shift+Nope"
        hide = "Ctrl+Shift+Alt+X"
    "#
    .parse()
    .unwrap();
    let (map, mut errors) = Keymap::with_overrides(Platform::Linux, Some(&table));
    errors.sort();
    assert_eq!(
        errors,
        [
            "close: unknown modifier: Hyper",
            "palette: expected chord string",
            "settings: unknown key: Nope",
            "unknown keys table: colors",
        ]
    );
    assert_eq!(
        map.label(Action::Palette).as_deref(),
        Some("Ctrl+Shift+Space")
    );
    assert_eq!(map.label(Action::Close).as_deref(), Some("Ctrl+Shift+W"));
    assert_eq!(map.label(Action::Settings).as_deref(), Some("Ctrl+Shift+,"));
    // Linux labels name Alt (macOS says Option) after Shift.
    assert_eq!(map.label(Action::Hide).as_deref(), Some("Ctrl+Shift+Alt+X"));

    let table: toml::Table = "bindings = 'Ctrl+Shift+X'".parse().unwrap();
    let (map, errors) = Keymap::with_overrides(Platform::Linux, Some(&table));
    assert_eq!(errors, ["bindings: expected table"]);
    assert_eq!(
        map.all(),
        Keymap::defaults(Platform::Linux).all(),
        "a malformed bindings value changes nothing"
    );
    assert!(Keymap::with_overrides(Platform::MacOs, None).1.is_empty());
    let table: toml::Table = "colors = 1".parse().unwrap();
    let (map, errors) = Keymap::with_overrides(Platform::Linux, Some(&table));
    assert_eq!(errors, ["unknown keys table: colors"]);
    assert_eq!(map.all(), Keymap::defaults(Platform::Linux).all());
}
