//! OMP status link: Ultra 9000's launch-scoped OMP extension reports each agent's status over
//! a private Unix socket.
//!
//! At startup the app writes the embedded extension into a private runtime directory and
//! listens on `omp-<pid>.sock` there. Every OMP launch gets the extension (`-e`), the socket
//! path and the path of a one-shot token file; the token names exactly one item. The
//! extension reads and deletes the file, so OMP's own subprocesses never learn the token. A
//! connection sends newline-delimited JSON, first a `hello` carrying the token; a newer
//! connection with the same token supersedes the older one. The server never writes, and
//! never logs message contents.
use crate::{platform, status::Attention, workspace};
use serde::Deserialize;
use std::{
    collections::HashMap,
    fs::{self, DirBuilder, File, OpenOptions},
    io::{BufRead, BufReader, Read, Write},
    os::unix::{
        fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
    sync::{Arc, LazyLock, Mutex, PoisonError},
    thread,
    time::Duration,
};

/// The extension OMP loads for every launch with `status = "omp"`.
const EXTENSION: &str = include_str!("../omp/ultra9000-status.ts");
/// Per instance, like the socket: instances of different versions may share the runtime dir.
const EXTENSION_PREFIX: &str = "ultra9000-status-";
/// Environment of an OMP launch: the socket path and the item's token file.
pub const LINK_ENV: &str = "ULTRA9000_OMP_LINK";
pub const TOKEN_FILE_ENV: &str = "ULTRA9000_OMP_TOKEN_FILE";
/// Name prefix of token files in the runtime directory.
const TOKEN_FILE_PREFIX: &str = "omp-token-";
/// Longest accepted line, newline excluded; a longer one drops the connection.
const MAX_LINE: usize = 64 * 1024;
/// Most background jobs kept from one `waiting` state.
const MAX_JOBS: usize = 8;
/// A connection must say hello within this time.
const HELLO_TIMEOUT: Duration = Duration::from_secs(5);
/// `sun_path` is 104 bytes on macOS (108 on Linux), including the terminating NUL.
const MAX_SOCKET_PATH: usize = 103;

/// What an OMP launch needs to reach the link. The token file exists only once issued.
#[derive(Clone, PartialEq, Eq)]
pub struct Credentials {
    pub socket: String,
    pub extension: String,
    pub token_file: String,
    pub token: String,
}

/// A message from an item's OMP, on the connection numbered `connection` (numbers grow with
/// every accepted hello, so a newer connection always has a larger one).
#[derive(Debug, PartialEq)]
pub struct Delivery {
    pub item: String,
    pub connection: u64,
    pub message: Message,
}

#[derive(Debug, PartialEq, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Message {
    /// The session OMP is in now.
    Session {
        id: String,
    },
    State(State),
    /// `<provider>/<id>`, or none.
    Model {
        model: Option<String>,
    },
    /// The OMP session name, or none.
    Name {
        name: Option<String>,
    },
}

/// The main agent's turn state, as the extension reports it.
#[derive(Debug, PartialEq, Deserialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub enum State {
    Working,
    NeedsYou {
        kind: Attention,
    },
    /// The turn ended but background jobs still run: at most [`MAX_JOBS`], oldest first.
    Waiting {
        #[serde(default)]
        jobs: Vec<Job>,
    },
    Done,
    Idle,
    Error,
}

/// A running background job: label, OMP job type and start time (epoch ms).
#[derive(Debug, PartialEq, Deserialize)]
pub struct Job {
    pub label: String,
    #[serde(rename = "jobType")]
    pub job_type: String,
    #[serde(rename = "startedAt")]
    pub started_at: u64,
}

#[derive(Deserialize)]
struct Hello {
    v: u32,
    #[serde(rename = "type")]
    kind: String,
    token: String,
}

/// Decode one line after the hello. Unknown types and values are skipped (`None`).
pub fn parse(line: &[u8]) -> Option<Message> {
    let mut message = serde_json::from_slice::<Message>(line).ok()?;
    if let Message::State(State::Waiting { jobs }) = &mut message {
        jobs.sort_by_key(|job| job.started_at);
        jobs.truncate(MAX_JOBS);
    }
    if let Message::Name { name: Some(name) } = &mut message {
        *name = clean_session_name(std::mem::take(name));
    }
    Some(message)
}

/// OMP titles can begin with a Nerd Font badge. Native text surfaces use their own
/// indicators; remove only that leading private-use prefix, never meaningful inner text.
pub fn clean_session_name(mut name: String) -> String {
    let private = |c: char| matches!(c, '\u{e000}'..='\u{f8ff}' | '\u{f0000}'..='\u{ffffd}' | '\u{100000}'..='\u{10fffd}');
    if name.starts_with(private) {
        let rest = name.trim_start_matches(|c: char| private(c) || c.is_whitespace());
        if !rest.is_empty() {
            let prefix = name.len() - rest.len();
            name.drain(..prefix);
        }
    }
    name
}

