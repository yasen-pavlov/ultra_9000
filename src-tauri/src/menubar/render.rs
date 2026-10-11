//! Native vector attachments and SF Pro text. Geometry matches assets/icons/tray/*.svg.
use super::{accessibility_label, title_runs, Run, Tone};
use crate::tray_model::BarCounts;
use block2::RcBlock;
use objc2::{
    define_class, msg_send, rc::Retained, runtime::AnyObject, AnyThread, MainThreadMarker,
    MainThreadOnly,
};
use objc2_app_kit::{
    NSAppearanceCustomization, NSAppearanceNameAqua, NSAppearanceNameDarkAqua,
    NSAutoresizingMaskOptions, NSBezierPath, NSCellImagePosition, NSColor, NSFont,
    NSFontAttributeName, NSFontWeightMedium, NSFontWeightSemibold, NSForegroundColorAttributeName,
    NSImage, NSKernAttributeName, NSStatusBarButton, NSStringDrawing, NSTextAttachment,
    NSWindingRule,
};
use objc2_foundation::{
    NSArray, NSDictionary, NSKeyValueObservingOptions, NSMutableAttributedString, NSNumber,
    NSObject, NSObjectNSKeyValueObserverRegistration, NSPoint, NSRange, NSRect, NSSize, NSString,
};
use std::{cell::RefCell, ffi::c_void};

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "U9KMenuBarAppearanceObserver"]
    #[thread_kind = MainThreadOnly]
    struct AppearanceObserver;

    impl AppearanceObserver {
        #[unsafe(method(observeValueForKeyPath:ofObject:change:context:))]
        fn appearance_changed(&self, _key: Option<&NSString>, _object: Option<&AnyObject>,
            _change: Option<&NSDictionary<NSString, AnyObject>>, _context: *mut c_void) {
            crate::macos::defer_on_main(Box::new(refresh_appearance));
        }
    }
);

struct StatusState {
    button: Retained<NSStatusBarButton>,
    observer: Retained<AppearanceObserver>,
    counts: BarCounts,
    dark: bool,
    scale: f64,
}
impl Drop for StatusState {
    fn drop(&mut self) {
        unsafe {
            self.button.removeObserver_forKeyPath(
                &self.observer,
                &NSString::from_str("effectiveAppearance"),
            );
        }
    }
}
thread_local! {
    static STATUS: RefCell<Option<StatusState>> = const { RefCell::new(None) };
    static MARKS: RefCell<[Option<Retained<NSImage>>; 2]> = const { RefCell::new([None, None]) };
    static INDICATORS: RefCell<[Option<Retained<NSImage>>; 4]> = const { RefCell::new([None, None, None, None]) };
    static APPEARANCES: Retained<NSArray<NSString>> = unsafe {
        NSArray::from_slice(&[NSAppearanceNameDarkAqua, NSAppearanceNameAqua])
    };
}

fn dark_bar(button: &NSStatusBarButton) -> bool {
    APPEARANCES.with(|appearances| {
        button
            .effectiveAppearance()
            .bestMatchFromAppearancesWithNames(appearances)
            .is_some_and(|name| unsafe { *name == *NSAppearanceNameDarkAqua })
    })
}

