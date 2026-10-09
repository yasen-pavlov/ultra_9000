//! NSEvent to key event conversion.
//!
//! Modified copy of winit 0.30.9 `src/platform_impl/macos/event.rs` (Apache-2.0).
//! Changes:
//! - Produces the embed host [`KeyEvent`]/[`Modifiers`] instead of winit's types, whose
//!   platform-specific fields cannot be constructed outside winit.
//! - `scancode_to_physicalkey` is taken from winit's public `PhysicalKeyExtScancode`.
//! - CoreFoundation calls use local FFI declarations; `LMGetKbdType` is called directly
//!   since all callers run on the main thread.
//! - `tracing` diagnostics use `log`.

use std::ffi::c_void;

use objc2_app_kit::{NSEvent, NSEventModifierFlags};
use winit::keyboard::{
    Key, KeyCode, KeyLocation, ModifiersState, NamedKey, NativeKey, NativeKeyCode, PhysicalKey,
    SmolStr,
};
use winit::platform::scancode::PhysicalKeyExtScancode;

use super::ffi;
use crate::host::{ElementState, KeyEvent, Modifiers, ModifiersKeys};

pub fn scancode_to_physicalkey(scancode: u32) -> PhysicalKey {
    PhysicalKey::from_scancode(scancode)
}

/// Ignores ALL modifiers.
pub fn get_modifierless_char(scancode: u16) -> Key {
    let mut string = [0; 16];
    let input_source;
    let layout;
    unsafe {
        input_source = ffi::TISCopyCurrentKeyboardLayoutInputSource();
        if input_source.is_null() {
            log::error!("`TISCopyCurrentKeyboardLayoutInputSource` returned null ptr");
            return Key::Unidentified(NativeKey::MacOS(scancode));
        }
        let layout_data =
            ffi::TISGetInputSourceProperty(input_source, ffi::kTISPropertyUnicodeKeyLayoutData);
        if layout_data.is_null() {
            ffi::CFRelease(input_source as *mut c_void);
            log::error!("`TISGetInputSourceProperty` returned null ptr");
            return Key::Unidentified(NativeKey::MacOS(scancode));
        }
        layout = ffi::CFDataGetBytePtr(layout_data as ffi::CFDataRef) as *const ffi::UCKeyboardLayout;
    }
    let keyboard_type = unsafe { ffi::LMGetKbdType() };

    let mut result_len = 0;
    let mut dead_keys = 0;
    let modifiers = 0;
    let translate_result = unsafe {
        ffi::UCKeyTranslate(
            layout,
            scancode,
            ffi::kUCKeyActionDisplay,
            modifiers,
            keyboard_type as u32,
            ffi::kUCKeyTranslateNoDeadKeysMask,
            &mut dead_keys,
            string.len() as ffi::UniCharCount,
            &mut result_len,
            string.as_mut_ptr(),
        )
    };
    unsafe {
        ffi::CFRelease(input_source as *mut c_void);
    }
    if translate_result != 0 {
        log::error!("`UCKeyTranslate` returned with the non-zero value: {}", translate_result);
        return Key::Unidentified(NativeKey::MacOS(scancode));
    }
    if result_len == 0 {
        // This is fine - not all keys have text representation.
        // For instance, users that have mapped the `Fn` key to toggle
        // keyboard layouts will hit this code path.
        return Key::Unidentified(NativeKey::MacOS(scancode));
    }
    let chars = String::from_utf16_lossy(&string[0..result_len as usize]);
    Key::Character(SmolStr::new(chars))
}

// Ignores all modifiers except for SHIFT (yes, even ALT is ignored).
fn get_logical_key_char(ns_event: &NSEvent, modifierless_chars: &str) -> Key {
    let string = unsafe { ns_event.charactersIgnoringModifiers() }
        .map(|s| s.to_string())
        .unwrap_or_default();
    if string.is_empty() {
        // Probably a dead key
        let first_char = modifierless_chars.chars().next();
        return Key::Dead(first_char);
    }
    Key::Character(SmolStr::new(string))
}

