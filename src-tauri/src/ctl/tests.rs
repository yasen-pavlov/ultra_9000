use super::*;
use crate::control::{Command, Server};
use std::{
    cell::RefCell,
    fs::{File, FileTimes},
    os::unix::fs::{symlink, PermissionsExt},
    path::Path,
    sync::Arc,
    time::{Duration, SystemTime},
};

/// A control server in its own runtime dir; `ok` decides each command's reply.
fn server(dir: &Path, ok: bool) -> Server {
    Server::start(
        &dir.join("ultra-9000"),
        move |command, responder| {
            responder.send(if ok {
                Ok([(
                    "seen".to_owned(),
                    json!(matches!(command, Command::RequestQuit {})),
                )]
                .into_iter()
                .collect())
            } else {
                Err(control::Failure {
                    error: "unknown_item",
                    message: "Item item-9 does not exist.".into(),
                })
            })
        },
        || {},
    )
    .unwrap()
}

fn socket(dir: &Path) -> PathBuf {
    dir.join("ultra-9000")
        .join(format!("control-{}.sock", std::process::id()))
}

fn lines(bytes: &[u8]) -> Vec<Value> {
    String::from_utf8_lossy(bytes)
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

#[test]
fn discovery_prefers_the_override_then_the_newest_live_instance() {
    let dir = tempfile::tempdir().unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let mut child = std::process::Command::new("true").spawn().unwrap();
    let dead = child.id();
    child.wait().unwrap();
    let parent = std::os::unix::process::parent_id();
    let me = std::process::id();
    let at = |name: String, age: u64| {
        let path = dir.path().join(name);
        let file = File::create(&path).unwrap();
        file.set_times(FileTimes::new().set_modified(SystemTime::now() - Duration::from_secs(age)))
            .unwrap();
        path
    };
    let older = at(format!("control-{parent}.sock"), 60);
    let newer = at(format!("control-{me}.sock"), 10);
    at(format!("control-{dead}.sock"), 0);
    at(format!("omp-{me}.sock"), 0);
    at("control-x.sock".into(), 0);
    assert_eq!(socket_candidates(None, dir.path()), [newer.clone(), older]);
    assert_eq!(
        socket_candidates(Some("/run/test.sock".into()), dir.path()),
        [PathBuf::from("/run/test.sock")]
    );
    assert_eq!(socket_candidates(Some("".into()), dir.path())[0], newer);
    assert!(socket_candidates(None, &dir.path().join("missing")).is_empty());
}

#[test]
fn discovery_trusts_no_directory_another_user_could_use() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("ultra-9000");
    fs::create_dir(&dir).unwrap();
    File::create(dir.join(format!("control-{}.sock", std::process::id()))).unwrap();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(socket_candidates(None, &dir).len(), 1);
    for mode in [0o755, 0o770, 0o707, 0o1777] {
        fs::set_permissions(&dir, fs::Permissions::from_mode(mode)).unwrap();
        assert!(socket_candidates(None, &dir).is_empty(), "mode {mode:o}");
    }
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
    let link = root.path().join("link");
    symlink(&dir, &link).unwrap();
    assert!(socket_candidates(None, &link).is_empty(), "a symlinked dir");
    let missing = root.path().join("missing");
    assert!(socket_candidates(None, &missing).is_empty());
    assert!(!missing.exists(), "discovery never creates the directory");
}

