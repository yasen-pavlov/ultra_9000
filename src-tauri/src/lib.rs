//! Ultra 9000: native Alacritty items controlled by a Tauri/Svelte agent harness.
//! Hide retains live processes; item close ends them; saved conversations stay tool-owned.

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
compile_error!("Ultra 9000 supports macOS and Linux (Wayland) only");

mod awake;
mod commands;
mod config;
mod control;
mod ctl;
mod discovery;
mod instance;
mod launch;
mod lifecycle;
#[cfg(target_os = "linux")]
mod linux;
mod logger;
#[cfg(target_os = "macos")]
mod macos;
mod menubar;
mod notify;
mod omp_link;
mod runtime;
mod shortcuts;
mod status;
#[cfg(all(test, target_os = "linux"))]
mod test_support;
#[cfg(target_os = "linux")]
mod tray_linux;
mod tray_model;
mod ui_state;
mod workspace;

/// Native lifecycle pieces of the current platform (dialogs, modal guard, main-loop deferral).
#[cfg(target_os = "linux")]
use linux as platform;
#[cfg(target_os = "macos")]
use macos as platform;

use tauri::window::Color;
use tauri::{
    App, AppHandle, RunEvent, Url, WebviewUrl, WebviewWindow, WebviewWindowBuilder, WindowEvent,
};

/// Terminal background, shown before the web UI paints.
const BACKGROUND: Color = Color(0x21, 0x21, 0x21, 0xff);

/// Start Ultra 9000 and run its event loop until the app quits.
pub fn run() {
    let _instance_lock = match instance::start() {
        Ok(instance::Launch::Owner(lock)) => lock,
        Ok(instance::Launch::HandedOver(pid)) => {
            eprintln!("Ultra 9000 is already running (pid {pid}); showing its window.");
            return;
        }
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    };
    // Keep the fd (and its flock) through startup, the event loop and final teardown.
    app().run(on_run_event);
}

/// Run `ultra9000 ctl <args>` (the shell control link's client) without starting the app;
/// returns the process exit code.
pub fn ctl(args: Vec<std::ffi::OsString>) -> i32 {
    ctl::main(args)
}

/// Production startup and platform entry points for the GUI integration tests
/// (`tests/gui_*.rs`), which drive them on the main thread with the real runtime. Not an API.
#[doc(hidden)]
pub mod testing {
    #[cfg(target_os = "macos")]
    pub use crate::commands::{
        popover_command, popover_hide, popover_set_height, popover_toggle_main,
    };
    #[cfg(target_os = "macos")]
    pub use crate::control::Command as PopoverCommand;
    pub use crate::lifecycle::{show_window, MAIN_WINDOW};
    #[cfg(target_os = "macos")]
    pub use crate::menubar::{show as popover_show, update_menu_bar_title};
    #[cfg(target_os = "macos")]
    pub use crate::platform::{application_hidden, defer_on_main};
    pub use crate::platform::{pick_directory, window_presentable};
    #[cfg(target_os = "macos")]
    pub use crate::tray_model::BarCounts;

    /// The application exactly as [`crate::run`] builds it; its setup (main window, terminal
    /// host, shortcuts, tray) runs on the event loop's first turn.
    pub fn app() -> tauri::App {
        crate::app()
    }

    /// [`crate::run`]'s event handler.
    pub fn on_run_event(app: &tauri::AppHandle, event: tauri::RunEvent) {
        crate::on_run_event(app, event);
    }

    /// Why the terminal host is unavailable, `None` once it started.
    pub fn terminal_host_error() -> Option<String> {
        crate::workspace::snapshot().map_or_else(Some, |state| state.host_error)
    }
}

