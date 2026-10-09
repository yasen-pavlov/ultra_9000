//! TTY related functionality.
//!
//! embed: MODIFIED copy of alacritty_terminal/src/tty/unix.rs at
//! def47a5dd511ed20607eb5d9e5e7140afd16b31d (search for "embed:").
//! - `Drop for Pty` no longer sends SIGHUP to the child's PID unconditionally and then blocks
//!   in `Child::wait`. Upstream signalled that PID even after `next_child_event` had reaped
//!   the child, when the PID may belong to an unrelated process. `Pty::terminate` signals only
//!   the PTY's live foreground process group (`TIOCSIG` on macOS; verified member pidfds
//!   on Linux) and the child's own process group while the child is unreaped, escalates to
//!   SIGKILL, and is bounded by `TERMINATE_TIMEOUT`.
//! - PTY regressions cover ignored SIGHUP, shell foreground reclamation and Linux
//!   leaderless foreground pipelines.

use std::ffi::CStr;
use std::fs::File;
use std::io::{Error, ErrorKind, Read, Result};
use std::mem::MaybeUninit;
use std::os::fd::OwnedFd;
use std::os::unix::io::AsRawFd;
#[cfg(target_os = "linux")]
use std::os::fd::FromRawFd;
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
#[cfg(target_os = "macos")]
use std::path::Path;
use std::process::{Child, Command};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};
use std::{env, ptr};

use libc::{c_int, TIOCSCTTY};
use log::{error, info, warn};
use polling::{Event, PollMode, Poller};
use rustix_openpty::openpty;
use rustix_openpty::rustix::termios::Winsize;
#[cfg(any(target_os = "linux", target_os = "macos"))]
use rustix_openpty::rustix::termios::{self, InputModes, OptionalActions};
use signal_hook::low_level::{pipe as signal_pipe, unregister as unregister_signal};
use signal_hook::{consts as sigconsts, SigId};

use crate::event::{OnResize, WindowSize};
use crate::tty::{ChildEvent, EventedPty, EventedReadWrite, Options};

// Interest in PTY read/writes.
pub(crate) const PTY_READ_WRITE_TOKEN: usize = 0;

// Interest in new child events.
pub(crate) const PTY_CHILD_EVENT_TOKEN: usize = 1;

/// embed: Time a hung-up process group gets to exit before it is sent SIGKILL.
const HANGUP_GRACE: Duration = Duration::from_millis(1_000);

/// embed: Time a killed process group gets to disappear.
const KILL_GRACE: Duration = Duration::from_millis(500);

/// embed: Time spent stopping the terminal's foreground jobs before the child is signalled.
#[cfg(any(target_os = "linux", target_os = "macos"))]
const FOREGROUND_BUDGET: Duration = Duration::from_millis(2_000);

/// embed: Process state polling interval while terminating.
const TERMINATE_POLL: Duration = Duration::from_millis(10);

/// embed: Upper bound for the termination performed when a [`Pty`] is dropped.
///
/// Foreground jobs take at most `FOREGROUND_BUDGET + KILL_GRACE`, the child at most
/// `HANGUP_GRACE + KILL_GRACE`.
pub const TERMINATE_TIMEOUT: Duration = Duration::from_millis(2_000 + 500 + 1_000 + 500);

macro_rules! die {
    ($($arg:tt)*) => {{
        error!($($arg)*);
        std::process::exit(1);
    }}
}

/// Really only needed on BSD, but should be fine elsewhere.
fn set_controlling_terminal(fd: c_int) {
    let res = unsafe {
        // TIOSCTTY changes based on platform and the `ioctl` call is different
        // based on architecture (32/64). So a generic cast is used to make sure
        // there are no issues. To allow such a generic cast the clippy warning
        // is disabled.
        #[allow(clippy::cast_lossless)]
        libc::ioctl(fd, TIOCSCTTY as _, 0)
    };

    if res < 0 {
        die!("ioctl TIOCSCTTY failed: {}", Error::last_os_error());
    }
}

#[derive(Debug)]
struct Passwd<'a> {
    name: &'a str,
    dir: &'a str,
    shell: &'a str,
}

