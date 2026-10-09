//! AppKit pieces of the application lifecycle that Tauri does not expose.

use std::cell::Cell;
use std::ffi::{c_char, c_void};
use std::path::Path;
use std::ptr::NonNull;
use std::rc::Rc;
use std::sync::OnceLock;

use alacritty_embed::embed::{self, AppKey};
use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Sel};
use objc2::{msg_send, sel, MainThreadMarker};
use objc2_app_kit::{
    NSApplication, NSEvent, NSEventMask, NSEventType, NSModalResponseOK, NSOpenPanel, NSView,
    NSWindow,
};
use objc2_foundation::{NSString, NSURL};

/// `NSApplicationTerminateReply` values.
const NS_TERMINATE_CANCEL: usize = 0;
const NS_TERMINATE_NOW: usize = 1;

mod ffi {
    use std::ffi::{c_char, c_void};

    use objc2::runtime::Sel;

    #[repr(C)]
    pub struct DispatchObject {
        _private: [u8; 0],
    }

    pub type DispatchFunction = extern "C" fn(*mut c_void);

    extern "C" {
        static _dispatch_main_q: DispatchObject;

        pub fn dispatch_async_f(
            queue: *const DispatchObject,
            context: *mut c_void,
            work: DispatchFunction,
        );

        pub fn object_getClass(object: *const c_void) -> *mut c_void;

        pub fn class_addMethod(
            class: *mut c_void,
            name: Sel,
            imp: *const c_void,
            types: *const c_char,
        ) -> u8;
    }

    pub fn main_queue() -> *const DispatchObject {
        std::ptr::addr_of!(_dispatch_main_q)
    }
}

pub type Task = Box<dyn FnOnce() + Send>;

/// Main window handle used for focus decisions.
pub type MainWindow = Retained<NSWindow>;

/// Run `task` on the main thread after the current AppKit callback returns.
///
/// Used to leave menu tracking, key monitors and WebKit IPC callbacks before showing a modal
/// alert or mutating the workspace.
pub fn defer_on_main(task: Task) {
    extern "C" fn run(context: *mut c_void) {
        let task = unsafe { Box::from_raw(context.cast::<Task>()) };
        task();
    }

    let context = Box::into_raw(Box::new(task)).cast::<c_void>();
    unsafe { ffi::dispatch_async_f(ffi::main_queue(), context, run) };
}

thread_local! {
    static MODAL_ACTIVE: Cell<bool> = const { Cell::new(false) };
}

/// Held while one of the app's modal decisions (quit, stop, folder picker) is open.
///
/// A second confirmation cannot start while one is open, so destructive decisions never
/// nest; callers revalidate their target after the modal returned.
pub struct ModalGuard(());

impl ModalGuard {
    /// Reserve the modal slot; `None` while another modal decision is open.
    pub fn acquire() -> Option<Self> {
        MODAL_ACTIVE.with(|active| (!active.replace(true)).then_some(Self(())))
    }

    pub fn is_held() -> bool {
        MODAL_ACTIVE.with(Cell::get)
    }
}

impl Drop for ModalGuard {
    fn drop(&mut self) {
        MODAL_ACTIVE.with(|active| active.set(false));
    }
}

fn main_thread() -> MainThreadMarker {
    MainThreadMarker::new().expect("AppKit call off the main thread")
}

/// Bring the app to the front, e.g. before a modal started from the menu-bar icon.
fn activate(mtm: MainThreadMarker) {
    #[allow(deprecated)]
    NSApplication::sharedApplication(mtm).activateIgnoringOtherApps(true);
}

/// Native folder-only picker starting at `initial` (if it is a directory).
///
/// Returns the chosen directory, `None` on cancel.
pub fn pick_directory(initial: Option<&Path>) -> Option<String> {
    let mtm = main_thread();
    activate(mtm);

    let panel = NSOpenPanel::openPanel(mtm);
    panel.setCanChooseDirectories(true);
    panel.setCanChooseFiles(false);
    panel.setAllowsMultipleSelection(false);
    panel.setCanCreateDirectories(true);
    panel.setPrompt(Some(&NSString::from_str("Choose")));
    panel.setMessage(Some(&NSString::from_str(
        "Choose the session's working directory",
    )));
    if let Some(initial) = initial.and_then(Path::to_str) {
        let url = NSURL::fileURLWithPath_isDirectory(&NSString::from_str(initial), true);
        panel.setDirectoryURL(Some(&url));
    }

    if panel.runModal() != NSModalResponseOK {
        return None;
    }

    let urls = panel.URLs();
    let url = urls.firstObject()?;
    url.path().map(|path| path.to_string())
}

/// # Safety
/// `view` is the live WKWebView, used only on the AppKit main thread.
pub unsafe fn focus_webview(view: *mut c_void) {
    let view = unsafe { &*view.cast::<NSView>() };
    if let Some(window) = view.window() {
        window.makeFirstResponder(Some(view));
    }
}