#[test]
fn one_shot_commands_print_the_reply_and_exit_by_its_outcome() {
    let dir = tempfile::tempdir().unwrap();
    let ok = server(dir.path(), true);
    let mut out = Vec::new();
    let stream = UnixStream::connect(socket(dir.path())).ok();
    assert_eq!(run_command(&mut out, "request_quit", None, stream), 0);
    assert_eq!(
        lines(&out),
        [json!({"type": "reply", "id": 1, "ok": true, "seen": true})]
    );
    ok.close();

    let failing_dir = tempfile::tempdir().unwrap();
    let _failing = server(failing_dir.path(), false);
    let mut out = Vec::new();
    let stream = UnixStream::connect(socket(failing_dir.path())).ok();
    assert_eq!(
        run_command(
            &mut out,
            "select_item",
            Some(r#"{"itemId":"item-9","id":5,"type":"x"}"#),
            stream
        ),
        1
    );
    assert_eq!(
        lines(&out)[0],
        json!({"type": "reply", "id": 1, "ok": false, "error": "unknown_item",
               "message": "Item item-9 does not exist."})
    );

    let mut out = Vec::new();
    assert_eq!(run_command(&mut out, "request_quit", None, None), 2);
    assert_eq!(lines(&out), [json!({"type": "disconnected"})]);
    let mut out = Vec::new();
    let stream = UnixStream::connect(socket(failing_dir.path())).ok();
    assert_eq!(run_command(&mut out, "select_item", Some("[1]"), stream), 2);
    assert!(out.is_empty(), "invalid arguments are a usage error");
}

#[test]
fn instance_handover_uses_the_control_hello_and_forwards_launcher_activation() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("legacy.sock");
    let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
    let owner = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(stream);
        let mut line = Vec::new();
        assert!(next_line(&mut reader, &mut line));
        let handshake: Value = serde_json::from_slice(&line).unwrap();
        assert_eq!(handshake["type"], "hello");
        assert_eq!(handshake["protocol"], PROTOCOL);
        assert_eq!(
            handshake["watch"], false,
            "handover must not withdraw the tray"
        );
        assert!(next_line(&mut reader, &mut line));
        let request: Value = serde_json::from_slice(&line).unwrap();
        assert_eq!(
            request,
            json!({"type":"set_window_visible","id":1,"visible":true,
                   "activationToken":"wayland-token","desktopStartupId":"desktop-id"})
        );
        // A pre-lock v0.3.0 owner ignores new fields but recognizes the existing command.
        reader
            .get_mut()
            .write_all(b"{\"type\":\"reply\",\"id\":1,\"ok\":true,\"visible\":true}\n")
            .unwrap();
    });
    assert_eq!(
        show_running_window(
            UnixStream::connect(path).unwrap(),
            Some("wayland-token"),
            Some("desktop-id"),
            REPLY_TIMEOUT
        ),
        Ok(())
    );
    owner.join().unwrap();
}

#[test]
fn handover_without_tokens_succeeds_and_rejected_commands_are_errors() {
    let dir = tempfile::tempdir().unwrap();
    let owner = Server::start(
        &dir.path().join("ultra-9000"),
        |command, responder| {
            assert_eq!(
                command,
                Command::SetWindowVisible {
                    visible: control::Visibility::Set(true),
                    activation_token: None,
                    desktop_startup_id: None,
                }
            );
            responder.send(Ok([("visible".into(), json!(true))].into_iter().collect()));
        },
        || {},
    )
    .unwrap();
    assert_eq!(
        show_running_window(
            UnixStream::connect(socket(dir.path())).unwrap(),
            None,
            None,
            REPLY_TIMEOUT
        ),
        Ok(())
    );
    owner.close();

    let dir = tempfile::tempdir().unwrap();
    let owner = server(dir.path(), false);
    let error = show_running_window(
        UnixStream::connect(socket(dir.path())).unwrap(),
        None,
        None,
        REPLY_TIMEOUT,
    )
    .unwrap_err();
    assert!(error.contains("unknown_item"));
    owner.close();
    let (client, peer) = UnixStream::pair().unwrap();
    drop(peer);
    assert!(show_running_window(client, None, None, REPLY_TIMEOUT)
        .unwrap_err()
        .contains("disconnected"));
}

#[test]
fn instance_handover_respects_an_exhausted_reply_deadline() {
    let (client, _owner) = UnixStream::pair().unwrap();
    assert!(show_running_window(client, None, None, Duration::ZERO).is_err());
}

/// Collects stdout; runs `on_line` for every completed line and fails the write that
/// completes a line `on_line` answers true for (stdout closed).
struct Stdout<'a> {
    written: Vec<u8>,
    seen: usize,
    on_line: &'a dyn Fn(&Value) -> bool,
}

