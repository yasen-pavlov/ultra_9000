//! A single I/O owner serializes config writes, discovery and UI-state persistence.
//! No disk access or login-shell probes run under the main-thread workspace borrow.
use crate::{
    config::{self, ConfigPaths, ConfigStore, Settings, SettingsError, SettingsState, WriteTarget},
    discovery::{self, Conversation, Index, StoreError},
    launch, platform,
    shortcuts::{Keymap, Platform},
    ui_state::{self, UiStateFile},
    workspace,
};
use notify::{RecursiveMode, Watcher};
use std::{
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, LazyLock, Mutex, PoisonError,
    },
    thread,
    time::{Duration, Instant},
};

pub const PLATFORM: Platform = if cfg!(target_os = "macos") {
    Platform::MacOs
} else {
    Platform::Linux
};
#[cfg(target_os = "macos")]
pub const BUNDLED: &str = concat!(
    include_str!("alacritty.toml"),
    include_str!("alacritty-macos.toml")
);
#[cfg(target_os = "linux")]
pub const BUNDLED: &str = concat!(
    include_str!("alacritty.toml"),
    include_str!("alacritty-linux.toml")
);

pub struct ConfigUpdate {
    pub settings: Settings,
    pub state: SettingsState,
    pub keys: Keymap,
    pub terminal: Option<String>,
}
pub enum Request {
    Reload,
    Scan,
    Set(
        String,
        Option<serde_json::Value>,
        WriteTarget,
        mpsc::Sender<Result<(), String>>,
    ),
    Starter(String, mpsc::Sender<Result<(String, String), String>>),
    Reveal(mpsc::Sender<Result<(), String>>),
    Save(UiStateFile, mpsc::Sender<Result<(), String>>),
    Bind(String, String, String),
    Forget(String),
    Report(SettingsError),
}
type WorkerChannel = (
    mpsc::Sender<Request>,
    Mutex<Option<mpsc::Receiver<Request>>>,
);
static WORKER: LazyLock<WorkerChannel> = LazyLock::new(|| {
    let (tx, rx) = mpsc::channel();
    (tx, Mutex::new(Some(rx)))
});
static SCAN_PENDING: AtomicBool = AtomicBool::new(false);
static RELOAD_PENDING: AtomicBool = AtomicBool::new(false);
pub fn send(request: Request) -> Result<(), String> {
    WORKER
        .0
        .send(request)
        .map_err(|_| "integration worker stopped".into())
}
/// Enqueue on the main thread, alongside the mutation; only await this reply off-main.
pub fn save_ui(ui: UiStateFile) -> Result<mpsc::Receiver<Result<(), String>>, String> {
    save_ui_to(ui, &WORKER.0)
}
fn save_ui_to(
    ui: UiStateFile,
    worker: &mpsc::Sender<Request>,
) -> Result<mpsc::Receiver<Result<(), String>>, String> {
    let (tx, rx) = mpsc::channel();
    worker
        .send(Request::Save(ui, tx))
        .map_err(|_| "integration worker stopped".to_owned())?;
    Ok(rx)
}
/// Explicit palette opens/retries share the watcher's coalesced scan request.
pub fn request_scan() -> Result<(), String> {
    if !SCAN_PENDING.swap(true, Ordering::AcqRel) {
        if let Err(error) = send(Request::Scan) {
            SCAN_PENDING.store(false, Ordering::Release);
            return Err(error);
        }
    }
    Ok(())
}
/// Main-thread consequences of worker requests, in the order the worker produced them.
enum Effect {
    Startup(Box<ConfigUpdate>, UiStateFile),
    Error(String),
    Config(Box<ConfigUpdate>),
    SettingsState(SettingsState),
    Discovery(Vec<Conversation>, Vec<StoreError>),
    Bind(String, String, Conversation),
    /// Resolve these tools on the login-shell PATH (off the worker, which must stay responsive).
    Probe(Vec<config::ToolSpec>),
}
impl Effect {
    fn apply(self) {
        match self {
            Self::Startup(update, ui) => {
                workspace::apply_config(*update);
                workspace::load_ui(ui);
            }
            Self::Error(e) => workspace::action_error(e),
            Self::Config(update) => workspace::apply_config(*update),
            Self::SettingsState(state) => workspace::apply_settings_state(state),
            Self::Discovery(conversations, errors) => {
                workspace::apply_discovery(conversations, errors)
            }
            Self::Bind(item, session, c) => workspace::bind_conversation(&item, &session, c),
            Self::Probe(_) => unreachable!("probes run on their own thread"),
        }
    }
}
fn post(effect: Effect) {
    match effect {
        Effect::Probe(tools) => probe(tools),
        effect => platform::defer_on_main(Box::new(move || effect.apply())),
    }
}
pub fn start() {
    let tx = WORKER.0.clone();
    let rx = WORKER
        .1
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .take()
        .expect("worker started once");
    thread::Builder::new()
        .name("u9k-integrations".into())
        .spawn(move || {
            let paths = match ConfigPaths::resolve() {
                Ok(p) => p,
                Err(e) => {
                    post(Effect::Error(e));
                    return;
                }
            };
            let home = PathBuf::from(launch::default_directory());
            let ui = ui_state::load(&paths.state_file);
            let mut worker = Worker::start(ConfigStore::load(paths), home, ui, post);
            let config_tx = tx.clone();
            let _config_watch = match config::watch(&worker.store.paths.dir, move || {
                if !RELOAD_PENDING.swap(true, Ordering::AcqRel) {
                    let _ = config_tx.send(Request::Reload);
                }
            }) {
                Ok(w) => Some(w),
                Err(e) => {
                    let file = worker.store.paths.dir.clone();
                    worker.report(&file, e);
                    None
                }
            };
            let watch_tx = tx.clone();
            worker.watch_stores(
                notify::recommended_watcher(move |event: Result<notify::Event, notify::Error>| {
                    if event.is_ok_and(|e| !matches!(e.kind, notify::EventKind::Access(_)))
                        && !SCAN_PENDING.swap(true, Ordering::AcqRel)
                    {
                        let _ = watch_tx.send(Request::Scan);
                    }
                })
                .ok(),
            );
            serve_requests(&rx, &mut worker);
        })
        .expect("start integrations worker");
}
/// Handle requests until every sender is gone, running due scans in between.
fn serve_requests(rx: &mpsc::Receiver<Request>, worker: &mut Worker<impl FnMut(Effect)>) {
    loop {
        match rx.recv_timeout(worker.timeout()) {
            Ok(request) => worker.handle(request),
            Err(mpsc::RecvTimeoutError::Timeout) => worker.scan_due(),
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
}
/// The process-wide request channel served from temp config, state and home directories, for
/// tests of callers that wait on worker replies (commands, lifecycle; Linux, where the test
/// thread can run main-thread tasks).
#[cfg(all(test, target_os = "linux"))]
pub(crate) mod test_support;
/// The integration worker's state; `post` receives every main-thread effect in order.
struct Worker<P: FnMut(Effect)> {
    store: ConfigStore,
    home: PathBuf,
    index: Index,
    /// Items waiting for their OMP session file: (item, tool, session id).
    pending_bindings: Vec<(String, String, String)>,
    terminal_good: Option<String>,
    watcher: Option<notify::RecommendedWatcher>,
    watched: Vec<(PathBuf, RecursiveMode)>,
    scan_at: Option<Instant>,
    post: P,
}
impl<P: FnMut(Effect)> Worker<P> {
    fn start(store: ConfigStore, home: PathBuf, ui: UiStateFile, post: P) -> Self {
        let mut worker = Self {
            store,
            index: Index::new(home.clone()),
            home,
            pending_bindings: Vec::new(),
            terminal_good: None,
            watcher: None,
            watched: Vec::new(),
            scan_at: None,
            post,
        };
        let update = compose(&mut worker.store, &mut worker.terminal_good);
        (worker.post)(Effect::Startup(Box::new(update), ui));
        worker.refresh();
        (worker.post)(Effect::Probe(worker.store.settings().tools.clone()));
        worker
    }
    fn watch_stores(&mut self, watcher: Option<notify::RecommendedWatcher>) {
        self.watcher = watcher;
        self.update_store_watch();
    }
    /// How long the worker may block before a coalesced scan is due.
    fn timeout(&self) -> Duration {
        self.scan_at
            .map(|at| at.saturating_duration_since(Instant::now()))
            .unwrap_or(Duration::from_secs(3600))
    }
    fn handle(&mut self, request: Request) {
        match request {
            Request::Scan => {
                self.scan_at = Some(Instant::now() + Duration::from_millis(150));
            }
            Request::Reload => {
                RELOAD_PENDING.store(false, Ordering::Release);
                let before = self.store.settings().tools.clone();
                let changed: config::Reloaded = self.store.reload();
                // Consume explicit reload flags: config values always publish, expensive scans only on settings changes.
                self.publish_config();
                if changed.settings {
                    self.refresh();
                    self.update_store_watch();
                }
                if before != self.store.settings().tools {
                    (self.post)(Effect::Probe(self.store.settings().tools.clone()));
                }
                let _ = (changed.terminal, changed.keys);
            }
            Request::Set(key, value, target, reply) => {
                let before = self.store.settings().tools.clone();
                let result = match value {
                    Some(v) => self.store.set(&key, v, target),
                    None => self.store.reset(&key, target),
                };
                if result.is_ok() {
                    self.publish_config();
                    self.refresh();
                    self.update_store_watch();
                    if before != self.store.settings().tools {
                        (self.post)(Effect::Probe(self.store.settings().tools.clone()));
                    }
                }
                let _ = reply.send(result);
            }
            Request::Starter(file, reply) => {
                let file = PathBuf::from(file);
                let result = self.store.ensure_starter(&file).map(|()| {
                    (
                        self.store.paths.dir.to_string_lossy().into(),
                        file.to_string_lossy().into(),
                    )
                });
                let _ = reply.send(result);
            }
            Request::Reveal(reply) => {
                let _ = reply.send(reveal(&self.store.paths.dir, OPENER));
            }
            Request::Save(ui, reply) => {
                let _ = reply.send(ui_state::save(&self.store.paths.state_file, &ui));
            }
            Request::Bind(item, tool, session) => {
                self.pending_bindings.retain(|(id, _, _)| id != &item);
                self.pending_bindings.push((item, tool, session));
                self.bind_pending();
            }
            Request::Forget(item) => {
                self.pending_bindings.retain(|(id, _, _)| id != &item);
            }
            Request::Report(error) => {
                self.store.report_error(error);
                self.publish_settings_state();
            }
        }
    }
    /// Run the coalesced scan once its debounce elapsed; nothing is due without a request.
    fn scan_due(&mut self) {
        if self.scan_at.take().is_none() {
            return;
        }
        SCAN_PENDING.store(false, Ordering::Release);
        self.refresh();
        self.update_store_watch();
    }
    fn report(&mut self, file: &Path, message: String) {
        report(&mut self.store, file, message);
        self.publish_settings_state();
    }
    fn publish_settings_state(&mut self) {
        (self.post)(Effect::SettingsState(self.store.snapshot()));
    }
    fn publish_config(&mut self) {
        let update = compose(&mut self.store, &mut self.terminal_good);
        (self.post)(Effect::Config(Box::new(update)));
    }
    fn refresh(&mut self) {
        let settings = self.store.settings();
        self.index.refresh(&settings.tools, &settings.roots);
        self.bind_pending();
        let conversations = self.index.conversations().to_vec();
        let errors = self.index.errors().to_vec();
        (self.post)(Effect::Discovery(conversations, errors));
    }
    fn bind_pending(&mut self) {
        let (index, post) = (&mut self.index, &mut self.post);
        self.pending_bindings.retain(|(item, tool, session)| {
            let Some(mut conversation) = index.omp_by_session_id(session) else {
                return true;
            };
            conversation.tool.clone_from(tool);
            post(Effect::Bind(item.clone(), session.clone(), conversation));
            false
        });
    }
    fn update_store_watch(&mut self) {
        let Some(watcher) = &mut self.watcher else {
            return;
        };
        let desired = desired_watches(&self.home, self.store.settings());
        if self.watched == desired {
            return;
        }
        for (path, _) in self.watched.drain(..) {
            let _ = watcher.unwatch(&path);
        }
        for (path, mode) in desired {
            if watcher.watch(&path, mode).is_ok() {
                self.watched.push((path, mode));
            }
        }
    }
}
fn report(store: &mut ConfigStore, file: &Path, message: String) {
    store.report_error(SettingsError {
        file: file.to_string_lossy().into(),
        line: None,
        key: None,
        message,
        last_good: None,
    });
}
fn compose(store: &mut ConfigStore, last_good: &mut Option<String>) -> ConfigUpdate {
    let (keys, errors) = Keymap::with_overrides(PLATFORM, store.keys_toml());
    for message in errors {
        let file = store.paths.keys.clone();
        report(store, &file, message);
    }
    let terminal = terminal_config(store.terminal_toml(), store.settings().terminal_font_size)
        .and_then(|text| alacritty_embed::embed::validate_config(&text).map(|()| text));
    let terminal = match terminal {
        Ok(text) => {
            *last_good = Some(text.clone());
            Some(text)
        }
        Err(message) => {
            let file = store.paths.terminal.clone();
            report(store, &file, message);
            last_good.clone()
        }
    };
    ConfigUpdate {
        settings: store.settings().clone(),
        state: store.snapshot(),
        keys,
        terminal,
    }
}
fn merge(base: &mut toml::Value, overlay: toml::Value) {
    match (base, overlay) {
        (toml::Value::Table(base), toml::Value::Table(overlay)) => {
            for (key, value) in overlay {
                if let Some(old) = base.get_mut(&key) {
                    merge(old, value);
                } else {
                    base.insert(key, value);
                }
            }
        }
        (base, overlay) => *base = overlay,
    }
}
pub fn terminal_config(user: &str, size: Option<f32>) -> Result<String, String> {
    // Documents parse as tables: `toml::Value::from_str` reads a single value expression.
    let mut value = toml::Value::Table(
        BUNDLED
            .parse()
            .map_err(|e: toml::de::Error| e.to_string())?,
    );
    if !user.is_empty() {
        merge(
            &mut value,
            toml::Value::Table(user.parse().map_err(|e: toml::de::Error| e.to_string())?),
        );
    }
    if let Some(size) = size {
        let mut overlay = toml::Table::new();
        let mut font = toml::Table::new();
        font.insert("size".into(), toml::Value::Float(size as f64));
        overlay.insert("font".into(), toml::Value::Table(font));
        merge(&mut value, toml::Value::Table(overlay));
    }
    toml::to_string(&value).map_err(|e| e.to_string())
}
fn probe(tools: Vec<config::ToolSpec>) {
    thread::spawn(move || {
        for tool in tools {
            let result = launch::resolve(&tool);
            platform::defer_on_main(Box::new(move || workspace::tool_resolved(tool, result)));
        }
    });
}
/// Store directories to watch: each existing store recursively, else its nearest existing
/// ancestor non-recursively, so a store created later is noticed.
fn desired_watches(home: &Path, settings: &Settings) -> Vec<(PathBuf, RecursiveMode)> {
    let mut desired = Vec::new();
    for dir in discovery::store_dirs(home, &settings.tools) {
        let (mut path, mut mode) = (dir, RecursiveMode::Recursive);
        while !path.is_dir() {
            if !path.pop() {
                break;
            }
            mode = RecursiveMode::NonRecursive;
        }
        if path.is_dir() && !desired.contains(&(path.clone(), mode)) {
            desired.push((path, mode));
        }
    }
    desired
}
#[cfg(target_os = "linux")]
const OPENER: &str = "xdg-open";
#[cfg(target_os = "macos")]
const OPENER: &str = "open";
/// Spawn the desktop opener with a literal path argument, never shell source.
fn spawn_opener(path: &Path, opener: &str, args: &[&str]) -> Result<std::process::Child, String> {
    std::process::Command::new(opener)
        .args(args)
        .arg(path)
        .stdin(std::process::Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())
}
/// Called on a worker: reap the opener and report missing-handler failures. In generic
/// mode xdg-open can wait for the viewer to exit; these files are not app-owned sessions.
pub(crate) fn open_file(path: &Path) -> Result<(), String> {
    open_file_with(path, OPENER, &[])
}
fn open_file_with(path: &Path, opener: &str, args: &[&str]) -> Result<(), String> {
    let status = spawn_opener(path, opener, args)
        .and_then(|mut child| child.wait().map_err(|e| e.to_string()))
        .map_err(|e| format!("Could not open {} with {opener}: {e}", path.display()))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "Could not open {}: {opener} {status}",
            path.display()
        ))
    }
}
/// Launchable files are shown in the file manager, never passed to their default handler.
#[cfg(target_os = "macos")]
pub(crate) fn reveal_file(path: &Path) -> Result<(), String> {
    open_file_with(path, OPENER, &["-R"])
}
#[cfg(target_os = "linux")]
pub(crate) fn reveal_file(path: &Path) -> Result<(), String> {
    reveal_file_with(path, zbus::blocking::connection::Builder::session(), OPENER)
}
#[cfg(target_os = "linux")]
fn reveal_file_with(
    path: &Path,
    builder: zbus::Result<zbus::blocking::connection::Builder<'_>>,
    opener: &str,
) -> Result<(), String> {
    let uri = tauri::Url::from_file_path(path)
        .map_err(|()| format!("Could not reveal {}: invalid absolute path", path.display()))?;
    let shown = builder
        .and_then(|builder| {
            builder
                .method_timeout(std::time::Duration::from_secs(25))
                .build()
        })
        .and_then(|connection| {
            connection
                .call_method(
                    Some("org.freedesktop.FileManager1"),
                    "/org/freedesktop/FileManager1",
                    Some("org.freedesktop.FileManager1"),
                    "ShowItems",
                    // zvariant encodes fixed Rust arrays as structs; ShowItems needs `as`.
                    &([uri.as_str()].as_slice(), ""),
                )?
                .body()
                .deserialize::<()>()
        });
    match shown {
        Ok(()) => return Ok(()),
        Err(zbus::Error::MethodError(name, _, _))
            if matches!(
                name.as_str(),
                "org.freedesktop.DBus.Error.ServiceUnknown"
                    | "org.freedesktop.DBus.Error.UnknownMethod"
                    | "org.freedesktop.DBus.Error.NameHasNoOwner"
            ) => {}
        Err(error) => {
            return Err(format!("Could not reveal {}: {error}", path.display()));
        }
    }
    // Missing/unsupported file-manager service: only open the containing directory.
    let parent = path.parent().ok_or_else(|| {
        format!(
            "Could not reveal {}: no containing directory",
            path.display()
        )
    })?;
    open_file_with(parent, opener, &[])
        .map_err(|error| format!("Could not reveal {}: {error}", path.display()))
}
/// Create the config directory if needed and open it with the desktop's `opener`.
fn reveal(dir: &Path, opener: &str) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    spawn_opener(dir, opener, &[]).map(|mut child| {
        thread::spawn(move || {
            let _ = child.wait();
        });
    })
}
pub fn wait_reply<T>(rx: mpsc::Receiver<Result<T, String>>) -> Result<T, String> {
    rx.recv()
        .map_err(|_| "integration request dropped".to_owned())?
}

#[cfg(test)]
mod tests;
