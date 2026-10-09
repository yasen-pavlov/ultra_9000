//! Terminal window context.
//!
//! Modified copy of alacritty/src/window_context.rs at
//! def47a5dd511ed20607eb5d9e5e7140afd16b31d.
//!
//! Changes for the embedded view host (search for "embed:"):
//! - `new_embedded` replaces `initial`/`additional`: the GL platform is initialized for the
//!   embedder's `TerminalView` exactly like upstream's non-Windows path (display and config
//!   without a native window handle: CGL on macOS, EGL on the Wayland display on Linux), no
//!   window is created.
//! - No CLI/IPC options or ref-test output; embed: supported live config reload is retained.
//! - The PTY I/O thread handle and child exit state are retained so the embedder can report
//!   the exited process and shut the session down deterministically. The main thread never
//!   signals the child: stopping it is left to `alacritty_terminal`'s `Pty` drop, the only
//!   place that knows whether the child's PID is still unreaped (see `shutdown`).
//! - `perform_action` runs a single binding action for AppKit responder actions (macOS).
//! - The embedder supplies the PTY options (program, arguments, working directory) of each
//!   session. Stopping a session's PTY (`begin_shutdown`) is separate from dropping its
//!   display, so a stopped session keeps its final terminal content until it is removed.
//! - embed: Removed unreachable touch state and standalone message buffers/plumbing.

use std::error::Error;
use std::collections::VecDeque;
#[cfg(not(windows))]
use std::os::unix::io::{AsRawFd, RawFd};
use std::rc::Rc;
use std::sync::mpsc;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use log::{info, warn};
#[cfg(target_os = "macos")]
use winit::raw_window_handle::{AppKitDisplayHandle, RawDisplayHandle};

use alacritty_terminal::event::Event as TerminalEvent;
use alacritty_terminal::event_loop::{EventLoop as PtyEventLoop, Msg, Notifier, State};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::Direction;
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::{Term, TermMode};
use alacritty_terminal::tty::{self, Pty};

use crate::clipboard::Clipboard;
#[cfg(target_os = "macos")]
use crate::config::Action;
use crate::config::UiConfig;
use crate::display::window::Window;
use crate::display::Display;
use crate::event::{ActionContext, Event, EventProxy, InlineSearchState, Mouse, SearchState};
#[cfg(target_os = "macos")]
use crate::host::ActiveEventLoop;
use crate::host::{EventLoopProxy, Modifiers, WindowEvent, WindowId, WinitEvent};
use crate::platform::embedder::View;
use crate::scheduler::Scheduler;
use crate::{input, renderer};

/// embed: Extra time a full shutdown waits, beyond `tty::TERMINATE_TIMEOUT`, for the PTY
/// thread to stop and drop its `Pty`.
const SHUTDOWN_MARGIN: Duration = Duration::from_millis(500);

type PtyThread = JoinHandle<(PtyEventLoop<Pty, EventProxy>, State)>;


// embed: Discrete host events, FIFO bounded independently of terminal-state snapshots.
pub(crate) enum PendingHostEvent { Notify(String), Bell }
const HOST_EVENT_LIMIT: usize = 128;
/// Event context for one individual Alacritty window.
pub struct WindowContext {
    pub display: Display,
    pub dirty: bool,
    event_queue: Vec<WinitEvent<Event>>,
    terminal: Arc<FairMutex<Term<EventProxy>>>,
    cursor_blink_timed_out: bool,
    modifiers: Modifiers,
    inline_search_state: InlineSearchState,
    search_state: SearchState,
    notifier: Notifier,
    mouse: Mouse,
    occluded: bool,
    preserve_title: bool,
    #[cfg(not(windows))]
    master_fd: RawFd,
    #[cfg(not(windows))]
    shell_pid: u32,
    config: Rc<UiConfig>,

    /// embed: PTY I/O thread, joined on shutdown.
    io_thread: Option<PtyThread>,
    /// embed: Child exit status, once the child exited.
    child_exit: Option<Option<i32>>,
    // embed: At most 128 events; each notification is at most 16 KiB.
    host_events: VecDeque<PendingHostEvent>,
}