impl Write for Stdout<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.written.extend_from_slice(buf);
        let mut closed = false;
        while let Some(end) = self.written[self.seen..].iter().position(|b| *b == b'\n') {
            let line: Value =
                serde_json::from_slice(&self.written[self.seen..self.seen + end]).unwrap();
            self.seen += end + 1;
            closed |= (self.on_line)(&line);
        }
        if closed {
            return Err(io::ErrorKind::BrokenPipe.into());
        }
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn watch_relays_lines_reconnects_with_backoff_and_reports_each_drop_once() {
    let first_dir = tempfile::tempdir().unwrap();
    let second_dir = tempfile::tempdir().unwrap();
    let first = Arc::new(server(first_dir.path(), true));
    let second = server(second_dir.path(), true);
    first.publish_raw(r#"{"from":"first"}"#);
    second.publish_raw(r#"{"from":"second"}"#);

    let attempts = RefCell::new(0);
    let mut connect = || {
        *attempts.borrow_mut() += 1;
        match *attempts.borrow() {
            3 => UnixStream::connect(socket(first_dir.path())).ok(),
            7 => UnixStream::connect(socket(second_dir.path())).ok(),
            _ => None,
        }
    };
    let closing = first.clone();
    let on_line = move |line: &Value| match line["from"].as_str() {
        // The app goes away while the client watches.
        Some("first") => {
            closing.close();
            false
        }
        // Stdout closes.
        Some("second") => true,
        _ => false,
    };
    let mut out = Stdout {
        written: Vec::new(),
        seen: 0,
        on_line: &on_line,
    };
    let mut slept = Vec::new();
    let result = run_watch(&mut connect, "noctalia", &mut out, &mut |d| {
        slept.push(d);
        true
    });
    assert_eq!(
        result.unwrap_err().kind(),
        io::ErrorKind::BrokenPipe,
        "watch ends only with stdout"
    );
    let printed: Vec<_> = lines(&out.written)
        .into_iter()
        .map(|l| l.get("from").cloned().unwrap_or(l["type"].clone()))
        .collect();
    assert_eq!(
        printed,
        [
            json!("disconnected"),
            json!("first"),
            json!("disconnected"),
            json!("second")
        ]
    );
    let secs: Vec<_> = slept.iter().map(Duration::as_secs).collect();
    assert_eq!(
        secs,
        [1, 2, 1, 2, 4, 8],
        "backoff restarts after a connection"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn an_orphaned_watcher_exits_while_no_app_runs() {
    let (reader, writer) = io::pipe().unwrap();
    let mut reader = Some(reader);
    let mut out = Vec::new();
    let mut waits = 0;
    let result = run_watch(&mut || None, "noctalia", &mut out, &mut |delay| {
        waits += 1;
        // The shell (stdout's reader) goes away during the third wait.
        if waits == 3 {
            reader = None;
        }
        reader_present(&writer, delay.min(Duration::from_millis(50)))
    });
    assert!(result.is_ok());
    assert!(reader.is_none());
    assert_eq!(waits, 3);
    assert_eq!(lines(&out), [json!({"type": "disconnected"})]);
}

#[cfg(target_os = "linux")]
#[test]
fn waiting_notices_a_closed_reader_at_once_and_otherwise_times_out() {
    use std::time::Instant;
    let (reader, writer) = io::pipe().unwrap();
    let started = Instant::now();
    assert!(reader_present(&writer, Duration::from_millis(100)));
    assert!(started.elapsed() >= Duration::from_millis(90));
    drop(reader);
    let started = Instant::now();
    assert!(!reader_present(&writer, Duration::from_secs(10)));
    assert!(started.elapsed() < Duration::from_secs(1));
}

#[test]
fn reconnect_delays_double_up_to_ten_seconds() {
    let mut delay = RETRY_FIRST;
    let mut delays = vec![delay.as_secs()];
    for _ in 0..5 {
        delay = next_delay(delay);
        delays.push(delay.as_secs());
    }
    assert_eq!(delays, [1, 2, 4, 8, 10, 10]);
}

#[test]
fn only_plain_command_names_are_sent() {
    assert!(is_command_name("select_item"));
    assert!(!is_command_name("watch-me"));
    assert!(!is_command_name(""));
    assert!(!is_command_name("Select"));
}

#[test]
fn document_cli_resolves_paths_and_explicit_selection_follow_flags() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("report.md"), "# Report\n").unwrap();
    let args = document_args("report.md", &[], dir.path()).unwrap();
    assert_eq!(
        args["path"],
        dir.path()
            .join("report.md")
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .as_ref()
    );
    assert_eq!(args["select"], false);
    assert_eq!(args["follow"], false);
    let flags = vec!["--follow".into(), "--select".into()];
    let args = document_args("./report.md", &flags, dir.path()).unwrap();
    assert_eq!(args["select"], true);
    assert_eq!(args["follow"], true);
    assert!(document_args("report.md", &["--unknown".into()], dir.path()).is_err());
    assert!(document_args("missing.md", &[], dir.path()).is_err());
}
