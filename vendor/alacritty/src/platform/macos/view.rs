//! The terminal `NSView`.
//!
//! Modified copy of winit 0.30.9 `src/platform_impl/macos/view.rs` (Apache-2.0), which is
//! the view upstream Alacritty receives its keyboard, IME and mouse input from.
//!
//! Changes:
//! - Events are queued to the embed host ([`crate::embed`]) instead of winit's application
//!   delegate, using the host [`KeyEvent`]/[`Modifiers`] types. Each view belongs to one
//!   embedder session and routes every event, batch and redraw request to that session.
//! - The view is a sibling of Tauri's WKWebView rather than a window's content view: focus is
//!   derived from first-responder status plus window key status (winit uses window key
//!   status), mouse-down claims first responder, window key/occlusion notifications are
//!   observed on whichever window the view moves to, and backing-scale changes are reported
//!   from `viewDidChangeBackingProperties` (winit reports them from its window delegate).
//! - While the view is hidden, size and scale-factor changes (AppKit autoresizing, display
//!   moves) are not reported; the current geometry is reported when the view is shown
//!   again (`viewDidUnhide`), so a hidden terminal never resizes its PTY. A hidden view
//!   refuses first responder, and [`TerminalView::transfer_first_responder`] hands it to the
//!   web view before the embedder hides the terminal.
//! - embed: Command key equivalents use the host-supplied physical key router. Matching
//!   application chords are handed to the host; unowned native menu chords (including
//!   Hide Others) pass through to AppKit. Other chords still reach Alacritty.
//! - embed: Layer-backed left corner masks match CSS; rounded views are not opaque.
//! - `copy:`/`paste:` responder actions run Alacritty's `Copy`/`Paste` actions for Edit menu
//!   clicks.
//! - `insertTab:`/`insertBackTab:` are not implemented so Tab never moves AppKit key view
//!   focus out of the terminal; `interpretKeyEvents:` reports them via
//!   `doCommandBySelector:` like any other command.
//! - Unused gesture/pressure events and the device event path are removed.

#![allow(clippy::unnecessary_cast)]

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Sel};
use objc2::{declare_class, msg_send, msg_send_id, mutability, sel, ClassType, DeclaredClass};
use objc2_app_kit::{
    NSApplication, NSCursor, NSEvent, NSEventModifierFlags, NSEventPhase, NSEventType,
    NSResponder, NSTextInputClient, NSTrackingArea, NSTrackingAreaOptions, NSView,
    NSViewBoundsDidChangeNotification, NSViewFrameDidChangeNotification, NSWindowDidBecomeKeyNotification,
    NSWindowDidChangeOcclusionStateNotification, NSWindowDidResignKeyNotification,
    NSWindowOcclusionState,
};
use objc2_foundation::{
    MainThreadMarker, NSArray, NSAttributedString, NSAttributedStringKey, NSCopying,
    NSMutableAttributedString, NSNotFound, NSNotificationCenter, NSObject, NSObjectProtocol,
    NSPoint, NSRange, NSRect, NSSize, NSString, NSUInteger,
};
use winit::dpi::{LogicalPosition, LogicalSize, PhysicalSize};
use winit::keyboard::{Key, KeyCode, KeyLocation, ModifiersState, NamedKey};
use winit::platform::macos::OptionAsAlt;

use super::cursor::{default_cursor, invisible_cursor};
use super::event::{
    code_to_key, code_to_location, create_key_event, event_mods, lalt_pressed, ralt_pressed,
    scancode_to_physicalkey,
};
use crate::embed::{self, EventBatch, SessionKey, ViewAction};
use crate::host::{
    ElementState, Ime, KeyEvent, Modifiers, MouseButton, MouseScrollDelta, TouchPhase,
    WindowEvent,
};

#[derive(Debug)]
struct CursorState {
    visible: bool,
    cursor: Retained<NSCursor>,
}

impl Default for CursorState {
    fn default() -> Self {
        Self { visible: true, cursor: default_cursor() }
    }
}

#[derive(Debug, Eq, PartialEq, Clone, Copy, Default)]
enum ImeState {
    #[default]
    /// The IME events are disabled, so only `ReceivedCharacter` is being sent to the user.
    Disabled,

    /// The ground state of enabled IME input. It means that both Preedit and regular keyboard
    /// input could be start from it.
    Ground,

    /// The IME is in preedit.
    Preedit,

    /// The text was just committed, so the next input from the keyboard must be ignored.
    Committed,
}

bitflags::bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq)]
    struct ModLocationMask: u8 {
        const LEFT     = 0b0001;
        const RIGHT    = 0b0010;
    }
}
impl ModLocationMask {
    fn from_location(loc: KeyLocation) -> ModLocationMask {
        match loc {
            KeyLocation::Left => ModLocationMask::LEFT,
            KeyLocation::Right => ModLocationMask::RIGHT,
            _ => unreachable!(),
        }
    }
}

fn key_to_modifier(key: &Key) -> Option<ModifiersState> {
    match key {
        Key::Named(NamedKey::Alt) => Some(ModifiersState::ALT),
        Key::Named(NamedKey::Control) => Some(ModifiersState::CONTROL),
        Key::Named(NamedKey::Super) => Some(ModifiersState::SUPER),
        Key::Named(NamedKey::Shift) => Some(ModifiersState::SHIFT),
        _ => None,
    }
}

fn get_right_modifier_code(key: &Key) -> KeyCode {
    match key {
        Key::Named(NamedKey::Alt) => KeyCode::AltRight,
        Key::Named(NamedKey::Control) => KeyCode::ControlRight,
        Key::Named(NamedKey::Shift) => KeyCode::ShiftRight,
        Key::Named(NamedKey::Super) => KeyCode::SuperRight,
        _ => unreachable!(),
    }
}

fn get_left_modifier_code(key: &Key) -> KeyCode {
    match key {
        Key::Named(NamedKey::Alt) => KeyCode::AltLeft,
        Key::Named(NamedKey::Control) => KeyCode::ControlLeft,
        Key::Named(NamedKey::Shift) => KeyCode::ShiftLeft,
        Key::Named(NamedKey::Super) => KeyCode::SuperLeft,
        _ => unreachable!(),
    }
}


