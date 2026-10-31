//! Native item/project authority. All mutation and embed calls are main-thread-only.
//! Lifecycle state is lossless; only bells have a bounded FIFO.
use crate::{
    config::{
        Settings, SettingsError, SettingsState, StatusSource, ToolIcon, ToolKind, ToolSpec,
        TrayShow,
    },
    discovery::{Conversation, ConversationList, StoreError},
    launch,
    omp_link::{self, Credentials, Delivery, Message},
    platform::{self, MainWindow},
    runtime::{self, ConfigUpdate, Request},
    shortcuts::{Action, Key, Keymap, Mods},
    status::{Activity, Attention},
    ui_state::{self, UiStateFile},
};
use alacritty_embed::embed::{self, Launch, SessionKey, TerminalState, ViewportRect};
use serde::{Deserialize, Serialize};
use std::{
    cell::{Cell, RefCell},
    collections::{BTreeMap, HashMap, VecDeque},
    ffi::c_void,
    path::{Path, PathBuf},
    rc::Rc,
};
use tauri::{AppHandle, Emitter, Manager};

mod file_links;
use file_links::{FileOrigin, FileTarget};

pub const WORKSPACE_STATE_EVENT: &str = "workspace-state";
pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationRef {
    pub tool: String,
    pub id: String,
    pub path: String,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Process {
    Starting,
    Running,
    Exited,
    Failed,
}
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemState {
    pub id: String,
    pub tool: String,
    pub kind: ToolKind,
    pub project_id: String,
    pub cwd: String,
    pub title: String,
    pub process: Process,
    pub exit_code: Option<i32>,
    pub failure: Option<String>,
    pub activity: Activity,
    pub attention: Option<Attention>,
    pub unread: bool,
    pub conversation: Option<ConversationRef>,
    pub model: Option<String>,
    pub started_at: u64,
    pub state_since: u64,
    pub pid: Option<u32>,
    /// Shell items only: the job holding the terminal instead of the shell (board 21, decision 20).
    pub foreground_job: Option<ForegroundJobState>,
    /// OMP items while `on-job`: the background jobs the agent waits on, oldest first.
    pub background_jobs: Vec<BackgroundJob>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ForegroundJobState {
    /// The group leader's command name (`cargo`, `ssh`).
    pub command: String,
    pub since_ms: u64,
}
/// A background job an OMP agent waits on (decision 22).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackgroundJob {
    /// OMP's job type (`bash`, `task`).
    pub kind: String,
    pub label: String,
    pub started_at_ms: u64,
}
impl From<omp_link::Job> for BackgroundJob {
    fn from(job: omp_link::Job) -> Self {
        Self {
            kind: job.job_type,
            label: job.label,
            started_at_ms: job.started_at,
        }
    }
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectState {
    pub id: String,
    pub name: String,
    pub path: String,
    pub pinned: bool,
    pub missing: bool,
    pub default_tool: String,
    pub item_ids: Vec<String>,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolInfo {
    pub(crate) id: String,
    pub(crate) label: String,
    pub(crate) glyph: String,
    icon: Option<ToolIcon>,
    accent: Option<String>,
    kind: ToolKind,
    status: StatusSource,
    can_resume: bool,
    available: Option<bool>,
    path: Option<String>,
    unavailable_reason: Option<String>,
}
impl ToolInfo {
    fn new(t: &ToolSpec) -> Self {
        Self {
            id: t.id.clone(),
            label: t.label.clone(),
            glyph: t.glyph.clone(),
            icon: t.icon.clone(),
            accent: t.accent.clone(),
            kind: t.kind,
            status: t.status,
            can_resume: t.resume.is_some(),
            available: None,
            path: None,
            unavailable_reason: None,
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum CloseReason {
    AgentWorking,
    AgentNeedsYou,
    ForegroundJob,
    Neovim,
    UnmonitoredAgent,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase"
)]
pub enum OpenableSheet {
    Palette {
        project_id: Option<String>,
        mode: PaletteMode,
    },
    Settings,
    Shortcuts,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PaletteMode {
    Open,
    New,
}
#[derive(Debug, Clone, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase"
)]
pub enum Sheet {
    Palette {
        project_id: Option<String>,
        mode: PaletteMode,
    },
    Settings,
    Shortcuts,
    ConfirmClose {
        item_id: String,
        reason: CloseReason,
        detail: Option<String>,
    },
    Quit {
        item_ids: Vec<String>,
    },
}
impl From<OpenableSheet> for Sheet {
    fn from(s: OpenableSheet) -> Self {
        match s {
            OpenableSheet::Palette { project_id, mode } => Self::Palette { project_id, mode },
            OpenableSheet::Settings => Self::Settings,
            OpenableSheet::Shortcuts => Self::Shortcuts,
        }
    }
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Notice {
    id: String,
    kind: &'static str,
    title: String,
    body: String,
    project_id: Option<String>,
    conversation: Option<ConversationRef>,
    at: u64,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LastSetEntry {
    tool: String,
    title: String,
    project_id: String,
    cwd: String,
    conversation: ConversationRef,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UiState {
    sidebar_expanded: bool,
    folded_project_ids: Vec<String>,
}
#[derive(Debug, Clone, Serialize)]
pub struct Shortcut {
    pub(crate) action: Action,
    pub(crate) label: Option<String>,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceState {
    pub revision: u64,
    pub presentation_epoch: u64,
    pub view: &'static str,
    pub sheet: Option<Sheet>,
    pub projects: Vec<ProjectState>,
    pub items: Vec<ItemState>,
    pub active_item_id: Option<String>,
    pub recent_item_ids: Vec<String>,
    pub tools: Vec<ToolInfo>,
    pub provider_icons: BTreeMap<String, String>,
    pub notices: Vec<Notice>,
    pub last_set: Option<Vec<LastSetEntry>>,
    pub ui: UiState,
    pub shortcuts: Vec<Shortcut>,
    pub default_cwd: String,
    pub host_error: Option<String>,
}
struct Record {
    key: Option<SessionKey>,
    state: ItemState,
    spec: ToolSpec,
    /// OMP items: what the status link reported.
    omp: Option<OmpStatus>,
    raw_title: String,
    conversation_title: Option<String>,
    closing: bool,
    extra: Option<Vec<String>>,
    /// A foreground group seen on the last poll but not yet published: (pgid, command, first seen).
    job_candidate: Option<(i32, String, u64)>,
}
/// What an OMP item's status link last reported.
#[derive(Default)]
struct OmpStatus {
    /// The newest connection heard from (0: none yet); older connections are stale.
    connection: u64,
    session: Option<String>,
    state: Option<omp_link::State>,
    name: Option<String>,
    /// OMP reported the item's model (possibly none); discovery no longer sets it.
    model_reported: bool,
}
#[derive(Clone, Copy, PartialEq, Eq)]
struct AppliedReport {
    key: SessionKey,
    epoch: u64,
    sequence: u64,
}
/// Everything the workspace model does outside its own state: app events, the main window,
/// embedded terminal sessions, launch preparation and desktop notifications. The app uses
/// [`Native`]; keeping these behind one boundary lets the model run headless in tests.
trait Host {
    /// Publish a changed snapshot: window title, tray and the `workspace-state` event.
    fn publish(&self, state: WorkspaceState);
    fn emit(&self, event: &str, payload: serde_json::Value);
    /// Resolve a launch off the main thread, then run [`finish_launch`] on it.
    fn prepare(
        &self,
        id: String,
        spec: ToolSpec,
        cwd: String,
        args: Vec<String>,
        link: Option<Credentials>,
    );
    /// Classify a clicked local file off-main, then dispatch on main after rechecking its origin.
    fn prepare_file(&self, origin: FileOrigin, uri: String);
    /// Open a validated file with the desktop default off-main, reporting failures on main.
    fn open_file(&self, path: String);
    /// Reveal a validated launcher in the file manager off-main, never execute it.
    fn reveal_file(&self, path: String);
    /// Status-link credentials for an OMP launch of `item`, if the link runs.
    fn link(&self, item: &str) -> Option<Credentials>;
    /// Invalidate `item`'s status-link token.
    fn unlink(&self, item: &str);
    fn create(&self, launch: Launch) -> Result<SessionKey, String>;
    fn state(&self, key: SessionKey) -> Option<TerminalState>;
    fn foreground_job(&self, key: SessionKey) -> Option<embed::ForegroundJob>;
    fn stop(&self, key: SessionKey) -> Result<(), String>;
    fn remove(&self, key: SessionKey) -> Result<(), String>;
    fn hide(&self, key: SessionKey);
    fn is_shown(&self, key: SessionKey) -> bool;
    fn set_viewport(
        &self,
        key: SessionKey,
        rect: ViewportRect,
        visible: bool,
    ) -> Result<(), String>;
    fn focus(&self, key: SessionKey) -> bool;
    fn reload_config(&self, toml: &str) -> Result<(), String>;
    /// Move keyboard focus to the web view's sidebar.
    fn focus_sidebar(&self);
    fn window_presentable(&self) -> bool;
    fn window_focused(&self) -> bool;
    fn notify(&self, item: &str, summary: &str, body: &str);
    fn keep_awake(&self, reason: Option<&'static str>);
    /// Probe pinned roots off the main thread, then [`apply_pinned_missing`] on it.
    fn probe_pinned(&self, pinned: Vec<PathBuf>);
}
/// The app's [`Host`]: the Tauri app, its main window and the embedded Alacritty host.
struct Native {
    app: AppHandle,
    window: RefCell<Option<MainWindow>>,
    web_view: Cell<*mut c_void>,
}
impl Host for Native {
    fn publish(&self, state: WorkspaceState) {
        if let Some(window) = self.app.get_webview_window(crate::lifecycle::MAIN_WINDOW) {
            let _ = window.set_title(&window_title(&state));
        }
        crate::lifecycle::update_tray(&self.app, &state);
        crate::control::publish(&state);
        if let Err(e) = self.app.emit(WORKSPACE_STATE_EVENT, state) {
            log::warn!("workspace event: {e}");
        }
    }
    fn emit(&self, event: &str, payload: serde_json::Value) {
        let _ = self.app.emit(event, payload);
    }
    fn prepare(
        &self,
        id: String,
        spec: ToolSpec,
        cwd: String,
        args: Vec<String>,
        link: Option<Credentials>,
    ) {
        std::thread::spawn(move || {
            // Without its token file the launch still runs, only without status.
            let link = link.filter(|link| {
                omp_link::issue(link)
                    .inspect_err(|e| log::warn!("OMP status link: {e}"))
                    .is_ok()
            });
            let prepared = launch::prepare(&spec, &cwd, args, link.as_ref());
            platform::defer_on_main(Box::new(move || finish_launch(&id, prepared)));
        });
    }
    fn prepare_file(&self, origin: FileOrigin, uri: String) {
        std::thread::spawn(move || {
            let target = file_links::validate(&uri, &origin.cwd);
            platform::defer_on_main(Box::new(move || finish_file_link(origin, target)));
        });
    }
    fn open_file(&self, path: String) {
        std::thread::spawn(move || {
            let result = runtime::open_file(Path::new(&path));
            platform::defer_on_main(Box::new(move || finish_system_file(result)));
        });
    }
    fn reveal_file(&self, path: String) {
        std::thread::spawn(move || {
            let result = runtime::reveal_file(Path::new(&path));
            platform::defer_on_main(Box::new(move || finish_system_file(result)));
        });
    }
    fn link(&self, item: &str) -> Option<Credentials> {
        omp_link::register(item)
    }
    fn unlink(&self, item: &str) {
        omp_link::forget(item);
    }
    fn create(&self, launch: Launch) -> Result<SessionKey, String> {
        embed::create(launch)
    }
    fn state(&self, key: SessionKey) -> Option<TerminalState> {
        embed::state(key)
    }
    fn foreground_job(&self, key: SessionKey) -> Option<embed::ForegroundJob> {
        embed::foreground_job(key)
    }
    fn stop(&self, key: SessionKey) -> Result<(), String> {
        embed::stop(key)
    }
    fn remove(&self, key: SessionKey) -> Result<(), String> {
        embed::remove(key)
    }
    fn hide(&self, key: SessionKey) {
        embed::hide(key);
    }
    fn is_shown(&self, key: SessionKey) -> bool {
        embed::is_shown(key)
    }
    fn set_viewport(
        &self,
        key: SessionKey,
        rect: ViewportRect,
        visible: bool,
    ) -> Result<(), String> {
        embed::set_viewport(key, rect, visible)
    }
    fn focus(&self, key: SessionKey) -> bool {
        embed::focus(key)
    }
    fn reload_config(&self, toml: &str) -> Result<(), String> {
        embed::reload_config(toml)
    }
    fn focus_sidebar(&self) {
        let view = self.web_view.get();
        if !view.is_null() {
            unsafe { platform::focus_webview(view) };
            let _ = self.app.emit(
                "ui-command",
                serde_json::json!({"command": "focus-sidebar"}),
            );
        }
    }
    fn window_presentable(&self) -> bool {
        self.window
            .borrow()
            .as_ref()
            .is_some_and(platform::window_presentable)
    }
    fn window_focused(&self) -> bool {
        #[cfg(target_os = "macos")]
        return self
            .window
            .borrow()
            .as_ref()
            .is_some_and(|window| window.isKeyWindow());
        #[cfg(target_os = "linux")]
        self.app
            .get_webview_window(crate::lifecycle::MAIN_WINDOW)
            .is_some_and(|win| win.is_focused().unwrap_or(false))
    }
    fn notify(&self, item: &str, summary: &str, body: &str) {
        crate::notify::desktop(item, summary, body);
    }
    fn keep_awake(&self, reason: Option<&'static str>) {
        crate::awake::hold(reason);
    }
    fn probe_pinned(&self, pinned: Vec<PathBuf>) {
        std::thread::spawn(move || {
            let missing = pinned_missing(&pinned);
            platform::defer_on_main(Box::new(move || apply_pinned_missing(&pinned, missing)));
        });
    }
}
struct Workspace {
    host: Rc<dyn Host>,
    host_error: Option<String>,
    default_cwd: String,
    records: Vec<Record>,
    active: Option<String>,
    recent: Vec<String>,
    sheet: Option<Sheet>,
    project_order: Vec<String>,
    pinned_missing: HashMap<String, bool>,
    next_item: u64,
    next_notice: u64,
    revision: u64,
    epoch: u64,
    last_sequence: Option<u64>,
    applied: Option<AppliedReport>,
    changed: bool,
    settings: Settings,
    settings_state: Option<SettingsState>,
    keys: Keymap,
    tools: Vec<ToolInfo>,
    terminal_config: String,
    ui: UiStateFile,
    notices: Vec<Notice>,
    conversations: Vec<Conversation>,
    store_errors: Vec<StoreError>,
    discovery_revision: u64,
    scanning: bool,
    waiting_cycle: Vec<String>,
    waiting_cursor: Option<String>,
    quitting: bool,
}
enum HostUpdate {
    State(TerminalState),
    Bell,
    OpenFile(String),
}
#[derive(Default)]
struct HostQueue {
    events: VecDeque<(SessionKey, HostUpdate)>,
    discrete: usize,
}
impl HostQueue {
    fn push(&mut self, key: SessionKey, update: HostUpdate) {
        if matches!(update, HostUpdate::Bell) {
            if self.discrete == 512 {
                // Only bells are expendable; lifecycle state and file clicks stay ordered.
                let oldest = self
                    .events
                    .iter()
                    .position(|(_, event)| matches!(event, HostUpdate::Bell))
                    .unwrap();
                self.events.remove(oldest);
            } else {
                self.discrete += 1;
            }
        }
        self.events.push_back((key, update));
    }
}
thread_local! {
    static WORKSPACE: RefCell<Option<Workspace>> = const { RefCell::new(None) };
    static NATIVE: RefCell<Option<Rc<Native>>> = const { RefCell::new(None) };
    static HOST_EVENTS: RefCell<HostQueue> = RefCell::new(HostQueue::default());
    static FLUSH_SCHEDULED: Cell<bool> = const { Cell::new(false) };
}
fn native() -> Result<Rc<Native>, String> {
    NATIVE
        .with(|cell| cell.borrow().clone())
        .ok_or_else(|| "workspace is not initialized".to_owned())
}
fn with<T>(f: impl FnOnce(&mut Workspace) -> Result<T, String>) -> Result<T, String> {
    let (result, emit, conversations) = WORKSPACE.with(|cell| {
        let mut guard = cell
            .try_borrow_mut()
            .map_err(|_| "workspace is busy".to_owned())?;
        let w = guard.as_mut().ok_or("workspace is not initialized")?;
        let discovery_revision = w.discovery_revision;
        let result = f(w);
        let emit = if w.changed {
            w.changed = false;
            w.revision += 1;
            let state = w.snapshot();
            let reason = crate::awake::reason(w.settings.keep_awake, &state.items);
            Some((w.host.clone(), state, reason))
        } else {
            None
        };
        let conversations = (discovery_revision != w.discovery_revision)
            .then(|| (w.host.clone(), w.discovery_revision));
        Ok::<_, String>((result, emit, conversations))
    })?;
    if let Some((host, state, reason)) = emit {
        host.keep_awake(reason);
        host.publish(state);
    }
    if let Some((host, revision)) = conversations {
        host.emit(
            "conversations-changed",
            serde_json::json!({"revision": revision}),
        );
    }
    result
}
/// Main window title: the active item and its project, else the app name.
fn window_title(state: &WorkspaceState) -> String {
    state
        .active_item_id
        .as_ref()
        .and_then(|id| state.items.iter().find(|i| &i.id == id))
        .map(|item| {
            let project = state
                .projects
                .iter()
                .find(|p| p.id == item.project_id)
                .map(|p| p.name.as_str())
                .unwrap_or("");
            format!("{} — {project}", item.title)
        })
        .unwrap_or_else(|| "Ultra 9000".into())
}
pub fn init(app: AppHandle) {
    let native = Rc::new(Native {
        app,
        window: RefCell::new(None),
        web_view: Cell::new(std::ptr::null_mut()),
    });
    NATIVE.with(|cell| *cell.borrow_mut() = Some(native.clone()));
    WORKSPACE.with(|cell| *cell.borrow_mut() = Some(Workspace::new(native)));
    runtime::start();
}
/// # Safety
/// `web_view` is the live platform webview, on the main thread.
pub unsafe fn attach_terminal_host(web_view: *mut c_void) {
    let config =
        with(|w| Ok(w.terminal_config.clone())).unwrap_or_else(|_| runtime::BUNDLED.into());
    let host = unsafe { embed::init(web_view, &config, Box::new(host_event)) };
    let window = unsafe { platform::window_of_view(web_view) };
    #[cfg(target_os = "macos")]
    if let Some(window) = &window {
        crate::lifecycle::install_macos_key_router(window.clone());
    }
    if let Ok(native) = native() {
        *native.window.borrow_mut() = window;
        native.web_view.set(web_view);
    }
    let _ = with(|w| {
        w.host_error = host
            .err()
            .map(|e| format!("The terminal host failed to start: {e}"));
        w.changed = true;
        Ok(())
    });
    // Foreground jobs change without any terminal event (a silent `sleep`), so poll them.
    std::thread::spawn(|| loop {
        std::thread::sleep(std::time::Duration::from_secs(1));
        platform::defer_on_main(Box::new(poll_jobs));
    });
}
/// Reads each running shell's PTY foreground group (an OS fact, never output). The snapshot is
/// emitted only when the published job changes.
fn poll_jobs() {
    let _ = with(|w| {
        let at = now();
        for record in &mut w.records {
            let seen = match (
                record.state.tool.as_str(),
                record.state.process,
                record.key,
                record.state.pid,
            ) {
                ("shell", Process::Running, Some(key), Some(pid)) => w
                    .host
                    .foreground_job(key)
                    .filter(|job| u32::try_from(job.pgid).ok() != Some(pid))
                    .map(|job| (job.pgid, job.name.unwrap_or_else(|| "job".into()))),
                _ => None,
            };
            let job = next_job(
                &record.state.foreground_job,
                &mut record.job_candidate,
                seen,
                at,
            );
            if record.state.foreground_job != job {
                record.state.foreground_job = job;
                w.changed = true;
            }
        }
        Ok(())
    });
}
/// Debounce (board 21): a job is published once the same group holds the terminal on two polls in
/// a row, so a quick `ls` never flashes; idle is published on the first idle poll.
fn next_job(
    current: &Option<ForegroundJobState>,
    candidate: &mut Option<(i32, String, u64)>,
    seen: Option<(i32, String)>,
    at: u64,
) -> Option<ForegroundJobState> {
    let Some((pgid, command)) = seen else {
        *candidate = None;
        return None;
    };
    match candidate {
        Some((last, _, since)) if *last == pgid => Some(ForegroundJobState {
            command,
            since_ms: *since,
        }),
        _ => {
            *candidate = Some((pgid, command, at));
            // A different group took over: keep showing the old job until the new one settles.
            current.clone()
        }
    }
}
pub fn snapshot() -> Result<WorkspaceState, String> {
    with(|w| Ok(w.snapshot()))
}
pub fn settings_snapshot() -> Result<SettingsState, String> {
    with(|w| {
        w.settings_state
            .clone()
            .ok_or_else(|| "Settings are still loading; try again.".into())
    })
}
pub fn settings() -> Result<Settings, String> {
    with(|w| Ok(w.settings.clone()))
}
pub fn notify_needs_you() -> Result<bool, String> {
    with(|w| Ok(w.settings.notify_needs_you))
}
pub fn tray_dark_icon() -> Result<bool, String> {
    with(|w| Ok(w.settings.tray_icon == "dark"))
}
pub fn tray_show() -> Result<TrayShow, String> {
    with(|w| Ok(w.settings.tray_show))
}
pub fn app_handle() -> Result<AppHandle, String> {
    native().map(|native| native.app.clone())
}
pub fn apply_config(update: ConfigUpdate) {
    let state = update.state.clone();
    let result = with(|w| {
        if let Some(text) = update.terminal {
            if text != w.terminal_config {
                if w.host_error.is_none() {
                    if let Err(message) = w.host.reload_config(&text) {
                        let _ = runtime::send(Request::Report(SettingsError {
                            file: format!("{}/terminal.toml", state.config_dir),
                            line: None,
                            key: None,
                            message,
                            last_good: None,
                        }));
                    } else {
                        w.terminal_config = text;
                    }
                } else {
                    w.terminal_config = text;
                }
            }
        }
        let previous = std::mem::take(&mut w.tools);
        w.tools = update
            .settings
            .tools
            .iter()
            .map(|spec| {
                if w.settings.tool(&spec.id) == Some(spec) {
                    previous
                        .iter()
                        .find(|t| t.id == spec.id)
                        .cloned()
                        .unwrap_or_else(|| ToolInfo::new(spec))
                } else {
                    ToolInfo::new(spec)
                }
            })
            .collect();
        w.settings = update.settings;
        w.keys = update.keys;
        w.settings_state = Some(update.state);
        w.changed = true;
        Ok((w.host.clone(), w.settings.pinned.clone()))
    });
    if let Ok((host, pinned)) = result {
        if let Ok(payload) = serde_json::to_value(&state) {
            host.emit("settings-state", payload);
        }
        // Filesystem metadata stays on a worker; stale pin probes are ignored when posted.
        host.probe_pinned(pinned);
    }
}
/// Whether each pinned root is missing (not a directory), keyed by project id.
fn pinned_missing(pinned: &[PathBuf]) -> HashMap<String, bool> {
    pinned
        .iter()
        .map(|p| (p.to_string_lossy().into_owned(), !p.is_dir()))
        .collect()
}
/// Applies a pin probe unless the pinned list changed while it ran.
fn apply_pinned_missing(pinned: &[PathBuf], missing: HashMap<String, bool>) {
    let _ = with(|w| {
        if w.settings.pinned == pinned && w.pinned_missing != missing {
            w.pinned_missing = missing;
            w.changed = true;
        }
        Ok(())
    });
}
pub fn apply_settings_state(state: SettingsState) {
    let host = with(|w| {
        w.settings_state = Some(state.clone());
        Ok(w.host.clone())
    });
    if let Ok(host) = host {
        if let Ok(payload) = serde_json::to_value(&state) {
            host.emit("settings-state", payload);
        }
    }
}
pub fn tool_resolved(spec: ToolSpec, result: Result<String, String>) {
    let _ = with(|w| {
        if w.settings.tool(&spec.id) == Some(&spec) {
            if let Some(info) = w.tools.iter_mut().find(|t| t.id == spec.id) {
                info.available = Some(result.is_ok());
                match result {
                    Ok(path) => {
                        info.path = Some(path);
                        info.unavailable_reason = None;
                    }
                    Err(e) => {
                        info.path = None;
                        info.unavailable_reason = Some(e);
                    }
                }
                w.changed = true;
            }
        }
        Ok(())
    });
}
pub fn load_ui(ui: UiStateFile) {
    let _ = with(|w| {
        w.ui = ui;
        w.changed = true;
        Ok(())
    });
}
pub fn apply_discovery(conversations: Vec<Conversation>, errors: Vec<StoreError>) {
    let _ = with(|w| {
        if !w.scanning && w.conversations == conversations && w.store_errors == errors {
            return Ok(());
        }
        w.conversations = conversations;
        w.store_errors = errors;
        w.scanning = false;
        w.discovery_revision += 1;
        for record in &mut w.records {
            if let Some(reference) = &record.state.conversation {
                if let Some(c) = w.conversations.iter().find(|c| {
                    c.tool == reference.tool
                        && c.id == reference.id
                        && c.path.to_string_lossy() == reference.path
                }) {
                    record.conversation_title = Some(c.title.clone());
                    let title = record.title();
                    // A model OMP reported itself is live; the saved one may lag behind it.
                    let model = if record.omp.as_ref().is_some_and(|o| o.model_reported) {
                        &record.state.model
                    } else {
                        &c.model
                    };
                    if record.state.title != title || record.state.model != *model {
                        record.state.model = model.clone();
                        record.state.title = title;
                        w.changed = true;
                    }
                }
            }
        }
        Ok(())
    });
}
pub fn conversations_list(
    project: Option<String>,
    query: Option<String>,
    limit: Option<usize>,
) -> Result<ConversationList, String> {
    with(|w| {
        let query = query.unwrap_or_default().to_lowercase();
        let conversations =
            matching_conversations(&w.conversations, project.as_deref(), &query, &w.settings)
                .take(limit.unwrap_or(200))
                .map(|c| {
                    let running = w.bound_item(&c.tool, &c.id, &c.path.to_string_lossy());
                    let elsewhere = running.is_none()
                        && now().saturating_sub(c.updated_at_ms.max(0) as u64)
                            < w.settings.open_elsewhere_hint_seconds.saturating_mul(1000);
                    c.entry(running, elsewhere)
                })
                .collect();
        Ok(ConversationList {
            revision: w.discovery_revision,
            conversations,
            errors: w.store_errors.clone(),
            scanning: w.scanning,
        })
    })
}
fn matching_conversations<'a>(
    conversations: &'a [Conversation],
    project: Option<&'a str>,
    query: &'a str,
    settings: &'a Settings,
) -> impl Iterator<Item = &'a Conversation> {
    conversations
        .iter()
        .filter(move |c| project.is_none_or(|p| c.project_root == Path::new(p)))
        .filter(move |c| {
            query.is_empty()
                || c.title.to_lowercase().contains(query)
                || settings
                    .project_name(&c.project_root)
                    .to_lowercase()
                    .contains(query)
                || c.model
                    .as_ref()
                    .is_some_and(|m| m.to_lowercase().contains(query))
        })
}
pub fn select_bound(tool: &str, id: &str, path: &str) -> Result<Option<String>, String> {
    with(|w| {
        let item = w.bound_item(tool, id, path);
        if let Some(item) = &item {
            w.select(item)?;
        }
        Ok(item)
    })
}
/// `cwd` and project were validated/resolved off-main. Row is published before PATH work.
pub fn begin_launch(
    tool: String,
    cwd: String,
    project: String,
    conversation: Option<Conversation>,
    anchor: Option<String>,
    replacement: Option<String>,
    extra: Option<Vec<String>>,
) -> Result<String, String> {
    with(|w| {
        if w.quitting {
            return Err("The application is quitting.".into());
        }
        if let Some(id) = w.reuse_bound(&tool, conversation.as_ref(), replacement.as_deref())? {
            return Ok(id);
        }
        let spec = w
            .settings
            .tool(&tool)
            .cloned()
            .ok_or_else(|| format!("Unknown tool: {tool}"))?;
        if conversation.is_some() && spec.resume.is_none() {
            return Err("This tool cannot resume conversations.".into());
        }
        let extra_saved = extra.clone();
        let args = launch_args(&spec, conversation.as_ref(), &cwd, extra);
        let reference = conversation.as_ref().map(|c| ConversationRef {
            tool: tool.clone(),
            id: c.id.clone(),
            path: c.path.to_string_lossy().into(),
        });
        let at = now();
        let id = format!("item-{}", w.next_item);
        w.next_item += 1;
        let omp = spec.status == StatusSource::Omp;
        let mut record = Record {
            key: None,
            spec: spec.clone(),
            omp: omp.then(OmpStatus::default),
            raw_title: String::new(),
            conversation_title: conversation.as_ref().map(|c| c.title.clone()),
            closing: false,
            extra: extra_saved,
            job_candidate: None,
            state: ItemState {
                id: id.clone(),
                tool,
                kind: spec.kind,
                project_id: project.clone(),
                cwd: cwd.clone(),
                title: spec.label.clone(),
                process: Process::Starting,
                exit_code: None,
                failure: None,
                activity: Activity::Unknown,
                attention: None,
                unread: false,
                conversation: reference,
                model: conversation.as_ref().and_then(|c| c.model.clone()),
                started_at: at,
                state_since: at,
                pid: None,
                foreground_job: None,
                background_jobs: Vec::new(),
            },
        };
        record.state.title = record.title();
        let index = w.launch_index(replacement, spec.kind, anchor.as_deref(), &project)?;
        if !w.project_order.contains(&project) {
            w.project_order.push(project);
        }
        w.records.insert(index, record);
        if w.records[index].state.conversation.is_some() {
            w.discovery_revision += 1;
        }
        w.select(&id)?;
        let link = if omp { w.host.link(&id) } else { None };
        w.host.prepare(id.clone(), spec, cwd, args, link);
        Ok(id)
    })
}
fn finish_launch(id: &str, prepared: Result<Launch, String>) {
    let _ = with(|w| {
        let Some(index) = w
            .records
            .iter()
            .position(|r| r.state.id == id && r.state.process == Process::Starting)
        else {
            return Ok(());
        };
        let launch = prepared
            .and_then(|launch| w.host_error.as_ref().map_or(Ok(launch), |e| Err(e.clone())));
        let result = launch.and_then(|launch| w.host.create(launch));
        let record = &mut w.records[index];
        match result {
            Ok(key) => {
                record.key = Some(key);
                record.state.process = Process::Running;
                if let Some(state) = w.host.state(key) {
                    record.state.pid = state.pid;
                    record.raw_title = state.title;
                    record.state.title = record.title();
                }
            }
            Err(failure) => {
                record.state.process = Process::Failed;
                record.state.failure = Some(failure.clone());
                if record.omp.is_some() {
                    w.host.unlink(id);
                }
                w.error_notice(failure);
            }
        }
        if w.records[index].state.conversation.is_some() {
            w.discovery_revision += 1;
        }
        w.records[index].state.state_since = now();
        w.changed = true;
        w.refresh_recent();
        Ok(())
    });
}
/// Tool, cwd, project id, resumable conversation and extra launch args of an item to restart.
pub type RestartTarget = (
    String,
    String,
    String,
    Option<Conversation>,
    Option<Vec<String>>,
);
pub fn restart_target(id: &str) -> Result<RestartTarget, String> {
    with(|w| {
        let r = &w.records[w.index(id)?];
        if !matches!(r.state.process, Process::Exited | Process::Failed) {
            return Err("Only exited or failed items can restart.".into());
        }
        let c = r.state.conversation.as_ref().map(|reference| {
            w.conversations
                .iter()
                .find(|c| {
                    c.tool == reference.tool
                        && c.id == reference.id
                        && c.path.to_string_lossy() == reference.path
                })
                .cloned()
                .unwrap_or(Conversation {
                    tool: reference.tool.clone(),
                    id: reference.id.clone(),
                    path: PathBuf::from(&reference.path),
                    title: r
                        .conversation_title
                        .clone()
                        .unwrap_or_else(|| r.state.title.clone()),
                    cwd: PathBuf::from(&r.state.cwd),
                    cwd_missing: false,
                    project_root: PathBuf::from(&r.state.project_id),
                    created_at_ms: r.state.started_at as i64,
                    updated_at_ms: now() as i64,
                    model: r.state.model.clone(),
                })
        });
        Ok((
            r.state.tool.clone(),
            r.state.cwd.clone(),
            r.state.project_id.clone(),
            c,
            r.extra.clone(),
        ))
    })
}
pub fn select(id: &str) -> Result<(), String> {
    with(|w| w.select(id))
}
/// Whether `id` is a row of the sidebar.
pub fn has_item(id: &str) -> Result<bool, String> {
    with(|w| Ok(w.index(id).is_ok()))
}
/// Empty menu-bar popovers launch in the configured default directory/tool, not an arbitrary
/// unlisted project or whichever live item happens to be selected in another window.
pub fn default_launch() -> Result<(String, String), String> {
    with(|w| Ok((w.default_cwd.clone(), w.settings.default_tool.clone())))
}

/// The default tool of a project the sidebar lists, `None` for any other project id.
pub fn listed_project_default_tool(project_id: &str) -> Result<Option<String>, String> {
    with(|w| {
        Ok(w.projects()
            .into_iter()
            .find(|p| p.id == project_id)
            .map(|p| p.default_tool))
    })
}
pub fn sheet_open(sheet: OpenableSheet) -> Result<(), String> {
    with(|w| {
        let before = w.presented();
        w.sheet = Some(sheet.into());
        w.present(before);
        Ok(())
    })
}
pub fn sheet_close() -> Result<(), String> {
    with(|w| {
        let before = w.presented();
        w.sheet = None;
        w.clear_presented_unread();
        w.present(before);
        Ok(())
    })
}
/// What closing an item did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CloseOutcome {
    Closed,
    /// Nothing stopped: the close rules want the user to confirm first.
    NeedsConfirmation(CloseReason),
}
/// Close from the UI: a needed confirmation opens the app's confirmation sheet (`"confirm"`).
pub fn close(id: &str, confirmed: bool) -> Result<&'static str, String> {
    close_item(id, confirmed, true).map(|outcome| match outcome {
        CloseOutcome::Closed => "closed",
        CloseOutcome::NeedsConfirmation(_) => "confirm",
    })
}
/// Close from outside the window (shell control link): the same rules, but a needed
/// confirmation is only reported, never shown in the app.
pub fn close_reporting(id: &str, confirmed: bool) -> Result<CloseOutcome, String> {
    close_item(id, confirmed, false)
}
fn close_item(id: &str, confirmed: bool, ask_in_app: bool) -> Result<CloseOutcome, String> {
    with(|w| {
        let index = w.index(id)?;
        if w.records[index].state.process != Process::Running {
            w.remove(index, false)?;
            return Ok(CloseOutcome::Closed);
        }
        if w.records[index].closing {
            return Ok(CloseOutcome::Closed);
        }
        let record = &w.records[index];
        let job = if record.state.tool == "shell" {
            record.key.and_then(|key| w.host.foreground_job(key))
        } else {
            None
        };
        if !confirmed {
            if let Some(reason) = close_reason(
                &record.state,
                record.spec.status,
                &w.settings,
                job.is_some(),
            ) {
                if !ask_in_app {
                    return Ok(CloseOutcome::NeedsConfirmation(reason));
                }
                if reason == CloseReason::ForegroundJob {
                    w.publish_shell_executable(index);
                }
                let before = w.presented();
                w.sheet = Some(Sheet::ConfirmClose {
                    item_id: id.into(),
                    reason: reason.clone(),
                    detail: job.and_then(|j| j.name),
                });
                w.present(before);
                return Ok(CloseOutcome::NeedsConfirmation(reason));
            }
        }
        if let Some(key) = w.records[index].key {
            w.host.stop(key)?;
        }
        w.records[index].closing = true;
        if matches!(&w.sheet, Some(Sheet::ConfirmClose { item_id, .. }) if item_id == id) {
            let before = w.presented();
            w.sheet = None;
            w.present(before);
        }
        Ok(CloseOutcome::Closed)
    })
}
pub fn close_reason(
    item: &ItemState,
    status: StatusSource,
    settings: &Settings,
    foreground: bool,
) -> Option<CloseReason> {
    if item.process != Process::Running {
        return None;
    }
    if status == StatusSource::Omp {
        match item.activity {
            Activity::Working | Activity::OnJob => return Some(CloseReason::AgentWorking),
            Activity::NeedsYou(_) => return Some(CloseReason::AgentNeedsYou),
            _ => {}
        }
    }
    if item.tool == "neovim" {
        Some(CloseReason::Neovim)
    } else if item.tool == "shell" && foreground && settings.confirm_close_with_foreground_job {
        Some(CloseReason::ForegroundJob)
    } else if item.kind == ToolKind::Agent
        && status != StatusSource::Omp
        && settings.confirm_close_unmonitored_agents
    {
        Some(CloseReason::UnmonitoredAgent)
    } else {
        None
    }
}
pub fn notice_dismiss(id: &str) -> Result<(), String> {
    with(|w| {
        let before = w.notices.len();
        w.notices.retain(|n| n.id != id);
        w.changed |= before != w.notices.len();
        Ok(())
    })
}
pub fn action_error(message: String) {
    let _ = with(|w| {
        w.error_notice(message);
        Ok(())
    });
}
pub fn ui_set(sidebar: Option<bool>, folded: Option<Vec<String>>) -> Result<UiStateFile, String> {
    with(|w| {
        if let Some(value) = sidebar {
            w.ui.sidebar_expanded = value;
        }
        if let Some(value) = folded {
            w.ui.folded_projects = value;
        }
        w.changed = true;
        Ok(w.ui.clone())
    })
}
pub fn last_set_dismiss() -> Result<UiStateFile, String> {
    with(|w| {
        w.ui.last_set.clear();
        w.changed = true;
        Ok(w.ui.clone())
    })
}
pub fn last_set_entries() -> Result<Vec<ui_state::LastSetEntry>, String> {
    with(|w| Ok(w.ui.last_set.clone()))
}
pub fn is_quitting() -> Result<bool, String> {
    with(|w| Ok(w.quitting))
}
pub fn quit_snapshot() -> Result<UiStateFile, String> {
    with(|w| {
        w.quitting = true;
        w.ui.last_set = w
            .records
            .iter()
            .filter(|r| r.state.kind == ToolKind::Agent && r.state.process == Process::Running)
            .filter_map(|r| {
                r.state
                    .conversation
                    .as_ref()
                    .map(|c| ui_state::LastSetEntry {
                        tool: r.state.tool.clone(),
                        title: r.state.title.clone(),
                        project_id: r.state.project_id.clone(),
                        cwd: r.state.cwd.clone(),
                        conversation_id: c.id.clone(),
                        conversation_path: c.path.clone(),
                    })
            })
            .collect();
        Ok(w.ui.clone())
    })
}
pub fn cancel_quitting() {
    let _ = with(|w| {
        w.quitting = false;
        Ok(())
    });
}
pub fn live_items() -> Result<Vec<String>, String> {
    with(|w| {
        Ok(w.records
            .iter()
            .filter(|r| matches!(r.state.process, Process::Starting | Process::Running))
            .map(|r| r.state.id.clone())
            .collect())
    })
}
pub fn open_quit() -> Result<(), String> {
    with(|w| {
        let before = w.presented();
        w.sheet = Some(Sheet::Quit {
            item_ids: w
                .records
                .iter()
                .filter(|r| matches!(r.state.process, Process::Starting | Process::Running))
                .map(|r| r.state.id.clone())
                .collect(),
        });
        w.present(before);
        Ok(())
    })
}
pub fn shutdown_all() {
    let _ = with(|w| {
        w.quitting = true;
        Ok(())
    });
    embed::shutdown_all();
}
pub fn invalidate_focus() {
    let _ = with(|w| {
        w.applied = None;
        Ok(())
    });
}
pub fn window_changed() {
    let _ = with(|w| {
        if !w.host.window_presentable() {
            w.applied = None;
        }
        w.clear_presented_unread();
        Ok(())
    });
    if let Ok(native) = native() {
        crate::lifecycle::refresh_tray(&native.app);
    }
}
pub fn set_viewport(
    id: &str,
    epoch: u64,
    rect: ViewportRect,
    visible: bool,
    sequence: u64,
) -> Result<bool, String> {
    if [rect.x, rect.y, rect.width, rect.height]
        .iter()
        .any(|v| !v.is_finite())
    {
        return Err("viewport values must be finite".into());
    }
    if rect.width < 0. || rect.height < 0. {
        return Err("viewport size must not be negative".into());
    }
    with(|w| {
        let Some(key) = w
            .records
            .iter()
            .find(|r| r.state.id == id)
            .and_then(|r| r.key)
        else {
            return Ok(false);
        };
        if w.active.as_deref() != Some(id)
            || w.epoch != epoch
            || w.last_sequence.is_some_and(|last| sequence <= last)
            || visible && w.sheet.is_some()
        {
            return Ok(false);
        }
        w.host.set_viewport(key, rect, visible)?;
        w.last_sequence = Some(sequence);
        w.applied = (visible && w.host.is_shown(key)).then_some(AppliedReport {
            key,
            epoch,
            sequence,
        });
        if w.applied.is_some() {
            w.clear_presented_unread();
        }
        Ok(true)
    })
}
pub fn focus(id: &str, epoch: u64, sequence: u64) -> Result<bool, String> {
    with(|w| {
        let Some(key) = w
            .records
            .iter()
            .find(|r| r.state.id == id)
            .and_then(|r| r.key)
        else {
            return Ok(false);
        };
        if w.sheet.is_some()
            || w.active.as_deref() != Some(id)
            || w.epoch != epoch
            || w.applied
                != Some(AppliedReport {
                    key,
                    epoch,
                    sequence,
                })
            || !w.host.window_presentable()
        {
            return Ok(false);
        }
        Ok(w.host.focus(key))
    })
}
fn host_event(key: SessionKey, event: embed::HostEvent<'_>) {
    let update = match event {
        embed::HostEvent::State(s) => HostUpdate::State(s.clone()),
        embed::HostEvent::Bell => HostUpdate::Bell,
        embed::HostEvent::OpenFile(uri) => HostUpdate::OpenFile(uri.to_owned()),
    };
    HOST_EVENTS.with(|events| events.borrow_mut().push(key, update));
    if !FLUSH_SCHEDULED.with(|scheduled| scheduled.replace(true)) {
        platform::defer_on_main(Box::new(flush_host_events));
    }
}
fn flush_host_events() {
    FLUSH_SCHEDULED.with(|scheduled| scheduled.set(false));
    let events = HOST_EVENTS.with(|events| std::mem::take(&mut *events.borrow_mut()));
    if let Ok((host, files)) = with(|w| {
        let mut files = Vec::new();
        for (key, event) in events.events {
            if let Some(file) = w.apply_host_event(key, event)? {
                files.push(file);
            }
        }
        w.refresh_recent();
        Ok((w.host.clone(), files))
    }) {
        // Neither the host-event queue nor the workspace may be borrowed while dispatching.
        for (origin, uri) in files {
            host.prepare_file(origin, uri);
        }
    }
}
fn finish_file_link(origin: FileOrigin, target: Result<FileTarget, String>) {
    // Session identity, not current selection, determines both the cwd and sidebar anchor.
    // Main-thread execution makes this check and either dispatch atomic with close/quit.
    let host = with(|w| {
        Ok((w.file_link_origin(origin.key).as_ref() == Some(&origin)).then(|| w.host.clone()))
    });
    let Ok(Some(host)) = host else {
        return;
    };
    let result = match target {
        Ok(FileTarget::Neovim(path)) => begin_launch(
            "neovim".into(),
            origin.cwd,
            origin.project,
            None,
            Some(origin.item),
            None,
            Some(vec!["--".into(), path]),
        )
        .map(|_| ()),
        Ok(FileTarget::System(path)) => {
            host.open_file(path);
            Ok(())
        }
        Ok(FileTarget::Reveal(path)) => {
            host.reveal_file(path);
            Ok(())
        }
        Err(error) => Err(error),
    };
    if let Err(error) = result {
        action_error(error);
    }
}
fn finish_system_file(result: Result<(), String>) {
    if let Err(error) = result {
        action_error(error);
    }
}
/// Argv for a launch: configured args, then file-open extras or the tool's resume args.
fn launch_args(
    spec: &ToolSpec,
    conversation: Option<&Conversation>,
    cwd: &str,
    extra: Option<Vec<String>>,
) -> Vec<String> {
    let mut args = launch::substitute(
        &spec.args,
        conversation.map_or("", |c| c.id.as_str()),
        &conversation.map_or_else(String::new, |c| c.path.to_string_lossy().into_owned()),
        cwd,
    );
    if let Some(extra) = extra {
        // File-open arguments are already user data, not configuration templates.
        args.extend(extra);
    } else if let Some(c) = conversation {
        args.extend(launch::substitute(
            spec.resume.as_ref().unwrap(),
            &c.id,
            &c.path.to_string_lossy(),
            cwd,
        ));
    }
    args
}
/// Desktop notification body for an unpresented item that changed activity, if enabled.
fn activity_notice(activity: Activity, settings: &Settings) -> Option<&'static str> {
    match activity {
        Activity::YourTurn => settings.notify_turn_finished.then_some("Your turn"),
        Activity::NeedsYou(_) => settings.notify_needs_you.then_some("Needs you"),
        _ => None,
    }
}
/// Lifecycle fields whose change republishes the workspace.
type Observed = (Process, Activity, Option<Attention>, Option<u32>, bool);
pub fn bind_conversation(item: &str, session: &str, c: Conversation) {
    let _ = with(|w| {
        let Some(r) = w.records.iter_mut().find(|r| {
            r.state.id == item && r.omp.as_ref().and_then(|o| o.session.as_deref()) == Some(session)
        }) else {
            return Ok(());
        };
        r.state.conversation = Some(ConversationRef {
            tool: r.state.tool.clone(),
            id: c.id,
            path: c.path.to_string_lossy().into(),
        });
        w.discovery_revision += 1;
        r.conversation_title = Some(c.title);
        r.state.title = r.title();
        w.changed = true;
        Ok(())
    });
}
/// A message from an OMP item's status link (main thread).
pub fn omp_link(delivery: Delivery) {
    let _ = with(|w| w.apply_link(delivery));
}
/// Native mapping of the link's turn state, the only place that interprets it.
fn link_activity(state: &omp_link::State) -> Activity {
    match state {
        omp_link::State::Working => Activity::Working,
        omp_link::State::Waiting { .. } => Activity::OnJob,
        omp_link::State::NeedsYou { kind } => Activity::NeedsYou(*kind),
        omp_link::State::Done | omp_link::State::Idle => Activity::YourTurn,
        omp_link::State::Error => Activity::Error,
    }
}
pub fn key_action(key: &embed::AppKey) -> Option<Action> {
    let named = key.named.map(|named| match named {
        embed::NamedKey::PageUp => Key::PageUp,
        embed::NamedKey::PageDown => Key::PageDown,
        embed::NamedKey::Up => Key::Up,
        embed::NamedKey::Down => Key::Down,
        embed::NamedKey::Left => Key::Left,
        embed::NamedKey::Right => Key::Right,
        embed::NamedKey::Home => Key::Home,
        embed::NamedKey::End => Key::End,
    });
    let physical = if cfg!(target_os = "macos") {
        Key::from_mac_keycode(key.code)
    } else {
        Key::from_evdev(key.code)
    };
    let keycode = named.or(physical)?;
    with(|w| {
        Ok(w.keys.lookup(
            Mods {
                ctrl: key.ctrl,
                shift: key.shift,
                alt: key.alt,
                logo: key.logo,
            },
            keycode,
        ))
    })
    .ok()
    .flatten()
}
pub fn dispatch(action: Action) -> Result<(), String> {
    use Action::*;
    match action {
        Hide => {
            let app = native()?.app.clone();
            crate::lifecycle::hide_app(&app)
        }
        Quit => {
            let app = native()?.app.clone();
            crate::lifecycle::request_quit(&app);
            Ok(())
        }
        NewAgent | NewTerminal | NewNeovim => {
            let (tool, cwd, anchor) = with(|w| Ok(w.new_item_target(action)))?;
            tauri::async_runtime::spawn(async move {
                if let Err(e) = crate::commands::launch_item(tool, cwd, anchor, None).await {
                    platform::defer_on_main(Box::new(move || action_error(e)));
                }
            });
            Ok(())
        }
        Close => {
            let target = with(|w| {
                Ok(w.active.clone().map(|id| { let confirmed = matches!(&w.sheet, Some(Sheet::ConfirmClose { item_id, .. }) if item_id == &id); (id, confirmed) }))
            })?;
            if let Some((id, confirmed)) = target {
                close(&id, confirmed)?;
            }
            Ok(())
        }
        FocusSidebar => {
            let host = with(|w| {
                w.applied = None;
                Ok(w.host.clone())
            })?;
            host.focus_sidebar();
            Ok(())
        }
        ToggleSidebar => {
            let ui = with(|w| {
                w.ui.sidebar_expanded = !w.ui.sidebar_expanded;
                w.changed = true;
                Ok(w.ui.clone())
            })?;
            persist_ui(ui);
            Ok(())
        }
        Palette | PaletteProject | Settings | Shortcuts => with(|w| {
            let before = w.presented();
            w.sheet = Some(match action {
                Settings => Sheet::Settings,
                Shortcuts => Sheet::Shortcuts,
                _ => Sheet::Palette {
                    project_id: if action == PaletteProject {
                        w.active
                            .as_ref()
                            .and_then(|id| w.records.iter().find(|r| &r.state.id == id))
                            .map(|r| r.state.project_id.clone())
                    } else {
                        None
                    },
                    mode: PaletteMode::Open,
                },
            });
            w.present(before);
            Ok(())
        }),
        JumpWaiting => jump_waiting().map(|_| ()),
        Item(_) | ItemPrevious | ItemNext | ProjectPrevious | ProjectNext => with(|w| {
            let projects = w.projects();
            let order = visible_order(&projects, &w.ui.folded_projects);
            let target = match action {
                Item(number) => number
                    .checked_sub(1)
                    .and_then(|n| order.get(n as usize))
                    .cloned(),
                ItemPrevious | ItemNext => cycle(&order, w.active.as_deref(), action == ItemNext),
                _ => {
                    let ids: Vec<_> = projects
                        .iter()
                        .filter(|p| !p.item_ids.is_empty())
                        .map(|p| p.id.clone())
                        .collect();
                    let current = w
                        .active
                        .as_ref()
                        .and_then(|id| w.records.iter().find(|r| &r.state.id == id))
                        .map(|r| r.state.project_id.as_str());
                    cycle(&ids, current, action == ProjectNext).and_then(|project| {
                        projects
                            .iter()
                            .find(|p| p.id == project)
                            .and_then(|p| p.item_ids.first())
                            .cloned()
                    })
                }
            };
            if let Some(id) = target {
                w.select(&id)?;
            }
            Ok(())
        }),
    }
}
/// Select the next waiting item (needs-you first, then unseen turns, oldest first) and return
/// it; `None` when nothing waits. Repeated jumps cycle through what was waiting.
pub fn jump_waiting() -> Result<Option<String>, String> {
    with(|w| {
        let (order, next) = w.waiting_order();
        if let Some(id) = &next {
            w.select(id)?;
            w.waiting_cycle = order;
            w.waiting_cursor = Some(id.clone());
        }
        Ok(next)
    })
}
/// The item [`jump_waiting`] would select now.
pub fn waiting_next() -> Result<Option<String>, String> {
    with(|w| Ok(w.waiting_order().1))
}
pub fn persist_ui(ui: UiStateFile) {
    let (tx, rx) = std::sync::mpsc::channel();
    if runtime::send(Request::Save(ui, tx)).is_ok() {
        std::thread::spawn(move || {
            if let Err(e) = runtime::wait_reply(rx) {
                platform::defer_on_main(Box::new(move || action_error(e)));
            }
        });
    }
}
impl Record {
    fn title(&self) -> String {
        derived_title(
            self.spec.status,
            self.omp.as_ref().and_then(|o| o.name.as_deref()),
            &self.raw_title,
            self.conversation_title.as_deref(),
            &self.spec.label,
        )
    }
    fn observed(&self) -> Observed {
        (
            self.state.process,
            self.state.activity,
            self.state.attention,
            self.state.pid,
            self.state.unread,
        )
    }
    /// Returns whether the title source changed.
    fn apply_terminal_state(&mut self, state: TerminalState, discovery_revision: &mut u64) -> bool {
        // OMP items take their title from the status link, never from the terminal.
        let title_source_changed = self.omp.is_none() && self.raw_title != state.title;
        self.raw_title = state.title;
        self.state.pid = state.pid;
        if !state.running && self.state.process == Process::Running {
            self.state.process = Process::Exited;
            self.state.exit_code = state.exit_code;
            if self.state.conversation.is_some() {
                *discovery_revision += 1;
            }
        }
        title_source_changed
    }
    /// OMP is in session `session_id` now; returns whether that dropped the bound conversation.
    fn start_session(&mut self, session_id: &str, discovery_revision: &mut u64) -> bool {
        let reset = self
            .state
            .conversation
            .as_ref()
            .is_some_and(|c| c.id != session_id);
        if reset {
            self.state.conversation = None;
            self.conversation_title = None;
            *discovery_revision += 1;
        }
        let _ = runtime::send(Request::Bind(
            self.state.id.clone(),
            self.state.tool.clone(),
            session_id.to_owned(),
        ));
        reset
    }
    /// Returns whether the displayed title changed.
    fn refresh_title(&mut self) -> bool {
        let title = self.title();
        if title == self.state.title {
            return false;
        }
        self.state.title = title;
        true
    }
    /// Your turn without a finished turn to announce: OMP reported `idle` (an aborted turn, a
    /// new or switched session). It neither marks the item unread nor notifies.
    fn quiet(&self) -> bool {
        self.derived_activity() == Activity::YourTurn
            && matches!(
                self.omp.as_ref().and_then(|o| o.state.as_ref()),
                Some(omp_link::State::Idle)
            )
    }
    fn derived_activity(&self) -> Activity {
        if self.state.process == Process::Exited
            && self.state.kind == ToolKind::Agent
            && self.state.exit_code != Some(0)
        {
            Activity::Error
        } else {
            self.omp
                .as_ref()
                .and_then(|o| o.state.as_ref())
                .map_or(Activity::Unknown, link_activity)
        }
    }
    /// For an exited item that should leave the sidebar, whether its agent session ended.
    fn exit_removal(&self, remove_on_clean_exit: bool) -> Option<bool> {
        if self.state.process != Process::Exited {
            return None;
        }
        let clean = self.state.exit_code == Some(0);
        let agent = self.state.kind == ToolKind::Agent;
        let ended = agent && (self.closing || clean);
        (self.closing || clean && (agent || remove_on_clean_exit)).then_some(ended)
    }
}
/// An item's title. OMP items use their session name; other items their terminal title. Both
/// fall back to the resumed conversation's title, then the tool's label.
pub fn derived_title(
    status: StatusSource,
    omp_name: Option<&str>,
    raw: &str,
    conversation: Option<&str>,
    label: &str,
) -> String {
    let title = if status == StatusSource::Omp {
        omp_name.unwrap_or_default()
    } else {
        raw
    }
    .trim();
    if title.is_empty() {
        conversation
            .filter(|t| !t.trim().is_empty())
            .unwrap_or(label)
            .into()
    } else {
        title.into()
    }
}
impl Workspace {
    fn new(host: Rc<dyn Host>) -> Self {
        let settings = Settings::default();
        let tools = settings.tools.iter().map(ToolInfo::new).collect();
        Self {
            host,
            host_error: Some("The terminal host is still starting.".into()),
            default_cwd: launch::default_directory(),
            records: Vec::new(),
            active: None,
            recent: Vec::new(),
            sheet: None,
            project_order: Vec::new(),
            pinned_missing: HashMap::new(),
            next_item: 1,
            next_notice: 1,
            revision: 1,
            epoch: 1,
            last_sequence: None,
            applied: None,
            changed: false,
            settings,
            settings_state: None,
            keys: Keymap::defaults(runtime::PLATFORM),
            tools,
            terminal_config: runtime::BUNDLED.into(),
            ui: UiStateFile::default(),
            notices: Vec::new(),
            conversations: Vec::new(),
            store_errors: Vec::new(),
            discovery_revision: 1,
            scanning: true,
            waiting_cycle: Vec::new(),
            waiting_cursor: None,
            quitting: false,
        }
    }
    /// Bound conversations reopen their live item instead of launching a duplicate.
    fn reuse_bound(
        &mut self,
        tool: &str,
        conversation: Option<&Conversation>,
        replacement: Option<&str>,
    ) -> Result<Option<String>, String> {
        let Some(c) = conversation else {
            return Ok(None);
        };
        let Some(id) = self.bound_item(tool, &c.id, &c.path.to_string_lossy()) else {
            return Ok(None);
        };
        if let Some(old) = replacement {
            let index = self.index(old)?;
            self.ensure_restartable(index)?;
            self.remove(index, false)?;
        }
        self.select(&id)?;
        Ok(Some(id))
    }
    fn ensure_restartable(&self, index: usize) -> Result<(), String> {
        if matches!(
            self.records[index].state.process,
            Process::Exited | Process::Failed
        ) {
            Ok(())
        } else {
            Err("Only exited or failed items can restart.".into())
        }
    }
    /// Where a new row goes: the replaced row's slot, after a terminal's anchor, or last.
    fn launch_index(
        &mut self,
        replacement: Option<String>,
        kind: ToolKind,
        anchor: Option<&str>,
        project: &str,
    ) -> Result<usize, String> {
        if let Some(old) = replacement {
            return self.vacate(&old);
        }
        if kind != ToolKind::Terminal {
            return Ok(self.records.len());
        }
        Ok(anchor
            .and_then(|id| {
                self.records
                    .iter()
                    .position(|r| r.state.id == id && r.state.project_id == project)
            })
            .map(|i| i + 1)
            .unwrap_or(self.records.len()))
    }
    /// Drops an exited/failed row being restarted and returns its index.
    fn vacate(&mut self, old: &str) -> Result<usize, String> {
        let position = self.index(old)?;
        self.ensure_restartable(position)?;
        if let Some(key) = self.records[position].key {
            self.host.remove(key)?;
        }
        if self.records[position].state.conversation.is_some() {
            self.discovery_revision += 1;
        }
        self.records.remove(position);
        let _ = runtime::send(Request::Forget(old.to_owned()));
        self.recent.retain(|i| i != old);
        Ok(position)
    }
    fn file_link_origin(&self, key: SessionKey) -> Option<FileOrigin> {
        if self.quitting {
            return None;
        }
        self.records
            .iter()
            .find(|r| r.key == Some(key) && !r.closing)
            .map(|r| FileOrigin {
                key,
                item: r.state.id.clone(),
                cwd: r.state.cwd.clone(),
                project: r.state.project_id.clone(),
            })
    }
    fn apply_host_event(
        &mut self,
        key: SessionKey,
        event: HostUpdate,
    ) -> Result<Option<(FileOrigin, String)>, String> {
        let state = match event {
            HostUpdate::OpenFile(uri) => {
                return Ok(self.file_link_origin(key).map(|origin| (origin, uri)));
            }
            HostUpdate::State(state) => Some(state),
            HostUpdate::Bell => None,
        };
        let Some(index) = self.records.iter().position(|r| r.key == Some(key)) else {
            return Ok(None);
        };
        let presented = self.is_presented(index);
        let record = &mut self.records[index];
        let before = record.observed();
        let was_quiet = record.quiet();
        let title_source_changed = state
            .is_some_and(|state| record.apply_terminal_state(state, &mut self.discovery_revision));
        let title_changed = title_source_changed && record.refresh_title();
        if record.omp.is_some() && record.state.process != before.0 {
            // The process ended: its token must not report for it any more.
            self.host.unlink(&record.state.id);
        }
        self.settle(index, presented, (before, was_quiet), title_changed)?;
        Ok(None)
    }
    /// Applies a status-link message to its running OMP item. Messages from a connection older
    /// than the newest one heard from were superseded and are dropped.
    fn apply_link(&mut self, delivery: Delivery) -> Result<(), String> {
        let Some(index) = self
            .records
            .iter()
            .position(|r| r.state.id == delivery.item && r.state.process == Process::Running)
        else {
            return Ok(());
        };
        let presented = self.is_presented(index);
        let record = &mut self.records[index];
        let before = record.observed();
        let was_quiet = record.quiet();
        let Some(link) = record.omp.as_mut() else {
            return Ok(());
        };
        if delivery.connection < link.connection {
            return Ok(());
        }
        link.connection = delivery.connection;
        let changed = match delivery.message {
            Message::Session { id } => {
                if link.session.as_deref() == Some(id.as_str()) {
                    false
                } else {
                    link.session = Some(id.clone());
                    let reset = record.start_session(&id, &mut self.discovery_revision);
                    if reset {
                        record.refresh_title();
                    }
                    reset
                }
            }
            Message::State(mut state) => {
                // The job list moves to the item; the link keeps only the state.
                let jobs = match &mut state {
                    omp_link::State::Waiting { jobs } => std::mem::take(jobs),
                    _ => Vec::new(),
                };
                let jobs: Vec<BackgroundJob> = jobs.into_iter().map(Into::into).collect();
                let changed = record.state.background_jobs != jobs;
                record.state.background_jobs = jobs;
                link.state = Some(state);
                changed
            }
            Message::Model { model } => {
                link.model_reported = true;
                let changed = record.state.model != model;
                record.state.model = model;
                changed
            }
            Message::Name { name } => {
                link.name = name;
                record.refresh_title()
            }
        };
        self.settle(index, presented, (before, was_quiet), changed)
    }
    /// Re-derives an updated item's activity: unread and desktop notification for an
    /// unpresented item, republication, and removal of a finished row. `before` is the item's
    /// [`Record::observed`] and [`Record::quiet`] state ahead of the update.
    fn settle(
        &mut self,
        index: usize,
        presented: bool,
        (before, was_quiet): (Observed, bool),
        changed: bool,
    ) -> Result<(), String> {
        let record = &mut self.records[index];
        let activity = record.derived_activity();
        let quiet = record.quiet();
        // A quiet your-turn is announced once it becomes a finished turn.
        let entered = activity != before.1 || was_quiet && !quiet;
        if entered || record.state.process != before.0 {
            record.state.state_since = now();
        }
        record.state.activity = activity;
        record.state.attention = activity.attention();
        // Jobs show only while the agent waits on them (e.g. not on an exited row).
        if activity != Activity::OnJob && !record.state.background_jobs.is_empty() {
            record.state.background_jobs.clear();
            self.changed = true;
        }
        if entered && !quiet && !presented {
            if activity == Activity::YourTurn {
                record.state.unread = true;
            }
            if let Some(body) = activity_notice(activity, &self.settings) {
                self.host
                    .notify(&record.state.id, &record.state.title, body);
            }
        }
        self.changed |= changed || before != record.observed();
        if let Some(ended) = record.exit_removal(self.settings.remove_on_clean_exit) {
            self.remove(index, ended)?;
        }
        Ok(())
    }
    fn index(&self, id: &str) -> Result<usize, String> {
        self.records
            .iter()
            .position(|r| r.state.id == id)
            .ok_or_else(|| format!("Item {id} does not exist."))
    }
    fn presented(&self) -> Option<SessionKey> {
        if self.sheet.is_none() {
            self.active
                .as_ref()
                .and_then(|id| self.records.iter().find(|r| &r.state.id == id))
                .and_then(|r| r.key)
        } else {
            None
        }
    }
    fn present(&mut self, before: Option<SessionKey>) {
        self.epoch += 1;
        self.applied = None;
        self.changed = true;
        if let Some(key) = before {
            self.host.hide(key);
        }
    }
    /// The waiting cycle and the item a jump selects next. A repeated jump keeps the your-turn
    /// entries it already visited but drops anything that resumed working or exited, and
    /// re-sorts newly arriving approvals.
    fn waiting_order(&self) -> (Vec<String>, Option<String>) {
        let remembered = if self.waiting_cursor.as_deref() == self.active.as_deref() {
            self.waiting_cycle.as_slice()
        } else {
            &[]
        };
        let order = waiting_cycle_order(self.records.iter().map(|r| &r.state), remembered);
        let next = cycle(&order, self.waiting_cursor.as_deref(), true);
        (order, next)
    }
    fn select(&mut self, id: &str) -> Result<(), String> {
        let index = self.index(id)?;
        let before = self.presented();
        self.active = Some(id.into());
        self.sheet = None;
        self.waiting_cursor = None;
        self.waiting_cycle.clear();
        self.records[index].state.unread = false;
        self.recent.retain(|i| i != id);
        self.recent.insert(0, id.into());
        self.refresh_recent();
        self.present(before);
        Ok(())
    }
    fn refresh_recent(&mut self) {
        // Starting selections retain their MRU position internally. Wire snapshots and
        // fallback eligibility exclude them until the process actually exists.
        retain_recent(&mut self.recent, self.records.iter().map(|r| &r.state));
        if let Some(id) = &self.active {
            if self
                .records
                .iter()
                .any(|r| &r.state.id == id && r.state.process != Process::Failed)
                && !self.recent.contains(id)
            {
                self.recent.insert(0, id.clone());
            }
        }
    }
    /// "Seen" for attention purposes: the item's terminal is on screen in the focused window.
    /// Hyprland keeps windows on other workspaces (or behind a focused tile) mapped and
    /// visible to GTK, so visibility alone would swallow every unseen turn.
    fn is_presented(&self, index: usize) -> bool {
        self.sheet.is_none()
            && self.active.as_deref() == Some(self.records[index].state.id.as_str())
            && self.host.window_presentable()
            && self.records[index]
                .key
                .is_some_and(|key| self.host.is_shown(key))
            && self.host.window_focused()
    }
    /// Tool, directory and sidebar anchor of a new-item shortcut: agents start in the active
    /// item's project with its default tool, terminals and Neovim in its directory.
    fn new_item_target(&self, action: Action) -> (String, String, Option<String>) {
        let active = self
            .active
            .as_ref()
            .and_then(|id| self.records.iter().find(|r| &r.state.id == id));
        let cwd = shortcut_cwd(action, active.map(|r| &r.state), &self.default_cwd);
        let tool = match action {
            Action::NewTerminal => "shell".into(),
            Action::NewNeovim => "neovim".into(),
            _ => active
                .map(|r| {
                    self.settings
                        .project_default_tool(Path::new(&r.state.project_id))
                        .to_owned()
                })
                .unwrap_or_else(|| self.settings.default_tool.clone()),
        };
        (tool, cwd, active.map(|r| r.state.id.clone()))
    }
    /// Publishes the executable of the shell running in this row, not its command-derived
    /// title or a possibly different login-shell PATH probe (shown by the close confirmation).
    fn publish_shell_executable(&mut self, index: usize) {
        let record = &self.records[index];
        let Some(state) = record.key.and_then(|key| self.host.state(key)) else {
            return;
        };
        if let Some(tool) = self.tools.iter_mut().find(|t| t.id == record.state.tool) {
            tool.path = Some(state.shell);
        }
    }
    fn clear_presented_unread(&mut self) {
        if let Some(index) = self
            .active
            .as_ref()
            .and_then(|id| self.records.iter().position(|r| &r.state.id == id))
        {
            let presented = self.is_presented(index);
            if acknowledge_unread(&mut self.records[index].state, presented) {
                self.changed = true;
            }
        }
    }
    fn remove(&mut self, index: usize, ended: bool) -> Result<(), String> {
        let before = self.presented();
        let record = &self.records[index];
        if let Some(key) = record.key {
            self.host.remove(key)?;
        }
        let id = record.state.id.clone();
        let active = self.active.as_deref() == Some(id.as_str());
        if ended && record.state.conversation.is_some() {
            self.ended_notice(index);
        }
        if self.records[index].state.conversation.is_some() {
            self.discovery_revision += 1;
        }
        if self.records.remove(index).omp.is_some() {
            self.host.unlink(&id);
        }
        let _ = runtime::send(Request::Forget(id.clone()));
        self.recent.retain(|i| i != &id);
        self.project_order
            .retain(|p| self.records.iter().any(|r| &r.state.project_id == p));
        let closed_sheet =
            matches!(&self.sheet, Some(Sheet::ConfirmClose { item_id, .. }) if item_id == &id);
        if closed_sheet {
            self.sheet = None;
        }
        if let Some(Sheet::Quit { item_ids }) = &mut self.sheet {
            item_ids.retain(|i| i != &id);
        }
        if active {
            self.active = selection_fallback(
                &self.recent,
                &self
                    .records
                    .iter()
                    .filter(|r| matches!(r.state.process, Process::Running | Process::Exited))
                    .map(|r| r.state.id.clone())
                    .collect::<Vec<_>>(),
            );
        }
        if active || closed_sheet {
            self.present(before);
        }
        self.changed = true;
        Ok(())
    }
    fn notice(
        &mut self,
        kind: &'static str,
        title: String,
        body: String,
        project: Option<String>,
        conversation: Option<ConversationRef>,
    ) {
        self.notices.push(Notice {
            id: format!("notice-{}", self.next_notice),
            kind,
            title,
            body,
            project_id: project,
            conversation,
            at: now(),
        });
        self.next_notice += 1;
        if self.notices.len() > 5 {
            self.notices.remove(0);
        }
        self.changed = true;
    }
    fn error_notice(&mut self, message: String) {
        self.notice("error", "Action failed".into(), message, None, None);
    }
    fn ended_notice(&mut self, index: usize) {
        let r = &self.records[index];
        self.notice(
            "ended",
            format!("“{}” ended", r.state.title),
            format!(
                "Conversation saved in {}.",
                self.settings.project_name(Path::new(&r.state.project_id))
            ),
            Some(r.state.project_id.clone()),
            r.state.conversation.clone(),
        );
    }
    fn bound_item(&self, tool: &str, id: &str, path: &str) -> Option<String> {
        find_bound_item(self.records.iter().map(|r| &r.state), tool, id, path)
    }
    fn projects(&self) -> Vec<ProjectState> {
        project_order(
            &self
                .settings
                .pinned
                .iter()
                .map(|p| p.to_string_lossy().into_owned())
                .collect::<Vec<_>>(),
            &self.project_order,
        )
        .into_iter()
        .filter_map(|id| {
            let item_ids: Vec<_> = self
                .records
                .iter()
                .filter(|r| r.state.project_id == id)
                .map(|r| r.state.id.clone())
                .collect();
            let path = Path::new(&id);
            let pinned = self.settings.pinned.iter().any(|p| p == path);
            if !pinned && item_ids.is_empty() {
                return None;
            }
            Some(ProjectState {
                name: self.settings.project_name(path),
                path: id.clone(),
                pinned,
                missing: pinned && self.pinned_missing.get(&id).copied().unwrap_or(false),
                default_tool: self.settings.project_default_tool(path).into(),
                id,
                item_ids,
            })
        })
        .collect()
    }
    fn snapshot(&self) -> WorkspaceState {
        let last_set = self
            .ui
            .last_set
            .iter()
            .map(|e| LastSetEntry {
                tool: e.tool.clone(),
                title: e.title.clone(),
                project_id: e.project_id.clone(),
                cwd: e.cwd.clone(),
                conversation: ConversationRef {
                    tool: e.tool.clone(),
                    id: e.conversation_id.clone(),
                    path: e.conversation_path.clone(),
                },
            })
            .collect::<Vec<_>>();
        WorkspaceState {
            revision: self.revision,
            presentation_epoch: self.epoch,
            view: if self.sheet.is_some() {
                "sheet"
            } else {
                "terminal"
            },
            sheet: self.sheet.clone(),
            projects: self.projects(),
            items: self.records.iter().map(|r| r.state.clone()).collect(),
            active_item_id: self.active.clone(),
            recent_item_ids: recent_running(&self.recent, self.records.iter().map(|r| &r.state)),
            tools: self.tools.clone(),
            provider_icons: self.settings.provider_icons.clone(),
            notices: self.notices.clone(),
            last_set: (!last_set.is_empty()).then_some(last_set),
            ui: UiState {
                sidebar_expanded: self.ui.sidebar_expanded,
                folded_project_ids: self.ui.folded_projects.clone(),
            },
            shortcuts: self
                .keys
                .all()
                .into_iter()
                .map(|(action, label)| Shortcut { action, label })
                .collect(),
            default_cwd: self.default_cwd.clone(),
            host_error: self.host_error.clone(),
        }
    }
}
fn recent_running<'a>(
    recent: &[String],
    items: impl Iterator<Item = &'a ItemState> + Clone,
) -> Vec<String> {
    recent
        .iter()
        .filter(|id| {
            items
                .clone()
                .any(|i| &i.id == *id && matches!(i.process, Process::Running | Process::Exited))
        })
        .cloned()
        .collect()
}
fn shortcut_cwd(action: Action, active: Option<&ItemState>, default: &str) -> String {
    active
        .map(|item| {
            if action == Action::NewAgent {
                &item.project_id
            } else {
                &item.cwd
            }
        })
        .map_or_else(|| default.to_owned(), Clone::clone)
}
fn running_binding(item: &ItemState) -> Option<&ConversationRef> {
    matches!(item.process, Process::Starting | Process::Running)
        .then_some(item.conversation.as_ref())
        .flatten()
}
fn find_bound_item<'a>(
    mut items: impl Iterator<Item = &'a ItemState>,
    tool: &str,
    id: &str,
    path: &str,
) -> Option<String> {
    items
        .find(|item| {
            running_binding(item).is_some_and(|c| c.tool == tool && c.id == id && c.path == path)
        })
        .map(|item| item.id.clone())
}
fn retain_recent<'a>(recent: &mut Vec<String>, items: impl Iterator<Item = &'a ItemState> + Clone) {
    recent.retain(|id| {
        items
            .clone()
            .any(|i| &i.id == id && i.process != Process::Failed)
    });
}
fn acknowledge_unread(item: &mut ItemState, presented: bool) -> bool {
    let changed = presented && item.unread;
    if changed {
        item.unread = false;
    }
    changed
}
pub fn selection_fallback(recent: &[String], existing: &[String]) -> Option<String> {
    recent.iter().find(|id| existing.contains(id)).cloned()
}
pub fn project_order(pinned: &[String], first_running: &[String]) -> Vec<String> {
    let mut order = Vec::new();
    for id in pinned.iter().chain(first_running) {
        if !order.contains(id) {
            order.push(id.clone());
        }
    }
    order
}
pub fn visible_order(projects: &[ProjectState], folded: &[String]) -> Vec<String> {
    projects
        .iter()
        .filter(|p| !folded.contains(&p.id))
        .flat_map(|p| p.item_ids.iter().cloned())
        .collect()
}
fn waiting_cycle_order<'a>(
    items: impl Iterator<Item = &'a ItemState>,
    remembered: &[String],
) -> Vec<String> {
    let mut items: Vec<_> = items
        .filter(|i| {
            i.process == Process::Running
                && (matches!(i.activity, Activity::NeedsYou(_))
                    || i.activity == Activity::YourTurn && (i.unread || remembered.contains(&i.id)))
        })
        .collect();
    items.sort_by_key(|i| {
        (
            !matches!(i.activity, Activity::NeedsYou(_)),
            i.state_since,
            i.started_at,
        )
    });
    items.into_iter().map(|i| i.id.clone()).collect()
}
fn cycle(order: &[String], current: Option<&str>, forward: bool) -> Option<String> {
    if order.is_empty() {
        return None;
    }
    let index = current.and_then(|id| order.iter().position(|i| i == id));
    let next = match index {
        Some(i) if forward => (i + 1) % order.len(),
        Some(i) => (i + order.len() - 1) % order.len(),
        None if forward => 0,
        None => order.len() - 1,
    };
    Some(order[next].clone())
}

/// Headless workspace for tests: a [`FakeHost`] stands in for the app window and the embedded
/// terminal host, emulating the embed contract (sessions must stop before removal; only shown
/// sessions take focus) and recording every effect.
#[cfg(test)]
pub(crate) mod test_support;
#[cfg(test)]
mod tests;