/// tray-icon removes and recreates the native button when [tray] show changes. KVO follows
/// that lifetime; count-only menu refreshes do not allocate or redraw the title.
pub fn update(button: Option<Retained<NSStatusBarButton>>, counts: BarCounts) {
    let mtm = MainThreadMarker::new().expect("status title off main");
    STATUS.with(|cache| {
        let mut cache = cache.borrow_mut();
        let Some(button) = button else {
            *cache = None;
            return;
        };
        let dark = dark_bar(&button);
        let scale = button
            .window()
            .map_or(1., |window| window.backingScaleFactor());
        if let Some(state) = cache
            .as_mut()
            .filter(|state| Retained::as_ptr(&state.button) == Retained::as_ptr(&button))
        {
            if state.counts.title_key() == counts.title_key()
                && state.dark == dark
                && state.scale == scale
            {
                if state.counts != counts {
                    update_accessibility(&button, counts);
                    state.counts = counts;
                }
                return;
            }
            apply(&button, counts, dark);
            state.counts = counts;
            state.dark = dark;
            state.scale = scale;
        } else {
            let observer: Retained<AppearanceObserver> =
                unsafe { msg_send![AppearanceObserver::alloc(mtm), init] };
            unsafe {
                button.addObserver_forKeyPath_options_context(
                    &observer,
                    &NSString::from_str("effectiveAppearance"),
                    NSKeyValueObservingOptions::New,
                    std::ptr::null_mut(),
                );
            }
            apply(&button, counts, dark);
            *cache = Some(StatusState {
                button,
                observer,
                counts,
                dark,
                scale,
            });
        }
    });
}

pub(super) fn refresh_appearance() {
    STATUS.with(|cache| {
        let mut cache = cache.borrow_mut();
        if let Some(state) = cache.as_mut() {
            let dark = dark_bar(&state.button);
            let scale = state
                .button
                .window()
                .map_or(1., |window| window.backingScaleFactor());
            if dark != state.dark || scale != state.scale {
                apply(&state.button, state.counts, dark);
                state.dark = dark;
                state.scale = scale;
            }
        }
    });
}

pub fn highlight(open: bool) {
    STATUS.with(|cache| {
        if let Some(state) = cache.borrow().as_ref() {
            state.button.highlight(open);
        }
    });
}

fn rgb(hex: u32, alpha: f64) -> Retained<NSColor> {
    NSColor::colorWithSRGBRed_green_blue_alpha(
        f64::from((hex >> 16) & 255) / 255.,
        f64::from((hex >> 8) & 255) / 255.,
        f64::from(hex & 255) / 255.,
        alpha,
    )
}
fn box_rect(x: f64, y: f64, width: f64, height: f64) -> NSRect {
    NSRect::new(NSPoint::new(x, y), NSSize::new(width, height))
}

fn mark(unread: bool) -> Retained<NSImage> {
    MARKS.with(|images| {
        images.borrow_mut()[usize::from(unread)]
            .get_or_insert_with(|| draw_mark(unread))
            .clone()
    })
}

fn draw_mark(unread: bool) -> Retained<NSImage> {
    let block = RcBlock::new(move |_| {
        NSColor::blackColor().setFill();
        let u = NSBezierPath::bezierPath();
        u.moveToPoint(NSPoint::new(2., 4.));
        u.lineToPoint(NSPoint::new(5., 4.));
        u.lineToPoint(NSPoint::new(5., 10.));
        u.appendBezierPathWithArcWithCenter_radius_startAngle_endAngle_clockwise(
            NSPoint::new(6., 10.),
            1.,
            180.,
            0.,
            true,
        );
        u.lineToPoint(NSPoint::new(7., 4.));
        u.lineToPoint(NSPoint::new(10., 4.));
        u.lineToPoint(NSPoint::new(10., 10.));
        u.appendBezierPathWithArcWithCenter_radius_startAngle_endAngle_clockwise(
            NSPoint::new(6., 10.),
            4.,
            0.,
            180.,
            false,
        );
        u.closePath();
        u.fill();
        NSBezierPath::fillRect(box_rect(12., 12., 4., 2.));
        if unread {
            NSBezierPath::bezierPathWithOvalInRect(box_rect(12., 2., 5., 5.)).fill();
        }
        objc2::runtime::Bool::YES
    });
    let image = NSImage::imageWithSize_flipped_drawingHandler(NSSize::new(18., 18.), true, &block);
    image.setTemplate(true);
    image
}

fn indicator(ring: bool, dark: bool) -> Retained<NSImage> {
    INDICATORS.with(|images| {
        images.borrow_mut()[usize::from(ring) * 2 + usize::from(dark)]
            .get_or_insert_with(|| draw_indicator(ring, dark))
            .clone()
    })
}

