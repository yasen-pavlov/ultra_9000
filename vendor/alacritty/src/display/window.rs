// Replacement for alacritty/src/display/window.rs at def47a5dd511ed20607eb5d9e5e7140afd16b31d.
//
// Upstream wraps a winit `Window` it creates itself. Here the terminal is hosted in an
// embedder-provided `TerminalView` (a custom NSView in the embedder's window), so this type
// keeps the public surface the upstream display/input/event code uses and implements it on
// the view and its NSWindow:
// - The terminal never creates, titles, shows, positions or decorates a window; `set_title`
//   only records the title for the embedder.
// - Window-level actions bound in Alacritty (fullscreen, maximize, minimize, urgency) act on
//   the view's NSWindow; macOS tab actions are inert because the single embedder window is
//   never part of a tab group.
// - Each terminal view belongs to one embedder session; the window ID is that session's key
//   and redraw requests wake only that session.
// - IME cursor area, mouse cursor icon/visibility and Option-as-Alt are applied to the view,
//   mirroring winit's implementation of the same calls.
// - `current_monitor` is replaced by `refresh_rate_millihertz`, the only use upstream.
// - On Linux the view is the GTK/Wayland `TerminalView` of `crate::platform::linux`: its raw
//   handles are the Wayland display and the view's own `wl_surface`; window-level actions
//   (fullscreen, maximize, minimize, hide, urgency hint) act on the GTK toplevel. macOS-only
//   upstream methods (tabs, simple fullscreen, Option-as-Alt, color space) stay macOS-only.
// - embed: Removed unused startup-only `set_maximized`/`set_fullscreen`; live toggle
//   actions remain, while initial toplevel presentation belongs to the embedder.

use std::fmt::{self, Display, Formatter};
#[cfg(target_os = "macos")]
use std::ptr::NonNull;

#[cfg(target_os = "macos")]
use objc2_app_kit::NSColorSpace;
#[cfg(target_os = "macos")]
use objc2_foundation::{NSPoint, NSSize};
use winit::dpi::{PhysicalPosition, PhysicalSize};
#[cfg(target_os = "macos")]
use winit::raw_window_handle::AppKitWindowHandle;
use winit::raw_window_handle::RawWindowHandle;
use winit::window::CursorIcon;

#[cfg(target_os = "linux")]
use gtk::prelude::*;

use alacritty_terminal::index::Point;

use crate::config::UiConfig;
use crate::display::SizeInfo;
use crate::host::WindowId;
use crate::platform::embedder::View;
#[cfg(target_os = "linux")]
use crate::platform::linux::TerminalView;
#[cfg(target_os = "macos")]
use crate::platform::macos::cursor::cursor_from_icon;
#[cfg(target_os = "macos")]
use crate::platform::macos::{self, TerminalView};

/// Window errors.
#[derive(Debug)]
pub enum Error {
    /// Error dealing with fonts.
    Font(crossfont::Error),
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Font(err) => err.source(),
        }
    }
}

impl Display for Error {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Error::Font(err) => err.fmt(f),
        }
    }
}

impl From<crossfont::Error> for Error {
    fn from(val: crossfont::Error) -> Self {
        Error::Font(val)
    }
}

/// The view which is used for displaying the terminal.
pub struct Window {
    /// Flag tracking that we have a frame we can draw.
    pub has_frame: bool,

    /// Cached scale factor for quickly scaling pixel sizes.
    pub scale_factor: f64,

    /// Flag indicating whether redraw was requested.
    pub requested_redraw: bool,

    /// Hold the window when terminal exits.
    pub hold: bool,

    /// The terminal title changed since the embedder last read it.
    pub(crate) title_changed: bool,

    view: View,

    /// Embedder session owning the view; also the window ID.
    session: crate::embed::SessionKey,

    /// Current window title.
    title: String,

    current_mouse_cursor: CursorIcon,
    mouse_visible: bool,
    #[cfg(target_os = "macos")]
    attention_request: Option<isize>,
}

