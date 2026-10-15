use super::*;
use std::{
    io::{Read, Write},
    os::unix::{fs::symlink, net::UnixStream},
    sync::mpsc,
    time::Duration,
};

struct Running {
    dir: tempfile::TempDir,
    link: Link,
    rx: mpsc::Receiver<Delivery>,
}
fn start() -> Running {
    let dir = tempfile::tempdir().unwrap();
    let (tx, rx) = mpsc::channel();
    let link = Link::start(&dir.path().join("ultra-9000"), move |delivery| {
        let _ = tx.send(delivery);
    })
    .unwrap();
    Running { dir, link, rx }
}
fn hello(token: &str) -> String {
    format!(r#"{{"v":1,"type":"hello","token":"{token}","omp":"18.8.7"}}"#)
}
/// A client that sends `lines`, each newline-terminated.
fn connect(link: &Link, lines: &[&str]) -> UnixStream {
    let mut stream = UnixStream::connect(&link.socket).unwrap();
    send(&mut stream, lines);
    stream
}
fn send(stream: &mut UnixStream, lines: &[&str]) {
    for line in lines {
        stream.write_all(line.as_bytes()).unwrap();
        stream.write_all(b"\n").unwrap();
    }
}
fn next(rx: &mpsc::Receiver<Delivery>) -> Delivery {
    rx.recv_timeout(Duration::from_secs(5))
        .expect("nothing was delivered")
}
fn nothing_more(rx: &mpsc::Receiver<Delivery>) {
    assert_eq!(rx.recv_timeout(Duration::from_millis(200)).ok(), None);
}
/// The server closed the connection (it never writes, so a read only sees the end).
fn closed(stream: &mut UnixStream) -> bool {
    closed_within(stream, Duration::from_secs(5))
}
fn closed_within(stream: &mut UnixStream, wait: Duration) -> bool {
    match stream.set_read_timeout(Some(wait)) {
        Ok(()) => {}
        // macOS refuses socket options (EINVAL) once the server has shut the connection down.
        Err(e) if e.kind() == std::io::ErrorKind::InvalidInput => return true,
        Err(e) => panic!("cannot set a read timeout: {e}"),
    }
    matches!(stream.read(&mut [0u8; 1]), Ok(0))
}
fn working() -> Message {
    Message::State(State::Working)
}

#[test]
fn session_names_remove_only_leading_private_use_badges() {
    for glyph in [
        '\u{e000}',
        '\u{f8ff}',
        '\u{f009e}',
        '\u{ffffd}',
        '\u{100000}',
        '\u{10fffd}',
    ] {
        let original = format!("{glyph}\t \u{e001}  NOTIF: Keep awake");
        assert_eq!(clean_session_name(original), "NOTIF: Keep awake");
        let inner = format!("NOTIF: {glyph} Keep awake");
        assert_eq!(clean_session_name(inner.clone()), inner);
        let only = format!("{glyph}  ");
        assert_eq!(clean_session_name(only.clone()), only);
    }
    assert_eq!(clean_session_name("Plain name".into()), "Plain name");
    assert_eq!(clean_session_name(String::new()), "");
}

#[test]
fn status_link_names_are_cleaned_before_delivery() {
    let line = serde_json::json!({"type": "name", "name": "\u{f009e} NOTIF: Keep awake"});
    assert_eq!(
        parse(line.to_string().as_bytes()),
        Some(Message::Name {
            name: Some("NOTIF: Keep awake".into())
        })
    );
}

#[test]
fn start_prepares_a_private_runtime_dir_extension_and_socket() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = dir.path().join("ultra-9000");
    fs::create_dir(&runtime).unwrap();
    fs::set_permissions(&runtime, fs::Permissions::from_mode(0o700)).unwrap();
    // A token file of another instance sharing the directory is left alone.
    fs::write(runtime.join("omp-token-0123"), "other").unwrap();
    // Files of an instance that still runs stay; those of one that is gone are removed.
    let mut child = std::process::Command::new("true").spawn().unwrap();
    let dead = child.id();
    child.wait().unwrap();
    let live = std::os::unix::process::parent_id();
    let instance = |pid: u32| {
        [
            runtime.join(format!("omp-{pid}.sock")),
            runtime.join(format!("control-{pid}.sock")),
            runtime.join(format!("ultra9000-status-{pid}.ts")),
        ]
    };
    for file in instance(dead).iter().chain(&instance(live)) {
        fs::write(file, "").unwrap();
    }
    let link = Link::start(&runtime, |_| {}).unwrap();
    assert_eq!(
        fs::read_to_string(runtime.join("omp-token-0123")).unwrap(),
        "other"
    );
    assert!(instance(dead).iter().all(|f| !f.exists()));
    assert!(instance(live).iter().all(|f| f.exists()));

    let mode = |path: &Path| fs::metadata(path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&runtime), 0o700);
    let extension = runtime.join(format!("ultra9000-status-{}.ts", std::process::id()));
    assert_eq!(fs::read_to_string(&extension).unwrap(), EXTENSION);
    assert_eq!(mode(&extension), 0o600);
    let socket = runtime.join(format!("omp-{}.sock", std::process::id()));
    assert!(socket.exists());

    let first = link.register("item-1").unwrap();
    let second = link.register("item-2").unwrap();
    assert_eq!(first.socket, socket.to_str().unwrap());
    assert_eq!(first.extension, extension.to_str().unwrap());
    for token in [&first.token, &second.token] {
        assert_eq!(token.len(), 32);
        assert!(token
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')));
    }
    assert_ne!(first.token, second.token);
    // The token file's name does not reveal the token.
    for c in [&first, &second] {
        let name = Path::new(&c.token_file)
            .file_name()
            .unwrap()
            .to_str()
            .unwrap();
        assert!(name.starts_with("omp-token-"));
        assert!(!name.contains(&c.token));
        assert_eq!(Path::new(&c.token_file).parent(), Some(runtime.as_path()));
    }
    assert_ne!(first.token_file, second.token_file);

    link.issue(&first).unwrap();
    link.issue(&second).unwrap();
    link.close();
    assert!(!socket.exists());
    assert!(!extension.exists());
    assert!(!Path::new(&first.token_file).exists());
    assert!(!Path::new(&second.token_file).exists());
    assert!(
        runtime.join("omp-token-0123").exists(),
        "only its own tokens"
    );
}

