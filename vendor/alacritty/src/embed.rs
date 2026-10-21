//! Embedding API: host independent Alacritty terminal sessions in native views next to the
//! embedder's web view (a `WKWebView` on macOS, a `WebKitWebView` on Linux).
//!
//! This module is new (not upstream). It replaces winit's run loop integration. The registry
//! below is platform independent; the native view, focus handover and main-loop callbacks come
//! from `crate::platform::embedder` (AppKit/libdispatch on macOS, GTK/Wayland/GLib on Linux):
//!
//! - Every session is a [`Slot`] in a main-thread-only registry, keyed by a [`SessionKey`]
//!   that is never reused. Each slot owns its own upstream-derived event [`Processor`]
//!   (terminal, PTY, display with its GL context and renderer, scheduler, clipboard), its
//!   terminal view and its pending-event queue.
//! - Native view callbacks, proxy wakeups and timers all carry the key of the session they
//!   were created for and end in [`pump`] for that session only, which feeds events to its
//!   processor in the same order winit would (window/user events, then `AboutToWait`, then
//!   `RedrawRequested` + draw). Callbacks for a removed session find no slot and are dropped.
//! - PTY events arrive through the session's [`EventLoopProxy`] from its PTY thread. The proxy
//!   wakes the main thread with one main-loop callback per batch, so bursts of output are
//!   coalesced.
//! - Scheduler deadlines (cursor blinking, frame pacing, selection scrolling, delayed search)
//!   arm a single one-shot main-loop timer per session for its earliest deadline. Nothing runs on a
//!   periodic timer while a terminal is idle or hidden.
//! - Each display makes its own GL context current before it renders, resizes or releases
//!   GL resources (upstream multi-window behaviour), so sessions never draw with another
//!   session's context.
//! - View lifetime and PTY lifetime are separate: hiding a view never resizes or restarts its
//!   PTY and hands keyboard focus (first responder) to the web view first, so a hidden terminal never keeps
//!   keyboard focus. Stopping a session ends its PTY but keeps its display and final content;
//!   only removing it releases the display and view.
//! - The host listener (state, bell and raw file-link URI) runs outside every session borrow
//!   and may call back into this module; discrete events share a bell-bounded FIFO.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::ffi::{c_void, CStr};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Instant;

#[cfg(target_os = "macos")]
use alacritty_terminal::term::TermMode;
use alacritty_terminal::tty;

use crate::config;
#[cfg(target_os = "macos")]
use crate::config::Action;
use crate::event::{Event, Processor};
use crate::host::{EventLoopProxy, WindowEvent};
use crate::platform::embedder::{self as native, Frame, View, WebView};
use crate::window_context::{self, WindowContext};


/// embed: Shared physical event conversion for the AppKit monitor and terminal fallback.
///
/// Takes the raw `NSEvent *`, so the embedder need not share this crate's objc2 generation.
///
/// # Safety
///
/// `event` must point to a valid `NSEvent` that stays alive for the duration of the call, and
/// the call must happen on the AppKit main thread.
#[cfg(target_os = "macos")]
pub unsafe fn app_key_from_macos_event(event: std::ptr::NonNull<c_void>) -> AppKey {
    let event = unsafe { event.cast::<objc2_app_kit::NSEvent>().as_ref() };
    crate::platform::macos::view::app_key(event)
}

/// Identifies one hosted terminal session for the lifetime of the process; never reused.
pub type SessionKey = u64;

/// Snapshot of a hosted terminal for the embedder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalState {
    /// Terminal title (set by the shell/applications through OSC sequences).
    pub title: String,
    /// Executable launched for the session.
    pub shell: String,
    /// PID of the process spawned on the PTY; retained after it exits.
    pub pid: Option<u32>,
    /// Whether the spawned process is still running.
    pub running: bool,
    /// embed: Normal child exit code; absent while running or when signalled/stopped.
    pub exit_code: Option<i32>,
    /// Terminal grid rows.
    pub rows: usize,
    /// Terminal grid columns.
    pub cols: usize,
    /// Whether the terminal view has keyboard focus.
    pub focused: bool,
}

/// Terminal rectangle and left corner radii in CSS pixels relative to the web view.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ViewportRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub top_left_radius: f64,
    pub bottom_left_radius: f64,
}

impl ViewportRect {
    /// Reject invalid geometry before reaching GTK/AppKit/GL, and bound both arcs.
    fn validated(mut self) -> Result<Self, String> {
        let values = [
            self.x, self.y, self.width, self.height,
            self.top_left_radius, self.bottom_left_radius,
        ];
        if values.iter().any(|value| !value.is_finite()) {
            return Err("viewport values must be finite".into());
        }
        if self.width < 0. || self.height < 0. {
            return Err("viewport size must not be negative".into());
        }
        if self.top_left_radius < 0. || self.bottom_left_radius < 0. {
            return Err("viewport corner radii must not be negative".into());
        }
        let limit = self.width.min(self.height) / 2.;
        self.top_left_radius = self.top_left_radius.min(limit);
        self.bottom_left_radius = self.bottom_left_radius.min(limit);
        Ok(self)
    }
}

