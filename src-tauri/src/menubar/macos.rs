//! AppKit implementation for the Tauri-owned menu-bar item and webview panel.
use super::{
    anchored_frame, click_route, panel_compatibility, startup_policy, ClickRoute, ClickTarget,
    MouseButton, MousePhase, Rect, MIN_HEIGHT, WIDTH,
};
use crate::{macos::defer_on_main, tray_model::BarCounts};
use block2::RcBlock;
use objc2::{
    define_class, msg_send,
    rc::Retained,
    runtime::{AnyClass, AnyObject, ProtocolObject},
    sel, ClassType, MainThreadMarker, MainThreadOnly,
};
use objc2_app_kit::{
    NSApplicationDidChangeScreenParametersNotification, NSColor, NSEvent, NSEventMask,
    NSEventModifierFlags, NSEventType, NSPanel, NSPopUpMenuWindowLevel, NSWindow,
    NSWindowCollectionBehavior, NSWindowDidResignKeyNotification, NSWindowStyleMask, NSWorkspace,
    NSWorkspaceActiveSpaceDidChangeNotification,
};
use objc2_foundation::{
    NSNotification, NSNotificationCenter, NSObjectProtocol, NSPoint, NSRect, NSSize,
};
use std::{cell::RefCell, ffi::c_void, ptr::NonNull};
use tauri::{
    tray::TrayIcon, App, AppHandle, Emitter, Manager, Runtime, WebviewUrl, WebviewWindowBuilder,
};

pub const WINDOW: &str = "popover";

define_class!(
    // No ivars: this class is installed on an existing TaoWindow allocation. We check the
    // instance sizes first; the Tauri delegate and content view are left in place.
    #[unsafe(super(NSPanel))]
    #[name = "U9KMenuBarPanel"]
    #[thread_kind = MainThreadOnly]
    pub struct MenuBarPanel;

    impl MenuBarPanel {
        #[unsafe(method(canBecomeKeyWindow))]
        fn can_become_key(&self) -> bool { true }

        #[unsafe(method(canBecomeMainWindow))]
        fn can_become_main(&self) -> bool { false }
    }
);

extern "C" {
    fn object_setClass(object: *mut c_void, class: *const AnyClass) -> *const AnyClass;
    fn class_getInstanceSize(class: *const AnyClass) -> usize;
}

type NotificationObserver = (
    Retained<NSNotificationCenter>,
    Retained<ProtocolObject<dyn NSObjectProtocol>>,
);

struct EventMonitors(Vec<Retained<AnyObject>>);
impl Drop for EventMonitors {
    fn drop(&mut self) {
        for token in &self.0 {
            // SAFETY: these are this module's retained AppKit monitor tokens. Each token
            // is removed once, on main, before releasing its block/captured app handle.
            unsafe { NSEvent::removeMonitor(token) };
        }
    }
}

struct PanelState {
    panel: Retained<NSWindow>,
    original_class: &'static AnyClass,
    original_style: NSWindowStyleMask,
    monitors: Option<EventMonitors>,
    height: f64,
    // Retain both observer tokens and remove them on teardown rather than leaking them.
    observers: Vec<NotificationObserver>,
}
impl Drop for PanelState {
    fn drop(&mut self) {
        for (center, token) in &self.observers {
            // SAFETY: each retained token was registered on this center and is removed
            // exactly once on main before PanelState releases its live window.
            unsafe { center.removeObserver((**token).as_ref()) };
        }
    }
}
thread_local! {
    static PANEL: RefCell<Option<PanelState>> = const { RefCell::new(None) };
    // tray-icon recreates NSStatusItem when [tray] show returns; never capture the first
    // status window permanently in the event monitor.
    static STATUS_WINDOW: RefCell<Option<Retained<NSWindow>>> = const { RefCell::new(None) };
}

fn main_thread() -> MainThreadMarker {
    MainThreadMarker::new().expect("menu-bar AppKit call off the main thread")
}

pub fn is_popover_window(window: &NSWindow) -> bool {
    PANEL.with(|state| {
        state.borrow().as_ref().is_some_and(|state| {
            std::ptr::from_ref(window).cast::<c_void>()
                == Retained::as_ptr(&state.panel).cast::<c_void>()
        })
    })
}