#[test]
fn token_files_are_private_one_shot_and_end_with_their_token() {
    let running = start();
    let first = running.link.register("item-1").unwrap();
    let file = Path::new(&first.token_file);
    assert_eq!(file.parent(), Some(&*running.dir.path().join("ultra-9000")));
    assert!(
        !file.exists(),
        "nothing is written before the launch issues it"
    );
    running.link.issue(&first).unwrap();
    assert_eq!(fs::read_to_string(file).unwrap(), first.token);
    assert_eq!(
        fs::metadata(file).unwrap().permissions().mode() & 0o777,
        0o600
    );

    // A restart of the item replaces the token; the old file goes, and cannot be issued again.
    let second = running.link.register("item-1").unwrap();
    assert!(!file.exists());
    assert!(running.link.issue(&first).is_err());
    assert!(!file.exists());

    // The token is good for connecting even after the extension deleted the file.
    running.link.issue(&second).unwrap();
    fs::remove_file(&second.token_file).unwrap();
    let mut stream = connect(
        &running.link,
        &[
            &hello(&second.token),
            r#"{"type":"state","state":"working"}"#,
        ],
    );
    assert_eq!(next(&running.rx).message, working());
    assert!(!closed_soon(&mut stream));

    // A connection deletes a file the extension left behind.
    let third = running.link.register("item-3").unwrap();
    running.link.issue(&third).unwrap();
    let _stream = connect(
        &running.link,
        &[
            &hello(&third.token),
            r#"{"type":"state","state":"working"}"#,
        ],
    );
    next(&running.rx);
    assert!(!Path::new(&third.token_file).exists());

    // Forgetting an item deletes its unread file, and a later issue writes nothing.
    let fourth = running.link.register("item-4").unwrap();
    running.link.issue(&fourth).unwrap();
    running.link.forget("item-4");
    assert!(!Path::new(&fourth.token_file).exists());
    assert!(running.link.issue(&fourth).is_err());
    assert!(!Path::new(&fourth.token_file).exists());
}