/// The token of a valid version-1 hello.
fn parse_hello(line: &[u8]) -> Option<String> {
    let hello = serde_json::from_slice::<Hello>(line).ok()?;
    (hello.v == 1
        && hello.kind == "hello"
        && hello.token.len() == 32
        && hello
            .token
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
    .then_some(hello.token)
}

/// A running link: its directory, socket, the extension it wrote and the token registry.
pub struct Link {
    dir: PathBuf,
    socket: PathBuf,
    extension: PathBuf,
    registry: Arc<Registry>,
}

impl Link {
    /// Prepare `dir` (private to this user), remove the socket and extension of instances that
    /// no longer run, write the extension and listen on the socket. Token files of other runs
    /// are left alone: another instance may share `dir`. Each delivery is handed to `deliver`
    /// from a connection thread.
    pub fn start(
        dir: &Path,
        deliver: impl Fn(Delivery) + Send + Sync + 'static,
    ) -> Result<Self, String> {
        private_dir(dir)?;
        remove_dead_instances(dir);
        let extension = dir.join(format!("{EXTENSION_PREFIX}{}.ts", std::process::id()));
        write_atomically(&extension, EXTENSION.as_bytes())
            .map_err(|e| format!("cannot write {}: {e}", extension.display()))?;
        let socket = dir.join(format!("omp-{}.sock", std::process::id()));
        if dir.to_str().is_none() {
            return Err(format!("{} is not valid UTF-8", dir.display()));
        }
        if socket.as_os_str().len() > MAX_SOCKET_PATH {
            return Err(format!("socket path {} is too long", socket.display()));
        }
        match fs::remove_file(&socket) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("cannot remove stale {}: {e}", socket.display())),
        }
        let listener = UnixListener::bind(&socket)
            .map_err(|e| format!("cannot listen on {}: {e}", socket.display()))?;
        fs::set_permissions(&socket, fs::Permissions::from_mode(0o600))
            .map_err(|e| format!("cannot restrict {}: {e}", socket.display()))?;
        let registry = Arc::new(Registry::default());
        let accepting = registry.clone();
        let deliver: Arc<dyn Fn(Delivery) + Send + Sync> = Arc::new(deliver);
        thread::Builder::new()
            .name("u9k-omp-link".into())
            .spawn(move || {
                for stream in listener.incoming() {
                    let Ok(stream) = stream else { continue };
                    let registry = accepting.clone();
                    let deliver = deliver.clone();
                    if let Err(e) = thread::Builder::new()
                        .name("u9k-omp-conn".into())
                        .spawn(move || serve(stream, &registry, &*deliver))
                    {
                        log::warn!("OMP status link: cannot serve a connection: {e}");
                    }
                }
            })
            .map_err(|e| format!("cannot start the listener: {e}"))?;
        Ok(Self {
            dir: dir.to_owned(),
            socket,
            extension,
            registry,
        })
    }

    /// A fresh token for `item`, replacing (and disconnecting) any earlier one. Its file is
    /// written by [`Link::issue`], off the main thread.
    pub fn register(&self, item: &str) -> Option<Credentials> {
        let (token, name) = match random_hex().and_then(|t| Ok((t, random_hex()?))) {
            Ok(random) => random,
            Err(e) => {
                log::warn!("OMP status link: no random token: {e}");
                return None;
            }
        };
        let token_file = self.dir.join(format!("{TOKEN_FILE_PREFIX}{name}"));
        self.registry
            .register(item, token.clone(), token_file.clone());
        Some(Credentials {
            socket: self.socket.to_string_lossy().into_owned(),
            extension: self.extension.to_string_lossy().into_owned(),
            token_file: token_file.to_string_lossy().into_owned(),
            token,
        })
    }

    /// Write the token file of registered `credentials` (mode 0600) before the launch. Fails
    /// if the token was forgotten meanwhile; then no file is left behind.
    pub fn issue(&self, credentials: &Credentials) -> Result<(), String> {
        self.registry.issue(credentials)
    }

    /// Invalidate `item`'s token, delete its file if still there and close its connection.
    pub fn forget(&self, item: &str) {
        self.registry.forget(item);
    }

    /// Remove the socket, the extension and every remaining token file; nothing can connect
    /// afterwards.
    pub fn close(&self) {
        let _ = fs::remove_file(&self.socket);
        let _ = fs::remove_file(&self.extension);
        self.registry.clear();
    }
}

#[derive(Default)]
struct Registry(Mutex<Tokens>);

#[derive(Default)]
struct Tokens {
    entries: HashMap<String, Entry>,
    last_connection: u64,
}

