use super::*;
use crate::workspace::{
    begin_launch, snapshot,
    test_support::{install, poll_foreground_jobs, FakeHost},
};
use alacritty_embed::embed::SessionKey;
use std::{os::unix::fs::PermissionsExt, sync::mpsc};

// ---------------------------------------------------------------------------------------------
// Snapshot projection
// ---------------------------------------------------------------------------------------------

fn launch(tool: &str, project: &str) -> String {
    begin_launch(
        tool.into(),
        project.into(),
        project.into(),
        None,
        None,
        None,
        None,
    )
    .unwrap()
}
fn running(host: &FakeHost, tool: &str, project: &str) -> (String, SessionKey) {
    let id = launch(tool, project);
    let key = host.complete(&id);
    (id, key)
}
fn context() -> Context {
    Context {
        window_visible: true,
        next_waiting: None,
        tray: TrayShow::Auto,
        home: Some("/home/me".into()),
    }
}
/// The projection of the current workspace, as JSON.
fn projected(context: &Context) -> Value {
    let state = snapshot().unwrap();
    serde_json::from_str(&encode(project(&state, context))).unwrap()
}
fn items(snapshot: &Value) -> Vec<Value> {
    snapshot["items"].as_array().unwrap().clone()
}
fn item<'a>(snapshot: &'a Value, id: &str) -> &'a Value {
    snapshot["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["id"] == id)
        .unwrap_or_else(|| panic!("{id} is not in the snapshot"))
}
const WORKING: &str = r#"{"type":"state","state":"working"}"#;
const DONE: &str = r#"{"type":"state","state":"done"}"#;
const APPROVAL: &str = r#"{"type":"state","state":"needs-you","kind":"approval"}"#;

#[test]
fn items_follow_the_sidebar_order_by_project_then_launch() {
    let host = install();
    let (a, _) = running(&host, "shell", "/home/me/work/one");
    let (b, _) = running(&host, "shell", "/home/me/work/two");
    let (c, _) = running(&host, "omp", "/home/me/work/one");
    let d = launch("neovim", "/elsewhere/three");
    let s = projected(&context());
    let order: Vec<_> = items(&s).iter().map(|i| i["id"].clone()).collect();
    assert_eq!(order, [json!(a), json!(c), json!(b), json!(d)]);
    assert_eq!(
        s["projects"],
        json!([
            {"id": "/home/me/work/one", "name": "one", "parentPath": "~/work", "pinned": false,
             "defaultTool": "omp", "itemIds": [a, c]},
            {"id": "/home/me/work/two", "name": "two", "parentPath": "~/work", "pinned": false,
             "defaultTool": "omp", "itemIds": [b]},
            {"id": "/elsewhere/three", "name": "three", "parentPath": "/elsewhere",
             "pinned": false, "defaultTool": "omp", "itemIds": [d]},
        ])
    );
    assert_eq!(s["app"]["pid"], std::process::id());
    assert_eq!(s["app"]["windowVisible"], true);
    assert_eq!(s["tray"], json!({"show": "auto"}));
    assert_eq!(s["waiting"], json!({"nextItemId": null}));
    let neovim = item(&s, &d);
    assert_eq!(
        (&neovim["kind"], &neovim["tool"], &neovim["toolLabel"]),
        (&json!("neovim"), &json!("neovim"), &json!("Neovim"))
    );
    assert_eq!(item(&s, &a)["kind"], "shell");
    assert_eq!(item(&s, &c)["kind"], "agent");
    assert_eq!(item(&s, &c)["toolGlyph"], "π");
}

