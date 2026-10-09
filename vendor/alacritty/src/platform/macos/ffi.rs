//! Keyboard layout FFI.
//!
//! Copied from winit 0.30.9 `src/platform_impl/macos/ffi.rs` (Apache-2.0), limited to the
//! declarations used for key translation. The two CoreFoundation functions previously came
//! from the `core-foundation` crate and are declared directly here.

#![allow(non_upper_case_globals, non_snake_case, dead_code)]

use std::ffi::c_void;

pub type CFStringRef = *const c_void;
pub type CFDataRef = *const c_void;

#[repr(transparent)]
pub struct TISInputSource(std::ffi::c_void);
pub type TISInputSourceRef = *mut TISInputSource;

#[repr(transparent)]
pub struct UCKeyboardLayout(std::ffi::c_void);

pub type OptionBits = u32;
pub type UniCharCount = std::os::raw::c_ulong;
pub type UniChar = std::os::raw::c_ushort;
pub type OSStatus = i32;

pub const kUCKeyActionDisplay: u16 = 3;
pub const kUCKeyTranslateNoDeadKeysMask: OptionBits = 1;

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    pub fn CFRelease(cf: *const c_void);
    pub fn CFDataGetBytePtr(the_data: CFDataRef) -> *const u8;
}

#[link(name = "Carbon", kind = "framework")]
extern "C" {
    pub static kTISPropertyUnicodeKeyLayoutData: CFStringRef;

    pub fn TISGetInputSourceProperty(
        inputSource: TISInputSourceRef,
        propertyKey: CFStringRef,
    ) -> *mut c_void;

    pub fn TISCopyCurrentKeyboardLayoutInputSource() -> TISInputSourceRef;

    pub fn LMGetKbdType() -> u8;

    pub fn UCKeyTranslate(
        keyLayoutPtr: *const UCKeyboardLayout,
        virtualKeyCode: u16,
        keyAction: u16,
        modifierKeyState: u32,
        keyboardType: u32,
        keyTranslateOptions: OptionBits,
        deadKeyState: *mut u32,
        maxStringLength: UniCharCount,
        actualStringLength: *mut UniCharCount,
        unicodeString: *mut UniChar,
    ) -> OSStatus;
}

/// libdispatch main queue scheduling, used to wake the AppKit main thread.
pub mod dispatch {
    use std::ffi::c_void;

    #[repr(C)]
    pub struct DispatchObject {
        _private: [u8; 0],
    }

    pub type DispatchQueue = *mut DispatchObject;
    pub type DispatchTime = u64;
    pub type DispatchFunction = extern "C" fn(*mut c_void);

    pub const DISPATCH_TIME_NOW: DispatchTime = 0;

    extern "C" {
        static _dispatch_main_q: DispatchObject;

        pub fn dispatch_async_f(
            queue: DispatchQueue,
            context: *mut c_void,
            work: DispatchFunction,
        );

        pub fn dispatch_after_f(
            when: DispatchTime,
            queue: DispatchQueue,
            context: *mut c_void,
            work: DispatchFunction,
        );

        pub fn dispatch_time(when: DispatchTime, delta: i64) -> DispatchTime;
    }

    /// Equivalent of the `dispatch_get_main_queue()` C macro.
    pub fn main_queue() -> DispatchQueue {
        std::ptr::addr_of!(_dispatch_main_q).cast_mut()
    }
}
