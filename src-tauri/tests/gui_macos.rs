//! macOS GUI integration test on the real Wry runtime and AppKit's main thread.
//!
//! Runs the production startup (locale, application menu, terminate hook, main window,
//! terminal host, tray) and checks the window's real AppKit state, hide-on-close and the Dock
//! reopen that restores it, then runs the production folder picker (`NSOpenPanel` modal) and
//! aborts it from a run-loop timer once it is the application's modal window. Starts no
//! session. Needs a window server session; without one it is skipped, unless
//! `U9K_REQUIRE_GUI_TESTS=1` makes that a failure.

#[cfg(target_os = "macos")]
fn main() -> std::process::ExitCode {
    macos::main()
}

#[cfg(not(target_os = "macos"))]
fn main() {}

#[cfg(target_os = "macos")]
mod macos {
    use std::cell::{Cell, RefCell};
    use std::ffi::{c_void, CStr};
    use std::future::Future;
    use std::pin::Pin;
    use std::process::ExitCode;
    use std::rc::Rc;
    use std::task::{Context, Poll, Waker};
    use std::time::{Duration, Instant};

    use objc2::rc::Retained;
    use objc2::{msg_send, sel, MainThreadMarker};
    use objc2_app_kit::{
        NSApplication, NSEvent, NSEventModifierFlags, NSEventType, NSWindow, NSWindowButton,
        NSWindowStyleMask, NSWindowTitleVisibility,
    };
    use objc2_foundation::{NSPoint, NSString};
    use tauri::{AppHandle, Manager, RunEvent, WebviewWindow};
    use ultra9000_app::testing;

    /// Waits on AppKit (show, hide, modal panel) give up after this.
    const TIMEOUT: Duration = Duration::from_secs(30);
    /// After the window first shows, time for the web view to settle before startup is judged.
    const SETTLE: Duration = Duration::from_secs(2);
    /// Interval of the scenario's and the modal check's main-thread polling.
    const POLL: Duration = Duration::from_millis(20);
    /// A hung AppKit step (e.g. a modal nobody aborts) fails the test instead of the CI job.
    const WATCHDOG: Duration = Duration::from_secs(240);