#[test]
fn states_map_process_and_activity_without_inference() {
    let host = install();
    host.focused.set(false);
    let starting = launch("omp", "/p");
    let (needs, _) = running(&host, "omp", "/p");
    host.omp(&needs, APPROVAL);
    let (turn, _) = running(&host, "omp", "/p");
    host.omp(&turn, WORKING);
    host.omp(&turn, DONE);
    let (job, _) = running(&host, "omp", "/p");
    host.omp(&job, WORKING);
    host.omp(
        &job,
        &json!({"type": "state", "state": "waiting",
                "jobs": [{"label": "bun run test", "jobType": "bash", "startedAt": 100}]})
        .to_string(),
    );
    let (working, _) = running(&host, "omp", "/p");
    host.omp(&working, WORKING);
    let (unmonitored, _) = running(&host, "claude", "/p");
    let (crashed, crashed_key) = running(&host, "claude", "/p");
    host.exit(crashed_key, Some(1));
    let failed = launch("shell", "/p");
    host.fail(&failed, "not on PATH");
    let (exited, exited_key) = running(&host, "shell", "/p");
    host.exit(exited_key, Some(2));
    let (busy, busy_key) = running(&host, "shell", "/p");
    host.set_job(busy_key, Some((900, "cargo")));
    poll_foreground_jobs();
    poll_foreground_jobs();

    let s = projected(&context());
    let state = |id: &str| item(&s, id)["state"].as_str().unwrap().to_owned();
    assert_eq!(state(&starting), "starting");
    assert_eq!(state(&needs), "needs-you");
    assert_eq!(item(&s, &needs)["attention"], "approval");
    assert_eq!(state(&turn), "your-turn");
    assert_eq!(
        item(&s, &turn)["unread"],
        true,
        "unseen: the window has no focus"
    );
    assert_eq!(state(&job), "on-job");
    assert_eq!(
        item(&s, &job)["jobs"],
        json!([{"kind": "bash", "label": "bun run test", "startedAtMs": 100}])
    );
    assert_eq!(state(&working), "working");
    assert_eq!(item(&s, &working)["jobs"], json!([]));
    assert_eq!(state(&unmonitored), "unknown");
    assert_eq!(state(&crashed), "error");
    assert_eq!(item(&s, &crashed)["exitCode"], 1);
    assert_eq!(state(&failed), "error");
    assert_eq!(state(&exited), "exited");
    assert_eq!(item(&s, &exited)["exitCode"], 2);
    assert_eq!(
        state(&busy),
        "unknown",
        "a busy shell has no activity of its own"
    );
    assert_eq!(item(&s, &busy)["foregroundJob"]["command"], "cargo");
    assert_eq!(item(&s, &working)["foregroundJob"], Value::Null);
    for i in items(&s) {
        assert_eq!(i["selected"], i["id"] == json!(busy), "only the shown item");
        if i["id"] != json!(needs) {
            assert_eq!(i["attention"], Value::Null);
        }
    }
}

#[test]
fn waiting_window_and_tray_come_from_the_context() {
    install();
    let s = projected(&Context {
        window_visible: false,
        next_waiting: Some("item-7".into()),
        tray: TrayShow::Never,
        home: None,
    });
    assert_eq!(s["app"]["windowVisible"], false);
    assert_eq!(s["waiting"]["nextItemId"], "item-7");
    assert_eq!(s["tray"]["show"], "never");
}

#[test]
fn fifteen_items_fit_far_below_the_line_limit_without_icons() {
    let host = install();
    for n in 0..15 {
        let project = format!("/home/me/Workbench/Projects/personal/project-{}", n % 4);
        let (id, _) = running(&host, if n % 3 == 0 { "shell" } else { "omp" }, &project);
        if n % 3 != 0 {
            host.omp(&id, WORKING);
            host.omp(
                &id,
                &json!({"type": "name", "name": "Fix the very long-winded session title of this agent row number"}).to_string(),
            );
        }
    }
    let mut state = snapshot().unwrap();
    state.provider_icons.insert(
        "anthropic".into(),
        format!("data:image/svg+xml;base64,{}", "A".repeat(50_000)),
    );
    let content = encode(project(
        &state,
        &Context {
            next_waiting: Some(state.items[1].id.clone()),
            ..context()
        },
    ));
    assert_eq!(
        serde_json::from_str::<Value>(&content).unwrap()["items"]
            .as_array()
            .unwrap()
            .len(),
        15
    );
    assert!(content.len() < 16 * 1024, "{} bytes", content.len());
    assert!(!content.contains("data:"));
    assert!(!content.contains("\"icon"));
}

