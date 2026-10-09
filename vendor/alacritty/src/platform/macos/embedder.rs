//! AppKit side of the embedding registry in [`crate::embed`] (not derived from winit).
//!
//! The registry itself is platform independent; this module provides the native pieces it
//! needs on macOS: the embedder's `WKWebView`, terminal `NSView` placement next to it,
//! first-responder handover, and libdispatch main-queue callbacks and timers.

use std::ffi::c_void;
use std::time::Duration;

use objc2::rc::Retained;
use objc2_app_kit::{NSAutoresizingMaskOptions, NSResponder, NSView, NSWindowOrderingMode};
use objc2_foundation::{MainThreadMarker, NSPoint, NSRect, NSSize};

use super::ffi::dispatch;
use super::TerminalView;
use crate::config::UiConfig;
use crate::embed::{self, Deferred, SessionKey, ViewportRect};

/// A terminal view of one session.
pub(crate) type View = Retained<TerminalView>;

/// Terminal view frame in its superview's coordinate space.
pub(crate) type Frame = NSRect;

/// Whether the caller runs on the AppKit main thread.
pub(crate) fn is_main_thread() -> bool {
    MainThreadMarker::new().is_some()
}

/// The embedder's `WKWebView`; terminal views are its siblings.
#[derive(Clone)]
pub(crate) struct WebView(Retained<NSView>);

impl WebView {
    /// Wrap the embedder's web view.
    ///
    /// # Safety
    ///
    /// `web_view` must point to a live `NSView`.
    pub(crate) unsafe fn from_raw(web_view: *mut c_void) -> Result<Self, String> {
        let web_view = unsafe { Retained::retain(web_view.cast::<NSView>()) }
            .ok_or_else(|| "web view pointer is null".to_owned())?;
        if unsafe { web_view.superview() }.is_none() {
            return Err("web view has no superview".into());
        }
        Ok(Self(web_view))
    }

    /// Create a hidden terminal view above the web view.
    ///
    /// It starts with `frame` (the last shown terminal frame), else the web view's frame, so
    /// the GL surface and PTY get the size they will most likely be shown with.
    pub(crate) fn create_view(
        &self,
        frame: Option<Frame>,
        config: &UiConfig,
        key: SessionKey,
    ) -> Result<View, String> {
        let mtm = MainThreadMarker::new()
            .ok_or_else(|| "terminal API called off the main thread".to_owned())?;
        let web_view = &self.0;
        let superview =
            unsafe { web_view.superview() }.ok_or_else(|| "web view has no superview".to_owned())?;

        let mut frame = frame.unwrap_or_else(|| web_view.frame());
        if frame.size.width < 1. || frame.size.height < 1. {
            frame.size = NSSize::new(800., 600.);
        }

        let view = TerminalView::new(mtm, frame, config.window.option_as_alt(), key);
        view.setHidden(true);

        // Keep the distance to every superview edge while the window live-resizes; the
        // embedder's layout report then sets the exact rectangle.
        unsafe {
            view.setAutoresizingMask(
                NSAutoresizingMaskOptions::NSViewWidthSizable
                    | NSAutoresizingMaskOptions::NSViewHeightSizable,
            )
        };
        unsafe {
            superview.addSubview_positioned_relativeTo(
                &view,
                NSWindowOrderingMode::NSWindowAbove,
                Some(web_view),
            )
        };

        Ok(view)
    }

    /// Convert a web view rectangle into the superview's coordinate space.
    ///
    /// Returns `None` if nothing of the rectangle is visible.
    pub(crate) fn native_frame(&self, view: &View, rect: ViewportRect) -> Option<Frame> {
        let superview = unsafe { view.superview() }?;
        let web_frame = self.0.frame();

        let x = web_frame.origin.x + rect.x;
        let y = if superview.isFlipped() {
            web_frame.origin.y + rect.y
        } else {
            web_frame.origin.y + web_frame.size.height - (rect.y + rect.height)
        };

        // Clip to both the web view and the superview.
        let bounds = superview.bounds();
        let min_x = x.max(web_frame.origin.x).max(bounds.origin.x);
        let min_y = y.max(web_frame.origin.y).max(bounds.origin.y);
        let max_x = (x + rect.width)
            .min(web_frame.origin.x + web_frame.size.width)
            .min(bounds.origin.x + bounds.size.width);
        let max_y = (y + rect.height)
            .min(web_frame.origin.y + web_frame.size.height)
            .min(bounds.origin.y + bounds.size.height);

        // Align to device pixels.
        let scale = view.scale_factor().max(1.);
        let align = |value: f64| (value * scale).round() / scale;
        let (min_x, min_y, max_x, max_y) = (align(min_x), align(min_y), align(max_x), align(max_y));

        let width = max_x - min_x;
        let height = max_y - min_y;
        if width < 1. || height < 1. {
            return None;
        }


        // A clipped edge must stay square rather than inventing a new panel corner.
        let aligned_x = align(x);
        let aligned_y = align(y);
        let left_visible = min_x == aligned_x;
        let (top_visible, bottom_visible) = if superview.isFlipped() {
            (min_y == aligned_y, max_y == align(y + rect.height))
        } else {
            (max_y == align(y + rect.height), min_y == aligned_y)
        };
        let limit = width.min(height) / 2.;
        view.set_corner_radii(
            NSSize::new(width, height),
            if left_visible && top_visible { rect.top_left_radius.min(limit) } else { 0. },
            if left_visible && bottom_visible { rect.bottom_left_radius.min(limit) } else { 0. },
        );
        Some(NSRect::new(
            NSPoint::new(min_x, min_y),
            NSSize::new(width, height),
        ))
    }
}