/// embed: Translate one AppKit key event using physical kVK codes, never characters.
pub fn app_key(event: &NSEvent) -> embed::AppKey {
    let flags = unsafe { event.modifierFlags() };
    let code = unsafe { event.keyCode() };
    let named = match code {
        116 => Some(embed::NamedKey::PageUp), 121 => Some(embed::NamedKey::PageDown),
        126 => Some(embed::NamedKey::Up), 125 => Some(embed::NamedKey::Down),
        123 => Some(embed::NamedKey::Left), 124 => Some(embed::NamedKey::Right),
        115 => Some(embed::NamedKey::Home), 119 => Some(embed::NamedKey::End),
        _ => None,
    };
    embed::AppKey {
        ctrl: flags.contains(NSEventModifierFlags::NSEventModifierFlagControl),
        shift: flags.contains(NSEventModifierFlags::NSEventModifierFlagShift),
        alt: flags.contains(NSEventModifierFlags::NSEventModifierFlagOption),
        logo: flags.contains(NSEventModifierFlags::NSEventModifierFlagCommand),
        code, named,
    }
}

/// embed: Preserve the base view's native-menu exclusions if the app router declines.
/// This deliberately matches charactersIgnoringModifiers, as the pre-router menu did.
fn is_native_menu_shortcut(event: &NSEvent) -> bool {
    let chord = unsafe { event.modifierFlags() }
        & (NSEventModifierFlags::NSEventModifierFlagCommand
            | NSEventModifierFlags::NSEventModifierFlagShift
            | NSEventModifierFlags::NSEventModifierFlagControl
            | NSEventModifierFlags::NSEventModifierFlagOption);
    let command = NSEventModifierFlags::NSEventModifierFlagCommand;
    let command_option = command | NSEventModifierFlags::NSEventModifierFlagOption;
    let command_shift = command | NSEventModifierFlags::NSEventModifierFlagShift;
    if chord == command_shift {
        let code = unsafe { event.keyCode() };
        return code == 33 || code == 30; // Physical ANSI brackets: previous/next tab.
    }
    if chord != command && chord != command_option { return false; }
    let chars = unsafe { event.charactersIgnoringModifiers() }
        .map(|s| s.to_string().to_lowercase())
        .unwrap_or_default();
    match chars.as_str() {
        "q" | "w" | "m" | "t" | "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" => {
            chord == command
        },
        "h" => true,
        _ => false,
    }
}

#[derive(Debug)]
pub struct ViewState {
    /// Embedder session this view belongs to; fixed for the view's lifetime.
    session: SessionKey,
    corner_radii: Cell<[f64; 2]>,
    cursor_state: RefCell<CursorState>,
    ime_position: Cell<NSPoint>,
    ime_size: Cell<NSSize>,
    modifiers: Cell<Modifiers>,
    phys_modifiers: RefCell<HashMap<Key, ModLocationMask>>,
    ime_state: Cell<ImeState>,
    input_source: RefCell<String>,

    /// True iff the application wants IME events.
    ///
    /// Can be set using `set_ime_allowed`
    ime_allowed: Cell<bool>,

    /// True if the current key event should be forwarded
    /// to the application, even during IME
    forward_key_to_app: Cell<bool>,

    marked_text: RefCell<Retained<NSMutableAttributedString>>,

    /// The state of the `Option` as `Alt`.
    option_as_alt: Cell<OptionAsAlt>,

    /// Whether this view is currently the window's first responder.
    is_first_responder: Cell<bool>,

    /// Last focus state reported to the host.
    reported_focus: Cell<bool>,

    /// Last scale factor reported to the host.
    reported_scale_factor: Cell<f64>,

    /// Last nonzero view size reported to the host.
    reported_size: Cell<PhysicalSize<u32>>,

    /// Last window occlusion state reported to the host.
    reported_occluded: Cell<bool>,
}

