//! The Linux terminal view: an input-only GTK widget plus a Wayland surface (new code).
//!
//! Event translation follows what winit 0.30.13's Wayland backend reports to upstream
//! Alacritty (`src/platform_impl/linux/wayland/seat/{keyboard,pointer}`, Apache-2.0), with GDK
//! events as the source and the same host-routing rules as the macOS view
//! (`crate::platform::macos::view`):
//! - Every event is queued to the view's own session; events of one GTK callback form one
//!   [`EventBatch`].
//! - Keys go through a `GtkIMMulticontext` first. A key the input method only turns into its
//!   own plain text (no preedit) is reported as a key, so bindings and the kitty keyboard
//!   protocol see it; compositions are reported as `Ime` preedit/commit events. Input methods
//!   that answer asynchronously (IBus) re-send unhandled keys, which are reported then.
//! - Pointer motion outside the view is ignored unless a button is held (GTK's implicit grab
//!   keeps delivering it to the view, like AppKit drags). A button press focuses the view.
//! - Wheel scrolling (GDK "Mouse" scroll source) is reported in lines (GTK's smooth-scroll
//!   units), touchpad and other continuous scrolling in physical pixels (GTK's Wayland
//!   backend divides the protocol's surface pixels by 10). Like winit, the first pixel event of
//!   a gesture is `TouchPhase::Started`, a scroll stop is `Ended` and everything else `Moved`;
//!   wheel events are always `Moved` and, unlike winit, end a pixel gesture, so the next
//!   touchpad gesture starts afresh (Alacritty resets its pixel accumulator on `Started`).
//! - While hidden, geometry changes are not reported; the current geometry is reported when
//!   the view is shown again, so a hidden terminal never resizes its PTY. A hidden view's
//!   input widget is hidden, so it cannot hold keyboard focus.
//! - The view is occluded while the GTK toplevel is unmapped (hidden to the tray), which also
//!   destroys the toplevel's `wl_surface`; the subsurface is recreated on the new surface when
//!   the toplevel is mapped again.

use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::ptr::NonNull;
use std::rc::Rc;

use gtk::glib::{Propagation, SignalHandlerId};
use gtk::prelude::*;
use gtk::{gdk, IMMulticontext};
use wayland_client::backend::ObjectId;
use wayland_client::protocol::wl_subsurface::WlSubsurface;
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::Proxy;
use winit::dpi::{LogicalPosition, PhysicalPosition, PhysicalSize};
use winit::keyboard::ModifiersState;
use winit::raw_window_handle::{
    RawDisplayHandle, RawWindowHandle, WaylandDisplayHandle, WaylandWindowHandle,
};
use winit::window::CursorIcon;

use super::embedder::{Frame, WebView};
use super::keyboard;
use crate::embed::{self, EventBatch, SessionKey};
use crate::host::{
    ElementState, Ime, Modifiers, ModifiersKeys, MouseButton, MouseScrollDelta, TouchPhase,
    WindowEvent,
};

/// The view's subsurface role on the toplevel surface it was created for.
struct Subsurface {
    subsurface: WlSubsurface,
    parent: *mut c_void,
}

/// Input method results collected while a key event is filtered.
#[derive(Default)]
struct KeyFilter {
    commits: Vec<String>,
    preedit_changed: bool,
}

pub struct TerminalView {
    /// Embedder session this view belongs to; fixed for the view's lifetime.
    session: SessionKey,
    web: WebView,
    widget: gtk::EventBox,
    im: IMMulticontext,
    surface: WlSurface,
    subsurface: RefCell<Option<Subsurface>>,
    frame: Cell<Frame>,
    /// Left corner radii in GTK logical pixels (CSS zoom already applied).
    corner_radii: Cell<[f64; 2]>,
    hidden: Cell<bool>,
    detached: Cell<bool>,

    /// Last scale factor reported to the host (also the surface's buffer scale).
    reported_scale_factor: Cell<f64>,
    /// Last nonzero view size reported to the host.
    reported_size: Cell<PhysicalSize<u32>>,
    /// Last focus state reported to the host.
    reported_focus: Cell<bool>,
    /// Last toplevel occlusion state reported to the host.
    reported_occluded: Cell<bool>,