/// Return a Passwd struct with pointers into the provided buf.
///
/// # Unsafety
///
/// If `buf` is changed while `Passwd` is alive, bad thing will almost certainly happen.
fn get_pw_entry(buf: &mut [i8; 1024]) -> Result<Passwd<'_>> {
    // Create zeroed passwd struct.
    let mut entry: MaybeUninit<libc::passwd> = MaybeUninit::uninit();

    let mut res: *mut libc::passwd = ptr::null_mut();

    // Try and read the pw file.
    let uid = unsafe { libc::getuid() };
    let status = unsafe {
        libc::getpwuid_r(uid, entry.as_mut_ptr(), buf.as_mut_ptr() as *mut _, buf.len(), &mut res)
    };
    let entry = unsafe { entry.assume_init() };

    if status < 0 {
        return Err(Error::new(ErrorKind::Other, "getpwuid_r failed"));
    }

    if res.is_null() {
        return Err(Error::new(ErrorKind::Other, "pw not found"));
    }

    // Sanity check.
    assert_eq!(entry.pw_uid, uid);

    // Build a borrowed Passwd struct.
    Ok(Passwd {
        name: unsafe { CStr::from_ptr(entry.pw_name).to_str().unwrap() },
        dir: unsafe { CStr::from_ptr(entry.pw_dir).to_str().unwrap() },
        shell: unsafe { CStr::from_ptr(entry.pw_shell).to_str().unwrap() },
    })
}

pub struct Pty {
    child: Child,
    file: File,
    signals: UnixStream,
    sig_id: SigId,
}

/// embed: Identity fields from Linux /proc/<pid>/stat, independent of a live group leader.
#[cfg(target_os = "linux")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ProcessIdentity {
    group: libc::pid_t,
    session: libc::pid_t,
    tty: i32,
    starttime: u64,
}

#[cfg(target_os = "linux")]
impl ProcessIdentity {
    fn parse(stat: &str) -> Option<Self> {
        // comm may contain spaces and parentheses. Fields after its final ')' start at 3.
        let (_, fields) = stat.rsplit_once(')')?;
        let mut fields = fields.split_ascii_whitespace();
        fields.next()?; // state (3)
        fields.next()?; // ppid (4)
        let group = fields.next()?.parse().ok()?; // pgrp (5)
        let session = fields.next()?.parse().ok()?; // session (6)
        let tty = fields.next()?.parse().ok()?; // tty_nr (7)
        let starttime = fields.nth(14)?.parse().ok()?; // starttime (22)
        Some(Self { group, session, tty, starttime })
    }

    fn read(pid: libc::pid_t) -> Option<Self> {
        Self::parse(&std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?)
    }

    fn is_foreground_member(self, shell: Self, group: libc::pid_t) -> bool {
        self.group == group && self.session == shell.session && self.tty == shell.tty
            && self.tty != 0
    }

    fn still_matches(self, current: Self, shell: Self, group: libc::pid_t) -> bool {
        self.starttime == current.starttime && current.is_foreground_member(shell, group)
    }
}

impl Pty {
    pub fn child(&self) -> &Child {
        &self.child
    }

    pub fn file(&self) -> &File {
        &self.file
    }