struct Entry {
    item: String,
    /// The one-shot token file; the extension deletes it once read.
    file: PathBuf,
    /// The current connection and a handle to shut it down.
    connection: Option<(u64, UnixStream)>,
}

impl Entry {
    fn disconnect(&mut self) {
        if let Some((_, stream)) = self.connection.take() {
            let _ = stream.shutdown(std::net::Shutdown::Both);
        }
    }
    /// End this token: close its connection and delete its file if the extension did not.
    fn end(&mut self) {
        self.disconnect();
        let _ = fs::remove_file(&self.file);
    }
}

impl Registry {
    fn tokens(&self) -> std::sync::MutexGuard<'_, Tokens> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn register(&self, item: &str, token: String, file: PathBuf) {
        let mut tokens = self.tokens();
        Self::end_item(&mut tokens, item);
        tokens.entries.insert(
            token,
            Entry {
                item: item.to_owned(),
                file,
                connection: None,
            },
        );
    }

    /// Holds the lock while writing, so a concurrent [`Registry::forget`] either prevents the
    /// file or deletes it.
    fn issue(&self, credentials: &Credentials) -> Result<(), String> {
        let tokens = self.tokens();
        let entry = tokens
            .entries
            .get(&credentials.token)
            .filter(|entry| entry.file.as_os_str() == credentials.token_file.as_str())
            .ok_or("the launch was cancelled")?;
        write_atomically(&entry.file, credentials.token.as_bytes())
            .map_err(|e| format!("cannot write the token file: {e}"))
    }

    fn forget(&self, item: &str) {
        Self::end_item(&mut self.tokens(), item);
    }

    fn clear(&self) {
        for (_, mut entry) in self.tokens().entries.drain() {
            entry.end();
        }
    }

    fn end_item(tokens: &mut Tokens, item: &str) {
        tokens.entries.retain(|_, entry| {
            let keep = entry.item != item;
            if !keep {
                entry.end();
            }
            keep
        });
    }

    /// Accept `stream` as `token`'s connection, superseding an earlier one. The token file
    /// has served its purpose and is deleted in case the extension could not.
    fn connect(&self, token: &str, stream: &UnixStream) -> Option<(String, u64)> {
        let handle = stream.try_clone().ok()?;
        let mut tokens = self.tokens();
        let number = tokens.last_connection + 1;
        let entry = tokens.entries.get_mut(token)?;
        entry.disconnect();
        let _ = fs::remove_file(&entry.file);
        entry.connection = Some((number, handle));
        let item = entry.item.clone();
        tokens.last_connection = number;
        Some((item, number))
    }

    fn is_current(&self, token: &str, connection: u64) -> bool {
        self.tokens()
            .entries
            .get(token)
            .and_then(|entry| entry.connection.as_ref())
            .is_some_and(|(number, _)| *number == connection)
    }

    fn disconnect(&self, token: &str, connection: u64) {
        if let Some(entry) = self.tokens().entries.get_mut(token) {
            if entry
                .connection
                .as_ref()
                .is_some_and(|(number, _)| *number == connection)
            {
                entry.disconnect();
            }
        }
    }
}

/// Read one connection until it ends, is superseded or misbehaves.
fn serve(stream: UnixStream, registry: &Registry, deliver: &dyn Fn(Delivery)) {
    let Ok(reader) = stream.try_clone() else {
        return;
    };
    let mut reader = BufReader::new(reader);
    let mut line = Vec::new();
    let _ = stream.set_read_timeout(Some(HELLO_TIMEOUT));
    let accepted = read_line(&mut reader, &mut line)
        .then(|| parse_hello(&line))
        .flatten()
        .and_then(|token| Some((registry.connect(&token, &stream)?, token)));
    let Some(((item, connection), token)) = accepted else {
        log::debug!("OMP status link: rejected a connection without a valid hello");
        return;
    };
    if stream.set_read_timeout(None).is_ok() {
        while read_line(&mut reader, &mut line) && registry.is_current(&token, connection) {
            if let Some(message) = parse(&line) {
                deliver(Delivery {
                    item: item.clone(),
                    connection,
                    message,
                });
            }
        }
    }
    registry.disconnect(&token, connection);
}

/// Read the next complete line into `line` (without its newline). False at the end of the
/// stream, on a read error or timeout, and for a line longer than [`MAX_LINE`].
fn read_line(reader: &mut impl BufRead, line: &mut Vec<u8>) -> bool {
    line.clear();
    match reader.take(MAX_LINE as u64 + 1).read_until(b'\n', line) {
        Ok(_) if line.last() == Some(&b'\n') => {
            line.pop();
            true
        }
        _ => false,
    }
}

