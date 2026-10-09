//! GTK/Wayland side of the embedding registry in [`crate::embed`] (new code).
//!
//! - [`WebView::from_raw`] receives the embedder's `WebKitWebView` widget, requires the GDK
//!   Wayland backend, replaces the web view's container (Tauri's otherwise empty `GtkBox`,
//!   the window's child) with a new `GtkOverlay` holding the web view and, above it, the
//!   terminals' input widgets, and opens a guest `wayland-client` connection on GTK's
//!   `wl_display` with its own event queue for the terminals' surfaces.
//! - Frames are integer logical pixels in the toplevel surface's coordinates, so subsurface
//!   positions and buffer sizes stay exact for the (integer) GTK scale factor.
//! - Main-loop work uses GLib sources on the default main context, the one Tauri's GTK event
//!   loop runs: idle sources for deferred pumps and wakeups (callable from any thread) and
//!   one-shot timeouts for scheduler deadlines.

use std::cell::RefCell;
use std::ffi::c_void;
use std::rc::Rc;
use std::time::Duration;

use gtk::glib::{self, ControlFlow, Priority};
use gtk::prelude::*;
use wayland_client::backend::Backend;
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::wl_compositor::WlCompositor;
use wayland_client::protocol::wl_region::WlRegion;
use wayland_client::protocol::wl_registry::WlRegistry;
use wayland_client::protocol::wl_subcompositor::WlSubcompositor;
use wayland_client::protocol::wl_subsurface::WlSubsurface;
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::{delegate_noop, Connection, Dispatch, EventQueue, QueueHandle};

use super::TerminalView;
use crate::config::UiConfig;
use crate::embed::{self, Deferred, SessionKey, ViewportRect};

/// A terminal view of one session.
pub(crate) type View = Rc<TerminalView>;

/// Terminal frame in logical pixels, relative to the GTK toplevel's surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Frame {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

/// Whether the caller runs on the GTK main thread.
pub(crate) fn is_main_thread() -> bool {
    gtk::is_initialized_main_thread()
}

/// The embedder's `WebKitWebView` with the overlay and Wayland connection of the terminals.
#[derive(Clone)]
pub(crate) struct WebView(Rc<Shared>);

struct Shared {
    web_view: gtk::Widget,
    overlay: gtk::Overlay,
    wayland: Wayland,
}

impl WebView {
    /// Prepare `web_view` (a `WebKitWebView` alone in a container that is the GTK window's
    /// child, as Tauri lays it out) for hosting terminals.
    ///
    /// # Safety
    ///
    /// `web_view` must point to a live `GtkWidget`.
    pub(crate) unsafe fn from_raw(web_view: *mut c_void) -> Result<Self, String> {
        if web_view.is_null() {
            return Err("web view pointer is null".into());
        }
        let web_view: gtk::Widget =
            unsafe { glib::translate::from_glib_none(web_view.cast::<gtk::ffi::GtkWidget>()) };

        let display = WidgetExt::display(&web_view);
        if !super::is_wayland(&display) {
            return Err("native terminals need GTK's Wayland backend; X11/XWayland is not \
                        supported (run on a Wayland session without GDK_BACKEND=x11)"
                .into());
        }
        let wl_display =
            super::wayland_display().ok_or_else(|| "no Wayland display connection".to_owned())?;
        let wayland = Wayland::connect(wl_display)?;

        // Tauri's undecorated-resize handler expects the web view's parent to be a direct
        // child of the GTK window, so the overlay takes the place of Tauri's (otherwise empty)
        // box in the window instead of being packed into the box.
        let toplevel =
            super::toplevel_of(&web_view).ok_or_else(|| "web view has no toplevel window".to_owned())?;
        let container = web_view
            .parent()
            .and_then(|parent| parent.downcast::<gtk::Container>().ok())
            .ok_or_else(|| "web view has no parent container".to_owned())?;
        if container.parent().as_ref() != Some(toplevel.upcast_ref::<gtk::Widget>()) {
            return Err("web view's container is not the window's child".into());
        }
        if container.children().len() != 1 {
            return Err("web view's container holds other widgets (a menu bar?)".into());
        }
        let had_focus = web_view.is_focus();

        let overlay = gtk::Overlay::new();
        container.remove(&web_view);
        toplevel.remove(&container);
        overlay.add(&web_view);
        toplevel.add(&overlay);
        overlay.show();
        web_view.show();
        if had_focus {
            web_view.grab_focus();
        }

        let web_view = Self(Rc::new(Shared { web_view, overlay, wayland }));
        log::info!(
            "Web view CSS pixel scale {} (gtk-xft-dpi {:?}, screen resolution {:?})",
            web_view.css_scale(),
            gtk::Settings::default().map(|settings| settings.gtk_xft_dpi()),
            gtk::gdk::Screen::default().map(|screen| screen.resolution()),
        );
        Ok(web_view)
    }

