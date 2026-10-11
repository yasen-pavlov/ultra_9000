//! `ultra9000 ctl`: the command-line client of the shell control link (`control.rs`), for
//! shell plugins and scripts. It runs without Tauri, GTK or the app's logger.
//!
//! `ctl watch` prints every line the app sends and keeps reconnecting; `ctl <command>` sends
//! one command and prints its reply. Both find the newest running instance's socket in the
//! runtime directory unless `ULTRA9000_CONTROL_SOCKET` names one.
use crate::{
    control::{self, MAX_LINE, PING_INTERVAL, PROTOCOL, SOCKET_ENV},
    omp_link,
};
use serde_json::{json, Map, Value};
use std::{
    ffi::OsString,
    fs,
    io::{self, BufRead, BufReader, Read, Write},
    os::{fd::AsFd, unix::net::UnixStream},
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime},
};

const USAGE: &str = "usage: ultra9000 ctl watch [--client NAME]\n       ultra9000 ctl <command> ['<json args object>']\n";
const DISCONNECTED: &str = "{\"type\":\"disconnected\"}";
/// First reconnect delay; it doubles up to [`RETRY_MAX`].
const RETRY_FIRST: Duration = Duration::from_secs(1);
const RETRY_MAX: Duration = Duration::from_secs(10);
/// A watched connection that stays silent this long (three missed pings) is dropped.
const SILENCE: Duration = Duration::from_secs(PING_INTERVAL.as_secs() * 3);
/// How long a one-shot command waits for its reply (a launch resolves the login PATH first).
const REPLY_TIMEOUT: Duration = Duration::from_secs(30);

/// Run `ultra9000 ctl <args>`; returns the process exit code.
pub fn main(args: Vec<OsString>) -> i32 {
    let args: Option<Vec<String>> = args.into_iter().map(|a| a.into_string().ok()).collect();
    let stdout = io::stdout();
    let mut out = stdout.lock();
    match args.as_deref() {
        Some([watch, rest @ ..]) if watch == "watch" => {
            let client = match rest {
                [] => "ctl",
                [flag, name] if flag == "--client" => name,
                _ => return usage(),
            };
            // Returns only once stdout is gone.
            let _ = run_watch(
                &mut || connect(&candidates()),
                client,
                &mut out,
                &mut |delay| reader_present(&io::stdout(), delay),
            );
            0
        }
        Some([command]) if is_command_name(command) => {
            run_command(&mut out, command, None, connect(&candidates()))
        }
        Some([command, args]) if is_command_name(command) => {
            run_command(&mut out, command, Some(args), connect(&candidates()))
        }
        _ => usage(),
    }
}

fn usage() -> i32 {
    eprint!("{USAGE}");
    2
}

fn is_command_name(name: &str) -> bool {
    !name.is_empty() && name.bytes().all(|b| b.is_ascii_lowercase() || b == b'_')
}

/// Sockets to try, best first: `$ULTRA9000_CONTROL_SOCKET`, else every live instance's in
/// the runtime directory.
fn candidates() -> Vec<PathBuf> {
    socket_candidates(std::env::var_os(SOCKET_ENV), &omp_link::runtime_dir())
}