impl WindowContext {
    /// Create the terminal inside the embedder's view.
    ///
    /// embed: Follows upstream `initial` for macOS and Linux: the CGL/EGL display and config
    /// are created without a native window handle, the context is created for the view's
    /// display (the embedder's Wayland display on Linux).
    pub fn new_embedded(
        view: View,
        proxy: EventLoopProxy<Event>,
        config: Rc<UiConfig>,
        pty_config: tty::Options,
    ) -> Result<Self, Box<dyn Error>> {
        #[cfg(target_os = "macos")]
        let raw_display_handle = RawDisplayHandle::AppKit(AppKitDisplayHandle::new());
        #[cfg(target_os = "linux")]
        let raw_display_handle = view.raw_display_handle();
        let raw_window_handle = None;

        let gl_display = renderer::platform::create_gl_display(
            raw_display_handle,
            raw_window_handle,
            config.debug.prefer_egl,
        )?;
        let gl_config = renderer::platform::pick_gl_config(&gl_display, raw_window_handle)?;

        let window = Window::new(view, &config);

        // Create context.
        let gl_context =
            renderer::platform::create_gl_context(&gl_display, &gl_config, raw_window_handle)?;

        let display = Display::new(window, gl_context, &config, false)?;

        Self::new(display, config, proxy, pty_config)
    }

    /// Create a new terminal window context.
    fn new(
        display: Display,
        config: Rc<UiConfig>,
        proxy: EventLoopProxy<Event>,
        pty_config: tty::Options,
    ) -> Result<Self, Box<dyn Error>> {

        let preserve_title = false;

        info!(
            "PTY dimensions: {:?} x {:?}",
            display.size_info.screen_lines(),
            display.size_info.columns()
        );

        let event_proxy = EventProxy::new(proxy, display.window.id());

        // Create the terminal.
        //
        // This object contains all of the state about what's being displayed. It's
        // wrapped in a clonable mutex since both the I/O loop and display need to
        // access it.
        let terminal = Term::new(config.term_options(), &display.size_info, event_proxy.clone());
        let terminal = Arc::new(FairMutex::new(terminal));

        // Create the PTY.
        //
        // The PTY forks a process to run the shell on the slave side of the
        // pseudoterminal. A file descriptor for the master side is retained for
        // reading/writing to the shell.
        let pty = tty::new(&pty_config, display.size_info.into(), display.window.id().into())?;

        #[cfg(not(windows))]
        let master_fd = pty.file().as_raw_fd();
        #[cfg(not(windows))]
        let shell_pid = pty.child().id();

        // Create the pseudoterminal I/O loop.
        //
        // PTY I/O is ran on another thread as to not occupy cycles used by the
        // renderer and input processing. Note that access to the terminal state is
        // synchronized since the I/O loop updates the state, and the display
        // consumes it periodically.
        let event_loop = PtyEventLoop::new(
            Arc::clone(&terminal),
            event_proxy.clone(),
            pty,
            pty_config.drain_on_exit,
            config.debug.ref_test,
        )?;

        // The event loop channel allows write requests from the event processor
        // to be sent to the pty loop and ultimately written to the pty.
        let loop_tx = event_loop.channel();

        // Kick off the I/O thread.
        let io_thread = event_loop.spawn();

        // Start cursor blinking, in case `Focused` isn't sent on startup.
        if config.cursor.style().blinking {
            event_proxy.send_event(TerminalEvent::CursorBlinkingChange.into());
        }

        // Create context for the Alacritty window.
        Ok(WindowContext {
            preserve_title,
            terminal,
            display,
            #[cfg(not(windows))]
            master_fd,
            #[cfg(not(windows))]
            shell_pid,
            config,
            notifier: Notifier(loop_tx),
            cursor_blink_timed_out: Default::default(),
            inline_search_state: Default::default(),
            search_state: Default::default(),
            event_queue: Default::default(),
            modifiers: Default::default(),
            occluded: Default::default(),
            mouse: Default::default(),
            dirty: Default::default(),
            io_thread: Some(io_thread),
            child_exit: None,
            host_events: VecDeque::new(),
        })
    }