    pub(super) fn web_view(&self) -> &gtk::Widget {
        &self.0.web_view
    }

    pub(super) fn overlay(&self) -> &gtk::Overlay {
        &self.0.overlay
    }

    pub(super) fn wayland(&self) -> &Wayland {
        &self.0.wayland
    }

    /// The GTK toplevel window of the web view.
    pub(super) fn toplevel(&self) -> Option<gtk::Window> {
        super::toplevel_of(&self.0.web_view)
    }

    /// Top-left corner of the web view in toplevel surface coordinates.
    pub(super) fn origin(&self) -> Option<(i32, i32)> {
        let toplevel = self.toplevel()?;
        self.0.web_view.translate_coordinates(&toplevel, 0, 0)
    }

    /// GTK logical pixels per CSS pixel of the web view.
    ///
    /// WebKitGTK scales pages by its font DPI on top of the zoom level (`refreshInternalScaling`
    /// in WebKitWebViewBase.cpp, `fontDPI` in WebCore's GdkCairoUtilities.cpp and
    /// `SystemSettingsManagerProxy::xftDPI` in SystemSettingsManagerProxyGtk.cpp, WebKitGTK
    /// 2.52): the page zoom factor is `zoom-level × fontDPI / 96`, where fontDPI is GDK's
    /// screen resolution with GTK 3 (it includes `GDK_DPI_SCALE`), else `gtk-xft-dpi / 1024`,
    /// and DPI changes within 2% of 96 are ignored. The UI reports terminal rectangles in CSS
    /// pixels.
    fn css_scale(&self) -> f64 {
        let web_view = &self.0.web_view;
        let zoom = if web_view.find_property("zoom-level").is_some() {
            web_view.property::<f64>("zoom-level")
        } else {
            1.
        };

        let resolution = gtk::gdk::Screen::default().map_or(-1., |screen| screen.resolution());
        let xft_dpi = gtk::Settings::default().map_or(-1, |settings| settings.gtk_xft_dpi());
        let dpi = if resolution > 0. {
            resolution
        } else if xft_dpi > 0 {
            f64::from(xft_dpi) / 1024.
        } else {
            96.
        };
        let page_scale = dpi / 96.;
        let page_scale = if (page_scale - 1.).abs() > 0.02 { page_scale } else { 1. };

        let scale = zoom * page_scale;
        if scale.is_finite() && scale > 0. {
            scale
        } else {
            1.
        }
    }

    /// Create a hidden terminal view above the web view, starting at `frame` (the last shown
    /// terminal frame), else the web view's area.
    pub(crate) fn create_view(
        &self,
        frame: Option<Frame>,
        _config: &UiConfig,
        key: SessionKey,
    ) -> Result<View, String> {
        let frame = frame.unwrap_or_else(|| {
            let (x, y) = self.origin().unwrap_or((0, 0));
            let allocation = self.0.web_view.allocation();
            Frame { x, y, width: allocation.width(), height: allocation.height() }
        });
        let frame = if frame.width < 1 || frame.height < 1 {
            Frame { width: 800, height: 600, ..frame }
        } else {
            frame
        };

        TerminalView::new(self.clone(), frame, key)
    }

    /// Convert a web view rectangle (CSS pixels) into a toplevel frame, clipped to the web
    /// view.
    ///
    /// Returns `None` if nothing of the rectangle is visible.
    pub(crate) fn native_frame(&self, view: &View, rect: ViewportRect) -> Option<Frame> {
        let (origin_x, origin_y) = self.origin()?;
        let allocation = self.0.web_view.allocation();
        let (mut frame, radii) = native_geometry(
            rect, self.css_scale(), allocation.width(), allocation.height(),
        )?;
        view.set_corner_radii(radii[0], radii[1]);
        frame.x += origin_x;
        frame.y += origin_y;
        Some(frame)
    }
}