declare_class!(
    pub struct TerminalView;

    unsafe impl ClassType for TerminalView {
        #[inherits(NSResponder, NSObject)]
        type Super = NSView;
        type Mutability = mutability::MainThreadOnly;
        const NAME: &'static str = "Ultra9000AlacrittyTerminalView";
    }

    impl DeclaredClass for TerminalView {
        type Ivars = ViewState;
    }

    unsafe impl TerminalView {
        #[method(isFlipped)]
        fn is_flipped(&self) -> bool {
            // Alacritty (through winit) uses the upper-left corner as the origin.
            true
        }

        #[method(isOpaque)]
        fn is_opaque(&self) -> bool {
            self.ivars().corner_radii.get() == [0.; 2]
        }

        #[method(viewDidMoveToWindow)]
        fn view_did_move_to_window(&self) {
            let _batch = EventBatch::new(self.session());
            self.observe_window();
            self.report_geometry();
            self.update_occlusion();
            self.update_focus();
        }

        #[method(frameDidChange:)]
        fn frame_did_change(&self, _event: &NSObject) {
            let _batch = EventBatch::new(self.session());
            self.report_geometry();
        }

        #[method(viewDidChangeBackingProperties)]
        fn view_did_change_backing_properties(&self) {
            let _batch = EventBatch::new(self.session());
            self.report_geometry();
        }

        #[method(viewDidUnhide)]
        fn view_did_unhide(&self) {
            // Report size/scale changes that happened while hidden.
            let _batch = EventBatch::new(self.session());
            self.report_geometry();
        }

        #[method(windowKeyStatusChanged:)]
        fn window_key_status_changed(&self, _notification: &NSObject) {
            let _batch = EventBatch::new(self.session());
            self.update_focus();
        }

        #[method(windowOcclusionChanged:)]
        fn window_occlusion_changed(&self, _notification: &NSObject) {
            let _batch = EventBatch::new(self.session());
            self.update_occlusion();
        }

        #[method(drawRect:)]
        fn draw_rect(&self, _rect: NSRect) {
            // AppKit asks for content (e.g. after the window is shown again). Rendering goes
            // through OpenGL, so only request a frame; no superclass drawing is needed.
            embed::request_redraw(self.session());
        }

        #[method(acceptsFirstResponder)]
        fn accepts_first_responder(&self) -> bool {
            // A hidden terminal must never receive keyboard input.
            !unsafe { self.isHiddenOrHasHiddenAncestor() }
        }

        #[method(becomeFirstResponder)]
        fn become_first_responder(&self) -> bool {
            let accepted: bool = unsafe { msg_send![super(self), becomeFirstResponder] };
            if accepted {
                let _batch = EventBatch::new(self.session());
                self.ivars().is_first_responder.set(true);
                self.update_focus();
            }
            accepted
        }

        #[method(resignFirstResponder)]
        fn resign_first_responder(&self) -> bool {
            let accepted: bool = unsafe { msg_send![super(self), resignFirstResponder] };
            if accepted {
                let _batch = EventBatch::new(self.session());
                self.ivars().is_first_responder.set(false);
                self.reset_modifiers();
                self.update_focus();
            }
            accepted
        }

        // This is necessary to prevent a beefy terminal error on MacBook Pros:
        // IMKInputSession [0x7fc573576ff0 presentFunctionRowItemTextInputViewWithEndpoint:completionHandler:] : [self textInputContext]=0x7fc573558e10 *NO* NSRemoteViewController to client, NSError=Error Domain=NSCocoaErrorDomain Code=4099 "The connection from pid 0 was invalidated from this process." UserInfo={NSDebugDescription=The connection from pid 0 was invalidated from this process.}, com.apple.inputmethod.EmojiFunctionRowItem
        #[method_id(touchBar)]
        fn touch_bar(&self) -> Option<Retained<NSObject>> {
            None
        }

        #[method(resetCursorRects)]
        fn reset_cursor_rects(&self) {
            let bounds = self.bounds();
            let cursor_state = self.ivars().cursor_state.borrow();
            // We correctly invoke `addCursorRect` only from inside `resetCursorRects`
            if cursor_state.visible {
                self.addCursorRect_cursor(bounds, &cursor_state.cursor);
            } else {
                self.addCursorRect_cursor(bounds, &invisible_cursor());
            }
        }

        #[method(performKeyEquivalent:)]
        fn perform_key_equivalent(&self, event: &NSEvent) -> bool {
            let is_key_down = unsafe { event.r#type() } == NSEventType::KeyDown;
            let has_command = unsafe { event.modifierFlags() }
                .contains(NSEventModifierFlags::NSEventModifierFlagCommand);

            // NOTE: No early `return`: `declare_class!` converts only the tail expression of
            // `bool` methods into the Objective-C `BOOL`.
            if is_key_down && has_command && self.ivars().is_first_responder.get() {
                // embed: The host's router consumes the chord here only if the local
                // monitor did not already consume it. No duplicate menu dispatch.
                if embed::application_shortcut(&app_key(event)) {
                    true
                } else if is_native_menu_shortcut(event) {
                    unsafe { msg_send![super(self), performKeyEquivalent: event] }
                } else {
                    self.handle_key_down(event);
                    true
                }
            } else {
                unsafe { msg_send![super(self), performKeyEquivalent: event] }
            }
        }

        #[method(copy:)]
        fn copy(&self, _sender: Option<&AnyObject>) {
            let _batch = EventBatch::new(self.session());
            embed::queue_view_action(self.session(), ViewAction::Copy);
        }

        #[method(paste:)]
        fn paste(&self, _sender: Option<&AnyObject>) {
            let _batch = EventBatch::new(self.session());
            embed::queue_view_action(self.session(), ViewAction::Paste);
        }
    }

    unsafe impl NSTextInputClient for TerminalView {
        #[method(hasMarkedText)]
        fn has_marked_text(&self) -> bool {
            self.ivars().marked_text.borrow().length() > 0
        }

        #[method(markedRange)]
        fn marked_range(&self) -> NSRange {
            let length = self.ivars().marked_text.borrow().length();
            if length > 0 {
                NSRange::new(0, length)
            } else {
                // Documented to return `{NSNotFound, 0}` if there is no marked range.
                NSRange::new(NSNotFound as NSUInteger, 0)
            }
        }

        #[method(selectedRange)]
        fn selected_range(&self) -> NSRange {
            // Documented to return `{NSNotFound, 0}` if there is no selection.
            NSRange::new(NSNotFound as NSUInteger, 0)
        }

        #[method(setMarkedText:selectedRange:replacementRange:)]
        fn set_marked_text(
            &self,
            string: &NSObject,
            selected_range: NSRange,
            _replacement_range: NSRange,
        ) {
            let _batch = EventBatch::new(self.session());

            // SAFETY: This method is guaranteed to get either a `NSString` or a `NSAttributedString`.
            let (marked_text, string) = if string.is_kind_of::<NSAttributedString>() {
                let string: *const NSObject = string;
                let string: *const NSAttributedString = string.cast();
                let string = unsafe { &*string };
                (
                    NSMutableAttributedString::from_attributed_nsstring(string),
                    string.string(),
                )
            } else {
                let string: *const NSObject = string;
                let string: *const NSString = string.cast();
                let string = unsafe { &*string };
                (
                    NSMutableAttributedString::from_nsstring(string),
                    string.copy(),
                )
            };

            // Update marked text.
            *self.ivars().marked_text.borrow_mut() = marked_text;

            // Notify IME is active if application still doesn't know it.
            if self.ivars().ime_state.get() == ImeState::Disabled {
                *self.ivars().input_source.borrow_mut() = self.current_input_source();
                self.queue_event(WindowEvent::Ime(Ime::Enabled));
            }

            if unsafe { self.hasMarkedText() } {
                self.ivars().ime_state.set(ImeState::Preedit);
            } else {
                // In case the preedit was cleared, set IME into the Ground state.
                self.ivars().ime_state.set(ImeState::Ground);
            }

            let cursor_range = if string.is_empty() {
                // An empty string basically means that there's no preedit, so indicate that by
                // sending a `None` cursor range.
                None
            } else {
                // Convert the selected range from UTF-16 indices to UTF-8 indices.
                let sub_string_a = unsafe { string.substringToIndex(selected_range.location) };
                let sub_string_b = unsafe { string.substringToIndex(selected_range.end()) };
                let lowerbound_utf8 = sub_string_a.len();
                let upperbound_utf8 = sub_string_b.len();
                Some((lowerbound_utf8, upperbound_utf8))
            };

            // Send WindowEvent for updating marked text
            self.queue_event(WindowEvent::Ime(Ime::Preedit(string.to_string(), cursor_range)));
        }

        #[method(unmarkText)]
        fn unmark_text(&self) {
            let _batch = EventBatch::new(self.session());
            *self.ivars().marked_text.borrow_mut() = NSMutableAttributedString::new();

            if let Some(input_context) = self.inputContext() {
                input_context.discardMarkedText();
            }

            self.queue_event(WindowEvent::Ime(Ime::Preedit(String::new(), None)));
            if self.is_ime_enabled() {
                // Leave the Preedit self.ivars()
                self.ivars().ime_state.set(ImeState::Ground);
            } else {
                log::warn!("Expected to have IME enabled when receiving unmarkText");
            }
        }

        #[method_id(validAttributesForMarkedText)]
        fn valid_attributes_for_marked_text(&self) -> Retained<NSArray<NSAttributedStringKey>> {
            NSArray::new()
        }

        #[method_id(attributedSubstringForProposedRange:actualRange:)]
        fn attributed_substring_for_proposed_range(
            &self,
            _range: NSRange,
            _actual_range: *mut NSRange,
        ) -> Option<Retained<NSAttributedString>> {
            None
        }

        #[method(characterIndexForPoint:)]
        fn character_index_for_point(&self, _point: NSPoint) -> NSUInteger {
            0
        }

        #[method(firstRectForCharacterRange:actualRange:)]
        fn first_rect_for_character_range(
            &self,
            _range: NSRange,
            _actual_range: *mut NSRange,
        ) -> NSRect {
            let rect = NSRect::new(
                self.ivars().ime_position.get(),
                self.ivars().ime_size.get()
            );
            // Return value is expected to be in screen coordinates, so we need a conversion here
            match self.window() {
                Some(window) => window.convertRectToScreen(self.convertRect_toView(rect, None)),
                None => rect,
            }
        }

        #[method(insertText:replacementRange:)]
        fn insert_text(&self, string: &NSObject, _replacement_range: NSRange) {
            let _batch = EventBatch::new(self.session());

            // SAFETY: This method is guaranteed to get either a `NSString` or a `NSAttributedString`.
            let string = if string.is_kind_of::<NSAttributedString>() {
                let string: *const NSObject = string;
                let string: *const NSAttributedString = string.cast();
                unsafe { &*string }.string().to_string()
            } else {
                let string: *const NSObject = string;
                let string: *const NSString = string.cast();
                unsafe { &*string }.to_string()
            };

            let is_control = string.chars().next().is_some_and(|c| c.is_control());

            // Commit only if we have marked text.
            if unsafe { self.hasMarkedText() } && self.is_ime_enabled() && !is_control {
                self.queue_event(WindowEvent::Ime(Ime::Preedit(String::new(), None)));
                self.queue_event(WindowEvent::Ime(Ime::Commit(string)));
                self.ivars().ime_state.set(ImeState::Committed);
            }
        }

        // Basically, we're sent this message whenever a keyboard event that doesn't generate a "human
        // readable" character happens, i.e. newlines, tabs, and Ctrl+C.
        #[method(doCommandBySelector:)]
        fn do_command_by_selector(&self, _command: Sel) {
            // We shouldn't forward any character from just committed text, since we'll end up sending
            // it twice with some IMEs like Korean one. We'll also always send `Enter` in that case,
            // which is not desired given it was used to confirm IME input.
            if self.ivars().ime_state.get() == ImeState::Committed {
                return;
            }

            self.ivars().forward_key_to_app.set(true);

            if unsafe { self.hasMarkedText() } && self.ivars().ime_state.get() == ImeState::Preedit
            {
                // Leave preedit so that we also report the key-up for this key.
                self.ivars().ime_state.set(ImeState::Ground);
            }
        }
    }

    unsafe impl TerminalView {
        #[method(keyDown:)]
        fn key_down(&self, event: &NSEvent) {
            self.handle_key_down(event);
        }

        #[method(keyUp:)]
        fn key_up(&self, event: &NSEvent) {
            let _batch = EventBatch::new(self.session());

            let event = replace_event(event, self.option_as_alt());
            self.update_modifiers(&event, false);

            // We want to send keyboard input when we are currently in the ground state.
            if matches!(
                self.ivars().ime_state.get(),
                ImeState::Ground | ImeState::Disabled
            ) {
                self.queue_event(WindowEvent::KeyboardInput {
                    event: create_key_event(&event, false, false),
                    is_synthetic: false,
                });
            }
        }

        #[method(flagsChanged:)]
        fn flags_changed(&self, event: &NSEvent) {
            let _batch = EventBatch::new(self.session());
            self.update_modifiers(event, true);
        }

        // Allows us to receive Cmd-. (the shortcut for closing a dialog)
        // https://bugs.eclipse.org/bugs/show_bug.cgi?id=300620#c6
        #[method(cancelOperation:)]
        fn cancel_operation(&self, _sender: Option<&AnyObject>) {
            let _batch = EventBatch::new(self.session());
            let mtm = MainThreadMarker::from(self);

            let event = match NSApplication::sharedApplication(mtm).currentEvent() {
                Some(event) => event,
                None => return,
            };

            self.update_modifiers(&event, false);
            let event = create_key_event(&event, true, unsafe { event.isARepeat() });

            self.queue_event(WindowEvent::KeyboardInput {
                event,
                is_synthetic: false,
            });
        }

        // In the past (?), `mouseMoved:` events were not generated when the
        // user hovered over a window from a separate window, and as such the
        // application might not know the location of the mouse in the event.
        //
        // To fix this, we emit `mouse_motion` inside of mouse click, mouse
        // scroll, magnify and other gesture event handlers, to ensure that
        // the application's state of where the mouse click was located is up
        // to date.
        //
        // See https://github.com/rust-windowing/winit/pull/1490 for history.

        #[method(mouseDown:)]
        fn mouse_down(&self, event: &NSEvent) {
            let _batch = EventBatch::new(self.session());
            self.claim_first_responder();
            self.mouse_motion(event);
            self.mouse_click(event, ElementState::Pressed);
        }

        #[method(mouseUp:)]
        fn mouse_up(&self, event: &NSEvent) {
            let _batch = EventBatch::new(self.session());
            self.mouse_motion(event);
            self.mouse_click(event, ElementState::Released);
        }

        #[method(rightMouseDown:)]
        fn right_mouse_down(&self, event: &NSEvent) {
            let _batch = EventBatch::new(self.session());
            self.claim_first_responder();
            self.mouse_motion(event);
            self.mouse_click(event, ElementState::Pressed);
        }

        #[method(rightMouseUp:)]
        fn right_mouse_up(&self, event: &NSEvent) {
            let _batch = EventBatch::new(self.session());
            self.mouse_motion(event);
            self.mouse_click(event, ElementState::Released);
        }

        #[method(otherMouseDown:)]
        fn other_mouse_down(&self, event: &NSEvent) {
            let _batch = EventBatch::new(self.session());
            self.claim_first_responder();
            self.mouse_motion(event);
            self.mouse_click(event, ElementState::Pressed);
        }

        #[method(otherMouseUp:)]
        fn other_mouse_up(&self, event: &NSEvent) {
            let _batch = EventBatch::new(self.session());
            self.mouse_motion(event);
            self.mouse_click(event, ElementState::Released);
        }

        #[method(mouseMoved:)]
        fn mouse_moved(&self, event: &NSEvent) {
            let _batch = EventBatch::new(self.session());
            self.mouse_motion(event);
        }

        #[method(mouseDragged:)]
        fn mouse_dragged(&self, event: &NSEvent) {
            let _batch = EventBatch::new(self.session());
            self.mouse_motion(event);
        }

        #[method(rightMouseDragged:)]
        fn right_mouse_dragged(&self, event: &NSEvent) {
            let _batch = EventBatch::new(self.session());
            self.mouse_motion(event);
        }

        #[method(otherMouseDragged:)]
        fn other_mouse_dragged(&self, event: &NSEvent) {
            let _batch = EventBatch::new(self.session());
            self.mouse_motion(event);
        }

        #[method(mouseEntered:)]
        fn mouse_entered(&self, _event: &NSEvent) {
            let _batch = EventBatch::new(self.session());
            self.queue_event(WindowEvent::CursorEntered);
        }

        #[method(mouseExited:)]
        fn mouse_exited(&self, _event: &NSEvent) {
            let _batch = EventBatch::new(self.session());
            self.queue_event(WindowEvent::CursorLeft);
        }

        #[method(scrollWheel:)]
        fn scroll_wheel(&self, event: &NSEvent) {
            let _batch = EventBatch::new(self.session());

            self.mouse_motion(event);

            let delta = {
                let (x, y) = unsafe { (event.scrollingDeltaX(), event.scrollingDeltaY()) };
                if unsafe { event.hasPreciseScrollingDeltas() } {
                    let delta = LogicalPosition::new(x, y).to_physical(self.scale_factor());
                    MouseScrollDelta::PixelDelta(delta)
                } else {
                    MouseScrollDelta::LineDelta(x as f32, y as f32)
                }
            };

            // The "momentum phase," if any, has higher priority than touch phase (the two should
            // be mutually exclusive anyhow, which is why the API is rather incoherent). If no momentum
            // phase is recorded (or rather, the started/ended cases of the momentum phase) then we
            // report the touch phase.
            #[allow(non_upper_case_globals)]
            let phase = match unsafe { event.momentumPhase() } {
                NSEventPhase::MayBegin | NSEventPhase::Began => TouchPhase::Started,
                NSEventPhase::Ended | NSEventPhase::Cancelled => TouchPhase::Ended,
                _ => match unsafe { event.phase() } {
                    NSEventPhase::MayBegin | NSEventPhase::Began => TouchPhase::Started,
                    NSEventPhase::Ended | NSEventPhase::Cancelled => TouchPhase::Ended,
                    _ => TouchPhase::Moved,
                },
            };

            self.update_modifiers(event, false);

            self.queue_event(WindowEvent::MouseWheel {
                delta,
                phase,
            });
        }

        // Allows us to receive Ctrl-Tab and Ctrl-Esc.
        // Note that this *doesn't* help with any missing Cmd inputs.
        // https://github.com/chromium/chromium/blob/a86a8a6bcfa438fa3ac2eba6f02b3ad1f8e0756f/ui/views/cocoa/bridged_content_view.mm#L816
        #[method(_wantsKeyDownForEvent:)]
        fn wants_key_down_for_event(&self, _event: &NSEvent) -> bool {
            true
        }

        #[method(acceptsFirstMouse:)]
        fn accepts_first_mouse(&self, _event: &NSEvent) -> bool {
            // winit's default, which Alacritty uses.
            true
        }
    }
);

impl TerminalView {
    pub(super) fn set_corner_radii(&self, size: NSSize, top: f64, bottom: f64) {
        self.ivars().corner_radii.set([top, bottom]);
        super::corner_mask::apply(self, size, top, bottom);
    }

    pub(crate) fn new(
        mtm: MainThreadMarker,
        frame: NSRect,
        option_as_alt: OptionAsAlt,
        session: SessionKey,
    ) -> Retained<Self> {
        let this = mtm.alloc().set_ivars(ViewState {
            session,
            corner_radii: Cell::new([0.; 2]),
            cursor_state: Default::default(),
            ime_position: Default::default(),
            ime_size: Default::default(),
            modifiers: Default::default(),
            phys_modifiers: Default::default(),
            ime_state: Default::default(),
            input_source: Default::default(),
            ime_allowed: Default::default(),
            forward_key_to_app: Default::default(),
            marked_text: Default::default(),
            option_as_alt: Cell::new(option_as_alt),
            is_first_responder: Cell::new(false),
            reported_focus: Cell::new(false),
            reported_scale_factor: Cell::new(0.),
            reported_size: Cell::new(PhysicalSize::new(0, 0)),
            reported_occluded: Cell::new(false),
        });
        let this: Retained<Self> = unsafe { msg_send_id![super(this), initWithFrame: frame] };

        this.setPostsFrameChangedNotifications(true);
        let notification_center = unsafe { NSNotificationCenter::defaultCenter() };
        unsafe {
            this.setPostsBoundsChangedNotifications(true);
            notification_center.addObserver_selector_name_object(
                &this,
                sel!(frameDidChange:),
                Some(NSViewFrameDidChangeNotification),
                Some(&this),
            );
            notification_center.addObserver_selector_name_object(
                &this,
                sel!(frameDidChange:),
                Some(NSViewBoundsDidChangeNotification),
                Some(&this),
            );
        }

        // Receive mouse motion and enter/exit only for the terminal's own visible area, so
        // hovering the sibling web view does not produce terminal mouse motion.
        let tracking_options = NSTrackingAreaOptions::NSTrackingMouseEnteredAndExited
            | NSTrackingAreaOptions::NSTrackingMouseMoved
            | NSTrackingAreaOptions::NSTrackingActiveInKeyWindow
            | NSTrackingAreaOptions::NSTrackingInVisibleRect;
        let tracking_area = unsafe {
            NSTrackingArea::initWithRect_options_owner_userInfo(
                mtm.alloc(),
                NSRect::ZERO,
                tracking_options,
                Some(&this),
                None,
            )
        };
        unsafe { this.addTrackingArea(&tracking_area) };

        *this.ivars().input_source.borrow_mut() = this.current_input_source();

        this
    }

    /// Embedder session this view belongs to.
    pub(crate) fn session(&self) -> SessionKey {
        self.ivars().session
    }

    fn queue_event(&self, event: WindowEvent) {
        embed::queue_window_event(self.session(), event);
    }

    /// `keyDown:` body, shared with command key equivalents.
    fn handle_key_down(&self, event: &NSEvent) {
        let _batch = EventBatch::new(self.session());
        {
            let mut prev_input_source = self.ivars().input_source.borrow_mut();
            let current_input_source = self.current_input_source();
            if *prev_input_source != current_input_source && self.is_ime_enabled() {
                *prev_input_source = current_input_source;
                drop(prev_input_source);
                self.ivars().ime_state.set(ImeState::Disabled);
                self.queue_event(WindowEvent::Ime(Ime::Disabled));
            }
        }

        // Get the characters from the event.
        let old_ime_state = self.ivars().ime_state.get();
        self.ivars().forward_key_to_app.set(false);
        let event = replace_event(event, self.option_as_alt());

        // The `interpretKeyEvents` function might call
        // `setMarkedText`, `insertText`, and `doCommandBySelector`.
        // It's important that we call this before queuing the KeyboardInput, because
        // we must send the `KeyboardInput` event during IME if it triggered
        // `doCommandBySelector`. (doCommandBySelector means that the keyboard input
        // is not handled by IME and should be handled by the application)
        if self.ivars().ime_allowed.get() {
            let events_for_nsview = NSArray::from_slice(&[&*event]);
            unsafe { self.interpretKeyEvents(&events_for_nsview) };

            // If the text was committed we must treat the next keyboard event as IME related.
            if self.ivars().ime_state.get() == ImeState::Committed {
                // Remove any marked text, so normal input can continue.
                *self.ivars().marked_text.borrow_mut() = NSMutableAttributedString::new();
            }
        }

        self.update_modifiers(&event, false);

        let had_ime_input = match self.ivars().ime_state.get() {
            ImeState::Committed => {
                // Allow normal input after the commit.
                self.ivars().ime_state.set(ImeState::Ground);
                true
            }
            ImeState::Preedit => true,
            // `key_down` could result in preedit clear, so compare old and current state.
            _ => old_ime_state != self.ivars().ime_state.get(),
        };

        if !had_ime_input || self.ivars().forward_key_to_app.get() {
            let key_event = create_key_event(&event, true, unsafe { event.isARepeat() });
            self.queue_event(WindowEvent::KeyboardInput {
                event: key_event,
                is_synthetic: false,
            });
        }
    }

    pub(crate) fn scale_factor(&self) -> f64 {
        match self.window() {
            Some(window) => window.backingScaleFactor() as f64,
            None => 1.0,
        }
    }

    /// Size of the view in physical pixels.
    pub(crate) fn physical_size(&self) -> PhysicalSize<u32> {
        let frame = self.frame();
        let logical_size = LogicalSize::new(frame.size.width as f64, frame.size.height as f64);
        logical_size.to_physical::<u32>(self.scale_factor())
    }

    fn is_ime_enabled(&self) -> bool {
        !matches!(self.ivars().ime_state.get(), ImeState::Disabled)
    }

    fn current_input_source(&self) -> String {
        self.inputContext()
            .and_then(|input_context| input_context.selectedKeyboardInputSource())
            .map(|input_source| input_source.to_string())
            .unwrap_or_default()
    }

    /// Make this view the first responder of its window.
    pub(crate) fn claim_first_responder(&self) {
        if let Some(window) = self.window() {
            if !self.ivars().is_first_responder.get() {
                let responder: &NSResponder = self;
                window.makeFirstResponder(Some(responder));
            }
        }
    }

    /// Hand first responder to `next` (the sibling web view) if this view holds it.
    ///
    /// Falls back to the window itself if `next` refuses, so the terminal never keeps
    /// keyboard focus. Must not be called while the embed host is borrowed: the responder
    /// change reports `Focused(false)` through the host.
    pub(crate) fn transfer_first_responder(&self, next: &NSView) {
        if !self.ivars().is_first_responder.get() {
            return;
        }
        let window = match self.window() {
            Some(window) => window,
            None => return,
        };

        let _batch = EventBatch::new(self.session());
        let next: &NSResponder = next;
        window.makeFirstResponder(Some(next));
        if self.ivars().is_first_responder.get() {
            window.makeFirstResponder(None);
        }
    }

    /// Observe key and occlusion state of the window the view is currently in.
    fn observe_window(&self) {
        let center = unsafe { NSNotificationCenter::defaultCenter() };

        // Drop observers for the previous window and re-add both geometry observers.
        unsafe {
            center.removeObserver(self);
            center.addObserver_selector_name_object(
                self,
                sel!(frameDidChange:),
                Some(NSViewFrameDidChangeNotification),
                Some(self),
            );
            center.addObserver_selector_name_object(
                self,
                sel!(frameDidChange:),
                Some(NSViewBoundsDidChangeNotification),
                Some(self),
            );
        }

        let window = match self.window() {
            Some(window) => window,
            None => return,
        };

        unsafe {
            center.addObserver_selector_name_object(
                self,
                sel!(windowKeyStatusChanged:),
                Some(NSWindowDidBecomeKeyNotification),
                Some(&window),
            );
            center.addObserver_selector_name_object(
                self,
                sel!(windowKeyStatusChanged:),
                Some(NSWindowDidResignKeyNotification),
                Some(&window),
            );
            center.addObserver_selector_name_object(
                self,
                sel!(windowOcclusionChanged:),
                Some(NSWindowDidChangeOcclusionStateNotification),
                Some(&window),
            );
        }
    }

    /// Report focus as "first responder of the key window".
    fn update_focus(&self) {
        let window_key = self.window().map_or(false, |window| window.isKeyWindow());
        let focused = window_key && self.ivars().is_first_responder.get();
        if focused != self.ivars().reported_focus.replace(focused) {
            self.queue_event(WindowEvent::Focused(focused));
        }
    }

    fn update_occlusion(&self) {
        let occluded = match self.window() {
            Some(window) => !window.occlusionState().contains(NSWindowOcclusionState::Visible),
            None => true,
        };
        if occluded != self.ivars().reported_occluded.replace(occluded) {
            self.queue_event(WindowEvent::Occluded(occluded));
            if !occluded {
                embed::request_redraw(self.session());
            }
        }
    }

    /// Report scale factor and size changes to the host, unless the view is hidden.
    ///
    /// Hidden geometry (e.g. from autoresizing) stays unreported; `viewDidUnhide` reports the
    /// then-current geometry.
    fn report_geometry(&self) {
        // AppKit autoresizing can change bounds before JS reports a new viewport.
        // Rebuild unequal-radius paths immediately, even while hidden; layer masks
        // do not otherwise resize their path with the terminal's backing layer.
        let [top, bottom] = self.ivars().corner_radii.get();
        if top > 0. || bottom > 0. {
            super::corner_mask::apply(self, self.bounds().size, top, bottom);
        }
        if self.window().is_none() || unsafe { self.isHiddenOrHasHiddenAncestor() } {
            return;
        }

        let scale_factor = self.scale_factor();
        let scale_changed = scale_factor != self.ivars().reported_scale_factor.replace(scale_factor);
        if scale_changed {
            self.queue_event(WindowEvent::ScaleFactorChanged { scale_factor });
        }

        let size = self.physical_size();
        if size.width != 0 && size.height != 0 {
            let size_changed = size != self.ivars().reported_size.replace(size);
            if scale_changed || size_changed {
                self.queue_event(WindowEvent::Resized(size));
            }
        }
    }

    /// Record the geometry the display was just created with as already reported.
    pub(crate) fn record_reported_geometry(&self) {
        self.ivars().reported_scale_factor.set(self.scale_factor());
        let size = self.physical_size();
        if size.width != 0 && size.height != 0 {
            self.ivars().reported_size.set(size);
        }
    }

    pub(crate) fn set_cursor_icon(&self, icon: Retained<NSCursor>) {
        let mut cursor_state = self.ivars().cursor_state.borrow_mut();
        cursor_state.cursor = icon;
        drop(cursor_state);
        self.invalidate_cursor_rects();
    }

    /// Set whether the cursor should be visible or not.
    pub(crate) fn set_cursor_visible(&self, visible: bool) {
        let mut cursor_state = self.ivars().cursor_state.borrow_mut();
        if visible != cursor_state.visible {
            cursor_state.visible = visible;
            drop(cursor_state);
            self.invalidate_cursor_rects();
        }
    }

    fn invalidate_cursor_rects(&self) {
        if let Some(window) = self.window() {
            window.invalidateCursorRectsForView(self);
        }
    }

    pub(crate) fn set_ime_allowed(&self, ime_allowed: bool) {
        if self.ivars().ime_allowed.get() == ime_allowed {
            return;
        }
        self.ivars().ime_allowed.set(ime_allowed);
        if self.ivars().ime_allowed.get() {
            return;
        }

        // Clear markedText
        *self.ivars().marked_text.borrow_mut() = NSMutableAttributedString::new();

        if self.ivars().ime_state.get() != ImeState::Disabled {
            self.ivars().ime_state.set(ImeState::Disabled);
            self.queue_event(WindowEvent::Ime(Ime::Disabled));
        }
    }

    pub(crate) fn set_ime_cursor_area(&self, position: NSPoint, size: NSSize) {
        self.ivars().ime_position.set(position);
        self.ivars().ime_size.set(size);
        if let Some(input_context) = self.inputContext() {
            input_context.invalidateCharacterCoordinates();
        }
    }

    /// Reset modifiers and emit a synthetic ModifiersChanged event if deemed necessary.
    pub(crate) fn reset_modifiers(&self) {
        if !self.ivars().modifiers.get().state().is_empty() {
            self.ivars().modifiers.set(Modifiers::default());
            self.queue_event(WindowEvent::ModifiersChanged(self.ivars().modifiers.get()));
        }
    }

    pub(crate) fn set_option_as_alt(&self, value: OptionAsAlt) {
        self.ivars().option_as_alt.set(value)
    }

    pub(crate) fn option_as_alt(&self) -> OptionAsAlt {
        self.ivars().option_as_alt.get()
    }

    /// Update modifiers if `event` has something different
    fn update_modifiers(&self, ns_event: &NSEvent, is_flags_changed_event: bool) {
        use ElementState::{Pressed, Released};

        let current_modifiers = event_mods(ns_event);
        let prev_modifiers = self.ivars().modifiers.get();
        self.ivars().modifiers.set(current_modifiers);

        // This function was called form the flagsChanged event, which is triggered
        // when the user presses/releases a modifier even if the same kind of modifier
        // has already been pressed.
        //
        // When flags changed event has key code of zero it means that event doesn't carry any key
        // event, thus we can't generate regular presses based on that. The `ModifiersChanged`
        // later will work though, since the flags are attached to the event and contain valid
        // information.
        'send_event: {
            if is_flags_changed_event && unsafe { ns_event.keyCode() } != 0 {
                let scancode = unsafe { ns_event.keyCode() };
                let physical_key = scancode_to_physicalkey(scancode as u32);

                let logical_key = code_to_key(physical_key, scancode);
                // Ignore processing of unknown modifiers because we can't determine whether
                // it was pressed or release reliably.
                //
                // Furthermore, sometimes normal keys are reported inside flagsChanged:, such as
                // when holding Caps Lock while pressing another key, see:
                // https://github.com/alacritty/alacritty/issues/8268
                let Some(event_modifier) = key_to_modifier(&logical_key) else {
                    break 'send_event;
                };

                let mut event = KeyEvent {
                    location: code_to_location(physical_key),
                    logical_key: logical_key.clone(),
                    physical_key,
                    repeat: false,
                    // We'll correct this later.
                    state: Pressed,
                    text: None,
                    text_with_all_modifiers: None,
                    key_without_modifiers: logical_key.clone(),
                };

                let location_mask = ModLocationMask::from_location(event.location);

                let mut phys_mod_state = self.ivars().phys_modifiers.borrow_mut();
                let phys_mod =
                    phys_mod_state.entry(logical_key).or_insert(ModLocationMask::empty());

                let is_active = current_modifiers.state().contains(event_modifier);
                let mut events = VecDeque::with_capacity(2);

                // There is no API for getting whether the button was pressed or released
                // during this event. For this reason we have to do a bit of magic below
                // to come up with a good guess whether this key was pressed or released.
                // (This is not trivial because there are multiple buttons that may affect
                // the same modifier)
                if !is_active {
                    event.state = Released;
                    if phys_mod.contains(ModLocationMask::LEFT) {
                        let mut event = event.clone();
                        event.location = KeyLocation::Left;
                        event.physical_key = get_left_modifier_code(&event.logical_key).into();
                        events.push_back(WindowEvent::KeyboardInput {
                            event,
                            is_synthetic: false,
                        });
                    }
                    if phys_mod.contains(ModLocationMask::RIGHT) {
                        event.location = KeyLocation::Right;
                        event.physical_key = get_right_modifier_code(&event.logical_key).into();
                        events.push_back(WindowEvent::KeyboardInput {
                            event,
                            is_synthetic: false,
                        });
                    }
                    *phys_mod = ModLocationMask::empty();
                } else {
                    if *phys_mod == location_mask {
                        // Here we hit a contradiction:
                        // The modifier state was "changed" to active,
                        // yet the only pressed modifier key was the one that we
                        // just got a change event for.
                        // This seemingly means that the only pressed modifier is now released,
                        // but at the same time the modifier became active.
                        //
                        // But this scenario is possible if we released modifiers
                        // while the application was not in focus. (Because we don't
                        // get informed of modifier key events while the application
                        // is not focused)

                        // In this case we prioritize the information
                        // about the current modifier state which means
                        // that the button was pressed.
                        event.state = Pressed;
                    } else {
                        phys_mod.toggle(location_mask);
                        let is_pressed = phys_mod.contains(location_mask);
                        event.state = if is_pressed { Pressed } else { Released };
                    }

                    events.push_back(WindowEvent::KeyboardInput {
                        event,
                        is_synthetic: false,
                    });
                }

                drop(phys_mod_state);

                for event in events {
                    self.queue_event(event);
                }
            }
        }

        if prev_modifiers == current_modifiers {
            return;
        }

        self.queue_event(WindowEvent::ModifiersChanged(self.ivars().modifiers.get()));
    }

    fn mouse_click(&self, event: &NSEvent, button_state: ElementState) {
        let button = mouse_button(event);

        self.update_modifiers(event, false);

        self.queue_event(WindowEvent::MouseInput {
            state: button_state,
            button,
        });
    }

    fn mouse_motion(&self, event: &NSEvent) {
        let window_point = unsafe { event.locationInWindow() };
        let view_point = self.convertPoint_fromView(window_point, None);
        let frame = self.frame();

        if view_point.x.is_sign_negative()
            || view_point.y.is_sign_negative()
            || view_point.x > frame.size.width
            || view_point.y > frame.size.height
        {
            let mouse_buttons_down = unsafe { NSEvent::pressedMouseButtons() };
            if mouse_buttons_down == 0 {
                // Point is outside of the client area (view) and no buttons are pressed
                return;
            }
        }

        let view_point = LogicalPosition::new(view_point.x, view_point.y);

        self.update_modifiers(event, false);

        self.queue_event(WindowEvent::CursorMoved {
            position: view_point.to_physical(self.scale_factor()),
        });
    }
}