    /// embed: Re-port def47a5 live reload, omitting host-owned window/IPC policies.
    pub fn update_config(&mut self, config: Rc<UiConfig>) {
        let old = std::mem::replace(&mut self.config, config);
        self.display.update_config(&self.config);
        self.terminal.lock().set_options(self.config.term_options());
        if (old.cursor.thickness() - self.config.cursor.thickness()).abs() > f32::EPSILON {
            self.display.pending_update.set_cursor_dirty();
        }
        if old.font != self.config.font {
            let scale = self.display.window.scale_factor as f32;
            // Preserve a runtime zoom, just like upstream.
            if self.display.font_size == old.font.size().scale(scale) {
                self.display.font_size = self.config.font.size().scale(scale);
            }
            self.display.pending_update.set_font(
                self.config.font.clone().with_size(self.display.font_size),
            );
        }
        if old.window.padding(1.) != self.config.window.padding(1.)
            || old.window.dynamic_padding != self.config.window.dynamic_padding
        {
            self.display.pending_update.dirty = true;
        }
        if !self.preserve_title && (!self.config.window.dynamic_title
            || self.display.window.title() == old.window.identity.title)
        {
            self.display.window.set_title(self.config.window.identity.title.clone());
        }
        #[cfg(target_os = "macos")]
        self.display.window.view().set_option_as_alt(self.config.window.option_as_alt());
        self.display.hint_state.update_alphabet(self.config.hints.alphabet());
        let event = Event::new(TerminalEvent::CursorBlinkingChange.into(), Some(self.id()));
        self.event_queue.push(event.into());
        self.dirty = true;
        self.display.window.request_redraw();
    }

    /// embed: Drop oldest on overflow; BEL retains its normal Alacritty visual handling.
    pub(crate) fn push_host_event(&mut self, event: PendingHostEvent) {
        if self.host_events.len() == HOST_EVENT_LIMIT { self.host_events.pop_front(); }
        self.host_events.push_back(event);
    }

    pub(crate) fn take_host_events(&mut self) -> VecDeque<PendingHostEvent> {
        std::mem::take(&mut self.host_events)
    }

    /// Draw the window.
    pub fn draw(&mut self, scheduler: &mut Scheduler) {
        self.display.window.requested_redraw = false;

        if self.occluded {
            return;
        }

        self.dirty = false;

        // Force the display to process any pending display update.
        self.display.process_renderer_update();

        // Request immediate re-draw if visual bell animation is not finished yet.
        if !self.display.visual_bell.completed() {
            // We can get an OS redraw which bypasses alacritty's frame throttling, thus
            // marking the window as dirty when we don't have frame yet.
            if self.display.window.has_frame {
                self.display.window.request_redraw();
            } else {
                self.dirty = true;
            }
        }

        // Redraw the window.
        let terminal = self.terminal.lock();
        self.display.draw(
            terminal,
            scheduler,
            &self.config,
            &mut self.search_state,
        );
    }

    /// Process events for this terminal window.
    pub fn handle_event(
        &mut self,
        #[cfg(target_os = "macos")] event_loop: &ActiveEventLoop,
        event_proxy: &EventLoopProxy<Event>,
        clipboard: &mut Clipboard,
        scheduler: &mut Scheduler,
        event: WinitEvent<Event>,
    ) {
        match event {
            WinitEvent::AboutToWait
            | WinitEvent::WindowEvent { event: WindowEvent::RedrawRequested } => {
                // Skip further event handling with no staged updates.
                if self.event_queue.is_empty() {
                    return;
                }

                // Continue to process all pending events.
            },
            event => {
                self.event_queue.push(event);
                return;
            },
        }

        let mut terminal = self.terminal.lock();

        let old_is_searching = self.search_state.history_index.is_some();

        let context = ActionContext {
            cursor_blink_timed_out: &mut self.cursor_blink_timed_out,
            inline_search_state: &mut self.inline_search_state,
            search_state: &mut self.search_state,
            modifiers: &mut self.modifiers,
            notifier: &mut self.notifier,
            display: &mut self.display,
            mouse: &mut self.mouse,
            dirty: &mut self.dirty,
            occluded: &mut self.occluded,
            terminal: &mut terminal,
            #[cfg(not(windows))]
            master_fd: self.master_fd,
            #[cfg(not(windows))]
            shell_pid: self.shell_pid,
            preserve_title: self.preserve_title,
            config: &self.config,
            event_proxy,
            #[cfg(target_os = "macos")]
            event_loop,
            clipboard,
            scheduler,
        };
        let mut processor = input::Processor::new(context);

        for event in self.event_queue.drain(..) {
            processor.handle_event(event);
        }

        // Process DisplayUpdate events.
        if self.display.pending_update.dirty {
            Self::submit_display_update(
                &mut terminal,
                &mut self.display,
                &mut self.notifier,
                &mut self.search_state,
                old_is_searching,
                &self.config,
            );
            self.dirty = true;
        }

        if self.dirty || self.mouse.hint_highlight_dirty {
            self.dirty |= self.display.update_highlighted_hints(
                &terminal,
                &self.config,
                &self.mouse,
                self.modifiers.state(),
            );
            self.mouse.hint_highlight_dirty = false;
        }

        // Don't call `request_redraw` when event is `RedrawRequested` since the `dirty` flag
        // represents the current frame, but redraw is for the next frame.
        if self.dirty
            && self.display.window.has_frame
            && !self.occluded
            && !matches!(event, WinitEvent::WindowEvent { event: WindowEvent::RedrawRequested })
        {
            self.display.window.request_redraw();
        }
    }