/// Pure CSS-to-GTK geometry, without accessing or initializing GTK.
fn native_geometry(
    rect: ViewportRect, scale: f64, allocation_width: i32, allocation_height: i32,
) -> Option<(Frame, [f64; 2])> {
    let (x, y) = (rect.x * scale, rect.y * scale);
    let (right, bottom) = ((rect.x + rect.width) * scale, (rect.y + rect.height) * scale);

    // Integer logical pixels: device pixel aligned for integer buffer scale factors.
    let min_x = x.max(0.).round();
    let min_y = y.max(0.).round();
    let max_x = right.min(f64::from(allocation_width)).round();
    let max_y = bottom.min(f64::from(allocation_height)).round();
    let width = max_x - min_x;
    let height = max_y - min_y;
    if width < 1. || height < 1. {
        return None;
    }

    // Frontend edges are rounded to whole CSS px (up to half a CSS px error).
    // Remove that uncertainty before quantizing exactly as the frame does: a
    // flush edge must not lose its arc at fractional WebKit page zoom. Anything
    // still outside the allocation is genuinely clipped and must stay square.
    let edge_error = 0.5 * scale;
    let left_visible = (x + edge_error).round() >= min_x;
    let top_visible = (y + edge_error).round() >= min_y;
    let bottom_visible = (bottom - edge_error).round() <= max_y;
    let limit = width.min(height) / 2.;
    let radii = [
        if left_visible && top_visible { (rect.top_left_radius * scale).min(limit) } else { 0. },
        if left_visible && bottom_visible { (rect.bottom_left_radius * scale).min(limit) } else { 0. },
    ];
    Some((Frame {
        x: min_x as i32, y: min_y as i32, width: width as i32, height: height as i32,
    }, radii))
}

#[cfg(test)]
mod geometry_tests {
    use super::{native_geometry, ViewportRect};

    fn rect(height: f64) -> ViewportRect {
        ViewportRect {
            x: 40., y: 0., width: 300., height,
            top_left_radius: 10., bottom_left_radius: 8.,
        }
    }

    #[test]
    fn rounded_flush_edges_keep_corners_at_fractional_css_zoom() {
        let (frame, radii) = native_geometry(rect(586.), 1.2, 500, 703).unwrap();
        assert_eq!(frame.height, 703);
        assert_eq!(radii, [12., 9.6]);
        // Exercise every CSS-rounding phase at the two common WebKit page zooms.
        for scale in [1.2, 1.25] {
            for height in 100..1000 {
                let css_height = (f64::from(height) / scale).round();
                let (frame, radii) = native_geometry(rect(css_height), scale, 500, height).unwrap();
                assert_eq!(frame.height, height);
                assert_eq!(radii, [10. * scale, 8. * scale]);
            }
        }
    }

    #[test]
    fn edge_rounding_tolerance_does_not_round_genuinely_clipped_corners() {
        let scale = 1.25;
        let base = rect(80.);
        // At the tolerance boundary, apply the same native-pixel rounding as
        // the frame (including its asymmetric handling of negative halves).
        let near = ViewportRect { x: -0.8, y: -0.8, height: 80.8, ..base };
        assert_eq!(native_geometry(near, scale, 500, 100).unwrap().1, [12.5, 10.]);
        let left = ViewportRect { x: -1., ..base };
        assert_eq!(native_geometry(left, scale, 500, 100).unwrap().1, [0., 0.]);
        let top = ViewportRect { y: -1., height: 81., ..base };
        assert_eq!(native_geometry(top, scale, 500, 100).unwrap().1, [0., 10.]);
        let bottom = ViewportRect { height: 81., ..base };
        assert_eq!(native_geometry(bottom, scale, 500, 100).unwrap().1, [12.5, 0.]);
    }
}

/// Guest connection on GTK's `wl_display`, with an event queue of its own.
pub(super) struct Wayland {
    pub(super) conn: Connection,
    queue: RefCell<EventQueue<WaylandState>>,
    pub(super) qh: QueueHandle<WaylandState>,
    pub(super) compositor: WlCompositor,
    pub(super) subcompositor: WlSubcompositor,
    pub(super) display: *mut c_void,
}

/// Dispatch state of the terminals' Wayland queue; no event needs handling.
pub(super) struct WaylandState;

