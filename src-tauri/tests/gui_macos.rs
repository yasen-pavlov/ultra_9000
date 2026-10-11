//! macOS GUI integration test on the real Wry runtime and AppKit's main thread.
//!
//! Runs the production startup (locale, application menu, terminate hook, main window,
//! terminal host, tray) and checks the window's real AppKit state, hide-on-close and the Dock
//! reopen that restores it, then runs the production folder picker (`NSOpenPanel` modal) and
//! aborts it from a run-loop timer once it is the application's modal window. Starts no
//! session. Needs a window server session; without one it is skipped, unless
//! `U9K_REQUIRE_GUI_TESTS=1` makes that a failure.
//!
//! Build the frontend, then run with `--features tauri/custom-protocol`: popover keyboard
//! assertions require the bundled Svelte UI (CI passes this flag). A plain GUI target with
//! neither bundled assets nor a development server cannot exercise WebKit Escape.

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
    use std::path::{Path, PathBuf};
    use std::pin::Pin;
    use std::process::ExitCode;
    use std::rc::Rc;
    use std::sync::mpsc;
    use std::task::{Context, Poll, Waker};
    use std::time::{Duration, Instant};

    use block2::RcBlock;

    use objc2::rc::Retained;
    use objc2::{class, msg_send, sel, AnyThread, MainThreadMarker};
    use objc2_app_kit::{
        NSAppearance, NSAppearanceCustomization, NSAppearanceNameAqua, NSAppearanceNameDarkAqua,
        NSApplication, NSApplicationActivationOptions, NSBitmapImageFileType, NSBitmapImageRep,
        NSDeviceRGBColorSpace, NSEvent, NSEventModifierFlags, NSEventType, NSImage,
        NSRunningApplication, NSStatusBarButton, NSView, NSWindow, NSWindowButton,
        NSWindowOcclusionState, NSWindowStyleMask, NSWindowTitleVisibility, NSWorkspace,
    };
    use objc2_foundation::{
        NSDictionary, NSError, NSNumber, NSObjectProtocol, NSPoint, NSProcessInfo, NSString,
    };
    use serde_json::{json, Value};
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

        // Visibility alone does not imply focus/activation, especially when another CI
        // step left Finder frontmost. Settle the same key/active precondition as a real Cmd+H.
        report.check(
            testing::show_window(app).is_ok(),
            "explicitly restore main focus before Cmd+H",
        );
        report.check(
            NSRunningApplication::currentApplication()
                .activateWithOptions(NSApplicationActivationOptions::ActivateAllWindows),
            "main application activation requested",
        );
        if !wait_for(
            report,
            "main key and our application frontmost before Cmd+H",
            || {
                ns_window.isKeyWindow()
                    && NSApplication::sharedApplication(mtm).isActive()
                    && frontmost_pid() == Some(current_pid())
            },
        )
        .await
        {
            return;
        }
        // Cmd+H (physical H) in the main window goes through the application key router's
        // local event monitor to the Hide action, which hides the application.
        report.check(
            post_key_press(mtm, &ns_window, KEY_H, "h", NSEventModifierFlags::Command),
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

        popover_scenario(app, report, mtm, &ns_window).await;

        folder_picker(report, mtm);
        report.completed.set(true);
    }

    /// Real AppKit regression coverage; no sessions, fake windows, or substitute renderer.
    async fn popover_scenario(
        app: &AppHandle,
        report: &Report,
        mtm: MainThreadMarker,
        main: &NSWindow,
    ) {
        let Some(window) = app.get_webview_window("popover") else {
            report.check(
                false,
                "startup creates the popover (menu-only fallback must fail this test)",
            );
            return;
        };
        // SAFETY: the Tauri-owned native window remains alive through the complete scenario.
        let Some(panel) = window
            .ns_window()
            .ok()
            .and_then(|raw| unsafe { Retained::retain(raw.cast::<NSWindow>()) })
        else {
            report.check(false, "popover has a native NSWindow");
            return;
        };
        let panel_class = objc2::runtime::AnyClass::get(c"U9KMenuBarPanel");
        report.check(
            panel_class.is_some_and(|class| panel.isKindOfClass(class)),
            "popover is the production panel subclass, not a fallback NSWindow",
        );
        report.check(!panel.isVisible(), "popover starts hidden");
        report.check(panel.hasShadow(), "popover has the native shadow");
        // Hand actual WindowServer foreground ownership to a known different app. Simply
        // setting NSApp.isActive=false with deactivate() does not establish that contract.
        let Some(finder) = NSRunningApplication::runningApplicationsWithBundleIdentifier(
            &NSString::from_str("com.apple.finder"),
        )
        .firstObject() else {
            report.check(false, "logged-in GUI runner has Finder");
            return;
        };
        let finder_pid = finder.processIdentifier();
        report.check(
            finder.activateWithOptions(NSApplicationActivationOptions::ActivateAllWindows),
            "Finder activation requested",
        );
        if !wait_for(
            report,
            "Finder owns the foreground before the status click",
            || frontmost_pid() == Some(finder_pid),
        )
        .await
        {
            return;
        }
        report.check(
            post_status_click(app, mtm),
            "first real status-button mouse down/up queued",
        );
        if !wait_for(report, "first status click opens a key popover", || {
            panel.isVisible() && panel.isKeyWindow()
        })
        .await
        {
            return;
        }
        report.check(
            frontmost_pid() == Some(finder_pid) && frontmost_pid() != Some(current_pid()),
            "nonactivating status click keeps Finder foreground while the panel takes key",
        );
        report.check(
            !main.isKeyWindow(),
            "popover key does not give the main window key focus",
        );
        sleep(SETTLE).await; // Allow the bundled popover to subscribe and settle its layout.
        report.check(
            frontmost_pid() == Some(finder_pid),
            "Finder stays foreground after the popover UI settles",
        );
        report.check(
            post_status_click(app, mtm),
            "second status-button mouse down/up queued",
        );
        if !wait_for(
            report,
            "second status click hides rather than reopens the panel",
            || !panel.isVisible(),
        )
        .await
        {
            return;
        }
        // Let both halves of the press and deferred resign-key/toggle callbacks finish.
        sleep(POLL).await;
        report.check(
            !panel.isVisible(),
            "second-click dismissal survives its deferred callbacks",
        );
        report.check(
            post_status_click(app, mtm),
            "third status click queued to reopen",
        );
        if !wait_for(report, "third status click reopens a key panel", || {
            panel.isVisible() && panel.isKeyWindow()
        })
        .await
        {
            return;
        }
        sleep(POLL).await;

        if let Some(directory) = shot_directory(report) {
            snapshot_popover(&window, &directory.join("popover-opened.png")).await;
        }

        let original_top = panel.frame().origin.y + panel.frame().size.height;
        let bottom = panel
            .screen()
            .map_or(0., |screen| screen.visibleFrame().origin.y);
        let available_height = (original_top - bottom - 8.).max(0.);
        for (requested, expected) in [(1., 200.), (9999., 600.), (420., 420.)] {
            let expected = f64::min(expected, available_height);
            report.check(
                testing::popover_set_height(app.clone(), requested)
                    .await
                    .is_ok(),
                "real popover_set_height command succeeds",
            );
            let frame = panel.frame();
            report.check(
                (frame.size.height - expected).abs() < 0.5,
                &format!(
                    "popover height {requested} clamps to {expected}, got {}",
                    frame.size.height
                ),
            );
            report.check(
                (frame.origin.y + frame.size.height - original_top).abs() < 0.5,
                "visible resizing keeps the current top edge",
            );
        }

        let reply = popover_reply(app, json!({"type":"jump_to_waiting"})).await;
        report.check(reply == Ok(json!({"ok":false,"error":"nothing_waiting","message":"Nothing is waiting for you."})), &format!("popover_command failure round trip: {reply:?}"));
        let reply = popover_reply(app, json!({"type":"set_window_visible","visible":false})).await;
        report.check(
            reply == Ok(json!({"ok":true,"visible":false})),
            "popover_command success round trip hides only main",
        );
        if !wait_for(report, "main hidden by control command", || {
            !main.isVisible()
        })
        .await
        {
            return;
        }
        report.check(
            panel.isVisible() && panel.isKeyWindow(),
            "main hide leaves popover open and key",
        );
        report.check(
            testing::popover_toggle_main(app.clone()).await.is_ok(),
            "popover_toggle_main shows main",
        );
        if !wait_for(
            report,
            "inactive main show and occlusion state settled",
            || {
                main.isVisible()
                    && main.isOnActiveSpace()
                    && main
                        .occlusionState()
                        .contains(NSWindowOcclusionState::Visible)
            },
        )
        .await
        {
            return;
        }
        report.check(
            panel.isKeyWindow() && frontmost_pid() == Some(finder_pid),
            "inactive h show keeps panel key and Finder foreground",
        );
        report.check(
            testing::popover_toggle_main(app.clone()).await.is_ok(),
            "popover_toggle_main hides main",
        );
        if !wait_for(report, "inactive main hide", || !main.isVisible()).await {
            return;
        }
        report.check(panel.isKeyWindow(), "inactive h hide keeps panel key");

        report.check(
            post_key_press(mtm, &panel, 53, "\u{1b}", NSEventModifierFlags::empty()),
            "plain Escape is posted to popover WebKit",
        );
        if !wait_for(report, "Escape invokes popover_hide", || !panel.isVisible()).await {
            return;
        }
        report.check(
            app.get_webview_window("popover").is_some(),
            "Escape hides rather than destroys the panel",
        );
        report.check(
            testing::popover_show(app).is_ok(),
            "popover reopens for outside-click test",
        );
        if !wait_for(report, "reopened panel key", || {
            panel.isVisible() && panel.isKeyWindow()
        })
        .await
        {
            return;
        }
        report.check(
            post_outside_click(mtm),
            "outside mouse-down is queued through the local AppKit monitor",
        );
        if !wait_for(report, "outside click dismisses popover", || {
            !panel.isVisible()
        })
        .await
        {
            return;
        }
        report.check(
            testing::popover_show(app).is_ok(),
            "popover reopens for direct hide command",
        );
        report.check(
            testing::popover_hide(app.clone()).await.is_ok(),
            "real popover_hide command succeeds",
        );
        if !wait_for(report, "direct hide command dismisses panel", || {
            !panel.isVisible()
        })
        .await
        {
            return;
        }
        let reply = popover_reply(app, json!({"type":"set_window_visible","visible":true})).await;
        report.check(
            reply == Ok(json!({"ok":true,"visible":true})),
            "control show reply round trip activates main deliberately",
        );
        if !wait_for(report, "main restored after popover tests", || {
            main.isVisible()
        })
        .await
        {
            return;
        }
        render_status_matrix(app, report, mtm).await;
    }

    async fn popover_reply(app: &AppHandle, value: Value) -> Result<Value, String> {
        let command: testing::PopoverCommand =
            serde_json::from_value(value).map_err(|error| error.to_string())?;
        let reply = testing::popover_command(app.clone(), command).await?;
        serde_json::to_value(reply).map_err(|error| error.to_string())
    }

    fn current_pid() -> libc::pid_t {
        libc::pid_t::try_from(std::process::id()).expect("process PID fits the platform pid_t")
    }

    fn frontmost_pid() -> Option<libc::pid_t> {
        NSWorkspace::sharedWorkspace()
            .frontmostApplication()
            .map(|app| app.processIdentifier())
    }

    fn post_key_press(
        mtm: MainThreadMarker,
        expected: &NSWindow,
        key_code: u16,
        characters: &str,
        modifiers: NSEventModifierFlags,
    ) -> bool {
        let app = NSApplication::sharedApplication(mtm);
        let Some(window) = app
            .keyWindow()
            .filter(|window| std::ptr::eq(&**window, expected))
        else {
            return false;
        };
        let text = NSString::from_str(characters);
        let timestamp = NSProcessInfo::processInfo().systemUptime();
        let make = |kind| {
            NSEvent::keyEventWithType_location_modifierFlags_timestamp_windowNumber_context_characters_charactersIgnoringModifiers_isARepeat_keyCode(
            kind, NSPoint::new(0., 0.), modifiers, timestamp, window.windowNumber(),
            None, &text, &text, false, key_code,
        )
        };
        let (Some(down), Some(up)) = (make(NSEventType::KeyDown), make(NSEventType::KeyUp)) else {
            return false;
        };
        // Like hardware input, target the actual key window and deliver a whole press
        // through AppKit's event queue/local monitors, never directly to WebKit.
        app.postEvent_atStart(&down, false);
        app.postEvent_atStart(&up, false);
        true
    }

    fn status_button(app: &AppHandle) -> Option<Retained<NSStatusBarButton>> {
        let tray = app.tray_by_id("ultra9000")?;
        let pointer = tray
            .with_inner_tray_icon(|inner| {
                inner
                    .ns_status_item()
                    .and_then(|item| item.button(MainThreadMarker::new().unwrap()))
                    .map_or(0, |button| Retained::as_ptr(&button) as usize)
            })
            .ok()?;
        // SAFETY: the app-managed tray retains this real button; called synchronously on
        // main while no tray visibility/removal operation can invalidate the pointer.
        unsafe { Retained::retain((pointer as *mut c_void).cast::<NSStatusBarButton>()) }
    }

    fn post_status_click(app: &AppHandle, mtm: MainThreadMarker) -> bool {
        let Some(button) = status_button(app) else {
            return false;
        };
        let Some(window) = button.window() else {
            return false;
        };
        let bounds = button.bounds();
        let location = button.convertPoint_toView(
            NSPoint::new(
                bounds.origin.x + bounds.size.width / 2.,
                bounds.origin.y + bounds.size.height / 2.,
            ),
            None,
        );
        let timestamp = NSProcessInfo::processInfo().systemUptime();
        let make = |kind| {
            NSEvent::mouseEventWithType_location_modifierFlags_timestamp_windowNumber_context_eventNumber_clickCount_pressure(
            kind, location, NSEventModifierFlags::empty(), timestamp,
            window.windowNumber(), None, 1, 1, 1.,
        )
        };
        let (Some(down), Some(up)) = (
            make(NSEventType::LeftMouseDown),
            make(NSEventType::LeftMouseUp),
        ) else {
            return false;
        };
        let native = NSApplication::sharedApplication(mtm);
        native.postEvent_atStart(&down, false);
        native.postEvent_atStart(&up, false);
        true
    }

    fn post_outside_click(mtm: MainThreadMarker) -> bool {
        // A windowNumber=0 event belongs to neither panel nor status window. This reaches
        // the production local monitor without clicking a launcher button or spawning an item.
        let Some(event) = NSEvent::mouseEventWithType_location_modifierFlags_timestamp_windowNumber_context_eventNumber_clickCount_pressure(
            NSEventType::LeftMouseDown, NSPoint::new(0., 0.), NSEventModifierFlags::empty(),
            0., 0, None, 1, 1, 1.,
        ) else { return false; };
        NSApplication::sharedApplication(mtm).postEvent_atStart(&event, false);
        true
    }

    fn shot_directory(report: &Report) -> Option<PathBuf> {
        let path = PathBuf::from(std::env::var_os("U9K_SHOT_DIR")?);
        match std::fs::create_dir_all(&path) {
            Ok(()) => Some(path),
            Err(error) => {
                report.check(false, &format!("create screenshot directory: {error}"));
                None
            }
        }
    }

    async fn render_status_matrix(app: &AppHandle, report: &Report, mtm: MainThreadMarker) {
        use testing::BarCounts;
        let Some(tray) = app.tray_by_id("ultra9000") else {
            report.check(false, "production status item exists");
            return;
        };
        let Some(button) = status_button(app) else {
            report.check(false, "native NSStatusBarButton is available");
            return;
        };
        let shots = shot_directory(report);
        let cases = [
            (
                "quiet",
                BarCounts {
                    running: 5,
                    ..Default::default()
                },
                "Ultra 9000, nothing waiting, 5 running",
            ),
            (
                "unread",
                BarCounts {
                    unread: 2,
                    running: 5,
                    ..Default::default()
                },
                "Ultra 9000, 2 unread, 5 running",
            ),
            (
                "need-1",
                BarCounts {
                    needs_you: 1,
                    running: 6,
                    ..Default::default()
                },
                "Ultra 9000, 1 needs you, 6 running",
            ),
            (
                "need-12",
                BarCounts {
                    needs_you: 12,
                    running: 14,
                    ..Default::default()
                },
                "Ultra 9000, 12 need you, 14 running",
            ),
            (
                "working",
                BarCounts {
                    working: 2,
                    running: 5,
                    ..Default::default()
                },
                "Ultra 9000, 2 working, 5 running",
            ),
            (
                "on-job",
                BarCounts {
                    on_job: 1,
                    running: 5,
                    ..Default::default()
                },
                "Ultra 9000, 1 on job, 5 running",
            ),
            (
                "error",
                BarCounts {
                    error: 1,
                    running: 5,
                    ..Default::default()
                },
                "Ultra 9000, 1 error, 5 running",
            ),
            (
                "all",
                BarCounts {
                    needs_you: 2,
                    error: 1,
                    working: 4,
                    on_job: 2,
                    unread: 3,
                    running: 14,
                },
                "Ultra 9000, 2 need you, 1 error, 4 working, 2 on job, 3 unread, 14 running",
            ),
            (
                "99-plus",
                BarCounts {
                    needs_you: 100,
                    working: 101,
                    running: 205,
                    ..Default::default()
                },
                "Ultra 9000, 100 need you, 101 working, 205 running",
            ),
        ];
        let mut aqua_raster: Option<(isize, isize, Vec<u8>)> = None;
        for (appearance_name, name) in [
            ("aqua", unsafe { NSAppearanceNameAqua }),
            ("dark-aqua", unsafe { NSAppearanceNameDarkAqua }),
        ] {
            let appearance = NSAppearance::appearanceNamed(name);
            report.check(appearance.is_some(), &format!("{appearance_name} exists"));
            button.setAppearance(appearance.as_deref());
            sleep(POLL).await; // KVO must run independently of workspace publishes.
            let mut quiet_width = 0.;
            let mut single_width = 0.;
            for (state, counts, label) in &cases {
                report.check(
                    testing::update_menu_bar_title(&tray, *counts).is_ok(),
                    &format!("{appearance_name}/{state} rendered by native production entry"),
                );
                sleep(POLL).await;
                let bounds = button.bounds();
                if *state == "quiet" {
                    quiet_width = bounds.size.width;
                }
                if *state == "working" {
                    single_width = bounds.size.width;
                }
                if *state != "quiet" && *state != "unread" {
                    report.check(
                        bounds.size.width > quiet_width,
                        &format!("{appearance_name}/{state} width grows beyond the quiet mark"),
                    );
                }
                if *state == "all" {
                    report.check(
                        bounds.size.width > single_width,
                        "all segments are wider than one working segment",
                    );
                }
                // SAFETY: accessibilityLabel is an NSString* (nullable) getter on this live
                // AppKit button; objc2 retains the returned value for the comparison.
                let actual: Option<Retained<NSString>> =
                    unsafe { msg_send![&*button, accessibilityLabel] };
                report.check(
                    actual.as_deref().map(NSString::to_string).as_deref() == Some(*label),
                    &format!("{appearance_name}/{state} exact accessible label"),
                );
                match render_button_2x(&button) {
                    Ok(bitmap) => {
                        if *state == "all" {
                            let bytes = bitmap_raster(&bitmap).expect("raster already validated");
                            if appearance_name == "aqua" {
                                aqua_raster = Some((
                                    bitmap.pixelsWide(),
                                    bitmap.pixelsHigh(),
                                    bytes.to_vec(),
                                ));
                            } else if let Some((width, height, aqua)) = &aqua_raster {
                                report.check(
                                    (*width, *height) == (bitmap.pixelsWide(), bitmap.pixelsHigh()),
                                    "appearance changes preserve raster geometry",
                                );
                                report.check(
                                    aqua.as_slice() != bytes,
                                    "Aqua and DarkAqua actually render different status pixels",
                                );
                            } else {
                                report.check(
                                    false,
                                    "Aqua reference raster exists for the appearance comparison",
                                );
                            }
                        }
                        if let Some(directory) = &shots {
                            report.check(
                                write_bitmap(
                                    &bitmap,
                                    &directory
                                        .join(format!("status-{state}-{appearance_name}.png")),
                                )
                                .is_ok(),
                                &format!("write {state}/{appearance_name} PNG"),
                            );
                        }
                    }
                    Err(error) => {
                        report.check(false, &format!("draw {state}/{appearance_name}: {error}"))
                    }
                }
            }
        }
        button.setAppearance(None);
        let _ = testing::update_menu_bar_title(&tray, BarCounts::default());
        report.check(
            NSApplication::sharedApplication(mtm).isActive(),
            "main-window show leaves the app active after popover scenario",
        );
    }

    fn render_button_2x(button: &NSStatusBarButton) -> Result<Retained<NSBitmapImageRep>, String> {
        let bounds = button.bounds();
        // SAFETY: null planes asks AppKit to own allocation; positive 2x dimensions,
        // 8-bit interleaved RGBA and RGB color space match the bitmap's storage layout.
        let bitmap = unsafe {
            NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bytesPerRow_bitsPerPixel(
                NSBitmapImageRep::alloc(), std::ptr::null_mut(),
                (bounds.size.width * 2.).ceil() as isize, (bounds.size.height * 2.).ceil() as isize,
                8, 4, true, false, NSDeviceRGBColorSpace, 0, 0,
            )
        }.ok_or("unable to allocate 2x status bitmap")?;
        bitmap.setSize(bounds.size);
        let pointer = bitmap.bitmapData();
        if pointer.is_null() {
            return Err("status bitmap has no raster storage".into());
        }
        let length = usize::try_from(bitmap.bytesPerRow() * bitmap.pixelsHigh())
            .map_err(|error| error.to_string())?;
        // SAFETY: this bitmap owns length bytes of writable interleaved raster storage.
        // Clear it first so a nonempty result proves drawing, not uninitialized allocation.
        unsafe { std::ptr::write_bytes(pointer, 0, length) };
        NSView::setNeedsDisplay(button, true);
        // This forces actual AppKit text/attachment/image drawing, not just construction
        // of an attributed string whose lazy NSImage handlers have never executed.
        button.cacheDisplayInRect_toBitmapImageRep(bounds, &bitmap);
        let bytes = bitmap_raster(&bitmap)?;
        if bytes.iter().all(|byte| *byte == 0) {
            return Err("status button rendered an empty bitmap".into());
        }
        Ok(bitmap)
    }

    fn bitmap_raster(bitmap: &NSBitmapImageRep) -> Result<&[u8], String> {
        let pointer = bitmap.bitmapData();
        if pointer.is_null() {
            return Err("status bitmap has no raster storage".into());
        }
        let length = usize::try_from(bitmap.bytesPerRow() * bitmap.pixelsHigh())
            .map_err(|error| error.to_string())?;
        // SAFETY: this test owns the bitmap, cacheDisplay has completed synchronously,
        // and no mutation occurs during the borrow of its interleaved backing buffer.
        Ok(unsafe { std::slice::from_raw_parts(pointer, length) })
    }

    fn write_bitmap(bitmap: &NSBitmapImageRep, path: &Path) -> Result<(), String> {
        let properties = NSDictionary::new();
        // SAFETY: the empty property dictionary has the correct generic key/value types;
        // AppKit's encoder returns a retained immutable NSData.
        let data = unsafe {
            bitmap.representationUsingType_properties(NSBitmapImageFileType::PNG, &properties)
        }
        .ok_or("unable to encode PNG")?;
        // SAFETY: this encoder-owned NSData is retained and never exposed to mutators;
        // the borrow lasts only for the synchronous file write.
        std::fs::write(path, unsafe { data.as_bytes_unchecked() })
            .map_err(|error| error.to_string())
    }

    /// Optional artifact capture only. Lack of WK snapshot support does not skip any
    /// behavioral assertion or status-button rasterization.
    async fn snapshot_popover(window: &WebviewWindow, path: &Path) {
        let (result, completed) = mpsc::channel();
        let path = path.to_owned();
        let queued = window.with_webview(move |webview| {
            // SAFETY: Wry supplies a live WKWebView on main. WebKit is already loaded;
            // WKSnapshotConfiguration inherits NSObject new, its setters take NSRect
            // and NSNumber*, and takeSnapshot's retained block takes NSImage*, NSError*.
            unsafe {
                let configuration: Retained<objc2::runtime::AnyObject> = msg_send![class!(WKSnapshotConfiguration), new];
                let view = &*webview.inner().cast::<NSView>();
                let _: () = msg_send![&*configuration, setRect: view.bounds()];
                let _: () = msg_send![&*configuration, setSnapshotWidth: &*NSNumber::new_f64(view.bounds().size.width * 2.)];
                let block: RcBlock<dyn Fn(*mut NSImage, *mut NSError)> = RcBlock::new(move |image: *mut NSImage, error: *mut NSError| {
                    let saved = if !error.is_null() {
                        Err((&*error).localizedDescription().to_string())
                    } else if let Some(image) = Retained::retain(image) {
                        image.TIFFRepresentation().and_then(|data| NSBitmapImageRep::imageRepWithData(&data))
                            .ok_or_else(|| "WK snapshot had no bitmap representation".to_owned())
                            .and_then(|bitmap| write_bitmap(&bitmap, &path))
                    } else { Err("WK snapshot returned no image".into()) };
                    let _ = result.send(saved);
                });
                let _: () = msg_send![view, takeSnapshotWithConfiguration: &*configuration, completionHandler: &*block];
            }
        });
        if let Err(error) = queued {
            println!("gui_macos: skipping optional popover PNG: {error}");
            return;
        }
        let deadline = Instant::now() + TIMEOUT;
        loop {
            match completed.try_recv() {
                Ok(result) => {
                    if let Err(error) = result {
                        println!("gui_macos: skipping optional popover PNG: {error}");
                    }
                    return;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    println!("gui_macos: skipping optional popover PNG: WK callback dropped");
                    return;
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
            if Instant::now() > deadline {
                println!("gui_macos: skipping optional popover PNG: WK snapshot timed out");
                return;
            }
            sleep(POLL).await;
        }
    }

    /// `kVK_ANSI_H`, the physical key the Hide shortcut (Cmd+H) is bound to.
    const KEY_H: u16 = 4;

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