impl Window {
    /// Wrap the terminal view.
    pub fn new(view: View, config: &UiConfig) -> Window {
        // Text cursor.
        let current_mouse_cursor = CursorIcon::Text;
        #[cfg(target_os = "macos")]
        view.set_cursor_icon(cursor_from_icon(current_mouse_cursor));
        #[cfg(target_os = "linux")]
        view.set_cursor_icon(current_mouse_cursor);

        // Enable IME.
        view.set_ime_allowed(true);

        #[cfg(target_os = "macos")]
        view.set_option_as_alt(config.window.option_as_alt());

        #[cfg(target_os = "macos")]
        use_srgb_color_space(&view);

        let scale_factor = view.scale_factor();
        log::info!("Window scale factor: {}", scale_factor);

        Self {
            hold: false,
            requested_redraw: false,
            title: config.window.identity.title.clone(),
            title_changed: true,
            current_mouse_cursor,
            mouse_visible: true,
            has_frame: true,
            #[cfg(target_os = "macos")]
            attention_request: None,
            scale_factor,
            session: view.session(),
            view,
        }
    }

    /// The terminal view.
    pub(crate) fn view(&self) -> &TerminalView {
        &self.view
    }

    #[cfg(target_os = "macos")]
    #[inline]
    pub fn raw_window_handle(&self) -> RawWindowHandle {
        let view = NonNull::from(&*self.view).cast();
        RawWindowHandle::AppKit(AppKitWindowHandle::new(view))
    }

    /// The view's own Wayland surface, the target of its EGL window surface.
    #[cfg(target_os = "linux")]
    #[inline]
    pub fn raw_window_handle(&self) -> RawWindowHandle {
        self.view.raw_window_handle()
    }

    #[inline]
    pub fn inner_size(&self) -> PhysicalSize<u32> {
        self.view.physical_size()
    }

    /// Set the window title.
    #[inline]
    pub fn set_title(&mut self, title: String) {
        if self.title != title {
            self.title = title;
            self.title_changed = true;
        }
    }

    /// Get the window title.
    #[inline]
    pub fn title(&self) -> &str {
        &self.title
    }

    #[inline]
    pub fn request_redraw(&mut self) {
        if !self.requested_redraw {
            self.requested_redraw = true;
            crate::embed::request_pump(self.session);
        }
    }

    #[inline]
    pub fn set_mouse_cursor(&mut self, cursor: CursorIcon) {
        if cursor != self.current_mouse_cursor {
            self.current_mouse_cursor = cursor;
            #[cfg(target_os = "macos")]
            self.view.set_cursor_icon(cursor_from_icon(cursor));
            #[cfg(target_os = "linux")]
            self.view.set_cursor_icon(cursor);
        }
    }

    /// Set mouse cursor visible.
    pub fn set_mouse_visible(&mut self, visible: bool) {
        if visible != self.mouse_visible {
            self.mouse_visible = visible;
            self.view.set_cursor_visible(visible);
        }
    }

    pub fn id(&self) -> WindowId {
        WindowId::from(self.session)
    }

    #[cfg(target_os = "macos")]
    pub fn set_urgent(&mut self, is_urgent: bool) {
        self.attention_request =
            macos::request_user_attention(self.attention_request.take(), is_urgent);
    }

    #[cfg(target_os = "macos")]
    pub fn set_minimized(&self, minimized: bool) {
        if let Some(window) = self.view.window() {
            if minimized {
                window.miniaturize(None);
            } else {
                unsafe { window.deminiaturize(None) };
            }
        }
    }

    /// Toggle the window's fullscreen state.
    #[cfg(target_os = "macos")]
    pub fn toggle_fullscreen(&self) {
        if let Some(window) = self.view.window() {
            window.toggleFullScreen(None);
        }
    }

    /// Toggle the window's maximized state.
    #[cfg(target_os = "macos")]
    pub fn toggle_maximized(&self) {
        if let Some(window) = self.view.window() {
            window.zoom(None);
        }
    }

    /// Inform windowing system about presenting to the window.
    ///
    /// Upstream forwards this to winit, which does nothing on macOS.
    #[cfg(target_os = "macos")]
    pub fn pre_present_notify(&self) {}

    /// Simple (non-Space) fullscreen is implemented by winit's own window delegate and is
    /// not available for the embedder-owned window.
    #[cfg(target_os = "macos")]
    pub fn toggle_simple_fullscreen(&self) {
        log::warn!("ToggleSimpleFullscreen is not supported by the embedded terminal");
    }

    /// Request (or clear) the user's attention through the toplevel's urgency hint.
    #[cfg(target_os = "linux")]
    pub fn set_urgent(&mut self, is_urgent: bool) {
        if let Some(window) = self.view.toplevel() {
            window.set_urgency_hint(is_urgent);
        }
    }

    #[cfg(target_os = "linux")]
    pub fn set_minimized(&self, minimized: bool) {
        if let Some(window) = self.view.toplevel() {
            if minimized {
                window.iconify();
            } else {
                window.deiconify();
            }
        }
    }