/// The override when set, else the `control-<pid>.sock` files in `dir` whose app still runs,
/// newest first; none unless `dir` is private to this user (the temp-dir fallback could have
/// been created by someone else). `dir` is never created.
pub(crate) fn socket_candidates(env: Option<OsString>, dir: &Path) -> Vec<PathBuf> {
    if let Some(path) = env.filter(|path| !path.is_empty()) {
        return vec![PathBuf::from(path)];
    }
    if !omp_link::is_private_dir(dir) {
        return Vec::new();
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut sockets: Vec<(SystemTime, PathBuf)> = entries
        .flatten()
        .filter(|entry| {
            entry
                .file_name()
                .to_str()
                .and_then(control::socket_pid_digits)
                .and_then(omp_link::pid_from_digits)
                .is_some_and(omp_link::process_alive)
        })
        .filter_map(|entry| {
            let modified = entry.metadata().and_then(|m| m.modified()).ok()?;
            Some((modified, entry.path()))
        })
        .collect();
    sockets.sort_by(|a, b| b.cmp(a));
    sockets.into_iter().map(|(_, path)| path).collect()
}

/// The first socket that accepts a connection.
fn connect(candidates: &[PathBuf]) -> Option<UnixStream> {
    candidates
        .iter()
        .find_map(|path| UnixStream::connect(path).ok())
}

fn hello(client: &str, watch: bool) -> String {
    let mut line = json!({
        "type": "hello",
        "protocol": PROTOCOL,
        "client": client,
        "version": env!("CARGO_PKG_VERSION"),
        "watch": watch,
    })
    .to_string();
    line.push('\n');
    line
}

/// The delay after `delay`: doubled, at most [`RETRY_MAX`].
fn next_delay(delay: Duration) -> Duration {
    (delay * 2).min(RETRY_MAX)
}

/// Relay every line of a watched connection to `out`, reconnecting forever: each drop (or a
/// failed first attempt) prints one `disconnected` line, then attempts follow after 1 s,
/// doubling to 10 s. `wait(delay)` waits out a delay and answers whether stdout's reader is
/// still there. Returns when it is gone or writing to `out` fails, so an orphaned watcher
/// exits even while no app runs.
fn run_watch(
    connect: &mut dyn FnMut() -> Option<UnixStream>,
    client: &str,
    out: &mut dyn Write,
    wait: &mut dyn FnMut(Duration) -> bool,
) -> io::Result<()> {
    let mut delay = RETRY_FIRST;
    let mut announced = false;
    loop {
        if let Some(mut stream) = connect() {
            if stream.write_all(hello(client, true).as_bytes()).is_ok() && relay(stream, out)? {
                announced = false;
                delay = RETRY_FIRST;
            }
        }
        if !announced {
            writeln!(out, "{DISCONNECTED}")?;
            out.flush()?;
            announced = true;
        }
        if !wait(delay) {
            return Ok(());
        }
        delay = next_delay(delay);
    }
}

/// Wait up to `timeout` for the reading end of `out` to go away (a closed pipe or socket);
/// false as soon as it has. A terminal or file never goes away. Linux reports a pipe whose
/// reader closed as a hang-up even when no events are requested; macOS does not, so there
/// the watcher only notices on its next write (the shell plugin it serves is Linux-only).
#[cfg(target_os = "linux")]
fn reader_present(out: &impl AsFd, timeout: Duration) -> bool {
    use std::{os::fd::AsRawFd, time::Instant};
    let deadline = Instant::now() + timeout;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        // Asking for no events still reports errors and hang-ups.
        let mut poll = libc::pollfd {
            fd: out.as_fd().as_raw_fd(),
            events: 0,
            revents: 0,
        };
        let millis = libc::c_int::try_from(left.as_millis()).unwrap_or(libc::c_int::MAX);
        // SAFETY: one valid pollfd for the duration of the call.
        let ready = unsafe { libc::poll(&mut poll, 1, millis) };
        if ready > 0 {
            return poll.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) == 0;
        }
        let interrupted =
            ready < 0 && io::Error::last_os_error().kind() == io::ErrorKind::Interrupted;
        if !interrupted || left.is_zero() {
            return true;
        }
    }
}
#[cfg(not(target_os = "linux"))]
fn reader_present(_out: &impl AsFd, timeout: Duration) -> bool {
    std::thread::sleep(timeout);
    true
}

/// Copy lines from `stream` to `out`, flushing each, until the connection ends or goes
/// silent. Returns whether any line arrived.
fn relay(stream: UnixStream, out: &mut dyn Write) -> io::Result<bool> {
    if stream.set_read_timeout(Some(SILENCE)).is_err() {
        return Ok(false);
    }
    let mut reader = BufReader::new(stream);
    let mut line = Vec::new();
    let mut relayed = false;
    while next_line(&mut reader, &mut line) {
        out.write_all(&line)?;
        out.write_all(b"\n")?;
        out.flush()?;
        relayed = true;
    }
    Ok(relayed)
}

