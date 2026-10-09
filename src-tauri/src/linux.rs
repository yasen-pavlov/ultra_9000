//! GTK pieces of the application lifecycle that Tauri does not expose (Linux counterpart of
//! `macos.rs`).
//!
//! Tauri runs GTK 3 on the main thread. Modal dialogs run a nested GTK main loop: callers hold
//! no workspace or terminal borrow across them and revalidate their target afterwards.

use alacritty_embed::embed::{AppKey, NamedKey};
use std::cell::{Cell, RefCell};
use std::path::Path;
use std::rc::Rc;

use gtk::glib::{self, ControlFlow, Priority, Propagation};
use gtk::prelude::*;
use gtk::{gdk, FileChooserAction, ResponseType};

pub type Task = Box<dyn FnOnce() + Send>;

/// Main window handle used for focus decisions.
pub type MainWindow = gtk::Window;

/// Run `task` on the main thread after the current GTK callback returns.
///
/// Used to leave key handlers and WebKit IPC callbacks before showing a modal dialog or
/// mutating the workspace. Unlike `MainContext::invoke`, this never runs inline.
pub fn defer_on_main(task: Task) {
    let mut task = Some(task);
    glib::idle_add_full(Priority::DEFAULT, move || {
        if let Some(task) = task.take() {
            task();
        }
        ControlFlow::Break
    });
}

thread_local! {
    static MODAL_ACTIVE: Cell<bool> = const { Cell::new(false) };
    /// Parent of the app's dialogs; set once the main window exists.
    static DIALOG_PARENT: RefCell<Option<gtk::Window>> = const { RefCell::new(None) };
}

/// Held while one of the app's modal decisions (quit, stop, folder picker) is open.
///
/// A second confirmation cannot start while one is open, so destructive decisions never
/// nest; callers revalidate their target after the modal returned.
pub struct ModalGuard(());

impl ModalGuard {
    /// Reserve the modal slot; `None` while another modal decision is open.
    pub fn acquire() -> Option<Self> {
        MODAL_ACTIVE.with(|active| (!active.replace(true)).then_some(Self(())))
    }

    fn is_held() -> bool {
        MODAL_ACTIVE.with(Cell::get)
    }
}

impl Drop for ModalGuard {
    fn drop(&mut self) {
        MODAL_ACTIVE.with(|active| active.set(false));
    }
}

fn dialog_parent() -> Option<gtk::Window> {
    DIALOG_PARENT.with(|parent| parent.borrow().clone())
}

/// Native folder-only chooser starting at `initial` (if it is a directory).
///
/// Returns the chosen directory, `None` on cancel.
pub fn pick_directory(initial: Option<&Path>) -> Option<String> {
    let parent = dialog_parent();
    let dialog = gtk::FileChooserDialog::with_buttons(
        Some("Choose the session's working directory"),
        parent.as_ref(),
        FileChooserAction::SelectFolder,
        &[
            ("Cancel", ResponseType::Cancel),
            ("Choose", ResponseType::Accept),
        ],
    );
    dialog.set_modal(true);
    dialog.set_create_folders(true);
    dialog.set_local_only(true);
    dialog.set_default_response(ResponseType::Accept);
    if let Some(initial) = initial {
        dialog.set_current_folder(initial);
    }

    let chosen = (dialog.run() == ResponseType::Accept)
        .then(|| dialog.filename())
        .flatten();
    // SAFETY: The dialog is owned here and not used afterwards.
    unsafe { dialog.destroy() };

    let chosen = chosen?;
    match chosen.to_str() {
        Some(path) => Some(path.to_owned()),
        None => {
            log::warn!("Chosen directory {} is not valid UTF-8", chosen.display());
            None
        }
    }
}

/// # Safety
/// `view` is the live WebKitWebView, used only on the GTK main thread.
pub unsafe fn focus_webview(view: *mut std::ffi::c_void) {
    use gtk::glib::translate::from_glib_none;
    let widget: gtk::Widget = unsafe { from_glib_none(view.cast::<gtk::ffi::GtkWidget>()) };
    widget.grab_focus();
}

/// The toplevel window containing the web view `web_view` (a `WebKitWebView` widget).
///
/// # Safety
///
/// `web_view` must point to a live `GtkWidget`.
pub unsafe fn window_of_view(web_view: *mut std::ffi::c_void) -> Option<MainWindow> {
    if web_view.is_null() {
        return None;
    }
    let widget: gtk::Widget =
        unsafe { glib::translate::from_glib_none(web_view.cast::<gtk::ffi::GtkWidget>()) };
    let toplevel = widget.toplevel()?;
    if !toplevel.is_toplevel() {
        return None;
    }
    toplevel.downcast::<gtk::Window>().ok()
}

/// Whether `window` is actually on screen for the user: shown, mapped and not minimized.
/// Keyboard focus is only handed to a terminal while this holds.
pub fn window_presentable(window: &MainWindow) -> bool {
    window.is_visible()
        && window.is_mapped()
        && window.window().is_some_and(|window| {
            !window
                .state()
                .intersects(gdk::WindowState::ICONIFIED | gdk::WindowState::WITHDRAWN)
        })
}

/// Build physical app keys independent of the active keyboard layout.
fn app_key(event: &gdk::EventKey) -> Option<AppKey> {
    let state = event.state();
    let named = match event.keyval() {
        gdk::keys::constants::Page_Up => Some(NamedKey::PageUp),
        gdk::keys::constants::Page_Down => Some(NamedKey::PageDown),
        gdk::keys::constants::Up => Some(NamedKey::Up),
        gdk::keys::constants::Down => Some(NamedKey::Down),
        gdk::keys::constants::Left => Some(NamedKey::Left),
        gdk::keys::constants::Right => Some(NamedKey::Right),
        gdk::keys::constants::Home => Some(NamedKey::Home),
        gdk::keys::constants::End => Some(NamedKey::End),
        _ => None,
    };
    Some(AppKey {
        ctrl: state.contains(gdk::ModifierType::CONTROL_MASK),
        shift: state.contains(gdk::ModifierType::SHIFT_MASK),
        // Lock/NumLock (LOCK/MOD2) are ignored. AltGr/extra modifiers must not
        // accidentally look like a bare Ctrl+Shift chord.
        alt: state.intersects(
            gdk::ModifierType::MOD1_MASK
                | gdk::ModifierType::MOD3_MASK
                | gdk::ModifierType::MOD5_MASK,
        ),
        logo: state.intersects(
            gdk::ModifierType::SUPER_MASK
                | gdk::ModifierType::MOD4_MASK
                | gdk::ModifierType::HYPER_MASK
                | gdk::ModifierType::META_MASK,
        ),
        code: event.hardware_keycode().checked_sub(8)?,
        named,
    })
}

/// The router runs on the main thread before GTK forwards to the focus widget.
/// A true result consumes the press. Actions requiring mutation/modal UI must be deferred.
pub fn install_key_router(window: &gtk::Window, router: Rc<dyn Fn(&AppKey) -> bool>) {
    DIALOG_PARENT.with(|parent| *parent.borrow_mut() = Some(window.clone()));
    window.connect_key_press_event(move |_, event| {
        if !ModalGuard::is_held() && app_key(event).is_some_and(|key| router(&key)) {
            Propagation::Stop
        } else {
            Propagation::Proceed
        }
    });
}
