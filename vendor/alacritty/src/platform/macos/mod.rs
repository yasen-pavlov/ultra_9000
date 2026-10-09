//! macOS view host.

mod corner_mask;
pub mod cursor;
pub mod embedder;
pub mod event;
pub mod ffi;
pub mod view;

use objc2_app_kit::{NSApplication, NSRequestUserAttentionType};
use objc2_foundation::MainThreadMarker;

pub use view::TerminalView;

fn main_thread() -> MainThreadMarker {
    MainThreadMarker::new().expect("AppKit call off the main thread")
}

/// `winit::platform::macos::ActiveEventLoopExtMacOS::hide_application`.
pub fn hide_application() {
    let app = NSApplication::sharedApplication(main_thread());
    app.hide(None);
}

/// `winit::platform::macos::ActiveEventLoopExtMacOS::hide_other_applications`.
pub fn hide_other_applications() {
    let app = NSApplication::sharedApplication(main_thread());
    app.hideOtherApplications(None);
}

/// Request or cancel critical user attention, like winit's `request_user_attention`.
///
/// Returns the request identifier to cancel later.
pub fn request_user_attention(previous: Option<isize>, urgent: bool) -> Option<isize> {
    let app = NSApplication::sharedApplication(main_thread());
    if let Some(request) = previous {
        unsafe { app.cancelUserAttentionRequest(request) };
    }

    urgent.then(|| app.requestUserAttention(NSRequestUserAttentionType::NSCriticalRequest))
}
