//! Linux GUI integration test on the real Wry runtime and a Wayland compositor.
//!
//! Runs the production startup (main window, terminal host, shortcut router, tray) on the
//! process's main thread, then checks the window's real GTK state, the navigation guard, focus
//! changes, hide-on-close, the physical-key shortcut router, the folder chooser, that the app
//! outlives its window, and that GLib/GTK logged no warnings. Starts no session. Without
//! `WAYLAND_DISPLAY` it is skipped, unless `U9K_REQUIRE_GUI_TESTS=1` makes that a failure.

#[cfg(target_os = "linux")]
fn main() -> std::process::ExitCode {
    linux::main()
}

#[cfg(not(target_os = "linux"))]
fn main() {}

#[cfg(target_os = "linux")]
mod linux {
    use std::cell::{Cell, RefCell};
    use std::path::Path;
    use std::process::ExitCode;
    use std::rc::Rc;
    use std::sync::{Mutex, PoisonError};
    use std::time::{Duration, Instant};

    use gtk::glib::translate::{IntoGlib, ToGlibPtr, ToGlibPtrMut};
    use gtk::glib::{self, ControlFlow, LogLevel};
    use gtk::prelude::*;
    use gtk::{gdk, FileChooserAction, ResponseType};
    use tauri::{AppHandle, Manager, RunEvent, Url, WebviewWindow, WindowEvent};
    use ultra9000_app::testing;

    /// Waits on the compositor (map, unmap) give up after this.
    const TIMEOUT: Duration = Duration::from_secs(30);
    /// After the first map, time for the web UI to load and paint before startup is judged.
    const SETTLE: Duration = Duration::from_secs(2);
    /// evdev `KEY_H`; GDK hardware keycodes are evdev codes + 8.
    const EVDEV_KEY_H: u16 = 35;

    /// GLib/GTK messages at warning level or above, from any domain.
    static GLIB_WARNINGS: Mutex<Vec<String>> = Mutex::new(Vec::new());