/// Create `KeyEvent` for the given `NSEvent`.
///
/// This function shouldn't be called when the IME input is in process.
pub(crate) fn create_key_event(ns_event: &NSEvent, is_press: bool, is_repeat: bool) -> KeyEvent {
    use ElementState::{Pressed, Released};
    let state = if is_press { Pressed } else { Released };

    let scancode = unsafe { ns_event.keyCode() };
    let mut physical_key = scancode_to_physicalkey(scancode as u32);

    // NOTE: The logical key should heed both SHIFT and ALT if possible.
    // For instance:
    // * Pressing the A key: logical key should be "a"
    // * Pressing SHIFT A: logical key should be "A"
    // * Pressing CTRL SHIFT A: logical key should also be "A"
    // This is not easy to tease out of `NSEvent`, but we do our best.

    let characters = unsafe { ns_event.characters() }.map(|s| s.to_string()).unwrap_or_default();
    let text_with_all_modifiers = if characters.is_empty() {
        None
    } else {
        if matches!(physical_key, PhysicalKey::Unidentified(_)) {
            // The key may be one of the funky function keys
            physical_key = extra_function_key_to_code(scancode, &characters);
        }
        Some(SmolStr::new(characters))
    };

    let key_from_code = code_to_key(physical_key, scancode);
    let (logical_key, key_without_modifiers) = if matches!(key_from_code, Key::Unidentified(_)) {
        // `get_modifierless_char/key_without_modifiers` ignores ALL modifiers.
        let key_without_modifiers = get_modifierless_char(scancode);

        let modifiers = unsafe { ns_event.modifierFlags() };
        let has_ctrl = modifiers.contains(NSEventModifierFlags::NSEventModifierFlagControl);
        let has_cmd = modifiers.contains(NSEventModifierFlags::NSEventModifierFlagCommand);

        let logical_key = match text_with_all_modifiers.as_ref() {
            // Only checking for ctrl and cmd here, not checking for alt because we DO want to
            // include its effect in the key. For example if -on the Germay layout- one
            // presses alt+8, the logical key should be "{"
            // Also not checking if this is a release event because then this issue would
            // still affect the key release.
            Some(text) if !has_ctrl && !has_cmd => {
                // Character heeding both SHIFT and ALT.
                Key::Character(text.clone())
            },

            _ => match key_without_modifiers.as_ref() {
                // Character heeding just SHIFT, ignoring ALT.
                Key::Character(ch) => get_logical_key_char(ns_event, ch),

                // Character ignoring ALL modifiers.
                _ => key_without_modifiers.clone(),
            },
        };

        (logical_key, key_without_modifiers)
    } else {
        (key_from_code.clone(), key_from_code)
    };

    let text = if is_press { logical_key.to_text().map(SmolStr::new) } else { None };

    let location = code_to_location(physical_key);

    KeyEvent {
        location,
        logical_key,
        physical_key,
        repeat: is_repeat,
        state,
        text,
        text_with_all_modifiers,
        key_without_modifiers,
    }
}

