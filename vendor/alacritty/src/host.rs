//! Host-neutral replacements for the winit event loop, window events and key events.
//!
//! Upstream Alacritty receives `winit::event::WindowEvent`s from a winit `EventLoop` that it
//! owns. In this embedding Tauri/tao owns the event loop (NSApplication on macOS, the GTK/GLib
//! main loop on Linux), so no winit `EventLoop` or `Window` exists. The types below keep the exact field and method shapes the
//! upstream input/event code consumes, so that code stays unmodified apart from imports.
//!
//! `KeyEvent` and `Modifiers` are needed because winit keeps the fields of its versions
//! private (`platform_specific`, `pressed_mods`), which makes them impossible to construct
//! outside of winit. Their layout and semantics follow winit 0.30.9
//! (`src/event.rs`, `src/keyboard.rs`, `src/platform/modifier_supplement.rs`, Apache-2.0).
//! embed: Unused shift/control/super accessors and window-event IDs are omitted. Alt-key
//! accessors are macOS-only, where Option-as-Alt consumes the sided modifier state.

use std::collections::VecDeque;
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use bitflags::bitflags;
use parking_lot::Mutex;
use winit::dpi::{PhysicalPosition, PhysicalSize};
#[cfg(target_os = "macos")]
use winit::keyboard::ModifiersKeyState;
use winit::keyboard::{Key, KeyLocation, ModifiersState, PhysicalKey, SmolStr};

pub use winit::event::{ElementState, Ime, MouseButton, MouseScrollDelta, TouchPhase};
pub use winit::window::WindowId;

/// Keyboard event, shaped after `winit::event::KeyEvent` 0.30.9.
///
/// The `text_with_all_modifiers` and `key_without_modifiers` fields correspond to winit's
/// macOS and Linux `KeyEventExtra`, exposed there through `KeyEventExtModifierSupplement`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct KeyEvent {
    pub physical_key: PhysicalKey,
    pub logical_key: Key,
    pub text: Option<SmolStr>,
    pub location: KeyLocation,
    pub state: ElementState,
    pub repeat: bool,
    pub(crate) text_with_all_modifiers: Option<SmolStr>,
    pub(crate) key_without_modifiers: Key,
}

impl KeyEvent {
    /// Identical to `KeyEvent::text` but this is affected by <kbd>Ctrl</kbd>.
    #[inline]
    pub fn text_with_all_modifiers(&self) -> Option<&str> {
        self.text_with_all_modifiers.as_ref().map(|s| s.as_str())
    }

    /// This value ignores all modifiers including, but not limited to <kbd>Shift</kbd>,
    /// <kbd>Caps Lock</kbd>, and <kbd>Ctrl</kbd>.
    #[inline]
    pub fn key_without_modifiers(&self) -> Key {
        self.key_without_modifiers.clone()
    }
}

bitflags! {
    /// Currently pressed modifier keys, as in winit's crate-private `ModifiersKeys`.
    #[derive(Default, Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub struct ModifiersKeys: u8 {
        const LSHIFT   = 0b0000_0001;
        const RSHIFT   = 0b0000_0010;
        const LCONTROL = 0b0000_0100;
        const RCONTROL = 0b0000_1000;
        const LALT     = 0b0001_0000;
        const RALT     = 0b0010_0000;
        const LSUPER   = 0b0100_0000;
        const RSUPER   = 0b1000_0000;
    }
}

/// Keyboard modifiers, shaped after `winit::event::Modifiers` 0.30.9.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Modifiers {
    pub(crate) state: ModifiersState,
    pub(crate) pressed_mods: ModifiersKeys,
}

impl Modifiers {
    pub(crate) fn new(state: ModifiersState, pressed_mods: ModifiersKeys) -> Self {
        Self { state, pressed_mods }
    }

    /// The state of the modifiers.
    pub fn state(&self) -> ModifiersState {
        self.state
    }

    /// The state of the left alt key.
    #[cfg(target_os = "macos")]
    pub fn lalt_state(&self) -> ModifiersKeyState {
        self.mod_state(ModifiersKeys::LALT)
    }

