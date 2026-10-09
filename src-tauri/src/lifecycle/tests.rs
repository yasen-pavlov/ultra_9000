use super::*;
use crate::{
    commands::test_support::{mock_app, Main},
    runtime::test_support::serve,
    workspace::{test_support::install, PaletteMode, Sheet},
};
use alacritty_embed::embed::AppKey;
use tauri::test::MockRuntime;
use tokio::sync::watch;

/// Main thread, served worker, fake-host workspace, mock app with its main window and the
/// tray's model channel (as `install_menu_bar_icon` registers it, minus D-Bus).
fn setup() -> (Main, App<MockRuntime>, watch::Receiver<TrayModel>) {
    let main = Main::acquire();
    serve();
    main.idle();
    install();
    WINDOW_SHOWN.store(true, Ordering::Release);
    let app = mock_app(true);
    let tray = crate::tray_linux::register(app.handle(), model(&workspace::snapshot().unwrap()));
    (main, app, tray)
}
fn launch(cwd: &str) -> String {
    workspace::begin_launch(
        "shell".into(),
        cwd.into(),
        cwd.into(),
        None,
        None,
        None,
        None,
    )
    .unwrap()
}
fn active() -> Option<String> {
    workspace::snapshot().unwrap().active_item_id
}
fn sheet() -> Option<Sheet> {
    workspace::snapshot().unwrap().sheet
}
/// The model the tray was last sent, if it changed since the last look.
fn sent(tray: &mut watch::Receiver<TrayModel>) -> Option<TrayModel> {
    tray.has_changed()
        .unwrap()
        .then(|| tray.borrow_and_update().clone())
}

#[test]
fn hide_and_show_track_the_window_in_the_tray_once_per_change() {
    let (_main, app, mut tray) = setup();
    hide_window(app.handle()).unwrap();
    let hidden = sent(&mut tray).expect("hiding updates the tray");
    assert!(!hidden.visible);
    assert_eq!(hidden.header, "Ultra 9000 · hidden · 0 running");
    assert_eq!(hidden.visibility_label(), "Show Ultra 9000");
    refresh_tray(app.handle());
    assert_eq!(sent(&mut tray), None, "an unchanged model is not resent");

    show_window(app.handle()).unwrap();
    let shown = sent(&mut tray).expect("showing updates the tray");
    assert!(shown.visible);
    assert_eq!(shown.header, "Ultra 9000 · 0 running");
    hide_app(app.handle()).unwrap();
    assert!(!sent(&mut tray).unwrap().visible, "Linux hides the window");

    let windowless = mock_app(false);
    assert_eq!(
        show_window(windowless.handle()).unwrap_err(),
        "main window not found"
    );
    assert_eq!(
        hide_window(windowless.handle()).unwrap_err(),
        "main window not found"
    );
    assert!(
        !WINDOW_SHOWN.load(Ordering::Acquire),
        "failures change nothing"
    );
}

#[test]
fn tray_actions_run_later_on_the_main_thread() {
    let (main, app, mut tray) = setup();
    let first = launch("/atlas");
    let second = launch("/meridian");
    assert_eq!(active().as_deref(), Some(second.as_str()));
    hide_window(app.handle()).unwrap();
    sent(&mut tray);

    handle_tray_action(app.handle(), TrayAction::Select(first.clone()));
    assert_eq!(
        active().as_deref(),
        Some(second.as_str()),
        "deferred, not inline"
    );
    main.idle();
    assert_eq!(active(), Some(first));
    assert!(
        sent(&mut tray).unwrap().visible,
        "selecting shows the window"
    );

    handle_tray_action(app.handle(), TrayAction::Hide);
    main.idle();
    assert!(!sent(&mut tray).unwrap().visible);
    handle_tray_action(app.handle(), TrayAction::More);
    main.idle();
    assert!(matches!(
        sheet(),
        Some(Sheet::Palette {
            project_id: None,
            mode: PaletteMode::Open
        })
    ));
    assert!(sent(&mut tray).unwrap().visible);
    handle_tray_action(app.handle(), TrayAction::Hide);
    handle_tray_action(app.handle(), TrayAction::Show);
    main.idle();
    assert!(WINDOW_SHOWN.load(Ordering::Acquire));

    handle_tray_action(app.handle(), TrayAction::Select("item-404".into()));
    main.idle();
    let notices = serde_json::to_value(workspace::snapshot().unwrap().notices).unwrap();
    assert!(
        notices.to_string().contains("item-404"),
        "a vanished item is reported: {notices}"
    );
}

#[test]
fn toggling_notifications_writes_the_setting_through_the_worker() {
    let (main, app, _tray) = setup();
    let before = workspace::notify_needs_you().unwrap();
    handle_tray_action(app.handle(), TrayAction::ToggleNotify);
    main.until("the toggled setting", || {
        workspace::notify_needs_you().unwrap() != before
    });
    handle_tray_action(app.handle(), TrayAction::ToggleNotify);
    main.until("the restored setting", || {
        workspace::notify_needs_you().unwrap() == before
    });
}

#[test]
fn quit_with_live_items_asks_first_and_coalesces_requests() {
    let (main, app, mut tray) = setup();
    let item = launch("/atlas");
    hide_window(app.handle()).unwrap();
    sent(&mut tray);
    request_quit(app.handle());
    request_quit(app.handle());
    assert!(QUIT_PENDING.load(Ordering::Acquire));
    assert!(sheet().is_none(), "the decision runs on the main thread");
    main.idle();
    assert!(!QUIT_PENDING.load(Ordering::Acquire));
    assert!(matches!(sheet(), Some(Sheet::Quit { item_ids }) if item_ids == [item]));
    assert!(
        sent(&mut tray).unwrap().visible,
        "the quit sheet needs the window"
    );
}

#[test]
fn bound_shortcut_keys_are_consumed_and_dispatched_later() {
    let (main, _app, _tray) = setup();
    let router = key_router();
    let key = |ctrl, shift, code| AppKey {
        ctrl,
        shift,
        alt: false,
        logo: false,
        code,
        named: None,
    };
    // Ctrl+Shift+Space (evdev 57) opens the palette on Linux.
    assert!(router(&key(true, true, 57)));
    assert!(sheet().is_none());
    main.idle();
    assert!(matches!(sheet(), Some(Sheet::Palette { .. })));
    assert!(
        !router(&key(false, false, 57)),
        "plain Space reaches the terminal"
    );
    assert!(!router(&key(true, true, 999)), "unknown key code");
}