pub fn code_to_key(key: PhysicalKey, scancode: u16) -> Key {
    let code = match key {
        PhysicalKey::Code(code) => code,
        PhysicalKey::Unidentified(code) => return Key::Unidentified(code.into()),
    };

    Key::Named(match code {
        KeyCode::Enter => NamedKey::Enter,
        KeyCode::Tab => NamedKey::Tab,
        KeyCode::Space => NamedKey::Space,
        KeyCode::Backspace => NamedKey::Backspace,
        KeyCode::Escape => NamedKey::Escape,
        KeyCode::SuperRight => NamedKey::Super,
        KeyCode::SuperLeft => NamedKey::Super,
        KeyCode::ShiftLeft => NamedKey::Shift,
        KeyCode::AltLeft => NamedKey::Alt,
        KeyCode::ControlLeft => NamedKey::Control,
        KeyCode::ShiftRight => NamedKey::Shift,
        KeyCode::AltRight => NamedKey::Alt,
        KeyCode::ControlRight => NamedKey::Control,

        KeyCode::NumLock => NamedKey::NumLock,
        KeyCode::AudioVolumeUp => NamedKey::AudioVolumeUp,
        KeyCode::AudioVolumeDown => NamedKey::AudioVolumeDown,

        // Other numpad keys all generate text on macOS (if I understand correctly)
        KeyCode::NumpadEnter => NamedKey::Enter,

        KeyCode::F1 => NamedKey::F1,
        KeyCode::F2 => NamedKey::F2,
        KeyCode::F3 => NamedKey::F3,
        KeyCode::F4 => NamedKey::F4,
        KeyCode::F5 => NamedKey::F5,
        KeyCode::F6 => NamedKey::F6,
        KeyCode::F7 => NamedKey::F7,
        KeyCode::F8 => NamedKey::F8,
        KeyCode::F9 => NamedKey::F9,
        KeyCode::F10 => NamedKey::F10,
        KeyCode::F11 => NamedKey::F11,
        KeyCode::F12 => NamedKey::F12,
        KeyCode::F13 => NamedKey::F13,
        KeyCode::F14 => NamedKey::F14,
        KeyCode::F15 => NamedKey::F15,
        KeyCode::F16 => NamedKey::F16,
        KeyCode::F17 => NamedKey::F17,
        KeyCode::F18 => NamedKey::F18,
        KeyCode::F19 => NamedKey::F19,
        KeyCode::F20 => NamedKey::F20,

        KeyCode::Insert => NamedKey::Insert,
        KeyCode::Home => NamedKey::Home,
        KeyCode::PageUp => NamedKey::PageUp,
        KeyCode::Delete => NamedKey::Delete,
        KeyCode::End => NamedKey::End,
        KeyCode::PageDown => NamedKey::PageDown,
        KeyCode::ArrowLeft => NamedKey::ArrowLeft,
        KeyCode::ArrowRight => NamedKey::ArrowRight,
        KeyCode::ArrowDown => NamedKey::ArrowDown,
        KeyCode::ArrowUp => NamedKey::ArrowUp,
        _ => return Key::Unidentified(NativeKey::MacOS(scancode)),
    })
}

pub fn code_to_location(key: PhysicalKey) -> KeyLocation {
    let code = match key {
        PhysicalKey::Code(code) => code,
        PhysicalKey::Unidentified(_) => return KeyLocation::Standard,
    };

    match code {
        KeyCode::SuperRight => KeyLocation::Right,
        KeyCode::SuperLeft => KeyLocation::Left,
        KeyCode::ShiftLeft => KeyLocation::Left,
        KeyCode::AltLeft => KeyLocation::Left,
        KeyCode::ControlLeft => KeyLocation::Left,
        KeyCode::ShiftRight => KeyLocation::Right,
        KeyCode::AltRight => KeyLocation::Right,
        KeyCode::ControlRight => KeyLocation::Right,

        KeyCode::NumLock => KeyLocation::Numpad,
        KeyCode::NumpadDecimal => KeyLocation::Numpad,
        KeyCode::NumpadMultiply => KeyLocation::Numpad,
        KeyCode::NumpadAdd => KeyLocation::Numpad,
        KeyCode::NumpadDivide => KeyLocation::Numpad,
        KeyCode::NumpadEnter => KeyLocation::Numpad,
        KeyCode::NumpadSubtract => KeyLocation::Numpad,
        KeyCode::NumpadEqual => KeyLocation::Numpad,
        KeyCode::Numpad0 => KeyLocation::Numpad,
        KeyCode::Numpad1 => KeyLocation::Numpad,
        KeyCode::Numpad2 => KeyLocation::Numpad,
        KeyCode::Numpad3 => KeyLocation::Numpad,
        KeyCode::Numpad4 => KeyLocation::Numpad,
        KeyCode::Numpad5 => KeyLocation::Numpad,
        KeyCode::Numpad6 => KeyLocation::Numpad,
        KeyCode::Numpad7 => KeyLocation::Numpad,
        KeyCode::Numpad8 => KeyLocation::Numpad,
        KeyCode::Numpad9 => KeyLocation::Numpad,

        _ => KeyLocation::Standard,
    }
}