    modifiers: Cell<Modifiers>,
    /// Hardware keycodes whose press was reported and not yet released.
    pressed_keys: RefCell<Vec<u16>>,

    /// True iff the application wants IME events (`set_ime_allowed`).
    ime_allowed: Cell<bool>,
    ime_enabled: Cell<bool>,
    ime_preedit: Cell<bool>,
    key_filter: RefCell<Option<KeyFilter>>,

    cursor_icon: Cell<CursorIcon>,
    cursor_visible: Cell<bool>,
    /// A pixel (touchpad) scroll gesture is in progress: its `Started` was reported.
    scroll_active: Cell<bool>,

    /// Map/unmap handlers on the toplevel, disconnected on detach.
    toplevel_handlers: RefCell<Vec<(gtk::Window, SignalHandlerId)>>,
}

impl TerminalView {
    /// Create a hidden view at `frame` above the web view.
    pub(crate) fn new(web: WebView, frame: Frame, session: SessionKey) -> Result<Rc<Self>, String> {
        let wayland = web.wayland();
        let scale = web.web_view().scale_factor().max(1);

        let surface = wayland.compositor.create_surface(&wayland.qh, ());
        // Pointer input goes through the surface to GTK's toplevel and the input widget.
        let region = wayland.compositor.create_region(&wayland.qh, ());
        surface.set_input_region(Some(&region));
        region.destroy();
        // EGL must composite the transparent corner pixels with the web chrome.
        // Keep the opaque region empty even when the terminal background is opaque.
        surface.set_opaque_region(None);
        surface.set_buffer_scale(scale);
        surface.commit();
        wayland.flush();

        let widget = gtk::EventBox::new();
        widget.set_visible_window(false);
        widget.set_above_child(false);
        widget.set_can_focus(true);
        widget.set_no_show_all(true);
        widget.set_halign(gtk::Align::Start);
        widget.set_valign(gtk::Align::Start);
        widget.add_events(
            gdk::EventMask::BUTTON_PRESS_MASK
                | gdk::EventMask::BUTTON_RELEASE_MASK
                | gdk::EventMask::POINTER_MOTION_MASK
                | gdk::EventMask::SCROLL_MASK
                | gdk::EventMask::SMOOTH_SCROLL_MASK
                | gdk::EventMask::ENTER_NOTIFY_MASK
                | gdk::EventMask::LEAVE_NOTIFY_MASK
                | gdk::EventMask::KEY_PRESS_MASK
                | gdk::EventMask::KEY_RELEASE_MASK
                | gdk::EventMask::FOCUS_CHANGE_MASK,
        );
        web.overlay().add_overlay(&widget);

        let view = Rc::new(Self {
            session,
            im: IMMulticontext::new(),
            surface,
            subsurface: RefCell::new(None),
            frame: Cell::new(frame),
            corner_radii: Cell::new([0.; 2]),
            hidden: Cell::new(true),
            detached: Cell::new(false),
            reported_scale_factor: Cell::new(0.),
            reported_size: Cell::new(PhysicalSize::new(0, 0)),
            reported_focus: Cell::new(false),
            reported_occluded: Cell::new(false),
            modifiers: Cell::new(Modifiers::default()),
            pressed_keys: RefCell::new(Vec::new()),
            ime_allowed: Cell::new(false),
            ime_enabled: Cell::new(false),
            ime_preedit: Cell::new(false),
            key_filter: RefCell::new(None),
            cursor_icon: Cell::new(CursorIcon::Default),
            cursor_visible: Cell::new(true),
            scroll_active: Cell::new(false),
            toplevel_handlers: RefCell::new(Vec::new()),
            widget,
            web,
        });
        view.update_widget_geometry();
        view.connect_signals();

        Ok(view)
    }