fn app() -> App {
    logger::init();

    // Like upstream Alacritty's `main`: give the shells a UTF-8 locale when the app is started
    // from Finder/Dock without LANG/LC_* in its environment (macOS only, as upstream), and set
    // TERM/COLORTERM.
    #[cfg(target_os = "macos")]
    alacritty_embed::locale::set_locale_environment();
    alacritty_embed::embed::setup_environment();

    let builder = tauri::Builder::default();
    // Linux has no application menu bar: Tauri would add a GTK menu bar inside the window.
    // App shortcuts are intercepted on the GTK window instead (`lifecycle::install_shortcuts`).
    #[cfg(target_os = "macos")]
    let builder = builder
        .menu(lifecycle::build_menu)
        .on_menu_event(|app, event| lifecycle::handle_menu_event(app, event.id().as_ref()));
    builder
        .invoke_handler(tauri::generate_handler![
            commands::workspace_state,
            commands::item_launch,
            commands::item_resume,
            commands::item_restart,
            commands::item_select,
            commands::item_close,
            commands::notice_dismiss,
            commands::sheet_open,
            commands::sheet_close,
            commands::app_quit,
            commands::last_set_resume,
            commands::last_set_dismiss,
            commands::ui_set,
            commands::path_check,
            commands::pick_directory,
            commands::conversations_list,
            commands::settings_state,
            commands::settings_set,
            commands::settings_reset,
            commands::settings_open_file,
            commands::settings_reveal,
            commands::terminal_set_viewport,
            commands::terminal_focus,
            commands::app_hide,
            commands::app_request_quit,
            commands::popover_command,
            commands::popover_hide,
            commands::popover_set_height,
            commands::popover_toggle_main,
        ])
        .setup(|app| {
            #[cfg(target_os = "macos")]
            notify::install();
            omp_link::start();
            workspace::init(app.handle().clone());
            control::start(app.handle().clone());
            #[cfg(target_os = "macos")]
            macos::install_terminate_hook(lifecycle::confirm_and_stop_sessions);
            let window = create_main_window(app)?;
            #[cfg(target_os = "linux")]
            lifecycle::install_shortcuts(&window)?;
            attach_terminal_host(&window)?;
            lifecycle::install_menu_bar_icon(app)?;
            #[cfg(target_os = "macos")]
            if let Err(error) = menubar::create(app) {
                log::warn!("Menu-bar popover unavailable; keeping the native menu: {error}");
                menubar::recover_menu_only(app.handle());
            }
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building Ultra 9000")
}

fn on_run_event(app: &AppHandle, event: RunEvent) {
    match event {
        // Successful panel conversion is process-lifetime: ordinary close requests only
        // dismiss it. Failed startup recovery uses destroy(), which bypasses this request.
        #[cfg(target_os = "macos")]
        RunEvent::WindowEvent {
            label,
            event: WindowEvent::CloseRequested { api, .. },
            ..
        } if label == menubar::WINDOW => {
            api.prevent_close();
            let app = app.clone();
            macos::defer_on_main(Box::new(move || {
                if let Err(error) = menubar::hide(&app) {
                    log::warn!("popover close: {error}");
                }
            }));
        }
        // No visible title bar or close button, but the OS can still ask to close (e.g.
        // Window > Close shortcuts from other tools): hide instead, keeping every session alive.
        RunEvent::WindowEvent {
            label,
            event: WindowEvent::CloseRequested { api, .. },
            ..
        } if label == lifecycle::MAIN_WINDOW => {
            api.prevent_close();
            if let Err(err) = lifecycle::hide_window(app) {
                log::error!("Unable to hide window: {err}");
            }
        }
        RunEvent::WindowEvent {
            label,
            event: WindowEvent::Focused(focused),
            ..
        } if label == lifecycle::MAIN_WINDOW => {
            if !focused {
                workspace::invalidate_focus();
            }
            workspace::window_changed();
        }
        RunEvent::WindowEvent {
            label,
            event: WindowEvent::Resized(_),
            ..
        } if label == lifecycle::MAIN_WINDOW => workspace::window_changed(),
        #[cfg(target_os = "macos")]
        RunEvent::WindowEvent {
            label,
            event: WindowEvent::ThemeChanged(_),
            ..
        } if label == lifecycle::MAIN_WINDOW => lifecycle::refresh_tray(app),
        // Dock icon click: restore the existing window and sessions.
        #[cfg(target_os = "macos")]
        RunEvent::Reopen { .. } => {
            if let Err(err) = lifecycle::show_window(app) {
                log::error!("Unable to show window: {err}");
            }
        }
        // Exits not requested through the confirmed quit path (e.g. last window closed).
        RunEvent::ExitRequested {
            code: None, api, ..
        } => api.prevent_exit(),
        // Final teardown; stopping sessions is a no-op when the confirmed quit already did.
        RunEvent::Exit => {
            workspace::shutdown_all();
            control::shutdown();
            omp_link::shutdown();
        }
        _ => (),
    }
}

fn create_main_window(app: &App) -> tauri::Result<WebviewWindow> {
    let builder = WebviewWindowBuilder::new(app, lifecycle::MAIN_WINDOW, WebviewUrl::default())
        .title("Ultra 9000")
        .inner_size(1200., 800.);
    // Tiling Wayland compositors give the window whatever tile it gets. A GTK minimum size
    // larger than the tile only makes GTK draw past it, and the compositor then clips or
    // squashes the window and its terminal subsurface, so Linux follows the tile instead.
    // macOS: AppKit only rounds (and clips, GL views included) titled windows, so the window
    // is titled with a transparent full-size titlebar, its title and traffic lights hidden:
    // system corners, border and shadow with no visible title bar (Alacritty's "Buttonless").
    #[cfg(target_os = "macos")]
    let builder = builder
        .min_inner_size(640., 400.)
        .decorations(true)
        .title_bar_style(tauri::TitleBarStyle::Overlay)
        .hidden_title(true);
    // tao (0.37+) installs a GTK titlebar on Wayland for undecorated windows after applying
    // visibility; on an already realized window GTK warns and unrealizes it to do so. Building
    // hidden and showing afterwards gives the same window without that cycle.
    #[cfg(target_os = "linux")]
    let builder = builder.visible(false).decorations(false);
    let window = builder
        .resizable(true)
        .shadow(true)
        .zoom_hotkeys_enabled(false)
        .background_color(BACKGROUND)
        .on_navigation(is_local_url)
        .build()?;
    // SAFETY: the window was just built on the main thread and is alive.
    #[cfg(target_os = "macos")]
    unsafe {
        macos::hide_window_buttons(window.ns_window()?)
    };
    #[cfg(target_os = "linux")]
    window.show()?;
    Ok(window)
}

/// Only the bundled UI (or the dev server in development) may be loaded.
fn is_local_url(url: &Url) -> bool {
    match url.scheme() {
        "tauri" => url.host_str() == Some("localhost"),
        "http" if tauri::is_dev() => {
            url.host_str() == Some("127.0.0.1") && url.port() == Some(1420)
        }
        _ => false,
    }
}

/// Prepare hosting terminal views next to the window's WKWebView. No session is started.
#[cfg(target_os = "macos")]
fn attach_terminal_host(window: &WebviewWindow) -> tauri::Result<()> {
    window.with_webview(|webview| {
        // SAFETY: Tauri runs this on the main thread with the live WKWebView, which Wry has
        // already installed in the window's content view.
        unsafe { workspace::attach_terminal_host(webview.inner()) };
    })
}

/// Prepare hosting terminals over the window's WebKitWebView. No session is started.
#[cfg(target_os = "linux")]
fn attach_terminal_host(window: &WebviewWindow) -> tauri::Result<()> {
    use gtk::glib::object::ObjectType;

    window.with_webview(|webview| {
        let web_view = webview.inner();
        // SAFETY: Tauri runs this on the main thread with the live WebKitWebView, which Wry
        // has already packed into the window's GTK box.
        unsafe { workspace::attach_terminal_host(web_view.as_ptr().cast()) };
    })
}