/// Move the view to `frame`. While hidden, the view defers the resulting resize until it is
/// shown.
pub(crate) fn place_view(view: &View, frame: Frame) {
    if view.frame() != frame {
        unsafe { view.setFrame(frame) };
    }
}

/// Show or hide the view. Showing reports geometry that changed while hidden
/// (`viewDidUnhide`).
pub(crate) fn set_view_hidden(view: &View, hidden: bool) {
    view.setHidden(hidden);
}

/// Whether the view is shown with a nonzero area.
pub(crate) fn view_is_shown(view: &View) -> bool {
    let frame = view.frame();
    !unsafe { view.isHidden() } && frame.size.width >= 1. && frame.size.height >= 1.
}

/// Hand first responder from the view to the web view, if the view holds it.
///
/// Must not be called while the session's host is borrowed: the responder change reports
/// `Focused(false)` through the host.
pub(crate) fn release_focus(view: &View, web_view: &WebView) {
    view.transfer_first_responder(&web_view.0);
}

/// Make the view its window's first responder.
pub(crate) fn take_focus(view: &View) -> bool {
    let window = match view.window() {
        Some(window) => window,
        None => return false,
    };
    let responder: &NSResponder = view;
    window.makeFirstResponder(Some(responder))
}

/// Remove the view from its superview.
pub(crate) fn detach_view(view: &View) {
    unsafe { view.removeFromSuperview() };
}

/// Drop `view` after the current AppKit callback, which may be one of the view's own.
pub(crate) fn release_view_later(view: View) {
    extern "C" fn release_view(view: *mut c_void) {
        drop(unsafe { Retained::from_raw(view.cast::<TerminalView>()) });
    }

    let view = Retained::into_raw(view);
    unsafe { dispatch::dispatch_async_f(dispatch::main_queue(), view.cast(), release_view) };
}

/// Run `kind` for session `key` on the main queue after the current callback.
pub(crate) fn defer(kind: Deferred, key: SessionKey) {
    extern "C" fn pump(context: *mut c_void) {
        embed::run_deferred(Deferred::Pump, context as SessionKey);
    }
    extern "C" fn wakeup(context: *mut c_void) {
        embed::run_deferred(Deferred::Wakeup, context as SessionKey);
    }
    extern "C" fn stopped(context: *mut c_void) {
        embed::run_deferred(Deferred::Stopped, context as SessionKey);
    }

    let work: dispatch::DispatchFunction = match kind {
        Deferred::Pump => pump,
        Deferred::Wakeup => wakeup,
        Deferred::Stopped => stopped,
    };
    unsafe { dispatch::dispatch_async_f(dispatch::main_queue(), key as *mut c_void, work) };
}

/// [`defer`] from any thread; the main queue accepts work from every thread.
pub(crate) fn defer_from_any_thread(kind: Deferred, key: SessionKey) {
    defer(kind, key);
}

/// Session and generation of an armed timer.
struct TimerContext {
    key: SessionKey,
    generation: usize,
}

/// Call [`embed::timer_fired`] for `key` and `generation` on the main queue after `delay`.
pub(crate) fn defer_after(delay: Duration, key: SessionKey, generation: usize) {
    extern "C" fn fired(context: *mut c_void) {
        let context = unsafe { Box::from_raw(context.cast::<TimerContext>()) };
        embed::timer_fired(context.key, context.generation);
    }

    let context = Box::into_raw(Box::new(TimerContext { key, generation }));
    let delay = i64::try_from(delay.as_nanos()).unwrap_or(i64::MAX);
    unsafe {
        let when = dispatch::dispatch_time(dispatch::DISPATCH_TIME_NOW, delay);
        dispatch::dispatch_after_f(when, dispatch::main_queue(), context.cast(), fired);
    }
}