    /// The state of the right alt key.
    #[cfg(target_os = "macos")]
    pub fn ralt_state(&self) -> ModifiersKeyState {
        self.mod_state(ModifiersKeys::RALT)
    }

    #[cfg(target_os = "macos")]
    fn mod_state(&self, modifier: ModifiersKeys) -> ModifiersKeyState {
        if self.pressed_mods.contains(modifier) {
            ModifiersKeyState::Pressed
        } else {
            ModifiersKeyState::Unknown
        }
    }
}

impl From<ModifiersState> for Modifiers {
    fn from(value: ModifiersState) -> Self {
        Self { state: value, pressed_mods: Default::default() }
    }
}

/// The subset of `winit::event::WindowEvent` which the terminal view produces.
#[derive(Debug, Clone, PartialEq)]
pub enum WindowEvent {
    Resized(PhysicalSize<u32>),
    ScaleFactorChanged { scale_factor: f64 },
    Focused(bool),
    Occluded(bool),
    KeyboardInput { event: KeyEvent, is_synthetic: bool },
    ModifiersChanged(Modifiers),
    Ime(Ime),
    CursorMoved { position: PhysicalPosition<f64> },
    CursorEntered,
    CursorLeft,
    MouseWheel { delta: MouseScrollDelta, phase: TouchPhase },
    MouseInput { state: ElementState, button: MouseButton },
    RedrawRequested,
}

/// The subset of `winit::event::Event` which the upstream window context consumes.
#[derive(Debug, Clone)]
pub enum WinitEvent<T> {
    UserEvent(T),
    WindowEvent { event: WindowEvent },
    AboutToWait,
}

/// Error returned when sending to a proxy whose host has shut down.
pub struct EventLoopClosed<T>(pub T);

impl<T> fmt::Debug for EventLoopClosed<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("EventLoopClosed(..)")
    }
}

/// embed: OSC notifications are bounded on the producer, even if the host never pumps.
pub(crate) const NOTIFICATION_COUNT_LIMIT: usize = 128;
pub(crate) const NOTIFICATION_BYTE_LIMIT: usize = 256 * 1024;

struct ProxyQueue<T> {
    events: VecDeque<(u64, T)>,
    notifications: VecDeque<(u64, T, usize)>,
    notification_bytes: usize,
    sequence: u64,
}

impl<T> Default for ProxyQueue<T> {
    fn default() -> Self {
        Self {
            events: VecDeque::new(),
            notifications: VecDeque::new(),
            notification_bytes: 0,
            sequence: 0,
        }
    }
}

impl<T> ProxyQueue<T> {
    fn push(&mut self, event: T, notification_bytes: Option<usize>) {
        let sequence = self.sequence;
        self.sequence = self.sequence.wrapping_add(1);
        if let Some(bytes) = notification_bytes {
            if bytes > NOTIFICATION_BYTE_LIMIT { return; }
            while self.notifications.len() >= NOTIFICATION_COUNT_LIMIT
                || self.notification_bytes + bytes > NOTIFICATION_BYTE_LIMIT
            {
                let (_, _, dropped_bytes) = self.notifications.pop_front().unwrap();
                self.notification_bytes -= dropped_bytes;
            }
            self.notification_bytes += bytes;
            self.notifications.push_back((sequence, event, bytes));
        } else {
            self.events.push_back((sequence, event));
        }
    }

    fn drain(&mut self) -> VecDeque<T> {
        let mut drained = VecDeque::with_capacity(self.events.len() + self.notifications.len());
        while !self.events.is_empty() || !self.notifications.is_empty() {
            let take_normal = match (self.events.front(), self.notifications.front()) {
                (Some((normal, _)), Some((notification, _, _))) => {
                    // Wrapping sequence order is safe across u64 rollover.
                    (normal.wrapping_sub(*notification) as i64) < 0
                },
                (Some(_), None) => true,
                _ => false,
            };
            if take_normal {
                drained.push_back(self.events.pop_front().unwrap().1);
            } else {
                drained.push_back(self.notifications.pop_front().unwrap().1);
            }
        }
        self.notification_bytes = 0;
        drained
    }
}

