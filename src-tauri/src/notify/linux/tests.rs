use super::{Event, Tracker};

fn shown(tracker: &mut Tracker<u64>, id: u32, item: &str, position: u64) {
    assert!(tracker.shown(":1.2", id, item.into(), position));
}

#[test]
fn clicks_after_registration_carry_their_own_token_once() {
    let mut tracker = Tracker::default();
    shown(&mut tracker, 1, "item-a", 10);
    shown(&mut tracker, 2, "item-b", 11);
    assert_eq!(
        tracker.event(":1.2", 1, 12, Event::Token("token".into())),
        None
    );
    assert_eq!(
        tracker.event(":1.2", 2, 13, Event::Action("default".into())),
        Some(("item-b".into(), None))
    );
    assert_eq!(
        tracker.event(":1.2", 1, 14, Event::Action("default".into())),
        Some(("item-a".into(), Some("token".into())))
    );
    assert_eq!(
        tracker.event(":1.2", 1, 15, Event::Action("default".into())),
        None
    );
    assert_eq!(tracker.replaces("item-a"), 0);
}

#[test]
fn unknown_ids_and_signals_before_registration_are_ignored() {
    let mut tracker = Tracker::default();
    assert_eq!(
        tracker.event(":1.2", 1, 5, Event::Action("default".into())),
        None
    );
    shown(&mut tracker, 1, "item-a", 10);
    for event in [
        Event::Token("old".into()),
        Event::Action("default".into()),
        Event::Closed,
    ] {
        assert_eq!(tracker.event(":1.2", 1, 9, event), None);
    }
    assert_eq!(tracker.replaces("item-a"), 1);
    assert_eq!(
        tracker.event(":1.2", 1, 11, Event::Action("default".into())),
        Some(("item-a".into(), None))
    );
}

#[test]
fn replacements_keep_one_id_and_old_closures_do_not_close_the_replacement() {
    let mut tracker = Tracker::default();
    shown(&mut tracker, 1, "item-a", 10);
    assert_eq!(
        tracker.event(":1.2", 1, 11, Event::Token("old".into())),
        None
    );
    shown(&mut tracker, 1, "item-a", 20);
    assert_eq!(tracker.event(":1.2", 1, 19, Event::Closed), None);
    assert_eq!(tracker.replaces("item-a"), 1);
    assert_eq!(tracker.shown.len(), 1);
    shown(&mut tracker, 2, "item-a", 30);
    assert_eq!(tracker.shown.len(), 1);
    assert_eq!(tracker.replaces("item-a"), 2);
    assert_eq!(tracker.event(":1.2", 1, 31, Event::Closed), None);
    assert_eq!(
        tracker.event(":1.2", 2, 32, Event::Action("default".into())),
        Some(("item-a".into(), None))
    );
}

#[test]
fn owner_changes_reset_ids_and_old_replies_and_events_cannot_restore_them() {
    let mut tracker = Tracker::default();
    shown(&mut tracker, 1, "item-a", 10);
    assert!(tracker.owner_changed(Some(":1.3"), 20));
    assert_eq!(tracker.replaces("item-a"), 0);
    assert!(tracker.shown.is_empty());
    assert!(!tracker.shown(":1.2", 1, "item-a".into(), 19));
    assert!(tracker.shown(":1.3", 1, "item-b".into(), 21));
    assert_eq!(
        tracker.event(":1.2", 1, 22, Event::Action("default".into())),
        None
    );
    assert!(
        !tracker.owner_changed(None, 19),
        "queued old owner change cannot erase a newer reply"
    );
    assert_eq!(
        tracker.event(":1.3", 1, 23, Event::Action("default".into())),
        Some(("item-b".into(), None))
    );
}

#[test]
fn non_default_actions_leave_the_notification_and_closed_forgets_it() {
    let mut tracker = Tracker::default();
    shown(&mut tracker, 1, "item-a", 10);
    assert_eq!(
        tracker.event(":1.2", 1, 11, Event::Action("other".into())),
        None
    );
    assert_eq!(tracker.replaces("item-a"), 1);
    assert_eq!(tracker.event(":1.2", 1, 12, Event::Closed), None);
    assert_eq!(tracker.replaces("item-a"), 0);
    assert_eq!(
        tracker.event(":1.2", 1, 13, Event::Action("default".into())),
        None
    );
}

mod dbus {
    use super::super::*;
    use crate::{
        commands::test_support::{mock_app, Main},
        lifecycle,
        runtime::test_support::serve,
        test_support::PrivateBus,
        workspace::{self, test_support::install},
    };
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
    use zbus::zvariant::OwnedValue;