fn draw_indicator(ring: bool, dark: bool) -> Retained<NSImage> {
    let block = RcBlock::new(move |_| {
        let purple = if dark { 0xc792ea } else { 0x7443a8 };
        if ring {
            rgb(purple, 1.).setFill();
            let path = NSBezierPath::bezierPathWithOvalInRect(box_rect(0.5, 0.5, 11., 11.));
            path.appendBezierPathWithOvalInRect(box_rect(2., 2., 8., 8.));
            path.setWindingRule(NSWindingRule::EvenOdd);
            path.fill();
            NSBezierPath::bezierPathWithOvalInRect(box_rect(4.5, 4.5, 3., 3.)).fill();
        } else {
            for (start, end, alpha) in [(45., 225., 0.35), (225., 405., 1.)] {
                rgb(purple, alpha).setStroke();
                let path = NSBezierPath::bezierPath();
                path.setLineWidth(2.);
                path.appendBezierPathWithArcWithCenter_radius_startAngle_endAngle_clockwise(
                    NSPoint::new(6., 6.),
                    4.5,
                    start,
                    end,
                    false,
                );
                path.stroke();
            }
        }
        objc2::runtime::Bool::YES
    });
    NSImage::imageWithSize_flipped_drawingHandler(NSSize::new(12., 12.), true, &block)
}

fn pill(text: String, dark: bool, scale: f64) -> Retained<NSImage> {
    let width = match text.len() {
        1 => 22.,
        2 => 29.,
        _ => 36.,
    };
    let block = RcBlock::new(move |_| {
        rgb(0xff9800, 1.).setFill();
        NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(
            box_rect(0., 0., width, 16.),
            8.,
            8.,
        )
        .fill();
        if !dark {
            let edge = 1. / scale;
            rgb(0xa86400, 1.).setStroke();
            let path = NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(
                box_rect(edge / 2., edge / 2., width - edge, 16. - edge),
                8. - edge / 2.,
                8. - edge / 2.,
            );
            path.setLineWidth(edge);
            path.stroke();
        }
        let ink = rgb(0x1d1200, 1.);
        ink.setFill();
        let bang = NSBezierPath::bezierPath();
        bang.moveToPoint(NSPoint::new(6.05, 4.));
        bang.lineToPoint(NSPoint::new(6.95, 4.));
        bang.curveToPoint_controlPoint1_controlPoint2(
            NSPoint::new(7.45, 4.5),
            NSPoint::new(7.226142, 4.),
            NSPoint::new(7.45, 4.223858),
        );
        bang.lineToPoint(NSPoint::new(7.2, 9.5));
        bang.lineToPoint(NSPoint::new(5.8, 9.5));
        bang.lineToPoint(NSPoint::new(5.55, 4.5));
        bang.curveToPoint_controlPoint1_controlPoint2(
            NSPoint::new(6.05, 4.),
            NSPoint::new(5.55, 4.223858),
            NSPoint::new(5.773858, 4.),
        );
        bang.closePath();
        bang.fill();
        NSBezierPath::bezierPathWithOvalInRect(box_rect(5.45, 10.15, 2.1, 2.1)).fill();
        let font =
            unsafe { NSFont::monospacedDigitSystemFontOfSize_weight(11., NSFontWeightSemibold) };
        let attrs: Retained<NSDictionary<NSString, AnyObject>> = unsafe {
            NSDictionary::from_slices(
                &[NSFontAttributeName, NSForegroundColorAttributeName],
                &[&*font as &AnyObject, &*ink as &AnyObject],
            )
        };
        let string = NSString::from_str(&text);
        // In a flipped image NSString drawing starts at the top; font.ascender positions
        // its baseline at the design's y=12 rather than vertically centering heuristically.
        unsafe {
            string
                .drawAtPoint_withAttributes(NSPoint::new(9.5, 12. - font.ascender()), Some(&attrs));
        }
        objc2::runtime::Bool::YES
    });
    NSImage::imageWithSize_flipped_drawingHandler(NSSize::new(width, 16.), true, &block)
}

