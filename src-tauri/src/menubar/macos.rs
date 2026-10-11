//! AppKit implementation for the Tauri-owned menu-bar item and webview panel.
use super::{anchored_frame, Rect, MIN_HEIGHT, WIDTH};
use crate::{macos::defer_on_main, tray_model::BarCounts};
use block2::RcBlock;
use objc2::{
    define_class, msg_send,
    rc::Retained,
    runtime::{AnyClass, ProtocolObject},
    sel, ClassType, MainThreadMarker, MainThreadOnly,
};
use objc2_app_kit::{
    NSColor, NSEvent, NSEventMask, NSEventModifierFlags, NSEventType, NSPanel,
    NSPopUpMenuWindowLevel, NSWindow, NSWindowCollectionBehavior, NSWindowDidResignKeyNotification,
    NSWindowStyleMask, NSWorkspace,
};
use objc2_foundation::{
    NSNotification, NSNotificationCenter, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString,
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

struct PanelState {
    panel: Retained<MenuBarPanel>,
    height: f64,
    // Retain both observer tokens and remove them on teardown rather than leaking them.
    observers: Vec<NotificationObserver>,
}
impl Drop for PanelState {
    fn drop(&mut self) {
        for (center, token) in &self.observers {
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
    // SAFETY: setup runs on main, the just-built Tauri window owns this live allocation.
    let panel = unsafe {
        let old = &*raw.cast::<NSWindow>();
        let class = MenuBarPanel::class();
        if class_getInstanceSize(class) > class_getInstanceSize(old.class()) {
            return Err(tauri::Error::Io(std::io::Error::other(
                "AppKit panel layout is larger than the Tauri window allocation",
            )));
        }
        // An already-initialized NSWindow lacks the WindowServer nonactivation tag. AppKit
        // does not synchronize it on setStyleMask; fail explicitly if the isolated SPI is
        // unavailable, rather than foregrounding the main app as a fake panel fallback.
        if !old.respondsToSelector(sel!(_setPreventsActivation:)) {
            return Err(tauri::Error::Io(std::io::Error::other(
                "AppKit does not support nonactivating panel conversion",
            )));
        }
        object_setClass(raw, class);
        let panel = Retained::retain(raw.cast::<MenuBarPanel>()).expect("live panel");
        panel.setStyleMask(NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel);
        let _: () = msg_send![&*panel, _setPreventsActivation: true];
        panel
    };
    panel.setFloatingPanel(true);
    panel.setBecomesKeyOnlyIfNeeded(false);
    panel.setHidesOnDeactivate(false);
    // A menu-bar panel must remain independently orderable while Cmd+H hides the app's
    // main window; do not unhide/activate the application just to open the panel.
    panel.setCanHide(false);
    panel.setLevel(NSPopUpMenuWindowLevel);
    panel.setCollectionBehavior(
        NSWindowCollectionBehavior::CanJoinAllSpaces
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
    let name = NSString::from_str("NSWorkspaceDidChangeActiveSpaceNotification");
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
    let observer = unsafe {
        center.addObserverForName_object_queue_usingBlock(Some(&name), None, None, &block)
    };
    observers.push((center, observer));
    let center = NSNotificationCenter::defaultCenter();
    let name = NSString::from_str("NSApplicationDidChangeScreenParametersNotification");
    let observer = unsafe {
        center.addObserverForName_object_queue_usingBlock(Some(&name), None, None, &block)
    };
    observers.push((center, observer));
    PANEL.with(|state| {
        *state.borrow_mut() = Some(PanelState {
            panel,
            height: MIN_HEIGHT,
            observers,
        })
    });
    install_status_click_monitor(app.handle())?;
    Ok(())
}

/// Monitor the *actual* status-window mouse event: Tauri's click payload has no modifiers.
/// Ctrl-left click is delivered to the existing tray view as right click, so tray-icon owns
/// menu tracking/routing for both forms. Snapshot visibility before resign-key dismissal so
/// clicking the item a second time hides instead of immediately reopening the panel.
fn install_status_click_monitor(app: &AppHandle) -> tauri::Result<()> {
    main_thread();
    let global_app = app.clone();
    let global: RcBlock<dyn Fn(NonNull<NSEvent>)> = RcBlock::new(move |_| {
        let app = global_app.clone();
        defer_on_main(Box::new(move || {
            if let Err(error) = hide(&app) {
                log::warn!("popover outside click: {error}");
            }
        }));
    });
    let monitor = NSEvent::addGlobalMonitorForEventsMatchingMask_handler(
        NSEventMask::LeftMouseDown | NSEventMask::RightMouseDown | NSEventMask::OtherMouseDown,
        &global,
    )
    .ok_or_else(|| {
        tauri::Error::Io(std::io::Error::other(
            "Unable to install the popover outside-click monitor",
        ))
    })?;
    std::mem::forget(monitor);
    let app = app.clone();
    let block: RcBlock<dyn Fn(NonNull<NSEvent>) -> *mut NSEvent> =
        RcBlock::new(move |event: NonNull<NSEvent>| {
            let event_ref = unsafe { event.as_ref() };
            let mtm = main_thread();
            let event_window = event_ref.window(mtm);
            let status_window = STATUS_WINDOW.with(|window| window.borrow().clone());
            let in_status = status_window.as_ref().is_some_and(|status| {
                event_window
                    .as_ref()
                    .is_some_and(|window| Retained::as_ptr(window) == Retained::as_ptr(status))
            });
            if !in_status {
                let in_panel = PANEL.with(|state| {
                    state.borrow().as_ref().is_some_and(|state| {
                        event_window.as_ref().is_some_and(|window| {
                            Retained::as_ptr(window).cast::<c_void>()
                                == Retained::as_ptr(&state.panel).cast::<c_void>()
                        })
                    })
                });
                if !in_panel && event_ref.r#type() != NSEventType::LeftMouseUp {
                    let app = app.clone();
                    defer_on_main(Box::new(move || {
                        if let Err(error) = hide(&app) {
                            log::warn!("popover local click-away: {error}");
                        }
                    }));
                }
                return event.as_ptr();
            }
            let status_window = status_window.expect("matched status window");
            if event_ref
                .modifierFlags()
                .contains(NSEventModifierFlags::Command)
            {
                return event.as_ptr(); // Preserve the system's Cmd-drag rearrangement.
            }
            if event_ref.r#type() == NSEventType::RightMouseDown {
                let app = app.clone();
                defer_on_main(Box::new(move || {
                    if let Err(error) = hide(&app) {
                        log::warn!("popover context menu: {error}");
                    }
                }));
                return event.as_ptr();
            }
            if event_ref.r#type() == NSEventType::OtherMouseDown {
                return event.as_ptr();
            }
            if event_ref
                .modifierFlags()
                .contains(NSEventModifierFlags::Control)
            {
                let app = app.clone();
                defer_on_main(Box::new(move || {
                    if let Err(error) = hide(&app) {
                        log::warn!("popover control-click: {error}");
                    }
                }));
                if let Some(view) = status_window
                    .contentView()
                    .and_then(|view| view.hitTest(event_ref.locationInWindow()))
                {
                    unsafe {
                        let _: () = if event_ref.r#type() == NSEventType::LeftMouseDown {
                            msg_send![&*view, rightMouseDown: event_ref]
                        } else {
                            msg_send![&*view, rightMouseUp: event_ref]
                        };
                    }
                    return std::ptr::null_mut();
                }
            } else if event_ref.r#type() == NSEventType::LeftMouseDown {
                let was_open = PANEL.with(|state| {
                    state
                        .borrow()
                        .as_ref()
                        .is_some_and(|state| state.panel.isVisible())
                });
                let app = app.clone();
                defer_on_main(Box::new(move || {
                    let result = if was_open { hide(&app) } else { show(&app) };
                    if let Err(error) = result {
                        log::warn!("popover toggle: {error}");
                    }
                }));
            } else if event_ref.r#type() == NSEventType::LeftMouseUp {
                // tray-icon clears its highlight on mouse-up; restore the panel's latched
                // highlight only after its native handler has returned.
                defer_on_main(Box::new(|| {
                    let open = PANEL.with(|state| {
                        state
                            .borrow()
                            .as_ref()
                            .is_some_and(|state| state.panel.isVisible())
                    });
                    super::render::highlight(open);
                }));
            }
            event.as_ptr()
        });
    let monitor = unsafe {
        NSEvent::addLocalMonitorForEventsMatchingMask_handler(
            NSEventMask::LeftMouseDown
                | NSEventMask::LeftMouseUp
                | NSEventMask::RightMouseDown
                | NSEventMask::OtherMouseDown,
            &block,
        )
    };
    // Like the application's key router, the status click monitor lives for the app lifetime.
    monitor
        .ok_or_else(|| {
            tauri::Error::Io(std::io::Error::other(
                "Unable to install the menu-bar click monitor",
            ))
        })
        .map(std::mem::forget)
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
    app.get_webview_window(WINDOW)
        .ok_or("popover webview is unavailable")?
        .emit("popover-opened", serde_json::json!({}))
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
        .with_webview(|webview| unsafe { crate::macos::focus_webview(webview.inner()) })
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
        if let Some(window) = app.get_webview_window(WINDOW) {
            window
                .emit("popover-hidden", serde_json::json!({}))
                .map_err(|error| error.to_string())?;
        }
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