pub fn create(app: &App) -> tauri::Result<()> {
    main_thread();
    let window = WebviewWindowBuilder::new(app, WINDOW, WebviewUrl::default())
        .title("Ultra 9000 — Menu Bar")
        .inner_size(WIDTH, MIN_HEIGHT)
        .visible(false)
        .focused(false)
        .decorations(false)
        .transparent(true)
        .background_color(tauri::window::Color(0, 0, 0, 0))
        .shadow(true)
        .resizable(false)
        .skip_taskbar(true)
        .zoom_hotkeys_enabled(false)
        .on_navigation(crate::is_local_url)
        .build()?;
    let raw = window.ns_window()?;
    // SAFETY: setup runs on main; Tauri owns this live NSWindow allocation. The replacement
    // is ivar-free and must fit. Never call set_focusable after conversion (Tao's ivar/KVO
    // class was replaced); a successfully configured panel lives for the whole process.
    // Failed setup alone restores the original class before destroying the Tauri window.
    let (panel, original_class, original_style) = unsafe {
        let old = &*raw.cast::<NSWindow>();
        let class = MenuBarPanel::class();
        let original_class = old.class();
        let original_style = old.styleMask();
        panel_compatibility(
            class_getInstanceSize(original_class),
            class_getInstanceSize(class),
            old.respondsToSelector(sel!(_setPreventsActivation:)),
        )
        .map_err(|message| tauri::Error::Io(std::io::Error::other(message)))?;
        object_setClass(raw, class);
        let panel = Retained::retain(raw.cast::<MenuBarPanel>()).expect("live panel");
        panel.setStyleMask(NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel);
        // AppKit's private setter takes BOOL and returns void; respondsToSelector checked
        // availability above, and objc2 checks this msg_send signature in debug builds.
        let _: () = msg_send![&*panel, _setPreventsActivation: true];
        (panel, original_class, original_style)
    };
    panel.setFloatingPanel(true);
    panel.setBecomesKeyOnlyIfNeeded(false);
    panel.setHidesOnDeactivate(false);
    // A menu-bar panel must remain independently orderable while Cmd+H hides the app's
    // main window; do not unhide/activate the application just to open the panel.
    panel.setCanHide(false);
    panel.setLevel(NSPopUpMenuWindowLevel);
    panel.setCollectionBehavior(
        NSWindowCollectionBehavior::MoveToActiveSpace
            | NSWindowCollectionBehavior::FullScreenAuxiliary
            | NSWindowCollectionBehavior::Transient
            | NSWindowCollectionBehavior::IgnoresCycle,
    );
    panel.setOpaque(false);
    panel.setBackgroundColor(Some(&NSColor::clearColor()));
    panel.setHasShadow(true);
    panel.setMovable(false);
    let mut observers = Vec::with_capacity(3);
    let center = NSNotificationCenter::defaultCenter();
    let app_handle = app.handle().clone();
    let block: RcBlock<dyn Fn(NonNull<NSNotification>)> = RcBlock::new(move |_| {
        let app = app_handle.clone();
        defer_on_main(Box::new(move || {
            if let Err(error) = hide(&app) {
                log::warn!("popover resign-key: {error}");
            }
        }));
    });
    // SAFETY: callbacks capture only the sendable app handle; all AppKit is deferred to main.
    let observer = unsafe {
        center.addObserverForName_object_queue_usingBlock(
            Some(NSWindowDidResignKeyNotification),
            Some(&panel),
            None,
            &block,
        )
    };
    observers.push((center, observer));
    let center = NSWorkspace::sharedWorkspace().notificationCenter();
    let app_handle = app.handle().clone();
    let block: RcBlock<dyn Fn(NonNull<NSNotification>)> = RcBlock::new(move |_| {
        let app = app_handle.clone();
        defer_on_main(Box::new(move || {
            if let Err(error) = hide(&app) {
                log::warn!("popover Space change: {error}");
            }
            super::render::refresh_appearance();
        }));
    });
    // SAFETY: the typed AppKit name is read on main. Workspace notifications are posted
    // on sharedWorkspace().notificationCenter(), not the default application center;
    // the retained block captures only a sendable app handle and defers all mutations.
    let observer = unsafe {
        center.addObserverForName_object_queue_usingBlock(
            Some(NSWorkspaceActiveSpaceDidChangeNotification),
            None,
            None,
            &block,
        )
    };
    observers.push((center, observer));
    let center = NSNotificationCenter::defaultCenter();
    // SAFETY: this typed AppKit notification is posted on the default center. Its block
    // is retained by the center and captures only a sendable app handle.
    let observer = unsafe {
        center.addObserverForName_object_queue_usingBlock(
            Some(NSApplicationDidChangeScreenParametersNotification),
            None,
            None,
            &block,
        )
    };
    observers.push((center, observer));
    PANEL.with(|state| {
        *state.borrow_mut() = Some(PanelState {
            panel: panel.into_super().into_super(),
            original_class,
            original_style,
            monitors: None,
            height: MIN_HEIGHT,
            observers,
        })
    });
    let monitors = install_status_click_monitor(app.handle())?;
    PANEL.with(|state| {
        if let Some(state) = state.borrow_mut().as_mut() {
            state.monitors = Some(monitors);
        }
    });
    Ok(())
}

