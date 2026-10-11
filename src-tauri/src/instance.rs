//! Process-lifetime instance lock and pre-GUI launch handover over the existing control link.
use crate::{control, ctl, omp_link};
use std::{
    ffi::OsStr,
    fs::{self, File, OpenOptions},
    io::{self, Seek, SeekFrom, Write},
    os::{
        fd::AsRawFd,
        unix::{fs::FileExt, fs::MetadataExt, fs::OpenOptionsExt, net::UnixStream},
    },
    path::Path,
    time::{Duration, Instant},
};

const STARTUP_TIMEOUT: Duration = Duration::from_secs(5);
const RETRY_INTERVAL: Duration = Duration::from_millis(100);

struct Key {
    filename: String,
    legacy_fallback: bool,
}

impl Key {
    fn select(debug: bool, override_name: Option<&OsStr>) -> Result<Self, String> {
        let filename = if let Some(name) = override_name {
            let name = name.to_str().filter(|name| {
                (1..=32).contains(&name.len())
                    && name.bytes().all(|b| {
                        b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-'
                    })
            });
            let name = name.ok_or(
                "ULTRA9000_INSTANCE must contain 1..=32 lowercase letters, digits, '_' or '-'.",
            )?;
            format!("instance-{name}.lock")
        } else if debug {
            "instance-debug.lock".into()
        } else {
            "instance.lock".into()
        };
        Ok(Self {
            filename,
            legacy_fallback: !debug && override_name.is_none(),
        })
    }
}

pub enum Launch {
    /// Never unlink the file: all launchers must flock the same inode after upgrades/crashes.
    Owner(File),
    HandedOver(libc::pid_t),
}

fn open_lock(path: &Path) -> Result<File, String> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
        .map_err(|e| format!("cannot open instance lock {}: {e}", path.display()))?;
    if !file.metadata().map_err(|e| e.to_string())?.is_file() {
        return Err(format!(
            "instance lock {} is not a regular file",
            path.display()
        ));
    }
    Ok(file)
}

fn flock(file: &File, operation: libc::c_int) -> io::Result<bool> {
    loop {
        // SAFETY: the fd remains open for the call; flock is supported on both targets.
        if unsafe { libc::flock(file.as_raw_fd(), operation | libc::LOCK_NB) } == 0 {
            return Ok(true);
        }
        let error = io::Error::last_os_error();
        match error.kind() {
            io::ErrorKind::WouldBlock => return Ok(false),
            io::ErrorKind::Interrupted => continue,
            _ => return Err(error),
        }
    }
}

fn acquire(file: &mut File, filename: &str, pid: u32) -> Result<bool, String> {
    if !flock(file, libc::LOCK_EX).map_err(|e| format!("cannot lock {filename}: {e}"))? {
        return Ok(false);
    }
    // Only the owner publishes. Include the key and require a complete record, so a loser
    // never interprets an old plain-pid record or a partially written new record as its owner.
    file.set_len(0)
        .and_then(|()| file.seek(SeekFrom::Start(0)))
        .and_then(|_| writeln!(file, "{pid} {filename}"))
        .map_err(|e| format!("cannot write instance owner to {filename}: {e}"))?;
    Ok(true)
}

fn owner_pid(file: &File, filename: &str) -> Option<libc::pid_t> {
    let mut bytes = [0; 80];
    let count = file.read_at(&mut bytes, 0).ok()?;
    let text = std::str::from_utf8(&bytes[..count])
        .ok()?
        .strip_suffix('\n')?;
    let (pid, key) = text.split_once(' ')?;
    (key == filename)
        .then(|| omp_link::pid_from_digits(pid))
        .flatten()
}

#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    Owner,
    HandedOver(libc::pid_t),
}

/// Each retry first attempts ownership on the same fd, so a quitting/crashed owner can be
/// replaced. Failed/disconnected handovers are retried, never mistaken for a successful launch.
fn wait_for_owner<T>(
    file: &mut File,
    key: &Key,
    own_pid: u32,
    handover: &mut impl FnMut(libc::pid_t, Option<T>, Duration) -> Result<(), String>,
    legacy: &mut impl FnMut() -> Option<(libc::pid_t, T)>,
    now: &mut impl FnMut() -> Duration,
    sleep: &mut impl FnMut(Duration),
) -> Result<Outcome, String> {
    let deadline = now() + STARTUP_TIMEOUT;
    let mut last_error = "owner has not published its control socket".to_owned();
    let mut owned = false;
    loop {
        if !owned {
            owned = acquire(file, &key.filename, own_pid)?;
        }
        let owner = if owned {
            if let Some((pid, stream)) = key.legacy_fallback.then(&mut *legacy).flatten() {
                Some((pid, Some(stream)))
            } else {
                return Ok(Outcome::Owner);
            }
        } else {
            owner_pid(file, &key.filename).map(|pid| (pid, None))
        };
        let remaining = deadline.saturating_sub(now());
        if let Some((pid, stream)) = owner {
            if !remaining.is_zero() {
                match handover(pid, stream, remaining) {
                    Ok(()) => return Ok(Outcome::HandedOver(pid)),
                    Err(error) => last_error = error,
                }
            }
        }
        let remaining = deadline.saturating_sub(now());
        if remaining.is_zero() {
            return Err(format!(
                "Ultra 9000 could not acquire its instance lock or show the running owner's window within 5 s: {last_error}; not starting another instance."
            ));
        }
        sleep(RETRY_INTERVAL.min(remaining));
    }
}