    fn connect_signals(self: &Rc<Self>) {
        let widget = &self.widget;

        let view = Rc::downgrade(self);
        widget.connect_key_press_event(move |_, event| {
            view.upgrade().map_or(Propagation::Proceed, |view| view.key(event, true))
        });
        let view = Rc::downgrade(self);
        widget.connect_key_release_event(move |_, event| {
            view.upgrade().map_or(Propagation::Proceed, |view| view.key(event, false))
        });
        let view = Rc::downgrade(self);
        widget.connect_button_press_event(move |_, event| {
            view.upgrade().map_or(Propagation::Proceed, |view| view.button(event, true))
        });
        let view = Rc::downgrade(self);
        widget.connect_button_release_event(move |_, event| {
            view.upgrade().map_or(Propagation::Proceed, |view| view.button(event, false))
        });
        let view = Rc::downgrade(self);
        widget.connect_motion_notify_event(move |_, event| {
            if let Some(view) = view.upgrade() {
                let _batch = EventBatch::new(view.session);
                view.mouse_motion(event.position(), event.state());
            }
            Propagation::Stop
        });
        let view = Rc::downgrade(self);
        widget.connect_enter_notify_event(move |_, _| {
            if let Some(view) = view.upgrade() {
                view.queue_event(WindowEvent::CursorEntered);
            }
            Propagation::Stop
        });
        let view = Rc::downgrade(self);
        widget.connect_leave_notify_event(move |_, _| {
            if let Some(view) = view.upgrade() {
                view.queue_event(WindowEvent::CursorLeft);
            }
            Propagation::Stop
        });
        let view = Rc::downgrade(self);
        widget.connect_scroll_event(move |_, event| {
            view.upgrade().map_or(Propagation::Proceed, |view| view.scroll(event))
        });
        let view = Rc::downgrade(self);
        widget.connect_focus_in_event(move |_, _| {
            if let Some(view) = view.upgrade() {
                view.focus_changed(true);
            }
            Propagation::Proceed
        });
        let view = Rc::downgrade(self);
        widget.connect_focus_out_event(move |_, _| {
            if let Some(view) = view.upgrade() {
                view.focus_changed(false);
            }
            Propagation::Proceed
        });
        let view = Rc::downgrade(self);
        widget.connect_scale_factor_notify(move |_| {
            if let Some(view) = view.upgrade() {
                view.report_geometry();
            }
        });
        let view = Rc::downgrade(self);
        widget.connect_realize(move |widget| {
            if let Some(view) = view.upgrade() {
                view.im.set_client_window(widget.window().as_ref());
                view.apply_cursor();
            }
        });
        let view = Rc::downgrade(self);
        widget.connect_unrealize(move |_| {
            if let Some(view) = view.upgrade() {
                view.im.set_client_window(None);
            }
        });

        let view = Rc::downgrade(self);
        self.im.connect_commit(move |_, text| {
            if let Some(view) = view.upgrade() {
                view.ime_commit(text);
            }
        });
        let view = Rc::downgrade(self);
        self.im.connect_preedit_changed(move |_| {
            if let Some(view) = view.upgrade() {
                view.ime_preedit_changed();
            }
        });

        if let Some(toplevel) = self.web.toplevel() {
            let view = Rc::downgrade(self);
            let map = toplevel.connect_map(move |_| {
                if let Some(view) = view.upgrade() {
                    view.toplevel_mapped(true);
                }
            });
            let view = Rc::downgrade(self);
            let unmap = toplevel.connect_unmap(move |_| {
                if let Some(view) = view.upgrade() {
                    view.toplevel_mapped(false);
                }
            });
            self.toplevel_handlers
                .borrow_mut()
                .extend([(toplevel.clone(), map), (toplevel, unmap)]);
        }
    }

    /// Embedder session this view belongs to.
    pub(crate) fn session(&self) -> SessionKey {
        self.session
    }

    fn queue_event(&self, event: WindowEvent) {
        embed::queue_window_event(self.session, event);
    }

    /// The GTK toplevel window showing the view.
    pub(crate) fn toplevel(&self) -> Option<gtk::Window> {
        self.web.toplevel()
    }

    pub(crate) fn scale_factor(&self) -> f64 {
        f64::from(self.widget.scale_factor().max(1))
    }

    pub(super) fn set_corner_radii(&self, top: f64, bottom: f64) {
        self.corner_radii.set([top, bottom]);
    }

    pub(crate) fn physical_corner_radii(&self) -> [f32; 2] {
        self.corner_radii.get().map(|radius| (radius * self.scale_factor()) as f32)
    }