/// Hide the application (Cmd+H), like `-[NSApplication hide:]`.
pub fn hide_application() {
    NSApplication::sharedApplication(main_thread()).hide(None);
}

/// The window containing `view`, if any.
///
/// # Safety
///
/// `view` must point to a live `NSView`.
pub unsafe fn window_of_view(view: *mut c_void) -> Option<Retained<NSWindow>> {
    let view = unsafe { Retained::retain(view.cast::<NSView>()) }?;
    view.window()
}

/// Whether `window` is actually on screen for the user: visible, not minimized, app not
/// hidden. Keyboard focus is only handed to a terminal while this holds.
pub fn window_presentable(window: &MainWindow) -> bool {
    window.isVisible() && !window.isMiniaturized() && !application_hidden()
}

pub fn application_hidden() -> bool {
    let app = NSApplication::sharedApplication(main_thread());
    app.isHidden()
}

/// Route physical keys before either the terminal, web control or native menu sees them.
/// The same router is the terminal's key-equivalent fallback; consumed events never reach
/// that fallback, so each chord executes once. Router actions must defer mutations.
pub fn install_key_router(window: Retained<NSWindow>, router: Rc<dyn Fn(&AppKey) -> bool>) {
    let filter_router = router.clone();
    embed::set_application_shortcut_filter(Box::new(move |key| {
        let app = NSApplication::sharedApplication(main_thread());
        !ModalGuard::is_held() && app.modalWindow().is_none() && filter_router(key)
    }));
    let block: RcBlock<dyn Fn(NonNull<NSEvent>) -> *mut NSEvent> =
        RcBlock::new(move |event: NonNull<NSEvent>| -> *mut NSEvent {
            let event_ref = unsafe { event.as_ref() };
            let mtm = main_thread();
            let app = NSApplication::sharedApplication(mtm);
            let in_window = event_ref.window(mtm).is_some_and(|event_window| {
                Retained::as_ptr(&event_window) == Retained::as_ptr(&window)
            });
            if !in_window
                || ModalGuard::is_held()
                || app.modalWindow().is_some()
                || event_ref.r#type() != NSEventType::KeyDown
            {
                return event.as_ptr();
            }
            // SAFETY: AppKit passes a live event to the monitor on the main thread.
            if router(&unsafe { embed::app_key_from_macos_event(event.cast()) }) {
                std::ptr::null_mut()
            } else {
                event.as_ptr()
            }
        });
    let monitor = unsafe {
        NSEvent::addLocalMonitorForEventsMatchingMask_handler(NSEventMask::KeyDown, &block)
    };
    match monitor {
        Some(monitor) => std::mem::forget(monitor), // App lifetime.
        None => log::error!("Unable to install the application key router"),
    }
}

/// Decides an AppKit-initiated termination (Dock "Quit", logout, `terminate:`).
static TERMINATE_HANDLER: OnceLock<fn() -> bool> = OnceLock::new();

extern "C" fn application_should_terminate(
    _this: *mut AnyObject,
    _cmd: Sel,
    _sender: *mut AnyObject,
) -> usize {
    match TERMINATE_HANDLER.get() {
        Some(handler) if !handler() => NS_TERMINATE_CANCEL,
        _ => NS_TERMINATE_NOW,
    }
}

/// Route `applicationShouldTerminate:` through `handler`.
///
/// tao's application delegate does not implement it, so without this hook Dock "Quit" and
/// logout would terminate immediately without confirmation. `handler` returns whether the
/// termination may proceed (after it stopped managed sessions).
pub fn install_terminate_hook(handler: fn() -> bool) {
    let mtm = main_thread();
    let app = NSApplication::sharedApplication(mtm);

    if TERMINATE_HANDLER.set(handler).is_err() {
        log::warn!("Terminate hook already installed");
        return;
    }

    unsafe {
        let delegate: *mut AnyObject = msg_send![&*app, delegate];
        if delegate.is_null() {
            log::warn!("No application delegate; system quit requests will not be confirmed");
            return;
        }

        let class = ffi::object_getClass(delegate.cast_const().cast());
        let types = c"Q@:@";
        let added = ffi::class_addMethod(
            class,
            sel!(applicationShouldTerminate:),
            application_should_terminate as *const c_void,
            types.as_ptr() as *const c_char,
        );
        if added == 0 {
            log::warn!(
                "Application delegate already implements applicationShouldTerminate:; system quit \
                 requests will not be confirmed"
            );
            return;
        }

        // NSApplication caches which optional delegate methods exist when the delegate is
        // set, so set it again to pick up the new method.
        let _: () = msg_send![&*app, setDelegate: std::ptr::null_mut::<AnyObject>()];
        let _: () = msg_send![&*app, setDelegate: delegate];
    }
}