#[test]
fn an_oversized_workspace_still_fits_one_line_and_stays_consistent() {
    let host = install();
    let deep = format!("/{}", "deep/".repeat(80));
    for n in 0..200 {
        running(&host, "shell", &format!("{deep}project-{n}"));
    }
    let content = encode(project(&snapshot().unwrap(), &context()));
    assert!(content.len() <= MAX_LINE - 96);
    let s: Value = serde_json::from_str(&content).unwrap();
    let listed: Vec<_> = s["projects"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|p| p["itemIds"].as_array().unwrap().clone())
        .collect();
    let ids: Vec<_> = items(&s).iter().map(|i| i["id"].clone()).collect();
    assert_eq!(listed, ids, "dropped items leave no dangling ids");
    assert!(ids.len() < 200 && !ids.is_empty());
}

#[test]
fn long_titles_are_clipped() {
    let host = install();
    let (id, key) = running(&host, "shell", "/p");
    host.retitle(key, &"x".repeat(1000));
    let s = projected(&context());
    let title = item(&s, &id)["title"].as_str().unwrap().to_owned();
    assert_eq!(title, format!("{}…", "x".repeat(MAX_TEXT)));
}

#[test]
fn models_use_the_sidebars_short_names() {
    for (model, short) in [
        ("anthropic/claude-opus-5-5", "opus 5.5"),
        ("claude-sonnet-4-6", "sonnet 4.6"),
        ("openai-codex/gpt-6.1-sol", "gpt 6.1 sol"),
        ("openrouter/moonshotai/kimi-k3", "kimi k3"),
        ("claude-opus-4-1-20250805", "opus 4.1 20250805"),
        ("claude-x-1-2-3", "x 1.2 3"),
        ("model-10-20", "model 10.20"),
        ("a-1-2b", "a 1 2b"),
        ("no-dashes-here", "no dashes here"),
        ("gemini-3-pro", "gemini 3 pro"),
    ] {
        assert_eq!(model_short(model), short, "{model}");
    }
}

#[test]
fn parent_paths_abbreviate_only_the_home_directory() {
    let home = Some(Path::new("/home/me"));
    assert_eq!(tilde(Path::new("/home/me"), home), "~");
    assert_eq!(tilde(Path::new("/home/me/a/b"), home), "~/a/b");
    assert_eq!(tilde(Path::new("/home/meow"), home), "/home/meow");
    assert_eq!(tilde(Path::new("/srv"), Some(Path::new("/"))), "/srv");
    assert_eq!(tilde(Path::new("/srv"), None), "/srv");
}

// ---------------------------------------------------------------------------------------------
// Protocol
// ---------------------------------------------------------------------------------------------

struct Running {
    _dir: tempfile::TempDir,
    server: Server,
    presence: mpsc::Receiver<Instant>,
}

/// A server whose commands reply at once with their decoded form.
fn start() -> Running {
    let dir = tempfile::tempdir().unwrap();
    let (tx, presence) = mpsc::channel();
    // `Sender` is `Sync`: the callback runs on connection threads.
    let server = Server::start(
        &dir.path().join("ultra-9000"),
        |command, responder| responder.send(Ok(done([("command", json!(format!("{command:?}")))]))),
        move || {
            let _ = tx.send(Instant::now());
        },
    )
    .unwrap();
    Running {
        _dir: dir,
        server,
        presence,
    }
}

struct Client {
    stream: UnixStream,
    reader: BufReader<UnixStream>,
}