    /// Size of the view in physical pixels.
    pub(crate) fn physical_size(&self) -> PhysicalSize<u32> {
        let frame = self.frame.get();
        let scale = self.widget.scale_factor().max(1);
        let physical = |logical: i32| u32::try_from(logical.max(0) * scale).unwrap_or(0);
        PhysicalSize::new(physical(frame.width), physical(frame.height))
    }

    /// The view's own surface, target of its EGL window surface.
    pub(crate) fn raw_window_handle(&self) -> RawWindowHandle {
        let surface = NonNull::new(self.surface.id().as_ptr().cast::<c_void>())
            .expect("terminal surface is alive");
        RawWindowHandle::Wayland(WaylandWindowHandle::new(surface))
    }

    /// GTK's Wayland display.
    pub(crate) fn raw_display_handle(&self) -> RawDisplayHandle {
        let display =
            NonNull::new(self.web.wayland().display).expect("Wayland display is connected");
        RawDisplayHandle::Wayland(WaylandDisplayHandle::new(display))
    }

    /// Refresh rate of the GDK monitor showing the toplevel, in millihertz.
    pub(crate) fn refresh_rate_millihertz(&self) -> Option<u32> {
        let window = self.toplevel()?.window()?;
        let monitor = window.display().monitor_at_window(&window)?;
        u32::try_from(monitor.refresh_rate()).ok().filter(|rate| *rate > 0)
    }

    /// Record the geometry the display was just created with as already reported.
    pub(crate) fn record_reported_geometry(&self) {
        self.reported_scale_factor.set(self.scale_factor());
        let size = self.physical_size();
        if size.width != 0 && size.height != 0 {
            self.reported_size.set(size);
        }
    }

    pub(super) fn place(&self, frame: Frame) {
        if self.frame.replace(frame) == frame {
            return;
        }
        self.update_widget_geometry();
        if !self.hidden.get() {
            self.attach_subsurface();
            self.report_geometry();
        }
    }

    pub(super) fn set_hidden(&self, hidden: bool) {
        if self.hidden.replace(hidden) == hidden {
            return;
        }

        if hidden {
            self.detach_subsurface();
            self.widget.hide();
        } else {
            self.update_widget_geometry();
            self.widget.show();
            self.attach_subsurface();
            // Geometry that changed while hidden.
            self.report_geometry();
        }
    }

    pub(super) fn is_shown(&self) -> bool {
        let frame = self.frame.get();
        !self.hidden.get() && !self.detached.get() && frame.width >= 1 && frame.height >= 1
    }

    /// Move keyboard focus to `next` (the web view) if the view holds it; unset the window's
    /// focus if `next` refuses, so the terminal never keeps keyboard focus.
    pub(super) fn release_focus(&self, next: &gtk::Widget) {
        if !self.widget.is_focus() {
            return;
        }
        let _batch = EventBatch::new(self.session);
        next.grab_focus();
        if self.widget.is_focus() {
            if let Some(toplevel) = self.toplevel() {
                toplevel.set_focus(None::<&gtk::Widget>);
            }
        }
    }

    /// Make the input widget the window's focus widget.
    pub(super) fn take_focus(&self) -> bool {
        if !self.is_shown() || !self.widget.is_visible() {
            return false;
        }
        self.widget.grab_focus();
        self.widget.is_focus()
    }

    /// Remove the input widget and destroy the Wayland surface. The GL surface must be gone.
    pub(super) fn detach(&self) {
        if self.detached.replace(true) {
            return;
        }

        self.detach_subsurface();
        for (toplevel, handler) in self.toplevel_handlers.take() {
            toplevel.disconnect(handler);
        }
        self.im.set_client_window(None);
        self.web.overlay().remove(&self.widget);
        self.surface.destroy();
        self.web.wayland().flush();
    }

    /// Position the input widget over the frame (overlay coordinates equal the web view's).
    fn update_widget_geometry(&self) {
        let (origin_x, origin_y) = self.web.origin().unwrap_or((0, 0));
        let frame = self.frame.get();
        self.widget.set_margin_start((frame.x - origin_x).max(0));
        self.widget.set_margin_top((frame.y - origin_y).max(0));
        self.widget.set_size_request(frame.width.max(1), frame.height.max(1));
    }