    #[derive(Debug)]
    struct Announcement {
        app: String,
        replaces: u32,
        icon: String,
        summary: String,
        body: String,
        actions: Vec<String>,
        hints: HashMap<String, OwnedValue>,
        timeout: i32,
        id: u32,
    }
    struct Notifications {
        recorded: mpsc::Sender<Announcement>,
        next: AtomicU32,
        fail: Arc<AtomicBool>,
    }
    #[zbus::interface(name = "org.freedesktop.Notifications")]
    impl Notifications {
        // The freedesktop Notify wire signature has eight arguments.
        #[allow(clippy::too_many_arguments)]
        fn notify(
            &self,
            app_name: &str,
            replaces_id: u32,
            app_icon: &str,
            summary: &str,
            body: &str,
            actions: Vec<String>,
            hints: HashMap<String, OwnedValue>,
            expire_timeout: i32,
        ) -> zbus::fdo::Result<u32> {
            let id = if replaces_id == 0 {
                self.next.fetch_add(1, Ordering::Relaxed)
            } else {
                replaces_id
            };
            let rejected = self.fail.load(Ordering::Relaxed);
            self.recorded
                .send(Announcement {
                    app: app_name.into(),
                    replaces: replaces_id,
                    icon: app_icon.into(),
                    summary: summary.into(),
                    body: body.into(),
                    actions,
                    hints,
                    timeout: expire_timeout,
                    id,
                })
                .unwrap();
            if rejected {
                Err(zbus::fdo::Error::Failed(
                    "test server rejected Notify".into(),
                ))
            } else {
                Ok(id)
            }
        }
    }
    fn server(bus: &PrivateBus) -> (Connection, mpsc::Receiver<Announcement>, Arc<AtomicBool>) {
        let (recorded, received) = mpsc::channel();
        let fail = Arc::new(AtomicBool::new(false));
        let connection = Builder::address(bus.address.as_str())
            .unwrap()
            .name(DESTINATION)
            .unwrap()
            .serve_at(
                PATH,
                Notifications {
                    recorded,
                    next: AtomicU32::new(1),
                    fail: fail.clone(),
                },
            )
            .unwrap()
            .build()
            .unwrap();
        (connection, received, fail)
    }
    fn announce(bus: &Bus, item: &str, text: &str) {
        bus.notify(Notification {
            identifier: crate::notify::identifier(item),
            summary: text.into(),
            body: "body as data".into(),
        })
        .unwrap();
    }
    fn action(server: &Connection, id: u32, action: &str) {
        server
            .emit_signal(
                None::<&str>,
                PATH,
                DESTINATION,
                "ActionInvoked",
                &(id, action),
            )
            .unwrap();
    }
    fn received(receiver: &mpsc::Receiver<Announcement>) -> Announcement {
        receiver
            .recv_timeout(Duration::from_secs(5))
            .expect("Notify not received")
    }