/// New-version debug/isolated instances are never legacy owners. Probe each lock without
/// creating it; a held exclusive flock means its published pid must be excluded from the scan.
fn locked_pids(dir: &Path) -> Vec<libc::pid_t> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name();
            let name = name.to_str()?;
            if !name.starts_with("instance") || !name.ends_with(".lock") {
                return None;
            }
            let file = OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
                .open(entry.path())
                .ok()?;
            if !file.metadata().ok()?.is_file() || flock(&file, libc::LOCK_SH).ok()? {
                return None;
            }
            owner_pid(&file, name)
        })
        .collect()
}

fn legacy_owner(dir: &Path) -> Option<(libc::pid_t, UnixStream)> {
    let excluded = locked_pids(dir);
    ctl::socket_candidates(None, dir)
        .into_iter()
        .find_map(|path| {
            let pid = path
                .file_name()?
                .to_str()
                .and_then(control::socket_pid_digits)
                .and_then(omp_link::pid_from_digits)?;
            if u32::try_from(pid).ok() == Some(std::process::id()) || excluded.contains(&pid) {
                return None;
            }
            UnixStream::connect(path).ok().map(|stream| (pid, stream))
        })
}

fn prepare_runtime_dir(dir: &Path) -> Result<(), String> {
    omp_link::private_dir(dir).map_err(|error| {
        let details = fs::symlink_metadata(dir).map_or_else(
            |e| format!("cannot inspect ownership/mode: {e}"),
            |meta| format!("owner uid {}, mode {:04o}, directory={} (must be your uid and a non-symlink directory with no group/other access)", meta.uid(), meta.mode() & 0o7777, meta.is_dir()),
        );
        // SAFETY: getuid has no preconditions and cannot fail.
        let uid = unsafe { libc::getuid() };
        format!("Cannot use instance runtime directory {}: {error}; {details}; your uid is {uid}. Set XDG_RUNTIME_DIR (Linux) or TMPDIR (macOS) to a private directory owned by you. If this is an abandoned/squatted directory, remove it only after ensuring no Ultra 9000 instance uses it; do not delete a live lock file.", dir.display())
    })
}

pub fn start() -> Result<Launch, String> {
    let override_name = std::env::var_os("ULTRA9000_INSTANCE");
    let key = Key::select(cfg!(debug_assertions), override_name.as_deref())?;
    let dir = omp_link::runtime_dir();
    prepare_runtime_dir(&dir)?;
    let mut file = open_lock(&dir.join(&key.filename))?;
    let started = Instant::now();
    let outcome = wait_for_owner(
        &mut file,
        &key,
        std::process::id(),
        &mut |pid, stream, timeout| {
            let stream = match stream {
                Some(stream) => stream,
                None => UnixStream::connect(dir.join(format!("control-{pid}.sock")))
                    .map_err(|e| format!("pid {pid}: {e}"))?,
            };
            handover(pid, stream, timeout)
        },
        &mut || legacy_owner(&dir),
        &mut || started.elapsed(),
        &mut std::thread::sleep,
    )?;
    Ok(match outcome {
        Outcome::Owner => Launch::Owner(file),
        Outcome::HandedOver(pid) => Launch::HandedOver(pid),
    })
}

fn handover(pid: libc::pid_t, stream: UnixStream, timeout: Duration) -> Result<(), String> {
    let token = std::env::var("XDG_ACTIVATION_TOKEN").ok();
    let startup_id = std::env::var("DESKTOP_STARTUP_ID").ok();
    handover_with_activation(stream, timeout, token.as_deref(), startup_id.as_deref())
        .map_err(|e| format!("pid {pid}: {e}"))
}

fn handover_with_activation(
    stream: UnixStream,
    timeout: Duration,
    activation_token: Option<&str>,
    desktop_startup_id: Option<&str>,
) -> Result<(), String> {
    // Activation is best effort: malformed launcher values must not prevent showing the app.
    ctl::show_running_window(
        stream,
        activation_token.filter(|id| control::valid_activation_id(id)),
        desktop_startup_id.filter(|id| control::valid_activation_id(id)),
        timeout,
    )
}

#[cfg(test)]
mod tests;
