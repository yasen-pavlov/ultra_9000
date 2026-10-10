//! Validated launch data and bounded login-shell PATH resolution.
//! Configured argv is positional data, never interpolated into shell source.

use std::ffi::{CString, OsStr};
use std::io::{BufRead, BufReader};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{mpsc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use crate::config::{StatusSource, ToolSpec};
use crate::omp_link::{self, Credentials};
use alacritty_embed::embed::{self, Launch};
use serde::Serialize;

/// Shells whose `-l -i -c` and `$0` semantics the OMP/Neovim profiles rely on.
const POSIX_SHELLS: &[&str] = &["sh", "bash", "zsh", "ksh", "dash"];

const EXEC_SCRIPT: &str = "exec \"$0\" \"$@\"";

/// Marker preceding the PATH printed by the login-shell probe.
const PATH_MARKER: &str = "__ULTRA9000_LOGIN_PATH__=";

/// Fixed probe script; prints the interactive login shell's PATH after the marker.
const PATH_SCRIPT: &str = "printf '\\n__ULTRA9000_LOGIN_PATH__=%s\\n' \"$PATH\"";

/// Longest time the login-shell PATH probe may take.
const PROBE_TIMEOUT: Duration = Duration::from_secs(15);

/// Time the probe shell gets to exit after printing its PATH.
const PROBE_EXIT_GRACE: Duration = Duration::from_secs(2);

/// PATH of the user's interactive login shell, once successfully probed.
static LOGIN_PATH: Mutex<Option<String>> = Mutex::new(None);
/// Resolve a configured tool off the main thread. An OMP launch with `link` loads Ultra 9000's
/// status extension and gets the link's socket and its item's token.
pub fn prepare(
    tool: &ToolSpec,
    cwd: &str,
    mut args: Vec<String>,
    link: Option<&Credentials>,
) -> Result<Launch, String> {
    let directory = validate_directory(cwd)?;
    let omp = tool.status == StatusSource::Omp;
    let link = link.filter(|_| omp);
    if let Some(link) = link {
        args.splice(0..0, ["-e".to_owned(), link.extension.clone()]);
    }
    let mut launch = if let Some(name) = &tool.command {
        tool_launch(name, &tool.label, directory, args)?
    } else {
        Launch {
            command: if args.is_empty() {
                None
            } else {
                Some((embed::login_shell(), args))
            },
            executable: None,
            working_directory: directory,
            env: Vec::new(),
        }
    };
    launch.env.clone_from(&tool.env);
    if omp {
        // The link variables belong to this launch only; configured values never apply. The
        // token itself stays out of the environment, which OMP's subprocesses inherit: the
        // extension reads it once from the file and deletes the file.
        launch
            .env
            .retain(|(key, _)| key != omp_link::LINK_ENV && key != omp_link::TOKEN_FILE_ENV);
        if let Some(link) = link {
            launch
                .env
                .push((omp_link::LINK_ENV.into(), link.socket.clone()));
            launch
                .env
                .push((omp_link::TOKEN_FILE_ENV.into(), link.token_file.clone()));
        }
        // Ultra 9000 owns attention notifications (notify.* settings) for the agents it
        // monitors; OMP's own ask/completion toasts would duplicate them. A tool-level
        // `env` entry for PI_NOTIFICATIONS still wins.
        if !launch.env.iter().any(|(key, _)| key == "PI_NOTIFICATIONS") {
            launch.env.push(("PI_NOTIFICATIONS".into(), "off".into()));
        }
    }
    Ok(launch)
}

pub fn resolve(tool: &ToolSpec) -> Result<String, String> {
    let Some(name) = &tool.command else {
        return executable_shell(embed::login_shell());
    };
    let shell = supported_shell(&tool.label)?;
    find_executable(name, &login_path(&shell)?)
        .map(|p| p.to_string_lossy().into_owned())
        .ok_or_else(|| "not on login-shell PATH".into())
}

/// The login shell itself, if it is a regular file the user may execute.
fn executable_shell(shell: String) -> Result<String, String> {
    if std::fs::metadata(&shell).is_ok_and(|m| m.is_file()) && executable_by_user(Path::new(&shell))
    {
        Ok(shell)
    } else {
        Err("login shell is not executable".into())
    }
}

pub fn substitute(template: &[String], id: &str, path: &str, cwd: &str) -> Vec<String> {
    template
        .iter()
        .map(|arg| {
            // Single pass: substitutions containing braces are still data.
            let mut out = String::with_capacity(arg.len());
            let mut rest = arg.as_str();
            while !rest.is_empty() {
                if let Some(next) = rest.strip_prefix("{id}") {
                    out.push_str(id);
                    rest = next;
                } else if let Some(next) = rest.strip_prefix("{path}") {
                    out.push_str(path);
                    rest = next;
                } else if let Some(next) = rest.strip_prefix("{cwd}") {
                    out.push_str(cwd);
                    rest = next;
                } else {
                    let ch = rest.chars().next().unwrap();
                    out.push(ch);
                    rest = &rest[ch.len_utf8()..];
                }
            }
            out
        })
        .collect()
}
pub fn default_directory() -> String {
    embed::home_directory().map_or_else(
        || "/".to_owned(),
        |home| home.to_string_lossy().into_owned(),
    )
}

/// Check that `input` names an existing, enterable directory.
///
/// A leading `~` means the home directory; otherwise the path must be absolute. The input is
/// used exactly: whitespace is a legal part of directory names.
pub fn validate_directory(input: &str) -> Result<PathBuf, String> {
    if input.trim().is_empty() {
        return Err("Choose a working directory.".into());
    }

    let path = match input.strip_prefix('~') {
        Some(rest) if rest.is_empty() || rest.starts_with('/') => {
            let home = embed::home_directory().ok_or_else(|| {
                "Your home directory is unknown; enter an absolute path.".to_owned()
            })?;
            home.join(rest.trim_start_matches('/'))
        }
        _ => PathBuf::from(input),
    };

    if !path.is_absolute() {
        return Err(format!("\u{201c}{input}\u{201d} is not an absolute path."));
    }

    // Normalize trailing separators without resolving symlinks, so the recorded directory is
    // the one the user chose.
    let path: PathBuf = path.components().collect();

    let metadata = std::fs::metadata(&path).map_err(|err| match err.kind() {
        std::io::ErrorKind::NotFound => format!("{} does not exist.", path.display()),
        std::io::ErrorKind::PermissionDenied => {
            format!("{} is not accessible (permission denied).", path.display())
        }
        _ => format!("{} cannot be used: {err}.", path.display()),
    })?;
    if !metadata.is_dir() {
        return Err(format!("{} is not a directory.", path.display()));
    }

    if !executable_by_user(&path) {
        return Err(format!(
            "{} is not accessible (permission denied).",
            path.display()
        ));
    }

    Ok(path)
}

fn supported_shell(label: &str) -> Result<String, String> {
    posix_shell(embed::login_shell(), label)
}

fn posix_shell(shell: String, label: &str) -> Result<String, String> {
    let shell_name = Path::new(&shell)
        .file_name()
        .and_then(OsStr::to_str)
        .unwrap_or("");
    if !POSIX_SHELLS.contains(&shell_name) {
        return Err(format!(
            "{label} sessions start through your login shell, but \u{201c}{shell}\u{201d} is not \
             a supported POSIX shell ({}).",
            POSIX_SHELLS.join(", ")
        ));
    }

    Ok(shell)
}

fn tool_launch(
    name: &str,
    label: &str,
    directory: PathBuf,
    extra: Vec<String>,
) -> Result<Launch, String> {
    let shell = supported_shell(label)?;
    let path = login_path(&shell)?;
    tool_launch_via(shell, &path, name, label, directory, extra)
}

/// Start `name` from `path` through `shell`'s `exec "$0" "$@"`, with `extra` as data.
fn tool_launch_via(
    shell: String,
    path: &str,
    name: &str,
    label: &str,
    directory: PathBuf,
    extra: Vec<String>,
) -> Result<Launch, String> {
    let executable = find_executable(name, path)
        .ok_or_else(|| format!("{label} ({name}) was not found on your login shell's PATH."))?;
    let executable = executable.to_string_lossy().into_owned();

    let mut args = ["-l", "-i", "-c", EXEC_SCRIPT, executable.as_str()]
        .map(str::to_owned)
        .to_vec();
    args.extend(extra);
    Ok(Launch {
        command: Some((shell, args)),
        executable: Some(executable),
        working_directory: directory,
        env: Vec::new(),
    })
}

/// First regular file named `name` in the `:`-separated `path` that the current user may
/// execute; entries the user cannot run are skipped, like the shell's own PATH search.
fn find_executable(name: &str, path: &str) -> Option<PathBuf> {
    path.split(':')
        .filter(|dir| dir.starts_with('/'))
        .map(|dir| Path::new(dir).join(name))
        .find(|candidate| {
            std::fs::metadata(candidate).is_ok_and(|metadata| metadata.is_file())
                && executable_by_user(candidate)
        })
}

/// Whether the current user has execute (for directories: search) access to `path`.
fn executable_by_user(path: &Path) -> bool {
    CString::new(path.as_os_str().as_bytes())
        .is_ok_and(|c_path| unsafe { libc::access(c_path.as_ptr(), libc::X_OK) } == 0)
}

/// PATH of the user's interactive login shell, probed once and cached on success.
fn login_path(shell: &str) -> Result<String, String> {
    let mut cached = LOGIN_PATH.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(path) = cached.as_ref() {
        return Ok(path.clone());
    }

    let path = probe_login_path(shell)?;
    *cached = Some(path.clone());
    Ok(path)
}

/// Run the login shell interactively with the fixed [`PATH_SCRIPT`] and read its PATH.
///
/// The probe is our own unreaped child leading its own session (and so its own process
/// group), so a hung probe can be killed without risk of signalling an unrelated process.
/// The new session also has no controlling terminal: when the app itself was started from a
/// terminal, an interactive shell in a background process group of that terminal would be
/// stopped by SIGTTOU/SIGTTIN as soon as it touched it, and the probe would time out.
fn probe_login_path(shell: &str) -> Result<String, String> {
    let failed =
        |reason: String| format!("Unable to read PATH from your login shell ({shell}): {reason}");

    let mut command = Command::new(shell);
    command
        .args(["-l", "-i", "-c", PATH_SCRIPT])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    // SAFETY: `setsid` is async-signal-safe and touches no memory of the parent.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    if let Some(home) = embed::home_directory() {
        command.current_dir(home);
    }

    let mut child = command.spawn().map_err(|err| failed(err.to_string()))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| failed("no output".into()))?;

    // Read on a helper: background jobs started by shell startup files may keep the pipe open
    // after the shell exits, so the reader is abandoned once the marker was seen.
    let (tx, rx) = mpsc::channel();
    let reader = thread::Builder::new()
        .name("login PATH probe".into())
        .spawn(move || {
            for line in BufReader::new(stdout).lines() {
                match line {
                    Ok(line) => {
                        if let Some(path) = line.strip_prefix(PATH_MARKER) {
                            let _ = tx.send(path.to_owned());
                            return;
                        }
                    }
                    Err(_) => return,
                }
            }
        });

    let result = match reader {
        Ok(_) => rx.recv_timeout(PROBE_TIMEOUT).map_err(|err| match err {
            mpsc::RecvTimeoutError::Timeout => failed("it did not finish in time".into()),
            mpsc::RecvTimeoutError::Disconnected => failed("it printed no PATH".into()),
        }),
        Err(err) => Err(failed(err.to_string())),
    };

    reap_probe(&mut child);

    let path = result?;
    if path.is_empty() {
        return Err(failed("it printed an empty PATH".into()));
    }
    Ok(path)
}

