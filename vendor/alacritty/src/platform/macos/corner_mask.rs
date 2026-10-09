//! embed: Layer-backed native left-corner clipping (new code).
use std::ffi::c_void;
use std::ptr;

use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, Bool};
use objc2::{msg_send, msg_send_id, Encoding, RefEncode};
use objc2_app_kit::NSView;
use objc2_foundation::{NSPoint, NSRect, NSSize};

/// Opaque CoreGraphics path; its pointer must encode as `^{CGPath=}`, not `^v`.
#[repr(C)]
struct CGPath {
    _private: [u8; 0],
}

// SAFETY: This is CoreGraphics' Objective-C encoding for CGPathRef.
unsafe impl RefEncode for CGPath {
    const ENCODING_REF: Encoding = Encoding::Pointer(&Encoding::Struct("CGPath", &[]));
}

#[link(name = "QuartzCore", kind = "framework")]
extern "C" {}
#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGPathCreateMutable() -> *const CGPath;
    fn CGPathMoveToPoint(path: *const CGPath, transform: *const c_void, x: f64, y: f64);
    fn CGPathAddLineToPoint(path: *const CGPath, transform: *const c_void, x: f64, y: f64);
    fn CGPathAddArc(path: *const CGPath, transform: *const c_void, x: f64, y: f64,
        radius: f64, start: f64, end: f64, clockwise: bool);
    fn CGPathCloseSubpath(path: *const CGPath);
    fn CGPathRelease(path: *const CGPath);
}

/// AppKit owns the backing layer. The common equal/one-square case uses CALayer's
/// cornerRadius; arbitrary unequal radii use a shape mask, rebuilt on geometry changes.
pub(super) fn apply(view: &NSView, size: NSSize, top: f64, bottom: f64) {
    let limit = size.width.min(size.height).max(0.) / 2.;
    let (top, bottom) = (top.min(limit), bottom.min(limit));
    unsafe {
        let _: () = msg_send![view, setWantsLayer: Bool::YES];
        let layer: Retained<AnyObject> = msg_send_id![view, layer];
        let transaction = AnyClass::get("CATransaction").expect("QuartzCore CATransaction");
        let _: () = msg_send![transaction, begin];
        let _: () = msg_send![transaction, setDisableActions: Bool::YES];
        let flipped: Bool = msg_send![&*layer, isGeometryFlipped];
        // These names are layer coordinates, not the flipped NSView's coordinates.
        // kCALayerMinXMinYCorner = 1<<0; kCALayerMinXMaxYCorner = 1<<2.
        let (min_y_radius, max_y_radius) = if flipped.as_bool() {
            (top, bottom)
        } else {
            (bottom, top)
        };
        let mut corners: usize = 0;
        if min_y_radius > 0. { corners |= 1 << 0; }
        if max_y_radius > 0. { corners |= 1 << 2; }
        let _: () = msg_send![&*layer, setMasksToBounds: Bool::YES];
        let _: () = msg_send![&*layer, setAllowsEdgeAntialiasing: Bool::YES];
        let _: () = msg_send![&*layer, setOpaque: Bool::new(top == 0. && bottom == 0.)];
        if top == bottom || top == 0. || bottom == 0. {
            let _: () = msg_send![&*layer, setMask: ptr::null::<AnyObject>()];
            let _: () = msg_send![&*layer, setCornerRadius: top.max(bottom)];
            let _: () = msg_send![&*layer, setMaskedCorners: corners];
        } else {
            let _: () = msg_send![&*layer, setCornerRadius: 0_f64];
            let shape_class = AnyClass::get("CAShapeLayer").expect("QuartzCore CAShapeLayer");
            let shape: Retained<AnyObject> = msg_send_id![shape_class, new];
            let frame = NSRect::new(NSPoint::new(0., 0.), size);
            let _: () = msg_send![&*shape, setFrame: frame];
            let _: () = msg_send![&*shape, setContentsScale: view.window().map_or(1., |w| w.backingScaleFactor())];
            let path = CGPathCreateMutable();
            let transform = ptr::null();
            let (width, height) = (size.width, size.height);
            let (low, high) = (min_y_radius, max_y_radius);
            CGPathMoveToPoint(path, transform, low, 0.);
            CGPathAddLineToPoint(path, transform, width, 0.);
            CGPathAddLineToPoint(path, transform, width, height);
            CGPathAddLineToPoint(path, transform, high, height);
            CGPathAddArc(path, transform, high, height - high, high,
                std::f64::consts::FRAC_PI_2, std::f64::consts::PI, false);
            CGPathAddLineToPoint(path, transform, 0., low);
            CGPathAddArc(path, transform, low, low, low,
                std::f64::consts::PI, 3. * std::f64::consts::FRAC_PI_2, false);
            CGPathCloseSubpath(path);
            let _: () = msg_send![&*shape, setPath: path];
            CGPathRelease(path);
            let _: () = msg_send![&*layer, setMask: &*shape];
        }
        let _: () = msg_send![transaction, commit];
    }
}