    /// embed: Stop the processes of the child's terminal session.
    ///
    /// Signals reach only:
    /// - The terminal's current foreground process group, through `TIOCSIG` on macOS
    ///   or pidfds for /proc-verified members of the child's session/terminal on Linux.
    ///   Each Linux member is revalidated after pidfd_open, including its starttime.
    /// - The child's own process group, through `killpg`, and only while the child is
    ///   unreaped. The child is a session leader, so its process group ID is its PID, and the
    ///   kernel does not reuse a PID before the process is reaped. Only this `Child` reaps it.
    ///
    /// No new foreground targets are collected once the child has been reaped, e.g. by
    /// `next_child_event`. Linux pidfds already verified before delivery remain safe targets
    /// through escalation even if the child exits or the shell reclaims the terminal.
    ///
    /// Foreground jobs are collected before stopping the child: its exit sends SIGHUP to
    /// the foreground group and detaches the terminal, preventing further safe collection.
    /// Linux retains verified pidfds so ignored-SIGHUP members can still be killed afterward.
    /// Processes outside the verified foreground set, like background jobs ignoring SIGHUP
    /// or daemons that started their own session, are not tracked and may survive.
    ///
    /// Takes at most `TERMINATE_TIMEOUT`. A child that is still running after SIGKILL is left
    /// unreaped, so its PID is never recycled while this `Pty` could signal it.
    fn terminate(&mut self) {
        if !self.child_running() {
            return;
        }

        #[cfg(any(target_os = "linux", target_os = "macos"))]
        self.stop_foreground_jobs();

        for (signal, grace) in [(libc::SIGHUP, HANGUP_GRACE), (libc::SIGKILL, KILL_GRACE)] {
            if !self.child_running() {
                return;
            }

            // The child was unreaped when checked above, and nothing else reaps it, so its PID
            // and process group ID cannot have been reused.
            let pid = self.child.id() as libc::pid_t;
            info!("Sending signal {signal} to child process group {pid}");
            unsafe {
                libc::killpg(pid, signal);
            }

            if self.wait_until(Instant::now() + grace, |pty| !pty.child_running()) {
                return;
            }
        }

        warn!("Child process {} did not exit after SIGKILL", self.child.id());
    }