/// Wait briefly for the probe shell, then kill its process group and reap it.
fn reap_probe(child: &mut std::process::Child) {
    let deadline = Instant::now() + PROBE_EXIT_GRACE;
    loop {
        match child.try_wait() {
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(20)),
            Ok(None) => break,
            // Exited and reaped, or its state is unknown: never signal it.
            Ok(Some(_)) | Err(_) => return,
        }
    }

    // Still running and unreaped, so the PID and its process group cannot have been reused.
    let pid = child.id() as libc::pid_t;
    unsafe { libc::killpg(pid, libc::SIGKILL) };
    let _ = child.wait();
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PathCheck {
    pub normalized: Option<String>,
    pub error: Option<String>,
    pub suggestion: Option<String>,
}

pub fn path_check(input: &str) -> PathCheck {
    match validate_directory(input) {
        Ok(path) => PathCheck {
            normalized: Some(path.to_string_lossy().into()),
            error: None,
            suggestion: None,
        },
        Err(error) => {
            let path = if input == "~" || input.starts_with("~/") {
                PathBuf::from(default_directory()).join(input.strip_prefix("~/").unwrap_or(""))
            } else {
                PathBuf::from(input)
            };
            let suggestion = path
                .parent()
                .zip(path.file_name())
                .and_then(|(parent, name)| {
                    let name = name.to_string_lossy();
                    let mut candidates: Vec<_> = std::fs::read_dir(parent)
                        .ok()?
                        .filter_map(Result::ok)
                        .filter_map(|entry| {
                            let p = entry.path();
                            validate_directory(&p.to_string_lossy()).ok().map(|p| {
                                (
                                    edit_distance(&name, &entry.file_name().to_string_lossy()),
                                    p,
                                )
                            })
                        })
                        .collect();
                    candidates.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
                    candidates
                        .into_iter()
                        .next()
                        .map(|(_, p)| p.to_string_lossy().into_owned())
                });
            PathCheck {
                normalized: None,
                error: Some(error),
                suggestion,
            }
        }
    }
}

fn edit_distance(a: &str, b: &str) -> usize {
    let b: Vec<_> = b.chars().collect();
    let mut row: Vec<_> = (0..=b.len()).collect();
    for (i, ac) in a.chars().enumerate() {
        let mut previous = row[0];
        row[0] = i + 1;
        for (j, bc) in b.iter().enumerate() {
            let old = row[j + 1];
            row[j + 1] = (row[j] + 1)
                .min(old + 1)
                .min(previous + usize::from(ac != *bc));
            previous = old;
        }
    }
    row[b.len()]
}

#[cfg(test)]
mod tests;
