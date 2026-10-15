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
