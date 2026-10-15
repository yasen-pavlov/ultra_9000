use super::*;
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc,
};

#[test]
fn the_delegate_presents_banners_and_lists_and_completes_ignored_actions() {
    let delegate = delegate();
    let called = Arc::new(AtomicUsize::new(0));
    let observed = called.clone();
    let presented = RcBlock::new(move |options: UNNotificationPresentationOptions| {
        assert_eq!(
            options,
            UNNotificationPresentationOptions::Banner | UNNotificationPresentationOptions::List
        );
        observed.fetch_add(1, Ordering::Relaxed);
    });
    // SAFETY: the delegate accepts absent unused center/notification arguments; the
    // real completion block remains live throughout the synchronous Objective-C call.
    unsafe {
        let _: () = msg_send![&*delegate,
            userNotificationCenter: None::<&UNUserNotificationCenter>,
            willPresentNotification: None::<&UNNotification>,
            withCompletionHandler: &*presented
        ];
    }
    assert_eq!(called.load(Ordering::Relaxed), 1);
    let observed = called.clone();
    let completed = RcBlock::new(move || {
        observed.fetch_add(1, Ordering::Relaxed);
    });
    let request = request(&crate::notify::identifier("item-1"), "summary", "body");
    delegate.respond(&NSString::from_str("dismiss"), &request, &completed);
    assert_eq!(called.load(Ordering::Relaxed), 2);
}

#[test]
fn notification_content_is_data_and_delivery_requires_authorization() {
    let content = request(
        "this-run/item-1",
        "Title \"'\\",
        "Body: display notification; ☃",
    );
    assert_eq!(content.identifier().to_string(), "this-run/item-1");
    assert_eq!(content.content().title().to_string(), "Title \"'\\");
    assert_eq!(
        content.content().body().to_string(),
        "Body: display notification; ☃"
    );
    assert!(content.trigger().is_none());
    let sent = AtomicBool::new(false);
    authorized(Bool::NO, || panic!("denied permission must not send"));
    authorized(Bool::YES, || sent.store(true, Ordering::Relaxed));
    assert!(sent.load(Ordering::Relaxed));
}

#[test]
fn an_unbundled_test_binary_installs_no_center_and_sends_nothing() {
    assert!(!NSBundle::mainBundle()
        .bundlePath()
        .to_string()
        .ends_with(".app"));
    install();
    assert!(INSTALLED.0.is_none());
    send(
        "item-1",
        "unbundled",
        "must not request permission or call the center",
    );
}