impl Client {
    fn connect(server: &Server, hello: &str) -> Self {
        let stream = UnixStream::connect(&server.socket).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let mut client = Self {
            reader: BufReader::new(stream.try_clone().unwrap()),
            stream,
        };
        client.send(hello);
        client
    }
    fn watch(server: &Server) -> Self {
        Self::connect(
            server,
            r#"{"type":"hello","protocol":1,"client":"test","version":"0","watch":true}"#,
        )
    }
    fn once(server: &Server) -> Self {
        Self::connect(server, r#"{"type":"hello","protocol":1,"client":"test"}"#)
    }
    fn send(&mut self, line: &str) {
        self.stream.write_all(line.as_bytes()).unwrap();
        self.stream.write_all(b"\n").unwrap();
    }
    /// The next line, `None` once the server closed the connection.
    fn next(&mut self) -> Option<Value> {
        let mut line = String::new();
        match self.reader.read_line(&mut line) {
            Ok(0) | Err(_) => None,
            Ok(_) => Some(serde_json::from_str(&line).unwrap()),
        }
    }
    fn expect(&mut self) -> Value {
        self.next().expect("the connection closed")
    }
}

#[test]
fn the_socket_is_private_per_instance_and_removed_on_close() {
    let running = start();
    let socket = &running.server.socket;
    assert_eq!(
        socket.file_name().unwrap().to_str().unwrap(),
        format!("control-{}.sock", std::process::id())
    );
    assert_eq!(
        fs::metadata(socket).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(
        fs::metadata(socket.parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    let mut client = Client::watch(&running.server);
    running
        .presence
        .recv_timeout(Duration::from_secs(5))
        .unwrap();
    running.server.close();
    assert!(!socket.exists());
    assert_eq!(client.next(), None, "open connections end too");
}

#[test]
fn a_watcher_gets_the_snapshot_at_once_and_replies_to_its_commands() {
    let running = start();
    running.server.hub.publish(r#"{"items":[]}"#.into());
    let mut client = Client::watch(&running.server);
    assert_eq!(
        client.expect(),
        json!({"type": "snapshot", "protocol": 1, "revision": 1, "items": []})
    );
    client.send(r#"{"type":"select_item","id":7,"itemId":"item-3","show":false,"extra":1}"#);
    assert_eq!(
        client.expect(),
        json!({"type": "reply", "id": 7, "ok": true,
               "command": "SelectItem { item_id: \"item-3\", show: false }"})
    );
    running.server.hub.publish(r#"{"items":[1]}"#.into());
    assert_eq!(client.expect()["revision"], 2);
    running.server.hub.publish(r#"{"items":[1]}"#.into());
    client.send(r#"{"type":"jump_to_waiting","id":8}"#);
    let reply = client.expect();
    assert_eq!(
        (&reply["type"], &reply["id"]),
        (&json!("reply"), &json!(8)),
        "an unchanged snapshot is not sent again"
    );
}

#[test]
fn every_command_decodes_its_arguments_and_defaults() {
    let decode = |line: &str| parse_command(line.as_bytes()).map(|(command, _)| command);
    assert_eq!(
        decode(r#"{"type":"select_item","id":1,"itemId":"item-1"}"#),
        Ok(Command::SelectItem {
            item_id: "item-1".into(),
            show: true
        })
    );
    for (visible, expected) in [
        ("true", Visibility::Set(true)),
        ("false", Visibility::Set(false)),
        ("\"toggle\"", Visibility::Toggle(Toggle::Toggle)),
    ] {
        assert_eq!(
            decode(&format!(
                r#"{{"type":"set_window_visible","id":1,"visible":{visible}}}"#
            )),
            Ok(Command::SetWindowVisible {
                visible: expected,
                activation_token: None,
                desktop_startup_id: None,
            })
        );
    }
    assert_eq!(
        decode(
            r#"{"type":"set_window_visible","id":1,"visible":true,"activationToken":"wayland-token","desktopStartupId":"desktop-id"}"#
        ),
        Ok(Command::SetWindowVisible {
            visible: Visibility::Set(true),
            activation_token: Some("wayland-token".into()),
            desktop_startup_id: Some("desktop-id".into()),
        })
    );
    assert_eq!(
        decode(r#"{"type":"set_window_visible","id":1,"visible":true,"activationToken":42}"#)
            .unwrap_err()
            .1
            .error,
        "bad_request"
    );
    assert_eq!(
        decode(r#"{"type":"new_item","id":1,"projectId":"/p","kind":"neovim"}"#),
        Ok(Command::NewItem {
            project_id: "/p".into(),
            kind: NewKind::Neovim,
            tool: None
        })
    );
    assert_eq!(
        decode(r#"{"type":"new_item","id":1,"projectId":"/p","kind":"agent","tool":"claude"}"#),
        Ok(Command::NewItem {
            project_id: "/p".into(),
            kind: NewKind::Agent,
            tool: Some("claude".into())
        })
    );
    assert_eq!(
        decode(r#"{"type":"open_palette","id":1}"#),
        Ok(Command::OpenPalette { project_id: None })
    );
    assert_eq!(
        decode(r#"{"type":"close_item","id":1,"itemId":"item-2"}"#),
        Ok(Command::CloseItem {
            item_id: "item-2".into(),
            confirmed: false
        })
    );
    assert_eq!(
        decode(r#"{"type":"request_quit","id":1}"#),
        Ok(Command::RequestQuit {})
    );
    assert_eq!(
        decode(r#"{"type":"set_window_visible","id":1,"visible":"maybe"}"#)
            .unwrap_err()
            .1
            .error,
        "bad_request"
    );
}

#[test]
fn activation_fields_accept_the_byte_boundary_and_reject_nul_and_oversize() {
    for field in ["activationToken", "desktopStartupId"] {
        for valid in ["".to_owned(), "a".repeat(4096), "é".repeat(2048)] {
            let request = json!({"type":"set_window_visible","id":1,"visible":true,field:valid});
            assert!(parse_command(request.to_string().as_bytes()).is_ok());
        }
        for invalid in ["a\0b".to_owned(), "a".repeat(4097), "é".repeat(2049)] {
            let request = json!({"type":"set_window_visible","id":1,"visible":true,field:invalid});
            assert_eq!(
                parse_command(request.to_string().as_bytes())
                    .unwrap_err()
                    .1
                    .error,
                "bad_request"
            );
        }
    }
}

#[test]
fn invalid_and_unknown_commands_get_error_replies() {
    let running = start();
    let mut client = Client::once(&running.server);
    client.send(r#"{"type":"frobnicate","id":1}"#);
    let reply = client.expect();
    assert_eq!(
        (&reply["id"], &reply["ok"], &reply["error"]),
        (&json!(1), &json!(false), &json!("unknown_command"))
    );
    assert!(reply["message"].as_str().unwrap().contains("frobnicate"));
    client.send(r#"{"type":"select_item","id":2}"#);
    assert_eq!(client.expect()["error"], "bad_request", "missing itemId");
    client.send(r#"{"type":"select_item","itemId":"item-1"}"#);
    let reply = client.expect();
    assert_eq!(
        (&reply["id"], &reply["error"]),
        (&Value::Null, &json!("bad_request"))
    );
    client.send("not json");
    assert_eq!(client.expect()["error"], "bad_request");
    client.send(r#"{"type":"request_quit","id":3}"#);
    assert_eq!(client.expect()["ok"], true, "the connection stays usable");
}

#[test]
fn a_one_shot_client_gets_no_snapshots_or_pings() {
    let running = start();
    running.server.hub.publish(r#"{"items":[]}"#.into());
    let mut client = Client::once(&running.server);
    client.send(r#"{"type":"request_quit","id":1}"#);
    assert_eq!(client.expect()["type"], "reply");
    running.server.hub.publish(r#"{"items":[2]}"#.into());
    client.send(r#"{"type":"request_quit","id":2}"#);
    assert_eq!(client.expect()["id"], 2);
    assert_eq!(running.server.presence().watchers, 0);
}

#[test]
fn a_wrong_protocol_or_missing_hello_is_refused() {
    let running = start();
    for hello in [
        r#"{"type":"hello","protocol":2,"watch":true}"#,
        r#"{"type":"hello","watch":true}"#,
        r#"{"type":"select_item","id":1,"itemId":"item-1"}"#,
        "garbage",
    ] {
        let mut client = Client::connect(&running.server, hello);
        assert_eq!(
            client.expect(),
            json!({"type": "error", "error": "protocol", "protocol": 1}),
            "{hello}"
        );
        assert_eq!(client.next(), None, "{hello}: closed");
    }
}

#[test]
fn an_oversized_line_ends_the_connection() {
    let running = start();
    let mut client = Client::once(&running.server);
    let long = format!(
        r#"{{"type":"request_quit","id":1,"pad":"{}"}}"#,
        "x".repeat(MAX_LINE)
    );
    // The server may stop reading mid-line; the rest of the write can fail.
    let _ = client.stream.write_all(long.as_bytes());
    let _ = client.stream.write_all(b"\n");
    assert_eq!(client.expect()["error"], "line_too_long");
    assert_eq!(client.next(), None);
}

#[test]
fn queued_snapshots_coalesce_to_the_newest() {
    let outbox = Outbox::default();
    for n in 1..=100 {
        outbox.offer(format!("snapshot {n}\n").into());
        if n == 50 {
            outbox.push("reply\n".into());
        }
    }
    let lines = outbox.take(Duration::ZERO).unwrap();
    let lines: Vec<&str> = lines.iter().map(|l| &**l).collect();
    assert_eq!(lines, ["snapshot 100\n", "reply\n"]);
    assert!(
        outbox.take(Duration::ZERO).unwrap().is_empty(),
        "nothing left"
    );
    outbox.push("last\n".into());
    outbox.finish();
    outbox.offer("too late\n".into());
    assert_eq!(&*outbox.take(Duration::ZERO).unwrap()[0], "last\n");
    assert_eq!(outbox.take(Duration::ZERO), None);
}

#[test]
fn a_slow_watcher_gets_the_newest_snapshot_not_a_backlog() {
    let running = start();
    running.server.hub.publish(r#"{"n":0}"#.into());
    let mut client = Client::watch(&running.server);
    assert_eq!(client.expect()["revision"], 1);
    // Far more than the socket buffers hold, while the client reads nothing.
    let pad = "x".repeat(32 * 1024);
    for n in 1..=500 {
        running
            .server
            .hub
            .publish(format!(r#"{{"n":{n},"pad":"{pad}"}}"#));
    }
    std::thread::sleep(Duration::from_millis(300));
    let mut received = Vec::new();
    while received.last() != Some(&json!(500)) {
        received.push(client.expect()["n"].clone());
    }
    assert!(
        received.len() < 100,
        "{} snapshots were queued",
        received.len()
    );
    assert!(
        received.windows(2).all(|w| w[0].as_u64() < w[1].as_u64()),
        "in order, newest last"
    );
}

#[test]
fn an_idle_watcher_is_pinged() {
    let running = start();
    running.server.hub.publish(r#"{"n":0}"#.into());
    let mut client = Client::watch(&running.server);
    client.expect();
    let quiet = Instant::now();
    assert_eq!(client.expect(), json!({"type": "ping"}));
    let after = quiet.elapsed();
    assert!(
        after >= PING_INTERVAL - Duration::from_millis(100) && after < PING_INTERVAL * 2,
        "{after:?}"
    );
}

#[test]
fn the_tray_icon_returns_five_seconds_after_the_last_watcher_left() {
    let now = Instant::now();
    let watched = Presence {
        watchers: 1,
        last_left: None,
    };
    let left = Presence {
        watchers: 0,
        last_left: Some(now),
    };
    assert_eq!(TRAY_HANDOVER, Duration::from_secs(5));
    assert!(Presence::default().icon_shown(TrayShow::Auto, now));
    assert!(!watched.icon_shown(TrayShow::Auto, now));
    assert!(!left.icon_shown(TrayShow::Auto, now + Duration::from_millis(4999)));
    assert!(left.icon_shown(TrayShow::Auto, now + TRAY_HANDOVER));
    assert!(watched.icon_shown(TrayShow::Always, now));
    assert!(!Presence::default().icon_shown(TrayShow::Never, now));

    let running = start();
    let one_shot = Client::once(&running.server);
    let first = Client::watch(&running.server);
    running
        .presence
        .recv_timeout(Duration::from_secs(5))
        .unwrap();
    let second = Client::watch(&running.server);
    running
        .presence
        .recv_timeout(Duration::from_secs(5))
        .unwrap();
    assert_eq!(
        running.server.presence().watchers,
        2,
        "one-shot clients don't count"
    );
    drop(first);
    running
        .presence
        .recv_timeout(Duration::from_secs(5))
        .unwrap();
    assert_eq!(running.server.presence().last_left, None);
    drop(second);
    let gone = running
        .presence
        .recv_timeout(Duration::from_secs(5))
        .unwrap();
    let presence = running.server.presence();
    assert_eq!(presence.watchers, 0);
    assert!(!presence.icon_shown(TrayShow::Auto, gone));
    // A refresh follows once the icon may return.
    let back = running
        .presence
        .recv_timeout(Duration::from_secs(10))
        .unwrap();
    assert!(back.duration_since(gone) >= TRAY_HANDOVER - Duration::from_millis(50));
    assert!(presence.icon_shown(TrayShow::Auto, back));
    drop(one_shot);
}

// ---------------------------------------------------------------------------------------------
// Commands, through the workspace functions the UI uses (headless main thread)
// ---------------------------------------------------------------------------------------------

#[cfg(target_os = "linux")]
mod commands {
    use super::*;
    use crate::{
        commands::test_support::{mock_app, Main},
        runtime::test_support::serve,
        workspace::{Sheet, WorkspaceState},
    };
    use tauri::test::MockRuntime;

    struct Fixture {
        main: Main,
        host: std::rc::Rc<FakeHost>,
        app: tauri::App<MockRuntime>,
        dir: tempfile::TempDir,
    }

    fn setup() -> Fixture {
        let main = Main::acquire();
        serve();
        main.idle();
        let host = install();
        let dir = tempfile::tempdir().unwrap();
        Fixture {
            main,
            host,
            app: mock_app(true),
            dir,
        }
    }

    impl Fixture {
        fn run(&self, command: Command) -> Result<Done, Failure> {
            self.main.run(execute(self.app.handle().clone(), command))
        }
        /// A project directory (a git root) with a running item of `tool` in it.
        fn project(&self, name: &str) -> String {
            let path = self.dir.path().canonicalize().unwrap().join(name);
            fs::create_dir_all(path.join(".git")).unwrap();
            path.to_string_lossy().into_owned()
        }
        fn running(&self, tool: &str, project: &str) -> (String, SessionKey) {
            let id = launch(tool, project);
            let key = self.host.complete(&id);
            (id, key)
        }
        fn state(&self) -> WorkspaceState {
            snapshot().unwrap()
        }
    }

    #[test]
    fn select_item_shows_the_window_and_selects() {
        let f = setup();
        let project = f.project("atlas");
        let (a, _) = f.running("shell", &project);
        let (b, _) = f.running("shell", &project);
        assert_eq!(f.state().active_item_id.as_deref(), Some(b.as_str()));
        lifecycle::hide_window(f.app.handle()).unwrap();
        assert!(!lifecycle::window_visible());
        assert_eq!(
            f.run(Command::SelectItem {
                item_id: a.clone(),
                show: true
            }),
            Ok(Done::new())
        );
        assert_eq!(f.state().active_item_id.as_deref(), Some(a.as_str()));
        assert!(lifecycle::window_visible());
        assert_eq!(
            f.run(Command::SelectItem {
                item_id: "item-99".into(),
                show: true
            })
            .unwrap_err()
            .error,
            "unknown_item"
        );
    }

    #[test]
    fn set_window_visible_shows_hides_and_toggles() {
        let f = setup();
        let visible = |visible| {
            f.run(Command::SetWindowVisible {
                visible,
                activation_token: None,
                desktop_startup_id: None,
            })
            .map(|done| done["visible"].clone())
        };
        assert_eq!(visible(Visibility::Set(false)), Ok(json!(false)));
        assert!(!lifecycle::window_visible());
        assert_eq!(visible(Visibility::Toggle(Toggle::Toggle)), Ok(json!(true)));
        assert!(lifecycle::window_visible());
        assert_eq!(
            visible(Visibility::Toggle(Toggle::Toggle)),
            Ok(json!(false))
        );
        assert_eq!(visible(Visibility::Set(true)), Ok(json!(true)));
        assert_eq!(
            f.run(Command::SetWindowVisible {
                visible: Visibility::Set(true),
                activation_token: Some("wayland-token".into()),
                desktop_startup_id: Some("desktop-id".into()),
            })
            .map(|done| done["visible"].clone()),
            Ok(json!(true)),
            "showing an already-visible window accepts best-effort activation fields"
        );
    }

    #[test]
    fn a_quitting_owner_rejects_handover_until_quitting_is_cancelled() {
        let f = setup();
        let show = || Command::SetWindowVisible {
            visible: Visibility::Set(true),
            activation_token: None,
            desktop_startup_id: None,
        };
        lifecycle::hide_window(f.app.handle()).unwrap();
        workspace::quit_snapshot().unwrap();
        assert_eq!(f.run(show()).unwrap_err().error, "quitting");
        assert!(
            !lifecycle::window_visible(),
            "a dying owner must not accept handover"
        );
        workspace::cancel_quitting();
        assert!(
            f.run(show()).is_ok(),
            "a failed/cancelled quit can show again"
        );
        assert!(lifecycle::window_visible());
    }

    #[test]
    fn jump_to_waiting_selects_what_needs_you_first() {
        let f = setup();
        let project = f.project("atlas");
        assert_eq!(
            f.run(Command::JumpToWaiting {}).unwrap_err().error,
            "nothing_waiting"
        );
        let (a, _) = f.running("omp", &project);
        let (b, _) = f.running("omp", &project);
        f.host.omp(&a, APPROVAL);
        assert_eq!(f.state().active_item_id.as_deref(), Some(b.as_str()));
        assert_eq!(
            f.run(Command::JumpToWaiting {}),
            Ok(done([("itemId", json!(a))]))
        );
        assert_eq!(f.state().active_item_id.as_deref(), Some(a.as_str()));
        assert!(lifecycle::window_visible());
    }

    #[test]
    fn new_item_launches_in_a_sidebar_project() {
        let f = setup();
        let project = f.project("atlas");
        assert_eq!(
            f.run(Command::NewItem {
                project_id: project.clone(),
                kind: NewKind::Shell,
                tool: None
            })
            .unwrap_err()
            .error,
            "unknown_project",
            "only projects the sidebar lists"
        );
        let first = launch("shell", &project);
        for (kind, tool) in [
            (NewKind::Agent, "omp"),
            (NewKind::Shell, "shell"),
            (NewKind::Neovim, "neovim"),
        ] {
            let id = f
                .run(Command::NewItem {
                    project_id: project.clone(),
                    kind,
                    tool: None,
                })
                .unwrap()["itemId"]
                .as_str()
                .unwrap()
                .to_owned();
            assert_ne!(id, first);
            let prepared = f.host.prepared.borrow().last().cloned().unwrap();
            assert_eq!(
                (
                    prepared.id.as_str(),
                    prepared.tool.as_str(),
                    prepared.cwd.as_str()
                ),
                (id.as_str(), tool, project.as_str())
            );
        }
        let failure = f
            .run(Command::NewItem {
                project_id: project.clone(),
                kind: NewKind::Agent,
                tool: Some("nope".into()),
            })
            .unwrap_err();
        assert_eq!(
            (failure.error, failure.message.as_str()),
            ("launch_failed", "Unknown tool: nope")
        );
    }

    #[test]
    fn close_item_reports_a_needed_confirmation_without_opening_the_sheet() {
        let f = setup();
        let project = f.project("atlas");
        let (agent, key) = f.running("omp", &project);
        f.host.omp(&agent, WORKING);
        let close = |confirmed| {
            f.run(Command::CloseItem {
                item_id: agent.clone(),
                confirmed,
            })
        };
        assert_eq!(
            close(false),
            Ok(done([
                ("result", json!("needs-confirmation")),
                ("reason", json!("agent-working")),
            ]))
        );
        assert!(f.state().sheet.is_none(), "the app shows no confirmation");
        assert!(f.host.calls_of("stop").is_empty());
        assert_eq!(close(true), Ok(done([("result", json!("closed"))])));
        assert_eq!(f.host.calls_of("stop"), [key]);

        let (neovim, _) = f.running("neovim", &project);
        assert_eq!(
            f.run(Command::CloseItem {
                item_id: neovim,
                confirmed: false
            })
            .unwrap()["reason"],
            "neovim"
        );
        let (shell, _) = f.running("shell", &project);
        assert_eq!(
            f.run(Command::CloseItem {
                item_id: shell.clone(),
                confirmed: false
            }),
            Ok(done([("result", json!("closed"))]))
        );
        assert_eq!(
            f.run(Command::CloseItem {
                item_id: "item-99".into(),
                confirmed: true
            })
            .unwrap_err()
            .error,
            "unknown_item"
        );
    }

    #[test]
    fn open_palette_and_request_quit_present_the_apps_own_sheets() {
        let f = setup();
        let project = f.project("atlas");
        let (shell, _) = f.running("shell", &project);
        f.run(Command::OpenPalette {
            project_id: Some(project.clone()),
        })
        .unwrap();
        assert!(matches!(
            &f.state().sheet,
            Some(Sheet::Palette { project_id: Some(p), .. }) if *p == project
        ));
        assert!(lifecycle::window_visible());
        f.run(Command::RequestQuit {}).unwrap();
        f.main.until("the quit sheet", || {
            matches!(f.state().sheet, Some(Sheet::Quit { .. }))
        });
        assert!(matches!(
            &f.state().sheet,
            Some(Sheet::Quit { item_ids }) if *item_ids == [shell.clone()]
        ));
    }
}