    /// embed: Run one binding action, for AppKit responder actions like Edit > Copy.
    ///
    /// The action runs after any staged events, like upstream's binding processing.
    #[cfg(target_os = "macos")]
    pub fn perform_action(
        &mut self,
        #[cfg(target_os = "macos")] event_loop: &ActiveEventLoop,
        event_proxy: &EventLoopProxy<Event>,
        clipboard: &mut Clipboard,
        scheduler: &mut Scheduler,
        action: Action,
    ) {
        self.handle_event(event_loop, event_proxy, clipboard, scheduler, WinitEvent::AboutToWait);

        let mut terminal = self.terminal.lock();
        let context = ActionContext {
            cursor_blink_timed_out: &mut self.cursor_blink_timed_out,
            inline_search_state: &mut self.inline_search_state,
            search_state: &mut self.search_state,
            modifiers: &mut self.modifiers,
            notifier: &mut self.notifier,
            display: &mut self.display,
            mouse: &mut self.mouse,
            dirty: &mut self.dirty,
            occluded: &mut self.occluded,
            terminal: &mut terminal,
            #[cfg(not(windows))]
            master_fd: self.master_fd,
            #[cfg(not(windows))]
            shell_pid: self.shell_pid,
            preserve_title: self.preserve_title,
            config: &self.config,
            event_proxy,
            #[cfg(target_os = "macos")]
            event_loop,
            clipboard,
            scheduler,
        };
        input::Processor::new(context).execute_action(&action);
        drop(terminal);

        if self.dirty && self.display.window.has_frame && !self.occluded {
            self.display.window.request_redraw();
        }
    }

    /// ID of this terminal context.
    pub fn id(&self) -> WindowId {
        self.display.window.id()
    }

    /// embed: PID of the child process spawned on the PTY.
    pub fn shell_pid(&self) -> u32 {
        self.shell_pid
    }

    /// embed: Whether the child process is still running.
    pub fn is_running(&self) -> bool {
        self.child_exit.is_none()
    }

    /// embed: Normal exit status only; signalled or explicitly stopped children have none.
    pub fn exit_code(&self) -> Option<i32> { self.child_exit.flatten() }

    /// embed: Query only while the PTY thread owns the live master descriptor.
    pub fn foreground_process_group(&self) -> Option<i32> {
        if !self.is_running() || self.shutdown_started() { return None; }
        let group = unsafe { libc::tcgetpgrp(self.master_fd) };
        (group > 0 && group != self.shell_pid as i32).then_some(group)
    }

    /// embed: Current terminal mode (for the macOS Edit menu paste).
    #[cfg(target_os = "macos")]
    pub fn mode(&self) -> TermMode {
        *self.terminal.lock().mode()
    }

    /// embed: Whether the search bar is active (for the macOS Edit menu paste).
    #[cfg(target_os = "macos")]
    pub fn search_active(&self) -> bool {
        self.search_state.history_index.is_some()
    }

    /// embed: Whether the terminal currently has keyboard focus.
    pub fn is_focused(&self) -> bool {
        self.terminal.lock().is_focused
    }

    /// embed: Current terminal grid size as `(rows, columns)`.
    pub fn grid_size(&self) -> (usize, usize) {
        (self.display.size_info.screen_lines(), self.display.size_info.columns())
    }

    /// embed: Record the child's exit status.
    pub fn on_child_exit(&mut self, status: i32) {
        info!("Child process {} exited with status {status}", self.shell_pid);
        self.child_exit = Some(Some(status));
    }

    /// embed: The PTY event loop finished; the child has exited even if no status arrived.
    pub fn on_terminal_exit(&mut self) {
        if self.child_exit.is_none() {
            self.child_exit = Some(None);
        }
        self.dirty = true;
        if self.display.window.has_frame {
            self.display.window.request_redraw();
        }
    }

    /// embed: Longest time a full shutdown waits for a PTY thread to stop and drop its `Pty`.
    pub const SHUTDOWN_TIMEOUT: Duration = tty::TERMINATE_TIMEOUT.saturating_add(SHUTDOWN_MARGIN);