    /// Map the surface as a subsurface of the toplevel's current surface at the frame.
    fn attach_subsurface(&self) {
        if self.detached.get() {
            return;
        }
        let toplevel = match self.toplevel() {
            Some(toplevel) => toplevel,
            None => return,
        };
        let parent = toplevel.window().map_or(std::ptr::null_mut(), |w| super::wl_surface_of(&w));
        if parent.is_null() {
            // Unmapped toplevel: attached again when it is mapped.
            self.detach_subsurface();
            return;
        }

        let wayland = self.web.wayland();
        let frame = self.frame.get();
        let mut subsurface = self.subsurface.borrow_mut();
        if subsurface.as_ref().map(|current| current.parent) != Some(parent) {
            if let Some(stale) = subsurface.take() {
                stale.subsurface.destroy();
            }

            let parent_surface = unsafe { ObjectId::from_ptr(WlSurface::interface(), parent.cast()) }
                .and_then(|id| WlSurface::from_id(&wayland.conn, id));
            let parent_surface = match parent_surface {
                Ok(parent_surface) => parent_surface,
                Err(err) => {
                    log::error!("Unable to use the window's Wayland surface: {err}");
                    return;
                },
            };

            let role = wayland.subcompositor.get_subsurface(
                &self.surface,
                &parent_surface,
                &wayland.qh,
                (),
            );
            // Frames are committed by the renderer without waiting for GTK's surface.
            role.set_desync();
            *subsurface = Some(Subsurface { subsurface: role, parent });
        }
        if let Some(current) = subsurface.as_ref() {
            current.subsurface.set_position(frame.x, frame.y);
        }
        drop(subsurface);
        wayland.flush();

        // The position applies with the parent's next commit; let GTK repaint (and commit)
        // the area instead of committing GTK's surface from here.
        toplevel.queue_draw_area(frame.x, frame.y, frame.width.max(1), frame.height.max(1));
    }

    /// Unmap the surface (destroying the subsurface role takes effect immediately).
    fn detach_subsurface(&self) {
        let subsurface = self.subsurface.borrow_mut().take();
        if let Some(subsurface) = subsurface {
            subsurface.subsurface.destroy();
            self.web.wayland().flush();
        }
    }

    /// Report scale factor and size changes to the host, unless the view is hidden.
    fn report_geometry(&self) {
        if self.hidden.get() || self.detached.get() {
            return;
        }
        let _batch = EventBatch::new(self.session);

        let scale_factor = self.scale_factor();
        let scale_changed = scale_factor != self.reported_scale_factor.replace(scale_factor);
        if scale_changed {
            // Double-buffered: applies with the first frame rendered at the new size.
            self.surface.set_buffer_scale(scale_factor as i32);
            self.queue_event(WindowEvent::ScaleFactorChanged { scale_factor });
        }

        let size = self.physical_size();
        if size.width != 0 && size.height != 0 {
            let size_changed = size != self.reported_size.replace(size);
            if scale_changed || size_changed {
                self.queue_event(WindowEvent::Resized(size));
            }
        }
    }

    fn toplevel_mapped(&self, mapped: bool) {
        let _batch = EventBatch::new(self.session);

        if mapped {
            if !self.hidden.get() {
                self.attach_subsurface();
            }
        } else {
            // GTK destroyed the toplevel's surface; the role is inert.
            self.detach_subsurface();
        }

        let occluded = !mapped;
        if occluded != self.reported_occluded.replace(occluded) {
            self.queue_event(WindowEvent::Occluded(occluded));
            if !occluded {
                embed::request_redraw(self.session);
            }
        }
    }

    fn focus_changed(&self, focused: bool) {
        let _batch = EventBatch::new(self.session);

        if focused {
            if self.ime_allowed.get() {
                self.im.focus_in();
            }
        } else {
            self.im.focus_out();
            self.pressed_keys.borrow_mut().clear();
            self.reset_modifiers();
        }

        if focused != self.reported_focus.replace(focused) {
            self.queue_event(WindowEvent::Focused(focused));
        }
    }