/// Create `dir` with mode 0700, or accept an existing directory only if this user owns it and
/// nobody else may use it. Shared with the control link (`control.rs`).
pub(crate) fn private_dir(dir: &Path) -> Result<(), String> {
    match DirBuilder::new().mode(0o700).create(dir) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(format!("cannot create {}: {e}", dir.display())),
    }
    if !is_private_dir(dir) {
        return Err(format!(
            "{} is not a private directory of this user",
            dir.display()
        ));
    }
    Ok(())
}

/// Whether `dir` is an existing directory (not a symlink) that this user owns and nobody else
/// may use. The control link's CLI trusts no socket outside such a directory.
pub(crate) fn is_private_dir(dir: &Path) -> bool {
    let Ok(meta) = fs::symlink_metadata(dir) else {
        return false;
    };
    // SAFETY: getuid has no preconditions and cannot fail.
    let uid = unsafe { libc::getuid() };
    meta.is_dir() && meta.uid() == uid && meta.mode() & 0o077 == 0
}

/// Remove the sockets and extension of instances whose process is gone (killed or crashed
/// before their own cleanup). A reused pid only keeps an old file, never removes a live one.
fn remove_dead_instances(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(pid) = name.to_str().and_then(instance_pid) else {
            continue;
        };
        if !process_alive(pid) {
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// Whether process `pid` may still exist; only a definite "no such process" counts as gone.
pub(crate) fn process_alive(pid: libc::pid_t) -> bool {
    // SAFETY: signal 0 only checks that the process exists; nothing is delivered.
    let signalled = unsafe { libc::kill(pid, 0) } == 0;
    signalled || std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
}

/// The app pid in an instance's file name: `omp-<pid>.sock`, `control-<pid>.sock` or
/// `ultra9000-status-<pid>.ts`.
fn instance_pid(name: &str) -> Option<libc::pid_t> {
    let digits = name
        .strip_prefix("omp-")
        .and_then(|rest| rest.strip_suffix(".sock"))
        .or_else(|| crate::control::socket_pid_digits(name))
        .or_else(|| {
            name.strip_prefix(EXTENSION_PREFIX)
                .and_then(|rest| rest.strip_suffix(".ts"))
        })?;
    pid_from_digits(digits)
}

/// A positive pid written as plain decimal digits.
pub(crate) fn pid_from_digits(digits: &str) -> Option<libc::pid_t> {
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok().filter(|pid| *pid > 0)
}

/// Replace `path` with `contents` (mode 0600) through a rename, so OMP never reads half a file.
fn write_atomically(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let temp = path.with_file_name(format!(".{name}.{}.tmp", std::process::id()));
    let result = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&temp)
        .and_then(|mut file| file.write_all(contents))
        .and_then(|()| fs::rename(&temp, path));
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

/// 32 lowercase hex characters from the system CSPRNG.
fn random_hex() -> std::io::Result<String> {
    let mut bytes = [0u8; 16];
    File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// Linux: `$XDG_RUNTIME_DIR/ultra-9000`, else a per-user directory in the temp dir; macOS:
/// the per-user `$TMPDIR`. Shared with the control link and its CLI client.
pub(crate) fn runtime_dir() -> PathBuf {
    #[cfg(target_os = "linux")]
    {
        if let Some(dir) = std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .filter(|dir| dir.is_absolute())
        {
            return dir.join("ultra-9000");
        }
        // SAFETY: getuid has no preconditions and cannot fail.
        let uid = unsafe { libc::getuid() };
        std::env::temp_dir().join(format!("ultra-9000-{uid}"))
    }
    #[cfg(target_os = "macos")]
    std::env::temp_dir().join("ultra-9000")
}

/// The app's link, started by [`start`] during setup; `None` if it is unavailable, and then
/// OMP items show no status.
static LINK: LazyLock<Option<Link>> = LazyLock::new(|| {
    Link::start(&runtime_dir(), |delivery| {
        platform::defer_on_main(Box::new(move || workspace::omp_link(delivery)));
    })
    .inspect_err(|e| log::warn!("OMP status link unavailable: {e}"))
    .ok()
});

/// Start the app's link before any item launches.
pub fn start() {
    LazyLock::force(&LINK);
}

/// Credentials for a new OMP launch of `item`, if the link runs.
pub fn register(item: &str) -> Option<Credentials> {
    LINK.as_ref()?.register(item)
}

/// Write the token file of `credentials` right before their launch (off the main thread).
pub fn issue(credentials: &Credentials) -> Result<(), String> {
    LINK.as_ref()
        .ok_or("the OMP status link is not running")?
        .issue(credentials)
}

/// Invalidate `item`'s token: its process ended or the item closed.
pub fn forget(item: &str) {
    if let Some(link) = &*LINK {
        link.forget(item);
    }
}

/// Remove the socket and remaining token files at exit.
pub fn shutdown() {
    if let Some(link) = &*LINK {
        link.close();
    }
}

#[cfg(test)]
mod tests;
