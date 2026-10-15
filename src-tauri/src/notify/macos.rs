//! UNUserNotificationCenter: notifications come from Ultra 9000 (its name and icon) and a
//! click activates it. macOS asks the user once for permission. The framework needs the
//! app bundle; a bare binary (`tauri dev`, the GUI tests) logs once and sends nothing.
use block2::RcBlock;
use objc2::{
    define_class, msg_send,
    rc::Retained,
    runtime::{Bool, NSObjectProtocol, ProtocolObject},
    AnyThread,
};
use objc2_foundation::{NSBundle, NSError, NSObject, NSString};
use objc2_user_notifications::{
    UNAuthorizationOptions, UNMutableNotificationContent, UNNotification,
    UNNotificationDefaultActionIdentifier, UNNotificationPresentationOptions,
    UNNotificationRequest, UNNotificationResponse, UNUserNotificationCenter,
    UNUserNotificationCenterDelegate,
};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    LazyLock,
};

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "U9KNotificationDelegate"]
    struct Delegate;

    unsafe impl NSObjectProtocol for Delegate {}

    unsafe impl UNUserNotificationCenterDelegate for Delegate {
        /// Notifications are only sent while the main window is not focused, but the app
        /// can still be active (the menu-bar popover): show them then too.
        #[unsafe(method(userNotificationCenter:willPresentNotification:withCompletionHandler:))]
        fn will_present(
            &self,
            _center: &UNUserNotificationCenter,
            _notification: &UNNotification,
            handler: &block2::DynBlock<dyn Fn(UNNotificationPresentationOptions)>,
        ) {
            handler
                .call((UNNotificationPresentationOptions::Banner
                    | UNNotificationPresentationOptions::List,));
        }

        #[unsafe(method(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:))]
        fn did_receive(
            &self,
            _center: &UNUserNotificationCenter,
            response: &UNNotificationResponse,
            handler: &block2::DynBlock<dyn Fn()>,
        ) {
            // SAFETY: a framework constant, immutable for the process lifetime.
            if response
                .actionIdentifier()
                .isEqualToString(unsafe { UNNotificationDefaultActionIdentifier })
            {
                let identifier = response.notification().request().identifier().to_string();
                super::clicked(&identifier, None);
            }
            handler.call(());
        }
    }
);

/// The delegate, retained for the process lifetime (the center holds it weakly). `None`
/// without the app bundle: notifications are disabled then.
struct Installed(Option<Retained<Delegate>>);
// SAFETY: the delegate has no state; the framework may call it from any thread.
unsafe impl Send for Installed {}
unsafe impl Sync for Installed {}
static INSTALLED: LazyLock<Installed> = LazyLock::new(|| {
    if !NSBundle::mainBundle()
        .bundlePath()
        .to_string()
        .ends_with(".app")
    {
        log::warn!("desktop notifications need the app bundle; disabled for this run");
        return Installed(None);
    }
    // SAFETY: a plain NSObject subclass without ivars; `init` is NSObject's.
    let delegate: Retained<Delegate> = unsafe { msg_send![Delegate::alloc(), init] };
    UNUserNotificationCenter::currentNotificationCenter()
        .setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    Installed(Some(delegate))
});

/// At startup, before any notification: route clicks to the app.
pub fn install() {
    LazyLock::force(&INSTALLED);
}

pub(super) fn send(item: &str, summary: &str, body: &str) {
    if INSTALLED.0.is_none() {
        return;
    }
    let identifier = super::identifier(item);
    let (summary, body) = (summary.to_owned(), body.to_owned());
    let center = UNUserNotificationCenter::currentNotificationCenter();
    // Prompts only the first time; afterwards it reports the user's choice at once.
    let posted = RcBlock::new(move |granted: Bool, _error: *mut NSError| {
        if granted.as_bool() {
            post(&identifier, &summary, &body);
        } else {
            static DENIED: AtomicBool = AtomicBool::new(false);
            if !DENIED.swap(true, Ordering::Relaxed) {
                log::warn!("notifications are turned off for Ultra 9000 in System Settings");
            }
        }
    });
    center
        .requestAuthorizationWithOptions_completionHandler(UNAuthorizationOptions::Alert, &posted);
}

/// The request identifier is the item's, so a newer notification replaces the older one.
fn post(identifier: &str, summary: &str, body: &str) {
    let content = UNMutableNotificationContent::new();
    content.setTitle(&NSString::from_str(summary));
    content.setBody(&NSString::from_str(body));
    let request = UNNotificationRequest::requestWithIdentifier_content_trigger(
        &NSString::from_str(identifier),
        &content,
        None,
    );
    let added = RcBlock::new(|error: *mut NSError| {
        // SAFETY: the framework passes nil or a valid NSError for the call's duration.
        if let Some(error) = unsafe { error.as_ref() } {
            log::warn!(
                "desktop notification failed: {}",
                error.localizedDescription()
            );
        }
    });
    UNUserNotificationCenter::currentNotificationCenter()
        .addNotificationRequest_withCompletionHandler(&request, Some(&added));
}