#[test]
fn a_shared_or_redirected_runtime_dir_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let shared = dir.path().join("shared");
    fs::create_dir(&shared).unwrap();
    fs::set_permissions(&shared, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(Link::start(&shared, |_| {}).is_err());
    assert_eq!(fs::read_dir(&shared).unwrap().count(), 0);

    let private = dir.path().join("private");
    fs::create_dir(&private).unwrap();
    fs::set_permissions(&private, fs::Permissions::from_mode(0o700)).unwrap();
    let redirected = dir.path().join("redirected");
    symlink(&private, &redirected).unwrap();
    assert!(Link::start(&redirected, |_| {}).is_err());
}

#[test]
fn messages_after_hello_reach_the_tokens_item_in_order() {
    let running = start();
    let token = running.link.register("item-7").unwrap().token;
    let hello = hello(&token);
    let mut stream = connect(
        &running.link,
        &[
            &hello,
            r#"{"type":"session","id":"s1","file":null,"cwd":"/p"}"#,
            // Unknown types, values and malformed lines are skipped; unknown fields ignored.
            r#"{"type":"stats","tokens":5}"#,
            r#"{"type":"state","state":"thinking"}"#,
            r#"{"type":"state","state":"needs-you","kind":"payment"}"#,
            "not json",
            r#"{"type":"model","model":"anthropic/opus","extra":1}"#,
            r#"{"type":"state","state":"needs-you","kind":"approval"}"#,
        ],
    );
    let first = next(&running.rx);
    assert_eq!(first.item, "item-7");
    assert_eq!(first.message, Message::Session { id: "s1".into() });
    let model = next(&running.rx);
    assert_eq!(model.connection, first.connection);
    assert_eq!(
        model.message,
        Message::Model {
            model: Some("anthropic/opus".into())
        }
    );
    assert_eq!(
        next(&running.rx).message,
        Message::State(State::NeedsYou {
            kind: Attention::Approval
        })
    );
    nothing_more(&running.rx);
    assert!(!closed_soon(&mut stream), "the connection stays open");
}

/// Whether the server closes the connection within a short wait.
fn closed_soon(stream: &mut UnixStream) -> bool {
    closed_within(stream, Duration::from_millis(200))
}