    /// embed: Stop the PTY thread and the managed child process, waiting at most
    /// [`Self::SHUTDOWN_TIMEOUT`].
    pub fn shutdown(&mut self) {
        if let Some(done) = self.begin_shutdown(|| ()) {
            wait_for_shutdown(&done, Self::SHUTDOWN_TIMEOUT);
        }
        self.finish_shutdown();
    }

    /// embed: Start stopping the PTY thread and the managed child process without waiting.
    ///
    /// The PTY thread stops and returns its event loop; dropping it drops the `Pty`, which
    /// stops the terminal's foreground jobs and the child's process group within
    /// `tty::TERMINATE_TIMEOUT` and never signals a child that was already reaped. No signal
    /// is sent from here: `shell_pid` stays cached after the PTY thread reaped the child, so
    /// it may name an unrelated process by now.
    ///
    /// The join (and thereby the `Pty` drop) runs on a helper thread, so a PTY thread that fails
    /// to stop cannot block the main thread. `notify` runs on that helper after the join.
    /// Returns a receiver that completes after the join, or `None` if the PTY thread was
    /// already being stopped or the helper could not be spawned (the join handle is then
    /// dropped and the PTY thread drops the `Pty` itself when it stops). Display state is kept;
    /// call [`Self::finish_shutdown`] once the PTY is gone.
    pub fn begin_shutdown(
        &mut self,
        notify: impl FnOnce() + Send + 'static,
    ) -> Option<mpsc::Receiver<()>> {
        let _ = self.notifier.0.send(Msg::Shutdown);

        let io_thread = self.io_thread.take()?;

        let (done_tx, done_rx) = mpsc::channel();
        let joiner = std::thread::Builder::new().name("PTY shutdown".into()).spawn(move || {
            drop(io_thread.join());
            let _ = done_tx.send(());
            notify();
        });

        match joiner {
            Ok(_) => Some(done_rx),
            Err(err) => {
                warn!("Failed to spawn the PTY shutdown thread: {err}");
                None
            },
        }
    }

    /// embed: Whether the PTY thread was handed to [`Self::begin_shutdown`].
    pub fn shutdown_started(&self) -> bool {
        self.io_thread.is_none()
    }

    /// embed: Record that the PTY and its child are gone after a shutdown.
    pub fn finish_shutdown(&mut self) {
        if self.child_exit.is_none() {
            self.child_exit = Some(None);
        }
    }

    /// Submit the pending changes to the `Display`.
    fn submit_display_update(
        terminal: &mut Term<EventProxy>,
        display: &mut Display,
        notifier: &mut Notifier,
        search_state: &mut SearchState,
        old_is_searching: bool,
        config: &UiConfig,
    ) {
        // Compute cursor positions before resize.
        let num_lines = terminal.screen_lines();
        let cursor_at_bottom = terminal.grid().cursor.point.line + 1 == num_lines;
        let origin_at_bottom = if terminal.mode().contains(TermMode::VI) {
            terminal.vi_mode_cursor.point.line == num_lines - 1
        } else {
            search_state.direction == Direction::Left
        };

        display.handle_update(terminal, notifier, search_state, config);

        let new_is_searching = search_state.history_index.is_some();
        if !old_is_searching && new_is_searching {
            // Scroll on search start to make sure origin is visible with minimal viewport motion.
            let display_offset = terminal.grid().display_offset();
            if display_offset == 0 && cursor_at_bottom && !origin_at_bottom {
                terminal.scroll_display(Scroll::Delta(1));
            } else if display_offset != 0 && origin_at_bottom {
                terminal.scroll_display(Scroll::Delta(-1));
            }
        }
    }
}

/// embed: Wait for a [`WindowContext::begin_shutdown`] receiver, at most `timeout`.
///
/// Returns whether the PTY thread finished in time.
pub fn wait_for_shutdown(done: &mpsc::Receiver<()>, timeout: Duration) -> bool {
    match done.recv_timeout(timeout) {
        Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => true,
        Err(mpsc::RecvTimeoutError::Timeout) => {
            warn!("PTY shutdown did not finish within {timeout:?}; not waiting any longer");
            false
        },
    }
}

impl Drop for WindowContext {
    fn drop(&mut self) {
        // Shutdown the terminal's PTY.
        let _ = self.notifier.0.send(Msg::Shutdown);
    }
}