// While F1-F20 have scancodes we can match on, we have to check against UTF-16
// constants for the rest.
// https://developer.apple.com/documentation/appkit/1535851-function-key_unicodes?preferredLanguage=occ
pub fn extra_function_key_to_code(scancode: u16, string: &str) -> PhysicalKey {
    if let Some(ch) = string.encode_utf16().next() {
        match ch {
            0xf718 => PhysicalKey::Code(KeyCode::F21),
            0xf719 => PhysicalKey::Code(KeyCode::F22),
            0xf71a => PhysicalKey::Code(KeyCode::F23),
            0xf71b => PhysicalKey::Code(KeyCode::F24),
            _ => PhysicalKey::Unidentified(NativeKeyCode::MacOS(scancode)),
        }
    } else {
        PhysicalKey::Unidentified(NativeKeyCode::MacOS(scancode))
    }
}

// The values are from the https://github.com/apple-oss-distributions/IOHIDFamily/blob/19666c840a6d896468416ff0007040a10b7b46b8/IOHIDSystem/IOKit/hidsystem/IOLLEvent.h#L258-L259
const NX_DEVICELCTLKEYMASK: NSEventModifierFlags = NSEventModifierFlags(0x00000001);
const NX_DEVICELSHIFTKEYMASK: NSEventModifierFlags = NSEventModifierFlags(0x00000002);
const NX_DEVICERSHIFTKEYMASK: NSEventModifierFlags = NSEventModifierFlags(0x00000004);
const NX_DEVICELCMDKEYMASK: NSEventModifierFlags = NSEventModifierFlags(0x00000008);
const NX_DEVICERCMDKEYMASK: NSEventModifierFlags = NSEventModifierFlags(0x00000010);
const NX_DEVICELALTKEYMASK: NSEventModifierFlags = NSEventModifierFlags(0x00000020);
const NX_DEVICERALTKEYMASK: NSEventModifierFlags = NSEventModifierFlags(0x00000040);
const NX_DEVICERCTLKEYMASK: NSEventModifierFlags = NSEventModifierFlags(0x00002000);

pub(super) fn lalt_pressed(event: &NSEvent) -> bool {
    unsafe { event.modifierFlags() }.contains(NX_DEVICELALTKEYMASK)
}

pub(super) fn ralt_pressed(event: &NSEvent) -> bool {
    unsafe { event.modifierFlags() }.contains(NX_DEVICERALTKEYMASK)
}

pub(super) fn event_mods(event: &NSEvent) -> Modifiers {
    let flags = unsafe { event.modifierFlags() };
    let mut state = ModifiersState::empty();
    let mut pressed_mods = ModifiersKeys::empty();

    state
        .set(ModifiersState::SHIFT, flags.contains(NSEventModifierFlags::NSEventModifierFlagShift));
    pressed_mods.set(ModifiersKeys::LSHIFT, flags.contains(NX_DEVICELSHIFTKEYMASK));
    pressed_mods.set(ModifiersKeys::RSHIFT, flags.contains(NX_DEVICERSHIFTKEYMASK));

    state.set(
        ModifiersState::CONTROL,
        flags.contains(NSEventModifierFlags::NSEventModifierFlagControl),
    );
    pressed_mods.set(ModifiersKeys::LCONTROL, flags.contains(NX_DEVICELCTLKEYMASK));
    pressed_mods.set(ModifiersKeys::RCONTROL, flags.contains(NX_DEVICERCTLKEYMASK));

    state.set(ModifiersState::ALT, flags.contains(NSEventModifierFlags::NSEventModifierFlagOption));
    pressed_mods.set(ModifiersKeys::LALT, flags.contains(NX_DEVICELALTKEYMASK));
    pressed_mods.set(ModifiersKeys::RALT, flags.contains(NX_DEVICERALTKEYMASK));

    state.set(
        ModifiersState::SUPER,
        flags.contains(NSEventModifierFlags::NSEventModifierFlagCommand),
    );
    pressed_mods.set(ModifiersKeys::LSUPER, flags.contains(NX_DEVICELCMDKEYMASK));
    pressed_mods.set(ModifiersKeys::RSUPER, flags.contains(NX_DEVICERCMDKEYMASK));

    Modifiers::new(state, pressed_mods)
}