    fn key(&self, event: &gdk::EventKey, pressed: bool) -> Propagation {
        if self.hidden.get() {
            return Propagation::Stop;
        }
        let _batch = EventBatch::new(self.session);

        if self.ime_allowed.get() {
            let preedit_before = self.ime_preedit.get();
            *self.key_filter.borrow_mut() = Some(KeyFilter::default());
            // May call back into `ime_commit`/`ime_preedit_changed`.
            let handled = self.im.filter_keypress(event);
            let filter = self.key_filter.borrow_mut().take().unwrap_or_default();

            let plain = handled
                && pressed
                && !preedit_before
                && !filter.preedit_changed
                && filter.commits.len() == 1
                && keyboard::keysym_to_utf8(*event.keyval()).as_deref()
                    == Some(filter.commits[0].as_str());
            if !plain {
                for text in filter.commits {
                    self.commit_ime(text);
                }
                if handled {
                    return Propagation::Stop;
                }
            }
        }

        self.report_key(event, pressed);
        Propagation::Stop
    }

    fn report_key(&self, event: &gdk::EventKey, pressed: bool) {
        let keycode = event.hardware_keycode();
        let modifier = keyboard::modifier_key(*event.keyval());

        let repeat = {
            let mut keys = self.pressed_keys.borrow_mut();
            if pressed {
                let repeat = keys.contains(&keycode);
                if !repeat {
                    keys.push(keycode);
                }
                repeat
            } else {
                match keys.iter().position(|key| *key == keycode) {
                    Some(index) => {
                        keys.swap_remove(index);
                    },
                    // The press went to the input method or another widget.
                    None if modifier.is_none() => return,
                    None => (),
                }
                false
            }
        };

        // Like Wayland, a modifier key's own press/release changes the modifiers after it.
        if modifier.is_none() {
            self.update_modifiers(event.state(), None);
        }

        let state = if pressed { ElementState::Pressed } else { ElementState::Released };
        self.queue_event(WindowEvent::KeyboardInput {
            event: keyboard::key_event(event, state, repeat),
            is_synthetic: false,
        });

        if let Some(modifier) = modifier {
            self.update_modifiers(event.state(), Some((modifier, pressed)));
        }
    }

    /// Update modifiers from a GDK event state (taken before the event) and an optional
    /// modifier key change, reporting `ModifiersChanged` if they differ.
    fn update_modifiers(
        &self,
        gdk_state: gdk::ModifierType,
        change: Option<((ModifiersState, ModifiersKeys), bool)>,
    ) {
        let current = self.modifiers.get();
        let mut state = ModifiersState::empty();
        state.set(ModifiersState::SHIFT, gdk_state.contains(gdk::ModifierType::SHIFT_MASK));
        state.set(ModifiersState::CONTROL, gdk_state.contains(gdk::ModifierType::CONTROL_MASK));
        state.set(ModifiersState::ALT, gdk_state.contains(gdk::ModifierType::MOD1_MASK));
        state.set(
            ModifiersState::SUPER,
            gdk_state.intersects(gdk::ModifierType::SUPER_MASK | gdk::ModifierType::MOD4_MASK),
        );

        let mut pressed_mods = current.pressed_mods;
        if let Some(((modifier, key), pressed)) = change {
            pressed_mods.set(key, pressed);
            let sides = match modifier {
                ModifiersState::SHIFT => ModifiersKeys::LSHIFT | ModifiersKeys::RSHIFT,
                ModifiersState::CONTROL => ModifiersKeys::LCONTROL | ModifiersKeys::RCONTROL,
                ModifiersState::ALT => ModifiersKeys::LALT | ModifiersKeys::RALT,
                _ => ModifiersKeys::LSUPER | ModifiersKeys::RSUPER,
            };
            state.set(modifier, pressed || pressed_mods.intersects(sides));
        }

        let modifiers = Modifiers::new(state, pressed_mods);
        if modifiers != current {
            self.modifiers.set(modifiers);
            self.queue_event(WindowEvent::ModifiersChanged(modifiers));
        }
    }