/// Program started on a new session's PTY.
#[derive(Debug, Clone)]
pub struct Launch {
    /// Program and arguments, passed to the program as data (never through a shell).
    ///
    /// `None` starts the configured shell, else the user's login shell exactly like upstream
    /// Alacritty.
    pub command: Option<(String, Vec<String>)>,
    /// Executable reported as the session's shell.
    ///
    /// `None` reports the program of `command`, else the effective shell.
    pub executable: Option<String>,
    /// Working directory of the spawned program.
    pub working_directory: PathBuf,
    /// embed: Overrides merged into this child's environment only.
    pub env: Vec<(String, String)>,
}

/// embed: Main-thread host callback, delivered outside all session borrows.
pub enum HostEvent<'a> {
    State(&'a TerminalState),
    Bell,
    /// Activated file URI, unchanged from the terminal hint.
    OpenFile(&'a str),
}

pub type HostListener = Box<dyn Fn(SessionKey, HostEvent<'_>)>;

/// embed: A physical application key plus the exact semantic modifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AppKey {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub logo: bool,
    /// evdev on Linux, kVK keyCode on macOS.
    pub code: u16,
    pub named: Option<NamedKey>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NamedKey { PageUp, PageDown, Up, Down, Left, Right, Home, End }

/// embed: The terminal view uses the host's predicate, not a duplicated shortcut list.
#[cfg(target_os = "macos")]
pub fn set_application_shortcut_filter(filter: Box<dyn Fn(&AppKey) -> bool>) {
    assert!(native::is_main_thread(), "shortcut filter installed off main thread");
    APPLICATION_SHORTCUT_FILTER.with(|cell| *cell.borrow_mut() = Some(Rc::from(filter)));
}

#[cfg(target_os = "macos")]
pub(crate) fn application_shortcut(key: &AppKey) -> bool {
    let filter = APPLICATION_SHORTCUT_FILTER.with(|cell| cell.borrow().clone());
    filter.is_some_and(|filter| filter(key))
}

#[cfg(target_os = "macos")]
thread_local! {
    static APPLICATION_SHORTCUT_FILTER: RefCell<Option<Rc<dyn Fn(&AppKey) -> bool>>> = RefCell::new(None);
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForegroundJob { pub pgid: i32, pub name: Option<String> }

/// Native clipboard actions (AppKit Edit menu responder actions) routed to Alacritty actions.
#[cfg(target_os = "macos")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ViewAction {
    Copy,
    Paste,
}

/// Work a session's callbacks hand to the main loop, see `crate::platform::embedder::defer`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Deferred {
    /// A pump requested from the main thread.
    Pump,
    /// A proxy wakeup from the PTY thread (or the main thread).
    Wakeup,
    /// The PTY of a stopping session is gone.
    Stopped,
}

enum Pending {
    Window(WindowEvent),
    #[cfg(target_os = "macos")]
    Action(ViewAction),
    Redraw,
}

type PendingQueue = Rc<RefCell<VecDeque<Pending>>>;

/// Outcome of a viewport report, applied in phases around responder changes.
enum ViewportChange {
    /// Applied; the view shows the terminal in this frame.
    Shown(Frame),
    /// Applied; the view stays hidden.
    Hidden,
    /// Accepted; the visible view must give up first responder before it is hidden.
    Hide(HideRequest),
}

/// A visible view that is to be hidden once it gave up first responder.
struct HideRequest {
    view: View,
    web_view: WebView,
    generation: u64,
}

/// Shared embedder state set by [`init`].
struct Embedder {
    web_view: WebView,
    // embed: Replaceable configuration for future sessions.
    config_toml: RefCell<String>,
    listener: Rc<dyn Fn(SessionKey, HostEvent<'_>)>,
    /// Last frame any session was shown in; new sessions start with it.
    last_frame: Cell<Option<Frame>>,
}

/// One session: its host plus the main-thread scheduling state of its callbacks.
struct Slot {
    key: SessionKey,
    host: RefCell<Option<Host>>,
    pending: PendingQueue,
    batch_depth: Cell<u32>,
    in_pump: Cell<bool>,
    async_pump_scheduled: Cell<bool>,
    timer_deadline: Cell<Option<Instant>>,
    timer_generation: Cell<usize>,
}

struct Host {
    processor: Processor,
    proxy: EventLoopProxy<Event>,
    view: View,
    web_view: WebView,
    shell: String,
    pending: PendingQueue,
    last_state: Option<TerminalState>,
    /// Increased by every viewport change, so a deferred hide cannot undo a newer report.
    viewport_generation: u64,
    window_occluded: bool,
    view_hidden: bool,
    /// Completes once a stop requested through [`stop`] has joined the PTY thread.
    stopping: Option<mpsc::Receiver<()>>,
}

thread_local! {
    static EMBEDDER: RefCell<Option<Rc<Embedder>>> = const { RefCell::new(None) };
    static SLOTS: RefCell<HashMap<SessionKey, Rc<Slot>>> = RefCell::new(HashMap::new());
    static NEXT_KEY: Cell<SessionKey> = const { Cell::new(1) };
}

fn main_thread() -> Result<(), String> {
    if native::is_main_thread() {
        Ok(())
    } else {
        Err("terminal API called off the main thread".to_owned())
    }
}

fn embedder() -> Option<Rc<Embedder>> {
    EMBEDDER.with(|embedder| embedder.borrow().clone())
}

fn slot(key: SessionKey) -> Option<Rc<Slot>> {
    SLOTS.with(|slots| slots.borrow().get(&key).cloned())
}

impl Slot {
    /// Run `f` with the host if it exists and is not already borrowed.
    fn with_host<T>(&self, f: impl FnOnce(&mut Host) -> T) -> Option<T> {
        let mut guard = self.host.try_borrow_mut().ok()?;
        guard.as_mut().map(f)
    }

    /// Run `f` with the host if it exists and is not mutably borrowed.
    fn with_host_ref<T>(&self, f: impl FnOnce(&Host) -> T) -> Option<T> {
        let guard = self.host.try_borrow().ok()?;
        guard.as_ref().map(f)
    }
}

/// Set terminal environment variables like upstream Alacritty's `main` (`TERM`, `COLORTERM`),
/// and tell OMP that the terminal speaks the kitty graphics protocol with Unicode placeholders.
///
/// OMP identifies the terminal as Alacritty (`ALACRITTY_WINDOW_ID`), whose profile has no image
/// protocol, so `PI_FORCE_IMAGE_PROTOCOL=kitty` and `PI_KITTY_PLACEHOLDERS=1` opt in. Values the
/// user already exported are kept. Every PTY child inherits these; nothing outside the app's
/// own processes is configured.
///
/// Call once at startup, before other threads read the environment.
pub fn setup_environment() {
    tty::setup_env();

    for (name, value) in [("PI_FORCE_IMAGE_PROTOCOL", "kitty"), ("PI_KITTY_PLACEHOLDERS", "1")] {
        if std::env::var_os(name).is_none() {
            std::env::set_var(name, value);
        }
    }
}

/// Prepare hosting terminal sessions next to `web_view`.
///
/// On macOS `web_view` is the `WKWebView` (an `NSView`); terminal views become its siblings.
/// On Linux it is the `WebKitWebView` `GtkWidget`, alone in a container that is the child of a
/// toplevel window on the Wayland GDK backend (Tauri's layout); a `GtkOverlay` replaces that
/// container and hosts the web view plus the terminals' input widgets above it, and terminals
/// render to Wayland subsurfaces of the toplevel.
///
/// `config_toml` is the Alacritty TOML configuration document used for every session. No
/// session is started.
///
/// # Safety
///
/// `web_view` must point to a live web view as described above: on macOS installed in a
/// window's view hierarchy, on Linux inside a realized toplevel window.
pub unsafe fn init(
    web_view: *mut c_void,
    config_toml: &str,
    listener: HostListener,
) -> Result<(), String> {
    main_thread()?;

    if embedder().is_some() {
        return Err("terminal host already initialized".into());
    }

    // Surface configuration errors at startup rather than on the first launch.
    config::from_toml(config_toml).map_err(|err| err.to_string())?;

    let web_view = unsafe { WebView::from_raw(web_view) }?;

    let embedder = Embedder {
        web_view,
        config_toml: RefCell::new(config_toml.to_owned()),
        listener: Rc::from(listener),
        last_frame: Cell::new(None),
    };
    EMBEDDER.with(|cell| *cell.borrow_mut() = Some(Rc::new(embedder)));

    Ok(())
}

/// embed: Validate without native views; safe to call on a configuration worker thread.
pub fn validate_config(toml: &str) -> Result<(), String> {
    config::from_toml(toml).map(|_| ()).map_err(|err| err.to_string())
}

/// embed: Atomically reject invalid config, then apply supported upstream live options.
pub fn reload_config(toml: &str) -> Result<(), String> {
    main_thread()?;
    let config = Rc::new(config::from_toml(toml).map_err(|err| err.to_string())?);
    let embedder = embedder().ok_or_else(|| "terminal host is not initialized".to_owned())?;
    let slots = SLOTS.with(|slots| slots.borrow().values().cloned().collect::<Vec<_>>());
    // No partial reload if called from inside a borrowed host operation.
    for slot in &slots {
        slot.host.try_borrow_mut().map_err(|_| "terminal session is busy".to_owned())?;
    }
    *embedder.config_toml.borrow_mut() = toml.to_owned();
    for slot in &slots {
        slot.with_host(|host| host.processor.update_config(config.clone()));
        request_slot_pump(slot);
    }
    Ok(())
}

/// embed: Inspect the live PTY's foreground group, never a retained PID after shutdown.
pub fn foreground_job(key: SessionKey) -> Option<ForegroundJob> {
    main_thread().ok()?;
    let pgid = slot(key)?.with_host_ref(|host| {
        host.processor.window_context()?.foreground_process_group()
    })??;
    #[cfg(target_os = "linux")]
    let name = std::fs::read_to_string(format!("/proc/{pgid}/comm"))
        .ok().map(|name| name.trim_end().to_owned()).filter(|name| !name.is_empty());
    #[cfg(target_os = "macos")]
    let name = {
        let mut buf = [0u8; 1024];
        let len = unsafe { libc::proc_name(pgid, buf.as_mut_ptr().cast(), buf.len() as u32) };
        (len > 0).then(|| String::from_utf8_lossy(&buf[..len as usize])
            .trim_end_matches('\0').to_owned()).filter(|name| !name.is_empty())
    };
    Some(ForegroundJob { pgid, name })
}

/// Create a hidden terminal view above the web view and spawn `launch` on its PTY.
///
/// The view stays hidden until the first visible [`set_viewport`]. On failure every partially
/// created resource (view, GL context, terminal, PTY) is released.
pub fn create(launch: Launch) -> Result<SessionKey, String> {
    main_thread()?;
    let embedder = embedder().ok_or_else(|| "terminal host is not initialized".to_owned())?;

    let result = create_inner(&embedder, launch);
    if let Err(err) = &result {
        log::error!("Unable to start terminal session: {err}");
    }
    result
}

fn create_inner(embedder: &Embedder, launch: Launch) -> Result<SessionKey, String> {
    let web_view = embedder.web_view.clone();

    let config = config::from_toml(&embedder.config_toml.borrow()).map_err(|err| err.to_string())?;

    // Same effective shell as `config.pty_config()`, so the reported shell matches the PTY.
    let shell = launch
        .executable
        .clone()
        .or_else(|| launch.command.as_ref().map(|(program, _)| program.clone()))
        .or_else(|| {
            config
                .effective_shell()
                .map(|program| program.program().to_owned())
        })
        .unwrap_or_else(login_shell);

    let mut pty_config = config.pty_config();
    if let Some((program, args)) = launch.command {
        pty_config.shell = Some(tty::Shell::new(program, args));
    }
    pty_config.working_directory = Some(launch.working_directory);
    // embed: Never mutate process-global environment for a launch.
    pty_config.env.extend(launch.env);
    // Every embedded session keeps its final screen after exit, like upstream's `--hold`,
    // which also enables the PTY's exit drain: output still unread when the child exits is
    // parsed (one bounded, non-blocking read) before the terminal is marked exited.
    pty_config.drain_on_exit = true;

    let key = NEXT_KEY.with(|next| {
        let key = next.get();
        next.set(key + 1);
        key
    });

    // Start with the last shown terminal frame (else the web view's) so the GL surface and PTY
    // get the size they will most likely be shown with.
    let view = web_view.create_view(embedder.last_frame.get(), &config, key)?;

    let proxy = EventLoopProxy::new(wake_from_any_thread, key);
    let mut processor = Processor::new(config, proxy.clone());

    if let Err(err) = processor.create_window(view.clone(), pty_config) {
        proxy.close();
        processor.exiting();
        native::detach_view(&view);
        return Err(err.to_string());
    }

    // The display was created from the view's current scale factor and size.
    view.record_reported_geometry();

    let pending = PendingQueue::default();
    let host = Host {
        processor,
        proxy,
        view,
        web_view,
        shell,
        pending: pending.clone(),
        last_state: None,
        viewport_generation: 0,
        window_occluded: false,
        view_hidden: true,
        stopping: None,
    };

    let slot = Rc::new(Slot {
        key,
        host: RefCell::new(Some(host)),
        pending,
        batch_depth: Cell::new(0),
        in_pump: Cell::new(false),
        async_pump_scheduled: Cell::new(false),
        timer_deadline: Cell::new(None),
        timer_generation: Cell::new(0),
    });
    SLOTS.with(|slots| slots.borrow_mut().insert(key, slot.clone()));

    // The view starts hidden: treat it as occluded until it is shown.
    slot.with_host(|host| host.update_occlusion());
    pump(key);

    Ok(key)
}

/// Current state of a session, if it exists.
pub fn state(key: SessionKey) -> Option<TerminalState> {
    main_thread().ok()?;
    slot(key)?.with_host_ref(Host::state)
}

/// Whether the session's process is still running.
pub fn is_running(key: SessionKey) -> bool {
    slot(key)
        .and_then(|slot| slot.with_host_ref(Host::is_running))
        .unwrap_or(false)
}

/// Update a session's terminal rectangle and visibility.
///
/// Hidden or empty geometry moves keyboard focus to the web view and hides the view without
/// resizing the PTY. Ordering of reports is the embedder's responsibility.
pub fn set_viewport(key: SessionKey, rect: ViewportRect, visible: bool) -> Result<(), String> {
    main_thread()?;

    let rect = rect.validated()?;

    let slot = slot(key).ok_or_else(|| "terminal session does not exist".to_owned())?;
    let change = slot
        .with_host(|host| host.set_viewport(rect, visible))
        .ok_or_else(|| "terminal session is not available".to_owned())?;

    match change {
        ViewportChange::Shown(frame) => {
            if let Some(embedder) = embedder() {
                embedder.last_frame.set(Some(frame));
            }
        }
        ViewportChange::Hidden => (),
        ViewportChange::Hide(request) => finish_hide(&slot, request),
    }

    pump(key);

    Ok(())
}

/// Hide a session's view immediately, releasing first responder first.
///
/// Used on presentation changes, before any geometry report of the new presentation.
pub fn hide(key: SessionKey) {
    if main_thread().is_err() {
        return;
    }
    let slot = match slot(key) {
        Some(slot) => slot,
        None => return,
    };

    if let Some(Some(request)) = slot.with_host(Host::begin_hide) {
        finish_hide(&slot, request);
        pump(key);
    }
}

/// Move first responder away from a visible view, then hide it.
fn finish_hide(slot: &Slot, request: HideRequest) {
    // Outside the host borrow: focus changes call back into both views, and the resulting
    // `Focused(false)` is processed (and reported) right away.
    native::release_focus(&request.view, &request.web_view);

    // The session may have been removed or received a newer report meanwhile.
    slot.with_host(|host| host.hide(request.generation));
}

/// Whether the session's view is currently shown with a nonzero area.
pub fn is_shown(key: SessionKey) -> bool {
    slot(key)
        .and_then(|slot| {
            slot.with_host_ref(|host| !host.view_hidden && native::view_is_shown(&host.view))
        })
        .unwrap_or(false)
}

/// Give keyboard focus to a session's terminal view.
///
/// Refused (`false`) while the view is hidden or has no area, so a stale focus request cannot
/// make an invisible terminal the first responder.
pub fn focus(key: SessionKey) -> bool {
    if main_thread().is_err() || !is_shown(key) {
        return false;
    }

    let view = match slot(key).and_then(|slot| slot.with_host_ref(|host| host.view.clone())) {
        Some(view) => view,
        None => return false,
    };

    // Called outside the host borrow: focus changes call back into the view.
    let _batch = EventBatch::new(key);
    native::take_focus(&view)
}

/// Start stopping a session's process; its display and final content stay.
///
/// Returns immediately. The PTY is stopped (bounded, reap-safe) on a helper thread, after
/// which the session reports `running: false`. Harmless for exited or already stopping
/// sessions.
pub fn stop(key: SessionKey) -> Result<(), String> {
    main_thread()?;
    let slot = slot(key).ok_or_else(|| "terminal session does not exist".to_owned())?;

    let unconfirmed = slot
        .with_host(|host| {
            if !host.is_running() {
                return false;
            }
            let window = match host.processor.window_context_mut() {
                Some(window) => window,
                None => return false,
            };
            if window.shutdown_started() {
                return false;
            }

            host.stopping = window.begin_shutdown(move || wake_stopped(key));
            // Without a helper there is nothing to wait for: the PTY thread received its
            // shutdown request and drops the PTY itself.
            host.stopping.is_none()
        })
        .ok_or_else(|| "terminal session is not available".to_owned())?;

    if unconfirmed {
        log::warn!("Session {key} stop cannot be awaited; reporting it as stopped");
        slot.with_host(Host::finish_stop);
        pump(key);
    }

    Ok(())
}

/// Called from a PTY shutdown helper thread once the session's PTY is gone.
fn wake_stopped(key: SessionKey) {
    native::defer_from_any_thread(Deferred::Stopped, key);
}

fn stopped_on_main(key: SessionKey) {
    let slot = match slot(key) {
        Some(slot) => slot,
        None => return,
    };

    match slot.host.try_borrow_mut() {
        Ok(mut host) => match host.as_mut() {
            Some(host) => host.finish_stop(),
            None => return,
        },
        Err(_) => {
            // Busy: retry after the current callback.
            native::defer(Deferred::Stopped, key);
            return;
        }
    }

    drop(slot);
    pump(key);
}

/// Remove an exited session: release its GL resources, terminal, PTY and view.
///
/// Late callbacks for the session are ignored afterwards.
pub fn remove(key: SessionKey) -> Result<(), String> {
    main_thread()?;
    let slot = slot(key).ok_or_else(|| "terminal session does not exist".to_owned())?;

    let running = slot
        .with_host_ref(Host::is_running)
        .ok_or_else(|| "terminal session is not available".to_owned())?;
    if running {
        return Err("terminal session is still running".into());
    }

    hide(key);

    let host = slot
        .host
        .try_borrow_mut()
        .map_err(|_| "terminal session is not available".to_owned())?
        .take();
    SLOTS.with(|slots| slots.borrow_mut().remove(&key));
    slot.pending.borrow_mut().clear();

    if let Some(mut host) = host {
        host.proxy.close();
        if let Some(done) = host.stopping.take() {
            window_context::wait_for_shutdown(&done, WindowContext::SHUTDOWN_TIMEOUT);
        }
        release(host);
    }

    Ok(())
}

/// Shut every session down: stop all child processes concurrently within one bounded wait,
/// then release GL resources and remove the views.
pub fn shutdown_all() {
    if main_thread().is_err() {
        log::error!("terminal shutdown called off the main thread");
        return;
    }

    let slots: Vec<Rc<Slot>> =
        SLOTS.with(|slots| slots.borrow_mut().drain().map(|(_, slot)| slot).collect());

    let mut hosts = Vec::with_capacity(slots.len());
    for slot in slots {
        slot.pending.borrow_mut().clear();
        match slot.host.try_borrow_mut() {
            Ok(mut host) => hosts.extend(host.take()),
            Err(_) => log::error!("Session {} is busy during shutdown", slot.key),
        }
    }

    let mut waits = Vec::new();
    for host in &mut hosts {
        host.proxy.close();
        if let Some(window) = host.processor.window_context_mut() {
            waits.extend(window.begin_shutdown(|| ()));
        }
        waits.extend(host.stopping.take());
    }

    let deadline = Instant::now() + WindowContext::SHUTDOWN_TIMEOUT;
    for done in &waits {
        window_context::wait_for_shutdown(done, deadline.saturating_duration_since(Instant::now()));
    }

    for host in hosts {
        release(host);
    }
}

/// Drop a detached host: GL context/surface before the view leaves the hierarchy.
fn release(mut host: Host) {
    host.processor.exiting();
    native::detach_view(&host.view);

    // A caller may run inside one of the view's native callbacks; keep the view alive until
    // that callback has returned.
    native::release_view_later(host.view.clone());
}

impl Host {
    fn state(&self) -> TerminalState {
        let window = self.processor.window_context();
        let (rows, cols) = window.map_or((0, 0), |window| window.grid_size());
        TerminalState {
            title: window
                .map(|window| window.display.window.title().to_owned())
                .unwrap_or_default(),
            shell: self.shell.clone(),
            pid: window.map(|window| window.shell_pid()),
            running: self.is_running(),
            exit_code: window.and_then(|window| window.exit_code()),
            rows,
            cols,
            focused: window.map_or(false, |window| window.is_focused()),
        }
    }

    fn is_running(&self) -> bool {
        self.processor
            .window_context()
            .map_or(false, |window| window.is_running())
    }

    /// Record a finished stop and show the final content.
    fn finish_stop(&mut self) {
        self.stopping = None;
        if let Some(window) = self.processor.window_context_mut() {
            window.finish_shutdown();
            window.dirty = true;
        }
        self.request_redraw();
    }

    /// Apply a viewport report.
    ///
    /// Hiding a visible view is left to [`Self::hide`], after the caller moved first responder
    /// away from it without holding the host borrow.
    fn set_viewport(&mut self, rect: ViewportRect, visible: bool) -> ViewportChange {
        self.viewport_generation += 1;

        let frame = if visible {
            self.native_frame(rect)
        } else {
            None
        };

        let change = match frame {
            Some(frame) => {
                // While hidden, the view defers the resulting resize until it is shown.
                native::place_view(&self.view, frame);
                // Radius-only reports must repaint too; no resize event is generated.
                if let Some(window) = self.processor.window_context_mut() {
                    window.display.mark_surface_dirty();
                    // Radius shrinking/removal must also replace the old transparent area.
                    window.display.damage_tracker.frame().mark_fully_damaged();
                    window.dirty = true;
                }
                self.pending.borrow_mut().push_back(Pending::Redraw);
                if self.view_hidden {
                    self.view_hidden = false;
                    // Reports geometry that changed while hidden.
                    native::set_view_hidden(&self.view, false);

                    if let Some(window) = self.processor.window_context_mut() {
                        window.display.mark_surface_dirty();
                    }
                    self.pending.borrow_mut().push_back(Pending::Redraw);
                }
                ViewportChange::Shown(frame)
            }
            None if !self.view_hidden => {
                return ViewportChange::Hide(HideRequest {
                    view: self.view.clone(),
                    web_view: self.web_view.clone(),
                    generation: self.viewport_generation,
                });
            }
            None => ViewportChange::Hidden,
        };

        self.update_occlusion();

        change
    }

    /// Start hiding a visible view outside of any viewport report.
    fn begin_hide(&mut self) -> Option<HideRequest> {
        if self.view_hidden {
            return None;
        }
        self.viewport_generation += 1;
        Some(HideRequest {
            view: self.view.clone(),
            web_view: self.web_view.clone(),
            generation: self.viewport_generation,
        })
    }

    /// Hide the view for the change `generation`, unless a newer change was applied since.
    fn hide(&mut self, generation: u64) {
        if self.viewport_generation != generation || self.view_hidden {
            return;
        }

        self.view_hidden = true;
        native::set_view_hidden(&self.view, true);
        self.update_occlusion();
    }

    /// Convert a web view rectangle into the view's native frame.
    ///
    /// Returns `None` if nothing of the rectangle is visible.
    fn native_frame(&self, rect: ViewportRect) -> Option<Frame> {
        self.web_view.native_frame(&self.view, rect)
    }

    /// Forward the combined window/view occlusion to the processor.
    fn update_occlusion(&mut self) {
        let occluded = self.window_occluded || self.view_hidden;
        self.processor.window_event(WindowEvent::Occluded(occluded));
        if !occluded {
            self.request_redraw();
        }
    }

    fn request_redraw(&mut self) {
        if let Some(window) = self.processor.window_context_mut() {
            window.display.window.request_redraw();
        }
    }

    fn requested_redraw(&self) -> bool {
        self.processor
            .window_context()
            .map_or(false, |window| window.display.window.requested_redraw)
    }

    fn pop_pending(&self) -> Option<Pending> {
        self.pending.borrow_mut().pop_front()
    }

    fn handle_pending(&mut self, pending: Pending) {
        match pending {
            Pending::Window(WindowEvent::Occluded(occluded)) => {
                self.window_occluded = occluded;
                self.update_occlusion();
            }
            Pending::Window(event) => self.processor.window_event(event),
            #[cfg(target_os = "macos")]
            Pending::Action(ViewAction::Copy) => self.processor.perform_action(Action::Copy),
            #[cfg(target_os = "macos")]
            Pending::Action(ViewAction::Paste) => {
                // Mirror Alacritty's default Cmd+V bindings: paste is disabled in Vi mode
                // unless the search bar is active.
                let blocked = self.processor.window_context().map_or(true, |window| {
                    window.mode().contains(TermMode::VI) && !window.search_active()
                });
                if !blocked {
                    self.processor.perform_action(Action::Paste);
                }
            }
            Pending::Redraw => self.request_redraw(),
        }
    }

    /// Run one event-loop iteration and return the next timer deadline.
    fn run_iteration(&mut self) -> Option<Instant> {
        let mut deadline;

        // Bounded so a continuously producing PTY cannot starve the main loop; remaining events
        // are picked up by the already scheduled wakeup.
        let mut rounds = 0;
        loop {
            while let Some(pending) = self.pop_pending() {
                self.handle_pending(pending);
            }

            let user_events = self.proxy.drain();
            let had_user_events = !user_events.is_empty();
            for event in user_events {
                self.processor.user_event(event);
            }

            deadline = self.processor.about_to_wait();

            rounds += 1;
            let pending_left = !self.pending.borrow().is_empty();
            if (!had_user_events && !pending_left) || rounds >= 4 {
                break;
            }
        }

        if self.requested_redraw() {
            self.processor.window_event(WindowEvent::RedrawRequested);
            deadline = self.processor.about_to_wait();
        }

        deadline
    }
}

/// Process all queued work of one session on the main thread.
fn pump(key: SessionKey) {
    let slot = match slot(key) {
        Some(slot) => slot,
        None => return,
    };

    // Re-entrant calls (native code calling back into the view while the host is busy) leave
    // their events queued for the running pump.
    if slot.in_pump.replace(true) {
        return;
    }

    let result = slot.with_host(|host| {
        let deadline = host.run_iteration();
        let state = host.state();
        let notify = (host.last_state.as_ref() != Some(&state)).then(|| {
            host.last_state = Some(state.clone());
            state
        });
        // embed: Transfer the FIFO out of the host borrow before delivery.
        let events = host.processor.window_context_mut()
            .map(|window| window.take_host_events()).unwrap_or_default();
        (deadline, notify, events)
    });


    let (deadline, notify, events) = match result {
        Some(result) => result,
        None => {
            // Removed (or being removed): nothing will consume queued events.
            if slot.host.try_borrow().map_or(false, |host| host.is_none()) {
                slot.pending.borrow_mut().clear();
            }
            slot.in_pump.set(false);
            return;
        }
    };

    arm_timer(&slot, deadline);

    // embed: Keep in_pump true through callbacks: reentry queues work, never interleaves
    // newer events into this FIFO. No host/registry borrow is held during delivery.
    if let Some(embedder) = embedder() {
        if let Some(state) = notify {
            (embedder.listener)(key, HostEvent::State(&state));
        }
        for event in events {
            // A callback may remove the session; do not deliver stale queued events.
            if slot.with_host_ref(|_| ()).is_none() { break; }
            match event {
                window_context::PendingHostEvent::Bell =>
                    (embedder.listener)(key, HostEvent::Bell),
                window_context::PendingHostEvent::OpenFile(uri) =>
                    (embedder.listener)(key, HostEvent::OpenFile(&uri)),
            }
        }
    }
    slot.in_pump.set(false);

    // Events queued while the host was busy but after it finished draining, or a redraw
    // requested while drawing.
    let redraw_left = slot.with_host_ref(Host::requested_redraw).unwrap_or(false);
    if redraw_left || !slot.pending.borrow().is_empty() {
        request_slot_pump(&slot);
    }
}

/// Queue an event from a session's terminal view.
pub(crate) fn queue_window_event(key: SessionKey, event: WindowEvent) {
    queue(key, Pending::Window(event));
}

/// Queue an Edit menu action from a session's terminal view.
#[cfg(target_os = "macos")]
pub(crate) fn queue_view_action(key: SessionKey, action: ViewAction) {
    queue(key, Pending::Action(action));
}

fn queue(key: SessionKey, pending: Pending) {
    let slot = match slot(key) {
        Some(slot) => slot,
        None => return,
    };
    slot.pending.borrow_mut().push_back(pending);
    if slot.batch_depth.get() == 0 {
        drop(slot);
        pump(key);
    }
}

/// Request a frame, e.g. because the platform asked the view to display.
pub(crate) fn request_redraw(key: SessionKey) {
    if let Some(slot) = slot(key) {
        slot.pending.borrow_mut().push_back(Pending::Redraw);
        request_slot_pump(&slot);
    }
}

/// Make sure a pump of the session runs soon on the main thread.
pub(crate) fn request_pump(key: SessionKey) {
    if let Some(slot) = slot(key) {
        request_slot_pump(&slot);
    }
}

fn request_slot_pump(slot: &Slot) {
    // A running pump checks for redraw requests and pending events before it returns.
    if slot.in_pump.get() || slot.batch_depth.get() > 0 {
        return;
    }

    if !slot.async_pump_scheduled.replace(true) {
        native::defer(Deferred::Pump, slot.key);
    }
}

fn async_pump(key: SessionKey) {
    if let Some(slot) = slot(key) {
        slot.async_pump_scheduled.set(false);
    }
    pump(key);
}

/// Proxy wakeup of one session, called from its PTY thread or the main thread.
fn wake_from_any_thread(key: SessionKey) {
    native::defer_from_any_thread(Deferred::Wakeup, key);
}

/// Main-loop entry of work handed out through `crate::platform::embedder::defer`.
pub(crate) fn run_deferred(kind: Deferred, key: SessionKey) {
    match kind {
        Deferred::Pump => async_pump(key),
        Deferred::Wakeup => pump(key),
        Deferred::Stopped => stopped_on_main(key),
    }
}

/// Arm a main-loop timer for the session's earliest scheduler deadline.
fn arm_timer(slot: &Slot, deadline: Option<Instant>) {
    let deadline = match deadline {
        Some(deadline) => deadline,
        None => return,
    };

    // An earlier (or equal) timer is already pending; it will re-arm for later deadlines.
    if slot
        .timer_deadline
        .get()
        .is_some_and(|armed| armed <= deadline)
    {
        return;
    }

    let generation = slot.timer_generation.get().wrapping_add(1);
    slot.timer_generation.set(generation);
    slot.timer_deadline.set(Some(deadline));

    let delay = deadline.saturating_duration_since(Instant::now());
    native::defer_after(delay, slot.key, generation);
}

/// A timer armed by [`arm_timer`] fired.
pub(crate) fn timer_fired(key: SessionKey, generation: usize) {
    let slot = match slot(key) {
        Some(slot) => slot,
        None => return,
    };

    // Superseded by an earlier timer, which already re-armed as needed.
    if slot.timer_generation.get() != generation {
        return;
    }

    slot.timer_deadline.set(None);
    drop(slot);
    pump(key);
}

/// Groups events queued by one native callback of a session's view so they are processed
/// together.
pub(crate) struct EventBatch {
    slot: Option<Rc<Slot>>,
}

impl EventBatch {
    pub(crate) fn new(key: SessionKey) -> Self {
        let slot = slot(key);
        if let Some(slot) = &slot {
            slot.batch_depth.set(slot.batch_depth.get() + 1);
        }
        Self { slot }
    }
}

impl Drop for EventBatch {
    fn drop(&mut self) {
        let slot = match self.slot.take() {
            Some(slot) => slot,
            None => return,
        };

        let depth = slot.batch_depth.get() - 1;
        slot.batch_depth.set(depth);

        if depth == 0 && !slot.pending.borrow().is_empty() {
            let key = slot.key;
            drop(slot);
            pump(key);
        }
    }
}

/// The user's login shell, resolved like `alacritty_terminal`'s PTY spawn: `$SHELL`, then
/// passwd.
pub fn login_shell() -> String {
    if let Ok(shell) = std::env::var("SHELL") {
        return shell;
    }

    let mut buf = [0 as libc::c_char; 1024];
    let mut entry: libc::passwd = unsafe { std::mem::zeroed() };
    let mut result: *mut libc::passwd = std::ptr::null_mut();
    let uid = unsafe { libc::getuid() };
    let status =
        unsafe { libc::getpwuid_r(uid, &mut entry, buf.as_mut_ptr(), buf.len(), &mut result) };
    if status == 0 && !result.is_null() && !entry.pw_shell.is_null() {
        return unsafe { CStr::from_ptr(entry.pw_shell) }
            .to_string_lossy()
            .into_owned();
    }

    String::new()
}

/// The user's home directory from passwd (like the PTY's `HOME`), else `$HOME`.
pub fn home_directory() -> Option<PathBuf> {
    let mut buf = [0 as libc::c_char; 1024];
    let mut entry: libc::passwd = unsafe { std::mem::zeroed() };
    let mut result: *mut libc::passwd = std::ptr::null_mut();
    let uid = unsafe { libc::getuid() };
    let status =
        unsafe { libc::getpwuid_r(uid, &mut entry, buf.as_mut_ptr(), buf.len(), &mut result) };
    if status == 0 && !result.is_null() && !entry.pw_dir.is_null() {
        let dir = unsafe { CStr::from_ptr(entry.pw_dir) }
            .to_string_lossy()
            .into_owned();
        if !dir.is_empty() {
            return Some(PathBuf::from(dir));
        }
    }

    std::env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
}

#[cfg(test)]
mod viewport_tests {
    use super::ViewportRect;

    #[test]
    fn validates_and_clamps_corners_independently() {
        let rect = ViewportRect {
            x: -1., y: 0., width: 100., height: 12.,
            top_left_radius: 0., bottom_left_radius: 10.,
        };
        let valid = rect.validated().unwrap();
        assert_eq!(valid.top_left_radius, 0.);
        assert_eq!(valid.bottom_left_radius, 6.);
        assert!(ViewportRect { top_left_radius: -1., ..rect }.validated().is_err());
        assert!(ViewportRect { bottom_left_radius: f64::NAN, ..rect }.validated().is_err());
        assert!(ViewportRect { top_left_radius: f64::INFINITY, ..rect }.validated().is_err());
        assert_eq!(ViewportRect { height: 0., ..rect }.validated().unwrap().bottom_left_radius, 0.);
    }
}