/// Optional popover failures must never take down the terminal/workspace or its menu.
pub fn recover_menu_only(app: &AppHandle) {
    main_thread();
    let window = app.get_webview_window(WINDOW);
    let policy = startup_policy(false, window.is_some());
    if let Some(mut state) = PANEL.with(|state| state.borrow_mut().take()) {
        for (center, token) in state.observers.drain(..) {
            // SAFETY: draining consumes each registration once on its original center;
            // no resign-key callback can run after restoring the old Objective-C class.
            unsafe { center.removeObserver((*token).as_ref()) };
        }
        drop(state.monitors.take());
        state.panel.orderOut(None);
        // SAFETY: only a failed, never-shown setup reaches this path, on main. All of our
        // observers/monitors are gone. NSWindow is valid before and after restoring the
        // saved runtime class; the private selector takes BOOL and returns void as above.
        unsafe {
            let _: () = msg_send![&*state.panel, _setPreventsActivation: false];
            state.panel.setStyleMask(state.original_style);
            object_setClass(
                Retained::as_ptr(&state.panel).cast_mut().cast(),
                state.original_class,
            );
        }
    }
    if policy.destroy_popover {
        if let Some(window) = window {
            if let Err(error) = window.destroy() {
                log::warn!("Unable to destroy failed popover: {error}");
            }
        }
    }
    if let Some(tray) = app.try_state::<crate::lifecycle::MenuBarIcon<tauri::Wry>>() {
        if let Err(error) = tray
            .0
            .set_show_menu_on_left_click(policy.menu_on_left_click)
        {
            log::error!("Unable to restore the native left-click menu: {error}");
        }
    }
}

fn panel_open() -> bool {
    PANEL.with(|state| {
        state
            .borrow()
            .as_ref()
            .is_some_and(|state| state.panel.isVisible())
    })
}