#[test]
fn connections_without_a_valid_hello_are_closed() {
    let running = start();
    let token = running.link.register("item-1").unwrap().token;
    let upper = hello(&token.to_uppercase());
    let wrong_version = hello(&token).replace(r#""v":1"#, r#""v":2"#);
    let unknown = hello(&"0".repeat(32));
    let session = r#"{"type":"session","id":"s1"}"#;
    for first in [session, "{}", &upper, &wrong_version, &unknown] {
        let mut stream = connect(&running.link, &[first, session]);
        assert!(closed(&mut stream), "{first}");
    }
    nothing_more(&running.rx);
}

#[test]
fn an_oversized_line_drops_the_connection() {
    let running = start();
    let token = running.link.register("item-1").unwrap().token;
    let hello = hello(&token);
    // The longest accepted line is exactly 64 KiB.
    let prefix = r#"{"type":"name","name":""#;
    let name = "n".repeat(MAX_LINE - prefix.len() - 2);
    let longest = format!(r#"{prefix}{name}"}}"#);
    assert_eq!(longest.len(), MAX_LINE);
    let mut stream = connect(&running.link, &[&hello, &longest]);
    assert_eq!(
        next(&running.rx).message,
        Message::Name { name: Some(name) }
    );
    // The server may close before the client finished writing; that is the expected outcome.
    let oversized = format!(
        "{}\n{{\"type\":\"state\",\"state\":\"working\"}}\n",
        "x".repeat(MAX_LINE + 1)
    );
    let _ = stream.write_all(oversized.as_bytes());
    assert!(closed(&mut stream));
    nothing_more(&running.rx);
}

#[test]
fn a_new_connection_with_the_same_token_supersedes_the_old_one() {
    let running = start();
    let token = running.link.register("item-1").unwrap().token;
    let hello = hello(&token);
    let state = r#"{"type":"state","state":"working"}"#;
    let mut old = connect(&running.link, &[&hello, state]);
    let first = next(&running.rx);
    let mut new = connect(&running.link, &[&hello, state]);
    let second = next(&running.rx);
    assert_eq!((first.message, second.message), (working(), working()));
    assert!(second.connection > first.connection);
    assert!(closed(&mut old));
    assert!(!closed_soon(&mut new));
    nothing_more(&running.rx);
}

#[test]
fn forgetting_or_registering_again_invalidates_the_old_token() {
    let running = start();
    let old = running.link.register("item-1").unwrap().token;
    let mut stream = connect(&running.link, &[&hello(&old)]);
    // A restart of the same item gets a new token; the old one stops working.
    let new = running.link.register("item-1").unwrap().token;
    assert!(closed(&mut stream));
    let mut stream = connect(&running.link, &[&hello(&old)]);
    assert!(closed(&mut stream));

    let mut stream = connect(
        &running.link,
        &[&hello(&new), r#"{"type":"state","state":"working"}"#],
    );
    assert_eq!(next(&running.rx).message, working());
    running.link.forget("item-1");
    assert!(closed(&mut stream));
    let mut stream = connect(&running.link, &[&hello(&new)]);
    assert!(closed(&mut stream));
    nothing_more(&running.rx);
}

#[test]
fn every_message_kind_decodes() {
    let parsed = |line: &str| parse(line.as_bytes());
    assert_eq!(
        parsed(r#"{"type":"session","id":"01a1","file":"/s/x.jsonl","cwd":"/abs"}"#),
        Some(Message::Session { id: "01a1".into() })
    );
    for (state, expected) in [
        ("working", State::Working),
        ("done", State::Done),
        ("idle", State::Idle),
        ("error", State::Error),
    ] {
        assert_eq!(
            parsed(&format!(r#"{{"type":"state","state":"{state}"}}"#)),
            Some(Message::State(expected))
        );
    }
    assert_eq!(
        parsed(r#"{"type":"state","state":"needs-you","kind":"question"}"#),
        Some(Message::State(State::NeedsYou {
            kind: Attention::Question
        }))
    );
    let job = |n: u64| format!(r#"{{"label":"job {n}","jobType":"bash","startedAt":{n},"pid":9}}"#);
    let jobs: Vec<_> = (0..10).rev().map(job).collect();
    let expected: Vec<_> = (0..8)
        .map(|n| Job {
            label: format!("job {n}"),
            job_type: "bash".into(),
            started_at: n,
        })
        .collect();
    // At most eight jobs are kept, the oldest, oldest first.
    assert_eq!(
        parsed(&format!(
            r#"{{"type":"state","state":"waiting","jobs":[{}]}}"#,
            jobs.join(",")
        )),
        Some(Message::State(State::Waiting { jobs: expected }))
    );
    assert_eq!(
        parsed(r#"{"type":"model","model":null}"#),
        Some(Message::Model { model: None })
    );
    assert_eq!(
        parsed(r#"{"type":"name","name":"QUAKE: Modify Quake ammo in memory"}"#),
        Some(Message::Name {
            name: Some("QUAKE: Modify Quake ammo in memory".into())
        })
    );
    assert_eq!(parsed(&hello(&"a".repeat(32))), None);
    assert_eq!(parsed(r#"{"type":"session"}"#), None);
    assert_eq!(parsed(r#"["state"]"#), None);
}