/// Read the next complete line (without its newline); false at the end, on an error or
/// timeout, or for an overlong line.
fn next_line(reader: &mut impl BufRead, line: &mut Vec<u8>) -> bool {
    line.clear();
    match reader.take(MAX_LINE as u64 + 1).read_until(b'\n', line) {
        Ok(_) if line.last() == Some(&b'\n') => {
            line.pop();
            true
        }
        _ => false,
    }
}

/// Send one command on `stream` and print its reply: exit 0 if ok, 1 if not, 2 without an
/// app (printing `disconnected`) or for invalid arguments.
fn run_command(
    out: &mut dyn Write,
    command: &str,
    args: Option<&str>,
    stream: Option<UnixStream>,
) -> i32 {
    let args = match args.map(serde_json::from_str::<Value>) {
        None => Map::new(),
        Some(Ok(Value::Object(args))) => args,
        Some(_) => {
            eprintln!("ultra9000 ctl: the arguments must be one JSON object");
            return 2;
        }
    };
    let reply = command_reply(command, args, stream, REPLY_TIMEOUT);
    let (code, line) = match &reply {
        Some((ok, line)) => (if *ok { 0 } else { 1 }, line.as_slice()),
        None => (2, DISCONNECTED.as_bytes()),
    };
    if out
        .write_all(line)
        .and_then(|()| out.write_all(b"\n"))
        .and_then(|()| out.flush())
        .is_err()
    {
        return 2;
    }
    code
}

/// Shared one-shot hello/request/reply path for ctl and pre-GUI instance handover.
fn command_reply(
    command: &str,
    mut request: Map<String, Value>,
    stream: Option<UnixStream>,
    timeout: Duration,
) -> Option<(bool, Vec<u8>)> {
    request.insert("type".into(), json!(command));
    request.insert("id".into(), json!(1));
    let mut line = hello("ctl", false);
    line.push_str(&Value::Object(request).to_string());
    line.push('\n');
    let mut stream = stream?;
    if timeout.is_zero() {
        return None;
    }
    let deadline = Instant::now() + timeout;
    stream.set_read_timeout(Some(timeout)).ok()?;
    stream.set_write_timeout(Some(timeout)).ok()?;
    stream.write_all(line.as_bytes()).ok()?;
    let mut reader = BufReader::new(stream);
    let mut line = Vec::new();
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return None;
        }
        reader.get_ref().set_read_timeout(Some(remaining)).ok()?;
        if !next_line(&mut reader, &mut line) {
            return None;
        }
        let Ok(message) = serde_json::from_slice::<Value>(&line) else {
            continue;
        };
        match message["type"].as_str() {
            Some("reply") if message["id"] == json!(1) => {
                return Some((message["ok"] == json!(true), line))
            }
            Some("error") => return Some((false, line)),
            _ => {}
        }
    }
}

pub(crate) fn show_running_window(
    stream: UnixStream,
    activation_token: Option<&str>,
    desktop_startup_id: Option<&str>,
    timeout: Duration,
) -> Result<(), String> {
    let mut args = Map::new();
    args.insert("visible".into(), json!(true));
    if let Some(token) = activation_token {
        args.insert("activationToken".into(), json!(token));
    }
    if let Some(id) = desktop_startup_id {
        args.insert("desktopStartupId".into(), json!(id));
    }
    match command_reply("set_window_visible", args, Some(stream), timeout) {
        Some((true, _)) => Ok(()),
        Some((false, reply)) => Err(String::from_utf8_lossy(&reply).into_owned()),
        None => Err("control link disconnected or timed out".into()),
    }
}

#[cfg(test)]
mod tests;