    pub fn main() -> ExitCode {
        if std::env::var_os("WAYLAND_DISPLAY").is_none_or(|display| display.is_empty()) {
            if std::env::var("U9K_REQUIRE_GUI_TESTS").as_deref() == Ok("1") {
                eprintln!("gui_linux: WAYLAND_DISPLAY is not set but U9K_REQUIRE_GUI_TESTS=1");
                return ExitCode::FAILURE;
            }
            println!("gui_linux: skipped, WAYLAND_DISPLAY is not set (no Wayland compositor)");
            return ExitCode::SUCCESS;
        }

        // GTK's accessibility bridge reports on the session's AT-SPI registry (absent or
        // partial on CI and isolated session buses), not on the app; keep it out of the check.
        std::env::set_var("NO_AT_BRIDGE", "1");
        // Before GTK initializes: every message (structured or not) reaches the writer.
        glib::log_set_writer_func(|level, fields| {
            if matches!(
                level,
                LogLevel::Warning | LogLevel::Critical | LogLevel::Error
            ) {
                let field = |key: &str| {
                    fields
                        .iter()
                        .find(|field| field.key() == key)
                        .and_then(|field| field.value_str())
                        .unwrap_or_default()
                        .to_owned()
                };
                let message = format!("{}-{level:?}: {}", field("GLIB_DOMAIN"), field("MESSAGE"));
                GLIB_WARNINGS
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push(message);
            }
            glib::log_writer_default(level, fields)
        });

        let report = Rc::new(Report::default());
        let mut started = false;
        let scenario_report = report.clone();
        let code = testing::app().run_return(move |app, event| {
            if matches!(event, RunEvent::Ready) && !started {
                started = true;
                let app = app.clone();
                let report = scenario_report.clone();
                glib::MainContext::default().spawn_local(async move {
                    scenario(&app, &report).await;
                    app.exit(report.exit_code());
                });
            }
            if matches!(event, RunEvent::ExitRequested { code: None, .. }) {
                scenario_report.exit_requested.set(true);
            }
            if let RunEvent::WindowEvent {
                label,
                event: WindowEvent::Focused(focused),
                ..
            } = &event
            {
                if label == testing::MAIN_WINDOW {
                    scenario_report.focus_events.borrow_mut().push(*focused);
                }
            }
            testing::on_run_event(app, event);
        });

        report.check(
            report.completed.get(),
            "scenario ran to its end (the event loop outlived the main window)",
        );
        let failures = report.failures.borrow().len();
        println!(
            "gui_linux: {} checks, {failures} failed",
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
        /// An exit was requested without a code (the last window went away).
        exit_requested: Cell<bool>,
        /// `Focused` events of the main window, in delivery order.
        focus_events: RefCell<Vec<bool>>,
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

    /// Poll `ready` on the GLib main loop (the app keeps running) until it holds.
    async fn wait_for(report: &Report, what: &str, ready: impl Fn() -> bool) -> bool {
        let deadline = Instant::now() + TIMEOUT;
        while !ready() {
            if Instant::now() > deadline {
                report.check(false, &format!("{what} within {TIMEOUT:?}"));
                return false;
            }
            glib::timeout_future(Duration::from_millis(20)).await;
        }
        true
    }

    fn shown(window: &gtk::Window) -> bool {
        window.is_visible() && window.is_mapped()
    }

    async fn scenario(app: &AppHandle, report: &Report) {
        let Some(window) = app.get_webview_window(testing::MAIN_WINDOW) else {
            report.check(false, "setup created the main window");
            return;
        };
        let gtk_window = match window.gtk_window() {
            Ok(gtk_window) => gtk_window.upcast::<gtk::Window>(),
            Err(err) => {
                report.check(false, &format!("main window has a GTK window: {err}"));
                return;
            }
        };

        if !wait_for(report, "main window mapped", || shown(&gtk_window)).await {
            return;
        }
        glib::timeout_future(SETTLE).await;
        startup(report, &window, &gtk_window);

        // The web view only ever shows the app's own UI.
        let app_url = window.url().ok();
        let foreign = Url::parse("data:text/html,foreign").expect("valid URL");
        report.check(
            window.navigate(foreign).is_ok(),
            "navigation request accepted",
        );
        glib::timeout_future(Duration::from_millis(500)).await;
        let url = window.url().ok();
        report.check(
            app_url.is_some() && url == app_url,
            &format!("navigation away from the app's UI is blocked ({url:?})"),
        );

        if !focus_change(report, &window, &gtk_window).await {
            return;
        }

        // A compositor close request hides the window; the app and its items stay alive.
        report.check(window.close().is_ok(), "close request accepted");
        if !wait_for(report, "window hidden on close request", || {
            !gtk_window.is_visible()
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

        if !show(app, report, &gtk_window).await {
            return;
        }

        // Ctrl+Shift+H by physical key is consumed by the app's router and hides the window.
        report.check(
            gtk_window.event(&ctrl_shift_h(&gtk_window)),
            "Ctrl+Shift+H consumed by the shortcut router",
        );
        if !wait_for(report, "window hidden by Ctrl+Shift+H", || {
            !gtk_window.is_visible()
        })
        .await
        {
            return;
        }
        if !show(app, report, &gtk_window).await {
            return;
        }

        folder_chooser(report, &gtk_window).await;

        let warnings = glib_warnings();
        report.check(
            warnings.is_empty(),
            &format!("no GLib/GTK warnings after hide, show and dialogs {warnings:?}"),
        );

        // Losing the last window does not end the app (its items and tray live on); only an
        // explicit exit does.
        report.check(window.destroy().is_ok(), "main window destroyed");
        if !wait_for(report, "main window gone", || {
            app.webview_windows().is_empty()
        })
        .await
        {
            return;
        }
        glib::timeout_future(Duration::from_millis(500)).await;
        report.check(
            report.exit_requested.get(),
            "losing the last window requested an exit",
        );
        report.completed.set(true);
    }

    fn glib_warnings() -> Vec<String> {
        GLIB_WARNINGS
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn startup(report: &Report, window: &WebviewWindow, gtk_window: &gtk::Window) {
        report.check(
            window.title().ok().as_deref() == Some("Ultra 9000"),
            "Tauri title is \"Ultra 9000\"",
        );
        report.check(
            gtk_window.title().as_deref() == Some("Ultra 9000"),
            "GTK title is \"Ultra 9000\"",
        );
        report.check(
            window.is_decorated().ok() == Some(false),
            "Tauri reports no decorations",
        );
        report.check(!gtk_window.is_decorated(), "GTK window is undecorated");
        report.check(
            window.is_visible().ok() == Some(true),
            "Tauri reports the window visible",
        );
        report.check(gtk_window.is_visible(), "GTK window is visible");
        report.check(gtk_window.is_realized(), "GTK window is realized");
        report.check(gtk_window.is_mapped(), "GTK window is mapped");
        report.check(
            gtk_window.display().type_().name() == "GdkWaylandDisplay",
            "GDK uses its Wayland backend",
        );
        report.check(
            testing::window_presentable(gtk_window),
            "window is presentable for terminal focus",
        );
        let host_error = testing::terminal_host_error();
        report.check(
            host_error.is_none(),
            &format!("terminal host attached to the web view {host_error:?}"),
        );
        let warnings = glib_warnings();
        report.check(
            warnings.is_empty(),
            &format!("no GLib/GTK warnings during startup {warnings:?}"),
        );
    }

    async fn show(app: &AppHandle, report: &Report, gtk_window: &gtk::Window) -> bool {
        let shown_ok = testing::show_window(app);
        report.check(
            shown_ok.is_ok(),
            &format!("show_window succeeds {shown_ok:?}"),
        );
        if !wait_for(report, "window mapped again after show", || {
            shown(gtk_window)
        })
        .await
        {
            return false;
        }
        report.check(
            testing::window_presentable(gtk_window),
            "shown window is presentable",
        );
        true
    }

    /// The compositor moves keyboard focus away from the window and back. Each change must
    /// reach the app's run-loop handler as a `Focused` event of the main window, and the
    /// window's focus state (the workspace's presented/unread gate) must follow it.
    async fn focus_change(
        report: &Report,
        window: &WebviewWindow,
        gtk_window: &gtk::Window,
    ) -> bool {
        for focused in [false, true] {
            report.focus_events.borrow_mut().clear();
            let mut event = focus_event(gtk_window, focused);
            gtk::main_do_event(&mut event);
            let delivered = || report.focus_events.borrow().last() == Some(&focused);
            if !wait_for(report, &format!("Focused({focused}) delivered"), delivered).await {
                return false;
            }
            report.check(
                delivered(),
                &format!("focus change reaches the run-loop handler as Focused({focused})"),
            );
            report.check(
                window.is_focused().ok() == Some(focused),
                &format!("window reports focused == {focused}"),
            );
        }
        true
    }

    /// A GDK focus change of `window`, as GDK delivers it from the compositor.
    fn focus_event(window: &gtk::Window, focus_in: bool) -> gdk::Event {
        let gdk_window = window.window().expect("mapped window has a GdkWindow");
        let mut event = gdk::Event::new(gdk::EventType::FocusChange);
        let keyboard = window
            .display()
            .default_seat()
            .and_then(|seat| seat.keyboard());
        event.set_device(keyboard.as_ref());
        // SAFETY: `event` is a freshly allocated GDK_FOCUS_CHANGE event, so its union is a
        // GdkEventFocus. The event takes the window reference that `to_glib_full` adds;
        // gdk_event_free releases it.
        unsafe {
            let raw: *mut gdk::ffi::GdkEvent = event.to_glib_none_mut().0;
            let focus = &mut *raw.cast::<gdk::ffi::GdkEventFocus>();
            focus.window = gdk_window.to_glib_full();
            focus.send_event = 1;
            focus.in_ = i16::from(focus_in);
        }
        event
    }

    /// A Ctrl+Shift+H key press on `window`, as GDK delivers it from the compositor.
    fn ctrl_shift_h(window: &gtk::Window) -> gdk::Event {
        let gdk_window = window.window().expect("mapped window has a GdkWindow");
        let mut event = gdk::Event::new(gdk::EventType::KeyPress);
        let keyboard = window
            .display()
            .default_seat()
            .and_then(|seat| seat.keyboard());
        event.set_device(keyboard.as_ref());
        // SAFETY: `event` is a freshly allocated GDK_KEY_PRESS event, so its union is a
        // GdkEventKey. The event takes the window reference that `to_glib_full` adds;
        // gdk_event_free releases it.
        unsafe {
            let raw: *mut gdk::ffi::GdkEvent = event.to_glib_none_mut().0;
            let key = &mut *raw.cast::<gdk::ffi::GdkEventKey>();
            key.window = gdk_window.to_glib_full();
            key.send_event = 1;
            key.time = gdk::ffi::GDK_CURRENT_TIME as u32;
            key.state = gdk::ffi::GDK_CONTROL_MASK | gdk::ffi::GDK_SHIFT_MASK;
            key.keyval = gdk::keys::constants::H.into_glib();
            key.hardware_keycode = EVDEV_KEY_H + 8;
        }
        event
    }

    /// Run the production folder chooser twice: cancelled, then accepted at its start folder.
    async fn folder_chooser(report: &Report, main_window: &gtk::Window) {
        let dir = match tempfile::tempdir() {
            Ok(dir) => dir,
            Err(err) => {
                report.check(false, &format!("temporary directory: {err}"));
                return;
            }
        };
        let expected = dir.path().to_str().map(str::to_owned);

        report.check(
            pick(report, main_window, dir.path(), ResponseType::Cancel).is_none(),
            "cancelled folder chooser returns None",
        );
        let chosen = pick(report, main_window, dir.path(), ResponseType::Accept);
        report.check(
            chosen.is_some() && chosen == expected,
            &format!("accepted folder chooser returns its start folder {chosen:?}"),
        );
        // Let the destroyed dialog go away before the final warning check.
        glib::timeout_future(Duration::from_millis(200)).await;
    }

    /// `pick_directory(folder)`, answered with `response` once its dialog shows `folder`.
    /// Aborts the process if no such dialog appears (its modal loop would never return).
    fn pick(
        report: &Report,
        main_window: &gtk::Window,
        folder: &Path,
        response: ResponseType,
    ) -> Option<String> {
        let deadline = Instant::now() + TIMEOUT;
        let main_window = main_window.clone();
        let shown_folder = folder.to_owned();
        let checks = Rc::new(RefCell::new(Vec::new()));
        let taken = checks.clone();
        glib::timeout_add_local(Duration::from_millis(20), move || {
            let dialog = gtk::Window::list_toplevels()
                .into_iter()
                .filter_map(|widget| widget.downcast::<gtk::FileChooserDialog>().ok())
                .find(|dialog| {
                    dialog.is_mapped()
                        && dialog.current_folder().as_deref() == Some(shown_folder.as_path())
                });
            let Some(dialog) = dialog else {
                if Instant::now() > deadline {
                    eprintln!(
                        "gui_linux: no folder chooser at {shown_folder:?} within {TIMEOUT:?}"
                    );
                    std::process::exit(1);
                }
                return ControlFlow::Continue;
            };
            taken.borrow_mut().extend([
                (
                    dialog.title().as_deref() == Some("Choose the session's working directory"),
                    "chooser titled for the session's working directory",
                ),
                (
                    dialog.transient_for().as_ref() == Some(&main_window),
                    "chooser is transient for the main window",
                ),
                (dialog.is_modal(), "chooser is modal"),
                (
                    dialog.action() == FileChooserAction::SelectFolder,
                    "chooser selects folders",
                ),
            ]);
            dialog.response(response);
            ControlFlow::Break
        });
        let chosen = testing::pick_directory(Some(folder));
        for (ok, what) in checks.borrow().iter() {
            report.check(*ok, what);
        }
        chosen
    }
}
