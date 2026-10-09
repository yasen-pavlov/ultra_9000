//! Linux (GTK 3 on Wayland) view host. New code, not upstream Alacritty or winit, except
//! where a file header names its source.
//!
//! Each terminal is a [`TerminalView`]: an input-only GTK widget in a `GtkOverlay` above the
//! embedder's `WebKitWebView` at the terminal rectangle, which receives keyboard (through a
//! `GtkIMMulticontext`), pointer and focus events, plus its own `wl_surface` that Alacritty's
//! EGL renderer draws into. While shown, that surface is a desynchronized subsurface of the
//! GTK toplevel's surface with an empty input region, so pointer input still reaches GTK.

pub mod embedder;
mod keyboard;
mod view;

use std::ffi::c_void;

use gtk::glib::translate::ToGlibPtr;
use gtk::glib::Cast;
use gtk::prelude::*;
use gtk::{gdk, glib};

pub use view::TerminalView;

/// Whether `display` is a GDK Wayland display.
fn is_wayland(display: &gdk::Display) -> bool {
    let wayland_type: glib::Type =
        unsafe { glib::translate::from_glib(gdk_wayland_sys::gdk_wayland_display_get_type()) };
    display.type_().is_a(wayland_type)
}

/// The `wl_display` of the default GDK display, if it uses the Wayland backend.
pub fn wayland_display() -> Option<*mut c_void> {
    let display = gdk::Display::default()?;
    if !is_wayland(&display) {
        return None;
    }
    let raw: *mut gdk::ffi::GdkDisplay = display.to_glib_none().0;
    let wl_display = unsafe { gdk_wayland_sys::gdk_wayland_display_get_wl_display(raw.cast()) };
    (!wl_display.is_null()).then_some(wl_display)
}

/// The `wl_surface` of a toplevel GDK Wayland window; null while the window is unmapped.
fn wl_surface_of(window: &gdk::Window) -> *mut c_void {
    let raw: *mut gdk::ffi::GdkWindow = window.to_glib_none().0;
    unsafe { gdk_wayland_sys::gdk_wayland_window_get_wl_surface(raw.cast()) }
}

/// Whether the GTK toplevel is currently fullscreen.
pub fn is_fullscreen(window: &gtk::Window) -> bool {
    window
        .window()
        .is_some_and(|window| window.state().contains(gdk::WindowState::FULLSCREEN))
}

/// The GTK toplevel window containing `widget`.
fn toplevel_of(widget: &gtk::Widget) -> Option<gtk::Window> {
    let toplevel = widget.toplevel()?;
    if !toplevel.is_toplevel() {
        return None;
    }
    toplevel.downcast::<gtk::Window>().ok()
}