    #[repr(C)]
    struct DispatchQueue {
        _private: [u8; 0],
    }

    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGSessionCopyCurrentDictionary() -> *const c_void;
    }

    #[repr(C)]
    struct CFRunLoopTimerContext {
        version: isize,
        info: *mut c_void,
        retain: *const c_void,
        release: *const c_void,
        copy_description: *const c_void,
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        static kCFRunLoopCommonModes: *const c_void;
        fn CFRelease(cf: *const c_void);
        fn CFAbsoluteTimeGetCurrent() -> f64;
        fn CFRunLoopGetMain() -> *mut c_void;
        fn CFRunLoopTimerCreate(
            allocator: *const c_void,
            fire_date: f64,
            interval: f64,
            flags: usize,
            order: isize,
            callout: extern "C" fn(timer: *mut c_void, info: *mut c_void),
            context: *mut CFRunLoopTimerContext,
        ) -> *mut c_void;
        fn CFRunLoopAddTimer(run_loop: *mut c_void, timer: *mut c_void, mode: *const c_void);
        fn CFRunLoopTimerInvalidate(timer: *mut c_void);
    }

    extern "C" {
        static _dispatch_main_q: DispatchQueue;
        fn dispatch_time(when: u64, delta: i64) -> u64;
        fn dispatch_after_f(
            when: u64,
            queue: *const DispatchQueue,
            context: *mut c_void,
            work: extern "C" fn(*mut c_void),
        );
    }

    /// `DISPATCH_TIME_NOW`.
    const DISPATCH_TIME_NOW: u64 = 0;

    type Task = Pin<Box<dyn Future<Output = ()>>>;

    thread_local! {
        /// The scenario, polled on the main queue until it completes.
        static TASK: RefCell<Option<Task>> = const { RefCell::new(None) };
        static PANEL_DEADLINE: Cell<Option<Instant>> = const { Cell::new(None) };
        /// Whether the modal window was a visible panel when it was aborted.
        static MODAL_PANEL_VISIBLE: Cell<bool> = const { Cell::new(false) };
    }

    /// Exit with a failure if the test has not finished by `WATCHDOG`. Runs on its own thread,
    /// so it fires even when the main thread is stuck in AppKit.
    fn start_watchdog() {
        std::thread::spawn(|| {
            std::thread::sleep(WATCHDOG);
            eprintln!("gui_macos: did not finish within {WATCHDOG:?}");
            std::process::exit(1);
        });
    }

    /// Whether this process can reach a window server (a logged-in GUI session).
    fn window_server_session() -> bool {
        // SAFETY: Returns a retained CFDictionary or null; released below.
        let session = unsafe { CGSessionCopyCurrentDictionary() };
        if session.is_null() {
            return false;
        }
        // SAFETY: `session` is the non-null object returned with +1 retain count above.
        unsafe { CFRelease(session) };
        true
    }

    pub fn main() -> ExitCode {
        if !window_server_session() {
            if std::env::var("U9K_REQUIRE_GUI_TESTS").as_deref() == Ok("1") {
                eprintln!("gui_macos: no window server session but U9K_REQUIRE_GUI_TESTS=1");
                return ExitCode::FAILURE;
            }
            println!("gui_macos: skipped, no window server session");
            return ExitCode::SUCCESS;
        }
        if MainThreadMarker::new().is_none() {
            eprintln!("gui_macos: not on the main thread");
            return ExitCode::FAILURE;
        }
        start_watchdog();

        let report = Rc::new(Report::default());
        let mut started = false;
        let scenario_report = report.clone();
        let code = testing::app().run_return(move |app, event| {
            if matches!(event, RunEvent::Ready) && !started {
                started = true;
                let app = app.clone();
                let report = scenario_report.clone();
                spawn(async move {
                    scenario(&app, &report).await;
                    app.exit(report.exit_code());
                });
            }
            if matches!(event, RunEvent::Reopen { .. }) {
                scenario_report.reopened.set(true);
            }
            testing::on_run_event(app, event);
        });

        report.check(report.completed.get(), "scenario ran to its end");
        let failures = report.failures.borrow().len();
        println!(
            "gui_macos: {} checks, {failures} failed",
            report.checks.get()
        );
        if code == 0 && failures == 0 {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        }
    }

    #[derive(Default)]
    struct Report {
        checks: Cell<usize>,
        failures: RefCell<Vec<String>>,
        /// A Dock reopen reached the run-loop handler.
        reopened: Cell<bool>,
        completed: Cell<bool>,
    }

    impl Report {
        fn check(&self, ok: bool, what: &str) {
            self.checks.set(self.checks.get() + 1);
            if ok {
                println!("ok - {what}");
            } else {
                println!("FAIL - {what}");
                self.failures.borrow_mut().push(what.to_owned());
            }
        }

        fn exit_code(&self) -> i32 {
            i32::from(!self.failures.borrow().is_empty())
        }
    }

    /// Run `work` on the main queue after `delay`. The main queue is serviced by the app's run
    /// loop, including the modal panel's run loop mode.
    fn after(delay: Duration, work: extern "C" fn(*mut c_void)) {
        let nanos = i64::try_from(delay.as_nanos()).unwrap_or(i64::MAX);
        // SAFETY: The main queue is a process-lifetime static; `work` ignores its (null)
        // context.
        unsafe {
            dispatch_after_f(
                dispatch_time(DISPATCH_TIME_NOW, nanos),
                std::ptr::addr_of!(_dispatch_main_q),
                std::ptr::null_mut(),
                work,
            );
        }
    }

    /// Drive `task` on the main queue; it polls its own conditions, so the waker is unused.
    fn spawn(task: impl Future<Output = ()> + 'static) {
        TASK.with(|slot| *slot.borrow_mut() = Some(Box::pin(task)));
        after(Duration::ZERO, poll_task);
    }

    extern "C" fn poll_task(_context: *mut c_void) {
        let Some(mut task) = TASK.with(|slot| slot.borrow_mut().take()) else {
            return;
        };
        let mut context = Context::from_waker(Waker::noop());
        if task.as_mut().poll(&mut context).is_pending() {
            TASK.with(|slot| *slot.borrow_mut() = Some(task));
            after(POLL, poll_task);
        }
    }

    /// Completes once `deadline` has passed (checked whenever the scenario is polled).
    struct Sleep(Instant);

    impl Future for Sleep {
        type Output = ();

        fn poll(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<()> {
            if Instant::now() >= self.0 {
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        }
    }

    fn sleep(duration: Duration) -> Sleep {
        Sleep(Instant::now() + duration)
    }

    /// Poll `ready` while the app's run loop keeps running until it holds.
    async fn wait_for(report: &Report, what: &str, ready: impl Fn() -> bool) -> bool {
        let deadline = Instant::now() + TIMEOUT;
        while !ready() {
            if Instant::now() > deadline {
                report.check(false, &format!("{what} within {TIMEOUT:?}"));
                return false;
            }
            sleep(POLL).await;
        }
        true
    }

    async fn scenario(app: &AppHandle, report: &Report) {
        let mtm = MainThreadMarker::new().expect("the main queue runs on the main thread");
        let Some(window) = app.get_webview_window(testing::MAIN_WINDOW) else {
            report.check(false, "setup created the main window");
            return;
        };
        // SAFETY: Tauri returns the live NSWindow of the main window; retained for this scope.
        let ns_window = window
            .ns_window()
            .ok()
            .and_then(|ptr| unsafe { Retained::retain(ptr.cast::<NSWindow>()) });
        let Some(ns_window) = ns_window else {
            report.check(false, "main window has an NSWindow");
            return;
        };

        if !wait_for(report, "main window visible", || ns_window.isVisible()).await {
            return;
        }
        sleep(SETTLE).await;
        startup(report, mtm, &window, &ns_window);

        // A close request hides the window; the app and its items stay alive.
        report.check(window.close().is_ok(), "close request accepted");
        if !wait_for(report, "window hidden on close request", || {
            !ns_window.isVisible()
        })
        .await
        {
            return;
        }
        report.check(
            app.get_webview_window(testing::MAIN_WINDOW).is_some(),
            "close request keeps the main window (hidden, not destroyed)",
        );
        report.check(
            window.is_visible().ok() == Some(false),
            "Tauri reports the window hidden",
        );

        // A Dock icon click restores it.
        report.check(dock_reopen(mtm), "application delegate handles Dock reopen");
        if !wait_for(report, "window shown again on Dock reopen", || {
            ns_window.isVisible()
        })
        .await
        {
            return;
        }
        report.check(
            report.reopened.get(),
            "Dock reopen reaches the run-loop handler",
        );
        report.check(
            testing::window_presentable(&ns_window),
            "reopened window is presentable",
        );

        // Cmd+H (physical H) in the main window goes through the application key router's
        // local event monitor to the Hide action, which hides the application.
        report.check(
            post_key_down(mtm, &ns_window, KEY_H, "h"),
            "Cmd+H key event posted",
        );
        if wait_for(
            report,
            "Cmd+H hides the application",
            testing::application_hidden,
        )
        .await
        {
            report.check(true, "Cmd+H routed to Hide hides the application");
            NSApplication::sharedApplication(mtm).unhide(None);
            if !wait_for(report, "application shown again", || {
                !testing::application_hidden() && ns_window.isVisible()
            })
            .await
            {
                return;
            }
        }

        folder_picker(report, mtm);
        report.completed.set(true);
    }

    /// `kVK_ANSI_H`, the physical key the Hide shortcut (Cmd+H) is bound to.
    const KEY_H: u16 = 4;

    /// Queue a Cmd+`characters` key-down for `window` on the application's event queue, so it is
    /// dequeued (and seen by local event monitors) like a real key press.
    fn post_key_down(
        mtm: MainThreadMarker,
        window: &NSWindow,
        key_code: u16,
        characters: &str,
    ) -> bool {
        let characters = NSString::from_str(characters);
        let Some(event) = NSEvent::keyEventWithType_location_modifierFlags_timestamp_windowNumber_context_characters_charactersIgnoringModifiers_isARepeat_keyCode(
            NSEventType::KeyDown,
            NSPoint::new(0., 0.),
            NSEventModifierFlags::Command,
            0.,
            window.windowNumber(),
            None,
            &characters,
            &characters,
            false,
            key_code,
        ) else {
            return false;
        };
        NSApplication::sharedApplication(mtm).postEvent_atStart(&event, false);
        true
    }

    fn startup(
        report: &Report,
        mtm: MainThreadMarker,
        window: &WebviewWindow,
        ns_window: &Retained<NSWindow>,
    ) {
        report.check(
            window.title().ok().as_deref() == Some("Ultra 9000"),
            "Tauri title is \"Ultra 9000\"",
        );
        report.check(
            ns_window.title().to_string() == "Ultra 9000",
            "NSWindow title is \"Ultra 9000\"",
        );
        report.check(
            window.is_decorated().ok() == Some(true),
            "Tauri reports a system-decorated window",
        );
        report.check(
            ns_window
                .styleMask()
                .contains(NSWindowStyleMask::Titled | NSWindowStyleMask::FullSizeContentView),
            "NSWindow is titled (AppKit rounds its corners) with a full-size content view",
        );
        report.check(
            ns_window.titlebarAppearsTransparent(),
            "NSWindow titlebar is transparent",
        );
        report.check(
            ns_window.titleVisibility() == NSWindowTitleVisibility::Hidden,
            "NSWindow title text is hidden",
        );
        let buttons_hidden = [
            NSWindowButton::CloseButton,
            NSWindowButton::MiniaturizeButton,
            NSWindowButton::ZoomButton,
        ]
        .into_iter()
        .all(|kind| {
            ns_window
                .standardWindowButton(kind)
                .is_none_or(|button| button.isHidden())
        });
        report.check(buttons_hidden, "traffic lights are hidden");
        report.check(ns_window.hasShadow(), "NSWindow has a shadow");
        let content = ns_window.contentView().map(|view| view.frame().size);
        let frame = ns_window.frame().size;
        report.check(
            content == Some(frame),
            &format!("content view fills the window, no title bar gap ({content:?} in {frame:?})"),
        );
        report.check(
            window.is_visible().ok() == Some(true),
            "Tauri reports the window visible",
        );
        report.check(ns_window.isVisible(), "NSWindow is visible");
        let min = ns_window.minSize();
        report.check(
            min.width == 640. && min.height == 400.,
            &format!(
                "NSWindow minimum size is 640x400 ({}x{})",
                min.width, min.height
            ),
        );
        report.check(
            testing::window_presentable(ns_window),
            "window is presentable for terminal focus",
        );
        report.check(!testing::application_hidden(), "application is not hidden");
        let host_error = testing::terminal_host_error();
        report.check(
            host_error.is_none(),
            &format!("terminal host attached to the web view {host_error:?}"),
        );

        let menus = main_menu_titles(mtm);
        report.check(
            menus == ["Ultra 9000", "Items", "Edit"],
            &format!("application menu installed {menus:?}"),
        );
        let app = NSApplication::sharedApplication(mtm);
        // SAFETY: `respondsToSelector:` is a side-effect-free NSObject query on the delegate.
        let terminate_hook = app.delegate().is_some_and(|delegate| unsafe {
            msg_send![&*delegate, respondsToSelector: sel!(applicationShouldTerminate:)]
        });
        report.check(
            terminate_hook,
            "application delegate decides termination (Dock Quit, logout)",
        );
        // SAFETY: Querying (null locale) does not change the process locale.
        let ctype = unsafe { libc::setlocale(libc::LC_CTYPE, std::ptr::null()) };
        let ctype = (!ctype.is_null()).then(|| {
            unsafe { CStr::from_ptr(ctype) }
                .to_string_lossy()
                .into_owned()
        });
        report.check(
            ctype
                .as_deref()
                .is_some_and(|ctype| ctype.contains("UTF-8")),
            &format!("shells get a UTF-8 character locale ({ctype:?})"),
        );
    }

    fn main_menu_titles(mtm: MainThreadMarker) -> Vec<String> {
        let app = NSApplication::sharedApplication(mtm);
        let Some(menu) = app.mainMenu() else {
            return Vec::new();
        };
        (0..menu.numberOfItems())
            .filter_map(|index| menu.itemAtIndex(index))
            .map(|item| item.title().to_string())
            .collect()
    }

    /// Ask the application delegate what AppKit asks it when the Dock icon is clicked while no
    /// window is visible. Returns whether the delegate implements it.
    fn dock_reopen(mtm: MainThreadMarker) -> bool {
        let app = NSApplication::sharedApplication(mtm);
        // SAFETY: Main thread; the delegate (tao's) implements this NSApplicationDelegate
        // method, checked first.
        unsafe {
            let Some(delegate) = app.delegate() else {
                return false;
            };
            let handles: bool = msg_send![
                &*delegate,
                respondsToSelector: sel!(applicationShouldHandleReopen:hasVisibleWindows:)
            ];
            if handles {
                let _: bool = msg_send![
                    &*delegate,
                    applicationShouldHandleReopen: &*app,
                    hasVisibleWindows: false
                ];
            }
            handles
        }
    }

    /// Run the production folder picker and abort it once it is the visible modal window.
    ///
    /// Like production (`commands::pick_directory` via `on_main_deferred`), the picker runs
    /// inside a main-dispatch-queue block, and GCD does not service the serial main queue again
    /// while that block sits in `runModal`. The abort therefore comes from a run-loop timer in
    /// the common modes, which the modal session keeps firing.
    fn folder_picker(report: &Report, mtm: MainThreadMarker) {
        let dir = match tempfile::tempdir() {
            Ok(dir) => dir,
            Err(err) => {
                report.check(false, &format!("temporary directory: {err}"));
                return;
            }
        };
        PANEL_DEADLINE.set(Some(Instant::now() + TIMEOUT));
        let timer = start_abort_timer();
        report.check(
            testing::pick_directory(Some(dir.path())).is_none(),
            "aborted folder picker returns None",
        );
        // SAFETY: `timer` was created and added to the main run loop by `start_abort_timer`;
        // invalidating removes it, releasing balances the create.
        unsafe {
            CFRunLoopTimerInvalidate(timer);
            CFRelease(timer);
        }
        report.check(
            MODAL_PANEL_VISIBLE.get(),
            "folder picker was the visible modal window",
        );
        let modal_after = NSApplication::sharedApplication(mtm).modalWindow();
        report.check(modal_after.is_none(), "no modal window after the picker");
        report.check(
            !testing::application_hidden(),
            "application is not hidden after the picker",
        );
    }

    /// A repeating main-run-loop timer in the common modes (which include the modal panel mode)
    /// that calls [`abort_when_modal`] every [`POLL`].
    fn start_abort_timer() -> *mut c_void {
        let interval = POLL.as_secs_f64();
        // SAFETY: Plain CoreFoundation calls on the main thread; a null context is allowed and
        // the callout ignores its info pointer. The run loop retains the timer it is added to.
        unsafe {
            let timer = CFRunLoopTimerCreate(
                std::ptr::null(),
                CFAbsoluteTimeGetCurrent() + interval,
                interval,
                0,
                0,
                abort_when_modal,
                std::ptr::null_mut(),
            );
            CFRunLoopAddTimer(CFRunLoopGetMain(), timer, kCFRunLoopCommonModes);
            timer
        }
    }

    /// Runs from the abort timer, also inside the panel's modal session: once the panel is the
    /// application's modal window, record what is observable and abort the modal session.
    extern "C" fn abort_when_modal(_timer: *mut c_void, _info: *mut c_void) {
        let mtm = MainThreadMarker::new().expect("run-loop timers fire on the main thread");
        let app = NSApplication::sharedApplication(mtm);
        let Some(panel) = app.modalWindow() else {
            if PANEL_DEADLINE
                .get()
                .is_some_and(|deadline| Instant::now() > deadline)
            {
                eprintln!("gui_macos: the folder picker did not become modal within {TIMEOUT:?}");
                std::process::exit(1);
            }
            return;
        };
        if MODAL_PANEL_VISIBLE.replace(panel.isVisible()) {
            return;
        }
        // Inside the panel's modal session; `runModal` returns NSModalResponseAbort.
        app.abortModal();
    }
}