    /// Toggle the toplevel's fullscreen state.
    #[cfg(target_os = "linux")]
    pub fn toggle_fullscreen(&self) {
        if let Some(window) = self.view.toplevel() {
            if crate::platform::linux::is_fullscreen(&window) {
                window.unfullscreen();
            } else {
                window.fullscreen();
            }
        }
    }

    /// Toggle the toplevel's maximized state.
    #[cfg(target_os = "linux")]
    pub fn toggle_maximized(&self) {
        if let Some(window) = self.view.toplevel() {
            if window.is_maximized() {
                window.unmaximize();
            } else {
                window.maximize();
            }
        }
    }

    /// Upstream's non-macOS `Hide` action: hide the toplevel. Every session keeps running.
    #[cfg(target_os = "linux")]
    pub fn set_visible(&self, visible: bool) {
        if let Some(window) = self.view.toplevel() {
            if visible {
                window.show();
            } else {
                window.hide();
            }
        }
    }

    /// Inform windowing system about presenting to the window.
    ///
    /// Upstream's winit requests a Wayland frame callback here. The embedded view presents
    /// with a zero swap interval instead, paced by the scheduler's frame timer that
    /// `Display::draw` arms after every frame (`request_frame`, using the GdkMonitor refresh
    /// rate), so nothing waits for frame callbacks on the GTK main loop.
    #[cfg(target_os = "linux")]
    pub fn pre_present_notify(&self) {}

    /// Refresh rate of the display the view is on, in millihertz.
    #[cfg(target_os = "macos")]
    pub fn refresh_rate_millihertz(&self) -> Option<u32> {
        let screen = self.view.window()?.screen()?;
        let fps = unsafe { screen.maximumFramesPerSecond() };
        (fps > 0).then(|| fps as u32 * 1000)
    }

    /// Refresh rate of the GDK monitor showing the toplevel, in millihertz.
    #[cfg(target_os = "linux")]
    pub fn refresh_rate_millihertz(&self) -> Option<u32> {
        self.view.refresh_rate_millihertz()
    }

    pub fn set_ime_allowed(&self, allowed: bool) {
        self.view.set_ime_allowed(allowed);
    }

    /// Adjust the IME editor position according to the new location of the cursor.
    pub fn update_ime_position(&self, point: Point<usize>, size: &SizeInfo) {
        let nspot_x = f64::from(size.padding_x() + point.column.0 as f32 * size.cell_width());
        let nspot_y = f64::from(size.padding_y() + point.line as f32 * size.cell_height());

        // NOTE: some compositors don't like excluding too much and try to render popup at the
        // bottom right corner of the provided area, so exclude just the full-width char to not
        // obscure the cursor and not render popup at the end of the window.
        let width = size.cell_width() as f64 * 2.;
        let height = size.cell_height as f64;

        // Same physical -> logical conversion as winit's `set_ime_cursor_area`.
        let spot = PhysicalPosition::new(nspot_x, nspot_y).to_logical::<f64>(self.scale_factor);
        let area = PhysicalSize::new(width, height).to_logical::<f64>(self.scale_factor);
        #[cfg(target_os = "macos")]
        self.view.set_ime_cursor_area(
            NSPoint::new(spot.x, spot.y),
            NSSize::new(area.width, area.height),
        );
        #[cfg(target_os = "linux")]
        self.view.set_ime_cursor_area(spot.x, spot.y, area.width, area.height);
    }

    /// Select tab at the given `index`.
    ///
    /// The embedder window is never part of a macOS tab group, so there is nothing to select.
    #[cfg(target_os = "macos")]
    pub fn select_tab_at_index(&self, _index: usize) {}

    /// Select the last tab.
    #[cfg(target_os = "macos")]
    pub fn select_last_tab(&self) {}

    /// Select next tab.
    #[cfg(target_os = "macos")]
    pub fn select_next_tab(&self) {}

    /// Select previous tab.
    #[cfg(target_os = "macos")]
    pub fn select_previous_tab(&self) {}

    #[cfg(target_os = "macos")]
    pub fn tabbing_id(&self) -> String {
        String::new()
    }
}

/// Upstream sets the sRGB color space on the terminal window for consistent colors.
#[cfg(target_os = "macos")]
fn use_srgb_color_space(view: &TerminalView) {
    if let Some(window) = view.window() {
        unsafe { window.setColorSpace(Some(&NSColorSpace::sRGBColorSpace())) };
    }
}