/// Monitor the *actual* status-window mouse event: Tauri's click payload has no modifiers.
/// Ctrl-left click is delivered to the existing tray view as right click, so tray-icon owns
/// menu tracking/routing for both forms. Snapshot visibility before resign-key dismissal so
/// clicking the item a second time hides instead of immediately reopening the panel.
fn install_status_click_monitor(app: &AppHandle) -> tauri::Result<EventMonitors> {
    main_thread();
    let global_app = app.clone();
    let global: RcBlock<dyn Fn(NonNull<NSEvent>)> = RcBlock::new(move |_| {
        if click_route(
            ClickTarget::Other,
            MouseButton::Left,
            MousePhase::Down,
            false,
            false,
            panel_open(),
        ) != ClickRoute::Dismiss
        {
            return;
        }
        let app = global_app.clone();
        defer_on_main(Box::new(move || {
            if let Err(error) = hide(&app) {
                log::warn!("popover outside click: {error}");
            }
        }));
    });
    let global = NSEvent::addGlobalMonitorForEventsMatchingMask_handler(
        NSEventMask::LeftMouseDown | NSEventMask::RightMouseDown | NSEventMask::OtherMouseDown,
        &global,
    )
    .ok_or_else(|| {
        tauri::Error::Io(std::io::Error::other(
            "Unable to install the popover outside-click monitor",
        ))
    })?;
    // Own each registration immediately: if local-monitor setup fails, this guard removes
    // the global monitor before create returns its error to the menu-only recovery path.
    let mut monitors = EventMonitors(Vec::with_capacity(2));
    monitors.0.push(global);
    let app = app.clone();
    let block: RcBlock<dyn Fn(NonNull<NSEvent>) -> *mut NSEvent> =
        RcBlock::new(move |event: NonNull<NSEvent>| {
            // SAFETY: AppKit supplies a live event for this synchronous main-thread callback;
            // the reference is not retained past the callback and mutations are deferred.
            let event_ref = unsafe { event.as_ref() };
            let mtm = main_thread();
            let event_window = event_ref.window(mtm);
            let status_window = STATUS_WINDOW.with(|window| window.borrow().clone());
            let target = if status_window.as_ref().is_some_and(|status| {
                event_window
                    .as_ref()
                    .is_some_and(|window| Retained::as_ptr(window) == Retained::as_ptr(status))
            }) {
                ClickTarget::Status
            } else if event_window
                .as_ref()
                .is_some_and(|window| is_popover_window(window))
            {
                ClickTarget::Panel
            } else {
                ClickTarget::Other
            };
            let (button, phase) = match event_ref.r#type() {
                NSEventType::LeftMouseDown => (MouseButton::Left, MousePhase::Down),
                NSEventType::LeftMouseUp => (MouseButton::Left, MousePhase::Up),
                NSEventType::RightMouseDown => (MouseButton::Right, MousePhase::Down),
                _ => (MouseButton::Other, MousePhase::Down),
            };
            let open = panel_open();
            let modifiers = event_ref.modifierFlags();
            let route = click_route(
                target,
                button,
                phase,
                modifiers.contains(NSEventModifierFlags::Control),
                modifiers.contains(NSEventModifierFlags::Command),
                open,
            );
            match route {
                ClickRoute::Pass => {}
                ClickRoute::ContextDown | ClickRoute::ContextUp => {
                    if open {
                        let app = app.clone();
                        defer_on_main(Box::new(move || {
                            if let Err(error) = hide(&app) {
                                log::warn!("popover control-click: {error}");
                            }
                        }));
                    }
                    if let Some(view) = status_window
                        .and_then(|window| window.contentView())
                        .and_then(|view| view.hitTest(event_ref.locationInWindow()))
                    {
                        // SAFETY: hitTest returns a live responder in the retained status
                        // window on main. NSResponder's rightMouseDown:/rightMouseUp: both
                        // take NSEvent* and return void; direct dispatch preserves tray-icon's
                        // existing menu tracking and does not retain this event pointer.
                        unsafe {
                            let _: () = if route == ClickRoute::ContextDown {
                                msg_send![&*view, rightMouseDown: event_ref]
                            } else {
                                msg_send![&*view, rightMouseUp: event_ref]
                            };
                        }
                        return std::ptr::null_mut();
                    }
                }
                ClickRoute::Dismiss | ClickRoute::Hide | ClickRoute::Show => {
                    let app = app.clone();
                    defer_on_main(Box::new(move || {
                        let result = if route == ClickRoute::Show {
                            show(&app)
                        } else {
                            hide(&app)
                        };
                        if let Err(error) = result {
                            log::warn!("popover click: {error}");
                        }
                    }));
                }
                ClickRoute::RestoreHighlight => {
                    // tray-icon clears highlight in its mouse-up handler; restore it after
                    // that callback, only if this panel remains open.
                    defer_on_main(Box::new(|| super::render::highlight(panel_open())));
                }
            }
            event.as_ptr()
        });
    // SAFETY: AppKit retains this block until removal. Returned event pointers are either
    // the same live input pointer or null (consumed); captures stay owned by this monitor.
    let local = unsafe {
        NSEvent::addLocalMonitorForEventsMatchingMask_handler(
            NSEventMask::LeftMouseDown
                | NSEventMask::LeftMouseUp
                | NSEventMask::RightMouseDown
                | NSEventMask::OtherMouseDown,
            &block,
        )
    }
    .ok_or_else(|| {
        tauri::Error::Io(std::io::Error::other(
            "Unable to install the menu-bar click monitor",
        ))
    })?;
    monitors.0.push(local);
    Ok(monitors)
}

