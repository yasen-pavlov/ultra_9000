use super::*;

#[test]
fn identifiers_only_resolve_valid_items_from_this_run() {
    let id = identifier("item-12");
    assert_eq!(item_of(&id), Some("item-12"));
    let run = RUN_TOKEN.as_str();
    assert_eq!(item_of("other-run/item-12"), None);
    assert_eq!(item_of(&format!("{}/item-12", std::process::id())), None);
    for malformed in [
        "".to_string(),
        "item-12".to_string(),
        format!("x{run}/item-12"),
        format!("{run}/"),
        format!("{run}/item-"),
        format!("{run}/item-0"),
        format!("{run}/item-x"),
        format!("{run}/item-1/extra"),
    ] {
        assert_eq!(item_of(&malformed), None, "{malformed}");
    }
}

#[cfg(target_os = "linux")]
mod commands {
    use super::*;
    use crate::{
        commands::test_support::{mock_app, Main},
        runtime::test_support::serve,
        workspace::test_support::install,
    };

    #[test]
    fn click_shows_and_selects_live_items_but_only_shows_for_closed_items() {
        let main = Main::acquire();
        serve();
        main.idle();
        let host = install();
        let app = mock_app(true);
        let launch = || {
            let id = workspace::begin_launch(
                "shell".into(),
                "/p".into(),
                "/p".into(),
                None,
                None,
                None,
                None,
            )
            .unwrap();
            host.complete(&id);
            id
        };
        let a = launch();
        let b = launch();
        lifecycle::hide_window(app.handle()).unwrap();
        assert!(!lifecycle::window_visible());
        open_item(app.handle(), Some(&a), Some("activation-token")).unwrap();
        assert!(lifecycle::window_visible());
        assert_eq!(
            workspace::snapshot().unwrap().active_item_id,
            Some(a.clone())
        );

        workspace::close(&a, true).unwrap();
        host.exit(host.key(&a).unwrap(), Some(0));
        assert!(!workspace::has_item(&a).unwrap());
        lifecycle::hide_window(app.handle()).unwrap();
        open_item(app.handle(), Some(&a), None).unwrap();
        assert!(lifecycle::window_visible());
        assert_eq!(
            workspace::snapshot().unwrap().active_item_id,
            Some(b.clone())
        );

        lifecycle::hide_window(app.handle()).unwrap();
        workspace::quit_snapshot().unwrap();
        open_item(app.handle(), Some(&b), None).unwrap();
        assert!(!lifecycle::window_visible());
        assert_eq!(workspace::snapshot().unwrap().active_item_id, Some(b));
    }
}