    /// Reset modifiers when focus leaves, like the macOS view.
    fn reset_modifiers(&self) {
        if self.modifiers.get() != Modifiers::default() {
            self.modifiers.set(Modifiers::default());
            self.queue_event(WindowEvent::ModifiersChanged(Modifiers::default()));
        }
    }

    fn button(&self, event: &gdk::EventButton, pressed: bool) -> Propagation {
        // Alacritty counts clicks itself; GDK's extra double/triple-press events are dropped.
        if pressed && event.event_type() != gdk::EventType::ButtonPress {
            return Propagation::Stop;
        }
        let _batch = EventBatch::new(self.session);

        if pressed && !self.widget.is_focus() && !self.hidden.get() {
            self.widget.grab_focus();
        }
        self.mouse_motion(event.position(), event.state() | button_mask(event.button()));

        let button = match event.button() {
            1 => MouseButton::Left,
            2 => MouseButton::Middle,
            3 => MouseButton::Right,
            8 => MouseButton::Back,
            9 => MouseButton::Forward,
            other => MouseButton::Other(u16::try_from(other).unwrap_or(u16::MAX)),
        };
        let state = if pressed { ElementState::Pressed } else { ElementState::Released };
        self.queue_event(WindowEvent::MouseInput { state, button });
        Propagation::Stop
    }

    fn mouse_motion(&self, (x, y): (f64, f64), state: gdk::ModifierType) {
        let frame = self.frame.get();
        let outside = x < 0. || y < 0. || x > f64::from(frame.width) || y > f64::from(frame.height);
        let buttons = gdk::ModifierType::BUTTON1_MASK
            | gdk::ModifierType::BUTTON2_MASK
            | gdk::ModifierType::BUTTON3_MASK
            | gdk::ModifierType::BUTTON4_MASK
            | gdk::ModifierType::BUTTON5_MASK;
        if outside && !state.intersects(buttons) {
            // Point is outside of the client area (view) and no buttons are pressed.
            return;
        }

        self.update_modifiers(state, None);
        let position = LogicalPosition::new(x, y).to_physical(self.scale_factor());
        self.queue_event(WindowEvent::CursorMoved { position });
    }

    fn scroll(&self, event: &gdk::EventScroll) -> Propagation {
        let _batch = EventBatch::new(self.session);
        self.mouse_motion(event.position(), event.state());

        let (delta, pixel) = match event.direction() {
            gdk::ScrollDirection::Smooth => {
                let (dx, dy) = event.delta();
                let wheel = event
                    .source_device()
                    .is_some_and(|device| device.source() == gdk::InputSource::Mouse);
                if wheel {
                    (MouseScrollDelta::LineDelta(-dx as f32, -dy as f32), false)
                } else {
                    let scale = self.scale_factor();
                    let delta = PhysicalPosition::new(-dx * 10. * scale, -dy * 10. * scale);
                    (MouseScrollDelta::PixelDelta(delta), true)
                }
            },
            // Emulated from smooth events, which are delivered as well.
            _ if event.is_pointer_emulated() => return Propagation::Stop,
            gdk::ScrollDirection::Up => (MouseScrollDelta::LineDelta(0., 1.), false),
            gdk::ScrollDirection::Down => (MouseScrollDelta::LineDelta(0., -1.), false),
            gdk::ScrollDirection::Left => (MouseScrollDelta::LineDelta(1., 0.), false),
            gdk::ScrollDirection::Right => (MouseScrollDelta::LineDelta(-1., 0.), false),
            _ => return Propagation::Stop,
        };

        // Phases as in winit 0.30.13's Wayland pointer (`seat/pointer/mod.rs`).
        let phase = if event.is_stop() {
            self.scroll_active.set(false);
            TouchPhase::Ended
        } else if !pixel {
            self.scroll_active.set(false);
            TouchPhase::Moved
        } else if self.scroll_active.replace(true) {
            TouchPhase::Moved
        } else {
            TouchPhase::Started
        };

        self.queue_event(WindowEvent::MouseWheel { delta, phase });
        Propagation::Stop
    }

    fn ime_commit(&self, text: &str) {
        if let Some(filter) = self.key_filter.borrow_mut().as_mut() {
            filter.commits.push(text.to_owned());
            return;
        }
        self.commit_ime(text.to_owned());
    }