/// Get the mouse button from the NSEvent.
fn mouse_button(event: &NSEvent) -> MouseButton {
    // The buttonNumber property only makes sense for the mouse events:
    // NSLeftMouse.../NSRightMouse.../NSOtherMouse...
    // For the other events, it's always set to 0.
    // MacOS only defines the left, right and middle buttons, 3..=31 are left as generic buttons,
    // but 3 and 4 are very commonly used as Back and Forward by hardware vendors and applications.
    match unsafe { event.buttonNumber() } {
        0 => MouseButton::Left,
        1 => MouseButton::Right,
        2 => MouseButton::Middle,
        3 => MouseButton::Back,
        4 => MouseButton::Forward,
        n => MouseButton::Other(n as u16),
    }
}

// NOTE: to get option as alt working we need to rewrite events
// we're getting from the operating system, which makes it
// impossible to provide such events as extra in `KeyEvent`.
fn replace_event(event: &NSEvent, option_as_alt: OptionAsAlt) -> Retained<NSEvent> {
    let ev_mods = event_mods(event).state;
    let ignore_alt_characters = match option_as_alt {
        OptionAsAlt::OnlyLeft if lalt_pressed(event) => true,
        OptionAsAlt::OnlyRight if ralt_pressed(event) => true,
        OptionAsAlt::Both if ev_mods.alt_key() => true,
        _ => false,
    } && !ev_mods.control_key()
        && !ev_mods.super_key();

    if ignore_alt_characters {
        let ns_chars = unsafe {
            event.charactersIgnoringModifiers().expect("expected characters to be non-null")
        };

        unsafe {
            NSEvent::keyEventWithType_location_modifierFlags_timestamp_windowNumber_context_characters_charactersIgnoringModifiers_isARepeat_keyCode(
                event.r#type(),
                event.locationInWindow(),
                event.modifierFlags(),
                event.timestamp(),
                event.windowNumber(),
                None,
                &ns_chars,
                &ns_chars,
                event.isARepeat(),
                event.keyCode(),
            )
            .unwrap()
        }
    } else {
        event.copy()
    }
}