    #[test]
    fn private_server_delivers_real_notifications_clicks_replacements_and_restarts() {
        let Some(private) = PrivateBus::start() else {
            return;
        };
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
        let (server, records, fail) = server(&private);
        let handle = app.handle().clone();
        let (observed, clicks) = mpsc::channel();
        let client = Bus::connect(
            Builder::address(private.address.as_str()).unwrap(),
            Arc::new(move |identifier, token| {
                let event = (identifier.to_owned(), token.clone());
                let handle = handle.clone();
                crate::notify::clicked(move || Ok(handle), identifier, token);
                observed.send(event).unwrap();
            }),
        )
        .unwrap();
        announce(&client, &a, "first");
        let first = received(&records);
        assert_eq!(first.app, "Ultra 9000");
        assert_eq!(first.replaces, 0);
        assert!(first.icon.is_empty());
        assert_eq!(first.summary, "first");
        assert_eq!(first.body, "body as data");
        assert_eq!(first.actions, ["default", "Show"]);
        assert_eq!(
            <&str>::try_from(first.hints.get("desktop-entry").unwrap()).unwrap(),
            "ultra9000"
        );
        assert_eq!(first.timeout, -1);
        announce(&client, &a, "replacement");
        let replacement = received(&records);
        assert_eq!(replacement.replaces, first.id);
        assert_eq!(replacement.id, first.id);
        // Ignore non-default actions and malformed/unknown signals without losing the id.
        action(&server, first.id, "other");
        server
            .emit_signal(None::<&str>, PATH, DESTINATION, "Unknown", &(first.id,))
            .unwrap();
        server
            .emit_signal(
                None::<&str>,
                PATH,
                DESTINATION,
                "ActionInvoked",
                &(first.id, 99u32),
            )
            .unwrap();
        server
            .emit_signal(
                None::<&str>,
                PATH,
                DESTINATION,
                "ActivationToken",
                &(first.id, "test-token"),
            )
            .unwrap();
        main.until("activation token", || {
            client
                .tracker()
                .shown
                .get(&first.id)
                .is_some_and(|s| s.token.as_deref() == Some("test-token"))
        });
        lifecycle::hide_window(app.handle()).unwrap();
        action(&server, first.id, "default");
        main.until("notification selects live item", || {
            lifecycle::window_visible()
                && workspace::snapshot().unwrap().active_item_id.as_deref() == Some(a.as_str())
        });
        assert_eq!(
            clicks
                .recv_timeout(Duration::from_secs(5))
                .unwrap()
                .1
                .as_deref(),
            Some("test-token")
        );

        announce(&client, &a, "closed");
        let closing = received(&records);
        server
            .emit_signal(
                None::<&str>,
                PATH,
                DESTINATION,
                "NotificationClosed",
                &(closing.id, 2u32),
            )
            .unwrap();
        main.until("closed notification forgotten", || {
            client.tracker().replaces(&crate::notify::identifier(&a)) == 0
        });
        announce(&client, &a, "fresh");
        assert_eq!(received(&records).replaces, 0);
        // A server failure is returned, not silently treated as delivery.
        fail.store(true, Ordering::Relaxed);
        assert!(client
            .notify(Notification {
                identifier: crate::notify::identifier(&a),
                summary: "rejected".into(),
                body: String::new()
            })
            .is_err());
        received(&records);
        fail.store(false, Ordering::Relaxed);

        server.release_name(DESTINATION).unwrap();
        main.until("old notification owner leaves", || {
            client.tracker().owner.is_none()
        });
        let (new_server, new_records, _) = self::server(&private);
        main.until("new notification owner", || {
            client.tracker().owner.as_deref() == new_server.unique_name().map(|n| n.as_str())
        });
        announce(&client, &a, "after restart");
        let restarted = received(&new_records);
        assert_eq!(restarted.replaces, 0);
        assert_eq!(restarted.id, 1);
        // A late signal from the previous owner cannot open the new owner's reused id.
        action(&server, restarted.id, "default");
        new_server
            .emit_signal(
                None::<&str>,
                PATH,
                DESTINATION,
                "ActivationToken",
                &(restarted.id, "new-token"),
            )
            .unwrap();
        main.until("new owner token", || {
            client
                .tracker()
                .shown
                .get(&restarted.id)
                .is_some_and(|s| s.token.as_deref() == Some("new-token"))
        });
        assert!(clicks.try_recv().is_err());
        workspace::close(&a, true).unwrap();
        host.exit(host.key(&a).unwrap(), Some(0));
        lifecycle::hide_window(app.handle()).unwrap();
        action(&new_server, restarted.id, "default");
        main.until("closed item only shows window", || {
            lifecycle::window_visible()
        });
        assert_eq!(
            workspace::snapshot().unwrap().active_item_id,
            Some(b.clone())
        );
        clicks.recv_timeout(Duration::from_secs(5)).unwrap();
        announce(&client, &b, "quitting");
        let last = received(&new_records);
        lifecycle::hide_window(app.handle()).unwrap();
        workspace::quit_snapshot().unwrap();
        action(&new_server, last.id, "default");
        main.until("quitting click delivered", || clicks.try_recv().is_ok());
        main.idle();
        assert!(!lifecycle::window_visible());
        // Exercise click error reporting without requiring a native runtime.
        crate::notify::clicked::<tauri::test::MockRuntime>(
            || Err("app no longer exists".into()),
            &crate::notify::identifier(&b),
            None,
        );
        main.idle();
        drop(private);
        main.until("disconnected bus observed", || {
            client.tracker().disconnected
        });
    }

    #[test]
    fn fifo_worker_preserves_announcement_order_and_continues_after_a_server_error() {
        let Some(private) = PrivateBus::start() else {
            return;
        };
        let (_server, records, fail) = server(&private);
        let worker = worker(Some(private.address.clone()), Arc::new(|_, _| {})).unwrap();
        enqueue(&worker, "item-1", "first", "one");
        let first = received(&records);
        fail.store(true, Ordering::Relaxed);
        enqueue(&worker, "item-1", "second", "two");
        assert_eq!(received(&records).summary, "second");
        fail.store(false, Ordering::Relaxed);
        enqueue(&worker, "item-1", "third", "three");
        let third = received(&records);
        assert_eq!(third.summary, "third");
        assert_eq!(third.replaces, first.id);
        drop(worker);
        let (dead, receiver) = mpsc::channel();
        drop(receiver);
        enqueue(&dead, "item-1", "cannot send", "");
    }
}