impl Dispatch<WlRegistry, GlobalListContents> for WaylandState {
    fn event(
        _state: &mut Self,
        _registry: &WlRegistry,
        _event: <WlRegistry as wayland_client::Proxy>::Event,
        _data: &GlobalListContents,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

delegate_noop!(WaylandState: WlCompositor);
delegate_noop!(WaylandState: WlSubcompositor);
delegate_noop!(WaylandState: WlRegion);
delegate_noop!(WaylandState: WlSubsurface);
delegate_noop!(WaylandState: ignore WlSurface);

impl Wayland {
    fn connect(display: *mut c_void) -> Result<Self, String> {
        // SAFETY: GTK keeps its display connected for the lifetime of the process.
        let backend = unsafe { Backend::from_foreign_display(display.cast()) };
        let conn = Connection::from_backend(backend);
        let (globals, queue) = registry_queue_init::<WaylandState>(&conn)
            .map_err(|err| format!("Wayland registry: {err}"))?;
        let qh = queue.handle();
        // Version 3 for `set_buffer_scale`; 4 adds nothing that is needed.
        let compositor = globals
            .bind::<WlCompositor, _, _>(&qh, 3..=4, ())
            .map_err(|err| format!("wl_compositor: {err}"))?;
        let subcompositor = globals
            .bind::<WlSubcompositor, _, _>(&qh, 1..=1, ())
            .map_err(|err| format!("wl_subcompositor: {err}"))?;

        Ok(Self { conn, queue: RefCell::new(queue), qh, compositor, subcompositor, display })
    }

    /// Send queued requests and drain events of the terminals' queue.
    pub(super) fn flush(&self) {
        if let Err(err) = self.conn.flush() {
            log::warn!("Unable to flush Wayland requests: {err}");
        }
        if let Ok(mut queue) = self.queue.try_borrow_mut() {
            if let Err(err) = queue.dispatch_pending(&mut WaylandState) {
                log::warn!("Unable to dispatch Wayland events: {err}");
            }
        }
    }
}

/// Move the view to `frame`; while hidden, geometry is reported once it is shown.
pub(crate) fn place_view(view: &View, frame: Frame) {
    view.place(frame);
}

/// Show (map the subsurface, show the input widget) or hide the view.
pub(crate) fn set_view_hidden(view: &View, hidden: bool) {
    view.set_hidden(hidden);
}

/// Whether the view is shown with a nonzero area.
pub(crate) fn view_is_shown(view: &View) -> bool {
    view.is_shown()
}

/// Hand keyboard focus from the view to the web view, if the view holds it.
pub(crate) fn release_focus(view: &View, web_view: &WebView) {
    view.release_focus(web_view.web_view());
}

/// Make the view's input widget its window's focus widget.
pub(crate) fn take_focus(view: &View) -> bool {
    view.take_focus()
}

/// Remove the input widget and destroy the view's Wayland surface (after its GL surface).
pub(crate) fn detach_view(view: &View) {
    view.detach();
}

/// Drop `view` after the current GTK callback, which may be one of the view's own.
pub(crate) fn release_view_later(view: View) {
    let mut view = Some(view);
    glib::idle_add_local_full(Priority::DEFAULT, move || {
        drop(view.take());
        ControlFlow::Break
    });
}

/// Run `kind` for session `key` on the main loop after the current callback.
pub(crate) fn defer(kind: Deferred, key: SessionKey) {
    glib::idle_add_local_full(Priority::DEFAULT, move || {
        embed::run_deferred(kind, key);
        ControlFlow::Break
    });
}

/// [`defer`] from any thread (PTY reader and shutdown helper threads).
///
/// Uses idle priority, so continuous PTY output cannot starve GTK input and redraws; the
/// rendering it causes is paced by the scheduler's frame timer.
pub(crate) fn defer_from_any_thread(kind: Deferred, key: SessionKey) {
    glib::idle_add_full(Priority::DEFAULT_IDLE, move || {
        embed::run_deferred(kind, key);
        ControlFlow::Break
    });
}

/// Call [`embed::timer_fired`] for `key` and `generation` on the main loop after `delay`.
pub(crate) fn defer_after(delay: Duration, key: SessionKey, generation: usize) {
    // GLib timeouts have millisecond resolution; round up so the deadline has passed.
    let millis = delay.as_nanos().div_ceil(1_000_000);
    let delay = Duration::from_millis(u64::try_from(millis).unwrap_or(u64::MAX));
    glib::timeout_add_local_full(delay, Priority::DEFAULT, move || {
        embed::timer_fired(key, generation);
        ControlFlow::Break
    });
}