    fn commit_ime(&self, text: String) {
        let _batch = EventBatch::new(self.session);
        self.enable_ime();
        self.ime_preedit.set(false);
        self.queue_event(WindowEvent::Ime(Ime::Preedit(String::new(), None)));
        self.queue_event(WindowEvent::Ime(Ime::Commit(text)));
    }

    fn enable_ime(&self) {
        if !self.ime_enabled.replace(true) {
            self.queue_event(WindowEvent::Ime(Ime::Enabled));
        }
    }

    fn ime_preedit_changed(&self) {
        if let Some(filter) = self.key_filter.borrow_mut().as_mut() {
            filter.preedit_changed = true;
        }

        let (text, _attributes, cursor) = self.im.preedit_string();
        let _batch = EventBatch::new(self.session);
        if text.is_empty() {
            if self.ime_preedit.replace(false) {
                self.queue_event(WindowEvent::Ime(Ime::Preedit(String::new(), None)));
            }
            return;
        }

        self.enable_ime();
        self.ime_preedit.set(true);
        // GTK reports the cursor in characters; winit in UTF-8 bytes.
        let cursor = usize::try_from(cursor).unwrap_or(0);
        let byte = text.char_indices().nth(cursor).map_or(text.len(), |(index, _)| index);
        self.queue_event(WindowEvent::Ime(Ime::Preedit(text.to_string(), Some((byte, byte)))));
    }

    pub(crate) fn set_ime_allowed(&self, allowed: bool) {
        if self.ime_allowed.replace(allowed) == allowed {
            return;
        }

        if allowed {
            if self.widget.has_focus() {
                self.im.focus_in();
            }
            return;
        }

        self.im.focus_out();
        self.im.reset();
        self.ime_preedit.set(false);
        if self.ime_enabled.replace(false) {
            self.queue_event(WindowEvent::Ime(Ime::Disabled));
        }
    }

    /// Candidate window position, in logical pixels relative to the view.
    pub(crate) fn set_ime_cursor_area(&self, x: f64, y: f64, width: f64, height: f64) {
        // Relative to the IM client window, the widget's GDK window.
        let allocation = self.widget.allocation();
        let area = gdk::Rectangle::new(
            allocation.x() + x as i32,
            allocation.y() + y as i32,
            width as i32,
            height as i32,
        );
        self.im.set_cursor_location(&area);
    }

    pub(crate) fn set_cursor_icon(&self, icon: CursorIcon) {
        self.cursor_icon.set(icon);
        self.apply_cursor();
    }

    /// Set whether the cursor should be visible or not.
    pub(crate) fn set_cursor_visible(&self, visible: bool) {
        if self.cursor_visible.replace(visible) != visible {
            self.apply_cursor();
        }
    }

    /// Apply the cursor to the widget's GDK window (the overlay's window for this child).
    fn apply_cursor(&self) {
        let window = match self.widget.window() {
            Some(window) => window,
            None => return,
        };
        // Never change the cursor of the whole web view.
        if self.web.web_view().window().as_ref() == Some(&window) {
            return;
        }

        let display = window.display();
        let cursor = if self.cursor_visible.get() {
            gdk::Cursor::from_name(&display, self.cursor_icon.get().name())
                .or_else(|| gdk::Cursor::from_name(&display, "default"))
        } else {
            gdk::Cursor::for_display(&display, gdk::CursorType::BlankCursor)
        };
        window.set_cursor(cursor.as_ref());
    }
}

impl Drop for TerminalView {
    fn drop(&mut self) {
        self.detach();
    }
}

/// GDK state bit of the pressed `button`, included in motion checks of its press.
fn button_mask(button: u32) -> gdk::ModifierType {
    match button {
        1 => gdk::ModifierType::BUTTON1_MASK,
        2 => gdk::ModifierType::BUTTON2_MASK,
        3 => gdk::ModifierType::BUTTON3_MASK,
        4 => gdk::ModifierType::BUTTON4_MASK,
        5 => gdk::ModifierType::BUTTON5_MASK,
        _ => gdk::ModifierType::empty(),
    }
}
