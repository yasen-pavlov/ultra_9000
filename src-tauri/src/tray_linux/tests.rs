use super::*;
#[test]
fn shortcuts_are_dbusmenu_properties_not_labels() {
    assert_eq!(
        shortcut_property(Some("Ctrl+Shift+H")),
        vec![vec!["Control", "Shift", "H"]]
    );
    assert!(shortcut_property(None).is_empty());
}
#[test]
fn icons_are_argb_network_byte_order() {
    let bytes = include_bytes!("../../icons/tray/light-attention-20.png");
    let rgba = Image::from_bytes(bytes).unwrap();
    let argb = icon(bytes).unwrap();
    assert_eq!(argb.data.len(), rgba.rgba().len());
    for (a, r) in argb
        .data
        .as_chunks::<4>()
        .0
        .iter()
        .zip(rgba.rgba().as_chunks::<4>().0)
    {
        assert_eq!(*a, [r[3], r[0], r[1], r[2]]);
    }
}

use crate::tray_model::Entry;
use ksni::Tray;
use std::sync::mpsc;

fn model(visible: bool, needs_you: usize, more_running: usize) -> TrayModel {
    let entry = |id: &str, label: &str| Entry {
        id: id.into(),
        label: label.into(),
        detail: "atlas · working".into(),
    };
    TrayModel {
        header: "Ultra 9000 · 4 running".into(),
        entries: vec![
            entry("item-1", "π !  fix_the_bug"),
            entry("item-2", ">_  shell"),
        ],
        more_running,
        visible,
        notify_checked: true,
        needs_you,
        tooltip: "Ultra 9000 · 1 need you".into(),
        hide_shortcut: Some("Ctrl+Shift+H".into()),
        quit_shortcut: Some("Ctrl+Shift+Q".into()),
        dark_icon: false,
    }
}
fn tray(model: TrayModel) -> (LinuxTray, mpsc::Receiver<TrayAction>) {
    let (tx, actions) = mpsc::channel();
    let (light, dark) = icon_sets().unwrap();
    let tray = LinuxTray {
        on_action: Box::new(move |action| tx.send(action).unwrap()),
        model,
        light,
        dark,
    };
    (tray, actions)
}
#[derive(Debug, PartialEq)]
enum Row {
    Item(String, bool, Vec<Vec<String>>),
    Check(String, bool),
    Separator,
}
fn rows(tray: &LinuxTray) -> Vec<Row> {
    tray.menu()
        .into_iter()
        .map(|item| match item {
            MenuItem::Standard(i) => Row::Item(i.label, i.enabled, i.shortcut),
            MenuItem::Checkmark(c) => Row::Check(c.label, c.checked),
            MenuItem::Separator => Row::Separator,
            _ => panic!("unexpected menu item kind"),
        })
        .collect()
}
/// Activate every enabled row top to bottom, recording the dispatched actions.
fn activate_all(tray: &mut LinuxTray, actions: &mpsc::Receiver<TrayAction>) -> Vec<TrayAction> {
    for item in tray.menu() {
        match item {
            MenuItem::Standard(i) if i.enabled => (i.activate)(tray),
            MenuItem::Checkmark(c) => (c.activate)(tray),
            _ => {}
        }
    }
    actions.try_iter().collect()
}
fn item(label: &str, shortcut: &[&str]) -> Row {
    let shortcut = if shortcut.is_empty() {
        vec![]
    } else {
        vec![shortcut.iter().map(|s| s.to_string()).collect()]
    };
    Row::Item(label.into(), true, shortcut)
}
fn same(a: &[Icon], b: &[Icon]) -> bool {
    a.len() == b.len()
        && a.iter()
            .zip(b)
            .all(|(a, b)| (a.width, a.height, &a.data) == (b.width, b.height, &b.data))
}

#[test]
fn visible_menu_lists_items_then_window_notify_and_quit() {
    let (mut tray, actions) = tray(model(true, 1, 2));
    assert_eq!(
        rows(&tray),
        [
            Row::Item("Ultra 9000 · 4 running".into(), false, vec![]),
            item("π !  fix__the__bug", &[]),
            item(">__  shell", &[]),
            item("+ 2 more running", &[]),
            Row::Separator,
            item("Hide Ultra 9000", &["Control", "Shift", "H"]),
            Row::Check(NOTIFY_LABEL.into(), true),
            Row::Separator,
            item(QUIT_LABEL, &["Control", "Shift", "Q"]),
        ]
    );
    assert_eq!(
        activate_all(&mut tray, &actions),
        [
            TrayAction::Select("item-1".into()),
            TrayAction::Select("item-2".into()),
            TrayAction::More,
            TrayAction::Hide,
            TrayAction::ToggleNotify,
            TrayAction::Quit,
        ]
    );
}

#[test]
fn hidden_menu_offers_show_without_the_hide_shortcut() {
    let mut hidden = model(false, 0, 0);
    hidden.notify_checked = false;
    hidden.entries.clear();
    let (mut tray, actions) = tray(hidden);
    assert_eq!(
        rows(&tray),
        [
            Row::Item("Ultra 9000 · 4 running".into(), false, vec![]),
            Row::Separator,
            item("Show Ultra 9000", &[]),
            Row::Check(NOTIFY_LABEL.into(), false),
            Row::Separator,
            item(QUIT_LABEL, &["Control", "Shift", "Q"]),
        ]
    );
    assert_eq!(
        activate_all(&mut tray, &actions),
        [TrayAction::Show, TrayAction::ToggleNotify, TrayAction::Quit]
    );
    // Clicking the icon itself shows the window; the menu opens on right click.
    tray.activate(0, 0);
    assert_eq!(actions.try_iter().collect::<Vec<_>>(), [TrayAction::Show]);
}

#[test]
fn status_and_icon_follow_attention_and_icon_theme() {
    let (light, dark) = icon_sets().unwrap();
    assert_eq!(light.normal.len(), 6);
    assert!(!same(&light.normal, &light.attention));
    assert!(!same(&light.normal, &dark.normal));

    let (mut tray, _) = tray(model(true, 0, 0));
    assert_eq!(tray.id(), "ultra9000");
    assert_eq!(tray.title(), "Ultra 9000");
    assert_eq!(tray.tool_tip().title, "Ultra 9000 · 1 need you");
    assert_eq!(tray.status(), Status::Active);
    assert!(same(&tray.icon_pixmap(), &light.normal));
    assert!(same(&tray.attention_icon_pixmap(), &light.attention));

    tray.model.needs_you = 2;
    assert_eq!(tray.status(), Status::NeedsAttention);
    assert!(same(&tray.icon_pixmap(), &light.attention));

    tray.model.dark_icon = true;
    assert!(same(&tray.icon_pixmap(), &dark.attention));
    assert!(same(&tray.attention_icon_pixmap(), &dark.attention));
    tray.model.needs_you = 0;
    assert!(same(&tray.icon_pixmap(), &dark.normal));
}
