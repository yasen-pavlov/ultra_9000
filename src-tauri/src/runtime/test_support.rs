use super::*;
pub(crate) struct Served {
    pub(crate) paths: ConfigPaths,
}
/// Serving starts once per test process, on first use.
static SERVED: LazyLock<Served> = LazyLock::new(start_serving);
/// The served worker's files. Returns after its startup effects were posted. Effects are
/// posted like in production, except that tools are never probed (that would run the
/// developer's login shell).
pub(crate) fn serve() -> &'static Served {
    &SERVED
}
fn root() -> PathBuf {
    std::env::temp_dir()
        .canonicalize()
        .unwrap()
        .join(format!("u9k-served-worker-{}", std::process::id()))
}
extern "C" fn remove_root() {
    let _ = std::fs::remove_dir_all(root());
}
fn start_serving() -> Served {
    let root = root();
    let _ = std::fs::remove_dir_all(&root);
    // The worker lives as long as the test process; its files go with it.
    // SAFETY: registers a plain function without captured state.
    unsafe { libc::atexit(remove_root) };
    let dir = root.join("config");
    let home = root.join("home");
    std::fs::create_dir_all(dir.join("machines")).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    let paths = ConfigPaths {
        ultra: dir.join("ultra.toml"),
        terminal: dir.join("terminal.toml"),
        keys: dir.join("keys.toml"),
        machine_file: dir.join("machines/test.toml"),
        machine: "test".into(),
        state_file: root.join("state/state.json"),
        dir,
    };
    let rx = WORKER
        .1
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take()
        .expect("the worker channel is served once");
    let (ready, started) = mpsc::channel();
    let store = ConfigStore::load(paths.clone());
    thread::spawn(move || {
        let ui = ui_state::load(&store.paths.state_file);
        let mut worker = Worker::start(store, home, ui, |effect| match effect {
            Effect::Probe(_) => {}
            effect => post(effect),
        });
        ready.send(()).unwrap();
        serve_requests(&rx, &mut worker);
    });
    started.recv().unwrap();
    Served { paths }
}