    /// embed: Hang up, and after `HANGUP_GRACE` kill, the terminal's foreground jobs.
    ///
    /// Repeats for each process group that takes over the foreground (e.g. the shell after
    /// its foreground job died), until the child exits or `FOREGROUND_BUDGET` passes.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    fn stop_foreground_jobs(&mut self) {
        let deadline = Instant::now() + FOREGROUND_BUDGET;
        while self.child_running() && Instant::now() < deadline {
            let group = match self.foreground_group() {
                Some(group) => group,
                None => {
                    // E.g. between a job's exit and the shell reclaiming the terminal.
                    let reclaim = deadline.min(Instant::now() + KILL_GRACE);
                    let reclaimed = self.wait_until(reclaim, |pty| {
                        pty.foreground_group().is_some() || !pty.child_running()
                    });
                    if reclaimed {
                        continue;
                    }
                    return;
                },
            };

            #[cfg(target_os = "linux")]
            {
                let Some(members) = self.foreground_members(group, deadline) else { continue };
                // embed: Retain process identities, never a numeric PGID, through escalation.
                // Foreground reclamation or the session leader's exit cannot retarget pidfds
                // and must not spare a surviving member which ignored the initial hangup.
                Self::signal_members(&members, libc::SIGHUP);
                if !self.wait_until(deadline.min(Instant::now() + HANGUP_GRACE), |_| {
                    Self::members_exited(&members)
                }) {
                    let kill_deadline = Instant::now() + KILL_GRACE;
                    Self::signal_members(&members, libc::SIGKILL);
                    self.wait_until(kill_deadline, |_| Self::members_exited(&members));
                }
            }
            #[cfg(target_os = "macos")]
            {
                // embed: TIOCSIG always targets the current foreground, never a retained PGID.
                let replaced = move |pty: &mut Self| {
                    !pty.child_running() || pty.foreground_group() != Some(group)
                };
                self.signal_foreground(libc::SIGHUP);
                if !self.wait_until(deadline.min(Instant::now() + HANGUP_GRACE), replaced) {
                    let kill_deadline = Instant::now() + KILL_GRACE;
                    self.signal_foreground(libc::SIGKILL);
                    self.wait_until(kill_deadline, replaced);
                }
            }
        }
    }

    /// embed: Send `signal` to the terminal's current foreground process group.
    #[cfg(target_os = "macos")]
    fn signal_foreground(&mut self, signal: c_int) {
        // embed: The Child owns reaping; no new signal after it was reaped.
        if !self.child_running() { return; }
        let res = unsafe {
            libc::ioctl(self.file.as_raw_fd(), libc::TIOCSIG as _, signal as libc::c_ulong)
        };
        if res < 0 {
            warn!("Failed to signal the PTY foreground process group: {}", Error::last_os_error());
        }
    }

    /// embed: Collect the complete verified foreground set before delivering any signal.
    /// Never signal a numeric foreground PGID; pidfds stay bound to the opened processes.
    #[cfg(target_os = "linux")]
    fn foreground_members(&mut self, group: libc::pid_t, deadline: Instant) -> Option<Vec<OwnedFd>> {
        let shell_pid = self.child.id() as libc::pid_t;
        let shell = ProcessIdentity::read(shell_pid)?;
        if shell.session != shell_pid || shell.tty == 0 { return None; }
        let processes = std::fs::read_dir("/proc").ok()?;
        let mut members = Vec::new();
        for entry in processes {
            // embed: An incomplete scan must not begin delivering signals.
            if Instant::now() >= deadline { return None; }
            let Ok(entry) = entry else { continue };
            let Some(pid) = entry.file_name().to_str().and_then(|name| name.parse().ok()) else {
                continue;
            };
            let Some(before) = ProcessIdentity::read(pid) else { continue };
            if !before.is_foreground_member(shell, group) { continue; }
            let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0u32) };
            if fd < 0 { continue; }
            let pidfd = unsafe { OwnedFd::from_raw_fd(fd as c_int) };
            let Some(after) = ProcessIdentity::read(pid) else { continue };
            if !before.still_matches(after, shell, group) { continue; }
            members.push(pidfd);
        }
        // embed: Validate foreground ownership once before delivery, never mid-delivery.
        // Reaping is safe here: no numeric signal follows it.
        if Instant::now() >= deadline || !self.child_running()
            || self.foreground_group() != Some(group)
        {
            return None;
        }
        Some(members)
    }

    /// embed: Deliver to every verified member even if an earlier delivery changes the TTY.
    #[cfg(target_os = "linux")]
    fn signal_members(members: &[OwnedFd], signal: c_int) {
        for pidfd in members {
            let res = unsafe {
                libc::syscall(
                    libc::SYS_pidfd_send_signal, pidfd.as_raw_fd(), signal,
                    ptr::null::<libc::siginfo_t>(), 0u32,
                )
            };
            if res < 0 && Error::last_os_error().raw_os_error() != Some(libc::ESRCH) {
                warn!("Failed to signal a PTY foreground member: {}", Error::last_os_error());
            }
        }
    }

    /// embed: Pidfd readiness reports exit without reaping or consulting reusable PIDs.
    #[cfg(target_os = "linux")]
    fn members_exited(members: &[OwnedFd]) -> bool {
        members.iter().all(|pidfd| {
            let mut fd = libc::pollfd { fd: pidfd.as_raw_fd(), events: libc::POLLIN, revents: 0 };
            unsafe { libc::poll(&mut fd, 1, 0) > 0 && fd.revents & libc::POLLIN != 0 }
        })
    }

    /// embed: ID of the terminal's foreground process group, if it has one.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    fn foreground_group(&self) -> Option<libc::pid_t> {
        let group = unsafe { libc::tcgetpgrp(self.file.as_raw_fd()) };
        (group > 0).then_some(group)
    }

    /// embed: Whether the child is still running.
    ///
    /// Reaps the child once it exited. `Child` caches the exit status, so from then on this is
    /// `false` without the PID being used again. An error, e.g. because the child was reaped
    /// outside of this `Child`, also counts as not running, so the PID is never signalled.
    fn child_running(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    /// embed: Poll until `done` holds or `deadline` passes. Returns whether `done` held.
    ///
    /// Pending output is discarded meanwhile: the I/O loop no longer reads the PTY, and an
    /// exiting session leader waits for the terminal's output queue to drain.
    fn wait_until(&mut self, deadline: Instant, mut done: impl FnMut(&mut Self) -> bool) -> bool {
        loop {
            self.discard_output();
            if done(self) {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            thread::sleep(TERMINATE_POLL);
        }
    }

    /// embed: Discard a bounded amount of pending PTY output.
    fn discard_output(&mut self) {
        let mut buf = [0u8; 4096];
        for _ in 0..16 {
            match self.file.read(&mut buf) {
                Ok(read) if read > 0 => (),
                _ => break,
            }
        }
    }
}

/// User information that is required for a new shell session.
struct ShellUser {
    user: String,
    home: String,
    shell: String,
}

impl ShellUser {
    /// look for shell, username, longname, and home dir in the respective environment variables
    /// before falling back on looking into `passwd`.
    fn from_env() -> Result<Self> {
        let mut buf = [0; 1024];
        let pw = get_pw_entry(&mut buf);

        let user = match env::var("USER") {
            Ok(user) => user,
            Err(_) => match pw {
                Ok(ref pw) => pw.name.to_owned(),
                Err(err) => return Err(err),
            },
        };

        let home = match env::var("HOME") {
            Ok(home) => home,
            Err(_) => match pw {
                Ok(ref pw) => pw.dir.to_owned(),
                Err(err) => return Err(err),
            },
        };

        let shell = match env::var("SHELL") {
            Ok(shell) => shell,
            Err(_) => match pw {
                Ok(ref pw) => pw.shell.to_owned(),
                Err(err) => return Err(err),
            },
        };

        Ok(Self { user, home, shell })
    }
}

#[cfg(not(target_os = "macos"))]
fn default_shell_command(shell: &str, _user: &str, _home: &str) -> Command {
    Command::new(shell)
}

#[cfg(target_os = "macos")]
fn default_shell_command(shell: &str, user: &str, home: &str) -> Command {
    let shell_name = shell.rsplit('/').next().unwrap();

    // On macOS, use the `login` command so the shell will appear as a tty session.
    let mut login_command = Command::new("/usr/bin/login");

    // Exec the shell with argv[0] prepended by '-' so it becomes a login shell.
    // `login` normally does this itself, but `-l` disables this.
    let exec = format!("exec -a -{} {}", shell_name, shell);

    // Since we use -l, `login` will not change directory to the user's home. However,
    // `login` only checks the current working directory for a .hushlogin file, causing
    // it to miss any in the user's home directory. We can fix this by doing the check
    // ourselves and passing `-q`
    let has_home_hushlogin = Path::new(home).join(".hushlogin").exists();

    // -f: Bypasses authentication for the already-logged-in user.
    // -l: Skips changing directory to $HOME and prepending '-' to argv[0].
    // -p: Preserves the environment.
    // -q: Act as if `.hushlogin` exists.
    //
    // XXX: we use zsh here over sh due to `exec -a`.
    let flags = if has_home_hushlogin { "-qflp" } else { "-flp" };
    login_command.args([flags, user, "/bin/zsh", "-fc", &exec]);
    login_command
}

/// Create a new TTY and return a handle to interact with it.
pub fn new(config: &Options, window_size: WindowSize, window_id: u64) -> Result<Pty> {
    let pty = openpty(None, Some(&window_size.to_winsize()))?;
    let (master, slave) = (pty.controller, pty.user);
    from_fd(config, window_id, master, slave)
}

/// Create a new TTY from a PTY's file descriptors.
pub fn from_fd(config: &Options, window_id: u64, master: OwnedFd, slave: OwnedFd) -> Result<Pty> {
    let master_fd = master.as_raw_fd();
    let slave_fd = slave.as_raw_fd();

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    if let Ok(mut termios) = termios::tcgetattr(&master) {
        // Set character encoding to UTF-8.
        termios.input_modes.set(InputModes::IUTF8, true);
        let _ = termios::tcsetattr(&master, OptionalActions::Now, &termios);
    }

    let user = ShellUser::from_env()?;

    let mut builder = if let Some(shell) = config.shell.as_ref() {
        let mut cmd = Command::new(&shell.program);
        cmd.args(shell.args.as_slice());
        cmd
    } else {
        default_shell_command(&user.shell, &user.user, &user.home)
    };

    // Setup child stdin/stdout/stderr as slave fd of PTY.
    builder.stdin(slave.try_clone()?);
    builder.stderr(slave.try_clone()?);
    builder.stdout(slave);

    // Setup shell environment.
    let window_id = window_id.to_string();
    builder.env("ALACRITTY_WINDOW_ID", &window_id);
    builder.env("USER", user.user);
    builder.env("HOME", user.home);
    // Set Window ID for clients relying on X11 hacks.
    builder.env("WINDOWID", window_id);
    for (key, value) in &config.env {
        builder.env(key, value);
    }

    // Prevent child processes from inheriting linux-specific startup notification env.
    builder.env_remove("XDG_ACTIVATION_TOKEN");
    builder.env_remove("DESKTOP_STARTUP_ID");

    let working_directory = config.working_directory.clone();
    unsafe {
        builder.pre_exec(move || {
            // Create a new process group.
            let err = libc::setsid();
            if err == -1 {
                return Err(Error::new(ErrorKind::Other, "Failed to set session id"));
            }

            // Set working directory, ignoring invalid paths.
            if let Some(working_directory) = working_directory.as_ref() {
                let _ = env::set_current_dir(working_directory);
            }

            set_controlling_terminal(slave_fd);

            // No longer need slave/master fds.
            libc::close(slave_fd);
            libc::close(master_fd);

            libc::signal(libc::SIGCHLD, libc::SIG_DFL);
            libc::signal(libc::SIGHUP, libc::SIG_DFL);
            libc::signal(libc::SIGINT, libc::SIG_DFL);
            libc::signal(libc::SIGQUIT, libc::SIG_DFL);
            libc::signal(libc::SIGTERM, libc::SIG_DFL);
            libc::signal(libc::SIGALRM, libc::SIG_DFL);

            Ok(())
        });
    }

    // Prepare signal handling before spawning child.
    let (signals, sig_id) = {
        let (sender, recv) = UnixStream::pair()?;

        // Register the recv end of the pipe for SIGCHLD.
        let sig_id = signal_pipe::register(sigconsts::SIGCHLD, sender)?;
        recv.set_nonblocking(true)?;
        (recv, sig_id)
    };

    match builder.spawn() {
        Ok(child) => {
            unsafe {
                // Maybe this should be done outside of this function so nonblocking
                // isn't forced upon consumers. Although maybe it should be?
                set_nonblocking(master_fd);
            }

            Ok(Pty { child, file: File::from(master), signals, sig_id })
        },
        Err(err) => Err(Error::new(
            err.kind(),
            format!(
                "Failed to spawn command '{}': {}",
                builder.get_program().to_string_lossy(),
                err
            ),
        )),
    }
}

impl Drop for Pty {
    fn drop(&mut self) {
        // Make sure the PTY is terminated properly.
        //
        // embed: Upstream sent SIGHUP to `child.id()` unconditionally, even when the child had
        // already been reaped and its PID reused, then blocked in `Child::wait`.
        self.terminate();

        // Clear signal-hook handler.
        unregister_signal(self.sig_id);
    }
}

impl EventedReadWrite for Pty {
    type Reader = File;
    type Writer = File;

    #[inline]
    unsafe fn register(
        &mut self,
        poll: &Arc<Poller>,
        mut interest: Event,
        poll_opts: PollMode,
    ) -> Result<()> {
        interest.key = PTY_READ_WRITE_TOKEN;
        unsafe {
            poll.add_with_mode(&self.file, interest, poll_opts)?;
        }

        unsafe {
            poll.add_with_mode(
                &self.signals,
                Event::readable(PTY_CHILD_EVENT_TOKEN),
                PollMode::Level,
            )
        }
    }

    #[inline]
    fn reregister(
        &mut self,
        poll: &Arc<Poller>,
        mut interest: Event,
        poll_opts: PollMode,
    ) -> Result<()> {
        interest.key = PTY_READ_WRITE_TOKEN;
        poll.modify_with_mode(&self.file, interest, poll_opts)?;

        poll.modify_with_mode(
            &self.signals,
            Event::readable(PTY_CHILD_EVENT_TOKEN),
            PollMode::Level,
        )
    }

    #[inline]
    fn deregister(&mut self, poll: &Arc<Poller>) -> Result<()> {
        poll.delete(&self.file)?;
        poll.delete(&self.signals)
    }

    #[inline]
    fn reader(&mut self) -> &mut File {
        &mut self.file
    }

    #[inline]
    fn writer(&mut self) -> &mut File {
        &mut self.file
    }
}

impl EventedPty for Pty {
    #[inline]
    fn next_child_event(&mut self) -> Option<ChildEvent> {
        // See if there has been a SIGCHLD.
        let mut buf = [0u8; 1];
        if let Err(err) = self.signals.read(&mut buf) {
            if err.kind() != ErrorKind::WouldBlock {
                error!("Error reading from signal pipe: {}", err);
            }
            return None;
        }

        // Match on the child process.
        match self.child.try_wait() {
            Err(err) => {
                error!("Error checking child process termination: {}", err);
                None
            },
            Ok(None) => None,
            Ok(exit_status) => Some(ChildEvent::Exited(exit_status.and_then(|s| s.code()))),
        }
    }
}

impl OnResize for Pty {
    /// Resize the PTY.
    ///
    /// Tells the kernel that the window size changed with the new pixel
    /// dimensions and line/column counts.
    fn on_resize(&mut self, window_size: WindowSize) {
        let win = window_size.to_winsize();

        let res = unsafe { libc::ioctl(self.file.as_raw_fd(), libc::TIOCSWINSZ, &win as *const _) };

        if res < 0 {
            die!("ioctl TIOCSWINSZ failed: {}", Error::last_os_error());
        }
    }
}

/// Types that can produce a `Winsize`.
pub trait ToWinsize {
    /// Get a `Winsize`.
    fn to_winsize(self) -> Winsize;
}

impl ToWinsize for WindowSize {
    fn to_winsize(self) -> Winsize {
        let ws_row = self.num_lines as libc::c_ushort;
        let ws_col = self.num_cols as libc::c_ushort;

        let ws_xpixel = ws_col * self.cell_width as libc::c_ushort;
        let ws_ypixel = ws_row * self.cell_height as libc::c_ushort;
        Winsize { ws_row, ws_col, ws_xpixel, ws_ypixel }
    }
}

unsafe fn set_nonblocking(fd: c_int) {
    use libc::{fcntl, F_GETFL, F_SETFL, O_NONBLOCK};

    let res = fcntl(fd, F_SETFL, fcntl(fd, F_GETFL, 0) | O_NONBLOCK);
    assert_eq!(res, 0);
}

#[test]
fn test_get_pw_entry() {
    let mut buf: [i8; 1024] = [0; 1024];
    let _pw = get_pw_entry(&mut buf).unwrap();
}

/// embed: Shutdown regression tests. They only signal processes they spawned themselves.
#[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
mod tests {
    use std::ffi::CString;
    use std::fs::{self, OpenOptions};
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::OpenOptionsExt;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;
    use crate::tty::Shell;

    // embed: A normal child status survives the PTY exit channel; signal death stays None.
    #[test]
    fn child_event_preserves_normal_and_signalled_exit_status() {
        let size = WindowSize { num_lines: 24, num_cols: 80, cell_width: 8, cell_height: 16 };
        for (script, expected) in [("exit 7", Some(7)), ("kill -TERM $$", None)] {
            let config = Options {
                shell: Some(Shell::new("/bin/sh".into(), vec!["-c".into(), script.into()])),
                ..Default::default()
            };
            let mut pty = new(&config, size, 0).unwrap();
            let mut observed = None;
            poll("child exit event", || {
                observed = pty.next_child_event();
                observed.is_some()
            });
            assert!(matches!(observed, Some(ChildEvent::Exited(code)) if code == expected));
            assert!(!pty.child_running());
            drop(pty); // Already-reaped status must make teardown a no-op.
        }
    }

    /// Poll `done` until it holds, failing with `what` after ten seconds.
    fn poll(what: &str, mut done: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !done() {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            thread::sleep(TERMINATE_POLL);
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn foreground_identity_rejects_changed_starttime() {
        let stat = "42 (a tricky ) command) S 1 42 41 34816 0 0 0 0 0 0 0 0 0 0 0 0 0 0 12345";
        let before = ProcessIdentity::parse(stat).unwrap();
        assert_eq!(before.starttime, 12345);
        let shell = ProcessIdentity { group: 41, session: 41, tty: 34816, starttime: 1 };
        assert!(before.still_matches(before, shell, 42));
        let recycled = ProcessIdentity { starttime: 12346, ..before };
        assert!(!before.still_matches(recycled, shell, 42));
        assert!(!before.still_matches(ProcessIdentity { session: 40, ..before }, shell, 42));
        assert!(!before.still_matches(ProcessIdentity { tty: 0, ..before }, shell, 42));
        assert!(!before.still_matches(ProcessIdentity { group: 43, ..before }, shell, 42));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn drop_stops_leaderless_foreground_pipeline_ignoring_hangup() {
        assert_foreground_job_stopped(
            r#"set -m; /bin/sh -c 'exit 0' | /bin/sh -c 'trap "" HUP; exec 3>"$FIFO"; sleep 60 & echo ready >&3; wait'"#,
            true,
            false,
        );
    }

    /// Dropping the PTY stops a foreground job that ignores SIGHUP, including its children.
    ///
    /// The job and its child hold the write end of a FIFO, which reaches EOF only once both
    /// exited, so no PID is inspected or signalled by the test.
    #[test]
    fn drop_stops_foreground_job_ignoring_hangup() {
        assert_foreground_job_stopped(
            r#"set -m; /bin/sh -c 'trap "" HUP; exec 3>"$FIFO"; sleep 60 & echo ready >&3; wait'"#,
            false,
            false,
        );
    }

    /// embed: The shell reclaims the TTY and exits after HUP kills the job's parent;
    /// its same-group child ignores HUP and must still receive the retained-set SIGKILL.
    #[cfg(target_os = "linux")]
    #[test]
    fn drop_escalates_ignored_hangup_child_after_shell_reclaims_tty() {
        assert_foreground_job_stopped(
            r#"set -m; /bin/sh -c 'trap "exit 0" HUP; exec 3>"$FIFO"; /bin/sh -c '\''trap "" HUP; echo ready >&3; exec sleep 60'\'' & wait'; echo reclaimed >"$RECLAIM""#,
            false,
            true,
        );
    }

    fn assert_foreground_job_stopped(script: &str, _leaderless: bool, reclaims: bool) {
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let dir = env::temp_dir().join(format!("alacritty-pty-{}-{nanos}", std::process::id()));
        fs::create_dir(&dir).unwrap();
        let fifo = dir.join("fifo");
        let reclaim = dir.join("reclaimed");
        let fifo_path = CString::new(fifo.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(fifo_path.as_ptr(), 0o600) }, 0);
        let mut reader =
            OpenOptions::new().read(true).custom_flags(libc::O_NONBLOCK).open(&fifo).unwrap();

        // `set -m` runs the job in its own process group as the terminal's foreground group,
        // so the session leader's exit alone would only send it the SIGHUP it ignores.
        let config = Options {
            shell: Some(Shell::new("/bin/sh".into(), vec!["-c".into(), script.into()])),
            env: [
                ("FIFO".to_owned(), fifo.to_string_lossy().into_owned()),
                ("RECLAIM".to_owned(), reclaim.to_string_lossy().into_owned()),
            ].into(),
            ..Default::default()
        };
        let size = WindowSize { num_lines: 24, num_cols: 80, cell_width: 8, cell_height: 16 };
        let pty = new(&config, size, 0).unwrap();

        let mut buf = [0u8; 16];
        poll("the job to start", || matches!(reader.read(&mut buf), Ok(read) if read > 0));
        let leader = pty.child().id() as libc::pid_t;
        poll("the job to take the terminal's foreground", || {
            let group = unsafe { libc::tcgetpgrp(pty.file().as_raw_fd()) };
            group > 0 && group != leader
        });
        #[cfg(target_os = "linux")]
        if _leaderless {
            poll("the foreground pipeline leader to exit and be reaped", || {
                let group = unsafe { libc::tcgetpgrp(pty.file().as_raw_fd()) };
                group > 0 && group != leader && ProcessIdentity::read(group).is_none()
            });
        }

        let start = Instant::now();
        drop(pty);
        assert!(start.elapsed() < TERMINATE_TIMEOUT + Duration::from_secs(1));
        if reclaims {
            assert_eq!(fs::read_to_string(reclaim).unwrap(), "reclaimed\n");
        }

        poll("the job and its child to exit", || match reader.read(&mut buf) {
            Ok(read) => read == 0,
            Err(err) if err.kind() == ErrorKind::WouldBlock => false,
            Err(err) => panic!("reading the FIFO failed: {err}"),
        });

        let _ = fs::remove_dir_all(&dir);
    }
}