fn rect(frame: NSRect) -> Rect {
    Rect {
        x: frame.origin.x,
        y: frame.origin.y,
        width: frame.size.width,
        height: frame.size.height,
    }
}

/// Called only on main; with_inner_tray_icon also executes its callback on main.
fn position<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    main_thread();
    let tray = app
        .try_state::<crate::lifecycle::MenuBarIcon<R>>()
        .ok_or("menu-bar item is unavailable")?;
    tray.0
        .with_inner_tray_icon(|inner| {
            let mtm = main_thread();
            let status = inner
                .ns_status_item()
                .ok_or("native status item is unavailable")?;
            let button = status
                .button(mtm)
                .ok_or("native status button is unavailable")?;
            let window = button.window().ok_or("status button has no window")?;
            let screen = window.screen().ok_or("status button has no screen")?;
            PANEL.with(|state| {
                let state = state.borrow();
                let state = state.as_ref().ok_or("popover is unavailable")?;
                let frame = anchored_frame(
                    rect(window.frame()),
                    rect(screen.visibleFrame()),
                    state.height,
                );
                state.panel.setFrame_display(
                    NSRect::new(
                        NSPoint::new(frame.x, frame.y),
                        NSSize::new(frame.width, frame.height),
                    ),
                    true,
                );
                state.panel.invalidateShadow();
                Ok::<(), String>(())
            })
        })
        .map_err(|error| error.to_string())?
}

fn show<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    position(app)?;
    focus_panel(app)?;
    super::render::highlight(true);
    app.emit_to(WINDOW, "popover-opened", serde_json::json!({}))
        .map_err(|error| error.to_string())
}

fn focus_panel<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    PANEL.with(|state| {
        let state = state.borrow();
        let state = state.as_ref().ok_or("popover is unavailable")?;
        state.panel.orderFrontRegardless();
        state.panel.makeKeyWindow();
        Ok::<(), String>(())
    })?;
    // Do not call Tauri set_focus/show: those paths activate NSApplication.
    let window = app
        .get_webview_window(WINDOW)
        .ok_or("popover webview is unavailable")?;
    window
        .with_webview(|webview| {
            // SAFETY: Tauri calls this on main with the live, window-owned WKWebView.
            unsafe { crate::macos::focus_webview(webview.inner()) }
        })
        .map_err(|error| error.to_string())
}

pub fn hide<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    main_thread();
    let was_open = PANEL.with(|state| {
        let state = state.borrow();
        let Some(state) = state.as_ref() else {
            return false;
        };
        let visible = state.panel.isVisible();
        state.panel.orderOut(None);
        visible
    });
    super::render::highlight(false);
    if was_open {
        app.emit_to(WINDOW, "popover-hidden", serde_json::json!({}))
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

pub fn set_height<R: Runtime>(app: &AppHandle<R>, height: f64) -> Result<(), String> {
    main_thread();
    let visible = PANEL.with(|state| {
        let mut state = state.borrow_mut();
        let state = state.as_mut().ok_or("popover is unavailable")?;
        state.height = height;
        Ok::<bool, String>(state.panel.isVisible())
    })?;
    // The bundled hidden webview may lay out while [tray] show removes the status item.
    // Save its content height now and anchor only when actually shown.
    if visible {
        position(app)
    } else {
        Ok(())
    }
}

/// One native rendering entry, cached by counts and the button's effectiveAppearance.
pub fn update_menu_bar_title<R: Runtime>(
    tray: &TrayIcon<R>,
    counts: BarCounts,
) -> tauri::Result<()> {
    tray.with_inner_tray_icon(move |inner| {
        let button = inner
            .ns_status_item()
            .and_then(|status| status.button(main_thread()));
        STATUS_WINDOW.with(|window| {
            *window.borrow_mut() = button.as_ref().and_then(|button| button.window());
        });
        super::render::update(button, counts);
    })
}