fn attachment(
    image: Option<&NSImage>,
    bounds: NSRect,
    kern: f64,
) -> Retained<NSMutableAttributedString> {
    let attachment = NSTextAttachment::initWithData_ofType(NSTextAttachment::alloc(), None, None);
    attachment.setImage(image);
    attachment.setBounds(bounds);
    let run = NSMutableAttributedString::from_nsstring(&NSString::from_str("\u{fffc}"));
    unsafe {
        run.addAttribute_value_range(
            objc2_app_kit::NSAttachmentAttributeName,
            &attachment,
            NSRange::new(0, 1),
        );
        run.addAttribute_value_range(
            NSKernAttributeName,
            &NSNumber::new_f64(kern),
            NSRange::new(0, 1),
        );
    }
    run
}

fn apply(button: &NSStatusBarButton, counts: BarCounts, dark: bool) {
    let font = unsafe { NSFont::monospacedDigitSystemFontOfSize_weight(13., NSFontWeightMedium) };
    let title = NSMutableAttributedString::new();
    let scale = button
        .window()
        .map_or(1., |window| window.backingScaleFactor())
        .max(1.);
    for run in title_runs(&counts) {
        let run = match run {
            Run::Spacer(width) => attachment(None, box_rect(0., 0., width, 0.), 0.),
            Run::Pill(text) => {
                let image = pill(text, dark, scale);
                attachment(
                    Some(&image),
                    box_rect(0., -3.5, image.size().width, 16.),
                    0.,
                )
            }
            Run::Arc | Run::Ring => {
                let image = indicator(matches!(run, Run::Ring), dark);
                attachment(Some(&image), box_rect(0., -1.5, 12., 12.), 3.)
            }
            Run::Text {
                text,
                tone,
                kern_after,
            } => {
                let run = NSMutableAttributedString::from_nsstring(&NSString::from_str(&text));
                let color = rgb(
                    match (tone, dark) {
                        (Tone::Purple, true) => 0xc792ea,
                        (Tone::Purple, false) => 0x7443a8,
                        (Tone::Error, true) => 0xff8da1,
                        (Tone::Error, false) => 0xa8172f,
                    },
                    1.,
                );
                let range = NSRange::new(0, text.encode_utf16().count());
                unsafe {
                    run.addAttribute_value_range(NSFontAttributeName, &font, range);
                    run.addAttribute_value_range(NSForegroundColorAttributeName, &color, range);
                    run.addAttribute_value_range(
                        NSKernAttributeName,
                        &NSNumber::new_f64(kern_after),
                        range,
                    );
                }
                run
            }
        };
        title.appendAttributedString(&run);
    }
    button.setImage(Some(&mark(counts.unread > 0)));
    button.setImagePosition(NSCellImagePosition::ImageLeading);
    button.setImageHugsTitle(true);
    button.setAttributedTitle(&title);
    // tray-icon's overlay is sized before the attributed title exists. Keep its whole
    // hit target in sync as AppKit changes the variable-length button, including counts
    // beyond the original 18pt icon; no parallel status item or menu ownership.
    for view in button.subviews() {
        if view.class().name().to_bytes() == b"TaoTrayTarget" {
            view.setAutoresizingMask(
                NSAutoresizingMaskOptions::ViewWidthSizable
                    | NSAutoresizingMaskOptions::ViewHeightSizable,
            );
            view.setFrame(button.bounds());
        }
    }
    update_accessibility(button, counts);
}

fn update_accessibility(button: &NSStatusBarButton, counts: BarCounts) {
    let label = NSString::from_str(&accessibility_label(&counts));
    unsafe {
        let _: () = msg_send![button, setAccessibilityLabel: &*label];
        let _: () = msg_send![button, setAccessibilityHelp: &*NSString::from_str("Click for the sidebar. Control-click for the menu.")];
    }
}