struct ProxyShared<T> {
    queue: Mutex<ProxyQueue<T>>,
    wake_pending: AtomicBool,
    closed: AtomicBool,
    /// Wakes the main thread for the owner identified by `context`.
    wake: fn(u64),
    context: u64,
}

/// Thread-safe user event sender, replacing `winit::event_loop::EventLoopProxy`.
///
/// Events are queued and the main thread is woken at most once per batch: `wake` is only
/// invoked when no wakeup is pending, so bursts of PTY `Wakeup` events are coalesced into
/// a single main-thread pump. Every proxy carries the `context` of its owning session, so a
/// wakeup is routed to that session only.
pub struct EventLoopProxy<T> {
    shared: Arc<ProxyShared<T>>,
}

impl<T> Clone for EventLoopProxy<T> {
    fn clone(&self) -> Self {
        Self { shared: self.shared.clone() }
    }
}

impl<T> fmt::Debug for EventLoopProxy<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EventLoopProxy").finish_non_exhaustive()
    }
}

impl<T> EventLoopProxy<T> {
    pub(crate) fn new(wake: fn(u64), context: u64) -> Self {
        Self {
            shared: Arc::new(ProxyShared {
                queue: Mutex::new(ProxyQueue::default()),
                wake_pending: AtomicBool::new(false),
                closed: AtomicBool::new(false),
                wake,
                context,
            }),
        }
    }

    /// Queue an event for the main thread.
    pub fn send_event(&self, event: T) -> Result<(), EventLoopClosed<T>> {
        self.enqueue(event, None)
    }

    /// embed: Notifications never enter the unbounded generic event queue. The bounded
    /// lane drops its oldest entries while keeping surviving events in global FIFO order.
    pub(crate) fn send_notification(
        &self,
        event: T,
        bytes: usize,
    ) -> Result<(), EventLoopClosed<T>> {
        self.enqueue(event, Some(bytes))
    }

    fn enqueue(&self, event: T, notification_bytes: Option<usize>) -> Result<(), EventLoopClosed<T>> {
        if self.shared.closed.load(Ordering::Acquire) {
            return Err(EventLoopClosed(event));
        }

        let mut queue = self.shared.queue.lock();
        // Close and enqueue share the lock, so a racing producer cannot retain events
        // after close cleared the queue.
        if self.shared.closed.load(Ordering::Acquire) {
            return Err(EventLoopClosed(event));
        }
        queue.push(event, notification_bytes);
        drop(queue);

        if !self.shared.wake_pending.swap(true, Ordering::AcqRel) {
            (self.shared.wake)(self.shared.context);
        }

        Ok(())
    }

    /// Take all queued events. Must only be called by the main-thread pump.
    pub(crate) fn drain(&self) -> VecDeque<T> {
        // Clear the flag before taking the queue, so a concurrent send after the take
        // schedules another wakeup instead of being stranded.
        self.shared.wake_pending.store(false, Ordering::Release);
        self.shared.queue.lock().drain()
    }

    /// Reject all future events and drop queued ones.
    pub(crate) fn close(&self) {
        self.shared.closed.store(true, Ordering::Release);
        *self.shared.queue.lock() = ProxyQueue::default();
    }
}

/// Replacement for the macOS parts of `winit::event_loop::ActiveEventLoop` used by
/// Alacritty's `Hide` and `HideOtherApplications` actions (macOS-only actions upstream).
#[cfg(target_os = "macos")]
#[derive(Debug, Default)]
pub struct ActiveEventLoop {
    _private: (),
}

#[cfg(target_os = "macos")]
impl ActiveEventLoop {
    pub(crate) fn new() -> Self {
        Self { _private: () }
    }

    /// Hide the application, like winit's `ActiveEventLoopExtMacOS::hide_application`.
    pub fn hide_application(&self) {
        crate::platform::macos::hide_application();
    }

    /// Hide other applications, like winit's
    /// `ActiveEventLoopExtMacOS::hide_other_applications`.
    pub fn hide_other_applications(&self) {
        crate::platform::macos::hide_other_applications();
    }
}
