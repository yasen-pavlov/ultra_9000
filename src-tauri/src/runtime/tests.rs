use super::*;
#[test]
fn save_requests_follow_mutations_not_reply_wait_order() {
    let (tx, rx) = mpsc::channel();
    let mut ui = UiStateFile {
        sidebar_expanded: false,
        ..Default::default()
    };
    let first = save_ui_to(ui.clone(), &tx).unwrap();
    ui.folded_projects.push("/atlas".into());
    let second = save_ui_to(ui, &tx).unwrap();
    let Request::Save(first_ui, first_reply) = rx.recv().unwrap() else {
        panic!("expected save")
    };
    let Request::Save(second_ui, second_reply) = rx.recv().unwrap() else {
        panic!("expected save")
    };
    assert!(first_ui.folded_projects.is_empty());
    assert_eq!(second_ui.folded_projects, vec!["/atlas"]);
    assert!(!second_ui.sidebar_expanded);
    // Continuations may complete in reverse order; the write order was fixed already.
    second_reply.send(Ok(())).unwrap();
    first_reply.send(Ok(())).unwrap();
    wait_reply(second).unwrap();
    wait_reply(first).unwrap();
}
#[test]
fn terminal_overlay_deep_merges_and_font_wins() {
    let text = terminal_config("[font.normal]\nfamily='Test font'\n[colors.primary]\nbackground='#123456'\n[font]\nsize=16\n", Some(19.0)).unwrap();
    let value: toml::Table = text.parse().unwrap();
    assert_eq!(
        value["font"]["normal"]["family"].as_str(),
        Some("Test font")
    );
    assert_eq!(value["font"]["size"].as_float(), Some(19.0));
    assert_eq!(
        value["colors"]["primary"]["background"].as_str(),
        Some("#123456")
    );
    assert!(value["colors"]["normal"].is_table());
    alacritty_embed::embed::validate_config(&text).unwrap();
}
use std::{
    fs,
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
/// Isolated config dir, state file and agent-store home under the temp dir.
struct Fixture {
    root: PathBuf,
    paths: ConfigPaths,
    home: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().canonicalize().unwrap().join(format!(
            "u9k-runtime-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let dir = root.join("config");
        let home = root.join("home");
        fs::create_dir_all(dir.join("machines")).unwrap();
        fs::create_dir_all(&home).unwrap();
        let paths = ConfigPaths {
            ultra: dir.join("ultra.toml"),
            terminal: dir.join("terminal.toml"),
            keys: dir.join("keys.toml"),
            machine_file: dir.join("machines/test.toml"),
            machine: "test".into(),
            state_file: root.join("state/state.json"),
            dir,
        };
        Self { root, paths, home }
    }
    fn worker(&self) -> (Worker<impl FnMut(Effect)>, mpsc::Receiver<Effect>) {
        let (tx, rx) = mpsc::channel();
        let worker = Worker::start(
            ConfigStore::load(self.paths.clone()),
            self.home.clone(),
            ui_state::load(&self.paths.state_file),
            move |effect| tx.send(effect).unwrap(),
        );
        (worker, rx)
    }
    /// An OMP session file as OMP writes it: header line, then entries.
    fn omp_session(&self, id: &str, title: &str, extra: &str) -> PathBuf {
        let bucket = self.home.join(".omp/agent/sessions/bucket");
        let cwd = self.home.join("project");
        fs::create_dir_all(&bucket).unwrap();
        fs::create_dir_all(&cwd).unwrap();
        let path = bucket.join(format!("date_{id}.jsonl"));
        fs::write(
            &path,
            format!(
                "{{\"type\":\"session\",\"id\":\"{id}\",\"title\":\"{title}\",\"cwd\":{}}}\n{extra}",
                serde_json::to_string(&cwd).unwrap()
            ),
        )
        .unwrap();
        path
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
fn drain(rx: &mpsc::Receiver<Effect>) -> Vec<Effect> {
    rx.try_iter().collect()
}
fn kind(effect: &Effect) -> &'static str {
    match effect {
        Effect::Startup(..) => "startup",
        Effect::Error(_) => "error",
        Effect::Config(_) => "config",
        Effect::SettingsState(_) => "settings-state",
        Effect::Discovery(..) => "discovery",
        Effect::Bind(..) => "bind",
        Effect::Probe(_) => "probe",
    }
}
fn kinds(effects: &[Effect]) -> Vec<&'static str> {
    effects.iter().map(kind).collect()
}
fn reply<T>(
    worker: &mut Worker<impl FnMut(Effect)>,
    request: impl FnOnce(mpsc::Sender<Result<T, String>>) -> Request,
) -> Result<T, String> {
    let (tx, rx) = mpsc::channel();
    worker.handle(request(tx));
    wait_reply(rx)
}
fn config(effect: &Effect) -> &ConfigUpdate {
    match effect {
        Effect::Startup(update, _) | Effect::Config(update) => update,
        _ => panic!("expected a config update, got {}", kind(effect)),
    }
}

#[test]
fn startup_publishes_config_and_ui_before_discovery_and_probes() {
    let f = Fixture::new();
    fs::write(&f.paths.ultra, "[sidebar]\nwidth = 280\n").unwrap();
    let saved = UiStateFile {
        sidebar_expanded: false,
        folded_projects: vec!["/atlas".into()],
        last_set: vec![],
    };
    ui_state::save(&f.paths.state_file, &saved).unwrap();
    f.omp_session("s1", "Saved work", "");

    let (worker, rx) = f.worker();
    let effects = drain(&rx);
    assert_eq!(kinds(&effects), ["startup", "discovery", "probe"]);
    let Effect::Startup(update, ui) = &effects[0] else {
        unreachable!()
    };
    assert_eq!(update.settings.sidebar_width, 280);
    assert_eq!(update.state.config_dir, f.paths.dir.to_string_lossy());
    assert!(update.terminal.as_deref().is_some_and(|t| !t.is_empty()));
    assert!(!ui.sidebar_expanded);
    assert_eq!(ui.folded_projects, ["/atlas"]);
    let Effect::Discovery(conversations, errors) = &effects[1] else {
        unreachable!()
    };
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(conversations.len(), 1);
    assert_eq!(conversations[0].title, "Saved work");
    let Effect::Probe(tools) = &effects[2] else {
        unreachable!()
    };
    assert_eq!(tools, &worker.store.settings().tools);
}

#[test]
fn settings_writes_reply_publish_and_rescan_but_failures_change_nothing() {
    let f = Fixture::new();
    let (mut worker, rx) = f.worker();
    drain(&rx);

    let set = |key: &str, value: Option<serde_json::Value>| {
        let key = key.to_owned();
        move |tx: mpsc::Sender<Result<(), String>>| Request::Set(key, value, WriteTarget::Auto, tx)
    };
    reply(&mut worker, set("sidebar.width", Some(300.into()))).unwrap();
    assert!(fs::read_to_string(&f.paths.ultra)
        .unwrap()
        .contains("width = 300"));
    let effects = drain(&rx);
    assert_eq!(kinds(&effects), ["config", "discovery"]);
    assert_eq!(config(&effects[0]).settings.sidebar_width, 300);

    assert_eq!(
        reply(&mut worker, set("tools.omp.command", Some("x".into()))).unwrap_err(),
        "setting is not UI-writable"
    );
    assert!(reply(&mut worker, set("sidebar.width", Some("wide".into()))).is_err());
    assert!(drain(&rx).is_empty(), "rejected writes publish nothing");
    assert_eq!(worker.store.settings().sidebar_width, 300);

    reply(&mut worker, set("sidebar.width", None)).unwrap();
    let effects = drain(&rx);
    assert_eq!(kinds(&effects), ["config", "discovery"]);
    assert_eq!(config(&effects[0]).settings.sidebar_width, 304);
}

#[test]
fn reloads_always_publish_config_but_rescan_and_probe_only_on_changes() {
    let f = Fixture::new();
    let (mut worker, rx) = f.worker();
    drain(&rx);

    worker.handle(Request::Reload);
    assert_eq!(kinds(&drain(&rx)), ["config"]);

    fs::write(&f.paths.ultra, "[sidebar]\nwidth = 250\n").unwrap();
    worker.handle(Request::Reload);
    assert_eq!(kinds(&drain(&rx)), ["config", "discovery"]);

    fs::write(
        &f.paths.ultra,
        "[sidebar]\nwidth = 250\n[tools.custom]\nlabel = \"Custom\"\ncommand = \"custom\"\n",
    )
    .unwrap();
    worker.handle(Request::Reload);
    let effects = drain(&rx);
    assert_eq!(kinds(&effects), ["config", "discovery", "probe"]);
    let Effect::Probe(tools) = &effects[2] else {
        unreachable!()
    };
    assert!(tools.iter().any(|t| t.id == "custom"));
}

#[test]
fn invalid_keys_are_reported_against_the_keys_file() {
    let f = Fixture::new();
    fs::write(&f.paths.keys, "[bindings]\nexplode = \"Ctrl+Shift+X\"\n").unwrap();
    let (_worker, rx) = f.worker();
    let effects = drain(&rx);
    let update = config(&effects[0]);
    let keys = f.paths.keys.to_string_lossy();
    assert!(
        update
            .state
            .errors
            .iter()
            .any(|e| e.file == keys && e.message == "unknown action: explode"),
        "{:?}",
        update.state.errors
    );
    let default = crate::shortcuts::Keymap::defaults(PLATFORM)
        .label(crate::shortcuts::Action::Palette)
        .unwrap();
    let palette = parse_chord(&default);
    assert_eq!(
        update.keys.lookup(palette.mods, palette.key),
        Some(crate::shortcuts::Action::Palette),
        "defaults stay bound"
    );
}
fn parse_chord(text: &str) -> crate::shortcuts::Chord {
    crate::shortcuts::parse_chord(text).unwrap()
}

#[test]
fn starter_files_are_created_only_for_app_configuration_files() {
    let f = Fixture::new();
    let (mut worker, _rx) = f.worker();
    let keys = f.paths.keys.to_string_lossy().into_owned();
    let (dir, file) = reply(&mut worker, |tx| Request::Starter(keys.clone(), tx)).unwrap();
    assert_eq!(dir, f.paths.dir.to_string_lossy());
    assert_eq!(file, keys);
    assert!(fs::read_to_string(&f.paths.keys)
        .unwrap()
        .contains("[bindings]"));
    // An existing file is opened as-is, never overwritten.
    fs::write(&f.paths.keys, "# mine\n").unwrap();
    reply(&mut worker, |tx| Request::Starter(keys.clone(), tx)).unwrap();
    assert_eq!(fs::read_to_string(&f.paths.keys).unwrap(), "# mine\n");
    let outside = f.root.join("elsewhere.toml");
    assert_eq!(
        reply(&mut worker, |tx| Request::Starter(
            outside.to_string_lossy().into(),
            tx
        ))
        .unwrap_err(),
        "not an app configuration file"
    );
    assert!(!outside.exists());
}

#[test]
fn ui_saves_persist_to_the_state_file_in_request_order() {
    let f = Fixture::new();
    let (mut worker, _rx) = f.worker();
    let mut ui = UiStateFile::default();
    ui.folded_projects.push("/first".into());
    reply(&mut worker, |tx| Request::Save(ui.clone(), tx)).unwrap();
    ui.folded_projects = vec!["/second".into()];
    ui.sidebar_expanded = false;
    reply(&mut worker, |tx| Request::Save(ui, tx)).unwrap();
    let saved = ui_state::load(&f.paths.state_file);
    assert_eq!(saved.folded_projects, ["/second"]);
    assert!(!saved.sidebar_expanded);
}

#[test]
fn pending_bindings_resolve_when_the_session_file_appears() {
    let f = Fixture::new();
    let (mut worker, rx) = f.worker();
    drain(&rx);

    worker.handle(Request::Bind(
        "item-1".into(),
        "omp-dev".into(),
        "stale".into(),
    ));
    // A newer session for the same item replaces the old pending one.
    worker.handle(Request::Bind(
        "item-1".into(),
        "omp-dev".into(),
        "s1".into(),
    ));
    worker.handle(Request::Bind("item-2".into(), "omp".into(), "s2".into()));
    worker.handle(Request::Forget("item-2".into()));
    assert!(drain(&rx).is_empty(), "no session file yet");

    f.omp_session("stale", "Old", "");
    let s1 = f.omp_session("s1", "Fresh", "");
    f.omp_session("s2", "Forgotten", "");
    worker.handle(Request::Scan);
    worker.scan_due();
    let effects = drain(&rx);
    assert_eq!(kinds(&effects), ["bind", "discovery"]);
    let Effect::Bind(item, session, conversation) = &effects[0] else {
        unreachable!()
    };
    assert_eq!((item.as_str(), session.as_str()), ("item-1", "s1"));
    assert_eq!(conversation.path, s1);
    assert_eq!(
        conversation.tool, "omp-dev",
        "the launching tool owns the binding"
    );
    assert!(worker.pending_bindings.is_empty());

    // A session file that already exists binds immediately.
    worker.handle(Request::Bind("item-3".into(), "omp".into(), "s2".into()));
    assert_eq!(kinds(&drain(&rx)), ["bind"]);
}

#[test]
fn scan_requests_coalesce_into_one_debounced_refresh() {
    let f = Fixture::new();
    let (mut worker, rx) = f.worker();
    drain(&rx);
    assert_eq!(worker.timeout(), Duration::from_secs(3600));
    worker.scan_due();
    assert!(drain(&rx).is_empty(), "nothing due without a scan request");

    worker.handle(Request::Scan);
    worker.handle(Request::Scan);
    assert!(worker.timeout() <= Duration::from_millis(150));
    worker.scan_due();
    assert_eq!(kinds(&drain(&rx)), ["discovery"]);
    worker.scan_due();
    assert!(drain(&rx).is_empty());
    assert_eq!(worker.timeout(), Duration::from_secs(3600));
}

#[test]
fn served_scans_wait_for_the_debounce() {
    let f = Fixture::new();
    let (paths, home) = (f.paths.clone(), f.home.clone());
    let (effects_tx, effects) = mpsc::channel();
    let (requests, requests_rx) = mpsc::channel();
    let server = std::thread::spawn(move || {
        let mut worker = Worker::start(
            ConfigStore::load(paths.clone()),
            home,
            ui_state::load(&paths.state_file),
            move |effect| effects_tx.send(effect).unwrap(),
        );
        serve_requests(&requests_rx, &mut worker);
    });
    // Startup effects are posted synchronously and end with the probe.
    while kind(&effects.recv_timeout(Duration::from_secs(5)).unwrap()) != "probe" {}

    // Timed from before the send: the worker cannot schedule the scan earlier than 150 ms after
    // it receives the request, and test-thread descheduling only makes the measured time longer.
    let sent = std::time::Instant::now();
    requests.send(Request::Scan).unwrap();
    let effect = effects.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(kind(&effect), "discovery");
    assert!(
        sent.elapsed() >= Duration::from_millis(150),
        "scan ran before the debounce ({:?})",
        sent.elapsed()
    );
    drop(requests);
    server.join().unwrap();
}

#[test]
fn reported_errors_publish_a_settings_snapshot() {
    let f = Fixture::new();
    let (mut worker, rx) = f.worker();
    drain(&rx);
    worker.handle(Request::Report(SettingsError {
        file: "ultra.toml".into(),
        line: Some(3),
        key: None,
        message: "oversized".into(),
        last_good: None,
    }));
    let watch_dir = f.paths.dir.clone();
    worker.report(&watch_dir, "watch failed".into());
    let effects = drain(&rx);
    assert_eq!(kinds(&effects), ["settings-state", "settings-state"]);
    let Effect::SettingsState(state) = &effects[1] else {
        unreachable!()
    };
    let messages: Vec<_> = state.errors.iter().map(|e| e.message.as_str()).collect();
    assert!(messages.contains(&"oversized") && messages.contains(&"watch failed"));
    assert!(state
        .errors
        .iter()
        .any(|e| e.file == watch_dir.to_string_lossy()));
}

#[test]
fn store_watches_use_nearest_existing_ancestor_until_the_store_exists() {
    let f = Fixture::new();
    let settings = Settings::default();
    assert_eq!(
        desired_watches(&f.home, &settings),
        [(f.home.clone(), RecursiveMode::NonRecursive)],
        "missing stores collapse onto one shallow home watch"
    );
    let omp = f.home.join(".omp/agent/sessions");
    fs::create_dir_all(&omp).unwrap();
    fs::create_dir_all(f.home.join(".claude")).unwrap();
    let desired = desired_watches(&f.home, &settings);
    assert!(desired.contains(&(omp, RecursiveMode::Recursive)));
    assert!(desired.contains(&(f.home.join(".claude"), RecursiveMode::NonRecursive)));
    assert!(desired.contains(&(f.home.clone(), RecursiveMode::NonRecursive)));
    assert!(desired.iter().all(|(path, _)| path.starts_with(&f.home)));

    let (mut worker, rx) = f.worker();
    worker.watch_stores(Some(notify::recommended_watcher(|_| {}).unwrap()));
    assert_eq!(worker.watched, desired);
    // A store created later is picked up by the next scan.
    let claude = f.home.join(".claude/projects");
    fs::create_dir_all(&claude).unwrap();
    worker.handle(Request::Scan);
    worker.scan_due();
    assert!(worker.watched.contains(&(claude, RecursiveMode::Recursive)));
    assert_eq!(worker.watched, desired_watches(&f.home, &settings));
    drop(rx);
}

#[test]
fn reveal_creates_the_config_dir_before_opening_it() {
    let f = Fixture::new();
    let dir = f.root.join("fresh/config");
    reveal(&dir, "true").unwrap();
    assert!(dir.is_dir());
    assert!(reveal(&dir, "/u9k/missing-opener").is_err());
    let file = f.root.join("file");
    fs::write(&file, "").unwrap();
    assert!(reveal(&file.join("config"), "true").is_err());
}

#[test]
fn invalid_terminal_config_keeps_the_last_good_terminal_and_reports() {
    let f = Fixture::new();
    // Well-formed TOML that Alacritty's strict validation rejects.
    let invalid = "[colors.primary]\nbackground = \"not a colour\"\n";
    fs::write(&f.paths.terminal, invalid).unwrap();
    let (mut worker, rx) = f.worker();
    let effects = drain(&rx);
    let update = config(&effects[0]);
    assert_eq!(update.terminal, None, "no terminal config was ever good");
    let terminal = f.paths.terminal.to_string_lossy();
    assert!(update.state.errors.iter().any(|e| e.file == terminal));

    fs::write(&f.paths.terminal, "[font]\nsize = 13.0\n").unwrap();
    worker.handle(Request::Reload);
    let good = config(&drain(&rx)[0]).terminal.clone().unwrap();
    let value: toml::Table = good.parse().unwrap();
    assert_eq!(value["font"]["size"].as_float(), Some(13.0));

    fs::write(&f.paths.terminal, invalid).unwrap();
    worker.handle(Request::Reload);
    let effects = drain(&rx);
    let update = config(&effects[0]);
    assert_eq!(update.terminal.as_deref(), Some(good.as_str()));
    assert!(update.state.errors.iter().any(|e| e.file == terminal));
}
